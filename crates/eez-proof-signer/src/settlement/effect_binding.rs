//! Binding Composer-claimed effects to locally validated execution evidence.

use alloy_primitives::{B256, I256};
use eez_protocol::abi::{ExecutionEntrySol, RootUpdateSol};
use eez_protocol::system_tx::outbound_settlement_source_counts;
use thiserror::Error;

use super::blocks::SettlingBlockObservations;
use super::state_chain::VerifiedRootUpdateChain;
use crate::validate::TransactionStateCheckpoint;

/// Shape classification of one claimed batch entry; every unsupported shape
/// is `Invalid` and fails closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimedEntryShape {
    Anchor,
    Outbound,
    Inbound,
    Lookup,
    Invalid,
}

/// Effect kind inferred from locally recovered settling-block evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ObservedEffectKind {
    Outbound,
    Inbound,
}

/// Effect kind after the anchor/invalid shapes have been rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EffectKind {
    Outbound,
    Inbound,
}

impl EffectKind {
    const fn claimed_shape(self) -> ClaimedEntryShape {
        match self {
            Self::Outbound => ClaimedEntryShape::Outbound,
            Self::Inbound => ClaimedEntryShape::Inbound,
        }
    }
}

/// Exact correspondence established while validating the effect prefix.
/// Private fields keep sibling modules from manufacturing a binding without
/// passing through [`bind_effects_to_execution`].
#[derive(Clone, Copy)]
pub(super) struct BoundEffect<'batch> {
    entry_index: usize,
    start_transaction_index: usize,
    user_transaction_index: usize,
    transaction_index: usize,
    source_entry_start: usize,
    source_entry_count: usize,
    kind: EffectKind,
    claimed_entry: &'batch ExecutionEntrySol,
    claimed_root_update: &'batch RootUpdateSol,
    recomputed_checkpoint: B256,
}

impl<'batch> BoundEffect<'batch> {
    pub(super) const fn entry_index(&self) -> usize {
        self.entry_index
    }

    pub(super) const fn transaction_index(&self) -> usize {
        self.transaction_index
    }

    pub(super) const fn start_transaction_index(&self) -> usize {
        self.start_transaction_index
    }

    pub(super) const fn user_transaction_index(&self) -> usize {
        self.user_transaction_index
    }

    pub(super) const fn source_entry_start(&self) -> usize {
        self.source_entry_start
    }

    pub(super) const fn source_entry_count(&self) -> usize {
        self.source_entry_count
    }

    pub(super) const fn kind(&self) -> EffectKind {
        self.kind
    }

    pub(super) const fn claimed_entry(&self) -> &'batch ExecutionEntrySol {
        self.claimed_entry
    }

    pub(super) const fn claimed_root_update(&self) -> &'batch RootUpdateSol {
        self.claimed_root_update
    }

    pub(super) const fn recomputed_checkpoint(&self) -> B256 {
        self.recomputed_checkpoint
    }
}

/// Ordered correspondence between claimed batch effects and settling-block
/// candidate positions.
/// Construction remains local so this type can serve as validation evidence.
pub(crate) struct BoundEffectSequence<'batch, 'settling> {
    effects: Vec<BoundEffect<'batch>>,
    settling_observations: &'settling SettlingBlockObservations,
}

impl<'batch, 'settling> BoundEffectSequence<'batch, 'settling> {
    pub(super) fn effects(&self) -> &[BoundEffect<'batch>] {
        &self.effects
    }

    pub(super) const fn settling_observations(&self) -> &'settling SettlingBlockObservations {
        self.settling_observations
    }

    pub(crate) fn inbound_count(&self) -> usize {
        self.count(EffectKind::Inbound)
    }

    pub(crate) fn outbound_count(&self) -> usize {
        self.count(EffectKind::Outbound)
    }

    fn count(&self, kind: EffectKind) -> usize {
        self.effects
            .iter()
            .filter(|effect| effect.kind == kind)
            .count()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum EffectPrefixError {
    #[error("leading settlement entry is {actual:?}; expected a canonical anchor")]
    LeadingEntryNotAnchor { actual: ClaimedEntryShape },
    #[error("settlement entry {entry_index} has an invalid effect shape")]
    InvalidEntry { entry_index: usize },
    #[error("settlement entry {entry_index} is a root-only anchor outside the leading position")]
    LaterAnchor { entry_index: usize },
    #[error(
        "claimed settlement effects number {claimed}, but re-execution observed {observed} effect candidates"
    )]
    EffectCountMismatch { claimed: usize, observed: usize },
    #[error(
        "anchor claims block hash {claimed_anchor_block_hash}; expected carrier endpoint is {validated_pre_settling_block_hash}"
    )]
    AnchorRootMismatch {
        validated_pre_settling_block_hash: B256,
        claimed_anchor_block_hash: B256,
    },
    #[error(
        "settlement effects require {expected} transaction state checkpoints, but received {actual}"
    )]
    TransactionStateCheckpointCountMismatch { expected: usize, actual: usize },
    #[error(
        "transaction state checkpoint {checkpoint_index} targets transaction {actual}; expected effect-candidate transaction {expected}"
    )]
    TransactionStateCheckpointIndexMismatch {
        checkpoint_index: usize,
        expected: usize,
        actual: usize,
    },
    #[error(
        "settlement entry {entry_index} is {claimed:?}, but effect-candidate transaction {transaction_index} is observed as {observed:?}"
    )]
    EffectKindMismatch {
        entry_index: usize,
        transaction_index: usize,
        claimed: ClaimedEntryShape,
        observed: ObservedEffectKind,
    },
    #[error(
        "settlement entry {entry_index} claims block hash {claimed_block_hash}; effect candidate {transaction_index} requires carrier hash {recomputed_checkpoint}"
    )]
    EffectBlockHashMismatch {
        entry_index: usize,
        transaction_index: usize,
        recomputed_checkpoint: B256,
        claimed_block_hash: B256,
    },
    #[error("anchor ether delta is {claimed}; expected zero")]
    NonZeroAnchorEtherDelta { claimed: I256 },
    #[error(
        "settling-block transactions do not form the claimed canonical effect groups: {reason}"
    )]
    InvalidEffectTransactionLayout { reason: String },
}

#[derive(Clone, Copy)]
struct EffectPosition {
    start: usize,
    user: usize,
    end: usize,
    source_entry_start: usize,
    source_entry_count: usize,
}

fn derive_effect_positions(
    settling: &SettlingBlockObservations,
    outbound_count: usize,
    inbound_count: usize,
    outbound_entries: &[&ExecutionEntrySol],
) -> Result<Vec<EffectPosition>, EffectPrefixError> {
    let loads = settling.outbound_load_candidates();
    let incoming = settling.inbound_candidates();
    let flags = settling.system_sender_flags();
    let invalid = |reason: String| EffectPrefixError::InvalidEffectTransactionLayout { reason };
    if incoming.len() != inbound_count {
        return Err(EffectPrefixError::EffectCountMismatch {
            claimed: inbound_count,
            observed: incoming.len(),
        });
    }
    let group_shapes = loads
        .iter()
        .map(|load| {
            let entries = load
                .inspection
                .as_ref()
                .map_err(|error| {
                    invalid(format!(
                        "invalid load at {}: {error}",
                        load.transaction_index
                    ))
                })?
                .as_entries();
            Ok((
                entries.len(),
                entries.iter().any(|entry| {
                    matches!(
                        entry,
                        eez_protocol::system_tx::OutboundSourceEntry::Static(_)
                    )
                }),
            ))
        })
        .collect::<Result<Vec<_>, EffectPrefixError>>()?;
    let source_counts =
        outbound_settlement_source_counts(outbound_entries.iter().copied(), &group_shapes)
            .map_err(&invalid)?;
    let mut positions = Vec::with_capacity(outbound_count + inbound_count);
    let mut host_index = 0;
    let mut next_position = loads
        .first()
        .map(|load| load.transaction_index)
        .or_else(|| incoming.first().map(|entry| entry.transaction_index));
    for load in loads {
        let start = load.transaction_index;
        let user = start
            .checked_add(1)
            .ok_or_else(|| invalid("outbound position overflow".into()))?;
        if next_position != Some(start)
            || flags.get(start) != Some(&true)
            || flags.get(user) != Some(&false)
        {
            return Err(invalid(format!(
                "outbound load at {start} must precede its signed origin without gaps"
            )));
        }
        let entries = load
            .inspection
            .as_ref()
            .map_err(|error| invalid(format!("invalid load at {start}: {error}")))?
            .as_entries();
        if entries.is_empty() {
            return Err(invalid("outbound load has no source entries".into()));
        }
        let mut source_entry_start = 0;
        while source_entry_start < entries.len() {
            // Checkpoint partitioning uses the same complete-load boundaries
            // as composition and follower replay. Root attempts cannot count
            // slots: natural reverts restore the cursor for identical retries.
            // Exact table binding below still authorizes every loaded entry.
            let source_entry_count = *source_counts
                .get(host_index)
                .ok_or_else(|| invalid("missing outbound settlement host".into()))?;
            let end = source_entry_start
                .checked_add(source_entry_count)
                .filter(|end| source_entry_count != 0 && *end <= entries.len())
                .ok_or_else(|| invalid("outbound host exceeds its source load".into()))?;
            positions.push(EffectPosition {
                start,
                user,
                end: user,
                source_entry_start,
                source_entry_count,
            });
            source_entry_start = end;
            host_index += 1;
        }
        next_position = user.checked_add(1);
    }
    if host_index != outbound_count {
        return Err(EffectPrefixError::EffectCountMismatch {
            claimed: outbound_count,
            observed: host_index,
        });
    }
    for candidate in incoming {
        let index = candidate.transaction_index;
        if next_position != Some(index) || flags.get(index) != Some(&true) {
            return Err(invalid(format!(
                "inbound delivery at {index} is not contiguous with the native suffix"
            )));
        }
        positions.push(EffectPosition {
            start: index,
            user: index,
            end: index,
            source_entry_start: 0,
            source_entry_count: 1,
        });
        next_position = index.checked_add(1);
    }
    if let Some(end) = next_position {
        if end != flags.len() {
            return Err(invalid("transactions follow the native suffix".into()));
        }
    } else if !settling.effect_candidate_positions().is_empty() {
        return Err(EffectPrefixError::EffectCountMismatch {
            claimed: 0,
            observed: settling.effect_candidate_positions().len(),
        });
    }
    Ok(positions)
}

/// Bind each verified root-update-chain entry to its settling-block candidate and
/// canonical transaction-prefix block hash.
///
/// [`VerifiedRootUpdateChain`] already guarantees a nonempty batch with exactly
/// one continuous, expected-rollup update per entry. This gate adds effect-shape,
/// candidate, checkpoint, and root bindings. Outbound effects retain their
/// immediate carrier schedule. Applying inbound entries claim their exact
/// replayed checkpoint. Canonical lookup-only inbound entries are retained in
/// the root-update chain but authorize no effect or transaction checkpoint.
pub(crate) fn bind_effects_to_execution<'batch, 'settling>(
    verified_state_chain: &VerifiedRootUpdateChain<'batch>,
    validated_settling_pre_block_hash: B256,
    computed_transaction_state_checkpoints: &[TransactionStateCheckpoint],
    settling_observations: &'settling SettlingBlockObservations,
) -> Result<BoundEffectSequence<'batch, 'settling>, EffectPrefixError> {
    let (leading, anchor_update) = verified_state_chain.leading();
    let settled_rollup = verified_state_chain.expected_rollup();
    // Preserve the specific value-conservation error for an otherwise
    // canonical anchor. Root-only checkpoint entries elsewhere in the batch
    // are still classified with the strict zero-delta predicate below.
    if anchor_update.etherDelta != Default::default()
        && is_root_only_entry_except_ether_delta(leading, anchor_update, settled_rollup)
    {
        return Err(EffectPrefixError::NonZeroAnchorEtherDelta {
            claimed: I256::from(anchor_update.etherDelta),
        });
    }
    let leading_kind = classify_claimed_entry(leading, anchor_update, settled_rollup);
    if leading_kind != ClaimedEntryShape::Anchor {
        return Err(EffectPrefixError::LeadingEntryNotAnchor {
            actual: leading_kind,
        });
    }

    let mut claimed_effects = Vec::with_capacity(verified_state_chain.trailing_len());
    for (entry_index, entry, update) in verified_state_chain.trailing() {
        match classify_claimed_entry(entry, update, settled_rollup) {
            ClaimedEntryShape::Anchor => {
                return Err(EffectPrefixError::LaterAnchor { entry_index });
            }
            ClaimedEntryShape::Lookup => {}
            ClaimedEntryShape::Invalid => {
                return Err(EffectPrefixError::InvalidEntry { entry_index });
            }
            ClaimedEntryShape::Outbound => {
                claimed_effects.push((entry_index, EffectKind::Outbound, entry, update));
            }
            ClaimedEntryShape::Inbound => {
                claimed_effects.push((entry_index, EffectKind::Inbound, entry, update));
            }
        }
    }

    let claimed = claimed_effects.len();
    let outbound_count = claimed_effects
        .iter()
        .filter(|(_, kind, _, _)| *kind == EffectKind::Outbound)
        .count();
    let inbound_count = claimed - outbound_count;
    let outbound_entries = claimed_effects
        .iter()
        .filter(|(_, kind, _, _)| *kind == EffectKind::Outbound)
        .map(|(_, _, entry, _)| *entry)
        .collect::<Vec<_>>();
    let effect_positions = derive_effect_positions(
        settling_observations,
        outbound_count,
        inbound_count,
        &outbound_entries,
    )?;
    // A canonical anchor carries no cross-chain value. Check this only after
    // the leading Composer entry has actually been classified as an anchor.
    let anchor_ether_delta = I256::from(anchor_update.etherDelta);
    if anchor_ether_delta != I256::ZERO {
        return Err(EffectPrefixError::NonZeroAnchorEtherDelta {
            claimed: anchor_ether_delta,
        });
    }
    // With no effects the batch never represents a distinct pre-settling
    // commitment; the chain gate binds the anchor's `newRoot` directly to the
    // final block hash.
    if claimed == 0 {
        if !computed_transaction_state_checkpoints.is_empty() {
            return Err(EffectPrefixError::TransactionStateCheckpointCountMismatch {
                expected: 0,
                actual: computed_transaction_state_checkpoints.len(),
            });
        }
        return Ok(BoundEffectSequence {
            effects: Vec::new(),
            settling_observations,
        });
    }

    // Several roots in one signed origin share its single transaction checkpoint.
    // Require the exact distinct endpoint sequence; no unused checkpoint is accepted.
    let mut endpoints = effect_positions
        .iter()
        .map(|position| position.end)
        .collect::<Vec<_>>();
    endpoints.dedup();
    if endpoints.len() != computed_transaction_state_checkpoints.len() {
        return Err(EffectPrefixError::TransactionStateCheckpointCountMismatch {
            expected: endpoints.len(),
            actual: computed_transaction_state_checkpoints.len(),
        });
    }
    for (index, (&end, checkpoint)) in endpoints
        .iter()
        .zip(computed_transaction_state_checkpoints)
        .enumerate()
    {
        if checkpoint.transaction_index != end {
            return Err(EffectPrefixError::TransactionStateCheckpointIndexMismatch {
                checkpoint_index: index,
                expected: end,
                actual: checkpoint.transaction_index,
            });
        }
    }
    let matched_checkpoints = effect_positions
        .iter()
        .map(|position| {
            let index = endpoints
                .binary_search(&position.end)
                .expect("endpoint collected from these positions");
            &computed_transaction_state_checkpoints[index]
        })
        .collect::<Vec<_>>();

    let (expected_anchor_root, outbound_effect_roots) =
        eez_protocol::settlement::settlement_root_carriers(
            validated_settling_pre_block_hash,
            matched_checkpoints
                .iter()
                .zip(&claimed_effects)
                .take(outbound_count)
                .map(|(checkpoint, (_, _, entry, _))| (checkpoint.block_hash, entry.success)),
        );
    if anchor_update.newRoot != expected_anchor_root {
        return Err(EffectPrefixError::AnchorRootMismatch {
            validated_pre_settling_block_hash: expected_anchor_root,
            claimed_anchor_block_hash: anchor_update.newRoot,
        });
    }

    let mut bound_effects = Vec::with_capacity(claimed);
    // Candidate positions and checkpoints were joined above. Bind direction
    // and the independently derived carrier block hash in that same order.
    for (effect_index, (entry_index, kind, entry, update)) in
        claimed_effects.into_iter().enumerate()
    {
        let position = effect_positions[effect_index];
        let transaction_index = position.end;
        let claimed_kind = kind.claimed_shape();
        let observed_kind = if settling_observations.system_sender_flags()[position.user] {
            ObservedEffectKind::Inbound
        } else {
            ObservedEffectKind::Outbound
        };
        let observed_bound_kind = match observed_kind {
            ObservedEffectKind::Outbound => EffectKind::Outbound,
            ObservedEffectKind::Inbound => EffectKind::Inbound,
        };
        if kind != observed_bound_kind {
            return Err(EffectPrefixError::EffectKindMismatch {
                entry_index,
                transaction_index,
                claimed: claimed_kind,
                observed: observed_kind,
            });
        }

        let recomputed_checkpoint = if effect_index < outbound_count {
            outbound_effect_roots[effect_index]
        } else {
            matched_checkpoints[effect_index].block_hash
        };
        let claimed_block_hash = update.newRoot;
        if claimed_block_hash != recomputed_checkpoint {
            return Err(EffectPrefixError::EffectBlockHashMismatch {
                entry_index,
                transaction_index,
                recomputed_checkpoint,
                claimed_block_hash,
            });
        }
        bound_effects.push(BoundEffect {
            entry_index,
            start_transaction_index: position.start,
            user_transaction_index: position.user,
            transaction_index,
            source_entry_start: position.source_entry_start,
            source_entry_count: position.source_entry_count,
            kind,
            claimed_entry: entry,
            claimed_root_update: update,
            recomputed_checkpoint: matched_checkpoints[effect_index].block_hash,
        });
    }

    Ok(BoundEffectSequence {
        effects: bound_effects,
        settling_observations,
    })
}

/// Classification establishes the complete supported shape for each effect
/// kind; the per-kind gates bind that shape to locally recovered evidence.
fn classify_claimed_entry(
    entry: &ExecutionEntrySol,
    _update: &RootUpdateSol,
    settled_rollup: u64,
) -> ClaimedEntryShape {
    if eez_protocol::system_tx::check_entry_shape(entry, "settlement").is_err() {
        return ClaimedEntryShape::Invalid;
    }

    if eez_protocol::settlement::is_lookup_only_inbound_entry(entry, settled_rollup) {
        return ClaimedEntryShape::Lookup;
    }
    if entry.proxyEntryHash != B256::ZERO {
        if !entry.success {
            return ClaimedEntryShape::Invalid;
        }
        return ClaimedEntryShape::Inbound;
    }
    if !entry.l2ToL1Calls.is_empty() {
        return if entry.success {
            ClaimedEntryShape::Outbound
        } else {
            ClaimedEntryShape::Invalid
        };
    }

    if eez_protocol::settlement::is_root_only_entry(entry, settled_rollup) {
        ClaimedEntryShape::Anchor
    } else {
        ClaimedEntryShape::Invalid
    }
}

fn is_root_only_entry_except_ether_delta(
    entry: &ExecutionEntrySol,
    update: &RootUpdateSol,
    settled_rollup: u64,
) -> bool {
    let mut normalized = entry.clone();
    normalized.rootUpdates[0].etherDelta = Default::default();
    debug_assert_eq!(normalized.rootUpdates[0].currentRoot, update.currentRoot);
    eez_protocol::settlement::is_root_only_entry(&normalized, settled_rollup)
}
