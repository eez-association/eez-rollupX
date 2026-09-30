//! Rollup block lifecycle: history writes remain, beacon and request calls do not.
//! Transaction execution, receipts and gas accounting delegate to the pinned
//! Ethereum executor, except that native transactions execute with the remaining
//! block gas instead of a reserved declared limit. The inner counters remain accessible to the candidate-local
//! prefix cache, whose checkpoint is always taken before block finalization.

use crate::{EezEvmFactory, EezReceiptBuilder, ensure_supported_transaction};
use alloy_consensus::{Transaction as _, TransactionEnvelope as _};
use alloy_eips::eip2718::Encodable2718 as _;
use alloy_evm::{
    Evm, EvmFactory, FromRecoveredTx, FromTxWithEncoded, RecoveredTx, TransactionEnvMut,
    block::{
        BlockExecutionError, BlockExecutionResult, BlockExecutor, BlockExecutorFactory,
        ExecutableTx, GasOutput, StateDB,
    },
    eth::{EthBlockExecutionCtx, EthBlockExecutor, EthTxResult, spec::EthExecutorSpec},
};
use alloy_primitives::{Address, U256};
use eez_primitives::{
    EEZL2_ADDRESS, EezTxEnvelope, EezTxType, Receipt, SYSTEM_ADDRESS, system_tx_gas_limit,
};
use revm::{Inspector, context::Block as _, primitives::hardfork::SpecId, state::Account};
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
}

impl<C> EezBlockExecutorFactory<C> {
    pub fn new(spec: Arc<C>, layer: ExecutionLayer) -> Self {
        Self {
            spec,
            layer,
            evm: EezEvmFactory,
        }
    }
}

#[derive(Debug)]
pub struct EezBlockExecutor<'a, E, S> {
    inner: EthBlockExecutor<'a, E, S, EezReceiptBuilder>,
    layer: ExecutionLayer,
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

/// Accounts whose ETH is dead by the start of every L2 block and is burned there.
///
/// - `SYSTEM_ADDRESS` receives `EEZL2`'s outbound "burn" transfers and proxy
///   pre-deployment sweeps, and anyone may transfer to it. It cannot spend:
///   ordinary transactions from it are rejected and each native transaction
///   spends only the value minted for it.
/// - `EEZL2` holds value minted for its current execution table. A table is
///   consumable only in the block that loaded it (`lastLoadBlock`), so any
///   balance left at the next block start is unconsumed or reverted value.
pub const BURNED_AT_BLOCK_START: [Address; 2] = [EEZL2_ADDRESS, SYSTEM_ADDRESS];

/// Zero [`BURNED_AT_BLOCK_START`] balances as a pre-execution change.
///
/// This runs before the first transaction, never at finalization: the signer
/// derives prefix block hashes from per-transaction state roots and requires
/// no post-transaction state changes. Pre-execution changes are part of every
/// prefix. Zero or absent accounts are left untouched, so a block without
/// protocol ETH commits no transition for them.
fn burn_protocol_balances<DB: StateDB>(db: &mut DB) -> Result<(), BlockExecutionError> {
    let mut burned = Vec::with_capacity(BURNED_AT_BLOCK_START.len());
    for address in BURNED_AT_BLOCK_START {
        let info = db.basic(address).map_err(|_| {
            BlockExecutionError::msg(format!("cannot load protocol account {address}"))
        })?;
        if let Some(info) = info
            && !info.balance.is_zero()
        {
            let mut account = Account::from(info);
            account.info.balance = U256::ZERO;
            account.mark_touch();
            burned.push((address, account));
        }
    }
    if !burned.is_empty() {
        db.commit_iter(&mut burned.into_iter());
    }
    Ok(())
}

impl<E, S> BlockExecutor for EezBlockExecutor<'_, E, S>
where
    E: Evm<
            DB: StateDB,
            Spec: Into<SpecId> + Clone,
            Tx: FromRecoveredTx<EezTxEnvelope>
                    + FromTxWithEncoded<EezTxEnvelope>
                    + TransactionEnvMut,
        >,
    S: EthExecutorSpec,
{
    type Transaction = EezTxEnvelope;
    type Receipt = Receipt;
    type Evm = E;
    type Result = EthTxResult<E::HaltReason, EezTxType>;

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
        burn_protocol_balances(self.inner.evm.db_mut())
    }

    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<Self::Result, BlockExecutionError> {
        let (mut env, recovered) = tx.into_parts();
        if self.layer == ExecutionLayer::Rollup {
            ensure_supported_transaction(recovered.tx()).map_err(BlockExecutionError::other)?;
            if matches!(recovered.tx(), EezTxEnvelope::System(_)) {
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
                let budget = system_tx_gas_limit(
                    self.inner.evm.block().gas_limit(),
                    self.inner.cumulative_tx_gas_used,
                );
                env.set_gas_limit(budget);
                // The remainder mirrors the pinned inner executor after its
                // declared-limit check.
                let result = self
                    .inner
                    .evm
                    .transact(env)
                    .map_err(|err| BlockExecutionError::evm(err, recovered.tx().trie_hash()))?;
                return Ok(EthTxResult {
                    result,
                    blob_gas_used: recovered.tx().blob_gas_used().unwrap_or_default(),
                    tx_type: recovered.tx().tx_type(),
                });
            }
        }
        self.inner
            .execute_transaction_without_commit((env, recovered))
    }

    fn commit_transaction(&mut self, result: Self::Result) -> GasOutput {
        self.inner.commit_transaction(result)
    }

    fn finish(self) -> Result<(E, BlockExecutionResult<Receipt>), BlockExecutionError> {
        if self.layer == ExecutionLayer::L1Simulation {
            return self.inner.finish();
        }
        // L2 has no consensus layer: no EIP-6110 parsing, EIP-7002/7251 calls,
        // beacon balance increments or DAO transition. EIP-2935 already ran.
        // A future execution fork must explicitly define its L2 lifecycle before
        // the Osaka allowlist above is extended.
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
    type TxExecutionResult = EthTxResult<<EezEvmFactory as EvmFactory>::HaltReason, EezTxType>;
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
        }
    }
}

#[cfg(test)]
mod tests;
