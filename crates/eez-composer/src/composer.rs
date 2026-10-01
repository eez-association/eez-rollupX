//! [`Composer`]: the umbrella that owns each rollup's cross-chain
//! produce → prove → submit path.
//!
//! Two entry points:
//!
//! - `compose_sync_slot` — called by the Sequencer on each Sync slot.
//!   Processes a bounded batch from the rollup's [`HeldPool`](crate::HeldPool),
//!   simulates eligible transactions, and returns a Sync block for optimistic
//!   L2 commit. Inbound survivors accompany `postBatch` in the L1 bundle;
//!   outbound survivors execute in the L2 Sync block and are carried in DA. A
//!   background observer records the settlement verdict for later slot-context
//!   recovery (see [`crate::optimistic`]).
//! - [`Composer::run`] — follows the shared `L1Watcher` event stream,
//!   logging confirmed vs external `BatchPosted` for this rollup. The
//!   L1-confirmed cursor + batch index are advanced by the Deriver (sole
//!   writer of [`L1CanonicalHead`](eez_l1::L1CanonicalHead)).
//!
//! Each batch is proved by the shared [`Prover`] and sent via the shared
//! [`Submitter`] bundle relay.

use std::collections::HashMap;
use std::sync::Arc;

use alloy_eips::Encodable2718;
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::Provider as _;
use async_trait::async_trait;
use eez_driver::{
    BUILDER_GAS_LIMIT, BlockCommitterHandle, ParentContext, RollupTiming, SyncSlotBlock,
    SyncSlotComposer, SyncSlotMode,
    observation::{BlockObservation, ObservedOutcome},
    witness::{ExecutionWitnessMode, block_observation_outcomes, block_witness_with_observations},
};
use eez_evm::EezEvmConfig;
use eez_l1::{BundleTarget, L1Event, SendOutcome, Submitter};
use eez_primitives::engine::EezEngineTypes;
use eez_prover::{ActionableProverFailure, BlockWitness, Prover, ProverError, ProvingContext};
use reth_primitives_traits::{AlloyBlockHeader, Block, BlockBody};
use reth_storage_api::{
    BlockReader, BlockSource, StateProvider, StateProviderFactory, TransactionsProvider,
};
use tokio::sync::broadcast;
use tracing::{Level, event};

use crate::held_pool::{HeldPool, HeldTx};
use crate::ingress::Direction;
use crate::local::{
    BuildError, InboundL2TargetSession, L1SlotState, L1TargetSession,
    build::{SyncBlockState, SyncPrefixHashes, declared_group_gas},
    build_sync_block,
};
use crate::optimistic::AttemptId;
use crate::optimistic::OptimisticallyIncluded;
use crate::prover_retry::{
    actionable_held_tx, observation_held_tx, partition_retryable, prove_with_retry_at,
    validate_actionable_prover_failure,
};
use crate::rollup::RollupState;
use crate::settlement_policy::{PolicyConfig, PolicyError};
use eez_protocol::settlement_manifest::{ManifestInput, ProposedUser, SettlementManifest};

mod admission;
mod config;
mod costs;
mod gas;
pub(crate) mod gas_trace;
mod policy;
pub use policy::{SettlementPolicyObservation, SettlementStatus};
mod posting;
mod selection;
mod sequencing;
pub use config::ComposerSettings;

/// Runtime config for the cross-chain execution path on Sync slots.
/// Carried inside [`CrossChainWiring`] next to the wired
/// cross-chain simulation: the keys and addresses needed to construct and sign
/// canonical L2 system transactions from composition batch entries.
///
/// Owned by `eez-node` at startup and shared via `Arc` because the
/// L1 submission resources are shared across candidate builds.
#[derive(Clone)]
pub struct CrossChainExecCtx {
    /// `EEZL2` address, where SYSTEM_ADDRESS calls both
    /// `loadExecutionTable` and `executeIncomingCrossChainCall`.
    pub eezl2_address: Address,
    /// L2 chain id committed by the native envelope.
    pub l2_chain_id: u64,
    /// Alloy provider for the embedded L1 RPC. Used to sign the
    /// `postAndVerifyBatch` transaction (nonce + fee reads). Submission goes
    /// through `submitter`.
    pub l1_provider: alloy_provider::RootProvider,
    /// Shared `Submitter` handle — the single L1 submission path.
    /// `Submitter` is internally `Arc<Inner>`, so `Clone` is cheap.
    /// `compose_sync_slot` hands it `[postBatch, inbound_user_tx_1, …]` via
    /// `Submitter::send_bundle`; outbound user transactions execute in the L2
    /// Sync block instead. The Submitter requires atomic `eth_sendBundle` for
    /// every multi-transaction settlement; only a lone postBatch may fall back
    /// to a plain execution RPC.
    pub submitter: eez_l1::Submitter,
    /// L1 EOA whose key signs the `postAndVerifyBatch` transaction. Different from
    /// the fixed keyless L2 SYSTEM_ADDRESS. For dev
    /// smoke this is typically the hardhat #0 deployer key; in
    /// production this is the based-rollup composer's L1 wallet.
    pub l1_poster_signer: alloy_signer_local::PrivateKeySigner,
    /// L1 chain id for EIP-155 signing of the `postAndVerifyBatch` transaction.
    pub l1_chain_id: u64,
    /// Physical sidecar encoding for the mandatory native blob DA carrier.
    pub blob_sidecar_encoding: eez_blob_codec::BlobSidecarEncoding,
    /// Address of the rollup's on-chain proof-system contract, embedded
    /// in `batch.proofSystems[0]`; `EEZ.postAndVerifyBatch` iterates
    /// `proofSystems[]` and calls `verify` on each. Deployment registers
    /// `ECDSAProofSystem`, which requires
    /// `ECDSA.recover(publicInputsHash, proof) == signer`; the remote proof
    /// signer signs that exact hash after validating the batch.
    pub ecdsa_proof_system_address: Address,
}

impl std::fmt::Debug for CrossChainExecCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrossChainExecCtx")
            .field("system_address", &eez_primitives::SYSTEM_ADDRESS)
            .field("eezl2_address", &self.eezl2_address)
            .field("l2_chain_id", &self.l2_chain_id)
            .field("blob_sidecar_encoding", &self.blob_sidecar_encoding)
            .finish_non_exhaustive()
    }
}

/// The cross-chain compose dependencies, wired together (all-or-
/// nothing) by `eez-node` startup when the embedded L1 is up.
pub struct CrossChainWiring {
    /// Rollup id of the entry chain.
    pub entry_rollup_id: eez_protocol::RollupId,
    /// All registered rollups (entry + followers). The entry is also
    /// in this map — composition orchestration uses it uniformly.
    pub rollups: HashMap<
        eez_protocol::RollupId,
        (
            Arc<dyn eez_protocol::executor::ChainClient + Send + Sync>,
            eez_protocol::TargetConfig,
        ),
    >,
    /// Runtime context for deriving and signing L2 system transactions from
    /// composition batches.
    pub exec_ctx: Arc<CrossChainExecCtx>,
    /// Concrete local clients for slot-scoped chained composition, un-erased so
    /// the drain can reach `L1SlotState` and `simulate_source_tx_on`. The same
    /// instances registered in `rollups`; they share one overlay channel.
    pub local: crate::local::LocalComposeClients,
}

/// Target sessions for one composition, keyed by the rollup they execute on.
type SlotSessions =
    HashMap<eez_protocol::RollupId, Box<dyn eez_protocol::TargetExecutionSession + Send>>;

#[derive(Clone, Copy)]
struct CompositionGas {
    source: u64,
    l1_target: u64,
}

/// Only L1 destination frames execute inside the outbound postBatch. Nested
/// callbacks on L2 execute in the Sync block instead. The composition inspector
/// returns intercepted calls' gas to their caller, so every recorded L1 frame
/// must be counted, including re-entry and reverted frames, without charging
/// the intervening L2 execution to L1. Protocol/proxy overhead remains separate.
fn l1_target_gas(actions: &[eez_protocol::ExecutedAction]) -> u64 {
    actions
        .iter()
        .filter(|action| action.target_rollup_id.is_mainnet())
        .filter_map(|action| action.outcome.gas_used())
        .fold(0, u64::saturating_add)
}

/// A one-entry [`SlotSessions`] — every drain composition seeds exactly one
/// target chain.
fn seed_session(
    rollup_id: eez_protocol::RollupId,
    session: impl eez_protocol::TargetExecutionSession + 'static,
) -> SlotSessions {
    HashMap::from([(
        rollup_id,
        Box::new(session) as Box<dyn eez_protocol::TargetExecutionSession + Send>,
    )])
}

/// Compose one source tx over caller-owned, slot-scoped execution contexts.
///
/// The sessions come back with the composition: the caller commits their
/// effects on accept and drops them on eviction.
///
/// Third element reports source and L1 target simulation gas for admission. Source
/// simulation bypasses proxy consumption; it is not a canonical receipt estimate.
/// # Errors
///
/// Source-side executor failures and composition/finalize failures, classified
/// by [`sim_error_is_poison`]. A source simulation that merely reverts is not
/// an error here — it finalizes to an empty composition, classified downstream.
// Preserve the protocol crate's structured public error type.
#[allow(clippy::result_large_err)]
#[tracing::instrument(skip_all, fields(tx_len = raw_tx.len(), %entry_rollup_id))]
fn compose_crosschain(
    cc: &CrossChainWiring,
    entry_rollup_id: eez_protocol::RollupId,
    entry_client: &crate::local::LocalChainClient,
    raw_tx: &[u8],
    sessions: SlotSessions,
    source_state: &mut crate::local::build::DraftDb,
    source_env: reth_evm::EvmEnvFor<EezEvmConfig>,
    source_hash: B256,
    l1_anchor: B256,
    gas_trace: Option<&gas_trace::Config>,
) -> eez_protocol::ComposerResult<(eez_protocol::Composition, SlotSessions, CompositionGas)> {
    use eez_protocol::ChainClient as _;
    use eez_protocol::composition::Rollup;

    // Overlay snapshots are transaction-local. Clear every participating
    // client so an unbalanced stack cannot affect this composition.
    entry_client.reset_composition_state();
    for (client, _) in cc.rollups.values() {
        client.reset_composition_state();
    }

    tracing::info!(
        name: "composer.simulate.start",
        %entry_rollup_id,
        tx_len = raw_tx.len(),
        rollup_count = cc.rollups.len(),
        "starting composition pipeline"
    );

    let seeded: Vec<eez_protocol::RollupId> = sessions.keys().copied().collect();

    // Assemble registered clients and configs; the seeded sessions replace the
    // lazy ones for the rollups this direction dispatches to.
    let mut rollups: HashMap<eez_protocol::RollupId, Rollup> =
        HashMap::with_capacity(cc.rollups.len());
    for (rollup_id, (client, config)) in &cc.rollups {
        rollups.insert(
            *rollup_id,
            Rollup {
                client: Arc::clone(client),
                session: None,
                config: config.clone(),
            },
        );
    }

    let mut builder =
        eez_protocol::CompositionBuilder::new(entry_rollup_id, rollups).with_sessions(sessions);
    let _trace =
        gas_trace.and_then(
            |config| match config.begin_admission(l1_anchor, source_hash, raw_tx) {
                Ok(scope) => scope,
                Err(error) => {
                    tracing::warn!(%source_hash, %error, "cannot start selected gas trace");
                    None
                }
            },
        );
    let source = entry_client
        .simulate_source_tx_on_with_retries(
            raw_tx.to_vec(),
            &mut builder,
            source_state,
            source_env,
            true,
        )
        .map_err(eez_protocol::CompositionError::from)?;
    if !source.success {
        return Err(eez_protocol::ProtocolErrorKind::SourceTransactionFailed.into());
    }
    let recorded_count = builder.recorded_count();
    // Reclaimed before `finalize` consumes the builder — the accepted effects
    // live in the sessions, not in the composition.
    let sessions = builder.take_sessions();

    // A session on a rollup nobody seeded means the dispatch opened a lazy one
    // and ran UNCHAINED — off this slot's states (invariant 6/8). Entry-chain
    // sessions are legitimate: overlay re-entry opens them there.
    if let Some(unseeded) = sessions
        .keys()
        .find(|id| **id != entry_rollup_id && !seeded.contains(id))
    {
        // Unavailable ⇒ classified TRANSIENT: a wiring gap is not the tx's
        // fault, so the slot aborts and re-queues rather than evicting.
        return Err(eez_protocol::ExecutorError::from(
            eez_protocol::ExecutorErrorKind::Unavailable(format!(
                "composition dispatched to rollup {unseeded}, which has no slot session; \
                 it would execute unchained against chain state instead of this slot's state"
            )),
        )
        .into());
    }

    let composition = builder.finalize()?;
    for action in &composition.actions {
        gas_trace::note_address(action.target_address, "cross_chain_target");
    }
    let target_gas = l1_target_gas(&composition.actions);
    if gas_trace::active() {
        gas_trace::emit(serde_json::json!({
            "kind": "admission_simulation_result", "source_hash": source_hash,
            "source_gas_used": source.gas_used, "l1_target_gas": target_gas,
            "recorded_calls": recorded_count,
        }));
    }

    tracing::info!(
        name: "composer.simulate.complete",
        target_count = composition.targets.len(),
        recorded = recorded_count,
        l1_target_gas = target_gas,
        "composition complete"
    );

    Ok((
        composition,
        sessions,
        CompositionGas {
            source: source.gas_used,
            l1_target: target_gas,
        },
    ))
}

impl std::fmt::Debug for CrossChainWiring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrossChainWiring")
            .field("entry_rollup_id", &self.entry_rollup_id)
            .field("rollups", &self.rollups.len())
            .finish()
    }
}

/// Failed prover/immediate relay rejection episodes before a held user_tx is
/// evicted as probable poison. Automatic admission's canonical settlement
/// failures use `MAX_AUTOMATIC_SETTLEMENT_ATTEMPTS` so a reduced retry can follow
/// the fifth exclusion. Both paths charge the same cumulative source counter.
/// After the applicable bound is reached, the transaction and
/// its nonce-dependent suffix are evicted so they cannot block the FIFO queue
/// indefinitely.
///
/// Drain-time simulations are chained per chain in canonical order
/// (`compose_crosschain` over the slot's L1 state and the Sync block under
/// construction — `docs/CHAINED-INTERSTATE-DESIGN.md`), so a co-bundled
/// prerequisite is already visible when its dependant composes. What this bound
/// backstops is the residual: L1 state that moves between compose time and the
/// bundle's inclusion block.
pub const MAX_BUNDLE_ATTEMPTS: u32 = 3;

pub(crate) const SUBMISSION_BACKOFF_THRESHOLD: u32 = 5;

/// Stand-in for `proofs[0]` when sizing before the prover runs (ECDSA is 65 B).
const MAX_PROOF_BYTES: usize = 128;

#[cfg(test)]
const DEFAULT_MAX_POSTBATCH_GAS: u64 = 16_777_216;

/// Execution for a batch with just the leading immediate entry, calldata aside.
/// Measured 139k, 10% slack; pinned by `contracts/test/PostBatchGasPins.t.sol`.
const POSTBATCH_BASE_GAS_PIN: u64 = 160_000;

/// One more entry costs EEZ.sol hashing, a state SSTORE, and a CREATE2 proxy
/// deploy for a new sender. Same pin test; measured 334k worst case, 10% slack.
const POSTBATCH_ENTRY_GAS_PIN: u64 = 370_000;

#[cfg(test)]
fn minimum_viable_postbatch_gas() -> u64 {
    projected_postbatch_gas(2, 0, 0).saturating_add(gas::framing::base(64))
}

/// EIP-7623 calldata floor — below it the tx is invalid and dies at simulation.
fn calldata_floor_gas(calldata: &[u8]) -> u64 {
    let nonzero = calldata.iter().filter(|byte| **byte != 0).count() as u64;
    let zero = calldata.len() as u64 - nonzero;
    21_000 + 10 * (zero + 4 * nonzero)
}

/// Exact standard-rate calldata gas: 4 per zero byte, 16 per non-zero byte.
fn calldata_gas(bytes: &[u8]) -> u64 {
    let nonzero = bytes.iter().filter(|byte| **byte != 0).count() as u64;
    let zero = bytes.len() as u64 - nonzero;
    4 * zero + 16 * nonzero
}

/// Whole-postBatch gas on EIP-7623's standard branch: tx base, calldata, pins,
/// probed target calls. The floor branch is checked before transaction signing.
const fn projected_postbatch_gas(entry_count: u64, calldata_gas: u64, target_gas: u64) -> u64 {
    21_000_u64
        .saturating_add(POSTBATCH_BASE_GAS_PIN)
        .saturating_add(POSTBATCH_ENTRY_GAS_PIN.saturating_mul(entry_count))
        .saturating_add(calldata_gas)
        .saturating_add(target_gas)
}

/// What one accepted held tx adds to the postBatch projection, per term.
#[derive(Debug, Clone, Copy, Default)]
struct TxL1Gas {
    estimate_reasons: gas::EstimateReasons,
    /// `batch.entries` rows it contributes, each costing [`POSTBATCH_ENTRY_GAS_PIN`].
    entries: u64,
    /// Saving justified by a bounded entry shape and canonical-parent evidence.
    entry_discount: u64,
    /// Standard-rate gas for every byte it puts in the postBatch calldata.
    calldata_gas: u64,
    /// Additional final calldata: root/hash rewrites and enclosing entry offsets.
    framing_gas: u64,
    /// Static entries make the final execution projection use general pins.
    general: bool,
    /// Probed cost of the target call EEZ.sol runs inline (outbound only).
    target_gas: u64,
}

impl TxL1Gas {
    /// Incremental work only: the post's intrinsic gas and leading anchor are
    /// already reserved. Admission and rejected-group diagnostics use this sum.
    const fn total(self) -> u64 {
        POSTBATCH_ENTRY_GAS_PIN
            .saturating_mul(self.entries)
            .saturating_sub(self.entry_discount)
            .saturating_add(self.calldata_gas)
            .saturating_add(self.target_gas)
    }
}

/// L1 gas one accepted tx adds. Only `batch.entries` reaches postBatch calldata;
/// DA entry copies and raw source transactions live in native blobs. Outbound
/// target calls execute inline; inbound companions keep their separate budget.
fn projected_tx_l1_gas(
    batch_entries: &[eez_protocol::abi::ExecutionEntrySol],
    target_gas: u64,
) -> TxL1Gas {
    use alloy_sol_types::SolValue as _;
    TxL1Gas {
        estimate_reasons: gas::EstimateReasons::unavailable(batch_entries.len() as u64),
        entries: batch_entries.len() as u64,
        entry_discount: 0,
        calldata_gas: batch_entries
            .iter()
            .map(|entry| calldata_gas(&entry.abi_encode()))
            .fold(0, u64::saturating_add),
        framing_gas: batch_entries
            .iter()
            .map(gas::framing::mutable)
            .fold(0, u64::saturating_add),
        general: false,
        target_gas,
    }
}

/// Preflight projection for postBatch plus ordered inbound companions. Protocol
/// pins include entry work omitted by source simulation. Only final native
/// bundle execution establishes actual gas consumption and complete effects.
#[derive(Debug, Clone, Copy)]
struct PostBatchGasBudget {
    /// Maximum complete pre-margin estimate, including framing and any floor.
    cap: u64,
    /// Running total, seeded with the leading immediate entry every postBatch
    /// carries. The projection is linear, so costs just add.
    projected: u64,
    calldata_gas: u64,
    framing_gas: u64,
    floor_active: bool,
    general: bool,
    /// The shaped projection does not charge a generic entry for the anchor.
    /// Restore it if a static pool forces final sizing onto general pins.
    anchor_discount: u64,
    entries: u64,
    entry_discount: u64,
    block_cap: u64,
    transaction_cap: u64,
    companion_gas: u64,
    /// Largest estimated source prefix plus the next companion's declared gas.
    /// Recheck it when postBatch grows: postBatch executes before every source.
    companion_required_gas: u64,
    rejected_constraint: Option<&'static str>,
}

impl PostBatchGasBudget {
    #[cfg(test)]
    fn new(max_gas: u64) -> Self {
        Self::with_calldata_limits(max_gas, 64, true)
    }

    fn with_calldata_limits(max_gas: u64, maximum_blobs: usize, floor_active: bool) -> Self {
        Self {
            cap: max_gas,
            projected: projected_postbatch_gas(1, 0, 0),
            calldata_gas: 0,
            framing_gas: gas::framing::base(maximum_blobs),
            floor_active,
            general: maximum_blobs > 64,
            anchor_discount: 0,
            entries: 0,
            entry_discount: 0,
            block_cap: u64::MAX,
            transaction_cap: u64::MAX,
            companion_gas: 0,
            companion_required_gas: 0,
            rejected_constraint: None,
        }
    }

    fn use_projection(&mut self, initial_gas: u64) {
        self.projected = if self.general {
            initial_gas.saturating_add(POSTBATCH_ENTRY_GAS_PIN)
        } else {
            self.anchor_discount = POSTBATCH_ENTRY_GAS_PIN;
            initial_gas
        };
    }

    fn full_estimate(&self) -> u64 {
        let standard = self.projected.saturating_add(self.framing_gas);
        if self.floor_active {
            standard.max(gas::framing::floor(
                self.calldata_gas.saturating_add(self.framing_gas),
            ))
        } else {
            standard
        }
    }

    /// Charge `cost` if it fits. `false` leaves the projection untouched, so the
    /// caller can retain this group and try another independent source.
    fn try_accept(&mut self, cost: TxL1Gas) -> bool {
        self.try_accept_with_companion(cost, 0, 0)
    }

    /// Establish a nonfit in the admission model without source execution using
    /// its least expensive shapes and zero new companion consumption. Valid
    /// inbound composition contains mutable or static entries (finalize rejects
    /// empty calls). Static-only sources need no mutable entry, but force general
    /// execution pins, so both possibilities must fail before skipping execution.
    /// Use the same admission arithmetic, including discount revocation
    /// and each signed transaction's start envelope; never cache a simulated
    /// price across changed parents or accepted prefixes.
    fn minimum_rejection(&self, inbound: bool, declared: u64) -> Option<&'static str> {
        if self.entries == 0 {
            return None;
        }
        let cost = TxL1Gas {
            estimate_reasons: Default::default(),
            entries: 1,
            entry_discount: POSTBATCH_ENTRY_GAS_PIN - gas::minimum_entry_gas(!inbound),
            calldata_gas: 0,
            target_gas: 0,
            ..Default::default()
        };
        let mut next = *self;
        if next.try_accept_with_companion(cost, if inbound { declared } else { 0 }, 0) {
            return None;
        }
        if inbound {
            let mut static_only = *self;
            if static_only.try_accept_with_companion(
                TxL1Gas {
                    general: true,
                    ..Default::default()
                },
                declared,
                0,
            ) {
                return None;
            }
        }
        next.rejected_constraint
    }

    /// Previous queue storage is temporary work, not evidence that a source
    /// can never fit. Retain such a first group until an independently due
    /// consolidation clears the old queue; do not evict its nonce chain.
    fn blocked_by_cleanup(
        &self,
        cost: TxL1Gas,
        declared: u64,
        estimated: u64,
        cleanup: u64,
    ) -> bool {
        if self.entries != 0 || cleanup == 0 {
            return false;
        }
        let mut empty = *self;
        empty.projected = empty.projected.saturating_sub(cleanup);
        empty.try_accept_with_companion(cost, declared, estimated)
    }

    fn try_accept_with_companion(
        &mut self,
        cost: TxL1Gas,
        declared_companion_gas: u64,
        estimated_companion_gas: u64,
    ) -> bool {
        let entries = self.entries.saturating_add(cost.entries);
        let general = self.general || cost.general;
        let discount = if !general && entries < gas::MAX_SHAPED_ENTRIES {
            self.entry_discount.saturating_add(cost.entry_discount)
        } else {
            0
        };
        // Crossing the measured batch-size range restores every entry's old
        // reserve, not just the last one's. Rejection leaves all totals intact.
        let next = POSTBATCH_ENTRY_GAS_PIN
            .checked_mul(cost.entries)
            .and_then(|gas| gas.checked_add(cost.calldata_gas))
            .and_then(|gas| gas.checked_add(cost.target_gas))
            .and_then(|gas| gas.checked_add(self.entry_discount))
            .and_then(|gas| gas.checked_add(self.projected))
            .and_then(|gas| gas.checked_add(if general { self.anchor_discount } else { 0 }))
            .and_then(|gas| gas.checked_sub(discount));
        let Some(next) = next else {
            self.rejected_constraint = Some("l1_tx_gas_budget");
            return false;
        };
        let Some(required) = self.companion_gas.checked_add(declared_companion_gas) else {
            self.rejected_constraint = Some("l1_bundle_gas_budget");
            return false;
        };
        let required = required.max(self.companion_required_gas);
        let mut next_budget = *self;
        next_budget.projected = next;
        next_budget.framing_gas = self.framing_gas.saturating_add(cost.framing_gas);
        next_budget.calldata_gas = self.calldata_gas.saturating_add(cost.calldata_gas);
        let full_estimate = next_budget.full_estimate();
        if full_estimate == u64::MAX
            || full_estimate > self.cap
            || declared_companion_gas > self.transaction_cap
            || estimated_companion_gas > declared_companion_gas
        {
            self.rejected_constraint = Some("l1_tx_gas_budget");
            return false;
        }
        if full_estimate
            .checked_add(required)
            .is_none_or(|gas| gas > self.block_cap)
        {
            self.rejected_constraint = Some("l1_bundle_gas_budget");
            return false;
        }
        self.rejected_constraint = None;
        self.projected = next;
        self.calldata_gas = next_budget.calldata_gas;
        self.framing_gas = next_budget.framing_gas;
        self.general = general;
        if general {
            self.anchor_discount = 0;
        }
        self.entries = entries;
        self.entry_discount = discount;
        // estimated <= declared, whose addition was checked above.
        self.companion_gas += estimated_companion_gas;
        self.companion_required_gas = required;
        true
    }
}

#[allow(clippy::too_many_arguments)]
fn trace_admission_budget(
    config: Option<&gas_trace::Config>,
    candidate: &crate::admission_events::Candidate,
    source_hash: B256,
    parent: B256,
    direction: &'static str,
    before: PostBatchGasBudget,
    after: PostBatchGasBudget,
    cost: TxL1Gas,
    entries: &[eez_protocol::abi::ExecutionEntrySol],
    simulated: CompositionGas,
    companion_limit: u64,
    companion_estimate: gas::CompanionEstimate,
    prior_cleanup: u64,
    gas_gate_accepted: bool,
) {
    let detail = serde_json::json!({
        "kind": "admission_budget", "source_hash": source_hash, "direction": direction,
        "l1_parent": parent,"fixed_drain_reserve_gas": 0,
        "framing_reserve_gas":cost.framing_gas,
        "framing_reserve_before":before.framing_gas,
        "framing_reserve_after":gas_gate_accepted.then_some(after.framing_gas),
        "full_estimate_before":before.full_estimate(),
        "full_estimate_after":gas_gate_accepted.then_some(after.full_estimate()),
        "gas_gate_accepted": gas_gate_accepted, "constraint": after.rejected_constraint,
        "projected_before": before.projected,
        "projected_after": gas_gate_accepted.then_some(after.projected),
        "initial_queue_cleanup": prior_cleanup,
        "entry_count": cost.entries,
        "entry_pin_gas": POSTBATCH_ENTRY_GAS_PIN.saturating_mul(cost.entries),
        "entry_discount": cost.entry_discount,
        "estimate_reasons": cost.estimate_reasons.json(),
        "batch_discount_revoked": before.general || cost.general || before.entries.saturating_add(cost.entries) >= gas::MAX_SHAPED_ENTRIES,
        "entry_reserve_gas": POSTBATCH_ENTRY_GAS_PIN.saturating_mul(cost.entries).saturating_sub(cost.entry_discount),
        "l1_call_count": entries.iter().map(|e| e.l2ToL1Calls.len() + e.expectedL1ToL2Calls.iter().map(|r| r.l2ToL1Calls.len()).sum::<usize>()).sum::<usize>(),
        "nested_result_count": entries.iter().map(|e| e.expectedL1ToL2Calls.len()).sum::<usize>(),
        "calldata_gas": cost.calldata_gas,
        "l1_target_sim_gas": cost.target_gas,
        "source_sim_gas": simulated.source,
        "companion_declared_gas": companion_limit,
        "companion_sim_gas": if direction == "inbound" { simulated.source } else { 0 },
        "companion_target_sim_gas": if direction == "inbound" { simulated.l1_target } else { 0 },
        "companion_estimate_gas": companion_estimate.gas,
        "companion_protocol_reserve_gas": companion_estimate.protocol_reserve,
        "companion_estimate_basis": companion_estimate.basis,
        "postbatch_planning_cap": before.cap,
        "whole_block_cap": before.block_cap,
    });
    candidate.emit("source_estimate", detail.clone());
    let Some(config) = config.filter(|config| config.selects(source_hash)) else {
        return;
    };
    let Ok(_trace) = config
        .begin("admission_budget", parent, source_hash)
        .inspect_err(|error| {
            event!(Level::WARN, %source_hash, %error, "cannot write selected admission gas ledger");
        })
    else {
        return;
    };
    gas_trace::emit(detail);
}

/// What the drain can do with a pair wanting `declared` gas on a Sync block
/// that already holds `gas_used`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockGasFit {
    /// Room in this block — accept.
    Accept,
    /// No room here, but an emptier block would take it: hold for a later slot.
    Defer,
    /// Over the whole block limit, so no slot can ever take it.
    Unfittable,
}

/// Least gas any transaction consumes: Ethereum's intrinsic base cost.
///
/// A native tx executes with the block's remaining gas (bounded by the Osaka
/// cap) and reserves nothing, so before execution only this lower bound of its
/// share is known. Admission prechecks charge it to reject groups that cannot
/// fit even minimally; `SyncBlockState::capacity_probe` decides authoritatively
/// once the group's transactions exist.
const MIN_TX_GAS: u64 = 21_000;

/// Whether a group fits the L2 Sync block, from a gas figure that must not
/// under-count what the executor admits on (ordinary declared limits) and may
/// under-count native use (see [`MIN_TX_GAS`]).
const fn block_gas_fit(gas_used: u64, declared: u64, block_gas_limit: u64) -> BlockGasFit {
    if gas_used.saturating_add(declared) <= block_gas_limit {
        BlockGasFit::Accept
    } else if declared > block_gas_limit {
        BlockGasFit::Unfittable
    } else {
        BlockGasFit::Defer
    }
}

/// Declared gas limit of a raw tx. An undecodable tx counts as the whole block
/// so it is refused, never read as free space (`invariant 7`).
fn declared_gas_limit(raw: &Bytes) -> u64 {
    use alloy_consensus::Transaction as _;
    use alloy_eips::eip2718::Decodable2718 as _;
    eez_primitives::EezTxEnvelope::decode_2718(&mut raw.as_ref())
        .map_or(u64::MAX, |tx| tx.gas_limit())
}

/// A group the draft refused for block capacity. With nothing before it the
/// block is as empty as any slot's, so no later slot can take it either.
const fn capacity_refusal(prefix_is_empty: bool) -> BlockGasFit {
    if prefix_is_empty {
        BlockGasFit::Unfittable
    } else {
        BlockGasFit::Defer
    }
}

/// The refused pair's numbers, for the block-gas cut event.
#[derive(Debug, Clone, Copy)]
struct BlockGasCut {
    gas_used: u64,
    declared: u64,
}

/// Emission bounds, resolved once at construction. `timing` is here for its
/// `k()` — the grid the historical chunk boundary snaps to.
#[derive(Debug, Clone, Copy)]
struct EmissionLimits {
    timing: RollupTiming,
    /// Cap on a batch's block span, `cursor+1 ..= terminal`.
    max_blocks: u64,
    /// Gas limit every postBatch is signed with, and the cap the drain uses.
    max_gas: u64,
}

impl EmissionLimits {
    /// An accumulated backlog can exceed the next posting window after a
    /// missed slot or catch-up burst. That is a splittable capacity condition;
    /// stale/backwards endpoints remain construction errors.
    fn checked_window_span(self, posted: u64, terminal: u64) -> Result<u64, PreparePostBatchError> {
        let span = terminal.checked_sub(posted).filter(|span| *span != 0).ok_or_else(|| {
            PreparePostBatchError::Build(format!(
                "sync block {terminal} <= L1-confirmed cursor {posted}; composer is behind its own posted batches"
            ))
        })?;
        if span > self.max_blocks {
            return Err(PreparePostBatchError::OverBudget {
                constraint: "window_limit",
                required: span,
                maximum: self.max_blocks,
            });
        }
        Ok(span)
    }
}

/// Whether [`Composer::recover_failed_batch`] moved the head, so the caller knows
/// whether its slot still owes a block.
#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryOutcome {
    /// A rollback and/or sibling commit landed — the caller's slot parent is stale.
    HeadMoved,
    /// Nothing was committed (bail, stale verdict, or nothing to roll back).
    HeadUnchanged,
}

#[derive(Debug, thiserror::Error)]
enum PreparePostBatchError {
    #[error("{0}")]
    Build(String),
    #[error("{constraint}: candidate requires {required}, resolved limit is {maximum}")]
    OverBudget {
        constraint: &'static str,
        required: u64,
        maximum: u64,
    },
    #[error(
        "terminal cross-chain composition alone needs {required} blobs, target permits {maximum}; historical splitting cannot repair it"
    )]
    CompositionBlobLimit { required: usize, maximum: usize },
    #[error("prover.prove: {0}")]
    Prover(#[source] ProverError),
    #[error("prover rejected {0}")]
    Actionable(ActionableProverFailure),
    #[error(
        "local observation {observation_index} for inbound composition {composition_index} \
         does not match its claimed outcome"
    )]
    ObservationMismatch {
        composition_index: usize,
        observation_index: usize,
        expected_success: bool,
        observed_success: bool,
        expected_output_hash: B256,
        observed_output_hash: B256,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SettlementFailureOutcome {
    requeued: usize,
    evicted: usize,
}

/// Count one failed settlement episode for every candidate, evict transactions
/// that reach the bound, and cascade each eviction through its sender/direction
/// nonce suffix. Survivors retain their attempt count and FIFO order.
fn recover_settlement_failure(
    pool: &HeldPool,
    rollup_id: u64,
    txs: Vec<HeldTx>,
) -> SettlementFailureOutcome {
    let mut retry = Vec::with_capacity(txs.len());
    let mut evicted = Vec::new();
    let mut evicted_chains: Vec<(Address, Direction, u64)> = Vec::new();

    for mut tx in txs {
        tx.attempts = tx.attempts.saturating_add(1);
        if tx.attempts >= MAX_BUNDLE_ATTEMPTS {
            evicted_chains.push((tx.sender, tx.direction, tx.nonce));
            event!(
                name: "eez.composer.recovery.poison_evicted",
                Level::ERROR,
                event_name = "eez.composer.recovery.poison_evicted",
                rollup_id,
                source = "Prover",
                tx_hash = %tx.hash,
                sender = %tx.sender,
                direction = ?tx.direction,
                nonce = tx.nonce,
                attempts = tx.attempts,
                "potentially valid user_tx evicted after MAX_BUNDLE_ATTEMPTS failed settlement episodes to preserve chain liveness; repeated component disagreement requires investigation; resubmit required",
            );
            evicted.push(tx);
        } else {
            retry.push(tx);
        }
    }

    // Without the missing nonce, later transactions cannot execute and would
    // keep invalidating batches that include them.
    for (sender, direction, nonce) in evicted_chains {
        retry.retain(|tx| {
            let cascade =
                tx.sender == sender && tx.direction == direction && tx.nonce > nonce;
            if cascade {
                event!(
                    name: "eez.composer.recovery.nonce_chain_evicted",
                    Level::ERROR,
                    event_name = "eez.composer.recovery.nonce_chain_evicted",
                    rollup_id,
                    source = "Prover",
                    tx_hash = %tx.hash,
                    sender = %tx.sender,
                    direction = ?tx.direction,
                    nonce = tx.nonce,
                    gap_at = nonce,
                    "potentially valid same-sender tx depends on an evicted nonce; evicting nonce suffix for liveness (resubmit in order)",
                );
                evicted.push(tx.clone());
            }
            !cascade
        });
        for tx in pool.evict_chain_at_or_above(sender, direction, nonce) {
            event!(
                name: "eez.composer.recovery.nonce_chain_evicted",
                Level::ERROR,
                event_name = "eez.composer.recovery.nonce_chain_evicted",
                rollup_id,
                source = "Prover",
                tx_hash = %tx.hash,
                sender = %tx.sender,
                direction = ?tx.direction,
                nonce = tx.nonce,
                gap_at = nonce,
                "potentially valid pooled tx depends on an evicted nonce; evicting nonce suffix for liveness (resubmit in order)",
            );
            evicted.push(tx);
        }
    }

    let outcome = SettlementFailureOutcome {
        requeued: retry.len(),
        evicted: evicted.len(),
    };
    pool.release_in_flight_batch(&evicted);
    pool.push_front_batch(retry);
    outcome
}

impl From<String> for PreparePostBatchError {
    fn from(error: String) -> Self {
        Self::Build(error)
    }
}

impl From<&str> for PreparePostBatchError {
    fn from(error: &str) -> Self {
        Self::Build(error.to_owned())
    }
}

fn first_observation_mismatch(
    schedule: &[(usize, BlockObservation)],
    outcomes: &[ObservedOutcome],
) -> Result<Option<ObservationMismatchDetails>, String> {
    if outcomes.len() != schedule.len() {
        return Err(format!(
            "local observation replay returned {} outcomes for {} claims",
            outcomes.len(),
            schedule.len(),
        ));
    }
    Ok(schedule.iter().zip(outcomes).enumerate().find_map(
        |(observation_index, ((composition_index, observation), outcome))| {
            (observation.claim.expected_success != outcome.success
                || observation.claim.expected_output != outcome.output)
                .then(|| ObservationMismatchDetails {
                    composition_index: *composition_index,
                    observation_index,
                    expected_success: observation.claim.expected_success,
                    observed_success: outcome.success,
                    expected_output_hash: alloy_primitives::keccak256(
                        &observation.claim.expected_output,
                    ),
                    observed_output_hash: alloy_primitives::keccak256(&outcome.output),
                })
        },
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ObservationMismatchDetails {
    composition_index: usize,
    observation_index: usize,
    expected_success: bool,
    observed_success: bool,
    expected_output_hash: B256,
    observed_output_hash: B256,
}

/// A state-dependent wait blocks only its source chain, not the entire slot.
fn source_condition(error: &eez_protocol::ComposerError) -> Option<&eez_protocol::SourceCondition> {
    if let eez_protocol::ComposerErrorKind::Executor(error) = error.kind()
        && let eez_protocol::ExecutorErrorKind::SourceBlocked(condition) = error.kind()
    {
        Some(condition)
    } else {
        None
    }
}

/// Deterministic invalid compositions are poison. Source conditions defer one
/// chain; infrastructure failures abort the slot. Neither is an eviction.
fn sim_error_is_poison(err: &eez_protocol::ComposerError) -> bool {
    use eez_protocol::{ComposerErrorKind, ExecutorErrorKind};
    match err.kind() {
        // Protocol failures = the composition itself is invalid
        // (EmptyCalls, broken chaining, unknown target, bad encoding) —
        // deterministic for this tx.
        ComposerErrorKind::Protocol(_) => true,
        // Executor failures: poison EXCEPT the clearly-transient ones.
        ComposerErrorKind::Executor(ee) => !matches!(
            ee.kind(),
            ExecutorErrorKind::Unavailable(_)
                | ExecutorErrorKind::SourceBlocked(_)
                | ExecutorErrorKind::Provider(_)
                | ExecutorErrorKind::Missing(_)
        ),
        // Lifecycle / internal (misconfigured, lock poisoned, double
        // register) — not the tx's fault → retry, don't evict.
        _ => false,
    }
}

/// Preserve a claimed source and its retry budget while freeing selection
/// capacity for independent chains. Its nonce suffix stays in the pool.
fn defer_source(
    pool: &HeldPool,
    selection: &mut selection::Selection,
    deferred: &mut Vec<(usize, HeldTx)>,
    source_state: crate::held_pool::SourceState,
    idx: usize,
    held: HeldTx,
    condition: eez_protocol::SourceCondition,
) {
    pool.record_blocked(&held, condition, source_state);
    selection.block(idx);
    deferred.push((idx, held));
}

/// Capacity misses retain ownership and stop younger admission across both
/// phases. Older sources may still be simulated to complete the FIFO prefix;
/// retain each refusal's reason across a later deadline or provider abort.
#[derive(Default)]
struct CapacityDeferrals {
    groups: Vec<(usize, HeldTx)>,
    reasons: std::collections::BTreeMap<&'static str, usize>,
    last_reason: Option<&'static str>,
    minimum_check_count: usize,
}

impl CapacityDeferrals {
    /// Early deferral uses only this candidate's budgets and the admission
    /// model's minimum group cost. Never use it on an empty candidate: normal simulation must still
    /// classify malformed or individually unpostable sources there.
    fn minimum_check(
        &mut self,
        candidate: &crate::admission_events::Candidate,
        held: &HeldTx,
        budget: PostBatchGasBudget,
        l2_used: u64,
    ) -> Option<&'static str> {
        if budget.entries == 0 {
            return None;
        }
        let inbound = held.direction == Direction::Inbound;
        let declared = declared_gas_limit(&held.raw_tx);
        let l2_minimum = if inbound {
            // A static-only lookup needs no L2 delivery transaction.
            0
        } else {
            MIN_TX_GAS.saturating_add(declared)
        };
        let reason = if block_gas_fit(l2_used, l2_minimum, BUILDER_GAS_LIMIT) == BlockGasFit::Accept
        {
            budget.minimum_rejection(inbound, declared)?
        } else {
            "l2_block_capacity"
        };
        self.minimum_check_count += 1;
        candidate.emit(
            "source_capacity_check",
            serde_json::json!({
                "source_hash":held.hash,"direction":if inbound {"inbound"} else {"outbound"},
                "constraint":reason,"basis":"minimum_admission_cost",
                "minimum_entry_gas":if inbound {0} else {gas::minimum_entry_gas(true)},
                "minimum_mutable_entry_gas":gas::minimum_entry_gas(!inbound),
                "minimum_static_general_fallback":inbound,
                "projected_before":budget.projected,"postbatch_planning_cap":budget.cap,
                "companion_prefix_gas":budget.companion_gas,
                "companion_required_gas":budget.companion_required_gas,
                "companion_declared_gas":if inbound {declared} else {0},
                "fixed_drain_reserve_gas":0,
                "framing_reserve_gas":budget.framing_gas,"full_estimate_gas":budget.full_estimate(),
                "whole_block_cap":budget.block_cap,"transaction_cap":budget.transaction_cap,
                "l2_used_gas":l2_used,"minimum_l2_gas":l2_minimum,"l2_block_cap":BUILDER_GAS_LIMIT,
            }),
        );
        Some(reason)
    }

    fn defer(
        &mut self,
        selection: &mut selection::Selection,
        candidate: &crate::admission_events::Candidate,
        idx: usize,
        held: HeldTx,
        reason: &'static str,
    ) {
        candidate.emit(
            "source_outcome",
            serde_json::json!({
                "source_hash":held.hash,"position":idx,"reason":reason,
            }),
        );
        // Capacity is a batch constraint, not a source eligibility condition.
        // Retain this group and every younger source without charging attempts.
        selection.cut(idx);
        self.groups.push((idx, held));
        *self.reasons.entry(reason).or_default() += 1;
        self.last_reason = Some(reason);
    }
}

alloy_sol_types::sol! {
    /// Storage getter for `EEZ.rollups[rollupId]` (auto-generated
    /// from `mapping(uint64 => RollupConfig) public rollups`).
    /// Returns the three public-getter fields in declaration order;
    /// this reader consumes `etherBalance`.
    #[sol(rpc)]
    interface IEEZReader {
        function rollups(uint64 rollupId)
            external
            view
            returns (address rollupContract, bytes32 root, uint256 etherBalance);
    }
}

/// L1-confirmed escrow (`rollups(rid).etherBalance`) an outbound withdrawal draws
/// down. A value-bearing composition cannot safely bypass this check: its
/// synthetic manager frame intentionally ignores transaction-balance validation.
async fn read_rollup_escrow(
    provider: &alloy_provider::RootProvider,
    rid: u64,
    anchor: B256,
) -> Result<U256, String> {
    let eez = std::env::var("EEZ_REGISTRY_ADDRESS")
        .map_err(|error| format!("EEZ_REGISTRY_ADDRESS: {error}"))?
        .parse::<Address>()
        .map_err(|error| format!("parse EEZ_REGISTRY_ADDRESS: {error}"))?;
    IEEZReader::new(eez, provider)
        .rollups(rid)
        .block(anchor.into())
        .call()
        .await
        .map(|rollup| rollup.etherBalance)
        .map_err(|error| format!("read rollup {rid} escrow: {error}"))
}

fn poison_gap_for(gaps: &[(Address, Direction, u64)], tx: &HeldTx) -> Option<u64> {
    gaps.iter()
        .filter(|(sender, direction, nonce)| {
            tx.sender == *sender && tx.direction == *direction && tx.nonce > *nonce
        })
        .map(|(_, _, nonce)| *nonce)
        .min()
}

fn push_poison_root(
    poison: &mut Vec<HeldTx>,
    poison_gaps: &mut Vec<(Address, Direction, u64)>,
    tx: HeldTx,
) {
    let gap = (tx.sender, tx.direction, tx.nonce);
    if !poison_gaps.contains(&gap) {
        poison_gaps.push(gap);
    }
    poison.push(tx);
}

/// Drop the drain indices, restoring the pool's FIFO order. The drain composes
/// in two direction phases, so a re-queue must be re-sorted or it would hand
/// the pool back a permutation of what it dealt out.
fn restore_pool_order(mut txs: Vec<(usize, HeldTx)>) -> Vec<HeldTx> {
    txs.sort_by_key(|(idx, _)| *idx);
    txs.into_iter().map(|(_, tx)| tx).collect()
}

/// Everything a transient abort still owes the pool: the failing transaction
/// (absent when it was evicted as poison first), the rest of the current phase,
/// and the whole untouched other phase.
fn abort_rest(
    failing: Option<(usize, HeldTx)>,
    rest_of_phase: &mut impl Iterator<Item = (usize, HeldTx)>,
    other_phase: Vec<(usize, HeldTx)>,
) -> Vec<(usize, HeldTx)> {
    let mut rest: Vec<(usize, HeldTx)> = failing.into_iter().collect();
    rest.extend(rest_of_phase);
    rest.extend(other_phase);
    rest
}

/// Back to the pool front in FIFO order, no attempt counted: nothing failed.
/// One above a nonce evicted this drain is dropped; it can never land.
fn requeue_unprocessed(
    pool: &crate::HeldPool,
    rollup_id: u64,
    poison_gaps: &[(Address, Direction, u64)],
    txs: Vec<(usize, HeldTx)>,
) {
    let mut requeue = restore_pool_order(txs);
    let mut cascade_evicted = Vec::new();
    requeue.retain(|tx| {
        let Some(gap_at) = poison_gap_for(poison_gaps, tx) else {
            return true;
        };
        event!(
            name: "eez.composer.cc_compose.poison_chain_evicted",
            Level::WARN,
            rollup_id,
            tx_hash = %tx.hash,
            sender = %tx.sender,
            nonce = tx.nonce,
            gap_at,
            "same-sender unprocessed tx above an evicted poison nonce; evicted instead of re-queued (resubmit in order)",
        );
        cascade_evicted.push(tx.clone());
        false
    });
    pool.release_in_flight_batch(&cascade_evicted);
    pool.requeue_composing(requeue);
}

/// Append `txs` to the Sync block under construction, stopping at the first one
/// that will not land. `Some((position, why))` means the block is half-extended
/// and must be reopened on the accepted list. `why` carries its class, so the
/// caller can tell a rejected tx from an unreachable backing store.
/// Append `txs` to the block and execute them. `Ok` carries their logs, so the
/// caller can check what a tx actually did against what it composed.
fn append_and_execute(
    prefix: &mut SyncBlockState,
    txs: &[Bytes],
) -> Result<Vec<alloy_primitives::Log>, (usize, BuildError)> {
    let mut logs = Vec::new();
    for (at, tx) in txs.iter().enumerate() {
        match prefix.execute_tx(tx) {
            Ok(outcome) if outcome.success => logs.extend(outcome.logs),
            Ok(outcome) => {
                return Err((
                    at,
                    BuildError::ExecuteTx {
                        idx: at,
                        msg: format!("reverted: {}", truncated_hex(&outcome.output)),
                    },
                ));
            }
            Err(e) => return Err((at, e)),
        }
    }
    Ok(logs)
}

/// Take the L1 session's accumulated effects for commit into the slot's state.
/// The payload shape is pinned by `L1TargetSession::checkpoint`: a boxed
/// `CacheState` and nothing else.
fn take_l1_cache(
    sessions: &mut SlotSessions,
    l1_rollup_id: eez_protocol::RollupId,
) -> Result<revm::database::CacheState, String> {
    let mut session = sessions
        .remove(&l1_rollup_id)
        .ok_or_else(|| format!("composition returned no session for L1 rollup {l1_rollup_id}"))?;
    let snapshot = session
        .checkpoint()
        .map_err(|e| format!("L1 session checkpoint: {e}"))?;
    snapshot
        .downcast::<revm::database::CacheState>()
        .map(|cache| *cache)
        .map_err(|_e| "L1 session checkpoint payload is not a CacheState".to_owned())
}

/// Hex of the first 32 bytes of a revert/return payload, for event messages.
fn truncated_hex(data: &Bytes) -> String {
    const MAX: usize = 32;
    if data.len() <= MAX {
        return alloy_primitives::hex::encode_prefixed(data);
    }
    format!(
        "{}… ({} bytes)",
        alloy_primitives::hex::encode_prefixed(&data[..MAX]),
        data.len()
    )
}

/// Composer umbrella. Cheaply [`Clone`]able (`Arc<Inner>`).
#[derive(Clone)]
pub struct Composer<L2: BlockReader> {
    inner: Arc<Inner<L2>>,
}

/// Invalid composer dependency wiring rejected at construction.
#[derive(Debug, thiserror::Error)]
pub enum ComposerConfigError {
    #[error(transparent)]
    Policy(#[from] PolicyError),
    /// The submitter observes a different L1 account than the one signing
    /// `postAndVerifyBatch`, so the Composer could misattribute its own batch.
    #[error(
        "L1 submission identity mismatch: Submitter poster {submitter_poster} does not match postBatch signer {post_batch_signer}"
    )]
    SubmissionIdentityMismatch {
        /// Account whose receipts and `BatchPosted` events the Submitter tracks.
        submitter_poster: Address,
        /// Account signing the `postAndVerifyBatch` transaction.
        post_batch_signer: Address,
    },
}

fn ensure_submission_identity(
    submitter_poster: Address,
    post_batch_signer: Address,
) -> Result<(), ComposerConfigError> {
    if submitter_poster != post_batch_signer {
        return Err(ComposerConfigError::SubmissionIdentityMismatch {
            submitter_poster,
            post_batch_signer,
        });
    }
    Ok(())
}

struct Inner<L2: BlockReader> {
    /// Per-rollup state keyed by rollup ID.
    rollups: HashMap<u64, RollupState<L2>>,
    /// Shared across rollups: one prover, one submitter.
    prover: Arc<dyn Prover>,
    submitter: Submitter,
    /// EVM config — used by [`build_sync_block`] to construct the
    /// per-Sync-slot block via reth-evm `BlockBuilder`.
    evm_config: EezEvmConfig,
    /// Cross-chain clients and execution context, guaranteed by the
    /// `eez-composer` entrypoint.
    cross_chain: CrossChainWiring,
    /// Handle to the binary-owned `BlockCommitter` actor (the sole engine-API
    /// owner), shared with the Sequencer and Deriver. Slot-context recovery
    /// uses it to reorg an optimistically committed Sync block after L1 failure.
    committer: BlockCommitterHandle<EezEngineTypes>,
    /// Per-block witnesses for [`eez_prover::ProvingContext::blocks`]. `None`
    /// means the configured in-process prover does not require block witnesses;
    /// it does not mean that the Composer has no prover.
    witness_source: Option<Arc<dyn eez_prover::ProvingWitnessSource>>,
    /// Bounds on what one postBatch may settle. Read from env once here so
    /// the emission decision, the boundary math, and the span guard in
    /// `prepare_post_batch_raw` can never disagree about the cap mid-run.
    emission: EmissionLimits,
    policy: policy::EnabledPolicy,
    posting: Arc<posting::PostingRuntime>,
    gas_trace: Option<gas_trace::Config>,
    max_user_txs_per_bundle: Option<usize>,
    /// Sync-block coinbase; the node configures Live blocks with the same value.
    fee_recipient: Address,
    automatic_admission: HashMap<u64, std::sync::Mutex<admission::AutomaticAdmission>>,
}

impl<L2: BlockReader> std::fmt::Debug for Composer<L2> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Composer")
            .field("rollup_ids", &self.inner.rollups.keys().collect::<Vec<_>>())
            .field("prover", &self.inner.prover)
            .field("submitter", &self.inner.submitter)
            .finish()
    }
}

impl<L2> Composer<L2>
where
    L2: BlockReader<Header = alloy_consensus::Header> + Send + Sync + 'static,
    <L2 as TransactionsProvider>::Transaction: Encodable2718,
{
    /// Construct the umbrella from launcher-validated settings, with pools
    /// bound to their attempt ledgers.
    ///
    /// # Errors
    ///
    /// Returns [`ComposerConfigError::SubmissionIdentityMismatch`] when the
    /// cross-chain postBatch signer does not match the Submitter's poster.
    pub fn new(
        rollups: HashMap<u64, RollupState<L2>>,
        prover: Arc<dyn Prover>,
        evm_config: EezEvmConfig,
        cross_chain: CrossChainWiring,
        committer: BlockCommitterHandle<EezEngineTypes>,
        witness_source: Option<Arc<dyn eez_prover::ProvingWitnessSource>>,
        settings: ComposerSettings,
    ) -> Result<Self, ComposerConfigError> {
        let submitter = cross_chain.exec_ctx.submitter.clone();
        let submitter_poster = submitter.poster_address();
        let post_batch_signer = cross_chain.exec_ctx.l1_poster_signer.address();
        ensure_submission_identity(submitter_poster, post_batch_signer)?;

        for rollup in rollups.values() {
            assert!(
                rollup.held_pool.uses_ledger(&rollup.optimistic),
                "held pool must use the rollup's optimistic ledger"
            );
            // Register event-counter baselines before the first submission.
            let _ = crate::submission_metrics::SubmissionMetrics::new(rollup.config.rollup_id);
        }

        let ComposerSettings {
            timing,
            policy: policy_config,
            max_blobs_per_batch: blob_cap,
            max_user_txs_per_bundle,
            posting_fees,
            gas_trace,
            emission,
            fee_recipient,
            ..
        } = settings;
        event!(name: "eez.composer.settlement.policy", Level::INFO,
            ?policy_config, "settlement policy settings apply until restart");
        let policy = policy::EnabledPolicy::new(policy_config, rollups.keys().copied(), blob_cap);
        let posting = Arc::new(posting::PostingRuntime {
            fees: posting_fees,
            chain: cross_chain.local.l1_entry.chain_provider().clone(),
            ctx: Arc::clone(&cross_chain.exec_ctx),
            registry: cross_chain.local.l1_entry.manager_address(),
            operator_gas_cap: emission.max_gas,
            operator_blob_cap: blob_cap,
            costs: std::sync::Mutex::default(),
            gas_trace: gas_trace.clone(),
        });
        event!(name: "eez.composer.emission.limits", Level::INFO,
            max_blocks = emission.max_blocks, configured_gas_cap = emission.max_gas,
            max_user_txs_per_bundle = ?max_user_txs_per_bundle,
            automatic_admission = max_user_txs_per_bundle.is_none(),
            k = timing.k(),
            %fee_recipient,
            "posting settings; policy resolves network gas limits per target");

        Ok(Self {
            inner: Arc::new(Inner {
                automatic_admission: rollups.keys().map(|&id| (id, Default::default())).collect(),
                rollups,
                prover,
                submitter,
                evm_config,
                cross_chain,
                committer,
                witness_source,
                emission,
                policy,
                posting,
                gas_trace,
                max_user_txs_per_bundle,
                fee_recipient,
            }),
        })
    }

    /// Run loop. Drains `l1_events` (subscribed by the caller before
    /// the `L1Watcher` starts): logs own/external
    /// `BatchPosted` attribution and drives optimistic recovery on
    /// `Reorg`/`Finalized` (the cursor and its retreats are owned by
    /// the Deriver via the shared `L1CanonicalHead`).
    ///
    /// Cross-chain Sync-slot composition is driven separately through
    /// the [`SyncSlotComposer`] trait (the Sequencer calls
    /// `compose_sync_slot` on its schedule), so this loop takes no
    /// batch-candidate input. Exits when the L1 event stream closes —
    /// the upstream `L1Watcher` task has exited.
    pub async fn run(self, mut l1_events: broadcast::Receiver<L1Event>) {
        let our_address = self.inner.submitter.poster_address();

        event!(
            name: "eez.composer.started",
            Level::INFO,
            rollup_count = self.inner.rollups.len(),
            our_address = %our_address,
            "composer umbrella loop started",
        );

        loop {
            let event = l1_events.recv().await;
            let closed = matches!(event, Err(broadcast::error::RecvError::Closed));
            self.inner.on_l1_event(&event, our_address);
            if closed {
                break;
            }
        }
    }
}

impl<L2> Inner<L2>
where
    L2: BlockReader<Header = alloy_consensus::Header> + Send + Sync + 'static,
    <L2 as TransactionsProvider>::Transaction: Encodable2718,
{
    /// Diagnostic-only handler for L1 events. State (cursor + reorg
    /// retreats) lives in the shared `L1CanonicalHead` written by the
    /// Deriver; here we just log own-vs-external batch attribution +
    /// flag the `expect_external_batches=false` violation when in
    /// sequenced mode.
    fn on_l1_event(
        &self,
        event: &Result<L1Event, broadcast::error::RecvError>,
        our_address: alloy_primitives::Address,
    ) {
        match event {
            Ok(L1Event::BatchPosted {
                l1_block_number,
                tx_hash,
                submitter,
                ..
            }) => {
                let is_ours = *submitter == our_address;
                if is_ours {
                    event!(
                        name: "eez.composer.batch.confirmed",
                        Level::INFO,
                        l1_block_number,
                        tx_hash = %tx_hash,
                        "our batch landed on L1",
                    );
                } else {
                    // Log level by whether any rollup expects external
                    // batches. Per-rollup attribution can refine once the
                    // BatchPosted event carries rollup_id.
                    let any_expects_external = self
                        .rollups
                        .values()
                        .any(|r| r.config.expect_external_batches);
                    if any_expects_external {
                        event!(
                            name: "eez.composer.batch.external",
                            Level::INFO,
                            l1_block_number,
                            tx_hash = %tx_hash,
                            submitter = %submitter,
                            "external batch landed (based mode)",
                        );
                    } else {
                        event!(
                            name: "eez.composer.batch.external.unexpected",
                            Level::ERROR,
                            l1_block_number,
                            tx_hash = %tx_hash,
                            submitter = %submitter,
                            "external batch landed in sequenced-mode rollup — someone else is sequencing our chain",
                        );
                    }
                }
            }
            Ok(L1Event::Reorg {
                common_ancestor_number,
                ..
            }) => {
                for rollup in self.rollups.values() {
                    rollup.l1_head.invalidate_scan();
                    if let Err(error) = rollup
                        .held_pool
                        .invalidate_after_l1(*common_ancestor_number)
                    {
                        event!(Level::ERROR, %error, "cannot apply L1 reorg; retaining transaction ownership");
                    }
                }
                self.posting.costs.lock().unwrap().invalidate_after(
                    *common_ancestor_number,
                    self.posting.ctx.l1_chain_id,
                    self.posting.native_currency(),
                );
            }
            Ok(L1Event::Finalized { block_number, .. }) => {
                for (&rollup_id, rollup) in &self.rollups {
                    let candidates = rollup.optimistic.finalization_candidates(*block_number);
                    let optimistic = Arc::clone(&rollup.optimistic);
                    let held_pool = Arc::clone(&rollup.held_pool);
                    let l1_head = Arc::clone(&rollup.l1_head);
                    let submitter = self.submitter.clone();
                    let posting = self.posting.clone();
                    tokio::spawn(async move {
                        for (id, block) in candidates {
                            if !optimistic.cost_recorded(id) {
                                continue;
                            }
                            match submitter.canonical_block_hash(block.number).await {
                                Ok(Some(hash)) if hash == block.hash => {
                                    if let Err(error) = optimistic.forget_finalized(id, block) {
                                        event!(Level::ERROR, %error, "finality update failed; retaining records");
                                    } else {
                                        posting
                                            .costs
                                            .lock()
                                            .unwrap()
                                            .forget_finalized(rollup_id, id.post_batch_hash);
                                    }
                                }
                                Ok(Some(_)) => {
                                    l1_head.invalidate_scan();
                                    posting.costs.lock().unwrap().invalidate_after(
                                        block.number.saturating_sub(1),
                                        posting.ctx.l1_chain_id,
                                        posting.native_currency(),
                                    );
                                    if let Err(error) = held_pool
                                        .invalidate_after_l1(block.number.saturating_sub(1))
                                    {
                                        event!(Level::ERROR, %error, "finality audit found a reorg; ownership invalidation failed");
                                    } else if let Err(error) =
                                        optimistic.forget_finalized(id, block)
                                    {
                                        // Invalidation reopened every canonical outcome still
                                        // owned here. Only fully transferred history or local
                                        // terminal policy decisions remain eligible for removal.
                                        event!(Level::ERROR, %error, "cannot prune resolved reorg history");
                                    }
                                }
                                other => event!(Level::WARN, result = ?other,
                                    "finality audit inconclusive; retaining recovery history"),
                            }
                        }
                    });
                }
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                event!(
                    name: "eez.composer.l1_events.lagged",
                    Level::WARN,
                    skipped,
                    "L1 event stream lagged",
                );
            }
            Err(broadcast::error::RecvError::Closed) => {
                event!(
                    name: "eez.composer.l1_events.closed",
                    Level::ERROR,
                    "L1 event stream closed",
                );
            }
        }
    }
}

#[async_trait]
impl<L2> SyncSlotComposer for Composer<L2>
where
    L2: BlockReader<Header = alloy_consensus::Header>
        + StateProviderFactory
        + Send
        + Sync
        + 'static,
    <L2 as TransactionsProvider>::Transaction: Encodable2718,
{
    fn has_cross_chain_work(&self, rollup_id: u64) -> bool {
        self.inner
            .rollups
            .get(&rollup_id)
            .is_some_and(|rollup| !rollup.held_pool.is_empty())
    }

    fn set_sequencing_target(&self, rollup_id: u64, sync_height: u64) {
        self.inner
            .policy
            .set_sequencing_target(rollup_id, sync_height);
    }

    fn ordinary_block_da_limit(
        &self,
        rollup_id: u64,
        parent: &reth_primitives_traits::SealedHeader<alloy_consensus::Header>,
        timestamp: u64,
    ) -> Result<usize, String> {
        self.sequencing_da_limit(rollup_id, parent, timestamp, false)
    }

    fn ordinary_transaction_da_limit(&self, rollup_id: u64) -> Result<usize, String> {
        let parent = self.inner.committer.last_header();
        let timestamp = parent
            .timestamp()
            .checked_add(self.inner.emission.timing.l2_block_time().as_secs())
            .ok_or("sequencing timestamp overflow")?;
        self.sequencing_da_limit(rollup_id, &parent, timestamp, true)
    }

    /// Drain `rollup_id`'s cross-chain `HeldPool`, compose its transactions,
    /// and select how the Sequencer handles the Sync boundary.
    ///
    /// Each eligible transaction runs through cross-chain simulation. Inbound
    /// survivors execute through the L1 bundle; outbound survivors execute in
    /// the constructed L2 Sync block and are carried in DA.
    async fn compose_sync_slot(
        &self,
        rollup_id: u64,
        parent: ParentContext,
        timestamp: u64,
        target_l1_block: Option<u64>,
        mode: SyncSlotMode,
    ) -> eez_driver::SyncSlotOutcome {
        // Bound the stack size of the whole slot orchestration future.
        let result = Box::pin(async {
        if let Some(source) = &self.inner.witness_source
            && let Err(error) = source.ensure_ready()
        {
            event!(name: "eez.composer.witness_resources.unavailable", Level::ERROR,
                rollup_id, %error, "durable witness resources unavailable; deferring new Sync block");
            return None;
        }
        // Some(n) → aim L1 block n exactly, pinned to this Sync block's
        // timestamp (a skipped L1 slot then drops the bundle instead of
        // settling with a drifted L2 timestamp); None → let the Submitter
        // derive a fresh future target from its canonical parent and pin that
        // target's expected timestamp (catch-up).
        let bundle_target = target_l1_block.map_or(BundleTarget::NextBlock, |block| {
            BundleTarget::Exact { block, timestamp }
        });
        event!(
            name: "eez.composer.sync_slot.invoked",
            Level::INFO,
            event_name = "eez.composer.sync_slot.invoked",
            rollup_id,
            timestamp,
            mode = ?mode,
            "compose_sync_slot invoked",
        );
        let rollup = self.inner.rollups.get(&rollup_id).or_else(|| {
            event!(
                name: "eez.composer.sync_slot.unknown_rollup",
                Level::ERROR,
                rollup_id,
                known_rollups = ?self.inner.rollups.keys().collect::<Vec<_>>(),
                "compose_sync_slot called for unknown rollup_id",
            );
            None
        })?;
        let pool = rollup.held_pool.as_ref();
        let cc = &self.inner.cross_chain;
        // Use the Sequencer-supplied parent header directly (it reflects
        // the just-committed block via `last_header`'s mirror); a
        // best-block re-lookup can race reth's provider-index and build
        // on a stale parent (see [`eez_driver::ParentContext`]).
        let parent_header = parent.header;
        let parent_number = parent_header.number();
        let suggested_fee_recipient = self.inner.fee_recipient;

        // Policy catch-up selects its earliest feasible target below.

        // Settled user dispositions require complete canonical evidence.
        // A matching height, including a competing batch, proves no user nonce.
        if let Some(attempt) = rollup.optimistic.pending_attempt() {
            match self.reconcile_attempt(rollup_id, rollup, attempt).await {
                Ok(RecoveryOutcome::HeadMoved) => return None,
                Ok(RecoveryOutcome::HeadUnchanged) => {}
                Err(error) => event!(Level::WARN, rollup_id, %error,
                    "settlement evidence incomplete; retaining all unresolved ownership"),
            }
        }
        let cursor = rollup.l1_head.cursor();
        let blocked = rollup.optimistic.blocking_height().is_some();
        let sync_height = parent_number + 1;
        self.log_settlement_backlog(rollup_id, cursor, sync_height, blocked);
        if blocked || mode != SyncSlotMode::Steady || pool.is_empty() {
            return match self
                .dispatch_minimal_postbatch(
                    &cc.exec_ctx,
                    rollup_id,
                    rollup,
                    &parent_header,
                    timestamp,
                    suggested_fee_recipient,
                    bundle_target,
                )
                .await
            {
                Ok(built) => built,
                Err(error) => {
                    event!(Level::ERROR, rollup_id, %error, "policy opportunity deferred; preserving pending settlement");
                    self.build_empty_slot_block(rollup, &parent_header, timestamp)
                }
            };
        }
        // ── Bounded emission ─────────────────────────────────────────
        // A stall holds `cursor` still while the head keeps cadence, growing the
        // range by K a slot until the signer's window wedges it.
        if sync_height.saturating_sub(cursor) > self.inner.emission.max_blocks {
            if let Err(err) = self
                .emit_historical_chunk(
                    &cc.exec_ctx,
                    rollup_id,
                    rollup,
                    cursor,
                    sync_height,
                    bundle_target,
                    "block_limit",
                )
                .await
            {
                event!(
                    name: "eez.composer.emission.historical_failed",
                    Level::ERROR,
                    rollup_id,
                    cursor,
                    sync_height,
                    error = %err,
                    "historical chunk emission failed; committing Sync block without emission — same bounded range retries next slot",
                );
            }
            return self.build_empty_slot_block(rollup, &parent_header, timestamp);
        }

        // Deferred-late: the bundle missed this Sync block's anchored L1 slot,
        // so suppress exact-target emission only. Late anchor-only catch-up
        // selects a feasible target in dispatch_minimal_postbatch above;
        // historical capacity splitting retains whichever target was selected.
        if matches!(mode, SyncSlotMode::Empty) {
            return self.build_empty_slot_block(rollup, &parent_header, timestamp);
        }

        // Catchup: structural-only — skip the drain, emit a minimal postBatch
        // (cross-chain stays pooled for the next Steady slot).
        if matches!(mode, SyncSlotMode::Catchup) {
            return self
                .dispatch_minimal_postbatch(
                    &cc.exec_ctx,
                    rollup_id,
                    rollup,
                    &parent_header,
                    timestamp,
                    suggested_fee_recipient,
                    bundle_target, // catch-up → derived future timestamp pin
                )
                .await
                .unwrap_or_else(|err| {
                    event!(
                        name: "eez.composer.catchup.minimal_failed",
                        Level::ERROR,
                        rollup_id,
                        error = %err,
                        "catchup minimal postBatch failed; Sequencer commits empty Sync",
                    );
                    None
                });
        }

        let pool_len_before = pool.len();
        // Select a finite FIFO snapshot. Automatic mode learns the preparation
        // cost; live gas/blob guards then admit only complete groups that fit.
        let automatic_mode = self.inner.max_user_txs_per_bundle.is_none();
        let mut automatic = self.inner.automatic_admission[&rollup_id]
            .lock()
            .unwrap()
            .begin(pool_len_before, self.inner.emission.timing, bundle_target);
        if let Some(limit) = self.inner.max_user_txs_per_bundle {
            automatic.selected = limit.min(pool_len_before);
            automatic.selection_limit_reason = Some("configured_count_limit");
        }
        let max_user_txs = automatic.selected;
        let drained = if max_user_txs > 0 {
            pool.candidates()
        } else {
            pool.pop_n(max_user_txs)
        };
        if drained.len() < automatic.selected {
            automatic.selected = drained.len();
            automatic.selection_limit_reason = Some("fifo_prefix_limit");
        }
        // When the pool is empty, do not return early. This slot attempts a
        // minimal postBatch whose leading immediate entry advances L1's stored
        // commitment to the empty Sync block's hash. Without it, L1's view
        // would stop advancing while L2 continues producing blocks. The L1
        // bundle contains only `postBatch` in this path.
        let candidate_count = drained.len();
        // Per-slot drain visibility (pool depth vs how many txs this bundle
        // took).
        event!(
            name: "eez.composer.sync_slot.drain",
            Level::INFO,
            event_name = "eez.composer.sync_slot.drain",
            rollup_id,
            cursor,
            parent_number,
            pool_len_before,
            candidate_count,
            admission_mode = if automatic_mode { "auto" } else { "fixed" },
            selection_limit = max_user_txs,
            "selected finite held-pool snapshot at Sync slot",
        );

        // Process held transactions through direction-specific nonce preflight
        // and simulation. The cross-chain path computes per-effect prefix block
        // hashes and stitches ABI root updates so their final endpoint equals
        // the locally built Sync-block hash.
        //
        // This path owns the composed Sync block, registers its survivors in
        // the optimistic ledger, and observes L1 settlement in the background.
        // Inbound source transactions execute through the L1 bundle; outbound
        // source transactions execute as user halves of L2 Sync pairs.
        // Boxed: the drain holds two live execution contexts, so its future is
        // past the inline-size budget.
        let result = Box::pin(self.compose_cross_chain_batch(
            cc,
            rollup_id,
            drained.clone(),
            &parent_header,
            timestamp,
            suggested_fee_recipient,
            bundle_target,
            &automatic,
        ))
        .await;
        if automatic_mode {
            self.inner.automatic_admission[&rollup_id]
                .lock()
                .unwrap()
                .finish(&automatic);
        }
            match result {
            Ok(Some(built)) => {
                event!(
                    name: "eez.composer.sync_slot.built",
                    Level::INFO,
                    rollup_id,
                    candidate_count,
                    parent_number,
                    timestamp,
                    "built Sync block after held-pool admission",
                );
                Some(built)
            }
            Ok(None) => None,
            Err(err) => {
                // Setup or fallback can fail after some candidates have been
                // classified. Do not restore evicted, released or requeued users.
                event!(
                    name: "eez.composer.cc_compose.failed",
                    Level::ERROR,
                    rollup_id,
                    error = %err,
                    "cross-chain compose failed; retaining held transactions while Sequencer commits a fallback Sync block",
                );
                pool.requeue_composing(drained);
                None
            }
            }
        })
        .await;
        match result {
            Some(built) => eez_driver::SyncSlotOutcome::Built(Box::new(built)),
            None => eez_driver::SyncSlotOutcome::EndTick,
        }
    }

    /// Reconcile pending source ownership on every tick, including between
    /// Sync slots. Observations alone cannot release reservations.
    async fn recover_failed(&self, rollup_id: u64) {
        let Some(rollup) = self.inner.rollups.get(&rollup_id) else {
            return;
        };
        for attempt in rollup.optimistic.reconciliation_attempts() {
            if let Err(error) = self.reconcile_attempt(rollup_id, rollup, attempt).await {
                event!(Level::WARN, rollup_id, %error,
                    "settlement recovery waits for conclusive canonical evidence");
            }
        }
        if let Err(error) = self.observe_settlement_cost(rollup_id, rollup).await {
            event!(Level::WARN, rollup_id, %error, "canonical receipt cost unavailable; retaining observability retry");
        }
    }

    async fn prepare_next_slot(&self, rollup_id: u64, target: u64, timestamp: u64) {
        if let Err(error) = self
            .replace_timed_out_attempt(rollup_id, target, timestamp)
            .await
        {
            event!(Level::WARN, rollup_id, %error,
                "next-slot replacement deferred; retaining submission history and users");
        }
    }
}

// Helper impl block for the cross-chain compose path. Lives outside
// the trait impl so we can use generic bounds independently.
impl<L2> Composer<L2>
where
    L2: BlockReader<Header = alloy_consensus::Header>
        + StateProviderFactory
        + Send
        + Sync
        + 'static,
    <L2 as TransactionsProvider>::Transaction: Encodable2718,
{
    /// Switch one active speculative branch at the next preparation deadline.
    /// Canonical derivation remains free to replace it if the original wins.
    async fn replace_timed_out_attempt(
        &self,
        rollup_id: u64,
        target: u64,
        timestamp: u64,
    ) -> Result<(), String> {
        use crate::optimistic::SubmissionObservation;
        let rollup = self.inner.rollups.get(&rollup_id).ok_or("unknown rollup")?;
        let Some(attempt) = rollup.optimistic.pending_attempt() else {
            return Ok(());
        };
        let Some((old_target, old_timestamp)) = attempt.dispatched_target else {
            return Ok(());
        };
        let timing = self.inner.emission.timing;
        if target != old_target
            || old_timestamp.checked_add(timing.l1_block_time().as_secs()) != Some(timestamp)
            || !matches!(
                attempt.observation,
                SubmissionObservation::Dispatched { .. } | SubmissionObservation::Uncertain
            )
        {
            return Ok(());
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_millis();
        let deadline = u128::from(old_timestamp) * 1_000 + timing.proof_window_open().as_millis();
        let cutoff = (u128::from(timestamp) * 1_000).saturating_sub(
            crate::prover_retry::POST_PROOF_RESERVE.as_millis()
                + timing.submission_slack().as_millis(),
        );
        if now < deadline || now >= cutoff {
            return Ok(());
        }
        let snapshot = rollup.l1_head.settlement_snapshot();
        let (Some(scanned), Some(endpoint)) = (snapshot.through, snapshot.endpoint) else {
            return Ok(());
        };
        let Some(parent) = self
            .inner
            .submitter
            .replacement_parent(target, timestamp)
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(());
        };
        if scanned != parent || endpoint.number >= attempt.sync_height {
            return Ok(());
        }
        let _guard = self
            .inner
            .committer
            .begin_reconcile()
            .await
            .map_err(|error| error.to_string())?;
        if !rollup.l1_head.generation_is(snapshot.generation)
            || self
                .inner
                .submitter
                .replacement_parent(target, timestamp)
                .await
                .map_err(|error| error.to_string())?
                != Some(parent)
        {
            return Ok(());
        }
        // Replace the old cross-chain Sync directly, so Reth reports the whole
        // removed suffix to its pool. An ancestor-only rewind loses ordinary
        // transactions in descendants. The sequencer rebuilds those blocks on
        // this empty Sync before composing the new terminal cross-chain group.
        let head = self.inner.committer.last_header();
        let own_hash = if head.number() == attempt.sync_height {
            Some(head.hash())
        } else {
            rollup
                .l2_provider
                .sealed_header(attempt.sync_height)
                .map_err(|error| error.to_string())?
                .map(|header| header.hash())
        };
        if own_hash != Some(attempt.manifest.sync_hash) {
            return Ok(());
        }
        let built = build_sync_block(
            rollup.l2_provider.as_ref(),
            &self.inner.evm_config,
            &attempt.parent,
            attempt
                .parent
                .timestamp()
                .checked_add(timing.l2_block_time().as_secs())
                .ok_or("replacement timestamp overflow")?,
            rollup
                .l1_head
                .next_block_context(&attempt.parent)
                .map_err(|e| e.to_string())?
                .1,
            Address::ZERO,
            &[],
        )
        .map_err(|error| error.to_string())?;
        if built.header.hash() != attempt.manifest.sync_hash {
            self.inner
                .committer
                .commit_derived(built.payload, built.header, true)
                .await
                .map_err(|error| error.to_string())?;
        }
        rollup.held_pool.release_for_replacement(attempt.id)?;
        event!(name: "eez.composer.settlement.replacement", Level::INFO,
            event_name = "eez.composer.settlement.replacement", rollup_id,
            original_post_batch = %attempt.id.post_batch_hash, target_block = target,
            original_timestamp = old_timestamp, replacement_timestamp = timestamp,
            "observation deadline passed; rebuilding for next slot with original submission retained as history");
        Ok(())
    }

    /// How many L2 blocks L1 has yet to record. Flat is healthy; monotonic
    /// growth is what every settlement stall looks like.
    fn log_settlement_backlog(
        &self,
        rollup_id: u64,
        cursor: u64,
        sync_height: u64,
        gate_blocked: bool,
    ) {
        let backlog = sync_height.saturating_sub(cursor);
        let max_blocks = self.inner.emission.max_blocks;
        if backlog > max_blocks.saturating_mul(2) {
            event!(
                name: "eez.composer.settlement.backlog",
                Level::WARN,
                rollup_id,
                cursor,
                sync_height,
                backlog,
                gate_blocked,
                max_blocks,
                "settlement backlog past 2x the batch cap; L1 is falling behind L2",
            );
        } else {
            event!(
                name: "eez.composer.settlement.backlog",
                Level::INFO,
                rollup_id,
                cursor,
                sync_height,
                backlog,
                gate_blocked,
                "settlement backlog",
            );
        }
    }

    /// Reconcile source nonces only against a completed canonical L1 scan.
    /// The engine lock is reacquired after network reads and the generation
    /// and L1 hash are rechecked before repairing L2 or transferring ownership.
    async fn reconcile_attempt(
        &self,
        rollup_id: u64,
        rollup: &RollupState<L2>,
        attempt: crate::optimistic::AttemptRecord,
    ) -> Result<RecoveryOutcome, String> {
        use crate::optimistic::{SourceOutcome, SubmissionObservation, TerminalReason};
        let snapshot = rollup.l1_head.settlement_snapshot();
        let (Some(at), Some(endpoint)) = (snapshot.through, snapshot.endpoint) else {
            return Ok(RecoveryOutcome::HeadUnchanged);
        };
        if attempt.observation == SubmissionObservation::Prepared {
            return Ok(RecoveryOutcome::HeadUnchanged);
        }
        if attempt
            .dispatched_target
            .is_some_and(|target| target.0 > at.number)
            || matches!(attempt.observation, SubmissionObservation::Included { block } if block.number > at.number)
        {
            return Ok(RecoveryOutcome::HeadUnchanged);
        }
        let submitter = &self.inner.submitter;
        let post = submitter
            .source_state_at(attempt.id.post_batch_hash, submitter.poster_address(), at)
            .await
            .map_err(|error| error.to_string())?;
        let evidence = snapshot.settlements.get(&attempt.id.post_batch_hash);
        if let Some(receipt) = post.receipt.as_ref().filter(|receipt| receipt.success) {
            let evidence = evidence.ok_or("included postBatch has not been reconciled")?;
            if evidence.l1_block != receipt.block || evidence.tx_index != receipt.transaction_index
            {
                return Err(
                    "postBatch receipt and reconciliation belong to different occurrences".into(),
                );
            }
            validate_attempt_evidence(&attempt, evidence)?;
        } else if evidence.is_some_and(|evidence| evidence.endpoint.is_some()) {
            return Err("canonical postBatch receipt is missing for applied settlement".into());
        } else if post.receipt.is_none() {
            if let Some((target, _)) = attempt.dispatched_target
                && submitter
                    .canonical_block_contains(target, attempt.id.post_batch_hash)
                    .await
                    .map_err(|error| error.to_string())?
            {
                return Err("postBatch receipt index is still catching up".into());
            }
            // A running observer may still classify an infrastructure failure.
            // Wait for its verdict so scheduling order cannot change charges.
            if matches!(
                attempt.observation,
                SubmissionObservation::Dispatched { .. }
            ) {
                return Ok(RecoveryOutcome::HeadUnchanged);
            }
        }

        // A skipped timestamp cannot reveal a bundle-size limit. Observer
        // labels alone are insufficient: bind a strict canonical timestamp gap
        // to this completed scan, and still classify every source below. Any
        // receipt, uncertain observation or unavailable header keeps backoff.
        let mut skipped_slot = false;
        if self.inner.max_user_txs_per_bundle.is_none()
            && !attempt.txs.is_empty()
            && post.receipt.is_none()
            && evidence.is_none()
            && let Some((target, timestamp)) = attempt.dispatched_target
            && matches!(attempt.observation,
                SubmissionObservation::Excluded { block, charge_attempt: false } if block == target)
        {
            match submitter
                .canonical_target_was_skipped(target, timestamp, at)
                .await
            {
                Ok(skipped) => skipped_slot = skipped,
                Err(error) => {
                    event!(Level::WARN, rollup_id, target, %error,
                        "skipped-slot evidence unavailable; retaining conservative admission backoff");
                }
            }
        }

        let mut inbound = Vec::with_capacity(attempt.txs.len());
        for tx in &attempt.txs {
            inbound.push(if tx.direction == Direction::Inbound {
                Some(
                    submitter
                        .source_state_at(tx.hash, tx.sender, at)
                        .await
                        .map_err(|error| error.to_string())?,
                )
            } else {
                None
            });
        }
        let _guard = self
            .inner
            .committer
            .begin_reconcile()
            .await
            .map_err(|error| error.to_string())?;
        if !rollup.l1_head.generation_is(snapshot.generation)
            || submitter
                .canonical_block_hash(at.number)
                .await
                .map_err(|error| error.to_string())?
                != Some(at.hash)
        {
            return Err("canonical settlement changed during source lookups".into());
        }
        // Read L2 nonces at the deriver's endpoint, never at the optimistic tip.
        let l2_state = rollup
            .l2_provider
            .state_by_block_hash(endpoint.hash)
            .map_err(|error| format!("canonical L2 source state: {error}"))?;
        let mut outcomes = Vec::with_capacity(attempt.txs.len());
        for ((tx, user), source) in attempt.txs.iter().zip(&attempt.manifest.users).zip(inbound) {
            let own_effects_applied = evidence.is_some_and(|settled| {
                !user.effects.is_empty()
                    && settled.applied_effects.start <= user.effects.start
                    && user.effects.end <= settled.applied_effects.end
            });
            let outcome = if let Some(source) = source {
                if let Some(receipt) = source.receipt {
                    if source.nonce <= tx.nonce {
                        return Err("source receipt and pinned source nonce disagree".into());
                    }
                    let reason = if !receipt.success {
                        TerminalReason::SourceReverted
                    } else if user.effects.is_empty() {
                        // Static execution leaves no mutable consumption log.
                        // The receipt proves source completion, not execution
                        // of every planned read.
                        TerminalReason::SourceCompleted
                    } else if own_effects_applied
                        && evidence
                            .is_some_and(|settled| source_uses_batch(&receipt, settled, &snapshot))
                        && user.source_actions.iter().all(|action| {
                            receipt
                                .consumed
                                .contains(&(action.call_hash, action.source_index))
                        })
                    {
                        TerminalReason::Committed
                    } else {
                        TerminalReason::SourceCompletedWithMissingEffects
                    };
                    SourceOutcome::Terminal(reason)
                } else if source.nonce > tx.nonce {
                    SourceOutcome::Terminal(TerminalReason::SourceNonceSpent)
                } else {
                    SourceOutcome::Retryable {
                        source_nonce: source.nonce,
                    }
                }
            } else {
                let nonce = l2_state
                    .account_nonce(&tx.sender)
                    .map_err(|error| format!("canonical L2 nonce: {error}"))?
                    .unwrap_or(0);
                if nonce <= tx.nonce {
                    if own_effects_applied {
                        return Err(
                            "validated outbound group did not spend its source nonce".into()
                        );
                    }
                    SourceOutcome::Retryable {
                        source_nonce: nonce,
                    }
                } else {
                    // Exact replay-group evidence remains valid through a
                    // same-height extension. A competing batch can include
                    // the same signed user; check its canonical metadata too.
                    let mut included = own_effects_applied;
                    if !included {
                        if let Some((_, metadata)) = rollup
                            .l2_provider
                            .transaction_by_hash_with_meta(tx.hash)
                            .map_err(|error| error.to_string())?
                        {
                            included = metadata.block_number <= endpoint.number
                                && rollup
                                    .l2_provider
                                    .sealed_header(metadata.block_number)
                                    .map_err(|error| error.to_string())?
                                    .is_some_and(|header| header.hash() == metadata.block_hash);
                        }
                        if !included {
                            included = rollup
                                .l2_provider
                                .transactions_by_block(endpoint.hash.into())
                                .map_err(|error| error.to_string())?
                                .is_some_and(|transactions| {
                                    transactions.iter().any(|transaction| {
                                        alloy_primitives::keccak256(transaction.encoded_2718())
                                            == tx.hash
                                    })
                                });
                        }
                    }
                    SourceOutcome::Terminal(if included {
                        TerminalReason::Committed
                    } else {
                        TerminalReason::SourceNonceSpent
                    })
                }
            };
            outcomes.push(outcome);
        }
        drop(l2_state);

        let mut result = RecoveryOutcome::HeadUnchanged;
        // Only an unapplied optimistic block still on our branch may be
        // replaced. Confirmed partial prefixes and competing canonical blocks
        // are repaired by the deriver and are never replaced here.
        if !attempt.txs.is_empty() && endpoint.number < attempt.sync_height {
            let head = self.inner.committer.last_header();
            let own_block = match head.number().cmp(&attempt.sync_height) {
                std::cmp::Ordering::Equal => Some(head.hash()),
                std::cmp::Ordering::Greater => rollup
                    .l2_provider
                    .sealed_header(attempt.sync_height)
                    .map_err(|error| error.to_string())?
                    .map(|header| header.hash()),
                std::cmp::Ordering::Less => None,
            };
            if own_block == Some(attempt.manifest.sync_hash) {
                let timestamp = attempt
                    .parent
                    .timestamp()
                    .checked_add(self.inner.emission.timing.l2_block_time().as_secs())
                    .ok_or("replacement timestamp overflow")?;
                let built = build_sync_block(
                    rollup.l2_provider.as_ref(),
                    &self.inner.evm_config,
                    &attempt.parent,
                    timestamp,
                    rollup
                        .l1_head
                        .next_block_context(&attempt.parent)
                        .map_err(|e| e.to_string())?
                        .1,
                    Address::ZERO,
                    &[],
                )
                .map_err(|error| error.to_string())?;
                // Switch directly to the replacement branch. Reth's bare
                // canonical-ancestor unwind updates its head without sending
                // the reorg notification used by pool maintenance. Unwinding
                // first would lose ordinary transactions in later Live blocks.
                // A branch switch reports the complete old chain, including
                // persisted blocks. The pool gate keeps our submitted sources
                // out of ordinary sequencing while their ownership reconciles.
                // A static-only inbound attempt may already have an empty
                // Sync. Nothing on L2 needs replacing in that case; pointing
                // at the same ancestor would unnecessarily discard descendants.
                if built.header.hash() != attempt.manifest.sync_hash {
                    self.inner
                        .committer
                        .commit_derived(built.payload, built.header, true)
                        .await
                        .map_err(|error| error.to_string())?;
                    result = RecoveryOutcome::HeadMoved;
                }
            }
        }
        let charge = post.receipt.is_some()
            || matches!(
                attempt.observation,
                SubmissionObservation::Excluded {
                    charge_attempt: true,
                    ..
                }
            );
        rollup.held_pool.resolve_attempt(
            attempt.id,
            at,
            &outcomes,
            charge,
            if self.inner.max_user_txs_per_bundle.is_none() {
                admission::MAX_AUTOMATIC_SETTLEMENT_ATTEMPTS
            } else {
                MAX_BUNDLE_ATTEMPTS
            },
            || rollup.l1_head.generation_is(snapshot.generation),
        )?;
        if self.inner.max_user_txs_per_bundle.is_none() && !outcomes.is_empty() && !skipped_slot {
            let mut automatic = self.inner.automatic_admission[&rollup_id].lock().unwrap();
            if !submission_backoff_required(&outcomes, false) {
                automatic.submission_completed();
            } else if charge {
                let next_selection_max = automatic.submission_failed(attempt.txs.len());
                event!(name: "eez.composer.admission.submission_failure", Level::INFO,
                    event_name = "eez.composer.admission.submission_failure", rollup_id,
                    submitted_groups = attempt.txs.len(),
                    consecutive_failures = automatic.submission_failures,
                    backoff_threshold = SUBMISSION_BACKOFF_THRESHOLD,
                    next_selection_max,
                    "confirmed incomplete settlement recorded for automatic admission");
                if let Some(next_selection_max) = next_selection_max {
                    event!(name: "eez.composer.admission.submission_backoff", Level::INFO,
                        event_name = "eez.composer.admission.submission_backoff", rollup_id,
                        submitted_groups = attempt.txs.len(), next_selection_max,
                        consecutive_failures = automatic.submission_failures,
                        "repeated incomplete settlements; reducing the next automatic selection");
                }
            }
        }
        if skipped_slot && !outcomes.is_empty() {
            event!(name: "eez.composer.admission.skipped_slot", Level::INFO,
                event_name = "eez.composer.admission.skipped_slot", rollup_id,
                submitted_groups = attempt.txs.len(), target = ?attempt.dispatched_target,
                "canonical target slot was skipped; source dispositions do not measure this candidate capacity");
        }
        event!(name: "eez.composer.settlement.reconciled", Level::INFO,
            event_name = "eez.composer.settlement.reconciled",
            rollup_id, attempt = ?attempt.id, at_l1 = at.number,
            post_batch_hash = %attempt.id.post_batch_hash,
            canonical_l2_number = endpoint.number, canonical_l2_hash = %endpoint.hash,
            outcomes = ?outcomes, charge_attempt = charge,
            "canonical source dispositions recorded and ownership transferred");
        Ok(result)
    }

    fn extend_blob_admission(
        &self,
        rollup_id: u64,
        budget: Option<admission::BlobAdmission>,
        semantic: &eez_protocol::blob_semantics::SemanticTransaction,
        companion: Option<&Bytes>,
        target: BundleTarget,
    ) -> Result<Option<(admission::BlobAdmission, bool)>, String> {
        let Some(budget) = budget else {
            return Ok(None);
        };
        let overhead = Self::cross_chain_da_overhead(rollup_id, std::slice::from_ref(semantic))
            .map_err(|e| e.to_string())?;
        let next = budget.extend(overhead, companion)?;
        let allowance = self.remaining_posting_blob_allowance(target, next.companion_blobs)?;
        Ok(Some((next, next.fits(allowance)?)))
    }

    /// Compose the drained transactions in canonical order over the slot's
    /// chained execution contexts, build the Sync block, register optimistic
    /// state, spawn L1 submission observation, and return the block for
    /// immediate L2 commit.
    ///
    /// Cadence: `CompositionBuilder::finalize` runs once per drained tx (via
    /// the phase arms below, each seeded with the slot's live sessions);
    /// [`Self::prepare_post_batch_raw`] runs once per slot, merging every
    /// survivor's `Composition`.
    ///
    /// # Errors
    /// Setup or fallback errors leave candidate ownership in the held pool.
    /// Callers restore only candidates still owned by this composition.
    async fn compose_cross_chain_batch(
        &self,
        cc: &CrossChainWiring,
        rollup_id: u64,
        drained: Vec<HeldTx>,
        parent_header: &reth_primitives_traits::SealedHeader<alloy_consensus::Header>,
        timestamp: u64,
        suggested_fee_recipient: Address,
        bundle_target: BundleTarget,
        admission: &admission::Attempt,
    ) -> Result<Option<SyncSlotBlock>, String> {
        let preparation_started_ms = crate::prover_retry::unix_time_millis();
        let ctx = cc.exec_ctx.as_ref();
        // Read the base SYSTEM_ADDRESS nonce from parent state. The canonical
        // builder allocates outbound-load nonces before inbound-delivery nonces.
        let parent_hash = parent_header.hash();
        let rollup =
            self.inner.rollups.get(&rollup_id).ok_or_else(|| {
                format!("unknown rollup_id {rollup_id} in compose_cross_chain_batch")
            })?;
        let (expected_timestamp, prev_randao) = rollup
            .l1_head
            .next_block_context(parent_header)
            .map_err(|e| e.to_string())?;
        if timestamp != expected_timestamp {
            return Err("candidate timestamp differs from the genesis schedule".into());
        }
        let pool = rollup.held_pool.as_ref();
        admission.witness_count(rollup.l1_head.settlement_snapshot().endpoint.map_or(
            usize::MAX,
            |endpoint| {
                usize::try_from(
                    parent_header
                        .number
                        .saturating_sub(endpoint.number)
                        .saturating_add(1),
                )
                .unwrap_or(usize::MAX)
            },
        ));

        let state = rollup
            .l2_provider
            .state_by_block_hash(parent_hash)
            .map_err(|e| format!("state_by_block_hash({parent_hash}): {e}"))?;
        let system_address = eez_primitives::SYSTEM_ADDRESS;
        // Base SYSTEM_ADDRESS nonce; the canonical builder advances it
        // internally (outbound loads, then inbound deliveries) post-drain.
        let nonce = state
            .account_nonce(&system_address)
            .map_err(|e| format!("account_nonce({system_address}): {e}"))?
            .unwrap_or(0);

        let stf_cfg = eez_protocol::system_tx::SystemTxContext {
            eezl2_address: ctx.eezl2_address,
            l2_chain_id: ctx.l2_chain_id,
            this_rollup_id: rollup_id,
        };

        // ── Optimistic chained compose (`docs/CHAINED-INTERSTATE-DESIGN.md` §4)
        // The Sync block commits to L2 immediately and a background task
        // observes the L1 bundle (see [`crate::optimistic`]). RICH when at
        // least one tx survives every build step; MINIMAL (postBatch with only
        // the leading immediate, empty Sync block) when the drain is empty, no
        // tx survives, or a build step fails.
        // The drain runs in canonical block order — all outbound, then all
        // inbound — over two slot-scoped states: the L1 state pinned at the
        // anchor and the Sync block under construction. Each tx simulates on a
        // FORK of both and, on accept, appends its canonical txs to the block
        // and commits its L1 effects to the state, so the next composition sees
        // its predecessors exactly as sequential execution will. Per tx: a
        // deterministic failure (sim, entry shape, reverting append) is POISON —
        // evict it plus its nonce cascade, rebuild the prefix, keep composing;
        // a transient failure aborts the slot — re-queue everything and degrade
        // to minimal.
        if drained.is_empty() {
            return self
                .dispatch_minimal_postbatch(
                    ctx,
                    rollup_id,
                    rollup,
                    parent_header,
                    timestamp,
                    suggested_fee_recipient,
                    bundle_target,
                )
                .await;
        }
        let candidate = {
            let mut previous = admission.previous_candidate.lock().unwrap();
            let candidate = Arc::new(crate::admission_events::Candidate::new(
                ctx.l1_chain_id,
                self.inner.posting.registry,
                rollup_id,
                parent_hash,
                bundle_target,
                drained.len(),
                pool.len(),
                (
                    admission.selected.min(drained.len()),
                    admission.selection_limit_reason.filter(|reason| {
                        admission.selected < drained.len() || *reason == "fifo_prefix_limit"
                    }),
                ),
                self.inner
                    .max_user_txs_per_bundle
                    .is_none()
                    .then_some(admission.submission_failures),
                previous.clone(),
            ));
            *previous = Some(candidate.id.clone());
            candidate
        };

        drop(state);

        // ── Slot execution contexts (design §2) ──────────────────────
        // One L1 state pinned at the anchor and one live Sync-block prefix,
        // both advanced only by accepted transactions. Failing to open either
        // is transient: nothing has been consumed, so the whole drain goes
        // back to the pool untouched.
        let local = &cc.local;
        let l2_dyn: Arc<dyn crate::local::provider::StateSnapshotProvider> =
            rollup.l2_provider.clone();
        // Rebuilding from the accepted list — not restoring a cache — is what
        // keeps the prefix provably equal to the block the canonical rebuild
        // produces.
        let reopen = |txs: &[Bytes]| {
            SyncBlockState::open(
                l2_dyn.clone(),
                &self.inner.evm_config,
                parent_header,
                timestamp,
                prev_randao,
                suggested_fee_recipient,
                txs,
            )
        };
        // Candidate-local execution checkpoints avoid rerunning every earlier
        // transaction at each effect boundary. A changed prospective prefix
        // invalidates reuse; final block and proof replay stay independent.
        let mut prefix_hashes = SyncPrefixHashes::new(
            rollup.l2_provider.as_ref(),
            &self.inner.evm_config,
            parent_header,
            timestamp,
            prev_randao,
            suggested_fee_recipient,
        );
        let mut prefix_hash_elapsed = std::time::Duration::ZERO;
        let mut hash_group = |prefix: &[Bytes], group: &[Bytes], positions: &[usize]| {
            let started = std::time::Instant::now();
            let txs: Vec<_> = prefix.iter().chain(group).cloned().collect();
            let positions: Vec<_> = positions.iter().map(|p| prefix.len() + p).collect();
            let result = prefix_hashes
                .at_positions(&txs, &positions)
                .map(|hashes| positions.into_iter().zip(hashes).collect::<Vec<_>>());
            prefix_hash_elapsed += started.elapsed();
            result
        };
        let composition_quote = self
            .inner
            .posting
            .quote(bundle_target, false)
            .await
            .map_err(|error| error.to_string())?;
        let slot_ctx = L1SlotState::open(&local.l1_entry)
            .map_err(|e| format!("L1SlotState::open: {e}"))
            .and_then(|mut state| {
                if composition_quote.quote.sample.hash != state.anchor.hash() {
                    return Err(
                        "L1 composition state changed while obtaining its target fees".into(),
                    );
                }
                state.posting = Some((
                    composition_quote.env,
                    ctx.l1_poster_signer.address(),
                    composition_quote.quote.estimated_effective_gas_price,
                ));
                reopen(&[])
                    .map(|draft| (state, draft))
                    .map_err(|e| format!("SyncBlockState::open: {e}"))
            });
        let (mut l1_state, mut draft) = match slot_ctx {
            Ok(contexts) => contexts,
            Err(e) => {
                event!(
                    name: "eez.composer.phase2.slot_setup_failed",
                    Level::WARN,
                    rollup_id,
                    error = %e,
                    drained = drained.len(),
                    "slot execution contexts unavailable; re-queueing the whole drain, degrading to minimal postBatch",
                );
                pool.requeue_composing(drained);
                return self
                    .dispatch_minimal_postbatch(
                        ctx,
                        rollup_id,
                        rollup,
                        parent_header,
                        timestamp,
                        suggested_fee_recipient,
                        bundle_target,
                    )
                    .await;
            }
        };
        event!(
            name: "eez.composer.phase2.slot_anchored",
            Level::INFO,
            rollup_id,
            l1_anchor = l1_state.anchor.number(),
            l1_anchor_hash = %l1_state.anchor.hash(),
            drained = drained.len(),
            "L1 state pinned and Sync block prefix opened for this slot",
        );

        // The Sync block's txs in canonical order, exactly as accepted: the
        // list the prefix is rebuilt from and the keystone assert compares the
        // canonical rebuild against.
        let mut sync_txs: Vec<Bytes> = Vec::new();
        let mut effect_hashes: Vec<(usize, B256)> = Vec::new();
        // SYSTEM_ADDRESS cursor: one nonce per appended system tx. Phase 1 runs
        // to completion first, so this counts outbound loads N..N+K-1 and then
        // inbound deliveries from N+K — `build_cross_chain_sync_pairs`' split,
        // by construction.
        let mut system_txs_appended: u64 = 0;

        // Drain indices ride along so a re-queue can restore pool order across
        // the two direction phases.
        let mut survivors: Vec<(usize, HeldTx)> = Vec::with_capacity(drained.len());
        // Inbound survivors' compositions (their `source.batch` = the L1
        // deferred entries) feed `prepare_post_batch_raw`'s merge.
        // Keep each inbound composition attached to its HeldTx. The same
        // compositions feed batch assembly and actionable entry resolution.
        let mut survivor_comps: Vec<(eez_protocol::Composition, B256)> =
            Vec::with_capacity(drained.len());
        // Native blob semantics preserve every accepted transaction boundary
        // and its resolved call forest. Phase order is canonical on the wire:
        // all L2-originating outbound transactions, then all L1-originating
        // inbound transactions.
        let mut semantic_transactions: Vec<eez_protocol::blob_semantics::SemanticTransaction> =
            Vec::with_capacity(drained.len());
        // Staged for the post-drain canonical rebuild, which must reproduce
        // `sync_txs` byte-for-byte. `pending_out` pairs an outbound settlement
        // entry with its user tx; `pending_in` holds inbound target-side
        // derivation entries; and `outbound_entries` holds settlement entries
        // spliced into `postBatch`.
        let mut pending_out: Vec<eez_protocol::system_tx::OutboundSyncGroup> = Vec::new();
        let mut pending_in: Vec<eez_protocol::entries::InboundSidecar> = Vec::new();
        let mut outbound_entries: Vec<eez_protocol::abi::ExecutionEntrySol> = Vec::new();
        let mut outbound_entry_traces: Vec<eez_protocol::entries::EntryTrace> = Vec::new();
        // Probed gas of the accepted outbound target calls; they re-execute
        // inside postAndVerifyBatch, so its gas limit must cover them.
        let mut outbound_target_gas: u64 = 0;
        // Escrow drawn down per outbound withdrawal (read once, lazily) so several
        // in one slot can't collectively over-drain. `None` = not yet read.
        let mut escrow_remaining: Option<U256> = None;
        let mut poison: Vec<HeldTx> = Vec::new();
        let mut poison_gaps: Vec<(Address, Direction, u64)> = Vec::new();
        // On a transient failure we abort the slot; this holds the error
        // string + the txs still needing re-queue (the failing tx + the
        // unprocessed remainder of both phases; survivors are added below).
        let mut transient: Option<(String, Vec<(usize, HeldTx)>)> = None;
        // Gas, not the tx cap, is what really bounds this bundle; overflow stays
        // held for later slots.
        let gas_budget = self
            .inner
            .posting
            .gas_budget(l1_state.anchor.header(), bundle_target)
            .map_err(|error| error.to_string())?;
        let estimated_budget = self.inner.posting.fees.estimate_budget(gas_budget);
        let mut budget = PostBatchGasBudget::with_calldata_limits(
            estimated_budget,
            self.posting_blob_allowance(bundle_target, 0)?,
            self.inner.posting.calldata_floor_active(bundle_target),
        );
        // This separate view never receives accepted speculative effects.
        // Failed lookups or unsupported prior queues retain conservative pins.
        let provider = local.l1_entry.chain_provider();
        let mut gas_projection = match provider
            .provider
            .state_by_block_hash(l1_state.anchor.hash())
            .map_err(|error| error.to_string())
            .and_then(|state| {
                gas::Projection::open(state.as_ref(), local.l1_entry.manager_address(), rollup_id)
            }) {
            Ok(projection) => projection,
            Err(error) => {
                event!(Level::WARN, rollup_id, %error, "gas shape evidence unavailable; retaining conservative reserves");
                None
            }
        };
        let mut gas_parent = if gas_projection.is_some() {
            L1TargetSession::new(&l1_state, local.l1_entry.clone()).ok()
        } else {
            None
        };
        if let Some(projection) = &gas_projection {
            budget.use_projection(projection.initial_gas());
        }
        let prior_cleanup = gas_projection.as_ref().map_or(0, |p| p.cleanup_gas);
        event!(name: "eez.composer.admission.gas_projection", Level::INFO,
            event_name = "eez.composer.admission.gas_projection", rollup_id,
            shaped = gas_projection.is_some(), prior_cleanup_gas = prior_cleanup,
            initial_gas = budget.projected, l1_anchor_hash = %l1_state.anchor.hash(),
            "postBatch admission reserves at the canonical parent");
        budget.block_cap = l1_state.anchor.gas_limit;
        budget.transaction_cap = self
            .inner
            .posting
            .transaction_gas_cap(l1_state.anchor.header(), bundle_target)
            .map_err(|error| error.to_string())?;
        // A deadline stops the drain; a capacity miss excludes every younger
        // source. Both sets remain owned and are restored without an attempt.
        let mut deferred: Vec<(usize, HeldTx)> = Vec::new();
        let mut capacity = CapacityDeferrals::default();
        let mut simulation_count = 0usize;
        let mut gas_constraint = None;
        // First gas refusal for the legacy cut event. If no group fits, this
        // describes the head considered for the existing unfittable eviction.
        // Individual observations retain all later constraints and costs.
        let mut rejected_cost: Option<u64> = None;
        // Retain diagnostics even when a later independent group fits.
        let mut block_gas_cut: Option<BlockGasCut> = None;
        let mut admission_cut: Option<&str> = None;
        let (ordinary_prefix_hash, mut blob_budget) = match build_sync_block(
            rollup.l2_provider.as_ref(),
            &self.inner.evm_config,
            parent_header,
            timestamp,
            prev_randao,
            suggested_fee_recipient,
            &[],
        ) {
            Ok(empty) => (
                empty.header.hash(),
                Some(admission::BlobAdmission::new(empty.block.into_block())),
            ),
            Err(error) => {
                event!(Level::WARN, rollup_id, %error, "admission template unavailable; retaining classified users");
                pool.requeue_composing(drained);
                return Ok(self.build_empty_slot_block(rollup, parent_header, timestamp));
            }
        };

        // Canonical block order is all outbound pairs then all inbound
        // deliveries, and L1 splits the same way physically (postBatch
        // immediates run before the bundle's user txs), so the drain composes
        // in two phases. A sender's nonce chain is never reordered: the two
        // directions live on different chains, and poison-gap bookkeeping is
        // keyed per (sender, direction).
        let mut selection = selection::Selection::new(
            &drained,
            admission.selected,
            l1_state.posting.as_ref().map_or_else(
                || l1_state.anchor.base_fee_per_gas.unwrap_or_default(),
                |(env, _, _)| env.block_env.basefee,
            ),
            draft.base_fee(),
        );
        let mut source_nonces = HashMap::new();
        let l1_anchor_hash = l1_state.anchor.hash();
        let mut source_nonce = |held: &HeldTx| -> Result<u64, String> {
            let key = (held.sender, held.direction);
            if let Some(nonce) = source_nonces.get(&key) {
                return Ok(*nonce);
            }
            let source = match held.direction {
                Direction::Outbound => rollup.l2_provider.state_by_block_hash(parent_hash),
                Direction::Inbound => local
                    .l1_entry
                    .chain_provider()
                    .provider
                    .state_by_block_hash(l1_anchor_hash),
            }
            .map_err(|e| format!("pinned source state: {e}"))?;
            let nonce = source
                .account_nonce(&held.sender)
                .map_err(|e| format!("pinned source nonce: {e}"))?
                .unwrap_or(0);
            source_nonces.insert(key, nonce);
            Ok(nonce)
        };
        let mut source_deferred = Vec::new();
        let source_state = crate::held_pool::SourceState {
            l1: eez_l1::L1BlockRef {
                number: l1_state.anchor.number(),
                hash: l1_state.anchor.hash(),
            },
            l2: eez_l1::L1BlockRef {
                number: parent_header.number(),
                hash: parent_hash,
            },
        };
        pool.begin_evaluation(source_state);
        let snapshot = drained.clone();
        // Observations follow examined work. Re-emitting every untouched suffix
        // on each candidate would make retained telemetry quadratic in backlog.
        let mut examined = std::collections::HashSet::new();
        let mut selection_filtered = 0usize;
        let (outbounds, mut inbounds): (Vec<(usize, HeldTx)>, Vec<(usize, HeldTx)>) = drained
            .into_iter()
            .enumerate()
            .partition(|(_, held)| held.direction == Direction::Outbound);

        // ── PHASE 1 — OUTBOUND (L2→L1) ───────────────────────────────
        // Source-sim runs on a fork of the Sync block against the L2 ENTRY
        // client (the L2 follower errors `Unavailable`); the L1 side executes
        // real `_processNCalls` frames on a fork of the state. On accept the
        // canonical `[load, user]` pair extends the block and the L1 frames
        // commit to the state.
        let mut out_iter = outbounds.into_iter();
        while let Some((idx, held)) = out_iter.next() {
            if !selection.consider(idx) {
                selection_filtered += 1;
                continue;
            }
            examined.insert(idx);
            candidate.emit(
                "source_selected",
                serde_json::json!({"source_hash":held.hash,
                "position":idx,"timing_basis":"source_checks","direction":match held.direction {
                    Direction::Inbound => "inbound", Direction::Outbound => "outbound",
                }}),
            );
            if !pool.claim(&held) {
                candidate.emit(
                    "source_outcome",
                    serde_json::json!({"source_hash":held.hash,"reason":"ownership_changed"}),
                );
                // A replacement is still older queued work. Revisit a fresh
                // snapshot rather than treating its chain as ineligible.
                selection.cut(idx);
                admission_cut = Some("snapshot_changed");
                continue;
            }
            let baseline = source_nonce(&held)?;
            if held.nonce < baseline {
                if pool.recheck_stale_sources(std::slice::from_ref(&held))? {
                    return Err("source nonce advanced; canonical reconciliation required".into());
                }
                selection.discard(idx);
                continue;
            }
            if !admission.can_check_group(survivors.len()) {
                admission_cut = Some("pipeline_time");
                deferred = abort_rest(
                    Some((idx, held)),
                    &mut out_iter,
                    std::mem::take(&mut inbounds),
                );
                break;
            }
            if let Some(gap_at) = poison_gap_for(&poison_gaps, &held) {
                event!(
                    name: "eez.composer.cc_compose.poison_chain_evicted",
                    Level::WARN,
                    rollup_id,
                    tx_idx = idx,
                    tx_hash = %held.hash,
                    sender = %held.sender,
                    nonce = held.nonce,
                    gap_at,
                    "same-sender tx above an evicted poison nonce in the same drain; evicted (resubmit in order)",
                );
                poison.push(held);
                continue;
            }
            if let Some(reason) =
                capacity.minimum_check(&candidate, &held, budget, draft.gas_used())
            {
                if reason == "l2_block_capacity" {
                    block_gas_cut = Some(BlockGasCut {
                        gas_used: draft.gas_used(),
                        declared: MIN_TX_GAS.saturating_add(declared_gas_limit(&held.raw_tx)),
                    });
                } else if gas_constraint.is_none() {
                    gas_constraint = Some(reason);
                    rejected_cost = Some(gas::minimum_entry_gas(true));
                }
                capacity.defer(&mut selection, &candidate, idx, held, reason);
                continue;
            }
            if !admission.can_start_group(survivors.len()) {
                admission_cut = Some("pipeline_time");
                deferred = abort_rest(
                    Some((idx, held)),
                    &mut out_iter,
                    std::mem::take(&mut inbounds),
                );
                break;
            }
            let contexts = L1TargetSession::new(&l1_state, local.l1_entry.clone())
                .map_err(|e| format!("L1TargetSession::new: {e}"))
                .and_then(|exec| {
                    draft
                        .fork()
                        .map(|fork| (exec, fork))
                        .map_err(|e| format!("SyncBlockState::fork: {e}"))
                });
            let (l1_exec, mut l2_fork) = match contexts {
                Ok(contexts) => contexts,
                Err(e) => {
                    transient = Some((
                        format!("outbound execution contexts tx#{idx}: {e}"),
                        abort_rest(
                            Some((idx, held)),
                            &mut out_iter,
                            std::mem::take(&mut inbounds),
                        ),
                    ));
                    break;
                }
            };
            let sessions = seed_session(cc.entry_rollup_id, l1_exec);
            let (state, env) = l2_fork.state_and_env();
            let env = env.clone();
            simulation_count += 1;
            let sim = compose_crosschain(
                cc,
                eez_protocol::RollupId(rollup_id),
                &local.l2_entry,
                held.raw_tx.as_ref(),
                sessions,
                state,
                env,
                held.hash,
                l1_state.anchor.hash(),
                self.inner.gas_trace.as_ref(),
            );
            match sim {
                Ok((composition, mut sessions, gas)) => {
                    let target_gas = gas.l1_target;
                    if composition.targets.iter().any(|target| {
                        target.rollup_id != eez_protocol::RollupId::MAINNET
                            && target.rollup_id != eez_protocol::RollupId(rollup_id)
                    }) {
                        event!(
                            name: "eez.composer.cc_compose.outbound_foreign_target",
                            Level::WARN,
                            rollup_id,
                            tx_idx = idx,
                            tx_hash = %held.hash,
                            "outbound composition reached an L2 other than its source rollup; this node cannot settle another rollup's Sync block",
                        );
                        selection.block(idx);

                        push_poison_root(&mut poison, &mut poison_gaps, held);
                        continue;
                    }
                    let Some(l1_target) = composition
                        .targets
                        .iter()
                        .find(|target| target.rollup_id == eez_protocol::RollupId::MAINNET)
                    else {
                        event!(name: "eez.composer.cc_compose.outbound_no_entries", Level::WARN, rollup_id, tx_idx = idx, tx_hash = %held.hash, "outbound tx produced no L1 host entry; evicting");
                        selection.block(idx);

                        push_poison_root(&mut poison, &mut poison_gaps, held);
                        continue;
                    };
                    let l1_entries = l1_target.batch.entries.clone();
                    if l1_entries.is_empty() {
                        event!(
                            name: "eez.composer.cc_compose.outbound_no_entries",
                            Level::WARN,
                            rollup_id,
                            tx_idx = idx,
                            tx_hash = %held.hash,
                            "outbound tx produced no L1 settlement entry; evicting (resubmit required)",
                        );
                        selection.block(idx);

                        push_poison_root(&mut poison, &mut poison_gaps, held);
                        continue;
                    }
                    let source_entries = &composition.source.batch.entries;
                    let source_static_entries = &composition.source.l2_static_entries;
                    let settlement_root_counts =
                        match eez_protocol::system_tx::outbound_settlement_root_counts(&l1_entries)
                        {
                            Ok(counts) => counts,
                            Err(error) => {
                                event!(
                                    name: "eez.composer.cc_compose.outbound_root_shape",
                                    Level::WARN,
                                    rollup_id,
                                    tx_idx = idx,
                                    tx_hash = %held.hash,
                                    error = %error,
                                    "outbound settlement commitment partition is unsupported; evicting",
                                );
                                selection.block(idx);

                                push_poison_root(&mut poison, &mut poison_gaps, held);
                                continue;
                            }
                        };
                    let settlement_source_count = settlement_root_counts.iter().sum::<usize>();
                    if source_entries.len() + source_static_entries.len() != settlement_source_count
                    {
                        event!(
                            name: "eez.composer.cc_compose.outbound_root_mismatch",
                            Level::WARN,
                            event_name = "eez.composer.cc_compose.outbound_root_mismatch",
                            rollup_id,
                            tx_idx = idx,
                            tx_hash = %held.hash,
                            source_entries = source_entries.len(),
                            source_static_entries = source_static_entries.len(),
                            settlement_entries = l1_entries.len(),
                            settlement_source_count,
                            "outbound source roots are not fully covered by the L1 settlement hosts; evicting",
                        );
                        selection.block(idx);

                        push_poison_root(&mut poison, &mut poison_gaps, held);
                        continue;
                    }
                    let separate_callback_entries = composition
                        .targets
                        .iter()
                        .filter(|target| target.rollup_id == eez_protocol::RollupId(rollup_id))
                        .flat_map(|target| target.batch.entries.iter().cloned())
                        .map(|entry| {
                            eez_protocol::entries::InboundSidecar::for_rollup(
                                &entry,
                                eez_protocol::RollupId(rollup_id),
                            )
                        })
                        .collect::<Result<Vec<_>, _>>();
                    let separate_callback_entries = match separate_callback_entries {
                        Ok(entries) => entries,
                        Err(error) => {
                            event!(Level::WARN, rollup_id, tx_idx = idx, tx_hash = %held.hash,
                                %error, "invalid outbound callback sidecar; evicting");
                            selection.block(idx);
                            push_poison_root(&mut poison, &mut poison_gaps, held);
                            continue;
                        }
                    };
                    let callback_counts = &composition.source.callback_entry_counts;
                    if callback_counts.len() != source_entries.len()
                        || callback_counts.iter().sum::<usize>() != separate_callback_entries.len()
                    {
                        event!(
                            name: "eez.composer.cc_compose.outbound_callback_mismatch",
                            Level::WARN,
                            rollup_id,
                            tx_idx = idx,
                            tx_hash = %held.hash,
                            callback_entries = separate_callback_entries.len(),
                            callback_counts = ?callback_counts,
                            source_entries = source_entries.len(),
                            "outbound callback-delivery partition does not match its source roots; evicting",
                        );
                        selection.block(idx);

                        push_poison_root(&mut poison, &mut poison_gaps, held);
                        continue;
                    }
                    let mut callback_cursor = 0usize;
                    let mut effects = source_entries
                        .iter()
                        .cloned()
                        .zip(callback_counts.iter().copied())
                        .map(|(source_entry, callback_count)| {
                            let callback_end = callback_cursor + callback_count;
                            let callback_entries =
                                separate_callback_entries[callback_cursor..callback_end].to_vec();
                            callback_cursor = callback_end;
                            eez_protocol::system_tx::OutboundSyncEffect {
                                source_entry: eez_protocol::system_tx::OutboundSourceEntry::Mutable(
                                    source_entry,
                                ),
                                callback_entries,
                            }
                        })
                        .collect::<Vec<_>>();
                    effects.extend(source_static_entries.iter().cloned().map(|source_entry| {
                        eez_protocol::system_tx::OutboundSyncEffect {
                            source_entry: eez_protocol::system_tx::OutboundSourceEntry::Static(
                                source_entry,
                            ),
                            callback_entries: Vec::new(),
                        }
                    }));
                    let outbound_group = eez_protocol::system_tx::OutboundSyncGroup {
                        effects,
                        user_tx: held.raw_tx.clone(),
                    };
                    // Gate a withdrawal that exceeds the rollup's L1 escrow —
                    // it would revert on-chain and drop the whole bundle.
                    // "ether out" is the amount of Ether being withdrawn in this outbound settlement entry.
                    // If missing, the entry is malformed and must be evicted.
                    // Review once reentrancy is available
                    let Some(need) = l1_entries.iter().try_fold(U256::ZERO, |total, entry| {
                        eez_protocol::entries::outbound_ether_out(entry)
                            .and_then(|value| total.checked_add(value))
                    }) else {
                        event!(name: "eez.composer.cc_compose.outbound_ether_out_missing", Level::WARN, rollup_id, tx_idx = idx, tx_hash = %held.hash, "outbound tx is missing ether out entry, likely malformed; evicting");
                        selection.block(idx);

                        push_poison_root(&mut poison, &mut poison_gaps, held);
                        continue;
                    };
                    // Check here (cheap early eviction, before any append work);
                    // the debit happens at accept below.
                    if need > U256::ZERO {
                        if escrow_remaining.is_none() {
                            match read_rollup_escrow(
                                &ctx.l1_provider,
                                rollup_id,
                                l1_state.anchor.hash(),
                            )
                            .await
                            {
                                Ok(escrow) => escrow_remaining = Some(escrow),
                                Err(error) => {
                                    transient = Some((
                                        format!("outbound escrow read tx#{idx}: {error}"),
                                        abort_rest(
                                            Some((idx, held)),
                                            &mut out_iter,
                                            std::mem::take(&mut inbounds),
                                        ),
                                    ));
                                    break;
                                }
                            }
                        }
                        if let Some(avail) = escrow_remaining
                            && need > avail
                        {
                            defer_source(
                                pool,
                                &mut selection,
                                &mut source_deferred,
                                source_state,
                                idx,
                                held,
                                eez_protocol::SourceCondition::Escrow {
                                    required: need,
                                    available: avail,
                                },
                            );
                            continue;
                        }
                    }
                    // `[load, user]` needs at least the load's base cost plus the
                    // user tx's declared limit. Before the L1 budget, so a refusal
                    // costs nothing; the probe below settles the exact fit.
                    let declared = MIN_TX_GAS.saturating_add(declared_gas_limit(&held.raw_tx));
                    match block_gas_fit(draft.gas_used(), declared, BUILDER_GAS_LIMIT) {
                        BlockGasFit::Accept => {}
                        BlockGasFit::Defer => {
                            block_gas_cut = Some(BlockGasCut {
                                gas_used: draft.gas_used(),
                                declared,
                            });
                            capacity.defer(
                                &mut selection,
                                &candidate,
                                idx,
                                held,
                                "l2_block_capacity",
                            );
                            continue;
                        }
                        BlockGasFit::Unfittable => {
                            event!(
                                name: "eez.composer.cc_compose.block_gas_unfittable",
                                Level::WARN,
                                rollup_id,
                                tx_idx = idx,
                                tx_hash = %held.hash,
                                declared,
                                block_gas_limit = BUILDER_GAS_LIMIT,
                                "outbound [load, user] pair declares more gas than a whole L2 block; no slot can ever take it — evicting (resubmit with a lower gas limit)",
                            );
                            selection.block(idx);

                            push_poison_root(&mut poison, &mut poison_gaps, held);
                            continue;
                        }
                    }
                    // Gate before accept: this entry settles inline, so over
                    // the cap the whole bundle dies silently (invariant 7).
                    let established = match (&mut gas_projection, &mut gas_parent) {
                        (Some(projection), Some(parent)) => {
                            match projection.immediate_evidence(
                                &l1_entries,
                                &composition.actions,
                                parent,
                            ) {
                                Ok(evidence) => evidence,
                                Err(error) => {
                                    event!(Level::DEBUG, %error, "proxy gas evidence unavailable; retaining entry reserve");
                                    None
                                }
                            }
                        }
                        _ => None,
                    };
                    let cost = gas_projection.as_ref().map_or_else(
                        || projected_tx_l1_gas(&l1_entries, target_gas),
                        |projection| projection.cost(&l1_entries, target_gas, true, established),
                    );
                    let mut next_budget = budget;
                    let gas_accepted = next_budget.try_accept(cost);
                    trace_admission_budget(
                        self.inner.gas_trace.as_ref(),
                        &candidate,
                        held.hash,
                        l1_state.anchor.hash(),
                        "outbound",
                        budget,
                        next_budget,
                        cost,
                        &l1_entries,
                        gas,
                        0,
                        gas::CompanionEstimate::declared(0),
                        prior_cleanup,
                        gas_accepted,
                    );
                    if !gas_accepted {
                        if budget.blocked_by_cleanup(cost, 0, 0, prior_cleanup) {
                            admission_cut = Some("prior_queue_cleanup");
                        }
                        if gas_constraint.is_none() {
                            gas_constraint = next_budget.rejected_constraint;
                            rejected_cost = Some(cost.total());
                        }
                        let reason = if budget.blocked_by_cleanup(cost, 0, 0, prior_cleanup) {
                            "prior_queue_cleanup"
                        } else {
                            next_budget
                                .rejected_constraint
                                .unwrap_or("l1_tx_gas_budget")
                        };
                        capacity.defer(&mut selection, &candidate, idx, held, reason);
                        continue;
                    }
                    // ── ACCEPT ───────────────────────────────────────
                    // Block first, state second: a pair evicted at append must
                    // leave the L1 state untouched. `build_outbound_pair` runs
                    // the shape gate itself, so an entry the Sync-block lowering
                    // cannot represent comes back as its `Err`.
                    let pairs_k = match eez_protocol::system_tx::build_cross_chain_sync_groups(
                        std::slice::from_ref(&outbound_group),
                        &[],
                        &stf_cfg,
                        nonce + system_txs_appended,
                    ) {
                        Ok(pairs) => pairs,
                        Err(e) => {
                            event!(
                                name: "eez.composer.cc_compose.shape_evicted",
                                Level::WARN,
                                rollup_id,
                                tx_idx = idx,
                                tx_hash = %held.hash,
                                error = %e,
                                "outbound entry shape is not representable in a Sync block; evicting (resubmit required)",
                            );
                            selection.block(idx);

                            push_poison_root(&mut poison, &mut poison_gaps, held);
                            continue;
                        }
                    };
                    let pair_txs = eez_protocol::system_tx::interleave_sync_block_txs(&pairs_k);
                    let semantic = eez_protocol::blob_semantics::SemanticTransaction {
                        origin_chain: eez_protocol::RollupId(rollup_id),
                        tx_data: held.raw_tx.clone(),
                        actions: composition.actions.clone(),
                    };
                    let next_blob = match self.extend_blob_admission(
                        rollup_id,
                        blob_budget,
                        &semantic,
                        None,
                        bundle_target,
                    ) {
                        Ok(Some((next, false))) if !survivors.is_empty() => {
                            event!(name: "eez.composer.admission.blob_cut", Level::INFO,
                                event_name = "eez.composer.admission.blob_cut", rollup_id,
                                companion_blobs = next.companion_blobs,
                                "terminal Sync would exceed the target's remaining blob allowance");
                            capacity.defer(
                                &mut selection,
                                &candidate,
                                idx,
                                held,
                                "composition_blob_limit",
                            );
                            continue;
                        }
                        Ok(next) => next.map(|(next, _)| next),
                        Err(error) => {
                            transient = Some((
                                error,
                                abort_rest(
                                    Some((idx, held)),
                                    &mut out_iter,
                                    std::mem::take(&mut inbounds),
                                ),
                            ));
                            break;
                        }
                    };
                    // The prefix-hash rebuild runs the canonical executor, which
                    // treats a group that does not fit as a hard error. Settle the
                    // fit first, as a capacity decision rather than a fault.
                    if let Err((at, why)) = draft.capacity_probe(&pair_txs) {
                        if why.is_provider() {
                            transient = Some((
                                format!("outbound capacity probe tx#{idx} at {at}: {why}"),
                                abort_rest(
                                    Some((idx, held)),
                                    &mut out_iter,
                                    std::mem::take(&mut inbounds),
                                ),
                            ));
                            break;
                        }
                        let declared = declared_group_gas(&pair_txs);
                        if capacity_refusal(sync_txs.is_empty()) == BlockGasFit::Unfittable {
                            event!(
                                name: "eez.composer.cc_compose.block_gas_unfittable",
                                Level::WARN,
                                rollup_id,
                                tx_idx = idx,
                                tx_hash = %held.hash,
                                declared,
                                refused_at = at,
                                error = %why,
                                block_gas_limit = BUILDER_GAS_LIMIT,
                                "outbound [load, user] pair does not fit an otherwise empty L2 block; no slot can ever take it — evicting",
                            );
                            selection.block(idx);

                            push_poison_root(&mut poison, &mut poison_gaps, held);
                            continue;
                        }
                        block_gas_cut = Some(BlockGasCut {
                            gas_used: draft.gas_used(),
                            declared,
                        });
                        capacity.defer(&mut selection, &candidate, idx, held, "l2_block_capacity");
                        continue;
                    }
                    let group_hashes = match hash_group(
                        &sync_txs,
                        &pair_txs,
                        &eez_protocol::system_tx::sync_pair_end_positions(&pairs_k),
                    ) {
                        Ok(hashes) => hashes,
                        Err(error) => {
                            transient = Some((
                                format!("outbound prefix hashes tx#{idx}: {error}"),
                                abort_rest(
                                    Some((idx, held)),
                                    &mut out_iter,
                                    std::mem::take(&mut inbounds),
                                ),
                            ));
                            break;
                        }
                    };
                    if let Some(next) = next_blob
                        && !admission.can_finish_group(survivors.len() + 1, next.required_blobs()?)
                    {
                        admission_cut = Some("pipeline_time");
                        deferred = abort_rest(
                            Some((idx, held)),
                            &mut out_iter,
                            std::mem::take(&mut inbounds),
                        );
                        break;
                    }
                    let pair_logs = match append_and_execute(&mut draft, &pair_txs) {
                        Ok(logs) => logs,
                        Err((at, why)) => {
                            // A backing-store failure says nothing about the tx —
                            // abort the slot instead of evicting a valid pair.
                            if why.is_provider() {
                                transient = Some((
                                    format!("outbound append tx#{idx} at {at}: {why}"),
                                    abort_rest(
                                        Some((idx, held)),
                                        &mut out_iter,
                                        std::mem::take(&mut inbounds),
                                    ),
                                ));
                                break;
                            }
                            // The probe already ran on the same state, so this is
                            // a defensive path; it keeps a capacity refusal from
                            // ever being charged to the tx.
                            if why.is_capacity() {
                                let unfittable = capacity_refusal(sync_txs.is_empty())
                                    == BlockGasFit::Unfittable;
                                draft = match reopen(&sync_txs) {
                                    Ok(rebuilt) => rebuilt,
                                    Err(e) => {
                                        transient = Some((
                                            format!(
                                                "Sync block prefix rebuild after tx#{idx}: {e}"
                                            ),
                                            abort_rest(
                                                Some((idx, held)),
                                                &mut out_iter,
                                                std::mem::take(&mut inbounds),
                                            ),
                                        ));
                                        break;
                                    }
                                };
                                if unfittable {
                                    event!(
                                        name: "eez.composer.cc_compose.block_gas_unfittable",
                                        Level::WARN,
                                        rollup_id,
                                        tx_idx = idx,
                                        tx_hash = %held.hash,
                                        refused_at = at,
                                        error = %why,
                                        "outbound [load, user] pair does not fit an otherwise empty L2 block — evicting",
                                    );
                                    selection.block(idx);
                                    push_poison_root(&mut poison, &mut poison_gaps, held);
                                } else {
                                    block_gas_cut = Some(BlockGasCut {
                                        gas_used: draft.gas_used(),
                                        declared: declared_group_gas(&pair_txs),
                                    });
                                    capacity.defer(
                                        &mut selection,
                                        &candidate,
                                        idx,
                                        held,
                                        "l2_block_capacity",
                                    );
                                }
                                continue;
                            }
                            event!(
                                name: "eez.composer.cc_compose.append_reverted",
                                Level::WARN,
                                rollup_id,
                                tx_idx = idx,
                                tx_hash = %held.hash,
                                appended_idx = at,
                                error = %why,
                                "outbound [load, user] pair does not execute on the Sync block prefix; evicting the tx and rebuilding the prefix",
                            );
                            selection.block(idx);

                            push_poison_root(&mut poison, &mut poison_gaps, held);
                            // A failed append may be half-applied; the accepted
                            // list is the only truth to rebuild from.
                            draft = match reopen(&sync_txs) {
                                Ok(rebuilt) => rebuilt,
                                Err(e) => {
                                    transient = Some((
                                        format!("Sync block prefix rebuild after tx#{idx}: {e}"),
                                        abort_rest(
                                            None,
                                            &mut out_iter,
                                            std::mem::take(&mut inbounds),
                                        ),
                                    ));
                                    break;
                                }
                            };
                            continue;
                        }
                    };
                    // The entry was composed from a sim that ran WITHOUT the
                    // preceding load tx, so re-check it against what the user tx
                    // actually did here (`docs/issues/outbound-sim-divergence.md`).
                    let observed = eez_protocol::outbound_gate::observations_from_logs(
                        &pair_logs,
                        stf_cfg.eezl2_address,
                    );
                    if let Err(e) = eez_protocol::outbound_gate::verify_expected_outbound_hashes(
                        &composition.source.outbound_event_hashes,
                        &observed,
                    ) {
                        event!(
                            name: "eez.composer.cc_compose.outbound_unobserved",
                            Level::WARN,
                            rollup_id,
                            tx_idx = idx,
                            tx_hash = %held.hash,
                            error = %e,
                            "composed outbound entry does not match the call the tx made on the Sync block; evicting",
                        );
                        selection.block(idx);

                        push_poison_root(&mut poison, &mut poison_gaps, held);
                        draft = match reopen(&sync_txs) {
                            Ok(rebuilt) => rebuilt,
                            Err(e) => {
                                transient = Some((
                                    format!("Sync block prefix rebuild after tx#{idx}: {e}"),
                                    abort_rest(None, &mut out_iter, std::mem::take(&mut inbounds)),
                                ));
                                break;
                            }
                        };
                        continue;
                    }
                    budget = next_budget;
                    blob_budget = next_blob;
                    effect_hashes.extend(group_hashes);
                    sync_txs.extend(pair_txs);
                    system_txs_appended +=
                        eez_protocol::system_tx::sync_pair_system_tx_count(&pairs_k) as u64;
                    // Debit at accept, not at the check above: a tx evicted in
                    // between never draws the escrow down on L1, and a budget
                    // reduced for it would evict later legitimate withdrawals.
                    if let Some(avail) = escrow_remaining {
                        escrow_remaining = Some(avail.saturating_sub(need));
                    }

                    // The state advances only now, behind the accepted block.
                    match take_l1_cache(&mut sessions, cc.entry_rollup_id) {
                        Ok(cache) => l1_state.cache = cache,
                        Err(e) => {
                            event!(
                                name: "eez.composer.cc_compose.l1_session_lost",
                                Level::ERROR,
                                rollup_id,
                                tx_idx = idx,
                                tx_hash = %held.hash,
                                error = %e,
                                "the L1 execution session did not come back from the composition; the slot's L1 state can no longer chain — degrading",
                            );
                            transient = Some((
                                format!("L1 session hand-off tx#{idx}: {e}"),
                                abort_rest(
                                    Some((idx, held)),
                                    &mut out_iter,
                                    std::mem::take(&mut inbounds),
                                ),
                            ));
                            break;
                        }
                    }

                    pending_out.push(outbound_group);
                    if let Some(projection) = &mut gas_projection {
                        projection.accept_immediate(l1_entries.len(), established);
                    }
                    outbound_entries.extend(l1_entries);
                    outbound_entry_traces.extend(l1_target.entry_traces.iter().cloned());
                    outbound_target_gas = outbound_target_gas.saturating_add(target_gas);
                    // NOT pushed to `survivor_comps`: its `source.batch` is OUR
                    // L2's entries (a dest=MAINNET call that must not settle on
                    // L1). The L1 settlement is `outbound_entries`, spliced
                    // separately with dest=rid.
                    semantic_transactions.push(semantic);
                    candidate.emit(
                        "source_accepted",
                        serde_json::json!({"source_hash":held.hash,
                        "position":idx,"projected_gas":budget.projected}),
                    );
                    selection.accept(idx);
                    pool.record_ready(&held, source_state);
                    survivors.push((idx, held));
                }
                Err(e) if source_condition(&e).is_some() => {
                    let condition = source_condition(&e)
                        .expect("matched source condition")
                        .clone();
                    candidate.emit(
                        "source_outcome",
                        serde_json::json!({"source_hash":held.hash,
                        "reason":"source_condition","condition":condition}),
                    );
                    defer_source(
                        pool,
                        &mut selection,
                        &mut source_deferred,
                        source_state,
                        idx,
                        held,
                        condition,
                    );
                }
                Err(e) if sim_error_is_poison(&e) => {
                    event!(
                        name: "eez.composer.cc_compose.outbound_poison",
                        Level::WARN,
                        rollup_id,
                        tx_idx = idx,
                        tx_hash = %held.hash,
                        error = %e,
                        "outbound tx fails simulation deterministically; evicting",
                    );
                    selection.block(idx);

                    push_poison_root(&mut poison, &mut poison_gaps, held);
                }
                Err(e) => {
                    transient = Some((
                        format!("compose_crosschain outbound tx#{idx}: {e}"),
                        abort_rest(
                            Some((idx, held)),
                            &mut out_iter,
                            std::mem::take(&mut inbounds),
                        ),
                    ));
                    break;
                }
            }
        }

        // ── PHASE 2 — INBOUND (L1→L2) ────────────────────────────────
        // Source-sim runs the L1 user tx on a fork of the state; the L2 side
        // probes the canonical delivery on a fork of the block and reads the
        // claim off the real `EEZL2 → proxy` frame. On accept the delivery
        // extends the block and the source fork's writes become the state.
        // A phase-1 abort already collected these txs into the re-queue set.
        let inbounds = if transient.is_some() {
            Vec::new()
        } else {
            inbounds
        };
        let mut in_iter = inbounds.into_iter();
        while let Some((idx, held)) = in_iter.next() {
            if !selection.consider(idx) {
                selection_filtered += 1;
                continue;
            }
            examined.insert(idx);
            candidate.emit(
                "source_selected",
                serde_json::json!({"source_hash":held.hash,
                "position":idx,"timing_basis":"source_checks","direction":match held.direction {
                    Direction::Inbound => "inbound", Direction::Outbound => "outbound",
                }}),
            );
            if !pool.claim(&held) {
                candidate.emit(
                    "source_outcome",
                    serde_json::json!({"source_hash":held.hash,"reason":"ownership_changed"}),
                );
                // A replacement is still older queued work. Revisit a fresh
                // snapshot rather than treating its chain as ineligible.
                selection.cut(idx);
                admission_cut = Some("snapshot_changed");
                continue;
            }
            let baseline = source_nonce(&held)?;
            if held.nonce < baseline {
                if pool.recheck_stale_sources(std::slice::from_ref(&held))? {
                    return Err("source nonce advanced; canonical reconciliation required".into());
                }
                selection.discard(idx);
                continue;
            }
            if !admission.can_check_group(survivors.len()) {
                admission_cut = Some("pipeline_time");
                deferred = abort_rest(Some((idx, held)), &mut in_iter, Vec::new());
                break;
            }
            if let Some(gap_at) = poison_gap_for(&poison_gaps, &held) {
                event!(
                    name: "eez.composer.cc_compose.poison_chain_evicted",
                    Level::WARN,
                    rollup_id,
                    tx_idx = idx,
                    tx_hash = %held.hash,
                    sender = %held.sender,
                    nonce = held.nonce,
                    gap_at,
                    "same-sender tx above an evicted poison nonce in the same drain; evicted (resubmit in order)",
                );
                poison.push(held);
                continue;
            }

            if let Some(reason) =
                capacity.minimum_check(&candidate, &held, budget, draft.gas_used())
            {
                if reason == "l2_block_capacity" {
                    block_gas_cut = Some(BlockGasCut {
                        gas_used: draft.gas_used(),
                        declared: MIN_TX_GAS,
                    });
                } else if gas_constraint.is_none() {
                    gas_constraint = Some(reason);
                    rejected_cost = Some(gas::minimum_entry_gas(false));
                }
                capacity.defer(&mut selection, &candidate, idx, held, reason);
                continue;
            }
            if !admission.can_start_group(survivors.len()) {
                admission_cut = Some("pipeline_time");
                deferred = abort_rest(Some((idx, held)), &mut in_iter, Vec::new());
                break;
            }
            let contexts = draft
                .fork()
                .map_err(|e| format!("SyncBlockState::fork: {e}"))
                .map(|fork| InboundL2TargetSession::new(fork, Arc::clone(&local.l2_entry)))
                .and_then(|probe| {
                    l1_state
                        .fork_state(&local.l1_entry)
                        .map(|source| (probe, source))
                        .map_err(|e| format!("L1SlotState::fork_state: {e}"))
                });
            let (probe, (mut l1_fork, l1_env)) = match contexts {
                Ok(contexts) => contexts,
                Err(e) => {
                    transient = Some((
                        format!("inbound execution contexts tx#{idx}: {e}"),
                        abort_rest(Some((idx, held)), &mut in_iter, Vec::new()),
                    ));
                    break;
                }
            };
            let sessions = seed_session(eez_protocol::RollupId(rollup_id), probe);
            simulation_count += 1;
            let sim = compose_crosschain(
                cc,
                cc.entry_rollup_id,
                &local.l1_entry,
                held.raw_tx.as_ref(),
                sessions,
                &mut l1_fork,
                l1_env,
                held.hash,
                l1_state.anchor.hash(),
                self.inner.gas_trace.as_ref(),
            );
            match sim {
                Ok((composition, _sessions, gas)) => {
                    if composition
                        .targets
                        .iter()
                        .any(|target| target.rollup_id != eez_protocol::RollupId(rollup_id))
                    {
                        event!(name: "eez.composer.cc_compose.inbound_foreign_target", Level::WARN, rollup_id, tx_idx = idx, tx_hash = %held.hash, "inbound composition reached another L2; this node cannot build that rollup's Sync block");
                        selection.block(idx);

                        push_poison_root(&mut poison, &mut poison_gaps, held);
                        continue;
                    }
                    let target_entries: Vec<eez_protocol::abi::ExecutionEntrySol> = composition
                        .targets
                        .iter()
                        .filter(|target| target.rollup_id == eez_protocol::RollupId(rollup_id))
                        .flat_map(|target| target.batch.entries.iter().cloned())
                        .collect();
                    let target_entries = match target_entries
                        .iter()
                        .map(|entry| {
                            eez_protocol::entries::InboundSidecar::for_rollup(
                                entry,
                                eez_protocol::RollupId(rollup_id),
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()
                    {
                        Ok(entries) => entries,
                        Err(e) => {
                            event!(
                                name: "eez.composer.cc_compose.shape_evicted",
                                Level::WARN,
                                rollup_id,
                                tx_idx = idx,
                                tx_hash = %held.hash,
                                error = %e,
                                "inbound composition uses an unsupported entry shape; evicting (resubmit required)",
                            );
                            selection.block(idx);

                            push_poison_root(&mut poison, &mut poison_gaps, held);
                            continue;
                        }
                    };

                    // Same gate: one delivery tx per entry, each at least its base
                    // cost. Foreign entries never ship; the probe below settles
                    // the exact fit once the deliveries exist.
                    let declared = MIN_TX_GAS.saturating_mul(target_entries.len() as u64);
                    match block_gas_fit(draft.gas_used(), declared, BUILDER_GAS_LIMIT) {
                        BlockGasFit::Accept => {}
                        BlockGasFit::Defer => {
                            block_gas_cut = Some(BlockGasCut {
                                gas_used: draft.gas_used(),
                                declared,
                            });
                            capacity.defer(
                                &mut selection,
                                &candidate,
                                idx,
                                held,
                                "l2_block_capacity",
                            );
                            continue;
                        }
                        BlockGasFit::Unfittable => {
                            event!(
                                name: "eez.composer.cc_compose.block_gas_unfittable",
                                Level::WARN,
                                rollup_id,
                                tx_idx = idx,
                                tx_hash = %held.hash,
                                declared,
                                deliveries = target_entries.len(),
                                block_gas_limit = BUILDER_GAS_LIMIT,
                                "inbound deliveries declare more gas than a whole L2 block; no slot can ever take them — evicting (resubmit as separate calls)",
                            );
                            selection.block(idx);

                            push_poison_root(&mut poison, &mut poison_gaps, held);
                            continue;
                        }
                    }
                    // Same gate, no target gas: a deferred entry only queues
                    // here, and its L1 half is a separate bundled tx.
                    let cost = gas_projection.as_ref().map_or_else(
                        || projected_tx_l1_gas(&composition.source.batch.entries, 0),
                        |projection| {
                            projection.cost(&composition.source.batch.entries, 0, false, None)
                        },
                    );
                    let mut cost = cost;
                    for entry in &composition.source.batch.staticEntries {
                        let (calldata, framing) = gas::framing::static_entry(entry);
                        cost.calldata_gas = cost.calldata_gas.saturating_add(calldata);
                        cost.framing_gas = cost.framing_gas.saturating_add(framing);
                        cost.general = true;
                    }
                    let companion_limit = declared_gas_limit(&held.raw_tx);
                    let companion_estimate = match (&mut gas_projection, &mut gas_parent) {
                        (Some(projection), Some(parent)) => projection.companion_estimate(
                            &composition.source.batch.entries, &composition.actions, gas, companion_limit, parent,
                        ).unwrap_or_else(|error| {
                            event!(Level::DEBUG, %error, "companion gas evidence unavailable; reserving signed limit");
                            gas::CompanionEstimate::declared(companion_limit)
                        }),
                        _ => gas::CompanionEstimate::declared(companion_limit),
                    };
                    let companion_gas = companion_estimate.gas;
                    let mut next_budget = budget;
                    let gas_accepted =
                        next_budget.try_accept_with_companion(cost, companion_limit, companion_gas);
                    trace_admission_budget(
                        self.inner.gas_trace.as_ref(),
                        &candidate,
                        held.hash,
                        l1_state.anchor.hash(),
                        "inbound",
                        budget,
                        next_budget,
                        cost,
                        &composition.source.batch.entries,
                        gas,
                        companion_limit,
                        companion_estimate,
                        prior_cleanup,
                        gas_accepted,
                    );
                    if !gas_accepted {
                        if budget.blocked_by_cleanup(
                            cost,
                            companion_limit,
                            companion_gas,
                            prior_cleanup,
                        ) {
                            admission_cut = Some("prior_queue_cleanup");
                        }
                        if gas_constraint.is_none() {
                            gas_constraint = next_budget.rejected_constraint;
                            rejected_cost = Some(cost.total());
                        }
                        let reason = if budget.blocked_by_cleanup(
                            cost,
                            companion_limit,
                            companion_gas,
                            prior_cleanup,
                        ) {
                            "prior_queue_cleanup"
                        } else {
                            next_budget
                                .rejected_constraint
                                .unwrap_or("l1_tx_gas_budget")
                        };
                        capacity.defer(&mut selection, &candidate, idx, held, reason);
                        continue;
                    }
                    // ── ACCEPT ───────────────────────────────────────
                    let deliveries = match eez_protocol::system_tx::build_inbound_system_txs(
                        &target_entries,
                        &stf_cfg,
                        nonce + system_txs_appended,
                    ) {
                        Ok(deliveries) if deliveries.is_empty() && !target_entries.is_empty() => {
                            event!(
                                name: "eez.composer.cc_compose.shape_evicted",
                                Level::WARN,
                                rollup_id,
                                tx_idx = idx,
                                tx_hash = %held.hash,
                                entries = target_entries.len(),
                                "every inbound target entry was skipped as foreign, which cannot happen for own-rollup targets; evicting",
                            );
                            selection.block(idx);

                            push_poison_root(&mut poison, &mut poison_gaps, held);
                            continue;
                        }
                        Ok(deliveries) => deliveries,
                        Err(e) => {
                            event!(
                                name: "eez.composer.cc_compose.shape_evicted",
                                Level::WARN,
                                rollup_id,
                                tx_idx = idx,
                                tx_hash = %held.hash,
                                error = %e,
                                "build_inbound_system_txs rejected the entries; evicting (resubmit required)",
                            );
                            selection.block(idx);

                            push_poison_root(&mut poison, &mut poison_gaps, held);
                            continue;
                        }
                    };
                    let semantic = eez_protocol::blob_semantics::SemanticTransaction {
                        origin_chain: eez_protocol::RollupId::MAINNET,
                        tx_data: held.raw_tx.clone(),
                        actions: composition.actions.clone(),
                    };
                    let next_blob = match self.extend_blob_admission(
                        rollup_id,
                        blob_budget,
                        &semantic,
                        Some(&held.raw_tx),
                        bundle_target,
                    ) {
                        Ok(Some((_, false))) if !survivors.is_empty() => {
                            capacity.defer(
                                &mut selection,
                                &candidate,
                                idx,
                                held,
                                "composition_blob_limit",
                            );
                            continue;
                        }
                        Ok(next) => next.map(|(next, _)| next),
                        Err(error) => {
                            transient = Some((
                                error,
                                abort_rest(Some((idx, held)), &mut in_iter, Vec::new()),
                            ));
                            break;
                        }
                    };
                    // Same capacity decision as the outbound path, before the
                    // canonical prefix rebuild could refuse the group.
                    if let Err((at, why)) = draft.capacity_probe(&deliveries) {
                        if why.is_provider() {
                            transient = Some((
                                format!("inbound capacity probe tx#{idx} at {at}: {why}"),
                                abort_rest(Some((idx, held)), &mut in_iter, Vec::new()),
                            ));
                            break;
                        }
                        let declared = declared_group_gas(&deliveries);
                        if capacity_refusal(sync_txs.is_empty()) == BlockGasFit::Unfittable {
                            event!(
                                name: "eez.composer.cc_compose.block_gas_unfittable",
                                Level::WARN,
                                rollup_id,
                                tx_idx = idx,
                                tx_hash = %held.hash,
                                declared,
                                deliveries = deliveries.len(),
                                refused_at = at,
                                error = %why,
                                block_gas_limit = BUILDER_GAS_LIMIT,
                                "inbound deliveries do not fit an otherwise empty L2 block; no slot can ever take them — evicting (resubmit as separate calls)",
                            );
                            selection.block(idx);

                            push_poison_root(&mut poison, &mut poison_gaps, held);
                            continue;
                        }
                        block_gas_cut = Some(BlockGasCut {
                            gas_used: draft.gas_used(),
                            declared,
                        });
                        capacity.defer(&mut selection, &candidate, idx, held, "l2_block_capacity");
                        continue;
                    }
                    let group_hashes = match hash_group(
                        &sync_txs,
                        &deliveries,
                        &(0..deliveries.len()).collect::<Vec<_>>(),
                    ) {
                        Ok(hashes) => hashes,
                        Err(error) => {
                            transient = Some((
                                format!("inbound prefix hashes tx#{idx}: {error}"),
                                abort_rest(Some((idx, held)), &mut in_iter, Vec::new()),
                            ));
                            break;
                        }
                    };
                    if let Some(next) = next_blob
                        && !admission.can_finish_group(survivors.len() + 1, next.required_blobs()?)
                    {
                        admission_cut = Some("pipeline_time");
                        deferred = abort_rest(Some((idx, held)), &mut in_iter, Vec::new());
                        break;
                    }
                    // The delivery re-runs the on-chain claim compare, so this
                    // append IS the verifier: a revert means the claims and the
                    // block disagree and the tx must go.
                    if let Err((at, why)) = append_and_execute(&mut draft, &deliveries) {
                        // A backing-store failure says nothing about the tx —
                        // abort the slot instead of evicting a valid delivery.
                        if why.is_provider() {
                            transient = Some((
                                format!("inbound append tx#{idx} at {at}: {why}"),
                                abort_rest(Some((idx, held)), &mut in_iter, Vec::new()),
                            ));
                            break;
                        }
                        // Defensive, as for outbound pairs: never charge a
                        // capacity refusal to the source.
                        if why.is_capacity() {
                            let unfittable =
                                capacity_refusal(sync_txs.is_empty()) == BlockGasFit::Unfittable;
                            draft = match reopen(&sync_txs) {
                                Ok(rebuilt) => rebuilt,
                                Err(e) => {
                                    transient = Some((
                                        format!("Sync block prefix rebuild after tx#{idx}: {e}"),
                                        abort_rest(Some((idx, held)), &mut in_iter, Vec::new()),
                                    ));
                                    break;
                                }
                            };
                            if unfittable {
                                event!(
                                    name: "eez.composer.cc_compose.block_gas_unfittable",
                                    Level::WARN,
                                    rollup_id,
                                    tx_idx = idx,
                                    tx_hash = %held.hash,
                                    refused_at = at,
                                    error = %why,
                                    "inbound deliveries do not fit an otherwise empty L2 block — evicting",
                                );
                                selection.block(idx);
                                push_poison_root(&mut poison, &mut poison_gaps, held);
                            } else {
                                block_gas_cut = Some(BlockGasCut {
                                    gas_used: draft.gas_used(),
                                    declared: declared_group_gas(&deliveries),
                                });
                                capacity.defer(
                                    &mut selection,
                                    &candidate,
                                    idx,
                                    held,
                                    "l2_block_capacity",
                                );
                            }
                            continue;
                        }
                        event!(
                            name: "eez.composer.cc_compose.append_reverted",
                            Level::WARN,
                            rollup_id,
                            tx_idx = idx,
                            tx_hash = %held.hash,
                            appended_idx = at,
                            error = %why,
                            "inbound delivery does not execute on the Sync block prefix; evicting the tx and rebuilding the prefix",
                        );
                        selection.block(idx);

                        push_poison_root(&mut poison, &mut poison_gaps, held);
                        draft = match reopen(&sync_txs) {
                            Ok(rebuilt) => rebuilt,
                            Err(e) => {
                                transient = Some((
                                    format!("Sync block prefix rebuild after tx#{idx}: {e}"),
                                    abort_rest(None, &mut in_iter, Vec::new()),
                                ));
                                break;
                            }
                        };
                        continue;
                    }
                    budget = next_budget;
                    if let Some(projection) = &mut gas_projection {
                        projection.accept_deferred(&composition.source.batch.entries);
                    }
                    blob_budget = next_blob;
                    effect_hashes.extend(group_hashes);
                    system_txs_appended += deliveries.len() as u64;
                    sync_txs.extend(deliveries);
                    // The source fork carries the L1 user tx's committed writes;
                    // later inbound sims must observe them (design §4 step 6).
                    l1_state.cache = l1_fork.cache;

                    let target_count = target_entries.len();
                    pending_in.extend(target_entries);
                    event!(
                        name: "eez.composer.cc_compose.tx",
                        Level::INFO,
                        rollup_id,
                        tx_idx = idx,
                        target_count,
                        "composition produced {{target_count}} target(s) for held tx #{{tx_idx}}",
                    );
                    semantic_transactions.push(semantic);
                    survivor_comps.push((composition, held.hash));
                    candidate.emit(
                        "source_accepted",
                        serde_json::json!({"source_hash":held.hash,
                        "position":idx,"projected_gas":budget.projected}),
                    );
                    selection.accept(idx);
                    pool.record_ready(&held, source_state);
                    survivors.push((idx, held));
                }
                Err(e) if source_condition(&e).is_some() => {
                    let condition = source_condition(&e)
                        .expect("matched source condition")
                        .clone();
                    candidate.emit(
                        "source_outcome",
                        serde_json::json!({"source_hash":held.hash,
                        "reason":"source_condition","condition":condition}),
                    );
                    defer_source(
                        pool,
                        &mut selection,
                        &mut source_deferred,
                        source_state,
                        idx,
                        held,
                        condition,
                    );
                }
                Err(e) if sim_error_is_poison(&e) => {
                    event!(
                        name: "eez.composer.cc_compose.poison_evicted",
                        Level::WARN,
                        rollup_id,
                        tx_idx = idx,
                        tx_hash = %held.hash,
                        sender = %held.sender,
                        nonce = held.nonce,
                        error = %e,
                        "held tx fails simulation deterministically (e.g. wrong proxy → EmptyCalls, or revert); evicting — it can never compose, resubmit required",
                    );
                    selection.block(idx);

                    push_poison_root(&mut poison, &mut poison_gaps, held);
                }
                Err(e) => {
                    // Transient (provider / transport / unavailable) —
                    // abort the slot, re-queue this tx + the remainder.
                    transient = Some((
                        format!("compose_crosschain inbound tx#{idx}: {e}"),
                        abort_rest(Some((idx, held)), &mut in_iter, Vec::new()),
                    ));
                    break;
                }
            }
        }

        // Eligible work gets the execution budget first. Classify fee waits
        // afterward, within the same preparation budget, so a large number of
        // underpriced senders cannot starve already selected independent work.
        for &idx in selection.fee_blocked() {
            if !admission.can_check_source(survivors.len()) {
                break;
            }
            let held = &snapshot[idx];
            // A replacement may have arrived since the snapshot. Claim only
            // the exact envelope, and retain it without charging an attempt.
            // A blocked successor stays queued behind its ready predecessor.
            if pool.claim(held) {
                // A source mined elsewhere is stale even when its old fee
                // cap is now too low. Do not retain it as a permanent fee wait.
                if held.nonce < source_nonce(held)? {
                    if pool.recheck_stale_sources(std::slice::from_ref(held))? {
                        return Err(
                            "source nonce advanced; canonical reconciliation required".into()
                        );
                    }
                    continue;
                }
                pool.record_blocked(held, eez_protocol::SourceCondition::Fee, source_state);
                source_deferred.push((idx, held.clone()));
            }
        }
        admission
            .source_deferred(!source_deferred.is_empty() || !selection.fee_blocked().is_empty());
        self.cancel_historical_predecessor_groups(
            rollup_id,
            source_deferred.iter().map(|(_, held)| held.hash),
        );
        for (index, held) in deferred
            .iter()
            .filter(|(index, _)| examined.contains(index))
        {
            candidate.emit(
                "source_outcome",
                serde_json::json!({"source_hash":held.hash,"position":index,
                "reason":admission_cut.unwrap_or("pipeline_time")}),
            );
        }
        for &index in selection
            .fee_blocked()
            .iter()
            .filter(|index| examined.contains(*index))
        {
            candidate.emit(
                "source_outcome",
                serde_json::json!({"source_hash":snapshot[index].hash,"reason":"source_fee"}),
            );
        }
        requeue_unprocessed(pool, rollup_id, &poison_gaps, source_deferred);

        // ── Deferred work: settle what fits, keep the rest held. ──────
        // Normal under load. Before the poison sweep, so an eviction here rides
        // the same nonce cascade.
        let resource_reason = capacity.last_reason;
        // Canonical execution may have spent capacity on younger outbounds
        // before reaching an older inbound. Such a candidate cannot be posted.
        let fifo_prefix_len = selection.fifo_prefix_len();
        let fifo_bypassed = selection.fifo_bypassed();
        deferred.extend(capacity.groups);
        admission.phase_finished(survivors.len(), !deferred.is_empty(), fifo_bypassed);
        let prefix_hash_executed_tx_count = prefix_hashes.executed_transactions;
        drop(prefix_hashes);
        candidate.duration("prefix_hashing", prefix_hash_elapsed);
        candidate.emit(
            "summary",
            serde_json::json!({
                "accepted":survivors.len(), "deferred":deferred.len(),
                "examined_count":examined.len(),
                "not_examined_count":snapshot.len().saturating_sub(examined.len()),
                "selection_filtered_count":selection_filtered,
                "source_fee_count":selection.fee_blocked().len(),
                "capacity_deferrals":capacity.reasons,
                "minimum_check_count":capacity.minimum_check_count,
                "simulation_count":simulation_count,
                "prefix_hash_ms":prefix_hash_elapsed.as_secs_f64()*1000.0,
                "prefix_hash_executed_tx_count":prefix_hash_executed_tx_count,
                "fifo_bypassed":fifo_bypassed,
                "fifo_prefix_len":fifo_prefix_len,
                "fifo_trigger":fifo_prefix_len.and(admission_cut.or(resource_reason)),
                "reason":admission.stop_reason(transient.as_ref().map(|_| "transient_compose").or(fifo_prefix_len.map(|_| "fifo_prefix_rebuild")).or(admission_cut).or(resource_reason), survivors.len(), snapshot.len()),
                "projected_gas":budget.projected,"entry_discount":budget.entry_discount,
                "framing_reserve_gas":budget.framing_gas,
                "full_estimate_gas":budget.full_estimate(),
                "calldata_bound_gas":budget.calldata_gas.saturating_add(budget.framing_gas),
                "calldata_floor_active":budget.floor_active,
                "general_execution_reserve":budget.general,
                "gas_margin_bps":self.inner.posting.fees.gas_margin_bps,
                "planning_cap":budget.cap,"whole_block_cap":budget.block_cap,
                "companion_gas":budget.companion_gas,
                "cold_timing_probe":admission.used_cold_timing_probe(),
            }),
        );
        if transient.is_none() && !deferred.is_empty() {
            let deferred_count = deferred.len();
            let accepted_count = survivors.len();
            // A block-gas cut needs gas on the block, which needs an accepted tx,
            // so the eviction branch below can only ever be an L1-budget cut.
            debug_assert!(
                accepted_count > 0 || block_gas_cut.is_none(),
                "a block-gas cut cannot happen with nothing accepted"
            );
            if let Some(constraint) = admission_cut
                .or(resource_reason.filter(|reason| *reason == "composition_blob_limit"))
            {
                event!(name: "eez.composer.admission.cut", Level::INFO,
                    event_name = "eez.composer.admission.cut", rollup_id,
                    accepted_count, deferred_count, constraint,
                    "complete groups deferred without charging attempts or releasing ownership");
            } else if accepted_count == 0 {
                // It lost against the whole budget, so no slot is roomier and
                // re-queueing would block the FIFO forever. It sits first.
                let (_, head) = deferred.remove(0);
                event!(
                    name: "eez.composer.sync_slot.gas_budget_cut",
                    Level::WARN,
                    event_name = "eez.composer.sync_slot.gas_budget_cut",
                    rollup_id,
                    accepted_count,
                    deferred_count,
                    projected_gas = budget.projected,
                    entry_discount = budget.entry_discount,
                    prior_cleanup_gas = prior_cleanup,
                    rejected_cost = rejected_cost.unwrap_or(0),
                    budget = budget.cap,
                    constraint = gas_constraint.unwrap_or("l1_tx_gas_budget"),
                    operator_gas_cap = self.inner.emission.max_gas,
                    network_transaction_gas_max = budget.transaction_cap,
                    block_gas_budget = budget.block_cap,
                    companion_gas = budget.companion_gas,
                    planning_margin = budget.framing_gas,
                    tx_hash = %head.hash,
                    "one complete group exceeds the resolved L1 gas budget; applying existing admission and nonce-chain rejection",
                );
                push_poison_root(&mut poison, &mut poison_gaps, head);
            } else if let Some(BlockGasCut { gas_used, declared }) = block_gas_cut {
                event!(
                    name: "eez.composer.sync_slot.block_gas_cut",
                    Level::INFO,
                    rollup_id,
                    accepted_count,
                    deferred_count,
                    gas_used,
                    declared,
                    block_gas_limit = BUILDER_GAS_LIMIT,
                    "Sync block gas limit reached; {{deferred_count}} held tx(s) stay queued for a later Sync slot",
                );
            } else {
                event!(
                    name: "eez.composer.sync_slot.gas_budget_cut",
                    Level::INFO,
                    event_name = "eez.composer.sync_slot.gas_budget_cut",
                    rollup_id,
                    accepted_count,
                    deferred_count,
                    projected_gas = budget.projected,
                    entry_discount = budget.entry_discount,
                    prior_cleanup_gas = prior_cleanup,
                    rejected_cost = rejected_cost.unwrap_or(0),
                    budget = budget.cap,
                    constraint = gas_constraint.unwrap_or("l1_tx_gas_budget"),
                    operator_gas_cap = self.inner.emission.max_gas,
                    network_transaction_gas_max = budget.transaction_cap,
                    block_gas_budget = budget.block_cap,
                    companion_gas = budget.companion_gas,
                    planning_margin = budget.framing_gas,
                    "L1 gas budget reached; complete deferred groups remain queued without a failed attempt",
                );
            }
        }

        // Evict the poison txs' gapped higher nonces from the pool — once
        // a sender's nonce N is evicted, N+1.. can never land.
        for tx in &poison {
            // Inclusive eviction releases the poison root's reservation too.
            let evicted = pool.evict_chain_at_or_above(tx.sender, tx.direction, tx.nonce);
            event!(
                name: "eez.composer.cc_compose.poison_eviction_completed",
                Level::WARN,
                event_name = "eez.composer.cc_compose.poison_eviction_completed",
                rollup_id,
                tx_hash = %tx.hash,
                sender = %tx.sender,
                nonce = tx.nonce,
                direction = ?tx.direction,
                "poison transaction and its nonce-chain suffix evicted",
            );
            for t in evicted {
                event!(
                    name: "eez.composer.cc_compose.poison_chain_evicted",
                    Level::WARN,
                    rollup_id,
                    tx_hash = %t.hash,
                    sender = %t.sender,
                    nonce = t.nonce,
                    gap_at = tx.nonce,
                    "same-sender tx above an evicted poison nonce; gapped chain can't land — evicted (resubmit in order)",
                );
            }
        }

        // ── Transient abort: re-queue survivors + remainder, minimal. ──
        // Earlier capacity misses are disjoint from the failing remainder.
        // Restore all three sets together so a later provider failure cannot
        // strand their composing reservations or alter their nonce order.
        if let Some((err, rest)) = transient {
            event!(
                name: "eez.composer.phase2.transient",
                Level::WARN,
                rollup_id,
                error = %err,
                survivors = survivors.len(),
                "transient compose failure; re-queueing and degrading to minimal postBatch this slot",
            );
            let mut requeue = survivors;
            requeue.extend(deferred);
            requeue.extend(rest);
            self.cancel_historical_predecessor_groups(
                rollup_id,
                requeue.iter().map(|(_, held)| held.hash),
            );
            // Both phases contribute; the pool is owed its own FIFO order.
            requeue_unprocessed(pool, rollup_id, &poison_gaps, requeue);
            return self
                .dispatch_minimal_postbatch(
                    ctx,
                    rollup_id,
                    rollup,
                    parent_header,
                    timestamp,
                    suggested_fee_recipient,
                    bundle_target,
                )
                .await;
        }

        if let Some(prefix_len) = fifo_prefix_len {
            // Rebuild all effects from the pinned parent; dropping younger
            // outbound effects from already-simulated inbounds is unsound.
            // Strictly fewer snapshot users bound recursion, and the shared
            // admission attempt retains the original absolute deadline.
            self.cancel_historical_predecessor_groups(
                rollup_id,
                snapshot.iter().map(|held| held.hash),
            );
            pool.retry_fifo_prefix(&snapshot, prefix_len);
            admission.rebuilding();
            // Refresh exact envelopes after restoration. Replacements and
            // recovered predecessors may have arrived during simulation.
            let retry = pool.candidates();
            if retry.is_empty() || retry.len() >= snapshot.len() {
                // A concurrent recovery can enlarge the prefix. Keep the
                // boundary for a fresh opportunity rather than recurse without
                // a strict size decrease or drop a required predecessor.
                return Ok(self.build_empty_slot_block(rollup, parent_header, timestamp));
            }
            let retry_after_error = retry.clone();
            let result = Box::pin(self.compose_cross_chain_batch(
                cc,
                rollup_id,
                retry,
                parent_header,
                timestamp,
                suggested_fee_recipient,
                bundle_target,
                admission,
            ))
            .await;
            if result.is_err() {
                pool.requeue_composing(retry_after_error);
            }
            return result;
        }

        requeue_unprocessed(pool, rollup_id, &poison_gaps, deferred);

        // Nothing composed (all evicted or over budget). Still post a minimal
        // batch so L1 keeps tracking L2's progression.
        if survivors.is_empty() {
            admission.completed();
            if let Some(constraint) = admission_cut {
                event!(name: "eez.composer.admission.deferred", Level::INFO,
                    event_name = "eez.composer.admission.deferred", rollup_id, constraint,
                    "no group admitted within this opportunity; users remain pending");
                if constraint == "pipeline_time" {
                    // A time-constrained cross-chain candidate must not consume
                    // the remaining window by substituting an empty L1 batch.
                    return Ok(self.build_empty_slot_block(rollup, parent_header, timestamp));
                }
            } else {
                let evicted = poison.len();
                event!(
                    name: "eez.composer.phase2.all_poison",
                    Level::WARN,
                    rollup_id,
                    evicted,
                    "no held tx survived the drain (deterministic failure or over the gas budget); emitting minimal postBatch",
                );
            }
            return self
                .dispatch_minimal_postbatch(
                    ctx,
                    rollup_id,
                    rollup,
                    parent_header,
                    timestamp,
                    suggested_fee_recipient,
                    bundle_target,
                )
                .await;
        }

        // Past the drain the pool's order is all that matters again.
        let mut survivors: Vec<HeldTx> = restore_pool_order(survivors);

        // ── Rebuild the Sync block's system txs via THE canonical builder —
        // deriver-byte-identical two-phase SYSTEM_ADDRESS nonces (outbound loads
        // N.., then inbound deliveries N+K..) + interleaved order
        // [load,user,…,deliveries]. Handles inbound / outbound / mixed
        // uniformly. A failure is systemic (signing / nonce overflow) →
        // re-queue survivors, degrade to minimal.
        let pairs = match eez_protocol::system_tx::build_cross_chain_sync_groups(
            &pending_out,
            &pending_in,
            &stf_cfg,
            nonce,
        ) {
            Ok(p) => p,
            Err(e) => {
                event!(
                    name: "eez.composer.phase2.sync_pairs_failed",
                    Level::WARN,
                    rollup_id,
                    error = %e,
                    "build_cross_chain_sync_pairs failed; re-queueing survivors, degrading to minimal postBatch",
                );
                pool.push_front_batch(survivors);
                return self
                    .dispatch_minimal_postbatch(
                        ctx,
                        rollup_id,
                        rollup,
                        parent_header,
                        timestamp,
                        suggested_fee_recipient,
                        bundle_target,
                    )
                    .await;
            }
        };

        // ── KEYSTONE ─────────────────────────────────────────────────
        // The block this drain appended tx-by-tx must be exactly what the
        // canonical builder — and therefore the deriver and the proof signer —
        // reconstructs from the same entries. Inequality is a composer bug, not
        // an input condition, so it degrades the slot loudly instead of posting
        // a block nobody else can rebuild.
        let canonical = eez_protocol::system_tx::interleave_sync_block_txs(&pairs);
        if canonical != sync_txs {
            let first_divergent = canonical
                .iter()
                .zip(&sync_txs)
                .position(|(a, b)| a != b)
                .unwrap_or(canonical.len().min(sync_txs.len()));
            event!(
                name: "eez.composer.phase2.canonical_mismatch",
                Level::ERROR,
                rollup_id,
                canonical_len = canonical.len(),
                appended_len = sync_txs.len(),
                first_divergent,
                "the incrementally appended Sync block disagrees with the canonical rebuild — a composer bug, never bad input; re-queueing survivors, degrading to minimal postBatch",
            );
            pool.push_front_batch(survivors);
            return self
                .dispatch_minimal_postbatch(
                    ctx,
                    rollup_id,
                    rollup,
                    parent_header,
                    timestamp,
                    suggested_fee_recipient,
                    bundle_target,
                )
                .await;
        }

        // ── Build the rich Sync block + postBatch from survivors. A ──
        // ── build / prepare failure here is systemic (not one tx) →  ──
        // ── re-queue survivors and degrade to minimal.               ──
        let built = match build_sync_block(
            rollup.l2_provider.as_ref(),
            &self.inner.evm_config,
            parent_header,
            timestamp,
            prev_randao,
            suggested_fee_recipient,
            &sync_txs,
        ) {
            Ok(b) => b,
            Err(e) => {
                event!(
                    name: "eez.composer.phase2.build_failed",
                    Level::WARN,
                    rollup_id,
                    error = %e,
                    "build_sync_block failed; re-queueing survivors, degrading to minimal postBatch",
                );
                pool.push_front_batch(survivors);
                return self
                    .dispatch_minimal_postbatch(
                        ctx,
                        rollup_id,
                        rollup,
                        parent_header,
                        timestamp,
                        suggested_fee_recipient,
                        bundle_target,
                    )
                    .await;
            }
        };
        // Belt check: every tx was receipt-verified on the very prefix this
        // block re-executes, so a failure here means the block and the prefix
        // disagree. Nothing in this class may reach the proof signer.
        if let Some(first_failed) = built.tx_successes.iter().position(|success| !success) {
            event!(
                name: "eez.composer.phase2.final_receipt_failed",
                Level::ERROR,
                rollup_id,
                tx_index = first_failed,
                tx_count = built.tx_successes.len(),
                "a Sync-block tx reverted in the final build although it succeeded when appended; re-queueing survivors, degrading to minimal postBatch",
            );
            pool.push_front_batch(survivors);
            return self
                .dispatch_minimal_postbatch(
                    ctx,
                    rollup_id,
                    rollup,
                    parent_header,
                    timestamp,
                    suggested_fee_recipient,
                    bundle_target,
                )
                .await;
        }
        // Per-effect intermediate L2 block hashes: the prover requires each
        // entry's `newRoot` ABI slot to commit its own effect-prefix block.
        // Failure here is systemic (like build/prepare) → degrade.
        let effect_end_positions = eez_protocol::system_tx::sync_pair_end_positions(&pairs);
        let cached_hashes = effect_hashes
            .iter()
            .map(|(position, _)| *position)
            .eq(effect_end_positions.iter().copied())
            .then(|| {
                effect_hashes
                    .into_iter()
                    .map(|(_, hash)| hash)
                    .collect::<Vec<_>>()
            })
            .ok_or("incremental effect positions disagree with the canonical Sync block");
        let pair_hashes = match cached_hashes {
            Ok(r) => r,
            Err(e) => {
                event!(
                    name: "eez.composer.phase2.pair_hashes_failed",
                    Level::WARN,
                    rollup_id,
                    error = %e,
                    "sync_block_pair_hashes failed; re-queueing survivors, degrading to minimal postBatch",
                );
                pool.push_front_batch(survivors);
                return self
                    .dispatch_minimal_postbatch(
                        ctx,
                        rollup_id,
                        rollup,
                        parent_header,
                        timestamp,
                        suggested_fee_recipient,
                        bundle_target,
                    )
                    .await;
            }
        };
        let comp_refs: Vec<&eez_protocol::Composition> = survivor_comps
            .iter()
            .map(|(composition, _)| composition)
            .collect();
        // Per-slot phase: one merge over all survivor compositions, now that
        // the built block hash and per-effect prefix hashes exist.
        let overhead = match Self::cross_chain_da_overhead(rollup_id, &semantic_transactions) {
            Ok(overhead) => overhead,
            Err(error) => {
                pool.push_front_batch(survivors);
                return Err(error.to_string());
            }
        };
        if let Err(error) = self
            .evaluate_settlement_policy(
                rollup_id,
                rollup,
                parent_header,
                &built,
                bundle_target,
                crate::settlement_policy::CrossChainWork::Executable,
                overhead,
            )
            .await
        {
            event!(Level::WARN, rollup_id, %error, "cross-chain policy evidence unavailable; retaining admitted groups");
            pool.push_front_batch(survivors);
            return Ok(self.build_empty_slot_block(rollup, parent_header, timestamp));
        }
        let preparation = self
            .prepare_post_batch_raw(
                ctx,
                rollup_id,
                &comp_refs,
                parent_header,
                built.header.hash(),
                ordinary_prefix_hash,
                Some(&built.block),
                &pair_hashes,
                &effect_end_positions,
                &outbound_entries,
                &outbound_entry_traces,
                &pending_out,
                &semantic_transactions,
                outbound_target_gas,
                gas_projection.as_ref(),
                bundle_target,
                Some(admission),
                Some(Arc::clone(&candidate)),
                preparation_started_ms,
            )
            .await;
        if let Err(
            error @ (PreparePostBatchError::CompositionBlobLimit { .. }
            | PreparePostBatchError::OverBudget {
                constraint: "l1_tx_gas_budget",
                ..
            }),
        ) = &preparation
        {
            // Historical DA is not calldata. Shrinking it cannot repair the
            // selected composition's execution-gas requirement.
            event!(name: "eez.composer.settlement.group_capacity", Level::WARN,
                event_name = "eez.composer.settlement.group_capacity", rollup_id, %error,
                admitted_groups = survivors.len(),
                "defer complete groups without charging a failed attempt or splitting history");
            if survivors.len() > 1 {
                // Reuse the existing bounded recomposition path. Removing the
                // FIFO tail preserves every retained sender's nonce prefix.
                let deferred = survivors.pop().expect("more than one survivor");
                pool.push_front_batch(vec![deferred]);
                let retry_after_error = survivors.clone();
                return match Box::pin(self.compose_cross_chain_batch(
                    cc,
                    rollup_id,
                    survivors,
                    parent_header,
                    timestamp,
                    suggested_fee_recipient,
                    bundle_target,
                    admission,
                ))
                .await
                {
                    Ok(result) => Ok(result),
                    Err(error) => {
                        pool.requeue_composing(retry_after_error);
                        Err(error)
                    }
                };
            }
            // An indivisible group remains visible/owned for manual handling.
            // Only an independent policy reason may post the ordinary backlog.
            self.cancel_historical_predecessor_groups(
                rollup_id,
                survivors.iter().map(|held| held.hash),
            );
            pool.push_front_batch(survivors);
            return self
                .dispatch_minimal_postbatch(
                    ctx,
                    rollup_id,
                    rollup,
                    parent_header,
                    timestamp,
                    suggested_fee_recipient,
                    bundle_target,
                )
                .await;
        }
        if matches!(preparation, Err(PreparePostBatchError::OverBudget { .. })) {
            // Capture a committed predecessor hash and its held group identity.
            // Never retain the hash of the uncommitted cross-chain Sync that
            // is about to be replaced by an empty cadence block.
            let predecessor = self.latch_historical_predecessor(
                rollup_id,
                rollup,
                built.header.number(),
                &survivors,
            );
            pool.push_front_batch(survivors);
            if let Err(error) = predecessor {
                event!(Level::ERROR, rollup_id, %error, "valid cross-chain work needs an unavailable or unpostable predecessor");
            }
            return self
                .dispatch_minimal_postbatch(
                    ctx,
                    rollup_id,
                    rollup,
                    parent_header,
                    timestamp,
                    suggested_fee_recipient,
                    bundle_target,
                )
                .await;
        }
        let (prepared_postbatch, preparation_timing) = match preparation {
            Ok(r) => r,
            Err(
                error @ (PreparePostBatchError::Actionable(_)
                | PreparePostBatchError::ObservationMismatch { .. }),
            ) => {
                let observation_mismatch = match &error {
                    PreparePostBatchError::ObservationMismatch {
                        composition_index,
                        observation_index,
                        expected_success,
                        observed_success,
                        expected_output_hash,
                        observed_output_hash,
                    } => Some(ObservationMismatchDetails {
                        composition_index: *composition_index,
                        observation_index: *observation_index,
                        expected_success: *expected_success,
                        observed_success: *observed_success,
                        expected_output_hash: *expected_output_hash,
                        observed_output_hash: *observed_output_hash,
                    }),
                    PreparePostBatchError::Actionable(_)
                    | PreparePostBatchError::Build(_)
                    | PreparePostBatchError::CompositionBlobLimit { .. }
                    | PreparePostBatchError::OverBudget { .. }
                    | PreparePostBatchError::Prover(_) => None,
                };
                let failed = match &error {
                    PreparePostBatchError::Actionable(failure) => actionable_held_tx(
                        *failure,
                        &survivors,
                        outbound_entries.len(),
                        &survivor_comps,
                    ),
                    PreparePostBatchError::ObservationMismatch {
                        composition_index, ..
                    } => observation_held_tx(*composition_index, &survivors, &survivor_comps),
                    PreparePostBatchError::Build(_)
                    | PreparePostBatchError::Prover(_)
                    | PreparePostBatchError::CompositionBlobLimit { .. }
                    | PreparePostBatchError::OverBudget { .. } => {
                        unreachable!("match arm accepts only attributable failures")
                    }
                }
                .cloned();
                let Some(failed) = failed else {
                    if let Some(mismatch) = observation_mismatch {
                        event!(
                            name: "eez.composer.observation.mismatch",
                            Level::ERROR,
                            rollup_id,
                            composition_index = mismatch.composition_index,
                            observation_index = mismatch.observation_index,
                            expected_success = mismatch.expected_success,
                            observed_success = mismatch.observed_success,
                            expected_output_hash = %mismatch.expected_output_hash,
                            observed_output_hash = %mismatch.observed_output_hash,
                            resolved_held_tx = false,
                            "composer simulation and pinned-prefix replay disagree, but the originating held transaction could not be resolved; retaining survivors and degrading to minimal postBatch",
                        );
                        // Local observation mismatches are construction failures, not
                        // failed proving episodes. Keep their existing fallback policy.
                        pool.push_front_batch(survivors);
                    } else {
                        event!(
                            name: "eez.composer.prover.actionable_unresolved",
                            Level::ERROR,
                            rollup_id,
                            failure = %error,
                            "attributable prover failure has no matching HeldTx; counting one opaque settlement failure and degrading to minimal postBatch",
                        );
                        let recovery = recover_settlement_failure(pool, rollup_id, survivors);
                        event!(
                            name: "eez.composer.prover.failure_recovered",
                            Level::WARN,
                            rollup_id,
                            requeued = recovery.requeued,
                            evicted = recovery.evicted,
                            "opaque prover failure recovered with bounded transaction eviction",
                        );
                    }
                    return self
                        .dispatch_minimal_postbatch(
                            ctx,
                            rollup_id,
                            rollup,
                            parent_header,
                            timestamp,
                            suggested_fee_recipient,
                            bundle_target,
                        )
                        .await;
                };

                let (retry, evicted_in_flight) = partition_retryable(survivors, &failed);
                let evicted_queued =
                    pool.evict_chain_at_or_above(failed.sender, failed.direction, failed.nonce);
                if let Some(mismatch) = observation_mismatch {
                    event!(
                        name: "eez.composer.observation.mismatch",
                        Level::WARN,
                        rollup_id,
                        composition_index = mismatch.composition_index,
                        observation_index = mismatch.observation_index,
                        expected_success = mismatch.expected_success,
                        observed_success = mismatch.observed_success,
                        expected_output_hash = %mismatch.expected_output_hash,
                        observed_output_hash = %mismatch.observed_output_hash,
                        resolved_held_tx = true,
                        tx_hash = %failed.hash,
                        sender = %failed.sender,
                        direction = ?failed.direction,
                        nonce = failed.nonce,
                        retrying = retry.len(),
                        evicted_in_flight = evicted_in_flight.len(),
                        evicted_queued = evicted_queued.len(),
                        "composer simulation and pinned-prefix replay disagree; fail-closed eviction of its nonce-chain suffix before rebuilding the batch",
                    );
                } else {
                    event!(
                        name: "eez.composer.prover.poison_ejected",
                        Level::ERROR,
                        rollup_id,
                        failure = %error,
                        tx_hash = %failed.hash,
                        sender = %failed.sender,
                        direction = ?failed.direction,
                        nonce = failed.nonce,
                        retrying = retry.len(),
                        "prover attributed the rejection to one held transaction; evicting its nonce-chain suffix for liveness; a valid transaction here indicates a Composer/prover disagreement",
                    );
                    for in_flight in &evicted_in_flight {
                        event!(
                            name: "eez.composer.prover.poison_in_flight_evicted",
                            Level::ERROR,
                            rollup_id,
                            tx_hash = %in_flight.hash,
                            sender = %in_flight.sender,
                            direction = ?in_flight.direction,
                            nonce = in_flight.nonce,
                            gap_at = failed.nonce,
                            "transaction belongs to a prover-attributed nonce-chain suffix; evicting for liveness",
                        );
                    }
                    for queued in &evicted_queued {
                        event!(
                            name: "eez.composer.prover.poison_queued_evicted",
                            Level::ERROR,
                            rollup_id,
                            tx_hash = %queued.hash,
                            sender = %queued.sender,
                            direction = ?queued.direction,
                            nonce = queued.nonce,
                            gap_at = failed.nonce,
                            "queued transaction depends on a prover-attributed nonce; evicting suffix for liveness",
                        );
                    }
                }

                // Each recursive attempt receives strictly fewer held
                // transactions, bounding recomposition by the original drain
                // size. Exact-slot attempts also retain the timestamp-based
                // proof cutoff.
                // The recursive call owns `retry`; retain this level's exact
                // set so an error can restore the users it still owns locally.
                let retry_after_error = retry.clone();
                let recomposed = Box::pin(self.compose_cross_chain_batch(
                    cc,
                    rollup_id,
                    retry,
                    parent_header,
                    timestamp,
                    suggested_fee_recipient,
                    bundle_target,
                    admission,
                ))
                .await;
                return match recomposed {
                    Ok(result) => Ok(result),
                    Err(error) => {
                        event!(
                                name: "eez.composer.prover.recompose_failed",
                                Level::ERROR,
                                rollup_id,
                                error = %error,
                                retrying = retry_after_error.len(),
                        "recomposition failed; restoring only candidates still owned by composition",
                            );
                        pool.requeue_composing(retry_after_error);
                        let fallback = self
                            .dispatch_minimal_postbatch(
                                ctx,
                                rollup_id,
                                rollup,
                                parent_header,
                                timestamp,
                                suggested_fee_recipient,
                                bundle_target,
                            )
                            .await
                            .unwrap_or_else(|fallback_error| {
                                event!(
                                    name: "eez.composer.prover.recompose_fallback_failed",
                                    Level::ERROR,
                                    rollup_id,
                                    error = %fallback_error,
                                    "minimal postBatch failed after recomposition error; Sequencer commits its fallback Sync block",
                                );
                                None
                            });
                        Ok(fallback)
                    }
                };
            }
            Err(PreparePostBatchError::Prover(error)) => {
                event!(Level::ERROR, rollup_id, %error,
                    "proof unavailable or rejected; retaining the complete range and all source ownership");
                pool.push_front_batch(survivors);
                return Ok(self.build_empty_slot_block(rollup, parent_header, timestamp));
            }
            Err(
                error
                @ (PreparePostBatchError::Build(_) | PreparePostBatchError::OverBudget { .. }),
            ) => {
                let error = error.to_string();
                event!(
                    name: "eez.composer.phase2.prepare_failed",
                    Level::WARN,
                    rollup_id,
                    error,
                    "postBatch construction failed; re-queueing survivors without charging a settlement attempt and degrading to minimal postBatch",
                );
                pool.push_front_batch(survivors);
                return Ok(self.build_empty_slot_block(rollup, parent_header, timestamp));
            }
            Err(PreparePostBatchError::CompositionBlobLimit { .. }) => {
                unreachable!("handled before historical splitting")
            }
        };
        let total_entries: usize = comp_refs.iter().map(|c| c.source.batch.entries.len()).sum();

        let manifest = match SettlementManifest::build(ManifestInput {
            rollup_id,
            sync_number: built.header.number(),
            sync_hash: built.header.hash(),
            users: survivors
                .iter()
                .map(|held| ProposedUser {
                    raw: &held.raw_tx,
                    source: eez_protocol::RollupId(if held.direction == Direction::Inbound {
                        0
                    } else {
                        rollup_id
                    }),
                    attempts: held.attempts,
                })
                .collect(),
            semantics: &semantic_transactions,
            outbound_groups: &pending_out,
            outbound_hosts: &outbound_entries,
            checkpoint_positions: &effect_end_positions,
            checkpoint_hashes: &pair_hashes,
        }) {
            Ok(manifest) => manifest,
            Err(error) => {
                event!(Level::ERROR, rollup_id, %error, "invalid user/effect manifest; retaining survivors");
                pool.push_front_batch(survivors);
                return self
                    .dispatch_minimal_postbatch(
                        ctx,
                        rollup_id,
                        rollup,
                        parent_header,
                        timestamp,
                        suggested_fee_recipient,
                        bundle_target,
                    )
                    .await;
            }
        };

        // ── Dispatch the rich L1 bundle and register the optimistic block. ──
        let sync_height = built.header.number();
        let post_batch_hash = post_batch_blob_transaction_hash(&prepared_postbatch)?;
        let mut bundle: Vec<Bytes> = Vec::with_capacity(1 + survivors.len());
        bundle.push(prepared_postbatch.clone());
        // Only INBOUND survivors ride the L1 bundle (L1-signed, execute on L1).
        // Outbound survivors are L2-signed — they run in the L2 Sync block + DA,
        // never the L1 bundle (an L2 tx is invalid on L1).
        // Preserve the held pool's exact FIFO order. Root pins were assigned at
        // those positions. Lookup-only transactions remain in this bundle so
        // L1 can return their proven result, but have no L2 system transaction.
        bundle.extend(
            survivors
                .iter()
                .filter(|held| held.direction == Direction::Inbound)
                .map(|held| held.raw_tx.clone()),
        );
        let attempt_id = match rollup.optimistic.begin(
            sync_height,
            post_batch_hash,
            parent_header.clone(),
            survivors.clone(),
            manifest,
            prepared_postbatch,
        ) {
            Ok(id) => id,
            Err(error) => {
                // Classification already removed poison and stale sources.
                // Do not return an error that requeues the original drain.
                event!(Level::ERROR, rollup_id, %error, "cannot record submission; retaining only classified survivors");
                pool.push_front_batch(survivors);
                return Ok(None);
            }
        };
        admission.completed();
        event!(
            name: "eez.composer.bundle.dispatched",
            Level::INFO,
            event_name = "eez.composer.bundle.dispatched",
            rollup_id,
            sync_height,
            post_batch_hash = %post_batch_hash,
            sync_hash = %built.header.hash(),
            user_hashes = ?survivors.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
            attempt_counts = ?survivors.iter().map(|tx| tx.attempts).collect::<Vec<_>>(),
            tx_count = bundle.len(),
            entry_count = total_entries,
            evicted_poison = poison.len(),
            "rich bundle dispatched to background observer; committing Sync block optimistically",
        );
        self.spawn_bundle_observer(
            ctx,
            rollup_id,
            sync_height,
            attempt_id,
            bundle,
            Arc::clone(&rollup.optimistic),
            bundle_target,
            &semantic_transactions,
            preparation_timing,
        );
        Ok(Some(SyncSlotBlock {
            payload: built.payload,
            header: built.header,
        }))
    }

    /// Build an empty Sync block, sign a leading-immediate-only postBatch
    /// covering `posted+1..=sync_height`, dispatch it to the background
    /// observer, and return the block for immediate commit. The Sync block is
    /// the last block of the batch range.
    ///
    /// If even the minimal postBatch can't be prepared, the block is
    /// still returned (commit-without-emission) — L2 cadence is
    /// unconditional; L1 catches up via the next emitted batch.
    async fn dispatch_minimal_postbatch(
        &self,
        ctx: &CrossChainExecCtx,
        rollup_id: u64,
        rollup: &RollupState<L2>,
        parent_header: &reth_primitives_traits::SealedHeader<alloy_consensus::Header>,
        timestamp: u64,
        suggested_fee_recipient: Address,
        bundle_target: BundleTarget,
    ) -> Result<Option<SyncSlotBlock>, String> {
        let (expected_timestamp, prev_randao) = rollup
            .l1_head
            .next_block_context(parent_header)
            .map_err(|e| e.to_string())?;
        if timestamp != expected_timestamp {
            return Err("minimal candidate timestamp differs from the genesis schedule".into());
        }
        let preparation_started_ms = crate::prover_retry::unix_time_millis();
        let empty_built = match build_sync_block(
            rollup.l2_provider.as_ref(),
            &self.inner.evm_config,
            parent_header,
            timestamp,
            prev_randao,
            suggested_fee_recipient,
            &[], // no system_txs — empty Sync block
        ) {
            Ok(built) => built,
            Err(err) => {
                event!(
                    name: "eez.composer.phase1.build_failed",
                    Level::ERROR,
                    rollup_id,
                    error = %err,
                    "minimal Sync block build failed; the explicit slot outcome determines whether to end this tick",
                );
                return Ok(None);
            }
        };

        // Keep a live target even when preparation exceeds its estimate. A
        // later execution height has no known parent to bind into the proof.
        let bundle_target = match bundle_target {
            BundleTarget::Exact { .. }
                if crate::prover_retry::PipelineDeadline::new(
                    self.inner.emission.timing,
                    bundle_target,
                    preparation_started_ms,
                )
                .is_some_and(|deadline| !deadline.remaining().is_zero()) =>
            {
                bundle_target
            }
            _ => self
                .inner
                .submitter
                .next_policy_target()
                .await
                .map_err(|error| error.to_string())?,
        };
        match self
            .evaluate_settlement_policy(
                rollup_id,
                rollup,
                parent_header,
                &empty_built,
                bundle_target,
                crate::settlement_policy::CrossChainWork::None,
                Default::default(),
            )
            .await
        {
            Ok(decision) if decision.is_due() && rollup.optimistic.blocking_height().is_none() => {}
            result => {
                if let Err(error) = result {
                    event!(name: "eez.composer.settlement.unavailable", Level::WARN, rollup_id, %error,
                        "settlement policy evidence unavailable; preserving pending work");
                }
                return Ok(Some(SyncSlotBlock {
                    payload: empty_built.payload,
                    header: empty_built.header,
                }));
            }
        }

        let preparation = self
            .prepare_post_batch_raw(
                ctx,
                rollup_id,
                &[], // no compositions → leading immediate only
                parent_header,
                empty_built.header.hash(),
                empty_built.header.hash(),
                Some(&empty_built.block),
                &[], // no cross-chain effects → no per-effect roots
                &[], // no cross-chain effects → no effect positions
                &[], // no outbound entries
                &[], // no outbound entry traces
                &[], // no outbound L2 sidecars
                &[], // no cross-chain semantic transactions
                0,   // no inline outbound target calls
                None,
                bundle_target,
                None,
                None,
                preparation_started_ms,
            )
            .await;
        if let Err(error) = &preparation
            && !matches!(error, PreparePostBatchError::OverBudget { .. })
        {
            event!(Level::ERROR, rollup_id, %error,
                "minimal batch unavailable; retaining complete range without capacity splitting");
            return Ok(Some(SyncSlotBlock {
                payload: empty_built.payload,
                header: empty_built.header,
            }));
        }
        let (minimal_postbatch_raw, preparation_timing) = match preparation {
            Ok(raw) => raw,
            Err(err) => {
                // The whole range will not fit; settle a bounded prefix of it so
                // the range shrinks and the next slot's rich batch can fit.
                let cursor = rollup.l1_head.cursor();
                let sync_height = parent_header.number() + 1;
                if let Err(chunk_err) = self
                    .emit_historical_chunk(
                        ctx,
                        rollup_id,
                        rollup,
                        cursor,
                        sync_height,
                        bundle_target,
                        match &err {
                            PreparePostBatchError::OverBudget { constraint, .. } => constraint,
                            _ => "preparation_fallback",
                        },
                    )
                    .await
                {
                    event!(
                        name: "eez.composer.emission.prefix_fallback_failed",
                        Level::ERROR,
                        rollup_id,
                        cursor,
                        sync_height,
                        error = %chunk_err,
                        full_range_error = %err,
                        "neither the full range nor a bounded prefix could be emitted",
                    );
                }
                return Ok(Some(SyncSlotBlock {
                    payload: empty_built.payload,
                    header: empty_built.header,
                }));
            }
        };
        let sync_height = empty_built.header.number();
        let post_batch_hash = post_batch_blob_transaction_hash(&minimal_postbatch_raw)?;
        let attempt_id = match rollup.optimistic.begin(
            sync_height,
            post_batch_hash,
            parent_header.clone(),
            Vec::new(),
            SettlementManifest::empty(sync_height, empty_built.header.hash()),
            minimal_postbatch_raw.clone(),
        ) {
            Ok(id) => id,
            Err(error) => {
                event!(Level::ERROR, rollup_id, %error, "cannot record minimal submission; committing without emission");
                return Ok(Some(SyncSlotBlock {
                    payload: empty_built.payload,
                    header: empty_built.header,
                }));
            }
        };
        event!(
            name: "eez.composer.phase1.bundle.dispatched",
            Level::INFO,
            event_name = "eez.composer.phase1.bundle.dispatched",
            rollup_id,
            sync_height,
            post_batch_hash = %post_batch_hash,
            "minimal postBatch dispatched to background observer (leading immediate only)",
        );
        self.spawn_bundle_observer(
            ctx,
            rollup_id,
            sync_height,
            attempt_id,
            vec![minimal_postbatch_raw],
            Arc::clone(&rollup.optimistic),
            bundle_target,
            &[],
            preparation_timing,
        );
        Ok(Some(SyncSlotBlock {
            payload: empty_built.payload,
            header: empty_built.header,
        }))
    }

    /// The cadence block for every no-emission path (gate-blocked, deferred-
    /// late, historical chunk) — one shape, the one a deriver rebuilds from L1.
    fn build_empty_slot_block(
        &self,
        rollup: &RollupState<L2>,
        parent_header: &reth_primitives_traits::SealedHeader<alloy_consensus::Header>,
        timestamp: u64,
    ) -> Option<SyncSlotBlock> {
        let (expected_timestamp, prev_randao) =
            rollup.l1_head.next_block_context(parent_header).ok()?;
        if timestamp != expected_timestamp {
            return None;
        }
        match build_sync_block(
            rollup.l2_provider.as_ref(),
            &self.inner.evm_config,
            parent_header,
            timestamp,
            prev_randao,
            Address::ZERO,
            &[],
        ) {
            Ok(built) => Some(SyncSlotBlock {
                payload: built.payload,
                header: built.header,
            }),
            Err(err) => {
                event!(
                    name: "eez.composer.sync_slot.build_failed",
                    Level::ERROR,
                    error = %err,
                    "empty Sync block build failed; the explicit slot outcome determines whether to end this tick",
                );
                None
            }
        }
    }

    /// A leading-immediate-only postBatch ending at a PAST on-grid empty block.
    /// `cursor` and an exact `bundle_target` belong to the caller's emission
    /// decision. Splitting capacity must not change the proof/submission target,
    /// including a next-slot replacement at an unchanged L1 execution height.
    async fn emit_historical_chunk(
        &self,
        ctx: &CrossChainExecCtx,
        rollup_id: u64,
        rollup: &RollupState<L2>,
        cursor: u64,
        sync_height: u64,
        bundle_target: BundleTarget,
        split_reason: &'static str,
    ) -> Result<(), String> {
        let preparation_started_ms = crate::prover_retry::unix_time_millis();
        let limits = self.inner.emission;
        let k = u64::from(limits.timing.k());
        // Under the cap the cap-bounded boundary is the sync height itself, which
        // is not a PAST block — step one K down so a range too big can still shrink.
        let snapped = limits
            .timing
            .historical_chunk_boundary(cursor, sync_height, limits.max_blocks)
            .unwrap_or_else(|| sync_height.saturating_sub(k));
        if snapped <= cursor {
            return Err(format!(
                "no on-grid boundary above cursor {cursor} below sync height {sync_height}"
            ));
        }
        // Candidates are highest-first. Cached exact lengths reject blob
        // overflow without another block walk or encoding pass. Preparation
        // distinguishes a hard gas budget
        // from unavailable inputs; only capacity permits trying a shorter range.
        // Endpoint-specific encoding and proof happen for a selected prefix.
        let candidates = self.empty_chunk_boundaries(rollup, cursor, snapped)?;
        let bundle_target = match bundle_target {
            BundleTarget::Exact { .. } => bundle_target,
            BundleTarget::NextBlock => self
                .inner
                .submitter
                .next_policy_target()
                .await
                .map_err(|error| error.to_string())?,
        };
        let provider = rollup.l2_provider.as_ref();
        let maximum_blobs = self.posting_blob_allowance(bundle_target, 0)?;
        let mut over_budget = 0usize;
        let mut split_reasons = std::collections::BTreeSet::from([split_reason]);
        for boundary in candidates {
            let boundary_header = provider
                .sealed_header(boundary)
                .map_err(|e| format!("sealed_header({boundary}): {e}"))?
                .ok_or_else(|| format!("local L2 header at boundary {boundary} missing"))?;
            if let Some(bytes) = self.historical_prefix_bytes(
                rollup_id,
                eez_l1::L2Endpoint {
                    number: boundary,
                    hash: boundary_header.hash(),
                },
            )? && bytes.div_ceil(eez_blob_codec::DATA_BYTES_PER_BLOB as u64)
                > maximum_blobs as u64
            {
                split_reasons.insert("blob_limit");
                over_budget += 1;
                continue;
            }
            let boundary_parent = provider
                .sealed_header(boundary - 1)
                .map_err(|e| format!("sealed_header({}): {e}", boundary - 1))?
                .ok_or_else(|| format!("local L2 header at {} missing", boundary - 1))?;

            let preparation = self
                .prepare_post_batch_raw(
                    ctx,
                    rollup_id,
                    &[], // no compositions → leading immediate only
                    &boundary_parent,
                    boundary_header.hash(),
                    boundary_header.hash(),
                    None, // terminal is committed → witnesses come from the store
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    0, // no inline outbound target calls
                    None,
                    bundle_target,
                    None,
                    None,
                    preparation_started_ms,
                )
                .await;
            let (raw, preparation_timing) = match preparation {
                Ok(raw) => raw,
                Err(PreparePostBatchError::OverBudget { constraint, .. }) => {
                    split_reasons.insert(constraint);
                    over_budget += 1;
                    continue;
                }
                Err(error) => return Err(error.to_string()),
            };

            event!(
                name: "eez.composer.emission.historical_chunk",
                Level::INFO,
                event_name = "eez.composer.emission.historical_chunk",
                split_reasons = ?split_reasons,
                rollup_id,
                cursor,
                boundary,
                sync_height,
                span = boundary - cursor,
                backlog = sync_height - cursor,
                over_budget,
                "settling the largest valid historical prefix within resolved capacity",
            );

            let post_batch_hash = post_batch_blob_transaction_hash(&raw)?;
            let attempt_id = rollup.optimistic.begin(
                boundary,
                post_batch_hash,
                boundary_parent,
                Vec::new(),
                SettlementManifest::empty(boundary, boundary_header.hash()),
                raw.clone(),
            )?;
            // A past terminal uses the future target already pinned for its
            // proof. Dispatch must not change the timestamp or its fork rules.
            self.spawn_bundle_observer(
                ctx,
                rollup_id,
                boundary,
                attempt_id,
                vec![raw],
                Arc::clone(&rollup.optimistic),
                bundle_target,
                &[],
                preparation_timing,
            );
            return Ok(());
        }
        Err(format!(
            "unpostable_range: every empty on-grid boundary in ({cursor}, {snapped}] exceeds {split_reasons:?} ({over_budget} candidates priced); retained for recovery",
        ))
    }

    /// On-grid heights in `(cursor, snapped]` whose block is EMPTY, highest
    /// first. Empty because the prover binds every settling-block tx to a
    /// claimed entry, so a tx-bearing terminal is unattestable for an
    /// anchor-only batch (§7c). Gas is NOT considered here — the caller prices
    /// each candidate on its fully encoded batch.
    fn empty_chunk_boundaries(
        &self,
        rollup: &RollupState<L2>,
        cursor: u64,
        snapped: u64,
    ) -> Result<Vec<u64>, String> {
        let k = u64::from(self.inner.emission.timing.k());
        let provider = rollup.l2_provider.as_ref();
        // Walk parent hashes rather than numbers: `block_by_number` races reth's
        // provider index for the newest block.
        let mut empty_at: Vec<bool> = Vec::new();
        let mut hash = provider
            .sealed_header(snapped)
            .map_err(|e| format!("sealed_header({snapped}): {e}"))?
            .ok_or_else(|| format!("local L2 header at {snapped} missing"))?
            .hash();
        let mut number = snapped;
        while number > cursor {
            let header = provider
                .sealed_header_by_hash(hash)
                .map_err(|e| format!("sealed_header_by_hash({hash}, n={number}): {e}"))?
                .ok_or_else(|| format!("local L2 header {hash} (n={number}) missing"))?;
            empty_at
                .push(header.transactions_root() == alloy_consensus::constants::EMPTY_ROOT_HASH);
            hash = header.parent_hash();
            number -= 1;
        }
        let step = usize::try_from(k).map_err(|e| format!("K overflows usize: {e}"))?;
        if step == 0 {
            return Err("K is zero".to_string());
        }
        let boundaries: Vec<u64> = (0..empty_at.len())
            .step_by(step)
            .filter(|&i| empty_at[i])
            .map(|i| snapped - i as u64)
            .collect();
        if boundaries.is_empty() {
            return Err(format!(
                "no empty on-grid boundary in ({cursor}, {snapped}]; retrying next slot"
            ));
        }
        Ok(boundaries)
    }

    /// Spawn the background bundle-observer task. It owns the submission inputs
    /// and records only the verdict in the optimistic ledger; it holds neither
    /// the Composer nor the block committer, so chain recovery stays in slot
    /// context through `recover_failed_batch`.
    fn spawn_bundle_observer(
        &self,
        ctx: &CrossChainExecCtx,
        rollup_id: u64,
        sync_height: u64,
        attempt_id: AttemptId,
        bundle: Vec<Bytes>,
        optimistic: Arc<OptimisticallyIncluded>,
        target: BundleTarget,
        semantics: &[eez_protocol::blob_semantics::SemanticTransaction],
        preparation_timing: crate::posting_events::PreparationTiming,
    ) {
        let submitter = ctx.submitter.clone();
        if let Some(counts) = optimistic.attempt_user_counts(attempt_id) {
            crate::submission_metrics::sync_requests(rollup_id, sync_height, counts);
        }
        let policy = self
            .settlement_status(rollup_id)
            .and_then(|status| status.policy);
        let reasons = policy.filter(|observation| {
            observation.decision.is_due() && matches!(target,
                BundleTarget::Exact { timestamp, .. } if timestamp == observation.target_l1_timestamp)
        }).map(|observation| crate::settlement_policy::DueReasons::ALL.into_iter()
            .filter_map(|(reason, name)| observation.decision.reasons.contains(reason).then_some(name))
            .collect());
        let posting_hash = optimistic.read(|state| {
            state
                .attempts
                .get(&attempt_id.generation)
                .filter(|attempt| attempt.id == attempt_id)
                .map(|attempt| attempt.manifest.sync_hash)
        });
        let events = posting_hash.map(|hash| {
            Arc::new(
                crate::posting_events::PostingEvents::new(
                    ctx.l1_chain_id,
                    self.inner.posting.registry,
                    rollup_id,
                    attempt_id.generation,
                    hash,
                    sync_height,
                    reasons,
                )
                .with_target(target),
            )
        });
        tokio::spawn(observe_bundle_outcome(
            rollup_id,
            sync_height,
            attempt_id,
            bundle,
            optimistic,
            submitter,
            target,
            self.inner.posting.clone(),
            semantics.to_vec(),
            events,
            preparation_timing,
        ));
    }

    /// Build + sign the L1 `postBatch` raw tx for a Sync slot's
    /// compositions, covering blocks `posted+1..=parent+1` (intermediate
    /// blocks + the new Sync block at `parent+1`, always the range's last
    /// block). Returns the signed EIP-2718 `postBatch` bytes and preparation
    /// timestamps for this candidate; the caller
    /// appends surviving inbound user transactions to the L1 bundle.
    ///
    /// A later `postAndVerifyBatch` for the same rollup and L1 block replaces the
    /// earlier deferred queue. The composer therefore merges one Sync slot into
    /// one batch so no earlier entries are lost. Entries are ordered as the
    /// leading anchor, outbound immediate entries in drain order, then inbound
    /// deferred entries in drain order. One proof covers the merged batch.
    ///
    /// **Chained block-hash commitments**: this function
    /// stitches the merged entries so `entries[k].currentRoot ==
    /// entries[k-1].newRoot` per rollup; EEZ.sol's `_applyRootUpdates`
    /// enforces the chain on L1 (`RootMismatch` revert) regardless of
    /// proof system. Immediate outbound settlement entries retain their
    /// established carrier plan. Applying inbound entries carry their exact L2
    /// checkpoint; lookup-only inbound entries receive a no-op update and no
    /// L2 transaction. The proof signer independently derives and verifies the
    /// same schedule; the last carrier is the final Sync-block hash.
    /// `sync_block_hash` is the required settlement-chain endpoint.
    ///
    /// `sync_block` is the terminal, `Some` only while freshly built; `None`
    /// (historical chunk) takes its witness from the store like the rest.
    ///
    /// `outbound_target_gas`: probed cost of the target calls EEZ.sol runs
    /// inline, which calldata size cannot predict.
    /// # Errors
    ///
    /// Returns an error for missing chain data, inconsistent IDs or commitments,
    /// malformed batch ranges, witness or proof failure, transaction signing,
    /// or RPC data required to build the submission.
    async fn prepare_post_batch_raw(
        &self,
        ctx: &CrossChainExecCtx,
        rollup_id: u64,
        compositions: &[&eez_protocol::Composition],
        parent_header: &reth_primitives_traits::SealedHeader<alloy_consensus::Header>,
        sync_block_hash: B256,
        ordinary_prefix_hash: B256,
        sync_block: Option<&reth_primitives_traits::RecoveredBlock<eez_primitives::Block>>,
        pair_hashes: &[B256],
        pair_positions: &[usize],
        outbound_entries: &[eez_protocol::abi::ExecutionEntrySol],
        outbound_entry_traces: &[eez_protocol::entries::EntryTrace],
        outbound_l2_groups: &[eez_protocol::system_tx::OutboundSyncGroup],
        semantic_transactions: &[eez_protocol::blob_semantics::SemanticTransaction],
        outbound_target_gas: u64,
        gas_projection: Option<&gas::Projection>,
        bundle_target: BundleTarget,
        admission: Option<&admission::Attempt>,
        candidate: Option<Arc<crate::admission_events::Candidate>>,
        preparation_started_ms: u64,
    ) -> Result<(Bytes, crate::posting_events::PreparationTiming), PreparePostBatchError> {
        use alloy_sol_types::SolCall;
        use eez_protocol::abi::{RollupIdWithProofSystemsSol, postAndVerifyBatchCall};

        let preparation_started = std::time::Instant::now();
        // Anchor-only windows bind the same canonical parent as cross-chain
        // proofs. Keep one deadline through preparation, proof and dispatch.
        let deadline = admission.and_then(|attempt| attempt.deadline).or_else(|| {
            crate::prover_retry::PipelineDeadline::new(
                self.inner.emission.timing,
                bundle_target,
                crate::prover_retry::unix_time_millis(),
            )
        });

        // Merge inbound compositions' L1 source batches, or start empty so an
        // idle Sync slot can carry only the leading immediate entry that
        // advances L1's stored commitment.
        let (mut batch, mut entry_traces) = if compositions.is_empty() {
            (eez_protocol::EvmBatch::default(), Vec::new())
        } else {
            let mut b = compositions[0].source.batch.clone();
            let mut traces = compositions[0].source.entry_traces.clone();
            for c in &compositions[1..] {
                b.entries.extend(c.source.batch.entries.iter().cloned());
                b.staticEntries
                    .extend(c.source.batch.staticEntries.iter().cloned());
                traces.extend(c.source.entry_traces.iter().cloned());
            }
            (b, traces)
        };

        // Prepend one leading immediate entry (`proxyEntryHash == 0`)
        // covering all ordinary L2 execution through the terminal prefix — EEZ.sol drains
        // it inline during postAndVerifyBatch, applying its root update
        // against L1's recorded commitment.
        //
        // `currentRoot` = L2.blockHash(posted) (the L1-confirmed cursor)
        // — must equal L1.config.root at postBatch time so the
        // deriver's check_claimed_state agrees. `newRoot` initially equals the
        // terminal ordinary-prefix block hash so later effect entries chain from it.
        // This is a Sync variant even when the prefix has no transactions.
        let rollup = self
            .inner
            .rollups
            .get(&rollup_id)
            .ok_or_else(|| format!("unknown rollup_id {rollup_id}"))?;
        let policy_anchor = rollup.l1_head.policy_anchor_snapshot();
        if policy_anchor.through.is_none() {
            return Err("canonical scan is incomplete while preparing settlement".into());
        }
        let endpoint = policy_anchor
            .endpoint
            .ok_or("validated settlement endpoint is unavailable")?;
        let (posted, pre_block_hash) = (endpoint.number, endpoint.hash);
        let pre_sync_block_hash = ordinary_prefix_hash;
        let immediate_entry = eez_protocol::abi::ExecutionEntrySol {
            rootUpdates: vec![eez_protocol::abi::RootUpdateSol {
                rollupId: rollup_id,
                currentRoot: pre_block_hash,
                newRoot: pre_sync_block_hash,
                etherDelta: eez_protocol::abi::EtherDeltaSol::ZERO,
            }],
            proxyEntryHash: B256::ZERO,
            l2ToL1Calls: Vec::new(),
            expectedL1ToL2Calls: Vec::new(),
            rollingHash: B256::ZERO,
            destinationRollupId: rollup_id,
            success: true,
            returnData: Bytes::new(),
        };
        batch.entries.insert(0, immediate_entry);
        entry_traces.insert(0, eez_protocol::entries::EntryTrace::default());

        // Splice OUTBOUND settlement entries after the leading anchor; its state
        // root update is attached below. The contract drains the contiguous
        // `proxyEntryHash==0` run inline, so order must be
        // `[anchor | outbound | inbound]`. `dest=rid` is the settlement's source
        // rollup (not the call's MAINNET target); `_validateStructure` checks it.
        for (k, oe) in outbound_entries.iter().enumerate() {
            let mut entry = oe.clone();
            entry.destinationRollupId = rollup_id;
            batch.entries.insert(1 + k, entry);
        }
        for (k, trace) in outbound_entry_traces.iter().cloned().enumerate() {
            entry_traces.insert(1 + k, trace);
        }

        // Each effect's settlement delta is the net value crossing this
        // rollup's boundary over the whole nested entry. L1 host flat arrays
        // are L2→L1 outflow; the paired L2 sidecar flat arrays are L1→L2
        // inflow. This mirrors TableGenerator's recursive prescan.
        let net_ether_delta =
            |inflow: U256, outflow: U256| -> Result<eez_protocol::abi::EtherDeltaSol, String> {
                let inflow = alloy_primitives::I256::try_from(inflow)
                    .map_err(|error| format!("entry ether inflow overflows I256: {error}"))?;
                let outflow = alloy_primitives::I256::try_from(outflow)
                    .map_err(|error| format!("entry ether outflow overflows I256: {error}"))?;
                let delta = inflow
                    .checked_sub(outflow)
                    .ok_or_else(|| "entry net ether delta overflows I256".to_string())?;
                if delta < alloy_primitives::I256::from(eez_protocol::abi::EtherDeltaSol::MIN)
                    || delta > alloy_primitives::I256::from(eez_protocol::abi::EtherDeltaSol::MAX)
                {
                    return Err("entry net ether delta overflows protocol int192".to_string());
                }
                Ok(eez_protocol::abi::EtherDeltaSol::from(delta))
            };
        let outbound_effect_count = outbound_l2_groups
            .iter()
            .map(|group| group.effects.len())
            .sum::<usize>();
        let outbound_settlement_root_counts =
            eez_protocol::system_tx::outbound_settlement_root_counts(outbound_entries)?;
        let covered_outbound_roots = outbound_settlement_root_counts.iter().sum::<usize>();
        if covered_outbound_roots != outbound_effect_count {
            return Err(format!(
                "outbound settlement hosts cover {covered_outbound_roots} roots, but Sync groups carry {outbound_effect_count}",
            )
            .into());
        }
        let outbound_effects = outbound_l2_groups
            .iter()
            .flat_map(|group| group.effects.iter())
            .collect::<Vec<_>>();
        let fifo_actions_by_composition = compositions
            .iter()
            .enumerate()
            .map(|(composition_index, composition)| {
                eez_protocol::settlement::composition_fifo_actions(composition).map_err(|error| {
                    format!("composition {composition_index} FIFO actions: {error}")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let inbound_effect_count = fifo_actions_by_composition
            .iter()
            .flatten()
            .filter(|action| {
                matches!(
                    action,
                    eez_protocol::settlement::FifoSourceAction::Mutable { applies: true }
                )
            })
            .count();
        if pair_hashes.len() != outbound_effect_count + inbound_effect_count {
            return Err(format!(
                "pair hashes cover {} effects, expected {outbound_effect_count} outbound + {inbound_effect_count} inbound",
                pair_hashes.len(),
            )
            .into());
        }
        if pair_positions.len() != pair_hashes.len() {
            return Err(format!(
                "pair positions cover {} effects, but pair hashes cover {}",
                pair_positions.len(),
                pair_hashes.len(),
            )
            .into());
        }
        let inbound_entry_count = compositions
            .iter()
            .map(|composition| composition.source.batch.entries.len())
            .sum::<usize>();
        let mut effect_ether_deltas =
            Vec::with_capacity(outbound_entries.len() + inbound_entry_count);
        let mut effect_block_hashes = Vec::with_capacity(outbound_entries.len());
        let mut effect_successes = Vec::with_capacity(outbound_entries.len());
        let mut outbound_effect_cursor = 0usize;
        for (settlement, root_count) in outbound_entries.iter().zip(outbound_settlement_root_counts)
        {
            let outflow = eez_protocol::entries::entry_flat_value_sum(settlement)
                .ok_or_else(|| "invalid outbound settlement value shape".to_string())?;
            let effect_end = outbound_effect_cursor + root_count;
            let inflow = outbound_effects[outbound_effect_cursor..effect_end]
                .iter()
                .try_fold(U256::ZERO, |total, effect| {
                    let source_value = match &effect.source_entry {
                        eez_protocol::system_tx::OutboundSourceEntry::Mutable(sidecar) => {
                            eez_protocol::entries::entry_flat_value_sum(sidecar).ok_or_else(
                                || "invalid outbound L2 sidecar value shape".to_string(),
                            )?
                        }
                        eez_protocol::system_tx::OutboundSourceEntry::Static(_) => U256::ZERO,
                    };
                    effect.callback_entries.iter().try_fold(
                        total
                            .checked_add(source_value)
                            .ok_or_else(|| "outbound L2 sidecar value sum overflow".to_string())?,
                        |subtotal, sidecar| {
                            let value =
                                eez_protocol::entries::entry_flat_value_sum(sidecar.as_abi_entry())
                                    .ok_or_else(|| {
                                        "invalid outbound callback sidecar value shape".to_string()
                                    })?;
                            subtotal
                                .checked_add(value)
                                .ok_or_else(|| "outbound L2 sidecar value sum overflow".to_string())
                        },
                    )
                })?;
            effect_ether_deltas.push(net_ether_delta(inflow, outflow)?);
            effect_block_hashes.push(pair_hashes[effect_end - 1]);
            effect_successes.push(settlement.success);
            outbound_effect_cursor = effect_end;
        }
        for (composition_index, (composition, fifo_actions)) in compositions
            .iter()
            .zip(&fifo_actions_by_composition)
            .enumerate()
        {
            let sidecars = composition
                .targets
                .iter()
                .filter(|target| target.rollup_id == eez_protocol::RollupId(rollup_id))
                .flat_map(|target| target.batch.entries.iter())
                .collect::<Vec<_>>();
            let applying_count = fifo_actions
                .iter()
                .filter(|action| {
                    matches!(
                        action,
                        eez_protocol::settlement::FifoSourceAction::Mutable { applies: true }
                    )
                })
                .count();
            if applying_count != sidecars.len() {
                return Err(format!(
                    "composition {composition_index} applying inbound/sidecar count mismatch: {applying_count} != {}",
                    sidecars.len(),
                )
                .into());
            }
            let mut source_cursor = 0usize;
            let mut sidecar_cursor = 0usize;
            for action in fifo_actions {
                let eez_protocol::settlement::FifoSourceAction::Mutable { applies } = action else {
                    continue;
                };
                let source_entry = composition
                    .source
                    .batch
                    .entries
                    .get(source_cursor)
                    .ok_or_else(|| {
                        format!(
                            "composition {composition_index} is missing mutable source entry {source_cursor}"
                        )
                    })?;
                source_cursor += 1;
                if !*applies {
                    effect_ether_deltas.push(eez_protocol::abi::EtherDeltaSol::ZERO);
                    continue;
                }
                let sidecar = sidecars.get(sidecar_cursor).ok_or_else(|| {
                    format!(
                        "composition {composition_index} is missing applying sidecar {sidecar_cursor}"
                    )
                })?;
                sidecar_cursor += 1;
                if source_entry.proxyEntryHash != sidecar.proxyEntryHash {
                    return Err(format!(
                        "inbound settlement/sidecar proxy hash mismatch: {} != {}",
                        source_entry.proxyEntryHash, sidecar.proxyEntryHash,
                    )
                    .into());
                }
                let outflow = eez_protocol::entries::entry_flat_value_sum(source_entry)
                    .ok_or_else(|| "invalid inbound L1 entry value shape".to_string())?;
                let inflow = eez_protocol::entries::entry_flat_value_sum(sidecar)
                    .ok_or_else(|| "invalid inbound L2 sidecar value shape".to_string())?;
                effect_ether_deltas.push(net_ether_delta(inflow, outflow)?);
            }
            if source_cursor != composition.source.batch.entries.len()
                || sidecar_cursor != sidecars.len()
            {
                return Err(format!(
                    "composition {composition_index} FIFO consumption mismatch: source {source_cursor}/{}, sidecars {sidecar_cursor}/{}",
                    composition.source.batch.entries.len(),
                    sidecars.len(),
                )
                .into());
            }
        }

        // Outbound effects execute in postBatch's immediate prefix, before any
        // inbound user transaction. Preserve their established carrier plan:
        // an immediate success (or the anchor) can persist checkpoints for
        // failed outbound entries.
        let outbound_settlement_count = outbound_entries.len();
        let (anchor_new_root, outbound_carrier_roots) =
            eez_protocol::settlement::settlement_root_carriers(
                pre_sync_block_hash,
                effect_block_hashes[..outbound_settlement_count]
                    .iter()
                    .copied()
                    .zip(
                        effect_successes[..outbound_settlement_count]
                            .iter()
                            .copied(),
                    ),
            );
        batch.entries[0].rootUpdates[0].newRoot = anchor_new_root;

        for (entry_index, new_root) in outbound_carrier_roots.iter().copied().enumerate() {
            let ether_delta = effect_ether_deltas[entry_index];
            batch.entries[1 + entry_index].rootUpdates = vec![eez_protocol::abi::RootUpdateSol {
                rollupId: rollup_id,
                currentRoot: B256::ZERO,
                newRoot: new_root,
                etherDelta: ether_delta,
            }];
        }

        // With no effects the anchor itself carries the terminal block. This
        // also gives a static-only FIFO transaction the root that is live after
        // postBatch and before its user transaction executes.
        if pair_hashes.is_empty() {
            batch.entries[0].rootUpdates[0].newRoot = sync_block_hash;
        }

        // Inbound entries execute in user-transaction FIFO order. Static calls
        // pin the root live at their exact action position; applying mutable
        // calls advance it to their independently built L2 checkpoint. Failed
        // calls and roots later reverted by their source frame are lookup-only:
        // they use a no-op update and have no L2 system transaction.
        let mut live_root = outbound_carrier_roots
            .last()
            .copied()
            .unwrap_or(batch.entries[0].rootUpdates[0].newRoot);
        let mut mutable_batch_index = 1 + outbound_settlement_count;
        let mut mutable_effect_index = outbound_settlement_count;
        let mut pair_hash_index = outbound_effect_count;
        let mut static_batch_index = 0usize;
        let mut observation_claims = Vec::new();

        for (composition_index, (composition, fifo_actions)) in compositions
            .iter()
            .zip(&fifo_actions_by_composition)
            .enumerate()
        {
            let mutable_count = composition.source.batch.entries.len();
            let applying_count = fifo_actions
                .iter()
                .filter(|action| {
                    matches!(
                        action,
                        eez_protocol::settlement::FifoSourceAction::Mutable { applies: true }
                    )
                })
                .count();
            let effect_end = pair_hash_index + applying_count;
            let checkpoints = pair_hashes.get(pair_hash_index..effect_end).ok_or_else(|| {
                format!(
                    "composition {composition_index} requires pair hashes {pair_hash_index}..{effect_end}, but only {} exist",
                    pair_hashes.len(),
                )
            })?;
            let plan = eez_protocol::settlement::fifo_transaction_root_plan(
                live_root,
                fifo_actions,
                checkpoints,
            )
            .map_err(|error| format!("composition {composition_index} root plan: {error}"))?;
            observation_claims.extend(
                eez_protocol::settlement::fifo_observation_claims(composition, &plan)
                    .map_err(|error| {
                        format!("composition {composition_index} observations: {error}")
                    })?
                    .into_iter()
                    .map(|claim| (composition_index, claim)),
            );

            for (local_index, new_root) in plan.mutable_new_roots.iter().copied().enumerate() {
                let entry = batch
                    .entries
                    .get_mut(mutable_batch_index + local_index)
                    .ok_or_else(|| {
                        format!(
                            "composition {composition_index} mutable entry {local_index} is missing from merged batch"
                        )
                    })?;
                if !entry.rootUpdates.is_empty() {
                    return Err(format!(
                        "composition {composition_index} mutable entry {local_index} unexpectedly carries pre-filled roots"
                    )
                    .into());
                }
                entry.rootUpdates = vec![eez_protocol::abi::RootUpdateSol {
                    rollupId: rollup_id,
                    currentRoot: B256::ZERO,
                    newRoot: new_root,
                    etherDelta: effect_ether_deltas[mutable_effect_index + local_index],
                }];
            }

            for (local_index, root) in plan.static_roots.iter().copied().enumerate() {
                let entry = batch
                    .staticEntries
                    .get_mut(static_batch_index + local_index)
                    .ok_or_else(|| {
                        format!(
                            "composition {composition_index} static entry {local_index} is missing from merged batch"
                        )
                    })?;
                if entry.destinationRollupId != rollup_id {
                    return Err(format!(
                        "static entry destination {} does not match settled rollup {rollup_id}",
                        entry.destinationRollupId,
                    )
                    .into());
                }
                if !entry.expectedRoots.is_empty() {
                    return Err(
                        "materialized static entry unexpectedly carries pre-filled roots".into(),
                    );
                }
                entry.expectedRoots = vec![eez_protocol::abi::ExpectedRootPerRollupSol {
                    rollupId: rollup_id,
                    root,
                }];
            }

            mutable_batch_index += mutable_count;
            mutable_effect_index += mutable_count;
            pair_hash_index = effect_end;
            static_batch_index += composition.source.batch.staticEntries.len();
            live_root = plan.endpoint;
        }

        if mutable_effect_index != effect_ether_deltas.len()
            || pair_hash_index != pair_hashes.len()
            || static_batch_index != batch.staticEntries.len()
        {
            return Err(format!(
                "FIFO settlement consumption mismatch: effects {mutable_effect_index}/{}, pair hashes {pair_hash_index}/{}, statics {static_batch_index}/{}",
                effect_ether_deltas.len(),
                pair_hashes.len(),
                batch.staticEntries.len(),
            )
            .into());
        }

        // Assign every lookup/static claim to the exact canonical block prefix
        // named by its FIFO pin. The terminal witness replays these calls at
        // those positions and includes every account/code/storage read.
        let observation_schedule = if observation_claims.is_empty() {
            Vec::new()
        } else {
            let terminal = sync_block.ok_or_else(|| {
                "lookup/static observations require the freshly built terminal block".to_owned()
            })?;
            let transaction_count = terminal.body().transactions.len();
            let mut schedule = Vec::with_capacity(observation_claims.len());
            for (composition_index, claim) in observation_claims {
                let after_transactions = if claim.pinned_block_hash == pre_sync_block_hash {
                    terminal
                        .body()
                        .transactions
                        .iter()
                        .position(|tx| matches!(tx, eez_primitives::EezTxEnvelope::System(_)))
                        .unwrap_or(transaction_count)
                } else if let Some(index) = pair_hashes
                    .iter()
                    .position(|hash| *hash == claim.pinned_block_hash)
                {
                    pair_positions[index].checked_add(1).ok_or_else(|| {
                        "observation transaction position overflows usize".to_owned()
                    })?
                } else if claim.pinned_block_hash == sync_block_hash {
                    transaction_count
                } else {
                    return Err(format!(
                        "lookup/static observation pins unknown Sync-block prefix {}",
                        claim.pinned_block_hash,
                    )
                    .into());
                };
                if after_transactions > transaction_count {
                    return Err(format!(
                        "lookup/static observation targets prefix {after_transactions} of a {transaction_count}-transaction Sync block"
                    )
                    .into());
                }
                schedule.push((
                    composition_index,
                    BlockObservation {
                        after_transactions,
                        manager_address: ctx.eezl2_address,
                        claim,
                    },
                ));
            }
            if schedule
                .windows(2)
                .any(|pair| pair[0].1.after_transactions > pair[1].1.after_transactions)
            {
                return Err("lookup/static observation schedule is not FIFO".into());
            }
            schedule
        };

        // Stitch the per-rollup root-update chain: EEZ.sol `_executeEntry`
        // enforces `config.root == update.currentRoot`, then `_applyRootUpdates` sets it to
        // `newRoot`, so each entry's `currentRoot` must chain to the prior
        // entry's `newRoot`. Outbound failed checkpoints use the immediate
        // carrier plan. Lookup-only inbound entries remain `R_k → R_k` and do
        // not advance the commitment.
        let mut running_roots: HashMap<u64, B256> = HashMap::new();
        for entry in &mut batch.entries {
            for update in &mut entry.rootUpdates {
                if let Some(prev_new) = running_roots.get(&update.rollupId).copied() {
                    update.currentRoot = prev_new;
                }
                running_roots.insert(update.rollupId, update.newRoot);
            }
        }

        // Fail before proving or submission if prefix reconstruction did not
        // converge on the exact full Sync-block hash.
        let stitched_endpoint = batch
            .entries
            .last()
            .and_then(|entry| entry.rootUpdates.last())
            .map(|update| update.newRoot);
        if stitched_endpoint != Some(sync_block_hash) {
            return Err(format!(
                "settlement chain endpoint {stitched_endpoint:?} does not match Sync-block hash {sync_block_hash}"
            )
            .into());
        }

        // The L1 rolling seed commits the finalized ordered root updates, so
        // it can only be computed after the stitch above has set every
        // `currentRoot` and the anchor-only path has set its final root.
        eez_protocol::entries::finalize_l1_entry_traces(&mut batch, &entry_traces)
            .map_err(|error| format!("finalize L1 rolling hashes: {error}"))?;

        // The contract drains the leading contiguous `proxyEntryHash == 0` run
        // inline: one anchor plus the outbound immediate entries.
        // Inbound deferred entries (proxyEntryHash != 0) queue for
        // `executeCrossChainCall` consumption. N=0 for inbound-only → 1.
        batch.immediateEntryCount = U256::from(1 + outbound_entries.len() as u64);

        // Registry-id settlement gate: refuse a batch carrying any non-registry
        // destinationRollupId (e.g. an un-rewritten MAINNET(0) outbound entry).
        ensure_batch_registry_native(&batch, rollup_id)?;
        batch.blockNumber = u64::MAX;
        batch.proofSystems = vec![ctx.ecdsa_proof_system_address];
        batch.rollupIdsWithProofSystems = vec![RollupIdWithProofSystemsSol {
            rollupId: rollup_id,
            proofSystemIndexes: vec![0u64],
        }];
        // Encode the full L2 block range this batch covers, not just the
        // Sync block: the composer accumulates intermediate live blocks between
        // Sync slots and the deriver must replay all of them. Range:
        // from = cursor+1 (first unposted L2
        // block), to = parent+1 (the Sync block).
        //
        // Ordinary blocks publish their exact execution inputs; the terminal
        // retains canonical full-block RLP. Headers, receipts and execution
        // requests are reconstructed from the preceding settled block. Intermediate
        // blocks [from..to-1] are walked via parent-hash
        // from `parent_header` — `block_by_hash` is reliable for
        // canonical blocks, unlike `block_by_number`, which races reth's
        // provider index for the newest block. Publish the ordinary prefix
        // and metadata; authenticated semantic actions reconstruct the complete
        // terminal suffix independently on followers and signers.
        // Reuse the SAME cursor read that anchored the leading
        // immediate's currentRoot above — a second read could race the
        // Deriver's cursor advance and desync the callData range from
        // the root-update anchor (TOCTOU).
        let from = posted
            .checked_add(1)
            .ok_or("settlement range height overflow")?;
        let sync_block_number = parent_header
            .number()
            .checked_add(1)
            .ok_or("settlement terminal height overflow")?;
        let span = self
            .inner
            .emission
            .checked_window_span(posted, sync_block_number)?;
        let span_len = usize::try_from(span).map_err(|e| format!("batch span overflow: {e}"))?;
        let mut block_rlps_rev: Vec<Vec<u8>> = Vec::with_capacity(span_len.saturating_sub(1));
        let mut witness_ids = Vec::with_capacity(span_len.saturating_sub(1));
        let mut cursor_hash = parent_header.hash();
        let mut cursor_number = parent_header.number();
        while cursor_number >= from {
            // `BlockSource::Any` so the lookup finds the parent even
            // while it's still "pending" in reth: at compose_sync_slot
            // time the Sequencer has done `newPayload(parent)` but the
            // promoting FCU fires on the next commit, so the parent is in
            // reth's tree but not yet canonical-head. Deeper ancestors
            // are already canonical, so `Any` finds them too.
            let block = rollup
                .l2_provider
                .find_block_by_hash(cursor_hash, BlockSource::Any)
                .map_err(|e| {
                    format!("l2_provider.find_block_by_hash({cursor_hash}, n={cursor_number}): {e}")
                })?
                .ok_or_else(|| {
                    format!("local L2 block hash {cursor_hash} (n={cursor_number}) missing")
                })?;
            let tx_bytes: Vec<Vec<u8>> = block
                .body()
                .transactions()
                .iter()
                .map(Encodable2718::encoded_2718)
                .collect();
            // Intermediate blocks may contain only user transactions; system
            // transactions belong to the Sync block. Reject both typed and
            // SYSTEM_ADDRESS-signed forms so an unrecovered failed Sync block
            // cannot reintroduce phantom cross-chain effects.
            for enc in &tx_bytes {
                let is_system = if enc.first() == Some(&eez_primitives::SYSTEM_TX_TYPE) {
                    true
                } else {
                    use alloy_eips::eip2718::Decodable2718 as _;
                    use reth_primitives_traits::SignerRecoverable as _;
                    let mut raw: &[u8] = enc.as_slice();
                    let tx = eez_primitives::EezTxEnvelope::decode_2718(&mut raw).map_err(|e| {
                        format!(
                            "system-tx guard: decode_2718 failed for a tx in \
                                 intermediate block {cursor_number}: {e}"
                        )
                    })?;
                    let signer = tx.recover_signer().map_err(|e| {
                        format!(
                            "system-tx guard: recover_signer failed for a tx in \
                             intermediate block {cursor_number}: {e}"
                        )
                    })?;
                    signer == eez_primitives::SYSTEM_ADDRESS
                };
                if is_system {
                    return Err(format!(
                        "intermediate block {cursor_number} carries a system transaction; \
                         emission is blocked until the failed Sync block is recovered"
                    )
                    .into());
                }
            }
            witness_ids.push((cursor_number, cursor_hash));
            block_rlps_rev.push(
                self.cached_block_rlp(rollup_id, cursor_hash)
                    .unwrap_or_else(|| alloy_rlp::encode(&block)),
            );
            if cursor_number == 0 {
                break;
            }
            cursor_hash = block.header().parent_hash();
            cursor_number -= 1;
        }
        if cursor_number != posted || cursor_hash != pre_block_hash {
            return Err(
                "candidate ancestry does not reach the exact validated settlement anchor".into(),
            );
        }
        block_rlps_rev.reverse();
        witness_ids.reverse();
        let mut block_rlps = block_rlps_rev;
        let terminal_rlp = match sync_block {
            Some(block) => alloy_rlp::encode(block.clone().into_block()),
            None => {
                let block = rollup
                    .l2_provider
                    .find_block_by_hash(sync_block_hash, BlockSource::Any)
                    .map_err(|error| format!("find terminal block {sync_block_hash}: {error}"))?
                    .ok_or_else(|| format!("terminal block {sync_block_hash} is missing"))?;
                alloy_rlp::encode(block)
            }
        };
        block_rlps.push(terminal_rlp);
        // Encoded `ExecutionEntrySol` values let followers reconstruct system
        // transactions. Outbound entries describe L1 settlement and L2 loads;
        // inbound target batches carry the L2 delivery inputs. These operations
        // and the native semantic stream are carried only by physical blobs;
        // `batch.callData` remains empty.
        let anchor_header = rollup
            .l2_provider
            .sealed_header_by_hash(pre_block_hash)
            .map_err(|error| format!("load operations anchor {pre_block_hash}: {error}"))?
            .ok_or_else(|| format!("operations anchor {pre_block_hash} is missing"))?;
        let payload = eez_driver::derivation::encode_operations(
            &self.inner.evm_config,
            &anchor_header,
            &block_rlps,
        )
        .map_err(|e| format!("encode derivable chain operations: {e}"))?;
        let semantic_messages = semantic_transactions
            .iter()
            .map(eez_protocol::blob_semantics::encode_semantic_transaction)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("encode native cross-chain semantics: {error}"))?
            .into_iter()
            .flatten()
            .collect();
        let operations_bytes = payload.len();
        let encoded_stream = eez_blob_codec::encode_native_stream(
            rollup_id,
            alloy_primitives::Bytes::from(payload),
            semantic_messages,
        )
        .map_err(|error| format!("encode native blob payload: {error}"))?;
        let encoded_stream_bytes = encoded_stream.blob_portion.len();
        self.record_final_encoding(
            rollup_id,
            eez_l1::L2Endpoint {
                number: posted,
                hash: pre_block_hash,
            },
            eez_l1::L2Endpoint {
                number: sync_block_number,
                hash: sync_block_hash,
            },
            encoded_stream_bytes,
        );
        let blob_count = encoded_stream_bytes.div_ceil(eez_blob_codec::DATA_BYTES_PER_BLOB);
        use alloy_consensus::Transaction as _;
        use alloy_eips::Decodable2718 as _;
        let companion_blobs = semantic_transactions
            .iter()
            .filter(|tx| tx.origin_chain.0 == 0)
            .try_fold(0u64, |total, tx| {
                let envelope = alloy_consensus::TxEnvelope::decode_2718(&mut tx.tx_data.as_ref())
                    .map_err(|error| format!("decode required L1 companion: {error}"))?;
                let count = envelope.blob_versioned_hashes().map_or(0, <[B256]>::len);
                total
                    .checked_add(u64::try_from(count).map_err(|_| "companion blob count overflow")?)
                    .ok_or_else(|| "companion blob total overflow".to_owned())
            })?;
        let maximum_blobs = self.posting_blob_allowance(bundle_target, companion_blobs)?;
        if blob_count > maximum_blobs {
            if !semantic_transactions.is_empty() {
                let terminal_payload = eez_driver::derivation::encode_operations(
                    &self.inner.evm_config,
                    parent_header,
                    &block_rlps[block_rlps.len() - 1..],
                )
                .map_err(|error| error.to_string())?;
                let semantic_bytes = encoded_stream_bytes
                    .checked_sub(
                        eez_blob_codec::native_stream_len(operations_bytes, 0)
                            .map_err(|error| error.to_string())?,
                    )
                    .ok_or("native semantic size underflow")?;
                let minimum_bytes =
                    eez_blob_codec::native_stream_len(terminal_payload.len(), semantic_bytes)
                        .map_err(|error| error.to_string())?;
                let required = minimum_bytes.div_ceil(eez_blob_codec::DATA_BYTES_PER_BLOB);
                if required > maximum_blobs {
                    return Err(PreparePostBatchError::CompositionBlobLimit {
                        required,
                        maximum: maximum_blobs,
                    });
                }
            }
            event!(
                name: "eez.composer.emission.candidate_over_blob_limit",
                Level::DEBUG,
                rollup_id,
                from,
                to = sync_block_number,
                blobs = blob_count,
                maximum = maximum_blobs,
                "candidate range exceeds the per-transaction blob limit",
            );
            return Err(PreparePostBatchError::OverBudget {
                constraint: "blob_limit",
                required: blob_count as u64,
                maximum: maximum_blobs as u64,
            });
        }
        let transaction_blobs = eez_blob_codec::pack(&encoded_stream.blob_portion)
            .map_err(|error| format!("pack native blob payload: {error}"))?;
        event!(name: "eez.composer.settlement.encoding", Level::INFO, rollup_id,
            from, to = sync_block_number, encoded_stream_bytes, blob_count, maximum_blobs,
            "sized native settlement data before KZG and proving");
        batch.blobIndices = (0..transaction_blobs.len()).map(U256::from).collect();
        batch.callData = Bytes::new();

        // Price before witnesses and proving. Only the typed OverBudget result
        // authorizes capacity-driven selection of a smaller historical prefix.
        let mut empty_projection = None;
        let posting = &self.inner.posting;
        let provider = &posting.chain;
        let number = provider
            .headers
            .best_block_number()
            .map_err(|error| error.to_string())?;
        let header = provider
            .headers
            .header_by_number(number)
            .map_err(|error| error.to_string())?
            .ok_or("L1 header unavailable for target gas budget")?;
        let BundleTarget::Exact {
            block: target_block,
            timestamp,
        } = bundle_target
        else {
            return Err("proof requires an exact L1 target".into());
        };
        if header.number.checked_add(1) != Some(target_block) || timestamp <= header.timestamp {
            return Err("proof target's canonical L1 parent is unavailable".into());
        }
        let l1_posting = eez_prover::L1PostingContext {
            chain_id: ctx.l1_chain_id,
            parent_number: header.number,
            parent_hash: header.hash_slow(),
            target_timestamp: timestamp,
        }
        .validate()
        .map_err(|error| error.to_string())?;
        if gas_projection.is_none() && semantic_transactions.is_empty() {
            let hash = alloy_consensus::Sealable::hash_slow(&header);
            empty_projection = provider.provider.state_by_block_hash(hash)
                .map_err(|error| error.to_string())
                .and_then(|state| gas::Projection::open(state.as_ref(), posting.registry, rollup_id))
                .unwrap_or_else(|error| {
                    event!(Level::WARN, rollup_id, %error, "empty post gas evidence unavailable; retaining conservative reserve");
                    None
                });
        }
        let ceiling = posting
            .gas_budget(&header, bundle_target)
            .map_err(|error| error.to_string())?;
        let mut sized = batch.clone();
        sized.proofs = vec![Bytes::from(vec![0xffu8; MAX_PROOF_BYTES])];
        let projected = postAndVerifyBatchCall { batch: sized }.abi_encode();
        // EIP-7623 charges max(standard, floor). Standard now covers the bytes the
        // drain could not see; floor is what a fat, entry-light batch hits.
        let mut projected_gas = gas_projection.or(empty_projection.as_ref()).map_or_else(
            || {
                projected_postbatch_gas(
                    batch.entries.len() as u64,
                    calldata_gas(&projected),
                    outbound_target_gas,
                )
            },
            |projection| {
                projection.batch_gas(&batch, calldata_gas(&projected), outbound_target_gas)
            },
        );
        let floor_active = posting.calldata_floor_active(bundle_target);
        if floor_active {
            projected_gas = projected_gas.max(calldata_floor_gas(&projected));
        }
        if let Some(candidate) = &candidate {
            candidate.emit(
                "full_projection",
                serde_json::json!({"projected_gas":projected_gas,
                "calldata_bytes":projected.len(),"calldata_gas":calldata_gas(&projected),
                "calldata_floor_gas":calldata_floor_gas(&projected),"gas_ceiling":ceiling,
                "entries":batch.entries.len(),"blobs":blob_count}),
            );
        }
        match posting.fees.declared_gas(projected_gas, ceiling) {
            Ok(_) => {}
            Err(eez_l1::posting_fees::PostingFeeError::GasBudget {
                estimated,
                margin_bps,
                maximum,
            }) => {
                let required = u64::try_from(
                    eez_l1::posting_fees::with_margin(u128::from(estimated), margin_bps)
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|_| "declared gas overflow")?;
                return Err(PreparePostBatchError::OverBudget {
                    constraint: "l1_tx_gas_budget",
                    required,
                    maximum,
                });
            }
            Err(error) => return Err(error.to_string().into()),
        }
        if projected_gas > ceiling {
            event!(
                name: "eez.composer.emission.candidate_over_budget",
                Level::DEBUG,
                rollup_id,
                from,
                to = sync_block_number,
                projected_gas,
                ceiling,
                calldata_bytes = projected.len(),
                "candidate range exceeds the postBatch gas budget",
            );
            return Err(PreparePostBatchError::OverBudget {
                constraint: "l1_tx_gas_budget",
                required: projected_gas,
                maximum: ceiling,
            });
        }

        if !rollup.l1_head.generation_is(policy_anchor.generation) {
            return Err("canonical settlement evidence changed while sizing the candidate".into());
        }
        let blob_preparation_started = std::time::Instant::now();
        let prepared_blob_sidecar = eez_blob_codec::prepare_blob_sidecar_with_limit(
            &transaction_blobs,
            ctx.blob_sidecar_encoding,
            maximum_blobs,
        )
        .map_err(|error| format!("prepare postBatch blob sidecar: {error}"))?;
        let blob_preparation_elapsed = blob_preparation_started.elapsed();
        if let Some(attempt) = admission {
            attempt.blobs_prepared(blob_count, blob_preparation_elapsed);
        }

        // Prove the assembled window (proofs[] empty — not part of the
        // publicInputsHash). Mock ignores the context; a remote prover re-executes
        // `blocks`. Settlement path, off block production.
        let terminal_observations = observation_schedule
            .iter()
            .map(|(_, observation)| observation.clone())
            .collect::<Vec<_>>();
        // The encoded payload and compact ancestry identities are sufficient;
        // release the full-block vector before materializing witness inputs.
        drop(block_rlps);
        let witness_started = std::time::Instant::now();
        let (block_witnesses, observed_outcomes) = match self.inner.witness_source.as_ref() {
            // Remote-prover mode. Intermediate blocks `[from..sync)` are committed
            // (served by the witness store); a freshly-built endpoint isn't, so
            // capture it here from the in-memory block.
            Some(src) => {
                // Witness generation is a CPU-heavy trie walk / re-exec. Run it on
                // the blocking pool so it can't stall async worker threads on the
                // settlement path. (Store hits are cheap; the rare store miss and
                // the endpoint capture are the heavy parts.)
                let src = Arc::clone(src);
                let l2_provider = Arc::clone(
                    &self
                        .inner
                        .rollups
                        .get(&rollup_id)
                        .ok_or_else(|| format!("unknown rollup_id {rollup_id}"))?
                        .l2_provider,
                );
                let evm_config = self.inner.evm_config.clone();
                let terminal_block = sync_block.cloned();
                tokio::task::spawn_blocking(
                    move || -> Result<(Vec<BlockWitness>, Vec<ObservedOutcome>), String> {
                        let mut ws = witness_ids.into_iter()
                            .map(|(number, hash)| src.block_witness(number, hash))
                            .collect::<Result<Vec<_>, String>>()
                            .map_err(|e| format!("witness_source: {e}"))?;
                        match terminal_block {
                            // Just-built: nothing can serve an uncommitted block, and
                            // its parent state is hot, so re-execute in-memory.
                            Some(block) => {
                                let (witness, outcomes) = block_witness_with_observations(
                                    l2_provider.as_ref(),
                                    &evm_config,
                                    &block,
                                    ExecutionWitnessMode::Legacy,
                                    &terminal_observations,
                                )
                                .map_err(|e| {
                                    format!(
                                        "terminal-block witness (block {}): {e}",
                                        block.header().number()
                                    )
                                })?;
                                ws.push(witness);
                                return Ok((ws, outcomes));
                            }
                            // Committed (historical chunk): from the store — an
                            // in-memory capture needs a possibly-pruned parent state.
                            None => {
                                if !terminal_observations.is_empty() {
                                    let error =
                                        "historical terminal block unexpectedly carries observations";
                                    return Err(error.to_owned());
                                }
                                ws.push(
                                    src.block_witness(sync_block_number, sync_block_hash)
                                        .map_err(|e| format!("witness_source: {e}"))?,
                                );
                            }
                        }
                        Ok((ws, Vec::new()))
                    },
                )
                .await
                .map_err(|e| format!("witness spawn_blocking join: {e}"))??
            }
            // Tests may use a lightweight prover without a witness source, but
            // local observation validation still applies to every claimed result.
            None if terminal_observations.is_empty() => (Vec::new(), Vec::new()),
            None => {
                let l2_provider = Arc::clone(
                    &self
                        .inner
                        .rollups
                        .get(&rollup_id)
                        .ok_or_else(|| format!("unknown rollup_id {rollup_id}"))?
                        .l2_provider,
                );
                let evm_config = self.inner.evm_config.clone();
                let terminal_block = sync_block.cloned().ok_or_else(|| {
                    "lookup/static observations require an in-memory terminal block".to_owned()
                })?;
                let outcomes = tokio::task::spawn_blocking(move || {
                    block_observation_outcomes(
                        l2_provider.as_ref(),
                        &evm_config,
                        &terminal_block,
                        &terminal_observations,
                    )
                    .map_err(|error| {
                        format!(
                            "terminal-block observations (block {}): {error}",
                            terminal_block.header().number()
                        )
                    })
                })
                .await
                .map_err(|error| format!("observation spawn_blocking join: {error}"))??;
                (Vec::new(), outcomes)
            }
        };
        let witness_elapsed = witness_started.elapsed();
        if let Some(mismatch) =
            first_observation_mismatch(&observation_schedule, &observed_outcomes)?
        {
            return Err(PreparePostBatchError::ObservationMismatch {
                composition_index: mismatch.composition_index,
                observation_index: mismatch.observation_index,
                expected_success: mismatch.expected_success,
                observed_success: mismatch.observed_success,
                expected_output_hash: mismatch.expected_output_hash,
                observed_output_hash: mismatch.observed_output_hash,
            });
        }
        let parent_provenance = if let Some(source) = &self.inner.witness_source {
            // Correlations are complete only after reconciliation has scanned
            // through the exact L1 parent used by this proof target.
            if policy_anchor.through
                != Some(eez_l1::L1BlockRef {
                    number: l1_posting.parent_number,
                    hash: l1_posting.parent_hash,
                })
            {
                return Err("candidate provenance is waiting for the canonical L1 scan".into());
            }
            let mut live_slot = None;
            if anchor_header.number != 0 {
                let schedule = rollup
                    .l1_head
                    .derivation_schedule()
                    .map_err(|e| e.to_string())?;
                let (mut low, mut high) = (
                    schedule.config().l1_reference.number,
                    l1_posting.parent_number,
                );
                while low <= high {
                    let middle = low + (high - low) / 2;
                    let header = provider
                        .headers
                        .header_by_number(middle)
                        .map_err(|e| e.to_string())?
                        .ok_or("canonical L1 slot header missing")?;
                    match header.timestamp.cmp(&anchor_header.timestamp) {
                        std::cmp::Ordering::Equal => {
                            live_slot = Some((middle, header.hash_slow()));
                            break;
                        }
                        std::cmp::Ordering::Less => {
                            let Some(next) = middle.checked_add(1) else {
                                break;
                            };
                            low = next;
                        }
                        std::cmp::Ordering::Greater => {
                            let Some(previous) = middle.checked_sub(1) else {
                                break;
                            };
                            high = previous;
                        }
                    }
                }
            }
            let source = Arc::clone(source);
            let parent = anchor_header.clone().unseal();
            tokio::task::spawn_blocking(move || source.parent_provenance(parent, live_slot))
                .await
                .map_err(|e| format!("parent provenance worker: {e}"))??
        } else {
            // Lightweight in-process test provers do not attest remotely.
            eez_protocol::l2::ParentProvenance {
                parent: anchor_header.clone().unseal(),
                accepted_l1_number: 0,
                live_slot_endpoints: Vec::new(),
            }
        };
        let proving_ctx = ProvingContext {
            parent_provenance,
            rollup_id,
            from_block: from,
            to_block: sync_block_number,
            batch: batch.clone(),
            blobs: transaction_blobs.clone(),
            blocks: block_witnesses.into(),
            l1_posting,
        };
        if let Some(attempt) = admission {
            attempt.prepared(rollup_id, semantic_transactions.len());
        }
        if semantic_transactions.is_empty() {
            let elapsed = preparation_started.elapsed();
            event!(name: "eez.composer.settlement.preparation_time", Level::INFO,
                event_name = "eez.composer.settlement.preparation_time", rollup_id,
                preparation_ms = elapsed.as_millis() as u64, blob_count,
                "measured anchor-only encoding, KZG and witness preparation before proving");
        }
        let preparation_elapsed_ms = preparation_started.elapsed().as_secs_f64() * 1000.0;
        let proof_gate_ms = crate::prover_retry::unix_time_millis();
        let workload = crate::prover_retry::Workload {
            groups: semantic_transactions.len(),
            blobs: proving_ctx.blobs.len(),
            witnesses: proving_ctx.blocks.len(),
        };
        let post_reserve = admission.map_or(crate::prover_retry::POST_PROOF_RESERVE, |attempt| {
            attempt.timings.post_reserve(workload)
        });
        let proof_cutoff_ms = if deadline.is_some() {
            None
        } else {
            crate::prover_retry::proof_start_cutoff_ms(self.inner.emission.timing, bundle_target)
        };
        let mut preparation_timing = crate::posting_events::PreparationTiming {
            preparation_started: preparation_started_ms,
            proof_requested: proof_cutoff_ms
                .is_none_or(|cutoff| proof_gate_ms <= cutoff)
                .then_some(proof_gate_ms),
            proof_received: None,
            proof_received_at: None,
            candidate,
            deadline,
            workload,
            timings: admission.map(|a| Arc::clone(&a.timings)),
        };
        if let Some(candidate) = &preparation_timing.candidate {
            let prediction = admission.map(|attempt| {
                attempt
                    .timings
                    .proof_prediction(self.inner.emission.timing.proof_time(), workload)
            });
            candidate.emit(
                "proof_requested",
                serde_json::json!({"deadline_ms":deadline.map(|d|d.unix_ms),
                "remaining_ms":deadline.map(|d|d.remaining().as_secs_f64()*1000.0),
                "post_reserve_ms":post_reserve.as_secs_f64()*1000.0,
                "proof_estimate_ms":prediction.as_ref().map(|p|p.duration.as_secs_f64()*1000.0),
                "proof_estimate_basis":prediction.as_ref().map(|p|p.basis),
                "nominal_preparation_ms":self.inner.emission.timing.preparation_time().as_millis(),
                "preparation_ms":proof_gate_ms.saturating_sub(preparation_started_ms),
                "blob_preparation_ms":blob_preparation_elapsed.as_secs_f64()*1000.0,
                "witness_preparation_ms":witness_elapsed.as_secs_f64()*1000.0,
                "groups":workload.groups,"blobs":workload.blobs,"witnesses":workload.witnesses}),
            );
            candidate.duration(
                "preparation",
                std::time::Duration::from_millis(
                    proof_gate_ms.saturating_sub(preparation_started_ms),
                ),
            );
        }
        let proof_started = std::time::Instant::now();
        let proof_result = if let Some(deadline) = deadline {
            crate::prover_retry::prove_with_shared_deadline(
                self.inner.prover.as_ref(),
                proving_ctx,
                self.inner.emission.timing,
                bundle_target,
                deadline,
                post_reserve,
            )
            .await
        } else {
            prove_with_retry_at(
                self.inner.prover.as_ref(),
                proving_ctx,
                self.inner.emission.timing,
                bundle_target,
                proof_gate_ms,
            )
            .await
        };
        let proof = match proof_result {
            Ok(proof) => proof,
            Err(error) => {
                let reason = if proof_cutoff_ms.is_some_and(|cutoff| proof_gate_ms > cutoff) {
                    "preparation_deadline"
                } else {
                    match &error {
                        eez_prover::ProverError::Retryable { kind, .. } => match kind {
                            eez_prover::RetryableProverError::DeadlineExceeded => "prover_deadline",
                            eez_prover::RetryableProverError::Unavailable => "prover_unavailable",
                            eez_prover::RetryableProverError::Aborted => "prover_aborted",
                        },
                        eez_prover::ProverError::Actionable { .. } => "prover_rejected_candidate",
                        eez_prover::ProverError::CacheMiss { .. } => "prover_cache_miss",
                        eez_prover::ProverError::Backend(_) => "prover_backend",
                    }
                };
                if let Some(attempt) = admission
                    && deadline.is_some_and(|d| d.remaining() <= post_reserve)
                {
                    attempt.window_exhausted();
                }
                if let Some(candidate) = &preparation_timing.candidate {
                    candidate.emit("proof_failed", serde_json::json!({"reason":reason,"proof_ms":proof_started.elapsed().as_secs_f64()*1000.0}));
                }
                let mut detail = preparation_timing.detail();
                detail["reason"] = serde_json::json!(reason);
                detail["preparation_ms"] = serde_json::json!(preparation_elapsed_ms);
                detail["proof_gate_ms"] = serde_json::json!(proof_gate_ms);
                detail["proof_start_cutoff_ms"] = serde_json::json!(proof_cutoff_ms);
                crate::posting_events::PostingEvents::new(
                    ctx.l1_chain_id,
                    self.inner.posting.registry,
                    rollup_id,
                    0,
                    sync_block_hash,
                    sync_block_number,
                    None,
                )
                .with_target(bundle_target)
                .preparation_failed(detail);
                let Some(failure) = error.actionable_failure() else {
                    return Err(PreparePostBatchError::Prover(error));
                };
                if let Err(validation_error) =
                    validate_actionable_prover_failure(failure, &batch, sync_block)
                {
                    event!(
                        name: "eez.composer.prover.actionable_invalid",
                        Level::ERROR,
                        rollup_id,
                        failure = %failure,
                        error = %validation_error,
                        "prover supplied actionable failure details that do not match the current request; treating the rejection as opaque",
                    );
                    return Err(PreparePostBatchError::Prover(error));
                }
                return Err(PreparePostBatchError::Actionable(failure));
            }
        };
        preparation_timing.proof_received = Some(crate::prover_retry::unix_time_millis());
        preparation_timing.proof_received_at = Some(std::time::Instant::now());
        if let Some(attempt) = admission {
            attempt
                .timings
                .observe_proof(workload, proof_started.elapsed());
        }
        if let Some(candidate) = &preparation_timing.candidate {
            candidate.duration("proof", proof_started.elapsed());
            candidate.emit(
                "proof_received",
                serde_json::json!({"proof_ms":proof_started.elapsed().as_secs_f64()*1000.0}),
            );
        }

        if !rollup.l1_head.generation_is(policy_anchor.generation) {
            return Err("canonical settlement evidence changed during proving".into());
        }
        batch.proofs = vec![proof];

        let calldata = postAndVerifyBatchCall {
            batch: batch.clone(),
        }
        .abi_encode();

        // Log both settlement anchors. `RootMismatch` means
        // `current_root` disagreed with L1's registered root at submission;
        // an over-budget `floor_gas` names the too-fat case.
        event!(
            name: "eez.composer.postbatch.anchors",
            Level::INFO,
            rollup_id,
            posted,
            from,
            sync_block_number,
            span,
            calldata_bytes = calldata.len(),
            floor_gas = calldata_floor_gas(&calldata),
            current_root = %pre_block_hash,
            claimed_final = %sync_block_hash,
            "postBatch commitments: current block hash at cursor, claimed final Sync block hash",
        );

        // Reuse the deployment identity validated during startup.
        let eez_address = self.inner.cross_chain.local.l1_entry.manager_address();

        let poster = ctx.l1_poster_signer.address();
        let poster_nonce = ctx
            .l1_provider
            .get_transaction_count(poster)
            .pending()
            .await
            .map_err(|error| format!("get_transaction_count({poster}): {error}"))?;
        // A timeout does not cancel the original postBatch. Reuse its nonce
        // for the replacement so both cannot execute on the same L1 branch,
        // even if a relay has exposed the first transaction to the pending pool.
        let replacement = rollup.optimistic.reconciliation_attempts().into_iter().find(|attempt| {
            attempt.replacement_released && attempt.dispatched_target.is_some_and(|(block, timestamp)| {
                matches!(bundle_target, BundleTarget::Exact { block: next, timestamp: next_time }
                    if next == block && timestamp.checked_add(self.inner.emission.timing.l1_block_time().as_secs()) == Some(next_time))
            })
        });
        let poster_nonce = if let Some(previous) = replacement {
            use alloy_consensus::{Signed, Transaction, TxEip4844WithSidecar};
            use alloy_eips::{Decodable2718, eip7594::BlobTransactionSidecarVariant};
            let mut input = previous.post_batch.as_ref();
            let signed =
                Signed::<TxEip4844WithSidecar<BlobTransactionSidecarVariant>>::decode_2718(
                    &mut input,
                )
                .map_err(|error| format!("replacement poster envelope: {error}"))?;
            if !input.is_empty() {
                return Err("trailing replacement poster bytes".into());
            }
            signed.tx().nonce()
        } else {
            poster_nonce
        };

        let posting = &self.inner.posting;
        let quote = posting
            .quote(bundle_target, false)
            .await
            .map_err(|error| error.to_string())?;
        if quote.quote.sample
            != (eez_l1::L1BlockRef {
                number: l1_posting.parent_number,
                hash: l1_posting.parent_hash,
            })
        {
            return Err("canonical L1 parent changed during proving".into());
        }
        let gas = posting
            .fees
            .declared_gas(projected_gas, quote.gas_budget)
            .map_err(|error| error.to_string())?;
        let transaction = eez_blob_codec::prepare_blob_transaction_with_sidecar(
            eez_blob_codec::BlobTransactionParams {
                chain_id: ctx.l1_chain_id,
                nonce: poster_nonce,
                gas_limit: gas,
                max_fee_per_gas: quote.quote.max_fee_per_gas,
                max_priority_fee_per_gas: quote.quote.max_priority_fee_per_gas,
                max_fee_per_blob_gas: quote.quote.max_fee_per_blob_gas,
                to: eez_address,
                value: U256::ZERO,
                access_list: Default::default(),
                input: calldata.into(),
            },
            prepared_blob_sidecar,
        )
        .map_err(|error| error.to_string())?;
        let raw = eez_blob_codec::sign_blob_transaction(transaction, &ctx.l1_poster_signer)
            .map_err(|error| error.to_string())?
            .pooled_transaction;
        Ok((raw, preparation_timing))
    }
}

/// Refuse to settle a batch carrying any `destinationRollupId` / `sourceRollupId`
/// that isn't this rollup's registry id — a wiring bug (e.g. an outbound entry
/// whose `dest` stayed at the call's MAINNET(0) target) that L1 would misattribute
/// and that folds into the `publicInputsHash`. Guards the outbound `dest=rid` rewrite.
fn ensure_batch_registry_native(
    batch: &eez_protocol::EvmBatch,
    expected_rollup_id: u64,
) -> Result<(), String> {
    for (i, entry) in batch.entries.iter().enumerate() {
        if entry.destinationRollupId != expected_rollup_id {
            return Err(format!(
                "entry[{i}].destinationRollupId = {} is not the configured registry id {expected_rollup_id} — \
                 a non-registry id reached the settlement batch (composition must be registry-native)",
                entry.destinationRollupId,
            ));
        }
        for (j, call) in entry.l2ToL1Calls.iter().enumerate() {
            if call.sourceRollupId != expected_rollup_id {
                return Err(format!(
                    "entry[{i}].l2ToL1Calls[{j}].sourceRollupId = {} is not the configured registry id {expected_rollup_id}",
                    call.sourceRollupId,
                ));
            }
        }
    }
    for (i, entry) in batch.staticEntries.iter().enumerate() {
        if entry.destinationRollupId != expected_rollup_id {
            return Err(format!(
                "staticEntries[{i}].destinationRollupId = {} is not the configured registry id {expected_rollup_id}",
                entry.destinationRollupId,
            ));
        }
        for (j, call) in entry.l2ToL1Calls.iter().enumerate() {
            if call.sourceRollupId != expected_rollup_id {
                return Err(format!(
                    "staticEntries[{i}].l2ToL1Calls[{j}].sourceRollupId = {} is not the configured registry id {expected_rollup_id}",
                    call.sourceRollupId,
                ));
            }
        }
    }
    Ok(())
}

/// A canonically proven skipped target does not measure submission capacity,
/// including when replacement attempts consumed some sources. Produced targets
/// retain the existing conservative learning for incomplete source outcomes.
fn submission_backoff_required(
    outcomes: &[crate::optimistic::SourceOutcome],
    skipped_slot: bool,
) -> bool {
    use crate::optimistic::{SourceOutcome, TerminalReason};
    let completed = |outcome: &SourceOutcome| {
        matches!(
            outcome,
            SourceOutcome::Terminal(TerminalReason::Committed | TerminalReason::SourceCompleted)
        )
    };
    // Consecutive canonical headers proved that this target was never built.
    // Source completion elsewhere (including a replacement) is not a capacity
    // observation for this candidate. Its dispositions remain fully reconciled;
    // a partially applied replacement is evaluated under its own produced target.
    !skipped_slot && outcomes.iter().any(|outcome| !completed(outcome))
}

/// Mutable consumption belongs to the most recent batch for this rollup in
/// the same L1 block. Queue indices are reset by every later postBatch.
fn source_uses_batch(
    receipt: &eez_l1::submitter::CanonicalSourceReceipt,
    batch: &eez_l1::ReconciledBatchSettlement,
    snapshot: &eez_l1::l1_canonical_head::SettlementSnapshot,
) -> bool {
    source_follows_postbatch(
        receipt.block,
        receipt.transaction_index,
        batch.l1_block,
        batch.tx_index,
        snapshot,
    )
}

/// Shared canonical transaction-position attribution for ownership and costs.
fn source_follows_postbatch(
    source_block: eez_l1::L1BlockRef,
    source_index: u64,
    post_block: eez_l1::L1BlockRef,
    post_index: u64,
    snapshot: &eez_l1::l1_canonical_head::SettlementSnapshot,
) -> bool {
    source_block == post_block
        && source_index > post_index
        && !snapshot.settlements.values().any(|next| {
            next.l1_block == post_block
                && next.tx_index > post_index
                && next.tx_index < source_index
        })
}

/// Verify that authoritative replay coordinates belong to this exact local
/// proposal before using them to classify its signed users.
fn validate_attempt_evidence(
    attempt: &crate::optimistic::AttemptRecord,
    evidence: &eez_l1::ReconciledBatchSettlement,
) -> Result<(), String> {
    use alloy_consensus::transaction::TxHashRef as _;
    use alloy_consensus::{Signed, Transaction, TxEip4844WithSidecar};
    use alloy_eips::{Decodable2718, eip7594::BlobTransactionSidecarVariant};
    let mut raw = attempt.post_batch.as_ref();
    let signed =
        Signed::<TxEip4844WithSidecar<BlobTransactionSidecarVariant>>::decode_2718(&mut raw)
            .map_err(|error| format!("recorded postBatch envelope: {error}"))?;
    if !raw.is_empty()
        || *signed.tx_hash() != attempt.id.post_batch_hash
        || alloy_primitives::keccak256(signed.tx().input()) != evidence.calldata_hash
    {
        return Err("reconciliation evidence does not match the signed postBatch".into());
    }
    let effects = &attempt.manifest.effects;
    let applied = &evidence.applied_effects;
    if applied.start > applied.end || applied.end > effects.len() {
        return Err("reconciliation slice exceeds the user manifest".into());
    }
    if !applied.is_empty() {
        let endpoint = evidence
            .endpoint
            .ok_or("producing settlement has no endpoint")?;
        if endpoint.number != attempt.sync_height || endpoint.hash != effects[applied.end - 1] {
            return Err("reconciliation endpoint differs from the manifest checkpoint".into());
        }
    }
    for user in &attempt.manifest.users {
        if let Some(group) = user.outbound_group {
            let selected = applied.start <= user.effects.start && user.effects.end <= applied.end;
            if selected != evidence.selection.outbound_groups.contains(&group) {
                return Err(
                    "reconciliation splits or misidentifies a signed outbound group".into(),
                );
            }
        }
    }
    Ok(())
}

async fn observe_bundle_outcome(
    rollup_id: u64,
    sync_height: u64,
    mut attempt_id: AttemptId,
    mut bundle: Vec<Bytes>,
    optimistic: Arc<OptimisticallyIncluded>,
    submitter: Submitter,
    target: BundleTarget,
    posting: Arc<posting::PostingRuntime>,
    semantics: Vec<eez_protocol::blob_semantics::SemanticTransaction>,
    events: Option<Arc<crate::posting_events::PostingEvents>>,
    preparation_timing: crate::posting_events::PreparationTiming,
) {
    use crate::optimistic::SubmissionObservation;
    let measurements = Arc::new(crate::submission_metrics::SubmissionMetrics::new(rollup_id));
    measurements.prepared();
    if let Some(events) = &events {
        events.emit(
            "prepared",
            attempt_id.post_batch_hash,
            preparation_timing.detail(),
        );
    }
    let finalization_started = std::time::Instant::now();
    let signing_elapsed = preparation_timing
        .proof_received_at
        .map_or(std::time::Duration::ZERO, |at| at.elapsed());
    let preparation = async {
        if preparation_timing
            .deadline
            .is_some_and(|d| d.remaining() <= crate::prover_retry::DELIVERY_RESERVE)
        {
            return Err((
                "pipeline_deadline",
                "shared window exhausted before finalization".into(),
            ));
        }
        if let Some(reason) = submitter
            .policy_submission_failure(target)
            .await
            .map_err(|error| ("target_unavailable", error.to_string()))?
        {
            return Err((
                reason.as_str(),
                "target parent or submission window prevents final pricing".to_owned(),
            ));
        }
        let attempt = optimistic
            .pending_attempt()
            .filter(|attempt| attempt.id == attempt_id)
            .ok_or_else(|| {
                (
                    "ownership_changed",
                    "prepared attempt no longer owns this submission".to_owned(),
                )
            })?;
        let (prepared, quote, execution) = posting
            .finalize(
                bundle.clone(),
                target,
                rollup_id,
                attempt.manifest.sync_hash,
                &semantics,
            )
            .await
            .map_err(|error| ("final_pricing_failed", error.to_string()))?;
        if preparation_timing
            .deadline
            .is_some_and(|d| d.remaining() <= crate::prover_retry::DELIVERY_RESERVE)
        {
            return Err((
                "pipeline_deadline",
                "shared finalization window exhausted".into(),
            ));
        }
        let hash = post_batch_blob_transaction_hash(&prepared[0])
            .map_err(|error| ("invalid_envelope", error))?;
        let id = optimistic
            .replace_prepared_post_batch(attempt_id, hash, prepared[0].clone())
            .map_err(|error| ("ledger_update_failed", error))?;
        Ok::<_, (&str, String)>((prepared, quote, id, execution))
    };
    let preparation = if let Some(deadline) = preparation_timing.deadline {
        tokio::time::timeout(
            deadline
                .remaining()
                .saturating_sub(crate::prover_retry::DELIVERY_RESERVE),
            preparation,
        )
        .await
        .unwrap_or_else(|_| {
            Err((
                "pipeline_deadline",
                "shared finalization window exhausted".into(),
            ))
        })
    } else {
        preparation.await
    };
    let preflight_elapsed = finalization_started.elapsed();
    let finalization_elapsed = signing_elapsed.saturating_add(preflight_elapsed);
    if let Some(candidate) = &preparation_timing.candidate {
        candidate.duration("finalization", finalization_elapsed);
    }
    let fee_quote = match preparation {
        Ok((prepared, quote, id, execution)) => {
            if let Some(timings) = &preparation_timing.timings {
                timings.observe_finalization(preparation_timing.workload, finalization_elapsed);
            }
            let detail = serde_json::json!({"candidate_id":preparation_timing.candidate.as_ref().map(|c|&c.id),
                "preflight_gas_used":execution.gas_used.get(..1),"preflight_gas_required":execution.gas_required.get(..1),
                "signed_gas_limits":execution.gas_limits.get(..1),"finalization_ms":finalization_elapsed.as_secs_f64()*1000.0,
                "signing_ms":signing_elapsed.as_secs_f64()*1000.0,"preflight_ms":preflight_elapsed.as_secs_f64()*1000.0});
            if let Some(candidate) = &preparation_timing.candidate {
                let mut detail = detail.clone();
                detail["transaction_hash"] = serde_json::json!(id.post_batch_hash);
                candidate.emit("preflight", detail);
                // One bounded event per companion; large bundles must not turn
                // the posting event into an oversized uncollectable log line.
                for (index, hash) in execution.transaction_hashes.iter().enumerate().skip(1) {
                    candidate.emit(
                        "source_preflight",
                        serde_json::json!({"source_hash":hash,
                        "preflight_used_gas":execution.gas_used.get(index),
                        "preflight_required_gas":execution.gas_required.get(index),
                        "signed_limit_gas":execution.gas_limits.get(index)}),
                    );
                }
            }
            bundle = prepared;
            attempt_id = id;
            if let Some(events) = &events {
                events.emit("repriced", id.post_batch_hash, detail);
            }
            quote
        }
        Err((reason, error)) => {
            if reason == "pipeline_deadline"
                && let Some(timings) = &preparation_timing.timings
            {
                // A censored sample is a lower bound on finishing work. Preserve
                // it with headroom so the next selection reserves more time.
                timings.observe_finalization(preparation_timing.workload, finalization_elapsed);
            }
            if let Some(candidate) = &preparation_timing.candidate {
                candidate.emit("preflight_failed", serde_json::json!({"reason":reason}));
            }
            measurements.preflight_failed();
            if let Some(events) = &events {
                events.emit(
                    "preflight_failed",
                    attempt_id.post_batch_hash,
                    serde_json::json!({"reason":reason}),
                );
            }
            event!(name: "eez.composer.postbatch.preflight_failed", Level::WARN,
                event_name = "eez.composer.postbatch.preflight_failed", rollup_id, %error,
                "unsent bundle deferred; due work retained without a user attempt penalty");
            if let BundleTarget::Exact { block, .. } = target {
                let _ = optimistic.observe(
                    attempt_id,
                    SubmissionObservation::Excluded {
                        block,
                        charge_attempt: false,
                    },
                );
            }
            return;
        }
    };
    let recorder = Arc::clone(&optimistic);
    let dispatch_measurements = Arc::clone(&measurements);
    let dispatch_events = events.clone();
    let dispatch_deadline = preparation_timing.deadline;
    let dispatch_candidate = preparation_timing.candidate.clone();
    let record_dispatch = move |block, timestamp| {
        if dispatch_deadline.is_some_and(|d| d.remaining().is_zero()) {
            return Err(eez_l1::L1Error::Submission(
                "shared submission window exhausted".into(),
            ));
        }
        if let Some(candidate) = &dispatch_candidate {
            candidate.emit(
                "dispatch",
                serde_json::json!({"transaction_hash":attempt_id.post_batch_hash,
                "remaining_ms":dispatch_deadline.map(|d|d.remaining().as_secs_f64()*1000.0)}),
            );
        }
        match recorder.observe(
            attempt_id,
            SubmissionObservation::Dispatched { block, timestamp },
        ) {
            Ok(true) => {
                dispatch_measurements.dispatch(block, timestamp);
                if let Some(events) = &dispatch_events {
                    events.dispatch(attempt_id.post_batch_hash, block, timestamp);
                }
                Ok(())
            }
            Ok(false) => Err(eez_l1::L1Error::Submission(
                "attempt was superseded before dispatch".into(),
            )),
            Err(error) => Err(eez_l1::L1Error::Submission(format!(
                "cannot record dispatch: {error}"
            ))),
        }
    };
    let outcome = if let Some(deadline) = preparation_timing.deadline {
        let candidate = preparation_timing.candidate.clone();
        submitter
            .send_bundle_quoted_before(
                &bundle,
                target,
                fee_quote,
                record_dispatch,
                deadline.instant(),
                move |delivery| {
                    if let Some(candidate) = candidate {
                        // Exact targets do not carry the transport's optional
                        // derived-parent guard. The quote was checked against
                        // the canonical head immediately before dispatch.
                        let expected_parent_hash = delivery.expected_parent_hash.or_else(|| {
                            (fee_quote.sample.number.checked_add(1) == Some(delivery.target_block))
                                .then_some(fee_quote.sample.hash)
                        });
                        candidate.duration("delivery", delivery.elapsed);
                        candidate.emit(
                            "delivery",
                            serde_json::json!({"transaction_hash":attempt_id.post_batch_hash,
                        "delivery_ms":delivery.elapsed.as_secs_f64()*1000.0,
                        "acknowledged":delivery.acknowledged,
                        "relay_bundle_hash":delivery.relay_bundle_hash,
                        "ack_remaining_ms":delivery.remaining.as_secs_f64()*1000.0,
                        "target_block":delivery.target_block,
                        "target_timestamp":delivery.target_timestamp,
                        "expected_parent_hash":expected_parent_hash,
                        "transaction_count":delivery.transaction_count,
                        // Per-relay outcomes of this single delivery attempt.
                        "builders":delivery.builders.iter()
                            .map(eez_l1::BuilderDelivery::json).collect::<Vec<_>>()}),
                        );
                    }
                },
            )
            .await
    } else {
        submitter
            .send_bundle_quoted(&bundle, target, fee_quote, record_dispatch)
            .await
    };
    measurements.outcome(outcome.as_ref().ok());
    if let Some(events) = &events {
        let (stage, detail) = match &outcome {
            Ok(SendOutcome::Included {
                l1_block,
                l1_block_hash,
                ..
            }) => (
                "included",
                serde_json::json!({"l1_block":l1_block,"l1_block_hash":l1_block_hash}),
            ),
            Ok(SendOutcome::Dropped {
                target_block,
                reason,
                ..
            }) => {
                let reason = match reason {
                    eez_l1::BundleDropReason::Excluded => "excluded",
                    eez_l1::BundleDropReason::SlotSkipped => "slot_skipped",
                    eez_l1::BundleDropReason::SubmissionWindowMissed => "submission_window_missed",
                    eez_l1::BundleDropReason::TargetChanged => "target_changed",
                };
                (
                    "excluded",
                    serde_json::json!({"target_block":target_block,"reason":reason}),
                )
            }
            Err(_) => ("uncertain", serde_json::json!({"reason":"observer_error"})),
        };
        events.emit(stage, attempt_id.post_batch_hash, detail);
    }

    let observation = match &outcome {
        Ok(SendOutcome::Included {
            l1_block,
            l1_block_hash,
            ..
        }) => SubmissionObservation::Included {
            block: eez_l1::L1BlockRef {
                number: *l1_block,
                hash: *l1_block_hash,
            },
        },
        Ok(SendOutcome::Dropped {
            target_block,
            reason,
            ..
        }) => SubmissionObservation::Excluded {
            block: *target_block,
            charge_attempt: reason.counts_toward_attempt_limit(),
        },
        Err(_) => SubmissionObservation::Uncertain,
    };
    event!(name: "eez.composer.bundle.observed", Level::INFO,
        event_name = "eez.composer.bundle.observed",
        rollup_id, sync_height, attempt = ?attempt_id, outcome = ?outcome,
        "bundle observed; canonical derivation and source receipts retain ownership authority");
    if let Err(error) = optimistic.observe(attempt_id, observation) {
        event!(Level::ERROR, rollup_id, %error, "cannot record observation; retaining attempt");
    }
}

/// Return the canonical Ethereum hash for a type-3 transaction-pool wrapper
/// carrying the mandatory postBatch blob sidecar.
fn post_batch_blob_transaction_hash(raw: &[u8]) -> Result<alloy_primitives::TxHash, String> {
    eez_blob_codec::pooled_blob_transaction_hash(raw)
        .map_err(|error| format!("decode signed postBatch blob transaction: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::TxHash;
    use alloy_sol_types::SolValue as _;

    fn gas_action(
        source: u64,
        target: u64,
        parent_index: Option<usize>,
        gas_used: u64,
        success: bool,
    ) -> eez_protocol::ExecutedAction {
        eez_protocol::ExecutedAction {
            parent_index,
            call_mode: eez_protocol::CallMode::Mutable,
            source_rollup_id: eez_protocol::RollupId(source),
            target_rollup_id: eez_protocol::RollupId(target),
            source_address: Address::repeat_byte(1),
            target_address: Address::repeat_byte(2),
            data: Bytes::new(),
            value: U256::ZERO,
            outcome: eez_protocol::ExecutionOutcome::Resolved {
                return_data: Vec::new(),
                gas_used,
                success,
            },
            revert_span: None,
        }
    }

    #[test]
    fn l1_admission_does_not_charge_l2_callback_execution() {
        // L2 -> L1 -> L2 -> L1. Both L1 frames execute in postBatch;
        // the callback's work belongs to the Sync block, however expensive.
        let mut actions = vec![
            gas_action(1, 0, None, 80_000, true),
            gas_action(0, 1, Some(0), 2_000_000, true),
            gas_action(1, 0, Some(1), 90_000, true),
        ];
        let entry = outbound_entry();
        let cost = projected_tx_l1_gas(std::slice::from_ref(&entry), l1_target_gas(&actions));
        assert_eq!(cost.target_gas, 170_000);
        let cap = projected_postbatch_gas(1, 0, 0)
            + gas::framing::base(64)
            + cost.total()
            + cost.framing_gas;
        assert!(PostBatchGasBudget::new(cap).try_accept(cost));

        actions[1] = gas_action(0, 1, Some(0), u64::MAX, true);
        let same = projected_tx_l1_gas(std::slice::from_ref(&entry), l1_target_gas(&actions));
        assert_eq!(same.total(), cost.total());
        assert!(PostBatchGasBudget::new(cap).try_accept(same));

        actions[2] = gas_action(1, 0, Some(1), 90_001, true);
        let larger = projected_tx_l1_gas(&[entry], l1_target_gas(&actions));
        assert!(!PostBatchGasBudget::new(cap).try_accept(larger));
    }

    #[test]
    fn reverted_l1_frames_still_consume_gas_and_overflow_cannot_wrap() {
        let mut actions = vec![
            gas_action(1, 0, None, 60_000, false),
            gas_action(0, 1, Some(0), 8_000_000, true),
            gas_action(1, 0, Some(1), 70_000, true),
            gas_action(1, 0, None, 80_000, true),
        ];
        actions[0].revert_span = Some(3);
        assert_eq!(l1_target_gas(&actions), 210_000);
        actions[0] = gas_action(1, 0, None, u64::MAX, false);
        assert_eq!(l1_target_gas(&actions), u64::MAX);
        assert_eq!(l1_target_gas(&[]), 0);
    }

    #[test]
    fn proven_skipped_slot_retains_capacity_even_when_sources_complete_elsewhere() {
        use crate::optimistic::{SourceOutcome, TerminalReason};
        let retry = SourceOutcome::Retryable { source_nonce: 4 };
        assert!(!submission_backoff_required(&[retry, retry], true));
        assert!(submission_backoff_required(&[retry, retry], false));
        for reason in [
            TerminalReason::Committed,
            TerminalReason::SourceReverted,
            TerminalReason::SourceCompletedWithMissingEffects,
            TerminalReason::SourceCompleted,
            TerminalReason::SourceNonceSpent,
            TerminalReason::AttemptsExhausted,
            TerminalReason::NonceGap,
            TerminalReason::PoolDiscarded,
        ] {
            assert!(
                !submission_backoff_required(&[retry, SourceOutcome::Terminal(reason)], true),
                "a skipped target cannot measure capacity: {reason:?}"
            );
        }
    }

    #[test]
    fn completed_or_empty_attempt_never_trains_failure_backoff() {
        use crate::optimistic::{SourceOutcome, TerminalReason};
        for skipped_slot in [false, true] {
            assert!(!submission_backoff_required(&[], skipped_slot));
            assert!(!submission_backoff_required(
                &[
                    SourceOutcome::Terminal(TerminalReason::Committed),
                    SourceOutcome::Terminal(TerminalReason::SourceCompleted),
                ],
                skipped_slot,
            ));
            assert_eq!(
                submission_backoff_required(
                    &[SourceOutcome::Terminal(TerminalReason::SourceReverted)],
                    skipped_slot
                ),
                !skipped_slot
            );
            assert!(submission_backoff_required(
                &[
                    SourceOutcome::Retryable { source_nonce: 4 },
                    SourceOutcome::Terminal(TerminalReason::SourceCompletedWithMissingEffects)
                ],
                false
            ));
        }
    }

    #[test]
    fn minimum_capacity_keeps_static_only_work_eligible_at_a_full_l2_block() {
        use alloy_consensus::{SignableTransaction, TxEnvelope, TxLegacy};
        use alloy_eips::eip2718::Encodable2718;
        let mut source = held(Address::repeat_byte(1), Direction::Inbound, 0, 1);
        let raw = TxEnvelope::from(
            TxLegacy {
                gas_limit: 100_000,
                ..Default::default()
            }
            .into_signed(alloy_primitives::Signature::new(
                U256::from(1),
                U256::from(2),
                false,
            )),
        );
        source.raw_tx = Bytes::from(raw.encoded_2718());
        let candidate = crate::admission_events::Candidate::new(
            10200,
            Address::repeat_byte(9),
            1,
            B256::repeat_byte(8),
            BundleTarget::NextBlock,
            1,
            1,
            (1, None),
            Some(0),
            None,
        );
        let mut budget = PostBatchGasBudget::new(1_000_000);
        budget.entries = 1;
        budget.general = true;
        budget.cap = budget.full_estimate() + 5_000;
        let mut capacity = CapacityDeferrals::default();
        assert_eq!(
            capacity.minimum_check(&candidate, &source, budget, BUILDER_GAS_LIMIT),
            None
        );
        assert_eq!(capacity.minimum_check_count, 0);
        source.direction = Direction::Outbound;
        assert_eq!(
            capacity.minimum_check(&candidate, &source, budget, BUILDER_GAS_LIMIT),
            Some("l2_block_capacity")
        );
        assert_eq!(capacity.minimum_check_count, 1);
    }

    #[test]
    fn minimum_capacity_check_keeps_empty_and_fitting_candidates_eligible() {
        let minimum = gas::minimum_entry_gas(true);
        let mut budget = PostBatchGasBudget::new(1_000_000);
        // The first source must reach ordinary classification even if malformed
        // or individually unpostable. Capacity misses cannot evict it early.
        assert_eq!(budget.minimum_rejection(false, u64::MAX), None);
        budget.entries = 1;
        budget.projected = budget.cap - budget.framing_gas - minimum;
        assert_eq!(budget.minimum_rejection(false, u64::MAX), None);
        budget.projected += 1;
        assert_eq!(budget.minimum_rejection(false, 0), Some("l1_tx_gas_budget"));
    }

    #[test]
    fn minimum_capacity_check_preserves_signed_prefix_envelopes_and_smaller_work() {
        let mut budget = PostBatchGasBudget::new(16_777_216);
        budget.entries = 5;
        budget.transaction_cap = 16_777_216;
        budget.projected = 6_000_000;
        budget.companion_gas = 9_500_000;
        budget.companion_required_gas = 9_600_000;
        budget.block_cap = budget.full_estimate() + budget.companion_gas + 1_000_000 - 1;
        let before = budget;
        assert_eq!(
            budget.minimum_rejection(true, 1_000_000),
            Some("l1_bundle_gas_budget")
        );
        assert_eq!(budget.minimum_rejection(true, 100_000), None);
        assert_eq!(budget.projected, before.projected);
        assert_eq!(budget.companion_gas, before.companion_gas);
        assert_eq!(budget.companion_required_gas, before.companion_required_gas);
        budget.companion_gas = 0;
        budget.companion_required_gas = 16_000_000;
        assert_eq!(
            budget.minimum_rejection(true, 100_000),
            Some("l1_bundle_gas_budget")
        );
    }

    #[test]
    fn minimum_capacity_check_is_no_stricter_than_supported_entry_prices() {
        for inbound in [false, true] {
            let minimum = gas::minimum_entry_gas(!inbound);
            for entries in [1, 2, 126, 127, 128] {
                for remaining in [0, minimum - 1, minimum, minimum + 1, 1_000_000] {
                    let mut budget = PostBatchGasBudget::new(16_777_216);
                    budget.entries = entries;
                    budget.projected = budget.cap - remaining;
                    budget.entry_discount = entries * 100_000;
                    budget.block_cap = 17_000_000;
                    budget.companion_gas = 1_000_000;
                    for added in [1, 2, 4] {
                        for extra in [0, 21_000, 200_000] {
                            let cost = TxL1Gas {
                                estimate_reasons: Default::default(),
                                entries: added,
                                entry_discount: added * (POSTBATCH_ENTRY_GAS_PIN - minimum),
                                calldata_gas: extra,
                                target_gas: extra,
                                ..Default::default()
                            };
                            let mut next = budget;
                            let fits = next.try_accept_with_companion(
                                cost,
                                if inbound { 1_000_000 } else { 0 },
                                if inbound { 150_000 } else { 0 },
                            );
                            if fits {
                                assert_eq!(budget.minimum_rejection(inbound, 1_000_000), None);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn companion_admission_reserves_the_whole_group_without_spending_a_rejected_budget() {
        let mut budget = PostBatchGasBudget::new(10_000_000);
        budget.block_cap = 17_000_000;
        budget.transaction_cap = 16_777_216;
        let cost = TxL1Gas {
            estimate_reasons: Default::default(),
            entries: 1,
            entry_discount: 0,
            calldata_gas: 0,
            target_gas: 0,
            ..Default::default()
        };
        assert!(budget.try_accept_with_companion(cost, 8_000_000, 8_000_000));
        let accepted = budget;
        assert!(!budget.try_accept_with_companion(cost, 8_000_000, 8_000_000));
        assert_eq!(budget.projected, accepted.projected);
        assert_eq!(budget.companion_gas, accepted.companion_gas);
        assert!(budget.try_accept_with_companion(cost, 1_000_000, 1_000_000));
    }

    #[test]
    fn companion_transaction_rule_and_overflow_are_independent_of_postbatch_capacity() {
        let mut budget = PostBatchGasBudget::new(30_000_000);
        budget.block_cap = 60_000_000;
        budget.transaction_cap = 16_777_216;
        let cost = TxL1Gas {
            estimate_reasons: Default::default(),
            entries: 0,
            entry_discount: 0,
            calldata_gas: 0,
            target_gas: 0,
            ..Default::default()
        };
        assert!(!budget.try_accept_with_companion(cost, 16_777_217, 16_777_217));
        assert_eq!(budget.companion_gas, 0);
        budget.companion_gas = u64::MAX;
        assert!(!budget.try_accept_with_companion(cost, 1, 1));
        assert_eq!(budget.companion_gas, u64::MAX);
    }

    #[test]
    fn companion_admission_uses_estimated_consumption_and_keeps_declared_headroom() {
        let mut budget = PostBatchGasBudget::new(16_777_216);
        budget.block_cap = 17_000_000;
        budget.transaction_cap = 16_777_216;
        let cost = TxL1Gas {
            estimate_reasons: Default::default(),
            entries: 1,
            entry_discount: 0,
            calldata_gas: 5_000,
            target_gas: 0,
            ..Default::default()
        };
        for _ in 0..17 {
            assert!(budget.try_accept_with_companion(cost, 1_000_000, 160_000));
        }
        assert_eq!(budget.companion_gas, 17 * 160_000);
        assert_eq!(budget.companion_required_gas, 16 * 160_000 + 1_000_000);
        let accepted = budget;
        assert!(
            !budget.try_accept_with_companion(cost, 16_777_216, 21_000),
            "a cheap next transaction still needs its declared gas available"
        );
        assert_eq!(budget.projected, accepted.projected);
        assert_eq!(budget.companion_gas, accepted.companion_gas);
        assert_eq!(
            budget.companion_required_gas,
            accepted.companion_required_gas
        );
    }

    #[test]
    fn growing_postbatch_rechecks_every_prior_companions_required_headroom() {
        let mut budget = PostBatchGasBudget::new(16_777_216);
        budget.block_cap = 10_000_000;
        let free = TxL1Gas {
            estimate_reasons: Default::default(),
            entries: 0,
            entry_discount: 0,
            calldata_gas: 0,
            target_gas: 0,
            ..Default::default()
        };
        assert!(budget.try_accept_with_companion(free, 8_000_000, 21_000));
        assert!(budget.try_accept_with_companion(free, 100_000, 21_000));
        budget.block_cap = budget.full_estimate() + 8_000_000 + 3 * POSTBATCH_ENTRY_GAS_PIN - 1;
        let accepted = budget;
        assert!(!budget.try_accept(TxL1Gas { entries: 3, ..free }));
        assert_eq!(budget.projected, accepted.projected);
        assert_eq!(budget.companion_required_gas, 8_000_000);
        assert!(!budget.try_accept_with_companion(free, 1, 2));
    }

    #[test]
    fn source_consumption_is_scoped_to_its_same_block_batch_window() {
        use eez_l1::{L1BlockRef, ReconciledBatchSettlement, ReplaySelection};
        let block = L1BlockRef {
            number: 100,
            hash: B256::with_last_byte(1),
        };
        let earlier = ReconciledBatchSettlement {
            l1_block: block,
            tx_hash: B256::with_last_byte(2),
            tx_index: 1,
            calldata_hash: B256::ZERO,
            applied_effects: 0..1,
            anchor_applied: true,
            endpoint: None,
            selection: ReplaySelection::default(),
        };
        let mut later = earlier.clone();
        later.tx_hash = B256::with_last_byte(3);
        later.tx_index = 4;
        let mut snapshot = eez_l1::l1_canonical_head::SettlementSnapshot::default();
        snapshot
            .settlements
            .insert(earlier.tx_hash, earlier.clone());
        snapshot.settlements.insert(later.tx_hash, later.clone());
        let mut receipt = eez_l1::submitter::CanonicalSourceReceipt {
            cost: Err(eez_l1::posting_fees::PostingFeeError::Unavailable(
                "test receipt",
            )),
            block,
            transaction_index: 2,
            success: true,
            consumed: Vec::new(),
        };
        assert!(source_uses_batch(&receipt, &earlier, &snapshot));
        assert!(!source_uses_batch(&receipt, &later, &snapshot));
        receipt.transaction_index = 5;
        assert!(!source_uses_batch(&receipt, &earlier, &snapshot));
        assert!(source_uses_batch(&receipt, &later, &snapshot));
        receipt.block.hash = B256::with_last_byte(9);
        assert!(!source_uses_batch(&receipt, &later, &snapshot));
        receipt.block = block;
        receipt.block.number += 1;
        assert!(!source_uses_batch(&receipt, &later, &snapshot));
    }

    #[test]
    fn submission_identity_must_match_post_batch_signer() {
        let poster = Address::repeat_byte(0xa);
        assert!(ensure_submission_identity(poster, poster).is_ok());

        let err = ensure_submission_identity(poster, Address::repeat_byte(0xb)).unwrap_err();
        assert!(matches!(
            err,
            ComposerConfigError::SubmissionIdentityMismatch {
                submitter_poster,
                post_batch_signer,
            } if submitter_poster == poster && post_batch_signer == Address::repeat_byte(0xb)
        ));
    }

    fn held(sender: Address, direction: Direction, nonce: u64, hash_byte: u8) -> HeldTx {
        HeldTx {
            raw_tx: Bytes::from(vec![hash_byte; 4]),
            hash: TxHash::repeat_byte(hash_byte),
            attempts: 0,
            max_fee_per_gas: u128::from(hash_byte),
            priority_fee_per_gas: u128::from(hash_byte),
            sender,
            nonce,
            direction,
        }
    }

    #[test]
    fn capacity_cuts_retain_the_fifo_suffix_without_attempts() {
        for reason in [
            "l1_tx_gas_budget",
            "l1_bundle_gas_budget",
            "l2_block_capacity",
            "composition_blob_limit",
        ] {
            let pool = HeldPool::new(Default::default());
            let users = vec![
                held(Address::repeat_byte(1), Direction::Outbound, 0, 1),
                held(Address::repeat_byte(2), Direction::Outbound, 0, 2),
                held(Address::repeat_byte(2), Direction::Outbound, 1, 3),
                held(Address::repeat_byte(3), Direction::Inbound, 0, 4),
            ];
            for user in &users {
                pool.push_contiguous(user.clone(), 0).unwrap();
            }
            let mut selection = selection::Selection::new(&users, 4, 0, 0);
            let candidate = crate::admission_events::Candidate::new(
                10200,
                Address::repeat_byte(9),
                1,
                B256::repeat_byte(8),
                BundleTarget::NextBlock,
                4,
                4,
                (4, None),
                Some(0),
                None,
            );
            assert!(selection.consider(0));
            assert!(pool.claim(&users[0]));
            selection.accept(0);
            assert!(selection.consider(1));
            assert!(pool.claim(&users[1]));
            let mut capacity = CapacityDeferrals::default();
            capacity.defer(&mut selection, &candidate, 1, users[1].clone(), reason);
            assert!(!selection.consider(2));
            assert!(!selection.consider(3));
            assert!(!selection.fifo_bypassed());
            // Returning the accepted prefix after an infrastructure abort must
            // also preserve the untouched suffix and the refused group.
            let mut requeue = vec![(0, users[0].clone())];
            requeue.extend(capacity.groups);
            requeue_unprocessed(&pool, 1, &[], requeue);
            let recovered = pool.candidates();
            let mut recovered_hashes: Vec<_> = recovered.iter().map(|t| t.hash).collect();
            recovered_hashes.sort();
            let mut expected_hashes: Vec<_> = users.iter().map(|t| t.hash).collect();
            expected_hashes.sort();
            assert_eq!(recovered_hashes, expected_hashes);
            // Recovered groups return to the pool front. The unclaimed suffix
            // can move relative to an independent chain, but not its own root.
            let position = |hash| recovered.iter().position(|t| t.hash == hash).unwrap();
            assert!(position(users[1].hash) < position(users[2].hash));
            for user in recovered {
                assert_eq!(user.attempts, 0);
                assert!(pool.claim(&user), "no composing reservation stranded");
            }
            assert_eq!(capacity.reasons[reason], 1);
        }
    }

    #[test]
    fn poison_gap_matches_only_same_chain_higher_nonce() {
        let sender = Address::repeat_byte(0xa);
        let other = Address::repeat_byte(0xb);
        let gaps = vec![(sender, Direction::Inbound, 3)];

        assert_eq!(
            poison_gap_for(&gaps, &held(sender, Direction::Inbound, 4, 1)),
            Some(3)
        );
        assert_eq!(
            poison_gap_for(&gaps, &held(sender, Direction::Inbound, 3, 2)),
            None
        );
        assert_eq!(
            poison_gap_for(&gaps, &held(sender, Direction::Outbound, 4, 3)),
            None
        );
        assert_eq!(
            poison_gap_for(&gaps, &held(other, Direction::Inbound, 4, 4)),
            None
        );
    }

    #[test]
    fn push_poison_root_records_gap_once() {
        let sender = Address::repeat_byte(0xc);
        let tx = held(sender, Direction::Inbound, 7, 1);
        let mut poison = Vec::new();
        let mut gaps = Vec::new();

        push_poison_root(&mut poison, &mut gaps, tx.clone());
        push_poison_root(&mut poison, &mut gaps, tx);

        assert_eq!(poison.len(), 2);
        assert_eq!(gaps, vec![(sender, Direction::Inbound, 7)]);
    }

    #[test]
    fn funding_condition_is_local_deferral_not_poison_or_infrastructure_failure() {
        let condition = eez_protocol::SourceCondition::Escrow {
            required: U256::from(10),
            available: U256::from(3),
        };
        let error = eez_protocol::ComposerError::from(
            eez_protocol::ExecutorErrorKind::SourceBlocked(condition.clone()),
        );
        assert_eq!(source_condition(&error), Some(&condition));
        assert!(!sim_error_is_poison(&error));
        let unavailable = eez_protocol::ComposerError::from(
            eez_protocol::ExecutorErrorKind::Missing("pinned state"),
        );
        assert_eq!(source_condition(&unavailable), None);
        assert!(!sim_error_is_poison(&unavailable));
        let malformed = eez_protocol::ComposerError::from(
            eez_protocol::ExecutorErrorKind::Encoding("invalid transaction".into()),
        );
        assert_eq!(source_condition(&malformed), None);
        assert!(sim_error_is_poison(&malformed));
    }

    #[test]
    fn funding_wait_preserves_retry_budget_and_reaches_work_beyond_a_full_prefix() {
        for limit in [1, 3, 12, 50] {
            let pool = HeldPool::new(Default::default());
            let sender = Address::repeat_byte(0xa);
            let mut root = held(sender, Direction::Outbound, 0, 1);
            root.attempts = MAX_BUNDLE_ATTEMPTS - 1;
            pool.push_contiguous(root.clone(), 0).unwrap();
            for nonce in 1..limit {
                pool.push_contiguous(
                    held(sender, Direction::Outbound, nonce as u64, (nonce + 1) as u8),
                    0,
                )
                .unwrap();
            }
            // The funding direction remains independent even for the same
            // address. A blocked prefix must not hide this candidate.
            let deposit = held(sender, Direction::Inbound, 0, 100);
            pool.push_contiguous(deposit.clone(), 0).unwrap();
            assert_eq!(pool.candidates()[limit].hash, deposit.hash);
            for opportunity in 1..=MAX_BUNDLE_ATTEMPTS + 1 {
                let snapshot = pool.candidates();
                let deposit_idx = snapshot
                    .iter()
                    .position(|tx| tx.hash == deposit.hash)
                    .unwrap();
                let mut selection = selection::Selection::new(&snapshot, limit, 0, 0);
                assert!(selection.consider(0));
                assert!(pool.claim(&snapshot[0]));
                let source = crate::held_pool::SourceState {
                    l1: eez_l1::L1BlockRef {
                        number: u64::from(opportunity),
                        hash: B256::with_last_byte(opportunity as u8),
                    },
                    l2: eez_l1::L1BlockRef {
                        number: 0,
                        hash: B256::ZERO,
                    },
                };
                pool.begin_evaluation(source);
                let mut deferred = Vec::new();
                defer_source(
                    &pool,
                    &mut selection,
                    &mut deferred,
                    source,
                    0,
                    snapshot[0].clone(),
                    eez_protocol::SourceCondition::Escrow {
                        required: U256::from(1),
                        available: U256::ZERO,
                    },
                );
                for (idx, tx) in snapshot.iter().enumerate() {
                    if tx.direction == Direction::Outbound && tx.nonce > 0 {
                        assert!(!selection.consider(idx));
                    }
                }
                assert!(selection.consider(deposit_idx));
                assert!(pool.claim(&snapshot[deposit_idx]));
                selection.accept(deposit_idx);
                // Exercise a subsequent slot abort too: independent work and
                // deferred users must both retain ownership without charges.
                deferred.push((deposit_idx, snapshot[deposit_idx].clone()));
                requeue_unprocessed(&pool, 1, &[], deferred);
                let queued = pool.candidates();
                assert_eq!(queued.len(), limit + 1);
                assert_eq!(queued[0].hash, root.hash);
                assert_eq!(queued[0].attempts, MAX_BUNDLE_ATTEMPTS - 1);
                assert!(queued[1..].iter().all(|tx| tx.attempts == 0));
            }
            // Once funding is available, the original identity and its nonce
            // chain can be claimed again without explicit readmission.
            let queued = pool.candidates();
            let mut selection = selection::Selection::new(&queued, limit, 0, 0);
            assert!(selection.consider(0));
            assert!(pool.claim(&queued[0]));
            selection.accept(0);
            assert_eq!(queued[0].hash, root.hash);
            pool.release_in_flight_batch(&queued[..1]);
            if limit > 1 {
                let successor = queued
                    .iter()
                    .position(|tx| tx.direction == Direction::Outbound && tx.nonce == 1)
                    .unwrap();
                assert!(selection.consider(successor));
                assert!(pool.claim(&queued[successor]));
                pool.release_in_flight_batch(&queued[successor..=successor]);
            }
        }
    }

    #[test]
    fn settlement_failures_increment_once_per_episode_and_preserve_fifo() {
        let pool = HeldPool::new(Default::default());
        let first = held(Address::repeat_byte(0xa), Direction::Inbound, 0, 1);
        let second = held(Address::repeat_byte(0xb), Direction::Inbound, 0, 2);
        pool.push_contiguous(first.clone(), 0).unwrap();
        pool.push_contiguous(second.clone(), 0).unwrap();

        let first_failure = recover_settlement_failure(&pool, 1, pool.pop_all());
        assert_eq!(
            first_failure,
            SettlementFailureOutcome {
                requeued: 2,
                evicted: 0,
            }
        );
        let after_first = pool.pop_all();
        assert_eq!(
            after_first.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
            vec![first.hash, second.hash]
        );
        assert!(after_first.iter().all(|tx| tx.attempts == 1));

        let second_failure = recover_settlement_failure(&pool, 1, after_first);
        assert_eq!(
            second_failure,
            SettlementFailureOutcome {
                requeued: 2,
                evicted: 0,
            }
        );
        let after_second = pool.pop_all();
        assert_eq!(
            after_second.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
            vec![first.hash, second.hash]
        );
        assert!(after_second.iter().all(|tx| tx.attempts == 2));
        pool.release_in_flight_batch(&after_second);
    }

    #[test]
    fn settlement_failure_limit_evicts_current_and_queued_nonce_suffix() {
        let pool = HeldPool::new(Default::default());
        let sender = Address::repeat_byte(0xa);
        let mut root = held(sender, Direction::Inbound, 0, 1);
        root.attempts = MAX_BUNDLE_ATTEMPTS - 1;
        let in_flight_suffix = held(sender, Direction::Inbound, 1, 2);
        let queued_suffix = held(sender, Direction::Inbound, 2, 3);
        let opposite_direction = held(sender, Direction::Outbound, 0, 4);
        let independent = held(Address::repeat_byte(0xb), Direction::Inbound, 0, 5);
        pool.push_contiguous(root, 0).unwrap();
        pool.push_contiguous(in_flight_suffix, 0).unwrap();
        pool.push_contiguous(queued_suffix, 0).unwrap();
        let failed = pool.pop_n(2);
        pool.push_contiguous(opposite_direction.clone(), 0).unwrap();
        pool.push_contiguous(independent.clone(), 0).unwrap();

        let recovery = recover_settlement_failure(&pool, 1, failed);
        assert_eq!(
            recovery,
            SettlementFailureOutcome {
                requeued: 0,
                evicted: 3,
            }
        );
        let remaining = pool.pop_all();
        assert_eq!(
            remaining.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
            vec![opposite_direction.hash, independent.hash]
        );
        pool.release_in_flight_batch(&remaining);
    }

    #[test]
    fn recursive_error_requeues_only_candidates_remaining_after_prover_eviction() {
        let pool = HeldPool::new(Default::default());
        let sender = Address::repeat_byte(0xc);
        let poison = held(sender, Direction::Inbound, 0, 1);
        let suffix = held(sender, Direction::Inbound, 1, 2);
        let independent = held(Address::repeat_byte(0xd), Direction::Inbound, 0, 3);
        pool.push_contiguous(poison.clone(), 0).unwrap();
        pool.push_contiguous(suffix.clone(), 0).unwrap();
        pool.push_contiguous(independent.clone(), 0).unwrap();

        let survivors = pool.pop_all();
        let (retry, evicted) = partition_retryable(survivors, &poison);
        assert_eq!(
            evicted.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
            vec![poison.hash, suffix.hash]
        );
        assert!(
            pool.evict_chain_at_or_above(poison.sender, poison.direction, poison.nonce)
                .is_empty()
        );

        // Mirrors the recursive-error arm: only its owned retry set returns to
        // the pool; the outer original drain must never be requeued.
        pool.push_front_batch(retry);
        let queued = pool.pop_all();
        assert_eq!(
            queued.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
            vec![independent.hash]
        );
        pool.release_in_flight_batch(&queued);
        assert!(pool.push_contiguous(poison, 0).is_ok());
    }

    #[test]
    fn inbound_owner_mapping_follows_post_batch_entry_order() {
        fn entry(marker: u8) -> eez_protocol::abi::ExecutionEntrySol {
            eez_protocol::abi::ExecutionEntrySol {
                returnData: Bytes::from(vec![marker]),
                ..Default::default()
            }
        }

        fn composition(
            entries: Vec<eez_protocol::abi::ExecutionEntrySol>,
        ) -> eez_protocol::Composition {
            eez_protocol::Composition {
                actions: Vec::new(),
                source: eez_protocol::SourceComposition {
                    l2_static_entries: Vec::new(),
                    rollup_id: eez_protocol::RollupId(1),
                    batch: eez_protocol::EvmBatch {
                        entries,
                        ..Default::default()
                    },
                    entry_traces: Vec::new(),
                    callback_entry_counts: Vec::new(),
                    outbound_event_hashes: Vec::new(),
                },
                targets: Vec::new(),
            }
        }

        let first = held(Address::repeat_byte(0xa), Direction::Inbound, 0, 0xa);
        let second = held(Address::repeat_byte(0xb), Direction::Inbound, 0, 0xb);
        let compositions = vec![
            (composition(vec![entry(0xa1), entry(0xa2)]), first.hash),
            (composition(vec![entry(0xb1)]), second.hash),
        ];

        let outbound = [entry(0x01), entry(0x02)];
        // Canonical postBatch order is anchor, outbound entries, then the
        // inbound entries grouped by their owning composition.
        let mut entries = vec![entry(0)];
        entries.extend(outbound.iter().cloned());
        entries.extend(
            compositions
                .iter()
                .flat_map(|(composition, _)| composition.source.batch.entries.iter().cloned()),
        );
        let batch = eez_protocol::EvmBatch {
            entries,
            ..Default::default()
        };
        assert_eq!(
            batch
                .entries
                .iter()
                .map(|entry| entry.returnData[0])
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 0xa1, 0xa2, 0xb1]
        );

        let survivors = vec![first.clone(), second.clone()];
        for (entry_index, expected) in [(4, first.hash), (5, second.hash)] {
            let failure = ActionableProverFailure::Inbound {
                entry_index,
                entry_hash: alloy_primitives::keccak256(batch.entries[entry_index].abi_encode()),
            };
            validate_actionable_prover_failure(failure, &batch, None).unwrap();
            assert_eq!(
                actionable_held_tx(failure, &survivors, outbound.len(), &compositions)
                    .unwrap()
                    .hash,
                expected
            );
        }
    }

    #[test]
    fn local_observation_mismatch_ejects_only_owner_nonce_suffix() {
        let inbound = held(Address::repeat_byte(0xa), Direction::Inbound, 0, 0xa);
        let suffix = held(inbound.sender, Direction::Inbound, 1, 0xc);
        let unrelated_outbound = held(Address::repeat_byte(0xb), Direction::Outbound, 0, 0xb);
        let unrelated_inbound = held(Address::repeat_byte(0xd), Direction::Inbound, 0, 0xd);
        let composition = eez_protocol::Composition {
            actions: Vec::new(),
            source: eez_protocol::SourceComposition {
                l2_static_entries: Vec::new(),
                rollup_id: eez_protocol::RollupId::MAINNET,
                batch: eez_protocol::EvmBatch {
                    staticEntries: vec![Default::default()],
                    ..Default::default()
                },
                entry_traces: Vec::new(),
                callback_entry_counts: Vec::new(),
                outbound_event_hashes: Vec::new(),
            },
            targets: Vec::new(),
        };
        let survivors = vec![
            unrelated_outbound.clone(),
            inbound.clone(),
            suffix.clone(),
            unrelated_inbound.clone(),
        ];
        let compositions = vec![(composition, inbound.hash)];

        let poison = observation_held_tx(0, &survivors, &compositions)
            .cloned()
            .unwrap();
        assert_eq!(poison.hash, inbound.hash);
        assert!(observation_held_tx(1, &survivors, &compositions).is_none());

        let (retry, evicted) = partition_retryable(survivors, &poison);
        assert_eq!(
            retry.iter().map(|held| held.hash).collect::<Vec<_>>(),
            vec![unrelated_outbound.hash, unrelated_inbound.hash],
        );
        assert_eq!(
            evicted.iter().map(|held| held.hash).collect::<Vec<_>>(),
            vec![inbound.hash, suffix.hash],
        );
    }

    #[test]
    fn local_observation_comparison_reports_exact_owner() {
        let observation = |expected_success, expected_output: Bytes| BlockObservation {
            after_transactions: 0,
            manager_address: Address::repeat_byte(0x11),
            claim: eez_protocol::settlement::FifoObservationClaim {
                pinned_block_hash: B256::repeat_byte(0x22),
                call_mode: eez_protocol::CallMode::Static,
                target_address: Address::repeat_byte(0x33),
                source_address: Address::repeat_byte(0x44),
                source_rollup_id: eez_protocol::RollupId::MAINNET,
                data: Bytes::new(),
                value: U256::ZERO,
                expected_success,
                expected_output,
            },
        };
        let schedule = vec![
            (3, observation(true, Bytes::from_static(b"first"))),
            (7, observation(false, Bytes::from_static(b"second"))),
        ];
        let matching = vec![
            ObservedOutcome {
                success: true,
                output: Bytes::from_static(b"first"),
            },
            ObservedOutcome {
                success: false,
                output: Bytes::from_static(b"second"),
            },
        ];
        assert_eq!(
            first_observation_mismatch(&schedule, &matching).unwrap(),
            None,
        );

        let mut mismatching = matching;
        mismatching[1].output = Bytes::from_static(b"forged");
        assert_eq!(
            first_observation_mismatch(&schedule, &mismatching).unwrap(),
            Some(ObservationMismatchDetails {
                composition_index: 7,
                observation_index: 1,
                expected_success: false,
                observed_success: false,
                expected_output_hash: alloy_primitives::keccak256(b"second"),
                observed_output_hash: alloy_primitives::keccak256(b"forged"),
            }),
        );
        assert!(first_observation_mismatch(&schedule, &mismatching[..1]).is_err());
    }

    #[test]
    fn eip7623_floor_matches_hand_computed_vector() {
        // 21000 + 10 * (zeros + 4*nonzeros).
        assert_eq!(calldata_floor_gas(&[]), 21_000);
        // 3 zeros (3 tokens) + 2 non-zeros (8 tokens) = 11 tokens = 110 gas.
        assert_eq!(calldata_floor_gas(&[0, 0xab, 0, 0xcd, 0]), 21_110);
        // A realistic batch's floor alone outgrows a multi-million fixed limit.
        assert!(calldata_floor_gas(&vec![0xff; 200 * 1024]) > 4_000_000);
    }

    /// The forge pin test re-declares both constants as literals, so a change
    /// here would leave it measuring the old value in silence. Bind them.
    #[test]
    fn solidity_pin_test_mirrors_the_rust_gas_pins() {
        let sol = include_str!("../../../contracts/test/PostBatchGasPins.t.sol");
        for (name, rust) in [
            ("POSTBATCH_BASE_GAS_PIN", POSTBATCH_BASE_GAS_PIN),
            ("POSTBATCH_ENTRY_GAS_PIN", POSTBATCH_ENTRY_GAS_PIN),
        ] {
            let decl = format!("uint256 private constant {name} = ");
            let rest = sol
                .split(&decl)
                .nth(1)
                .unwrap_or_else(|| panic!("{name} is not declared in PostBatchGasPins.t.sol"));
            let literal: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '_')
                .collect();
            let sol_value: u64 = literal
                .replace('_', "")
                .parse()
                .unwrap_or_else(|e| panic!("{name} literal {literal:?}: {e}"));
            assert_eq!(
                sol_value, rust,
                "{name} drifted: Solidity pins {sol_value}, Rust projects {rust}",
            );
        }
    }

    /// Any ceiling the clamp accepts must let the drain take at least one tx.
    #[test]
    fn the_lowest_accepted_ceiling_still_admits_one_held_tx() {
        let mut budget = PostBatchGasBudget::new(minimum_viable_postbatch_gas());
        assert!(
            budget.try_accept(TxL1Gas {
                estimate_reasons: Default::default(),
                entries: 1,
                entry_discount: 0,
                calldata_gas: 0,
                target_gas: 0,
                ..Default::default()
            }),
            "the lowest accepted ceiling admits no tx — every drain would evict \
             its first held tx as poison",
        );
    }

    /// The pre-prove gate sizes the batch with a `MAX_PROOF_BYTES` stand-in for
    /// the proof it does not have yet. That is only safe if the stand-in is a
    /// worst case: longer than any real proof, and priced at the dearer
    /// non-zero-byte rate. Both halves are pinned here — if a proof system ever
    /// returns something bigger, this fails before the gate turns optimistic and
    /// the signing-side ceiling has to catch it instead.
    #[test]
    fn proof_placeholder_is_a_worst_case() {
        /// r‖s‖v — the attestation `ECDSAProofSystem` verifies.
        const ECDSA_PROOF_BYTES: usize = 65;
        const { assert!(MAX_PROOF_BYTES >= ECDSA_PROOF_BYTES) };
        // Non-zero bytes are 4 EIP-7623 tokens, zeros are 1, so an all-0xff
        // stand-in prices above any real proof of the same length.
        let placeholder = [0xffu8; MAX_PROOF_BYTES];
        assert!(
            calldata_floor_gas(&placeholder) >= calldata_floor_gas(&[0x11u8; ECDSA_PROOF_BYTES])
        );
        assert!(calldata_floor_gas(&placeholder) >= calldata_floor_gas(&[0u8; MAX_PROOF_BYTES]));
        // Monotonic in length, so a shorter real proof can never cost more.
        assert!(calldata_floor_gas(&[0xffu8; 96]) <= calldata_floor_gas(&placeholder));
    }

    #[test]
    fn empty_candidate_grid_is_highest_first() {
        // The pure half of boundary selection: which on-grid offsets below the
        // snapped height are empty. Regression: an earlier revision folded a
        // per-block framing allowance into the same vector that carried the
        // emptiness flag, so NO candidate ever qualified and emission died.
        let pick = |empty: &[bool], k: usize| -> Vec<usize> {
            (0..empty.len()).step_by(k).filter(|&i| empty[i]).collect()
        };
        assert_eq!(pick(&[true; 12], 5), vec![0, 5, 10]);
        // Snapped height carries txs → the next K down leads.
        let mut e = [true; 12];
        e[0] = false;
        assert_eq!(pick(&e, 5), vec![5, 10]);
        // Only off-grid blocks empty → nothing to settle at.
        let mut e = [false; 12];
        e[3] = true;
        assert!(pick(&e, 5).is_empty());
        // Range shorter than one step still offers the snapped height.
        assert_eq!(pick(&[true, true, true], 5), vec![0]);
    }

    #[test]
    fn oversized_backlog_requests_a_bounded_historical_prefix() {
        let limits = EmissionLimits {
            timing: RollupTiming::new(5_000, 1_000, 1_000, 100),
            max_blocks: 65_536,
            max_gas: 30_000_000,
        };
        // The native catch-up run reached 66,630 before a post could complete.
        // A generic Build error made policy mode retain the range forever.
        assert!(matches!(
            limits.checked_window_span(0, 66_630),
            Err(PreparePostBatchError::OverBudget {
                constraint: "window_limit",
                required: 66_630,
                maximum: 65_536,
            })
        ));
        let prefix = limits
            .timing
            .historical_chunk_boundary(0, 66_630, limits.max_blocks)
            .unwrap();
        assert_eq!(prefix, 65_535);
        assert_eq!(limits.checked_window_span(0, prefix).unwrap(), 65_535);
        assert_eq!(limits.checked_window_span(0, 65_536).unwrap(), 65_536);
        for posted in [0, 42, u64::MAX - 65_536] {
            assert_eq!(
                limits.checked_window_span(posted, posted + 65_536).unwrap(),
                65_536
            );
        }
        for (posted, terminal) in [(4, 4), (5, 4), (u64::MAX, 0)] {
            assert!(
                matches!(
                    limits.checked_window_span(posted, terminal),
                    Err(PreparePostBatchError::Build(_))
                ),
                "an invalid endpoint must not authorize capacity splitting"
            );
        }
    }

    /// Gas one outbound target call used on L1, measured on a kurtosis devnet
    /// (the same run measured ~336k marginal per outbound entry in total).
    const MEASURED_TARGET_GAS: u64 = 47_763;

    /// An outbound settlement entry of realistic size: one L1 call with a
    /// 4-byte selector plus two words of arguments.
    fn outbound_entry() -> eez_protocol::abi::ExecutionEntrySol {
        eez_protocol::abi::ExecutionEntrySol {
            l2ToL1Calls: vec![eez_protocol::abi::L2ToL1CallSol {
                revertNextNCalls: 0,
                isStatic: false,
                gas: 0,
                sourceAddress: Address::repeat_byte(0x11),
                sourceRollupId: 1,
                targetAddress: Address::repeat_byte(0x22),
                value: U256::ZERO,
                data: Bytes::from(vec![0x55u8; 68]),
            }],
            destinationRollupId: 1,
            success: true,
            ..Default::default()
        }
    }

    /// The single no-call anchor inserted by `prepare_post_batch_raw`: one
    /// own-rollup root update, zero ether delta and no proxy creation.
    fn gas_projection_anchor() -> eez_protocol::abi::ExecutionEntrySol {
        eez_protocol::abi::ExecutionEntrySol {
            rootUpdates: vec![eez_protocol::abi::RootUpdateSol {
                rollupId: 1,
                currentRoot: B256::repeat_byte(0x33),
                newRoot: B256::repeat_byte(0x44),
                etherDelta: eez_protocol::abi::EtherDeltaSol::ZERO,
            }],
            destinationRollupId: 1,
            success: true,
            ..Default::default()
        }
    }

    #[test]
    fn projection_charges_the_pins_the_calldata_and_the_probed_target() {
        // The cheapest postBatch: tx base + base pin + one leading entry.
        assert_eq!(
            projected_postbatch_gas(1, 0, 0),
            21_000 + POSTBATCH_BASE_GAS_PIN + POSTBATCH_ENTRY_GAS_PIN
        );
        // Entries are linear in the pin; calldata and probed target gas add raw.
        assert_eq!(
            projected_postbatch_gas(4, 500, 100),
            21_000 + POSTBATCH_BASE_GAS_PIN + 4 * POSTBATCH_ENTRY_GAS_PIN + 600
        );
        // Standard EIP-7623 rates, well under the 10-per-token floor the
        // signing path refuses on.
        assert_eq!(calldata_gas(&[]), 0);
        assert_eq!(calldata_gas(&[0, 0xab, 0, 0xcd, 0]), 3 * 4 + 2 * 16);
        // The bug this replaces: at a 50-tx bundle cap the real cost passes the
        // EIP-7825 per-tx cap, so no gas limit makes the batch postable.
        assert!(
            projected_postbatch_gas(51, 0, 50 * MEASURED_TARGET_GAS) > DEFAULT_MAX_POSTBATCH_GAS,
            "50 outbound entries must be recognised as unpostable",
        );
    }

    #[test]
    fn native_da_changes_only_blob_references_in_postbatch_calldata() {
        use alloy_sol_types::SolCall as _;

        let entries = vec![gas_projection_anchor(), outbound_entry()];
        let cost = projected_tx_l1_gas(&entries[1..], MEASURED_TARGET_GAS);
        let mut batch = eez_protocol::EvmBatch {
            entries,
            immediateEntryCount: U256::from(2),
            proofs: vec![Bytes::from(vec![0xff; MAX_PROOF_BYTES])],
            ..Default::default()
        };
        let capacity = eez_blob_codec::DATA_BYTES_PER_BLOB;
        let mut one_blob_calldata = None;
        let mut previous_calldata_length = 0;
        for (payload_bytes, expected_blobs) in
            [(1, 1), (capacity / 2, 1), (capacity, 2), (capacity * 2, 3)]
        {
            let encoded = eez_blob_codec::encode_native_stream(
                1,
                Bytes::from(vec![0x77; payload_bytes]),
                Vec::new(),
            )
            .unwrap();
            let blobs = eez_blob_codec::pack(&encoded.blob_portion).unwrap();
            assert_eq!(blobs.len(), expected_blobs);
            batch.blobIndices = (0..blobs.len()).map(U256::from).collect();
            assert!(batch.callData.is_empty());
            let calldata = eez_protocol::abi::postAndVerifyBatchCall {
                batch: batch.clone(),
            }
            .abi_encode();
            if expected_blobs == 1 {
                assert_eq!(
                    one_blob_calldata
                        .get_or_insert_with(|| calldata.clone())
                        .as_slice(),
                    calldata.as_slice(),
                    "changing blob payload length must not duplicate it in L1 calldata",
                );
            } else {
                assert_eq!(
                    calldata.len(),
                    previous_calldata_length + 32,
                    "one additional blob adds its ABI index word, not its payload",
                );
            }
            previous_calldata_length = calldata.len();
            assert_eq!(
                projected_tx_l1_gas(&batch.entries[1..], MEASURED_TARGET_GAS).total(),
                cost.total(),
            );
        }
    }

    #[test]
    fn real_entry_calldata_and_its_floor_remain_part_of_final_gas() {
        use alloy_sol_types::SolCall as _;

        let mut entry = outbound_entry();
        let original = projected_tx_l1_gas(std::slice::from_ref(&entry), MEASURED_TARGET_GAS);
        entry.l2ToL1Calls[0].data = Bytes::from(vec![0xff; 100_000]);
        let large = projected_tx_l1_gas(std::slice::from_ref(&entry), MEASURED_TARGET_GAS);
        assert!(large.calldata_gas > original.calldata_gas + 1_500_000);
        assert_eq!(large.entries, original.entries);
        assert_eq!(large.target_gas, MEASURED_TARGET_GAS);
        let calldata = eez_protocol::abi::postAndVerifyBatchCall {
            batch: eez_protocol::EvmBatch {
                entries: vec![gas_projection_anchor(), entry],
                immediateEntryCount: U256::from(2),
                blobIndices: vec![U256::ZERO],
                proofs: vec![Bytes::from(vec![0xff; MAX_PROOF_BYTES])],
                ..Default::default()
            },
        }
        .abi_encode();
        let standard = projected_postbatch_gas(2, calldata_gas(&calldata), MEASURED_TARGET_GAS);
        assert!(
            calldata_floor_gas(&calldata) > standard,
            "large real L1 calldata still requires the separate final fork-specific floor",
        );
        assert!(calldata_gas(&calldata) > large.calldata_gas);
        assert_eq!(projected_tx_l1_gas(&[], 0).total(), 0);
    }

    #[test]
    fn drain_budget_cuts_the_accepted_prefix_below_the_cap() {
        let entries = [outbound_entry()];
        let cost = projected_tx_l1_gas(&entries, MEASURED_TARGET_GAS);
        // Charge the actual entry ABI and inline call. Its DA copy and source
        // transaction are blob bytes, so neither adds postBatch calldata gas.
        assert_eq!(cost.entries, 1);
        assert_eq!(cost.target_gas, MEASURED_TARGET_GAS);
        let encoded = {
            use alloy_sol_types::SolValue as _;
            calldata_gas(&entries[0].abi_encode())
        };
        assert_eq!(cost.calldata_gas, encoded);

        let mut budget = PostBatchGasBudget::new(DEFAULT_MAX_POSTBATCH_GAS);
        // Framing is now charged per candidate, while the cap retains the
        // complete pre-margin transaction budget.
        assert_eq!(budget.cap, DEFAULT_MAX_POSTBATCH_GAS);
        let per_tx = cost.total() + cost.framing_gas;
        let expected = (budget.cap - budget.full_estimate()) / per_tx;
        let mut accepted = 0u64;
        // The drain never offers more than the per-bundle cap.
        while accepted < 50 && budget.try_accept(cost) {
            accepted += 1;
        }
        assert_eq!(accepted, expected);
        // The cut is what makes the batch postable at all.
        assert!(accepted < 50);
        assert!(budget.projected <= DEFAULT_MAX_POSTBATCH_GAS);
        // A rejected tx leaves the projection untouched, so the accepted prefix
        // is exactly what the batch will carry.
        let projected = budget.projected;
        assert!(!budget.try_accept(cost));
        assert_eq!(budget.projected, projected);
        // The drain and the prepare-time belt agree on the same entries: the
        // leading immediate plus one per accepted tx.
        assert_eq!(
            projected,
            projected_postbatch_gas(
                accepted + 1,
                accepted * cost.calldata_gas,
                accepted * MEASURED_TARGET_GAS,
            )
        );
    }

    /// Two outbound pairs near the EIP-7825 cap overflow a 30M block, so the
    /// second must be refused before `build_sync_block` hard-errors on it. The
    /// load reserves nothing: the precheck charges only its base cost, and the
    /// user tx's declared limit still has to fit.
    /// Native txs reserve no budget, so groups whose native parts together
    /// declare more than the block (each at the 16.7M cap) are admitted on the
    /// precheck; only actual execution decides.
    #[test]
    fn native_groups_are_not_refused_for_the_cap_they_do_not_reserve() {
        let two_loads = [native_raw(0), native_raw(1)];
        assert_eq!(
            declared_group_gas(&two_loads),
            2 * eez_primitives::SYSTEM_TX_GAS_CAP
        );
        assert!(declared_group_gas(&two_loads) > BUILDER_GAS_LIMIT);
        // The precheck for two deliveries charges only their base cost.
        assert_eq!(
            block_gas_fit(20_000_000, 2 * MIN_TX_GAS, BUILDER_GAS_LIMIT),
            BlockGasFit::Accept,
        );
        // A group refused behind an empty prefix can never fit; otherwise it waits.
        assert_eq!(capacity_refusal(true), BlockGasFit::Unfittable);
        assert_eq!(capacity_refusal(false), BlockGasFit::Defer);
    }

    fn native_raw(nonce: u64) -> Bytes {
        use alloy_eips::eip2718::Encodable2718 as _;
        let tx: eez_primitives::EezTxEnvelope = eez_primitives::SystemTransaction {
            chain_id: 1,
            nonce,
            to: eez_primitives::EEZL2_ADDRESS,
            value: U256::ZERO,
            input: Bytes::new(),
        }
        .into();
        tx.encoded_2718().into()
    }

    #[test]
    fn block_gas_fit_refuses_the_second_fat_outbound_pair() {
        // Live shapes: 30M block, a `loadExecutionTable` burning 100k, a user tx
        // declaring 16.7M and burning 13.3M of it.
        const USER_DECLARED: u64 = 16_700_000;
        const USER_BURNED: u64 = 13_300_000;
        const LOAD_BURNED: u64 = 100_000;
        let pair = MIN_TX_GAS + USER_DECLARED;

        // Pair 1 fits an empty block.
        assert_eq!(
            block_gas_fit(0, pair, BUILDER_GAS_LIMIT),
            BlockGasFit::Accept,
        );
        // Pair 2 sees the burn of pair 1 and does not.
        let after_one = LOAD_BURNED + USER_BURNED;
        assert_eq!(
            block_gas_fit(after_one, pair, BUILDER_GAS_LIMIT),
            BlockGasFit::Defer,
            "the second fat pair must be deferred, not built",
        );
        // Ground truth for the deferral: the builder itself would refuse pair 2's
        // user tx, since its declared limit exceeds what the block has left.
        assert!(USER_DECLARED > BUILDER_GAS_LIMIT - after_one - LOAD_BURNED);

        // Exact boundary: equal fits, one over defers.
        assert_eq!(
            block_gas_fit(BUILDER_GAS_LIMIT - pair, pair, BUILDER_GAS_LIMIT),
            BlockGasFit::Accept,
        );
        assert_eq!(
            block_gas_fit(BUILDER_GAS_LIMIT - pair + 1, pair, BUILDER_GAS_LIMIT),
            BlockGasFit::Defer,
        );

        // Head of line: a pair over the whole block is unfittable at any prefix,
        // so it is evicted rather than deferred forever.
        assert_eq!(
            block_gas_fit(0, BUILDER_GAS_LIMIT + 1, BUILDER_GAS_LIMIT),
            BlockGasFit::Unfittable,
        );
        assert_eq!(
            block_gas_fit(after_one, BUILDER_GAS_LIMIT + 1, BUILDER_GAS_LIMIT),
            BlockGasFit::Unfittable,
        );
        // An undecodable tx counts as the whole block, so it can never be
        // mistaken for free space.
        assert_eq!(
            declared_gas_limit(&Bytes::from_static(b"not a tx")),
            u64::MAX
        );
        assert_eq!(
            block_gas_fit(0, u64::MAX, BUILDER_GAS_LIMIT),
            BlockGasFit::Unfittable,
        );
    }
}
