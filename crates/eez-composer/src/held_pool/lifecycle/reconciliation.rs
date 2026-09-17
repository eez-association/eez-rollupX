//! Snapshot actual ownership changes only after the existing ledger commits.
//! Receipt inclusions are never replaced by the reconciliation scan's watermark.
use super::{HeldPool, Lifecycle, MAX_MEMBERS, Pending};
use crate::optimistic::{AttemptId, AttemptState, SourceOutcome, TerminalReason, UserLocation};
use alloy_primitives::B256;
use eez_l1::{
    L1BlockRef, L2Endpoint, ReconciledBatchSettlement,
    submitter::{CanonicalSourceReceipt, CanonicalSourceState},
};
use eez_telemetry::{
    CaptureFailure,
    schema::{self, Event, Hash32},
};

const MAX_CONSUMED: usize = 32768;

#[derive(Debug, Clone, Copy)]
pub(crate) struct SourceObservation {
    pub nonce: u64,
    pub block: Option<schema::BlockRef>,
    pub basis: schema::SourceBasis,
}

/// Borrow observations already used by native reconciliation. No additional RPC
/// or provider reads are introduced to make telemetry available.
#[derive(Clone, Copy)]
pub(crate) struct ReconciliationFacts<'a> {
    pub endpoint: L2Endpoint,
    pub post: Option<&'a CanonicalSourceReceipt>,
    pub applied: Option<&'a ReconciledBatchSettlement>,
    pub inbound: &'a [Option<CanonicalSourceState>],
    pub sources: Option<&'a [SourceObservation]>,
}

struct Before {
    users: Vec<(B256, u64, UserLocation)>,
}

fn block(value: L1BlockRef) -> schema::BlockRef {
    schema::BlockRef {
        number: value.number,
        hash: Hash32(value.hash.0),
    }
}

fn endpoint(value: L2Endpoint) -> schema::BlockRef {
    schema::BlockRef {
        number: value.number,
        hash: Hash32(value.hash.0),
    }
}

fn receipt(value: &CanonicalSourceReceipt) -> schema::ReceiptRef {
    schema::ReceiptRef {
        block: block(value.block),
        transaction_index: value.transaction_index,
        success: value.success,
    }
}

pub(super) fn outcome(value: TerminalReason) -> schema::Outcome {
    match value {
        TerminalReason::Committed => schema::Outcome::Committed,
        TerminalReason::SourceReverted => schema::Outcome::SourceReverted,
        TerminalReason::SourceCompletedWithMissingEffects => {
            schema::Outcome::SourceCompletedWithMissingEffects
        }
        TerminalReason::SourceCompleted => schema::Outcome::SourceCompleted,
        TerminalReason::SourceNonceSpent => schema::Outcome::SourceNonceSpent,
        TerminalReason::AttemptsExhausted => schema::Outcome::AttemptsExhausted,
        TerminalReason::NonceGap => schema::Outcome::NonceGap,
        TerminalReason::PoolDiscarded => schema::Outcome::PoolDiscarded,
    }
}

impl Before {
    fn capture(ledger: &AttemptState, id: AttemptId) -> Option<Result<Self, CaptureFailure>> {
        ledger
            .attempts
            .get(&id.generation)
            .filter(|attempt| attempt.id == id && attempt.reconciled_at.is_none())?;
        let users = ledger
            .users
            .iter()
            .filter(|(_, user)| {
                (user.owner == id.generation && user.location == UserLocation::AwaitingEvidence)
                    || matches!(
                        user.location,
                        UserLocation::Queued | UserLocation::Composing
                    )
            })
            .take(MAX_MEMBERS + 1)
            .map(|(hash, user)| (*hash, user.owner, user.location.clone()))
            .collect::<Vec<_>>();
        Some(if users.len() > MAX_MEMBERS {
            Err(CaptureFailure::Limit)
        } else {
            Ok(Self { users })
        })
    }
}

impl Lifecycle {
    fn chain_scopes(&self) -> (schema::ChainScope, schema::ChainScope) {
        (
            schema::ChainScope {
                chain_id: self.context.l1.chain_id,
                genesis_hash: Hash32(self.context.l1.genesis_hash.0),
                rollup_id: 0,
            },
            schema::ChainScope {
                chain_id: self.context.l2.chain_id,
                genesis_hash: Hash32(self.context.l2.genesis_hash.0),
                rollup_id: self.context.rollup_id,
            },
        )
    }

    fn reconciliation_details(
        &self,
        ledger: &AttemptState,
        id: AttemptId,
        before: Before,
        facts: Option<ReconciliationFacts<'_>>,
    ) -> Result<schema::ReconciliationDetails, CaptureFailure> {
        let attempt = ledger
            .attempts
            .get(&id.generation)
            .filter(|attempt| attempt.id == id)
            .ok_or(CaptureFailure::MissingContext)?;
        if attempt.txs.len() > MAX_MEMBERS {
            return Err(CaptureFailure::Limit);
        }
        let mut sources = Vec::new();
        if let Some(facts) = facts {
            let observations = facts.sources.ok_or(CaptureFailure::Limit)?;
            if observations.len() != attempt.txs.len() || facts.inbound.len() != attempt.txs.len() {
                return Err(CaptureFailure::MissingContext);
            }
            let consumed = facts
                .inbound
                .iter()
                .filter_map(|state| state.as_ref()?.receipt.as_ref())
                .try_fold(0usize, |count, receipt| {
                    count.checked_add(receipt.consumed.len())
                });
            if consumed.is_none_or(|count| count > MAX_CONSUMED) {
                return Err(CaptureFailure::Limit);
            }
            for ((tx, observed), inbound) in attempt.txs.iter().zip(observations).zip(facts.inbound)
            {
                let source_receipt = inbound.as_ref().and_then(|state| state.receipt.as_ref());
                sources.push(schema::ReconciledSource {
                    transaction_hash: Hash32(tx.hash.0),
                    direction: self.context.source(tx.hash, tx.direction).direction,
                    observed_nonce: observed.nonce,
                    source_block: observed.block,
                    basis: observed.basis,
                    receipt: source_receipt.map(receipt),
                    consumed: source_receipt.map_or_else(Vec::new, |receipt| {
                        receipt
                            .consumed
                            .iter()
                            .map(|(hash, index)| schema::SourceAction {
                                call_hash: Hash32(hash.0),
                                source_index: *index as u64,
                            })
                            .collect()
                    }),
                });
            }
        }
        let mut resolutions = Vec::new();
        for (hash, owner, previous) in before.users {
            let user = ledger
                .users
                .get(&hash)
                .filter(|user| user.owner == owner)
                .ok_or(CaptureFailure::MissingContext)?;
            if user.location == previous {
                continue;
            }
            let admission = self
                .admissions
                .get(&hash)
                .ok_or(CaptureFailure::MissingContext)?;
            let disposition = match user.location {
                UserLocation::Queued => schema::ResolutionDisposition::Requeued {
                    charged_attempts: user.tx.attempts,
                },
                UserLocation::Terminal { reason, canonical } => {
                    schema::ResolutionDisposition::Terminal {
                        outcome: outcome(reason),
                        assurance: if canonical.is_none() {
                            schema::Assurance::LocalPolicy
                        } else if facts.is_some() {
                            schema::Assurance::NodeReconciled
                        } else {
                            schema::Assurance::Unknown
                        },
                    }
                }
                _ => return Err(CaptureFailure::MissingContext),
            };
            let owner = ledger
                .attempts
                .get(&owner)
                .ok_or(CaptureFailure::MissingContext)?
                .id;
            resolutions.push(schema::SourceResolution {
                member: admission.member,
                owner: self.attempt_id(owner),
                disposition,
                elapsed_ns: admission
                    .admitted_at
                    .elapsed()
                    .as_nanos()
                    .try_into()
                    .unwrap_or(u64::MAX),
            });
        }
        Ok(schema::ReconciliationDetails {
            l1: self.chain_scopes().0,
            l2: self.chain_scopes().1,
            l2_snapshot: facts.map(|facts| endpoint(facts.endpoint)),
            post: facts.and_then(|facts| facts.post).map(receipt),
            applied: facts.and_then(|facts| facts.applied).map(|applied| {
                schema::AppliedSettlement {
                    block: block(applied.l1_block),
                    transaction_hash: Hash32(applied.tx_hash.0),
                    transaction_index: applied.tx_index,
                    calldata_hash: Hash32(applied.calldata_hash.0),
                    effects_start: applied.applied_effects.start as u64,
                    effects_end: applied.applied_effects.end as u64,
                    anchor_applied: applied.anchor_applied,
                    endpoint: applied.endpoint.map(endpoint),
                    outbound_groups_start: applied.selection.outbound_groups.start as u64,
                    outbound_groups_end: applied.selection.outbound_groups.end as u64,
                    inbound_effects_start: applied.selection.inbound_effects.start as u64,
                    inbound_effects_end: applied.selection.inbound_effects.end as u64,
                }
            }),
            sources,
            resolutions,
        })
    }

    fn reconciled(
        &self,
        ledger: &AttemptState,
        id: AttemptId,
        at: L1BlockRef,
        before: Result<Before, CaptureFailure>,
        facts: Option<ReconciliationFacts<'_>>,
    ) -> Option<Pending> {
        let ticket = self.publisher.capture()?;
        let reconciliation = ticket.reconciliation_id();
        let details = match before
            .and_then(|before| self.reconciliation_details(ledger, id, before, facts))
        {
            Ok(details) => details,
            Err(failure) => return Some(self.pending(ticket, Err(failure))),
        };
        let terminals = details
            .resolutions
            .iter()
            .filter(|resolution| {
                matches!(
                    resolution.disposition,
                    schema::ResolutionDisposition::Terminal { .. }
                )
            })
            .copied()
            .collect::<Vec<_>>();
        let mut pending = self.pending(
            ticket,
            Ok(Event::Reconciled {
                attempt: self.attempt_id(id),
                anchor: block(at),
                details: Some(Box::new(details)),
            }),
        );
        for resolution in terminals {
            if let schema::ResolutionDisposition::Terminal { outcome, assurance } =
                resolution.disposition
                && let Some(ticket) = self.publisher.capture()
            {
                pending.events.push((
                    ticket,
                    Event::Terminal {
                        member: resolution.member,
                        attempt: Some(resolution.owner),
                        outcome,
                        assurance,
                        // The checked scan is not a receipt inclusion. Exact references
                        // live in the linked reconciliation and remain independently checked.
                        anchor: None,
                        elapsed_ns: resolution.elapsed_ns,
                        reconciliation: Some(reconciliation),
                    },
                ));
            }
        }
        Some(pending)
    }
}

impl HeldPool {
    /// Keep height-only invalidation compatible; without an observed ancestor
    /// hash it cannot publish an exact reorg anchor.
    pub(in crate::held_pool) fn invalidate_after_l1_capture(
        &self,
        ancestor: u64,
        capture: impl FnOnce(&Lifecycle) -> Option<Pending>,
    ) -> Result<(), String> {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        state.ledger.invalidate_after_l1(ancestor)?;
        state.telemetry.invalidate_after_l1(ancestor);
        if let Some(lifecycle) = &mut state.lifecycle {
            lifecycle.invalidate_readiness();
        }
        state.apply_resolution(&[]);
        let pending = state.lifecycle.as_ref().and_then(capture);
        drop(state);
        if let Some(pending) = pending {
            pending.publish();
        }
        Ok(())
    }

    pub(crate) fn observe_l1_reorg(
        &self,
        ancestor: L1BlockRef,
        old_head_hash: B256,
        new_head: L1BlockRef,
    ) -> Result<(), String> {
        self.invalidate_after_l1_capture(ancestor.number, |lifecycle| {
            let ticket = lifecycle.publisher.capture()?;
            let (l1, l2) = lifecycle.chain_scopes();
            Some(lifecycle.pending(
                ticket,
                Ok(Event::Reorg {
                    ancestor: block(ancestor),
                    details: Some(schema::ReorgDetails {
                        l1,
                        l2,
                        old_head_hash: Hash32(old_head_hash.0),
                        new_head: block(new_head),
                    }),
                }),
            ))
        })
    }

    pub(crate) fn capture_source_observations(
        &self,
        count: usize,
    ) -> Option<Vec<SourceObservation>> {
        let state = self.state.lock().expect("held_pool mutex poisoned");
        (state.lifecycle.is_some() && count <= MAX_MEMBERS).then(|| Vec::with_capacity(count))
    }

    pub(crate) fn resolve_attempt_with_evidence(
        &self,
        id: AttemptId,
        as_of: L1BlockRef,
        outcomes: &[SourceOutcome],
        charge_attempt: bool,
        attempt_limit: u32,
        facts: Option<ReconciliationFacts<'_>>,
        is_canonical: impl FnOnce() -> bool,
    ) -> Result<(), String> {
        let mut state = self.state.lock().expect("held_pool mutex poisoned");
        if !is_canonical() {
            return Err("canonical settlement changed before ownership transfer".into());
        }
        let before = (state.lifecycle.is_some() || state.telemetry.native_enabled())
            .then(|| state.ledger.read(|ledger| Before::capture(ledger, id)))
            .flatten();
        let gaps = state
            .ledger
            .resolve(id, as_of, outcomes, charge_attempt, attempt_limit)?;
        let local_capture = if gaps.is_empty() {
            None
        } else {
            super::LocalCapture::before_unsubmitted(
                &state,
                state
                    .txs
                    .iter()
                    .map(|tx| (tx.hash, tx.sender, tx.direction, tx.nonce))
                    .chain(
                        state
                            .in_flight
                            .iter()
                            .map(|(hash, (sender, direction, nonce))| {
                                (*hash, *sender, *direction, *nonce)
                            }),
                    )
                    .filter(|(_, sender, direction, nonce)| {
                        gaps.iter().any(|gap| {
                            gap.sender == *sender
                                && gap.direction == *direction
                                && *nonce >= gap.nonce
                        })
                    })
                    .map(|(hash, _, direction, _)| (hash, schema::Outcome::NonceGap, direction)),
            )
        };
        // Retention may prune clocks, but apply_resolution can itself establish
        // local terminal ownership. Save bounded timing inputs now and select
        // the actual terminal transitions only after that operation completes.
        let timing_before = if state.telemetry.native_enabled() {
            before
                .as_ref()
                .and_then(|before| before.as_ref().ok())
                .map(|before| {
                    before
                        .users
                        .iter()
                        .map(|(hash, _, _)| {
                            (
                                state.telemetry.native_admission_started(*hash),
                                state.telemetry.native_partition_clock(*hash),
                            )
                        })
                        .collect::<Vec<_>>()
                })
        } else {
            None
        };
        state.apply_resolution(&gaps);
        let terminal_at = timing_before.as_ref().map(|_| std::time::Instant::now());
        let mut terminal_metrics = Vec::new();
        let metric_gap = state.telemetry.native_enabled() && matches!(&before, Some(Err(_)));
        if let (Some(Ok(before)), Some(timing_before), Some(terminal_at)) =
            (&before, timing_before, terminal_at)
        {
            state.ledger.read(|ledger| {
                for ((hash, owner, previous), (origin, clock)) in
                    before.users.iter().zip(timing_before)
                {
                    if let Some(user) = ledger.users.get(hash)
                        && user.owner == *owner
                        && user.location != *previous
                        && let UserLocation::Terminal { reason, .. } = user.location
                        && let Some(mut observation) = state.telemetry.native_terminal_at(
                            *hash,
                            user.tx.direction,
                            outcome(reason),
                            terminal_at,
                        )
                    {
                        observation.elapsed =
                            origin.and_then(|at| terminal_at.checked_duration_since(at));
                        observation.readiness =
                            origin.and_then(|at| clock?.finish(at, terminal_at));
                        terminal_metrics.push(observation);
                    }
                }
            });
        }
        let pending = before.and_then(|before| {
            state.lifecycle.as_ref().and_then(|lifecycle| {
                state
                    .ledger
                    .read(|ledger| lifecycle.reconciled(ledger, id, as_of, before, facts))
            })
        });
        let local_pending = local_capture.map(|capture| capture.after(&state));
        let ledger = std::sync::Arc::clone(&state.ledger);
        state.telemetry.observe_completion(&ledger);
        drop(state);
        for observation in terminal_metrics {
            observation.publish();
        }
        if metric_gap {
            eez_telemetry::operational::terminal_capture_gap(CaptureFailure::Limit);
        }
        if let Some(pending) = pending {
            pending.publish();
        }
        if let Some(pending) = local_pending {
            pending.publish();
        }
        Ok(())
    }
}
