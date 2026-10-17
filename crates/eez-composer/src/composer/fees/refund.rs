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
    let Some(refund) = &runtime.collection.l1_refund else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Err("refund bundle missing post");
    }
    if raw.len() == 1 {
        return Ok(None);
    }
    let policy = runtime
        .collection
        .pricing
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
    // A shared batch may deliberately cost more than its largest refund.
    // Keep the floor for receipt observability, never as a batch size gate.
    selected.map(Some).ok_or("refund external payer missing")
}

#[derive(Debug, Clone)]
pub(crate) struct Policy {
    pub recipient: Address,
    pub percent: u8,
}

impl Policy {
    pub fn parse(
        recipient: Option<String>,
        percent: Option<String>,
    ) -> Result<Option<Self>, String> {
        if recipient.is_none() && percent.is_none() {
            return Ok(None);
        }
        let recipient = recipient
            .as_deref()
            .and_then(|s| s.parse::<Address>().ok())
            .filter(|address| !address.is_zero())
            .ok_or("EEZ_FEES_L1_REFUND_RECIPIENT must be a nonzero 20-byte address")?;
        let percent = percent
            .as_deref()
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<u8>().ok())
            .filter(|p| (1..=99).contains(p))
            .ok_or("EEZ_FEES_L1_REFUND_PERCENT must be an integer 1..99")?;
        Ok(Some(Self { recipient, percent }))
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
        let policy =
            super::super::pricing::tests::parse(super::super::pricing::tests::policy_json())
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
        let subsidized = pick(&execution, Address::ZERO, plan.projected + U256::from(1))
            .unwrap()
            .unwrap();
        assert_eq!(subsidized.request.source(), *capped.tx_hash());
        assert!(subsidized.projected < subsidized.minimum);
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
    fn refund_settings_require_both_terms_and_preserve_the_supported_range() {
        assert!(Policy::parse(None, None).unwrap().is_none());
        let recipient = Some(Address::repeat_byte(1).to_string());
        for percent in ["1", "50", "90", "99"] {
            assert_eq!(
                Policy::parse(recipient.clone(), Some(percent.into()))
                    .unwrap()
                    .unwrap()
                    .percent,
                percent.parse::<u8>().unwrap()
            );
        }
        for percent in [
            None,
            Some(""),
            Some("0"),
            Some("100"),
            Some("256"),
            Some("-1"),
            Some("+90"),
            Some("1.5"),
            Some(" 90"),
        ] {
            assert!(Policy::parse(recipient.clone(), percent.map(str::to_owned)).is_err());
        }
        for recipient in [
            None,
            Some(""),
            Some("bad"),
            Some("0x0000000000000000000000000000000000000000"),
        ] {
            assert!(Policy::parse(recipient.map(str::to_owned), Some("90".into())).is_err());
        }
    }
}
