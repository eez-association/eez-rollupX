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
//!
//! One dispatch sends the same serialized request to every configured relay
//! concurrently (see [`crate::config::BuilderRelays`]). It is one delivery
//! attempt regardless of the relay count: at least one acknowledgement starts
//! the same canonical observation as before, and only when no relay
//! acknowledges does the dispatch fail.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy_eips::{BlockNumberOrTag, Decodable2718};
use alloy_primitives::{B256, TxHash, hex};
use alloy_provider::{Provider, ProviderBuilder, RootProvider};
use tracing::{Level, event};

use crate::config::{BuilderRelays, SubmitterConfig};
use crate::error::{L1Error, L1Result};
use crate::l1_reader::L1Reader;

/// Bounded delivery observation for one dispatch to every configured relay.
/// A missing acknowledgement after dispatch is uncertain submission, not
/// evidence that a relay did not receive the bundle. Acknowledgement, by one
/// relay or several, never implies inclusion.
#[derive(Debug, Clone)]
pub struct BundleDelivery {
    /// Time to the first acknowledgement, or to the end of the fan-out when no
    /// relay acknowledged.
    pub elapsed: Duration,
    /// At least one relay acknowledged the bundle.
    pub acknowledged: bool,
    /// Correlation identifier from the first acknowledging relay, if it is a
    /// valid 32-byte hash. It is not an inclusion commitment and may differ
    /// between relays; see [`Self::builders`] for each relay's value.
    pub relay_bundle_hash: Option<B256>,
    /// Shared delivery window left at the first acknowledgement, or at the
    /// end of the fan-out when no relay acknowledged.
    pub remaining: Duration,
    pub target_block: u64,
    pub target_timestamp: u64,
    pub expected_parent_hash: Option<B256>,
    pub transaction_count: usize,
    /// Per-relay outcomes in configured order.
    pub builders: Vec<BuilderDelivery>,
}

/// How one relay answered a dispatch. Relay-supplied error text is not
/// retained here; only the classification derived from the error variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuilderDeliveryStatus {
    /// The relay returned a JSON-RPC result.
    Acknowledged,
    /// HTTP error status or JSON-RPC error other than method-not-found.
    Rejected,
    /// JSON-RPC `-32601`: the endpoint has no `eth_sendBundle`.
    Unsupported,
    /// Connection, timeout or response-decoding failure; delivery uncertain.
    TransportError,
    /// No reply before the shared delivery deadline; delivery uncertain.
    DeadlineExceeded,
    /// Still pending when another relay had acknowledged and the latest-safe
    /// submission boundary passed; delivery uncertain.
    AbandonedAfterAcknowledgement,
}

impl BuilderDeliveryStatus {
    /// Stable identifier used in structured logs, delivery events and
    /// bundle captures.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Acknowledged => "acknowledged",
            Self::Rejected => "rejected",
            Self::Unsupported => "unsupported",
            Self::TransportError => "transport_error",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::AbandonedAfterAcknowledgement => "abandoned_after_acknowledgement",
        }
    }

    const fn of(result: &L1Result<Option<B256>>) -> Self {
        match result {
            Ok(_) => Self::Acknowledged,
            Err(L1Error::BundleRpcUnsupported) => Self::Unsupported,
            Err(L1Error::Provider(_)) => Self::TransportError,
            Err(_) => Self::Rejected,
        }
    }
}

/// One relay's part of a dispatch, identified by configured position and host
/// only because relay URLs may carry credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuilderDelivery {
    pub index: usize,
    pub host: String,
    pub status: BuilderDeliveryStatus,
    /// This relay's correlation identifier, if it acknowledged with a valid
    /// 32-byte `bundleHash`.
    pub relay_bundle_hash: Option<B256>,
    /// Time from dispatch to this relay's reply or abandonment.
    pub elapsed: Duration,
}

impl BuilderDelivery {
    /// Log-safe JSON form shared by delivery events and bundle captures.
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "index": self.index,
            "host": self.host,
            "outcome": self.status.as_str(),
            "relay_bundle_hash": self.relay_bundle_hash,
            "elapsed_ms": self.elapsed.as_secs_f64() * 1000.0,
        })
    }
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
pub(crate) fn policy_target_slot_offset(
    parent: u64,
    target: u64,
    slot_seconds: u64,
) -> Option<u64> {
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

/// Select the first unexpired slot for the observed head's immediate child.
/// Native proofs bind the parent hash, so an estimate cannot select a future
/// execution height whose parent is still unknown. Missing slots retain the
/// existing bounded replacement allowance; a changed head requires a new proof.
async fn select_policy_target<P: Provider, C: Clock>(
    provider: &P,
    timing: SubmissionTiming,
    clock: &C,
) -> L1Result<BundleTarget> {
    let tip = rpc_block(provider, BlockNumberOrTag::Latest)
        .await?
        .ok_or_else(|| L1Error::Provider("policy target L1 head is unavailable".into()))?;
    let now_ms = clock.now_unix_ms()?;
    let ready_ms = now_ms
        .checked_add(timing.deadline_slack_ms)
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
    if offset > MAX_MISSING_SLOT_REPLACEMENTS + 1 {
        return Err(L1Error::Provider(
            "policy target missing-slot allowance exhausted".into(),
        ));
    }
    let block = tip
        .header
        .number
        .checked_add(1)
        .ok_or_else(|| L1Error::Provider("policy target height overflow".into()))?;
    let timestamp_ms = offset
        .checked_mul(timing.l1_block_time_ms)
        .and_then(|value| tip_ms.checked_add(value))
        .ok_or_else(|| L1Error::Provider("policy target timestamp overflow".into()))?;
    let timestamp = timestamp_ms / 1_000;
    event!(name: "eez.submitter.policy.target_selected", Level::INFO,
        event_name = "eez.submitter.policy.target_selected",
        head_number = tip.header.number, head_timestamp = tip.header.timestamp,
        selected_at_ms = now_ms,
        submission_slack_ms = timing.deadline_slack_ms,
        target_block = block, target_timestamp = timestamp, slot_offset = offset,
        "selected policy target with a known canonical parent");
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

    /// Pin the next execution height to the observed canonical parent and the
    /// earliest unexpired slot. Preparation/proving share that slot's deadline;
    /// a timing estimate cannot authorize a proof with an unknown parent hash.
    pub async fn next_policy_target(&self) -> L1Result<BundleTarget> {
        select_policy_target(
            &self.inner.target_provider,
            self.inner.submission_timing,
            &SystemClock,
        )
        .await
    }

    /// Resolve deferred proof work against a known canonical parent. Already
    /// pinned targets are retained; a changed target requires a new proof.
    pub async fn resolve_target(&self, target: BundleTarget) -> L1Result<BundleTarget> {
        match target {
            BundleTarget::Exact { .. } => Ok(target),
            BundleTarget::NextBlock => self.next_policy_target().await,
        }
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
        quote: impl Into<crate::refund::BundlePricing>,
        record_dispatch: impl FnOnce(u64, u64) -> L1Result<()> + Send,
    ) -> L1Result<SendOutcome> {
        self.send_bundle_checked(raw_txs, target, Some(quote.into()), record_dispatch, None)
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
        quote: impl Into<crate::refund::BundlePricing>,
        record_dispatch: impl FnOnce(u64, u64) -> L1Result<()> + Send,
        deadline: tokio::time::Instant,
        observed: impl FnOnce(BundleDelivery) + Send + 'static,
    ) -> L1Result<SendOutcome> {
        self.send_bundle_checked(
            raw_txs,
            target,
            Some(quote.into()),
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
        quote: Option<crate::refund::BundlePricing>,
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
        let mut reverting_hashes = inbound_revert_hashes(raw_txs, chain_id, post_batch_hash)?;
        let refund = quote.and_then(|pricing| pricing.refund);
        if let Some(refund) = refund {
            require_refund_source(
                raw_txs,
                refund,
                self.poster_address(),
                &mut reverting_hashes,
            )?;
        }
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
            if !quote.quote.is_current(chain_id, current, target) {
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
                refund,
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

    /// Observe a successful plain transfer from the canonical block beneficiary
    /// after the selected source. This profile requires a dedicated recipient;
    /// unrelated or aggregated payments cannot qualify as an attributed refund.
    /// This read does not establish consumption or change request ownership.
    pub async fn refund_payment_at(
        &self,
        block: crate::L1BlockRef,
        source_index: u64,
        request: crate::refund::BundleRefund,
    ) -> L1Result<crate::refund::ObservedPayment> {
        use crate::refund::ObservedPayment;
        use alloy_consensus::{Transaction as _, transaction::SignerRecoverable as _};
        use alloy_sol_types::SolCall as _;
        let provider = &self.inner.target_provider;
        require_canonical_block(provider, block).await?;
        let full = provider
            .get_block_by_hash(block.hash)
            .full()
            .await
            .map_err(|e| L1Error::Provider(format!("refund block: {e}")))?
            .ok_or_else(|| L1Error::Provider("refund block unavailable".into()))?;
        if full.header.hash != block.hash || full.header.number != block.number {
            return Err(L1Error::Provider("refund block identity changed".into()));
        }
        let txs = full
            .transactions
            .as_transactions()
            .ok_or_else(|| L1Error::Provider("refund block lacks full transactions".into()))?;
        let index = usize::try_from(source_index).map_err(|e| L1Error::Provider(e.to_string()))?;
        if txs
            .get(index)
            .is_none_or(|tx| tx.inner.tx_hash() != &request.source())
        {
            return Err(L1Error::Provider(
                "refund source index differs from canonical block".into(),
            ));
        }
        let mut payment = ObservedPayment::Missing;
        for (index, tx) in txs.iter().enumerate().skip(index + 1) {
            if tx.to() == Some(self.inner.reader.eez())
                && tx.input().get(..4)
                    == Some(eez_protocol::abi::postAndVerifyBatchCall::SELECTOR.as_slice())
            {
                // A later native post starts a different funding group. An
                // end-of-block aggregate cannot be attributed by this profile.
                break;
            }
            if tx.to() != Some(request.recipient())
                || !tx.input().is_empty()
                || tx.value().is_zero()
            {
                continue;
            }
            let sender = tx
                .inner
                .inner()
                .recover_signer()
                .map_err(|e| L1Error::Provider(e.to_string()))?;
            if sender != full.header.beneficiary || sender == request.recipient() {
                continue;
            }
            let hash = *tx.inner.tx_hash();
            let receipt = provider
                .get_transaction_receipt(hash)
                .await
                .map_err(|e| L1Error::Provider(format!("refund receipt: {e}")))?
                .ok_or_else(|| L1Error::Provider("refund receipt unavailable".into()))?;
            if receipt.transaction_hash != hash
                || receipt.block_hash != Some(block.hash)
                || receipt.block_number != Some(block.number)
                || receipt.transaction_index != Some(index as u64)
            {
                return Err(L1Error::Provider("refund receipt identity changed".into()));
            }
            if !receipt.status() || receipt.gas_used != 21_000 {
                continue;
            }
            if payment != ObservedPayment::Missing {
                payment = ObservedPayment::Ambiguous;
                break;
            }
            payment = ObservedPayment::Confirmed {
                hash,
                value: tx.value(),
            };
        }
        require_canonical_block(provider, block).await?;
        Ok(payment)
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
        refund: Option<crate::refund::BundleRefund>,
        delivery: Option<DeliveryWindow>,
    ) -> L1Result<SendOutcome> {
        let hexes: Vec<String> = raw_txs
            .iter()
            .map(|t| hex::encode_prefixed(t.as_ref()))
            .collect();
        let hex_refs: Vec<&str> = hexes.iter().map(String::as_str).collect();
        // Serialize once so every relay receives byte-identical requests.
        let mut request =
            bundle_request(&hex_refs, target.block, target.timestamp, reverting_hashes);
        if let Some(refund) = refund {
            refund.apply(&mut request["params"][0]);
        }
        let body = serde_json::to_vec(&request).map_err(|error| {
            L1Error::Submission(format!("encode eth_sendBundle request: {error}"))
        })?;
        // One signature over the shared bytes authenticates every relay copy.
        let signature = self
            .config
            .bundle_signer
            .as_ref()
            .map(|signer| flashbots_signature(signer, &body))
            .transpose()?;
        // After one acknowledgement, a reply that arrives after the
        // latest-safe submission boundary cannot improve this attempt. Stop
        // waiting for slower relays there so they cannot delay observation.
        // Without an acknowledgement each relay keeps its existing bound.
        let (_, latest_safe_ms) =
            submission_window_bounds(target.timestamp, self.submission_timing);
        let straggler_cutoff = SystemClock.now_unix_ms().ok().map(|now_ms| {
            tokio::time::Instant::now()
                + Duration::from_millis(latest_safe_ms.saturating_sub(now_ms))
        });
        let fan_out = fan_out_bundle(
            &self.http,
            &self.config.builder_relays,
            alloy_primitives::bytes::Bytes::from(body),
            signature.as_ref(),
            target.block,
            raw_txs.len(),
            delivery.as_ref().map(|window| window.deadline),
            straggler_cutoff,
        )
        .await;
        event!(
            name: "eez.submitter.bundle.fanout",
            Level::INFO,
            event_name = "eez.submitter.bundle.fanout",
            tx_hash = %post_batch_hash,
            target_block = target.block,
            builders = fan_out.builders.len(),
            acknowledged = fan_out.acknowledged_count(),
            outcomes = %fan_out.outcomes_json(),
            "eth_sendBundle fan-out finished",
        );
        if let Some(window) = delivery {
            let reply_at = fan_out.first_ack.map_or(fan_out.finished_at, |ack| ack.at);
            (window.observed)(BundleDelivery {
                elapsed: reply_at.saturating_duration_since(fan_out.started_at),
                acknowledged: fan_out.first_ack.is_some(),
                relay_bundle_hash: fan_out.first_ack.and_then(|ack| ack.relay_bundle_hash),
                remaining: window.deadline.saturating_duration_since(reply_at),
                target_block: target.block,
                target_timestamp: target.timestamp,
                expected_parent_hash: target.expected_parent_hash,
                transaction_count: raw_txs.len(),
                builders: fan_out.builders.clone(),
            });
        }
        let builders = fan_out.builders.clone();
        let result = match fan_out.into_acknowledgement(&self.config.builder_relays) {
            Ok(_) => {
                // Relay path: Exact and derived NextBlock targets both carry a
                // min/max timestamp pin, which unlocks the early verdict.
                // Acknowledgements never decide inclusion; observation does.
                self.observe(post_batch_hash, target).await
            }
            Err(other) => Err(other),
        };
        self.capture_bundle_attempt(&request, post_batch_hash, target, &result, &builders);
        result
    }

    fn capture_bundle_attempt(
        &self,
        request: &serde_json::Value,
        post_batch_hash: TxHash,
        target: PreparedTarget,
        result: &L1Result<SendOutcome>,
        builders: &[BuilderDelivery],
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
        // Version 2 replaces v1's `builderRpcUrl` with per-relay outcomes.
        // URLs may carry credentials and are never written; replay supplies
        // the endpoint explicitly (testing/kurtosis/scripts/replay-bundle.sh).
        let metadata = serde_json::json!({
            "version": 2,
            "builders": builders.iter().map(|builder| serde_json::json!({
                "index": builder.index,
                "host": builder.host,
                "outcome": builder.status.as_str(),
                "relayBundleHash": builder.relay_bundle_hash,
                "elapsedMs": builder.elapsed.as_secs_f64() * 1000.0,
            })).collect::<Vec<_>>(),
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

/// Error for a relay that did not reply before the shared delivery deadline.
/// Kept identical to the single-relay message so existing diagnostics match.
const DELIVERY_DEADLINE_MESSAGE: &str =
    "relay acknowledgement exceeded the shared deadline; delivery uncertain";

type RelayRequest<'a> = std::pin::Pin<Box<dyn Future<Output = L1Result<Option<B256>>> + Send + 'a>>;

/// First acknowledgement observed during a fan-out.
#[derive(Debug, Clone, Copy)]
struct FirstAcknowledgement {
    at: tokio::time::Instant,
    relay_bundle_hash: Option<B256>,
}

/// Completed fan-out: per-relay outcomes plus the errors needed to classify a
/// dispatch that no relay acknowledged.
#[derive(Debug)]
struct FanOut {
    started_at: tokio::time::Instant,
    finished_at: tokio::time::Instant,
    first_ack: Option<FirstAcknowledgement>,
    builders: Vec<BuilderDelivery>,
    /// Errors of non-acknowledging relays, in configured order.
    errors: Vec<(usize, L1Error)>,
}

impl FanOut {
    fn acknowledged_count(&self) -> usize {
        self.builders
            .iter()
            .filter(|builder| builder.status == BuilderDeliveryStatus::Acknowledged)
            .count()
    }

    fn outcomes_json(&self) -> serde_json::Value {
        serde_json::Value::Array(self.builders.iter().map(BuilderDelivery::json).collect())
    }

    /// Acknowledged when any relay acknowledged. Otherwise classify the
    /// failure from the error variants, never from error text:
    ///
    /// - one relay: its error, unchanged from the single-relay behavior;
    /// - every relay answered `-32601`: [`L1Error::BundleRpcUnsupported`];
    /// - every other failure was transport ([`L1Error::is_transport`]):
    ///   [`L1Error::Provider`], since no relay is known to have answered;
    /// - otherwise [`L1Error::Submission`] (a rejection or deadline expiry).
    ///
    /// Unsupported relays carry no information about this bundle, so they do
    /// not decide the class when other relays failed differently.
    fn into_acknowledgement(self, relays: &BuilderRelays) -> L1Result<Option<B256>> {
        if let Some(ack) = self.first_ack {
            return Ok(ack.relay_bundle_hash);
        }
        let mut errors = self.errors;
        if errors.len() == 1 {
            return Err(errors.remove(0).1);
        }
        if errors
            .iter()
            .all(|(_, error)| matches!(error, L1Error::BundleRpcUnsupported))
        {
            return Err(L1Error::BundleRpcUnsupported);
        }
        let transport_only = errors
            .iter()
            .filter(|(_, error)| !matches!(error, L1Error::BundleRpcUnsupported))
            .all(|(_, error)| error.is_transport());
        let summary = errors
            .iter()
            .map(|(index, error)| format!("{}: {error}", relays.label(*index)))
            .collect::<Vec<_>>()
            .join("; ");
        let message = format!(
            "no builder acknowledged eth_sendBundle ({} configured): {summary}",
            relays.len()
        );
        Err(if transport_only {
            L1Error::Provider(message)
        } else {
            L1Error::Submission(message)
        })
    }
}

/// Send one serialized request to every relay concurrently and collect each
/// relay's outcome.
///
/// Requests are polled in this task (no spawned work), so dropping the
/// returned future cancels every outstanding request. Each relay is bounded by
/// the HTTP client timeout and, when present, the shared `deadline`. After the
/// first acknowledgement, relays still pending at `straggler_cutoff` are
/// abandoned. Without an acknowledgement every relay keeps its full bound.
#[allow(clippy::too_many_arguments)]
async fn fan_out_bundle(
    http: &reqwest::Client,
    relays: &BuilderRelays,
    body: alloy_primitives::bytes::Bytes,
    signature: Option<&reqwest::header::HeaderValue>,
    target_block: u64,
    tx_count: usize,
    deadline: Option<tokio::time::Instant>,
    straggler_cutoff: Option<tokio::time::Instant>,
) -> FanOut {
    use std::task::Poll;

    let started_at = tokio::time::Instant::now();
    let mut pending: Vec<Option<RelayRequest<'_>>> = relays
        .urls()
        .iter()
        .enumerate()
        .map(|(index, url)| {
            let request: RelayRequest<'_> = Box::pin(post_bundle(
                http,
                url,
                body.clone(),
                signature,
                relays,
                index,
                target_block,
                tx_count,
            ));
            Some(request)
        })
        .collect();
    let mut replies: Vec<Option<(L1Result<Option<B256>>, tokio::time::Instant)>> =
        (0..pending.len()).map(|_| None).collect();
    let mut first_ack: Option<FirstAcknowledgement> = None;
    let mut cutoff = deadline;
    let mut cutoff_after_ack = false;
    loop {
        let next = std::future::poll_fn(|cx| {
            let mut open = false;
            for (index, slot) in pending.iter_mut().enumerate() {
                if let Some(request) = slot {
                    if let Poll::Ready(result) = request.as_mut().poll(cx) {
                        *slot = None;
                        return Poll::Ready(Some((index, result)));
                    }
                    open = true;
                }
            }
            if open {
                Poll::Pending
            } else {
                Poll::Ready(None)
            }
        });
        let polled = match cutoff {
            Some(at) => tokio::time::timeout_at(at, next).await.ok(),
            None => Some(next.await),
        };
        let Some(Some((index, result))) = polled else {
            break;
        };
        let at = tokio::time::Instant::now();
        if first_ack.is_none()
            && let Ok(relay_bundle_hash) = &result
        {
            first_ack = Some(FirstAcknowledgement {
                at,
                relay_bundle_hash: *relay_bundle_hash,
            });
            if let Some(straggler) = straggler_cutoff
                && cutoff.is_none_or(|current| straggler < current)
            {
                cutoff = Some(straggler);
                cutoff_after_ack = true;
            }
        }
        replies[index] = Some((result, at));
    }
    let finished_at = tokio::time::Instant::now();
    let mut builders = Vec::with_capacity(replies.len());
    let mut errors = Vec::new();
    for (index, reply) in replies.into_iter().enumerate() {
        let (status, relay_bundle_hash, at) = match reply {
            Some((result, at)) => {
                let status = BuilderDeliveryStatus::of(&result);
                match result {
                    Ok(hash) => (status, hash, at),
                    Err(error) => {
                        errors.push((index, error));
                        (status, None, at)
                    }
                }
            }
            None if cutoff_after_ack => (
                BuilderDeliveryStatus::AbandonedAfterAcknowledgement,
                None,
                finished_at,
            ),
            None => {
                errors.push((index, L1Error::Submission(DELIVERY_DEADLINE_MESSAGE.into())));
                (BuilderDeliveryStatus::DeadlineExceeded, None, finished_at)
            }
        };
        builders.push(BuilderDelivery {
            index,
            host: relays.host(index).to_owned(),
            status,
            relay_bundle_hash,
            elapsed: at.saturating_duration_since(started_at),
        });
    }
    FanOut {
        started_at,
        finished_at,
        first_ack,
        builders,
        errors,
    }
}

/// Describe a relay HTTP failure without its URL, which may carry credentials.
/// `reqwest` puts the URL only in the outer error; the source chain (connect,
/// timeout, status or decode cause) is kept for diagnosis.
fn relay_error(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut message = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// `X-Flashbots-Signature` value authenticating one serialized request body:
/// `<address>:<signature>`, where the signature is an EIP-191 personal-message
/// signature over the 0x-prefixed lowercase hex TEXT of `keccak256(body)`
/// (ethers' `wallet.signMessage(id(body))`), as Flashbots, BuilderNet and
/// Quasar verify it. It must be computed over the exact bytes sent.
///
/// # Errors
///
/// [`L1Error::Submission`] if signing fails or the value is not a valid header.
fn flashbots_signature(
    signer: &alloy_signer_local::PrivateKeySigner,
    body: &[u8],
) -> L1Result<reqwest::header::HeaderValue> {
    use alloy_signer::SignerSync;
    let digest_text = hex::encode_prefixed(alloy_primitives::keccak256(body));
    let signature = signer
        .sign_message_sync(digest_text.as_bytes())
        .map_err(|error| L1Error::Submission(format!("sign eth_sendBundle request: {error}")))?;
    reqwest::header::HeaderValue::from_str(&format!(
        "{}:{}",
        signer.address(),
        hex::encode_prefixed(signature.as_bytes())
    ))
    .map_err(|error| L1Error::Submission(format!("X-Flashbots-Signature header: {error}")))
}

/// POST one serialized bundle to a Flashbots-style `eth_sendBundle` relay.
/// Returns an optional, strictly decoded relay bundle hash on a successful reply
/// (any non-error `result` — most relays use `{"result": {"bundleHash":
/// "0x..."}}`, the chiado builder returns `{"result": null}`). Errors
/// surface relay-side rejections as [`L1Error::Submission`].
///
/// The caller is responsible for picking a not-yet-proposed block — the relay
/// silently drops bundles aimed at already-built blocks. Errors and logs omit
/// the URL (it may carry credentials) and name the relay by position and host.
/// `signature`, when present, is sent as `X-Flashbots-Signature` and must have
/// been computed over `body` (see [`flashbots_signature`]).
///
/// # Errors
///
/// - [`L1Error::Provider`] on transport (DNS, TCP, timeout, JSON decode) failure.
/// - [`L1Error::Submission`] when the relay HTTP status is non-2xx OR
///   the JSON body carries an `error` field.
/// - [`L1Error::BundleRpcUnsupported`] for JSON-RPC `-32601`.
#[allow(clippy::too_many_arguments)]
async fn post_bundle(
    http: &reqwest::Client,
    url: &url::Url,
    body: alloy_primitives::bytes::Bytes,
    signature: Option<&reqwest::header::HeaderValue>,
    relays: &BuilderRelays,
    index: usize,
    target_block: u64,
    tx_count: usize,
) -> L1Result<Option<B256>> {
    // Permit only the validated signed L1 users to revert; postBatch remains
    // required. Invalid-transaction omission depends on builder configuration,
    // and every user still requires canonical source reconciliation.
    let mut request = http
        .post(url.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json");
    if let Some(signature) = signature {
        request = request.header("X-Flashbots-Signature", signature.clone());
    }
    let resp: serde_json::Value = request
        .body(body)
        .send()
        .await
        .map_err(|e| L1Error::Provider(format!("eth_sendBundle POST: {}", relay_error(e))))?
        .error_for_status()
        .map_err(|e| L1Error::Submission(format!("eth_sendBundle HTTP: {}", relay_error(e))))?
        .json()
        .await
        .map_err(|e| L1Error::Provider(format!("eth_sendBundle decode: {}", relay_error(e))))?;
    event!(
        name: "eez.submitter.bundle.sent",
        Level::INFO,
        builder_index = index,
        builder_host = relays.host(index),
        target_block,
        tx_count,
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

/// A selected payer cannot be optional: a builder must reject the bundle when
/// that source fails. Other users retain the protocol's partial-consumption
/// permissions. This check does not attest to the builder's eventual payment.
fn require_refund_source(
    raw_txs: &[alloy_primitives::Bytes],
    refund: crate::refund::BundleRefund,
    poster: alloy_primitives::Address,
    reverting_hashes: &mut Vec<B256>,
) -> L1Result<()> {
    use alloy_consensus::transaction::SignerRecoverable;
    if !reverting_hashes.contains(&refund.source()) {
        return Err(L1Error::Submission(
            "refund source is not a unique L1 companion".into(),
        ));
    }
    for raw in raw_txs.iter().skip(1) {
        let tx = alloy_consensus::TxEnvelope::decode_2718(&mut raw.as_ref())
            .map_err(|e| L1Error::Submission(format!("refund source envelope: {e}")))?;
        if *tx.tx_hash() == refund.source() {
            let sender = tx
                .recover_signer()
                .map_err(|e| L1Error::Submission(format!("refund source signature: {e}")))?;
            if sender == poster || sender == refund.recipient() {
                return Err(L1Error::Submission(
                    "refund source must be externally funded".into(),
                ));
            }
            reverting_hashes.retain(|hash| *hash != refund.source());
            return Ok(());
        }
    }
    Err(L1Error::Submission(
        "refund source is absent from the signed bundle".into(),
    ))
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
                builder_relays: crate::BuilderRelays::new(vec![url.clone()]).unwrap(),
                target_rpc_url: Some(url),
                poster: B256::with_last_byte(1).to_string().parse().unwrap(),
                bundle_signer: None,
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
                let mut flashbots_signature = None;
                loop {
                    let mut line = String::new();
                    assert!(stream.read_line(&mut line).await.unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            length = Some(value.trim().parse::<usize>().unwrap());
                        } else if name.eq_ignore_ascii_case("x-flashbots-signature") {
                            flashbots_signature = Some(value.trim().to_string());
                        }
                    }
                }
                let mut body = vec![0; length.expect("JSON RPC request length")];
                stream.read_exact(&mut body).await.unwrap();
                let mut request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                // Record the raw body and auth header for signature tests;
                // unsigned requests keep their previous shape.
                if let Some(signature) = flashbots_signature {
                    request["__x_flashbots_signature"] = serde_json::json!(signature);
                    request["__raw_body"] =
                        serde_json::json!(String::from_utf8(body.clone()).unwrap());
                }
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
            let tip_time = super::SystemClock.now_unix_ms().unwrap().div_ceil(1_000);
            let (url, server) = rpc_server(
                vec![serde_json::json!({
                    "result": mock_block(41, tip_time, B256::ZERO)
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
                    block: 42,
                    timestamp: tip_time + slot_secs
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
                    None,
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
        let delivery = observed.lock().unwrap().take().unwrap();
        assert!(delivery.relay_bundle_hash.is_none());
        assert!(delivery.remaining.is_zero());
        assert_eq!(
            delivery
                .builders
                .iter()
                .map(|builder| builder.status)
                .collect::<Vec<_>>(),
            [super::BuilderDeliveryStatus::DeadlineExceeded]
        );
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
            let relays = crate::BuilderRelays::new(vec![url.clone()]).unwrap();
            let body = serde_json::to_vec(&bundle_request(&["0x01"], 42, PIN, &[])).unwrap();
            let result = super::post_bundle(
                &reqwest::Client::new(),
                &url,
                body.into(),
                None,
                &relays,
                0,
                42,
                1,
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
                    let raw_metadata =
                        std::fs::read_to_string(attempt.join("metadata.json")).unwrap();
                    // Relay URLs may carry credentials and are never captured.
                    assert!(!raw_metadata.contains(url.as_str().trim_end_matches('/')));
                    let mut metadata: serde_json::Value =
                        serde_json::from_str(&raw_metadata).unwrap();
                    let elapsed = metadata["builders"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("elapsedMs")
                        .unwrap();
                    assert!(elapsed.as_f64().unwrap() >= 0.0);
                    let expected = serde_json::json!({
                        "version": 2,
                        "builders": [{
                            "index": 0, "host": "127.0.0.1",
                            "outcome": if outcome == "dropped" { "acknowledged" } else { "unsupported" },
                            "relayBundleHash": null,
                        }],
                        "postBatchHash": post_hash.to_string(), "targetBlock": 42,
                        "pinnedTimestamp": PIN, "expectedParentHash": B256::ZERO.to_string(),
                        "outcome": outcome,
                        "dropReason": (outcome == "dropped").then_some("excluded"),
                        "error": (outcome == "submission_error").then_some("builder relay does not support eth_sendBundle"),
                        "requestFile": "request.json",
                    });
                    assert_eq!(metadata, expected);
                }
            }
        }
    }

    const RELAY_SECRET: &str = "fixture-relay-secret";

    /// One-request `eth_sendBundle` relay. Reports the exact request body as
    /// soon as it is read, waits `delay`, then answers with `response`
    /// (JSON-RPC envelope added) or never answers when `response` is `None`.
    async fn fake_relay(
        response: Option<serde_json::Value>,
        delay: Duration,
    ) -> (
        url::Url,
        tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
        tokio::task::JoinHandle<()>,
    ) {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        // Credentials and secret path/query parts must never reach errors,
        // logs, delivery events or captures.
        let url = format!(
            "http://relay:{RELAY_SECRET}@{}/{RELAY_SECRET}?key={RELAY_SECRET}",
            listener.local_addr().unwrap()
        )
        .parse()
        .unwrap();
        let (bodies, received) = tokio::sync::mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            let mut stream = BufReader::new(listener.accept().await.unwrap().0);
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
            let mut body = vec![0; length.expect("bundle request length")];
            stream.read_exact(&mut body).await.unwrap();
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
            // Tests that ignore the body drop the receiver.
            let _ = bodies.send(body);
            tokio::time::sleep(delay).await;
            let Some(mut response) = response else {
                std::future::pending::<()>().await;
                return;
            };
            response["jsonrpc"] = serde_json::json!("2.0");
            response["id"] = request["id"].clone();
            let body = serde_json::to_vec(&response).unwrap();
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len(),
            );
            stream.get_mut().write_all(header.as_bytes()).await.unwrap();
            stream.get_mut().write_all(&body).await.unwrap();
        });
        (url, received, server)
    }

    /// A relay URL whose port refuses connections: a transport failure.
    fn unreachable_relay() -> url::Url {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        format!("http://relay:{RELAY_SECRET}@{address}/{RELAY_SECRET}")
            .parse()
            .unwrap()
    }

    fn ack(hash: Option<B256>) -> serde_json::Value {
        serde_json::json!({"result": hash.map(|hash| serde_json::json!({"bundleHash": hash}))})
    }

    fn rejection(code: i64) -> serde_json::Value {
        serde_json::json!({"error": {"code": code, "message": "fixture relay rejection"}})
    }

    async fn run_fan_out(
        urls: Vec<url::Url>,
        deadline: Option<Duration>,
        straggler: Option<Duration>,
    ) -> (super::FanOut, crate::BuilderRelays) {
        let relays = crate::BuilderRelays::new(urls).unwrap();
        let body = serde_json::to_vec(&bundle_request(&["0x01"], 42, PIN, &[])).unwrap();
        let now = tokio::time::Instant::now();
        let fan_out = super::fan_out_bundle(
            &reqwest::Client::new(),
            &relays,
            body.into(),
            None,
            42,
            1,
            deadline.map(|delay| now + delay),
            straggler.map(|delay| now + delay),
        )
        .await;
        (fan_out, relays)
    }

    fn signing_key() -> alloy_signer_local::PrivateKeySigner {
        B256::with_last_byte(1).to_string().parse().unwrap()
    }

    #[test]
    fn flashbots_signature_matches_the_independent_reference_vector() {
        // Produced with cast (EIP-191 over the hex text of keccak256(body)),
        // the procedure Titan, BuilderNet and Quasar accepted live.
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"eth_sendBundle","params":[]}"#;
        let header = super::flashbots_signature(&signing_key(), body).unwrap();
        assert_eq!(
            header.to_str().unwrap(),
            "0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf:\
             0xde098351bb885003ea9ef6f6ede76d4fc38fc44e61ff44ff7bf63a9ac0ab40fe\
             0abf97616c9707188f974278244635efa6c81927bb3a07d6f6fe7ca3938e8dfb1b"
        );
    }

    #[test]
    fn flashbots_signature_recovers_the_signer_from_the_digest_text_only() {
        let body = serde_json::to_vec(&bundle_request(&["0x01"], 42, PIN, &[])).unwrap();
        let header = super::flashbots_signature(&signing_key(), &body).unwrap();
        let (address, signature) = header.to_str().unwrap().split_once(':').unwrap();
        assert_eq!(
            address.parse::<alloy_primitives::Address>().unwrap(),
            signing_key().address()
        );
        let signature: alloy_primitives::Signature = signature.parse().unwrap();
        let text = alloy_primitives::hex::encode_prefixed(alloy_primitives::keccak256(&body));
        assert_eq!(
            signature.recover_address_from_msg(text.as_bytes()).unwrap(),
            signing_key().address()
        );
        // Signing the raw hash bytes instead of their text is a different scheme.
        let raw = alloy_primitives::keccak256(&body);
        assert_ne!(
            signature.recover_address_from_msg(raw.as_slice()).ok(),
            Some(signing_key().address())
        );
        // Any change to the body invalidates the header.
        let mut tampered = body.clone();
        tampered.push(b' ');
        assert_ne!(
            super::flashbots_signature(&signing_key(), &tampered).unwrap(),
            header
        );
    }

    #[tokio::test]
    async fn bundle_requests_carry_the_signature_only_when_a_key_is_configured() {
        let ok = serde_json::json!({"result": {"bundleHash": B256::with_last_byte(7)}});
        let body = serde_json::to_vec(&bundle_request(&["0x01"], 42, PIN, &[])).unwrap();
        let header = super::flashbots_signature(&signing_key(), &body).unwrap();
        for signature in [None, Some(&header)] {
            let (first, first_server) = rpc_server(vec![ok.clone()], false).await;
            let (second, second_server) = rpc_server(vec![ok.clone()], false).await;
            let relays = crate::BuilderRelays::new(vec![first, second]).unwrap();
            let fan_out = super::fan_out_bundle(
                &reqwest::Client::new(),
                &relays,
                body.clone().into(),
                signature,
                42,
                1,
                None,
                None,
            )
            .await;
            assert_eq!(fan_out.acknowledged_count(), 2);
            for server in [first_server, second_server] {
                let request = server.await.unwrap().remove(0);
                match signature {
                    None => assert!(request.get("__x_flashbots_signature").is_none()),
                    Some(header) => {
                        // Every relay copy carries the same header over the exact bytes sent.
                        assert_eq!(request["__x_flashbots_signature"], header.to_str().unwrap());
                        assert_eq!(
                            request["__raw_body"],
                            String::from_utf8(body.clone()).unwrap()
                        );
                    }
                }
            }
        }
    }

    fn statuses(builders: &[super::BuilderDelivery]) -> Vec<super::BuilderDeliveryStatus> {
        builders.iter().map(|builder| builder.status).collect()
    }

    fn assert_redacted(text: &str) {
        assert!(!text.contains(RELAY_SECRET), "relay secret leaked: {text}");
    }

    #[tokio::test]
    async fn fan_out_sends_identical_requests_and_needs_one_acknowledgement() {
        use super::BuilderDeliveryStatus::{Acknowledged, Rejected, TransportError};
        let hash = B256::repeat_byte(0xcd);
        let (rejecting, mut rejected_body, rejecting_server) =
            fake_relay(Some(rejection(-32000)), Duration::ZERO).await;
        let (accepting, mut accepted_body, accepting_server) =
            fake_relay(Some(ack(Some(hash))), Duration::from_millis(50)).await;
        let unreachable = unreachable_relay();
        let (fan_out, relays) = run_fan_out(
            vec![rejecting, unreachable, accepting],
            Some(Duration::from_secs(5)),
            None,
        )
        .await;
        let rejected_body = rejected_body.recv().await.unwrap();
        let accepted_body = accepted_body.recv().await.unwrap();
        assert_eq!(
            rejected_body, accepted_body,
            "relays must receive identical bytes"
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&accepted_body).unwrap(),
            bundle_request(&["0x01"], 42, PIN, &[])
        );
        assert_eq!(
            statuses(&fan_out.builders),
            [Rejected, TransportError, Acknowledged]
        );
        assert_eq!(fan_out.builders[2].relay_bundle_hash, Some(hash));
        assert_eq!(fan_out.acknowledged_count(), 1);
        assert_redacted(&fan_out.outcomes_json().to_string());
        assert_eq!(fan_out.into_acknowledgement(&relays).unwrap(), Some(hash));
        rejecting_server.await.unwrap();
        accepting_server.await.unwrap();
    }

    #[tokio::test]
    async fn fan_out_failure_class_follows_error_variants() {
        // (relay responses, expected class); `None` is an unreachable relay.
        type Case = (Vec<Option<serde_json::Value>>, &'static str);
        let cases: Vec<Case> = vec![
            (
                vec![Some(rejection(-32601)), Some(rejection(-32601))],
                "unsupported",
            ),
            (vec![None, None], "transport"),
            (vec![Some(rejection(-32601)), None], "transport"),
            (vec![Some(rejection(-32000)), None], "submission"),
            (
                vec![Some(rejection(-32601)), Some(rejection(-32000))],
                "submission",
            ),
            // A single relay keeps its exact pre-fan-out error.
            (vec![Some(rejection(-32601))], "unsupported"),
            (vec![Some(rejection(-32000))], "single_submission"),
        ];
        for (relays, expected) in cases {
            let mut urls = Vec::new();
            let mut servers = Vec::new();
            for relay in &relays {
                match relay {
                    Some(response) => {
                        let (url, _, server) =
                            fake_relay(Some(response.clone()), Duration::ZERO).await;
                        urls.push(url);
                        servers.push(server);
                    }
                    None => urls.push(unreachable_relay()),
                }
            }
            let (fan_out, relays) = run_fan_out(urls, Some(Duration::from_secs(5)), None).await;
            assert_eq!(fan_out.acknowledged_count(), 0);
            let error = fan_out.into_acknowledgement(&relays).unwrap_err();
            assert_redacted(&error.to_string());
            match (expected, &error) {
                ("unsupported", L1Error::BundleRpcUnsupported) => {}
                ("transport", L1Error::Provider(message))
                | ("submission", L1Error::Submission(message)) => {
                    assert!(
                        message.starts_with("no builder acknowledged eth_sendBundle"),
                        "{message}"
                    );
                    assert!(message.contains("builder[0] (127.0.0.1)"), "{message}");
                }
                ("single_submission", L1Error::Submission(message)) => {
                    assert!(message.starts_with("eth_sendBundle: "), "{message}");
                }
                _ => panic!("expected {expected}, got {error:?}"),
            }
            for server in servers {
                server.await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn slow_relays_are_bounded_by_the_deadline_or_the_latest_safe_boundary() {
        use super::BuilderDeliveryStatus::{
            AbandonedAfterAcknowledgement, Acknowledged, DeadlineExceeded,
        };
        // Shared deadline: a silent relay cannot extend the fan-out.
        let (silent, _, silent_server) = fake_relay(None, Duration::ZERO).await;
        let (fast, _, fast_server) = fake_relay(Some(ack(None)), Duration::ZERO).await;
        let started = tokio::time::Instant::now();
        let (fan_out, relays) = run_fan_out(
            vec![silent, fast],
            Some(Duration::from_millis(300)),
            Some(Duration::from_mins(1)),
        )
        .await;
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(300), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
        assert_eq!(
            statuses(&fan_out.builders),
            [DeadlineExceeded, Acknowledged]
        );
        assert!(fan_out.first_ack.unwrap().at < started + Duration::from_millis(300));
        assert_eq!(fan_out.into_acknowledgement(&relays).unwrap(), None);
        silent_server.abort();
        fast_server.await.unwrap();

        // No delivery deadline: after an acknowledgement the latest-safe
        // boundary releases observation instead of the 30 s client timeout.
        let (silent, _, silent_server) = fake_relay(None, Duration::ZERO).await;
        let (fast, _, fast_server) = fake_relay(Some(ack(None)), Duration::ZERO).await;
        let started = tokio::time::Instant::now();
        let (fan_out, _) =
            run_fan_out(vec![silent, fast], None, Some(Duration::from_millis(200))).await;
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            statuses(&fan_out.builders),
            [AbandonedAfterAcknowledgement, Acknowledged]
        );
        silent_server.abort();
        fast_server.await.unwrap();

        // Without an acknowledgement the boundary does not cut delivery short:
        // only the shared deadline ends it, as with a single relay.
        let (first, _, first_server) = fake_relay(None, Duration::ZERO).await;
        let (second, _, second_server) = fake_relay(None, Duration::ZERO).await;
        let started = tokio::time::Instant::now();
        let (fan_out, relays) = run_fan_out(
            vec![first, second],
            Some(Duration::from_millis(300)),
            Some(Duration::ZERO),
        )
        .await;
        assert!(started.elapsed() >= Duration::from_millis(300));
        assert_eq!(
            statuses(&fan_out.builders),
            [DeadlineExceeded, DeadlineExceeded]
        );
        assert!(matches!(
            fan_out.into_acknowledgement(&relays),
            Err(L1Error::Submission(message)) if message.contains(super::DELIVERY_DEADLINE_MESSAGE)
        ));
        first_server.abort();
        second_server.abort();
    }

    #[tokio::test]
    async fn one_rejecting_relay_does_not_prevent_observed_inclusion() {
        let (post, post_hash) = signed_user(1, 0);
        let block_hash = B256::with_last_byte(1);
        let relay_hash = B256::repeat_byte(0xef);
        let receipt = serde_json::json!({
            "transactionHash": post_hash, "transactionIndex": "0x0",
            "blockHash": block_hash, "blockNumber": "0x2a",
            "from": alloy_primitives::Address::ZERO,
            "to": alloy_primitives::Address::ZERO,
            "gasUsed": "0x5208", "effectiveGasPrice": "0x1",
            "cumulativeGasUsed": "0x5208", "type": "0x2", "status": "0x1",
            "logs": [], "logsBloom": alloy_primitives::Bloom::ZERO,
        });
        let mut block = serde_json::to_value(mock_block(42, PIN, block_hash)).unwrap();
        block["transactions"] = serde_json::json!([post_hash]);
        let (target_url, target_server) = rpc_server(
            vec![
                serde_json::json!({"result": receipt}),
                serde_json::json!({"result": block}),
            ],
            false,
        )
        .await;
        let (rejecting, _, rejecting_server) =
            fake_relay(Some(rejection(-32000)), Duration::ZERO).await;
        let (accepting, _, accepting_server) =
            fake_relay(Some(ack(Some(relay_hash))), Duration::ZERO).await;
        let mut submitter = test_submitter(target_url);
        std::sync::Arc::get_mut(&mut submitter.inner)
            .unwrap()
            .config
            .builder_relays = crate::BuilderRelays::new(vec![rejecting, accepting]).unwrap();
        let observed = std::sync::Arc::new(std::sync::Mutex::new(None));
        let saved = observed.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            submitter.inner.dispatch_and_observe(
                &[post],
                post_hash,
                PreparedTarget {
                    block: 42,
                    timestamp: PIN,
                    expected_parent_hash: None,
                },
                &[],
                None,
                Some(super::DeliveryWindow {
                    deadline: tokio::time::Instant::now() + Duration::from_secs(5),
                    observed: Box::new(move |delivery| *saved.lock().unwrap() = Some(delivery)),
                }),
            ),
        )
        .await
        .unwrap();
        assert!(
            matches!(result, Ok(super::SendOutcome::Included { tx_hash, l1_block: 42, .. }) if tx_hash == post_hash),
            "{result:?}"
        );
        let delivery = observed.lock().unwrap().take().unwrap();
        assert!(delivery.acknowledged);
        assert_eq!(delivery.relay_bundle_hash, Some(relay_hash));
        assert_eq!(
            statuses(&delivery.builders),
            [
                super::BuilderDeliveryStatus::Rejected,
                super::BuilderDeliveryStatus::Acknowledged
            ]
        );
        for builder in &delivery.builders {
            assert_eq!(builder.host, "127.0.0.1");
            assert_redacted(&builder.json().to_string());
        }
        let requests = target_server.await.unwrap();
        assert_eq!(requests[0]["method"], "eth_getTransactionReceipt");
        rejecting_server.await.unwrap();
        accepting_server.await.unwrap();
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

    #[tokio::test]
    async fn refund_observation_requires_canonical_unique_plain_builder_payment() {
        use crate::refund::{BundleRefund, ObservedPayment};
        use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
        use alloy_network::TxSignerSync;
        use alloy_primitives::{Address, TxKind, U256};
        let signer: alloy_signer_local::PrivateKeySigner =
            B256::with_last_byte(2).to_string().parse().unwrap();
        let recipient = Address::repeat_byte(9);
        let block_hash = B256::repeat_byte(3);
        let make = |nonce, to, value, input| {
            let mut tx = TxEip1559 {
                chain_id: 1,
                nonce,
                gas_limit: 21_000,
                max_fee_per_gas: 10,
                max_priority_fee_per_gas: 1,
                to: TxKind::Call(to),
                value,
                input,
                ..Default::default()
            };
            let signature = signer.sign_transaction_sync(&mut tx).unwrap();
            let tx = TxEnvelope::Eip1559(tx.into_signed(signature));
            let mut value = serde_json::to_value(&tx).unwrap();
            value["from"] = serde_json::json!(signer.address());
            value["hash"] = serde_json::json!(tx.tx_hash());
            value["blockHash"] = serde_json::json!(block_hash);
            value["blockNumber"] = serde_json::json!("0x2a");
            value["transactionIndex"] = serde_json::json!(format!("{nonce:#x}"));
            (value, *tx.tx_hash())
        };
        let (source, source_hash) =
            make(0, Address::repeat_byte(1), U256::ZERO, Default::default());
        let mut header = mock_block(42, PIN, block_hash);
        header.header.inner.beneficiary = signer.address();
        let header = serde_json::to_value(header).unwrap();
        for scenario in [
            "paid",
            "missing",
            "wrong_recipient",
            "wrong_sender",
            "reverted",
            "code",
            "ambiguous",
            "reorg",
            "later_post",
        ] {
            let mut full = header.clone();
            let mut txs = vec![source.clone()];
            if scenario == "later_post" {
                use alloy_sol_types::SolCall as _;
                txs.push(
                    make(
                        1,
                        Address::ZERO,
                        U256::ZERO,
                        eez_protocol::abi::postAndVerifyBatchCall::SELECTOR
                            .to_vec()
                            .into(),
                    )
                    .0,
                );
            }
            let mut receipts = vec![];
            let count = if scenario == "missing" {
                0
            } else if scenario == "ambiguous" {
                2
            } else {
                1
            };
            let mut expected_hash = B256::ZERO;
            for index in 1..=count {
                let index = index + u64::from(scenario == "later_post");
                let to = if scenario == "wrong_recipient" {
                    Address::repeat_byte(8)
                } else {
                    recipient
                };
                let (tx, hash) = make(index, to, U256::from(500), Default::default());
                expected_hash = hash;
                txs.push(tx);
                if !["wrong_recipient", "wrong_sender", "later_post"].contains(&scenario) {
                    receipts.push(serde_json::json!({"result":{
                        "transactionHash":hash,"transactionIndex":format!("{index:#x}"),
                        "blockHash":block_hash,"blockNumber":"0x2a","from":signer.address(),"to":to,
                        "gasUsed":if scenario == "code" {"0x5209"} else {"0x5208"},"effectiveGasPrice":"0x1",
                        "cumulativeGasUsed":"0xa410","type":"0x2","status":if scenario == "reverted" {"0x0"} else {"0x1"},
                        "logs":[],"logsBloom":alloy_primitives::Bloom::ZERO
                    }}));
                }
            }
            full["transactions"] = serde_json::json!(txs);
            if scenario == "wrong_sender" {
                full["miner"] = serde_json::json!(Address::repeat_byte(8));
            }
            let mut final_header = header.clone();
            if scenario == "reorg" {
                final_header["hash"] = serde_json::json!(B256::repeat_byte(4));
            }
            let mut replies = vec![
                serde_json::json!({"result":header}),
                serde_json::json!({"result":full}),
            ];
            replies.extend(receipts);
            replies.push(serde_json::json!({"result":final_header}));
            let (url, server) = rpc_server(replies, true).await;
            let result = test_submitter(url)
                .refund_payment_at(
                    crate::L1BlockRef {
                        number: 42,
                        hash: block_hash,
                    },
                    0,
                    BundleRefund::new(recipient, 90, source_hash).unwrap(),
                )
                .await;
            match scenario {
                "paid" => assert_eq!(
                    result.unwrap(),
                    ObservedPayment::Confirmed {
                        hash: expected_hash,
                        value: U256::from(500)
                    }
                ),
                "ambiguous" => assert_eq!(result.unwrap(), ObservedPayment::Ambiguous),
                "reorg" => assert!(result.is_err()),
                _ => assert_eq!(result.unwrap(), ObservedPayment::Missing, "{scenario}"),
            }
            server.await.unwrap();
        }
    }

    #[test]
    fn refund_payer_is_required_and_cannot_be_the_poster() {
        use crate::refund::BundleRefund;
        use alloy_consensus::transaction::SignerRecoverable;
        use alloy_eips::Decodable2718;
        let (first, first_hash) = signed_user(1, 0);
        let (second, second_hash) = signed_user(1, 1);
        let raw = vec![
            alloy_primitives::Bytes::from_static(&[3, 0xc0]),
            first.clone(),
            second,
        ];
        let recipient = alloy_primitives::Address::repeat_byte(9);
        let terms = BundleRefund::new(recipient, 90, first_hash).unwrap();
        let mut allowed = vec![first_hash, second_hash];
        super::require_refund_source(&raw, terms, alloy_primitives::Address::ZERO, &mut allowed)
            .unwrap();
        assert_eq!(allowed, vec![second_hash]);
        let hexes = raw.iter().map(ToString::to_string).collect::<Vec<_>>();
        let refs = hexes.iter().map(String::as_str).collect::<Vec<_>>();
        let mut request = bundle_request(&refs, 42, PIN, &allowed);
        terms.apply(&mut request["params"][0]);
        assert_eq!(request["params"][0]["refundPercent"], 90);
        assert_eq!(
            request["params"][0]["refundRecipient"],
            serde_json::json!(recipient)
        );
        assert_eq!(
            request["params"][0]["refundTxHashes"],
            serde_json::json!([first_hash])
        );
        assert_eq!(
            request["params"][0]["revertingTxHashes"],
            serde_json::json!([second_hash])
        );
        assert_eq!(request["params"][0]["txs"], serde_json::json!(hexes));
        assert_eq!(request["params"][0]["minTimestamp"], PIN);
        assert_eq!(request["params"][0]["maxTimestamp"], PIN);
        let sender = alloy_consensus::TxEnvelope::decode_2718(&mut first.as_ref())
            .unwrap()
            .recover_signer()
            .unwrap();
        assert!(
            super::require_refund_source(&raw, terms, sender, &mut vec![first_hash, second_hash])
                .is_err()
        );
        let returned_to_sender = BundleRefund::new(sender, 90, first_hash).unwrap();
        assert!(
            super::require_refund_source(
                &raw,
                returned_to_sender,
                recipient,
                &mut vec![first_hash, second_hash]
            )
            .is_err()
        );
        let absent = BundleRefund::new(recipient, 90, B256::repeat_byte(7)).unwrap();
        assert!(
            super::require_refund_source(
                &raw,
                absent,
                recipient,
                &mut vec![first_hash, second_hash]
            )
            .is_err()
        );
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
    async fn policy_proof_target_keeps_a_known_canonical_parent() {
        for (slot_ms, slack_ms) in [(5_000, 700), (12_000, 500)] {
            let timing = SubmissionTiming::new(
                Duration::from_millis(slot_ms),
                Duration::from_millis(7_500),
                Duration::from_millis(slack_ms),
            )
            .unwrap();
            // Include the incident's late preparation point and exact deadline
            // equality. Missing slots advance time, never the unknown height.
            let cutoff = slot_ms - slack_ms;
            for (elapsed_ms, offset) in [
                (0, 1),
                (3_000, 1),
                (cutoff - 1, 1),
                (cutoff, 2),
                (slot_ms, 2),
                (3 * slot_ms + cutoff, 5),
            ] {
                let replies = Asserter::new();
                replies.push_success(&mock_block(100, PIN, B256::with_last_byte(1)));
                let provider = ProviderBuilder::new().connect_mocked_client(replies);
                let clock = FakeClock::new(PIN * 1_000 + elapsed_ms);
                let target = super::select_policy_target(&provider, timing, &clock)
                    .await
                    .unwrap();
                assert_eq!(
                    target,
                    BundleTarget::Exact {
                        block: 101,
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
                let target = super::select_policy_target(&provider, timing, &clock)
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
            super::select_policy_target(&provider, timing(5_000), &clock)
                .await
                .unwrap_err()
                .to_string()
                .contains("head is unavailable")
        );
        for (height, timestamp, now_ms) in [
            (u64::MAX, PIN, PIN * 1_000),
            (100, u64::MAX, PIN * 1_000),
            (100, PIN, u64::MAX),
            (100, PIN, PIN * 1_000 + 5 * 5_000),
            (100, u64::MAX / 1_000, 0),
        ] {
            let replies = Asserter::new();
            replies.push_success(&mock_block(height, timestamp, B256::ZERO));
            let provider = ProviderBuilder::new().connect_mocked_client(replies);
            assert!(
                super::select_policy_target(&provider, timing(5_000), &FakeClock::new(now_ms),)
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
            let target = super::select_policy_target(&provider, timing(5_000), &clock)
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
