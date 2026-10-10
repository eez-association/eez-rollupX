//! Composer-specific retry policy and actionable proof-failure recovery.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy_primitives::{B256, Bytes};
use alloy_sol_types::SolValue as _;
use eez_driver::RollupTiming;
use eez_l1::BundleTarget;
use eez_prover::{
    ActionableProverFailure, Prover, ProverError, ProvingContext, RetryableProverError,
};
use reth_primitives_traits::SignedTransaction;
use tokio::time::{Instant, sleep_until, timeout};
use tracing::{Level, event};

use crate::{Direction, HeldTx};

const INITIAL_PROVER_RETRY_BACKOFF_MS: u64 = 100;
const MAX_PROVER_RETRY_BACKOFF_MS: u64 = 1_000;

/// Resolve a request-validated proof failure to the held transaction that
/// produced it.
pub(crate) fn actionable_held_tx<'a>(
    failure: ActionableProverFailure,
    survivors: &'a [HeldTx],
    outbound_entry_count: usize,
    inbound_compositions: &[(eez_protocol::Composition, B256)],
) -> Option<&'a HeldTx> {
    let (direction, held_hash) = match failure {
        ActionableProverFailure::Outbound {
            transaction_hash, ..
        } => (Direction::Outbound, transaction_hash),
        ActionableProverFailure::Inbound { entry_index, .. } => {
            // PostBatch order is `[anchor | outbound... | inbound transaction
            // entries...]`. Every deferred entry belongs to exactly one held
            // L1 transaction, including lookup-only entries.
            let mut batch_index = 1 + outbound_entry_count;
            let mut owner = None;
            for (composition, held_hash) in inbound_compositions {
                let entry_count = composition.source.batch.entries.len();
                if (batch_index..batch_index + entry_count).contains(&entry_index) {
                    owner = Some(*held_hash);
                    break;
                }
                batch_index += entry_count;
            }
            (Direction::Inbound, owner?)
        }
    };
    survivors
        .iter()
        .find(|tx| tx.direction == direction && tx.hash == held_hash)
}

/// Resolve a locally replayed observation to the inbound transaction whose
/// composition produced it. Unlike prover hints, this index is created and
/// consumed entirely inside the Composer.
pub(crate) fn observation_held_tx<'a>(
    composition_index: usize,
    survivors: &'a [HeldTx],
    inbound_compositions: &[(eez_protocol::Composition, B256)],
) -> Option<&'a HeldTx> {
    let held_hash = inbound_compositions.get(composition_index)?.1;
    survivors
        .iter()
        .find(|tx| tx.direction == Direction::Inbound && tx.hash == held_hash)
}

pub(crate) fn partition_retryable(
    survivors: Vec<HeldTx>,
    poison: &HeldTx,
) -> (Vec<HeldTx>, Vec<HeldTx>) {
    survivors.into_iter().partition(|tx| {
        tx.sender != poison.sender || tx.direction != poison.direction || tx.nonce < poison.nonce
    })
}

/// Verify both references in an actionable failure against the exact request
/// before allowing Composer state to change.
///
/// An honest, version-compatible prover answering the current request should
/// always pass this gate. Failure means the details are malformed, stale,
/// cross-request, malicious, or disagree with Composer's request encoding; in
/// every case the hint remains non-actionable. The caller may count the request
/// as an opaque prover rejection, but must not use the hint to select pool entries.
pub(crate) fn validate_actionable_prover_failure(
    failure: ActionableProverFailure,
    batch: &eez_protocol::EvmBatch,
    sync_block: Option<&reth_primitives_traits::RecoveredBlock<eez_primitives::Block>>,
) -> Result<(), String> {
    match failure {
        ActionableProverFailure::Outbound {
            transaction_index,
            transaction_hash,
        } => {
            let block =
                sync_block.ok_or("outbound proof failure has no in-memory terminal Sync block")?;
            let transaction = block
                .body()
                .transactions()
                .nth(transaction_index)
                .ok_or_else(|| {
                    format!(
                        "outbound proof failure transaction index {transaction_index} is out of range"
                    )
                })?;
            let actual = transaction.recalculate_hash();
            if actual != transaction_hash {
                return Err(format!(
                    "outbound proof failure hash {transaction_hash} does not match transaction \
                     {transaction_index} hash {actual}"
                ));
            }
        }
        ActionableProverFailure::Inbound {
            entry_index,
            entry_hash,
        } => {
            let entry = batch.entries.get(entry_index).ok_or_else(|| {
                format!("inbound proof failure entry index {entry_index} is out of range")
            })?;
            let actual = alloy_primitives::keccak256(entry.abi_encode());
            if actual != entry_hash {
                return Err(format!(
                    "inbound proof failure hash {entry_hash} does not match entry {entry_index} \
                     hash {actual}"
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn unix_time_millis() -> u64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

/// Latest wall-clock millisecond at which a full proof attempt may start.
///
/// The Sequencer applies the same budget before entering the Composer. Repeating
/// it here closes the race introduced by retry backoff: the proof budget is
/// reserved before the relay-submission slack, so a retry cannot consume either.
pub(crate) fn proof_start_cutoff_ms(timing: RollupTiming, target: BundleTarget) -> Option<u64> {
    let BundleTarget::Exact { timestamp, .. } = target else {
        return None;
    };
    let proof_ms = u64::try_from(timing.proof_time().as_millis()).unwrap_or(u64::MAX);
    let slack_ms = u64::try_from(timing.submission_slack().as_millis()).unwrap_or(u64::MAX);
    Some(
        timestamp
            .saturating_mul(1_000)
            .saturating_sub(slack_ms)
            .saturating_sub(proof_ms),
    )
}

/// One exact-target window shared by preparation, proof and submission. The
/// monotonic bound never moves forward when the wall clock moves backwards;
/// forward clock corrections can only shorten the remaining target window.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PipelineDeadline {
    pub unix_ms: u64,
    monotonic: Instant,
}

impl PipelineDeadline {
    pub fn new(timing: RollupTiming, target: BundleTarget, now_ms: u64) -> Option<Self> {
        let BundleTarget::Exact { timestamp, .. } = target else {
            return None;
        };
        let slack = u64::try_from(timing.submission_slack().as_millis()).unwrap_or(u64::MAX);
        let unix_ms = timestamp.saturating_mul(1000).saturating_sub(slack);
        Some(Self {
            unix_ms,
            monotonic: Instant::now() + Duration::from_millis(unix_ms.saturating_sub(now_ms)),
        })
    }

    pub fn instant(self) -> Instant {
        Instant::now() + self.remaining()
    }

    pub fn remaining(self) -> Duration {
        self.monotonic
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(
                self.unix_ms.saturating_sub(unix_time_millis()),
            ))
    }
}

/// Bootstrap allowance for signed-envelope construction, two ordered native
/// preflights and relay delivery. Observed finalization samples may raise it.
/// This is separate from relay slack, which remains outside the usable window.
pub(crate) const POST_PROOF_RESERVE: Duration = Duration::from_millis(150);
pub(crate) const DELIVERY_RESERVE: Duration = Duration::from_millis(25);

/// Work dimensions bound reuse and upward scaling of a timing observation.
/// Samples are local to the running binary/prover and disappear on restart.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Workload {
    pub groups: usize,
    pub blobs: usize,
    pub witnesses: usize,
}

#[derive(Default, Debug)]
struct TimingHistory(std::sync::Mutex<std::collections::VecDeque<(Workload, Duration)>>);
impl TimingHistory {
    fn observe(&self, workload: Workload, elapsed: Duration) {
        let mut samples = self.0.lock().unwrap();
        if samples.len() == 32 {
            samples.pop_front();
        }
        samples.push_back((workload, elapsed));
    }
    fn estimate(&self, workload: Workload) -> Option<Duration> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(sample, _)| {
                sample.groups >= workload.groups
                    && sample.blobs >= workload.blobs
                    && sample.witnesses >= workload.witnesses
            })
            .map(|(_, elapsed)| *elapsed)
            .max()
            .map(|d| d.saturating_add(d / 4))
    }

    fn extrapolate(&self, workload: Workload) -> Option<(Duration, Duration)> {
        let samples = self.0.lock().unwrap();
        let observed = samples.iter().map(|(_, elapsed)| *elapsed).max()?;
        let supplied = |sample: &Workload| {
            [
                sample.groups.min(workload.groups),
                sample.blobs.min(workload.blobs),
                sample.witnesses.min(workload.witnesses),
            ]
        };
        let scaled = samples
            .iter()
            .filter(|(sample, elapsed)| {
                let covered = supplied(sample);
                // A wider measured proof supersedes a cheaper sparse sample's
                // extrapolation once it supplies at least as much of every
                // requested dimension. Surplus old history is irrelevant to
                // this comparison, but unseen history/blob growth and more
                // expensive sparse proofs retain their own scaling terms.
                // Strict dominance preserves at least one of the 32 samples.
                !samples.iter().any(|(other, other_elapsed)| {
                    let other = supplied(other);
                    other_elapsed >= elapsed
                        && other != covered
                        && other.iter().zip(covered).all(|(a, b)| *a >= b)
                })
            })
            .map(|(sample, elapsed)| {
                [
                    (workload.groups, sample.groups),
                    (workload.blobs, sample.blobs),
                    (workload.witnesses, sample.witnesses),
                ]
                .into_iter()
                .filter(|(requested, observed)| *requested > (*observed).max(1))
                .map(|(requested, observed)| {
                    let nanos = elapsed
                        .as_nanos()
                        .checked_mul(requested as u128)
                        .map(|n| n.div_ceil(observed.max(1) as u128));
                    match nanos.filter(|n| *n <= Duration::MAX.as_nanos()) {
                        Some(n) => {
                            Duration::new((n / 1_000_000_000) as u64, (n % 1_000_000_000) as u32)
                        }
                        None => Duration::MAX,
                    }
                })
                .fold(*elapsed, Duration::max)
            })
            .max()
            .unwrap_or(observed)
            .max(observed);
        Some((
            scaled.saturating_add(scaled / 4),
            observed.saturating_add(observed / 4),
        ))
    }
}

pub(crate) struct ProofPrediction {
    pub duration: Duration,
    pub basis: &'static str,
}

/// Bounded histories with 25% headroom. Proofs outside observed coverage scale
/// non-dominated samples by the largest workload ratio, capped at the greater
/// of the bootstrap estimate and the observed proof floor. Dominance is relative
/// to the requested dimensions and cannot discard a more expensive sample.
/// Scaling never prices a larger request below an observed slower proof.
/// This avoids returning to a prohibitive cold allocation for each new witness
/// count while keeping the shared deadline authoritative. Finalization requires
/// covering samples because envelope/preflight cost need not scale like proof work.
#[derive(Default, Debug)]
pub(crate) struct PipelineTimings {
    proof: TimingHistory,
    finalization: TimingHistory,
}
impl PipelineTimings {
    pub fn proof_estimate(&self, configured: Duration, workload: Workload) -> Duration {
        self.proof_prediction(configured, workload).duration
    }
    pub fn proof_prediction(&self, configured: Duration, workload: Workload) -> ProofPrediction {
        let (duration, basis) = if let Some(estimate) = self.proof.estimate(workload) {
            (estimate, "covering_sample")
        } else if let Some((estimate, observed_floor)) = self.proof.extrapolate(workload) {
            (
                estimate.min(configured.max(observed_floor)),
                "scaled_sample",
            )
        } else {
            return ProofPrediction {
                duration: configured,
                basis: "configured_seed",
            };
        };
        ProofPrediction {
            duration: duration.max(Duration::from_millis(25)),
            basis,
        }
    }
    pub fn post_reserve(&self, workload: Workload) -> Duration {
        self.finalization
            .estimate(workload)
            .map(|d| d.saturating_add(DELIVERY_RESERVE))
            .unwrap_or(POST_PROOF_RESERVE)
            .max(POST_PROOF_RESERVE)
    }
    pub fn observe_proof(&self, workload: Workload, elapsed: Duration) {
        self.proof.observe(workload, elapsed);
    }
    pub fn observe_finalization(&self, workload: Workload, elapsed: Duration) {
        self.finalization.observe(workload, elapsed);
    }
}

pub(crate) async fn prove_with_shared_deadline(
    prover: &dyn Prover,
    ctx: ProvingContext,
    timing: RollupTiming,
    target: BundleTarget,
    deadline: PipelineDeadline,
    reserve: Duration,
) -> Result<Bytes, ProverError> {
    let budget = deadline.remaining().saturating_sub(reserve);
    if budget.is_zero() {
        return Err(ProverError::Retryable {
            kind: RetryableProverError::DeadlineExceeded,
            message: "shared settlement window exhausted before proving".into(),
        });
    }
    prove_with_retry_window(
        prover,
        ctx,
        timing,
        target,
        budget,
        Some((deadline, reserve)),
    )
    .await
}

/// Exponential backoff with ±20% wall-clock jitter and a fixed cap.
fn prover_retry_backoff(failed_attempt: u32) -> Duration {
    let jitter_seed = u64::from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos(),
    );
    prover_retry_backoff_with_seed(failed_attempt, jitter_seed)
}

fn prover_retry_backoff_with_seed(failed_attempt: u32, jitter_seed: u64) -> Duration {
    let shift = failed_attempt.saturating_sub(1).min(31);
    let base_ms = (INITIAL_PROVER_RETRY_BACKOFF_MS << shift).min(MAX_PROVER_RETRY_BACKOFF_MS);
    let jitter = base_ms / 5;
    let offset = jitter_seed % (jitter * 2 + 1);
    Duration::from_millis(base_ms - jitter + offset)
}

fn retry_budget(
    timing: RollupTiming,
    target: BundleTarget,
    now_ms: u64,
) -> Result<Duration, ProverError> {
    match proof_start_cutoff_ms(timing, target) {
        Some(cutoff_ms) if now_ms > cutoff_ms => Err(ProverError::Retryable {
            kind: RetryableProverError::DeadlineExceeded,
            message: "Composer proof-attempt cutoff reached".to_owned(),
        }),
        Some(cutoff_ms) => Ok(Duration::from_millis(cutoff_ms - now_ms)),
        None => Ok(timing.proof_time()),
    }
}

/// Run one complete proving operation with the Composer profile's closed
/// retryable-error allowlist. Every attempt is independently bounded by the
/// configured proof-time budget.
pub(crate) async fn prove_with_retry_at(
    prover: &dyn Prover,
    ctx: ProvingContext,
    timing: RollupTiming,
    target: BundleTarget,
    now_ms: u64,
) -> Result<Bytes, ProverError> {
    let budget = retry_budget(timing, target, now_ms)?;
    prove_with_retry_window(prover, ctx, timing, target, budget, None).await
}

async fn prove_with_retry_window(
    prover: &dyn Prover,
    ctx: ProvingContext,
    timing: RollupTiming,
    target: BundleTarget,
    budget: Duration,
    shared: Option<(PipelineDeadline, Duration)>,
) -> Result<Bytes, ProverError> {
    let retry_deadline = Instant::now() + budget;
    let mut attempt = 1_u32;

    loop {
        let attempt_timeout = if let Some((deadline, reserve)) = shared {
            deadline
                .remaining()
                .saturating_sub(reserve)
                .min(retry_deadline.saturating_duration_since(Instant::now()))
        } else {
            match target {
                BundleTarget::NextBlock => retry_deadline.saturating_duration_since(Instant::now()),
                BundleTarget::Exact { .. } => timing.proof_time(),
            }
        };
        if shared.is_some() && attempt_timeout.is_zero() {
            return Err(ProverError::Retryable {
                kind: RetryableProverError::DeadlineExceeded,
                message: "shared settlement window exhausted".into(),
            });
        }
        let result = match timeout(attempt_timeout, prover.prove(ctx.clone())).await {
            Ok(result) => result,
            Err(_) => Err(ProverError::Retryable {
                kind: RetryableProverError::DeadlineExceeded,
                message: format!("Composer proof attempt exceeded its {attempt_timeout:?} budget"),
            }),
        };
        let error = match result {
            Ok(proof) => return Ok(proof),
            Err(error) => error,
        };
        let Some(kind) = error.retryable_kind() else {
            return Err(error);
        };

        let wake_at = Instant::now() + prover_retry_backoff(attempt);
        if wake_at >= retry_deadline {
            event!(
                name: "eez.composer.prover.retry_cutoff",
                Level::WARN,
                attempt,
                ?kind,
                ?target,
                "not retrying proof because its retry budget is exhausted",
            );
            return Err(error);
        }

        event!(
            name: "eez.composer.prover.retry",
            Level::WARN,
            attempt,
            next_attempt = attempt + 1,
            ?kind,
            ?target,
            "retrying complete proof request after transient failure",
        );
        sleep_until(wake_at).await;
        // The planned wake was safe, but the runtime may resume this task late.
        if Instant::now() >= retry_deadline {
            event!(
                name: "eez.composer.prover.retry_cutoff",
                Level::WARN,
                attempt,
                ?kind,
                ?target,
                "proof retry budget exhausted while waiting",
            );
            return Err(error);
        }
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use alloy_consensus::SignableTransaction as _;
    use alloy_primitives::{Address, TxHash};
    use async_trait::async_trait;

    use super::*;

    fn held(sender: Address, direction: Direction, nonce: u64, hash_byte: u8) -> HeldTx {
        HeldTx {
            raw_tx: Bytes::from(vec![hash_byte; 4]),
            hash: TxHash::repeat_byte(hash_byte),
            attempts: 0,
            excluded_slots: 0,
            max_fee_per_gas: u128::from(hash_byte),
            priority_fee_per_gas: u128::from(hash_byte),
            sender,
            nonce,
            direction,
        }
    }

    #[test]
    fn actionable_failures_resolve_direction_specific_held_identities() {
        fn composition(entry_count: usize) -> eez_protocol::Composition {
            eez_protocol::Composition {
                actions: Vec::new(),
                source: eez_protocol::SourceComposition {
                    l2_static_entries: Vec::new(),
                    rollup_id: eez_protocol::RollupId(1),
                    batch: eez_protocol::EvmBatch {
                        entries: vec![Default::default(); entry_count],
                        ..Default::default()
                    },
                    entry_traces: Vec::new(),
                    callback_entry_counts: Vec::new(),
                    outbound_event_hashes: Vec::new(),
                },
                targets: Vec::new(),
            }
        }

        let sender = Address::repeat_byte(0xc);
        let outbound = held(sender, Direction::Outbound, 1, 1);
        let inbound = held(sender, Direction::Inbound, 1, 2);
        let second_inbound = held(Address::repeat_byte(0xd), Direction::Inbound, 0, 3);
        let survivors = vec![outbound.clone(), inbound.clone(), second_inbound.clone()];
        let inbound_compositions = vec![
            (composition(2), inbound.hash),
            (composition(1), second_inbound.hash),
        ];

        let resolved = actionable_held_tx(
            ActionableProverFailure::Outbound {
                transaction_index: 3,
                transaction_hash: outbound.hash,
            },
            &survivors,
            2,
            &inbound_compositions,
        )
        .unwrap();
        assert_eq!(resolved.hash, outbound.hash);

        // [anchor, outbound 0, outbound 1, inbound 0, inbound 1, inbound 2]
        let resolved = actionable_held_tx(
            ActionableProverFailure::Inbound {
                entry_index: 4,
                entry_hash: B256::repeat_byte(0xee),
            },
            &survivors,
            2,
            &inbound_compositions,
        )
        .unwrap();
        assert_eq!(resolved.hash, inbound.hash);

        assert!(
            actionable_held_tx(
                ActionableProverFailure::Inbound {
                    entry_index: 2,
                    entry_hash: B256::repeat_byte(0xee),
                },
                &survivors,
                2,
                &inbound_compositions,
            )
            .is_none()
        );
    }

    #[test]
    fn actionable_failures_map_lookup_only_entries_to_their_held_transaction() {
        fn failed_composition() -> eez_protocol::Composition {
            eez_protocol::Composition {
                actions: vec![eez_protocol::ExecutedAction {
                    parent_index: None,
                    call_mode: eez_protocol::CallMode::Mutable,
                    target_address: Address::repeat_byte(0x22),
                    target_rollup_id: eez_protocol::RollupId(1),
                    source_rollup_id: eez_protocol::RollupId::MAINNET,
                    source_address: Address::repeat_byte(0x11),
                    data: Bytes::new(),
                    value: Default::default(),
                    outcome: eez_protocol::ExecutionOutcome::Resolved {
                        return_data: Vec::new(),
                        gas_used: 0,
                        success: false,
                    },
                    revert_span: None,
                }],
                source: eez_protocol::SourceComposition {
                    l2_static_entries: Vec::new(),
                    rollup_id: eez_protocol::RollupId::MAINNET,
                    batch: eez_protocol::EvmBatch {
                        entries: vec![Default::default()],
                        ..Default::default()
                    },
                    entry_traces: vec![Default::default()],
                    callback_entry_counts: vec![0],
                    outbound_event_hashes: Vec::new(),
                },
                targets: Vec::new(),
            }
        }

        fn successful_composition() -> eez_protocol::Composition {
            let mut composition = failed_composition();
            composition.actions[0].outcome = eez_protocol::ExecutionOutcome::Resolved {
                return_data: Vec::new(),
                gas_used: 0,
                success: true,
            };
            composition.source.batch.entries[0].success = true;
            composition
        }

        let first = held(Address::repeat_byte(0xa1), Direction::Inbound, 0, 1);
        let second = held(Address::repeat_byte(0xa2), Direction::Inbound, 0, 2);
        let survivors = vec![first.clone(), second.clone()];
        let compositions = vec![
            (failed_composition(), first.hash),
            (successful_composition(), second.hash),
        ];

        assert_eq!(
            actionable_held_tx(
                ActionableProverFailure::Inbound {
                    entry_index: 2,
                    entry_hash: B256::repeat_byte(0xee),
                },
                &survivors,
                0,
                &compositions,
            )
            .map(|held| held.hash),
            Some(second.hash),
        );
    }

    #[test]
    fn proof_ejection_removes_only_the_same_direction_nonce_suffix() {
        let sender = Address::repeat_byte(0xc);
        let poison = held(sender, Direction::Inbound, 2, 2);
        let survivors = vec![
            held(sender, Direction::Inbound, 1, 1),
            poison.clone(),
            held(sender, Direction::Inbound, 3, 3),
            held(sender, Direction::Outbound, 3, 4),
            held(Address::repeat_byte(0xd), Direction::Inbound, 3, 5),
        ];

        let (retry, evicted) = partition_retryable(survivors, &poison);

        assert_eq!(
            retry.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
            vec![
                TxHash::repeat_byte(1),
                TxHash::repeat_byte(4),
                TxHash::repeat_byte(5),
            ]
        );
        assert_eq!(
            evicted.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
            vec![TxHash::repeat_byte(2), TxHash::repeat_byte(3)]
        );
    }

    #[test]
    fn actionable_references_must_match_the_exact_proving_request() {
        let transaction: reth_ethereum_primitives::TransactionSigned =
            alloy_consensus::TxLegacy::default()
                .into_signed(alloy_primitives::Signature::test_signature())
                .into();
        let transaction_hash = transaction.recalculate_hash();
        let body: eez_primitives::BlockBody = alloy_consensus::BlockBody {
            transactions: vec![transaction.into()],
            ..Default::default()
        };
        let block = reth_primitives_traits::RecoveredBlock::new_unhashed(
            eez_primitives::Block::new(Default::default(), body),
            vec![Address::ZERO],
        );

        let mut batch = eez_protocol::EvmBatch::default();
        batch
            .entries
            .push(eez_protocol::abi::ExecutionEntrySol::default());
        let entry_hash = alloy_primitives::keccak256(batch.entries[0].abi_encode());

        assert!(
            validate_actionable_prover_failure(
                ActionableProverFailure::Outbound {
                    transaction_index: 0,
                    transaction_hash,
                },
                &batch,
                Some(&block),
            )
            .is_ok()
        );
        assert!(
            validate_actionable_prover_failure(
                ActionableProverFailure::Inbound {
                    entry_index: 0,
                    entry_hash,
                },
                &batch,
                Some(&block),
            )
            .is_ok()
        );
        assert!(
            validate_actionable_prover_failure(
                ActionableProverFailure::Outbound {
                    transaction_index: 0,
                    transaction_hash: B256::repeat_byte(0xff),
                },
                &batch,
                Some(&block),
            )
            .is_err()
        );
        assert!(
            validate_actionable_prover_failure(
                ActionableProverFailure::Inbound {
                    entry_index: 1,
                    entry_hash,
                },
                &batch,
                Some(&block),
            )
            .is_err()
        );
    }

    #[derive(Debug)]
    struct ScriptedProver {
        outcomes: Mutex<VecDeque<Result<Bytes, ProverError>>>,
        calls: AtomicUsize,
    }

    impl ScriptedProver {
        fn new(outcomes: Vec<Result<Bytes, ProverError>>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into()),
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl Prover for ScriptedProver {
        async fn prove(&self, _ctx: ProvingContext) -> Result<Bytes, ProverError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.outcomes
                .lock()
                .unwrap()
                .pop_front()
                .expect("scripted prover ran out of outcomes")
        }

        fn vkey(&self) -> B256 {
            B256::ZERO
        }
    }

    #[derive(Debug, Default)]
    struct TimeoutProver {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Prover for TimeoutProver {
        async fn prove(&self, _ctx: ProvingContext) -> Result<Bytes, ProverError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            std::future::pending().await
        }

        fn vkey(&self) -> B256 {
            B256::ZERO
        }
    }

    #[derive(Debug, Default)]
    struct AlwaysRetryableProver {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Prover for AlwaysRetryableProver {
        async fn prove(&self, _ctx: ProvingContext) -> Result<Bytes, ProverError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Err(retryable(RetryableProverError::Unavailable))
        }

        fn vkey(&self) -> B256 {
            B256::ZERO
        }
    }

    #[derive(Debug, Default)]
    struct DelayedFailureThenStallProver {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Prover for DelayedFailureThenStallProver {
        async fn prove(&self, _ctx: ProvingContext) -> Result<Bytes, ProverError> {
            if self.calls.fetch_add(1, Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(300)).await;
                return Err(retryable(RetryableProverError::Unavailable));
            }
            std::future::pending().await
        }

        fn vkey(&self) -> B256 {
            B256::ZERO
        }
    }

    fn retryable(kind: RetryableProverError) -> ProverError {
        ProverError::Retryable {
            kind,
            message: "transient test failure".to_owned(),
        }
    }

    fn test_context() -> ProvingContext {
        ProvingContext {
            parent_provenance: eez_protocol::l2::ParentProvenance {
                parent: Default::default(),
                accepted_l1_number: 0,
                live_slot_endpoints: Vec::new(),
            },
            rollup_id: 1,
            from_block: 1,
            to_block: 1,
            batch: Default::default(),
            blobs: Vec::new(),
            blocks: Vec::new().into(),
            l1_posting: eez_prover::L1PostingContext {
                parent_number: 99,
                parent_hash: B256::repeat_byte(0x42),
                chain_id: 1,
                target_timestamp: 1,
            },
        }
    }

    fn test_timing() -> RollupTiming {
        RollupTiming::new(2_000, 1_000, 500, 100, 1)
    }

    const TEST_NOW_MS: u64 = 1_700_000_000_000;

    #[test]
    fn measured_wider_proof_supersedes_sparse_scaling_for_covered_history() {
        let timings = PipelineTimings::default();
        let configured = Duration::from_millis(500);
        timings.observe_proof(
            Workload {
                groups: 1,
                blobs: 1,
                witnesses: 10,
            },
            Duration::from_millis(17),
        );
        timings.observe_proof(
            Workload {
                groups: 22,
                blobs: 1,
                witnesses: 5,
            },
            Duration::from_millis(43),
        );
        let request = Workload {
            groups: 110,
            blobs: 2,
            witnesses: 5,
        };
        assert_eq!(
            timings.proof_estimate(configured, request),
            Duration::from_micros(268_750),
            "the measured wider proof must replace the one-group scaling term"
        );
        assert_eq!(
            timings.proof_estimate(
                configured,
                Workload {
                    witnesses: 20,
                    ..request
                }
            ),
            configured,
            "a shorter history sample cannot replace the longer history observation"
        );
        timings.observe_proof(
            Workload {
                groups: 1,
                blobs: 3,
                witnesses: 5,
            },
            Duration::from_millis(40),
        );
        assert_eq!(
            timings.proof_estimate(
                configured,
                Workload {
                    blobs: 3,
                    ..request
                }
            ),
            configured,
            "a sample with more physical blobs remains relevant"
        );
    }

    #[test]
    fn expensive_sparse_proof_remains_a_scaling_observation() {
        let timings = PipelineTimings::default();
        timings.observe_proof(
            Workload {
                groups: 1,
                blobs: 1,
                witnesses: 10,
            },
            Duration::from_millis(600),
        );
        timings.observe_proof(
            Workload {
                groups: 22,
                blobs: 1,
                witnesses: 5,
            },
            Duration::from_millis(43),
        );
        assert_eq!(
            timings.proof_estimate(
                Duration::from_millis(500),
                Workload {
                    groups: 110,
                    blobs: 2,
                    witnesses: 5
                },
            ),
            Duration::from_millis(750),
            "a cheap wider sample cannot erase a slower sparse proof"
        );
    }

    #[test]
    fn proof_calibration_scales_each_dimension_and_retains_bounded_history() {
        let timings = PipelineTimings::default();
        let small = Workload {
            groups: 10,
            blobs: 1,
            witnesses: 5,
        };
        let configured = Duration::from_millis(450);
        assert_eq!(
            timings.proof_prediction(configured, small).basis,
            "configured_seed"
        );
        timings.observe_proof(small, Duration::from_millis(80));
        assert_eq!(
            timings.proof_prediction(configured, small).basis,
            "covering_sample"
        );
        assert_eq!(
            timings.proof_estimate(configured, small),
            Duration::from_millis(100)
        );
        for (large, expected_ms) in [
            (
                Workload {
                    groups: 11,
                    ..small
                },
                110,
            ),
            (Workload { blobs: 2, ..small }, 200),
            (
                Workload {
                    witnesses: 6,
                    ..small
                },
                120,
            ),
            (
                Workload {
                    groups: 20,
                    blobs: 3,
                    witnesses: 10,
                },
                300,
            ),
            (
                Workload {
                    groups: usize::MAX,
                    ..small
                },
                450,
            ),
        ] {
            let prediction = timings.proof_prediction(configured, large);
            assert_eq!(prediction.basis, "scaled_sample");
            assert_eq!(prediction.duration, Duration::from_millis(expected_ms));
        }
        timings.observe_proof(small, Duration::from_millis(400));
        for _ in 0..30 {
            timings.observe_proof(small, Duration::from_millis(80));
        }
        assert_eq!(
            timings.proof_estimate(configured, small),
            Duration::from_millis(500)
        );
        timings.observe_proof(small, Duration::from_millis(80));
        timings.observe_proof(small, Duration::from_millis(80));
        assert_eq!(timings.proof.0.lock().unwrap().len(), 32);
        assert_eq!(
            timings.proof_estimate(configured, small),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn proof_extrapolation_handles_zero_dimensions_and_saturates_overflow() {
        let timings = PipelineTimings::default();
        let configured = Duration::from_millis(450);
        timings.observe_proof(Workload::default(), Duration::from_millis(10));
        let workload = Workload {
            groups: 1,
            blobs: 2,
            witnesses: 0,
        };
        assert_eq!(
            timings.proof_estimate(configured, workload),
            Duration::from_millis(25)
        );
        timings.observe_proof(Workload::default(), Duration::MAX);
        let huge = Workload {
            groups: usize::MAX,
            ..workload
        };
        assert_eq!(timings.proof_estimate(configured, huge), Duration::MAX);
        // Covered slow observations remain authoritative even above the seed.
        assert_eq!(
            timings.proof_estimate(configured, Workload::default()),
            Duration::MAX
        );
        // Proof scaling does not train the finalization history.
        assert_eq!(timings.post_reserve(huge), POST_PROOF_RESERVE);
    }

    #[test]
    fn scaling_never_prices_larger_work_below_a_slow_observation() {
        let timings = PipelineTimings::default();
        let small = Workload {
            groups: 1,
            blobs: 1,
            witnesses: 2,
        };
        timings.observe_proof(small, Duration::from_millis(600));
        let configured = Duration::from_millis(450);
        assert_eq!(
            timings.proof_estimate(configured, small),
            Duration::from_millis(750)
        );
        assert_eq!(
            timings.proof_estimate(
                configured,
                Workload {
                    witnesses: 3,
                    ..small
                }
            ),
            Duration::from_millis(750)
        );
    }

    #[derive(Debug)]
    struct SlowSuccess {
        duration: Duration,
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Prover for SlowSuccess {
        async fn prove(&self, _ctx: ProvingContext) -> Result<Bytes, ProverError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            tokio::time::sleep(self.duration).await;
            Ok(Bytes::from_static(b"proof"))
        }
        fn vkey(&self) -> B256 {
            B256::ZERO
        }
    }

    fn virtual_deadline(remaining_ms: u64) -> PipelineDeadline {
        // The virtual clock owns elapsed time; a distant wall-clock bound avoids
        // coupling paused-time tests to real scheduler delays.
        PipelineDeadline {
            unix_ms: unix_time_millis() + 60_000,
            monotonic: Instant::now() + Duration::from_millis(remaining_ms),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn preparation_overrun_keeps_the_same_candidate_when_the_complete_pipeline_fits() {
        let start = Instant::now();
        let deadline = virtual_deadline(1450);
        let prover = SlowSuccess {
            duration: Duration::from_millis(100),
            calls: AtomicUsize::new(0),
        };
        tokio::time::advance(Duration::from_millis(1200)).await;
        let proof = prove_with_shared_deadline(
            &prover,
            test_context(),
            test_timing(),
            BundleTarget::Exact {
                block: 5,
                timestamp: 1,
            },
            deadline,
            POST_PROOF_RESERVE,
        )
        .await
        .unwrap();
        assert_eq!(proof, Bytes::from_static(b"proof"));
        assert_eq!(prover.calls.load(Ordering::Relaxed), 1);
        tokio::time::advance(Duration::from_millis(100 + 20)).await;
        assert_eq!(start.elapsed(), Duration::from_millis(1420));
        assert_eq!(deadline.remaining(), Duration::from_millis(30));
        tokio::time::advance(Duration::from_millis(29)).await;
        assert!(!deadline.remaining().is_zero());
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(deadline.remaining().is_zero());
        let error = prove_with_shared_deadline(
            &prover,
            test_context(),
            test_timing(),
            BundleTarget::Exact {
                block: 5,
                timestamp: 1,
            },
            deadline,
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            ProverError::Retryable {
                kind: RetryableProverError::DeadlineExceeded,
                ..
            }
        ));
        assert_eq!(prover.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn fast_preparation_lends_time_to_proof_and_retries_cannot_reset_the_window() {
        let prover = SlowSuccess {
            duration: Duration::from_millis(800),
            calls: AtomicUsize::new(0),
        };
        let target = BundleTarget::Exact {
            block: 5,
            timestamp: 1,
        };
        let deadline = virtual_deadline(1450);
        tokio::time::advance(Duration::from_millis(100)).await;
        assert!(
            prove_with_shared_deadline(
                &prover,
                test_context(),
                test_timing(),
                target,
                deadline,
                POST_PROOF_RESERVE
            )
            .await
            .is_ok()
        );
        let start = Instant::now();
        let deadline = virtual_deadline(500);
        let stalled = DelayedFailureThenStallProver::default();
        let error = prove_with_shared_deadline(
            &stalled,
            test_context(),
            test_timing(),
            target,
            deadline,
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            ProverError::Retryable {
                kind: RetryableProverError::DeadlineExceeded,
                ..
            }
        ));
        assert_eq!(start.elapsed(), Duration::from_millis(450));
        assert_eq!(stalled.calls.load(Ordering::Relaxed), 2);
        assert_eq!(deadline.remaining(), Duration::from_millis(50));
    }

    #[tokio::test(start_paused = true)]
    async fn clock_correction_cannot_extend_the_shared_window() {
        let mut deadline = virtual_deadline(500);
        tokio::time::advance(Duration::from_millis(400)).await;
        deadline.unix_ms += 10_000; // wall clock moves backwards relative to target
        assert_eq!(deadline.remaining(), Duration::from_millis(100));
        deadline.unix_ms = unix_time_millis().saturating_sub(1);
        assert!(deadline.remaining().is_zero());
    }

    #[tokio::test(start_paused = true)]
    async fn retryable_prover_error_retries_the_complete_operation() {
        let prover = ScriptedProver::new(vec![
            Err(retryable(RetryableProverError::Unavailable)),
            Ok(Bytes::from_static(b"proof")),
        ]);

        let proof = prove_with_retry_at(
            &prover,
            test_context(),
            test_timing(),
            BundleTarget::NextBlock,
            TEST_NOW_MS,
        )
        .await
        .unwrap();

        assert_eq!(proof, Bytes::from_static(b"proof"));
        assert_eq!(prover.calls(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn non_retryable_prover_error_is_attempted_once() {
        let prover = ScriptedProver::new(vec![Err(ProverError::Backend(
            "fatal test failure".to_owned(),
        ))]);

        let error = prove_with_retry_at(
            &prover,
            test_context(),
            test_timing(),
            BundleTarget::NextBlock,
            TEST_NOW_MS,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ProverError::Backend(_)));
        assert_eq!(prover.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn actionable_prover_error_is_returned_without_retrying_same_request() {
        let failure = ActionableProverFailure::Outbound {
            transaction_index: 3,
            transaction_hash: B256::repeat_byte(0xaa),
        };
        let prover = ScriptedProver::new(vec![Err(ProverError::Actionable {
            failure,
            message: "outbound transaction failed validation".to_owned(),
        })]);

        let error = prove_with_retry_at(
            &prover,
            test_context(),
            test_timing(),
            BundleTarget::NextBlock,
            TEST_NOW_MS,
        )
        .await
        .unwrap_err();

        assert_eq!(error.actionable_failure(), Some(failure));
        assert_eq!(prover.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn expired_slot_cutoff_starts_no_proof_attempt() {
        let prover = ScriptedProver::new(vec![Ok(Bytes::from_static(b"unexpected"))]);

        let error = prove_with_retry_at(
            &prover,
            test_context(),
            test_timing(),
            BundleTarget::Exact {
                block: 1,
                timestamp: 1,
            },
            TEST_NOW_MS,
        )
        .await
        .unwrap_err();

        assert_eq!(
            error.retryable_kind(),
            Some(RetryableProverError::DeadlineExceeded)
        );
        assert_eq!(prover.calls(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_attempt_exhausts_the_next_block_retry_window() {
        let prover = TimeoutProver::default();

        let error = prove_with_retry_at(
            &prover,
            test_context(),
            test_timing(),
            BundleTarget::NextBlock,
            TEST_NOW_MS,
        )
        .await
        .unwrap_err();

        assert_eq!(
            error.retryable_kind(),
            Some(RetryableProverError::DeadlineExceeded)
        );
        assert_eq!(prover.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn next_block_retry_attempt_uses_only_the_remaining_budget() {
        let timing = RollupTiming::new(3_000, 1_000, 1_000, 100, 1);
        let prover = DelayedFailureThenStallProver::default();
        let started = Instant::now();

        let error = prove_with_retry_at(
            &prover,
            test_context(),
            timing,
            BundleTarget::NextBlock,
            TEST_NOW_MS,
        )
        .await
        .unwrap_err();

        assert_eq!(
            error.retryable_kind(),
            Some(RetryableProverError::DeadlineExceeded)
        );
        assert_eq!(prover.calls.load(Ordering::Relaxed), 2);
        assert_eq!(Instant::now().duration_since(started), timing.proof_time());
    }

    #[tokio::test(start_paused = true)]
    async fn next_block_retries_until_its_total_wait_budget_is_exhausted() {
        let timing = RollupTiming::new(5_000, 1_000, 2_000, 100, 2);
        let prover = AlwaysRetryableProver::default();
        let started = Instant::now();

        let error = prove_with_retry_at(
            &prover,
            test_context(),
            timing,
            BundleTarget::NextBlock,
            TEST_NOW_MS,
        )
        .await
        .unwrap_err();

        assert_eq!(
            error.retryable_kind(),
            Some(RetryableProverError::Unavailable)
        );
        let elapsed = Instant::now().duration_since(started);
        assert!(
            elapsed >= Duration::from_millis(INITIAL_PROVER_RETRY_BACKOFF_MS * 4 / 5),
            "retrying should include a real backoff rather than hot-looping"
        );
        assert!(elapsed < timing.proof_time());
        assert!(
            prover.calls.load(Ordering::Relaxed) > 3,
            "the retry window should outlive the old three-attempt cap"
        );
    }

    #[test]
    fn backoff_stays_within_twenty_percent_of_its_capped_exponential_base() {
        for attempt in 1_u32..=40 {
            let shift = attempt.saturating_sub(1).min(31);
            let base_ms =
                (INITIAL_PROVER_RETRY_BACKOFF_MS << shift).min(MAX_PROVER_RETRY_BACKOFF_MS);
            for seed in [0, 1, u64::MAX / 2, u64::MAX] {
                let delay_ms =
                    u64::try_from(prover_retry_backoff_with_seed(attempt, seed).as_millis())
                        .unwrap();
                assert!(
                    delay_ms >= base_ms * 4 / 5,
                    "attempt {attempt}, seed {seed}"
                );
                assert!(
                    delay_ms <= base_ms * 6 / 5,
                    "attempt {attempt}, seed {seed}"
                );
            }
        }
    }

    #[test]
    fn proof_cutoff_reserves_proof_time_and_submission_slack() {
        let timing = RollupTiming::new(12_000, 2_000, 4_000, 1_500, 3);
        assert_eq!(
            proof_start_cutoff_ms(
                timing,
                BundleTarget::Exact {
                    block: 7,
                    timestamp: 100,
                }
            ),
            Some(94_500)
        );
        assert_eq!(proof_start_cutoff_ms(timing, BundleTarget::NextBlock), None);
    }
}
