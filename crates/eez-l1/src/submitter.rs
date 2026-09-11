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
use alloy_primitives::{B256, TxHash, U256, hex};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::Filter;
use alloy_sol_types::SolEvent;
use eez_protocol::abi::L2ExecutionPerformed;
use tracing::{Level, event};

use crate::config::SubmitterConfig;
use crate::error::{L1Error, L1Result};
use crate::l1_reader::L1Reader;

/// Wall-clock cap on the target-block + inclusion check.
const TARGET_WAIT_BUDGET: Duration = Duration::from_secs(30);
const TARGET_POLL_INTERVAL: Duration = Duration::from_millis(500);
/// L1 block offset for [`BundleTarget::NextBlock`]. slack=2 (over the
/// minimal latest+1) gives a one-block cushion for when our local
/// `latest` is stale by the time the relay sees the bundle.
const NEXT_BLOCK_SLACK: u64 = 2;

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
    Included {
        tx_hash: TxHash,
        l1_block: u64,
        /// Same L1 tx emitted `L2ExecutionPerformed` for our rollup,
        /// i.e. the contract advanced its block-hash commitment.
        state_applied: bool,
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

/// Thin L1-interaction primitive. Cheaply [`Clone`]able.
#[derive(Clone)]
pub struct Submitter {
    inner: Arc<Inner>,
}

struct Inner {
    config: SubmitterConfig,
    submission_timing: SubmissionTiming,
    reader: L1Reader,
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
        timestamp: u64,
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
                timestamp: target_timestamp,
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
                    timestamp: target_timestamp,
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
                timestamp: target_timestamp,
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
                    timestamp: target_timestamp,
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
    /// Returns [`L1Error::Config`] when the earliest and latest submission
    /// bounds do not form a non-empty window.
    pub fn new(
        config: SubmitterConfig,
        l1_block_time: Duration,
        submission_slack: Duration,
    ) -> L1Result<Self> {
        let reader = L1Reader::new(config.reader.clone());
        let submission_timing = SubmissionTiming::new(
            l1_block_time,
            config.bundle_submission_lead,
            submission_slack,
        )?;
        Ok(Self {
            inner: Arc::new(Inner {
                config,
                submission_timing,
                reader,
                // Bounded timeout so a hanging/unreachable builder relay cannot
                // stall the one-in-flight gate forever: a timed-out `eth_sendBundle`
                // returns Err, which `observe_bundle_outcome` treats as a drop →
                // the gate reopens and the next slot retries. `Client::new()` has
                // NO timeout, so a relay that accepts the TCP connection but never
                // responds freezes posting permanently (cursor stuck at 0).
                http: reqwest::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new()),
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

    /// L1 EOA address this Submitter sends from. Used by the Composer
    /// to identify "our own" `L1Event::BatchPosted` events vs.
    /// external ones in a multi-composer based-rollup deployment.
    #[must_use]
    pub fn poster_address(&self) -> alloy_primitives::Address {
        self.inner.config.poster.address()
    }

    /// Atomically bundle already-signed, 2718-encoded `raw_txs`
    /// (`[postBatch, user_tx_1, …]`, signed upstream) into one
    /// `eth_sendBundle` POST. Flashbots ordering: the bundle lands in
    /// `target_block` as one atomic insert or is dropped — no subset.
    ///
    /// Observation is best-effort: only `raw_txs[0]` (the postBatch) is
    /// checked for inclusion; if it's in, the bundle either succeeded or
    /// a proposer broke atomicity (their bug, not ours).
    ///
    /// # Errors
    ///
    /// - [`L1Error::Provider`] on RPC failure (block fetch, log fetch).
    /// - [`L1Error::Submission`] if the relay rejects the bundle.
    pub async fn send_bundle(
        &self,
        raw_txs: &[alloy_primitives::Bytes],
        target: BundleTarget,
        expected_final_state: Option<alloy_primitives::B256>,
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
        let target_provider = self.inner.build_target_provider();
        let clock = SystemClock;
        let prepared = prepare_submission_target(
            &target_provider,
            target,
            self.inner.submission_timing,
            &clock,
            post_batch_hash,
        )
        .await?;
        let PreparedTarget {
            block: target_block,
            timestamp: pin_timestamp,
            expected_parent_hash,
        } = match prepared {
            TargetPreparation::Ready(target) => target,
            TargetPreparation::Missed {
                block,
                timestamp: _,
                reason,
            } => return Ok(dropped(post_batch_hash, block, reason)),
        };

        // Formatting/logging normally takes microseconds, but enforce the
        // deadline again immediately before the HTTP request. We may send
        // after the not-before boundary; we must never knowingly send after
        // the latest-safe boundary and then blame user transactions for it.
        if !matches!(
            submission_window_state(
                clock.now_unix_ms()?,
                pin_timestamp,
                self.inner.submission_timing,
            ),
            SubmissionWindowState::Open
        ) {
            return Ok(dropped(
                post_batch_hash,
                target_block,
                BundleDropReason::SubmissionWindowMissed,
            ));
        }
        // Dispatch breadcrumb: correlate this postBatch tx to the L1 block we
        // aim it at. `SendOutcome::Included` later carries the ACTUAL inclusion
        // block, so joining on tx_hash gives the N+1 next-slot targeting hit-rate.
        event!(
            name: "eez.submitter.bundle.dispatch",
            Level::INFO,
            tx_hash = %post_batch_hash,
            target_block,
            exact = matches!(target, BundleTarget::Exact { .. }),
            tx_count = raw_txs.len(),
            "dispatching bundle to builder",
        );
        // One bundle, one target block. Atomic bundle semantics:
        // rbuilder either includes the whole bundle in `target_block`
        // in the specified order, or drops it. A relay without a bundle API is
        // accepted only for a lone postBatch; ordered multi-transaction
        // settlement fails closed inside `dispatch_and_observe`.
        self.inner
            .dispatch_and_observe(
                raw_txs,
                post_batch_hash,
                target_block,
                Some(pin_timestamp),
                expected_parent_hash,
                expected_final_state,
            )
            .await
    }

    /// `true` iff L1 already has a receipt for `tx_hash` (included,
    /// regardless of status). Used by the composer's bundle-failure
    /// path to avoid re-queueing user_txs whose nonce was burned by a
    /// reverted-but-included execution.
    ///
    /// # Errors
    ///
    /// [`L1Error::Provider`] on RPC failure.
    pub async fn receipt_exists(&self, tx_hash: TxHash) -> L1Result<bool> {
        // Target-tip RPC, not the embedded L1 — the embedded node can
        // lag the canonical tip by 2-3 blocks, and a missed receipt
        // here re-queues a burned-nonce tx that poisons the next
        // bundle's simulation.
        let provider = self.inner.build_target_provider();
        Ok(provider
            .get_transaction_receipt(tx_hash)
            .await
            .map_err(|e| L1Error::Provider(format!("get_transaction_receipt: {e}")))?
            .is_some())
    }
}

impl Inner {
    /// Canonical target-chain provider used for scheduling and observation.
    /// Falls back to the main RPC when no override URL is set. See
    /// `SubmitterConfig::target_rpc_url`.
    fn build_target_provider(&self) -> impl Provider + use<> {
        let url = self
            .config
            .target_rpc_url
            .clone()
            .unwrap_or_else(|| self.reader.rpc_url());
        ProviderBuilder::new()
            .disable_recommended_fillers()
            .connect_http(url)
    }

    /// Submit `raw_txs` (postBatch first) and observe the outcome. Try
    /// the relay's `eth_sendBundle`; if it's a plain execution RPC
    /// ([`L1Error::BundleRpcUnsupported`]), only a lone postBatch may degrade
    /// to mempool submission. Multi-transaction settlement is order- and
    /// all-or-nothing-sensitive, so it fails closed without a bundle relay.
    async fn dispatch_and_observe(
        &self,
        raw_txs: &[alloy_primitives::Bytes],
        post_batch_hash: TxHash,
        target_block: u64,
        pin_timestamp: Option<u64>,
        expected_parent_hash: Option<B256>,
        expected_final_state: Option<alloy_primitives::B256>,
    ) -> L1Result<SendOutcome> {
        let hexes: Vec<String> = raw_txs
            .iter()
            .map(|t| hex::encode_prefixed(t.as_ref()))
            .collect();
        let hex_refs: Vec<&str> = hexes.iter().map(String::as_str).collect();
        let result = match post_bundle(
            &self.http,
            self.config.builder_rpc_url.as_str(),
            &hex_refs,
            target_block,
            pin_timestamp,
        )
        .await
        {
            Ok(()) => {
                // Relay path: Exact and derived NextBlock targets both carry a
                // min/max timestamp pin, which unlocks the early verdict.
                self.observe(
                    post_batch_hash,
                    target_block,
                    pin_timestamp,
                    expected_parent_hash,
                    expected_final_state,
                )
                .await
            }
            Err(L1Error::BundleRpcUnsupported) => {
                require_atomic_relay_or_lone_postbatch(raw_txs.len())?;
                event!(
                    name: "eez.submitter.bundle.mempool_fallback",
                    Level::INFO,
                    event_name = "eez.submitter.bundle.mempool_fallback",
                    target_block,
                    tx_count = raw_txs.len(),
                    "relay has no eth_sendBundle; submitting the lone postBatch via mempool",
                );
                let target_provider = self.build_target_provider();
                let _pending = alloy_provider::Provider::send_raw_transaction(
                    &target_provider,
                    raw_txs[0].as_ref(),
                )
                .await
                .map_err(|error| {
                    L1Error::Submission(format!("mempool fallback: postBatch rejected: {error}"))
                })?;
                // No bundle API, no timestamp pin: these txs can genuinely land
                // in a later block, so the conservative rule is the only one.
                self.observe(
                    post_batch_hash,
                    target_block,
                    None,
                    None,
                    expected_final_state,
                )
                .await
            }
            Err(other) => Err(other),
        };
        match &result {
            Ok(SendOutcome::Dropped { reason, .. }) => self.capture_bundle_attempt(
                &hex_refs,
                post_batch_hash,
                target_block,
                pin_timestamp,
                expected_parent_hash,
                expected_final_state,
                "dropped",
                Some(reason.as_str()),
                None,
            ),
            Err(error) => self.capture_bundle_attempt(
                &hex_refs,
                post_batch_hash,
                target_block,
                pin_timestamp,
                expected_parent_hash,
                expected_final_state,
                "submission_error",
                None,
                Some(&error.to_string()),
            ),
            Ok(SendOutcome::Included { .. }) => {}
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn capture_bundle_attempt(
        &self,
        raw_tx_hexes: &[&str],
        post_batch_hash: TxHash,
        target_block: u64,
        pin_timestamp: Option<u64>,
        expected_parent_hash: Option<B256>,
        expected_final_state: Option<alloy_primitives::B256>,
        outcome: &str,
        drop_reason: Option<&str>,
        error: Option<&str>,
    ) {
        let Some(root) = self.config.bundle_capture_dir.as_ref() else {
            return;
        };
        let attempt = root.join(format!("bundle-{target_block}-{post_batch_hash}"));
        let request = bundle_request(raw_tx_hexes, target_block, pin_timestamp);
        let metadata = serde_json::json!({
            "version": 1,
            "builderRpcUrl": self.config.builder_rpc_url.as_str(),
            "postBatchHash": post_batch_hash.to_string(),
            "targetBlock": target_block,
            "pinnedTimestamp": pin_timestamp,
            "expectedParentHash": expected_parent_hash.map(|hash| hash.to_string()),
            "expectedFinalState": expected_final_state.map(|hash| hash.to_string()),
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
                target_block,
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

    /// Bundle observation, transport-agnostic: poll for the postBatch receipt,
    /// use the relay timestamp pin to prove when the target is dead, then derive
    /// `state_applied` from the inclusion block's `L2ExecutionPerformed`
    /// events via [`Self::settlement_in_block`].
    ///
    /// `pin_timestamp` is set for every relay submission. It is absent only for
    /// the lone-postBatch mempool fallback, which has no enforceable slot pin.
    async fn observe(
        &self,
        tx_hash: TxHash,
        target_block: u64,
        pin_timestamp: Option<u64>,
        expected_parent_hash: Option<B256>,
        expected_final_state: Option<alloy_primitives::B256>,
    ) -> L1Result<SendOutcome> {
        // Failure must mean PROVABLY DEAD, not merely slow. A bundle is
        // pinned to `target_block` — included in order there or never —
        // so death = the chain passed the target (head > target) without
        // our tx. A wall-clock-only budget here gave false FAILED
        // verdicts for bundles that later landed: recovery reorged + re-
        // emitted, the original settled anyway, and the two timelines
        // fought forever (RootMismatch, phantom L2 effects).
        // Receipt, head, and settlement logs all read from the SAME
        // tip provider — mixing the lagging embedded L1 for receipts
        // would re-open the false-death window. If the tip later reorgs
        // the bundle in, downstream converges it (Watcher → Deriver →
        // recovery cursor re-check drops the stale verdict).
        //
        // `pinned` buys a slot without weakening that: min == max timestamp
        // leaves one satisfiable height, so the block there decides it.
        let start = tokio::time::Instant::now();
        let target_provider = self.build_target_provider();
        let mut slow_logged = false;
        loop {
            // Transient RPC failures are retried, never escalated to a
            // failure verdict — while polling, the ledger stays Pending
            // and the composer's gate stays closed.
            match target_provider.get_transaction_receipt(tx_hash).await {
                Ok(Some(receipt)) => {
                    let l1_block = receipt.block_number.ok_or_else(|| {
                        L1Error::Provider("receipt present but block_number missing".into())
                    })?;
                    let state_applied = self
                        .settlement_in_block(&target_provider, l1_block, expected_final_state)
                        .await?;
                    return Ok(SendOutcome::Included {
                        tx_hash,
                        l1_block,
                        state_applied,
                    });
                }
                Ok(None) => {
                    let verdict = match pin_timestamp {
                        Some(pin_ts) => {
                            pinned_slot_check(
                                &target_provider,
                                target_block,
                                tx_hash,
                                pin_ts,
                                expected_parent_hash,
                            )
                            .await
                        }
                        None => PinnedVerdict::Pending,
                    };
                    match verdict {
                        PinnedVerdict::Excluded => {
                            return Ok(dropped(tx_hash, target_block, BundleDropReason::Excluded));
                        }
                        PinnedVerdict::SlotSkipped => {
                            return Ok(dropped(
                                tx_hash,
                                target_block,
                                BundleDropReason::SlotSkipped,
                            ));
                        }
                        PinnedVerdict::ParentChanged => {
                            return Ok(dropped(
                                tx_hash,
                                target_block,
                                BundleDropReason::TargetChanged,
                            ));
                        }
                        // Our tx IS in the pinned block; only the receipt read
                        // trails it. Poll on, skipping the head rule.
                        PinnedVerdict::Included => {}
                        PinnedVerdict::Pending => match target_provider.get_block_number().await {
                            Ok(head) if head > target_block => {
                                return Ok(dropped(
                                    tx_hash,
                                    target_block,
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
                    target_block,
                    elapsed_secs = start.elapsed().as_secs(),
                    "bundle observation exceeding budget; still polling (verdict requires inclusion or target passage)",
                );
            }
            tokio::time::sleep(TARGET_POLL_INTERVAL).await;
        }
    }

    /// Did L1's stored root for our rollup reach the claimed state
    /// in `l1_block`?
    ///
    /// - `Some(root)`: true iff some `L2ExecutionPerformed(rollupId,
    ///   newRoot)` in the block has `newRoot == root`. The leading
    ///   immediate entry always emits this event, so matching "any
    ///   event" would report settled even with every deferred entry
    ///   unconsumed; requiring the FINAL root means L1 reached exactly
    ///   the state the Sync block claims.
    /// - `None`: any event for our rollupId.
    async fn settlement_in_block<P: Provider>(
        &self,
        provider: &P,
        l1_block: u64,
        expected_final_state: Option<alloy_primitives::B256>,
    ) -> L1Result<bool> {
        let winners = Filter::new()
            .address(self.reader.eez())
            .event_signature(L2ExecutionPerformed::SIGNATURE_HASH)
            .topic1(U256::from(self.reader.rollup_id()))
            .from_block(l1_block)
            .to_block(l1_block);
        let logs = provider
            .get_logs(&winners)
            .await
            .map_err(|e| L1Error::Provider(format!("get_logs(L2ExecutionPerformed): {e}")))?;
        Ok(match expected_final_state {
            Some(root) => logs
                .iter()
                .any(|l| l.data().data.as_ref() == root.as_slice()),
            None => !logs.is_empty(),
        })
    }
}

/// POST a multi-tx bundle to a Flashbots-style `eth_sendBundle` relay.
/// Returns `Ok(())` if the relay returns a successful JSON-RPC reply
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
    pin_timestamp: Option<u64>,
) -> L1Result<()> {
    // STRICT all-or-nothing: `revertingTxHashes`/`droppingTxHashes`
    // empty. Per rbuilder's order commit, txs execute in submitted order
    // and any revert outside those whitelists fails the WHOLE bundle.
    // Whitelisting user_txs in `revertingTxHashes` instead lets the
    // relay silently DROP a reverting one (observed on chiado: block
    // 21566886 landed postBatch + 2 of 3 user_txs), advancing L1 to a
    // mid-chain prefix root and desyncing the composer. Sim is in-order
    // too, so user_txs simulate after the postBatch — no whitelist
    // needed for a "pre-postBatch state" sim.
    let body = bundle_request(raw_tx_hexes, target_block, pin_timestamp);
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
        // Typed so `send_bundle` can degrade to mempool submission.
        if err.get("code").and_then(serde_json::Value::as_i64) == Some(-32601) {
            return Err(L1Error::BundleRpcUnsupported);
        }
        return Err(L1Error::Submission(format!("eth_sendBundle: {err}")));
    }
    Ok(())
}

fn bundle_request(
    raw_tx_hexes: &[&str],
    target_block: u64,
    pin_timestamp: Option<u64>,
) -> serde_json::Value {
    let mut bundle_params = serde_json::json!({
        "txs": raw_tx_hexes,
        "blockNumber": format!("0x{target_block:x}"),
    });
    if let Some(ts) = pin_timestamp {
        // Pin inclusion to the exact L1 slot the L2 Sync block anchored
        // to. The builder enforces min/maxTimestamp against block.timestamp
        // (verified on chiado), so a skipped slot won't match and the
        // bundle drops rather than settling with a drifted L2 timestamp.
        bundle_params["minTimestamp"] = serde_json::json!(ts);
        bundle_params["maxTimestamp"] = serde_json::json!(ts);
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
    /// Built without us; atomic bundles land in that block or nowhere.
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
    target_block: u64,
    tx_hash: TxHash,
    pin_ts: u64,
    expected_parent_hash: Option<B256>,
) -> PinnedVerdict {
    match provider
        .get_block_by_number(BlockNumberOrTag::Number(target_block))
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
            pin_ts,
            expected_parent_hash,
        ),
        Err(err) => {
            event!(
                name: "eez.submitter.observe.pinned_block_read_failed",
                Level::WARN,
                tx_hash = %tx_hash,
                target_block,
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

fn require_atomic_relay_or_lone_postbatch(transaction_count: usize) -> L1Result<()> {
    if transaction_count == 1 {
        return Ok(());
    }
    Err(L1Error::Submission(format!(
        "relay has no eth_sendBundle; refusing non-atomic {transaction_count}-transaction settlement"
    )))
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
        prepare_submission_target, require_atomic_relay_or_lone_postbatch,
        submission_window_bounds, submission_window_state,
    };

    const PIN: u64 = 1_700_000_012;

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
                timestamp: PIN,
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
                timestamp: PIN,
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
                timestamp: PIN + 24,
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
                timestamp: PIN + 24,
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
                timestamp: target_timestamp,
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
    fn unsupported_bundle_rpc_fails_closed_for_ordered_settlement() {
        assert!(require_atomic_relay_or_lone_postbatch(1).is_ok());
        assert!(matches!(
            require_atomic_relay_or_lone_postbatch(2),
            Err(L1Error::Submission(message)) if message.contains("refusing non-atomic 2-transaction")
        ));
        assert!(matches!(
            require_atomic_relay_or_lone_postbatch(4),
            Err(L1Error::Submission(message)) if message.contains("refusing non-atomic 4-transaction")
        ));
    }

    #[test]
    fn replay_request_preserves_exact_transactions_target_and_timestamp_pin() {
        let request = bundle_request(&["0x0102", "0x0304"], 42, Some(PIN));
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
