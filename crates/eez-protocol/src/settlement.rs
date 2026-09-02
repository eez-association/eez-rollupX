//! Settlement transaction-framing helpers shared by the composer and proof
//! signer.

use alloy_primitives::{Address, B256};

use crate::abi::{EtherDeltaSol, ExecutionEntrySol};
use crate::{CallMode, Composition, EntryRollingHash, ExecutedAction};

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

/// Whether a deferred entry is a canonical L1-origin lookup that changes no L2
/// state.
///
/// Lookup-only entries retain a nonzero proxy hash plus the source-visible
/// outcome, but contain no destination execution trace and a zero-value no-op
/// root update. Recomputing the rolling seed makes the classification exact;
/// an arbitrary deferred entry cannot opt out of execution merely by setting
/// `currentRoot == newRoot`.
#[must_use]
pub fn is_lookup_only_inbound_entry(entry: &ExecutionEntrySol, settled_rollup: u64) -> bool {
    let [update] = entry.rootUpdates.as_slice() else {
        return false;
    };
    entry.proxyEntryHash != B256::ZERO
        && entry.l2ToL1Calls.is_empty()
        && entry.expectedL1ToL2Calls.is_empty()
        && entry.destinationRollupId == settled_rollup
        && update.rollupId == settled_rollup
        && update.currentRoot == update.newRoot
        && update.etherDelta == EtherDeltaSol::ZERO
        && entry.rollingHash
            == EntryRollingHash::seed_for_l1(
                [(settled_rollup, update.currentRoot)],
                entry.proxyEntryHash,
            )
            .current()
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
            .revert_span
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

/// Recover the top-level source actions that must execute in FIFO order.
///
/// Native semantics retain the exact call forest. Only root actions emitted by
/// the composition's source transaction belong to its L1 execution frame;
/// nested calls execute inside their parent entry and therefore do not get a
/// separate root pin. This projection also cross-checks the materialized source
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
    let applied = l1_applied_root_mask(&composition.actions)?;
    let mut actions = Vec::new();
    let mut mutable_cursor = 0usize;
    let mut static_cursor = 0usize;

    for (action_index, action) in composition
        .actions
        .iter()
        .enumerate()
        .filter(|(_, action)| {
            action.parent_index.is_none() && action.source_rollup_id == composition.source.rollup_id
        })
    {
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
                actions.push(FifoSourceAction::Mutable {
                    applies: applied[action_index],
                });
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
                actions.push(FifoSourceAction::Static);
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
    /// Root pin for each top-level static entry, in static-entry order.
    pub static_roots: Vec<B256>,
    /// Root live after the complete transaction.
    pub endpoint: B256,
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

/// A sync-block transaction is a system transaction iff its recovered signer
/// matches the deployment's system address and it targets `EEZL2`.
/// The proof signer and composer must use the same deployment values so their
/// pair-end positions, and therefore per-effect settlement block hashes, agree.
#[must_use]
pub fn is_system_tx(
    signer: Address,
    to: Option<Address>,
    expected_system_address: Address,
    eezl2_address: Address,
) -> bool {
    signer == expected_system_address && to == Some(eezl2_address)
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
        FifoSourceAction, fifo_transaction_root_plan, is_lookup_only_inbound_entry,
        is_root_only_entry, is_system_tx, l1_applied_root_mask, pair_end_positions,
        settlement_root_carriers,
    };
    use crate::abi::{EtherDeltaSol, ExecutionEntrySol, ExpectedL1ToL2CallSol, RootUpdateSol};
    use crate::rolling_hash::EntryRollingHash;
    use crate::{CallMode, ExecutedAction, ExecutionOutcome, RollupId};

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
            revert_span: None,
        }
    }

    #[test]
    fn system_transaction_identity_uses_deployment_addresses() {
        let system_address = address!("1111111111111111111111111111111111111111");
        let eezl2_address = address!("2222222222222222222222222222222222222222");

        assert!(is_system_tx(
            system_address,
            Some(eezl2_address),
            system_address,
            eezl2_address,
        ));
        assert!(!is_system_tx(
            address!("3333333333333333333333333333333333333333"),
            Some(eezl2_address),
            system_address,
            eezl2_address,
        ));
        assert!(!is_system_tx(
            system_address,
            Some(address!("4444444444444444444444444444444444444444")),
            system_address,
            eezl2_address,
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
        actions[0].revert_span = Some(3);

        assert_eq!(
            l1_applied_root_mask(&actions).unwrap(),
            [false, false, false, true]
        );
    }

    #[test]
    fn applied_root_mask_rejects_out_of_bounds_source_revert_span() {
        let mut actions = [action(None, CallMode::Mutable, true)];
        actions[0].revert_span = Some(2);

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
}
