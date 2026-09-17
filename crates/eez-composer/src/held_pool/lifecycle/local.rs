//! Local disposal follows actual reservation removal. Retained canonical owners
//! remain unresolved even if an ordinary pool operation releases a reservation.
use super::{MAX_MEMBERS, Pending};
use crate::{
    held_pool::PoolState,
    ingress::Direction,
    optimistic::{AttemptId, UserLocation},
};
use alloy_primitives::B256;
use eez_telemetry::{
    CaptureFailure, Publisher,
    operational::TerminalObservation,
    schema::{self, Event, Member, Outcome},
};
use std::{collections::BTreeMap, time::Instant};

struct Candidate {
    member: Option<Member>,
    owner: Option<AttemptId>,
    admitted_at: Option<Instant>,
    metric_admitted_at: Option<Instant>,
    direction: Direction,
    outcome: Outcome,
}

pub(in crate::held_pool) struct LocalCapture {
    publisher: Option<Publisher>,
    metrics: bool,
    candidates: Result<BTreeMap<B256, Candidate>, CaptureFailure>,
}

/// Both diagnostic transports share the same authoritative removal check. Metrics
/// do not depend on lifecycle identity allocation or writer availability.
pub(in crate::held_pool) struct LocalPending {
    lifecycle: Option<Pending>,
    metrics: Vec<TerminalObservation>,
    metric_gap: Option<CaptureFailure>,
}
impl LocalPending {
    #[cfg(test)]
    pub(in crate::held_pool) fn observations(&self) -> &[TerminalObservation] {
        &self.metrics
    }
    pub(in crate::held_pool) fn publish(self) {
        for observation in self.metrics {
            observation.publish();
        }
        if let Some(failure) = self.metric_gap {
            eez_telemetry::operational::terminal_capture_gap(failure);
        }
        if let Some(pending) = self.lifecycle {
            pending.publish();
        }
    }
}

impl LocalCapture {
    /// Copy only active candidates before retention can prune their admission.
    /// No ticket is allocated until the existing mutation finishes.
    pub(in crate::held_pool) fn before(
        state: &PoolState,
        candidates: impl IntoIterator<Item = (B256, Outcome, Direction)>,
    ) -> Option<Self> {
        Self::capture(state, candidates, false)
    }

    pub(in crate::held_pool) fn before_unsubmitted(
        state: &PoolState,
        candidates: impl IntoIterator<Item = (B256, Outcome, Direction)>,
    ) -> Option<Self> {
        Self::capture(state, candidates, true)
    }

    fn capture(
        state: &PoolState,
        candidates: impl IntoIterator<Item = (B256, Outcome, Direction)>,
        unsubmitted_only: bool,
    ) -> Option<Self> {
        let lifecycle = state.lifecycle.as_ref();
        let metrics = state.telemetry.native_enabled();
        if lifecycle.is_none() && !metrics {
            return None;
        }
        let captured = state.ledger.read(|ledger| {
            let mut captured = BTreeMap::new();
            for (hash, outcome, direction) in candidates {
                if !state.by_hash.contains(&hash) && !state.in_flight.contains_key(&hash) {
                    continue;
                }
                let user = ledger.users.get(&hash);
                if unsubmitted_only && user.is_some() {
                    continue;
                }
                if user.is_some_and(|user| {
                    !matches!(
                        user.location,
                        UserLocation::Queued | UserLocation::Composing
                    )
                }) {
                    continue;
                }
                let admission = lifecycle.and_then(|lifecycle| lifecycle.admissions.get(&hash));
                let owner = user
                    .map(|user| {
                        ledger
                            .attempts
                            .get(&user.owner)
                            .map(|attempt| attempt.id)
                            .ok_or(CaptureFailure::MissingContext)
                    })
                    .transpose()?;
                captured.entry(hash).or_insert(Candidate {
                    member: admission.map(|admission| admission.member),
                    owner,
                    admitted_at: admission.map(|admission| admission.admitted_at),
                    metric_admitted_at: state.telemetry.native_admission_started(hash),
                    direction,
                    outcome,
                });
                if captured.len() > MAX_MEMBERS {
                    return Err(CaptureFailure::Limit);
                }
            }
            Ok(captured)
        });
        Some(Self {
            publisher: lifecycle.map(|lifecycle| lifecycle.publisher.clone()),
            metrics,
            candidates: captured,
        })
    }

    pub(in crate::held_pool) fn after(self, state: &PoolState) -> LocalPending {
        let mut pending = LocalPending {
            lifecycle: self.publisher.map(|publisher| Pending {
                publisher,
                events: Vec::new(),
                failure: None,
            }),
            metrics: Vec::new(),
            metric_gap: None,
        };
        let candidates = match self.candidates {
            Ok(candidates) => candidates,
            Err(failure) => {
                if let Some(lifecycle) = &mut pending.lifecycle {
                    lifecycle.failure = lifecycle
                        .publisher
                        .capture()
                        .map(|ticket| (ticket, failure));
                }
                pending.metric_gap = self.metrics.then_some(failure);
                return pending;
            }
        };
        // Missing lifecycle context loses that entire capture, as before. The
        // independent local-duration population remains usable.
        let lifecycle_complete = candidates
            .values()
            .all(|candidate| candidate.member.is_some() && candidate.admitted_at.is_some());
        if !lifecycle_complete && let Some(lifecycle) = &mut pending.lifecycle {
            lifecycle.failure = lifecycle
                .publisher
                .capture()
                .map(|ticket| (ticket, CaptureFailure::MissingContext));
        }
        state.ledger.read(|ledger| {
            for (hash, candidate) in candidates {
                if state.by_hash.contains(&hash) || state.in_flight.contains_key(&hash) {
                    continue;
                }
                // Ordinary release cannot resolve an awaiting canonical owner.
                // Submitted users must have reached the existing local terminal
                // state before this diagnostic local-policy claim is available.
                if let Some(user) = ledger.users.get(&hash)
                    && (!matches!(
                        user.location,
                        UserLocation::Terminal {
                            canonical: None,
                            ..
                        }
                    ) || candidate
                        .owner
                        .is_none_or(|owner| owner.generation != user.owner))
                {
                    continue;
                }
                if self.metrics {
                    pending.metrics.push(TerminalObservation {
                        direction: match candidate.direction {
                            Direction::Inbound => schema::Direction::Inbound,
                            Direction::Outbound => schema::Direction::Outbound,
                        },
                        outcome: candidate.outcome,
                        elapsed: candidate.metric_admitted_at.map(|at| at.elapsed()),
                    });
                }
                if lifecycle_complete
                    && let Some(lifecycle) = &mut pending.lifecycle
                    && let (Some(member), Some(admitted_at)) =
                        (candidate.member, candidate.admitted_at)
                    && let Some(ticket) = lifecycle.publisher.capture()
                {
                    let attempt = candidate.owner.map(|owner| schema::AttemptId {
                        epoch: lifecycle.publisher.epoch(),
                        generation: owner.generation,
                        post_batch_hash: schema::Hash32(owner.post_batch_hash.0),
                    });
                    lifecycle.events.push((
                        ticket,
                        Event::Terminal {
                            member,
                            attempt,
                            outcome: candidate.outcome,
                            assurance: schema::Assurance::LocalPolicy,
                            anchor: None,
                            elapsed_ns: admitted_at
                                .elapsed()
                                .as_nanos()
                                .try_into()
                                .unwrap_or(u64::MAX),
                            reconciliation: None,
                        },
                    ));
                }
            }
        });
        pending
    }
}
