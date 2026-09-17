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
}
impl Pending {
    pub(super) fn publish(self) {
        for (ticket, event) in self.events {
            self.publisher.publish(ticket, event, None);
        }
    }
}
impl Lifecycle {
    pub(super) fn admitted(
        &mut self,
        hash: B256,
        direction: Direction,
        replaced: Option<B256>,
    ) -> Pending {
        let mut pending = Pending {
            publisher: self.publisher.clone(),
            events: Vec::with_capacity(2),
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
        let lifecycle = self.lifecycle.as_mut()?;
        if !self.by_hash.contains(&hash) && !self.in_flight.contains_key(&hash) {
            return None;
        }
        lifecycle.readiness(hash, source, blocked)
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
        let mut events = Vec::new();
        for id in reader::inventory(dir).unwrap() {
            let mut offset = 0;
            loop {
                let batch = reader::read_segment(dir, id, offset).unwrap();
                for line in batch.lines {
                    let text = std::str::from_utf8(&line.bytes).unwrap();
                    assert!(!text.contains("private pending envelope"));
                    if let Record::Data(record) = serde_json::from_slice(&line.bytes).unwrap() {
                        let sequence = record.sequence;
                        let payload = eez_telemetry::assemble(&[record]).unwrap();
                        assert!(
                            !serde_json::to_string(&payload)
                                .unwrap()
                                .contains("private pending envelope")
                        );
                        events.push((sequence, payload.event));
                    }
                }
                offset = batch.next_offset;
                if batch.snapshot_complete {
                    break;
                }
            }
        }
        events.sort_by_key(|(sequence, _)| *sequence);
        events
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
        assert_eq!(events.len(), 4);
        assert_eq!(
            events
                .iter()
                .filter(|(_, event)| matches!(event, Event::Admitted { .. }))
                .count(),
            1
        );
        for (_, event) in events.iter().skip(1) {
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
}
