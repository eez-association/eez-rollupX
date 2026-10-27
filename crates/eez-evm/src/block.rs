//! Rollup block lifecycle: history writes remain, beacon and request calls do not.
//! Transaction execution, receipts and gas accounting delegate to the pinned
//! Ethereum executor, except that native transactions execute with the remaining
//! block gas instead of a reserved declared limit. The inner counters remain accessible to the candidate-local
//! prefix cache, whose checkpoint is always taken before block finalization.

use crate::balances::clear_protocol_balances;
use crate::{EezEvmFactory, EezReceiptBuilder, ensure_supported_transaction};
use alloy_consensus::{Transaction as _, TransactionEnvelope as _, Typed2718 as _};
use alloy_eips::eip2718::Encodable2718 as _;
use alloy_evm::{
    Evm, EvmFactory, RecoveredTx,
    block::{
        BlockExecutionError, BlockExecutionResult, BlockExecutor, BlockExecutorFactory,
        ExecutableTx, GasOutput, StateDB, TxResult,
    },
    eth::{EthBlockExecutionCtx, EthBlockExecutor, EthTxResult, spec::EthExecutorSpec},
};
use eez_primitives::{EezTxEnvelope, EezTxType, Receipt, system_tx_gas_limit};
use eez_protocol::system_tx::ExecutionGroupState;
use revm::{
    DatabaseCommit, Inspector,
    context::result::ResultAndState,
    context::{Block as _, Transaction as _},
    primitives::hardfork::SpecId,
};
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

/// L1 simulation retains Ethereum's complete lifecycle. Runtime L2 wiring uses
/// `Rollup`; the distinction must never be inferred from transaction contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionLayer {
    Rollup,
    L1Simulation,
}

#[derive(Clone, Debug)]
pub struct EezBlockExecutorFactory<C> {
    spec: Arc<C>,
    layer: ExecutionLayer,
    evm: EezEvmFactory,
    pub(crate) history: Option<crate::history::HistoryConfig>,
}

impl<C: reth_chainspec::EthChainSpec<Header = alloy_consensus::Header>> EezBlockExecutorFactory<C> {
    pub fn new(spec: Arc<C>, layer: ExecutionLayer) -> Self {
        let evm = match layer {
            ExecutionLayer::Rollup => EezEvmFactory {
                collector: crate::profile::schedule(spec.as_ref())
                    .map(|schedule| Some(schedule.config().fee_collector)),
            },
            ExecutionLayer::L1Simulation => EezEvmFactory::for_l1_simulation(),
        };
        Self {
            spec,
            layer,
            evm,
            history: None,
        }
    }
}

#[derive(Debug)]
pub struct EezBlockExecutor<'a, E, S> {
    inner: EthBlockExecutor<'a, E, S, EezReceiptBuilder>,
    layer: ExecutionLayer,
    history: Option<crate::history::HistoryConfig>,
    history_inherited: bool,
    group_state: ExecutionGroupState,
}

impl<'a, E, S> Deref for EezBlockExecutor<'a, E, S> {
    type Target = EthBlockExecutor<'a, E, S, EezReceiptBuilder>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl<E, S> DerefMut for EezBlockExecutor<'_, E, S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// An execution result plus its uncommitted group cursor. Dropping a preview
/// must discard both its balance cleanup and its proposed cursor advancement.
#[derive(Debug)]
pub struct EezTxResult<H> {
    inner: EthTxResult<H, EezTxType>,
    group_state: ExecutionGroupState,
}

impl<H: Send + 'static> TxResult for EezTxResult<H> {
    type HaltReason = H;

    fn result(&self) -> &ResultAndState<H> {
        &self.inner.result
    }

    fn into_result(self) -> ResultAndState<H> {
        self.inner.result
    }
}

impl<E, S> EezBlockExecutor<'_, E, S> {
    /// Cursor paired with the committed prefix, including an unfinished load
    /// while a Composer candidate is still being constructed.
    pub const fn group_state(&self) -> ExecutionGroupState {
        self.group_state
    }
}

impl<E, S> BlockExecutor for EezBlockExecutor<'_, E, S>
where
    E: Evm<DB: StateDB, Spec: Into<SpecId> + Clone, Tx = revm::context::TxEnv>,
    S: EthExecutorSpec,
{
    type Transaction = EezTxEnvelope;
    type Receipt = Receipt;
    type Evm = E;
    type Result = EezTxResult<E::HaltReason>;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        if self.layer == ExecutionLayer::L1Simulation {
            return self.inner.apply_pre_execution_changes();
        }
        // No beacon rewards or withdrawals are allowed to be silently omitted
        // when finish bypasses Ethereum's consensus-layer post-processing.
        if !self.inner.ctx.ommers.is_empty()
            || self
                .inner
                .ctx
                .withdrawals
                .as_ref()
                .is_some_and(|w| !w.is_empty())
        {
            return Err(BlockExecutionError::msg(
                "RollupX does not admit ommers or beacon withdrawals",
            ));
        }
        let spec: SpecId = (*self.inner.evm.cfg_env().spec()).into();
        if spec != SpecId::OSAKA {
            return Err(BlockExecutionError::msg(
                "RollupX execution profile requires Osaka",
            ));
        }
        self.inner
            .system_caller
            .apply_blockhashes_contract_call(self.inner.ctx.parent_hash, &mut self.inner.evm)?;
        let mut cleanup = Default::default();
        clear_protocol_balances(self.inner.evm.db_mut(), &mut cleanup)
            .map_err(|_| BlockExecutionError::msg("cannot load protocol cleanup accounts"))?;
        self.inner.evm.db_mut().commit(cleanup);
        let history = self.history.as_ref().ok_or_else(|| {
            BlockExecutionError::msg("mandatory L1 history evidence source is not configured")
        })?;
        self.history_inherited = crate::history::begin(&mut self.inner.evm, history)?;
        Ok(())
    }

    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<Self::Result, BlockExecutionError> {
        let (mut env, recovered) = tx.into_parts();
        let (next_group, completes_group) = if self.layer == ExecutionLayer::Rollup {
            self.group_state
                .after(recovered.tx())
                .map_err(BlockExecutionError::msg)?
        } else {
            (self.group_state, false)
        };
        if self.layer == ExecutionLayer::Rollup {
            ensure_supported_transaction(recovered.tx()).map_err(BlockExecutionError::other)?;
            let first = self.inner.receipts.is_empty();
            if (recovered.tx().is_history() && !first)
                || (first && !recovered.tx().is_history() && !self.history_inherited)
            {
                return Err(BlockExecutionError::msg(
                    "duplicate/misplaced history transaction or incorrect inherited PREVRANDAO",
                ));
            }
            if recovered.tx().is_history() {
                let history = self
                    .history
                    .as_ref()
                    .ok_or_else(|| BlockExecutionError::msg("missing history configuration"))?;
                env.data = crate::history::validate(&mut self.inner.evm, history, recovered.tx())?;
            }

            if eez_primitives::is_native_type(recovered.tx().ty()) {
                // Ethereum's rule charges a transaction's declared limit against
                // the block before execution. A native transaction instead runs
                // with the remaining block gas, bounded by the Osaka cap, so it
                // never needs reserved headroom and the block total can never
                // exceed the limit. The profile is Osaka-only (checked in
                // `apply_pre_execution_changes`), so `cumulative_tx_gas_used` is
                // the block-gas measure the inner executor also applies.
                //
                // A budget below the intrinsic cost is rejected by revm, which
                // invalidates the block: execution never shortens the call data
                // or skips the transaction.
                let mut budget = system_tx_gas_limit(
                    self.inner.evm.block().gas_limit(),
                    self.inner.cumulative_tx_gas_used,
                );
                if recovered.tx().is_history() {
                    budget = budget.min(eez_protocol::l1_history::IMPORT_GAS_LIMIT);
                }
                if recovered.tx().is_history() {
                    env.gas_limit = budget;
                }
                if !recovered.tx().is_history()
                    && (recovered.tx().gas_limit() != budget || env.gas_limit() != budget)
                {
                    return Err(BlockExecutionError::msg(
                        "native gas limit differs from its block position",
                    ));
                }
                // The remainder mirrors the pinned inner executor after its
                // declared-limit check.
                let mut result = self
                    .inner
                    .evm
                    .transact(env)
                    .map_err(|err| BlockExecutionError::evm(err, recovered.tx().trie_hash()))?;
                if recovered.tx().is_history()
                    && (!result.result.is_success()
                        || result
                            .result
                            .output()
                            .is_none_or(|output| !output.is_empty())
                        || !result.result.logs().is_empty())
                {
                    return Err(BlockExecutionError::msg("history transaction failed"));
                }
                if completes_group {
                    clear_protocol_balances(self.inner.evm.db_mut(), &mut result.state).map_err(
                        |_| BlockExecutionError::msg("cannot load protocol cleanup accounts"),
                    )?;
                }
                return Ok(EezTxResult {
                    group_state: next_group,
                    inner: EthTxResult {
                        result,
                        blob_gas_used: recovered.tx().blob_gas_used().unwrap_or_default(),
                        tx_type: recovered.tx().tx_type(),
                    },
                });
            }
        }
        let mut inner = self
            .inner
            .execute_transaction_without_commit((env, recovered))?;
        if completes_group {
            clear_protocol_balances(self.inner.evm.db_mut(), &mut inner.result.state)
                .map_err(|_| BlockExecutionError::msg("cannot load protocol cleanup accounts"))?;
        }
        Ok(EezTxResult {
            inner,
            group_state: next_group,
        })
    }

    fn commit_transaction(&mut self, result: Self::Result) -> GasOutput {
        self.group_state = result.group_state;
        self.inner.commit_transaction(result.inner)
    }

    fn finish(self) -> Result<(E, BlockExecutionResult<Receipt>), BlockExecutionError> {
        if self.layer == ExecutionLayer::L1Simulation {
            return self.inner.finish();
        }
        // L2 has no consensus layer: no EIP-6110 parsing, EIP-7002/7251 calls,
        // beacon balance increments or DAO transition. EIP-2935 already ran.
        // A future execution fork must explicitly define its L2 lifecycle before
        // the Osaka allowlist above is extended.
        if !self.history_inherited && self.inner.receipts.is_empty() {
            return Err(BlockExecutionError::msg(
                "empty block PREVRANDAO differs from inherited L1 origin",
            ));
        }
        self.group_state
            .ensure_complete()
            .map_err(BlockExecutionError::msg)?;
        let inner = self.inner;
        Ok((
            inner.evm,
            BlockExecutionResult {
                receipts: inner.receipts,
                requests: Default::default(),
                gas_used: inner.cumulative_tx_gas_used,
                blob_gas_used: 0,
            },
        ))
    }

    fn evm(&self) -> &E {
        &self.inner.evm
    }
    fn evm_mut(&mut self) -> &mut E {
        &mut self.inner.evm
    }
    fn receipts(&self) -> &[Receipt] {
        &self.inner.receipts
    }
}

impl<C: EthExecutorSpec + 'static> BlockExecutorFactory for EezBlockExecutorFactory<C> {
    type EvmFactory = EezEvmFactory;
    type ExecutionCtx<'a> = EthBlockExecutionCtx<'a>;
    type Transaction = EezTxEnvelope;
    type Receipt = Receipt;
    type TxExecutionResult = EezTxResult<<EezEvmFactory as EvmFactory>::HaltReason>;
    type Executor<'a, DB: StateDB, I: Inspector<<EezEvmFactory as EvmFactory>::Context<DB>>> =
        EezBlockExecutor<'a, <EezEvmFactory as EvmFactory>::Evm<DB, I>, &'a Arc<C>>;

    fn evm_factory(&self) -> &EezEvmFactory {
        &self.evm
    }
    fn create_executor<'a, DB, I>(
        &'a self,
        evm: <EezEvmFactory as EvmFactory>::Evm<DB, I>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Executor<'a, DB, I>
    where
        DB: StateDB,
        I: Inspector<<EezEvmFactory as EvmFactory>::Context<DB>>,
    {
        EezBlockExecutor {
            inner: EthBlockExecutor::new(evm, ctx, &self.spec, EezReceiptBuilder),
            layer: self.layer,
            history: self.history.clone(),
            history_inherited: true,
            group_state: ExecutionGroupState::default(),
        }
    }
}

#[cfg(test)]
mod tests;
