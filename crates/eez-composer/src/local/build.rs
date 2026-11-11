//! Build a Sync block from cross-chain system transactions.
//!
//! Manual block-construction path for recorded history and whole cross-chain
//! groups, without selecting pool transactions. Uses the same reth-evm
//! `BlockBuilder` machinery the Deriver uses to replay L1-derived blocks
//! (`eez_deriver::Deriver::execute_block`).
//!
//! [`SyncBlockState`] exposes the same construction as a LIVE, incrementally
//! extended state: the Sync block under construction is itself the composition
//! session, so a claim read off it is the value the committed block produces.

use alloy_consensus::{Header, Transaction};
use alloy_eips::{Decodable2718, Encodable2718};
use alloy_primitives::{Address, B256, Bytes};
use alloy_rpc_types_engine::ExecutionData;
use eez_driver::{BUILDER_EXTRA_DATA, BUILDER_GAS_LIMIT};
use eez_evm::EezEvmConfig;
use eez_primitives::engine::EezEngineTypes;
use eez_primitives::{Block, EezTxEnvelope as TransactionSigned, SYSTEM_TX_GAS_CAP};
use eez_protocol::system_tx::{ExecutionGroupState, PlannedTransaction};
use reth_chainspec::EthereumHardforks;
use reth_evm::{
    ConfigureEvm, Evm as _, EvmEnvFor, FromRecoveredTx as _, InspectorFor, NextBlockEnvAttributes,
    execute::BlockBuilder,
};
use reth_payload_primitives::PayloadTypes;
use reth_primitives_traits::{
    Recovered, RecoveredBlock, SealedHeader, SignedTransaction, SignerRecoverable,
};
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::{EvmStateProviderAdapter, StateProviderBox, StateProviderFactory};
use revm::database::{CacheState, State};
use revm::inspector::NoOpInspector;
use revm::{
    DatabaseCommit,
    context::result::{EVMError, ExecutionResult, ResultAndState},
    context::{Cfg as _, TxEnv},
};
use std::sync::Arc;
use thiserror::Error;

mod prefix;
pub(crate) use prefix::SyncPrefixHashes;

/// The revm state a Sync block is built over: the parent state provider plus
/// every change committed by the block's txs so far.
pub type DraftDb = State<StateProviderDatabase<EvmStateProviderAdapter<StateProviderBox>>>;

/// Errors raised by [`build_sync_block`].
#[derive(Debug, Error)]
pub enum BuildError {
    /// Provider lookup (state, header) failed.
    #[error("provider: {0}")]
    Provider(String),
    /// EIP-2718 decode failed for the held tx at `idx`.
    #[error("decode tx #{idx}: {msg}")]
    DecodeTx { idx: usize, msg: String },
    /// Signer recovery failed for the held tx at `idx`.
    #[error("recover signer for tx #{idx}")]
    RecoverSigner { idx: usize },
    /// Revm rejected the tx at execution time (gas, balance, …).
    #[error("execute tx #{idx}: {msg}")]
    ExecuteTx { idx: usize, msg: String },
    /// The tx does not fit the block's remaining gas at its position: either an
    /// ordinary tx's declared limit exceeds it, or a native tx's position budget
    /// would change what the tx does compared with the full cap. A capacity
    /// condition of this block, not a fault of the tx; an emptier block may
    /// take it. Nothing was committed.
    #[error("tx #{idx} does not fit the {available} gas left in the block: {reason}")]
    BlockCapacity {
        idx: usize,
        available: u64,
        reason: &'static str,
    },
    /// A requested prefix block-hash checkpoint did not identify a transaction.
    #[error("checkpoint tx position {position} is outside a {tx_count}-transaction Sync block")]
    InvalidCheckpointPosition { position: usize, tx_count: usize },
    /// `reth-evm` block-builder primitive surfaced an error during
    /// setup, pre-execution changes, or `finish`.
    #[error("block builder: {0}")]
    Builder(String),
}

impl BuildError {
    /// Whether local state/prefix evidence is unavailable. These failures have
    /// no validated attribution to the candidate user; retry without eviction.
    #[must_use]
    pub const fn is_provider(&self) -> bool {
        matches!(self, Self::Provider(_))
    }

    /// Whether the tx was refused only for this block's remaining gas, so the
    /// caller must defer it rather than blame it.
    #[must_use]
    pub const fn is_capacity(&self) -> bool {
        matches!(self, Self::BlockCapacity { .. })
    }
}

/// Built Sync-block artifact ready for [`commit_derived`].
///
/// [`commit_derived`]: eez_driver::BlockCommitterHandle::commit_derived
#[derive(Debug)]
pub struct BuiltSyncBlock {
    /// Exact selected L1 hash used by the mandatory import. This fences a
    /// delayed commit against reorgs that leave PREVRANDAO unchanged.
    pub l1_origin_hash: B256,
    /// Engine-API payload — pass to `commit_derived`.
    pub payload: ExecutionData,
    /// Sealed header of the new block (cursor mirror in the committer).
    pub header: SealedHeader<Header>,
    /// The recovered Sync block. Remote-prover mode captures the endpoint witness
    /// from here, since the block isn't committed yet (no store can serve it).
    pub block: RecoveredBlock<Block>,
    /// Receipt-level success per tx, in block order. The composer gates a
    /// dispatch on every system tx and outbound user tx having succeeded — a
    /// reverted delivery means the block's claims contradict its execution.
    pub tx_successes: Vec<bool>,
    /// Cumulative native receipt gas in transaction order, including zero-price
    /// system transactions. Fee observations use differences between receipts,
    /// never the intercepted composition gas or signed gas limits.
    pub tx_cumulative_gas_used: Vec<u64>,
}

/// Fixed-profile attributes for one candidate. The caller captures RANDAO from
/// authenticated canonical history once, before opening any prefix snapshot.
fn next_block_attributes(
    evm_config: &EezEvmConfig,
    timestamp: u64,
    prev_randao: B256,
    suggested_fee_recipient: Address,
) -> NextBlockEnvAttributes {
    let chain_spec = evm_config.chain_spec();
    NextBlockEnvAttributes {
        timestamp,
        suggested_fee_recipient,
        prev_randao,
        gas_limit: BUILDER_GAS_LIMIT,
        parent_beacon_block_root: chain_spec
            .is_cancun_active_at_timestamp(timestamp)
            .then_some(B256::ZERO),
        withdrawals: chain_spec
            .is_shanghai_active_at_timestamp(timestamp)
            .then(alloy_eips::eip4895::Withdrawals::default),
        extra_data: Bytes::from_static(BUILDER_EXTRA_DATA),
        slot_number: None,
    }
}

/// Decode a raw EIP-2718 tx and recover its signer. `idx` is the tx's position
/// in the block, for error reporting.
fn recover_tx(raw: &Bytes, idx: usize) -> Result<Recovered<TransactionSigned>, BuildError> {
    let tx = TransactionSigned::decode_2718_exact(raw).map_err(|e| BuildError::DecodeTx {
        idx,
        msg: e.to_string(),
    })?;
    SignedTransaction::try_into_recovered(tx).map_err(|_| BuildError::RecoverSigner { idx })
}

/// Open the revm state for a block built on `parent_hash`, optionally
/// preloaded with an already-warmed cache.
fn open_draft_db(
    provider: &dyn super::provider::StateSnapshotProvider,
    parent_hash: B256,
    cache: Option<CacheState>,
) -> Result<DraftDb, BuildError> {
    let state_provider = provider
        .state_by_block_hash(parent_hash)
        .map_err(|e| BuildError::Provider(format!("state_by_block_hash({parent_hash}): {e}")))?;
    let mut builder = State::builder()
        .with_database(StateProviderDatabase::new(EvmStateProviderAdapter(
            state_provider,
        )))
        .with_bundle_update();
    if let Some(cache) = cache {
        builder = builder.with_cached_prestate(cache);
    }
    Ok(builder.build())
}

fn freeze_history(config: &EezEvmConfig, timestamp: u64) -> Result<EezEvmConfig, BuildError> {
    let chain = config
        .l1_headers(timestamp)
        .map_err(|e| BuildError::Builder(e.to_string()))?;
    config
        .clone()
        .with_l1_history(Arc::new(eez_evm::history::WitnessHistory(chain)))
        .map_err(|e| BuildError::Builder(e.to_string()))
}

/// Freeze ready evidence and plan the protocol prefix from local parent state.
/// No L1 provider is queried; callers retain this config for the whole candidate.
pub(crate) fn prepare_block_history<P: StateProviderFactory>(
    provider: &P,
    config: &EezEvmConfig,
    parent: &SealedHeader<Header>,
    timestamp: u64,
) -> Result<
    (
        EezEvmConfig,
        eez_protocol::l1_history::HistoryExecution,
        Vec<Bytes>,
    ),
    BuildError,
> {
    let config = freeze_history(config, timestamp)?;
    let state = provider
        .state_by_block_hash(parent.hash())
        .map_err(|e| BuildError::Provider(e.to_string()))?;
    let mut db = StateProviderDatabase::new(EvmStateProviderAdapter(state.as_ref()));
    let (history, transaction) = config
        .prepare_history(&mut db, parent, timestamp)
        .map_err(|e| BuildError::Builder(e.to_string()))?;
    let prefix = transaction
        .into_iter()
        .map(|tx| tx.encoded_2718().into())
        .collect();
    Ok((config, history, prefix))
}

/// Build a new Sync block on top of `parent`, selecting an eligible history
/// update unless `sync_txs` already includes one, then executing in order.
/// Recorded bodies require exact replay to preserve history omission.
///
/// The returned [`BuiltSyncBlock`] is committed via
/// [`BlockCommitterHandle::commit_derived`] — same engine-API tail
/// the Deriver uses for L1-derived blocks (single source of truth for
/// `newPayload` + head-FCU).
///
/// # Errors
///
/// See [`BuildError`].
///
/// [`BlockCommitterHandle::commit_derived`]: eez_driver::BlockCommitterHandle::commit_derived
pub fn build_sync_block<P>(
    l2_provider: &P,
    evm_config: &EezEvmConfig,
    parent: &SealedHeader<Header>,
    timestamp: u64,
    suggested_fee_recipient: Address,
    sync_txs: &[Bytes],
) -> Result<BuiltSyncBlock, BuildError>
where
    P: StateProviderFactory,
{
    let (evm_config, _, prefix) =
        prepare_block_history(l2_provider, evm_config, parent, timestamp)?;
    let includes_history = sync_txs.first().is_some_and(|raw| {
        TransactionSigned::decode_2718_exact(raw).is_ok_and(|tx| tx.is_history())
    });
    let transactions = if includes_history {
        sync_txs.to_vec()
    } else {
        prefix.into_iter().chain(sync_txs.iter().cloned()).collect()
    };
    replay_sync_block(
        l2_provider,
        &evm_config,
        parent,
        timestamp,
        suggested_fee_recipient,
        &transactions,
    )
}

/// Reexecute an exact recorded body, including its choice to omit history.
/// Later L1 evidence can validate an existing update but cannot add one. This
/// also supplies canonical hashes for complete recorded transaction prefixes.
pub(crate) fn replay_sync_block<P: StateProviderFactory>(
    l2_provider: &P,
    evm_config: &EezEvmConfig,
    parent: &SealedHeader<Header>,
    timestamp: u64,
    suggested_fee_recipient: Address,
    transactions: &[Bytes],
) -> Result<BuiltSyncBlock, BuildError> {
    let evm_config = freeze_history(evm_config, timestamp)?;
    let parent_hash = parent.hash();
    let state_provider = l2_provider
        .state_by_block_hash(parent_hash)
        .map_err(|e| BuildError::Provider(format!("state_by_block_hash({parent_hash}): {e}")))?;
    let mut state_db = StateProviderDatabase::new(EvmStateProviderAdapter(state_provider.as_ref()));
    let first = transactions
        .first()
        .map(|raw| TransactionSigned::decode_2718_exact(raw))
        .transpose()
        .map_err(|e| BuildError::Builder(e.to_string()))?;
    let history = evm_config
        .validate_history_input(&mut state_db, parent, timestamp, first.as_ref())
        .map_err(|e| BuildError::Builder(e.to_string()))?;
    let l1_origin_hash = history.origin_hash;
    let prev_randao = history.prev_randao;
    let mut db = State::builder()
        .with_database(state_db)
        .with_bundle_update()
        .build();

    let attributes =
        next_block_attributes(&evm_config, timestamp, prev_randao, suggested_fee_recipient);

    let mut builder = evm_config
        .builder_for_next_block(&mut db, parent, attributes)
        .map_err(|e| BuildError::Builder(format!("builder_for_next_block: {e}")))?;

    builder
        .apply_pre_execution_changes()
        .map_err(|e| BuildError::Builder(format!("apply_pre_execution_changes: {e}")))?;

    for (idx, tx_bytes) in transactions.iter().enumerate() {
        builder
            .execute_transaction(recover_tx(tx_bytes, idx)?)
            .map_err(|e| BuildError::ExecuteTx {
                idx,
                msg: e.to_string(),
            })?;
    }

    let outcome = builder
        .finish(state_provider.as_ref(), None)
        .map_err(|e| BuildError::Builder(format!("finish: {e}")))?;
    let tx_successes = outcome
        .execution_result
        .receipts
        .iter()
        .map(|receipt| receipt.success)
        .collect();
    let tx_cumulative_gas_used = outcome
        .execution_result
        .receipts
        .iter()
        .map(|receipt| receipt.cumulative_gas_used)
        .collect();
    let block = outcome.block;
    let sealed_block = block.sealed_block().clone();
    let header = sealed_block.sealed_header().clone();
    let payload = <EezEngineTypes as PayloadTypes>::block_to_payload(sealed_block, None);
    Ok(BuiltSyncBlock {
        l1_origin_hash,
        payload,
        header,
        block,
        tx_successes,
        tx_cumulative_gas_used,
    })
}

/// Outcome of one transaction executed on a live prefix state.
///
/// A reverted tx is a successful *execution* with `success == false` — only a
/// tx revm refuses outright (nonce, balance, decode) is a [`BuildError`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxOutcome {
    /// Receipt-level status: false for a revert or a halt.
    pub success: bool,
    /// Gas charged to the tx (after refunds), as the receipt reports it.
    pub gas_used: u64,
    /// Pre-refund gas, including the calldata floor, for limit sizing.
    pub gas_required: u64,
    /// Return data if `success`, revert data otherwise; empty if the tx halted
    /// without output.
    pub output: Bytes,
    /// Logs the tx emitted. The composer checks the outbound calls it actually
    /// made against the entries it composed.
    pub logs: Vec<alloy_primitives::Log>,
}

fn exec_outcome<H>(result: &ExecutionResult<H>) -> TxOutcome {
    TxOutcome {
        success: result.is_success(),
        gas_used: result.tx_gas_used(),
        gas_required: result.gas().total_gas_spent().max(result.gas().floor_gas()),
        output: result.output().cloned().unwrap_or_default(),
        logs: result.logs().to_vec(),
    }
}

/// Upper bound of the block gas `txs` can take, as the executor's admission
/// rules see them: a native tx at most its cap, an ordinary tx its declared
/// limit. An undecodable tx counts as unbounded.
#[must_use]
pub fn declared_group_gas(txs: &[Bytes]) -> u64 {
    txs.iter().fold(0u64, |total, raw| {
        let declared =
            TransactionSigned::decode_2718_exact(raw).map_or(u64::MAX, |tx| tx.gas_limit());
        total.saturating_add(declared)
    })
}

/// Admission bound before local plans have their position-specific gas encoded.
pub fn planned_group_gas(txs: &[PlannedTransaction]) -> u64 {
    txs.iter().fold(0u64, |total, tx| {
        total.saturating_add(match tx {
            PlannedTransaction::Native(_) => SYSTEM_TX_GAS_CAP,
            PlannedTransaction::User(raw) => declared_group_gas(std::slice::from_ref(raw)),
        })
    })
}

/// Execute one raw tx against `state` under `evm_env` at a block position with
/// `gas_used` already spent, and commit it: the per-tx half of the block
/// executor (`eez_evm`'s `EezBlockExecutor` transacts and commits the same way;
/// the rest of its work is receipts, which carry no state).
///
/// The executor's gas rules are applied here too, so the draft never holds a
/// tx the canonical rebuild would refuse or execute differently:
/// - an ordinary tx must declare no more than the remaining block gas;
/// - a native tx runs with `system_tx_gas_limit` of the remaining gas.
///
/// A native budget below the cap makes the result depend on how full the block
/// is: an inner call may be starved by the 63/64 rule, or code may read
/// `GAS`. Cross-chain effects must not depend on packing, so a clamped native
/// tx is accepted only if its result and state changes equal those of the same
/// tx under the full cap on the same pre-state; otherwise it is refused with
/// [`BuildError::BlockCapacity`] and nothing is committed. Native txs pay no
/// fees, so no balance depends on the budget and the comparison is exact. The
/// reference run costs one extra execution, only for native txs placed after
/// more than `block gas limit - cap` of gas.
///
/// The caller tracks cumulative block gas off [`TxOutcome::gas_used`] — the
/// same per-tx figure the block executor adds to its own total.
///
/// The uninspected path passes [`NoOpInspector`], the same inspector reth uses
/// when a caller supplies none.
fn execute_and_commit_inspected<I>(
    evm_config: &EezEvmConfig,
    state: &mut DraftDb,
    evm_env: &EvmEnvFor<EezEvmConfig>,
    gas_used: u64,
    group_state: &mut ExecutionGroupState,
    raw: &Bytes,
    idx: usize,
    inspector: I,
) -> Result<TxOutcome, BuildError>
where
    I: for<'db> InspectorFor<EezEvmConfig, &'db mut DraftDb>,
{
    let recovered = recover_tx(raw, idx)?;
    let (next_group, completes_group) =
        group_state
            .after(recovered.inner())
            .map_err(|msg| BuildError::ExecuteTx {
                idx,
                msg: msg.into(),
            })?;
    let block_gas_limit = evm_env.block_env.gas_limit;
    let available = block_gas_limit.saturating_sub(gas_used);
    let tx_env = TxEnv::from_recovered_tx(recovered.inner(), recovered.signer());
    let mut reference = None;
    match recovered.inner() {
        TransactionSigned::History(_) => {
            return Err(BuildError::ExecuteTx {
                idx,
                msg: "history must be validated in the initial block prefix".into(),
            });
        }
        TransactionSigned::System(tx) => {
            let cap = SYSTEM_TX_GAS_CAP;
            let budget = available.min(cap);
            if tx.gas_limit != budget {
                return Err(BuildError::ExecuteTx {
                    idx,
                    msg: "native encoded gas differs from its block position".into(),
                });
            }
            if budget < cap {
                // An error here is the tx's own (shape, value), exactly as in an
                // emptier block; the clamped run below is then not attempted.
                let mut evm = evm_config.evm_with_env(&mut *state, evm_env.clone());
                let mut reference_env = tx_env.clone();
                reference_env.gas_limit = cap;
                reference = Some(evm.transact(reference_env).map_err(|error| match error {
                    EVMError::Database(err) => {
                        BuildError::Provider(format!("execute tx #{idx}: {err}"))
                    }
                    other => BuildError::ExecuteTx {
                        idx,
                        msg: other.to_string(),
                    },
                })?);
            }
        }
        TransactionSigned::Ethereum(tx) => {
            // The executor's Ethereum rule, including Osaka's per-tx cap.
            let charged = tx.gas_limit().min(evm_env.cfg_env.tx_gas_limit_cap());
            if charged > available {
                return Err(BuildError::BlockCapacity {
                    idx,
                    available,
                    reason: "declared gas limit exceeds the remaining block gas",
                });
            }
        }
    }
    let inspector = crate::composer::quote_trace::inspect(
        inspector,
        evm_env.cfg_env.chain_id,
        "native_system",
        &tx_env,
    );
    let mut evm = evm_config.evm_with_env_and_inspector(state, evm_env.clone(), inspector);
    // A database read failure is the store being unavailable, not the tx being
    // invalid: the two classes drive opposite recoveries in the drain.
    let mut result = match (evm.transact(tx_env), reference) {
        (Err(EVMError::Database(err)), _) => {
            return Err(BuildError::Provider(format!("execute tx #{idx}: {err}")));
        }
        // The same tx validated under the cap, so the budget is the only cause.
        (Err(_), Some(_)) => {
            return Err(BuildError::BlockCapacity {
                idx,
                available,
                reason: "native budget is below the transaction's intrinsic gas",
            });
        }
        (Err(other), None) => {
            return Err(BuildError::ExecuteTx {
                idx,
                msg: other.to_string(),
            });
        }
        (Ok(clamped), Some(reference)) if !same_execution(&clamped, &reference) => {
            return Err(BuildError::BlockCapacity {
                idx,
                available,
                reason: "native execution under the remaining gas differs from the cap",
            });
        }
        (Ok(result), _) => result,
    };
    let outcome = exec_outcome(&result.result);
    if completes_group {
        eez_evm::balances::clear_protocol_balances(evm.db_mut(), &mut result.state)
            .map_err(|error| BuildError::Provider(format!("cleanup tx #{idx}: {error}")))?;
    }
    evm.db_mut().commit(result.state);
    *group_state = next_group;
    Ok(outcome)
}

/// Equal status, gas, output, logs and state changes.
fn same_execution<H: PartialEq>(left: &ResultAndState<H>, right: &ResultAndState<H>) -> bool {
    left.result == right.result && left.state == right.state
}

/// Live, incrementally extended execution state of the Sync block under
/// construction: the parent state, the block's env, and every tx appended so
/// far.
///
/// Same parent, same [`next_block_attributes`], same pre-execution changes and
/// same per-tx execution as [`build_sync_block`], so "L2 state at tx k" here is
/// the state the committed block has at tx k. Post-execution changes
/// (withdrawals, balance increments) are NOT applied — those belong to block
/// close, not to mid-block state.
pub struct SyncBlockState {
    provider: Arc<dyn super::provider::StateSnapshotProvider>,
    evm_config: EezEvmConfig,
    parent_hash: B256,
    evm_env: EvmEnvFor<EezEvmConfig>,
    state: DraftDb,
    /// Number of txs applied — the next tx's block position.
    applied: usize,
    /// Cumulative gas of the applied txs, matching the block executor's own
    /// total: the drain needs it to stop before the block overflows.
    gas_used: u64,
    group_state: ExecutionGroupState,
}

impl std::fmt::Debug for SyncBlockState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncBlockState")
            .field("parent_hash", &self.parent_hash)
            .field("applied", &self.applied)
            .field("gas_used", &self.gas_used)
            .finish_non_exhaustive()
    }
}

impl SyncBlockState {
    /// Open the state of a Sync block on `parent` whose first `prefix_txs` have
    /// executed.
    ///
    /// # Errors
    ///
    /// See [`BuildError`].
    pub fn open(
        provider: Arc<dyn super::provider::StateSnapshotProvider>,
        evm_config: &EezEvmConfig,
        parent: &SealedHeader<Header>,
        timestamp: u64,
        suggested_fee_recipient: Address,
        prefix_txs: &[Bytes],
    ) -> Result<Self, BuildError> {
        let parent_hash = parent.hash();
        let mut state = open_draft_db(provider.as_ref(), parent_hash, None)?;
        let (mut history, transaction) = evm_config
            .prepare_history(&mut state, parent, timestamp)
            .map_err(|e| BuildError::Builder(e.to_string()))?;
        let includes_history = prefix_txs.first().is_some_and(|raw| {
            TransactionSigned::decode_2718_exact(raw).is_ok_and(|tx| tx.is_history())
        });
        if includes_history {
            let first = TransactionSigned::decode_2718_exact(&prefix_txs[0])
                .map_err(|e| BuildError::Builder(e.to_string()))?;
            history = evm_config
                .validate_history_input(&mut state, parent, timestamp, Some(&first))
                .map_err(|e| BuildError::Builder(e.to_string()))?;
        }
        let prev_randao = history.prev_randao;
        let transactions: Vec<Bytes> = if includes_history {
            prefix_txs.to_vec()
        } else {
            transaction
                .into_iter()
                .map(|tx| tx.encoded_2718().into())
                .chain(prefix_txs.iter().cloned())
                .collect()
        };

        let attributes =
            next_block_attributes(evm_config, timestamp, prev_randao, suggested_fee_recipient);
        let evm_env = evm_config
            .next_evm_env(parent, &attributes)
            .map_err(|e| BuildError::Builder(format!("next_evm_env: {e}")))?;

        // Drive the prefix through the real block builder, then drop it WITHOUT
        // `finish`: that applies the pre-execution changes exactly once and
        // leaves `state` at the mid-block point appended txs continue from.
        let mut gas_used: u64 = 0;
        let group_state;
        {
            let mut builder = evm_config
                .builder_for_next_block(&mut state, parent, attributes)
                .map_err(|e| BuildError::Builder(format!("builder_for_next_block: {e}")))?;
            builder
                .apply_pre_execution_changes()
                .map_err(|e| BuildError::Builder(format!("apply_pre_execution_changes: {e}")))?;
            for (idx, raw) in transactions.iter().enumerate() {
                // The builder's own per-tx figure, so a reopened prefix reports
                // the gas the block it rebuilds actually holds.
                let gas = builder
                    .execute_transaction(recover_tx(raw, idx)?)
                    .map_err(|e| BuildError::ExecuteTx {
                        idx,
                        msg: e.to_string(),
                    })?;
                gas_used = gas_used.saturating_add(gas.tx_gas_used());
            }
            group_state = builder.executor().group_state();
        }

        Ok(Self {
            provider,
            evm_config: evm_config.clone(),
            parent_hash,
            evm_env,
            state,
            applied: transactions.len(),
            gas_used,
            group_state,
        })
    }

    /// Gas the applied txs have used, the same total the block builder holds.
    /// The block's remaining room is measured against it.
    #[must_use]
    pub const fn gas_used(&self) -> u64 {
        self.gas_used
    }

    /// Base fee used by canonical transactions in this draft's execution environment.
    #[must_use]
    pub const fn base_fee(&self) -> u64 {
        self.evm_env.block_env.basefee
    }

    /// Native environment shared by independent forks of this block prefix.
    pub(crate) fn evm_env(&self) -> &EvmEnvFor<EezEvmConfig> {
        &self.evm_env
    }

    /// Execute an unsigned legacy or type-2 intent on this disposable quote draft.
    /// Signature recovery is inapplicable; nonce, balance, fees, gas and native
    /// execution checks remain enabled. No payload or submission is produced.
    pub(crate) fn preview_unsigned(&mut self, tx: TxEnv) -> Result<TxOutcome, BuildError> {
        if !matches!(tx.tx_type, 0 | 2)
            || tx.gas_limit
                > self
                    .evm_env
                    .block_env
                    .gas_limit
                    .saturating_sub(self.gas_used)
        {
            return Err(BuildError::Builder(
                "invalid unsigned quote gas/type".into(),
            ));
        }
        let inspector = crate::composer::quote_trace::inspect(
            NoOpInspector,
            self.evm_env.cfg_env.chain_id,
            "native_source",
            &tx,
        );
        let tracing = inspector.is_capturing();
        let mut evm = self.evm_config.evm_with_env_and_inspector(
            &mut self.state,
            self.evm_env.clone(),
            inspector,
        );
        // This path normally has no inspector; retain its ordinary execution
        // mode unless the request explicitly asks for call capture.
        evm.set_inspector_enabled(tracing);
        let mut result = evm.transact(tx).map_err(|error| BuildError::ExecuteTx {
            idx: self.applied,
            msg: error.to_string(),
        })?;
        let outcome = exec_outcome(&result.result);
        let (next_group, completes_group) = self.group_state.after_ordinary();
        if completes_group {
            eez_evm::balances::clear_protocol_balances(evm.db_mut(), &mut result.state)
                .map_err(|error| BuildError::Provider(error.to_string()))?;
        }
        evm.db_mut().commit(result.state);
        self.group_state = next_group;
        self.applied += 1;
        self.gas_used = self.gas_used.saturating_add(outcome.gas_used);
        Ok(outcome)
    }

    /// Execute one raw EIP-2718 tx with full real-STF semantics and commit it,
    /// extending the prefix by one.
    ///
    /// # Errors
    ///
    /// See [`BuildError`].
    pub fn execute_tx(&mut self, raw: &Bytes) -> Result<TxOutcome, BuildError> {
        let outcome = execute_and_commit_inspected(
            &self.evm_config,
            &mut self.state,
            &self.evm_env,
            self.gas_used,
            &mut self.group_state,
            raw,
            self.applied,
            NoOpInspector,
        )?;
        self.applied += 1;
        self.gas_used = self.gas_used.saturating_add(outcome.gas_used);
        Ok(outcome)
    }

    /// Resolve only locally constructed plans on an isolated copy of this
    /// exact prefix. Every next native gas limit uses the preceding receipt's
    /// charged gas. A rejected group leaves the production draft untouched.
    pub fn prepare_transactions(
        &mut self,
        plans: &[PlannedTransaction],
        sources: &[eez_protocol::native_source::SourceEntry],
    ) -> Result<Vec<Bytes>, (usize, BuildError)> {
        if plans
            .iter()
            .filter(|plan| matches!(plan, PlannedTransaction::Native(_)))
            .count()
            != sources.len()
        {
            return Err((
                0,
                BuildError::Builder("native source group count differs from plans".into()),
            ));
        }
        let mut sources = sources.iter();
        let mut fork = self.fork().map_err(|error| (0, error))?;
        let mut transactions = Vec::with_capacity(plans.len());
        for (index, plan) in plans.iter().enumerate() {
            let source = match plan {
                PlannedTransaction::Native(_) => sources
                    .next()
                    .ok_or_else(|| {
                        (
                            index,
                            BuildError::Builder("native source group missing".into()),
                        )
                    })?
                    .hash(),
                PlannedTransaction::User(_) => B256::ZERO,
            };
            let raw = fork
                .encode_plan(plan, source)
                .map_err(|error| (index, error))?;
            let outcome = fork
                .execute_committed(&raw)
                .map_err(|error| (index, error))?;
            if !outcome.success {
                return Err((
                    index,
                    BuildError::ExecuteTx {
                        idx: index,
                        msg: "native group execution reverted or halted".into(),
                    },
                ));
            }
            transactions.push(raw);
        }
        Ok(transactions)
    }

    /// Open a throwaway fork of this state: a fresh revm state over the same
    /// parent state provider, preloaded with a clone of this state's cache and
    /// running the same env. Nothing executed on the fork touches the block.
    ///
    /// # Errors
    ///
    /// See [`BuildError`].
    pub fn fork(&mut self) -> Result<SyncBlockFork, BuildError> {
        let state = open_draft_db(
            self.provider.as_ref(),
            self.parent_hash,
            Some(self.state.cache.clone()),
        )?;
        Ok(SyncBlockFork {
            evm_config: self.evm_config.clone(),
            evm_env: self.evm_env.clone(),
            state,
            applied: self.applied,
            gas_used: self.gas_used,
            group_state: self.group_state,
        })
    }
}

/// Restore point of a [`SyncBlockFork`]. The cache holds every state effect a
/// rollback undoes; `applied` and `gas_used` ride along so both rewind too.
#[derive(Clone, Debug)]
pub struct ForkSnapshot {
    cache: CacheState,
    applied: usize,
    gas_used: u64,
    group_state: ExecutionGroupState,
}

/// A throwaway copy of a [`SyncBlockState`] for simulation: probes and source
/// sims run here, and only their accepted effects are appended to the real
/// block.
pub struct SyncBlockFork {
    evm_config: EezEvmConfig,
    evm_env: EvmEnvFor<EezEvmConfig>,
    state: DraftDb,
    applied: usize,
    gas_used: u64,
    group_state: ExecutionGroupState,
}

impl std::fmt::Debug for SyncBlockFork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncBlockFork")
            .field("applied", &self.applied)
            .field("gas_used", &self.gas_used)
            .finish_non_exhaustive()
    }
}

impl SyncBlockFork {
    /// Encode a construction plan at the current fork position; raw user bytes
    /// pass through exactly and received native bodies never enter this API.
    pub(crate) fn encode_plan(
        &self,
        plan: &PlannedTransaction,
        source_hash: B256,
    ) -> Result<Bytes, BuildError> {
        plan.at_position(self.evm_env.block_env.gas_limit, self.gas_used, source_hash)
            .map_err(BuildError::Builder)
    }
    pub(crate) fn remaining_gas(&self) -> u64 {
        self.evm_env
            .block_env
            .gas_limit
            .saturating_sub(self.gas_used)
    }

    /// Chain id carried by the block environment backing this fork.
    #[must_use]
    pub fn chain_id(&self) -> u64 {
        self.evm_env.cfg_env.chain_id
    }

    /// Environment for synthetic composition frames. These frames are not
    /// transactions in the block, so fee-market, sender, nonce, and block-gas
    /// validation do not apply; the canonical system transaction is executed
    /// separately before the effect is accepted.
    fn simulation_env(&self) -> EvmEnvFor<EezEvmConfig> {
        let mut env = self.evm_env.clone();
        super::session::disable_checks(&mut env);
        env
    }

    /// Execute one raw EIP-2718 tx on the fork and commit it there.
    ///
    /// # Errors
    ///
    /// See [`BuildError`].
    #[cfg(test)]
    pub fn execute_tx(&mut self, raw: &Bytes) -> Result<TxOutcome, BuildError> {
        self.execute_committed(raw)
    }

    pub(crate) fn execute_committed(&mut self, raw: &Bytes) -> Result<TxOutcome, BuildError> {
        self.execute_committed_inspected(raw, NoOpInspector)
    }

    pub(crate) fn execute_committed_inspected<I>(
        &mut self,
        raw: &Bytes,
        inspector: I,
    ) -> Result<TxOutcome, BuildError>
    where
        I: for<'db> InspectorFor<EezEvmConfig, &'db mut DraftDb>,
    {
        let outcome = execute_and_commit_inspected(
            &self.evm_config,
            &mut self.state,
            &self.evm_env,
            self.gas_used,
            &mut self.group_state,
            raw,
            self.applied,
            inspector,
        )?;
        self.applied += 1;
        self.gas_used = self.gas_used.saturating_add(outcome.gas_used);
        Ok(outcome)
    }

    /// Execute an unsigned ordinary transaction on disposable native state.
    /// Unlike a synthetic target call this advances nonce, fees, transaction
    /// position and cumulative gas exactly as a signed user transaction does.
    pub(crate) fn execute_unsigned<I>(
        &mut self,
        tx_env: TxEnv,
        inspector: I,
    ) -> Result<TxOutcome, BuildError>
    where
        I: for<'db> InspectorFor<EezEvmConfig, &'db mut DraftDb>,
    {
        if eez_primitives::is_native_type(tx_env.tx_type) {
            return Err(BuildError::Builder(
                "unsigned source must be ordinary".into(),
            ));
        }
        let available = self
            .evm_env
            .block_env
            .gas_limit
            .saturating_sub(self.gas_used);
        if tx_env.gas_limit > available
            || tx_env.gas_limit > self.evm_env.cfg_env.tx_gas_limit_cap()
        {
            return Err(BuildError::BlockCapacity {
                idx: self.applied,
                available,
                reason: "unsigned user gas exceeds transaction or remaining block limit",
            });
        }
        let mut env = self.evm_env.clone();
        // eth_estimateGas permits omitted fee fields (gas price zero). Keep
        // nonce, value, sender and balance checks, and normal fee accounting.
        env.cfg_env.disable_base_fee = true;
        let inspector = crate::composer::quote_trace::inspect(
            inspector,
            env.cfg_env.chain_id,
            "native_source",
            &tx_env,
        );
        let mut evm = self
            .evm_config
            .evm_with_env_and_inspector(&mut self.state, env, inspector);
        let mut result = evm.transact(tx_env).map_err(|error| match error {
            EVMError::Database(error) => BuildError::Provider(error.to_string()),
            other => BuildError::ExecuteTx {
                idx: self.applied,
                msg: other.to_string(),
            },
        })?;
        let outcome = exec_outcome(&result.result);
        let (next_group, completes_group) = self.group_state.after_ordinary();
        if completes_group {
            eez_evm::balances::clear_protocol_balances(evm.db_mut(), &mut result.state)
                .map_err(|error| BuildError::Provider(error.to_string()))?;
        }
        evm.db_mut().commit(result.state);
        self.group_state = next_group;
        self.applied += 1;
        self.gas_used = self.gas_used.saturating_add(outcome.gas_used);
        Ok(outcome)
    }

    /// Execute a synthetic target-call frame on this fork under an inspector.
    ///
    /// This is composition simulation, not a block transaction: it commits the
    /// target effects to the fork but does not advance the block transaction
    /// index. The eventual canonical system transaction replays the same effect
    /// on the real draft after materialization.
    pub fn execute_call_inspected<I>(
        &mut self,
        tx_env: revm::context::TxEnv,
        inspector: I,
    ) -> Result<TxOutcome, BuildError>
    where
        I: for<'db> InspectorFor<EezEvmConfig, &'db mut DraftDb>,
    {
        let caller = tx_env.caller;
        let env = self.simulation_env();
        let inspector = crate::composer::quote_trace::inspect(
            inspector,
            env.cfg_env.chain_id,
            "composition",
            &tx_env,
        );
        let mut evm = self
            .evm_config
            .evm_with_env_and_inspector(&mut self.state, env, inspector);
        let mut result = evm
            .transact_funded_call(tx_env)
            .map_err(|error| match error {
                EVMError::Database(error) => {
                    BuildError::Provider(format!("execute synthetic target call: {error}"))
                }
                other => BuildError::ExecuteTx {
                    idx: self.applied,
                    msg: format!("synthetic target call: {other}"),
                },
            })?;
        let outcome = exec_outcome(&result.result);
        super::reset_frame_caller_nonce(&mut result.state, caller);
        evm.db_mut().commit(result.state);
        Ok(outcome)
    }

    /// Execute a synthetic read-only target frame without committing its
    /// transaction state. The inspector forces the first frame static, so
    /// state-changing opcodes fail exactly as they do under STATICCALL.
    pub fn execute_static_call_inspected<I>(
        &mut self,
        tx_env: revm::context::TxEnv,
        inspector: I,
    ) -> Result<TxOutcome, BuildError>
    where
        I: for<'db> InspectorFor<EezEvmConfig, &'db mut DraftDb>,
    {
        let env = self.simulation_env();
        let inspector = crate::composer::quote_trace::inspect(
            inspector,
            env.cfg_env.chain_id,
            "composition",
            &tx_env,
        );
        let mut evm = self
            .evm_config
            .evm_with_env_and_inspector(&mut self.state, env, inspector);
        let result = evm.transact(tx_env).map_err(|error| match error {
            EVMError::Database(error) => {
                BuildError::Provider(format!("execute synthetic static target call: {error}"))
            }
            other => BuildError::ExecuteTx {
                idx: self.applied,
                msg: format!("synthetic static target call: {other}"),
            },
        })?;
        Ok(exec_outcome(&result.result))
    }

    /// Execute a read-only synthetic manager frame and return its output.
    pub fn view_call(&mut self, tx_env: revm::context::TxEnv) -> Result<Bytes, BuildError> {
        let env = self.simulation_env();
        let mut evm = self.evm_config.evm_with_env(&mut self.state, env);
        let result = evm.transact(tx_env).map_err(|error| match error {
            EVMError::Database(error) => {
                BuildError::Provider(format!("execute synthetic view call: {error}"))
            }
            other => BuildError::ExecuteTx {
                idx: self.applied,
                msg: format!("synthetic view call: {other}"),
            },
        })?;
        if !result.result.is_success() {
            return Err(BuildError::ExecuteTx {
                idx: self.applied,
                msg: format!("synthetic view call reverted: {:?}", result.result.output()),
            });
        }
        Ok(result.result.output().cloned().unwrap_or_default())
    }

    /// Restore point for composition rollback.
    pub fn snapshot(&self) -> ForkSnapshot {
        ForkSnapshot {
            cache: self.state.cache.clone(),
            applied: self.applied,
            gas_used: self.gas_used,
            group_state: self.group_state,
        }
    }

    /// Roll back to a [`Self::snapshot`].
    pub fn restore(&mut self, snapshot: ForkSnapshot) {
        self.state.cache = snapshot.cache;
        self.applied = snapshot.applied;
        self.gas_used = snapshot.gas_used;
        self.group_state = snapshot.group_state;
    }

    /// Raw state + env, for callers that drive their own EVM (a source
    /// simulation threading its inspector through this fork).
    pub fn state_and_env(&mut self) -> (&mut DraftDb, &EvmEnvFor<EezEvmConfig>) {
        (&mut self.state, &self.evm_env)
    }
}

/// Per-effect intermediate L2 block hashes — the canonical header hash after
/// each cross-chain effect's transaction group, in transaction order.
///
/// The prover requires each settlement entry's `newRoot` ABI field to equal its
/// effect-prefix block hash (not necessarily the final Sync-block hash), so the
/// composer fills it with these values. Each value is computed by rebuilding
/// the Sync block over the corresponding prefix of `sync_txs`.
///
/// # Errors
///
/// See [`BuildError`]. A sync tx that fails to decode is treated as a non-system
/// tx (pair-end), matching the prover's fail-safe flagging.
pub fn sync_block_pair_hashes<P>(
    l2_provider: &P,
    evm_config: &EezEvmConfig,
    parent: &SealedHeader<Header>,
    timestamp: u64,
    suggested_fee_recipient: Address,
    sync_txs: &[Bytes],
    system_address: Address,
    eezl2_address: Address,
) -> Result<Vec<B256>, BuildError>
where
    P: StateProviderFactory,
{
    // Per-tx system flags must match the proof signer's classification so
    // pair-end positions agree on both sides.
    let flags: Vec<bool> = sync_txs
        .iter()
        .map(|raw| {
            let Ok(tx) = TransactionSigned::decode_2718_exact(raw) else {
                return false;
            };
            let to = tx.to();
            !tx.is_history()
                && tx.recover_signer().is_ok_and(|signer| {
                    eez_protocol::settlement::is_system_tx(
                        alloy_consensus::Typed2718::ty(&tx),
                        signer,
                        to,
                        system_address,
                        eezl2_address,
                    )
                })
        })
        .collect();

    let positions = eez_protocol::settlement::pair_end_positions(&flags);
    sync_block_hashes_at_positions(
        l2_provider,
        evm_config,
        parent,
        timestamp,
        suggested_fee_recipient,
        sync_txs,
        &positions,
    )
}

/// Canonical block hashes after explicit inclusive transaction positions.
///
/// Positions refer to the exact recorded body, including any history update.
/// Each must end a complete group; a native load alone cannot be a checkpoint.
pub fn sync_block_hashes_at_positions<P>(
    l2_provider: &P,
    evm_config: &EezEvmConfig,
    parent: &SealedHeader<Header>,
    timestamp: u64,
    suggested_fee_recipient: Address,
    sync_txs: &[Bytes],
    positions: &[usize],
) -> Result<Vec<B256>, BuildError>
where
    P: StateProviderFactory,
{
    if let Some(&position) = positions
        .iter()
        .find(|&&position| position >= sync_txs.len())
    {
        return Err(BuildError::InvalidCheckpointPosition {
            position,
            tx_count: sync_txs.len(),
        });
    }
    positions
        .iter()
        .map(|&p| {
            replay_sync_block(
                l2_provider,
                evm_config,
                parent,
                timestamp,
                suggested_fee_recipient,
                &sync_txs[..=p],
            )
            .map(|prefix| prefix.header.hash())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{BlockHeader as _, TxLegacy};
    use alloy_network::TxSignerSync;
    use alloy_primitives::{TxKind, U256, address, bytes};
    use alloy_signer_local::PrivateKeySigner;
    use reth_chainspec::ChainSpecBuilder;
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};

    const TIMESTAMP: u64 = 1_012;
    const FEE_RECIPIENT: Address = address!("0000000000000000000000000000000000000f00");
    const BOB: Address = address!("000000000000000000000000000000000000b0b0");
    const STORE: Address = address!("00000000000000000000000000000000005107e0");
    const REVERTER: Address = address!("000000000000000000000000000000000000bad0");

    /// Everything the two execution paths need: one provider, one env, one
    /// tx list.
    struct Fixture {
        provider: MockEthProvider,
        evm_config: EezEvmConfig,
        parent: SealedHeader<Header>,
        txs: Vec<Bytes>,
    }

    fn test_signer() -> PrivateKeySigner {
        PrivateKeySigner::from_bytes(&B256::with_last_byte(7)).expect("test key")
    }

    fn sign_tx(nonce: u64, to: Address, value: U256, gas_limit: u64) -> Bytes {
        let mut tx = TxLegacy {
            chain_id: Some(1),
            nonce,
            gas_price: 1,
            gas_limit,
            to: TxKind::Call(to),
            value,
            input: Bytes::new(),
        };
        let sig = test_signer()
            .sign_transaction_sync(&mut tx)
            .expect("sign test tx");
        let signed = reth_ethereum_primitives::TransactionSigned::new_unhashed(
            reth_ethereum_primitives::Transaction::Legacy(tx),
            sig,
        );
        let mut buf = Vec::new();
        signed.encode_2718(&mut buf);
        Bytes::from(buf)
    }

    fn rollup_config() -> EezEvmConfig {
        use eez_protocol::l1_history::{
            HISTORY_ADDRESS, HeaderChain, HeaderRules, WitnessHistory, genesis_storage,
        };
        let reference = Header {
            timestamp: 964,
            ..Default::default()
        };
        let endpoint = Header {
            number: 1,
            // A real available update exercises history in every prefix. An
            // unchanged endpoint no longer creates a synthetic empty import.
            timestamp: 988,
            parent_hash: reference.hash_slow(),
            ..Default::default()
        };
        let mut genesis: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../../genesis.json")).unwrap();
        genesis["timestamp"] = serde_json::json!("0x3dc"); // 988: parent 1 at 1000, child 2 at 1012.
        genesis["config"]["chainId"] = serde_json::json!(1);
        // A private L1: mainnet references require the 2-second, six-position schedule.
        genesis["config"]["rollupX"]["l1Reference"]["chainId"] = serde_json::json!(1337);
        genesis["config"]["rollupX"]["blockIntervalSeconds"] = serde_json::json!(12);
        genesis["config"]["rollupX"]["syncIntervalBlocks"] = serde_json::json!(2);
        genesis["config"]["rollupX"]["historyUpdateOffset"] = serde_json::json!(1);
        genesis["config"]["rollupX"]["l1Reference"]["timestamp"] = serde_json::json!(964);
        genesis["config"]["rollupX"]["l1Reference"]["hash"] =
            serde_json::json!(reference.hash_slow());
        genesis["alloc"][format!("{HISTORY_ADDRESS:#x}")]["storage"] =
            serde_json::to_value(genesis_storage()).unwrap();
        let mut chain = HeaderChain::default();
        chain
            .append(
                &[
                    alloy_rlp::encode(reference).into(),
                    alloy_rlp::encode(endpoint).into(),
                ],
                &HeaderRules {
                    london_block: None,
                    shanghai_time: None,
                    cancun_time: None,
                    prague_time: None,
                    unsupported_time: None,
                },
                2,
            )
            .unwrap();
        EezEvmConfig::new(Arc::new(
            eez_evm::profile::chain_spec(serde_json::from_value(genesis).unwrap()).unwrap(),
        ))
        .with_l1_history(Arc::new(WitnessHistory(Arc::new(chain))))
        .unwrap()
    }

    fn fixture() -> Fixture {
        let evm_config = rollup_config();

        let provider: MockEthProvider = MockEthProvider::new();
        for address in [
            eez_protocol::l1_history::HISTORY_ADDRESS,
            eez_protocol::l1_history::HISTORY_IMPLEMENTATION_ADDRESS,
            eez_protocol::l1_history::HISTORY_ADMIN_ADDRESS,
        ] {
            let account = &evm_config.chain_spec().genesis().alloc[&address];
            provider.add_account(
                address,
                ExtendedAccount::new(account.nonce.unwrap(), U256::ZERO)
                    .with_bytecode(account.code.clone().unwrap())
                    .extend_storage(
                        account
                            .storage
                            .iter()
                            .flatten()
                            .map(|(key, value)| (*key, U256::from_be_bytes(value.0))),
                    ),
            );
        }
        provider.add_account(
            test_signer().address(),
            ExtendedAccount::new(0, U256::from(1_000_000_000_000_000_000_u128)),
        );
        // `SSTORE 1 -> slot 0`, then return the 32-byte word 42: the write makes
        // a repeat call cheap, so gas alone shows whether the prefix chained.
        provider.add_account(
            STORE,
            ExtendedAccount::new(0, U256::ZERO)
                .with_bytecode(bytes!("6001600055602a60005260206000f3")),
        );
        // Revert with `0xdeadbeef`.
        provider.add_account(
            REVERTER,
            ExtendedAccount::new(0, U256::ZERO).with_bytecode(bytes!("63deadbeef6000526004601cfd")),
        );

        let parent = SealedHeader::seal_slow(Header {
            number: 1,
            timestamp: 1_000,
            gas_limit: BUILDER_GAS_LIMIT,
            base_fee_per_gas: Some(0),
            excess_blob_gas: Some(0),
            blob_gas_used: Some(0),
            ommers_hash: alloy_consensus::EMPTY_OMMER_ROOT_HASH,
            withdrawals_root: Some(alloy_consensus::EMPTY_ROOT_HASH),
            parent_beacon_block_root: Some(B256::ZERO),
            requests_hash: Some(alloy_eips::eip7685::EMPTY_REQUESTS_HASH),
            ..Default::default()
        });

        let txs = vec![
            sign_tx(0, BOB, U256::from(1), 21_000),
            sign_tx(1, STORE, U256::ZERO, 100_000),
            sign_tx(2, REVERTER, U256::ZERO, 100_000),
            sign_tx(3, STORE, U256::ZERO, 100_000),
            sign_tx(4, BOB, U256::from(1), 21_000),
        ];

        Fixture {
            provider,
            evm_config,
            parent,
            txs,
        }
    }

    fn parent_with_base_fee(base_fee_per_gas: u64) -> SealedHeader<Header> {
        SealedHeader::seal_slow(Header {
            number: 1,
            timestamp: 1_000,
            gas_limit: BUILDER_GAS_LIMIT,
            base_fee_per_gas: Some(base_fee_per_gas),
            excess_blob_gas: Some(0),
            blob_gas_used: Some(0),
            ommers_hash: alloy_consensus::EMPTY_OMMER_ROOT_HASH,
            withdrawals_root: Some(alloy_consensus::EMPTY_ROOT_HASH),
            parent_beacon_block_root: Some(B256::ZERO),
            requests_hash: Some(alloy_eips::eip7685::EMPTY_REQUESTS_HASH),
            ..Default::default()
        })
    }

    impl Fixture {
        fn history_prefix(&self) -> Vec<Bytes> {
            prepare_block_history(&self.provider, &self.evm_config, &self.parent, TIMESTAMP)
                .unwrap()
                .2
        }

        fn history_gas(&self) -> u64 {
            self.build(&[]).header.gas_used()
        }

        fn build(&self, txs: &[Bytes]) -> BuiltSyncBlock {
            build_sync_block(
                &self.provider,
                &self.evm_config,
                &self.parent,
                TIMESTAMP,
                FEE_RECIPIENT,
                txs,
            )
            .expect("build sync block")
        }

        fn open(&self, prefix: &[Bytes]) -> SyncBlockState {
            SyncBlockState::open(
                Arc::new(self.provider.clone()),
                &self.evm_config,
                &self.parent,
                TIMESTAMP,
                FEE_RECIPIENT,
                prefix,
            )
            .expect("open prefix state")
        }

        /// Gas charged to each tx INSIDE `build_sync_block`, read off the header
        /// of every tx prefix — the arbiter the block itself agrees with.
        fn builder_gas(&self) -> Vec<u64> {
            let cumulative: Vec<u64> = (0..=self.txs.len())
                .map(|k| self.build(&self.txs[..k]).header.gas_used())
                .collect();
            cumulative.windows(2).map(|w| w[1] - w[0]).collect()
        }
    }

    #[test]
    fn recorded_history_omission_survives_late_evidence() {
        let f = fixture();
        let available = f.evm_config.l1_headers(TIMESTAMP).unwrap();
        let unavailable = f
            .evm_config
            .clone()
            .with_l1_history(Arc::new(eez_evm::history::WitnessHistory(Arc::new(
                available.range(0, 0).unwrap(),
            ))))
            .unwrap();
        // This fixture's child is both a Sync boundary and an eligible update.
        // Test empty and nonempty recorded bodies after the evidence changes.
        for transactions in [&[][..], &f.txs[..2]] {
            let omitted = build_sync_block(
                &f.provider,
                &unavailable,
                &f.parent,
                TIMESTAMP,
                FEE_RECIPIENT,
                transactions,
            )
            .unwrap();
            assert_eq!(omitted.block.body().transactions.len(), transactions.len());
            assert_eq!(omitted.header.mix_hash, f.parent.mix_hash);

            let rebuilt = replay_sync_block(
                &f.provider,
                &f.evm_config,
                &f.parent,
                TIMESTAMP,
                FEE_RECIPIENT,
                transactions,
            )
            .unwrap();
            assert_eq!(rebuilt.block.body(), omitted.block.body());
            assert_eq!(rebuilt.header.hash(), omitted.header.hash());
            assert_eq!(rebuilt.header.gas_used(), omitted.header.gas_used());
            assert_eq!(rebuilt.l1_origin_hash, omitted.l1_origin_hash);

            let fresh = f.build(transactions);
            assert!(fresh.block.body().transactions[0].is_history());
            assert_ne!(fresh.header.hash(), omitted.header.hash());
            let fresh_body = fresh
                .block
                .body()
                .transactions
                .iter()
                .map(|tx| tx.encoded_2718().into())
                .collect::<Vec<_>>();
            assert_eq!(
                replay_sync_block(
                    &f.provider,
                    &f.evm_config,
                    &f.parent,
                    TIMESTAMP,
                    FEE_RECIPIENT,
                    &fresh_body,
                )
                .unwrap()
                .header
                .hash(),
                fresh.header.hash(),
            );
            if !transactions.is_empty() {
                assert_eq!(
                    sync_block_hashes_at_positions(
                        &f.provider,
                        &f.evm_config,
                        &f.parent,
                        TIMESTAMP,
                        FEE_RECIPIENT,
                        transactions,
                        &[transactions.len() - 1],
                    )
                    .unwrap(),
                    vec![omitted.header.hash()],
                );
            }
        }
    }

    #[test]
    fn synthetic_fork_frames_ignore_transaction_fee_validation() {
        let mut f = fixture();
        f.parent = parent_with_base_fee(1_000);
        let mut block = f.open(&[]);
        let mut fork = block.fork().expect("fork block prefix");
        let tx_env = revm::context::TxEnv {
            caller: Address::ZERO,
            gas_limit: 100_000,
            kind: TxKind::Call(STORE),
            ..Default::default()
        };

        let output = fork
            .view_call(tx_env.clone())
            .expect("synthetic view is not a fee-paying transaction");
        assert_eq!(U256::from_be_slice(&output), U256::from(42));

        let outcome = fork
            .execute_call_inspected(tx_env, NoOpInspector)
            .expect("synthetic target call is not a fee-paying transaction");
        assert!(outcome.success);
        assert_eq!(U256::from_be_slice(&outcome.output), U256::from(42));
    }

    #[test]
    fn synthetic_fork_deliveries_preserve_proxy_balance_nonce_and_source_rollback() {
        use revm::Database;
        let mut f = fixture();
        f.parent = parent_with_base_fee(1_000);
        f.provider.add_account(
            BOB,
            ExtendedAccount::new(1, U256::from(13)).with_bytecode(bytes!("00")),
        );
        let mut block = f.open(&[]);
        let mut fork = block.fork().unwrap();
        let before = fork.snapshot();
        let tx = TxEnv {
            caller: BOB,
            gas_limit: 100_000,
            kind: TxKind::Call(STORE),
            value: U256::from(7),
            ..Default::default()
        };
        for expected_balance in [7, 14] {
            assert!(
                fork.execute_call_inspected(tx.clone(), NoOpInspector)
                    .unwrap()
                    .success
            );
            let proxy = fork.state.basic(BOB).unwrap().unwrap();
            assert_eq!(proxy.balance, U256::from(13));
            assert_eq!(proxy.nonce, 1);
            assert_eq!(
                fork.state.basic(STORE).unwrap().unwrap().balance,
                U256::from(expected_balance)
            );
        }
        assert!(
            !fork
                .execute_call_inspected(
                    TxEnv {
                        kind: TxKind::Call(REVERTER),
                        ..tx.clone()
                    },
                    NoOpInspector
                )
                .unwrap()
                .success
        );
        assert_eq!(
            fork.state.basic(BOB).unwrap().unwrap().balance,
            U256::from(13)
        );
        assert_eq!(
            fork.state.basic(REVERTER).unwrap().unwrap().balance,
            U256::ZERO
        );
        // A source-frame revert also rewinds earlier successful deliveries.
        fork.restore(before);
        assert_eq!(
            fork.state.basic(STORE).unwrap().unwrap().balance,
            U256::ZERO
        );
        assert_eq!(
            fork.state.basic(BOB).unwrap().unwrap().balance,
            U256::from(13)
        );
        assert!(
            fork.execute_call_inspected(tx, NoOpInspector)
                .unwrap()
                .success
        );
        assert_eq!(
            fork.state.basic(STORE).unwrap().unwrap().balance,
            U256::from(7)
        );
        assert_eq!(
            fork.state
                .basic(FEE_RECIPIENT)
                .unwrap()
                .map(|account| account.balance)
                .unwrap_or_default(),
            U256::ZERO
        );
    }

    /// THE equivalence guard: appending txs to a live [`SyncBlockState`]
    /// executes them exactly as `build_sync_block`'s block builder does. Gas is
    /// the witness — it is state-dependent (the second `SSTORE` is ~20k cheaper
    /// only if the first one's write is visible), so equal per-tx gas plus
    /// equal receipt status means both paths ran the same STF over the same
    /// intermediate states.
    #[test]
    fn prefix_state_execution_matches_build_sync_block() {
        let f = fixture();
        let full = f.build(&f.txs);
        assert_eq!(
            full.tx_successes,
            vec![true, true, true, false, true, true],
            "receipt statuses, including the reverting tx",
        );
        let builder_gas = f.builder_gas();
        let mut cumulative = 0_u64;
        assert_eq!(
            full.tx_cumulative_gas_used,
            std::iter::once(f.history_gas())
                .chain(builder_gas.iter().copied())
                .map(|gas| {
                    cumulative += gas;
                    cumulative
                })
                .collect::<Vec<_>>(),
            "retained native receipt gas agrees with independently built prefixes",
        );
        // Ground truth, so the comparisons below can't pass on empty numbers.
        assert_eq!(builder_gas[0], 21_000, "plain transfer");

        let mut prefix = f.open(&[]);
        let outcomes: Vec<TxOutcome> = f
            .txs
            .iter()
            .map(|raw| prefix.execute_tx(raw).expect("execute on prefix"))
            .collect();

        for (idx, outcome) in outcomes.iter().enumerate() {
            assert_eq!(
                outcome.success,
                full.tx_successes[idx + 1],
                "tx #{idx} status disagrees with the built block",
            );
            assert_eq!(
                outcome.gas_used, builder_gas[idx],
                "tx #{idx} gas disagrees with the built block",
            );
        }

        // A revert is an outcome, not an error, and carries its data.
        assert_eq!(outcomes[2].output, bytes!("deadbeef"));
        assert_eq!(U256::from_be_slice(&outcomes[1].output), U256::from(42));
        // Non-vacuous: the repeat SSTORE is cheap ONLY because the first one is
        // visible, so this comparison fails if `open` restarts from the parent.
        assert!(
            outcomes[1].gas_used > outcomes[3].gas_used + 15_000,
            "prefix must chain: {} vs {}",
            outcomes[1].gas_used,
            outcomes[3].gas_used,
        );
    }

    /// `open(prefix)` lands on the state the block builder holds at that tx:
    /// executing tx k over prefix `[0..k]` reproduces tx k's in-block outcome
    /// for every k — builder-executed prefix, `transact`-executed tail.
    #[test]
    fn prefix_open_matches_the_same_position_in_the_block() {
        let f = fixture();
        let full = f.build(&f.txs);
        let builder_gas = f.builder_gas();

        for (k, raw) in f.txs.iter().enumerate() {
            let outcome = f.open(&f.txs[..k]).execute_tx(raw).expect("execute tx k");
            assert_eq!(outcome.success, full.tx_successes[k + 1], "tx #{k} status");
            assert_eq!(outcome.gas_used, builder_gas[k], "tx #{k} gas");
        }
    }

    #[test]
    fn unsigned_quote_replay_matches_signed_receipts_and_enforces_source_checks() {
        use alloy_consensus::{TxEip1559, TypedTransaction};
        use revm::Database;
        let f = fixture();
        // GASPRICE is state-visible: a proposed fee must actually be executed.
        f.provider.add_account(
            STORE,
            ExtendedAccount::new(0, U256::ZERO).with_bytecode(bytes!("3a60005560006000f3")),
        );
        for legacy in [false, true] {
            for tip in [1_u128, 99] {
                for to in [STORE, REVERTER] {
                    let mut tx: TypedTransaction = if legacy {
                        TxLegacy {
                            chain_id: Some(1),
                            nonce: 0,
                            gas_limit: 100_000,
                            gas_price: tip,
                            to: TxKind::Call(to),
                            ..Default::default()
                        }
                        .into()
                    } else {
                        TxEip1559 {
                            chain_id: 1,
                            nonce: 0,
                            gas_limit: 100_000,
                            max_priority_fee_per_gas: tip,
                            max_fee_per_gas: tip,
                            to: TxKind::Call(to),
                            ..Default::default()
                        }
                        .into()
                    };
                    let sig = test_signer().sign_transaction_sync(&mut tx).unwrap();
                    let raw = Bytes::from(tx.into_envelope(sig).encoded_2718());
                    let envelope = TransactionSigned::decode_2718_exact(&raw).unwrap();
                    let env = f
                        .evm_config
                        .tx_env(Recovered::new_unchecked(envelope, test_signer().address()));
                    let mut preview = f.open(&[]);
                    let actual = preview.preview_unsigned(env.clone()).unwrap();
                    let signed = f.build(&[raw]);
                    assert_eq!(actual.gas_used + f.history_gas(), signed.header.gas_used);
                    assert_eq!(actual.success, signed.tx_successes[1]);
                    assert_eq!(
                        preview.state.storage(STORE, U256::ZERO).unwrap(),
                        if to == STORE {
                            U256::from(tip)
                        } else {
                            U256::ZERO
                        }
                    );
                    assert!(
                        preview.preview_unsigned(env.clone()).is_err(),
                        "nonce cannot be spent twice"
                    );
                    let mut unfunded = env.clone();
                    unfunded.caller = BOB;
                    assert!(
                        f.open(&[]).preview_unsigned(unfunded).is_err(),
                        "normal balance checks remain active"
                    );
                    let mut gapped = env;
                    gapped.nonce = 1;
                    assert!(
                        f.open(&[]).preview_unsigned(gapped).is_err(),
                        "normal nonce checks remain active"
                    );
                    assert_eq!(
                        f.open(&[])
                            .state
                            .basic(test_signer().address())
                            .unwrap()
                            .unwrap()
                            .nonce,
                        0,
                        "preview never changes provider state"
                    );
                }
            }
        }
    }

    /// A fork sees the prefix but never writes back to it, and `restore` is a
    /// real restore point — for the cache and for the tx position.
    #[test]
    fn fork_is_isolated_and_snapshot_restore_rewinds() {
        let f = fixture();
        let builder_gas = f.builder_gas();
        let mut prefix = f.open(&f.txs[..1]);

        let mut fork = prefix.fork().expect("fork");
        let on_fork = fork.execute_tx(&f.txs[1]).expect("execute on fork");
        assert_eq!(on_fork.gas_used, builder_gas[1]);

        // The fork's write never reached the block: the same tx still applies
        // to the prefix, with the same result.
        let on_block = prefix.execute_tx(&f.txs[1]).expect("execute on prefix");
        assert_eq!(on_block, on_fork, "fork must not advance the block");

        let snapshot = fork.snapshot();
        let applied_before = fork.applied;
        let first = fork.execute_tx(&f.txs[2]).expect("probe run");
        fork.restore(snapshot);
        assert_eq!(
            fork.applied, applied_before,
            "restore must rewind the tx position with the cache",
        );
        let second = fork.execute_tx(&f.txs[2]).expect("replay after restore");
        assert_eq!(first, second, "restore must rewind the probe's effects");
        assert!(!first.success);

        // Non-vacuous: without the rewind the nonce is spent and revm rejects
        // the replay outright.
        assert!(
            fork.execute_tx(&f.txs[2]).is_err(),
            "a spent nonce must be rejected — the restore above is what makes \
             the replay legal",
        );
    }

    #[test]
    fn unsigned_user_matches_signed_execution_and_spends_nonce() {
        use revm::Database as _;
        let f = fixture();
        for (index, raw) in f.txs.iter().enumerate() {
            let mut signed_draft = f.open(&f.txs[..index]);
            let mut unsigned_draft = f.open(&f.txs[..index]);
            let mut signed = signed_draft.fork().unwrap();
            let mut unsigned = unsigned_draft.fork().unwrap();
            let decoded = TransactionSigned::decode_2718_exact(raw).unwrap();
            let sender = decoded.recover_signer().unwrap();
            let recovered = Recovered::new_unchecked(decoded, sender);
            let tx = f.evm_config.tx_env(&recovered);
            let expected = signed.execute_committed(raw).unwrap();
            let actual = unsigned
                .execute_unsigned(tx.clone(), NoOpInspector)
                .unwrap();
            assert_eq!(actual, expected, "source transaction {index}");
            assert_eq!(unsigned.applied, signed.applied);
            assert_eq!(unsigned.gas_used, signed.gas_used);
            assert_eq!(
                unsigned.state.basic(sender).unwrap(),
                signed.state.basic(sender).unwrap()
            );
            assert!(
                unsigned.execute_unsigned(tx, NoOpInspector).is_err(),
                "spent nonce must remain spent, including after a revert"
            );
        }
    }

    #[test]
    fn repeated_probe_forks_match_native_setup_and_user_execution() {
        let f = fixture();
        let builder_gas = f.builder_gas();
        for (prefix_len, &setup_gas) in builder_gas.iter().enumerate().take(f.txs.len() - 1) {
            let mut prepared = f.open(&f.txs[..prefix_len]);
            let prefix_gas = prepared.gas_used();
            let raw = &f.txs[prefix_len + 1];
            let decoded = TransactionSigned::decode_2718_exact(raw).unwrap();
            let sender = decoded.recover_signer().unwrap();
            let tx = f
                .evm_config
                .tx_env(Recovered::new_unchecked(decoded, sender));
            let mut expected = f.open(&f.txs[..=prefix_len]);
            let outcome = expected.execute_tx(raw).unwrap();
            for _ in 0..2 {
                let mut probe = prepared.fork().unwrap();
                let setup = probe.execute_committed(&f.txs[prefix_len]).unwrap();
                assert_eq!(setup.gas_used, setup_gas);
                assert_eq!(
                    probe.execute_unsigned(tx.clone(), NoOpInspector).unwrap(),
                    outcome
                );
                assert_eq!(probe.gas_used, expected.gas_used());
                assert_eq!(probe.applied, prefix_len + 3);
                assert!(probe.execute_unsigned(tx.clone(), NoOpInspector).is_err());
            }
            // A fresh probe must inherit only the original prefix, including
            // when a previous setup/user reverted or consumed its nonce.
            assert_eq!(prepared.gas_used(), prefix_gas);
            assert_eq!(
                prepared.execute_tx(&f.txs[prefix_len]).unwrap().gas_used,
                setup_gas
            );
        }
    }

    #[test]
    fn explicit_hash_positions_reject_out_of_bounds_without_panicking() {
        let f = fixture();
        let error = sync_block_hashes_at_positions(
            &f.provider,
            &f.evm_config,
            &f.parent,
            TIMESTAMP,
            FEE_RECIPIENT,
            &f.txs[..1],
            &[1],
        )
        .expect_err("position one is outside a one-transaction block");
        assert!(matches!(
            error,
            BuildError::InvalidCheckpointPosition {
                position: 1,
                tx_count: 1,
            }
        ));
    }

    #[test]
    fn sync_builder_rejects_l1_execution_profiles() {
        for config in [
            EezEvmConfig::for_l1_simulation(Arc::new(
                ChainSpecBuilder::mainnet().cancun_activated().build(),
            )),
            EezEvmConfig::for_l1_simulation(Arc::new(
                ChainSpecBuilder::mainnet().prague_activated().build(),
            )),
        ] {
            let f = fixture();
            let error = build_sync_block(
                &f.provider,
                &config,
                &f.parent,
                TIMESTAMP,
                FEE_RECIPIENT,
                &f.txs,
            )
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("L1 history source is not configured")
            );
        }
    }

    #[test]
    fn prefix_checkpoint_matches_independent_rebuild_with_history_and_reverts() {
        let f = fixture();
        let transactions = f
            .history_prefix()
            .into_iter()
            .chain(f.txs.iter().cloned())
            .collect::<Vec<_>>();
        // Exercise receipt bloom and log ordering as well as SSTORE and a
        // reverted transaction. MockEthProvider has no real trie; compare
        // complete pre-finalization state separately from header equality.
        f.provider.add_account(
            STORE,
            ExtendedAccount::new(0, U256::ZERO)
                .with_bytecode(bytes!("6001600055602a60005260206000a060206000f3")),
        );
        // System-call counters detect applying pre-block changes twice or
        // retaining post-block mutations in the next checkpoint.
        for address in [
            alloy_eips::eip4788::BEACON_ROOTS_ADDRESS,
            alloy_eips::eip2935::HISTORY_STORAGE_ADDRESS,
            alloy_eips::eip7002::WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
            alloy_eips::eip7251::CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
        ] {
            f.provider.add_account(
                address,
                ExtendedAccount::new(0, U256::ZERO).with_bytecode(bytes!("60005460010160005500")),
            );
        }
        let mut hashes = SyncPrefixHashes::new(
            &f.provider,
            &f.evm_config,
            &f.parent,
            TIMESTAMP,
            B256::ZERO,
            FEE_RECIPIENT,
        );
        for p in 0..transactions.len() {
            let actual = hashes.at_positions(&transactions[..=p], &[p]).unwrap();
            assert_eq!(actual, vec![f.build(&transactions[..=p]).header.hash()]);
            hashes.assert_state_matches_rebuild();
        }
        assert_eq!(hashes.executed_transactions, transactions.len());
    }

    #[test]
    fn prefix_hashing_does_not_burn_donations_in_the_continuing_draft() {
        use revm::Database as _;
        let f = fixture();
        // Record SYSTEM.balance so premature finalization changes storage.
        let mut code = vec![0x73];
        code.extend_from_slice(eez_primitives::SYSTEM_ADDRESS.as_slice());
        code.extend_from_slice(&[0x31, 0x5f, 0x55, 0x00]);
        f.provider.add_account(
            STORE,
            ExtendedAccount::new(1, U256::ZERO).with_bytecode(code.into()),
        );
        let transactions: Vec<_> = f
            .history_prefix()
            .into_iter()
            .chain([
                sign_tx(0, eez_primitives::SYSTEM_ADDRESS, U256::from(17), 21_000),
                sign_tx(1, STORE, U256::ZERO, 100_000),
            ])
            .collect();
        let mut hashes = SyncPrefixHashes::new(
            &f.provider,
            &f.evm_config,
            &f.parent,
            TIMESTAMP,
            B256::ZERO,
            FEE_RECIPIENT,
        );
        for count in 1..=transactions.len() {
            assert_eq!(
                hashes.prefix(&transactions[..count]).unwrap(),
                f.build(&transactions[..count]).header.hash()
            );
            hashes.assert_state_matches_rebuild();
        }
        let mut draft = f.open(&transactions);
        assert_eq!(
            draft
                .state
                .basic(eez_primitives::SYSTEM_ADDRESS)
                .unwrap()
                .unwrap()
                .balance,
            U256::from(17)
        );
        assert_eq!(
            draft.state.storage(STORE, U256::ZERO).unwrap(),
            U256::from(17)
        );
        assert_eq!(hashes.executed_transactions, transactions.len());
    }

    #[test]
    fn prefix_checkpoint_discards_rejected_suffix_and_recovers_after_errors() {
        let f = fixture();
        let transactions = f
            .history_prefix()
            .into_iter()
            .chain(f.txs.iter().cloned())
            .collect::<Vec<_>>();
        let mut hashes = SyncPrefixHashes::new(
            &f.provider,
            &f.evm_config,
            &f.parent,
            TIMESTAMP,
            B256::ZERO,
            FEE_RECIPIENT,
        );
        hashes.at_positions(&transactions, &[2, 4]).unwrap();
        let mut replacement = transactions[..3].to_vec();
        replacement.push(sign_tx(2, BOB, U256::from(17), 21_000));
        assert_eq!(
            hashes.at_positions(&replacement, &[3]).unwrap(),
            vec![f.build(&replacement).header.hash()]
        );
        hashes.assert_state_matches_rebuild();
        // Nonmonotonic and repeated positions preserve the existing helper's
        // behavior, while a changed/shorter prefix never inherits later writes.
        let positions = [5, 0, 0, 3];
        assert_eq!(
            hashes.at_positions(&transactions, &positions).unwrap(),
            sync_block_hashes_at_positions(
                &f.provider,
                &f.evm_config,
                &f.parent,
                TIMESTAMP,
                FEE_RECIPIENT,
                &transactions,
                &positions
            )
            .unwrap()
        );
        hashes.assert_state_matches_rebuild();
        assert!(matches!(
            hashes.at_positions(&transactions, &[transactions.len()]),
            Err(BuildError::InvalidCheckpointPosition { .. })
        ));
        let mut malformed = transactions[..3].to_vec();
        malformed.push(bytes!("ff"));
        assert!(matches!(
            hashes.at_positions(&malformed, &[2, 3]),
            Err(BuildError::DecodeTx { idx: 3, .. })
        ));
        let mut invalid_nonce = transactions[..3].to_vec();
        invalid_nonce.push(transactions[1].clone());
        assert!(matches!(
            hashes.at_positions(&invalid_nonce, &[3]),
            Err(BuildError::ExecuteTx { idx: 3, .. })
        ));
        assert_eq!(
            hashes.at_positions(&transactions, &[5]).unwrap(),
            vec![f.build(&transactions).header.hash()]
        );
        hashes.assert_state_matches_rebuild();
    }

    #[test]
    fn prefix_checkpoint_executes_each_transaction_once_in_growing_candidate() {
        let f = fixture();
        let txs: Vec<_> = f
            .history_prefix()
            .into_iter()
            .chain((0..64).map(|nonce| sign_tx(nonce, STORE, U256::ZERO, 100_000)))
            .collect();
        let started = std::time::Instant::now();
        let expected = sync_block_hashes_at_positions(
            &f.provider,
            &f.evm_config,
            &f.parent,
            TIMESTAMP,
            FEE_RECIPIENT,
            &txs,
            &(0..65).collect::<Vec<_>>(),
        )
        .unwrap();
        let baseline = started.elapsed();
        let mut hashes = SyncPrefixHashes::new(
            &f.provider,
            &f.evm_config,
            &f.parent,
            TIMESTAMP,
            B256::ZERO,
            FEE_RECIPIENT,
        );
        let started = std::time::Instant::now();
        let actual: Vec<_> = (0..65)
            .map(|p| hashes.at_positions(&txs[..=p], &[p]).unwrap()[0])
            .collect();
        let candidate = started.elapsed();
        assert_eq!(actual, expected);
        assert_eq!(hashes.executed_transactions, 65);
        hashes.assert_state_matches_rebuild();
        eprintln!(
            "65-prefix comparison: baseline={baseline:?}, candidate={candidate:?}; EVM transactions 2145 -> 65; mock provider, no trie performance coverage"
        );
    }

    /// Block gas is the drain's accept budget, so a fork must start from the
    /// block's total and a rollback must give it back.
    #[test]
    fn fork_inherits_and_restores_gas_used() {
        let f = fixture();
        let builder_gas = f.builder_gas();

        let mut prefix = f.open(&f.txs[..2]);
        let inherited = f.history_gas() + builder_gas[0] + builder_gas[1];
        assert_eq!(
            prefix.gas_used(),
            inherited,
            "a reopened prefix reports the gas the block builder charged",
        );

        let mut fork = prefix.fork().expect("fork");
        assert_eq!(
            fork.gas_used, inherited,
            "a fork starts at the block's total"
        );

        let snapshot = fork.snapshot();
        fork.execute_tx(&f.txs[2]).expect("probe run");
        assert_eq!(fork.gas_used, inherited + builder_gas[2]);
        fork.restore(snapshot);
        assert_eq!(
            fork.gas_used, inherited,
            "restore must rewind the gas with the cache",
        );

        // The fork's spend never reached the block.
        assert_eq!(prefix.gas_used(), inherited);
        prefix.execute_tx(&f.txs[2]).expect("execute on prefix");
        assert_eq!(prefix.gas_used(), inherited + builder_gas[2]);
    }

    /// A reverted tx still burns its gas in the block, so the drain must count
    /// it — `txs[2]` reverts and is charged all the same.
    #[test]
    fn reverted_txs_are_charged_to_the_block() {
        let f = fixture();
        let mut prefix = f.open(&[]);
        for raw in &f.txs {
            prefix.execute_tx(raw).expect("execute on prefix");
        }
        assert_eq!(
            prefix.gas_used(),
            f.build(&f.txs).header.gas_used(),
            "the live prefix and the built block must agree on block gas",
        );
    }

    const LOOP: Address = address!("0000000000000000000000000000000000001009");

    /// The fixture plus an EEZL2 stand-in running `eezl2_code`, and a looping
    /// contract an ordinary tx can burn a chosen amount of block gas in.
    fn native_fixture(eezl2_code: Bytes) -> Fixture {
        let f = fixture();
        f.provider.add_account(
            eez_primitives::EEZL2_ADDRESS,
            ExtendedAccount::new(0, U256::ZERO).with_bytecode(eezl2_code),
        );
        f.provider.add_account(
            LOOP,
            ExtendedAccount::new(0, U256::ZERO).with_bytecode(bytes!("5b600056")),
        );
        f
    }

    fn native_sources(count: u64) -> Vec<eez_protocol::native_source::SourceEntry> {
        let context =
            eez_protocol::native_source::LiveSourceContext::new(1, B256::repeat_byte(0x81))
                .unwrap();
        (0..count).map(|index| context.entry(index)).collect()
    }

    fn native_plan() -> PlannedTransaction {
        let entry =
            eez_protocol::entries::InboundSidecar::new(eez_protocol::entries::IncomingEntry {
                target: BOB,
                source: STORE,
                value: U256::ZERO,
                data: Bytes::new(),
                source_rollup_id: eez_protocol::RollupId::MAINNET,
                l2_rollup_id: eez_protocol::RollupId(1),
                return_data: Bytes::new(),
                success: true,
            })
            .unwrap();
        let plan = eez_protocol::system_tx::build_inbound_system_txs(
            &[entry],
            &eez_protocol::system_tx::SystemTxContext {
                eezl2_address: eez_primitives::EEZL2_ADDRESS,
                l2_chain_id: 1,
                this_rollup_id: 1,
            },
        )
        .unwrap()
        .remove(0);
        PlannedTransaction::Native(plan)
    }

    fn native(gas_used: u64) -> Bytes {
        native_plan()
            .at_position(BUILDER_GAS_LIMIT, gas_used, B256::ZERO)
            .unwrap()
    }

    /// A 16M ordinary burn leaves 14M, below the native cap.
    fn heavy_prefix() -> Vec<Bytes> {
        vec![sign_tx(0, LOOP, U256::ZERO, 16_000_000)]
    }

    #[test]
    fn group_cleanup_is_identical_in_forks_unsigned_execution_and_prefix_rebuilds() {
        use revm::Database as _;
        let f = native_fixture(bytes!("00"));
        let entry = eez_protocol::abi::L2ExecutionEntrySol {
            incomingCalls: vec![eez_protocol::abi::CrossChainCallSol {
                revertNextNCalls: 0,
                isStatic: false,
                gas: 0,
                sourceAddress: STORE,
                sourceRollupId: 0,
                targetAddress: BOB,
                value: U256::from(37),
                data: Bytes::new(),
            }],
            ..Default::default()
        };
        let plan = eez_protocol::system_tx::build_outbound_load_table_txs(
            &[entry],
            &eez_protocol::system_tx::SystemTxContext {
                eezl2_address: eez_primitives::EEZL2_ADDRESS,
                l2_chain_id: 1,
                this_rollup_id: 1,
            },
        )
        .unwrap()
        .remove(0);
        let mut draft = f.open(&[]);
        let raw = PlannedTransaction::Native(plan)
            .at_position(BUILDER_GAS_LIMIT, draft.gas_used(), B256::ZERO)
            .unwrap();
        let user = sign_tx(0, eez_primitives::SYSTEM_ADDRESS, U256::from(11), 21_000);
        draft.execute_tx(&raw).unwrap();
        assert_eq!(
            draft
                .state
                .basic(eez_primitives::EEZL2_ADDRESS)
                .unwrap()
                .unwrap()
                .balance,
            U256::from(37)
        );
        let mut fork = draft.fork().unwrap();
        let snapshot = fork.snapshot();
        fork.execute_committed(&user).unwrap();
        assert_eq!(
            fork.state
                .basic(eez_primitives::EEZL2_ADDRESS)
                .unwrap()
                .unwrap()
                .balance,
            U256::ZERO
        );
        let signed_state = fork.state.cache.clone();
        fork.restore(snapshot);
        assert_eq!(
            fork.state
                .basic(eez_primitives::EEZL2_ADDRESS)
                .unwrap()
                .unwrap()
                .balance,
            U256::from(37)
        );
        let decoded = TransactionSigned::decode_2718_exact(&user).unwrap();
        let sender = decoded.recover_signer().unwrap();
        let tx = f
            .evm_config
            .tx_env(Recovered::new_unchecked(decoded, sender));
        fork.execute_unsigned(tx, NoOpInspector).unwrap();
        assert_eq!(fork.state.cache, signed_state);
        assert!(
            draft.group_state.ensure_complete().is_err(),
            "fork must not consume the real draft's load"
        );
        draft.execute_tx(&user).unwrap();
        assert_eq!(draft.state.cache, signed_state);
        let transactions: Vec<_> = f.history_prefix().into_iter().chain([raw, user]).collect();
        let mut hashes = SyncPrefixHashes::new(
            &f.provider,
            &f.evm_config,
            &f.parent,
            TIMESTAMP,
            B256::ZERO,
            FEE_RECIPIENT,
        );
        let actual = hashes.at_positions(&transactions, &[0, 2]).unwrap();
        assert_eq!(actual[1], f.build(&transactions).header.hash());
        hashes.assert_state_matches_rebuild();
        assert_eq!(draft.gas_used(), f.build(&transactions).header.gas_used());
    }

    #[test]
    fn clamped_native_tx_with_budget_independent_execution_matches_the_built_block() {
        // STOP: the same result under any budget above intrinsic gas.
        let f = native_fixture(bytes!("00"));
        let mut txs = heavy_prefix();
        let mut draft = f.open(&[]);
        let burned = draft.execute_tx(&txs[0]).unwrap();
        assert_eq!(burned.gas_used, 16_000_000);
        // Ethereum's declared-limit rule would refuse the cap here; the native
        // budget is the 14M remainder instead.
        assert!(BUILDER_GAS_LIMIT - burned.gas_used < SYSTEM_TX_GAS_CAP);
        let prepared = draft
            .prepare_transactions(&[native_plan()], &native_sources(1))
            .unwrap();
        txs.extend(prepared);
        let delivered = draft.execute_tx(&txs[1]).unwrap();
        assert!(delivered.success);
        let built = f.build(&txs);
        assert_eq!(built.tx_successes, vec![true, false, true]);
        assert_eq!(
            built.tx_cumulative_gas_used,
            vec![
                f.history_gas(),
                f.history_gas() + burned.gas_used,
                draft.gas_used()
            ]
        );
        assert_eq!(draft.gas_used(), built.header.gas_used());
    }

    #[test]
    fn clamped_native_tx_whose_result_depends_on_the_budget_is_a_capacity_refusal() {
        // An infinite loop halts at whatever budget it gets; `GAS; SSTORE`
        // succeeds but stores the budget. Both would commit a packing-dependent
        // effect, which consensus permits but the Composer must not choose.
        for code in [bytes!("5b600056"), bytes!("5a60005500")] {
            let f = native_fixture(code.clone());
            let prefix = heavy_prefix();
            let mut draft = f.open(&prefix);
            let before = draft.gas_used();

            let refused = draft
                .prepare_transactions(&[native_plan()], &native_sources(1))
                .unwrap_err();
            assert_eq!(refused.0, 0);
            assert!(refused.1.is_capacity(), "{code}: {:?}", refused.1);
            let error = draft.execute_tx(&native(draft.gas_used())).unwrap_err();
            assert!(error.is_capacity(), "{code}: {error:?}");
            // Nothing committed: the same draft still takes other work.
            assert_eq!(draft.gas_used(), before);
            assert!(
                draft
                    .execute_tx(&sign_tx(1, BOB, U256::from(1), 21_000))
                    .is_ok()
            );

            // An emptier block gives the full cap, so the same tx is accepted.
            let mut empty = f.open(&[]);
            assert!(
                empty.execute_tx(&native(empty.gas_used())).is_ok(),
                "{code}"
            );
        }
    }

    #[test]
    fn ordinary_tx_declaring_more_than_the_remaining_gas_is_a_capacity_refusal() {
        let f = native_fixture(bytes!("00"));
        let mut draft = f.open(&heavy_prefix());
        let error = draft
            .execute_tx(&sign_tx(
                1,
                BOB,
                U256::from(1),
                BUILDER_GAS_LIMIT - draft.gas_used() + 1,
            ))
            .unwrap_err();
        assert!(error.is_capacity(), "{error:?}");
        assert!(
            draft
                .execute_tx(&sign_tx(
                    1,
                    BOB,
                    U256::from(1),
                    BUILDER_GAS_LIMIT - draft.gas_used()
                ))
                .is_ok()
        );
    }

    #[test]
    fn received_native_gas_is_rejected_instead_of_normalized() {
        let f = native_fixture(bytes!("00"));
        let mut draft = f.open(&heavy_prefix());
        let before = draft.gas_used();
        let wrong = native(0);
        let error = draft.execute_tx(&wrong).unwrap_err();
        assert!(!error.is_capacity());
        assert!(error.to_string().contains("encoded gas differs"));
        assert_eq!(draft.gas_used(), before);
        let raw = draft
            .prepare_transactions(&[native_plan()], &native_sources(1))
            .unwrap()
            .remove(0);
        assert_ne!(raw, wrong);
        assert!(draft.execute_tx(&raw).unwrap().success);
    }

    #[test]
    fn native_groups_are_probed_on_actual_use_not_on_their_caps() {
        // Two deliveries together declare 2 x 16.7M > 30M but use ~21k each.
        let f = native_fixture(bytes!("00"));
        let plans = [native_plan(), native_plan()];
        assert!(planned_group_gas(&plans) > BUILDER_GAS_LIMIT);
        let mut draft = f.open(&[]);
        let group = draft
            .prepare_transactions(&plans, &native_sources(2))
            .unwrap();
        // The probe ran on a fork: the draft itself is untouched.
        assert_eq!(draft.gas_used(), f.history_gas());
        for raw in &group {
            assert!(draft.execute_tx(raw).unwrap().success);
        }
        assert_eq!(draft.gas_used(), f.build(&group).header.gas_used());
    }

    #[test]
    fn source_groups_keep_original_ordinals_without_mutating_the_draft() {
        let f = native_fixture(bytes!("00"));
        let mut draft = f.open(&[]);
        let before = draft.gas_used();
        let plans = [native_plan(), native_plan()];
        let sources = native_sources(3);
        for wrong in [&sources[..1], &[], &sources[..]] {
            let (_, error) = draft.prepare_transactions(&plans, wrong).unwrap_err();
            assert!(error.to_string().contains("source group count"));
            assert_eq!(draft.gas_used(), before);
        }
        // A selected suffix retains its full-block ordinal instead of starting at zero.
        let group = draft.prepare_transactions(&plans, &sources[1..]).unwrap();
        assert_eq!(draft.gas_used(), before, "resolution is speculative");
        for (raw, source) in group.iter().zip(&sources[1..]) {
            let TransactionSigned::System(native) =
                TransactionSigned::decode_2718_exact(raw).unwrap()
            else {
                panic!("native delivery")
            };
            assert_eq!(native.source_hash, source.hash());
            draft.execute_tx(raw).unwrap();
        }
        assert_eq!(draft.gas_used(), f.build(&group).header.gas_used());
    }

    #[test]
    fn prepared_inbound_groups_reuse_earlier_effect_checkpoint_hashes() {
        let f = native_fixture(bytes!("00"));
        let mut complete = f.history_prefix();
        let mut draft = f.open(&[]);
        let context =
            eez_protocol::native_source::LiveSourceContext::new(1, B256::repeat_byte(0x81))
                .unwrap();
        let mut hashes = SyncPrefixHashes::new(
            &f.provider,
            &f.evm_config,
            &f.parent,
            TIMESTAMP,
            B256::ZERO,
            FEE_RECIPIENT,
        );
        for first_group in [0, 3, 6] {
            let plans = [native_plan(), native_plan(), native_plan()];
            let sources =
                [first_group, first_group + 1, first_group + 2].map(|index| context.entry(index));
            let deliveries = draft.prepare_transactions(&plans, &sources).unwrap();
            let positions = (complete.len()..complete.len() + deliveries.len()).collect::<Vec<_>>();
            complete.extend(deliveries.iter().cloned());
            // Production asks for every effect hash immediately after preparing
            // a bracket. The first requested prefix precedes the latest cache.
            let actual = hashes.at_positions(&complete, &positions).unwrap();
            for (&position, hash) in positions.iter().zip(actual) {
                assert_eq!(hash, f.build(&complete[..=position]).header.hash());
            }
            for delivery in deliveries {
                draft.execute_tx(&delivery).unwrap();
            }
            hashes.assert_state_matches_rebuild();
            assert_eq!(
                hashes.executed_transactions,
                complete.len(),
                "preparation followed by effect hashing must execute each transaction once"
            );
        }
        assert_eq!(draft.gas_used(), f.build(&complete).header.gas_used());
    }
}
