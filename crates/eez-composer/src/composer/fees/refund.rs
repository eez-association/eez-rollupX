//! Opt-in recovery from one external L1 payer. Enabling this profile asserts
//! that every configured relay has been qualified for an immediate EOA refund.
//! A projected refund is never recorded as received revenue.
use super::super::posting::{BundleExecution, PostingRuntime};
use alloy_consensus::{Transaction, transaction::SignerRecoverable};
use alloy_eips::Decodable2718;
use alloy_primitives::{Address, U256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Plan {
    pub request: eez_l1::refund::BundleRefund,
    pub minimum: U256,
    pub projected: U256,
}

/// Only final signed native receipts may select the refund source. The return
/// value is bound to the prepared post hash by the caller before dispatch.
pub(in crate::composer) fn select(
    runtime: &PostingRuntime,
    quote: eez_l1::posting_fees::FeeQuote,
    raw: &[alloy_primitives::Bytes],
    execution: &BundleExecution,
) -> Result<Option<Plan>, &'static str> {
    let Some(refund) = &runtime.l1_refund_policy else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Err("refund bundle missing post");
    }
    if raw.len() == 1 {
        return Ok(None);
    }
    if runtime
        .l1_refund_failed
        .load(std::sync::atomic::Ordering::Acquire)
    {
        return Err("l1_refund_route_unqualified");
    }
    let policy = runtime
        .shadow_fee_policy
        .as_ref()
        .ok_or("refund cost policy missing")?;
    if execution.gas_used.len() != raw.len()
        || execution.transaction_hashes.len() != raw.len()
        || execution.blob_counts.len() != raw.len()
    {
        return Err("refund native receipt coverage");
    }
    let cost = quote
        .estimate(execution.gas_used[0], execution.blob_counts[0])
        .map_err(|_| "refund posting cost")?;
    let minimum = policy.required_l1_payment(cost.total)?.minimum_l1;
    let state = runtime
        .chain
        .provider
        .state_by_block_hash(quote.sample.hash)
        .map_err(|_| "refund recipient state")?;
    if state
        .account_code(&refund.recipient)
        .map_err(|_| "refund recipient code")?
        .is_some_and(|c| !c.is_empty())
    {
        return Err("refund recipient must be a plain EOA");
    }
    select_payer(
        policy,
        refund,
        quote,
        raw,
        execution,
        runtime.ctx.l1_poster_signer.address(),
        minimum,
    )
}

fn select_payer(
    policy: &super::pricing::Policy,
    refund: &Policy,
    quote: eez_l1::posting_fees::FeeQuote,
    raw: &[alloy_primitives::Bytes],
    execution: &BundleExecution,
    poster: Address,
    minimum: U256,
) -> Result<Option<Plan>, &'static str> {
    let eez_l1::BundleTarget::Exact { timestamp, .. } = quote.target else {
        return Err("refund target unresolved");
    };
    let base = quote
        .estimated_effective_gas_price
        .checked_sub(quote.max_priority_fee_per_gas)
        .and_then(|v| u64::try_from(v).ok())
        .ok_or("refund base fee")?;
    let mut selected: Option<Plan> = None;
    for (index, bytes) in raw.iter().enumerate().skip(1) {
        let mut remaining = bytes.as_ref();
        let tx = alloy_consensus::TxEnvelope::decode_2718(&mut remaining)
            .map_err(|_| "refund source decoding")?;
        if !remaining.is_empty()
            || tx.tx_hash() != &execution.transaction_hashes[index]
            || tx.chain_id() != Some(quote.chain_id)
        {
            return Err("refund source binding");
        }
        let sender = tx.recover_signer().map_err(|_| "refund source signature")?;
        if sender == refund.recipient || !policy.quote_eligible(sender, poster, timestamp) {
            continue;
        }
        let tip = tx
            .effective_tip_per_gas(base)
            .ok_or("refund source fee cap")?;
        let projected = refund.net(
            U256::from(tip) * U256::from(execution.gas_used[index]),
            quote,
        )?;
        if selected.is_none_or(|p| projected > p.projected) {
            selected = Some(Plan {
                request: eez_l1::refund::BundleRefund::new(
                    refund.recipient,
                    refund.percent,
                    *tx.tx_hash(),
                )?,
                minimum,
                projected,
            });
        }
    }
    selected
        .filter(|p| p.projected >= p.minimum)
        .map(Some)
        .ok_or("l1_tip_underfunded")
}

pub(crate) const SETTING: &str = "EEZ_L1_FEE_REFUND_POLICY";

#[derive(Debug, Clone)]
pub(crate) struct Policy {
    pub recipient: Address,
    pub percent: u8,
}

impl Policy {
    pub fn parse(raw: Option<String>) -> Result<Option<Self>, String> {
        let Some(raw) = raw else { return Ok(None) };
        let parse = || {
            if raw.len() > 1024 {
                return Err("JSON exceeds 1024 bytes".to_owned());
            }
            let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
            let fields = value.as_object().ok_or("expected JSON object")?;
            if fields.len() != 3
                || fields
                    .keys()
                    .any(|k| !["version", "recipient", "percent"].contains(&k.as_str()))
            {
                return Err(
                    "version, recipient and percent are required; unknown fields rejected"
                        .to_owned(),
                );
            }
            let recipient = value["recipient"]
                .as_str()
                .ok_or("recipient must be an address")?
                .parse::<Address>()
                .map_err(|e| e.to_string())?;
            let percent = value["percent"]
                .as_u64()
                .and_then(|v| u8::try_from(v).ok())
                .ok_or("percent must be an integer 1..99")?;
            let policy = Self { recipient, percent };
            if value["version"].as_u64() != Some(1)
                || policy.recipient.is_zero()
                || !(1..=99).contains(&policy.percent)
            {
                return Err("version 1 requires nonzero recipient and percent 1..99".to_owned());
            }
            Ok(policy)
        };
        parse().map(Some).map_err(|e| format!("{SETTING}: {e}"))
    }

    /// This profile only supports plain EOA payouts. Charge at the posting
    /// transaction's effective price, conservatively including its priority fee.
    pub fn payout_cost(&self, quote: eez_l1::posting_fees::FeeQuote) -> U256 {
        U256::from(21_000) * U256::from(quote.estimated_effective_gas_price)
    }

    pub fn gross(
        &self,
        required: U256,
        quote: eez_l1::posting_fees::FeeQuote,
    ) -> Result<U256, &'static str> {
        eez_l1::refund::gross_payment(required, self.percent, self.payout_cost(quote))
    }

    pub fn net(
        &self,
        gross: U256,
        quote: eez_l1::posting_fees::FeeQuote,
    ) -> Result<U256, &'static str> {
        eez_l1::refund::net_payment(gross, self.percent, self.payout_cost(quote))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn refund_selection_uses_receipt_gas_effective_tip_and_external_money() {
        use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope, TxLegacy};
        use alloy_eips::Encodable2718;
        use alloy_primitives::{B256, Bytes, Signature};
        let legacy: TxEnvelope = TxLegacy {
            chain_id: Some(1),
            gas_limit: 1_000_000,
            gas_price: 101,
            ..Default::default()
        }
        .into_signed(Signature::test_signature())
        .into();
        let capped: TxEnvelope = TxEip1559 {
            chain_id: 1,
            gas_limit: 300_000,
            max_fee_per_gas: 11,
            max_priority_fee_per_gas: 1_000,
            ..Default::default()
        }
        .into_signed(Signature::test_signature())
        .into();
        let quote = eez_l1::posting_fees::FeeQuote {
            chain_id: 1,
            sample: eez_l1::L1BlockRef {
                number: 1,
                hash: B256::repeat_byte(1),
            },
            target: eez_l1::BundleTarget::Exact {
                block: 2,
                timestamp: 100,
            },
            max_fee_per_gas: 3,
            max_priority_fee_per_gas: 2,
            max_fee_per_blob_gas: 1,
            estimated_effective_gas_price: 3,
            estimated_blob_gas_price: 1,
        };
        let policy = super::super::pricing::Policy::parse(
            Some(super::super::pricing::tests::policy_json().to_string()),
            Address::repeat_byte(4),
        )
        .unwrap()
        .unwrap();
        let refund = Policy {
            recipient: Address::repeat_byte(9),
            percent: 90,
        };
        let raw = vec![
            Bytes::from_static(&[3]),
            legacy.encoded_2718().into(),
            capped.encoded_2718().into(),
        ];
        let mut execution = BundleExecution {
            gas_used: vec![100_000, 10_000, 200_000],
            gas_required: vec![100_000, 10_000, 200_000],
            gas_limits: vec![110_000, 1_000_000, 300_000],
            transaction_hashes: vec![B256::repeat_byte(5), *legacy.tx_hash(), *capped.tx_hash()],
            blob_counts: vec![1, 0, 0],
        };
        let pick = |execution: &BundleExecution, poster, minimum| {
            select_payer(&policy, &refund, quote, &raw, execution, poster, minimum)
        };
        let plan = pick(&execution, Address::ZERO, U256::from(500_000))
            .unwrap()
            .unwrap();
        assert_eq!(plan.request.source(), *capped.tx_hash());
        assert_eq!(plan.projected, U256::from(1_737_000));
        assert_eq!(
            pick(&execution, Address::ZERO, plan.projected + U256::from(1)),
            Err("l1_tip_underfunded")
        );
        let poster = capped.recover_signer().unwrap();
        assert_eq!(
            pick(&execution, poster, U256::from(500_000))
                .unwrap()
                .unwrap()
                .request
                .source(),
            *legacy.tx_hash()
        );
        let recipient_refund = Policy {
            recipient: poster,
            percent: 90,
        };
        assert_eq!(
            select_payer(
                &policy,
                &recipient_refund,
                quote,
                &raw,
                &execution,
                Address::ZERO,
                U256::from(500_000)
            )
            .unwrap()
            .unwrap()
            .request
            .source(),
            *legacy.tx_hash()
        );
        execution.transaction_hashes[2] = B256::ZERO;
        assert_eq!(
            pick(&execution, Address::ZERO, U256::from(1)),
            Err("refund source binding")
        );
    }

    #[test]
    fn refund_policy_is_explicit_strict_and_optional() {
        assert!(Policy::parse(None).unwrap().is_none());
        let good = json!({"version":1,"recipient":Address::repeat_byte(1),"percent":90});
        assert_eq!(
            Policy::parse(Some(good.to_string()))
                .unwrap()
                .unwrap()
                .percent,
            90
        );
        for raw in ["", "null", "{}", "[]"] {
            assert!(Policy::parse(Some(raw.into())).is_err());
        }
        for (key, value) in [
            ("version", json!(2)),
            ("percent", json!(0)),
            ("percent", json!(100)),
            ("percent", json!("90")),
            ("recipient", json!(Address::ZERO)),
            ("extra", json!(true)),
        ] {
            let mut invalid = good.clone();
            invalid[key] = value;
            assert!(Policy::parse(Some(invalid.to_string())).is_err());
        }
    }
}
