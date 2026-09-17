//! Capture follows the pool -> ledger lock order used by ownership transfer.
//! Failed ledger mutations emit nothing; publication happens after both locks.
use super::{HeldPool, Lifecycle, MAX_MEMBERS, Pending};
use crate::{
    HeldTx,
    optimistic::{AttemptId, AttemptRecord, SubmissionObservation},
};
use alloy_primitives::{B256, Bytes};
use eez_protocol::settlement_manifest::SettlementManifest;
use eez_telemetry::{
    CaptureFailure, Ticket,
    schema::{self, Event, Hash32},
};
use reth_primitives_traits::SealedHeader;

// Bound copies before allocation under the owner. Serialization still enforces
// the transport's independent 8 MiB limit. Exceeding either loses telemetry only.
const MAX_EFFECTS: usize = 32768;
const MAX_ACTIONS: usize = 32768;

/// Exact values already calculated by preparation, without decoding/copying the
/// signed envelope again. These describe a candidate, never purchased receipts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PreparationFacts {
    pub preparation: Option<schema::PreparationId>,
    pub encoded_bytes: usize,
    pub blob_count: usize,
    pub blob_allowance: usize,
    pub nonce: u64,
}

impl Lifecycle {
    fn attempt_id(&self, id: AttemptId) -> schema::AttemptId {
        schema::AttemptId {
            epoch: self.publisher.epoch(),
            generation: id.generation,
            post_batch_hash: Hash32(id.post_batch_hash.0),
        }
    }

    fn pending(&self, ticket: Ticket, event: Result<Event, CaptureFailure>) -> Pending {
        let (events, failure) = match event {
            Ok(event) => (vec![(ticket, event)], None),
            Err(reason) => (Vec::new(), Some((ticket, reason))),
        };
        Pending {
            publisher: self.publisher.clone(),
            events,
            failure,
        }
    }

    fn prepared(&self, attempt: &AttemptRecord, facts: PreparationFacts) -> Option<Pending> {
        let ticket = self.publisher.capture()?;
        Some(self.pending(ticket, self.prepared_event(attempt, facts)))
    }

    fn prepared_event(
        &self,
        attempt: &AttemptRecord,
        facts: PreparationFacts,
    ) -> Result<Event, CaptureFailure> {
        let manifest = &attempt.manifest;
        if manifest.users.len() > MAX_MEMBERS
            || manifest.effects.len() > MAX_EFFECTS
            || manifest
                .users
                .iter()
                .try_fold(0usize, |count, user| {
                    count.checked_add(user.source_actions.len())
                })
                .is_none_or(|count| count > MAX_ACTIONS)
        {
            return Err(CaptureFailure::Limit);
        }
        let blob_count = facts
            .blob_count
            .try_into()
            .map_err(|_| CaptureFailure::Limit)?;
        let blob_allowance = facts
            .blob_allowance
            .try_into()
            .map_err(|_| CaptureFailure::Limit)?;
        let members = self.members(&attempt.txs)?;
        let plans = manifest
            .users
            .iter()
            .map(|user| schema::SourcePlan {
                sender: schema::Address20(user.sender.0.0),
                nonce: user.nonce,
                charged_attempts: user.attempts,
                outbound_group: user.outbound_group.map(|index| index as u64),
                effects_start: user.effects.start as u64,
                effects_end: user.effects.end as u64,
                source_actions: user
                    .source_actions
                    .iter()
                    .map(|action| schema::SourceAction {
                        call_hash: Hash32(action.call_hash.0),
                        source_index: action.source_index as u64,
                    })
                    .collect(),
            })
            .collect();
        Ok(Event::AttemptPrepared {
            attempt: self.attempt_id(attempt.id),
            sync_height: attempt.sync_height,
            members,
            encoded_bytes: facts.encoded_bytes as u64,
            blob_count,
            blob_allowance,
            details: Some(schema::AttemptDetails {
                preparation: facts.preparation,
                l2_chain_id: self.context.l2.chain_id,
                l2_genesis_hash: Hash32(self.context.l2.genesis_hash.0),
                rollup_id: self.context.rollup_id,
                sync_hash: Hash32(manifest.sync_hash.0),
                nonce: facts.nonce,
                plans,
                effects: manifest.effects.iter().map(|hash| Hash32(hash.0)).collect(),
            }),
        })
    }

    fn observation(
        &self,
        id: AttemptId,
        observation: SubmissionObservation,
        nonce: u64,
    ) -> Option<Pending> {
        let ticket = self.publisher.capture()?;
        let attempt = self.attempt_id(id);
        let observation = match observation {
            SubmissionObservation::Dispatched { block, timestamp } => {
                return Some(self.pending(
                    ticket,
                    Ok(Event::Dispatched {
                        attempt,
                        target_block: block,
                        target_timestamp: timestamp,
                        transaction_hash: attempt.post_batch_hash,
                        nonce,
                    }),
                ));
            }
            SubmissionObservation::Prepared => schema::SubmissionObservation::Prepared,
            SubmissionObservation::Included { block } => schema::SubmissionObservation::Included {
                block: schema::BlockRef {
                    number: block.number,
                    hash: Hash32(block.hash.0),
                },
            },
            SubmissionObservation::Excluded {
                block,
                charge_attempt,
            } => schema::SubmissionObservation::Excluded {
                block,
                charge_attempt,
            },
            SubmissionObservation::Uncertain => schema::SubmissionObservation::Uncertain,
        };
        Some(self.pending(
            ticket,
            Ok(Event::SubmissionObserved {
                attempt,
                observation,
            }),
        ))
    }
}

impl HeldPool {
    /// Register using the existing ledger; snapshot exact membership while
    /// ingress/recovery cannot change the admissions associated with its users.
    pub(crate) fn begin_attempt(
        &self,
        sync_height: u64,
        post_batch_hash: B256,
        parent: SealedHeader<alloy_consensus::Header>,
        txs: Vec<HeldTx>,
        manifest: SettlementManifest,
        post_batch: Bytes,
        facts: PreparationFacts,
    ) -> Result<AttemptId, String> {
        let state = self.state.lock().expect("held_pool mutex poisoned");
        let id = state.ledger.begin(
            sync_height,
            post_batch_hash,
            parent,
            txs,
            manifest,
            post_batch,
        )?;
        let pending = state.lifecycle.as_ref().and_then(|lifecycle| {
            state.ledger.read(|ledger| {
                // A newly prepared unresolved attempt cannot be pruned by finality.
                ledger
                    .attempts
                    .get(&id.generation)
                    .and_then(|attempt| lifecycle.prepared(attempt, facts))
            })
        });
        drop(state);
        if let Some(pending) = pending {
            pending.publish();
        }
        Ok(id)
    }

    pub(crate) fn pending_attempt(&self) -> Option<AttemptRecord> {
        let state = self.state.lock().expect("held_pool mutex poisoned");
        state.ledger.pending_attempt()
    }

    pub(crate) fn replace_prepared_post_batch(
        &self,
        id: AttemptId,
        hash: B256,
        raw: Bytes,
    ) -> Result<AttemptId, String> {
        let state = self.state.lock().expect("held_pool mutex poisoned");
        let replacement = state.ledger.replace_prepared_post_batch(id, hash, raw)?;
        let pending = state
            .lifecycle
            .as_ref()
            .filter(|_| replacement != id)
            .and_then(|lifecycle| {
                let ticket = lifecycle.publisher.capture()?;
                Some(lifecycle.pending(
                    ticket,
                    Ok(Event::AttemptRepriced {
                        previous: lifecycle.attempt_id(id),
                        replacement: lifecycle.attempt_id(replacement),
                    }),
                ))
            });
        drop(state);
        if let Some(pending) = pending {
            pending.publish();
        }
        Ok(replacement)
    }

    pub(crate) fn observe_attempt(
        &self,
        id: AttemptId,
        observation: SubmissionObservation,
        nonce: u64,
    ) -> Result<bool, String> {
        let state = self.state.lock().expect("held_pool mutex poisoned");
        let accepted = state.ledger.observe(id, observation.clone())?;
        let pending = state
            .lifecycle
            .as_ref()
            .filter(|_| accepted)
            .and_then(|lifecycle| lifecycle.observation(id, observation, nonce));
        drop(state);
        if let Some(pending) = pending {
            pending.publish();
        }
        Ok(accepted)
    }
}
