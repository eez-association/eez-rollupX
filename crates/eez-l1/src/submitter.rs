//! Thin signed L1-submission primitive: sends `postAndVerifyBatch` via
//! `eth_sendBundle`. Read-only canonical-chain scans live in
//! [`L1Reader`].
//!
//! `eth_sendBundle` pins inclusion to one L1 block. If the bundle
//! isn't in that block we report [`SendOutcome::Dropped`] and the
//! Composer rebuilds on the next tick with a fresh target + nonce.
//! Real Flashbots-style relays don't consume the nonce on miss; the
//! anvil-side `scripts/builder-stub.py` does (it forwards via
//! `eth_sendRawTransaction`), but the cursor-race guard +
//! `pending` nonce read keep both paths correct.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy_eips::{BlockNumberOrTag, Decodable2718};
use alloy_primitives::{B256, TxHash, hex};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use tracing::{Level, event};

use crate::config::SubmitterConfig;
use crate::error::{L1Error, L1Result};
use crate::l1_reader::L1Reader;

/// Bounded delivery observation. A missing acknowledgement after dispatch is
/// uncertain submission, not evidence that the relay did not receive the bundle.
#[derive(Debug, Clone, Copy)]
pub struct BundleDelivery {
    pub elapsed: Duration,
    pub acknowledged: bool,
    /// Relay-supplied correlation identifier, if it is a valid 32-byte hash.
    /// It is not an inclusion commitment and may differ between relays.
    pub relay_bundle_hash: Option<B256>,
    /// Shared delivery window left after the reply or transport failure.
    pub remaining: Duration,
    pub target_block: u64,
    pub target_timestamp: u64,
    pub expected_parent_hash: Option<B256>,
    pub transaction_count: usize,
}

struct DeliveryWindow {
    deadline: tokio::time::Instant,
    observed: Box<dyn FnOnce(BundleDelivery) + Send>,
}

/// Wall-clock cap on the target-block + inclusion check.
const TARGET_WAIT_BUDGET: Duration = Duration::from_secs(30);
const TARGET_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Legacy L1 block offset for [`BundleTarget::NextBlock`]. slack=2 (over the
/// minimal latest+1) gives a one-block cushion for when our local
/// `latest` is stale by the time the relay sees the bundle.
const NEXT_BLOCK_SLACK: u64 = 2;

/// Additional exact-target opportunities permitted on one unchanged L1 parent.
/// The scheduler and submission gates share this bound so consecutive skipped
/// slots can be retried without unbounded proving work and retained submissions
/// during a disconnected L1 episode. Every replacement retains its predecessor.
pub const MAX_MISSING_SLOT_REPLACEMENTS: u64 = 4;

/// Return the aligned, bounded slot offset from the observed canonical parent.
fn policy_target_slot_offset(parent: u64, target: u64, slot_seconds: u64) -> Option<u64> {
    let distance = target.checked_sub(parent)?;
    if distance.checked_rem(slot_seconds)? != 0 {
        return None;
    }
    let offset = distance.checked_div(slot_seconds)?;
    (1..=MAX_MISSING_SLOT_REPLACEMENTS + 1)
        .contains(&offset)
        .then_some(offset)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_field_names)]
struct SubmissionTiming {
    l1_block_time_ms: u64,
    lead_ms: u64,
    deadline_slack_ms: u64,
}

impl SubmissionTiming {
    fn new(l1_block_time: Duration, lead: Duration, deadline_slack: Duration) -> L1Result<Self> {
        let l1_block_time_ms = duration_millis("L1 block time", l1_block_time)?;
        let lead_ms = duration_millis("bundle submission lead", lead)?;
        let deadline_slack_ms = duration_millis("submission slack", deadline_slack)?;
        if l1_block_time_ms == 0 || !l1_block_time_ms.is_multiple_of(1_000) {
            return Err(L1Error::Config(format!(
                "L1 block time must be a positive whole number of seconds, got {l1_block_time_ms} ms"
            )));
        }
        if lead_ms <= deadline_slack_ms {
            return Err(L1Error::Config(format!(
                "bundle submission lead ({lead_ms} ms) must exceed submission slack ({deadline_slack_ms} ms)"
            )));
        }
        Ok(Self {
            l1_block_time_ms,
            lead_ms,
            deadline_slack_ms,
        })
    }

    const fn l1_block_time_secs(self) -> u64 {
        self.l1_block_time_ms / 1_000
    }
}

fn duration_millis(name: &str, value: Duration) -> L1Result<u64> {
    u64::try_from(value.as_millis())
        .map_err(|_| L1Error::Config(format!("{name} does not fit in u64 milliseconds")))
}

/// Which L1 block the bundle should land in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleTarget {
    /// Resolved to `latest + NEXT_BLOCK_SLACK` at send time. The target's
    /// canonical parent supplies an expected slot timestamp; the request is
    /// timestamp-pinned so a skipped slot fails closed and retries.
    NextBlock,
    /// Land in exactly L1 block `block`, and only if its timestamp equals
    /// `timestamp`. The timestamp pin makes the settlement slot match the
    /// L2 Sync block's anchored slot by construction: a gnosis block's
    /// timestamp IS its slot time, so a skipped slot (block lands a slot
    /// late) won't match — the bundle drops instead of settling with a
    /// drifted L2 timestamp.
    Exact { block: u64, timestamp: u64 },
}

/// One bundle attempt's outcome. `Dropped` is the expected miss path —
/// caller rebuilds on the next tick.
#[derive(Debug, Clone)]
pub enum SendOutcome {
    /// The postBatch receipt belongs to a canonical L1 block. Canonical
    /// derivation and source-chain evidence still decide each user's outcome.
    Included {
        tx_hash: TxHash,
        l1_block: u64,
        l1_block_hash: B256,
    },
    Dropped {
        tx_hash: TxHash,
        target_block: u64,
        reason: BundleDropReason,
    },
}

/// Why a bundle did not settle. Only [`Self::Excluded`] can plausibly be
/// caused by a poison user transaction; local timing and chain instability
/// must never consume a user's bounded relay-attempt budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleDropReason {
    /// The intended timestamp was built without this bundle, or the chain
    /// passed an unobservable target. This includes auction loss, builder
    /// discard, and relay-side simulation failure.
    Excluded,
    /// The target height was proposed at a different timestamp.
    SlotSkipped,
    /// The bundle was ready only after the configured submission deadline.
    SubmissionWindowMissed,
    /// The selected target's parent changed or the target was built while the
    /// composer waited for the builder gate.
    TargetChanged,
}

impl BundleDropReason {
    /// Whether retries should count toward poison-transaction eviction.
    #[must_use]
    pub const fn counts_toward_attempt_limit(self) -> bool {
        matches!(self, Self::Excluded)
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Excluded => "excluded",
            Self::SlotSkipped => "slot_skipped",
            Self::SubmissionWindowMissed => "submission_window_missed",
            Self::TargetChanged => "target_changed",
        }
    }
}

/// Receipt bound to a checked canonical L1 block.
#[derive(Debug, Clone)]
pub struct CanonicalSourceReceipt {
    pub block: crate::L1BlockRef,
    pub transaction_index: u64,
    pub success: bool,
    /// Persisted mutable queue consumptions in this exact source transaction.
    pub consumed: Vec<(B256, usize)>,
    /// Observability failure must not change canonical ownership evidence.
    pub cost: Result<crate::posting_fees::ReceiptCost, crate::posting_fees::PostingFeeError>,
}

/// Source nonce and optional receipt read against one immutable L1 state.
#[derive(Debug, Clone)]
pub struct CanonicalSourceState {
    pub nonce: u64,
    pub receipt: Option<CanonicalSourceReceipt>,
}

/// Thin L1-interaction primitive. Cheaply [`Clone`]able.
#[derive(Clone)]
pub struct Submitter {
    inner: Arc<Inner>,
}

struct Inner {
    config: SubmitterConfig,
    submission_timing: SubmissionTiming,
    reader: L1Reader,
    target_provider: RootProvider,
    http: reqwest::Client,
}

trait Clock {
    fn now_unix_ms(&self) -> L1Result<u64>;

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;
}

#[derive(Debug, Clone, Copy)]
struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_ms(&self) -> L1Result<u64> {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                L1Error::Provider(format!("submission clock is before Unix epoch: {error}"))
            })?;
        u64::try_from(elapsed.as_millis()).map_err(|_| {
            L1Error::Provider("submission clock does not fit in u64 milliseconds".into())
        })
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send {
        tokio::time::sleep(duration)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PreparedTarget {
    block: u64,
    timestamp: u64,
    expected_parent_hash: Option<B256>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TargetPreparation {
    Ready(PreparedTarget),
    Missed {
        block: u64,
        reason: BundleDropReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubmissionWindowState {
    Waiting(Duration),
    Open,
    Expired,
}

fn submission_window_bounds(target_timestamp: u64, timing: SubmissionTiming) -> (u64, u64) {
    let target_ms = target_timestamp.saturating_mul(1_000);
    (
        target_ms.saturating_sub(timing.lead_ms),
        target_ms.saturating_sub(timing.deadline_slack_ms),
    )
}

fn submission_window_state(
    now_ms: u64,
    target_timestamp: u64,
    timing: SubmissionTiming,
) -> SubmissionWindowState {
    let (not_before_ms, deadline_ms) = submission_window_bounds(target_timestamp, timing);
    if now_ms >= deadline_ms {
        SubmissionWindowState::Expired
    } else if now_ms < not_before_ms {
        SubmissionWindowState::Waiting(Duration::from_millis(not_before_ms - now_ms))
    } else {
        SubmissionWindowState::Open
    }
}

async fn wait_for_submission_window<C: Clock>(
    clock: &C,
    target_block: u64,
    target_timestamp: u64,
    timing: SubmissionTiming,
    tx_hash: TxHash,
) -> L1Result<bool> {
    loop {
        let now_ms = clock.now_unix_ms()?;
        match submission_window_state(now_ms, target_timestamp, timing) {
            SubmissionWindowState::Waiting(delay) => {
                let (not_before_ms, deadline_ms) =
                    submission_window_bounds(target_timestamp, timing);
                event!(
                    name: "eez.submitter.bundle.gate_wait",
                    Level::INFO,
                    tx_hash = %tx_hash,
                    target_block,
                    target_timestamp,
                    not_before_ms,
                    deadline_ms,
                    wait_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                    "bundle ready before builder gate; retaining locally",
                );
                clock.sleep(delay).await;
            }
            SubmissionWindowState::Open => return Ok(true),
            SubmissionWindowState::Expired => return Ok(false),
        }
    }
}

async fn rpc_block<P: Provider>(
    provider: &P,
    number: BlockNumberOrTag,
) -> L1Result<Option<alloy_rpc_types_eth::Block>> {
    provider
        .get_block_by_number(number)
        .await
        .map_err(|error| L1Error::Provider(format!("get_block_by_number({number}): {error}")))
}

/// Select the first slot with room for preparation and submission slack.
/// Read the clock after the head RPC so its latency consumes the budget.
/// Builder lead opens the dispatch window; it is not a minimum preparation
/// duration (on Chiado it can exceed an entire slot).
async fn select_policy_target<P: Provider, C: Clock>(
    provider: &P,
    timing: SubmissionTiming,
    preparation_time: Duration,
    clock: &C,
) -> L1Result<BundleTarget> {
    let tip = rpc_block(provider, BlockNumberOrTag::Latest)
        .await?
        .ok_or_else(|| L1Error::Provider("policy target L1 head is unavailable".into()))?;
    let now_ms = clock.now_unix_ms()?;
    let preparation_ms = duration_millis("policy preparation time", preparation_time)?;
    let ready_ms = now_ms
        .checked_add(preparation_ms)
        .and_then(|value| value.checked_add(timing.deadline_slack_ms))
        .ok_or_else(|| L1Error::Provider("policy preparation deadline overflow".into()))?;
    let tip_ms = tip
        .header
        .timestamp
        .checked_mul(1_000)
        .ok_or_else(|| L1Error::Provider("policy head timestamp overflow".into()))?;
    // The submission deadline is exclusive. Even at equality, choose the
    // following slot rather than start with an already exhausted budget.
    let offset = (ready_ms.saturating_sub(tip_ms) / timing.l1_block_time_ms)
        .checked_add(1)
        .ok_or_else(|| L1Error::Provider("policy target offset overflow".into()))?;
    let block = tip
        .header
        .number
        .checked_add(offset)
        .ok_or_else(|| L1Error::Provider("policy target height overflow".into()))?;
    let timestamp_ms = offset
        .checked_mul(timing.l1_block_time_ms)
        .and_then(|value| tip_ms.checked_add(value))
        .ok_or_else(|| L1Error::Provider("policy target timestamp overflow".into()))?;
    let timestamp = timestamp_ms / 1_000;
    event!(name: "eez.submitter.policy.target_selected", Level::INFO,
        event_name = "eez.submitter.policy.target_selected",
        head_number = tip.header.number, head_timestamp = tip.header.timestamp,
        selected_at_ms = now_ms, preparation_ms,
        submission_slack_ms = timing.deadline_slack_ms,
        target_block = block, target_timestamp = timestamp, block_offset = offset,
        "selected earliest policy target with preparation and submission time");
    Ok(BundleTarget::Exact { block, timestamp })
}

async fn wait_for_block<P: Provider, C: Clock>(
    provider: &P,
    number: u64,
    clock: &C,
    tx_hash: TxHash,
) -> L1Result<alloy_rpc_types_eth::Block> {
    let mut waiting_logged = false;
    loop {
        if let Some(block) = rpc_block(provider, BlockNumberOrTag::Number(number)).await? {
            return Ok(block);
        }
        if !waiting_logged {
            waiting_logged = true;
            event!(
                name: "eez.submitter.bundle.target_parent_wait",
                Level::INFO,
                tx_hash = %tx_hash,
                parent_block = number,
                "waiting for canonical target parent before scheduling bundle",
            );
        }
        clock.sleep(TARGET_POLL_INTERVAL).await;
    }
}

/// Conclusive local reason the fixed policy target cannot be submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicySubmissionFailure {
    ParentUnavailable,
    ParentTimestampMismatch,
    WindowExpired,
    TargetChanged,
}

impl PolicySubmissionFailure {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParentUnavailable => "target_parent_unavailable",
            Self::ParentTimestampMismatch => "target_parent_timestamp_mismatch",
            Self::WindowExpired => "submission_window_expired",
            Self::TargetChanged => "target_changed",
        }
    }
}

async fn prepare_policy_submission<P: Provider, C: Clock>(
    provider: &P,
    target: BundleTarget,
    timing: SubmissionTiming,
    clock: &C,
) -> L1Result<bool> {
    Ok(policy_submission_failure(provider, target, timing, clock)
        .await?
        .is_none())
}

// A policy proof may be prepared before its target's parent exists. Wait within
// the existing deadline, then use the normal final gate without sliding the target.
async fn policy_submission_failure<P: Provider, C: Clock>(
    provider: &P,
    target: BundleTarget,
    timing: SubmissionTiming,
    clock: &C,
) -> L1Result<Option<PolicySubmissionFailure>> {
    let BundleTarget::Exact { block, timestamp } = target else {
        return Err(L1Error::Submission(
            "policy target must be resolved before preparation".into(),
        ));
    };
    let parent_number = block
        .checked_sub(1)
        .ok_or_else(|| L1Error::Submission("target cannot be genesis".into()))?;
    let mut parent_missing = false;
    loop {
        if matches!(
            submission_window_state(clock.now_unix_ms()?, timestamp, timing),
            SubmissionWindowState::Expired
        ) {
            return Ok(Some(if parent_missing {
                PolicySubmissionFailure::ParentUnavailable
            } else {
                PolicySubmissionFailure::WindowExpired
            }));
        }
        if let Some(parent) = rpc_block(provider, BlockNumberOrTag::Number(parent_number)).await? {
            // Exact timestamp pins allow a bounded chain of replacements after
            // missing slots. This admits preparation, not a skipped-slot verdict:
            // the final gate still rejects an already-produced target height,
            // and the caller re-executes the whole bundle on the current parent.
            if policy_target_slot_offset(
                parent.header.timestamp,
                timestamp,
                timing.l1_block_time_secs(),
            )
            .is_none()
            {
                return Ok(Some(PolicySubmissionFailure::ParentTimestampMismatch));
            }
            break;
        }
        parent_missing = true;
        clock.sleep(TARGET_POLL_INTERVAL).await;
    }
    Ok(
        match prepare_submission_target(provider, target, timing, clock, B256::ZERO).await? {
            TargetPreparation::Ready(_) => None,
            TargetPreparation::Missed {
                reason: BundleDropReason::SubmissionWindowMissed,
                ..
            } => Some(PolicySubmissionFailure::WindowExpired),
            TargetPreparation::Missed { .. } => Some(PolicySubmissionFailure::TargetChanged),
        },
    )
}

async fn prepare_submission_target<P: Provider, C: Clock>(
    provider: &P,
    target: BundleTarget,
    timing: SubmissionTiming,
    clock: &C,
    tx_hash: TxHash,
) -> L1Result<TargetPreparation> {
    let (target_block, target_timestamp, expected_parent) = match target {
        BundleTarget::Exact { block, timestamp } => (block, timestamp, None),
        BundleTarget::NextBlock => {
            let tip = rpc_block(provider, BlockNumberOrTag::Latest)
                .await?
                .ok_or_else(|| L1Error::Provider("latest L1 block is unavailable".into()))?;
            let target_block = tip
                .header
                .number
                .checked_add(NEXT_BLOCK_SLACK)
                .ok_or_else(|| L1Error::Provider("L1 target block overflow".into()))?;
            let parent_number = target_block - 1;
            let parent = wait_for_block(provider, parent_number, clock, tx_hash).await?;
            let timestamp = parent
                .header
                .timestamp
                .checked_add(timing.l1_block_time_secs())
                .ok_or_else(|| L1Error::Provider("L1 target timestamp overflow".into()))?;
            (
                target_block,
                timestamp,
                Some((parent_number, parent.header.hash)),
            )
        }
    };

    loop {
        if !wait_for_submission_window(clock, target_block, target_timestamp, timing, tx_hash)
            .await?
        {
            return Ok(TargetPreparation::Missed {
                block: target_block,
                reason: BundleDropReason::SubmissionWindowMissed,
            });
        }

        // A NextBlock target is meaningful only while the parent used to
        // derive its number and timestamp remains canonical. Exact targets are
        // protected by their caller-supplied timestamp pin.
        if let Some((parent_number, expected_hash)) = expected_parent {
            let canonical_parent =
                rpc_block(provider, BlockNumberOrTag::Number(parent_number)).await?;
            if canonical_parent.as_ref().map(|block| block.header.hash) != Some(expected_hash) {
                return Ok(TargetPreparation::Missed {
                    block: target_block,
                    reason: BundleDropReason::TargetChanged,
                });
            }
        }

        let latest = rpc_block(provider, BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| L1Error::Provider("latest L1 block is unavailable".into()))?;
        if latest.header.number >= target_block {
            return Ok(TargetPreparation::Missed {
                block: target_block,
                reason: BundleDropReason::TargetChanged,
            });
        }

        // RPC checks consume wall time. Re-evaluate both boundaries after
        // them; a backward clock correction loops and waits again, while a
        // deadline overrun fails without touching the relay.
        match submission_window_state(clock.now_unix_ms()?, target_timestamp, timing) {
            SubmissionWindowState::Open => {
                event!(
                    name: "eez.submitter.bundle.gate_open",
                    Level::INFO,
                    tx_hash = %tx_hash,
                    target_block,
                    target_timestamp,
                    dispatch_time_ms = clock.now_unix_ms()?,
                    lead_ms = timing.lead_ms,
                    deadline_slack_ms = timing.deadline_slack_ms,
                    "bundle submission window open",
                );
                return Ok(TargetPreparation::Ready(PreparedTarget {
                    block: target_block,
                    timestamp: target_timestamp,
                    expected_parent_hash: expected_parent.map(|(_, hash)| hash),
                }));
            }
            SubmissionWindowState::Waiting(_) => {}
            SubmissionWindowState::Expired => {
                return Ok(TargetPreparation::Missed {
                    block: target_block,
                    reason: BundleDropReason::SubmissionWindowMissed,
                });
            }
        }
    }
}

impl std::fmt::Debug for Submitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Submitter")
            .field("config", &self.inner.config)
            .finish()
    }
}

impl Submitter {
    /// Build a Submitter from its config.
    ///
    /// # Errors
    ///
    /// Returns [`L1Error::Config`] when the submission bounds do not form a
    /// non-empty window or the bounded HTTP client cannot be initialized.
    pub fn new(
        config: SubmitterConfig,
        l1_block_time: Duration,
        submission_slack: Duration,
    ) -> L1Result<Self> {
        Self::new_inner(config, l1_block_time, submission_slack, None)
    }

    /// Share the same selected network rules with canonical derivation.
    pub fn new_with_blob_rules(
        config: SubmitterConfig,
        l1_block_time: Duration,
        submission_slack: Duration,
        rules: crate::BlobRuleSource,
    ) -> L1Result<Self> {
        Self::new_inner(config, l1_block_time, submission_slack, Some(rules))
    }

    fn new_inner(
        config: SubmitterConfig,
        l1_block_time: Duration,
        submission_slack: Duration,
        rules: Option<crate::BlobRuleSource>,
    ) -> L1Result<Self> {
        let mut reader = L1Reader::new(config.reader.clone());
        if let Some(rules) = rules {
            reader = reader.with_blob_rules(rules);
        }
        let submission_timing = SubmissionTiming::new(
            l1_block_time,
            config.bundle_submission_lead,
            submission_slack,
        )?;
        let request_timeout = Duration::from_secs(30);
        let target_provider = Inner::build_target_provider(&config, request_timeout);
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                submission_timing,
                reader,
                target_provider,
                // Bound both dispatch and canonical source reads. Timeout is
                // inconclusive: ownership stays reserved until reconciliation
                // succeeds. Never substitute an unbounded client on failure.
                http: reqwest::Client::builder()
                    .timeout(request_timeout)
                    .build()
                    .map_err(|error| L1Error::Config(format!("submitter HTTP client: {error}")))?,
            }),
        })
    }

    /// Clone the read-only canonical L1 client contained by this Submitter.
    /// The Deriver uses it without gaining access to submission credentials or
    /// builder routing.
    #[must_use]
    pub fn reader(&self) -> L1Reader {
        self.inner.reader.clone()
    }

    /// Identify the still-current parent within the bounded replacement window.
    /// Absence is permission to speculate only; it is never settlement evidence.
    pub async fn replacement_parent(
        &self,
        block: u64,
        timestamp: u64,
    ) -> L1Result<Option<crate::L1BlockRef>> {
        let parent = rpc_block(&self.inner.target_provider, BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| L1Error::Provider("latest L1 block is unavailable".into()))?;
        let offset = policy_target_slot_offset(
            parent.header.timestamp,
            timestamp,
            self.inner.submission_timing.l1_block_time_secs(),
        );
        Ok((parent.header.number.checked_add(1) == Some(block)
            && offset.is_some_and(|offset| offset >= 2))
        .then_some(crate::L1BlockRef {
            number: parent.header.number,
            hash: parent.header.hash,
        }))
    }

    /// L1 EOA address this Submitter sends from. Used by the Composer
    /// to identify "our own" `L1Event::BatchPosted` events vs.
    /// external ones in a multi-composer based-rollup deployment.
    #[must_use]
    pub fn poster_address(&self) -> alloy_primitives::Address {
        self.inner.config.poster.address()
    }

    /// Choose the earliest feasible target before policy evaluation and proof
    /// preparation, reserving the caller's existing proof-time budget and the
    /// network's submission slack. A timely historical batch can target the
    /// immediate next L1 block without the legacy one-block cushion.
    /// Exact timestamp pinning rejects skipped slots through the normal retry
    /// path instead of silently moving the policy deadline at send time.
    pub async fn next_policy_target(&self, preparation_time: Duration) -> L1Result<BundleTarget> {
        select_policy_target(
            &self.inner.target_provider,
            self.inner.submission_timing,
            preparation_time,
            &SystemClock,
        )
        .await
    }

    /// Pin a deferred target before proving, retaining the existing two-block
    /// offset. The signer and sender must use the same fork timestamp. Skipped
    /// slots fail the exact-target check and are retried with a fresh proof.
    pub async fn resolve_target(&self, target: BundleTarget) -> L1Result<BundleTarget> {
        if matches!(target, BundleTarget::Exact { .. }) {
            return Ok(target);
        }
        let tip = rpc_block(&self.inner.target_provider, BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| L1Error::Provider("latest L1 block is unavailable".into()))?;
        let block = tip
            .header
            .number
            .checked_add(NEXT_BLOCK_SLACK)
            .ok_or_else(|| L1Error::Provider("L1 target block overflow".into()))?;
        let timestamp = self
            .inner
            .submission_timing
            .l1_block_time_secs()
            .checked_mul(NEXT_BLOCK_SLACK)
            .and_then(|offset| tip.header.timestamp.checked_add(offset))
            .ok_or_else(|| L1Error::Provider("L1 target timestamp overflow".into()))?;
        Ok(BundleTarget::Exact { block, timestamp })
    }

    /// Submit only after the caller has recorded the exact target.
    /// The callback runs before external dispatch and may veto it on error.
    /// Only signed L1 users following the required postBatch may revert.
    /// Observation of postBatch alone never completes individual users.
    pub async fn send_bundle_recorded(
        &self,
        raw_txs: &[alloy_primitives::Bytes],
        target: BundleTarget,
        record_dispatch: impl FnOnce(u64, u64) -> L1Result<()> + Send,
    ) -> L1Result<SendOutcome> {
        self.send_bundle_checked(raw_txs, target, None, record_dispatch, None)
            .await
    }

    /// Reuse the normal eligibility gate before refreshing an unsent policy
    /// attempt. The composer calls this from its existing submission observer,
    /// so waiting for a future L1 parent does not stall L2 block production.
    pub async fn wait_for_policy_submission(&self, target: BundleTarget) -> L1Result<bool> {
        prepare_policy_submission(
            &self.inner.target_provider,
            target,
            self.inner.submission_timing,
            &SystemClock,
        )
        .await
    }

    /// Same eligibility gate as `wait_for_policy_submission`, with a bounded
    /// diagnostic reason. This never relaxes a deadline or target check.
    pub async fn policy_submission_failure(
        &self,
        target: BundleTarget,
    ) -> L1Result<Option<PolicySubmissionFailure>> {
        policy_submission_failure(
            &self.inner.target_provider,
            target,
            self.inner.submission_timing,
            &SystemClock,
        )
        .await
    }

    /// Dispatch a final simulated bundle only while its fee/state sample is
    /// still the selected network's canonical head and the target is unchanged.
    pub async fn send_bundle_quoted(
        &self,
        raw_txs: &[alloy_primitives::Bytes],
        target: BundleTarget,
        quote: crate::posting_fees::FeeQuote,
        record_dispatch: impl FnOnce(u64, u64) -> L1Result<()> + Send,
    ) -> L1Result<SendOutcome> {
        self.send_bundle_checked(raw_txs, target, Some(quote), record_dispatch, None)
            .await
    }

    /// Use the caller's original monotonic deadline for relay delivery. The
    /// existing wall-clock/parent gate still applies; canonical observation can
    /// continue after delivery expires. Cancellation after dispatch returns an
    /// uncertain transport error and must not release source ownership.
    pub async fn send_bundle_quoted_before(
        &self,
        raw_txs: &[alloy_primitives::Bytes],
        target: BundleTarget,
        quote: crate::posting_fees::FeeQuote,
        record_dispatch: impl FnOnce(u64, u64) -> L1Result<()> + Send,
        deadline: tokio::time::Instant,
        observed: impl FnOnce(BundleDelivery) + Send + 'static,
    ) -> L1Result<SendOutcome> {
        self.send_bundle_checked(
            raw_txs,
            target,
            Some(quote),
            record_dispatch,
            Some(DeliveryWindow {
                deadline,
                observed: Box::new(observed),
            }),
        )
        .await
    }

    async fn send_bundle_checked(
        &self,
        raw_txs: &[alloy_primitives::Bytes],
        target: BundleTarget,
        quote: Option<crate::posting_fees::FeeQuote>,
        record_dispatch: impl FnOnce(u64, u64) -> L1Result<()> + Send,
        delivery: Option<DeliveryWindow>,
    ) -> L1Result<SendOutcome> {
        fail::fail_point!("submitter::send::start", |_| Err(L1Error::Submission(
            "injected failpoint: submitter::send::start".into()
        )));
        if raw_txs.is_empty() {
            return Err(L1Error::Submission(
                "send_bundle called with no txs".to_string(),
            ));
        }
        let post_batch_hash = if raw_txs[0].first() == Some(&3) {
            eez_blob_codec::pooled_blob_transaction_hash(&raw_txs[0]).map_err(|error| {
                L1Error::Submission(format!(
                    "send_bundle: decode pooled postBatch blob transaction: {error}"
                ))
            })?
        } else {
            let post_batch_envelope = alloy_consensus::TxEnvelope::decode_2718(
                &mut raw_txs[0].as_ref(),
            )
            .map_err(|error| {
                L1Error::Submission(format!("send_bundle: decode postBatch envelope: {error}"))
            })?;
            *post_batch_envelope.tx_hash()
        };
        let target_provider = &self.inner.target_provider;
        let chain_id = target_provider
            .get_chain_id()
            .await
            .map_err(|error| L1Error::Provider(format!("bundle source chain: {error}")))?;
        let reverting_hashes = inbound_revert_hashes(raw_txs, chain_id, post_batch_hash)?;
        let clock = SystemClock;
        let prepared = prepare_submission_target(
            target_provider,
            target,
            self.inner.submission_timing,
            &clock,
            post_batch_hash,
        )
        .await?;
        let prepared = match prepared {
            TargetPreparation::Ready(target) => target,
            TargetPreparation::Missed { block, reason } => {
                return Ok(dropped(post_batch_hash, block, reason));
            }
        };

        // Formatting/logging normally takes microseconds, but enforce the
        // deadline again immediately before the HTTP request. We may send
        // after the not-before boundary; we must never knowingly send after
        // the latest-safe boundary and then blame user transactions for it.
        if let Some(quote) = quote {
            let head = rpc_block(target_provider, BlockNumberOrTag::Latest)
                .await?
                .ok_or_else(|| L1Error::Provider("fee sample head is unavailable".into()))?;
            let current = crate::L1BlockRef {
                number: head.header.number,
                hash: head.header.hash,
            };
            if !quote.is_current(chain_id, current, target) {
                return Ok(dropped(
                    post_batch_hash,
                    prepared.block,
                    BundleDropReason::TargetChanged,
                ));
            }
        }
        if !matches!(
            submission_window_state(
                clock.now_unix_ms()?,
                prepared.timestamp,
                self.inner.submission_timing,
            ),
            SubmissionWindowState::Open
        ) {
            return Ok(dropped(
                post_batch_hash,
                prepared.block,
                BundleDropReason::SubmissionWindowMissed,
            ));
        }
        if delivery
            .as_ref()
            .is_some_and(|window| tokio::time::Instant::now() >= window.deadline)
        {
            return Ok(dropped(
                post_batch_hash,
                prepared.block,
                BundleDropReason::SubmissionWindowMissed,
            ));
        }
        record_dispatch(prepared.block, prepared.timestamp)?;
        // Dispatch breadcrumb: correlate this postBatch tx to the L1 block we
        // aim it at. `SendOutcome::Included` later carries the ACTUAL inclusion
        // block, so joining on tx_hash gives the N+1 next-slot targeting hit-rate.
        event!(
            name: "eez.submitter.bundle.dispatch",
            Level::INFO,
            event_name = "eez.submitter.bundle.dispatch",
            target_timestamp = prepared.timestamp,
            tx_hash = %post_batch_hash,
            target_block = prepared.block,
            exact = matches!(target, BundleTarget::Exact { .. }),
            tx_count = raw_txs.len(),
            "dispatching bundle to builder",
        );
        // Every bundle uses one timestamp-pinned target and permits only the
        // validated inbound source users to revert. postBatch is always required.
        self.inner
            .dispatch_and_observe(
                raw_txs,
                post_batch_hash,
                prepared,
                &reverting_hashes,
                delivery,
            )
            .await
    }

    /// Verify a canonical L1 identity using the same provider as source reads.
    pub async fn canonical_block_hash(&self, number: u64) -> L1Result<Option<B256>> {
        Ok(self
            .inner
            .target_provider
            .get_block_by_number(BlockNumberOrTag::Number(number))
            .await
            .map_err(|error| L1Error::Provider(error.to_string()))?
            .map(|block| block.header.hash))
    }

    /// Detect a receipt-index lag before treating a target as excluding us.
    pub async fn canonical_block_contains(&self, number: u64, tx_hash: B256) -> L1Result<bool> {
        let block = self
            .inner
            .target_provider
            .get_block_by_number(BlockNumberOrTag::Number(number))
            .hashes()
            .await
            .map_err(|error| L1Error::Provider(error.to_string()))?
            .ok_or_else(|| L1Error::Provider("target block is unavailable".into()))?;
        Ok(block.transactions.hashes().any(|hash| hash == tx_hash))
    }

    /// Prove that no canonical block could satisfy the dispatched timestamp.
    /// A different timestamp at the target height alone is insufficient: a
    /// stale height can point past a slot that was actually built. Require the
    /// pin to lie strictly between consecutive canonical headers, inside the
    /// deriver's completed scan. This is scheduling evidence, not a source
    /// disposition or permission to reuse the missed composition.
    pub async fn canonical_target_was_skipped(
        &self,
        target_block: u64,
        target_timestamp: u64,
        at: crate::L1BlockRef,
    ) -> L1Result<bool> {
        canonical_target_was_skipped(
            &self.inner.target_provider,
            target_block,
            target_timestamp,
            at,
        )
        .await
    }

    /// Read a signed source transaction and nonce at the deriver's completed
    /// L1 snapshot. Pending nonces, future receipts and reorg-straddling reads
    /// cannot establish exclusion or completion at this snapshot.
    pub async fn source_state_at(
        &self,
        tx_hash: B256,
        sender: alloy_primitives::Address,
        at: crate::L1BlockRef,
    ) -> L1Result<CanonicalSourceState> {
        read_canonical_source(
            &self.inner.target_provider,
            self.inner.reader.eez(),
            self.inner.reader.rollup_id(),
            tx_hash,
            sender,
            at,
        )
        .await
    }

    /// `true` iff L1 currently has a receipt for `tx_hash`, regardless of
    /// status. This convenience lookup is not canonical settlement evidence;
    /// recovery uses `source_state_at` instead.
    ///
    /// # Errors
    ///
    /// [`L1Error::Provider`] on RPC failure.
    pub async fn receipt_exists(&self, tx_hash: TxHash) -> L1Result<bool> {
        // Use the canonical target RPC; the embedded L1 can lag its tip.
        Ok(self
            .inner
            .target_provider
            .get_transaction_receipt(tx_hash)
            .await
            .map_err(|e| L1Error::Provider(format!("get_transaction_receipt: {e}")))?
            .is_some())
    }
}

async fn canonical_target_was_skipped<P: Provider>(
    provider: &P,
    target_block: u64,
    target_timestamp: u64,
    at: crate::L1BlockRef,
) -> L1Result<bool> {
    let Some(parent_number) = target_block.checked_sub(1) else {
        return Ok(false);
    };
    if target_block > at.number {
        return Ok(false);
    }
    require_canonical_block(provider, at).await?;
    let target = provider
        .get_block_by_number(BlockNumberOrTag::Number(target_block))
        .await
        .map_err(|error| L1Error::Provider(format!("canonical target header: {error}")))?
        .ok_or_else(|| L1Error::Provider("canonical target header is unavailable".into()))?;
    if target.header.number != target_block {
        return Err(L1Error::Provider(
            "canonical target height disagrees".into(),
        ));
    }
    if target_block == at.number && target.header.hash != at.hash {
        return Err(L1Error::Provider(
            "canonical target differs from the completed scan".into(),
        ));
    }
    if target.header.timestamp <= target_timestamp {
        return Ok(false);
    }
    let parent = provider
        .get_block_by_number(BlockNumberOrTag::Number(parent_number))
        .await
        .map_err(|error| L1Error::Provider(format!("canonical target parent: {error}")))?
        .ok_or_else(|| L1Error::Provider("canonical target parent is unavailable".into()))?;
    if parent.header.number != parent_number || target.header.parent_hash != parent.header.hash {
        return Err(L1Error::Provider(
            "canonical target and parent headers disagree".into(),
        ));
    }
    require_canonical_block(provider, at).await?;
    Ok(parent.header.timestamp < target_timestamp)
}

/// Keep all nonce/receipt reads inside the same canonical-hash bracket.
async fn read_canonical_source<P: Provider>(
    provider: &P,
    eez: alloy_primitives::Address,
    rollup_id: u64,
    tx_hash: B256,
    sender: alloy_primitives::Address,
    at: crate::L1BlockRef,
) -> L1Result<CanonicalSourceState> {
    require_canonical_block(provider, at).await?;
    let nonce = provider
        .get_transaction_count(sender)
        .block_id(at.hash.into())
        .await
        .map_err(|error| L1Error::Provider(format!("canonical source nonce: {error}")))?;
    // Alloy defaults a missing effectiveGasPrice to zero. Keep field presence
    // for accounting while still accepting the receipt's ownership evidence.
    let raw_receipt: serde_json::Value = provider
        .raw_request("eth_getTransactionReceipt".into(), (tx_hash,))
        .await
        .map_err(|error| L1Error::Provider(format!("canonical source receipt: {error}")))?;
    let price_present = raw_receipt
        .get("effectiveGasPrice")
        .or_else(|| raw_receipt.get("gasPrice"))
        .is_some_and(|value| !value.is_null());
    let receipt: Option<alloy_rpc_types_eth::TransactionReceipt> =
        serde_json::from_value(raw_receipt).map_err(|error| {
            L1Error::Provider(format!("canonical source receipt encoding: {error}"))
        })?;
    let receipt = match receipt {
        Some(receipt) => {
            let number = receipt
                .block_number
                .ok_or_else(|| L1Error::Provider("source receipt lacks block number".into()))?;
            let hash = receipt
                .block_hash
                .ok_or_else(|| L1Error::Provider("source receipt lacks block hash".into()))?;
            if number > at.number {
                return Err(L1Error::Provider(
                    "source receipt is ahead of the completed canonical scan".into(),
                ));
            }
            if receipt.transaction_hash != tx_hash {
                return Err(L1Error::Provider(
                    "source receipt has the wrong transaction hash".into(),
                ));
            }
            let block = crate::L1BlockRef { number, hash };
            require_canonical_block(provider, block).await?;
            let transaction_index = receipt.transaction_index.ok_or_else(|| {
                L1Error::Provider("source receipt lacks transaction index".into())
            })?;
            let mut consumed = Vec::new();
            for log in receipt.inner.logs() {
                if log.address() != eez {
                    continue;
                }
                if let Ok(event) = log.log_decode::<eez_protocol::abi::ExecutionConsumed>()
                    && event.inner.data.rollupId == rollup_id
                {
                    let index =
                        usize::try_from(event.inner.data.entryQueueIndex).map_err(|error| {
                            L1Error::Provider(format!("source queue index: {error}"))
                        })?;
                    consumed.push((event.inner.data.crossChainCallHash, index));
                }
            }
            Some(CanonicalSourceReceipt {
                block,
                transaction_index,
                success: receipt.status(),
                consumed,
                cost: if price_present {
                    crate::posting_fees::ReceiptCost::from_receipt(&receipt)
                } else {
                    Err(crate::posting_fees::PostingFeeError::Unavailable(
                        "execution price in canonical receipt",
                    ))
                },
            })
        }
        None => None,
    };
    require_canonical_block(provider, at).await?;
    Ok(CanonicalSourceState { nonce, receipt })
}

async fn require_canonical_block<P: Provider>(
    provider: &P,
    block: crate::L1BlockRef,
) -> L1Result<()> {
    let canonical = provider
        .get_block_by_number(BlockNumberOrTag::Number(block.number))
        .await
        .map_err(|error| L1Error::Provider(format!("canonical block: {error}")))?;
    if canonical.is_none_or(|canonical| canonical.header.hash != block.hash) {
        return Err(L1Error::Provider(format!(
            "L1 block {} is missing or no longer canonical",
            block.number
        )));
    }
    Ok(())
}

impl Inner {
    /// Build the shared canonical provider once, retaining its connection pool.
    /// Falls back to the main RPC when no override URL is set. See
    /// `SubmitterConfig::target_rpc_url`.
    fn build_target_provider(config: &SubmitterConfig, request_timeout: Duration) -> RootProvider {
        let url = config
            .target_rpc_url
            .clone()
            .unwrap_or_else(|| config.reader.rpc_url.clone());
        ProviderBuilder::new()
            .disable_recommended_fillers()
            .with_reqwest(url, |builder| {
                builder
                    .timeout(request_timeout)
                    .build()
                    .expect("canonical source HTTP client configuration is valid")
            })
    }

    /// Submit `raw_txs` (postBatch first) through the required bundle relay and
    /// observe the outcome. Preserve transaction order and the timestamp pin.
    async fn dispatch_and_observe(
        &self,
        raw_txs: &[alloy_primitives::Bytes],
        post_batch_hash: TxHash,
        target: PreparedTarget,
        reverting_hashes: &[B256],
        delivery: Option<DeliveryWindow>,
    ) -> L1Result<SendOutcome> {
        let hexes: Vec<String> = raw_txs
            .iter()
            .map(|t| hex::encode_prefixed(t.as_ref()))
            .collect();
        let hex_refs: Vec<&str> = hexes.iter().map(String::as_str).collect();
        let request = post_bundle(
            &self.http,
            self.config.builder_rpc_url.as_str(),
            &hex_refs,
            target.block,
            target.timestamp,
            reverting_hashes,
        );
        let acknowledgement = if let Some(window) = delivery {
            let started = tokio::time::Instant::now();
            let result = tokio::time::timeout_at(window.deadline, request)
                .await
                .unwrap_or_else(|_| {
                    Err(L1Error::Submission(
                        "relay acknowledgement exceeded the shared deadline; delivery uncertain"
                            .into(),
                    ))
                });
            (window.observed)(BundleDelivery {
                elapsed: started.elapsed(),
                acknowledged: result.is_ok(),
                relay_bundle_hash: result.as_ref().ok().copied().flatten(),
                remaining: window
                    .deadline
                    .saturating_duration_since(tokio::time::Instant::now()),
                target_block: target.block,
                target_timestamp: target.timestamp,
                expected_parent_hash: target.expected_parent_hash,
                transaction_count: raw_txs.len(),
            });
            result
        } else {
            request.await
        };
        let result = match acknowledgement {
            Ok(_) => {
                // Relay path: Exact and derived NextBlock targets both carry a
                // min/max timestamp pin, which unlocks the early verdict.
                self.observe(post_batch_hash, target).await
            }
            Err(other) => Err(other),
        };
        self.capture_bundle_attempt(
            &hex_refs,
            post_batch_hash,
            target,
            &result,
            reverting_hashes,
        );
        result
    }

    fn capture_bundle_attempt(
        &self,
        raw_tx_hexes: &[&str],
        post_batch_hash: TxHash,
        target: PreparedTarget,
        result: &L1Result<SendOutcome>,
        reverting_hashes: &[B256],
    ) {
        let Some(root) = self.config.bundle_capture_dir.as_ref() else {
            return;
        };
        let (outcome, drop_reason, error) = match result {
            Ok(SendOutcome::Dropped { reason, .. }) => ("dropped", Some(reason.as_str()), None),
            Err(error) => ("submission_error", None, Some(error.to_string())),
            Ok(SendOutcome::Included { .. }) => return,
        };
        let attempt = root.join(format!("bundle-{}-{post_batch_hash}", target.block));
        let request = bundle_request(
            raw_tx_hexes,
            target.block,
            target.timestamp,
            reverting_hashes,
        );
        let metadata = serde_json::json!({
            "version": 1,
            "builderRpcUrl": self.config.builder_rpc_url.as_str(),
            "postBatchHash": post_batch_hash.to_string(),
            "targetBlock": target.block,
            "pinnedTimestamp": target.timestamp,
            "expectedParentHash": target.expected_parent_hash.map(|hash| hash.to_string()),
            "outcome": outcome,
            "dropReason": drop_reason,
            "error": error,
            "requestFile": "request.json",
        });
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(&attempt)?;
            std::fs::write(
                attempt.join("request.json"),
                serde_json::to_vec_pretty(&request).map_err(std::io::Error::other)?,
            )?;
            std::fs::write(
                attempt.join("metadata.json"),
                serde_json::to_vec_pretty(&metadata).map_err(std::io::Error::other)?,
            )?;
            Ok(())
        };
        match write() {
            Ok(()) => event!(
                name: "eez.submitter.bundle.capture",
                Level::WARN,
                path = %attempt.display(),
                target_block = target.block,
                tx_hash = %post_batch_hash,
                "captured rejected bundle request for deterministic replay",
            ),
            Err(capture_error) => event!(
                name: "eez.submitter.bundle.capture_failed",
                Level::WARN,
                path = %attempt.display(),
                error = %capture_error,
                "failed to persist rejected bundle replay capture",
            ),
        }
    }

    /// Observe the canonical postBatch receipt or use the relay's exact
    /// timestamp pin to prove exclusion. Settlement belongs to the deriver.
    async fn observe(&self, tx_hash: TxHash, target: PreparedTarget) -> L1Result<SendOutcome> {
        // Failure must mean PROVABLY DEAD, not merely slow. A bundle is
        // pinned to `target_block` — included in order there or never —
        // so death = the chain passed the target (head > target) without
        // our tx. A wall-clock-only budget here gave false FAILED
        // verdicts for bundles that later landed: recovery reorged + re-
        // emitted, the original settled anyway, and the two timelines
        // fought forever (RootMismatch, phantom L2 effects).
        // Receipt, head, and canonical block all read from the SAME
        // tip provider — mixing the lagging embedded L1 for receipts
        // would re-open the false-death window. The Composer may switch to a
        // deadline replacement while this observer retains the original record;
        // that switch is never recorded here as a failure. If the tip later reorgs
        // the bundle in, downstream converges it (Watcher → Deriver →
        // recovery cursor re-check drops the stale verdict).
        //
        // `pinned` buys a slot without weakening that: min == max timestamp
        // leaves one satisfiable height, so the block there decides it.
        let start = tokio::time::Instant::now();
        let target_provider = &self.target_provider;
        let mut slow_logged = false;
        loop {
            // Transient RPC failures are retried, never escalated to a
            // failure verdict — while polling, the ledger stays Pending
            // without asserting a failure. A bounded deadline replacement can
            // proceed separately while canonical reconciliation retains history.
            match target_provider.get_transaction_receipt(tx_hash).await {
                Ok(Some(receipt)) => {
                    let l1_block = receipt.block_number.ok_or_else(|| {
                        L1Error::Provider("receipt present but block_number missing".into())
                    })?;
                    let l1_block_hash = receipt.block_hash.ok_or_else(|| {
                        L1Error::Provider("receipt present but block_hash missing".into())
                    })?;
                    require_canonical_block(
                        target_provider,
                        crate::L1BlockRef {
                            number: l1_block,
                            hash: l1_block_hash,
                        },
                    )
                    .await?;
                    return Ok(SendOutcome::Included {
                        tx_hash,
                        l1_block,
                        l1_block_hash,
                    });
                }
                Ok(None) => {
                    let verdict = pinned_slot_check(target_provider, target, tx_hash).await;
                    match verdict {
                        PinnedVerdict::Excluded => {
                            return Ok(dropped(tx_hash, target.block, BundleDropReason::Excluded));
                        }
                        PinnedVerdict::SlotSkipped => {
                            return Ok(dropped(
                                tx_hash,
                                target.block,
                                BundleDropReason::SlotSkipped,
                            ));
                        }
                        PinnedVerdict::ParentChanged => {
                            return Ok(dropped(
                                tx_hash,
                                target.block,
                                BundleDropReason::TargetChanged,
                            ));
                        }
                        // Our tx IS in the pinned block; only the receipt read
                        // trails it. Poll on, skipping the head rule.
                        PinnedVerdict::Included => {}
                        PinnedVerdict::Pending => match target_provider.get_block_number().await {
                            Ok(head) if head > target.block => {
                                return Ok(dropped(
                                    tx_hash,
                                    target.block,
                                    BundleDropReason::Excluded,
                                ));
                            }
                            Ok(_) => {}
                            Err(err) => event!(
                                name: "eez.submitter.observe.head_read_failed",
                                Level::WARN,
                                tx_hash = %tx_hash,
                                error = %err,
                                "head read failed during bundle observation; retrying",
                            ),
                        },
                    }
                }
                Err(err) => event!(
                    name: "eez.submitter.observe.receipt_read_failed",
                    Level::WARN,
                    tx_hash = %tx_hash,
                    error = %err,
                    "receipt read failed during bundle observation; retrying",
                ),
            }
            if !slow_logged && start.elapsed() >= TARGET_WAIT_BUDGET {
                slow_logged = true;
                // Sanity breadcrumb only — NOT a verdict. The loop
                // keeps going; death is decided solely by the chain
                // passing the target block.
                event!(
                    name: "eez.submitter.observe.slow",
                    Level::WARN,
                    tx_hash = %tx_hash,
                    target_block = target.block,
                    elapsed_secs = start.elapsed().as_secs(),
                    "bundle observation exceeding budget; still polling (verdict requires inclusion or target passage)",
                );
            }
            tokio::time::sleep(TARGET_POLL_INTERVAL).await;
        }
    }
}

/// POST a multi-tx bundle to a Flashbots-style `eth_sendBundle` relay.
/// Returns an optional, strictly decoded relay bundle hash on a successful reply
/// (any non-error `result` — most relays use `{"result": {"bundleHash":
/// "0x..."}}`, the chiado builder returns `{"result": null}`). Errors
/// surface relay-side rejections as [`L1Error::Submission`].
///
/// `target_block` is encoded as `0x{hex}` in the bundle params. Caller
/// is responsible for picking a not-yet-proposed block — the relay
/// silently drops bundles aimed at already-built blocks.
///
/// # Errors
///
/// - [`L1Error::Provider`] on transport (DNS, TCP, JSON decode) failure.
/// - [`L1Error::Submission`] when the relay HTTP status is non-2xx OR
///   the JSON body carries an `error` field.
async fn post_bundle(
    http: &reqwest::Client,
    builder_rpc_url: &str,
    raw_tx_hexes: &[&str],
    target_block: u64,
    pin_timestamp: u64,
    reverting_hashes: &[B256],
) -> L1Result<Option<B256>> {
    // Permit only the validated signed L1 users to revert; postBatch remains
    // required. Invalid-transaction omission depends on builder configuration,
    // and every user still requires canonical source reconciliation.
    let body = bundle_request(raw_tx_hexes, target_block, pin_timestamp, reverting_hashes);
    let resp: serde_json::Value = http
        .post(builder_rpc_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| L1Error::Provider(format!("eth_sendBundle POST: {e}")))?
        .error_for_status()
        .map_err(|e| L1Error::Submission(format!("eth_sendBundle HTTP: {e}")))?
        .json()
        .await
        .map_err(|e| L1Error::Provider(format!("eth_sendBundle decode: {e}")))?;
    event!(
        name: "eez.submitter.bundle.sent",
        Level::INFO,
        target_block,
        tx_count = raw_tx_hexes.len(),
        response = %resp,
        "eth_sendBundle response received",
    );
    if let Some(err) = resp.get("error") {
        // JSON-RPC -32601 = method not found: the configured relay is a
        // plain execution RPC (dev reth, anvil) with no bundle API.
        // Distinguish an unsupported relay from a rejected bundle.
        if err.get("code").and_then(serde_json::Value::as_i64) == Some(-32601) {
            return Err(L1Error::BundleRpcUnsupported);
        }
        return Err(L1Error::Submission(format!("eth_sendBundle: {err}")));
    }
    // Correlation metadata is optional: null and unfamiliar success payloads
    // keep their existing acknowledgement semantics. Never retain arbitrary
    // relay strings in structured delivery history.
    Ok(resp
        .get("result")
        .and_then(|result| result.get("bundleHash"))
        .and_then(serde_json::Value::as_str)
        .and_then(|hash| hash.parse().ok()))
}

/// Revert permission is limited to signed L1 users actually following the
/// required postBatch. Permission to drop invalid transactions is separate and
/// is not requested by this submitter.
fn inbound_revert_hashes(
    raw_txs: &[alloy_primitives::Bytes],
    chain_id: u64,
    post_batch_hash: B256,
) -> L1Result<Vec<B256>> {
    use alloy_consensus::{Transaction, transaction::SignerRecoverable};
    let mut seen = std::collections::HashSet::from([post_batch_hash]);
    let mut hashes = Vec::with_capacity(raw_txs.len().saturating_sub(1));
    for raw in raw_txs.iter().skip(1) {
        let mut encoded = raw.as_ref();
        let tx = alloy_consensus::TxEnvelope::decode_2718(&mut encoded)
            .map_err(|error| L1Error::Submission(format!("partial inbound envelope: {error}")))?;
        if !encoded.is_empty() || tx.chain_id() != Some(chain_id) {
            return Err(L1Error::Submission(
                "partial bundle user must be an exact signed envelope for the L1 source chain"
                    .into(),
            ));
        }
        tx.recover_signer()
            .map_err(|error| L1Error::Submission(format!("partial inbound signature: {error}")))?;
        let hash = *tx.tx_hash();
        if !seen.insert(hash) {
            return Err(L1Error::Submission(
                "duplicate or required postBatch hash in partial user list".into(),
            ));
        }
        hashes.push(hash);
    }
    Ok(hashes)
}

fn bundle_request(
    raw_tx_hexes: &[&str],
    target_block: u64,
    pin_timestamp: u64,
    reverting_hashes: &[B256],
) -> serde_json::Value {
    // Pin inclusion to the exact L1 slot the L2 Sync block anchored to.
    let mut bundle_params = serde_json::json!({
        "txs": raw_tx_hexes,
        "blockNumber": format!("0x{target_block:x}"),
        "minTimestamp": pin_timestamp,
        "maxTimestamp": pin_timestamp,
    });
    if !reverting_hashes.is_empty() {
        bundle_params["revertingTxHashes"] = serde_json::json!(reverting_hashes);
    }
    serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "eth_sendBundle",
        "params": [bundle_params],
    })
}

/// Verdict for a relay-submitted, timestamp-pinned bundle, read off the
/// canonical block at the pinned height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinnedVerdict {
    /// Height not built (or not visible) yet — no verdict.
    Pending,
    /// Our postBatch is in it; the receipt follows.
    Included,
    /// Built without our required postBatch; the bundle's target has expired.
    Excluded,
    /// Another timestamp filled the height. Timestamps strictly increase, so
    /// no later block can satisfy `minTimestamp == maxTimestamp == pin_ts`.
    SlotSkipped,
    /// A catch-up target's canonical parent changed after dispatch.
    ParentChanged,
}

/// `block` is `(timestamp, contains our tx, parent hash)` at the pinned height,
/// `None` while unobservable. Inclusion outranks every mismatch — in is in.
fn pinned_verdict(
    block: Option<(u64, bool, B256)>,
    pin_ts: u64,
    expected_parent_hash: Option<B256>,
) -> PinnedVerdict {
    match block {
        None => PinnedVerdict::Pending,
        Some((_, true, _)) => PinnedVerdict::Included,
        Some((ts, false, _)) if ts != pin_ts => PinnedVerdict::SlotSkipped,
        Some((_, false, parent_hash))
            if expected_parent_hash.is_some_and(|expected| expected != parent_hash) =>
        {
            PinnedVerdict::ParentChanged
        }
        Some(_) => PinnedVerdict::Excluded,
    }
}

/// Read the canonical block at the pinned height (hashes only) and apply
/// [`pinned_verdict`]. RPC failures yield `Pending`, never a verdict.
async fn pinned_slot_check<P: Provider>(
    provider: &P,
    target: PreparedTarget,
    tx_hash: TxHash,
) -> PinnedVerdict {
    match provider
        .get_block_by_number(BlockNumberOrTag::Number(target.block))
        .hashes()
        .await
    {
        Ok(block) => pinned_verdict(
            block.map(|b| {
                (
                    b.header.timestamp,
                    b.transactions.hashes().any(|h| h == tx_hash),
                    b.header.parent_hash,
                )
            }),
            target.timestamp,
            target.expected_parent_hash,
        ),
        Err(err) => {
            event!(
                name: "eez.submitter.observe.pinned_block_read_failed",
                Level::WARN,
                tx_hash = %tx_hash,
                target_block = target.block,
                error = %err,
                "pinned block read failed during bundle observation; retrying",
            );
            PinnedVerdict::Pending
        }
    }
}

fn dropped(tx_hash: TxHash, target_block: u64, reason: BundleDropReason) -> SendOutcome {
    event!(
        name: "eez.submitter.bundle.dropped",
        Level::WARN,
        target_block,
        tx_hash = %tx_hash,
        reason = reason.as_str(),
        "bundle dropped; composer will retry on next tick",
    );
    SendOutcome::Dropped {
        tx_hash,
        target_block,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use alloy_primitives::B256;
    use alloy_provider::ProviderBuilder;
    use alloy_transport::mock::Asserter;

    use crate::config::DEFAULT_BUNDLE_SUBMISSION_LEAD_MS;

    use super::{
        BundleDropReason, BundleTarget, Clock, L1Error, PinnedVerdict, PreparedTarget,
        SubmissionTiming, SubmissionWindowState, TargetPreparation, bundle_request, pinned_verdict,
        prepare_submission_target, submission_window_bounds, submission_window_state,
    };

    const PIN: u64 = 1_700_000_012;

    fn test_submitter(url: url::Url) -> super::Submitter {
        super::Submitter::new(
            crate::SubmitterConfig {
                reader: crate::L1ReaderConfig {
                    rpc_url: url.clone(),
                    beacon_url: None,
                    eez: alloy_primitives::Address::ZERO,
                    rollup_id: 1,
                    deploy_block: 0,
                },
                builder_rpc_url: url.clone(),
                target_rpc_url: Some(url),
                poster: B256::with_last_byte(1).to_string().parse().unwrap(),
                bundle_capture_dir: None,
                bundle_submission_lead: Duration::from_millis(DEFAULT_BUNDLE_SUBMISSION_LEAD_MS),
            },
            Duration::from_secs(12),
            Duration::from_millis(500),
        )
        .unwrap()
    }

    async fn rpc_server(
        responses: Vec<serde_json::Value>,
        keep_alive: bool,
    ) -> (url::Url, tokio::task::JoinHandle<Vec<serde_json::Value>>) {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            let mut connection = None;
            for mut response in responses {
                let mut stream = match connection.take() {
                    Some(stream) => stream,
                    None => BufReader::new(listener.accept().await.unwrap().0),
                };
                let mut length = None;
                loop {
                    let mut line = String::new();
                    assert!(stream.read_line(&mut line).await.unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
                let mut body = vec![0; length.expect("JSON RPC request length")];
                stream.read_exact(&mut body).await.unwrap();
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                response["jsonrpc"] = serde_json::json!("2.0");
                response["id"] = request["id"].clone();
                let body = serde_json::to_vec(&response).unwrap();
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: {}\r\n\r\n",
                    body.len(),
                    if keep_alive { "keep-alive" } else { "close" },
                );
                stream.get_mut().write_all(header.as_bytes()).await.unwrap();
                stream.get_mut().write_all(&body).await.unwrap();
                requests.push(request);
                if keep_alive {
                    connection = Some(stream);
                }
            }
            requests
        });
        (url, server)
    }

    #[tokio::test]
    async fn deferred_proof_target_is_pinned_once_with_the_network_slot_time() {
        for slot_secs in [5, 12] {
            let (url, server) = rpc_server(
                vec![serde_json::json!({
                    "result": mock_block(41, PIN, B256::ZERO)
                })],
                false,
            )
            .await;
            let mut submitter = test_submitter(url);
            std::sync::Arc::get_mut(&mut submitter.inner)
                .unwrap()
                .submission_timing = timing(slot_secs * 1_000);
            let target = submitter
                .resolve_target(BundleTarget::NextBlock)
                .await
                .unwrap();
            assert_eq!(
                target,
                BundleTarget::Exact {
                    block: 43,
                    timestamp: PIN + 2 * slot_secs
                }
            );
            let requests = server.await.unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0]["method"], "eth_getBlockByNumber");
            // The RPC is gone: an already pinned target must never be retargeted.
            assert_eq!(submitter.resolve_target(target).await.unwrap(), target);
        }
    }

    #[tokio::test]
    async fn deferred_proof_target_refuses_missing_heads_and_overflow() {
        for result in [
            serde_json::Value::Null,
            serde_json::to_value(mock_block(u64::MAX, PIN, B256::ZERO)).unwrap(),
            serde_json::to_value(mock_block(41, u64::MAX, B256::ZERO)).unwrap(),
        ] {
            let (url, server) =
                rpc_server(vec![serde_json::json!({ "result": result })], false).await;
            assert!(
                test_submitter(url)
                    .resolve_target(BundleTarget::NextBlock)
                    .await
                    .is_err()
            );
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn every_submission_records_dispatch_and_requires_a_relay() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        for (user_count, fail_record) in [(0, false), (2, false), (2, true)] {
            let now = super::SystemClock.now_unix_ms().unwrap() / 1_000;
            let timestamp = now + 30;
            let mut responses = vec![
                serde_json::json!({"result": "0x1"}),
                serde_json::json!({"result": mock_block(41, now, B256::ZERO)}),
            ];
            if !fail_record {
                responses.push(serde_json::json!({
                    "error": {"code": -32601, "message": "method not found"}
                }));
            }
            let (url, server) = rpc_server(responses, false).await;
            let mut submitter = test_submitter(url);
            // Keep this transport test inside the send window without sleeps.
            Arc::get_mut(&mut submitter.inner)
                .unwrap()
                .submission_timing
                .lead_ms = 60_000;
            let signed = (0..=user_count)
                .map(|nonce| signed_user(1, nonce))
                .collect::<Vec<_>>();
            let raw = signed
                .iter()
                .map(|(raw, _)| raw.clone())
                .collect::<Vec<_>>();
            let recorded = AtomicBool::new(false);
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                submitter.send_bundle_recorded(
                    &raw,
                    BundleTarget::Exact {
                        block: 42,
                        timestamp,
                    },
                    |block, pin| {
                        assert_eq!((block, pin), (42, timestamp));
                        recorded.store(true, Ordering::SeqCst);
                        if fail_record {
                            Err(L1Error::Submission("dispatch recording failed".into()))
                        } else {
                            Ok(())
                        }
                    },
                ),
            )
            .await
            .expect("submission must finish without observing or falling back to mempool");
            assert!(recorded.load(Ordering::SeqCst));
            if fail_record {
                assert!(
                    matches!(result, Err(L1Error::Submission(message)) if message == "dispatch recording failed")
                );
            } else {
                assert!(
                    matches!(result, Err(L1Error::BundleRpcUnsupported)),
                    "{result:?}"
                );
            }
            let requests = server.await.unwrap();
            assert_eq!(requests[0]["method"], "eth_chainId");
            assert_eq!(requests[1]["method"], "eth_getBlockByNumber");
            if !fail_record {
                let hashes = signed
                    .iter()
                    .skip(1)
                    .map(|(_, hash)| *hash)
                    .collect::<Vec<_>>();
                let hexes = raw.iter().map(ToString::to_string).collect::<Vec<_>>();
                let hex_refs = hexes.iter().map(String::as_str).collect::<Vec<_>>();
                assert_eq!(
                    requests[2],
                    bundle_request(&hex_refs, 42, timestamp, &hashes)
                );
            }
        }
    }

    #[tokio::test]
    async fn quoted_dispatch_checks_network_head_and_target_before_recording_or_sending() {
        use crate::posting_fees::FeeQuote;
        use std::sync::{Arc, atomic::AtomicBool};
        for mismatch in ["none", "chain", "hash", "height", "target"] {
            let now = super::SystemClock.now_unix_ms().unwrap() / 1_000;
            let target = BundleTarget::Exact {
                block: 42,
                timestamp: now + 30,
            };
            let hash = B256::repeat_byte(41);
            let mut quote = FeeQuote {
                chain_id: 1,
                sample: crate::L1BlockRef { number: 41, hash },
                target,
                max_priority_fee_per_gas: 2,
                max_fee_per_gas: 10,
                max_fee_per_blob_gas: 3,
                estimated_effective_gas_price: 4,
                estimated_blob_gas_price: 1,
            };
            match mismatch {
                "chain" => quote.chain_id = 10200,
                "hash" => quote.sample.hash = B256::repeat_byte(40),
                "height" => quote.sample.number = 40,
                "target" => {
                    quote.target = BundleTarget::Exact {
                        block: 42,
                        timestamp: now + 31,
                    }
                }
                _ => {}
            }
            let mut responses = vec![
                serde_json::json!({"result": "0x1"}),
                serde_json::json!({"result": mock_block(41, now, hash)}),
                serde_json::json!({"result": mock_block(41, now, hash)}),
            ];
            if mismatch == "none" {
                responses.push(
                    serde_json::json!({"error": {"code": -32601, "message": "method not found"}}),
                );
            }
            let (url, server) = rpc_server(responses, false).await;
            let mut submitter = test_submitter(url);
            Arc::get_mut(&mut submitter.inner)
                .unwrap()
                .submission_timing
                .lead_ms = 60_000;
            let raw = vec![signed_user(1, 0).0, signed_user(1, 1).0];
            let recorded = AtomicBool::new(false);
            let outcome = tokio::time::timeout(
                Duration::from_secs(5),
                submitter.send_bundle_quoted(&raw, target, quote, |_, _| {
                    recorded.store(true, Ordering::SeqCst);
                    Ok(())
                }),
            )
            .await
            .unwrap();
            assert_eq!(recorded.load(Ordering::SeqCst), mismatch == "none");
            if mismatch == "none" {
                assert!(outcome.is_err());
            } else {
                assert!(
                    matches!(
                        outcome,
                        Ok(super::SendOutcome::Dropped {
                            reason: super::BundleDropReason::TargetChanged,
                            ..
                        })
                    ),
                    "{mismatch}: {outcome:?}"
                );
            }
            let requests = server.await.unwrap();
            assert_eq!(requests.len(), if mismatch == "none" { 4 } else { 3 });
            if mismatch == "none" {
                assert_eq!(requests[3]["method"], "eth_sendBundle");
                assert_eq!(
                    requests[3]["params"][0]["txs"],
                    serde_json::json!(raw.iter().map(ToString::to_string).collect::<Vec<_>>())
                );
            }
        }
    }

    #[tokio::test]
    async fn delivery_deadline_bounds_acknowledgement_without_claiming_exclusion() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let (received, request) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = vec![0; 8192];
            let count = stream.read(&mut bytes).await.unwrap();
            assert!(count > 0);
            received.send(()).unwrap();
            // Headers arrive, but the body never completes. Sending the request
            // already crossed the external boundary; timeout is uncertainty.
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
                .await
                .unwrap();
            std::future::pending::<()>().await;
        });
        let submitter = test_submitter(url);
        let (post, hash) = signed_user(1, 0);
        let observed = std::sync::Arc::new(std::sync::Mutex::new(None));
        let saved = observed.clone();
        let result = tokio::spawn(async move {
            submitter
                .inner
                .dispatch_and_observe(
                    &[post],
                    hash,
                    PreparedTarget {
                        block: 42,
                        timestamp: PIN,
                        expected_parent_hash: None,
                    },
                    &[],
                    Some(super::DeliveryWindow {
                        deadline: tokio::time::Instant::now() + Duration::from_millis(300),
                        observed: Box::new(move |delivery| *saved.lock().unwrap() = Some(delivery)),
                    }),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), request)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            result.await.unwrap(),
            Err(crate::L1Error::Submission(_))
        ));
        assert_eq!(
            observed
                .lock()
                .unwrap()
                .as_ref()
                .map(|delivery| delivery.acknowledged),
            Some(false)
        );
        let delivery = observed.lock().unwrap().unwrap();
        assert!(delivery.relay_bundle_hash.is_none());
        assert!(delivery.remaining.is_zero());
        assert_eq!(
            (
                delivery.target_block,
                delivery.target_timestamp,
                delivery.transaction_count
            ),
            (42, PIN, 1)
        );
        server.abort();
    }

    #[tokio::test]
    async fn relay_correlation_is_optional_and_does_not_change_rejections() {
        let hash = B256::repeat_byte(0xab);
        for (response, expected, rejected) in [
            (serde_json::json!({"result": null}), None, false),
            (
                serde_json::json!({"result": {"bundleHash": hash}}),
                Some(hash),
                false,
            ),
            (
                serde_json::json!({"result": {"bundleHash": "unbounded relay text"}}),
                None,
                false,
            ),
            (
                serde_json::json!({"result": {"bundleHash": hash}, "error": {"code": -32000, "message": "rejected"}}),
                None,
                true,
            ),
        ] {
            let (url, server) = rpc_server(vec![response], false).await;
            let result = super::post_bundle(
                &reqwest::Client::new(),
                url.as_str(),
                &["0x01"],
                42,
                PIN,
                &[],
            )
            .await;
            if rejected {
                assert!(matches!(result, Err(L1Error::Submission(_))));
            } else {
                assert_eq!(result.unwrap(), expected);
            }
            let requests = server.await.unwrap();
            assert_eq!(requests[0]["params"][0]["minTimestamp"], PIN);
            assert_eq!(requests[0]["params"][0]["maxTimestamp"], PIN);
        }
    }

    #[tokio::test]
    async fn dispatch_capture_preserves_outcomes_and_replay_requests() {
        struct CaptureDirectory(std::path::PathBuf);
        impl Drop for CaptureDirectory {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "eez-bundle-capture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&path).unwrap();
        let directory = CaptureDirectory(path);
        let (post, post_hash) = signed_user(1, 0);
        let (user, user_hash) = signed_user(1, 1);
        let raw = [post, user];
        let target = PreparedTarget {
            block: 42,
            timestamp: PIN,
            expected_parent_hash: Some(B256::ZERO),
        };
        let block_hash = B256::with_last_byte(1);
        for outcome in ["included", "dropped", "submission_error"] {
            for capture in ["enabled", "disabled", "unwritable"] {
                let responses = if outcome == "submission_error" {
                    vec![serde_json::json!({
                        "error": {"code": -32601, "message": "method not found"}
                    })]
                } else {
                    let receipt = if outcome == "included" {
                        serde_json::json!({
                            "transactionHash": post_hash, "transactionIndex": "0x0",
                            "blockHash": block_hash, "blockNumber": "0x2a",
                            "from": alloy_primitives::Address::ZERO,
                            "to": alloy_primitives::Address::ZERO,
                            "gasUsed": "0x5208", "effectiveGasPrice": "0x1",
                            "cumulativeGasUsed": "0x5208", "type": "0x2", "status": "0x1",
                            "logs": [], "logsBloom": alloy_primitives::Bloom::ZERO,
                        })
                    } else {
                        serde_json::Value::Null
                    };
                    let mut block = serde_json::to_value(mock_block(42, PIN, block_hash)).unwrap();
                    if outcome == "included" {
                        block["transactions"] = serde_json::json!([post_hash, user_hash]);
                    }
                    vec![
                        serde_json::json!({"result": null}),
                        serde_json::json!({"result": receipt}),
                        serde_json::json!({"result": block}),
                    ]
                };
                let (url, server) = rpc_server(responses, false).await;
                let mut submitter = test_submitter(url.clone());
                let root = directory.0.join(format!("{outcome}-{capture}"));
                if capture == "unwritable" {
                    std::fs::write(&root, "not a directory").unwrap();
                }
                std::sync::Arc::get_mut(&mut submitter.inner)
                    .unwrap()
                    .config
                    .bundle_capture_dir = (capture != "disabled").then(|| root.clone());
                let result = tokio::time::timeout(
                    Duration::from_secs(5),
                    submitter.inner.dispatch_and_observe(
                        &raw,
                        post_hash,
                        target,
                        &[user_hash],
                        None,
                    ),
                )
                .await
                .expect("capture must not interrupt dispatch or observation");
                match (&result, outcome) {
                    (
                        Ok(super::SendOutcome::Included {
                            tx_hash,
                            l1_block,
                            l1_block_hash,
                        }),
                        "included",
                    ) => {
                        assert_eq!(
                            (*tx_hash, *l1_block, *l1_block_hash),
                            (post_hash, 42, block_hash)
                        );
                    }
                    (
                        Ok(super::SendOutcome::Dropped {
                            tx_hash,
                            target_block,
                            reason,
                        }),
                        "dropped",
                    ) => {
                        assert_eq!(
                            (*tx_hash, *target_block, *reason),
                            (post_hash, 42, BundleDropReason::Excluded)
                        );
                    }
                    (Err(L1Error::BundleRpcUnsupported), "submission_error") => {}
                    _ => panic!("unexpected result for {outcome}/{capture}: {result:?}"),
                }
                let requests = server.await.unwrap();
                assert_eq!(requests[0]["method"], "eth_sendBundle");
                if capture == "unwritable" {
                    assert_eq!(std::fs::read(&root).unwrap(), b"not a directory");
                } else if capture == "disabled" || outcome == "included" {
                    assert!(
                        !root.exists(),
                        "successful or disabled capture must write nothing"
                    );
                } else {
                    let attempt = root.join(format!("bundle-42-{post_hash}"));
                    assert_eq!(
                        std::fs::read(attempt.join("request.json")).unwrap(),
                        serde_json::to_vec_pretty(&requests[0]).unwrap(),
                    );
                    let expected = serde_json::json!({
                        "version": 1, "builderRpcUrl": url.as_str(),
                        "postBatchHash": post_hash.to_string(), "targetBlock": 42,
                        "pinnedTimestamp": PIN, "expectedParentHash": B256::ZERO.to_string(),
                        "outcome": outcome,
                        "dropReason": (outcome == "dropped").then_some("excluded"),
                        "error": (outcome == "submission_error").then_some("builder relay does not support eth_sendBundle"),
                        "requestFile": "request.json",
                    });
                    assert_eq!(
                        std::fs::read(attempt.join("metadata.json")).unwrap(),
                        serde_json::to_vec_pretty(&expected).unwrap(),
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn observer_requires_canonical_inclusion_without_scanning_settlement() {
        for case in [
            "success",
            "reverted",
            "missing number",
            "missing hash",
            "reorged",
            "missing block",
        ] {
            let tx_hash = B256::with_last_byte(3);
            let block_hash = B256::with_last_byte(1);
            let mut receipt = serde_json::json!({
                "transactionHash": tx_hash, "transactionIndex": "0x2",
                "blockHash": block_hash, "blockNumber": "0x2a",
                "from": alloy_primitives::Address::ZERO, "to": alloy_primitives::Address::ZERO,
                "gasUsed": "0x5208", "effectiveGasPrice": "0x1", "cumulativeGasUsed": "0x5208",
                "type": "0x2", "status": if case == "reverted" { "0x0" } else { "0x1" },
                "logs": [], "logsBloom": alloy_primitives::Bloom::ZERO,
            });
            let expected_error = match case {
                "missing number" => {
                    receipt["blockNumber"] = serde_json::Value::Null;
                    Some("receipt present but block_number missing")
                }
                "missing hash" => {
                    receipt["blockHash"] = serde_json::Value::Null;
                    Some("receipt present but block_hash missing")
                }
                "reorged" | "missing block" => {
                    Some("L1 block 42 is missing or no longer canonical")
                }
                _ => None,
            };
            let mut responses = vec![serde_json::json!({"result": receipt})];
            if !matches!(case, "missing number" | "missing hash") {
                let block = if case == "missing block" {
                    serde_json::Value::Null
                } else {
                    serde_json::to_value(mock_block(
                        42,
                        PIN,
                        if case == "reorged" {
                            B256::ZERO
                        } else {
                            block_hash
                        },
                    ))
                    .unwrap()
                };
                responses.push(serde_json::json!({"result": block}));
            }
            let (url, server) = rpc_server(responses, false).await;
            let submitter = test_submitter(url);
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                submitter.inner.observe(
                    tx_hash,
                    PreparedTarget {
                        block: 42,
                        timestamp: PIN,
                        expected_parent_hash: None,
                    },
                ),
            )
            .await
            .expect("inclusion observation must not depend on settlement log RPCs");
            if let Some(expected) = expected_error {
                assert!(matches!(result, Err(L1Error::Provider(message)) if message == expected));
            } else {
                // Even a reverted postBatch is included; only canonical
                // reconciliation can decide whether any user completed.
                assert!(matches!(result, Ok(super::SendOutcome::Included {
                    tx_hash: observed,
                    l1_block: 42,
                    l1_block_hash,
                }) if observed == tx_hash && l1_block_hash == block_hash));
            }
            let requests = server.await.unwrap();
            assert_eq!(requests[0]["method"], "eth_getTransactionReceipt");
            if requests.len() == 2 {
                assert_eq!(requests[1]["method"], "eth_getBlockByNumber");
                assert_eq!(requests[1]["params"][0], "0x2a");
            }
        }
    }

    #[tokio::test]
    async fn canonical_provider_reuses_connections_without_caching_chain_state() {
        for override_target in [false, true] {
            let first = mock_block(42, PIN, B256::with_last_byte(1));
            let replacement = mock_block(42, PIN, B256::with_last_byte(2));
            let (url, server) = rpc_server(
                vec![
                    serde_json::json!({"result": first}),
                    serde_json::json!({"error": {"code": -32000, "message": "unavailable"}}),
                    serde_json::json!({"result": replacement}),
                    serde_json::json!({"result": null}),
                ],
                true,
            )
            .await;
            let mut config = test_submitter(url.clone()).inner.config.clone();
            config.target_rpc_url = override_target.then_some(url);
            if override_target {
                config.reader.rpc_url = "http://127.0.0.1:0".parse().unwrap();
            }
            let submitter =
                super::Submitter::new(config, Duration::from_secs(12), Duration::from_millis(500))
                    .unwrap();
            let shared = submitter.clone();
            // The server accepts only one connection. Each call must still
            // make a fresh RPC, including after an error or same-height reorg.
            tokio::time::timeout(Duration::from_secs(5), async {
                assert_eq!(
                    submitter.canonical_block_hash(42).await.unwrap(),
                    Some(first.header.hash)
                );
                assert!(matches!(
                    shared.canonical_block_hash(42).await,
                    Err(L1Error::Provider(_))
                ));
                assert_eq!(
                    shared.canonical_block_hash(42).await.unwrap(),
                    Some(replacement.header.hash)
                );
                assert!(!submitter.receipt_exists(B256::ZERO).await.unwrap());
                let requests = server.await.unwrap();
                assert_eq!(requests.len(), 4);
                for request in &requests[..3] {
                    assert_eq!(request["method"], "eth_getBlockByNumber");
                    assert_eq!(request["params"][0], "0x2a");
                }
                assert_eq!(requests[3]["method"], "eth_getTransactionReceipt");
            })
            .await
            .expect("canonical reads must reuse the live HTTP connection");
        }
    }

    #[tokio::test]
    async fn canonical_source_reads_bound_stalled_headers_and_response_bodies() {
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        for stalled_body in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url: url::Url = format!("http://{}", listener.local_addr().unwrap())
                .parse()
                .unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert!(stream.read(&mut [0; 1024]).await.unwrap() > 0);
                if stalled_body {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
                        .await
                        .unwrap();
                }
                std::future::pending::<()>().await;
            });
            let mut submitter = test_submitter(url);
            let inner = Arc::get_mut(&mut submitter.inner).unwrap();
            inner.target_provider =
                super::Inner::build_target_provider(&inner.config, Duration::from_millis(100));
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                submitter.source_state_at(
                    B256::ZERO,
                    alloy_primitives::Address::ZERO,
                    crate::L1BlockRef {
                        number: 100,
                        hash: B256::with_last_byte(2),
                    },
                ),
            )
            .await;
            server.abort();
            assert!(matches!(
                result.expect("source reads must use the configured HTTP timeout"),
                Err(L1Error::Provider(_))
            ));
        }
    }

    fn signed_user(chain_id: u64, nonce: u64) -> (alloy_primitives::Bytes, B256) {
        use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
        use alloy_eips::Encodable2718;
        use alloy_network::TxSignerSync;
        use alloy_primitives::TxKind;
        let signer: alloy_signer_local::PrivateKeySigner =
            B256::with_last_byte(1).to_string().parse().unwrap();
        let mut tx = TxEip1559 {
            chain_id,
            nonce,
            gas_limit: 100_000,
            max_fee_per_gas: 10,
            max_priority_fee_per_gas: 1,
            to: TxKind::Call(signer.address()),
            ..Default::default()
        };
        let signature = signer.sign_transaction_sync(&mut tx).unwrap();
        let tx = TxEnvelope::Eip1559(tx.into_signed(signature));
        (tx.encoded_2718().into(), *tx.tx_hash())
    }

    #[test]
    fn partial_request_only_allows_actual_signed_inbound_users_to_revert() {
        let (first, first_hash) = signed_user(1, 0);
        let (second, second_hash) = signed_user(1, 1);
        // The leading postBatch may have a pooled blob sidecar; the allowlist
        // must leave these bytes alone and hash only the actual L1 users.
        let post = alloy_primitives::Bytes::from_static(&[3, 0xc0]);
        let hashes = super::inbound_revert_hashes(
            &[post.clone(), first.clone(), second.clone()],
            1,
            B256::ZERO,
        )
        .unwrap();
        assert_eq!(hashes, vec![first_hash, second_hash]);
        let hexes = [post.to_string(), first.to_string(), second.to_string()];
        let raws = hexes.iter().map(String::as_str).collect::<Vec<_>>();
        let request = bundle_request(&raws, 42, PIN, &hashes);
        let params = &request["params"][0];
        assert_eq!(
            params["revertingTxHashes"],
            serde_json::json!([first_hash, second_hash])
        );
        assert!(params.get("droppingTxHashes").is_none());
        assert_eq!(params["txs"], serde_json::json!(hexes));
        assert_eq!(params["minTimestamp"], PIN);
        assert_eq!(params["maxTimestamp"], PIN);
        let post_only = bundle_request(&raws[..1], 42, PIN, &[]);
        assert!(post_only["params"][0].get("revertingTxHashes").is_none());
    }

    #[test]
    fn inbound_revert_allowlist_rejects_wrong_chain_duplicate_and_malformed_users() {
        let (user, hash) = signed_user(1, 0);
        let post = alloy_primitives::Bytes::new();
        assert!(
            super::inbound_revert_hashes(&[post.clone(), user.clone()], 2, B256::ZERO).is_err()
        );
        assert!(super::inbound_revert_hashes(&[post.clone(), user.clone()], 1, hash).is_err());
        assert!(
            super::inbound_revert_hashes(
                &[post.clone(), user.clone(), user.clone()],
                1,
                B256::ZERO
            )
            .is_err()
        );
        let mut trailing = user.to_vec();
        trailing.push(0);
        assert!(
            super::inbound_revert_hashes(&[post.clone(), trailing.into()], 1, B256::ZERO).is_err()
        );
        assert!(
            super::inbound_revert_hashes(
                &[post.clone(), alloy_primitives::Bytes::new()],
                1,
                B256::ZERO
            )
            .is_err()
        );
        assert!(
            super::inbound_revert_hashes(&[post], 1, B256::ZERO)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn production_default_opens_at_seven_and_a_half_seconds() {
        assert_eq!(DEFAULT_BUNDLE_SUBMISSION_LEAD_MS, 7_500);
    }

    #[derive(Debug)]
    struct FakeClock {
        now_ms: AtomicU64,
        sleeps: Mutex<Vec<Duration>>,
    }

    impl FakeClock {
        fn new(now_ms: u64) -> Self {
            Self {
                now_ms: AtomicU64::new(now_ms),
                sleeps: Mutex::new(Vec::new()),
            }
        }

        fn sleeps(&self) -> Vec<Duration> {
            self.sleeps.lock().expect("sleep lock poisoned").clone()
        }
    }

    impl Clock for FakeClock {
        fn now_unix_ms(&self) -> super::L1Result<u64> {
            Ok(self.now_ms.load(Ordering::SeqCst))
        }

        fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send {
            self.sleeps
                .lock()
                .expect("sleep lock poisoned")
                .push(duration);
            self.now_ms.fetch_add(
                u64::try_from(duration.as_millis()).expect("test duration fits u64"),
                Ordering::SeqCst,
            );
            std::future::ready(())
        }
    }

    fn timing(block_ms: u64) -> SubmissionTiming {
        SubmissionTiming::new(
            Duration::from_millis(block_ms),
            Duration::from_millis(7_500),
            Duration::from_millis(500),
        )
        .unwrap()
    }

    fn mock_block(number: u64, timestamp: u64, hash: B256) -> alloy_rpc_types_eth::Block {
        let mut block: alloy_rpc_types_eth::Block = alloy_rpc_types_eth::Block::default();
        block.header.inner.number = number;
        block.header.inner.timestamp = timestamp;
        block.header.hash = hash;
        block
    }

    #[tokio::test]
    async fn skipped_slot_requires_a_strict_gap_between_canonical_headers() {
        for cadence in [5, 12] {
            for missed_slots in [1, 3] {
                for scan_number in [42, 45] {
                    let parent = mock_block(41, PIN, B256::with_last_byte(1));
                    let mut target = mock_block(
                        42,
                        PIN + (missed_slots + 1) * cadence,
                        B256::with_last_byte(2),
                    );
                    target.header.inner.parent_hash = parent.header.hash;
                    let scan = if scan_number == 42 {
                        target.clone()
                    } else {
                        mock_block(scan_number, PIN + 10 * cadence, B256::with_last_byte(3))
                    };
                    let replies = Asserter::new();
                    for block in [&scan, &target, &parent, &scan] {
                        replies.push_success(block);
                    }
                    let provider = ProviderBuilder::new().connect_mocked_client(replies);
                    assert!(
                        super::canonical_target_was_skipped(
                            &provider,
                            42,
                            PIN + cadence,
                            crate::L1BlockRef {
                                number: scan_number,
                                hash: scan.header.hash,
                            },
                        )
                        .await
                        .unwrap(),
                        "cadence={cadence}, missed_slots={missed_slots}, scan={scan_number}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn timestamp_mismatch_without_a_skipped_slot_does_not_exempt_backoff() {
        for (parent_ts, target_ts) in [
            (PIN - 5, PIN),
            (PIN - 10, PIN - 5),
            (PIN, PIN + 5),
            (PIN + 5, PIN + 10),
        ] {
            let parent = mock_block(41, parent_ts, B256::with_last_byte(1));
            let mut target = mock_block(42, target_ts, B256::with_last_byte(2));
            target.header.inner.parent_hash = parent.header.hash;
            let replies = Asserter::new();
            replies.push_success(&target);
            replies.push_success(&target);
            if target_ts > PIN {
                replies.push_success(&parent);
                replies.push_success(&target);
            }
            let provider = ProviderBuilder::new().connect_mocked_client(replies);
            assert!(
                !super::canonical_target_was_skipped(
                    &provider,
                    42,
                    PIN,
                    crate::L1BlockRef {
                        number: 42,
                        hash: target.header.hash
                    },
                )
                .await
                .unwrap(),
                "parent={parent_ts}, target={target_ts}"
            );
        }
    }

    #[tokio::test]
    async fn skipped_slot_evidence_rejects_unavailable_or_reorged_headers() {
        let parent = mock_block(41, PIN - 5, B256::with_last_byte(1));
        let mut target = mock_block(42, PIN + 5, B256::with_last_byte(2));
        target.header.inner.parent_hash = parent.header.hash;
        for failure in [
            "initial",
            "target",
            "target height",
            "target hash",
            "parent",
            "parent hash",
            "parent height",
            "final",
        ] {
            let replies = Asserter::new();
            if failure == "initial" {
                replies.push_success(&mock_block(42, PIN + 5, B256::with_last_byte(3)));
            } else {
                replies.push_success(&target);
                if failure == "target" {
                    replies.push_success(&serde_json::Value::Null);
                } else if failure == "target height" {
                    replies.push_success(&mock_block(43, PIN + 5, target.header.hash));
                } else if failure == "target hash" {
                    replies.push_success(&mock_block(42, PIN + 5, B256::ZERO));
                } else {
                    replies.push_success(&target);
                    match failure {
                        "parent" => replies.push_failure_msg("header unavailable"),
                        "parent hash" => replies.push_success(&mock_block(41, PIN - 5, B256::ZERO)),
                        "parent height" => {
                            replies.push_success(&mock_block(40, PIN - 5, parent.header.hash))
                        }
                        "final" => {
                            replies.push_success(&parent);
                            replies.push_success(&mock_block(42, PIN + 5, B256::with_last_byte(3)));
                        }
                        _ => unreachable!(),
                    }
                }
            }
            let provider = ProviderBuilder::new().connect_mocked_client(replies);
            assert!(
                super::canonical_target_was_skipped(
                    &provider,
                    42,
                    PIN,
                    crate::L1BlockRef {
                        number: 42,
                        hash: target.header.hash
                    },
                )
                .await
                .is_err(),
                "{failure} cannot prove a canonical skipped slot"
            );
        }
    }

    #[tokio::test]
    async fn skipped_slot_evidence_cannot_precede_the_completed_scan() {
        let provider = ProviderBuilder::new().connect_mocked_client(Asserter::new());
        for target in [0, 43] {
            assert!(
                !super::canonical_target_was_skipped(
                    &provider,
                    target,
                    PIN,
                    crate::L1BlockRef {
                        number: 42,
                        hash: B256::ZERO
                    },
                )
                .await
                .unwrap()
            );
        }
    }

    #[tokio::test]
    async fn canonical_source_reads_reject_reorgs_and_rpc_errors() {
        let block = mock_block(42, PIN, B256::with_last_byte(1));
        let at = crate::L1BlockRef {
            number: 42,
            hash: block.header.hash,
        };
        for failure in ["initial", "nonce", "receipt", "final"] {
            let replies = Asserter::new();
            if failure == "initial" {
                replies.push_success(&mock_block(42, PIN, B256::with_last_byte(2)));
            } else {
                replies.push_success(&block);
                if failure == "nonce" {
                    replies.push_failure_msg("source state unavailable");
                } else {
                    replies.push_success(&"0x0");
                    if failure == "receipt" {
                        replies.push_failure_msg("receipt index unavailable");
                    } else {
                        replies.push_success(&serde_json::Value::Null);
                        replies.push_success(&mock_block(42, PIN, B256::with_last_byte(2)));
                    }
                }
            }
            let provider = ProviderBuilder::new().connect_mocked_client(replies);
            let result = super::read_canonical_source(
                &provider,
                Default::default(),
                1,
                B256::with_last_byte(3),
                Default::default(),
                at,
            )
            .await;
            assert!(
                result.is_err(),
                "{failure} must retain ownership, not prove omission"
            );
        }
    }

    #[tokio::test]
    async fn canonical_source_distinguishes_absent_future_and_reverted_receipts() {
        let block = mock_block(42, PIN, B256::with_last_byte(1));
        let at = crate::L1BlockRef {
            number: 42,
            hash: block.header.hash,
        };
        let tx_hash = B256::with_last_byte(3);
        for kind in [
            "absent",
            "future",
            "reverted",
            "wrong hash",
            "missing index",
            "missing price",
        ] {
            let replies = Asserter::new();
            replies.push_success(&block);
            replies.push_success(&"0x1");
            if kind == "absent" {
                replies.push_success(&serde_json::Value::Null);
            } else {
                replies.push_success(&serde_json::json!({
                    "transactionHash": if kind == "wrong hash" { B256::ZERO } else { tx_hash },
                    "transactionIndex": if kind == "missing index" { None } else { Some("0x2") },
                    "blockHash": at.hash,
                    "blockNumber": if kind == "future" { "0x2b" } else { "0x2a" },
                    "from": alloy_primitives::Address::ZERO, "to": alloy_primitives::Address::ZERO,
                    "gasUsed": "0x5208", "effectiveGasPrice": if kind == "missing price" { None } else { Some("0x1") }, "cumulativeGasUsed": "0x5208",
                    "type": "0x2", "status": "0x0", "logs": [],
                    "logsBloom": alloy_primitives::Bloom::ZERO,
                }));
                if matches!(kind, "reverted" | "missing index" | "missing price") {
                    replies.push_success(&block);
                }
            }
            replies.push_success(&block);
            let provider = ProviderBuilder::new().connect_mocked_client(replies);
            let result = super::read_canonical_source(
                &provider,
                Default::default(),
                1,
                tx_hash,
                Default::default(),
                at,
            )
            .await;
            if matches!(kind, "future" | "wrong hash" | "missing index") {
                assert!(result.is_err(), "unresolved provenance is inconclusive");
            } else {
                let source = result.unwrap();
                assert_eq!(source.nonce, 1);
                if matches!(kind, "reverted" | "missing price") {
                    let receipt = source.receipt.unwrap();
                    assert!(!receipt.success, "included revert spends its nonce");
                    assert_eq!(receipt.block, at);
                    assert_eq!(receipt.transaction_index, 2);
                    assert!(receipt.consumed.is_empty());
                    if kind == "missing price" {
                        assert!(
                            receipt.cost.is_err(),
                            "missing price must not become free execution"
                        );
                    } else {
                        assert_eq!(
                            receipt.cost.unwrap().paid.total,
                            alloy_primitives::U256::from(21_000)
                        );
                    }
                } else {
                    assert!(source.receipt.is_none());
                }
            }
        }
    }

    #[test]
    fn submission_timing_rejects_empty_or_inverted_window() {
        let invalid = [
            (0, 7_500, 500),
            (12_500, 7_500, 500),
            (12_000, 500, 500),
            (12_000, 499, 500),
        ];
        for (block_ms, lead_ms, slack_ms) in invalid {
            assert!(
                SubmissionTiming::new(
                    Duration::from_millis(block_ms),
                    Duration::from_millis(lead_ms),
                    Duration::from_millis(slack_ms),
                )
                .is_err(),
                "accepted block={block_ms}, lead={lead_ms}, slack={slack_ms}",
            );
        }

        // A lead longer than the chain's slot is intentional on short-slot
        // networks: the window is already open when the builder job starts.
        assert!(
            SubmissionTiming::new(
                Duration::from_secs(5),
                Duration::from_millis(7_500),
                Duration::from_millis(700),
            )
            .is_ok()
        );
    }

    #[test]
    fn submission_window_has_exact_open_and_closed_boundaries() {
        let timing = timing(12_000);
        let (not_before, deadline) = submission_window_bounds(PIN, timing);
        assert_eq!(not_before, PIN * 1_000 - 7_500);
        assert_eq!(deadline, PIN * 1_000 - 500);
        assert_eq!(
            submission_window_state(not_before - 1, PIN, timing),
            SubmissionWindowState::Waiting(Duration::from_millis(1))
        );
        assert_eq!(
            submission_window_state(not_before, PIN, timing),
            SubmissionWindowState::Open
        );
        assert_eq!(
            submission_window_state(deadline - 1, PIN, timing),
            SubmissionWindowState::Open
        );
        assert_eq!(
            submission_window_state(deadline, PIN, timing),
            SubmissionWindowState::Expired
        );
    }

    #[tokio::test]
    async fn policy_target_uses_the_first_slot_with_a_complete_preparation_budget() {
        for (slot_ms, proof_ms, slack_ms) in [(5_000, 1_500, 700), (12_000, 4_000, 500)] {
            let timing = SubmissionTiming::new(
                Duration::from_millis(slot_ms),
                Duration::from_millis(7_500),
                Duration::from_millis(slack_ms),
            )
            .unwrap();
            let cutoff = slot_ms - proof_ms - slack_ms;
            for (elapsed_ms, offset) in [
                (0, 1),
                (cutoff - 1, 1),
                (cutoff, 2),
                (cutoff + 1, 2),
                (slot_ms, 2),
                (3 * slot_ms + cutoff, 5),
            ] {
                let replies = Asserter::new();
                replies.push_success(&mock_block(100, PIN, B256::with_last_byte(1)));
                let provider = ProviderBuilder::new().connect_mocked_client(replies);
                let clock = FakeClock::new(PIN * 1_000 + elapsed_ms);
                let target = super::select_policy_target(
                    &provider,
                    timing,
                    Duration::from_millis(proof_ms),
                    &clock,
                )
                .await
                .unwrap();
                assert_eq!(
                    target,
                    BundleTarget::Exact {
                        block: 100 + offset,
                        timestamp: PIN + offset * slot_ms / 1_000,
                    },
                    "slot={slot_ms}, elapsed={elapsed_ms}"
                );
                assert!(
                    clock.sleeps().is_empty(),
                    "selection must not stall sequencing"
                );
            }
        }
    }

    #[tokio::test]
    async fn policy_drain_can_prepare_and_submit_in_consecutive_l1_blocks() {
        for slot_ms in [5_000, 12_000] {
            let timing = SubmissionTiming::new(
                Duration::from_millis(slot_ms),
                Duration::from_millis(7_500),
                Duration::from_millis(700),
            )
            .unwrap();
            let mut selected = Vec::new();
            // Each preceding post has reconciled two seconds into its slot.
            // Follow selection through proof preparation and the real parent /
            // dispatch gate, without speculating on an unconfirmed predecessor.
            for index in 0..6 {
                let head = mock_block(
                    100 + index,
                    PIN + index * slot_ms / 1_000,
                    B256::with_last_byte(u8::try_from(index).unwrap()),
                );
                let clock = FakeClock::new(head.header.timestamp * 1_000 + 2_000);
                let replies = Asserter::new();
                replies.push_success(&head); // selection
                replies.push_success(&head); // canonical parent before final pricing
                replies.push_success(&head); // head recheck at dispatch window
                let provider = ProviderBuilder::new().connect_mocked_client(replies);
                let target = super::select_policy_target(
                    &provider,
                    timing,
                    Duration::from_millis(1_500),
                    &clock,
                )
                .await
                .unwrap();
                clock.sleep(Duration::from_millis(1_500)).await;
                assert!(
                    super::prepare_policy_submission(&provider, target, timing, &clock)
                        .await
                        .unwrap()
                );
                let BundleTarget::Exact { block, timestamp } = target else {
                    panic!("policy target must retain the exact timestamp pin");
                };
                assert_eq!(timestamp, head.header.timestamp + slot_ms / 1_000);
                selected.push(block);
            }
            assert_eq!(selected, vec![101, 102, 103, 104, 105, 106]);
        }
    }

    #[tokio::test]
    async fn policy_target_missing_head_or_overflow_fails_without_a_fallback() {
        let clock = FakeClock::new(PIN * 1_000);
        let replies = Asserter::new();
        replies.push_success(&serde_json::Value::Null);
        let provider = ProviderBuilder::new().connect_mocked_client(replies);
        assert!(
            super::select_policy_target(&provider, timing(5_000), Duration::ZERO, &clock)
                .await
                .unwrap_err()
                .to_string()
                .contains("head is unavailable")
        );
        for (height, timestamp, now_ms, preparation) in [
            (u64::MAX, PIN, PIN * 1_000, Duration::ZERO),
            (100, u64::MAX, PIN * 1_000, Duration::ZERO),
            (100, PIN, u64::MAX, Duration::ZERO),
            (100, PIN, PIN * 1_000, Duration::MAX),
            (100, u64::MAX / 1_000, 0, Duration::ZERO),
        ] {
            let replies = Asserter::new();
            replies.push_success(&mock_block(height, timestamp, B256::ZERO));
            let provider = ProviderBuilder::new().connect_mocked_client(replies);
            assert!(
                super::select_policy_target(
                    &provider,
                    timing(5_000),
                    preparation,
                    &FakeClock::new(now_ms),
                )
                .await
                .is_err()
            );
        }
    }

    #[tokio::test]
    async fn selected_policy_target_still_rejects_late_preparation_and_skipped_slots() {
        for late_preparation in [true, false] {
            let clock = FakeClock::new(PIN * 1_000 + 2_000);
            let replies = Asserter::new();
            replies.push_success(&mock_block(100, PIN, B256::with_last_byte(1)));
            if !late_preparation {
                // Same height, different slot after a head change. Never slide
                // the proof's timestamp to make it fit the new canonical chain.
                replies.push_success(&mock_block(100, PIN + 5, B256::with_last_byte(2)));
            }
            let provider = ProviderBuilder::new().connect_mocked_client(replies);
            let target = super::select_policy_target(
                &provider,
                timing(5_000),
                Duration::from_millis(1_500),
                &clock,
            )
            .await
            .unwrap();
            if late_preparation {
                clock.sleep(Duration::from_millis(2_500)).await;
            }
            assert!(
                !super::prepare_policy_submission(&provider, target, timing(5_000), &clock)
                    .await
                    .unwrap()
            );
        }
    }

    #[tokio::test]
    async fn policy_waits_for_chiado_parent_inside_an_already_open_window() {
        let clock = FakeClock::new(PIN * 1_000 - 6_000);
        let replies = Asserter::new();
        replies.push_success(&serde_json::Value::Null);
        replies.push_success(&serde_json::Value::Null);
        let parent = mock_block(41, PIN - 5, B256::with_last_byte(1));
        replies.push_success(&parent);
        replies.push_success(&parent);
        let provider = ProviderBuilder::new().connect_mocked_client(replies);
        assert!(
            super::prepare_policy_submission(
                &provider,
                BundleTarget::Exact {
                    block: 42,
                    timestamp: PIN
                },
                timing(5_000),
                &clock,
            )
            .await
            .unwrap()
        );
        assert_eq!(clock.sleeps(), vec![Duration::from_millis(500); 2]);
    }

    #[tokio::test]
    async fn replacement_gate_accepts_bounded_missing_slots_but_rejects_late_parent_and_larger_gaps()
     {
        for (distance, advanced, accepted) in [
            (5, false, true),
            (10, false, true),
            (10, true, false),
            (15, false, true),
            (20, false, true),
            (25, false, true),
            (25, true, false),
            (30, false, false),
            (0, false, false),
            (9, false, false),
            (24, false, false),
        ] {
            let replies = Asserter::new();
            let parent = mock_block(41, PIN - distance, B256::with_last_byte(1));
            replies.push_success(&parent);
            if matches!(distance, 5 | 10 | 15 | 20 | 25) {
                replies.push_success(&if advanced {
                    mock_block(42, PIN - 5, B256::with_last_byte(2))
                } else {
                    parent
                });
            }
            let provider = ProviderBuilder::new().connect_mocked_client(replies);
            assert_eq!(
                super::prepare_policy_submission(
                    &provider,
                    BundleTarget::Exact {
                        block: 42,
                        timestamp: PIN
                    },
                    timing(5_000),
                    &FakeClock::new(PIN * 1_000 - 4_000)
                )
                .await
                .unwrap(),
                accepted
            );
        }
    }

    #[test]
    fn policy_slot_offsets_reject_underflow_zero_slots_and_unaligned_targets() {
        for (parent, target, slot, expected) in [
            (100, 112, 12, Some(1)),
            (100, 124, 12, Some(2)),
            (100, 160, 12, Some(5)),
            (100, 172, 12, None),
            (100, 159, 12, None),
            (100, 99, 12, None),
            (100, 100, 12, None),
            (100, 112, 0, None),
            (u64::MAX - 25, u64::MAX, 5, Some(5)),
            (u64::MAX, 0, 5, None),
        ] {
            assert_eq!(
                super::policy_target_slot_offset(parent, target, slot),
                expected
            );
        }
    }

    #[tokio::test]
    async fn policy_gate_reports_specific_reason_without_changing_waits() {
        use super::PolicySubmissionFailure as Failure;
        let target = BundleTarget::Exact {
            block: 42,
            timestamp: PIN,
        };
        for (now, parent, expected) in [
            (PIN * 1000, None, Failure::WindowExpired),
            (PIN * 1000 - 1000, None, Failure::ParentUnavailable),
            (
                PIN * 1000 - 4000,
                Some(PIN - 4),
                Failure::ParentTimestampMismatch,
            ),
        ] {
            let replies = Asserter::new();
            if now < PIN * 1000 {
                replies.push_success(
                    &parent
                        .map(|time| {
                            serde_json::to_value(mock_block(41, time, B256::with_last_byte(1)))
                                .unwrap()
                        })
                        .unwrap_or(serde_json::Value::Null),
                );
            }
            let provider = ProviderBuilder::new().connect_mocked_client(replies);
            let clock = FakeClock::new(now);
            assert_eq!(
                super::policy_submission_failure(&provider, target, timing(5000), &clock)
                    .await
                    .unwrap(),
                Some(expected)
            );
            assert_eq!(
                clock.sleeps().len(),
                usize::from(expected == Failure::ParentUnavailable)
            );
        }
    }

    #[tokio::test]
    async fn policy_parent_wait_expires_and_rejects_changed_slot_timing() {
        let target = BundleTarget::Exact {
            block: 42,
            timestamp: PIN,
        };
        let clock = FakeClock::new(PIN * 1_000 - 1_000);
        let replies = Asserter::new();
        replies.push_success(&serde_json::Value::Null);
        let provider = ProviderBuilder::new().connect_mocked_client(replies);
        assert!(
            !super::prepare_policy_submission(&provider, target, timing(5_000), &clock)
                .await
                .unwrap()
        );
        assert_eq!(clock.sleeps(), vec![Duration::from_millis(500)]);

        for timestamp in [PIN - 6, PIN - 4] {
            let clock = FakeClock::new(PIN * 1_000 - 4_000);
            let replies = Asserter::new();
            replies.push_success(&mock_block(41, timestamp, B256::with_last_byte(1)));
            let provider = ProviderBuilder::new().connect_mocked_client(replies);
            assert!(
                !super::prepare_policy_submission(&provider, target, timing(5_000), &clock)
                    .await
                    .unwrap()
            );
            assert!(clock.sleeps().is_empty());
        }
    }

    #[tokio::test]
    async fn exact_target_is_retained_until_the_window_opens() {
        let target_block = 42;
        let clock = FakeClock::new(PIN * 1_000 - 9_000);
        let asserter = Asserter::new();
        asserter.push_success(&mock_block(
            target_block - 1,
            PIN - 12,
            B256::with_last_byte(1),
        ));
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = prepare_submission_target(
            &provider,
            BundleTarget::Exact {
                block: target_block,
                timestamp: PIN,
            },
            timing(12_000),
            &clock,
            B256::with_last_byte(0xAA),
        )
        .await
        .unwrap();

        assert_eq!(
            result,
            TargetPreparation::Ready(PreparedTarget {
                block: target_block,
                timestamp: PIN,
                expected_parent_hash: None,
            })
        );
        assert_eq!(clock.sleeps(), vec![Duration::from_millis(1_500)]);
    }

    #[tokio::test]
    async fn exact_target_missed_deadline_never_reaches_target_rpc() {
        let clock = FakeClock::new(PIN * 1_000 - 500);
        let provider = ProviderBuilder::new().connect_mocked_client(Asserter::new());
        let result = prepare_submission_target(
            &provider,
            BundleTarget::Exact {
                block: 42,
                timestamp: PIN,
            },
            timing(12_000),
            &clock,
            B256::with_last_byte(0xAA),
        )
        .await
        .unwrap();

        assert_eq!(
            result,
            TargetPreparation::Missed {
                block: 42,
                reason: BundleDropReason::SubmissionWindowMissed,
            }
        );
        assert!(clock.sleeps().is_empty());
    }

    #[tokio::test]
    async fn exact_target_built_during_gate_wait_is_not_submitted() {
        let clock = FakeClock::new(PIN * 1_000 - 9_000);
        let asserter = Asserter::new();
        asserter.push_success(&mock_block(42, PIN, B256::with_last_byte(2)));
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);
        let result = prepare_submission_target(
            &provider,
            BundleTarget::Exact {
                block: 42,
                timestamp: PIN,
            },
            timing(12_000),
            &clock,
            B256::with_last_byte(0xAA),
        )
        .await
        .unwrap();

        assert_eq!(
            result,
            TargetPreparation::Missed {
                block: 42,
                reason: BundleDropReason::TargetChanged,
            }
        );
    }

    #[tokio::test]
    async fn next_block_waits_for_parent_and_uses_a_timestamp_pin() {
        let base_number = 100;
        let base_timestamp = PIN;
        let parent_hash = B256::with_last_byte(0x11);
        let parent = mock_block(base_number + 1, base_timestamp + 12, parent_hash);
        let clock = FakeClock::new((base_timestamp + 12) * 1_000);
        let asserter = Asserter::new();
        asserter.push_success(&mock_block(
            base_number,
            base_timestamp,
            B256::with_last_byte(0x10),
        ));
        asserter.push_success(&serde_json::Value::Null);
        asserter.push_success(&parent);
        asserter.push_success(&parent);
        asserter.push_success(&parent);
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = prepare_submission_target(
            &provider,
            BundleTarget::NextBlock,
            timing(12_000),
            &clock,
            B256::with_last_byte(0xAA),
        )
        .await
        .unwrap();

        assert_eq!(
            result,
            TargetPreparation::Ready(PreparedTarget {
                block: base_number + 2,
                timestamp: base_timestamp + 24,
                expected_parent_hash: Some(parent_hash),
            })
        );
        assert_eq!(
            clock.sleeps(),
            vec![Duration::from_millis(500), Duration::from_secs(4)]
        );
    }

    #[tokio::test]
    async fn next_block_parent_reorg_during_wait_fails_without_submission() {
        let base_number = 100;
        let parent_a = mock_block(base_number + 1, PIN + 12, B256::with_last_byte(0xA1));
        let parent_b = mock_block(base_number + 1, PIN + 12, B256::with_last_byte(0xB1));
        let clock = FakeClock::new((PIN + 12) * 1_000);
        let asserter = Asserter::new();
        asserter.push_success(&mock_block(base_number, PIN, B256::with_last_byte(0x10)));
        asserter.push_success(&parent_a);
        asserter.push_success(&parent_b);
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = prepare_submission_target(
            &provider,
            BundleTarget::NextBlock,
            timing(12_000),
            &clock,
            B256::with_last_byte(0xAA),
        )
        .await
        .unwrap();

        assert_eq!(
            result,
            TargetPreparation::Missed {
                block: base_number + 2,
                reason: BundleDropReason::TargetChanged,
            }
        );
    }

    #[tokio::test]
    async fn next_block_built_during_gate_wait_fails_without_submission() {
        let base_number = 100;
        let parent_hash = B256::with_last_byte(0xA1);
        let parent = mock_block(base_number + 1, PIN + 12, parent_hash);
        let target = mock_block(base_number + 2, PIN + 24, B256::with_last_byte(0xA2));
        let clock = FakeClock::new((PIN + 12) * 1_000);
        let asserter = Asserter::new();
        asserter.push_success(&mock_block(base_number, PIN, B256::with_last_byte(0x10)));
        asserter.push_success(&parent);
        asserter.push_success(&parent);
        asserter.push_success(&target);
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = prepare_submission_target(
            &provider,
            BundleTarget::NextBlock,
            timing(12_000),
            &clock,
            B256::with_last_byte(0xAA),
        )
        .await
        .unwrap();

        assert_eq!(
            result,
            TargetPreparation::Missed {
                block: base_number + 2,
                reason: BundleDropReason::TargetChanged,
            }
        );
    }

    #[tokio::test]
    async fn next_block_parent_observed_after_deadline_is_not_submitted() {
        let base_number = 100;
        let parent = mock_block(base_number + 1, PIN + 12, B256::with_last_byte(0xA1));
        let target_timestamp = PIN + 24;
        let clock = FakeClock::new(target_timestamp * 1_000 - 500);
        let asserter = Asserter::new();
        asserter.push_success(&mock_block(base_number, PIN, B256::with_last_byte(0x10)));
        asserter.push_success(&parent);
        let provider = ProviderBuilder::new().connect_mocked_client(asserter);

        let result = prepare_submission_target(
            &provider,
            BundleTarget::NextBlock,
            timing(12_000),
            &clock,
            B256::with_last_byte(0xAA),
        )
        .await
        .unwrap();

        assert_eq!(
            result,
            TargetPreparation::Missed {
                block: base_number + 2,
                reason: BundleDropReason::SubmissionWindowMissed,
            }
        );
    }

    #[test]
    fn only_actual_exclusion_counts_against_user_attempts() {
        assert!(BundleDropReason::Excluded.counts_toward_attempt_limit());
        for reason in [
            BundleDropReason::SlotSkipped,
            BundleDropReason::SubmissionWindowMissed,
            BundleDropReason::TargetChanged,
        ] {
            assert!(!reason.counts_toward_attempt_limit(), "{reason:?}");
        }
    }

    #[test]
    fn replay_request_preserves_exact_transactions_target_and_timestamp_pin() {
        let request = bundle_request(&["0x0102", "0x0304"], 42, PIN, &[]);
        assert_eq!(request["method"], "eth_sendBundle");
        assert_eq!(request["params"][0]["txs"][0], "0x0102");
        assert_eq!(request["params"][0]["txs"][1], "0x0304");
        assert_eq!(request["params"][0]["blockNumber"], "0x2a");
        assert_eq!(request["params"][0]["minTimestamp"], PIN);
        assert_eq!(request["params"][0]["maxTimestamp"], PIN);
    }

    #[test]
    fn pinned_verdict_covers_every_arm() {
        for (case, block, want) in [
            ("height unbuilt", None, PinnedVerdict::Pending),
            (
                "block carries our tx",
                Some((PIN, true, B256::with_last_byte(1))),
                PinnedVerdict::Included,
            ),
            (
                "pinned slot built without us",
                Some((PIN, false, B256::with_last_byte(1))),
                PinnedVerdict::Excluded,
            ),
            (
                "timestamp mismatch",
                Some((PIN + 5, false, B256::with_last_byte(1))),
                PinnedVerdict::SlotSkipped,
            ),
            // Inclusion is the stronger fact: a builder that ignored the pin
            // still settled us, so never call that height a skip.
            (
                "included despite mismatch",
                Some((PIN + 5, true, B256::with_last_byte(1))),
                PinnedVerdict::Included,
            ),
        ] {
            assert_eq!(pinned_verdict(block, PIN, None), want, "{case}");
        }

        assert_eq!(
            pinned_verdict(
                Some((PIN, false, B256::with_last_byte(2))),
                PIN,
                Some(B256::with_last_byte(1)),
            ),
            PinnedVerdict::ParentChanged,
        );
    }
}
