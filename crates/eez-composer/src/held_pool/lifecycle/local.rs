//! Local disposal follows actual reservation removal. Retained canonical owners
//! remain unresolved even if an ordinary pool operation releases a reservation.
use super::{MAX_MEMBERS, Pending};
use crate::{
    held_pool::PoolState,
    optimistic::{AttemptId, UserLocation},
};
use alloy_primitives::B256;
use eez_telemetry::{
    CaptureFailure, Publisher,
    schema::{self, Event, Member, Outcome},
};
use std::{collections::BTreeMap, time::Instant};

struct Candidate {
    member: Member,
    owner: Option<AttemptId>,
    admitted_at: Instant,
    outcome: Outcome,
}

pub(in crate::held_pool) struct LocalCapture {
    publisher: Publisher,
    candidates: Result<BTreeMap<B256, Candidate>, CaptureFailure>,
}

impl LocalCapture {
    /// Copy only active candidates before retention can prune their admission.
    /// No ticket is allocated until the existing mutation finishes.
    pub(in crate::held_pool) fn before(
        state: &PoolState,
        candidates: impl IntoIterator<Item = (B256, Outcome)>,
    ) -> Option<Self> {
        Self::capture(state, candidates, false)
    }

    pub(in crate::held_pool) fn before_unsubmitted(
        state: &PoolState,
        candidates: impl IntoIterator<Item = (B256, Outcome)>,
    ) -> Option<Self> {
        Self::capture(state, candidates, true)
    }

    fn capture(
        state: &PoolState,
        candidates: impl IntoIterator<Item = (B256, Outcome)>,
        unsubmitted_only: bool,
    ) -> Option<Self> {
        let lifecycle = state.lifecycle.as_ref()?;
        let captured = state.ledger.read(|ledger| {
            let mut captured = BTreeMap::new();
            for (hash, outcome) in candidates {
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
                let admission = lifecycle
                    .admissions
                    .get(&hash)
                    .ok_or(CaptureFailure::MissingContext)?;
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
                    member: admission.member,
                    owner,
                    admitted_at: admission.admitted_at,
                    outcome,
                });
                if captured.len() > MAX_MEMBERS {
                    return Err(CaptureFailure::Limit);
                }
            }
            Ok(captured)
        });
        Some(Self {
            publisher: lifecycle.publisher.clone(),
            candidates: captured,
        })
    }

    pub(in crate::held_pool) fn after(self, state: &PoolState) -> Pending {
        let mut pending = Pending {
            publisher: self.publisher,
            events: Vec::new(),
            failure: None,
        };
        let candidates = match self.candidates {
            Ok(candidates) => candidates,
            Err(failure) => {
                pending.failure = pending.publisher.capture().map(|ticket| (ticket, failure));
                return pending;
            }
        };
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
                if let Some(ticket) = pending.publisher.capture() {
                    let attempt = candidate.owner.map(|owner| schema::AttemptId {
                        epoch: pending.publisher.epoch(),
                        generation: owner.generation,
                        post_batch_hash: schema::Hash32(owner.post_batch_hash.0),
                    });
                    pending.events.push((
                        ticket,
                        Event::Terminal {
                            member: candidate.member,
                            attempt,
                            outcome: candidate.outcome,
                            assurance: schema::Assurance::LocalPolicy,
                            anchor: None,
                            elapsed_ns: candidate
                                .admitted_at
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
