//! Compact projections of the existing pricing engine. Each envelope is
//! simulated independently, under one snapshot, deadline and worker permit.
use super::{
    Arc, AtomicBool, BlockReader, Budget, Composer, FeeQuoteError, Header, L2FeeRequest,
    MAX_SOURCE_GAS, PreparedL2FeeQuote, StateProviderFactory, Transaction, U256, Value, json,
    unavailable, unix_now,
};
use crate::composer::estimate::EstimateError;

#[derive(Clone, Debug)]
pub struct CompactFeeRequest {
    transaction: Value,
    types: Vec<u8>,
}

impl CompactFeeRequest {
    /// Fee fields and unsupported transaction extensions are rejected. Defaults
    /// resolve only missing fields; explicit null or malformed values are errors.
    pub fn parse(transaction: &Value, options: &Value) -> Result<Self, FeeQuoteError> {
        let options = options
            .as_object()
            .ok_or(FeeQuoteError::Invalid("expected options object"))?;
        if options.keys().any(|key| key != "transactionTypes") {
            return Err(FeeQuoteError::Invalid("unknown quote option"));
        }
        let types = match options.get("transactionTypes") {
            None => match transaction.get("type") {
                Some(value) => vec![tx_type(value)?],
                None => vec![0, 2],
            },
            Some(value) => {
                let types = value
                    .as_array()
                    .filter(|v| !v.is_empty() && v.len() <= 2)
                    .ok_or(FeeQuoteError::Invalid(
                        "transactionTypes must contain one or two types",
                    ))?
                    .iter()
                    .map(tx_type)
                    .collect::<Result<Vec<_>, _>>()?;
                if types.len() == 2 && types[0] == types[1] {
                    return Err(FeeQuoteError::Invalid("duplicate transaction type"));
                }
                if let Some(value) = transaction.get("type")
                    && types != [tx_type(value)?]
                {
                    return Err(FeeQuoteError::Invalid(
                        "type conflicts with transactionTypes",
                    ));
                }
                types
            }
        };
        let request = Self {
            transaction: transaction.clone(),
            types,
        };
        // Reuse the established strict parser and limits before acquiring work.
        request.resolve(1, 0, MAX_SOURCE_GAS, request.types[0])?;
        Ok(request)
    }

    fn resolve(
        &self,
        chain: u64,
        nonce: u64,
        gas: u64,
        ty: u8,
    ) -> Result<L2FeeRequest, FeeQuoteError> {
        let mut value = self.transaction.clone();
        let fields = value
            .as_object_mut()
            .ok_or(FeeQuoteError::Invalid("expected transaction object"))?;
        for (key, default) in [
            ("chainId", json!(format!("{chain:#x}"))),
            ("nonce", json!(format!("{nonce:#x}"))),
            ("gas", json!(format!("{gas:#x}"))),
            ("value", json!("0x0")),
            ("data", json!("0x")),
        ] {
            fields.entry(key).or_insert(default);
        }
        fields.insert("type".into(), json!(format!("{ty:#x}")));
        L2FeeRequest::parse(&value)
    }
}

fn tx_type(value: &Value) -> Result<u8, FeeQuoteError> {
    match value.as_str() {
        Some("0x0") => Ok(0),
        Some("0x2") => Ok(2),
        _ => Err(FeeQuoteError::Invalid(
            "transaction type must be 0x0 or 0x2",
        )),
    }
}

#[derive(Debug)]
pub struct PreparedCompactFeeQuote {
    request: CompactFeeRequest,
    prepared: PreparedL2FeeQuote,
}

pub(super) fn estimate_error(error: EstimateError) -> FeeQuoteError {
    tracing::debug!(%error, "compact quote estimation failed");
    match error {
        EstimateError::ContextChanged => FeeQuoteError::Stale,
        EstimateError::SettlementPending => {
            FeeQuoteError::Unavailable("settlement reconciliation pending")
        }
        EstimateError::Timeout => FeeQuoteError::Limit("gas estimation deadline"),
        EstimateError::Invalid(_) => FeeQuoteError::Invalid("gas estimation input"),
        EstimateError::InsufficientFunds(balance) => FeeQuoteError::InsufficientFunds(balance),
        EstimateError::Unsupported(_) | EstimateError::Revert(_) | EstimateError::Effects(_) => {
            FeeQuoteError::Unsupported("gas estimation or final effect validation failed")
        }
        EstimateError::Unavailable(_) => FeeQuoteError::Unavailable("gas estimation"),
    }
}

impl<L2> Composer<L2>
where
    L2: BlockReader<Block = eez_primitives::Block, Header = Header>
        + StateProviderFactory
        + Send
        + Sync
        + 'static,
{
    pub async fn prepare_compact_fee_quote(
        &self,
        rollup_id: u64,
        direction: crate::Direction,
        request: CompactFeeRequest,
    ) -> Result<PreparedCompactFeeQuote, FeeQuoteError> {
        let transaction = request.resolve(
            match direction {
                crate::Direction::Inbound => self.inner.posting.ctx.l1_chain_id,
                crate::Direction::Outbound => self.inner.posting.ctx.l2_chain_id,
            },
            0,
            MAX_SOURCE_GAS,
            request.types[0],
        )?;
        let prepared = self
            .prepare_fee_quote(direction, rollup_id, transaction)
            .await?;
        Ok(PreparedCompactFeeQuote { request, prepared })
    }

    /// Blocking, bounded work. Both projections execute exactly this path; debug
    /// adds diagnostics after calculation, never a different pricing algorithm.
    pub fn quote_fee(
        &self,
        input: PreparedCompactFeeQuote,
        cancelled: Arc<AtomicBool>,
        debug: bool,
    ) -> Result<Value, FeeQuoteError> {
        let PreparedCompactFeeQuote {
            request,
            mut prepared,
        } = input;
        let budget = Arc::new(Budget::new(cancelled));
        budget.check()?;
        let estimator = self
            .gas_estimator(prepared.rollup_id)
            .map_err(estimate_error)?;
        let mut snapshot = self.fee_snapshot(&prepared, &budget)?;
        let parent = snapshot.l2_parent.clone();
        let result = (|| {
            let direction = prepared.direction;
            let chain = match direction {
                crate::Direction::Inbound => self.inner.posting.ctx.l1_chain_id,
                crate::Direction::Outbound => self.inner.posting.ctx.l2_chain_id,
            };
            let explicit_nonce = request
                .transaction
                .get("nonce")
                .map(|_| prepared.request.tx.nonce());
            let (nonce, predecessors) =
                self.replay_quote_predecessors(&prepared, &mut snapshot, &budget, explicit_nonce)?;
            let rollup_id = prepared.rollup_id;
            let rollup = &self.inner.rollups[&rollup_id];
            let sender = prepared.request.sender;
            let check_nonce = || {
                if rollup.held_pool.owns_nonce(sender, direction, nonce) {
                    Err(FeeQuoteError::Invalid("source nonce is already pending"))
                } else {
                    self.validate_quote_predecessors(rollup_id, &predecessors)
                }
            };
            check_nonce()?;
            let issued = unix_now()?;
            let expiry = prepared.expires_at;
            let mut options = Vec::new();
            let mut diagnostics = Vec::new();
            for &ty in &request.types {
                budget.check()?;
                let result = (|| {
                    let mut transaction = request.resolve(chain, nonce, MAX_SOURCE_GAS, ty)?;
                    if request.transaction.get("gas").is_none() {
                        let fields = transaction.fields(0, 0);
                        let gas = estimator
                            .estimate_on_snapshot(
                                direction,
                                serde_json::from_value(fields)
                                    .map_err(|e| unavailable("estimation transaction", e))?,
                                budget.deadline,
                                &snapshot,
                            )
                            .map_err(estimate_error)?;
                        budget.check()?;
                        transaction = request.resolve(chain, nonce, gas, ty)?;
                    }
                    prepared.request = transaction;
                    let mut detail = self.quote_in_context(&prepared, &snapshot, &budget)?;
                    budget.check()?;
                    let tx = &detail["transaction"];
                    let cap = price(tx, if ty == 0 { "gasPrice" } else { "maxFeePerGas" })?;
                    let tip = if ty == 0 {
                        cap
                    } else {
                        price(tx, "maxPriorityFeePerGas")?
                    };
                    let validated_gas = if direction == crate::Direction::Outbound {
                        estimator
                            .validate_quote(
                                &snapshot,
                                prepared.request.env(tip, cap),
                                budget.deadline,
                            )
                            .map_err(estimate_error)?
                    } else {
                        detail["estimate"]["sourceGasUsed"]
                            .as_u64()
                            .ok_or(FeeQuoteError::Unavailable("L1 receipt gas"))?
                    };
                    if detail["estimate"]["sourceGasUsed"].as_u64() != Some(validated_gas) {
                        return Err(FeeQuoteError::Unsupported(
                            "final native gas differs from priced execution",
                        ));
                    }
                    detail["issuedAt"] = json!(issued);
                    detail["expiresAt"] = json!(expiry);
                    compact_option(&detail).map(|option| (option, detail))
                })();
                match result {
                    Ok((option, detail)) => {
                        options.push(option);
                        diagnostics.push(detail);
                    }
                    // Execution can support one encoding but not the other. Never
                    // return a successful option after infrastructure/context failure.
                    Err(
                        error @ (FeeQuoteError::Unsupported(_)
                        | FeeQuoteError::Invalid(_)
                        | FeeQuoteError::InsufficientFunds(_)
                        | FeeQuoteError::Underfunded(_)
                        | FeeQuoteError::L1Underfunded(_)
                        | FeeQuoteError::SourceFeeCap { .. }),
                    ) => {
                        let mut failed = json!({"type":format!("{ty:#x}"), "error":{"code":error.code(), "message":error.to_string()}});
                        if let Some(details) = error.diagnostics() {
                            failed["error"]["details"] = details;
                        }
                        options.push(failed.clone());
                        diagnostics.push(failed);
                    }
                    Err(error) => return Err(error),
                }
            }
            budget.check()?;
            // Recheck canonical identity and bounded age after ALL variants.
            // Advancing heads do not invalidate an advisory simulation.
            check_nonce()?;
            if unix_now()? >= expiry {
                return Err(FeeQuoteError::Stale);
            }
            let mut response = json!({"version":1, "chainId":format!("{chain:#x}"), "nonce":format!("{nonce:#x}"),
            "expiresAt":format!("{expiry:#x}"), "options":options});
            if debug {
                response["diagnostics"] = json!({
                    "recoveryMode":if direction == crate::Direction::Inbound {"builder_refund"} else if self.enforces_l2_fee_tariff() {"enforced_tariff"} else {"advisory"},
                    "options":diagnostics
                });
            }
            Ok(response)
        })();
        // Also recheck failures: canonical source progress can make an old
        // prefix look invalid while this request is calculating its quote.
        self.validate_fee_context(&prepared, &parent)?;
        result
    }
}

fn price(tx: &Value, key: &str) -> Result<u128, FeeQuoteError> {
    tx[key]
        .as_str()
        .and_then(|v| v.strip_prefix("0x"))
        .and_then(|v| u128::from_str_radix(v, 16).ok())
        .ok_or(FeeQuoteError::Unavailable("internal quote price"))
}

fn compact_option(detail: &Value) -> Result<Value, FeeQuoteError> {
    let transaction = &detail["transaction"];
    let mut option = json!({"type":transaction["type"], "gas":transaction["gas"], "requestHash":detail["requestHash"]});
    let cap = if transaction["type"] == "0x0" {
        option["gasPrice"] = transaction["gasPrice"].clone();
        price(transaction, "gasPrice")?
    } else {
        option["maxPriorityFeePerGas"] = transaction["maxPriorityFeePerGas"].clone();
        option["maxFeePerGas"] = transaction["maxFeePerGas"].clone();
        price(transaction, "maxFeePerGas")?
    };
    let gas = detail["estimate"]["sourceGasUsed"]
        .as_u64()
        .ok_or(FeeQuoteError::Unavailable("internal quote gas"))?;
    // The unsigned engine uses cap = base + tip, so this is the effective
    // price. The caller computes the separate wallet reservation from the limit.
    option["estimatedFee"] = json!(format!("{:#x}", U256::from(cap) * U256::from(gas)));
    Ok(option)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Typed2718;
    use alloy_primitives::{Address, keccak256};

    fn transaction() -> Value {
        json!({"from":Address::repeat_byte(0x11), "to":Address::repeat_byte(0x22)})
    }

    #[test]
    fn omitted_fields_resolve_without_overwriting_explicit_transaction() {
        let request = CompactFeeRequest::parse(&transaction(), &json!({})).unwrap();
        assert_eq!(request.types, [0, 2]);
        for ty in request.types.iter().copied() {
            let resolved = request.resolve(906969, 7, 6_050_247, ty).unwrap();
            assert_eq!(resolved.tx.nonce(), 7);
            assert_eq!(resolved.tx.chain_id(), Some(906969));
            assert_eq!(resolved.tx.gas_limit(), 6_050_247);
            assert_eq!(resolved.tx.ty(), ty);
        }
        let mut explicit = transaction();
        explicit["nonce"] = json!("0x9");
        explicit["chainId"] = json!("0x27d9");
        explicit["gas"] = json!("0x1000000");
        explicit["type"] = json!("0x0");
        let request = CompactFeeRequest::parse(&explicit, &json!({})).unwrap();
        assert_eq!(request.types, [0]);
        let resolved = request.resolve(1, 0, 21_000, 0).unwrap();
        assert_eq!(resolved.tx.nonce(), 9);
        assert_eq!(resolved.tx.chain_id(), Some(10201));
        assert_eq!(resolved.tx.gas_limit(), 16_777_216);
    }

    #[test]
    fn compact_parser_rejects_ambiguous_and_unbounded_requests() {
        for options in [
            Value::Null,
            json!([]),
            json!({"gas":1}),
            json!({"transactionTypes":[]}),
            json!({"transactionTypes":["0x2","0x2"]}),
            json!({"transactionTypes":["0x0","0x2","0x0"]}),
            json!({"transactionTypes":["0x02"]}),
            json!({"transactionTypes":["0x1"]}),
        ] {
            assert!(
                CompactFeeRequest::parse(&transaction(), &options).is_err(),
                "{options}"
            );
        }
        for (key, value) in [
            ("nonce", json!("0x00")),
            ("gas", json!("0x1000001")),
            ("gas", Value::Null),
            ("from", Value::Null),
            ("to", Value::Null),
            ("gasPrice", json!("0x1")),
            ("accessList", json!([])),
        ] {
            let mut bad = transaction();
            bad[key] = value;
            assert!(CompactFeeRequest::parse(&bad, &json!({})).is_err(), "{bad}");
        }
        let mut typed = transaction();
        typed["type"] = json!("0x0");
        assert!(CompactFeeRequest::parse(&typed, &json!({"transactionTypes":["0x2"]})).is_err());
        assert!(
            CompactFeeRequest::parse(&typed, &json!({"transactionTypes":["0x0","0x2"]})).is_err()
        );
    }

    #[test]
    fn compact_projection_keeps_type_specific_fields_and_charged_gas() {
        let request = CompactFeeRequest::parse(&transaction(), &json!({})).unwrap();
        let mut hashes = Vec::new();
        for ty in [0, 2] {
            let request = request.resolve(906969, 1, 6_050_247, ty).unwrap();
            let tx = request.fields(5, 15);
            let mut transaction = tx.clone();
            for fee in ["gasPrice", "maxFeePerGas", "maxPriorityFeePerGas"] {
                transaction.as_object_mut().unwrap().remove(fee);
            }
            let detail = json!({"transaction":tx,"requestHash":keccak256(transaction.to_string()),
                "estimate":{"sourceGasUsed":100_000}, "policy":{"private":"debug only"}});
            let option = compact_option(&detail).unwrap();
            assert_eq!(option["estimatedFee"], "0x16e360");
            assert_eq!(option.get("gasPrice").is_some(), ty == 0);
            assert_eq!(option.get("maxFeePerGas").is_some(), ty == 2);
            assert!(option.get("policy").is_none());
            assert!(option.get("transaction").is_none());
            hashes.push(option["requestHash"].clone());
        }
        assert_ne!(hashes[0], hashes[1], "binding includes envelope type");
    }

    #[test]
    fn normalized_binding_matches_independent_sdk_vectors() {
        for (ty, expected) in [
            (
                0,
                "0x873184abbd3d864413844c1aad934f779432aba75064a5ed1c7060547c249712",
            ),
            (
                2,
                "0xb027810c300a8577d3d15e441e23fad18defd8cf8f1485f2c42dfcaa015b3c31",
            ),
        ] {
            let mut input = transaction();
            input["data"] = json!("0x1234");
            let request = CompactFeeRequest::parse(&input, &json!({}))
                .unwrap()
                .resolve(1, 0, 100_000, ty)
                .unwrap();
            let mut fields = request.fields(5, 15);
            for key in ["gasPrice", "maxFeePerGas", "maxPriorityFeePerGas"] {
                fields.as_object_mut().unwrap().remove(key);
            }
            assert_eq!(format!("{:#x}", keccak256(fields.to_string())), expected);
        }
    }
}
