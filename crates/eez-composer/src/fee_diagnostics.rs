//! Source/poster balance and measured payment shortfalls. These diagnostics convey a
//! calculation; they neither reserve a price nor authorize queue disposition.

use alloy_primitives::{Address, U256};
use eez_l1::{L1BlockRef, posting_fees::FeeQuote};
use serde_json::{Value, json};

/// The poster cannot run a settlement probe within its sampled balance. The
/// affordable gas bound is diagnostic, not a measured settlement requirement.
#[derive(Clone, Debug, thiserror::Error)]
#[error("posting wallet {poster} cannot fund the settlement gas probe on chain {chain_id}")]
pub struct PosterBalanceError {
    pub poster: Address,
    pub chain_id: u64,
    pub available: U256,
    pub blob_reserve: U256,
    pub affordable_gas: u64,
    pub gas_budget: u64,
    pub sample: L1BlockRef,
}

impl PosterBalanceError {
    pub fn json(&self) -> Value {
        json!({
            "reason":"poster_balance", "scope":"poster", "poster":self.poster,
            "chainId":format!("{:#x}", self.chain_id),
            "availableBalance":self.available, "blobFeeReservation":self.blob_reserve,
            "maxAffordableGas":format!("{:#x}", self.affordable_gas),
            "gasBudget":format!("{:#x}", self.gas_budget),
            "l1Sample":{"number":format!("{:#x}",self.sample.number),"hash":self.sample.hash},
        })
    }

    pub(crate) fn limits_probe(&self) -> bool {
        self.affordable_gas < self.gas_budget
    }

    pub(crate) fn explains(&self, gas_limit: u64, error: &impl reth_evm::EvmError) -> bool {
        use reth_evm::InvalidTxError;
        use revm::context_interface::result::InvalidTransaction;
        error.as_invalid_tx_err().is_some_and(|error| {
            (self.limits_probe()
                && gas_limit == self.affordable_gas
                && error.is_gas_limit_too_low())
                || matches!(
                    error.as_invalid_tx_err(),
                    Some(InvalidTransaction::LackOfFundForMaxFee { .. })
                )
        })
    }
}

/// Failed source-chain balance validation, distinct from settlement-tip coverage.
/// `required` includes value and the maximum fee reservation when provided by
/// the EVM; a pre-execution value check can report value alone before pricing.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "insufficient funds for transaction: balance {available}, required {required} on chain {chain_id}"
)]
pub struct SourceBalanceError {
    pub available: U256,
    pub required: U256,
    pub chain_id: u64,
}

impl SourceBalanceError {
    pub(crate) fn from_executor(
        chain_id: u64,
        error: &eez_protocol::ExecutorError,
    ) -> Option<Self> {
        match error.kind() {
            eez_protocol::ExecutorErrorKind::SourceBlocked(
                eez_protocol::SourceCondition::Balance {
                    required,
                    available,
                },
            ) => Some(Self {
                available: *available,
                required: *required,
                chain_id,
            }),
            _ => None,
        }
    }

    pub fn json(&self) -> Value {
        json!({
            "reason":"source_balance", "sourceChainId":format!("{:#x}",self.chain_id),
            "availableBalance":self.available, "requiredBalance":self.required,
            "shortfall":self.required.saturating_sub(self.available),
        })
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use revm::context_interface::result::{EVMError, InvalidTransaction};
    use std::convert::Infallible;

    #[test]
    fn poster_diagnostic_does_not_misclassify_a_tighter_replay_or_unrelated_error() {
        let mut funds = PosterBalanceError {
            poster: Address::ZERO,
            chain_id: 1,
            available: U256::from(50_000),
            blob_reserve: U256::ZERO,
            affordable_gas: 50_000,
            gas_budget: 100_000,
            sample: L1BlockRef {
                number: 1,
                hash: Default::default(),
            },
        };
        let intrinsic: EVMError<Infallible> =
            EVMError::Transaction(InvalidTransaction::CallGasCostMoreThanGasLimit {
                initial_gas: 60_000,
                gas_limit: 50_000,
            });
        assert!(funds.explains(50_000, &intrinsic));
        // A smaller measured replay may fail while the affordable probe works;
        // that must preserve the existing fallback to the successful probe.
        assert!(!funds.explains(40_000, &intrinsic));
        funds.affordable_gas = funds.gas_budget;
        assert!(!funds.explains(100_000, &intrinsic));
        let balance: EVMError<Infallible> =
            EVMError::Transaction(InvalidTransaction::LackOfFundForMaxFee {
                fee: Box::new(U256::from(100_000)),
                balance: Box::new(funds.available),
            });
        assert!(funds.explains(100_000, &balance));
        let nonce: EVMError<Infallible> =
            EVMError::Transaction(InvalidTransaction::NonceTooLow { tx: 0, state: 1 });
        assert!(!funds.explains(100_000, &nonce));
        let provider: EVMError<Infallible> = EVMError::Custom("unavailable".into());
        assert!(!funds.explains(100_000, &provider));
    }
}
