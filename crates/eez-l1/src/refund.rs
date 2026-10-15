//! Fixed-percentage, single-source builder refund requests. These terms are
//! projected payments, not evidence that a builder transferred funds.

use alloy_primitives::{Address, B256, U256, U512};

/// Evidence for the immediate EOA profile. A payment must come from the block
/// beneficiary after the selected source. Multiple candidates are ambiguous;
/// they are not aggregated into a claimed refund.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedPayment {
    Missing,
    Ambiguous,
    Confirmed { hash: B256, value: U256 },
}

/// Final native fee sample plus the optional externally funded refund. Keeping
/// them together binds both dispatch gates to one prepared bundle.
#[derive(Debug, Clone, Copy)]
pub struct BundlePricing {
    pub quote: crate::posting_fees::FeeQuote,
    pub refund: Option<BundleRefund>,
}

impl From<crate::posting_fees::FeeQuote> for BundlePricing {
    fn from(quote: crate::posting_fees::FeeQuote) -> Self {
        Self {
            quote,
            refund: None,
        }
    }
}

/// The selected source must be an external, mandatory L1 companion. The
/// submitter verifies that binding against the exact signed bundle before I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BundleRefund {
    recipient: Address,
    percent: u8,
    source: B256,
}

impl BundleRefund {
    pub fn new(recipient: Address, percent: u8, source: B256) -> Result<Self, &'static str> {
        if recipient.is_zero() || source.is_zero() || !(1..=99).contains(&percent) {
            return Err("refund requires nonzero recipient/source and percentage 1..99");
        }
        Ok(Self {
            recipient,
            percent,
            source,
        })
    }

    pub const fn recipient(self) -> Address {
        self.recipient
    }
    pub const fn percent(self) -> u8 {
        self.percent
    }
    pub const fn source(self) -> B256 {
        self.source
    }

    pub(crate) fn apply(self, params: &mut serde_json::Value) {
        params["refundPercent"] = self.percent.into();
        params["refundRecipient"] = serde_json::json!(self.recipient);
        params["refundTxHashes"] = serde_json::json!([self.source]);
    }
}

/// Deduct the complete payout cost after applying the percentage. This is the
/// conservative common bound for providers which instead deduct it first.
pub fn net_payment(gross: U256, percent: u8, payout_cost: U256) -> Result<U256, &'static str> {
    if !(1..=99).contains(&percent) {
        return Err("invalid refund percentage");
    }
    let value = U512::from(gross) * U512::from(percent) / U512::from(100);
    let value = U256::checked_from_limbs_slice(value.as_limbs()).ok_or("refund amount overflow")?;
    Ok(value.saturating_sub(payout_cost))
}

/// Smallest gross priority payment whose conservative net reaches `required`.
pub fn gross_payment(required: U256, percent: u8, payout_cost: U256) -> Result<U256, &'static str> {
    if !(1..=99).contains(&percent) {
        return Err("invalid refund percentage");
    }
    let numerator = (U512::from(required) + U512::from(payout_cost)) * U512::from(100);
    let denominator = U512::from(percent);
    let rounded = numerator / denominator + U512::from(!((numerator % denominator).is_zero()));
    U256::checked_from_limbs_slice(rounded.as_limbs()).ok_or("refund amount overflow")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refund_covers_cost_after_deductions_and_rounds_up() {
        for percent in [1, 50, 90, 99] {
            for required in [1, 2, 99, 100, 10001] {
                let required = U256::from(required);
                let overhead = U256::from(21);
                let gross = gross_payment(required, percent, overhead).unwrap();
                assert!(net_payment(gross, percent, overhead).unwrap() >= required);
                assert!(net_payment(gross - U256::from(1), percent, overhead).unwrap() < required);
            }
        }
        assert_eq!(
            net_payment(U256::from(10), 90, U256::from(20)).unwrap(),
            U256::ZERO
        );
    }

    #[test]
    fn refund_validation_rejects_full_refund_zero_and_overflow() {
        for percent in [0, 100, 255] {
            assert!(
                BundleRefund::new(Address::repeat_byte(1), percent, B256::repeat_byte(2)).is_err()
            );
            assert!(net_payment(U256::from(100), percent, U256::ZERO).is_err());
            assert!(gross_payment(U256::from(100), percent, U256::ZERO).is_err());
        }
        assert!(BundleRefund::new(Address::ZERO, 90, B256::repeat_byte(2)).is_err());
        assert!(BundleRefund::new(Address::repeat_byte(1), 90, B256::ZERO).is_err());
        assert!(gross_payment(U256::MAX, 99, U256::from(1)).is_err());
        assert!(net_payment(U256::MAX, 99, U256::ZERO).is_ok());
    }
}
