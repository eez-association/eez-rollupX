//! Measured payment shortfalls. These diagnostics convey a calculation; they
//! neither reserve a price nor authorize queue disposition.

use alloy_primitives::U256;
use eez_l1::{L1BlockRef, posting_fees::FeeQuote};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FundingDiagnostic {
    /// Gross priority payment, in native base units on `payment_chain_id`.
    /// L1 recovery includes the percentage/payout allowance in `required`.
    pub required: U256,
    pub offered: U256,
    pub payment_chain_id: u64,
    pub source_gas_used: u64,
    pub source_base_fee: u128,
    /// Admission prices one source; scheduler deferral prices the whole prefix.
    pub candidate_prefix: bool,
    pub l1_sample: L1BlockRef,
    pub l2_parent: L1BlockRef,
    pub l1_priority_fee: u128,
    pub l1_execution_price: u128,
    pub l1_blob_price: u128,
}

impl FundingDiagnostic {
    pub(crate) fn measured(
        posting: FeeQuote,
        l2_parent: L1BlockRef,
        payment_chain_id: u64,
        required: U256,
        offered: U256,
        source_gas_used: u64,
        source_base_fee: u128,
    ) -> Self {
        Self {
            required,
            offered,
            payment_chain_id,
            source_gas_used,
            source_base_fee,
            candidate_prefix: false,
            l1_sample: posting.sample,
            l2_parent,
            l1_priority_fee: posting.max_priority_fee_per_gas,
            l1_execution_price: posting.estimated_effective_gas_price,
            l1_blob_price: posting.estimated_blob_gas_price,
        }
    }

    pub(crate) fn for_prefix(mut self) -> Self {
        self.candidate_prefix = true;
        self
    }

    pub fn json(&self) -> Value {
        json!({
            "reason":"settlement_fee", "paymentBasis":"gross_priority_payment",
            "scope":if self.candidate_prefix { "candidate_prefix" } else { "source" },
            "paymentChainId":format!("{:#x}",self.payment_chain_id),
            "requiredPayment":self.required, "offeredPayment":self.offered,
            "shortfall":self.required.saturating_sub(self.offered),
            "sourceGasUsed":format!("{:#x}",self.source_gas_used),
            "sourceBaseFeePerGas":format!("{:#x}",self.source_base_fee),
            "l1PriorityFeePerGas":format!("{:#x}",self.l1_priority_fee),
            "l1ExecutionPrice":format!("{:#x}",self.l1_execution_price),
            "l1BlobPrice":format!("{:#x}",self.l1_blob_price),
            "l1Sample":{"number":format!("{:#x}",self.l1_sample.number),"hash":self.l1_sample.hash},
            "l2Parent":{"number":format!("{:#x}",self.l2_parent.number),"hash":self.l2_parent.hash},
        })
    }
}

impl std::fmt::Display for FundingDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "settlement payment {} is below required {} on chain {}",
            self.offered, self.required, self.payment_chain_id
        )
    }
}
