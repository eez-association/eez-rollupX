//! Engine-API consumer that advances the chain on a 2s cadence aligned to
//! wall-clock time, with **greedy backfill** of any timestamp slots that
//! were missed due to a reorg or downtime.
//!
//! [`Sequencer::run`] is the long-running loop. Each iteration handles one
//! of:
//!
//! 1. A scheduler tick. `Sequencer::advance` runs in a loop, producing
//!    blocks at deterministic `parent.timestamp() + BLOCK_TIME_SECS`
//!    slots until the chain catches up to the tick's wall-clock target.
//!    Most ticks produce a single block (the chain is on cadence); after
//!    a reorg or a stall, the same tick may produce several filler
//!    blocks to backfill the slots that passed during the rewind.
//! 2. A slower FCU-refresh timer, used to keep reth's engine view alive
//!    during quiet periods.
//!
//! The Sequencer does **not** keep a local mirror of the canonical head.
//! Every iteration of `advance` reads the current head from the
//! [`BlockCommitterHandle::last_header`] — the actor is the single source
//! of truth, updated synchronously inside every Sequence + Derive command.
//! This is what fixes the race where the Deriver-driven canonical change
//! was visible to the actor immediately, but the Sequencer's own mirror
//! lagged behind reth's `CanonStateNotification` broadcast — producing
//! "invalid payload attributes" errors when the Sequencer issued FCU+attrs
//! based on a stale parent.

use core::fmt;
use std::{sync::Arc, time::Duration};

use alloy_primitives::{Address, B256};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_ethereum_engine_primitives::EthPayloadAttributes;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{
    AlloyBlockHeader, HeaderTy, NodePrimitives, SealedHeader, SealedHeaderFor,
};
use tracing::{Level, event};

use tokio::sync::mpsc;

use crate::block_committer::BlockCommitterHandle;
use crate::error::{DriverError, DriverResult};
use crate::slot::{SlotEvent, SlotKind, SyncSlotComposerHandle, SyncSlotMode};
use crate::timing::{RollupTiming, SlotComposition};

/// How often the sequencer re-publishes the current forkchoice state.
///
/// Even when no new block is being proposed, sending an FCU keeps reth's
/// engine convinced the chain is alive. Mirrors the cadence reth's own dev
/// miner uses.
const FCU_REFRESH: Duration = Duration::from_secs(1);

/// Max speculative gap the Sequencer can run ahead of L1-confirmed
/// cursor. Pauses beyond this so reth's state-retention
/// window keeps recent ancestors alive for the Deriver's replay.
pub const DEFAULT_MAX_SPECULATIVE_DEPTH: u64 = 64;

/// Reconciliation can replace the parent or its authenticated execution seed
/// after composition. Stop this trigger and rebuild from the current context
/// on the next one. `phase` identifies the commit site.
fn log_stale_candidate(phase: &str) {
    event!(
        name: "eez.sequencer.stale_candidate",
        Level::DEBUG,
        phase,
        "execution context changed before commit; next trigger rebuilds the candidate",
    );
}

// The cross-chain composer owns its remaining pipeline estimate. The scheduler
// rejects only an expired target; ordinary-only posting retains its prior gate.
fn sync_slot_too_late(
    timing: RollupTiming,
    cross_chain: bool,
    timestamp: u64,
    now_ms: u64,
) -> bool {
    let slack_ms = u64::try_from(timing.submission_slack().as_millis()).unwrap_or(u64::MAX);
    let deadline_ms = timestamp.saturating_mul(1000).saturating_sub(slack_ms);
    if cross_chain || timing.start_after_block().is_some() {
        now_ms >= deadline_ms
    } else {
        let proof_ms = u64::try_from(timing.proof_time().as_millis()).unwrap_or(u64::MAX);
        now_ms.saturating_add(proof_ms) > deadline_ms
    }
}

/// One volatile cutoff decision. The expected tip is advanced only by this
/// sequencer's successful commits; a branch substitution invalidates the plan.
#[derive(Debug, Clone, Copy)]
struct SlotDecision {
    target: u64,
    l1_parent: eez_l1::L1BlockRef,
    generation: u64,
    tip: B256,
    /// Only a successful not-due hint suppresses posting at its boundary.
    /// Errors/late recovery retain the existing Empty policy opportunity.
    skip_post: bool,
}

impl SlotDecision {
    fn matches(
        self,
        target: u64,
        tip: B256,
        snapshot: eez_l1::l1_canonical_head::PolicyAnchorSnapshot,
    ) -> bool {
        self.target == target
            && self.tip == tip
            && self.generation == snapshot.generation
            && snapshot.through == Some(self.l1_parent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slot::{ParentContext, SyncSlotBlock, SyncSlotComposer, SyncSlotOutcome};
    use std::sync::Mutex;

    #[test]
    fn late_cross_chain_entry_uses_shared_deadline_and_preserves_ordinary_gate() {
        let timing = RollupTiming::new(5000, 1000, 450, 450);
        assert!(!sync_slot_too_late(timing, false, 10, 9100));
        assert!(sync_slot_too_late(timing, false, 10, 9101));
        assert!(!sync_slot_too_late(timing, true, 10, 9101));
        assert!(!sync_slot_too_late(timing, true, 10, 9549));
        assert!(sync_slot_too_late(timing, true, 10, 9550));
        assert!(sync_slot_too_late(timing, true, 10, 9551));
    }

    #[derive(Default)]
    struct PolicyComposer {
        result: &'static str,
        calls: Arc<Mutex<Vec<SyncSlotMode>>>,
        prepare: Arc<std::sync::atomic::AtomicBool>,
        plans: Arc<Mutex<Vec<u64>>>,
    }

    #[async_trait::async_trait]
    impl SyncSlotComposer for PolicyComposer {
        async fn should_prepare(
            &self,
            _: u64,
            parent: ParentContext,
            _: u64,
            _: u64,
            _: eez_l1::L1BlockRef,
        ) -> Result<bool, String> {
            self.plans.lock().unwrap().push(parent.header.number());
            if self.result == "plan-error" {
                return Err("canonical planning evidence unavailable".into());
            }
            Ok(self.prepare.load(std::sync::atomic::Ordering::SeqCst))
        }

        fn set_sequencing_target(&self, _: u64, _: u64) {}

        fn ordinary_block_da_limit(
            &self,
            _: u64,
            _: &SealedHeader<alloy_consensus::Header>,
            _: u64,
        ) -> Result<usize, String> {
            Ok(usize::MAX)
        }

        fn ordinary_transaction_da_limit(&self, _: u64) -> Result<usize, String> {
            Ok(usize::MAX)
        }

        async fn compose_sync_slot(
            &self,
            _: u64,
            parent: ParentContext,
            timestamp: u64,
            target: Option<u64>,
            mode: SyncSlotMode,
        ) -> SyncSlotOutcome {
            assert_eq!(target.is_some(), mode == SyncSlotMode::Steady);
            self.calls.lock().unwrap().push(mode);
            if self.result == "end" || (self.result == "end-steady" && mode == SyncSlotMode::Steady)
            {
                return SyncSlotOutcome::EndTick;
            }
            let header = SealedHeader::seal_slow(alloy_consensus::Header {
                parent_hash: if self.result == "stale" {
                    B256::ZERO
                } else {
                    parent.header.hash()
                },
                number: parent.header.number + 1,
                timestamp,
                transactions_root: alloy_consensus::constants::EMPTY_ROOT_HASH,
                mix_hash: if self.result == "stale-seed" {
                    B256::repeat_byte(0x55)
                } else {
                    B256::ZERO
                },
                ..Default::default()
            });
            SyncSlotOutcome::Built(Box::new(SyncSlotBlock {
                payload: alloy_rpc_types_engine::ExecutionData::from_block_unchecked(
                    header.hash(),
                    &alloy_consensus::Block::<alloy_consensus::TxEnvelope>::new(
                        header.clone().unseal(),
                        Default::default(),
                    ),
                ),
                header,
            }))
        }
    }

    #[tokio::test]
    async fn explicit_policy_outcomes_never_use_sync_pool_fallback() {
        for mode in [SyncSlotMode::Steady, SyncSlotMode::Empty] {
            for result in ["end", "empty", "stale", "stale-seed"] {
                let future = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + 3600;
                let timestamp = if mode == SyncSlotMode::Empty {
                    5
                } else {
                    future
                };
                let parent = SealedHeader::seal_slow(alloy_consensus::Header {
                    number: 4,
                    timestamp: timestamp - 1,
                    ..Default::default()
                });
                let l1_head = test_seed_head(&parent);
                let (committer, commands) = crate::block_committer::test_committer(parent);
                let calls = Arc::new(Mutex::new(Vec::new()));
                let composer: SyncSlotComposerHandle = Arc::new(PolicyComposer {
                    result,
                    calls: Arc::clone(&calls),
                    ..Default::default()
                });
                let (_, receiver) = mpsc::channel(1);
                let mut sequencer = Sequencer::composer(
                    EthAttributesBuilder::new(Arc::new(reth_chainspec::ChainSpec::default())),
                    committer.clone(),
                    receiver,
                    RollupTiming::new(5000, 1000, 1000, 100),
                    1,
                    composer,
                    0,
                    Arc::clone(&l1_head),
                );
                let target = 5;
                sequencer
                    .advance_sync_slot(target, timestamp, 1)
                    .await
                    .unwrap();
                let commands = commands.lock().unwrap();
                let live = usize::try_from(target - 5).unwrap();
                assert_eq!(*calls.lock().unwrap(), vec![mode]);
                assert!(commands[..live].iter().all(|command| *command == "pool"));
                match result {
                    "end" | "stale" | "stale-seed" => {
                        assert_eq!(commands.len(), live, "{mode:?}/{result}");
                        assert_eq!(committer.last_header().number, target - 1);
                    }
                    "empty" => {
                        assert_eq!(&commands[live..], &["prebuilt"]);
                        assert_eq!(committer.last_header().number, target);
                        assert_eq!(
                            committer.last_header().transactions_root,
                            alloy_consensus::constants::EMPTY_ROOT_HASH
                        );
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    #[tokio::test]
    async fn position_cutoff_orders_ticks_and_defers_late_work_without_a_burst() {
        use std::sync::atomic::{AtomicBool, Ordering};
        for prepare in [false, true] {
            for tick_first in [false, true] {
                let now = crate::slot::now_unix_millis() / 1000;
                let parent = SealedHeader::seal_slow(alloy_consensus::Header {
                    number: 1,
                    timestamp: now - 1,
                    ..Default::default()
                });
                let l1_head = test_seed_head(&parent);
                let l1_parent = eez_l1::L1BlockRef {
                    number: 7,
                    hash: B256::repeat_byte(7),
                };
                l1_head.mark_scanned(
                    l1_parent,
                    eez_l1::L2Endpoint {
                        number: 0,
                        hash: B256::ZERO,
                    },
                );
                let (committer, commands) = crate::block_committer::test_committer(parent);
                let calls = Arc::new(Mutex::new(Vec::new()));
                let plans = Arc::new(Mutex::new(Vec::new()));
                let available = Arc::new(AtomicBool::new(prepare));
                let (_, receiver) = mpsc::channel(1);
                let mut sequencer = Sequencer::composer(
                    EthAttributesBuilder::new(Arc::new(reth_chainspec::ChainSpec::default())),
                    committer.clone(),
                    receiver,
                    RollupTiming::new(5000, 1000, 100, 100).with_start_after_block(2),
                    1,
                    Arc::new(PolicyComposer {
                        result: "empty",
                        calls: calls.clone(),
                        plans: plans.clone(),
                        prepare: available.clone(),
                    }),
                    0,
                    l1_head,
                );
                if tick_first {
                    sequencer
                        .advance_anchored_live_tick_at(5, now * 1000)
                        .await
                        .unwrap();
                }
                sequencer
                    .advance_position_slot(5, now + 3, l1_parent)
                    .await
                    .unwrap();
                assert_eq!(
                    *plans.lock().unwrap(),
                    vec![2],
                    "cutoff must be committed before planning"
                );
                if prepare {
                    assert_eq!(committer.last_header().number(), 5);
                    assert_eq!(*calls.lock().unwrap(), vec![SyncSlotMode::Steady]);
                } else {
                    assert_eq!(committer.last_header().number(), 2);
                    available.store(true, Ordering::SeqCst);
                    sequencer
                        .advance_position_slot(5, now + 3, l1_parent)
                        .await
                        .unwrap();
                    assert_eq!(
                        plans.lock().unwrap().len(),
                        1,
                        "duplicate trigger cannot reopen cutoff"
                    );
                    sequencer
                        .advance_anchored_live_tick_at(5, now * 1000 + 999)
                        .await
                        .unwrap();
                    assert_eq!(
                        committer.last_header().number(),
                        2,
                        "no future ordinary block"
                    );
                    for offset in 1..=3 {
                        sequencer
                            .advance_anchored_live_tick_at(5, (now + offset) * 1000)
                            .await
                            .unwrap();
                        assert_eq!(committer.last_header().number(), 2 + offset);
                    }
                    assert_eq!(*calls.lock().unwrap(), vec![SyncSlotMode::Ordinary]);
                }
                assert_eq!(
                    *commands.lock().unwrap(),
                    vec!["pool", "pool", "pool", "prebuilt"]
                );
                sequencer
                    .advance_anchored_live_tick_at(5, (now + 10) * 1000)
                    .await
                    .unwrap();
                assert_eq!(
                    committer.last_header().number(),
                    5,
                    "cannot outrun known target"
                );
            }
        }
    }

    #[tokio::test]
    async fn position_end_tick_finishes_empty_and_stale_evidence_never_opens_the_suffix() {
        use std::sync::atomic::AtomicBool;
        for result in ["end-steady", "plan-error", "empty"] {
            let now = crate::slot::now_unix_millis() / 1000;
            let parent = SealedHeader::seal_slow(alloy_consensus::Header {
                number: 2,
                timestamp: now,
                ..Default::default()
            });
            let l1_head = test_seed_head(&parent);
            let l1_parent = eez_l1::L1BlockRef {
                number: 7,
                hash: B256::repeat_byte(7),
            };
            let anchor = eez_l1::L2Endpoint {
                number: 0,
                hash: B256::ZERO,
            };
            l1_head.mark_scanned(l1_parent, anchor);
            let (committer, commands) = crate::block_committer::test_committer(parent);
            let calls = Arc::new(Mutex::new(Vec::new()));
            let plans = Arc::new(Mutex::new(Vec::new()));
            let (_, receiver) = mpsc::channel(1);
            let mut sequencer = Sequencer::composer(
                EthAttributesBuilder::new(Arc::new(reth_chainspec::ChainSpec::default())),
                committer.clone(),
                receiver,
                RollupTiming::new(5000, 1000, 100, 100).with_start_after_block(2),
                1,
                Arc::new(PolicyComposer {
                    result,
                    calls: calls.clone(),
                    plans: plans.clone(),
                    prepare: Arc::new(AtomicBool::new(result == "end-steady")),
                }),
                0,
                l1_head.clone(),
            );
            let stale = eez_l1::L1BlockRef {
                hash: B256::repeat_byte(8),
                ..l1_parent
            };
            sequencer
                .advance_position_slot(5, now + 3, stale)
                .await
                .unwrap();
            assert!(plans.lock().unwrap().is_empty());
            let planned = sequencer.advance_position_slot(5, now + 3, l1_parent).await;
            if result == "plan-error" {
                planned.unwrap();
                sequencer
                    .advance_anchored_live_tick_at(5, (now + 3) * 1000)
                    .await
                    .unwrap();
                assert_eq!(committer.last_header().number(), 5);
                assert_eq!(*calls.lock().unwrap(), vec![SyncSlotMode::Empty]);
            } else if result == "end-steady" {
                planned.unwrap();
                assert_eq!(committer.last_header().number(), 4);
                sequencer
                    .advance_anchored_live_tick_at(5, (now + 3) * 1000)
                    .await
                    .unwrap();
                assert_eq!(committer.last_header().number(), 5);
                assert_eq!(
                    *calls.lock().unwrap(),
                    vec![SyncSlotMode::Steady, SyncSlotMode::Empty]
                );
                assert_eq!(*commands.lock().unwrap(), vec!["pool", "pool", "prebuilt"]);
            } else {
                planned.unwrap();
                let decision = sequencer.slot_decision.unwrap();
                assert!(!decision.matches(
                    5,
                    B256::repeat_byte(99),
                    l1_head.policy_anchor_snapshot()
                ));
                l1_head.mark_scanned(l1_parent, anchor);
                sequencer
                    .advance_anchored_live_tick_at(5, (now + 3) * 1000)
                    .await
                    .unwrap();
                assert_eq!(committer.last_header().number(), 5);
                assert_eq!(*plans.lock().unwrap(), vec![2, 2]);
                assert_eq!(*calls.lock().unwrap(), vec![SyncSlotMode::Ordinary]);
            }
        }
    }

    #[tokio::test]
    async fn position_ticks_retry_scan_lag_and_preserve_the_prior_boundary_before_replacement_prefix()
     {
        let now = crate::slot::now_unix_millis() / 1000;
        let parent = SealedHeader::seal_slow(alloy_consensus::Header {
            number: 2,
            timestamp: now,
            ..Default::default()
        });
        let l1_head = test_seed_head(&parent);
        let l1_parent = eez_l1::L1BlockRef {
            number: 7,
            hash: B256::repeat_byte(7),
        };
        let (committer, _) = crate::block_committer::test_committer(parent);
        let calls = Arc::new(Mutex::new(Vec::new()));
        let plans = Arc::new(Mutex::new(Vec::new()));
        let (_, receiver) = mpsc::channel(1);
        let mut sequencer = Sequencer::composer(
            EthAttributesBuilder::new(Arc::new(reth_chainspec::ChainSpec::default())),
            committer.clone(),
            receiver,
            RollupTiming::new(5000, 1000, 100, 100).with_start_after_block(2),
            1,
            Arc::new(PolicyComposer {
                result: "empty",
                calls: calls.clone(),
                plans: plans.clone(),
                ..Default::default()
            }),
            0,
            l1_head.clone(),
        );
        sequencer
            .advance_position_slot(5, now + 3, l1_parent)
            .await
            .unwrap();
        assert!(
            plans.lock().unwrap().is_empty(),
            "incomplete scan cannot decide"
        );
        l1_head.mark_scanned(
            l1_parent,
            eez_l1::L2Endpoint {
                number: 0,
                hash: B256::ZERO,
            },
        );
        sequencer
            .advance_anchored_live_tick_at(5, (now + 1) * 1000)
            .await
            .unwrap();
        assert_eq!(*plans.lock().unwrap(), vec![2]);
        assert_eq!(committer.last_header().number(), 3);
        // No new L1 head: scheduler has published a bounded replacement at T.
        sequencer.position_target = Some((10, now + 8, l1_parent));
        sequencer
            .advance_anchored_live_tick_at(10, (now + 3) * 1000)
            .await
            .unwrap();
        assert_eq!(committer.last_header().number(), 5);
        assert_eq!(*calls.lock().unwrap(), vec![SyncSlotMode::Ordinary]);
        sequencer
            .advance_anchored_live_tick_at(10, (now + 4) * 1000)
            .await
            .unwrap();
        assert_eq!(
            committer.last_header().number(),
            6,
            "replacement position 1 does not wait for the cutoff"
        );
        assert_eq!(
            *plans.lock().unwrap(),
            vec![2],
            "replacement cutoff is not due yet"
        );
    }

    fn test_seed_head(parent: &alloy_consensus::Header) -> Arc<eez_l1::L1CanonicalHead> {
        use eez_protocol::l2::{DerivationConfig, GenesisL1Reference, Schedule};
        let timestamp = parent.timestamp.checked_sub(parent.number).unwrap();
        let genesis = alloy_consensus::Header {
            timestamp,
            ..Default::default()
        };
        let schedule = Schedule::new(
            DerivationConfig {
                block_interval_seconds: 1,
                sync_interval_blocks: 5,
                l1_reference: GenesisL1Reference {
                    chain_id: 1337,
                    number: 0,
                    hash: B256::repeat_byte(1),
                    timestamp,
                    prev_randao: B256::ZERO,
                },
            },
            &genesis,
        )
        .unwrap();
        let head = Arc::new(eez_l1::L1CanonicalHead::default());
        head.restore_derivation(eez_l1::derivation::SeedHistory::new(schedule));
        head
    }

    #[tokio::test]
    async fn guarded_catchup_preserves_every_intermediate_empty_boundary() {
        for k in [5, 6, 12] {
            for residue in 0..k {
                for result in ["empty", "end", "stale"] {
                    let parent = SealedHeader::seal_slow(alloy_consensus::Header {
                        number: k - 1 + residue,
                        timestamp: 100,
                        ..Default::default()
                    });
                    let l1_head = test_seed_head(&parent);
                    let (committer, commands) = crate::block_committer::test_committer(parent);
                    let calls = Arc::new(Mutex::new(Vec::new()));
                    let (_, receiver) = mpsc::channel(1);
                    let mut sequencer = Sequencer::composer(
                        EthAttributesBuilder::new(Arc::new(reth_chainspec::ChainSpec::default())),
                        committer.clone(),
                        receiver,
                        RollupTiming::new(u32::try_from(k * 1000).unwrap(), 1000, 1000, 100),
                        1,
                        Arc::new(PolicyComposer {
                            result,
                            calls: calls.clone(),
                            ..Default::default()
                        }),
                        0,
                        Arc::clone(&l1_head),
                    );
                    for _ in 0..=(k * 2) {
                        let parent = committer.last_header();
                        let outcome = sequencer
                            .commit_one(SlotKind::Live, &parent, 3 * k + residue)
                            .await;
                        if result != "empty" {
                            assert!(outcome.is_err());
                            assert_eq!(committer.last_header().number, k - 1 + residue);
                            assert!(
                                commands.lock().unwrap().is_empty(),
                                "failed empty boundary must not use the mempool"
                            );
                            break;
                        }
                        outcome.unwrap();
                        if committer.last_header().number % k == residue {
                            assert_eq!(
                                committer.last_header().transactions_root,
                                alloy_consensus::constants::EMPTY_ROOT_HASH
                            );
                        }
                    }
                    if result == "empty" {
                        assert_eq!(*calls.lock().unwrap(), vec![SyncSlotMode::Empty; 3]);
                        assert_eq!(
                            commands
                                .lock()
                                .unwrap()
                                .iter()
                                .filter(|command| **command == "prebuilt")
                                .count(),
                            3
                        );
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn catchup_can_use_the_current_future_slot_but_never_a_past_or_capped_slot() {
        for k in [5, 6, 12] {
            for residue in 0..k {
                for case in ["aligned", "late", "misaligned", "capped", "speculative_cap"] {
                    let start = k - 1 + residue;
                    let target = if case == "capped" { 1000 * k } else { 3 * k } + residue;
                    let timestamp = if case == "late" {
                        100_000
                    } else {
                        crate::slot::now_unix_millis() / 1000 + 3600
                    };
                    let parent = SealedHeader::seal_slow(alloy_consensus::Header {
                        number: start,
                        timestamp: timestamp - (target - start) + u64::from(case == "misaligned"),
                        ..Default::default()
                    });
                    let l1_head = test_seed_head(&parent);
                    let (committer, commands) = crate::block_committer::test_committer(parent);
                    let calls = Arc::new(Mutex::new(Vec::new()));
                    let (_, receiver) = mpsc::channel(1);
                    let timing =
                        RollupTiming::new(u32::try_from(k * 1000).unwrap(), 1000, 500, 700)
                            .with_preparation_time_ms(500);
                    let mut sequencer = Sequencer::composer(
                        EthAttributesBuilder::new(Arc::new(reth_chainspec::ChainSpec::default())),
                        committer.clone(),
                        receiver,
                        timing,
                        1,
                        Arc::new(PolicyComposer {
                            result: "empty",
                            calls: Arc::clone(&calls),
                            ..Default::default()
                        }),
                        u64::from(case == "speculative_cap"),
                        Arc::clone(&l1_head),
                    );
                    sequencer
                        .advance_sync_slot(target, timestamp, 100)
                        .await
                        .unwrap();
                    let calls = calls.lock().unwrap();
                    assert_eq!(
                        *calls.last().unwrap(),
                        if case == "aligned" {
                            SyncSlotMode::Steady
                        } else {
                            SyncSlotMode::Catchup
                        },
                        "K={k}, residue={residue}, {case}",
                    );
                    assert!(
                        calls[..calls.len() - 1]
                            .iter()
                            .all(|mode| *mode == SyncSlotMode::Empty)
                    );
                    let final_header = committer.last_header();
                    assert_eq!(final_header.number % k, residue);
                    assert_eq!(
                        final_header.transactions_root,
                        alloy_consensus::constants::EMPTY_ROOT_HASH
                    );
                    if case == "capped" {
                        assert!(final_header.number < target);
                        assert!(final_header.number - start <= crate::MAX_BLOCKS_PER_CATCHUP);
                    } else {
                        assert_eq!(final_header.number, target);
                    }
                    assert_eq!(
                        commands
                            .lock()
                            .unwrap()
                            .iter()
                            .filter(|c| **c == "prebuilt")
                            .count(),
                        calls.len()
                    );
                }
            }
        }
    }
}

/// Builds payload attributes from captured, parent-derived time and RANDAO.
/// The sequencer obtains both together from authenticated canonical history.
#[derive(Debug, Clone)]
pub struct EthAttributesBuilder<ChainSpec> {
    chain_spec: Arc<ChainSpec>,
    suggested_fee_recipient: Address,
}

impl<ChainSpec> EthAttributesBuilder<ChainSpec> {
    /// Creates a builder using the given chainspec and a zero fee recipient.
    #[must_use]
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self {
            chain_spec,
            suggested_fee_recipient: Address::ZERO,
        }
    }

    /// Configures the `suggested_fee_recipient` field on built attributes.
    #[must_use]
    pub const fn with_fee_recipient(mut self, recipient: Address) -> Self {
        self.suggested_fee_recipient = recipient;
        self
    }
}

impl<ChainSpec> EthAttributesBuilder<ChainSpec>
where
    ChainSpec: EthChainSpec + EthereumHardforks + Send + Sync + 'static,
{
    /// Build with the exact checked position; never clamp or normalize time.
    #[must_use]
    pub fn build(&self, timestamp: u64, prev_randao: B256) -> EthPayloadAttributes {
        EthPayloadAttributes {
            timestamp,
            prev_randao,
            suggested_fee_recipient: self.suggested_fee_recipient,
            withdrawals: self
                .chain_spec
                .is_shanghai_active_at_timestamp(timestamp)
                .then(Default::default),
            parent_beacon_block_root: self
                .chain_spec
                .is_cancun_active_at_timestamp(timestamp)
                .then_some(B256::ZERO),
            // Amsterdam-fork addition; not active for stage-1 dev chains.
            slot_number: None,
            target_gas_limit: None,
        }
    }
}

/// Translates scheduler proposals into engine-API `forkchoiceUpdated`
/// then `newPayload` via the [`BlockCommitter`](crate::BlockCommitterHandle).
/// Stateless w.r.t. head — reads `committer.last_header()` (the actor
/// writes synchronously on every Sequence/Derive).
pub struct Sequencer<T, ChainSpec>
where
    T: PayloadTypes<PayloadAttributes = EthPayloadAttributes>,
{
    attributes: EthAttributesBuilder<ChainSpec>,
    schedule_rx: mpsc::Receiver<SlotEvent>,
    committer: BlockCommitterHandle<T>,
    /// Per-rollup timing (L1/L2 cadence, proof window, slack). Used to
    /// compute per-trigger Live/Future/Sync block composition and per-
    /// block timestamps.
    timing: RollupTiming,
    rollup_id: u64,
    composer: SyncSlotComposerHandle,
    max_speculative_depth: u64,
    l1_head: Arc<eez_l1::L1CanonicalHead>,
    /// Consecutive Sync blocks skipped for off-grid drift. A skip self-heals
    /// next trigger; a run of them is a stall and must be loud (invariant 7).
    off_grid_skips: u32,
    slot_decision: Option<SlotDecision>,
    position_target: Option<(u64, u64, eez_l1::L1BlockRef)>,
}

impl<T, ChainSpec> fmt::Debug for Sequencer<T, ChainSpec>
where
    T: PayloadTypes<PayloadAttributes = EthPayloadAttributes>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sequencer")
            .field("committer", &self.committer)
            .field("rollup_id", &self.rollup_id)
            .field("max_speculative_depth", &self.max_speculative_depth)
            .finish_non_exhaustive()
    }
}

impl<T, ChainSpec> Sequencer<T, ChainSpec>
where
    T: PayloadTypes<
            PayloadAttributes = EthPayloadAttributes,
            ExecutionData = alloy_rpc_types_engine::ExecutionData,
        > + Send
        + Sync
        + 'static,
    <T::BuiltPayload as BuiltPayload>::Primitives:
        NodePrimitives<BlockHeader = alloy_consensus::Header> + Send + Sync + 'static,
    SealedHeaderFor<<T::BuiltPayload as BuiltPayload>::Primitives>: Send,
    ChainSpec: EthChainSpec<Header = HeaderTy<<T::BuiltPayload as BuiltPayload>::Primitives>>
        + EthereumHardforks
        + Send
        + Sync
        + 'static,
{
    /// Construct an L1-anchored composer sequencer. All composer-only
    /// dependencies are mandatory and share the binary-owned committer with
    /// the Composer and Deriver.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn composer(
        attributes: EthAttributesBuilder<ChainSpec>,
        committer: BlockCommitterHandle<T>,
        schedule_rx: mpsc::Receiver<SlotEvent>,
        timing: RollupTiming,
        rollup_id: u64,
        composer: SyncSlotComposerHandle,
        max_speculative_depth: u64,
        l1_head: Arc<eez_l1::L1CanonicalHead>,
    ) -> Self {
        Self {
            attributes,
            schedule_rx,
            committer,
            timing,
            rollup_id,
            composer,
            max_speculative_depth,
            l1_head,
            off_grid_skips: 0,
            slot_decision: None,
            position_target: None,
        }
    }

    /// Runs the sequencer loop until cancellation. Errors during advance
    /// are logged and the loop continues; channel-closed = Scheduler
    /// task exited, Sequencer exits cleanly.
    pub async fn run(mut self) {
        let mut fcu_interval = tokio::time::interval(FCU_REFRESH);
        loop {
            tokio::select! {
                maybe_event = self.schedule_rx.recv() => {
                    let Some(event) = maybe_event else {
                        event!(
                            name: "eez.sequencer.schedule_rx.closed",
                            Level::ERROR,
                            "schedule channel closed; sequencer task exiting",
                        );
                        break;
                    };
                    if let Err(err) = self.advance(event).await {
                        event!(
                            name: "eez.sequencer.advance.failed",
                            Level::ERROR,
                            error = %err,
                            "advance failed: {{error}}",
                        );
                    }
                }
                _ = fcu_interval.tick() => {
                    if let Err(err) = self.committer.refresh_forkchoice().await {
                        event!(
                            name: "eez.sequencer.fcu.failed",
                            Level::WARN,
                            error = %err,
                            "forkchoice refresh failed: {{error}}",
                        );
                    }
                }
            }
        }
    }

    /// Dispatch on slot event variant.
    async fn advance(&mut self, event: SlotEvent) -> DriverResult<()> {
        match event {
            SlotEvent::SyncSlot {
                block_height,
                timestamp,
                l1_head,
                l1_hash,
            } => {
                if self.timing.start_after_block().is_some() {
                    self.advance_position_slot(
                        block_height,
                        timestamp,
                        eez_l1::L1BlockRef {
                            number: l1_head,
                            hash: l1_hash,
                        },
                    )
                    .await
                } else {
                    self.advance_sync_slot(block_height, timestamp, l1_head)
                        .await
                }
            }
            SlotEvent::LiveTick {
                sync_slot_block_height,
                timestamp,
                l1_head,
                l1_hash,
            } => {
                self.position_target = Some((
                    sync_slot_block_height,
                    timestamp,
                    eez_l1::L1BlockRef {
                        number: l1_head,
                        hash: l1_hash,
                    },
                ));
                self.advance_anchored_live_tick(sync_slot_block_height)
                    .await
            }
        }
    }

    /// Legacy ticks commit one block within the Live region. Position-mode
    /// ticks catch up only already-due blocks, bounded by MAX_BLOCKS_PER_CATCHUP,
    /// and retry an unavailable cutoff without requiring another L1 head.
    async fn advance_anchored_live_tick(
        &mut self,
        sync_slot_block_height: u64,
    ) -> DriverResult<()> {
        self.advance_anchored_live_tick_at(sync_slot_block_height, crate::slot::now_unix_millis())
            .await
    }

    async fn advance_anchored_live_tick_at(
        &mut self,
        sync_slot_block_height: u64,
        now_ms: u64,
    ) -> DriverResult<()> {
        let budget = if self.timing.start_after_block().is_some() {
            crate::MAX_BLOCKS_PER_CATCHUP
        } else {
            1
        };
        for _ in 0..budget {
            let before = self.committer.last_header().hash();
            self.advance_one_anchored_tick(sync_slot_block_height, now_ms)
                .await?;
            if before == self.committer.last_header().hash() {
                break;
            }
        }
        Ok(())
    }

    async fn advance_one_anchored_tick(
        &mut self,
        mut sync_slot_block_height: u64,
        now_ms: u64,
    ) -> DriverResult<()> {
        let rollup_id = self.rollup_id;
        let composer = Arc::clone(&self.composer);
        // Before the head read: a mid-slot verdict would otherwise wait K
        // blocks, and recovery can reorg + substitute under us.
        composer.recover_failed(rollup_id).await;
        if self.timing.start_after_block().is_some() {
            let parent = self.committer.last_header();
            let snapshot = self.l1_head.policy_anchor_snapshot();
            if let Some(decision) = self.slot_decision.filter(|d| {
                parent.number() < d.target && d.matches(d.target, parent.hash(), snapshot)
            }) {
                // A newly armed target must not overwrite the previous
                // interval's authenticated non-posting boundary decision.
                sync_slot_block_height = decision.target;
            } else if let Some((target, timestamp, l1_parent)) = self.position_target
                && target == sync_slot_block_height
                && parent.number() < target
                && parent.number()
                    >= target.saturating_sub(u64::from(self.timing.future_count()) + 1)
                && now_ms
                    >= timestamp.saturating_mul(1000).saturating_sub(
                        self.timing
                            .l1_block_time()
                            .saturating_sub(self.timing.proof_window_open())
                            .as_millis() as u64,
                    )
            {
                self.advance_position_slot(target, timestamp, l1_parent)
                    .await?;
            }
        }
        let last_header = self.committer.last_header();
        let head = last_header.number();
        // Reserve = Future blocks + the Sync block itself. Mirrors
        // `live_region_end` in `RollupTiming::per_trigger_composition`.
        let reserve = u64::from(self.timing.future_count()) + 1;
        let live_region_end = sync_slot_block_height.saturating_sub(reserve);
        let limit = if self.timing.start_after_block().is_some() {
            let snapshot = self.l1_head.policy_anchor_snapshot();
            if self.slot_decision.is_some_and(|decision| {
                decision.matches(sync_slot_block_height, last_header.hash(), snapshot)
            }) {
                sync_slot_block_height
            } else {
                self.slot_decision = None;
                live_region_end
            }
        } else {
            live_region_end
        };
        if head >= limit {
            // Live region full (steady state between anchor fire and
            // the next L1 head). Future + Sync come from the anchor.
            return Ok(());
        }
        if self.timing.start_after_block().is_some()
            && last_header
                .timestamp()
                .saturating_add(self.timing.l2_block_time().as_secs())
                > now_ms / 1000
        {
            return Ok(());
        }
        if self.speculative_limit_paused(head) {
            return Ok(());
        }
        match self
            .commit_one(SlotKind::Live, &last_header, sync_slot_block_height)
            .await
        {
            Ok(()) => Ok(()),
            Err(err) if err.is_stale_candidate() => {
                log_stale_candidate("live_tick");
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    /// True if the bundle (ready at ~`now + proof_time`) can't reach the
    /// relay by `sync_slot_timestamp − submission_slack`, i.e. it would miss
    /// the immediate next L1 slot. Steady state has headroom; fires only on
    /// a late trigger, where the slot is deferred.
    fn sync_block_misses_l1_slot(&self, sync_slot_timestamp: u64) -> bool {
        sync_slot_too_late(
            self.timing,
            self.composer.has_cross_chain_work(self.rollup_id),
            sync_slot_timestamp,
            crate::slot::now_unix_millis(),
        )
    }

    /// One skip self-heals next trigger; a run of them means production is
    /// wedged, so escalate past a few.
    fn note_off_grid_skip(&mut self) {
        self.off_grid_skips = self.off_grid_skips.saturating_add(1);
        if self.off_grid_skips >= 3 {
            event!(
                name: "eez.sequencer.sync_slot.off_grid_stalled",
                Level::ERROR,
                consecutive_skips = self.off_grid_skips,
                "consecutive Sync blocks skipped for off-grid drift; production is not advancing",
            );
        }
    }

    /// L1-anchored handler: read current head, compute the per-trigger
    /// Live/Future/Sync split via
    /// [`RollupTiming::per_trigger_composition`], produce accordingly.
    async fn advance_sync_slot(
        &mut self,
        sync_slot_block_height: u64,
        sync_slot_timestamp: u64,
        l1_head: u64,
    ) -> DriverResult<()> {
        let rollup_id = self.rollup_id;
        let composer = Arc::clone(&self.composer);
        // Before the composition: a pending verdict would seed the whole burst
        // on a doomed head, for compose_sync_slot to reorg away at its end.
        composer.recover_failed(rollup_id).await;
        composer
            .prepare_next_slot(rollup_id, l1_head + 1, sync_slot_timestamp)
            .await;
        self.produce_slot_suffix(sync_slot_block_height, sync_slot_timestamp, l1_head)
            .await
    }

    /// Position mode decides only after a committed cutoff prefix. Both the
    /// reliable cutoff event and later ticks can retry unavailable context.
    async fn advance_position_slot(
        &mut self,
        target: u64,
        timestamp: u64,
        l1_parent: eez_l1::L1BlockRef,
    ) -> DriverResult<()> {
        self.position_target = Some((target, timestamp, l1_parent));
        let composer = Arc::clone(&self.composer);
        composer.recover_failed(self.rollup_id).await;
        let l1_target = l1_parent
            .number
            .checked_add(1)
            .ok_or_else(|| DriverError::provider("L1 target overflow"))?;
        composer
            .prepare_next_slot(self.rollup_id, l1_target, timestamp)
            .await;
        let snapshot = self.l1_head.policy_anchor_snapshot();
        if snapshot.through != Some(l1_parent) {
            log_stale_candidate("position.target");
            return Ok(());
        }
        let parent = self.committer.last_header();
        if parent.number() >= target
            || self
                .slot_decision
                .is_some_and(|d| d.matches(target, parent.hash(), snapshot))
        {
            return Ok(());
        }
        self.slot_decision = None;
        let cutoff = target.saturating_sub(u64::from(self.timing.future_count()) + 1);
        for _ in 0..crate::MAX_BLOCKS_PER_CATCHUP {
            let parent = self.committer.last_header();
            if parent.number() >= cutoff {
                break;
            }
            if parent
                .timestamp()
                .saturating_add(self.timing.l2_block_time().as_secs())
                > crate::slot::now_unix_millis() / 1000
                || (parent.number() >= target.saturating_sub(u64::from(self.timing.k()))
                    && self.speculative_limit_paused(parent.number()))
            {
                return Ok(());
            }
            match self.commit_one(SlotKind::Live, &parent, target).await {
                Ok(()) => {}
                Err(error) if error.is_stale_candidate() => {
                    log_stale_candidate("position.prefix");
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
        }
        let parent = self.committer.last_header();
        if parent.number() < cutoff || parent.number() >= target {
            return Ok(());
        }
        let expected_timestamp = (target - parent.number())
            .checked_mul(self.timing.l2_block_time().as_secs())
            .and_then(|offset| parent.timestamp().checked_add(offset));
        if expected_timestamp != Some(timestamp) || !self.l1_head.generation_is(snapshot.generation)
        {
            log_stale_candidate("position.cutoff");
            return Ok(());
        }
        let deadline_ms = timestamp
            .saturating_mul(1000)
            .saturating_sub(self.timing.submission_slack().as_millis() as u64);
        let remaining =
            Duration::from_millis(deadline_ms.saturating_sub(crate::slot::now_unix_millis()))
                .saturating_sub(composer.post_proof_reserve());
        let (prepare, skip_post) = if remaining.is_zero() {
            (false, false)
        } else {
            match tokio::time::timeout(
                remaining.min(self.timing.l2_block_time()),
                composer.should_prepare(
                    self.rollup_id,
                    crate::ParentContext {
                        header: parent.clone(),
                    },
                    target,
                    timestamp,
                    l1_parent,
                ),
            )
            .await
            {
                Ok(Ok(prepare)) => (prepare, !prepare),
                Ok(Err(error)) => {
                    event!(Level::WARN, %error, target, "preparation hint unavailable; preserving paced recovery path");
                    (false, false)
                }
                Err(_) => (false, false),
            }
        };
        if !self.l1_head.generation_is(snapshot.generation)
            || self.committer.last_header().hash() != parent.hash()
        {
            log_stale_candidate("position.plan");
            return Ok(());
        }
        self.slot_decision = Some(SlotDecision {
            target,
            l1_parent,
            generation: snapshot.generation,
            tip: parent.hash(),
            skip_post,
        });
        event!(name: "eez.sequencer.slot.decision", Level::INFO,
            event_name = "eez.sequencer.slot.decision", target, timestamp, prepare,
            cutoff_hash = %parent.hash(), generation = snapshot.generation, skip_post,
            "slot cutoff decided; late work cannot change an ordinary decision");
        let reserve_ms =
            u64::try_from(composer.post_proof_reserve().as_millis()).unwrap_or(u64::MAX);
        if prepare
            && crate::slot::now_unix_millis().saturating_add(reserve_ms) < deadline_ms
            && !self.sync_block_misses_l1_slot(timestamp)
        {
            self.produce_slot_suffix(target, timestamp, l1_parent.number)
                .await?;
        }
        Ok(())
    }

    async fn produce_slot_suffix(
        &mut self,
        sync_slot_block_height: u64,
        sync_slot_timestamp: u64,
        l1_head: u64,
    ) -> DriverResult<()> {
        let rollup_id = self.rollup_id;
        let composer = Arc::clone(&self.composer);
        let head = self.committer.last_header().number();
        // Catchup ignores the speculative cap (a steady-state bound): a
        // dropped pb heals by recomposing next trigger, which freezing to
        // Idle would block. The steady (Slot) arm still enforces the cap.
        let comp = self.timing.per_trigger_composition(
            head,
            sync_slot_block_height,
            crate::MAX_BLOCKS_PER_CATCHUP,
        );

        event!(
            name: "eez.sequencer.sync_slot.composition",
            Level::INFO,
            head,
            sync_slot_block_height,
            sync_slot_timestamp,
            composition = ?comp,
            "computed per-trigger slot composition",
        );

        // Target an absolute height, not a count. The committer is shared, so a
        // count re-derived from a moved head would land the Sync block off-grid.
        let terminal = match comp {
            SlotComposition::Catchup { live } => Some(head + live + 1),
            SlotComposition::Slot { .. } | SlotComposition::Idle => None,
        };

        match comp {
            SlotComposition::Idle => {}
            SlotComposition::Catchup { live } => {
                // `live` is grid-snapped + capped by per_trigger_composition;
                // produce that many Live blocks then the terminal Sync.
                for _ in 0..live {
                    let last_header = self.committer.last_header();
                    // Once the burst reaches this target's Future region, use
                    // the ordinary slot layout even though older intervals had
                    // to be rebuilt first. Intermediate Sync boundaries remain
                    // empty through commit_one's existing grid guard.
                    let live_end = sync_slot_block_height
                        .saturating_sub(u64::from(self.timing.future_count()) + 1);
                    let kind = if terminal == Some(sync_slot_block_height)
                        && last_header.number() >= live_end
                    {
                        SlotKind::Future
                    } else {
                        SlotKind::Live
                    };
                    match self
                        .commit_one(kind, &last_header, sync_slot_block_height)
                        .await
                    {
                        Ok(()) => {}
                        Err(err) if err.is_stale_candidate() => {
                            log_stale_candidate("catchup.live");
                            return Ok(());
                        }
                        Err(err) => return Err(err),
                    }
                }

                // A capped/past terminal remains structural-only. If catch-up
                // reached the actual future target, recheck its timestamp and
                // deadline now: beginning the tick behind must not cost another
                // otherwise eligible L1 opportunity.
                if terminal != Some(self.committer.last_header().number() + 1) {
                    event!(
                        name: "eez.sequencer.sync_slot.off_grid_skipped",
                        Level::WARN,
                        head_now = self.committer.last_header().number(),
                        terminal = ?terminal,
                        "head moved during the burst; skipping the Sync block rather than placing it off-grid",
                    );
                    self.note_off_grid_skip();
                    return Ok(());
                }
                let last_header = self.committer.last_header();
                let sync_ts = last_header
                    .timestamp()
                    .saturating_add(self.timing.l2_block_time().as_secs());

                let can_settle = terminal == Some(sync_slot_block_height)
                    && sync_ts == sync_slot_timestamp
                    && !self.sync_block_misses_l1_slot(sync_ts)
                    && self.position_context_matches(sync_slot_block_height, last_header.hash())
                    && !self.speculative_limit_paused(last_header.number());
                let (target, mode) = if can_settle {
                    (Some(l1_head + 1), SyncSlotMode::Steady)
                } else {
                    (None, SyncSlotMode::Catchup)
                };

                let parent = crate::slot::ParentContext {
                    header: last_header.clone(),
                };
                let prebuilt = composer
                    .compose_sync_slot(rollup_id, parent, sync_ts, target, mode)
                    .await;
                let prebuilt = match prebuilt {
                    crate::SyncSlotOutcome::Built(built) => *built,
                    crate::SyncSlotOutcome::EndTick => return Ok(()),
                };
                if let Err(err) = self.commit_one_prebuilt(SlotKind::Sync, prebuilt).await {
                    if err.is_stale_candidate() {
                        log_stale_candidate("catchup.sync.prebuilt");
                        return Ok(());
                    }
                    return Err(err);
                }
                self.off_grid_skips = 0;
            }
            SlotComposition::Slot { live, future } => {
                for _ in 0..live {
                    let last_header = self.committer.last_header();
                    if self.speculative_limit_paused(last_header.number()) {
                        return Ok(()); // Defer Future + Sync to next trigger.
                    }
                    match self
                        .commit_one(SlotKind::Live, &last_header, sync_slot_block_height)
                        .await
                    {
                        Ok(()) => {}
                        Err(err) if err.is_stale_candidate() => {
                            log_stale_candidate("slot.live");
                            return Ok(());
                        }
                        Err(err) => return Err(err),
                    }
                }
                for _ in 0..future {
                    let last_header = self.committer.last_header();
                    if self.speculative_limit_paused(last_header.number()) {
                        return Ok(()); // Defer remaining Future + Sync to next trigger.
                    }
                    match self
                        .commit_one(SlotKind::Future, &last_header, sync_slot_block_height)
                        .await
                    {
                        Ok(()) => {}
                        Err(err) if err.is_stale_candidate() => {
                            log_stale_candidate("slot.future");
                            return Ok(());
                        }
                        Err(err) => return Err(err),
                    }
                }
                let last_header = self.committer.last_header();
                if self.speculative_limit_paused(last_header.number()) {
                    return Ok(()); // Defer the Sync (postBatch terminal) to next trigger.
                }
                let expected_sync_ts = last_header
                    .timestamp()
                    .saturating_add(self.timing.l2_block_time().as_secs());
                if expected_sync_ts != sync_slot_timestamp {
                    // The head moved under us. This block would settle, and an
                    // off-grid root wedges the deriver. Skip; next trigger realigns.
                    event!(
                        name: "eez.sequencer.sync_slot.off_grid_skipped",
                        Level::WARN,
                        expected = expected_sync_ts,
                        sync_slot_timestamp,
                        head = last_header.number(),
                        sync_slot_block_height,
                        "sync-slot drift: parent + L2_block_time != the L1-derived sync timestamp; skipping the Sync block rather than settling off-grid",
                    );
                    self.note_off_grid_skip();
                    return Ok(());
                }

                // Defer-on-lateness: a late trigger that can't land the bundle
                // in the next L1 slot commits an empty block and holds the pool
                // (next slot covers it). Else drain into a prebuilt Sync block.
                // Check against expected_sync_ts — the ts the bundle is pinned
                // to below — so an off-grid trigger already past its pin defers
                // instead of composing a bundle that can't make its slot.
                let prebuilt = if self.sync_block_misses_l1_slot(expected_sync_ts)
                    || !self.position_context_matches(sync_slot_block_height, last_header.hash())
                {
                    event!(
                        name: "eez.sequencer.sync_slot.deferred_late",
                        Level::WARN,
                        sync_slot_block_height,
                        sync_slot_timestamp,
                        l1_head,
                        "trigger too late for the immediate L1 slot; committing an empty block and holding the pool — next slot's batch covers it",
                    );
                    // Empty mode keeps this grid height batch-boundary-eligible.
                    // The explicit outcome also lets a failed required empty
                    // build end the tick without admitting pool transactions.
                    let parent = crate::slot::ParentContext {
                        header: last_header.clone(),
                    };
                    composer
                        .compose_sync_slot(
                            rollup_id,
                            parent,
                            expected_sync_ts,
                            None,
                            SyncSlotMode::Empty,
                        )
                        .await
                } else {
                    let parent = crate::slot::ParentContext {
                        header: last_header.clone(),
                    };
                    composer
                        // Steady: aim the immediate next L1 block, pinned to
                        // expected_sync_ts (re-derivable; == the L1 slot ts on-grid).
                        .compose_sync_slot(
                            rollup_id,
                            parent,
                            expected_sync_ts,
                            Some(l1_head + 1),
                            SyncSlotMode::Steady,
                        )
                        .await
                };
                let prebuilt = match prebuilt {
                    crate::SyncSlotOutcome::Built(built) => *built,
                    crate::SyncSlotOutcome::EndTick => return Ok(()),
                };
                match self.commit_one_prebuilt(SlotKind::Sync, prebuilt).await {
                    Ok(()) => {}
                    Err(err) if err.is_stale_candidate() => {
                        log_stale_candidate("slot.sync.prebuilt");
                        return Ok(());
                    }
                    Err(err) => return Err(err),
                }
                self.off_grid_skips = 0;
            }
        }
        Ok(())
    }

    /// True if the speculative-depth limit is reached; logged at DEBUG.
    fn speculative_limit_paused(&self, head_number: u64) -> bool {
        if self.max_speculative_depth == 0 {
            return false;
        }
        let confirmed = self.l1_head.cursor();
        let speculative_depth = head_number.saturating_sub(confirmed);
        if speculative_depth >= self.max_speculative_depth {
            event!(
                name: "eez.sequencer.speculative.paused",
                Level::DEBUG,
                head_number,
                confirmed,
                speculative_depth,
                max_depth = self.max_speculative_depth,
                "paused: speculative depth at cap; waiting for Deriver to catch up",
            );
            return true;
        }
        false
    }

    /// Commit a pre-built Sync block (from the cross-chain composer)
    /// via `commit_derived` — same engine-API tail the Deriver uses
    /// for L1-derived blocks. Holds the reconcile lock so a sync-slot
    /// commit can't race with a deriver-side reconcile.
    async fn commit_one_prebuilt(
        &mut self,
        kind: SlotKind,
        built: crate::slot::SyncSlotBlock,
    ) -> DriverResult<()> {
        // `commit_derived` doesn't take the lock itself (only
        // `commit_sequenced` does, and the Deriver already holds it via
        // `begin_reconcile`), so hold it here too.
        let _reconcile_guard = self.committer.begin_reconcile().await?;
        // Stale-parent check under the lock, like commit_sequenced: the
        // Deriver (or a failed-batch recovery) can move the head between
        // compose and commit, and a prebuilt block on a stale parent
        // would silently reorg that newer work via commit_derived's FCU.
        let current_head = self.committer.last_header().hash();
        let built_parent = built.header.parent_hash();
        if current_head != built_parent {
            return Err(DriverError::stale_parent(built_parent, current_head));
        }
        let (timestamp, seed) = self
            .l1_head
            .next_block_context(&self.committer.last_header())
            .map_err(DriverError::provider)?;
        if built.header.timestamp != timestamp || built.header.mix_hash != seed {
            return Err(DriverError::stale_context(built_parent));
        }
        let block_number = built.header.number();
        let block_hash = built.header.hash();
        let block_timestamp = built.header.timestamp();
        let _outcome = self
            .committer
            // feed_witness=true: this is a PRODUCED Sync block — emit it to the
            // prover witness-capture feed (the follower/deriver passes false).
            .commit_derived(built.payload, built.header, true)
            .await?;

        self.advance_decision_tip(built_parent, block_hash);
        event!(
            name: "eez.sequencer.block.produced",
            Level::INFO,
            slot.kind = %kind,
            block.number = block_number,
            block.hash = %block_hash,
            block.timestamp = block_timestamp,
            "produced block {block_number} hash={block_hash} ts={block_timestamp} kind={kind}",
        );
        Ok(())
    }

    /// Commit one block of the given kind at
    /// `parent.timestamp() + L2_block_time`. Logs `eez.sequencer.block.produced`.
    async fn commit_one(
        &mut self,
        kind: SlotKind,
        parent: &SealedHeader<ChainSpec::Header>,
        sync_slot_height: u64,
    ) -> DriverResult<()> {
        self.composer
            .set_sequencing_target(self.rollup_id, sync_slot_height);
        let parent_hash = parent.hash();
        let (next_ts, _) = self
            .l1_head
            .next_block_context(parent)
            .map_err(DriverError::provider)?;
        if kind != SlotKind::Sync
            && parent.number().checked_add(1).is_some_and(|n| {
                n % u64::from(self.timing.k()) == sync_slot_height % u64::from(self.timing.k())
            })
        {
            // Catch-up may span several Sync intervals. Each intermediate grid
            // boundary must remain an admissible historical settlement endpoint.
            let mode = if self.slot_decision.is_some_and(|decision| {
                decision.skip_post
                    && parent.number().checked_add(1) == Some(decision.target)
                    && decision.matches(
                        decision.target,
                        parent.hash(),
                        self.l1_head.policy_anchor_snapshot(),
                    )
            }) {
                SyncSlotMode::Ordinary
            } else {
                SyncSlotMode::Empty
            };
            let outcome = self
                .composer
                .compose_sync_slot(
                    self.rollup_id,
                    crate::ParentContext {
                        header: parent.clone(),
                    },
                    next_ts,
                    None,
                    mode,
                )
                .await;
            return match outcome {
                crate::SyncSlotOutcome::Built(block) => {
                    self.commit_one_prebuilt(SlotKind::Sync, *block).await
                }
                _ => Err(DriverError::invalid_forkchoice(
                    "required empty DA boundary unavailable".to_owned(),
                )),
            };
        }
        let outcome = self
            .committer
            .commit_sequenced(parent_hash, || {
                let (timestamp, seed) = self
                    .l1_head
                    .next_block_context(parent)
                    .map_err(DriverError::provider)?;
                Ok(self.attributes.build(timestamp, seed))
            })
            .await?;
        let timestamp = outcome.header.timestamp();
        let block_number = outcome.header.number();
        let block_hash = outcome.header.hash();

        self.advance_decision_tip(parent_hash, block_hash);

        event!(
            name: "eez.sequencer.block.produced",
            Level::INFO,
            slot.kind = %kind,
            block.number = block_number,
            block.hash = %block_hash,
            block.timestamp = timestamp,
            "produced block {block_number} hash={block_hash} ts={timestamp} kind={kind}",
        );
        Ok(())
    }

    fn position_context_matches(&self, target: u64, tip: B256) -> bool {
        self.timing.start_after_block().is_none()
            || self.slot_decision.is_some_and(|decision| {
                decision.matches(target, tip, self.l1_head.policy_anchor_snapshot())
            })
    }

    fn advance_decision_tip(&mut self, parent: B256, hash: B256) {
        if let Some(decision) = &mut self.slot_decision {
            if decision.tip == parent && self.l1_head.generation_is(decision.generation) {
                decision.tip = hash;
            } else {
                self.slot_decision = None;
            }
        }
    }
}
