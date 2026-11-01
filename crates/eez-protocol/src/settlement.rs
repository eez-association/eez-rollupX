//! Settlement transaction-framing helpers shared by the composer and proof
//! signer.

use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_sol_types::SolCall as _;

use crate::abi::{EtherDeltaSol, ExecutionEntrySol};
use crate::{CallMode, Composition, EntryRollingHash, ExecutedAction, ExecutionOutcome};

/// Whether an entry is the canonical inert shape used for the leading anchor.
///
/// This shape can advance only the settled rollup's commitment. It cannot
/// transfer value, execute a call, return data, or smuggle an unrelated
/// rolling trace.
#[must_use]
pub fn is_root_only_entry(entry: &ExecutionEntrySol, settled_rollup: u64) -> bool {
    let [update] = entry.rootUpdates.as_slice() else {
        return false;
    };
    entry.proxyEntryHash == B256::ZERO
        && entry.l2ToL1Calls.is_empty()
        && entry.expectedL1ToL2Calls.is_empty()
        && entry.success
        && entry.returnData.is_empty()
        && entry.destinationRollupId == settled_rollup
        && update.rollupId == settled_rollup
        && update.etherDelta == EtherDeltaSol::ZERO
        && entry.rollingHash
            == EntryRollingHash::seed_for_l1([(settled_rollup, update.currentRoot)], B256::ZERO)
                .current()
}

/// Whether a deferred entry has the shape of a non-applying L1-origin lookup.
///
/// Callback tables may remain present even when the source frame rolls back.
/// This classification establishes no execution authority: native semantic
/// binding must reproduce the complete source table and destination observation
/// replay must validate the claimed outcome. Empty tables additionally require
/// the exact seed hash, since they contain no callbacks that could extend it.
#[must_use]
pub fn is_lookup_only_inbound_entry(entry: &ExecutionEntrySol, settled_rollup: u64) -> bool {
    let [update] = entry.rootUpdates.as_slice() else {
        return false;
    };
    let empty_trace = entry.l2ToL1Calls.is_empty() && entry.expectedL1ToL2Calls.is_empty();
    entry.proxyEntryHash != B256::ZERO
        && entry.destinationRollupId == settled_rollup
        && update.rollupId == settled_rollup
        && update.currentRoot == update.newRoot
        && update.etherDelta == EtherDeltaSol::ZERO
        && crate::system_tx::check_entry_shape(entry, "lookup").is_ok()
        && (!empty_trace
            || entry.rollingHash
                == EntryRollingHash::seed_for_l1(
                    [(settled_rollup, update.currentRoot)],
                    entry.proxyEntryHash,
                )
                .current())
}

/// Mark the L1-origin root actions whose destination execution may be applied
/// to L2.
///
/// A mutable root applies only when its target call succeeded and its source
/// frame was not later reverted. Static roots, failed target calls, and every
/// root covered by a root-level revert span are lookups: their result remains
/// in the semantic stream and source entry, but they authorize no L2 state
/// transition.
///
/// The input must be the validated contiguous preorder action stream used by
/// materialization. The returned vector is parallel to `actions` and is false
/// for nested actions.
///
/// # Errors
///
/// Returns an error if a root-level revert span overflows or exceeds the action
/// stream. Full forest-shape validation remains the materializer's job.
pub fn l1_applied_root_mask(actions: &[ExecutedAction]) -> Result<Vec<bool>, String> {
    let mut applied = vec![false; actions.len()];
    let mut reverted_until = 0usize;

    for (index, action) in actions.iter().enumerate() {
        if action.parent_index.is_some() {
            continue;
        }

        let own_revert_end = action
            .revert_spans
            .iter()
            .copied()
            .max()
            .map(|span| {
                index
                    .checked_add(span as usize)
                    .filter(|end| *end <= actions.len())
                    .ok_or_else(|| {
                        format!(
                            "root action {index} revert span {span} exceeds {} actions",
                            actions.len()
                        )
                    })
            })
            .transpose()?;
        let source_frame_reverted = index < reverted_until || own_revert_end.is_some();
        if let Some(end) = own_revert_end {
            reverted_until = reverted_until.max(end);
        }

        applied[index] = action.call_mode == CallMode::Mutable
            && action.outcome.is_success()
            && !source_frame_reverted;
    }

    Ok(applied)
}

/// One top-level source-chain action in the order in which its originating
/// transaction executes it.
///
/// Applying mutable actions have a corresponding settling-block checkpoint.
/// Lookup-only mutable and static actions do not create an L2 transaction;
/// they observe the commitment live at their exact FIFO position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FifoSourceAction {
    /// A mutable cross-chain call.
    Mutable {
        /// Whether destination execution is authorized to change L2 state.
        applies: bool,
    },
    /// A top-level static cross-chain read.
    Static,
}

/// One source-visible call result that does not have a canonical L2
/// transaction and must therefore be re-executed by the proof signer.
///
/// `pinned_block_hash` identifies the exact Sync-block prefix at which the
/// call was observed. The remaining fields are the complete synthetic EVM
/// input and the exact result committed by native blob semantics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FifoObservationClaim {
    /// Canonical closed-prefix block hash selecting the in-block transaction
    /// boundary observed before block-finalization cleanup. Replay uses the
    /// authenticated body and parent state, not the closed prefix's state root.
    pub pinned_block_hash: B256,
    /// Mutable lookup or static observation.
    pub call_mode: CallMode,
    /// Destination contract on this rollup.
    pub target_address: Address,
    /// Original L1 caller used to derive the destination proxy address.
    pub source_address: Address,
    /// Chain on which `source_address` made the intercepted call.
    pub source_rollup_id: crate::RollupId,
    /// Target calldata.
    pub data: Bytes,
    /// Native value for a mutable lookup. Static observations always carry zero.
    pub value: U256,
    /// Independently reproducible success bit.
    pub expected_success: bool,
    /// Exact return data on success or revert data on failure.
    pub expected_output: Bytes,
    /// Static callbacks the target makes to the source chain, in call order.
    /// Always empty for a mutable lookup.
    pub callbacks: Vec<FifoObservationCallback>,
    /// Shared-materializer delivery used only in discarded scratch execution.
    /// Required for mutable observations; absent for static observations.
    pub delivery: Option<FifoObservationDelivery>,
    /// Nested source rollback scopes beginning here, outermost first. Each
    /// length counts observation claims, including this one. Derived from
    /// native action spans; never accepted as an independent RPC input.
    pub rollback_spans: Vec<usize>,
}

/// Manager execution derived from bound native actions, never supplied over RPC.
/// Its successful delivery checks the trace; the root target outcome is captured
/// separately and compared with the semantic claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FifoObservationDelivery {
    /// Exact executeIncomingCrossChainCall encoding from the shared materializer.
    pub calldata: Bytes,
    /// Checked native funding for the complete table.
    pub value: U256,
}

/// One static callback a static observation makes to its source chain.
///
/// This rollup's witness cannot prove the callback's result. Replay answers
/// the callback with `output`, and the source chain enforces that answer:
/// it executes the callback when resolving the static entry and checks its
/// result against the entry's static rolling hash, which commits to `output`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FifoObservationCallback {
    /// Caller on this rollup, which is the callback's source.
    pub source_address: Address,
    /// Callback target on the source chain.
    pub target_address: Address,
    /// Callback calldata.
    pub data: Bytes,
    /// Success bit committed alongside the output by the static rolling hash.
    pub success: bool,
    /// Exact return data on success or revert data on failure.
    pub output: Bytes,
}

/// Recover the top-level source actions that must execute in FIFO order.
///
/// Native semantics retain the exact call forest. Mutable roots advance the
/// source schedule. Reads recursively reached from a static root have no mutable
/// context: every source-chain pool lookup pins the same live root, including
/// nested reads. Calls inside mutable entries retain their parent's context.
/// This projection also cross-checks the materialized source
/// arrays so a malformed semantic payload cannot silently change action kind,
/// outcome, or cardinality.
///
/// # Errors
///
/// Returns an error when native actions and materialized source entries do not
/// describe the same ordered mutable/static actions.
pub fn composition_fifo_actions(
    composition: &Composition,
) -> Result<Vec<FifoSourceAction>, String> {
    composition_fifo_action_indices(composition)
        .map(|actions| actions.into_iter().map(|(_, action)| action).collect())
}

/// Recover the absolute semantic-action index for every FIFO source action.
///
/// This is the indexed form of [`composition_fifo_actions`]. Keeping the
/// projection and materialized-table cross-check in one place prevents
/// observation verification from binding a different action order.
fn composition_fifo_action_indices(
    composition: &Composition,
) -> Result<Vec<(usize, FifoSourceAction)>, String> {
    let applied = l1_applied_root_mask(&composition.actions)?;
    let mut actions = Vec::new();
    let mut mutable_cursor = 0usize;
    let mut static_cursor = 0usize;
    let mut static_root = false;

    for (action_index, action) in composition.actions.iter().enumerate() {
        if action.parent_index.is_none() {
            static_root = action.call_mode == CallMode::Static;
        }
        if action.source_rollup_id != composition.source.rollup_id
            || (action.parent_index.is_some() && !static_root)
        {
            continue;
        }
        let semantic_success = action.outcome.is_success();
        match action.call_mode {
            CallMode::Mutable => {
                let Some(entry) = composition.source.batch.entries.get(mutable_cursor) else {
                    return Err(format!(
                        "native semantics contain mutable source action {mutable_cursor} without a materialized entry"
                    ));
                };
                if entry.success != semantic_success {
                    return Err(format!(
                        "mutable source action {mutable_cursor} outcome differs from its materialized entry"
                    ));
                }
                actions.push((
                    action_index,
                    FifoSourceAction::Mutable {
                        applies: applied[action_index],
                    },
                ));
                mutable_cursor += 1;
            }
            CallMode::Static => {
                let Some(entry) = composition.source.batch.staticEntries.get(static_cursor) else {
                    return Err(format!(
                        "native semantics contain static source action {static_cursor} without a materialized entry"
                    ));
                };
                if entry.success != semantic_success {
                    return Err(format!(
                        "static source action {static_cursor} outcome differs from its materialized entry"
                    ));
                }
                actions.push((action_index, FifoSourceAction::Static));
                static_cursor += 1;
            }
        }
    }

    if mutable_cursor != composition.source.batch.entries.len()
        || static_cursor != composition.source.batch.staticEntries.len()
    {
        return Err(format!(
            "native FIFO source actions materialize as {mutable_cursor} mutable/{static_cursor} static entries, expected {}/{}",
            composition.source.batch.entries.len(),
            composition.source.batch.staticEntries.len(),
        ));
    }

    Ok(actions)
}

/// Root assignments for one FIFO source transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FifoTransactionRootPlan {
    /// `newRoot` for each mutable source entry, in mutable-entry order.
    pub mutable_new_roots: Vec<B256>,
    /// Root pin for each source static-pool entry, in preorder.
    pub static_roots: Vec<B256>,
    /// Root live after the complete transaction.
    pub endpoint: B256,
}

/// Bind every lookup/static root action to its pinned state and claimed
/// result.
///
/// Applying mutable actions are omitted because their canonical system
/// transaction is already re-executed by the signer. Mutable lookups derive a
/// complete destination table from the native action tree. The signer executes
/// that table through the authenticated destination manager in discarded state;
/// source callbacks are bound to the source entry checked on L1. Static lookups
/// carry their source callbacks explicitly (see [`FifoObservationCallback`]).
/// Local EVM subcalls are replayed normally.
///
/// # Errors
///
/// Returns an error if `plan` does not cover the exact FIFO projection, an
/// observation outcome is unresolved, a rollback scope crosses canonical
/// prefixes, or the action tree requires execution on a third chain.
pub fn fifo_observation_claims(
    composition: &Composition,
    plan: &FifoTransactionRootPlan,
) -> Result<Vec<FifoObservationClaim>, String> {
    let indexed = composition_fifo_action_indices(composition)?;
    let mutable_count = indexed
        .iter()
        .filter(|(_, action)| matches!(action, FifoSourceAction::Mutable { .. }))
        .count();
    let static_count = indexed
        .iter()
        .filter(|(_, action)| matches!(action, FifoSourceAction::Static))
        .count();
    if plan.mutable_new_roots.len() != mutable_count || plan.static_roots.len() != static_count {
        return Err(format!(
            "FIFO observation plan covers {}/{} mutable/static roots, expected {mutable_count}/{static_count}",
            plan.mutable_new_roots.len(),
            plan.static_roots.len(),
        ));
    }

    let mut claims = Vec::new();
    let mut claim_actions = Vec::new();
    let mut mutable_cursor = 0usize;
    let mut static_cursor = 0usize;
    for (action_index, fifo_action) in indexed {
        let (observe, pinned_block_hash) = match fifo_action {
            FifoSourceAction::Mutable { applies } => {
                let root = plan.mutable_new_roots[mutable_cursor];
                mutable_cursor += 1;
                (!applies, root)
            }
            FifoSourceAction::Static => {
                let root = plan.static_roots[static_cursor];
                static_cursor += 1;
                (true, root)
            }
        };
        if !observe {
            continue;
        }
        let action = &composition.actions[action_index];

        let callbacks = if action.call_mode == CallMode::Static {
            static_observation_callbacks(&composition.actions, action_index)?
        } else {
            Vec::new()
        };
        let delivery = if action.call_mode == CallMode::Mutable {
            let entry =
                crate::entries::materialize_lookup_execution(&composition.actions, action_index)
                    .map_err(|error| error.to_string())?;
            let value = crate::system_tx::committed_l2_table_value(std::slice::from_ref(&entry))?;
            let calldata = crate::abi::executeIncomingCrossChainCallCall {
                _entries: vec![entry],
                _staticEntries: Vec::new(),
            }
            .abi_encode()
            .into();
            Some(FifoObservationDelivery { calldata, value })
        } else {
            None
        };

        if action.call_mode == CallMode::Static && action.value != U256::ZERO {
            return Err(format!(
                "static source action {action_index} carries nonzero value"
            ));
        }
        let ExecutionOutcome::Resolved {
            return_data,
            success,
            ..
        } = &action.outcome
        else {
            return Err(format!(
                "lookup/static source action {action_index} has an unresolved outcome"
            ));
        };
        claim_actions.push(action_index);
        claims.push(FifoObservationClaim {
            pinned_block_hash,
            call_mode: action.call_mode,
            target_address: action.target_address,
            source_address: action.source_address,
            source_rollup_id: action.source_rollup_id,
            data: action.data.clone(),
            value: action.value,
            expected_success: *success,
            expected_output: Bytes::copy_from_slice(return_data),
            callbacks,
            delivery,
            rollback_spans: Vec::new(),
        });
    }
    for (claim_index, &action_index) in claim_actions.iter().enumerate() {
        let action = &composition.actions[action_index];
        if action.parent_index.is_some() {
            continue;
        }
        for &span in &action.revert_spans {
            let end = action_index
                .checked_add(span as usize)
                .filter(|&end| span != 0 && end <= composition.actions.len())
                .ok_or_else(|| {
                    format!("invalid observation rollback span at action {action_index}")
                })?;
            let end_claim = claim_actions.partition_point(|&index| index < end);
            if claims[claim_index..end_claim]
                .iter()
                .any(|claim| claim.pinned_block_hash != claims[claim_index].pinned_block_hash)
            {
                return Err("observation rollback scope crosses canonical prefixes".into());
            }
            claims[claim_index]
                .rollback_spans
                .push(end_claim - claim_index);
        }
    }
    Ok(claims)
}

/// Direct static callbacks of this read. Deeper reads back into the witnessed
/// chain have their own pool entries and observation claims at the same prefix;
/// they must not be answered as if they were direct children of this read.
fn static_observation_callbacks(
    actions: &[ExecutedAction],
    root: usize,
) -> Result<Vec<FifoObservationCallback>, String> {
    let source_rollup_id = actions[root].source_rollup_id;
    let mut callbacks = Vec::new();
    for (index, action) in actions.iter().enumerate().skip(root + 1) {
        let Some(parent) = action.parent_index else {
            break;
        };
        if parent < root {
            break;
        }
        if parent != root {
            continue;
        }
        let ExecutionOutcome::Resolved {
            return_data,
            success,
            ..
        } = &action.outcome
        else {
            return Err(format!(
                "static source action {root} has an unresolved callback {index}"
            ));
        };
        if action.call_mode != CallMode::Static
            || action.target_rollup_id != source_rollup_id
            || action.value != U256::ZERO
        {
            return Err(format!(
                "static source action {root} has descendant {index} outside the static-callback profile"
            ));
        }
        callbacks.push(FifoObservationCallback {
            source_address: action.source_address,
            target_address: action.target_address,
            data: action.data.clone(),
            success: *success,
            output: Bytes::copy_from_slice(return_data),
        });
    }
    Ok(callbacks)
}

/// Build the canonical root schedule for one inbound source transaction.
///
/// Applying mutable entries advance the commitment at their FIFO position.
/// Lookup-only mutable entries receive a no-op update and static entries pin
/// the root that is consequently live at their position. There is no trailing
/// root checkpoint: a lookup cannot move the L2 commitment.
///
/// # Errors
///
/// Returns an error if the number of applying mutable actions differs from the
/// number of independently derived effect checkpoints.
pub fn fifo_transaction_root_plan(
    start_root: B256,
    actions: &[FifoSourceAction],
    effect_checkpoints: &[B256],
) -> Result<FifoTransactionRootPlan, String> {
    let mutable_count = actions
        .iter()
        .filter(|action| matches!(action, FifoSourceAction::Mutable { .. }))
        .count();
    let applying_count = actions
        .iter()
        .filter(|action| matches!(action, FifoSourceAction::Mutable { applies: true }))
        .count();
    if applying_count != effect_checkpoints.len() {
        return Err(format!(
            "FIFO root plan has {applying_count} applying mutable actions but {} effect checkpoints",
            effect_checkpoints.len(),
        ));
    }

    let mut live_root = start_root;
    let mut mutable_cursor = 0usize;
    let mut mutable_new_roots = Vec::with_capacity(mutable_count);
    let mut static_roots = Vec::new();
    for action in actions {
        match *action {
            FifoSourceAction::Mutable { applies } => {
                if applies {
                    live_root = effect_checkpoints[mutable_cursor];
                    mutable_cursor += 1;
                }
                mutable_new_roots.push(live_root);
            }
            FifoSourceAction::Static => static_roots.push(live_root),
        }
    }

    Ok(FifoTransactionRootPlan {
        mutable_new_roots,
        static_roots,
        endpoint: live_root,
    })
}

/// Classify the native envelope, recovered sender and target together. A legacy
/// transaction claiming the reserved sender cannot acquire system semantics.
/// Deployment parameters must match the fixed native protocol addresses.
#[must_use]
pub fn is_system_tx(
    tx_type: u8,
    signer: Address,
    to: Option<Address>,
    expected_system_address: Address,
    eezl2_address: Address,
) -> bool {
    tx_type == eez_primitives::SYSTEM_TX_TYPE
        && expected_system_address == eez_primitives::SYSTEM_ADDRESS
        && eezl2_address == eez_primitives::EEZL2_ADDRESS
        && signer == expected_system_address
        && to == Some(eezl2_address)
}

/// Pair-end tx positions in a sync block — one per settled cross-chain effect,
/// in tx order. A position ends a pair iff it is a USER tx (the `user` half of
/// an outbound `[load | user]` pair) OR a SYSTEM tx followed by a system tx / the
/// block end (a standalone inbound system tx). Single source of truth shared by
/// the composer (per-effect `RootUpdate.newRoot` stitch) and proof signer
/// (effect/checkpoint binding), so the settled block hashes line up entry-for-entry.
#[must_use]
pub fn pair_end_positions(is_system: &[bool]) -> Vec<usize> {
    (0..is_system.len())
        .filter(|&i| !is_system[i] || is_system.get(i + 1).copied().unwrap_or(true))
        .collect()
}

/// Assign execution checkpoints to settlement entries without relying on a
/// failed L1 entry to persist its root update.
///
/// `effects` contains each settlement entry's L2 checkpoint and claimed L1
/// execution outcome in entry order. A leading run of failed entries is
/// carried by the successful anchor. Every later successful entry carries its
/// own checkpoint and those of the immediately following failed entries.
/// Failed entries receive a no-op commitment (`currentRoot == newRoot` after chain
/// stitching), because `EEZ.sol` rolls their root updates back together with
/// the intentionally reverted call.
///
/// The returned tuple is `(anchor_new_root, effect_new_roots)`.
#[must_use]
pub fn settlement_root_carriers(
    pre_settling_root: B256,
    effects: impl IntoIterator<Item = (B256, bool)>,
) -> (B256, Vec<B256>) {
    let effects = effects.into_iter().collect::<Vec<_>>();
    let mut anchor_new_root = pre_settling_root;
    let mut effect_new_roots = vec![pre_settling_root; effects.len()];
    let mut active_success = None;

    // Assign every checkpoint to the most recent successful entry, or to the
    // anchor while the sequence still consists only of failed entries.
    for (effect_index, (checkpoint, success)) in effects.iter().copied().enumerate() {
        if success {
            active_success = Some(effect_index);
        }
        if let Some(carrier_index) = active_success {
            effect_new_roots[carrier_index] = checkpoint;
        } else {
            anchor_new_root = checkpoint;
        }
    }

    // Failed entries must claim the block hash already persisted by their carrier.
    // This also makes their root update a no-op when the chain is stitched.
    let mut persisted_root = anchor_new_root;
    for (effect_index, (_, success)) in effects.iter().copied().enumerate() {
        if success {
            persisted_root = effect_new_roots[effect_index];
        } else {
            effect_new_roots[effect_index] = persisted_root;
        }
    }

    (anchor_new_root, effect_new_roots)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::address;

    use alloy_primitives::{Address, B256, Bytes, U256};

    use super::{
        FifoSourceAction, FifoTransactionRootPlan, fifo_observation_claims,
        fifo_transaction_root_plan, is_lookup_only_inbound_entry, is_root_only_entry, is_system_tx,
        l1_applied_root_mask, pair_end_positions, settlement_root_carriers,
    };
    use crate::abi::{
        EtherDeltaSol, ExecutionEntrySol, ExpectedL1ToL2CallSol, RootUpdateSol,
        StaticExecutionEntrySol,
    };
    use crate::rolling_hash::EntryRollingHash;
    use crate::{
        CallMode, Composition, EvmBatch, ExecutedAction, ExecutionOutcome, RollupId,
        SourceComposition,
    };

    fn action(parent_index: Option<usize>, call_mode: CallMode, success: bool) -> ExecutedAction {
        ExecutedAction {
            parent_index,
            call_mode,
            target_address: Address::repeat_byte(0x11),
            target_rollup_id: RollupId(1),
            source_rollup_id: RollupId::MAINNET,
            source_address: Address::repeat_byte(0x22),
            data: Bytes::new(),
            value: U256::ZERO,
            outcome: ExecutionOutcome::Resolved {
                return_data: Vec::new(),
                gas_used: 0,
                success,
            },
            revert_spans: vec![],
        }
    }

    fn composition(actions: Vec<ExecutedAction>) -> Composition {
        let mut batch = EvmBatch::default();
        for action in actions.iter().filter(|action| {
            action.parent_index.is_none() && action.source_rollup_id == RollupId::MAINNET
        }) {
            match action.call_mode {
                CallMode::Mutable => batch.entries.push(ExecutionEntrySol {
                    success: action.outcome.is_success(),
                    ..Default::default()
                }),
                CallMode::Static => batch.staticEntries.push(StaticExecutionEntrySol {
                    success: action.outcome.is_success(),
                    ..Default::default()
                }),
            }
        }
        Composition {
            source: SourceComposition {
                l2_static_entries: Vec::new(),
                rollup_id: RollupId::MAINNET,
                batch,
                entry_traces: Vec::new(),
                outbound_event_hashes: Vec::new(),
            },
            targets: Vec::new(),
            actions,
        }
    }

    #[test]
    fn system_transaction_identity_requires_native_type_and_fixed_addresses() {
        let system_address = eez_primitives::SYSTEM_ADDRESS;
        let eezl2_address = eez_primitives::EEZL2_ADDRESS;

        assert!(is_system_tx(
            eez_primitives::SYSTEM_TX_TYPE,
            system_address,
            Some(eezl2_address),
            system_address,
            eezl2_address,
        ));
        assert!(!is_system_tx(
            eez_primitives::SYSTEM_TX_TYPE,
            address!("3333333333333333333333333333333333333333"),
            Some(eezl2_address),
            system_address,
            eezl2_address,
        ));
        assert!(!is_system_tx(
            eez_primitives::SYSTEM_TX_TYPE,
            system_address,
            Some(address!("4444444444444444444444444444444444444444")),
            system_address,
            eezl2_address,
        ));
        assert!(!is_system_tx(
            0,
            system_address,
            Some(eezl2_address),
            system_address,
            eezl2_address
        ));
    }

    #[test]
    fn root_only_entry_shape_is_exact_and_fail_closed() {
        let rollup_id = 7;
        let current = B256::repeat_byte(0x11);
        let mut entry = ExecutionEntrySol {
            rootUpdates: vec![RootUpdateSol {
                rollupId: rollup_id,
                currentRoot: current,
                newRoot: B256::repeat_byte(0x22),
                etherDelta: EtherDeltaSol::ZERO,
            }],
            destinationRollupId: rollup_id,
            success: true,
            rollingHash: EntryRollingHash::seed_for_l1([(rollup_id, current)], B256::ZERO)
                .current(),
            ..Default::default()
        };
        assert!(is_root_only_entry(&entry, rollup_id));

        entry.returnData = vec![1].into();
        assert!(!is_root_only_entry(&entry, rollup_id));
        entry.returnData = Default::default();
        entry.rootUpdates[0].etherDelta = EtherDeltaSol::ONE;
        assert!(!is_root_only_entry(&entry, rollup_id));
        entry.rootUpdates[0].etherDelta = EtherDeltaSol::ZERO;
        entry.rollingHash = B256::ZERO;
        assert!(!is_root_only_entry(&entry, rollup_id));
    }

    #[test]
    fn lookup_only_entry_shape_is_exact_and_fail_closed() {
        let rollup_id = 7;
        let root = B256::repeat_byte(0x11);
        let proxy = B256::repeat_byte(0x22);
        let mut entry = ExecutionEntrySol {
            rootUpdates: vec![RootUpdateSol {
                rollupId: rollup_id,
                currentRoot: root,
                newRoot: root,
                etherDelta: EtherDeltaSol::ZERO,
            }],
            proxyEntryHash: proxy,
            destinationRollupId: rollup_id,
            success: false,
            returnData: vec![0xde, 0xad].into(),
            rollingHash: EntryRollingHash::seed_for_l1([(rollup_id, root)], proxy).current(),
            ..Default::default()
        };
        assert!(is_lookup_only_inbound_entry(&entry, rollup_id));

        entry.rootUpdates[0].newRoot = B256::repeat_byte(0x33);
        assert!(!is_lookup_only_inbound_entry(&entry, rollup_id));
        entry.rootUpdates[0].newRoot = root;
        entry.expectedL1ToL2Calls.push(ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: B256::ZERO,
            l2ToL1Calls: Vec::new(),
            revertedOrStaticRollingHash: B256::ZERO,
            success: true,
            returnData: Default::default(),
        });
        // Classification allows callback tables. Semantic binding and replay
        // must independently reject an unbound or forged row.
        assert!(is_lookup_only_inbound_entry(&entry, rollup_id));
        entry.rootUpdates[0].etherDelta = EtherDeltaSol::ONE;
        assert!(!is_lookup_only_inbound_entry(&entry, rollup_id));
    }

    #[test]
    fn applied_root_mask_requires_mutable_success_without_source_revert() {
        let actions = [
            action(None, CallMode::Mutable, true),
            action(None, CallMode::Mutable, false),
            action(None, CallMode::Static, true),
        ];

        assert_eq!(
            l1_applied_root_mask(&actions).unwrap(),
            [true, false, false]
        );
    }

    #[test]
    fn applied_root_mask_excludes_every_root_in_a_source_revert_span() {
        let mut actions = [
            action(None, CallMode::Mutable, true),
            action(Some(0), CallMode::Mutable, true),
            action(None, CallMode::Mutable, true),
            action(None, CallMode::Mutable, true),
        ];
        actions[0].revert_spans = vec![3];

        assert_eq!(
            l1_applied_root_mask(&actions).unwrap(),
            [false, false, false, true]
        );
    }

    #[test]
    fn applied_root_mask_rejects_out_of_bounds_source_revert_span() {
        let mut actions = [action(None, CallMode::Mutable, true)];
        actions[0].revert_spans = vec![2];

        assert!(l1_applied_root_mask(&actions).is_err());
    }

    /// A pair ends at every user (non-system) tx, at a system tx followed by a
    /// system tx, and at the last tx regardless. A system tx followed by a user
    /// tx is NOT a pair end — it pairs with that user tx.
    #[test]
    fn pair_end_positions_cases() {
        assert_eq!(pair_end_positions(&[]), Vec::<usize>::new());
        // all user txs → each ends its own pair
        assert_eq!(pair_end_positions(&[false, false, false]), vec![0, 1, 2]);
        // system → user: only the user (index 1) ends the pair
        assert_eq!(pair_end_positions(&[true, false]), vec![1]);
        // system → system: the first system is a standalone pair end; the last
        // tx is always a pair end
        assert_eq!(pair_end_positions(&[true, true]), vec![0, 1]);
        // user then trailing system (last) → both are pair ends
        assert_eq!(pair_end_positions(&[false, true]), vec![0, 1]);
        // two outbound [load|user] pairs → ends at each user
        assert_eq!(pair_end_positions(&[true, false, true, false]), vec![1, 3]);
        // a lone system tx (also the last) → pair end
        assert_eq!(pair_end_positions(&[true]), vec![0]);
    }

    #[test]
    fn root_carriers_absorb_failed_effect_checkpoints() {
        let pre = B256::repeat_byte(0x10);
        let roots = (1..=5).map(B256::with_last_byte).collect::<Vec<_>>();

        let (anchor, entries) = settlement_root_carriers(
            pre,
            roots.iter().copied().zip([false, false, true, false, true]),
        );

        assert_eq!(anchor, roots[1]);
        assert_eq!(entries, [roots[1], roots[1], roots[3], roots[3], roots[4]]);
    }

    #[test]
    fn root_carriers_preserve_the_direct_success_path() {
        let pre = B256::repeat_byte(0x10);
        let first = B256::repeat_byte(0x11);
        let second = B256::repeat_byte(0x12);

        let (anchor, entries) = settlement_root_carriers(pre, [(first, true), (second, true)]);

        assert_eq!(anchor, pre);
        assert_eq!(entries, [first, second]);
    }

    #[test]
    fn fifo_plan_pins_statics_at_their_exact_success_positions() {
        let r0 = B256::repeat_byte(0x10);
        let r1 = B256::repeat_byte(0x11);
        let r2 = B256::repeat_byte(0x12);
        let plan = fifo_transaction_root_plan(
            r0,
            &[
                FifoSourceAction::Static,
                FifoSourceAction::Mutable { applies: true },
                FifoSourceAction::Static,
                FifoSourceAction::Mutable { applies: true },
                FifoSourceAction::Static,
            ],
            &[r1, r2],
        )
        .unwrap();

        assert_eq!(plan.mutable_new_roots, [r1, r2]);
        assert_eq!(plan.static_roots, [r0, r1, r2]);
        assert_eq!(plan.endpoint, r2);
    }

    #[test]
    fn fifo_plan_keeps_failed_effects_behind_earlier_static_reads() {
        let r0 = B256::repeat_byte(0x20);
        let plan = fifo_transaction_root_plan(
            r0,
            &[
                FifoSourceAction::Static,
                FifoSourceAction::Mutable { applies: false },
                FifoSourceAction::Static,
            ],
            &[],
        )
        .unwrap();

        assert_eq!(plan.mutable_new_roots, [r0]);
        assert_eq!(plan.static_roots, [r0, r0]);
        assert_eq!(plan.endpoint, r0);
    }

    #[test]
    fn fifo_plan_advances_a_later_success_over_prior_failures() {
        let r0 = B256::repeat_byte(0x30);
        let success = B256::repeat_byte(0x32);
        let plan = fifo_transaction_root_plan(
            r0,
            &[
                FifoSourceAction::Mutable { applies: false },
                FifoSourceAction::Static,
                FifoSourceAction::Mutable { applies: true },
            ],
            &[success],
        )
        .unwrap();

        assert_eq!(plan.mutable_new_roots, [r0, success]);
        assert_eq!(plan.static_roots, [r0]);
        assert_eq!(plan.endpoint, success);
    }

    #[test]
    fn fifo_plan_rejects_action_checkpoint_count_mismatch() {
        let root = B256::ZERO;
        assert!(
            fifo_transaction_root_plan(root, &[FifoSourceAction::Mutable { applies: true }], &[],)
                .is_err()
        );
    }

    #[test]
    fn fifo_plan_keeps_a_lookup_only_tail_at_the_live_root() {
        let root = B256::repeat_byte(0x41);
        let plan =
            fifo_transaction_root_plan(root, &[FifoSourceAction::Mutable { applies: false }], &[])
                .unwrap();

        assert_eq!(plan.mutable_new_roots, [root]);
        assert_eq!(plan.endpoint, root);
    }

    #[test]
    fn observation_claims_bind_exact_fifo_pins_inputs_and_outputs() {
        let static_root = B256::repeat_byte(0x41);
        let success_root = B256::repeat_byte(0x42);
        let mut static_action = action(None, CallMode::Static, true);
        static_action.target_address = Address::repeat_byte(0x31);
        static_action.source_address = Address::repeat_byte(0x32);
        static_action.data = Bytes::from_static(b"static input");
        static_action.outcome = ExecutionOutcome::Resolved {
            return_data: b"static output".to_vec(),
            gas_used: 17,
            success: true,
        };
        let mut lookup_action = action(None, CallMode::Mutable, false);
        lookup_action.target_address = Address::repeat_byte(0x33);
        lookup_action.source_address = Address::repeat_byte(0x34);
        lookup_action.data = Bytes::from_static(b"lookup input");
        lookup_action.value = U256::from(9);
        lookup_action.outcome = ExecutionOutcome::Resolved {
            return_data: b"lookup revert".to_vec(),
            gas_used: 23,
            success: false,
        };
        let applying_action = action(None, CallMode::Mutable, true);
        let composition = composition(vec![static_action, lookup_action, applying_action]);
        let plan = FifoTransactionRootPlan {
            mutable_new_roots: vec![static_root, success_root],
            static_roots: vec![static_root],
            endpoint: success_root,
        };

        let claims = fifo_observation_claims(&composition, &plan).unwrap();

        assert_eq!(claims.len(), 2);
        assert_eq!(claims[0].pinned_block_hash, static_root);
        assert_eq!(claims[0].call_mode, CallMode::Static);
        assert_eq!(claims[0].target_address, Address::repeat_byte(0x31));
        assert_eq!(claims[0].source_address, Address::repeat_byte(0x32));
        assert_eq!(claims[0].data, Bytes::from_static(b"static input"));
        assert!(claims[0].expected_success);
        assert_eq!(
            claims[0].expected_output,
            Bytes::from_static(b"static output")
        );
        assert_eq!(claims[1].pinned_block_hash, static_root);
        assert_eq!(claims[1].call_mode, CallMode::Mutable);
        assert_eq!(claims[1].value, U256::from(9));
        assert!(!claims[1].expected_success);
        assert_eq!(
            claims[1].expected_output,
            Bytes::from_static(b"lookup revert")
        );
    }

    #[test]
    fn observation_claims_reject_callbacks_to_an_unwitnessed_third_chain() {
        let plan = FifoTransactionRootPlan {
            mutable_new_roots: Vec::new(),
            static_roots: vec![B256::repeat_byte(0x51)],
            endpoint: B256::repeat_byte(0x51),
        };
        let mut child = action(Some(0), CallMode::Mutable, true);
        child.source_rollup_id = RollupId(1);
        child.target_rollup_id = RollupId(2);

        let static_root = composition(vec![action(None, CallMode::Static, true), child.clone()]);
        let error = fifo_observation_claims(&static_root, &plan).unwrap_err();
        assert!(
            error.contains("outside the static-callback profile"),
            "{error}"
        );

        let lookup = composition(vec![action(None, CallMode::Mutable, false), child]);
        let plan = FifoTransactionRootPlan {
            mutable_new_roots: vec![B256::repeat_byte(0x51)],
            static_roots: Vec::new(),
            endpoint: B256::repeat_byte(0x51),
        };
        let error = fifo_observation_claims(&lookup, &plan).unwrap_err();
        assert!(
            error.contains("lookup execution is not one complete destination table"),
            "{error}"
        );
    }

    #[test]
    fn mutable_lookup_derives_a_complete_callback_table() {
        use crate::abi::executeIncomingCrossChainCallCall;
        use alloy_sol_types::SolCall as _;

        let root = action(None, CallMode::Mutable, false);
        let mut callback = action(Some(0), CallMode::Mutable, true);
        callback.source_rollup_id = RollupId(1);
        callback.target_rollup_id = RollupId::MAINNET;
        callback.data = Bytes::from_static(b"source callback");
        let plan = FifoTransactionRootPlan {
            mutable_new_roots: vec![B256::repeat_byte(0x51)],
            static_roots: Vec::new(),
            endpoint: B256::repeat_byte(0x51),
        };
        let claims = fifo_observation_claims(&composition(vec![root, callback]), &plan).unwrap();
        assert_eq!(claims.len(), 1);
        assert!(!claims[0].expected_success);
        let delivery = claims[0].delivery.as_ref().unwrap();
        let call = executeIncomingCrossChainCallCall::abi_decode(&delivery.calldata).unwrap();
        assert_eq!(call._entries.len(), 1);
        assert!(call._staticEntries.is_empty());
        assert_eq!(call._entries[0].incomingCalls.len(), 1);
        assert_eq!(call._entries[0].expectedOutgoingCalls.len(), 1);
        assert!(call._entries[0].expectedOutgoingCalls[0].success);
        assert_ne!(
            call._entries[0].expectedOutgoingCalls[0].expectedOutgoingHash,
            B256::ZERO
        );
        assert_eq!(delivery.value, U256::ZERO);
    }

    #[test]
    fn observation_rollback_spans_preserve_nested_root_order() {
        let pin = B256::repeat_byte(0x61);
        let mut outer = action(None, CallMode::Mutable, true);
        outer.revert_spans = vec![4];
        let mut inner = action(None, CallMode::Mutable, true);
        inner.revert_spans = vec![2];
        let composition = composition(vec![
            outer,
            inner,
            action(None, CallMode::Static, true),
            action(None, CallMode::Static, true),
            action(None, CallMode::Static, true),
        ]);
        let claims = fifo_observation_claims(
            &composition,
            &FifoTransactionRootPlan {
                mutable_new_roots: vec![pin; 2],
                static_roots: vec![pin; 3],
                endpoint: pin,
            },
        )
        .unwrap();
        assert_eq!(claims.len(), 5);
        assert_eq!(claims[0].rollback_spans, [4]);
        assert_eq!(claims[1].rollback_spans, [2]);
        assert!(
            claims[2..]
                .iter()
                .all(|claim| claim.rollback_spans.is_empty())
        );
        let error = fifo_observation_claims(
            &composition,
            &FifoTransactionRootPlan {
                mutable_new_roots: vec![pin; 2],
                static_roots: vec![B256::ZERO, pin, pin],
                endpoint: pin,
            },
        )
        .unwrap_err();
        assert!(
            error.contains("rollback scope crosses canonical prefixes"),
            "{error}"
        );
    }

    #[test]
    fn observation_claims_carry_static_callbacks_to_the_source_chain() {
        let root = action(None, CallMode::Static, true);
        let mut callback = action(Some(0), CallMode::Static, true);
        callback.source_rollup_id = RollupId(1);
        callback.target_rollup_id = RollupId::MAINNET;
        callback.source_address = Address::repeat_byte(0x61);
        callback.target_address = Address::repeat_byte(0x62);
        callback.data = Bytes::from_static(b"callback input");
        callback.outcome = ExecutionOutcome::Resolved {
            return_data: b"callback output".to_vec(),
            gas_used: 0,
            success: true,
        };
        let plan = FifoTransactionRootPlan {
            mutable_new_roots: Vec::new(),
            static_roots: vec![B256::repeat_byte(0x53)],
            endpoint: B256::repeat_byte(0x53),
        };

        let claims =
            fifo_observation_claims(&composition(vec![root, callback.clone()]), &plan).unwrap();
        let [claim] = claims.as_slice() else {
            panic!("expected one static claim");
        };
        assert_eq!(
            claim.callbacks,
            [super::FifoObservationCallback {
                source_address: Address::repeat_byte(0x61),
                target_address: Address::repeat_byte(0x62),
                data: Bytes::from_static(b"callback input"),
                success: true,
                output: Bytes::from_static(b"callback output"),
            }]
        );

        // The source chain binds failure and revert bytes in the same fold.
        callback.outcome = ExecutionOutcome::Resolved {
            return_data: Vec::new(),
            gas_used: 0,
            success: false,
        };
        let claims = fifo_observation_claims(
            &composition(vec![action(None, CallMode::Static, true), callback]),
            &plan,
        )
        .unwrap();
        assert!(!claims[0].callbacks[0].success);
        assert!(claims[0].callbacks[0].output.is_empty());
    }

    #[test]
    fn observation_claims_bind_multi_action_rollback_regions() {
        let mut first = action(None, CallMode::Mutable, true);
        first.revert_spans = vec![2];
        let second = action(None, CallMode::Mutable, true);
        let composition = composition(vec![first, second]);
        let root = B256::repeat_byte(0x52);
        let plan = FifoTransactionRootPlan {
            mutable_new_roots: vec![root, root],
            static_roots: Vec::new(),
            endpoint: root,
        };

        let claims = fifo_observation_claims(&composition, &plan).unwrap();
        assert_eq!(claims.len(), 2);
        assert_eq!(claims[0].rollback_spans, [2]);
        assert!(claims[1].rollback_spans.is_empty());
        assert!(claims.iter().all(|claim| claim.delivery.is_some()));
    }

    #[test]
    fn observation_claims_accept_single_action_rollback_regions() {
        let mut action = action(None, CallMode::Mutable, true);
        action.revert_spans = vec![1];
        action.outcome = ExecutionOutcome::Resolved {
            return_data: b"rolled back".to_vec(),
            gas_used: 31,
            success: true,
        };
        let composition = composition(vec![action]);
        let root = B256::repeat_byte(0x53);
        let plan = FifoTransactionRootPlan {
            mutable_new_roots: vec![root],
            static_roots: Vec::new(),
            endpoint: root,
        };

        let claims = fifo_observation_claims(&composition, &plan).unwrap();

        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].pinned_block_hash, root);
        assert!(claims[0].expected_success);
        assert_eq!(
            claims[0].expected_output,
            Bytes::from_static(b"rolled back")
        );
    }

    #[test]
    fn observation_claims_reject_value_on_static_calls() {
        let mut static_action = action(None, CallMode::Static, true);
        static_action.value = U256::ONE;
        let composition = composition(vec![static_action]);
        let plan = FifoTransactionRootPlan {
            mutable_new_roots: Vec::new(),
            static_roots: vec![B256::repeat_byte(0x61)],
            endpoint: B256::repeat_byte(0x61),
        };

        let error = fifo_observation_claims(&composition, &plan).unwrap_err();

        assert!(error.contains("carries nonzero value"));
    }
}
