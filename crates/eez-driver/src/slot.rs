//! Slot timing primitives: kinds, events, and the spawners that emit
//! them.
//!
//! [`SlotKind`] is the per-block label (Live / Future / Sync) attached
//! at production time for logging + dashboards.
//!
//! [`SlotEvent`] is the trigger event sent into the Sequencer's
//! `mpsc::Receiver<SlotEvent>`. Two variants:
//!
//! - [`SlotEvent::SyncSlot`] — L1-anchored sync-slot trigger. Produced
//!   by [`spawn_l1_anchored`], which subscribes to an
//!   [`eez_l1::L1HeadStream`]
//!   and sleeps until `L1.timestamp + proof_window_open` after each
//!   head — i.e. the wall-clock moment when the prover should start
//!   so the postBatch bundle reaches the relay before the next L1 block.
//!   If that block is absent, bounded additional triggers target successive
//!   timestamps on the same L1 parent; a new head replaces the pending timer.
//! - [`SlotEvent::LiveTick`] — wall-clock `l2_block_time` tick, also
//!   produced by [`spawn_l1_anchored`] between sync-slot triggers, so
//!   the RPC head advances one Live block per cadence instead of in
//!   per-anchor bursts. The Sequencer commits at most ONE Live block
//!   per tick, and only while the head is below the live region of the
//!   carried sync-slot height (the Future + Sync reserve belongs to
//!   the anchor trigger — ticks never outrun it).
//!
//! The Sequencer is source-agnostic — it just consumes `SlotEvent`s.
//! Tests inject fake schedules by sending events directly into the
//! channel.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy_consensus::Header;
use alloy_rpc_types_engine::ExecutionData;
use async_trait::async_trait;
use eez_l1::submitter::MAX_MISSING_SLOT_REPLACEMENTS;
use reth_primitives_traits::SealedHeader;
use tokio::sync::mpsc;
use tokio::time::{Instant, MissedTickBehavior, interval_at};
use tracing::{Level, event};

use crate::timing::RollupTiming;

/// Classification of the slot a single block is being produced for.
///
/// Logged on every commit so dashboards can show the Live / Future /
/// Sync mix per slot. Distinct from [`SlotEvent`] — that's a *trigger*
/// (one per sync slot or one per interval tick), whereas `SlotKind`
/// labels each individual block produced inside a trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlotKind {
    /// Normal block at current wall-clock cadence.
    Live,
    /// Proof-window padding block — timestamp is in the future
    /// relative to wall-clock at production time; reth accepts as
    /// long as timestamps strictly increase.
    Future,
    /// Sync block — last block of a sync slot, where cross-chain
    /// system txs and the matching `ExecutionEntries` land. Timestamp
    /// matches the L1 block that will carry the corresponding
    /// `postBatch`.
    Sync,
}

impl core::fmt::Display for SlotKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Live => f.write_str("live"),
            Self::Future => f.write_str("future"),
            Self::Sync => f.write_str("sync"),
        }
    }
}

/// Trigger event from a spawner telling the Sequencer when to act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SlotEvent {
    /// L1-anchored: produce the rest of a sync slot. Sequencer reads
    /// current head and
    /// [`RollupTiming::per_trigger_composition`](crate::RollupTiming::per_trigger_composition)
    /// to decide the Live / Future / Sync block count for this trigger.
    SyncSlot {
        /// L2 block height the Sync block will land at.
        block_height: u64,
        /// Wall-clock unix timestamp the Sync block will advertise.
        /// Normally `L1.timestamp_of_anchor + L1_block_time`; one missing-slot
        /// replacement may target the following timestamp on the same parent.
        timestamp: u64,
        /// L1 block this slot is anchored to; the postBatch targets
        /// `l1_head + 1` in steady state.
        l1_head: u64,
    },
    /// L1-anchored wall-clock tick — commit exactly one Live block,
    /// but only while the head is below the live region of the next
    /// sync slot (`sync_slot_block_height - future_count - 1`). The
    /// Future + Sync reserve is produced by the [`Self::SyncSlot`]
    /// trigger; the Sequencer no-ops ticks once the live region is
    /// full so ticks cannot outrun the anchor.
    LiveTick {
        /// Latest anchor-computed L2 height the Sync block will land
        /// at. The Sequencer derives the live-region bound from this
        /// plus its own [`RollupTiming`].
        sync_slot_block_height: u64,
    },
}

/// Pre-built Sync block produced by a [`SyncSlotComposer`] — ready to
/// commit via [`BlockCommitterHandle::commit_derived`].
///
/// [`BlockCommitterHandle::commit_derived`]: crate::block_committer::BlockCommitterHandle::commit_derived
#[derive(Debug)]
pub struct SyncSlotBlock {
    /// Engine-API payload.
    pub payload: ExecutionData,
    /// Sealed header of the new block (head-mirror update on commit).
    pub header: SealedHeader<Header>,
}

/// Explicit control flow at a Sync boundary. A policy deferral must never
/// accidentally admit mempool transactions into the required empty Sync.
#[derive(Debug)]
pub enum SyncSlotOutcome {
    Built(Box<SyncSlotBlock>),
    /// A stale parent or failed required empty build ends this tick. The
    /// existing scheduler will catch up from the current head on its next tick.
    EndTick,
}

/// Parent context the Sequencer supplies to a [`SyncSlotComposer`].
///
/// The full `SealedHeader<Header>` from the Sequencer's synchronous
/// `last_header` mirror — already reflects the just-committed parent,
/// and passing it whole avoids a `sealed_header_by_hash` re-lookup that
/// can race reth's provider-index propagation for the newest block.
#[derive(Debug, Clone)]
pub struct ParentContext {
    /// Sealed header of the parent block — full struct so the composer
    /// hands it to `evm_config.builder_for_next_block` directly.
    pub header: SealedHeader<Header>,
}

/// How a Sync slot is being produced. Gates cross-chain content: only a
/// caught-up [`Steady`](Self::Steady) block can settle its postBatch in the
/// same L1 slot (same timestamp), so [`Catchup`](Self::Catchup) blocks are
/// structural-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncSlotMode {
    /// Caught-up, aim-ahead slot — drains the cross-chain pool.
    Steady,
    /// Behind-the-wall-clock catchup — empty Sync block; cross-chain
    /// waits for the next [`Steady`](Self::Steady) slot.
    Catchup,
    /// Deterministic EMPTY block, no emission (deferred-late slots). Unlike
    /// the mempool-fed `commit_one` fallback it keeps grid heights
    /// batch-boundary-eligible.
    Empty,
}

/// Per-Sync-slot block producer for cross-chain content.
///
/// Called by the Sequencer before each Sync block. The production impl
/// (`eez-composer::Composer`) drains its `HeldPool`, resolves each held
/// tx into system txs and selects an explicit outcome. `mode` gates the drain
/// (see [`SyncSlotMode`]). Lives here, not in
/// `eez-composer`, to avoid a dependency cycle.
#[async_trait]
pub trait SyncSlotComposer: Send + Sync + 'static {
    /// Cross-chain admission owns its combined preparation/proof deadline.
    /// Ordinary-only settlement retains the configured proof-start gate.
    fn has_cross_chain_work(&self, _rollup_id: u64) -> bool {
        false
    }

    /// The scheduler owns the Sync grid, including its genesis/slot offset.
    /// Publish its target before starting an ordinary payload job.
    fn set_sequencing_target(&self, rollup_id: u64, sync_height: u64);

    /// Maximum encoded block item size for this exact parent. Errors stop
    /// production, never remove a bound.
    fn ordinary_block_da_limit(
        &self,
        rollup_id: u64,
        parent: &SealedHeader<Header>,
        timestamp: u64,
    ) -> Result<usize, String>;

    /// Fresh-interval bound used to reject individually unpostable ordinary
    /// transactions without rejecting valid transactions waiting for capacity.
    fn ordinary_transaction_da_limit(&self, rollup_id: u64) -> Result<usize, String>;

    /// Compose a Sync slot. `target_l1_block`: `Some(n)` aims the postBatch
    /// at exactly L1 block `n` (steady state, `l1_head + 1`); `None` aims the
    /// next available block (catch-up). `mode` gates the drain: `Catchup`
    /// blocks are empty (cross-chain waits for the next `Steady` slot).
    async fn compose_sync_slot(
        &self,
        rollup_id: u64,
        parent: ParentContext,
        timestamp: u64,
        target_l1_block: Option<u64>,
        mode: SyncSlotMode,
    ) -> SyncSlotOutcome;

    /// Run pending failed-bundle recovery before this tick's commit. May move
    /// the head (reorg + substitution), so callers re-read it. Default: no-op.
    async fn recover_failed(&self, rollup_id: u64) {
        let _ = rollup_id;
    }

    /// Before building the next interval, optionally replace an unresolved
    /// speculative suffix whose observation deadline has passed. This is not
    /// a settlement verdict; canonical replay still decides which batch won.
    async fn prepare_next_slot(&self, rollup_id: u64, target: u64, timestamp: u64) {
        let _ = (rollup_id, target, timestamp);
    }
}

/// Cheap clone-able handle for a [`SyncSlotComposer`].
pub type SyncSlotComposerHandle = Arc<dyn SyncSlotComposer>;

/// Sync-slot trigger armed by an L1 head, waiting for its
/// proof-window-open instant.
#[derive(Debug, Clone, Copy)]
struct PendingSyncSlot {
    /// Wall-clock unix millisecond at which to fire
    /// (`L1.timestamp + proof_window_open`).
    trigger_at_ms: u64,
    /// L2 block height the Sync block will land at.
    block_height: u64,
    /// Wall-clock unix timestamp the Sync block will advertise.
    timestamp: u64,
    /// L1 block of the anchoring head; carried into [`SlotEvent::SyncSlot`].
    l1_head: u64,
    /// L1 block timestamp of the anchoring head (logging only).
    l1_timestamp: u64,
    replacement_count: u64,
}

impl PendingSyncSlot {
    fn new(head: eez_l1::L1HeadInfo, timing: RollupTiming, genesis_timestamp: u64) -> Self {
        let timestamp = head
            .timestamp
            .saturating_add(timing.l1_block_time().as_secs());
        Self {
            trigger_at_ms: head.timestamp.saturating_mul(1_000).saturating_add(
                u64::try_from(timing.proof_window_open().as_millis()).unwrap_or(u64::MAX),
            ),
            block_height: timestamp.saturating_sub(genesis_timestamp)
                / timing.l2_block_time().as_secs(),
            timestamp,
            l1_head: head.block_number,
            l1_timestamp: head.timestamp,
            replacement_count: 0,
        }
    }

    /// Bound replacement opportunities per observed head during an outage.
    /// A missing slot advances time, not the execution block number.
    fn replacement(self, timing: RollupTiming) -> Option<Self> {
        if self.replacement_count >= MAX_MISSING_SLOT_REPLACEMENTS {
            return None;
        }
        Some(Self {
            trigger_at_ms: self
                .trigger_at_ms
                .checked_add(u64::try_from(timing.l1_block_time().as_millis()).ok()?)?,
            block_height: self.block_height.checked_add(u64::from(timing.k()))?,
            timestamp: self
                .timestamp
                .checked_add(timing.l1_block_time().as_secs())?,
            replacement_count: self.replacement_count + 1,
            ..self
        })
    }

    fn remaining(self, now_ms: u64) -> Duration {
        Duration::from_millis(self.trigger_at_ms.saturating_sub(now_ms))
    }
}

/// Spawn the L1-anchored scheduler. Returns the receiver side of the
/// channel.
///
/// On each L1 head from `source`:
///
/// 1. Compute the deterministic sync-slot block height + timestamp
///    from `(l2_genesis_timestamp, L1.timestamp, L1_block_time,
///    L2_block_time)`. Same formula across all nodes given the same
///    genesis.
/// 2. Arm a trigger for wall-clock `L1.timestamp + proof_window_open`
///    — the moment preparation should start so proof and submission finish
///    before the next L1 block lands.
/// 3. When the trigger instant passes, send [`SlotEvent::SyncSlot`].
/// 4. Arm the next preparation deadline on the same head, up to the shared
///    replacement bound. A new head resets the allowance; exhaustion disarms it.
///
/// Between anchor triggers an internal `l2_block_time` ticker emits
/// [`SlotEvent::LiveTick`] carrying the latest anchor-computed sync
/// height, so the Sequencer advances one Live block per cadence rather
/// than in per-anchor bursts. Ticks are suppressed until the first L1
/// head (so they can't outrun the anchor) and sent with `try_send` — a
/// full channel just means the Sequencer is busy and the next tick
/// re-nudges, which beats blocking the armed trigger behind queued ticks.
///
/// Exits when the source closes or the receiver is dropped.
#[must_use]
pub fn spawn_l1_anchored(
    mut source: eez_l1::L1HeadStream,
    timing: RollupTiming,
    l2_genesis_timestamp: u64,
) -> mpsc::Receiver<SlotEvent> {
    let (tx, rx) = mpsc::channel(8);
    tokio::spawn(async move {
        let mut ticker = interval_at(
            Instant::now() + timing.l2_block_time(),
            timing.l2_block_time(),
        );
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        // Latest anchor-computed sync-slot height. None until the
        // first L1 head; LiveTicks are suppressed until then.
        let mut latest_sync_height: Option<u64> = None;
        // The shared submission bound also caps timers, including attempts
        // which cannot finish preparation. A new head resets this allowance.
        let mut pending: Option<PendingSyncSlot> = None;

        loop {
            // Remaining wait for the armed trigger, recomputed from unix
            // time each iteration so re-creating the sleep when another
            // select branch wins doesn't stretch the deadline. ZERO when
            // disarmed — the branch is disabled, the future never polled.
            let trigger_wait = pending.map_or(Duration::ZERO, |p| {
                // Align the wake to the `trigger_at` instant in ms. A
                // whole-second sleep wakes N seconds after the loop's
                // fractional phase, firing the trigger ~0.8s late and
                // eating the send-lead before the slot.
                p.remaining(now_unix_millis())
            });

            tokio::select! {
                maybe_head = source.next_head() => {
                    let Some(head) = maybe_head else {
                        event!(
                            name: "eez.slot.l1_anchored.source_closed",
                            Level::ERROR,
                            "L1 head source closed; L1-anchored scheduler exiting",
                        );
                        return;
                    };

                    let next = PendingSyncSlot::new(head, timing, l2_genesis_timestamp);
                    let block_height = next.block_height;

                    if let Some(stale) = pending.filter(|slot| slot.replacement_count == 0) {
                        // Only reachable when heads arrive faster than
                        // proof_window_open apart — a watcher catch-up
                        // burst. The newest anchor wins; the Sequencer's
                        // Catchup covers the skipped slot's span.
                        event!(
                            name: "eez.slot.l1_anchored.trigger_superseded",
                            Level::WARN,
                            stale_block_height = stale.block_height,
                            block_height,
                            l1_head = head.block_number,
                            "L1 head arrived before armed sync-slot trigger fired; superseding with newest anchor",
                        );
                    }
                    latest_sync_height = Some(block_height);
                    pending = Some(next);
                }
                () = tokio::time::sleep(trigger_wait), if pending.is_some() => {
                    let fired = pending
                        .take()
                        .expect("branch enabled only while pending.is_some()");
                    pending = fired.replacement(timing);
                    if fired.replacement_count == MAX_MISSING_SLOT_REPLACEMENTS {
                        event!(
                            name: "eez.slot.l1_anchored.replacement_limit",
                            Level::INFO,
                            event_name = "eez.slot.l1_anchored.replacement_limit",
                            l1_head = fired.l1_head,
                            l1_timestamp = fired.l1_timestamp,
                            last_target_timestamp = fired.timestamp,
                            replacements = fired.replacement_count,
                            "last bounded replacement trigger; waiting for a new L1 head afterward",
                        );
                    }
                    latest_sync_height = Some(fired.block_height);
                    event!(
                        name: "eez.slot.l1_anchored.trigger",
                        Level::DEBUG,
                        l1_head = fired.l1_head,
                        l1_timestamp = fired.l1_timestamp,
                        sync_slot_timestamp = fired.timestamp,
                        block_height = fired.block_height,
                        "sync-slot trigger firing",
                    );
                    if tx
                        .send(SlotEvent::SyncSlot {
                            block_height: fired.block_height,
                            timestamp: fired.timestamp,
                            l1_head: fired.l1_head,
                        })
                        .await
                        .is_err()
                    {
                        return; // Sequencer dropped the receiver; exit cleanly.
                    }
                }
                _ = ticker.tick() => {
                    let Some(sync_slot_block_height) = latest_sync_height else {
                        continue; // No anchor yet; ticks must not outrun it.
                    };
                    match tx.try_send(SlotEvent::LiveTick { sync_slot_block_height }) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            // Sequencer is mid-burst; the next tick
                            // re-nudges, nothing is lost.
                            event!(
                                name: "eez.slot.l1_anchored.live_tick_skipped",
                                Level::DEBUG,
                                sync_slot_block_height,
                                "schedule channel full; skipping live tick",
                            );
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            return; // Sequencer dropped the receiver; exit cleanly.
                        }
                    }
                }
            }
        }
    });
    rx
}

/// Wall-clock unix time in milliseconds — the defer-on-lateness check
/// needs sub-second precision.
pub(crate) fn now_unix_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_slot_replacements_are_bounded_and_do_not_invent_an_l1_block() {
        let timing = RollupTiming::new(5000, 1000, 1000, 700).with_preparation_time_ms(500);
        let first = PendingSyncSlot::new(
            eez_l1::L1HeadInfo {
                block_number: 7,
                block_hash: [1; 32],
                timestamp: 100,
            },
            timing,
            1,
        );
        let retry = first.replacement(timing).unwrap();
        assert_eq!((first.timestamp, first.trigger_at_ms), (105, 102_800));
        assert_eq!(
            (
                retry.timestamp,
                retry.trigger_at_ms,
                retry.block_height,
                retry.l1_head
            ),
            (110, 107_800, 109, 7)
        );
        let mut last = retry;
        for replacement_count in 2..=4 {
            last = last.replacement(timing).unwrap();
            assert_eq!(last.timestamp, 105 + 5 * replacement_count);
            assert_eq!(last.trigger_at_ms, 102_800 + 5_000 * replacement_count);
            assert_eq!(last.block_height, 104 + 5 * replacement_count);
            assert_eq!(last.l1_head, 7);
            assert_eq!(last.replacement_count, replacement_count);
        }
        assert!(last.replacement(timing).is_none());
        let delayed = PendingSyncSlot::new(
            eez_l1::L1HeadInfo {
                block_number: 8,
                block_hash: [2; 32],
                timestamp: 105,
            },
            timing,
            1,
        );
        assert_eq!(delayed.timestamp, retry.timestamp);
        assert_eq!(delayed.l1_head, 8);
        assert_eq!(delayed.replacement_count, 0);
        assert!(delayed.replacement(timing).is_some());
    }

    #[test]
    fn missing_slot_replacement_rejects_overflow_without_wrapping() {
        let timing = RollupTiming::new(5000, 1000, 1000, 700);
        let first = PendingSyncSlot::new(
            eez_l1::L1HeadInfo {
                block_number: 7,
                block_hash: [1; 32],
                timestamp: 100,
            },
            timing,
            1,
        );
        for invalid in [
            PendingSyncSlot {
                trigger_at_ms: u64::MAX,
                ..first
            },
            PendingSyncSlot {
                block_height: u64::MAX,
                ..first
            },
            PendingSyncSlot {
                timestamp: u64::MAX,
                ..first
            },
        ] {
            assert!(invalid.replacement(timing).is_none());
        }
    }

    #[test]
    fn scheduler_preserves_fractional_trigger_and_catches_up_without_extending_it() {
        let head = eez_l1::L1HeadInfo {
            block_number: 7,
            block_hash: [1; 32],
            timestamp: 100,
        };
        let base = RollupTiming::new(5000, 1000, 500, 700);
        let pending = PendingSyncSlot::new(head, base.with_preparation_time_ms(500), 1);
        let old = PendingSyncSlot::new(head, base, 1);
        assert_eq!(
            (pending.block_height, pending.timestamp, pending.l1_head),
            (104, 105, 7)
        );
        assert_eq!(
            (old.block_height, old.timestamp),
            (pending.block_height, pending.timestamp)
        );
        assert_eq!(old.remaining(103_100), Duration::ZERO);
        assert_eq!(pending.remaining(103_100), Duration::from_millis(200));
        assert_eq!(pending.remaining(103_299), Duration::from_millis(1));
        assert_eq!(pending.remaining(103_300), Duration::ZERO);
        assert_eq!(pending.remaining(105_000), Duration::ZERO);
    }
}
