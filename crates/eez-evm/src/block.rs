//! Rollup lifecycle and atomic native groups over Reth's evm2 block accumulator.
//! Closed prefixes finalize copies; continuing execution retains the unfinalized draft.
use crate::balances::clear_protocol_balances;
use crate::{
    EezEvmFactory, EezEvmTx, EezEvmTypes, EezReceiptBuilder, ensure_supported_transaction,
};
use alloy_consensus::{Transaction as _, TransactionEnvelope as _, Typed2718 as _};
use alloy_eip7928::{BlockAccessIndex, BlockAccessList};
use eez_primitives::{EezTxEnvelope, EezTxType, Receipt, system_tx_gas_limit};
use eez_protocol::system_tx::ExecutionGroupState;
use evm2::{
    SpecId, TxResultWithState,
    evm::{Bal, HISTORY_STORAGE_ADDRESS, PendingState, SystemTx},
};
use reth_evm::{
    BlockExecutionError, BlockExecutionOutput, BlockExecutor, BlockExecutorFactory,
    BlockTransactionResult, BundleState, ExecutorTx, GasOutput, RecoveredTx, StateUpdate,
};
use reth_evm_ethereum::{
    EthBlockExecutionCtx, EthBlockExecutor, EthBlockExecutorFactory, EthEvmEnv,
    EthTransactionResultWithState,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionLayer {
    Rollup,
    L1Simulation,
}

#[derive(Clone, Debug)]
pub struct EezBlockExecutorFactory<C> {
    ethereum: EthBlockExecutorFactory<EezReceiptBuilder, C, EezEvmFactory>,
    layer: ExecutionLayer,
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
            ethereum: EthBlockExecutorFactory::new_with_receipt_builder(
                EezReceiptBuilder,
                spec,
                evm,
            ),
            layer,
            history: None,
        }
    }
}

pub struct EezBlockExecutor<'a> {
    inner: EthBlockExecutor<'a, EezEvmTypes, &'a EezReceiptBuilder>,
    layer: ExecutionLayer,
    history: Option<crate::history::HistoryConfig>,
    history_inherited: bool,
    group_state: ExecutionGroupState,
}
impl std::fmt::Debug for EezBlockExecutor<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EezBlockExecutor")
            .field("layer", &self.layer)
            .field("group_state", &self.group_state)
            .finish_non_exhaustive()
    }
}
/// The group cursor belongs to the provisional result, including any cleanup delta.
/// Dropping this value discards all three; commit advances the cursor only after state acceptance.
#[derive(Debug)]
pub struct EezTxResult {
    inner: EthTransactionResultWithState<EezEvmTypes, EezTxType>,
    group_state: ExecutionGroupState,
}
impl BlockTransactionResult<EezEvmTypes> for EezTxResult {
    fn result(&self) -> &TxResultWithState<EezEvmTypes> {
        self.inner.result()
    }
    fn into_result(self) -> TxResultWithState<EezEvmTypes> {
        self.inner.into_result()
    }
}
/// Execution bookkeeping copied with the cache for a prefix in the same block environment.
/// This contains no EVM or execution entry point; callers must pair it with its exact cache.
#[derive(Clone, Debug)]
pub struct EezBlockCheckpoint {
    block_state: reth_execution_types::BlockState,
    receipts: Vec<Receipt>,
    cumulative_gas_used: u64,
    block_regular_gas_used: u64,
    block_state_gas_used: u64,
    blob_gas_used: u64,
    group_state: ExecutionGroupState,
    history_inherited: bool,
}
// Equality is for checkpoint verification: compare accumulated transitions using the
// same bundle representation consumed by block finalization, plus every receipt/profile field.
impl PartialEq for EezBlockCheckpoint {
    fn eq(&self, other: &Self) -> bool {
        self.receipts == other.receipts
            && self.cumulative_gas_used == other.cumulative_gas_used
            && self.block_regular_gas_used == other.block_regular_gas_used
            && self.block_state_gas_used == other.block_state_gas_used
            && self.blob_gas_used == other.blob_gas_used
            && self.group_state == other.group_state
            && self.history_inherited == other.history_inherited
            && self.block_state.clone().into_bundle() == other.block_state.clone().into_bundle()
    }
}
impl Eq for EezBlockCheckpoint {}
impl EezBlockExecutor<'_> {
    pub fn execution_checkpoint(&self) -> Result<EezBlockCheckpoint, BlockExecutionError> {
        self.validate_prefix()?;
        // Keep this exhaustive: adding local execution state requires a checkpoint decision.
        let Self {
            inner,
            layer: _,
            history: _,
            history_inherited,
            group_state,
        } = self;
        Ok(EezBlockCheckpoint {
            block_state: inner.block_state().clone(),
            receipts: inner.inner.receipts.clone(),
            cumulative_gas_used: inner.inner.cumulative_gas_used,
            block_regular_gas_used: inner.inner.block_regular_gas_used,
            block_state_gas_used: inner.inner.block_state_gas_used,
            blob_gas_used: inner.inner.blob_gas_used,
            group_state: *group_state,
            history_inherited: *history_inherited,
        })
    }
    /// Restore only alongside the corresponding cache, in the same parent/block environment.
    /// Inspectors and state hooks are not copied; the caller must not install them on this path.
    pub fn restore_execution_checkpoint(
        &mut self,
        saved: EezBlockCheckpoint,
    ) -> Result<(), BlockExecutionError> {
        if self.group_state != saved.group_state {
            return Err(BlockExecutionError::msg(
                "prefix restore requires an idle group cursor",
            ));
        }
        self.inner.inner.block_state = saved.block_state;
        self.inner.inner.receipts = saved.receipts;
        self.inner.inner.cumulative_gas_used = saved.cumulative_gas_used;
        self.inner.inner.block_regular_gas_used = saved.block_regular_gas_used;
        self.inner.inner.block_state_gas_used = saved.block_state_gas_used;
        self.inner.inner.blob_gas_used = saved.blob_gas_used;
        self.group_state = saved.group_state;
        self.history_inherited = saved.history_inherited;
        Ok(())
    }

    pub fn block_state(&self) -> &reth_execution_types::BlockState {
        self.inner.block_state()
    }
    pub const fn group_state(&self) -> ExecutionGroupState {
        self.group_state
    }
    pub fn validate_prefix(&self) -> Result<(), BlockExecutionError> {
        if self.layer != ExecutionLayer::Rollup {
            return Err(BlockExecutionError::msg(
                "prefix hashing requires RollupX execution",
            ));
        }
        if !self.history_inherited && self.inner.receipts().is_empty() {
            return Err(BlockExecutionError::msg(
                "empty block PREVRANDAO differs from inherited L1 origin",
            ));
        }
        self.group_state
            .ensure_complete()
            .map_err(BlockExecutionError::msg)
    }
}
impl<'a> BlockExecutor for EezBlockExecutor<'a> {
    type Transaction = EezTxEnvelope;
    type Receipt = Receipt;
    type Evm = crate::EezEvm<'a>;
    type TransactionResultWithState = EezTxResult;
    type BlockAccessList = Bal;
    fn evm(&self) -> &Self::Evm {
        crate::EezEvm::from_ref(self.inner.evm())
    }
    fn evm_mut(&mut self) -> &mut Self::Evm {
        crate::EezEvm::from_mut(self.inner.evm_mut())
    }
    fn set_state_hook(&mut self, hook: impl FnMut(StateUpdate) + Send + 'static) -> bool {
        self.inner.set_state_hook(hook)
    }
    fn convert_block_access_list(list: &BlockAccessList) -> Result<Bal, BlockExecutionError> {
        <EthBlockExecutor<'a, EezEvmTypes, &'a EezReceiptBuilder> as BlockExecutor>::convert_block_access_list(list)
    }
    fn set_block_access_list(&mut self, list: Arc<Bal>) {
        self.inner.set_block_access_list(list);
    }
    fn set_block_access_index(&mut self, index: BlockAccessIndex) {
        self.inner.set_block_access_index(index);
    }
    fn enable_block_access_list_builder(&mut self) {
        self.inner.enable_block_access_list_builder();
    }
    fn take_block_access_list(&mut self) -> Option<BlockAccessList> {
        self.inner.take_block_access_list()
    }
    fn validate_transaction_gas_limit(&mut self, gas: u64) -> Result<(), BlockExecutionError> {
        self.inner.validate_transaction_gas_limit(gas)
    }
    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        if self.layer == ExecutionLayer::L1Simulation {
            return self.inner.apply_pre_execution_changes();
        }
        let ctx = self.inner.context();
        if !ctx.ommers.is_empty() || ctx.withdrawals.as_ref().is_some_and(|w| !w.is_empty()) {
            return Err(BlockExecutionError::msg(
                "RollupX does not admit ommers or beacon withdrawals",
            ));
        }
        if self.inner.evm().spec_id() != SpecId::OSAKA {
            return Err(BlockExecutionError::msg(
                "RollupX execution profile requires Osaka",
            ));
        }
        self.inner
            .evm()
            .ext()
            .collector
            .map_err(BlockExecutionError::other)?;
        let parent_hash = ctx.parent_hash;
        if !self.inner.evm().block().number.is_zero() {
            let out = self
                .inner
                .evm_mut()
                .system_call(SystemTx::new(HISTORY_STORAGE_ADDRESS, parent_hash.0.into()))
                .map_err(BlockExecutionError::other)?
                .detach();
            self.inner.commit_pending_state(&out.pending_state);
        }
        let history = self.history.as_ref().ok_or_else(|| {
            BlockExecutionError::msg("mandatory L1 history evidence source is not configured")
        })?;
        self.history_inherited = crate::history::begin(self.inner.evm_mut(), history)?;
        Ok(())
    }
    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutorTx<Self>,
    ) -> Result<EezTxResult, BlockExecutionError> {
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
            let first = self.inner.receipts().is_empty();
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
                let data = crate::history::validate(self.inner.evm_mut(), history, recovered.tx())?;
                let EezEvmTx::Native { call, .. } = env.inner_mut() else {
                    return Err(BlockExecutionError::msg(
                        "history transaction requires native environment",
                    ));
                };
                call.input = data;
            }
            if eez_primitives::is_native_type(recovered.tx().ty()) {
                let limit = u64::try_from(self.inner.evm().block().gas_limit)
                    .map_err(|_| BlockExecutionError::msg("L2 block gas limit exceeds uint64"))?;
                let mut budget = system_tx_gas_limit(limit, self.inner.inner.cumulative_gas_used);
                if recovered.tx().is_history() {
                    budget = budget.min(eez_protocol::l1_history::IMPORT_GAS_LIMIT);
                }
                let EezEvmTx::Native { call, .. } = env.inner_mut() else {
                    return Err(BlockExecutionError::msg(
                        "native transaction requires native environment",
                    ));
                };
                if recovered.tx().is_history() {
                    call.gas_limit = budget;
                }
                if !recovered.tx().is_history()
                    && (recovered.tx().gas_limit() != budget || call.gas_limit != budget)
                {
                    return Err(BlockExecutionError::msg(
                        "native gas limit differs from its block position",
                    ));
                }
                let mut result = self
                    .inner
                    .evm_mut()
                    .transact(&env)
                    .map_err(BlockExecutionError::other)?
                    .detach();
                if recovered.tx().is_history()
                    && (!result.result.is_success()
                        || !result.result.output.is_empty()
                        || !result.result.logs.is_empty())
                {
                    return Err(BlockExecutionError::msg("history transaction failed"));
                }
                if completes_group {
                    clear_protocol_balances(
                        self.inner.evm_mut().overlay_db_mut(),
                        &mut result.pending_state,
                    )
                    .map_err(|_| {
                        BlockExecutionError::msg("cannot load protocol cleanup accounts")
                    })?;
                }
                return Ok(EezTxResult {
                    group_state: next_group,
                    inner: EthTransactionResultWithState::new(
                        result,
                        recovered.tx().tx_type(),
                        recovered.tx().blob_gas_used().unwrap_or_default(),
                    ),
                });
            }
        }
        let credit = crate::EezEvm::from_ref(self.inner.evm())
            .credit(&env)
            .map_err(evm2::block::map_handler_error)?;
        let executed_env = env.clone();
        let inner = self
            .inner
            .execute_transaction_without_commit((env, recovered))?;
        let (result, ty, blob_gas) = inner.into_parts();
        let mut result =
            crate::reconstruction::finalize(self.inner.evm_mut(), &executed_env, result, credit)
                .map_err(evm2::block::map_handler_error)?;
        if completes_group {
            clear_protocol_balances(
                self.inner.evm_mut().overlay_db_mut(),
                &mut result.pending_state,
            )
            .map_err(|_| BlockExecutionError::msg("cannot load protocol cleanup accounts"))?;
        }
        Ok(EezTxResult {
            inner: EthTransactionResultWithState::new(result, ty, blob_gas),
            group_state: next_group,
        })
    }
    fn commit_transaction(
        &mut self,
        output: EezTxResult,
    ) -> Result<GasOutput, BlockExecutionError> {
        let gas = self.inner.commit_transaction(output.inner)?;
        self.group_state = output.group_state;
        Ok(gas)
    }
    fn receipts(&self) -> &[Receipt] {
        self.inner.receipts()
    }
    fn into_state(self) -> BundleState {
        self.inner.into_state()
    }
    fn finish_with_block_access_list(
        mut self,
    ) -> Result<(BlockExecutionOutput<Receipt>, Option<BlockAccessList>), BlockExecutionError> {
        if self.layer == ExecutionLayer::L1Simulation {
            return self.inner.finish_with_block_access_list();
        }
        self.validate_prefix()?;
        let mut cleanup = PendingState::default();
        clear_protocol_balances(self.inner.evm_mut().overlay_db_mut(), &mut cleanup)
            .map_err(|_| BlockExecutionError::msg("cannot load protocol cleanup accounts"))?;
        // Terminal writes must enter the same accumulator used by witnesses and state roots.
        self.inner.commit_pending_state(&cleanup);
        let bal = self.inner.take_block_access_list();
        let result = reth_execution_types::BlockExecutionResult {
            receipts: std::mem::take(&mut self.inner.inner.receipts),
            requests: Default::default(),
            gas_used: self.inner.inner.cumulative_gas_used,
            blob_gas_used: 0,
        };
        Ok((
            BlockExecutionOutput::new(result, self.inner.into_state()),
            bal,
        ))
    }
}
impl<
    C: reth_chainspec::EthChainSpec<Header = alloy_consensus::Header>
        + reth_chainspec::EthereumHardforks,
> BlockExecutorFactory for EezBlockExecutorFactory<C>
{
    type EvmFactory = EezEvmFactory;
    type EvmTypes = EezEvmTypes;
    type Transaction = EezTxEnvelope;
    type Receipt = Receipt;
    type Evm<'a> = crate::EezEvm<'a>;
    type EvmEnv = EthEvmEnv<EezEvmTypes>;
    type ExecutionCtx<'a>
        = EthBlockExecutionCtx<'a>
    where
        Self: 'a;
    type Executor<'a>
        = EezBlockExecutor<'a>
    where
        Self: 'a;
    fn evm_factory(&self) -> &EezEvmFactory {
        self.ethereum.evm_factory()
    }
    fn evm_with_env<'a, DB: reth_evm::DynDatabase + 'a>(
        &self,
        db: DB,
        env: Self::EvmEnv,
    ) -> Self::Evm<'a> {
        crate::EezEvm::new(self.ethereum.evm_with_env(db, env))
    }
    fn create_executor<'a>(
        &'a self,
        evm: Self::Evm<'a>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Executor<'a>
    where
        Self: 'a,
    {
        EezBlockExecutor {
            inner: self.ethereum.create_executor(evm.into_inner(), ctx),
            layer: self.layer,
            history: self.history.clone(),
            history_inherited: true,
            group_state: Default::default(),
        }
    }
}
#[cfg(test)]
mod tests;
