//! Composer integration of the pure policy and canonical activity index.

use std::sync::Mutex;

use reth_chainspec::EthChainSpec as _;

use super::*;
use crate::settlement_policy::{
    CrossChainWork, DrainCoverage, PolicyDecision, PolicySnapshot, PolicyState, SettlementAnchor,
    index::{ActivityIndex, BlockActivity, DaOverhead},
};

#[derive(Debug, Default)]
struct RollupPolicy {
    state: PolicyState,
    activity: ActivityIndex,
    anchor: Option<SettlementAnchor>,
    predecessor: Option<HistoricalPredecessor>,
    estimate: Option<(eez_l1::L2Endpoint, eez_l1::L2Endpoint, u64)>,
    /// Immutable encodings measured by the index, retained only while pending.
    block_rlps: HashMap<B256, (u64, Vec<u8>)>,
    sequencing_target: Option<u64>,
    observation: Option<(u64, SettlementPolicyObservation)>,
}

/// Last successful policy evaluation. A due decision is not a submission or
/// evidence that settlement occurred. The anchor can be the initial registration.
#[derive(Clone, Copy, Debug)]
pub struct SettlementPolicyObservation {
    pub decision: PolicyDecision,
    pub anchor: SettlementAnchor,
    pub target_l1_timestamp: u64,
    pub block_threshold: u64,
    /// Conditional pure-L2 deadline, not a guarantee of inclusion at this time.
    pub pure_interval_deadline_ms: Option<u64>,
    pub evaluated_at_ms: u64,
}

impl RollupPolicy {
    fn observed_policy(
        &self,
        canonical: eez_l1::l1_canonical_head::PolicyAnchorSnapshot,
    ) -> Option<SettlementPolicyObservation> {
        let (generation, observation) = self.observation?;
        (canonical.through.is_some()
            && canonical.generation == generation
            && canonical.endpoint == Some(observation.anchor.endpoint))
        .then_some(observation)
    }
}

/// Bounded read-only policy/ledger observations; no provider or RPC calls.
#[derive(Clone, Debug)]
pub struct SettlementStatus {
    pub policy: Option<SettlementPolicyObservation>,
    /// Lowest unresolved attempt's observation. Inclusion alone is not completion.
    pub pending_submission: Option<crate::optimistic::SubmissionObservation>,
}

fn interval_deadline(
    anchor_seconds: u64,
    mode: crate::settlement_policy::PureL2Mode,
    has_pure: bool,
) -> Option<u64> {
    match mode {
        crate::settlement_policy::PureL2Mode::Interval { milliseconds } if has_pure => {
            anchor_seconds
                .checked_mul(1000)?
                .checked_add(milliseconds.get())
        }
        _ => None,
    }
}

impl<L2: BlockReader> Composer<L2> {
    /// Read existing policy and submission state without cloning transaction
    /// payloads. A new canonical generation hides an obsolete policy evaluation.
    pub fn settlement_status(&self, rollup_id: u64) -> Option<SettlementStatus> {
        let rollup = self.inner.rollups.get(&rollup_id)?;
        let runtime = self.inner.policy.rollups.get(&rollup_id)?;
        let policy = runtime
            .lock()
            .unwrap()
            .observed_policy(rollup.l1_head.policy_anchor_snapshot());
        Some(SettlementStatus {
            policy,
            pending_submission: rollup.optimistic.pending_observation(),
        })
    }
}

/// Urgency belongs to the held groups, not to each newly scheduled empty Sync.
#[derive(Debug)]
struct HistoricalPredecessor {
    endpoint: eez_l1::L2Endpoint,
    groups: Vec<B256>,
}

impl HistoricalPredecessor {
    fn cancel_groups(&mut self, groups: impl IntoIterator<Item = B256>) -> bool {
        for hash in groups {
            self.groups.retain(|group| *group != hash);
        }
        !self.groups.is_empty()
    }

    fn reconcile(
        &mut self,
        settled: eez_l1::L2Endpoint,
        canonical: Option<eez_l1::L2Endpoint>,
        mut is_pending: impl FnMut(B256) -> bool,
    ) -> bool {
        self.groups.retain(|hash| is_pending(*hash));
        !self.groups.is_empty()
            && self.endpoint.number > settled.number
            && canonical == Some(self.endpoint)
    }
}

#[derive(Debug)]
pub(super) struct EnabledPolicy {
    pub config: PolicyConfig,
    pub max_blobs_per_batch: Option<u64>,
    rollups: HashMap<u64, Mutex<RollupPolicy>>,
    /// A cancelled planning RPC must not accumulate detached ancestry workers.
    /// Its owned permit stays with the blocking job until that job exits.
    planning: Arc<tokio::sync::Semaphore>,
}

impl EnabledPolicy {
    pub(super) fn set_sequencing_target(&self, rollup_id: u64, target: u64) {
        if let Some(rollup) = self.rollups.get(&rollup_id) {
            rollup.lock().unwrap().sequencing_target = Some(target);
        }
    }

    pub(super) fn sequencing_target(&self, rollup_id: u64) -> Option<u64> {
        self.rollups
            .get(&rollup_id)?
            .lock()
            .unwrap()
            .sequencing_target
    }

    pub fn new(
        config: PolicyConfig,
        rollups: impl Iterator<Item = u64>,
        max_blobs_per_batch: Option<u64>,
    ) -> Self {
        Self {
            config,
            max_blobs_per_batch,
            rollups: rollups.map(|id| (id, Mutex::default())).collect(),
            planning: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
}

impl<L2> Composer<L2>
where
    L2: BlockReader<Header = alloy_consensus::Header>
        + StateProviderFactory
        + Send
        + Sync
        + 'static,
    <L2 as TransactionsProvider>::Transaction: Encodable2718,
{
    pub(super) async fn plan_preparation(
        &self,
        rollup_id: u64,
        parent: reth_primitives_traits::SealedHeader<alloy_consensus::Header>,
        target_height: u64,
        target_timestamp: u64,
        l1_parent: eez_l1::L1BlockRef,
    ) -> Result<bool, PolicyError> {
        let permit = Arc::clone(&self.inner.policy.planning)
            .try_acquire_owned()
            .map_err(|_| {
                PolicyError::Unavailable("previous preparation planner still running".into())
            })?;
        let rollup = self
            .inner
            .rollups
            .get(&rollup_id)
            .ok_or_else(|| PolicyError::Unavailable("unknown planning rollup".into()))?;
        if rollup.optimistic.blocking_height().is_some() {
            // Preserve the existing Empty reconciliation/settlement opportunity,
            // without bursting merely because blocked users remain queued.
            return Err(PolicyError::Unavailable(
                "settlement owner still blocks preparation".into(),
            ));
        }
        let (snapshot, anchor) = self.canonical_policy_anchor(rollup_id, rollup).await?;
        if snapshot.through != Some(l1_parent) {
            return Err(PolicyError::Unavailable(
                "preparation target parent is not reconciled".into(),
            ));
        }
        let composer = Self {
            inner: Arc::clone(&self.inner),
        };
        // Database reads and ordinary encoding stay off the async executor.
        // The sequencer owns the wait; timeout leaves at most this one worker.
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let rollup = composer
                .inner
                .rollups
                .get(&rollup_id)
                .ok_or_else(|| PolicyError::Unavailable("unknown planning rollup".into()))?;
            let policy = &composer.inner.policy;
            let runtime = policy
                .rollups
                .get(&rollup_id)
                .ok_or_else(|| PolicyError::Unavailable("unknown policy rollup".into()))?;
            let blocks = target_height
                .checked_sub(anchor.endpoint.number)
                .ok_or(PolicyError::Arithmetic("planning target below anchor"))?;
            if target_height <= parent.number() {
                return Err(PolicyError::Unavailable(
                    "planning terminal already built".into(),
                ));
            }
            let window_limit_due = blocks
                .checked_add(u64::from(composer.inner.emission.timing.k()))
                .is_none_or(|n| n > composer.inner.emission.max_blocks);
            let target = BundleTarget::Exact {
                block: l1_parent
                    .number
                    .checked_add(1)
                    .ok_or(PolicyError::Arithmetic("planning L1 target"))?,
                timestamp: target_timestamp,
            };
            let effective_blobs = composer
                .posting_blob_allowance(target, 0)
                .map_err(PolicyError::Unavailable)? as u64;
            let mut runtime = runtime.lock().unwrap();
            let (latest_pure_block, estimated_stream_bytes) =
                if blocks <= composer.inner.emission.max_blocks {
                    composer.update_activity_index(
                        rollup,
                        &mut runtime,
                        anchor.endpoint,
                        eez_l1::L2Endpoint {
                            number: parent.number(),
                            hash: parent.hash(),
                        },
                    )?;
                    (
                        runtime.activity.latest_pure(),
                        Some(runtime.activity.estimated_stream_bytes_with_overhead(
                            &eez_payload_codec::v0::PayloadSizer::default(),
                            DaOverhead::default(),
                        )?),
                    )
                } else {
                    // The exact window trigger already requires preparation. Do
                    // not scan an unbounded outage backlog just to make that hint.
                    (None, None)
                };
            let reasons = runtime.state.preparation_reasons(
                &policy.config,
                crate::settlement_policy::SchedulingSnapshot {
                    anchor,
                    target_l2_number: target_height,
                    target_l1_timestamp: target_timestamp,
                    latest_pure_block,
                    estimated_stream_bytes,
                    window_limit_due,
                    effective_blobs,
                    usable_bytes_per_blob: eez_blob_codec::DATA_BYTES_PER_BLOB as u64,
                },
            )?;
            // Occupancy is only a bounded admission opportunity, never a
            // declaration that a group is executable or canonically consumed.
            let queue_hint =
                !rollup.held_pool.is_empty() && rollup.optimistic.blocking_height().is_none();
            let prepare = !reasons.is_empty() || queue_hint || runtime.predecessor.is_some();
            if !rollup.l1_head.generation_is(snapshot.generation)
                || composer.inner.committer.last_header().hash() != parent.hash()
            {
                return Err(PolicyError::Unavailable(
                    "preparation context changed during planning".into(),
                ));
            }
            event!(name: "eez.composer.slot.plan", Level::INFO,
                event_name = "eez.composer.slot.plan", rollup_id, target_height,
                planned_at_ms = u64::try_from(std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis())
                    .unwrap_or(u64::MAX),
                target_timestamp, prepare, ?reasons, queue_hint,
                "preparation hint; final admission and canonical settlement remain authoritative");
            Ok(prepare)
        })
        .await
        .map_err(|e| PolicyError::Unavailable(format!("preparation planner worker: {e}")))?
    }

    /// Missing canonical registration/commitment evidence leaves readiness
    /// closed. The node retries through its existing boot reconciliation loop.
    pub async fn validate_settlement_policy_bootstrap(&self) -> Result<(), PolicyError> {
        self.validate_post_batch_target().await?;
        for (&rollup_id, rollup) in &self.inner.rollups {
            self.canonical_policy_anchor(rollup_id, rollup).await?;
        }
        Ok(())
    }

    /// A postBatch target other than the registry must be a deployed contract
    /// whose `eez()` is this registry, so posts cannot reach another registry
    /// or an address that would accept and drop them. Missing code is retried
    /// like any other unavailable boot input; a different registry is a
    /// configuration error. This checks identity only: whether the wrapper
    /// forwards the exact calldata is proved per attempt by final preflight,
    /// which executes the signed poster and requires the registry's effects.
    async fn validate_post_batch_target(&self) -> Result<(), PolicyError> {
        alloy_sol_types::sol! {
            #[sol(rpc)]
            interface IPostBatchTarget {
                function eez() external view returns (address);
            }
        }
        let posting = &self.inner.posting;
        let (target, registry) = (posting.post_batch_target, posting.registry);
        if target == registry {
            return Ok(());
        }
        let provider = &self.inner.cross_chain.exec_ctx.l1_provider;
        let code = provider.get_code_at(target).await.map_err(|error| {
            PolicyError::Unavailable(format!("EEZ_L1_POSTBATCH_TARGET {target} code: {error}"))
        })?;
        if code.is_empty() {
            return Err(PolicyError::Unavailable(format!(
                "EEZ_L1_POSTBATCH_TARGET {target} has no code"
            )));
        }
        let forwarded = IPostBatchTarget::new(target, provider)
            .eez()
            .call()
            .await
            .map_err(|error| {
                PolicyError::Unavailable(format!("EEZ_L1_POSTBATCH_TARGET {target} eez(): {error}"))
            })?;
        if forwarded != registry {
            return Err(PolicyError::Config(format!(
                "EEZ_L1_POSTBATCH_TARGET {target} forwards to {forwarded}, not registry {registry}"
            )));
        }
        event!(name: "eez.composer.postbatch.target_validated", Level::INFO,
            event_name = "eez.composer.postbatch.target_validated", %target, %registry,
            "postBatch transactions are sent through a wrapper that forwards to the registry");
        Ok(())
    }

    /// Run after canonical boot reconciliation and before ingress/sequencing
    /// become ready. No policy intent or submission artifact is created here.
    pub async fn validate_settlement_policy_startup(&self) -> Result<(), PolicyError> {
        let policy = &self.inner.policy;
        for (&rollup_id, rollup) in &self.inner.rollups {
            let (snapshot, anchor) = self.canonical_policy_anchor(rollup_id, rollup).await?;
            if policy.max_blobs_per_batch.is_none() {
                continue;
            }
            let target = self
                .inner
                .submitter
                .next_policy_target()
                .await
                .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
            let allowance = self
                .posting_blob_allowance(target, 0)
                .map_err(PolicyError::Unavailable)?;
            let BundleTarget::Exact {
                timestamp: target_time,
                ..
            } = target
            else {
                unreachable!("resolved target");
            };
            let slot_seconds = self.inner.emission.timing.l1_block_time().as_secs();
            let mut cursor = self.inner.committer.last_header();
            let mut measured = Vec::new();
            while cursor.number() > anchor.endpoint.number {
                let block = rollup
                    .l2_provider
                    .find_block_by_hash(cursor.hash(), BlockSource::Any)
                    .map_err(|error| PolicyError::Unavailable(error.to_string()))?
                    .ok_or_else(|| {
                        PolicyError::Unavailable(
                            "retained L2 block missing during posting-cap validation".into(),
                        )
                    })?;
                let encoded = alloy_rlp::encode(&block);
                let boundary = block.body().transactions().is_empty()
                    && cursor
                        .timestamp()
                        .abs_diff(target_time)
                        .is_multiple_of(slot_seconds);
                let parent_hash = cursor.parent_hash();
                let parent = rollup
                    .l2_provider
                    .sealed_header_by_hash(parent_hash)
                    .map_err(|error| PolicyError::Unavailable(error.to_string()))?
                    .ok_or_else(|| {
                        PolicyError::Unavailable(
                            "retained L2 parent missing during posting-cap validation".into(),
                        )
                    })?;
                if parent.number().checked_add(1) != Some(cursor.number()) {
                    return Err(PolicyError::Unavailable(
                        "retained L2 parent height mismatch".into(),
                    ));
                }
                measured.push((
                    eez_l1::L2Endpoint {
                        number: cursor.number(),
                        hash: cursor.hash(),
                    },
                    measure_ordinary(&self.inner.evm_config, &parent, &encoded)?,
                    boundary,
                ));
                cursor = parent;
            }
            if cursor.hash() != anchor.endpoint.hash {
                return Err(PolicyError::Unavailable(
                    "retained L2 history does not reach the validated anchor".into(),
                ));
            }
            validate_retained_byte_segments(measured.into_iter().rev(), allowance)?;
            if !rollup.l1_head.generation_is(snapshot.generation) {
                return Err(PolicyError::Unavailable(
                    "canonical history changed during posting-cap validation".into(),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn cross_chain_da_overhead(
        rollup_id: u64,
        transactions: &[eez_protocol::blob_semantics::SemanticTransaction],
    ) -> Result<DaOverhead, PolicyError> {
        semantic_overhead(rollup_id, transactions)
    }

    pub(super) fn record_final_encoding(
        &self,
        rollup_id: u64,
        anchor: eez_l1::L2Endpoint,
        candidate: eez_l1::L2Endpoint,
        actual_bytes: usize,
    ) {
        let policy = &self.inner.policy;
        let Some(runtime) = policy.rollups.get(&rollup_id) else {
            return;
        };
        let estimate = runtime.lock().unwrap().estimate;
        let whole_pending_estimate = estimate.map(|(_, _, bytes)| bytes);
        let cached = estimate.filter(|(a, c, _)| *a == anchor && *c == candidate);
        let estimated_bytes = cached.map_or(actual_bytes as u64, |(_, _, bytes)| bytes);
        event!(name: "eez.composer.settlement.encoding_estimate", Level::INFO,
            event_name = "eez.composer.settlement.encoding_estimate", rollup_id,
            from = anchor.number, to = candidate.number, candidate_hash = %candidate.hash,
            estimate_source = if cached.is_some() { "incremental_candidate" } else { "exact_selected_prefix" },
            estimated_bytes, actual_bytes,
            estimate_error_bytes = actual_bytes as i128 - i128::from(estimated_bytes),
            whole_pending_estimate = ?whole_pending_estimate,
            "compared meaningful stream estimate with final encoding, excluding physical padding");
    }

    pub(super) fn cached_block_rlp(&self, rollup_id: u64, hash: B256) -> Option<Vec<u8>> {
        self.inner
            .policy
            .rollups
            .get(&rollup_id)?
            .lock()
            .unwrap()
            .block_rlps
            .get(&hash)
            .map(|(_, rlp)| rlp.clone())
    }

    pub(super) fn historical_prefix_bytes(
        &self,
        rollup_id: u64,
        endpoint: eez_l1::L2Endpoint,
    ) -> Result<Option<u64>, String> {
        let runtime = self
            .inner
            .policy
            .rollups
            .get(&rollup_id)
            .ok_or("unknown policy rollup")?
            .lock()
            .unwrap();
        // Prefix sums start at the index anchor, so both it and the policy
        // anchor must still be the validated canonical endpoint.
        let canonical = self.inner.rollups[&rollup_id]
            .l1_head
            .policy_anchor_snapshot()
            .endpoint;
        if canonical != runtime.anchor.map(|anchor| anchor.endpoint)
            || canonical != runtime.activity.anchor()
        {
            return Err("canonical anchor changed before historical prefix sizing".into());
        }
        runtime
            .activity
            .historical_stream_bytes(endpoint)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    pub(super) fn latch_historical_predecessor(
        &self,
        rollup_id: u64,
        rollup: &RollupState<L2>,
        sync_height: u64,
        groups: &[HeldTx],
    ) -> Result<(), String> {
        let runtime = self
            .inner
            .policy
            .rollups
            .get(&rollup_id)
            .ok_or("unknown policy rollup")?;
        let mut runtime = runtime.lock().unwrap();
        if runtime.predecessor.as_ref().is_some_and(|previous| {
            previous
                .groups
                .iter()
                .any(|hash| groups.iter().any(|group| group.hash == *hash))
        }) {
            return Ok(());
        }
        let cursor = rollup.l1_head.cursor();
        let snapped = sync_height
            .checked_sub(u64::from(self.inner.emission.timing.k()))
            .ok_or("no historical predecessor before the first Sync boundary")?;
        if snapped <= cursor {
            return Err("no historical predecessor above the validated anchor".into());
        }
        let boundary = self
            .empty_chunk_boundaries(rollup, cursor, snapped)?
            .into_iter()
            .next()
            .ok_or("no progressing empty historical predecessor")?;
        let header = rollup
            .l2_provider
            .sealed_header(boundary)
            .map_err(|error| error.to_string())?
            .ok_or("historical predecessor header unavailable")?;
        let endpoint = eez_l1::L2Endpoint {
            number: boundary,
            hash: header.hash(),
        };
        let groups: Vec<_> = groups.iter().map(|group| group.hash).collect();
        event!(name: "eez.composer.settlement.predecessor", Level::INFO,
            event_name = "eez.composer.settlement.predecessor", rollup_id,
            required_endpoint = ?endpoint, groups = ?groups,
            "actual capacity requires a finite historical predecessor for these admitted groups");
        runtime.predecessor = Some(HistoricalPredecessor { endpoint, groups });
        Ok(())
    }

    /// Earlier history no longer helps groups whose fresh validation is
    /// unavailable or whose individual composition cannot fit. Other groups
    /// can keep their already captured finite dependency.
    pub(super) fn cancel_historical_predecessor_groups(
        &self,
        rollup_id: u64,
        groups: impl IntoIterator<Item = B256>,
    ) {
        let Some(runtime) = self.inner.policy.rollups.get(&rollup_id) else {
            return;
        };
        let mut runtime = runtime.lock().unwrap();
        if let Some(predecessor) = &mut runtime.predecessor
            && !predecessor.cancel_groups(groups)
        {
            runtime.predecessor = None;
        }
    }

    pub(super) fn posting_blob_allowance(
        &self,
        target: BundleTarget,
        companion_blobs: u64,
    ) -> Result<usize, String> {
        let maximum = self.remaining_posting_blob_allowance(target, companion_blobs)?;
        if maximum == 0 {
            return Err(eez_blob_codec::BlobLimitError::CompanionsExhaustCapacity.to_string());
        }
        Ok(maximum)
    }

    pub(super) fn remaining_posting_blob_allowance(
        &self,
        target: BundleTarget,
        companion_blobs: u64,
    ) -> Result<usize, String> {
        let BundleTarget::Exact { timestamp, .. } = target else {
            return Err("posting target must be resolved before blob sizing".into());
        };
        self.resolved_blob_allowance(timestamp, companion_blobs, true)
    }

    pub(super) fn resolved_blob_allowance(
        &self,
        timestamp: u64,
        companion_blobs: u64,
        log: bool,
    ) -> Result<usize, String> {
        let policy = &self.inner.policy;
        let params = self
            .inner
            .cross_chain
            .local
            .l1_entry
            .chain_provider()
            .evm_config
            .chain_spec()
            .blob_params_at_timestamp(timestamp)
            .ok_or("target fork blob rules unavailable")?;
        let limits =
            eez_blob_codec::BlobLimits::from_params(params).map_err(|error| error.to_string())?;
        let maximum = match limits.outgoing(companion_blobs, u64::MAX, policy.max_blobs_per_batch) {
            Ok(maximum) => maximum,
            Err(eez_blob_codec::BlobLimitError::CompanionsExhaustCapacity) => 0,
            Err(error) => return Err(error.to_string()),
        };
        if log {
            event!(name: "eez.composer.settlement.blob_limits", Level::INFO,
            event_name = "eez.composer.settlement.blob_limits",
            source = "selected L1 chain profile at target timestamp",
            target_l1_timestamp = timestamp,
            network_transaction_max = params.max_blobs_per_tx,
            network_block_max = params.max_blob_count,
            required_companion_blobs = companion_blobs,
            transport_max = u64::MAX,
            operator_cap = ?policy.max_blobs_per_batch,
            effective_max = maximum,
            "resolved independent blob allowance terms");
        }
        usize::try_from(maximum)
            .map_err(|_| "blob allowance exceeds addressable transport capacity".into())
    }

    async fn canonical_policy_anchor(
        &self,
        rollup_id: u64,
        rollup: &RollupState<L2>,
    ) -> Result<
        (
            eez_l1::l1_canonical_head::PolicyAnchorSnapshot,
            SettlementAnchor,
        ),
        PolicyError,
    > {
        let runtime = self
            .inner
            .policy
            .rollups
            .get(&rollup_id)
            .ok_or_else(|| PolicyError::Unavailable("unknown rollup".into()))?;
        let snapshot = rollup.l1_head.policy_anchor_snapshot();
        if snapshot.through.is_none() {
            return Err(PolicyError::Unavailable(
                "canonical L1 scan is incomplete".into(),
            ));
        }
        let endpoint = snapshot.endpoint.ok_or_else(|| {
            PolicyError::Unavailable("validated L2 endpoint is unavailable".into())
        })?;
        let cached = runtime.lock().unwrap().anchor;
        let anchor = if let Some(commitment) = snapshot.commitment {
            if commitment.endpoint != endpoint {
                return Err(PolicyError::Unavailable(
                    "canonical commitment and scan endpoint disagree".into(),
                ));
            }
            if let Some(cached) = cached.filter(|cached| {
                cached.l1 == commitment.l1
                    && cached.tx_hash == commitment.tx_hash
                    && cached.tx_index == commitment.tx_index
                    && cached.endpoint == commitment.endpoint
            }) {
                cached
            } else {
                let timestamp = self
                    .inner
                    .submitter
                    .reader()
                    .canonical_timestamp(commitment.l1)
                    .await
                    .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
                SettlementAnchor {
                    l1: commitment.l1,
                    l1_timestamp: timestamp,
                    tx_hash: commitment.tx_hash,
                    tx_index: commitment.tx_index,
                    endpoint,
                }
            }
        } else if endpoint.number == 0 {
            if let Some(cached) = cached.filter(|cached| cached.endpoint == endpoint) {
                if let Err(error) = self
                    .inner
                    .submitter
                    .reader()
                    .canonical_timestamp(cached.l1)
                    .await
                {
                    runtime.lock().unwrap().anchor = None;
                    return Err(PolicyError::Unavailable(error.to_string()));
                }
                cached
            } else {
                let registration = self
                    .inner
                    .submitter
                    .reader()
                    .registration_anchor(endpoint.hash)
                    .await
                    .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
                SettlementAnchor {
                    l1: registration.commitment.l1,
                    l1_timestamp: registration.timestamp,
                    tx_hash: registration.commitment.tx_hash,
                    tx_index: registration.commitment.tx_index,
                    endpoint,
                }
            }
        } else {
            return Err(PolicyError::Unavailable(
                "settlement correlation has not been reconciled into canonical evidence".into(),
            ));
        };
        if !rollup.l1_head.generation_is(snapshot.generation) {
            return Err(PolicyError::Unavailable(
                "canonical anchor changed during lookup".into(),
            ));
        }
        runtime.lock().unwrap().anchor = Some(anchor);
        Ok((snapshot, anchor))
    }

    /// Rebase the byte index on the validated anchor and extend it through
    /// `parent`, caching each newly measured block encoding. Stale or orphaned
    /// encodings are pruned, including when the update fails.
    fn update_activity_index(
        &self,
        rollup: &RollupState<L2>,
        runtime: &mut RollupPolicy,
        endpoint: eez_l1::L2Endpoint,
        parent: eez_l1::L2Endpoint,
    ) -> Result<(), PolicyError> {
        let RollupPolicy {
            activity,
            block_rlps,
            ..
        } = runtime;
        let updated = activity.update(endpoint, parent, |hash| {
            let block = rollup
                .l2_provider
                .find_block_by_hash(hash, BlockSource::Any)
                .map_err(|error| {
                    PolicyError::Unavailable(format!("pending block {hash}: {error}"))
                })?
                .ok_or_else(|| PolicyError::Unavailable(format!("pending block {hash} missing")))?;
            let encoded = alloy_rlp::encode(&block);
            // Failed cross-chain Sync blocks remain behind the existing
            // recovery gate. Detect their system halves so they cannot be
            // mislabeled as ordinary transaction activity.
            let mut has_system = false;
            let transactions = block.body().transactions();
            for tx in transactions {
                use alloy_eips::Decodable2718 as _;
                use reth_primitives_traits::SignerRecoverable as _;
                let bytes = tx.encoded_2718();
                let tx =
                    eez_primitives::EezTxEnvelope::decode_2718_exact(&bytes).map_err(|error| {
                        PolicyError::Unavailable(format!("pending transaction decoding: {error}"))
                    })?;
                if !tx.is_history()
                    && tx.recover_signer().map_err(|error| {
                        PolicyError::Unavailable(format!("pending signer: {error}"))
                    })? == eez_primitives::SYSTEM_ADDRESS
                {
                    has_system = true;
                    break;
                }
            }
            let parent = rollup
                .l2_provider
                .sealed_header_by_hash(block.header().parent_hash())
                .map_err(|error| PolicyError::Unavailable(error.to_string()))?
                .ok_or_else(|| PolicyError::Unavailable("ordinary sizing parent missing".into()))?;
            let ordinary = measure_ordinary(&self.inner.evm_config, &parent, &encoded)?;
            block_rlps.insert(hash, (block.header().number(), encoded));
            Ok(BlockActivity {
                endpoint: eez_l1::L2Endpoint {
                    number: block.header().number(),
                    hash,
                },
                parent_hash: block.header().parent_hash(),
                ordinary,
                pure_transactions: if has_system {
                    0
                } else {
                    u64::try_from(
                        transactions
                            .iter()
                            .filter(|tx| !eez_primitives::is_native_type(tx.ty()))
                            .count(),
                    )
                    .map_err(|_| PolicyError::Arithmetic("included pure transactions"))?
                },
            })
        });
        // New blocks append without scanning the existing backlog. Reorgs,
        // anchor changes and failed reads can leave stale/orphan encodings;
        // prune those against the index before returning, including on error.
        if !updated.as_ref().is_ok_and(|invalidated| !*invalidated) {
            block_rlps.retain(|hash, (number, _)| {
                activity
                    .endpoint_at(*number)
                    .is_some_and(|block| block.number > endpoint.number && block.hash == *hash)
            });
        }
        updated.map(|_| ())
    }

    /// Historical prefixes are sized from the byte index, whose prefix sums
    /// start at its anchor. Each landed prefix advances canonical settlement,
    /// and the block-cap path does not run policy evaluation, so refresh both
    /// the anchor and the index here before selecting the next prefix.
    pub(super) async fn refresh_historical_index(
        &self,
        rollup_id: u64,
        rollup: &RollupState<L2>,
        parent: &reth_primitives_traits::SealedHeader<alloy_consensus::Header>,
    ) -> Result<(), String> {
        let (snapshot, anchor) = self
            .canonical_policy_anchor(rollup_id, rollup)
            .await
            .map_err(|error| error.to_string())?;
        let runtime = self
            .inner
            .policy
            .rollups
            .get(&rollup_id)
            .ok_or("unknown policy rollup")?;
        self.update_activity_index(
            rollup,
            &mut runtime.lock().unwrap(),
            anchor.endpoint,
            eez_l1::L2Endpoint {
                number: parent.number(),
                hash: parent.hash(),
            },
        )
        .map_err(|error| error.to_string())?;
        if !rollup.l1_head.generation_is(snapshot.generation) {
            return Err("canonical evidence changed while refreshing the historical index".into());
        }
        Ok(())
    }

    pub(super) async fn evaluate_settlement_policy(
        &self,
        rollup_id: u64,
        rollup: &RollupState<L2>,
        parent: &reth_primitives_traits::SealedHeader<alloy_consensus::Header>,
        built: &crate::local::BuiltSyncBlock,
        target: BundleTarget,
        cross_chain: CrossChainWork,
        overhead: DaOverhead,
    ) -> Result<PolicyDecision, PolicyError> {
        // Errors or cancelled evaluations must not leave a previous successful
        // decision looking like the outcome of this opportunity.
        if let Some(runtime) = self.inner.policy.rollups.get(&rollup_id) {
            runtime.lock().unwrap().observation = None;
        }
        let (snapshot, anchor) = self.canonical_policy_anchor(rollup_id, rollup).await?;
        let endpoint = anchor.endpoint;
        let policy = &self.inner.policy;
        let runtime = policy
            .rollups
            .get(&rollup_id)
            .expect("resolved policy rollup");
        let BundleTarget::Exact {
            timestamp: target_l1_timestamp,
            ..
        } = target
        else {
            return Err(PolicyError::Unavailable(
                "policy target must be resolved before evaluation".into(),
            ));
        };
        let effective_blobs = u64::try_from(
            self.posting_blob_allowance(target, 0)
                .map_err(PolicyError::Unavailable)?,
        )
        .map_err(|_| PolicyError::Arithmetic("effective blob allowance"))?;

        let mut runtime = runtime.lock().unwrap();
        self.update_activity_index(
            rollup,
            &mut runtime,
            endpoint,
            eez_l1::L2Endpoint {
                number: parent.number(),
                hash: parent.hash(),
            },
        )?;
        // Resolve the captured hash against this exact pending branch. The
        // held pool includes fresh users that have not yet entered an attempt.
        if let Some(mut predecessor) = runtime.predecessor.take() {
            let canonical = runtime.activity.endpoint_at(predecessor.endpoint.number);
            if predecessor.reconcile(endpoint, canonical, |hash| rollup.held_pool.owns(hash)) {
                runtime.predecessor = Some(predecessor);
            }
        }
        let cross_chain = match cross_chain {
            CrossChainWork::None => runtime
                .predecessor
                .as_ref()
                .map_or(CrossChainWork::None, |intent| {
                    CrossChainWork::Predecessor(intent.endpoint)
                }),
            work => work,
        };
        let drain_coverage = match runtime.state.drain() {
            None => DrainCoverage::Pending,
            Some(drain) => {
                let canonical = if drain.endpoint.number <= endpoint.number {
                    rollup
                        .l2_provider
                        .sealed_header(drain.endpoint.number)
                        .map_err(|error| {
                            PolicyError::Unavailable(format!("drain coverage: {error}"))
                        })?
                        .map(|header| eez_l1::L2Endpoint {
                            number: header.number(),
                            hash: header.hash(),
                        })
                } else if drain.endpoint.number == built.header.number() {
                    Some(eez_l1::L2Endpoint {
                        number: built.header.number(),
                        hash: built.header.hash(),
                    })
                } else {
                    runtime.activity.endpoint_at(drain.endpoint.number)
                };
                match canonical {
                    Some(canonical) if canonical != drain.endpoint => DrainCoverage::Replaced,
                    Some(_)
                        if drain.endpoint.number <= endpoint.number
                            && rollup.optimistic.blocking_height().is_none() =>
                    {
                        DrainCoverage::Covered
                    }
                    _ => DrainCoverage::Pending,
                }
            }
        };
        let terminal_size =
            eez_driver::derivation::operations_size(&built.block.clone().into_block())
                .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
        let logical_blocks = built
            .header
            .number()
            .checked_sub(endpoint.number)
            .ok_or(PolicyError::Arithmetic("candidate window count"))?;
        let window_limit_due = logical_blocks
            .checked_add(u64::from(self.inner.emission.timing.k()))
            .is_none_or(|next| next > self.inner.emission.max_blocks);
        let estimated_stream_bytes = if logical_blocks <= self.inner.emission.max_blocks {
            Some(
                runtime
                    .activity
                    .estimated_stream_bytes_with_overhead(&terminal_size, overhead)?,
            )
        } else {
            None
        };
        let latest_pure_block = runtime.activity.latest_pure();
        let pending_pure_l2_transactions = runtime.activity.pending_pure_transactions()?;
        if !rollup.l1_head.generation_is(snapshot.generation) {
            return Err(PolicyError::Unavailable(
                "canonical evidence changed during evaluation".into(),
            ));
        }
        runtime.anchor = Some(anchor);
        runtime.estimate = estimated_stream_bytes.map(|bytes| {
            (
                endpoint,
                eez_l1::L2Endpoint {
                    number: built.header.number(),
                    hash: built.header.hash(),
                },
                bytes,
            )
        });
        let decision = runtime.state.evaluate(
            &policy.config,
            PolicySnapshot {
                anchor,
                candidate: eez_l1::L2Endpoint {
                    number: built.header.number(),
                    hash: built.header.hash(),
                },
                target_l1_timestamp,
                latest_pure_block,
                cross_chain,
                estimated_stream_bytes,
                window_limit_due,
                effective_blobs,
                usable_bytes_per_blob: eez_blob_codec::DATA_BYTES_PER_BLOB as u64,
                drain_coverage,
            },
        )?;
        runtime.observation = Some((
            snapshot.generation,
            SettlementPolicyObservation {
                decision,
                anchor,
                target_l1_timestamp,
                block_threshold: policy.config.max_unsettled_l2_blocks.get(),
                pure_interval_deadline_ms: interval_deadline(
                    anchor.l1_timestamp,
                    policy.config.pure_l2,
                    latest_pure_block.is_some(),
                ),
                evaluated_at_ms: crate::prover_retry::unix_time_millis(),
            },
        ));
        event!(name: "eez.composer.settlement.decision", Level::INFO, rollup_id,
            event_name = "eez.composer.settlement.decision",
            reasons = ?decision.reasons, due = decision.is_due(), required_endpoint = ?decision.required_endpoint,
            unsettled_blocks = decision.unsettled_blocks, settlement_age_ms = decision.settlement_age_ms,
            pending_pure_l2_transactions,
            anchor_l2_number = anchor.endpoint.number, anchor_l2_hash = %anchor.endpoint.hash,
            anchor_l1_number = anchor.l1.number, anchor_l1_hash = %anchor.l1.hash,
            last_settlement_timestamp = anchor.l1_timestamp,
            overdue_blocks = decision.unsettled_blocks.saturating_sub(policy.config.max_unsettled_l2_blocks.get()),
            overdue_interval_ms = match policy.config.pure_l2 {
                crate::settlement_policy::PureL2Mode::Interval { milliseconds } if latest_pure_block.is_some() =>
                    decision.settlement_age_ms.saturating_sub(milliseconds.get()),
                _ => 0,
            },
            fullness_threshold_bps = ?policy.config.blob_fullness_bps,
            usable_bytes_per_blob = eez_blob_codec::DATA_BYTES_PER_BLOB,
            estimated_stream_bytes = ?estimated_stream_bytes, effective_blobs, target_l1_timestamp,
            "settlement policy evaluated against canonical evidence");
        Ok(decision)
    }
}

fn measure_ordinary(
    config: &EezEvmConfig,
    parent: &reth_primitives_traits::SealedHeader,
    encoded: &[u8],
) -> Result<eez_payload_codec::v0::PayloadSizer, PolicyError> {
    let block: eez_primitives::Block = alloy_rlp::decode_exact(encoded)
        .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
    eez_driver::derivation::full_attributes(config, parent, &block)
        .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
    eez_driver::derivation::operations_size(&block)
        .map_err(|error| PolicyError::Unavailable(error.to_string()))
}

/// Validate every indivisible, already accepted segment. An unfinished slot's
/// retained bytes are checked too; future blocks still undergo exact final
/// packing. Startup cannot forecast transactions or headers not produced yet.
fn validate_retained_byte_segments(
    blocks: impl IntoIterator<
        Item = (
            eez_l1::L2Endpoint,
            eez_payload_codec::v0::PayloadSizer,
            bool,
        ),
    >,
    allowance: usize,
) -> Result<(), PolicyError> {
    let capacity = allowance
        .checked_mul(eez_blob_codec::DATA_BYTES_PER_BLOB)
        .ok_or(PolicyError::Arithmetic("startup posting capacity"))?;
    let mut ordinary = eez_payload_codec::v0::PayloadSizer::default();
    for (endpoint, next, boundary) in blocks {
        let combined = ordinary
            .combine(&next)
            .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
        let payload = combined
            .encoded_len()
            .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
        let size = eez_blob_codec::native_stream_len(payload, 0)
            .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
        if size > capacity {
            return Err(PolicyError::Config(format!(
                "EEZ_MAX_BLOBS_PER_BATCH with the target fork permits {allowance} blobs, but retained unsplittable history through {} ({}) already needs {} bytes; increase/remove the operator cap before restarting",
                endpoint.number, endpoint.hash, size,
            )));
        }
        ordinary = if boundary {
            eez_payload_codec::v0::PayloadSizer::default()
        } else {
            combined
        };
    }
    Ok(())
}

/// Exact native semantic encoding, shared by admission and fee estimation.
pub(super) fn semantic_overhead(
    rollup_id: u64,
    transactions: &[eez_protocol::blob_semantics::SemanticTransaction],
) -> Result<DaOverhead, PolicyError> {
    let messages = transactions
        .iter()
        .map(eez_protocol::blob_semantics::encode_semantic_transaction)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| PolicyError::Unavailable(error.to_string()))?
        .into_iter()
        .flatten()
        .collect();
    let encoded = eez_blob_codec::encode_native_stream(rollup_id, Bytes::new(), messages)
        .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
    let semantic_bytes = encoded
        .blob_portion
        .len()
        .checked_sub(
            eez_blob_codec::native_stream_len(0, 0)
                .map_err(|error| PolicyError::Unavailable(error.to_string()))?,
        )
        .ok_or(PolicyError::Arithmetic("semantic stream overhead"))?;
    Ok(DaOverhead { semantic_bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_deadline_is_conditional_and_uses_the_canonical_anchor() {
        use crate::settlement_policy::PureL2Mode;
        let interval = PureL2Mode::Interval {
            milliseconds: 15_000.try_into().unwrap(),
        };
        assert_eq!(interval_deadline(100, interval, true), Some(115_000));
        assert_eq!(interval_deadline(110, interval, true), Some(125_000));
        assert_eq!(interval_deadline(100, interval, false), None);
        assert_eq!(interval_deadline(100, PureL2Mode::Always, true), None);
        assert_eq!(interval_deadline(u64::MAX, interval, true), None);
    }

    #[test]
    fn telemetry_rejects_incomplete_or_changed_canonical_evidence() {
        use eez_l1::{L1BlockRef, L2Endpoint, l1_canonical_head::PolicyAnchorSnapshot};
        let endpoint = L2Endpoint {
            number: 10,
            hash: B256::repeat_byte(10),
        };
        let l1 = L1BlockRef {
            number: 100,
            hash: B256::repeat_byte(100),
        };
        let canonical = PolicyAnchorSnapshot {
            generation: 1,
            through: Some(l1),
            endpoint: Some(endpoint),
            commitment: None,
        };
        let mut runtime = RollupPolicy::default();
        assert!(runtime.observed_policy(canonical).is_none());
        runtime.observation = Some((
            1,
            SettlementPolicyObservation {
                decision: PolicyDecision {
                    reasons: crate::settlement_policy::DueReasons::GENERAL,
                    required_endpoint: Some(endpoint),
                    unsettled_blocks: 150,
                    settlement_age_ms: 1000,
                },
                anchor: SettlementAnchor {
                    l1,
                    endpoint,
                    l1_timestamp: 1000,
                    tx_hash: B256::ZERO,
                    tx_index: 0,
                },
                target_l1_timestamp: 1005,
                block_threshold: 150,
                pure_interval_deadline_ms: None,
                evaluated_at_ms: 1000,
            },
        ));
        assert!(
            runtime
                .observed_policy(canonical)
                .unwrap()
                .decision
                .is_due()
        );
        for changed in [
            PolicyAnchorSnapshot {
                generation: 2,
                ..canonical
            },
            PolicyAnchorSnapshot {
                through: None,
                ..canonical
            },
            PolicyAnchorSnapshot {
                endpoint: Some(L2Endpoint {
                    hash: B256::repeat_byte(11),
                    ..endpoint
                }),
                ..canonical
            },
        ] {
            assert!(runtime.observed_policy(changed).is_none());
        }
        runtime.observation = None; // A new evaluation clears its previous outcome before awaiting.
        assert!(runtime.observed_policy(canonical).is_none());
    }

    #[test]
    fn predecessor_tracks_live_groups_exact_hash_and_only_its_finite_range() {
        let endpoint = |number| eez_l1::L2Endpoint {
            number,
            hash: B256::with_last_byte(number as u8),
        };
        let first = B256::with_last_byte(201);
        let second = B256::with_last_byte(202);
        let mut intent = HistoricalPredecessor {
            endpoint: endpoint(24),
            groups: vec![first, second],
        };
        assert!(intent.cancel_groups([B256::ZERO]));
        assert!(intent.cancel_groups([first]));
        assert_eq!(intent.groups, vec![second]);
        assert_eq!(intent.endpoint, endpoint(24));
        assert!(!intent.cancel_groups([second]));
        intent.groups = vec![first, second];
        assert!(intent.reconcile(endpoint(12), Some(endpoint(24)), |_| true));
        assert!(intent.reconcile(endpoint(18), Some(endpoint(24)), |hash| hash == second));
        assert_eq!(intent.endpoint, endpoint(24));
        assert_eq!(intent.groups, vec![second]);
        assert!(!intent.reconcile(endpoint(18), Some(endpoint(24)), |_| false));
        let mut intent = HistoricalPredecessor {
            endpoint: endpoint(24),
            groups: vec![first],
        };
        assert!(!intent.reconcile(endpoint(24), Some(endpoint(24)), |_| true));
        assert!(!intent.reconcile(
            endpoint(18),
            Some(eez_l1::L2Endpoint {
                number: 24,
                hash: B256::with_last_byte(99),
            }),
            |_| true
        ));
        assert!(!intent.reconcile(endpoint(18), None, |_| true));
    }

    #[test]
    fn startup_cap_checks_unsplittable_history_and_respects_existing_empty_boundaries() {
        let block = |number, boundary| {
            (
                eez_l1::L2Endpoint {
                    number,
                    hash: B256::with_last_byte(number as u8),
                },
                eez_payload_codec::v0::PayloadSizer::run(&eez_payload_codec::v0::BlockRun {
                    blocks: 1,
                    beneficiary: Default::default(),
                    extra_data: &[],
                    transactions: &[vec![0; 100_000].into()],
                })
                .unwrap(),
                boundary,
            )
        };
        assert!(validate_retained_byte_segments([block(1, false), block(2, true)], 1).is_err());
        assert!(validate_retained_byte_segments([block(1, true), block(2, true)], 1).is_ok());
        assert!(validate_retained_byte_segments([block(1, false), block(2, true)], 2).is_ok());
        assert!(
            validate_retained_byte_segments([block(1, false), block(2, false)], 1).is_err(),
            "a not-yet-complete slot cannot hide retained bytes already over the cap"
        );
        assert!(validate_retained_byte_segments([block(1, false)], usize::MAX).is_err());
    }
}
