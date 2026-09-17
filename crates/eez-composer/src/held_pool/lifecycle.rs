//! Diagnostic admission identities follow the existing pool owner. They never
//! authorize execution, replacement, recovery or eviction. Only active/retained
//! owners keep metadata in memory; durable history belongs to the observer.
use super::{Direction, HeldPool, SourceState};
use alloy_primitives::B256;
use eez_telemetry::{
    Publisher, Ticket,
    schema::{self, Assurance, BlockReason, Event, Hash32, Member, Outcome, Source},
};
use std::{collections::HashMap, time::Instant};

mod attempts;
mod local;
pub(super) use local::LocalCapture;
mod preparation;
mod reconciliation;
pub(crate) use attempts::PreparationFacts;
pub(crate) use reconciliation::{ReconciliationFacts, SourceObservation};
const MAX_MEMBERS: usize = 4096;

#[derive(Debug, Clone, Copy)]
pub struct ChainIdentity {
    pub chain_id: u64,
    pub genesis_hash: B256,
}
#[derive(Debug, Clone, Copy)]
pub struct LifecycleContext {
    pub l1: ChainIdentity,
    pub l2: ChainIdentity,
    pub rollup_id: u64,
}
impl LifecycleContext {
    fn source(self, hash: B256, direction: Direction) -> Source {
        let (chain, rollup_id, direction) = match direction {
            Direction::Inbound => (self.l1, 0, schema::Direction::Inbound),
            Direction::Outbound => (self.l2, self.rollup_id, schema::Direction::Outbound),
        };
        Source {
            chain_id: chain.chain_id,
            genesis_hash: Hash32(chain.genesis_hash.0),
            rollup_id,
            transaction_hash: Hash32(hash.0),
            direction,
        }
    }
}
#[derive(Debug)]
struct Admission {
    member: Member,
    admitted_at: Instant,
    readiness: Option<(SourceState, Option<BlockReason>)>,
}
#[derive(Debug)]
pub(super) struct Lifecycle {
    publisher: Publisher,
    context: LifecycleContext,
    admissions: HashMap<B256, Admission>,
}
/// Tickets and small typed snapshots only. Call publish after releasing the
/// pool (and therefore its nested ledger) lock; encoding and queue I/O live there.
pub(super) struct Pending {
    publisher: Publisher,
    events: Vec<(Ticket, Event)>,
    failure: Option<(Ticket, eez_telemetry::CaptureFailure)>,
}
impl Pending {
    pub(super) fn publish(self) {
        if let Some((ticket, failure)) = self.failure {
            self.publisher.discard(ticket, failure);
        }
        for (ticket, event) in self.events {
            self.publisher.publish(ticket, event, None);
        }
    }
}
impl Lifecycle {
    fn members(&self, txs: &[crate::HeldTx]) -> Result<Vec<Member>, eez_telemetry::CaptureFailure> {
        use eez_telemetry::CaptureFailure;
        if txs.len() > MAX_MEMBERS {
            return Err(CaptureFailure::Limit);
        }
        txs.iter()
            .map(|tx| {
                self.admissions
                    .get(&tx.hash)
                    .filter(|admission| {
                        admission.member.source == self.context.source(tx.hash, tx.direction)
                    })
                    .map(|admission| admission.member)
                    .ok_or(CaptureFailure::MissingContext)
            })
            .collect()
    }
    pub(super) fn admitted(
        &mut self,
        hash: B256,
        direction: Direction,
        replaced: Option<B256>,
    ) -> Pending {
        let mut pending = Pending {
            publisher: self.publisher.clone(),
            events: Vec::with_capacity(2),
            failure: None,
        };
        let previous = self
            .admissions
            .remove(&hash)
            .map(|admission| admission.member.admission);
        let replaced = replaced.and_then(|hash| self.admissions.remove(&hash));
        // The protocol transition already committed. If capture is unavailable,
        // do not attach an old admission ID to this newly accepted ownership.
        if let Some(ticket) = self.publisher.capture() {
            let member = Member {
                source: self.context.source(hash, direction),
                admission: ticket.admission_id(),
            };
            pending.events.push((
                ticket,
                Event::Admitted {
                    source: member.source,
                    previous,
                    replaced: replaced
                        .as_ref()
                        .map(|admission| admission.member.admission),
                },
            ));
            self.admissions.insert(
                hash,
                Admission {
                    member,
                    admitted_at: Instant::now(),
                    readiness: None,
                },
            );
        }
        if let Some(old) = replaced
            && let Some(ticket) = self.publisher.capture()
        {
            pending.events.push((
                ticket,
                Event::Terminal {
                    reconciliation: None,
                    member: old.member,
                    attempt: None,
                    outcome: Outcome::Replaced,
                    assurance: Assurance::LocalPolicy,
                    anchor: None,
                    elapsed_ns: old
                        .admitted_at
                        .elapsed()
                        .as_nanos()
                        .try_into()
                        .unwrap_or(u64::MAX),
                },
            ));
        }
        pending
    }
    pub(super) fn readiness(
        &mut self,
        hash: B256,
        source: SourceState,
        blocked: Option<BlockReason>,
    ) -> Option<Pending> {
        let admission = self.admissions.get_mut(&hash)?;
        if admission.readiness == Some((source, blocked)) {
            return None;
        }
        admission.readiness = Some((source, blocked));
        let ticket = self.publisher.capture()?;
        let block = |value: eez_l1::L1BlockRef| schema::BlockRef {
            number: value.number,
            hash: Hash32(value.hash.0),
        };
        Some(Pending {
            publisher: self.publisher.clone(),
            failure: None,
            events: vec![(
                ticket,
                Event::Readiness {
                    member: admission.member,
                    source_state: schema::SourceState {
                        l1: block(source.l1),
                        l2: block(source.l2),
                    },
                    blocked,
                },
            )],
        })
    }
    pub(super) fn retain(&mut self, status: &super::status::QueueTelemetry) {
        self.admissions.retain(|hash, _| status.has_admission(hash));
    }
    pub(super) fn invalidate_readiness(&mut self) {
        for admission in self.admissions.values_mut() {
            admission.readiness = None;
        }
    }
}
impl super::PoolState {
    pub(super) fn capture_readiness(
        &mut self,
        hash: B256,
        source: SourceState,
        blocked: Option<BlockReason>,
    ) -> Option<Pending> {
        if !self.by_hash.contains(&hash) && !self.in_flight.contains_key(&hash) {
            return None;
        }
        self.telemetry
            .native_readiness(hash, source, blocked.is_none());
        self.lifecycle.as_mut()?.readiness(hash, source, blocked)
    }
}

impl HeldPool {
    // Legacy policy does not populate RPC readiness. Record its validated group
    // observation without changing that API or its scheduling behavior.
    pub(crate) fn record_lifecycle_ready(&self, hash: B256, source: SourceState) {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        let pending = state.capture_readiness(hash, source, None);
        drop(state);
        if let Some(pending) = pending {
            pending.publish();
        }
    }

    /// Configure capture once, before ingress. Startup uses the actual embedded
    /// chain specifications; no additional environment-derived chain identity.
    /// A failed setup must disable/report telemetry without changing ownership.
    pub fn enable_lifecycle(
        &self,
        publisher: Publisher,
        context: LifecycleContext,
    ) -> Result<(), &'static str> {
        if context.rollup_id == 0 {
            return Err("lifecycle requires a nonzero L2 rollup identity");
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "held pool unavailable for lifecycle setup")?;
        if state.lifecycle.is_some()
            || !state.by_hash.is_empty()
            || !state.in_flight.is_empty()
            || state
                .ledger
                .read(|data| !data.users.is_empty() || !data.attempts.is_empty())
        {
            return Err("lifecycle capture must be configured once before admission");
        }
        state.lifecycle = Some(Lifecycle {
            publisher,
            context,
            admissions: HashMap::new(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    mod local;
    mod reconciliation;
    use super::*;
    use crate::HeldTx;
    use alloy_primitives::Address;
    use eez_telemetry::{
        SpoolConfig, Writer, reader,
        schema::{Identity, Record},
    };
    use std::{fs, time::Duration};
    fn context() -> LifecycleContext {
        LifecycleContext {
            l1: ChainIdentity {
                chain_id: 1,
                genesis_hash: B256::repeat_byte(11),
            },
            l2: ChainIdentity {
                chain_id: 2,
                genesis_hash: B256::repeat_byte(22),
            },
            rollup_id: 7,
        }
    }
    fn setup() -> (tempfile::TempDir, HeldPool, Publisher, Writer) {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o750)).unwrap();
        }
        let config: SpoolConfig =
            serde_json::from_value(serde_json::json!({"directory":dir.path()})).unwrap();
        let (publisher, writer) = Writer::start(
            Identity {
                environment: "devnet".into(),
                deployment: "capture".into(),
                service: "eez-composer".into(),
                instance: "fixture".into(),
            },
            config,
        )
        .unwrap();
        let pool = HeldPool::new(Default::default());
        pool.enable_lifecycle(publisher.clone(), context()).unwrap();
        (dir, pool, publisher, writer)
    }
    fn tx(tag: u8, direction: Direction) -> HeldTx {
        HeldTx {
            raw_tx: alloy_primitives::Bytes::from_static(
                b"private pending envelope must never be exported",
            ),
            hash: B256::repeat_byte(tag),
            sender: Address::repeat_byte(3),
            nonce: 0,
            attempts: 0,
            direction,
            max_fee_per_gas: 100,
            priority_fee_per_gas: 10,
        }
    }
    fn events(dir: &std::path::Path) -> Vec<(u64, Event)> {
        let mut chunks = std::collections::BTreeMap::new();
        for id in reader::inventory(dir).unwrap() {
            let mut offset = 0;
            loop {
                let batch = reader::read_segment(dir, id, offset).unwrap();
                for line in batch.lines {
                    let text = std::str::from_utf8(&line.bytes).unwrap();
                    assert!(!text.contains("private pending envelope"));
                    if let Record::Data(record) = serde_json::from_slice(&line.bytes).unwrap() {
                        chunks
                            .entry(record.sequence)
                            .or_insert_with(Vec::new)
                            .push(record);
                    }
                }
                offset = batch.next_offset;
                if batch.snapshot_complete {
                    break;
                }
            }
        }
        chunks
            .into_iter()
            .map(|(sequence, chunks)| {
                let payload = eez_telemetry::assemble(&chunks).unwrap();
                assert!(
                    !serde_json::to_string(&payload)
                        .unwrap()
                        .contains("private pending envelope")
                );
                (sequence, payload.event)
            })
            .collect()
    }
    #[test]
    fn accepted_transitions_only_with_explicit_replacement_and_chain_identity() {
        let (dir, pool, publisher, writer) = setup();
        let original = tx(1, Direction::Inbound);
        pool.push_contiguous(original.clone(), 0).unwrap();
        pool.push_contiguous(original.clone(), 0).unwrap();
        let mut rejected = tx(2, Direction::Inbound);
        rejected.nonce = 2;
        assert!(pool.push_contiguous(rejected, 0).is_err());
        assert!(pool.push_contiguous(tx(2, Direction::Inbound), 0).is_err());
        assert_eq!(publisher.stats().last_assigned_sequence, 1);
        let mut replacement = tx(2, Direction::Inbound);
        replacement.max_fee_per_gas = 110;
        replacement.priority_fee_per_gas = 11;
        pool.push_contiguous(replacement.clone(), 0).unwrap();
        pool.push_contiguous(tx(3, Direction::Outbound), 0).unwrap();
        assert_eq!(pool.len(), 2);
        assert!(pool.owns(replacement.hash));
        assert!(!pool.owns(original.hash));
        assert!(writer.shutdown(Duration::from_secs(8)));
        let events = events(dir.path());
        assert_eq!(events.len(), 4);
        let Event::Admitted {
            source: incoming,
            previous: None,
            replaced: None,
        } = events[0].1
        else {
            panic!("first admission")
        };
        assert_eq!(incoming.chain_id, 1);
        assert_eq!(incoming.rollup_id, 0);
        assert_eq!(incoming.genesis_hash, Hash32([11; 32]));
        let Event::Admitted {
            replaced: Some(old),
            ..
        } = events[1].1
        else {
            panic!("replacement link")
        };
        assert_eq!(old.sequence, events[0].0);
        assert_eq!(old.epoch, publisher.epoch());
        assert!(matches!(
            events[2].1,
            Event::Terminal {
                outcome: Outcome::Replaced,
                assurance: Assurance::LocalPolicy,
                anchor: None,
                ..
            }
        ));
        let Event::Admitted {
            source: outgoing, ..
        } = events[3].1
        else {
            panic!("outbound admission")
        };
        assert_eq!(outgoing.chain_id, 2);
        assert_eq!(outgoing.rollup_id, 7);
        assert_eq!(outgoing.genesis_hash, Hash32([22; 32]));
    }
    #[test]
    fn readiness_is_scoped_deduplicated_and_recovery_does_not_readmit() {
        let (dir, pool, publisher, writer) = setup();
        let held = tx(1, Direction::Inbound);
        pool.push_contiguous(held.clone(), 0).unwrap();
        let source = SourceState {
            l1: eez_l1::L1BlockRef {
                number: 1,
                hash: B256::repeat_byte(4),
            },
            l2: eez_l1::L1BlockRef {
                number: 2,
                hash: B256::repeat_byte(5),
            },
        };
        pool.begin_evaluation(source);
        pool.record_blocked(&held, eez_protocol::SourceCondition::Fee, source);
        pool.record_blocked(&held, eez_protocol::SourceCondition::Fee, source);
        pool.record_ready(&held, source);
        pool.record_ready(&held, source);
        let drained = pool.pop_all();
        pool.push_front_batch(drained);
        assert_eq!(publisher.stats().last_assigned_sequence, 3);
        // Same-height hash replacement is a new source observation.
        let changed = SourceState {
            l1: eez_l1::L1BlockRef {
                hash: B256::repeat_byte(6),
                ..source.l1
            },
            ..source
        };
        pool.record_ready(&held, changed);
        let _ = pool.evict_chain_at_or_above(held.sender, held.direction, held.nonce);
        pool.record_ready(&held, changed);
        pool.record_blocked(&held, eez_protocol::SourceCondition::Fee, changed);
        assert!(writer.shutdown(Duration::from_secs(8)));
        let events = events(dir.path());
        assert_eq!(events.len(), 5);
        assert!(matches!(
            events[4].1,
            Event::Terminal {
                outcome: Outcome::PoolDiscarded,
                assurance: Assurance::LocalPolicy,
                ..
            }
        ));
        assert_eq!(
            events
                .iter()
                .filter(|(_, event)| matches!(event, Event::Admitted { .. }))
                .count(),
            1
        );
        for (_, event) in events.iter().skip(1).take(3) {
            let Event::Readiness { member, .. } = event else {
                panic!("readiness observation")
            };
            assert_eq!(member.admission.sequence, events[0].0);
        }
    }

    #[test]
    fn legacy_readiness_capture_preserves_rpc_compatibility() {
        let (dir, pool, _, writer) = setup();
        let held = tx(1, Direction::Inbound);
        pool.push_contiguous(held.clone(), 0).unwrap();
        let source = SourceState {
            l1: eez_l1::L1BlockRef {
                number: 1,
                hash: B256::repeat_byte(4),
            },
            l2: eez_l1::L1BlockRef {
                number: 2,
                hash: B256::repeat_byte(5),
            },
        };
        let before = pool.queue_status();
        pool.record_lifecycle_ready(held.hash, source);
        pool.record_lifecycle_ready(held.hash, source);
        let after = pool.queue_status();
        for key in ["ready", "unknown", "sourceState", "evaluatedAtMs"] {
            assert_eq!(before[key], after[key]);
        }
        assert!(writer.shutdown(Duration::from_secs(8)));
        let events = events(dir.path());
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[1].1,
            Event::Readiness { blocked: None, .. }
        ));
    }

    #[test]
    fn closed_writer_and_disabled_capture_do_not_change_admission() {
        let (dir, pool, publisher, writer) = setup();
        assert!(writer.shutdown(Duration::from_secs(8)));
        pool.push_contiguous(tx(1, Direction::Inbound), 0).unwrap();
        assert_eq!(pool.len(), 1);
        assert_eq!(publisher.stats().last_assigned_sequence, 0);
        assert!(events(dir.path()).is_empty());
        let disabled = HeldPool::new(Default::default());
        disabled
            .push_contiguous(tx(2, Direction::Inbound), 0)
            .unwrap();
        assert_eq!(disabled.len(), 1);
        assert!(
            disabled
                .enable_lifecycle(publisher.clone(), context())
                .is_err()
        );
        assert!(pool.enable_lifecycle(publisher, context()).is_err());
    }

    fn manifest(txs: &[HeldTx]) -> eez_protocol::settlement_manifest::SettlementManifest {
        use eez_protocol::settlement_manifest::{
            SettlementManifest, SourceActionManifest, UserManifest,
        };
        SettlementManifest {
            sync_number: 10,
            sync_hash: B256::repeat_byte(80),
            effects: txs.iter().map(|tx| tx.hash).collect(),
            users: txs
                .iter()
                .enumerate()
                .map(|(index, tx)| UserManifest {
                    tx_hash: tx.hash,
                    source_rollup_id: eez_protocol::RollupId(
                        if tx.direction == Direction::Inbound {
                            0
                        } else {
                            7
                        },
                    ),
                    sender: tx.sender,
                    nonce: tx.nonce,
                    attempts: tx.attempts,
                    outbound_group: (tx.direction == Direction::Outbound).then_some(index),
                    effects: index..index + 1,
                    source_actions: if tx.direction == Direction::Inbound {
                        vec![SourceActionManifest {
                            call_hash: tx.hash,
                            source_index: index,
                        }]
                    } else {
                        Vec::new()
                    },
                })
                .collect(),
        }
    }

    fn prepare(
        pool: &HeldPool,
        txs: Vec<HeldTx>,
        manifest: eez_protocol::settlement_manifest::SettlementManifest,
    ) -> Result<crate::optimistic::AttemptId, String> {
        pool.begin_attempt(
            10,
            B256::repeat_byte(90),
            reth_primitives_traits::SealedHeader::seal_slow(alloy_consensus::Header {
                number: 9,
                ..Default::default()
            }),
            txs,
            manifest,
            alloy_primitives::Bytes::from_static(b"signed envelope is never exported"),
            preparation_facts(),
        )
    }

    fn preparation_facts() -> PreparationFacts {
        PreparationFacts {
            preparation: None,
            encoded_bytes: 4321,
            blob_count: 1,
            blob_allowance: 3,
            nonce: 77,
        }
    }

    #[tokio::test]
    async fn preparation_preserves_results_and_links_success_to_native_attempt() {
        use schema::PreparationOutcome;
        let (dir, pool, publisher, writer) = setup();
        let candidate = tx(1, Direction::Inbound);
        pool.push_contiguous(candidate.clone(), 0).unwrap();
        let sync = eez_l1::L2Endpoint {
            number: 10,
            hash: B256::repeat_byte(80),
        };
        let bytes = alloy_primitives::Bytes::from_static(b"private pending envelope");
        let started = Instant::now();
        let (returned, facts) = pool
            .observe_preparation(
                sync,
                std::slice::from_ref(&candidate),
                std::future::ready(Ok::<_, u8>((bytes.clone(), preparation_facts()))),
                |_| PreparationOutcome::ConstructionFailed,
            )
            .await
            .unwrap();
        let elapsed_bound = started.elapsed().as_nanos();
        assert_eq!(returned, bytes);
        assert_eq!(
            (
                facts.encoded_bytes,
                facts.blob_count,
                facts.blob_allowance,
                facts.nonce
            ),
            (4321, 1, 3, 77)
        );
        let id = facts.preparation.unwrap();
        assert_eq!(id.epoch, publisher.epoch());
        let txs = pool.pop_all();
        let manifest = manifest(&txs);
        pool.begin_attempt(
            10,
            B256::repeat_byte(90),
            reth_primitives_traits::SealedHeader::seal_slow(alloy_consensus::Header {
                number: 9,
                ..Default::default()
            }),
            txs,
            manifest,
            returned,
            facts,
        )
        .unwrap();
        let error = "private pending envelope";
        let result = pool
            .observe_preparation(
                sync,
                std::slice::from_ref(&candidate),
                std::future::ready(Err::<(alloy_primitives::Bytes, PreparationFacts), _>(error)),
                |_| PreparationOutcome::CapacityLimited,
            )
            .await;
        assert_eq!(result.unwrap_err(), error);
        assert!(writer.shutdown(Duration::from_secs(8)));
        let events = events(dir.path());
        assert_eq!(events.len(), 6);
        assert_eq!(events[1].0, id.sequence);
        let Event::PreparationStarted { members, .. } = &events[1].1 else {
            panic!("preparation membership")
        };
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].admission.sequence, events[0].0);
        assert_eq!(members[0].source.transaction_hash.0, candidate.hash.0);
        assert!(matches!(
            events[1].1,
            Event::PreparationStarted {
                l2_chain_id: 2,
                rollup_id: 7,
                sync: schema::BlockRef { number: 10, .. },
                ..
            }
        ));
        assert!(
            matches!(events[2].1, Event::PreparationFinished {preparation, outcome:PreparationOutcome::Prepared, elapsed_ns} if preparation == id && u128::from(elapsed_ns) <= elapsed_bound)
        );
        assert!(
            matches!(&events[3].1, Event::AttemptPrepared {details:Some(details), ..} if details.preparation == Some(id))
        );
        assert!(matches!(
            events[5].1,
            Event::PreparationFinished {
                outcome: PreparationOutcome::CapacityLimited,
                ..
            }
        ));
    }

    #[test]
    fn preparation_first_poll_and_drop_distinguish_unstarted_cancelled_and_panicked() {
        use schema::PreparationOutcome;
        use std::task::{Context, Poll, Waker};
        let (dir, pool, publisher, writer) = setup();
        let sync = eez_l1::L2Endpoint {
            number: 10,
            hash: B256::repeat_byte(80),
        };
        let pending =
            || std::future::pending::<Result<(alloy_primitives::Bytes, PreparationFacts), u8>>();
        drop(pool.observe_preparation(sync, &[], pending(), |_| {
            PreparationOutcome::ConstructionFailed
        }));
        assert_eq!(publisher.stats().last_assigned_sequence, 0);
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(pool.observe_preparation(sync, &[], pending(), |_| {
            PreparationOutcome::ConstructionFailed
        }));
        assert!(matches!(future.as_mut().poll(&mut context), Poll::Pending));
        assert_eq!(publisher.stats().last_assigned_sequence, 1);
        drop(future);
        assert_eq!(publisher.stats().last_assigned_sequence, 2);
        let mut future = Box::pin(pool.observe_preparation(
            sync,
            &[],
            std::future::poll_fn(
                |_| -> Poll<Result<(alloy_primitives::Bytes, PreparationFacts), u8>> {
                    panic!("controlled preparation panic")
                },
            ),
            |_| PreparationOutcome::ConstructionFailed,
        ));
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| future
                .as_mut()
                .poll(&mut context)))
            .is_err()
        );
        drop(future);
        assert_eq!(publisher.stats().last_assigned_sequence, 4);
        assert!(writer.shutdown(Duration::from_secs(8)));
        let events = events(dir.path());
        assert!(matches!(
            events[1].1,
            Event::PreparationFinished {
                outcome: PreparationOutcome::Cancelled,
                ..
            }
        ));
        assert!(matches!(
            events[3].1,
            Event::PreparationFinished {
                outcome: PreparationOutcome::Panicked,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn disabled_or_closed_preparation_capture_preserves_work_and_errors() {
        let (dir, pool, publisher, writer) = setup();
        assert!(writer.shutdown(Duration::from_secs(8)));
        let disabled = HeldPool::new(Default::default());
        let sync = eez_l1::L2Endpoint {
            number: 10,
            hash: B256::repeat_byte(80),
        };
        for pool in [&pool, &disabled] {
            let (_, facts) = pool
                .observe_preparation(
                    sync,
                    &[],
                    std::future::ready(Ok::<_, u8>((
                        alloy_primitives::Bytes::new(),
                        preparation_facts(),
                    ))),
                    |_| panic!("success must not classify an error"),
                )
                .await
                .unwrap();
            assert!(facts.preparation.is_none());
            let result = pool
                .observe_preparation(
                    sync,
                    &[],
                    std::future::ready(Err::<(alloy_primitives::Bytes, PreparationFacts), _>(7u8)),
                    |_| panic!("disabled capture must not classify an error"),
                )
                .await;
            assert_eq!(result.unwrap_err(), 7);
        }
        assert_eq!(publisher.stats().last_assigned_sequence, 0);
        assert!(events(dir.path()).is_empty());
    }

    #[tokio::test]
    async fn incomplete_preparation_membership_is_a_gap_without_cancelling_work() {
        use schema::PreparationOutcome;
        for oversized in [false, true] {
            let (dir, pool, publisher, writer) = setup();
            let candidates = vec![tx(1, Direction::Inbound); if oversized { 4097 } else { 1 }];
            let (_, facts) = pool
                .observe_preparation(
                    eez_l1::L2Endpoint {
                        number: 10,
                        hash: B256::repeat_byte(80),
                    },
                    &candidates,
                    std::future::ready(Ok::<_, u8>((
                        alloy_primitives::Bytes::new(),
                        preparation_facts(),
                    ))),
                    |_| PreparationOutcome::ConstructionFailed,
                )
                .await
                .unwrap();
            let id = facts.preparation.unwrap();
            assert_eq!(publisher.stats().dropped_records, 1);
            assert_eq!(publisher.stats().dropped_sequence_min, Some(id.sequence));
            assert!(writer.shutdown(Duration::from_secs(8)));
            let events = events(dir.path());
            assert_eq!(events.len(), 1);
            assert!(
                matches!(events[0].1, Event::PreparationFinished {preparation, outcome:PreparationOutcome::Prepared, ..} if preparation == id)
            );
        }
    }

    #[test]
    fn attempts_preserve_membership_and_only_committed_variants_and_dispatches_emit() {
        use crate::optimistic::SubmissionObservation;
        let (dir, pool, publisher, writer) = setup();
        pool.push_contiguous(tx(1, Direction::Inbound), 0).unwrap();
        pool.push_contiguous(tx(2, Direction::Outbound), 0).unwrap();
        let txs = pool.pop_all();
        let invalid = eez_protocol::settlement_manifest::SettlementManifest::empty(10, B256::ZERO);
        assert!(prepare(&pool, txs.clone(), invalid).is_err());
        assert_eq!(publisher.stats().last_assigned_sequence, 2);
        let attempt = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
        assert!(prepare(&pool, txs.clone(), manifest(&txs)).is_err());
        assert_eq!(publisher.stats().last_assigned_sequence, 3);
        let raw = alloy_primitives::Bytes::from_static(b"repriced private envelope");
        assert!(
            pool.replace_prepared_post_batch(attempt, attempt.post_batch_hash, raw.clone())
                .is_err()
        );
        let repriced = pool
            .replace_prepared_post_batch(attempt, B256::repeat_byte(91), raw.clone())
            .unwrap();
        assert_eq!(repriced.generation, attempt.generation);
        assert_eq!(
            pool.replace_prepared_post_batch(repriced, repriced.post_batch_hash, raw)
                .unwrap(),
            repriced
        );
        assert!(
            !pool
                .observe_attempt(attempt, SubmissionObservation::Uncertain, 77)
                .unwrap()
        );
        assert_eq!(publisher.stats().last_assigned_sequence, 4);
        let dispatch = SubmissionObservation::Dispatched {
            block: 100,
            timestamp: 200,
        };
        assert!(
            pool.observe_attempt(repriced, dispatch.clone(), 77)
                .unwrap()
        );
        assert!(pool.observe_attempt(repriced, dispatch, 77).is_err());
        assert!(
            pool.replace_prepared_post_batch(repriced, B256::repeat_byte(92), Default::default())
                .is_err()
        );
        assert_eq!(publisher.stats().last_assigned_sequence, 5);
        assert!(
            pool.observe_attempt(repriced, SubmissionObservation::Uncertain, 77)
                .unwrap()
        );
        assert_eq!(pool.pending_attempt().unwrap().txs.len(), 2);
        assert!(writer.shutdown(Duration::from_secs(8)));
        let events = events(dir.path());
        assert_eq!(events.len(), 6);
        let Event::AttemptPrepared {
            attempt: captured,
            members,
            details: Some(details),
            encoded_bytes,
            blob_count,
            blob_allowance,
            ..
        } = &events[2].1
        else {
            panic!("complete preparation")
        };
        assert_eq!(captured.generation, attempt.generation);
        assert_eq!(captured.epoch, publisher.epoch());
        assert_eq!((*encoded_bytes, *blob_count, *blob_allowance), (4321, 1, 3));
        assert_eq!(details.nonce, 77);
        assert_eq!(details.l2_genesis_hash, Hash32([22; 32]));
        assert_eq!(details.sync_hash, Hash32([80; 32]));
        assert_eq!(details.plans.len(), members.len());
        assert_eq!(
            details.plans[0].source_actions[0].call_hash,
            Hash32([1; 32])
        );
        assert_eq!(details.plans[1].outbound_group, Some(1));
        for (index, member) in members.iter().enumerate() {
            assert_eq!(member.admission.sequence, events[index].0);
        }
        assert!(
            matches!(events[3].1, Event::AttemptRepriced {previous, replacement} if previous.post_batch_hash != replacement.post_batch_hash && previous.generation == replacement.generation)
        );
        assert!(matches!(
            events[4].1,
            Event::Dispatched {
                nonce: 77,
                target_block: 100,
                target_timestamp: 200,
                ..
            }
        ));
        assert!(matches!(
            events[5].1,
            Event::SubmissionObserved {
                observation: schema::SubmissionObservation::Uncertain,
                ..
            }
        ));
    }

    #[test]
    fn retry_links_same_admissions_to_a_new_generation_without_counting_old_callbacks() {
        use crate::optimistic::{SourceOutcome, SubmissionObservation};
        let (dir, pool, publisher, writer) = setup();
        let held = tx(1, Direction::Inbound);
        pool.push_contiguous(held, 0).unwrap();
        let txs = pool.pop_all();
        let first = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
        pool.resolve_attempt(
            first,
            eez_l1::L1BlockRef {
                number: 100,
                hash: B256::repeat_byte(100),
            },
            &[SourceOutcome::Retryable { source_nonce: 0 }],
            false,
            3,
            || true,
        )
        .unwrap();
        let txs = pool.pop_all();
        let next = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
        assert_ne!(first.generation, next.generation);
        let before = publisher.stats().last_assigned_sequence;
        assert!(
            !pool
                .observe_attempt(first, SubmissionObservation::Uncertain, 77)
                .unwrap()
        );
        assert_eq!(publisher.stats().last_assigned_sequence, before);
        assert!(writer.shutdown(Duration::from_secs(8)));
        let events = events(dir.path());
        let memberships = events
            .iter()
            .filter_map(|(_, event)| {
                if let Event::AttemptPrepared { members, .. } = event {
                    Some(members)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(memberships.len(), 2);
        assert_eq!(memberships[0], memberships[1]);
        assert_eq!(
            events
                .iter()
                .filter(|(_, event)| matches!(event, Event::Admitted { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn oversized_or_missing_capture_context_leaves_explicit_gap_without_rejecting_attempt() {
        for missing in [false, true] {
            let (dir, pool, publisher, writer) = setup();
            let held = tx(1, Direction::Inbound);
            pool.push_contiguous(held.clone(), 0).unwrap();
            let txs = pool.pop_all();
            let mut manifest = manifest(&txs);
            if missing {
                pool.state
                    .lock()
                    .unwrap()
                    .lifecycle
                    .as_mut()
                    .unwrap()
                    .admissions
                    .clear();
            } else {
                manifest.effects = vec![B256::ZERO; 32769];
            }
            let id = prepare(&pool, txs, manifest).unwrap();
            assert_eq!(pool.pending_attempt().unwrap().id, id);
            assert!(pool.owns(held.hash));
            assert_eq!(publisher.stats().last_assigned_sequence, 2);
            assert_eq!(publisher.stats().dropped_records, 1);
            assert_eq!(publisher.stats().dropped_sequence_min, Some(2));
            assert!(writer.shutdown(Duration::from_secs(8)));
            assert_eq!(events(dir.path()).len(), 1);
        }
    }

    #[test]
    fn competing_dispatch_callbacks_publish_only_the_accepted_transition() {
        use crate::optimistic::SubmissionObservation;
        use std::sync::{Arc, Barrier};
        let (dir, pool, publisher, writer) = setup();
        let pool = Arc::new(pool);
        pool.push_contiguous(tx(1, Direction::Inbound), 0).unwrap();
        let txs = pool.pop_all();
        let attempt = prepare(&pool, txs.clone(), manifest(&txs)).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let callbacks = (0..2)
            .map(|offset| {
                let pool = pool.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    pool.observe_attempt(
                        attempt,
                        SubmissionObservation::Dispatched {
                            block: 100 + offset,
                            timestamp: 200 + offset,
                        },
                        77,
                    )
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = callbacks
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Ok(true)))
                .count(),
            1
        );
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        assert_eq!(publisher.stats().last_assigned_sequence, 3);
        let target = pool.pending_attempt().unwrap().dispatched_target.unwrap();
        assert!(writer.shutdown(Duration::from_secs(8)));
        let events = events(dir.path());
        let dispatches = events
            .iter()
            .filter_map(|(_, event)| match event {
                Event::Dispatched {
                    target_block,
                    target_timestamp,
                    ..
                } => Some((*target_block, *target_timestamp)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(dispatches, vec![target]);
    }
}
