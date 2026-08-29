//! Continuity checks for Composer-claimed root-update chains.

use std::num::NonZeroU64;

use alloy_primitives::B256;
use eez_protocol::abi::{ExecutionEntrySol, RootUpdateSol};
use thiserror::Error;

use super::post_batch::CanonicalPostBatch;

#[derive(Clone, Copy)]
struct VerifiedRootUpdateEntry<'batch> {
    claimed_entry: &'batch ExecutionEntrySol,
    claimed_update: &'batch RootUpdateSol,
}

/// A nonempty, single-rollup root-update chain bound to validated endpoints.
///
/// Private fields make construction exclusive to [`verify_root_update_chain`].
/// Downstream effect binding can therefore use each retained entry/update pair
/// without repeating the nonempty and exactly-one-update checks.
pub(crate) struct VerifiedRootUpdateChain<'batch> {
    expected_rollup: u64,
    leading: VerifiedRootUpdateEntry<'batch>,
    trailing: Vec<VerifiedRootUpdateEntry<'batch>>,
}

impl<'batch> VerifiedRootUpdateChain<'batch> {
    pub(super) const fn expected_rollup(&self) -> u64 {
        self.expected_rollup
    }

    pub(super) const fn leading(&self) -> (&'batch ExecutionEntrySol, &'batch RootUpdateSol) {
        (self.leading.claimed_entry, self.leading.claimed_update)
    }

    pub(super) fn trailing(
        &self,
    ) -> impl ExactSizeIterator<Item = (usize, &'batch ExecutionEntrySol, &'batch RootUpdateSol)> + '_
    {
        self.trailing
            .iter()
            .enumerate()
            .map(|(index, entry)| (index + 1, entry.claimed_entry, entry.claimed_update))
    }

    pub(super) fn trailing_len(&self) -> usize {
        self.trailing.len()
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum RootUpdateChainError {
    #[error("batch has no execution entries")]
    NoEntries,
    #[error("batch entry {entry_index} has {actual} root updates; expected exactly one")]
    UpdateCount { entry_index: usize, actual: usize },
    #[error("batch claims rollup id {claimed}; expected rollup id {expected}")]
    ExpectedRollupMismatch { expected: u64, claimed: u64 },
    #[error(
        "batch entry {entry_index} claims rollup id {claimed}; expected common rollup id {expected}"
    )]
    RollupMismatch {
        entry_index: usize,
        expected: u64,
        claimed: u64,
    },
    #[error(
        "leading root update claims initial root {claimed}; validated window root is {validated}"
    )]
    InitialRootMismatch { validated: B256, claimed: B256 },
    #[error(
        "root-update chain breaks before entry {entry_index}: previous claimed post-state is {previous_claimed_post_state}, next claimed pre-state is {next_claimed_pre_state}"
    )]
    ChainBreak {
        entry_index: usize,
        previous_claimed_post_state: B256,
        next_claimed_pre_state: B256,
    },
    #[error("root-update chain claims final root {claimed}; validated final root is {validated}")]
    FinalMismatch { validated: B256, claimed: B256 },
    #[error(
        "batch static entry {entry_index} pins root {claimed}; validated final root is {validated}"
    )]
    StaticRootMismatch {
        entry_index: usize,
        validated: B256,
        claimed: B256,
    },
}

/// Require one claimed root update per entry, the expected rollup, and a
/// continuous root chain between the locally validated window endpoints.
///
/// This checks continuity of Composer claims; `bind_effects_to_execution`
/// separately proves that the leading entry is an anchor and binds interior
/// roots to transaction checkpoints.
pub(crate) fn verify_root_update_chain(
    batch: &CanonicalPostBatch,
    expected_rollup_id: NonZeroU64,
    validated_window_pre_state_root: B256,
    validated_window_post_state_root: B256,
) -> Result<VerifiedRootUpdateChain<'_>, RootUpdateChainError> {
    let submitted_batch = batch.as_batch();
    let (leading_claimed_entry, trailing_claimed_entries) =
        submitted_batch
            .entries
            .split_first()
            .ok_or(RootUpdateChainError::NoEntries)?;
    let leading_claimed_update = sole_update(leading_claimed_entry.rootUpdates.as_slice(), 0)?;
    let expected_rollup = expected_rollup_id.get();
    if leading_claimed_update.rollupId != expected_rollup {
        return Err(RootUpdateChainError::ExpectedRollupMismatch {
            expected: expected_rollup,
            claimed: leading_claimed_update.rollupId,
        });
    }
    if leading_claimed_update.currentRoot != validated_window_pre_state_root {
        return Err(RootUpdateChainError::InitialRootMismatch {
            validated: validated_window_pre_state_root,
            claimed: leading_claimed_update.currentRoot,
        });
    }

    let claimed_rollup = leading_claimed_update.rollupId;
    let mut previous_claimed_post_state = leading_claimed_update.newRoot;
    let mut verified_trailing = Vec::with_capacity(trailing_claimed_entries.len());
    for (entry_index, entry) in trailing_claimed_entries.iter().enumerate() {
        let entry_index = entry_index + 1;
        let claimed_update = sole_update(entry.rootUpdates.as_slice(), entry_index)?;
        if claimed_update.rollupId != claimed_rollup {
            return Err(RootUpdateChainError::RollupMismatch {
                entry_index,
                expected: claimed_rollup,
                claimed: claimed_update.rollupId,
            });
        }
        if claimed_update.currentRoot != previous_claimed_post_state {
            return Err(RootUpdateChainError::ChainBreak {
                entry_index,
                previous_claimed_post_state,
                next_claimed_pre_state: claimed_update.currentRoot,
            });
        }
        previous_claimed_post_state = claimed_update.newRoot;
        verified_trailing.push(VerifiedRootUpdateEntry {
            claimed_entry: entry,
            claimed_update,
        });
    }

    if previous_claimed_post_state != validated_window_post_state_root {
        return Err(RootUpdateChainError::FinalMismatch {
            validated: validated_window_post_state_root,
            claimed: previous_claimed_post_state,
        });
    }

    for (entry_index, entry) in submitted_batch.staticEntries.iter().enumerate() {
        let [pin] = entry.expectedRoots.as_slice() else {
            return Err(RootUpdateChainError::StaticRootMismatch {
                entry_index,
                validated: validated_window_post_state_root,
                claimed: B256::ZERO,
            });
        };
        if pin.root != validated_window_post_state_root {
            return Err(RootUpdateChainError::StaticRootMismatch {
                entry_index,
                validated: validated_window_post_state_root,
                claimed: pin.root,
            });
        }
    }

    Ok(VerifiedRootUpdateChain {
        expected_rollup,
        leading: VerifiedRootUpdateEntry {
            claimed_entry: leading_claimed_entry,
            claimed_update: leading_claimed_update,
        },
        trailing: verified_trailing,
    })
}

fn sole_update(
    root_updates: &[RootUpdateSol],
    entry_index: usize,
) -> Result<&RootUpdateSol, RootUpdateChainError> {
    let [update] = root_updates else {
        return Err(RootUpdateChainError::UpdateCount {
            entry_index,
            actual: root_updates.len(),
        });
    };
    Ok(update)
}
