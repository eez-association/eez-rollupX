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
    trace_calls: bool,
}

impl CompactFeeRequest {
    /// Fee fields and unsupported transaction extensions are rejected. Defaults
    /// resolve only missing fields; explicit null or malformed values are errors.
    pub fn parse(transaction: &Value, options: &Value) -> Result<Self, FeeQuoteError> {
        let options = options
            .as_object()
            .ok_or(FeeQuoteError::Invalid("expected options object"))?;
        if options
            .keys()
            .any(|key| key != "transactionTypes" && key != "trace")
        {
            return Err(FeeQuoteError::Invalid("unknown quote option"));
        }
        let trace_calls = match options.get("trace") {
            None => false,
            Some(Value::String(mode)) if mode == "calls" => true,
            Some(_) => return Err(FeeQuoteError::Invalid("trace must be \"calls\"")),
        };
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
            trace_calls,
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
    estimate_error_at("gas_estimation", error)
}

pub(super) fn estimate_error_at(stage: &'static str, error: EstimateError) -> FeeQuoteError {
    tracing::debug!(%error, stage, "compact quote estimation failed");
    match error {
        EstimateError::ContextChanged => FeeQuoteError::Stale,
        EstimateError::SettlementPending => {
            FeeQuoteError::Unavailable("settlement reconciliation pending")
        }
        EstimateError::Timeout => FeeQuoteError::Limit("gas estimation deadline"),
        EstimateError::Invalid(_) => FeeQuoteError::Invalid("gas estimation input"),
        EstimateError::InsufficientFunds(balance) => FeeQuoteError::InsufficientFunds(balance),
        EstimateError::PosterFunds(balance) => FeeQuoteError::PosterFunds(balance),
        EstimateError::Unsupported(_) | EstimateError::Revert(_) | EstimateError::Effects(_) => {
            FeeQuoteError::execution(
                "gas estimation or final effect validation failed",
                stage,
                error,
            )
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
        if request.trace_calls && !debug {
            return Err(FeeQuoteError::Invalid(
                "trace is available only in eez_debugQuoteFee",
            ));
        }
        let budget = Arc::new(Budget::new(cancelled));
        budget.check()?;
        let estimator = self
            .gas_estimator(prepared.rollup_id)
            .map_err(estimate_error)?;
        let mut snapshot = self.fee_snapshot(&prepared, &budget)?;
        let parent = snapshot.l2_parent.clone();
        let mut trace = None;
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
            // Predecessors establish this request's state but their private
            // pending transaction bodies do not belong in its debug export.
            trace = if request.trace_calls {
                let eez_l1::BundleTarget::Exact { block, timestamp } =
                    prepared.posting.quote.target
                else {
                    return Err(FeeQuoteError::Unavailable("unresolved trace target"));
                };
                Some(crate::composer::quote_trace::Scope::begin(
                    request.transaction.clone(),
                    json!({
                        "rollupId":rollup_id,
                        "l1ChainId":format!("{:#x}",self.inner.posting.ctx.l1_chain_id),
                        "l2ChainId":format!("{:#x}",self.inner.posting.ctx.l2_chain_id),
                        "l1ParentHash":prepared.posting.quote.sample.hash,
                        "l1ParentNumber":prepared.posting.quote.sample.number,
                        "l1TargetNumber":block,"l1TargetTimestamp":timestamp,
                        "l2ParentHash":parent.hash(),"l2ParentNumber":parent.number,
                        "l2Timestamp":snapshot.l2_timestamp,
                        "nonce":format!("{nonce:#x}"),
                        "pendingPredecessors":predecessors.len(),
                        "simulation":true
                    }),
                ))
            } else {
                None
            };
            let issued = unix_now()?;
            let expiry = prepared.expires_at;
            let mut options = Vec::new();
            let mut diagnostics = Vec::new();
            for &ty in &request.types {
                budget.check()?;
                crate::composer::quote_trace::phase(ty, "gas_estimation");
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
                    crate::composer::quote_trace::phase(ty, "fee_simulation");
                    let mut detail = self
                        .quote_in_context(&prepared, &snapshot, &budget)
                        .map_err(|error| error.at_stage("fee_simulation"))?;
                    budget.check()?;
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
                        | FeeQuoteError::Execution { .. }
                        | FeeQuoteError::Invalid(_)
                        | FeeQuoteError::InsufficientFunds(_)
                        | FeeQuoteError::Underfunded(_)
                        | FeeQuoteError::L1Underfunded(_)
                        | FeeQuoteError::SourceFeeCap { .. }),
                    ) => {
                        options.push(failed_option(ty, &error, false));
                        if debug {
                            diagnostics.push(failed_option(ty, &error, true));
                        }
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
        let result = self.validate_fee_context(&prepared, &parent).and(result);
        // Even an infrastructure/funding/context failure must finalize the
        // request-local collector. It still invalidates ALL quote options.
        match (result, trace) {
            (Ok(mut response), Some(trace)) => {
                response["diagnostics"]["trace"] = trace.finish();
                Ok(response)
            }
            (Err(error), Some(trace)) => Err(error.with_trace(trace.finish())),
            (result, None) => result,
        }
    }
}

fn failed_option(ty: u8, error: &FeeQuoteError, debug: bool) -> Value {
    let mut failed = json!({"type":format!("{ty:#x}"), "error":{"code":error.code(), "message":error.to_string()}});
    let details = if debug {
        error.debug_diagnostics()
    } else {
        error.diagnostics()
    };
    if let Some(details) = details {
        failed["error"]["details"] = details;
    }
    failed
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
    // A type-2 cap includes base-fee headroom, so the charged price is
    // `base + tip`. The caller computes the separate wallet reservation from the limit.
    let charged = if transaction["type"] == "0x0" {
        cap
    } else {
        let base = ["l1BaseFeePerGas", "l2BaseFeePerGas"]
            .iter()
            .find_map(|key| detail["estimate"][key].as_str())
            .and_then(|base| base.parse::<u128>().ok())
            .ok_or(FeeQuoteError::Unavailable("internal quote base fee"))?;
        cap.min(base.saturating_add(price(transaction, "maxPriorityFeePerGas")?))
    };
    option["estimatedFee"] = json!(format!("{:#x}", U256::from(charged) * U256::from(gas)));
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
    fn operational_failure_retains_bounded_trace_without_exposing_provider_errors() {
        let trace = json!({"format":"eez-quote-call-trace","runs":[{"id":0,"frames":[]} ]});
        let error = estimate_error(EstimateError::Unavailable(
            "https://private.example/?token=secret".into(),
        ))
        .with_trace(trace.clone());
        assert_eq!(error.code(), "quote_unavailable");
        assert!(matches!(error.untraced(), FeeQuoteError::Unavailable(_)));
        let captured = error.trace().unwrap();
        assert_eq!(captured["runs"], trace["runs"]);
        assert_eq!(captured["partial"], true);
        assert_eq!(captured["failure"]["code"], "quote_unavailable");
        assert!(!captured.to_string().contains("secret"));
        assert!(!captured.to_string().contains("private.example"));
        assert!(error.diagnostics().is_none());
    }

    #[test]
    fn call_tracing_is_explicit_and_strict() {
        assert!(
            !CompactFeeRequest::parse(&transaction(), &json!({}))
                .unwrap()
                .trace_calls
        );
        assert!(
            CompactFeeRequest::parse(&transaction(), &json!({"trace":"calls"}))
                .unwrap()
                .trace_calls
        );
        for value in [
            Value::Null,
            json!(false),
            json!(true),
            json!(""),
            json!("steps"),
            json!({}),
        ] {
            assert!(CompactFeeRequest::parse(&transaction(), &json!({"trace":value})).is_err());
        }
    }
    #[test]
    fn failed_options_preserve_execution_cause_only_in_debug_projection() {
        use alloy_sol_types::{Revert, SolError};

        for (stage, cause, reason, message) in [
            (
                "gas_estimation",
                EstimateError::Revert(Revert::from("order expired").abi_encode().into()),
                "execution_reverted",
                "order expired",
            ),
            (
                "final_validation",
                EstimateError::Effects("return bytes differ".into()),
                "effects_mismatch",
                "return bytes differ",
            ),
            (
                "fee_simulation",
                EstimateError::Unsupported("unsupported call shape".into()),
                "unsupported_execution",
                "unsupported call shape",
            ),
        ] {
            let error = estimate_error_at(stage, cause);
            for ty in [0, 2] {
                let compact = failed_option(ty, &error, false);
                let debug = failed_option(ty, &error, true);
                assert_eq!(compact["error"]["code"], "unsupported_transaction");
                assert_eq!(compact["error"]["message"], debug["error"]["message"]);
                assert!(compact["error"].get("details").is_none());
                assert!(compact.get("gas").is_none());
                assert_eq!(debug["error"]["details"]["stage"], stage);
                assert_eq!(debug["error"]["details"]["reason"], reason);
                assert_eq!(debug["error"]["details"]["message"], message);
            }
        }
        let error = FeeQuoteError::Unsupported("withdrawal shape").at_stage("fee_simulation");
        assert_eq!(
            error.debug_diagnostics().unwrap()["message"],
            "withdrawal shape"
        );
    }

    #[test]
    fn diagnostic_projection_preserves_operational_and_funding_errors() {
        let error = estimate_error(EstimateError::Unavailable("private provider detail".into()));
        let debug = failed_option(2, &error, true);
        assert_eq!(debug["error"]["code"], "quote_unavailable");
        assert!(!debug.to_string().contains("private provider detail"));
        assert!(debug["error"].get("details").is_none());
        for (error, code) in [
            (EstimateError::ContextChanged, "stale_context"),
            (EstimateError::SettlementPending, "quote_unavailable"),
            (EstimateError::Timeout, "quote_limit"),
        ] {
            assert_eq!(estimate_error(error).code(), code);
        }
        let error = FeeQuoteError::SourceFeeCap {
            cap: 1,
            base: 2,
            chain_id: 1,
        };
        let compact = failed_option(2, &error, false);
        assert_eq!(compact, failed_option(2, &error, true));
        assert_eq!(compact["error"]["details"]["reason"], "source_base_fee");
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
            // Base 10 and tip 5: legacy pays its full price 15, while type 2
            // carries the 150%-base cap 20 but is still charged 15. Scaling
            // rounds up, and overflow is rejected rather than saturated.
            let cap = request.quoted_fee_cap(10, 5, 150).unwrap();
            assert_eq!(cap, if ty == 0 { 15 } else { 20 });
            assert_eq!(
                request.quoted_fee_cap(10, 5, 200).unwrap(),
                if ty == 0 { 15 } else { 25 }
            );
            assert_eq!(
                request.quoted_fee_cap(7, 0, 150).unwrap(),
                if ty == 0 { 7 } else { 11 }
            );
            assert_eq!(
                request.quoted_fee_cap(u128::MAX / 100 + 1, 0, 150).is_err(),
                ty == 2
            );
            let tx = request.fields(5, cap);
            let mut transaction = tx.clone();
            for fee in ["gasPrice", "maxFeePerGas", "maxPriorityFeePerGas"] {
                transaction.as_object_mut().unwrap().remove(fee);
            }
            let detail = json!({"transaction":tx,"requestHash":keccak256(transaction.to_string()),
                "estimate":{"sourceGasUsed":100_000,"l2BaseFeePerGas":"10"}, "policy":{"private":"debug only"}});
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
