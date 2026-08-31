//! Binding Composer-claimed effects to locally validated execution evidence.

use alloy_primitives::{B256, I256};
use eez_protocol::RollupId;
use eez_protocol::abi::{ExecutionEntrySol, RootUpdateSol};
use eez_protocol::entries::{
    NestedPairDirection, verify_nested_group, verify_static_outbound_pair,
};
use eez_protocol::rolling_hash::EntryRollingHash;
use eez_protocol::system_tx::OutboundSourceEntry;
use eez_protocol::system_tx::outbound_settlement_root_counts;
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
    callback_start_transaction_index: usize,
    kind: EffectKind,
    claimed_entry: &'batch ExecutionEntrySol,
    claimed_root_update: &'batch RootUpdateSol,
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

    pub(super) const fn callback_start_transaction_index(&self) -> usize {
        self.callback_start_transaction_index
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
    #[error("settlement entry {entry_index} is a second anchor")]
    LaterAnchor { entry_index: usize },
    #[error("settlement entry {entry_index} has an invalid effect shape")]
    InvalidEntry { entry_index: usize },
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
    callback_start: usize,
}

fn derive_effect_positions(
    settling: &SettlingBlockObservations,
    outbound_count: usize,
    inbound_count: usize,
    outbound_entries: &[&ExecutionEntrySol],
    settled_rollup: u64,
) -> Result<Vec<EffectPosition>, EffectPrefixError> {
    let loads = settling.outbound_load_candidates();
    let incoming = settling.inbound_candidates();
    let flags = settling.system_sender_flags();
    let claimed = outbound_count + inbound_count;
    if claimed == 0 {
        let observed = settling.effect_candidate_positions().len();
        return if observed == 0 {
            Ok(Vec::new())
        } else {
            Err(EffectPrefixError::EffectCountMismatch { claimed, observed })
        };
    }
    let loaded_source_count = loads
        .iter()
        .map(|load| {
            load.inspection
                .as_ref()
                .map(|entries| entries.as_entries().len())
        })
        .collect::<Result<Vec<_>, _>>()
        .ok()
        .map(|counts| counts.into_iter().sum::<usize>());
    let claimed_source_count = outbound_entries.iter().try_fold(
        0usize,
        |total, entry| -> Result<usize, EffectPrefixError> {
            let count = outbound_settlement_root_counts(std::slice::from_ref(*entry))
                .map_err(|reason| EffectPrefixError::InvalidEffectTransactionLayout { reason })?[0];
            total.checked_add(count).ok_or_else(|| {
                EffectPrefixError::InvalidEffectTransactionLayout {
                    reason: "outbound source-entry count overflows usize".into(),
                }
            })
        },
    )?;
    if loaded_source_count != Some(claimed_source_count) || incoming.len() < inbound_count {
        // Keep malformed selector-level candidates in the later direction
        // gates. Boolean framing is sufficient for the legacy two-tx shape;
        // complete decoded candidates are required only to disambiguate
        // grouped callbacks from standalone inbound effects.
        let legacy = settling.effect_candidate_positions();
        if legacy.len() != claimed {
            return Err(EffectPrefixError::EffectCountMismatch {
                claimed,
                observed: legacy.len(),
            });
        }
        return legacy
            .into_iter()
            .map(|end| {
                if flags.get(end).copied() == Some(true) {
                    Ok(EffectPosition {
                        start: end,
                        user: end,
                        end,
                        source_entry_start: 0,
                        source_entry_count: 1,
                        callback_start: end,
                    })
                } else {
                    let start = end.saturating_sub(1);
                    Ok(EffectPosition {
                        start,
                        user: end,
                        end,
                        source_entry_start: 0,
                        source_entry_count: 1,
                        callback_start: end,
                    })
                }
            })
            .collect();
    }
    let callback_count = incoming.len() - inbound_count;
    let (callbacks, standalone_inbound) = incoming.split_at(callback_count);
    let first_standalone = standalone_inbound
        .first()
        .map_or(flags.len(), |candidate| candidate.transaction_index);
    let mut callback_cursor = 0usize;
    let mut positions = Vec::with_capacity(outbound_count + inbound_count);

    let mut outbound_cursor = 0usize;
    for (index, load) in loads.iter().enumerate() {
        let start = load.transaction_index;
        let user = start.checked_add(1).ok_or_else(|| {
            EffectPrefixError::InvalidEffectTransactionLayout {
                reason: "outbound user transaction index overflows".into(),
            }
        })?;
        if flags.get(start).copied() != Some(true) || flags.get(user).copied() != Some(false) {
            return Err(EffectPrefixError::InvalidEffectTransactionLayout {
                reason: format!(
                    "outbound load at {start} is not immediately followed by a user transaction"
                ),
            });
        }
        let boundary = loads
            .get(index + 1)
            .map_or(first_standalone, |next| next.transaction_index);
        if boundary <= user {
            return Err(EffectPrefixError::InvalidEffectTransactionLayout {
                reason: format!("outbound group at {start} overlaps its next effect"),
            });
        }
        let entries = load
            .inspection
            .as_ref()
            .expect("decoded source count established above")
            .as_entries();
        let mut transaction_index = user + 1;
        let group_callback_end = callback_cursor + (boundary - (user + 1));
        let mut source_entry_start = 0usize;
        while source_entry_start < entries.len() {
            let claimed = *outbound_entries.get(outbound_cursor).ok_or_else(|| {
                EffectPrefixError::InvalidEffectTransactionLayout {
                    reason: "missing claimed host for outbound source roots".into(),
                }
            })?;
            let source_entry_count = outbound_settlement_root_counts(std::slice::from_ref(claimed))
                .map_err(|reason| EffectPrefixError::InvalidEffectTransactionLayout { reason })?[0];
            if source_entry_count == 0 || source_entry_start + source_entry_count > entries.len() {
                return Err(EffectPrefixError::InvalidEffectTransactionLayout {
                    reason: format!(
                        "claimed outbound host {outbound_cursor} covers {source_entry_count} roots at source offset {source_entry_start}, but the load carries {}",
                        entries.len(),
                    ),
                });
            }
            let callback_start = transaction_index;
            let remaining_callbacks = group_callback_end.saturating_sub(callback_cursor);
            let callback_count = if source_entry_start + source_entry_count == entries.len() {
                remaining_callbacks
            } else {
                (0..=remaining_callbacks)
                    .find(|&take| {
                        let callback_entries = callbacks[callback_cursor..callback_cursor + take]
                            .iter()
                            .map(|candidate| {
                                candidate.inspection.as_ref().ok().map(|observation| {
                                    observation.derived_da_entry.as_entry().clone()
                                })
                            })
                            .collect::<Option<Vec<_>>>();
                        let Some(callback_entries) = callback_entries else {
                            return false;
                        };
                        let source_entries = &entries
                            [source_entry_start..source_entry_start + source_entry_count];
                        match source_entries {
                            [OutboundSourceEntry::Static(entry)]
                                if callback_entries.is_empty() =>
                            {
                                verify_static_outbound_pair(
                                    claimed,
                                    entry,
                                    RollupId(settled_rollup),
                                )
                                .is_ok()
                            }
                            _ => source_entries
                                .iter()
                                .map(|entry| entry.as_mutable().cloned())
                                .collect::<Option<Vec<_>>>()
                                .map(|mut entries| {
                                    entries.extend(callback_entries);
                                    verify_nested_group(
                                        claimed,
                                        &entries,
                                        NestedPairDirection::Outbound,
                                        RollupId(settled_rollup),
                                    )
                                    .is_ok()
                                })
                                .unwrap_or(false),
                        }
                    })
                    .ok_or_else(|| EffectPrefixError::InvalidEffectTransactionLayout {
                        reason: format!(
                            "cannot partition callbacks for outbound settlement host {outbound_cursor}"
                        ),
                    })?
            };
            let mut endpoint = user;
            for _ in 0..callback_count {
                let Some(callback) = callbacks.get(callback_cursor) else {
                    return Err(EffectPrefixError::InvalidEffectTransactionLayout {
                        reason: format!(
                            "transaction {transaction_index} follows an outbound user but is not a callback delivery"
                        ),
                    });
                };
                if callback.transaction_index != transaction_index
                    || flags.get(transaction_index).copied() != Some(true)
                {
                    return Err(EffectPrefixError::InvalidEffectTransactionLayout {
                        reason: format!(
                            "outbound callback delivery is not canonical at transaction {transaction_index}"
                        ),
                    });
                }
                callback_cursor += 1;
                transaction_index += 1;
                endpoint = transaction_index - 1;
            }
            positions.push(EffectPosition {
                start,
                user,
                end: endpoint,
                source_entry_start,
                source_entry_count,
                callback_start,
            });
            source_entry_start += source_entry_count;
            outbound_cursor += 1;
        }
        if transaction_index != boundary {
            return Err(EffectPrefixError::InvalidEffectTransactionLayout {
                reason: format!(
                    "outbound group at {start} accounts for callbacks through {transaction_index}, expected boundary {boundary}"
                ),
            });
        }
    }
    if outbound_cursor != outbound_entries.len() {
        return Err(EffectPrefixError::InvalidEffectTransactionLayout {
            reason: format!(
                "{} claimed outbound settlement hosts remain after covering all loads",
                outbound_entries.len() - outbound_cursor,
            ),
        });
    }
    if callback_cursor != callbacks.len() {
        return Err(EffectPrefixError::InvalidEffectTransactionLayout {
            reason: "callback deliveries are not contained by an outbound group".into(),
        });
    }
    for candidate in standalone_inbound {
        let index = candidate.transaction_index;
        if flags.get(index).copied() != Some(true) {
            return Err(EffectPrefixError::InvalidEffectTransactionLayout {
                reason: format!("inbound effect transaction {index} is not a system transaction"),
            });
        }
        positions.push(EffectPosition {
            start: index,
            user: index,
            end: index,
            source_entry_start: 0,
            source_entry_count: 1,
            callback_start: index,
        });
    }
    Ok(positions)
}

/// Bind each verified root-update-chain entry to its settling-block candidate and
/// canonical transaction-prefix block hash.
///
/// [`VerifiedRootUpdateChain`] already guarantees a nonempty batch with exactly
/// one continuous, expected-rollup update per entry. This gate adds effect-shape,
/// candidate, checkpoint, and block-hash-carrier bindings. A successful entry carries
/// its checkpoint and those of any immediately following failed entries. The
/// anchor carries leading failed checkpoints, and failed entries claim no-op
/// no-op commitment updates because their L1 execution rolls updates back.
pub(crate) fn bind_effects_to_execution<'batch, 'settling>(
    verified_state_chain: &VerifiedRootUpdateChain<'batch>,
    validated_settling_pre_block_hash: B256,
    computed_transaction_state_checkpoints: &[TransactionStateCheckpoint],
    settling_observations: &'settling SettlingBlockObservations,
) -> Result<BoundEffectSequence<'batch, 'settling>, EffectPrefixError> {
    let (leading, anchor_update) = verified_state_chain.leading();
    let settled_rollup = verified_state_chain.expected_rollup();
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
        settled_rollup,
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

    let has_grouped_callbacks = effect_positions
        .iter()
        .any(|position| position.end != position.user)
        || effect_positions
            .windows(2)
            .any(|positions| positions[0].user == positions[1].user);
    if computed_transaction_state_checkpoints.len() != claimed && !has_grouped_callbacks {
        return Err(EffectPrefixError::TransactionStateCheckpointCountMismatch {
            expected: claimed,
            actual: computed_transaction_state_checkpoints.len(),
        });
    }
    let mut matched_checkpoints = Vec::with_capacity(claimed);
    for (effect_index, position) in effect_positions.iter().copied().enumerate() {
        let transaction_index = position.end;
        let checkpoint = if has_grouped_callbacks {
            computed_transaction_state_checkpoints
                .iter()
                .find(|checkpoint| checkpoint.transaction_index == transaction_index)
                .ok_or(EffectPrefixError::TransactionStateCheckpointCountMismatch {
                    expected: claimed,
                    actual: computed_transaction_state_checkpoints.len(),
                })?
        } else {
            let checkpoint = &computed_transaction_state_checkpoints[effect_index];
            if checkpoint.transaction_index != transaction_index {
                return Err(EffectPrefixError::TransactionStateCheckpointIndexMismatch {
                    checkpoint_index: effect_index,
                    expected: transaction_index,
                    actual: checkpoint.transaction_index,
                });
            }
            checkpoint
        };
        matched_checkpoints.push(checkpoint);
    }

    let (expected_anchor_root, expected_effect_roots) =
        eez_protocol::settlement::settlement_root_carriers(
            validated_settling_pre_block_hash,
            matched_checkpoints
                .iter()
                .zip(&claimed_effects)
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

        let recomputed_checkpoint = expected_effect_roots[effect_index];
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
            callback_start_transaction_index: position.callback_start,
            kind,
            claimed_entry: entry,
            claimed_root_update: update,
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
    update: &RootUpdateSol,
    settled_rollup: u64,
) -> ClaimedEntryShape {
    if eez_protocol::system_tx::check_entry_shape(entry, "settlement").is_err() {
        return ClaimedEntryShape::Invalid;
    }

    if entry.proxyEntryHash != B256::ZERO {
        return ClaimedEntryShape::Inbound;
    }
    if !entry.l2ToL1Calls.is_empty() {
        return if entry.success {
            ClaimedEntryShape::Outbound
        } else {
            ClaimedEntryShape::Invalid
        };
    }

    if !entry.expectedL1ToL2Calls.is_empty() {
        return ClaimedEntryShape::Invalid;
    }

    let expected_anchor_hash =
        EntryRollingHash::seed_for_l1([(update.rollupId, update.currentRoot)], B256::ZERO)
            .current();
    if entry.success
        && entry.destinationRollupId == settled_rollup
        && entry.returnData.is_empty()
        && entry.rollingHash == expected_anchor_hash
    {
        ClaimedEntryShape::Anchor
    } else {
        ClaimedEntryShape::Invalid
    }
}
