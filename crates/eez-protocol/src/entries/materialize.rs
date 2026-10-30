//! Execution-tree materialization for mutable nested cross-chain calls.
//!
//! The implementation mirrors `eez-core-protocol`'s `TableGenerator`: one
//! entry context is active per executing chain, destination-side calls are
//! appended to that context's current flat array, and calls leaving an active
//! chain open rows in its unified reentrant table. Rows are pushed before
//! their children, preserving the protocol's strict forward-cursor layout.

use std::collections::HashMap;

use alloy_primitives::{B256, Bytes, U256, keccak256};
use alloy_sol_types::SolValue as _;

use crate::abi::{
    ExecutionEntrySol, ExpectedL1ToL2CallSol, L2StaticExecutionEntrySol, StaticExecutionEntrySol,
};
use crate::action::{CallHashInput, common_cross_chain_call_hash, l2_outbound_call_hash};
use crate::rolling_hash::{EntryRollingHash, StaticCallRollingHash};
use crate::system_tx::lower_call_to_l2;
use crate::{CallMode, EvmBatch, ExecutedAction, ProtocolErrorKind, ProtocolResult, RollupId};

use super::l1_call_from_action;

/// Symbolic execution trace for one mutable entry.
///
/// L2 entries are finalized immediately from their state-independent seed.
/// L1 entries retain this trace until settlement attaches the ordered
/// `RootUpdate`s whose current roots form the entry seed.
#[derive(Clone, Debug, Default)]
pub struct EntryTrace {
    events: Vec<TraceEvent>,
}

#[derive(Clone, Debug)]
enum TraceEvent {
    CallBegin(B256),
    CallEnd { success: bool, return_data: Bytes },
    NestedBegin { row_index: usize, call_hash: B256 },
    NestedEnd,
    NestedRevert { row_index: usize },
    StaticLookup { row_index: usize, call_hash: B256 },
    RegionBegin,
    RegionRestore,
}

/// Construction-only positions for choosing the first reusable row. L1 roots
/// are not attached yet, so these keys never authorize execution: finalization
/// recomputes every position from the actual root-bound seed and checks that
/// the selected row is still the first matching candidate.
struct ConstructionPosition {
    rolling: EntryRollingHash,
    nested: Vec<EntryRollingHash>,
    regions: Vec<EntryRollingHash>,
}

impl ConstructionPosition {
    fn new(proxy_hash: B256) -> Self {
        Self {
            rolling: EntryRollingHash::seed_for_l2(proxy_hash),
            nested: Vec::new(),
            regions: Vec::new(),
        }
    }

    fn apply(&mut self, event: &TraceEvent) {
        // These events come only from the validated forest walk. Every close
        // is emitted by the same frame/scope that emitted its matching open.
        match event {
            TraceEvent::CallBegin(hash) => self.rolling.call_begin(*hash),
            TraceEvent::CallEnd {
                success,
                return_data,
            } => self.rolling.call_end(*success, return_data),
            TraceEvent::NestedBegin { call_hash, .. } => {
                self.nested.push(self.rolling.clone());
                self.rolling.nested_begin(*call_hash);
            }
            TraceEvent::NestedEnd => {
                self.nested
                    .pop()
                    .expect("forest frame opened before closing");
                self.rolling.nested_end();
            }
            TraceEvent::NestedRevert { .. } => {
                self.rolling = self
                    .nested
                    .pop()
                    .expect("forest frame opened before reverting");
            }
            TraceEvent::StaticLookup { .. } => {}
            TraceEvent::RegionBegin => self.regions.push(self.rolling.clone()),
            TraceEvent::RegionRestore => {
                self.rolling = self
                    .regions
                    .pop()
                    .expect("forest scope opened before restoring");
            }
        }
    }
}

/// One chain's ABI entries and their parallel symbolic traces.
#[derive(Debug)]
pub(crate) struct MaterializedBatch {
    pub(crate) batch: EvmBatch,
    pub(crate) l2_static_entries: Vec<L2StaticExecutionEntrySol>,
    pub(crate) entry_traces: Vec<EntryTrace>,
    /// Prefix of `batch.entries` belonging to the transaction's entry side.
    ///
    /// For an L2-origin transaction these are the outbound entries consumed by
    /// the user transaction. Calls returning from L1 to the source L2 stay in
    /// those entries' incoming-call arrays so they execute while the outbound
    /// frame is still open. L1 origins have no same-chain target split, so all
    /// of their entries are source entries.
    pub(crate) source_entry_count: usize,
    /// L2-origin proxy calls whose frames committed and must retain an event.
    pub(crate) source_outbound_hashes: Vec<B256>,
}

/// Materialize every chain touched by one source transaction.
///
/// Failed mutable frames retain their descendant execution before restoring
/// their own state/hash; the caller's resolver frame reverts naturally.
/// Nested source-frame revert spans are lowered to per-destination-array
/// `revertNextNCalls` markers. An L1 root covered by a root-level revert is a
/// lookup only: its source-visible result and callback table are retained,
/// but no canonical L2 delivery is materialized. The executing source chain restores its own rolling-hash
/// branch for nested regions, while every other chain preserves its execution
/// folds and rolls back state through the protocol marker.
/// Static roots and nested reads leave their source's tagged hash unchanged.
/// Each pending read owns an untagged callback fold, whether its lookup resides
/// in a pool or a mutable host row. Table construction alone does not authorize
/// a lookup-only result: the signer must independently replay its execution.
pub(crate) fn materialize_batches(
    recorded: &[ExecutedAction],
    entry_rollup_id: RollupId,
) -> ProtocolResult<HashMap<RollupId, MaterializedBatch>> {
    let forest = CallForest::new(recorded, entry_rollup_id)?;
    let l2_plan = (!entry_rollup_id.is_mainnet())
        .then(|| L2RootPlan::build(recorded, &forest))
        .transpose()?;
    let mut materializer = Materializer::new(
        recorded,
        forest.children,
        forest.subtree_ends,
        forest.regions,
    );

    let source_entry_count;
    if entry_rollup_id.is_mainnet() {
        let applied_roots = crate::settlement::l1_applied_root_mask(recorded)
            .map_err(ProtocolErrorKind::InvalidEncoding)?;
        source_entry_count = forest
            .roots
            .iter()
            .filter(|&&root| recorded[root].call_mode == CallMode::Mutable)
            .count();
        for root in forest.roots {
            if recorded[root].call_mode == CallMode::Static {
                materializer.execute_static(root, entry_rollup_id, 0)?;
                continue;
            }
            let call = recorded[root].clone();
            let entry_index = materializer.open_entry(
                entry_rollup_id,
                source_call_hash(&call),
                call.target_rollup_id,
                &call,
            );
            if applied_roots[root] {
                materializer.begin_region(root);
                materializer.execute_call(root)?;
                materializer.close_region_at_subtree(root)?;
            } else {
                materializer.execute_lookup_call(root)?;
            }
            materializer.close_entry(entry_rollup_id, entry_index);
        }
    } else {
        // Each source-table cursor slot has one L2 entry. A return from L1 to
        // L2 is executed from that same entry's incoming-call array while the
        // original outbound frame is open; it is not a later synthetic
        // delivery. Every root in the one L2 user transaction shares one
        // immediate L1 L2Tx host. A reverted source frame restores its source
        // cursor, so a later retry reuses the same source entry while every L1
        // attempt remains in the host's flat call array.
        let plan = l2_plan.expect("non-mainnet entry has an L2 root plan");
        source_entry_count = plan.slot_roots.len();
        for &root in &plan.slot_roots {
            let call = recorded[root].clone();
            materializer.push_entry(
                entry_rollup_id,
                source_call_hash(&call),
                call.target_rollup_id,
                &call,
            );
        }
        let host_index = materializer.open_host_entry(RollupId::MAINNET, entry_rollup_id);
        for assignment in plan.assignments {
            materializer.origin_cursor = assignment.entry_cursor;
            materializer.begin_region(assignment.root);
            if let Some(source_slot) = assignment.source_slot {
                materializer.install_context(entry_rollup_id, source_slot);
            }
            if recorded[assignment.root].call_mode == CallMode::Static {
                materializer.execute_static(
                    assignment.root,
                    entry_rollup_id,
                    assignment.entry_cursor,
                )?;
            } else {
                materializer.execute_call(assignment.root)?;
            }
            if let Some(source_slot) = assignment.source_slot {
                materializer.close_entry(entry_rollup_id, source_slot);
            }
            materializer.close_region_at_subtree(assignment.root)?;
        }
        materializer.close_entry(RollupId::MAINNET, host_index);
    }

    materializer.finish(entry_rollup_id, source_entry_count)
}

/// Build a noncanonical destination execution for one mutable L1 root.
/// The enclosing source rollback is replayed by the observation scheduler;
/// child-frame rollbacks remain part of the table. The delivery completes only
/// after its rolling hash is checked, even when the actual target frame fails.
pub(crate) fn materialize_lookup_execution(
    recorded: &[ExecutedAction],
    root_index: usize,
) -> ProtocolResult<crate::abi::L2ExecutionEntrySol> {
    let invalid = |reason: &str| {
        crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(reason.into()))
    };
    let root = recorded
        .get(root_index)
        .ok_or_else(|| invalid("lookup root is missing"))?;
    if root.parent_index.is_some()
        || !root.source_rollup_id.is_mainnet()
        || root.target_rollup_id.is_mainnet()
        || root.call_mode != CallMode::Mutable
    {
        return Err(invalid("lookup execution requires a mutable L1 root"));
    }
    let destination = root.target_rollup_id;
    let end = recorded
        .iter()
        .enumerate()
        .skip(root_index + 1)
        .find(|(_, action)| action.parent_index.is_none())
        .map_or(recorded.len(), |(index, _)| index);
    let mut calls = recorded[root_index..end].to_vec();
    for call in &mut calls {
        call.parent_index = call
            .parent_index
            .map(|parent| {
                parent
                    .checked_sub(root_index)
                    .ok_or_else(|| invalid("lookup child escapes its root"))
            })
            .transpose()?;
    }
    calls[0].revert_spans.clear();
    let forest = CallForest::new(&calls, RollupId::MAINNET)?;
    let mut materializer =
        Materializer::new(&calls, forest.children, forest.subtree_ends, forest.regions);
    let source_entry = materializer.open_entry(
        RollupId::MAINNET,
        source_call_hash(&calls[0]),
        destination,
        &calls[0],
    );
    materializer.execute_call(0)?;
    materializer.close_entry(RollupId::MAINNET, source_entry);
    let mut outputs = materializer.finish(RollupId::MAINNET, 1)?;
    outputs.remove(&RollupId::MAINNET);
    let target = outputs
        .remove(&destination)
        .ok_or_else(|| invalid("lookup has no destination execution"))?;
    if !outputs.is_empty()
        || target.batch.entries.len() != 1
        || !target.l2_static_entries.is_empty()
    {
        return Err(invalid(
            "lookup execution is not one complete destination table",
        ));
    }
    Ok(crate::system_tx::lower_entry_to_l2(
        &target.batch.entries[0],
    ))
}

/// Finalize nested position keys and the rolling hash for L1 entries after
/// settlement has attached the exact ordered root updates.
pub fn finalize_l1_entry_traces(batch: &mut EvmBatch, traces: &[EntryTrace]) -> ProtocolResult<()> {
    if batch.entries.len() != traces.len() {
        return Err(ProtocolErrorKind::InvalidEncoding(format!(
            "L1 entry/trace count mismatch: {} entries, {} traces",
            batch.entries.len(),
            traces.len(),
        ))
        .into());
    }
    for (entry_index, (entry, trace)) in batch.entries.iter_mut().zip(traces).enumerate() {
        if entry.rootUpdates.is_empty() {
            return Err(ProtocolErrorKind::InvalidEncoding(format!(
                "L1 entry {entry_index} has no RootUpdates"
            ))
            .into());
        }
        finalize_trace(
            entry,
            trace,
            EntryRollingHash::seed_for_l1(
                entry
                    .rootUpdates
                    .iter()
                    .map(|update| (update.rollupId, update.currentRoot)),
                entry.proxyEntryHash,
            ),
        )?;
    }
    Ok(())
}

fn finalize_trace(
    entry: &mut ExecutionEntrySol,
    trace: &EntryTrace,
    mut rolling_hash: EntryRollingHash,
) -> ProtocolResult<()> {
    for row in &mut entry.expectedL1ToL2Calls {
        row.expectedL1toL2Hash = B256::ZERO;
    }
    let mut next_row = 0usize;
    let mut seen_rows = vec![false; entry.expectedL1ToL2Calls.len()];
    let mut reverted_hashes = vec![None; entry.expectedL1ToL2Calls.len()];
    let mut nested_frames: Vec<(usize, EntryRollingHash, usize)> = Vec::new();
    let mut region_state: Vec<(EntryRollingHash, usize)> = Vec::new();
    for event in &trace.events {
        match event {
            TraceEvent::CallBegin(call_hash) => rolling_hash.call_begin(*call_hash),
            TraceEvent::CallEnd {
                success,
                return_data,
            } => rolling_hash.call_end(*success, return_data),
            TraceEvent::NestedBegin {
                row_index,
                call_hash,
            } => {
                let position_key = nested_position_key(*call_hash, rolling_hash.current());
                if *row_index >= entry.expectedL1ToL2Calls.len()
                    || *row_index < next_row
                    || entry.expectedL1ToL2Calls[next_row..*row_index]
                        .iter()
                        .any(|row| row.expectedL1toL2Hash == position_key)
                {
                    return Err(ProtocolErrorKind::InvalidEncoding(format!(
                        "reentrant trace row {row_index} is not forward ordered at {next_row}"
                    ))
                    .into());
                }
                let row = entry
                    .expectedL1ToL2Calls
                    .get_mut(*row_index)
                    .ok_or_else(|| {
                        crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(format!(
                            "reentrant trace references missing row {row_index}"
                        )))
                    })?;
                seen_rows[*row_index] = true;
                if row.expectedL1toL2Hash != B256::ZERO && row.expectedL1toL2Hash != position_key {
                    return Err(ProtocolErrorKind::InvalidEncoding(format!(
                        "reused reentrant row {row_index} has inconsistent position keys"
                    ))
                    .into());
                }
                row.expectedL1toL2Hash = position_key;
                nested_frames.push((*row_index, rolling_hash.clone(), next_row));
                rolling_hash.nested_begin(*call_hash);
                next_row = *row_index + 1;
            }
            TraceEvent::NestedEnd => {
                nested_frames.pop().ok_or_else(|| {
                    crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                        "nested end has no open frame".into(),
                    ))
                })?;
                rolling_hash.nested_end();
            }
            TraceEvent::NestedRevert { row_index } => {
                let (open_row, saved_hash, saved_cursor) =
                    nested_frames.pop().ok_or_else(|| {
                        crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                            "nested revert has no open frame".into(),
                        ))
                    })?;
                if open_row != *row_index {
                    return Err(ProtocolErrorKind::InvalidEncoding(format!(
                        "nested revert closes row {row_index}, expected {open_row}"
                    ))
                    .into());
                }
                let row = entry
                    .expectedL1ToL2Calls
                    .get_mut(*row_index)
                    .ok_or_else(|| {
                        crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(format!(
                            "nested revert references missing row {row_index}"
                        )))
                    })?;
                if row.success {
                    return Err(ProtocolErrorKind::InvalidEncoding(format!(
                        "nested revert row {row_index} is marked successful"
                    ))
                    .into());
                }
                let hash = rolling_hash.current();
                if reverted_hashes[*row_index].is_some_and(|previous| previous != hash) {
                    return Err(ProtocolErrorKind::InvalidEncoding(format!(
                        "reused reverted row {row_index} has a different execution commitment"
                    ))
                    .into());
                }
                reverted_hashes[*row_index] = Some(hash);
                row.revertedOrStaticRollingHash = hash;
                rolling_hash = saved_hash;
                next_row = saved_cursor;
            }
            TraceEvent::StaticLookup {
                row_index,
                call_hash,
            } => {
                if *row_index < next_row {
                    return Err(ProtocolErrorKind::InvalidEncoding(format!(
                        "static trace row {row_index} is not forward ordered at {next_row}"
                    ))
                    .into());
                }
                let row = entry
                    .expectedL1ToL2Calls
                    .get_mut(*row_index)
                    .ok_or_else(|| {
                        crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(format!(
                            "static trace references missing row {row_index}"
                        )))
                    })?;
                seen_rows[*row_index] = true;
                let position_key = nested_position_key(*call_hash, rolling_hash.current());
                if row.expectedL1toL2Hash != B256::ZERO && row.expectedL1toL2Hash != position_key {
                    return Err(ProtocolErrorKind::InvalidEncoding(format!(
                        "reused static row {row_index} has inconsistent position keys"
                    ))
                    .into());
                }
                row.expectedL1toL2Hash = position_key;
            }
            TraceEvent::RegionBegin => {
                region_state.push((rolling_hash.clone(), next_row));
            }
            TraceEvent::RegionRestore => {
                let (saved_hash, saved_cursor) = region_state.pop().ok_or_else(|| {
                    crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                        "revert region restore has no saved state".into(),
                    ))
                })?;
                rolling_hash = saved_hash;
                next_row = saved_cursor;
            }
        }
    }
    if !nested_frames.is_empty() {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "entry trace ended with open nested frames".into(),
        )
        .into());
    }
    if !region_state.is_empty() {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "entry trace ended with an open revert region".into(),
        )
        .into());
    }
    if seen_rows.iter().any(|seen| !seen) {
        return Err(ProtocolErrorKind::InvalidEncoding(format!(
            "entry trace did not exercise every reentrant row: cursor={next_row}, seen_rows={seen_rows:?}",
        ))
        .into());
    }
    entry.rollingHash = rolling_hash.current();
    Ok(())
}

fn nested_position_key(call_hash: B256, rolling_hash: B256) -> B256 {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(call_hash.as_slice());
    input[32..].copy_from_slice(rolling_hash.as_slice());
    keccak256(input)
}

struct CallForest {
    roots: Vec<usize>,
    children: Vec<Vec<usize>>,
    subtree_ends: Vec<usize>,
    regions: HashMap<usize, Vec<RevertRegion>>,
}

#[derive(Clone, Debug)]
struct RevertRegion {
    end: usize,
    parent_index: Option<usize>,
    host_chain: RollupId,
}

#[derive(Clone, Copy, Debug)]
struct L2RootAssignment {
    root: usize,
    source_slot: Option<usize>,
    entry_cursor: usize,
}

#[derive(Debug)]
struct L2RootPlan {
    assignments: Vec<L2RootAssignment>,
    slot_roots: Vec<usize>,
}

impl L2RootPlan {
    fn build(recorded: &[ExecutedAction], forest: &CallForest) -> ProtocolResult<Self> {
        let mut assignments = Vec::with_capacity(forest.roots.len());
        let mut slot_roots: Vec<usize> = Vec::new();
        let mut cursor = 0usize;
        let mut active_regions = Vec::new();

        for &root in &forest.roots {
            for region in forest.regions.get(&root).into_iter().flatten() {
                if region.parent_index.is_none() {
                    active_regions.push((region.end, cursor));
                }
            }
            let entry_cursor = cursor;
            let source_slot = if recorded[root].call_mode == CallMode::Static {
                None
            } else {
                let current = &recorded[root];
                // A restored cursor scans forward for the first matching key.
                // A different call after rollback can append another candidate.
                let existing = slot_roots
                    .iter()
                    .enumerate()
                    .skip(cursor)
                    .find(|(_, previous)| {
                        source_call_hash(&recorded[**previous]) == source_call_hash(current)
                    })
                    .map(|(slot, _)| slot);
                let slot = if let Some(slot) = existing {
                    let previous = &recorded[slot_roots[slot]];
                    if previous.outcome.is_success() != current.outcome.is_success()
                        || previous.outcome.return_data() != current.outcome.return_data()
                    {
                        return Err(ProtocolErrorKind::InvalidEncoding(format!(
                            "L2 source cursor slot {slot} is reused with a different outcome"
                        ))
                        .into());
                    }
                    slot
                } else {
                    slot_roots.push(root);
                    slot_roots.len() - 1
                };
                // A failed source entry reverts its cursor advance in EEZL2.
                cursor = if current.outcome.is_success() {
                    slot + 1
                } else {
                    entry_cursor
                };
                Some(slot)
            };
            assignments.push(L2RootAssignment {
                root,
                source_slot,
                entry_cursor,
            });
            while active_regions
                .last()
                .is_some_and(|(end, _)| *end == forest.subtree_ends[root])
            {
                cursor = active_regions.pop().expect("scope checked above").1;
            }
        }
        if !active_regions.is_empty() {
            return Err(ProtocolErrorKind::InvalidEncoding(
                "L2 source cursor ended with an open root-level revert region".into(),
            )
            .into());
        }

        Ok(Self {
            assignments,
            slot_roots,
        })
    }
}

impl CallForest {
    fn new(recorded: &[ExecutedAction], entry_rollup_id: RollupId) -> ProtocolResult<Self> {
        super::ensure_materializable_calls(recorded)?;
        let mut roots = Vec::new();
        let mut children = vec![Vec::new(); recorded.len()];
        let mut depths = vec![0usize; recorded.len()];
        for (index, call) in recorded.iter().enumerate() {
            match call.parent_index {
                None => {
                    if call.source_rollup_id != entry_rollup_id {
                        return Err(ProtocolErrorKind::InvalidEncoding(format!(
                            "root call {index} originates on {}, expected entry rollup {entry_rollup_id}",
                            call.source_rollup_id,
                        ))
                        .into());
                    }
                    roots.push(index);
                }
                Some(parent) => {
                    let parent_call = recorded.get(parent).ok_or_else(|| {
                        crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(format!(
                            "call {index} references missing parent {parent}"
                        )))
                    })?;
                    if parent >= index {
                        return Err(ProtocolErrorKind::InvalidEncoding(format!(
                            "call {index} parent {parent} is not earlier in preorder"
                        ))
                        .into());
                    }
                    depths[index] = depths[parent] + 1;
                    if depths[index] > super::MAX_NESTING_DEPTH {
                        return Err(ProtocolErrorKind::Unsupported(
                            "nested call depth exceeds the materializer limit",
                        )
                        .into());
                    }
                    if parent_call.target_rollup_id != call.source_rollup_id {
                        return Err(ProtocolErrorKind::InvalidEncoding(format!(
                            "call {index} source {} does not match parent {parent} destination {}",
                            call.source_rollup_id, parent_call.target_rollup_id,
                        ))
                        .into());
                    }
                    children[parent].push(index);
                }
            }
        }

        let mut traversal = Vec::with_capacity(recorded.len());
        let mut pending = roots.iter().rev().copied().collect::<Vec<_>>();
        while let Some(index) = pending.pop() {
            traversal.push(index);
            pending.extend(children[index].iter().rev().copied());
        }
        if traversal.iter().copied().ne(0..recorded.len()) {
            return Err(ProtocolErrorKind::InvalidEncoding(
                "recorded parent links do not describe a contiguous preorder traversal".into(),
            )
            .into());
        }

        let mut subtree_ends = (1..=recorded.len()).collect::<Vec<_>>();
        for index in (0..recorded.len()).rev() {
            if let Some(last_child) = children[index].last() {
                subtree_ends[index] = subtree_ends[*last_child];
            }
        }

        let mut regions = HashMap::new();
        let mut region_ends = Vec::new();
        for (index, call) in recorded.iter().enumerate() {
            while region_ends.last().is_some_and(|end| *end <= index) {
                region_ends.pop();
            }
            for &span in &call.revert_spans {
                if span == 0 {
                    return Err(ProtocolErrorKind::InvalidEncoding(format!(
                        "call {index} has an empty revert span"
                    ))
                    .into());
                }
                let end = index.checked_add(span as usize).ok_or_else(|| {
                    crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(format!(
                        "call {index} revert span overflows"
                    )))
                })?;
                if end > recorded.len() {
                    return Err(ProtocolErrorKind::InvalidEncoding(format!(
                        "call {index} revert span ends at {end}, beyond {} calls",
                        recorded.len()
                    ))
                    .into());
                }
                if region_ends.last().is_some_and(|outer| end > *outer) {
                    return Err(ProtocolErrorKind::Unsupported(
                        "crossing rollback scopes are not supported",
                    )
                    .into());
                }

                // A source EVM frame can cover one or more complete sibling call
                // subtrees. Reject a span that cuts through a subtree or changes
                // parent level; such a range cannot be represented by the
                // protocol's contiguous per-array rollback windows.
                let parent = call.parent_index;
                let mut cursor = index;
                while cursor < end {
                    if recorded[cursor].parent_index != parent {
                        return Err(ProtocolErrorKind::InvalidEncoding(format!(
                            "call {index} revert span does not cover complete sibling subtrees"
                        ))
                        .into());
                    }
                    cursor = subtree_ends[cursor];
                }
                if cursor != end {
                    return Err(ProtocolErrorKind::InvalidEncoding(format!(
                        "call {index} revert span ends inside a nested call subtree"
                    ))
                    .into());
                }

                regions
                    .entry(index)
                    .or_insert_with(Vec::new)
                    .push(RevertRegion {
                        end,
                        parent_index: parent,
                        host_chain: call.source_rollup_id,
                    });
                region_ends.push(end);
            }
        }

        Ok(Self {
            roots,
            children,
            subtree_ends,
            regions,
        })
    }
}

#[derive(Debug)]
struct ActiveContext {
    entry_index: usize,
    frame_rows: Vec<usize>,
    next_row: usize,
}

#[derive(Default)]
struct ChainOutput {
    entries: Vec<ExecutionEntrySol>,
    static_entries: Vec<StaticExecutionEntrySol>,
    l2_static_entries: Vec<L2StaticExecutionEntrySol>,
    traces: Vec<EntryTrace>,
    positions: Vec<ConstructionPosition>,
    immediate_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CallArrayLocation {
    chain: RollupId,
    entry_index: usize,
    frame_row: Option<usize>,
}

struct ActiveRegion {
    end: usize,
    other_natural: Option<RollupId>,
    parent_index: Option<usize>,
    host_chain: RollupId,
    branched: bool,
    saved_row_cursor: Option<usize>,
    saved_outbound_hash_count: usize,
    first_calls: HashMap<CallArrayLocation, (usize, usize)>,
}

struct Materializer<'a> {
    recorded: &'a [ExecutedAction],
    children: Vec<Vec<usize>>,
    subtree_ends: Vec<usize>,
    regions: HashMap<usize, Vec<RevertRegion>>,
    outputs: HashMap<RollupId, ChainOutput>,
    active: HashMap<RollupId, ActiveContext>,
    active_regions: Vec<ActiveRegion>,
    source_outbound_hashes: Vec<B256>,
    static_frames: HashMap<RollupId, Vec<(StaticFrame, StaticCallRollingHash)>>,
    origin_cursor: usize,
}

/// The innermost pending read owns callbacks on its source chain. A pool read
/// has no mutable context; a row read remains pinned to its host's live hash.
enum StaticFrame {
    L1Pool(usize),
    L2Pool(usize),
    Row { entry: usize, row: usize },
}

impl<'a> Materializer<'a> {
    fn new(
        recorded: &'a [ExecutedAction],
        children: Vec<Vec<usize>>,
        subtree_ends: Vec<usize>,
        regions: HashMap<usize, Vec<RevertRegion>>,
    ) -> Self {
        Self {
            recorded,
            children,
            subtree_ends,
            regions,
            outputs: HashMap::new(),
            active: HashMap::new(),
            active_regions: Vec::new(),
            source_outbound_hashes: Vec::new(),
            static_frames: HashMap::new(),
            origin_cursor: 0,
        }
    }

    fn begin_region(&mut self, node_index: usize) {
        for region in self.regions.get(&node_index).cloned().unwrap_or_default() {
            self.open_region(region, None);
        }
    }

    fn open_region(&mut self, region: RevertRegion, other_natural: Option<RollupId>) {
        let branched =
            region.parent_index.is_some() && self.active.contains_key(&region.host_chain);
        let saved_row_cursor = branched.then(|| self.active[&region.host_chain].next_row);
        if branched {
            self.push_event(region.host_chain, TraceEvent::RegionBegin);
        }
        self.active_regions.push(ActiveRegion {
            end: region.end,
            parent_index: region.parent_index,
            host_chain: region.host_chain,
            other_natural,
            branched,
            saved_row_cursor,
            saved_outbound_hash_count: self.source_outbound_hashes.len(),
            first_calls: HashMap::new(),
        });
    }

    fn close_region_at_subtree(&mut self, node_index: usize) -> ProtocolResult<()> {
        let boundary = self.subtree_ends[node_index];
        // A last descendant shares its ancestor's end index. Only the scope's
        // own sibling level may close it, after that child's NESTED_END fold.
        while self.active_regions.last().is_some_and(|region| {
            region.end <= boundary && region.parent_index == self.recorded[node_index].parent_index
        }) {
            let region = self.active_regions.pop().expect("region checked above");
            if region.end != boundary {
                return Err(ProtocolErrorKind::InvalidEncoding(format!(
                    "revert region ending at {} crossed subtree boundary {boundary}",
                    region.end,
                ))
                .into());
            }
            // Match TableGenerator._closeRollback: inner scopes assign first;
            // an outer scope skips an already reverted prefix at its start.
            for (location, (mut first, count)) in region.first_calls {
                let end = first + count;
                while first < end {
                    let marker = usize::from(self.call_at_mut(&location, first).revertNextNCalls);
                    if marker == 0 {
                        break;
                    }
                    first += marker;
                }
                if first > end {
                    return Err(ProtocolErrorKind::InvalidEncoding(
                        "inner rollback crosses its outer scope".into(),
                    )
                    .into());
                }
                if first < end {
                    self.call_at_mut(&location, first).revertNextNCalls =
                        u16::try_from(end - first).map_err(|_| {
                            ProtocolErrorKind::Unsupported(
                                "revert region exceeds the uint16 per-array call limit",
                            )
                        })?;
                }
            }
            self.source_outbound_hashes
                .truncate(region.saved_outbound_hash_count);
            if region.branched {
                self.active
                    .get_mut(&region.host_chain)
                    .expect("branched region host is active")
                    .next_row = region
                    .saved_row_cursor
                    .expect("branched region saved its row cursor");
                self.push_event(region.host_chain, TraceEvent::RegionRestore);
            }
        }
        Ok(())
    }

    fn open_host_entry(&mut self, chain: RollupId, destination: RollupId) -> usize {
        let output = self.outputs.entry(chain).or_default();
        let entry_index = output.entries.len();
        output.entries.push(ExecutionEntrySol {
            destinationRollupId: destination.0,
            success: true,
            ..Default::default()
        });
        output.traces.push(EntryTrace::default());
        output.positions.push(ConstructionPosition::new(B256::ZERO));
        output.immediate_count += 1;
        self.install_context(chain, entry_index);
        entry_index
    }

    fn open_entry(
        &mut self,
        chain: RollupId,
        proxy_entry_hash: B256,
        destination: RollupId,
        outer: &ExecutedAction,
    ) -> usize {
        let entry_index = self.push_entry(chain, proxy_entry_hash, destination, outer);
        self.outputs
            .get_mut(&chain)
            .expect("chain output")
            .immediate_count += 1;
        self.install_context(chain, entry_index);
        entry_index
    }

    /// Open an L2 entry used by `executeIncomingCrossChainCall`.
    ///
    /// A failed destination call and a failed delivery are different things.
    /// The former is folded into this entry's rolling hash and is replayed by
    /// the source-chain entry.  The delivery itself must still commit, or the
    /// system transaction would discard the verified call trace.  Successful
    /// calls keep their pre-computed return value; failed calls have no normal
    /// return value on the destination side.
    fn open_delivery_entry(
        &mut self,
        chain: RollupId,
        proxy_entry_hash: B256,
        destination: RollupId,
        outer: &ExecutedAction,
    ) -> usize {
        let entry_index = self.push_entry(chain, proxy_entry_hash, destination, outer);
        self.outputs
            .get_mut(&chain)
            .expect("delivery chain output")
            .immediate_count += 1;
        if !outer.outcome.is_success() {
            let entry = &mut self
                .outputs
                .get_mut(&chain)
                .expect("delivery chain output")
                .entries[entry_index];
            entry.success = true;
            entry.returnData = Bytes::new();
        }
        self.install_context(chain, entry_index);
        entry_index
    }

    fn push_entry(
        &mut self,
        chain: RollupId,
        proxy_entry_hash: B256,
        destination: RollupId,
        outer: &ExecutedAction,
    ) -> usize {
        let output = self.outputs.entry(chain).or_default();
        let entry_index = output.entries.len();
        output.entries.push(ExecutionEntrySol {
            proxyEntryHash: proxy_entry_hash,
            destinationRollupId: destination.0,
            success: outer.outcome.is_success(),
            returnData: Bytes::copy_from_slice(
                outer
                    .outcome
                    .return_data()
                    .expect("materializable outcome is resolved"),
            ),
            ..Default::default()
        });
        output.traces.push(EntryTrace::default());
        output
            .positions
            .push(ConstructionPosition::new(proxy_entry_hash));
        entry_index
    }

    /// Mirror the core's static-frame stack without opening mutable delivery
    /// contexts. Callback hashes belong to the innermost pending read, while
    /// only calls executed directly by a mutable host affect its tagged hash.
    fn execute_static(
        &mut self,
        node: usize,
        origin: RollupId,
        cursor: usize,
    ) -> ProtocolResult<()> {
        let call = self.recorded[node].clone();
        let source = call.source_rollup_id;
        let destination = call.target_rollup_id;
        let output = Bytes::copy_from_slice(call.outcome.return_data().expect("resolved outcome"));
        let location = if let Some(context) = self.active.get(&source) {
            let entry = context.entry_index;
            let (row, _) = self.push_row(source, &call)?;
            self.push_event(
                source,
                TraceEvent::StaticLookup {
                    row_index: row,
                    call_hash: source_call_hash(&call),
                },
            );
            Some(StaticFrame::Row { entry, row })
        } else if source == origin {
            let table = self.outputs.entry(source).or_default();
            if source.is_mainnet() {
                let index = table.static_entries.len();
                table.static_entries.push(StaticExecutionEntrySol {
                    proxyEntryHash: source_call_hash(&call),
                    destinationRollupId: destination.0,
                    success: call.outcome.is_success(),
                    returnData: output.clone(),
                    ..Default::default()
                });
                Some(StaticFrame::L1Pool(index))
            } else {
                let index = table.l2_static_entries.len();
                table.l2_static_entries.push(L2StaticExecutionEntrySol {
                    expectedEntryIndex: U256::from(cursor),
                    proxyEntryHash: source_call_hash(&call),
                    success: call.outcome.is_success(),
                    returnData: output.clone(),
                    ..Default::default()
                });
                Some(StaticFrame::L2Pool(index))
            }
        } else {
            None
        };
        let has_lookup = location.is_some();
        if let Some(location) = location {
            self.static_frames
                .entry(source)
                .or_default()
                .push((location, StaticCallRollingHash::new()));
        }
        let callback = self
            .static_frames
            .get_mut(&destination)
            .and_then(|frames| frames.last_mut());
        let direct = callback.is_none() && self.active.contains_key(&destination);
        if let Some((location, hash)) = callback {
            let table = self
                .outputs
                .get_mut(&destination)
                .expect("static frame has a table");
            let encoded = l1_call_from_action(&call);
            match *location {
                StaticFrame::L1Pool(index) => table.static_entries[index].l2ToL1Calls.push(encoded),
                StaticFrame::L2Pool(index) => table.l2_static_entries[index]
                    .incomingCalls
                    .push(lower_call_to_l2(&encoded)),
                StaticFrame::Row { entry, row } => table.entries[entry].expectedL1ToL2Calls[row]
                    .l2ToL1Calls
                    .push(encoded),
            }
            hash.append(call.outcome.is_success(), &output);
        } else if direct {
            self.append_flat_call(destination, &call);
            self.push_event(
                destination,
                TraceEvent::CallBegin(destination_call_hash(&call)),
            );
        }
        for child in self.children[node].clone() {
            self.execute_static(child, origin, cursor)?;
        }
        if direct {
            self.push_event(
                destination,
                TraceEvent::CallEnd {
                    success: call.outcome.is_success(),
                    return_data: output,
                },
            );
        }
        if has_lookup {
            let (location, hash) = self
                .static_frames
                .get_mut(&source)
                .and_then(Vec::pop)
                .expect("this read opened a static frame");
            let table = self
                .outputs
                .get_mut(&source)
                .expect("static frame has a table");
            match location {
                StaticFrame::L1Pool(index) => {
                    table.static_entries[index].rollingHash = hash.current()
                }
                StaticFrame::L2Pool(index) => {
                    table.l2_static_entries[index].rollingHash = hash.current()
                }
                StaticFrame::Row { entry, row } => {
                    table.entries[entry].expectedL1ToL2Calls[row].revertedOrStaticRollingHash =
                        hash.current()
                }
            }
        }
        Ok(())
    }

    fn install_context(&mut self, chain: RollupId, entry_index: usize) {
        let previous = self.active.insert(
            chain,
            ActiveContext {
                entry_index,
                frame_rows: Vec::new(),
                next_row: 0,
            },
        );
        debug_assert!(previous.is_none(), "one active entry per chain");
    }

    fn close_entry(&mut self, chain: RollupId, entry_index: usize) {
        let context = self.active.remove(&chain).expect("active entry context");
        debug_assert_eq!(context.entry_index, entry_index);
        debug_assert!(context.frame_rows.is_empty());
    }

    /// A lookup-only L1 root still executes its source-side callbacks when
    /// the manager resolves it. Build that complete source table, while keeping
    /// its destination execution out of canonical delivery outputs. Witness
    /// replay must independently validate the discarded destination execution.
    fn execute_lookup_call(&mut self, node_index: usize) -> ProtocolResult<()> {
        debug_assert_eq!(self.active.len(), 1, "lookup begins at an L1 root boundary");
        debug_assert!(self.active.contains_key(&RollupId::MAINNET));
        let canonical: HashMap<_, _> = self
            .outputs
            .extract_if(|chain, _| !chain.is_mainnet())
            .collect();
        let hashes = self.source_outbound_hashes.len();
        self.execute_call(node_index)?;
        self.outputs.retain(|chain, _| chain.is_mainnet());
        self.outputs.extend(canonical);
        self.source_outbound_hashes.truncate(hashes);
        Ok(())
    }

    fn execute_call(&mut self, node_index: usize) -> ProtocolResult<()> {
        let call = self.recorded[node_index].clone();
        let destination = call.target_rollup_id;
        if call.call_mode == CallMode::Mutable
            && !call.source_rollup_id.is_mainnet()
            && destination.is_mainnet()
            && call.outcome.is_success()
        {
            self.source_outbound_hashes.push(source_call_hash(&call));
        }
        let opened = if self.active.contains_key(&destination) {
            None
        } else {
            if destination.is_mainnet() {
                return Err(ProtocolErrorKind::InvalidEncoding(format!(
                    "call {node_index} reaches L1 without an active L1 host entry"
                ))
                .into());
            }
            Some(self.open_delivery_entry(
                destination,
                destination_call_hash(&call),
                destination,
                &call,
            ))
        };

        self.append_flat_call(destination, &call);
        self.push_event(
            destination,
            TraceEvent::CallBegin(destination_call_hash(&call)),
        );
        let children = self.children[node_index].clone();
        if !call.outcome.is_success() && !children.is_empty() {
            // The failed target frame restores its own hash/state; the caller
            // naturally reverts its resolver frame after checking the mini-entry.
            self.open_region(
                RevertRegion {
                    end: self.subtree_ends[node_index],
                    parent_index: Some(node_index),
                    host_chain: destination,
                },
                Some(call.source_rollup_id),
            );
        }
        for child in children {
            self.begin_region(child);
            self.execute_reentrant(destination, child)?;
            self.close_region_at_subtree(child)?;
        }
        self.push_event(
            destination,
            TraceEvent::CallEnd {
                success: call.outcome.is_success(),
                return_data: Bytes::copy_from_slice(
                    call.outcome
                        .return_data()
                        .expect("materializable outcome is resolved"),
                ),
            },
        );

        if let Some(entry_index) = opened {
            self.close_entry(destination, entry_index);
        }
        Ok(())
    }

    fn execute_reentrant(
        &mut self,
        source_chain: RollupId,
        node_index: usize,
    ) -> ProtocolResult<()> {
        let call = self.recorded[node_index].clone();
        if call.call_mode == CallMode::Static {
            return self.execute_static(
                node_index,
                self.recorded[0].source_rollup_id,
                self.origin_cursor,
            );
        }
        if call.source_rollup_id != source_chain {
            return Err(ProtocolErrorKind::InvalidEncoding(format!(
                "nested call {node_index} source {} does not match executing chain {source_chain}",
                call.source_rollup_id,
            ))
            .into());
        }
        let saved_cursor = self.active[&source_chain].next_row;
        let (row_index, reused) = self.push_row(source_chain, &call)?;
        let entry_index = self.active[&source_chain].entry_index;
        // Rebuild each invocation independently, then compare its complete
        // callback program with the first use. Appending to the cached row
        // would silently execute every callback twice on a retry.
        let prior_calls = reused.then(|| {
            std::mem::take(
                &mut self
                    .outputs
                    .get_mut(&source_chain)
                    .expect("active output")
                    .entries[entry_index]
                    .expectedL1ToL2Calls[row_index]
                    .l2ToL1Calls,
            )
        });
        self.push_event(
            source_chain,
            TraceEvent::NestedBegin {
                row_index,
                call_hash: source_call_hash(&call),
            },
        );
        self.active
            .get_mut(&source_chain)
            .expect("source chain active")
            .frame_rows
            .push(row_index);

        self.execute_call(node_index)?;

        let popped = self
            .active
            .get_mut(&source_chain)
            .expect("source chain active")
            .frame_rows
            .pop();
        debug_assert_eq!(popped, Some(row_index));
        if let Some(prior) = prior_calls {
            let current = &self.outputs[&source_chain].entries[entry_index].expectedL1ToL2Calls
                [row_index]
                .l2ToL1Calls;
            if prior.len() != current.len()
                || prior
                    .iter()
                    .zip(current)
                    .any(|(a, b)| a.abi_encode() != b.abi_encode())
            {
                return Err(ProtocolErrorKind::InvalidEncoding(format!(
                    "reused reentrant row {row_index} has a different callback array"
                ))
                .into());
            }
        }

        if call.outcome.is_success() {
            self.push_event(source_chain, TraceEvent::NestedEnd);
        } else {
            self.push_event(source_chain, TraceEvent::NestedRevert { row_index });
            self.active
                .get_mut(&source_chain)
                .expect("source context active")
                .next_row = saved_cursor;
        }
        Ok(())
    }

    fn append_flat_call(&mut self, chain: RollupId, call: &ExecutedAction) {
        let context = self.active.get(&chain).expect("destination context active");
        let entry_index = context.entry_index;
        let frame = context.frame_rows.last().copied();
        let entry = &mut self.outputs.get_mut(&chain).expect("chain output").entries[entry_index];
        let encoded = l1_call_from_action(call);
        let call_index = match frame {
            Some(row) => {
                let calls = &mut entry.expectedL1ToL2Calls[row].l2ToL1Calls;
                let index = calls.len();
                calls.push(encoded);
                index
            }
            None => {
                let index = entry.l2ToL1Calls.len();
                entry.l2ToL1Calls.push(encoded);
                index
            }
        };

        let location = CallArrayLocation {
            chain,
            entry_index,
            frame_row: frame,
        };
        for region in &mut self.active_regions {
            if chain != region.host_chain && Some(chain) != region.other_natural {
                let (first, count) = region
                    .first_calls
                    .entry(location.clone())
                    .or_insert((call_index, 0));
                let end = (*first + *count).max(call_index + 1);
                *first = (*first).min(call_index);
                *count = end - *first;
            }
        }
    }

    fn call_at_mut(
        &mut self,
        location: &CallArrayLocation,
        call_index: usize,
    ) -> &mut crate::abi::L2ToL1CallSol {
        let entry = &mut self
            .outputs
            .get_mut(&location.chain)
            .expect("call-array chain exists")
            .entries[location.entry_index];
        match location.frame_row {
            Some(row) => &mut entry.expectedL1ToL2Calls[row].l2ToL1Calls[call_index],
            None => &mut entry.l2ToL1Calls[call_index],
        }
    }

    fn push_row(
        &mut self,
        chain: RollupId,
        call: &ExecutedAction,
    ) -> ProtocolResult<(usize, bool)> {
        let context = self.active.get_mut(&chain).expect("source context active");
        let entry_index = context.entry_index;
        let output = self.outputs.get_mut(&chain).expect("chain output");
        let key = nested_position_key(
            source_call_hash(call),
            output.positions[entry_index].rolling.current(),
        );
        let entry = &mut output.entries[entry_index];
        let row_index = if call.call_mode == CallMode::Static {
            entry.expectedL1ToL2Calls.len()
        } else {
            entry
                .expectedL1ToL2Calls
                .iter()
                .enumerate()
                .skip(context.next_row)
                .find(|(_, row)| row.expectedL1toL2Hash == key)
                .map(|(index, _)| index)
                .unwrap_or(entry.expectedL1ToL2Calls.len())
        };
        if call.call_mode == CallMode::Mutable {
            context.next_row = row_index + 1;
        }
        if let Some(row) = entry.expectedL1ToL2Calls.get(row_index) {
            let return_data = call
                .outcome
                .return_data()
                .expect("materializable outcome is resolved");
            if row.success != call.outcome.is_success() || row.returnData.as_ref() != return_data {
                return Err(ProtocolErrorKind::InvalidEncoding(format!(
                    "reverted reentrant cursor row {row_index} is reused with a different outcome"
                ))
                .into());
            }
            return Ok((row_index, true));
        }
        if row_index != entry.expectedL1ToL2Calls.len() {
            return Err(ProtocolErrorKind::InvalidEncoding(format!(
                "reentrant row cursor {row_index} advances beyond {} rows",
                entry.expectedL1ToL2Calls.len(),
            ))
            .into());
        }
        entry.expectedL1ToL2Calls.push(ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: key,
            l2ToL1Calls: Vec::new(),
            revertedOrStaticRollingHash: B256::ZERO,
            success: call.outcome.is_success(),
            returnData: Bytes::copy_from_slice(
                call.outcome
                    .return_data()
                    .expect("materializable outcome is resolved"),
            ),
        });
        Ok((row_index, false))
    }

    fn push_event(&mut self, chain: RollupId, event: TraceEvent) {
        let entry_index = self
            .active
            .get(&chain)
            .expect("chain context active")
            .entry_index;
        let output = self.outputs.get_mut(&chain).expect("chain output");
        output.positions[entry_index].apply(&event);
        output.traces[entry_index].events.push(event);
    }

    fn finish(
        mut self,
        entry_rollup_id: RollupId,
        source_entry_count: usize,
    ) -> ProtocolResult<HashMap<RollupId, MaterializedBatch>> {
        debug_assert!(self.active.is_empty());
        if !self.active_regions.is_empty() {
            return Err(ProtocolErrorKind::InvalidEncoding(
                "materialization ended with an open revert region".into(),
            )
            .into());
        }
        let mut result = HashMap::with_capacity(self.outputs.len());
        for (chain, mut output) in self.outputs.drain() {
            if !chain.is_mainnet() {
                for (entry, trace) in output.entries.iter_mut().zip(&output.traces) {
                    let proxy_entry_hash = entry.proxyEntryHash;
                    finalize_trace(
                        entry,
                        trace,
                        EntryRollingHash::seed_for_l2(proxy_entry_hash),
                    )?;
                }
            }
            result.insert(
                chain,
                MaterializedBatch {
                    batch: EvmBatch {
                        entries: output.entries,
                        staticEntries: output.static_entries,
                        immediateEntryCount: U256::from(
                            if chain == entry_rollup_id && !chain.is_mainnet() {
                                0
                            } else {
                                output.immediate_count
                            },
                        ),
                        ..Default::default()
                    },
                    l2_static_entries: output.l2_static_entries,
                    entry_traces: output.traces,
                    source_entry_count: if chain == entry_rollup_id {
                        source_entry_count
                    } else {
                        0
                    },
                    source_outbound_hashes: if chain == entry_rollup_id {
                        self.source_outbound_hashes.clone()
                    } else {
                        Vec::new()
                    },
                },
            );
        }
        Ok(result)
    }
}

fn destination_call_hash(call: &ExecutedAction) -> B256 {
    common_cross_chain_call_hash(CallHashInput {
        call_mode: call.call_mode,
        source_address: call.source_address,
        source_rollup_id: call.source_rollup_id,
        target_address: call.target_address,
        target_rollup_id: call.target_rollup_id,
        value: call.value,
        data: &call.data,
    })
}

fn source_call_hash(call: &ExecutedAction) -> B256 {
    let input = CallHashInput {
        call_mode: call.call_mode,
        source_address: call.source_address,
        source_rollup_id: call.source_rollup_id,
        target_address: call.target_address,
        target_rollup_id: call.target_rollup_id,
        value: call.value,
        data: &call.data,
    };
    if call.source_rollup_id.is_mainnet() {
        common_cross_chain_call_hash(input)
    } else {
        // The current runtime profile pins USE_GAS_LEFT=false, so source-side
        // L2 hashes carry the protocol's zero callGas value.
        l2_outbound_call_hash(input, 0)
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{Address, I256};
    use alloy_sol_types::SolValue;

    use super::*;
    use crate::abi::{EtherDeltaSol, RootUpdateSol};

    fn action(
        index: usize,
        parent_index: Option<usize>,
        source: RollupId,
        target: RollupId,
    ) -> ExecutedAction {
        ExecutedAction {
            parent_index,
            call_mode: CallMode::Mutable,
            source_rollup_id: source,
            source_address: Address::with_last_byte(0x10 + index as u8),
            target_rollup_id: target,
            target_address: Address::with_last_byte(0x20 + index as u8),
            value: U256::from(index),
            data: Bytes::from(vec![0x30 + index as u8]),
            outcome: crate::ExecutionOutcome::Resolved {
                return_data: vec![0x40 + index as u8],
                gas_used: 21_000 + index as u64,
                success: true,
            },
            revert_spans: vec![],
        }
    }

    #[test]
    fn failed_inbound_root_is_lookup_only_and_has_no_l2_delivery() {
        let l2 = RollupId(1);
        let mut call = action(0, None, RollupId::MAINNET, l2);
        call.outcome = crate::ExecutionOutcome::Resolved {
            return_data: b"destination reverted".to_vec(),
            gas_used: 30_000,
            success: false,
        };

        let mut batches = materialize_batches(std::slice::from_ref(&call), RollupId::MAINNET)
            .expect("materialize failed inbound root");
        let mut l1 = batches.remove(&RollupId::MAINNET).expect("L1 source");
        assert!(batches.is_empty());

        let l1_entry = &mut l1.batch.entries[0];
        assert!(!l1_entry.success);
        assert_eq!(l1_entry.returnData.as_ref(), b"destination reverted");
        l1_entry.rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0x11),
            newRoot: B256::with_last_byte(0x11),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut l1.batch, &l1.entry_traces)
            .expect("finalize failed inbound root");
        assert!(crate::settlement::is_lookup_only_inbound_entry(
            &l1.batch.entries[0],
            l2.0,
        ));
    }

    #[test]
    fn source_revert_span_makes_the_complete_call_tree_lookup_only() {
        let l2 = RollupId(1);
        let mut calls = vec![
            action(0, None, RollupId::MAINNET, l2),
            action(1, Some(0), l2, RollupId::MAINNET),
            action(2, None, RollupId::MAINNET, l2),
        ];
        calls[0].revert_spans = vec![2];

        let mut batches = materialize_batches(&calls, RollupId::MAINNET)
            .expect("materialize source-reverted roots");
        let l1 = batches.remove(&RollupId::MAINNET).expect("L1 source");
        let l2_output = batches.remove(&l2).expect("surviving L2 delivery");
        assert!(batches.is_empty());

        assert_eq!(l1.batch.entries.len(), 2);
        assert_eq!(l1.batch.entries[0].l2ToL1Calls.len(), 1);
        assert_eq!(l1.batch.entries[0].l2ToL1Calls[0].data, calls[1].data);
        assert_eq!(
            l1.batch.entries[0].l2ToL1Calls[0].targetAddress,
            calls[1].target_address
        );
        assert!(l1.batch.entries[0].expectedL1ToL2Calls.is_empty());
        assert_eq!(l2_output.batch.entries.len(), 1);
        assert_eq!(
            l2_output.batch.entries[0].proxyEntryHash,
            destination_call_hash(&calls[2]),
        );
    }

    #[test]
    fn four_hop_ping_pong_matches_table_generator_layout_and_hashes() {
        // L2-1 → L1 → L2-1 → L1 → L2-1. This is the same alternating
        // topology as the core protocol's reentrant E2E: the L1 host has two
        // forward rows, while every call returning to L2 stays in the one
        // source entry consumed by the user transaction.
        let l2 = RollupId(1);
        let calls = vec![
            action(0, None, l2, RollupId::MAINNET),
            action(1, Some(0), RollupId::MAINNET, l2),
            action(2, Some(1), l2, RollupId::MAINNET),
            action(3, Some(2), RollupId::MAINNET, l2),
        ];
        let hashes = calls.iter().map(destination_call_hash).collect::<Vec<_>>();

        let mut batches = materialize_batches(&calls, l2).expect("materialize ping-pong");
        let mut l1 = batches.remove(&RollupId::MAINNET).expect("L1 host");
        let l2_output = batches.remove(&l2).expect("L2 origin entry");
        assert!(batches.is_empty());

        let l1_entry = &l1.batch.entries[0];
        assert_eq!(l1.batch.immediateEntryCount, U256::from(1));
        assert_eq!(l1_entry.proxyEntryHash, B256::ZERO);
        assert_eq!(l1_entry.l2ToL1Calls.len(), 1);
        assert_eq!(l1_entry.l2ToL1Calls[0].data, calls[0].data);
        assert_eq!(l1_entry.expectedL1ToL2Calls.len(), 2);
        assert_eq!(l1_entry.expectedL1ToL2Calls[0].l2ToL1Calls.len(), 1);
        assert_eq!(
            l1_entry.expectedL1ToL2Calls[0].l2ToL1Calls[0].data,
            calls[2].data
        );
        assert!(l1_entry.expectedL1ToL2Calls[1].l2ToL1Calls.is_empty());

        assert_eq!(l2_output.source_entry_count, 1);
        assert_eq!(
            l2_output.source_outbound_hashes,
            [source_call_hash(&calls[0]), source_call_hash(&calls[2])]
        );
        assert_eq!(l2_output.batch.entries.len(), 1);
        assert_eq!(l2_output.batch.immediateEntryCount, U256::ZERO);
        let source_entry = &l2_output.batch.entries[0];
        assert_eq!(source_entry.l2ToL1Calls.len(), 1);
        assert_eq!(source_entry.l2ToL1Calls[0].data, calls[1].data);
        assert_eq!(source_entry.expectedL1ToL2Calls.len(), 1);
        assert_eq!(source_entry.expectedL1ToL2Calls[0].l2ToL1Calls.len(), 1);
        assert_eq!(
            source_entry.expectedL1ToL2Calls[0].l2ToL1Calls[0].data,
            calls[3].data
        );
        assert_eq!(source_entry.destinationRollupId, RollupId::MAINNET.0);
        assert_eq!(source_entry.proxyEntryHash, source_call_hash(&calls[0]));

        let mut l2_hash = EntryRollingHash::seed_for_l2(source_call_hash(&calls[0]));
        l2_hash.call_begin(hashes[1]);
        let l2_fire = l2_hash.current();
        assert_eq!(
            source_entry.expectedL1ToL2Calls[0].expectedL1toL2Hash,
            nested_position_key(source_call_hash(&calls[2]), l2_fire)
        );
        l2_hash.nested_begin(source_call_hash(&calls[2]));
        l2_hash.call_begin(hashes[3]);
        l2_hash.call_end(true, calls[3].outcome.return_data().unwrap());
        l2_hash.nested_end();
        l2_hash.call_end(true, calls[1].outcome.return_data().unwrap());
        assert_eq!(source_entry.rollingHash, l2_hash.current());

        l1.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0x51),
            newRoot: B256::with_last_byte(0x52),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut l1.batch, &l1.entry_traces)
            .expect("finalize state-dependent L1 trace");
        let l1_entry = &l1.batch.entries[0];
        let mut l1_hash =
            EntryRollingHash::seed_for_l1([(l2.0, B256::with_last_byte(0x51))], B256::ZERO);
        l1_hash.call_begin(hashes[0]);
        let first_fire = l1_hash.current();
        assert_eq!(
            l1_entry.expectedL1ToL2Calls[0].expectedL1toL2Hash,
            nested_position_key(source_call_hash(&calls[1]), first_fire)
        );
        l1_hash.nested_begin(source_call_hash(&calls[1]));
        l1_hash.call_begin(hashes[2]);
        let second_fire = l1_hash.current();
        assert_eq!(
            l1_entry.expectedL1ToL2Calls[1].expectedL1toL2Hash,
            nested_position_key(source_call_hash(&calls[3]), second_fire)
        );
        l1_hash.nested_begin(source_call_hash(&calls[3]));
        l1_hash.nested_end();
        l1_hash.call_end(true, calls[2].outcome.return_data().unwrap());
        l1_hash.nested_end();
        l1_hash.call_end(true, calls[0].outcome.return_data().unwrap());
        assert_eq!(l1_entry.rollingHash, l1_hash.current());

        let verified = crate::entries::verify_nested_group(
            l1_entry,
            &l2_output.batch.entries,
            crate::entries::NestedPairDirection::Outbound,
            l2,
        )
        .expect("independently verify materialized pair");
        assert_eq!(
            verified.l2_outbound_hashes,
            vec![source_call_hash(&calls[0]), source_call_hash(&calls[2])]
        );
        assert_eq!(verified.ether_delta, I256::try_from(2).unwrap());

        let mut tampered_l1 = l1_entry.clone();
        tampered_l1.returnData = Bytes::from_static(&[0xff]);
        let error = crate::entries::verify_nested_group(
            &tampered_l1,
            &l2_output.batch.entries,
            crate::entries::NestedPairDirection::Outbound,
            l2,
        )
        .expect_err("reject non-canonical outbound host return data");
        assert!(matches!(
            error.kind(),
            ProtocolErrorKind::InvalidEncoding(reason)
                if reason.contains("host return data must be empty")
        ));
    }

    #[test]
    fn flat_l2_roots_share_one_l1_transaction_host() {
        let l2 = RollupId(1);
        let calls = vec![
            action(0, None, l2, RollupId::MAINNET),
            action(1, None, l2, RollupId::MAINNET),
        ];

        let mut batches = materialize_batches(&calls, l2).expect("materialize flat roots");
        let mut l1 = batches.remove(&RollupId::MAINNET).expect("L1 host");
        let l2_output = batches.remove(&l2).expect("L2 source");
        assert!(batches.is_empty());

        assert_eq!(l2_output.source_entry_count, 2);
        assert_eq!(l2_output.batch.entries.len(), 2);
        assert_eq!(l1.batch.immediateEntryCount, U256::from(1));
        assert_eq!(l1.batch.entries.len(), 1);
        assert_eq!(l1.batch.entries[0].l2ToL1Calls.len(), 2);
        assert!(l1.batch.entries[0].expectedL1ToL2Calls.is_empty());

        l1.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0x61),
            newRoot: B256::with_last_byte(0x62),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut l1.batch, &l1.entry_traces)
            .expect("finalize grouped L1 host");

        let mut expected =
            EntryRollingHash::seed_for_l1([(l2.0, B256::with_last_byte(0x61))], B256::ZERO);
        for call in &calls {
            expected.call_begin(destination_call_hash(call));
            expected.call_end(true, call.outcome.return_data().unwrap());
        }
        assert_eq!(l1.batch.entries[0].rollingHash, expected.current());

        let verified = crate::entries::verify_nested_group(
            &l1.batch.entries[0],
            &l2_output.batch.entries,
            crate::entries::NestedPairDirection::Outbound,
            l2,
        )
        .expect("independently verify flat multi-root host");
        assert_eq!(
            verified.l2_outbound_hashes,
            calls.iter().map(source_call_hash).collect::<Vec<_>>()
        );
        let outflow = calls
            .iter()
            .fold(U256::ZERO, |total, call| total + call.value);
        assert_eq!(verified.ether_delta, -I256::try_from(outflow).unwrap());
    }

    #[test]
    fn callback_bearing_l2_roots_share_one_l1_host_and_keep_callbacks_inline() {
        let l2 = RollupId(1);
        let calls = vec![
            action(0, None, l2, RollupId::MAINNET),
            action(1, Some(0), RollupId::MAINNET, l2),
            action(2, None, l2, RollupId::MAINNET),
            action(3, Some(2), RollupId::MAINNET, l2),
        ];

        let mut batches = materialize_batches(&calls, l2).expect("materialize callback roots");
        let l1 = batches.remove(&RollupId::MAINNET).expect("L1 hosts");
        let l2_output = batches.remove(&l2).expect("L2 source and callbacks");
        assert!(batches.is_empty());

        assert_eq!(l2_output.source_entry_count, 2);
        assert_eq!(l2_output.batch.entries.len(), 2);
        assert!(
            l2_output.batch.entries.iter().all(|entry| {
                entry.l2ToL1Calls.len() == 1 && entry.expectedL1ToL2Calls.is_empty()
            })
        );
        assert_eq!(l1.batch.immediateEntryCount, U256::from(1));
        assert_eq!(l1.batch.entries.len(), 1);
        assert!(
            l1.batch.entries.iter().all(|entry| {
                entry.l2ToL1Calls.len() == 2 && entry.expectedL1ToL2Calls.len() == 2
            })
        );
    }

    #[test]
    fn nested_source_revert_regions_preserve_lookup_only_roots() {
        let l1 = RollupId::MAINNET;
        let l2 = RollupId(1);
        for parent in [None, Some(0)] {
            let (source, target) = if parent.is_some() { (l2, l1) } else { (l1, l2) };
            let mut calls = vec![action(0, None, l1, l2), action(1, parent, source, target)];
            calls[0].revert_spans = vec![2];
            materialize_batches(&calls, l1).expect("one complete source rollback region");

            calls[1].revert_spans = vec![1];
            let batches = materialize_batches(&calls, l1).expect("properly nested source regions");
            assert_eq!(batches.len(), 1, "reverted roots authorize no L2 delivery");
            assert!(batches.contains_key(&l1));
            assert!(batches[&l1].source_outbound_hashes.is_empty());
        }
    }

    #[test]
    fn materialization_rejects_crossing_source_revert_regions() {
        let l1 = RollupId::MAINNET;
        let l2 = RollupId(1);
        let mut calls = vec![
            action(0, None, l1, l2),
            action(1, None, l1, l2),
            action(2, None, l1, l2),
        ];
        calls[0].revert_spans = vec![2];
        calls[1].revert_spans = vec![2];
        let error =
            materialize_batches(&calls, l1).expect_err("crossing scopes cannot be bracketed");
        assert!(
            error.to_string().contains("crossing rollback scopes"),
            "{error}"
        );
    }

    #[test]
    fn root_revert_region_is_lowered_to_one_l1_array_span() {
        let l2 = RollupId(1);
        let mut calls = vec![
            action(0, None, l2, RollupId::MAINNET),
            action(1, None, l2, RollupId::MAINNET),
            action(2, None, l2, RollupId::MAINNET),
        ];
        calls[1].revert_spans = vec![2];

        let mut batches = materialize_batches(&calls, l2).expect("materialize rollback window");
        let mut l1 = batches.remove(&RollupId::MAINNET).expect("L1 host");
        let l2_output = batches.remove(&l2).expect("L2 source");
        assert!(batches.is_empty());
        assert_eq!(l2_output.batch.entries.len(), 3);
        assert_eq!(l1.batch.entries.len(), 1);
        let table_calls = &l1.batch.entries[0].l2ToL1Calls;
        assert_eq!(table_calls.len(), 3);
        assert_eq!(table_calls[0].revertNextNCalls, 0);
        assert_eq!(table_calls[1].revertNextNCalls, 2);
        assert_eq!(table_calls[2].revertNextNCalls, 0);
        assert_eq!(
            crate::system_tx::outbound_settlement_root_counts(&l1.batch.entries).unwrap(),
            [3]
        );

        l1.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0x71),
            newRoot: B256::with_last_byte(0x72),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut l1.batch, &l1.entry_traces).expect("finalize rollback host");
        let mut expected =
            EntryRollingHash::seed_for_l1([(l2.0, B256::with_last_byte(0x71))], B256::ZERO);
        for call in &calls {
            expected.call_begin(destination_call_hash(call));
            expected.call_end(
                call.outcome.is_success(),
                call.outcome.return_data().expect("resolved test outcome"),
            );
        }
        assert_eq!(l1.batch.entries[0].rollingHash, expected.current());

        let verified = crate::entries::verify_nested_group(
            &l1.batch.entries[0],
            &l2_output.batch.entries,
            crate::entries::NestedPairDirection::Outbound,
            l2,
        )
        .expect("verify committed root and reverted sibling span");
        assert_eq!(verified.l2_outbound_hashes, [source_call_hash(&calls[0])]);
        assert_eq!(verified.ether_delta, I256::ZERO);

        let mut overlapping = l1.batch.entries[0].clone();
        overlapping
            .l2ToL1Calls
            .push(overlapping.l2ToL1Calls[0].clone());
        overlapping.l2ToL1Calls[2].revertNextNCalls = 2;
        let overlapping = ExecutionEntrySol::abi_decode(&overlapping.abi_encode())
            .expect("overlapping spans still have valid ABI encoding");
        // The public verifier checks array shape before interpreting hashes.
        // This must reach that guard, not a later rolling-hash mismatch.
        let error = crate::entries::verify_nested_group(
            &overlapping,
            &l2_output.batch.entries,
            crate::entries::NestedPairDirection::Outbound,
            l2,
        )
        .expect_err("inner rollback marker crosses its enclosing span");
        assert!(
            matches!(
                error.kind(),
                ProtocolErrorKind::InvalidEncoding(reason)
                    if reason == "L1 entry flat calls call 2 starts a crossing revert span"
            ),
            "{error}"
        );
    }

    #[test]
    fn root_revert_retry_reuses_source_slot_and_l1_host() {
        let l2 = RollupId(1);
        let mut calls = vec![
            action(0, None, l2, RollupId::MAINNET),
            action(0, None, l2, RollupId::MAINNET),
        ];
        calls[0].revert_spans = vec![1];

        let mut batches = materialize_batches(&calls, l2).expect("materialize rollback retry");
        let mut l1 = batches.remove(&RollupId::MAINNET).expect("L1 host");
        let l2_output = batches.remove(&l2).expect("L2 source");
        assert!(batches.is_empty());

        assert_eq!(l2_output.source_entry_count, 1);
        assert_eq!(l2_output.batch.entries.len(), 1);
        assert_eq!(l1.batch.entries.len(), 1);
        assert_eq!(l1.batch.immediateEntryCount, U256::from(1));
        assert_eq!(l1.batch.entries[0].l2ToL1Calls.len(), 2);
        assert_eq!(l1.batch.entries[0].l2ToL1Calls[0].revertNextNCalls, 1);
        assert_eq!(l1.batch.entries[0].l2ToL1Calls[1].revertNextNCalls, 0);
        assert_eq!(
            crate::system_tx::outbound_settlement_root_counts(&l1.batch.entries).unwrap(),
            [1]
        );

        l1.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0x81),
            newRoot: B256::with_last_byte(0x82),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut l1.batch, &l1.entry_traces)
            .expect("finalize rollback retry host");

        let verified = crate::entries::verify_nested_group(
            &l1.batch.entries[0],
            &l2_output.batch.entries,
            crate::entries::NestedPairDirection::Outbound,
            l2,
        )
        .expect("verify rollback retry group");
        assert_eq!(verified.l2_outbound_hashes, [source_call_hash(&calls[1])]);
        assert_eq!(
            verified.ether_delta,
            -I256::try_from(calls[1].value).unwrap()
        );
    }

    #[test]
    fn nested_revert_retry_reuses_reentrant_row() {
        let l2 = RollupId(1);
        let mut calls = vec![
            action(0, None, RollupId::MAINNET, l2),
            action(1, Some(0), l2, RollupId::MAINNET),
            action(1, Some(0), l2, RollupId::MAINNET),
        ];
        calls[1].revert_spans = vec![1];

        let mut batches = materialize_batches(&calls, RollupId::MAINNET)
            .expect("materialize nested rollback retry");
        let mut l1 = batches.remove(&RollupId::MAINNET).expect("L1 source");
        let l2_output = batches.remove(&l2).expect("L2 destination");
        assert!(batches.is_empty());
        assert_eq!(l1.source_outbound_hashes, [source_call_hash(&calls[2])]);

        l1.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0x91),
            newRoot: B256::with_last_byte(0x92),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut l1.batch, &l1.entry_traces)
            .expect("finalize nested rollback retry");

        let l1_entry = &l1.batch.entries[0];
        let l2_entry = &l2_output.batch.entries[0];
        assert_eq!(l1_entry.l2ToL1Calls.len(), 2);
        assert_eq!(l1_entry.l2ToL1Calls[0].revertNextNCalls, 1);
        assert_eq!(l1_entry.l2ToL1Calls[1].revertNextNCalls, 0);
        assert_eq!(l2_entry.expectedL1ToL2Calls.len(), 1);

        let verified = crate::entries::verify_nested_pair(
            l1_entry,
            l2_entry,
            crate::entries::NestedPairDirection::Inbound,
            l2,
        )
        .expect("verify reused nested row");
        assert_eq!(verified.l2_outbound_hashes, [source_call_hash(&calls[2])]);
        assert_eq!(verified.ether_delta, I256::try_from(-1).unwrap());
    }

    #[test]
    fn reverted_reentrant_retry_reuses_the_complete_callback_array() {
        let l2 = RollupId(1);
        for target_fails in [false, true] {
            let mut calls = vec![
                action(0, None, RollupId::MAINNET, l2),
                action(1, Some(0), l2, RollupId::MAINNET),
                action(2, Some(1), RollupId::MAINNET, l2),
                action(1, Some(0), l2, RollupId::MAINNET),
                action(2, Some(3), RollupId::MAINNET, l2),
            ];
            if target_fails {
                for index in [1, 3] {
                    calls[index].outcome = crate::ExecutionOutcome::Resolved {
                        success: false,
                        return_data: vec![0xde, 0xad],
                        gas_used: 0,
                    };
                }
            } else {
                calls[1].revert_spans = vec![2];
            }
            let mut batches = materialize_batches(&calls, RollupId::MAINNET).unwrap();
            let mut source = batches.remove(&RollupId::MAINNET).unwrap();
            let target = batches.remove(&l2).unwrap();
            source.batch.entries[0].rootUpdates.push(RootUpdateSol {
                rollupId: 1,
                currentRoot: B256::with_last_byte(1),
                newRoot: B256::with_last_byte(2),
                etherDelta: EtherDeltaSol::ZERO,
            });
            finalize_l1_entry_traces(&mut source.batch, &source.entry_traces).unwrap();
            assert_eq!(target.batch.entries[0].expectedL1ToL2Calls.len(), 1);
            assert_eq!(
                target.batch.entries[0].expectedL1ToL2Calls[0]
                    .l2ToL1Calls
                    .len(),
                1
            );
            crate::entries::verify_nested_pair(
                &source.batch.entries[0],
                &target.batch.entries[0],
                crate::entries::NestedPairDirection::Inbound,
                l2,
            )
            .unwrap();
            if target_fails {
                let mut trace = target.entry_traces[0].clone();
                let last_callback = trace
                    .events
                    .iter_mut()
                    .rev()
                    .find_map(|event| match event {
                        TraceEvent::CallEnd { return_data, .. }
                            if return_data.as_ref() == [0x42] =>
                        {
                            Some(return_data)
                        }
                        _ => None,
                    })
                    .unwrap();
                *last_callback = Bytes::from_static(&[0x43]);
                let mut entry = target.batch.entries[0].clone();
                let seed = EntryRollingHash::seed_for_l2(entry.proxyEntryHash);
                let error = finalize_trace(&mut entry, &trace, seed).unwrap_err();
                assert!(
                    error.to_string().contains("different execution commitment"),
                    "{error}"
                );
            }
            let mut forged = calls.clone();
            forged[4].data = Bytes::from_static(b"different callback");
            let error = materialize_batches(&forged, RollupId::MAINNET)
                .expect_err("changed callback must reject");
            assert!(
                error.to_string().contains("different callback array"),
                "{error}"
            );
        }
    }

    #[test]
    fn failed_nested_row_commits_subhash_but_restores_host_hash() {
        let l2 = RollupId(1);
        let mut calls = vec![
            action(0, None, RollupId::MAINNET, l2),
            action(1, Some(0), l2, RollupId::MAINNET),
            action(2, Some(1), RollupId::MAINNET, l2),
        ];
        calls[2].outcome = crate::ExecutionOutcome::Resolved {
            return_data: b"inner reverts".to_vec(),
            gas_used: 30_000,
            success: false,
        };

        let mut batches =
            materialize_batches(&calls, RollupId::MAINNET).expect("materialize failed nested row");
        let mut l1 = batches.remove(&RollupId::MAINNET).expect("L1 source");
        let l2_output = batches.remove(&l2).expect("L2 target");
        assert!(batches.is_empty());
        let l1_entry = &mut l1.batch.entries[0];
        l1_entry.rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0x71),
            newRoot: B256::with_last_byte(0x72),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut l1.batch, &l1.entry_traces)
            .expect("finalize failed nested row");

        let entry = &l1.batch.entries[0];
        assert_eq!(entry.l2ToL1Calls.len(), 1);
        assert_eq!(entry.expectedL1ToL2Calls.len(), 1);
        let row = &entry.expectedL1ToL2Calls[0];
        assert!(!row.success);
        assert_eq!(row.returnData.as_ref(), b"inner reverts");

        let mut expected = EntryRollingHash::seed_for_l1(
            [(l2.0, B256::with_last_byte(0x71))],
            source_call_hash(&calls[0]),
        );
        expected.call_begin(destination_call_hash(&calls[1]));
        let fire_hash = expected.current();
        assert_eq!(
            row.expectedL1toL2Hash,
            nested_position_key(source_call_hash(&calls[2]), fire_hash)
        );
        let mut reverted = expected.clone();
        reverted.nested_begin(source_call_hash(&calls[2]));
        assert_eq!(row.revertedOrStaticRollingHash, reverted.current());
        expected.call_end(true, calls[1].outcome.return_data().unwrap());
        assert_eq!(entry.rollingHash, expected.current());

        let verified = crate::entries::verify_nested_pair(
            entry,
            &l2_output.batch.entries[0],
            crate::entries::NestedPairDirection::Inbound,
            l2,
        )
        .expect("independently verify failed nested frame");
        assert_eq!(
            verified.ether_delta,
            I256::try_from(-1).unwrap(),
            "the successful L2-to-L1 parent value commits; the failed nested value rolls back"
        );

        let mut tampered = entry.clone();
        tampered.expectedL1ToL2Calls[0].revertedOrStaticRollingHash = B256::repeat_byte(0xee);
        crate::entries::verify_nested_pair(
            &tampered,
            &l2_output.batch.entries[0],
            crate::entries::NestedPairDirection::Inbound,
            l2,
        )
        .expect_err("reject a tampered failed-frame sub-hash");
    }

    #[test]
    fn l2_origin_nested_revert_retry_stays_inline_and_reuses_l1_row() {
        let l2 = RollupId(1);
        let mut calls = vec![
            action(0, None, l2, RollupId::MAINNET),
            action(1, Some(0), RollupId::MAINNET, l2),
            action(1, Some(0), RollupId::MAINNET, l2),
        ];
        calls[1].revert_spans = vec![1];

        let mut batches =
            materialize_batches(&calls, l2).expect("materialize L2-origin nested retry");
        let mut l1 = batches.remove(&RollupId::MAINNET).expect("L1 host");
        let l2_output = batches.remove(&l2).expect("L2 source");
        assert!(batches.is_empty());

        assert_eq!(l2_output.source_entry_count, 1);
        assert_eq!(l2_output.batch.entries.len(), 1);
        let source_entry = &l2_output.batch.entries[0];
        assert_eq!(source_entry.l2ToL1Calls.len(), 2);
        assert_eq!(source_entry.l2ToL1Calls[0].revertNextNCalls, 1);
        assert_eq!(source_entry.l2ToL1Calls[1].revertNextNCalls, 0);

        assert_eq!(l1.batch.entries.len(), 1);
        assert_eq!(l1.batch.entries[0].l2ToL1Calls.len(), 1);
        assert_eq!(l1.batch.entries[0].expectedL1ToL2Calls.len(), 1);

        l1.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0xa1),
            newRoot: B256::with_last_byte(0xa2),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut l1.batch, &l1.entry_traces)
            .expect("finalize L2-origin nested retry");

        crate::entries::verify_nested_group(
            &l1.batch.entries[0],
            &l2_output.batch.entries,
            crate::entries::NestedPairDirection::Outbound,
            l2,
        )
        .expect("verify L2-origin nested retry");
    }

    #[test]
    fn l2_origin_failed_callback_is_inline_without_a_rollback_marker() {
        let l2 = RollupId(1);
        let mut calls = vec![
            action(0, None, l2, RollupId::MAINNET),
            action(1, Some(0), RollupId::MAINNET, l2),
        ];
        calls[1].outcome = crate::ExecutionOutcome::Resolved {
            return_data: b"callback reverted".to_vec(),
            gas_used: 30_000,
            success: false,
        };

        let mut batches = materialize_batches(&calls, l2).expect("materialize failed L2 callback");
        let mut l1 = batches.remove(&RollupId::MAINNET).expect("L1 host");
        let l2_output = batches.remove(&l2).expect("L2 source");
        assert!(batches.is_empty());

        assert_eq!(l2_output.source_entry_count, 1);
        assert_eq!(l2_output.batch.entries.len(), 1);
        assert_eq!(
            l2_output.source_outbound_hashes,
            [source_call_hash(&calls[0])]
        );
        let source_entry = &l2_output.batch.entries[0];
        assert_eq!(source_entry.l2ToL1Calls.len(), 1);
        assert_eq!(source_entry.l2ToL1Calls[0].revertNextNCalls, 0);
        assert_eq!(l1.batch.entries[0].expectedL1ToL2Calls.len(), 1);
        assert!(!l1.batch.entries[0].expectedL1ToL2Calls[0].success);
        assert_eq!(
            crate::entries::outbound_ether_out(&l1.batch.entries[0]),
            Some(U256::ZERO),
            "escrow projection must accept an unfinalized failed callback row"
        );

        l1.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0xb1),
            newRoot: B256::with_last_byte(0xb2),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut l1.batch, &l1.entry_traces)
            .expect("finalize failed L2 callback");

        crate::entries::verify_nested_group(
            &l1.batch.entries[0],
            &l2_output.batch.entries,
            crate::entries::NestedPairDirection::Outbound,
            l2,
        )
        .expect("verify failed L2 callback");
    }

    #[test]
    fn naturally_reverted_l2_root_has_no_committed_event_or_value_outflow() {
        let l2 = RollupId(1);
        let mut call = action(0, None, l2, RollupId::MAINNET);
        call.value = U256::from(7);
        call.outcome = crate::ExecutionOutcome::Resolved {
            return_data: b"L1 target reverted".to_vec(),
            gas_used: 30_000,
            success: false,
        };

        let mut batches = materialize_batches(std::slice::from_ref(&call), l2)
            .expect("materialize naturally failed L2 root");
        let mut l1 = batches.remove(&RollupId::MAINNET).expect("L1 host");
        let l2_output = batches.remove(&l2).expect("L2 source");
        assert!(batches.is_empty());
        assert!(
            l2_output.source_outbound_hashes.is_empty(),
            "the reverting L2 proxy frame rolls its receipt event back"
        );

        l1.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0xd1),
            newRoot: B256::with_last_byte(0xd2),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut l1.batch, &l1.entry_traces)
            .expect("finalize naturally failed L2 root");

        let verified = crate::entries::verify_nested_group(
            &l1.batch.entries[0],
            &l2_output.batch.entries,
            crate::entries::NestedPairDirection::Outbound,
            l2,
        )
        .expect("verify naturally failed L2 root");
        assert!(verified.l2_outbound_hashes.is_empty());
        assert_eq!(
            verified.ether_delta,
            I256::ZERO,
            "the failed root's value transfer rolls back with its proxy frame"
        );
    }

    #[test]
    fn naturally_reverted_l2_proxy_call_requires_no_committed_event() {
        let l2 = RollupId(1);
        let mut calls = vec![
            action(0, None, RollupId::MAINNET, l2),
            action(1, Some(0), l2, RollupId::MAINNET),
        ];
        calls[1].outcome = crate::ExecutionOutcome::Resolved {
            return_data: b"L1 target reverted".to_vec(),
            gas_used: 30_000,
            success: false,
        };

        let mut batches = materialize_batches(&calls, RollupId::MAINNET)
            .expect("materialize naturally failed outbound callback");
        let mut l1 = batches.remove(&RollupId::MAINNET).expect("L1 source");
        let l2_output = batches.remove(&l2).expect("L2 destination");
        assert!(batches.is_empty());
        assert!(
            l1.source_outbound_hashes.is_empty(),
            "a reverting EEZL2 proxy frame rolls its event back"
        );

        l1.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0xc1),
            newRoot: B256::with_last_byte(0xc2),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut l1.batch, &l1.entry_traces)
            .expect("finalize naturally failed outbound callback");
        let verified = crate::entries::verify_nested_group(
            &l1.batch.entries[0],
            &l2_output.batch.entries,
            crate::entries::NestedPairDirection::Inbound,
            l2,
        )
        .expect("verify naturally failed outbound callback");
        assert!(verified.l2_outbound_hashes.is_empty());
    }

    /// A static read (index 0) whose target makes one static callback to the
    /// reading chain (index 1), as in the protocol's
    /// `topLevelStaticReentrantCounter` scenarios.
    fn static_round_trip(source: RollupId, remote: RollupId) -> Vec<ExecutedAction> {
        let mut read = action(0, None, source, remote);
        read.call_mode = CallMode::Static;
        read.value = U256::ZERO;
        let mut callback = action(1, Some(0), remote, source);
        callback.call_mode = CallMode::Static;
        callback.value = U256::ZERO;
        vec![read, callback]
    }

    /// The protocol's untagged static fold over one result, written out from
    /// its definition: `keccak256(prev ‖ success ‖ returnData)` from zero.
    fn one_static_result_hash(success: bool, return_data: &[u8]) -> B256 {
        let mut preimage = vec![0u8; 32];
        preimage.push(u8::from(success));
        preimage.extend_from_slice(return_data);
        keccak256(preimage)
    }

    #[test]
    fn l1_static_root_carries_its_static_callback() {
        let l2 = RollupId(1);
        let calls = static_round_trip(RollupId::MAINNET, l2);
        let mut batches =
            materialize_batches(&calls, RollupId::MAINNET).expect("materialize static round trip");
        let l1 = batches.remove(&RollupId::MAINNET).expect("L1 source");
        assert!(batches.is_empty(), "a static read delivers nothing to L2");
        assert!(l1.batch.entries.is_empty());

        let [entry] = l1.batch.staticEntries.as_slice() else {
            panic!("expected one static entry");
        };
        assert_eq!(entry.proxyEntryHash, source_call_hash(&calls[0]));
        assert_eq!(entry.returnData.as_ref(), [0x40]);
        let [callback] = entry.l2ToL1Calls.as_slice() else {
            panic!("expected one callback");
        };
        assert!(callback.isStatic);
        assert_eq!(callback.gas, 0);
        assert_eq!(callback.revertNextNCalls, 0);
        assert_eq!(callback.sourceRollupId, l2.0);
        assert_eq!(callback.sourceAddress, calls[1].source_address);
        assert_eq!(callback.targetAddress, calls[1].target_address);
        assert_eq!(callback.data, calls[1].data);
        assert_eq!(entry.rollingHash, one_static_result_hash(true, &[0x41]));
        crate::system_tx::check_static_entry_shape(entry, "test").expect("valid static entry");
    }

    /// Materialize an L2 static round trip, finalize its L1 host and return
    /// both sides for verification.
    fn l2_static_round_trip_tables() -> (crate::abi::ExecutionEntrySol, L2StaticExecutionEntrySol) {
        let l2 = RollupId(1);
        let calls = static_round_trip(l2, RollupId::MAINNET);
        let mut batches = materialize_batches(&calls, l2).expect("materialize static round trip");
        let mut host = batches.remove(&RollupId::MAINNET).expect("L1 host");
        let source = batches.remove(&l2).expect("L2 static source");
        assert!(batches.is_empty());
        assert!(
            source.batch.entries.is_empty(),
            "the callback runs from the static entry, not a delivery entry"
        );
        host.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0xd1),
            newRoot: B256::with_last_byte(0xd2),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut host.batch, &host.entry_traces).expect("finalize host");
        let [static_entry] =
            <[_; 1]>::try_from(source.l2_static_entries).expect("one static entry");
        (host.batch.entries.remove(0), static_entry)
    }

    fn verify_l2_static_round_trip(
        host: &crate::abi::ExecutionEntrySol,
        source: &L2StaticExecutionEntrySol,
    ) -> ProtocolResult<crate::entries::NestedPairVerification> {
        crate::entries::verify_outbound_source_group(
            host,
            &[crate::system_tx::OutboundSourceEntry::Static(
                source.clone(),
            )],
            RollupId(1),
        )
    }

    #[test]
    fn l2_static_root_callback_is_bound_to_its_l1_host_row() {
        let (host, source) = l2_static_round_trip_tables();
        let calls = static_round_trip(RollupId(1), RollupId::MAINNET);

        let [callback] = source.incomingCalls.as_slice() else {
            panic!("expected one callback");
        };
        assert!(callback.isStatic);
        assert_eq!(callback.sourceRollupId, RollupId::MAINNET.0);
        assert_eq!(callback.sourceAddress, calls[1].source_address);
        assert_eq!(callback.targetAddress, calls[1].target_address);
        assert_eq!(source.rollingHash, one_static_result_hash(true, &[0x41]));
        assert_eq!(source.returnData.as_ref(), [0x40]);
        crate::system_tx::check_l2_static_entry_shape(&source, "test")
            .expect("valid L2 static entry");

        // L1 runs the read in the host, and the callback resolves the host's
        // static row, keyed right after the read's CALL_BEGIN.
        let [read] = host.l2ToL1Calls.as_slice() else {
            panic!("expected the read as the host's only flat call");
        };
        assert!(read.isStatic);
        let [row] = host.expectedL1ToL2Calls.as_slice() else {
            panic!("expected one static row");
        };
        let mut rolling = EntryRollingHash::seed_for_l1(
            host.rootUpdates
                .iter()
                .map(|update| (update.rollupId, update.currentRoot)),
            B256::ZERO,
        );
        rolling.call_begin(destination_call_hash(&calls[0]));
        assert_eq!(
            row.expectedL1toL2Hash,
            nested_position_key(source_call_hash(&calls[1]), rolling.current())
        );
        assert!(row.success);
        assert_eq!(row.returnData.as_ref(), [0x41]);
        assert!(row.l2ToL1Calls.is_empty());
        assert_eq!(row.revertedOrStaticRollingHash, B256::ZERO);
        // The static row does not fold into the host hash.
        rolling.call_end(true, &[0x40]);
        assert_eq!(host.rollingHash, rolling.current());

        let verified = verify_l2_static_round_trip(&host, &source).expect("verify round trip");
        assert!(verified.l2_outbound_hashes.is_empty());
        assert_eq!(verified.ether_delta, I256::ZERO);
    }

    #[test]
    fn l2_static_root_callback_rejects_unbound_results() {
        let (host, source) = l2_static_round_trip_tables();
        let rejected = |host: &crate::abi::ExecutionEntrySol,
                        source: &L2StaticExecutionEntrySol,
                        reason: &str| {
            let error = verify_l2_static_round_trip(host, source)
                .expect_err("tampered static round trip must be rejected");
            assert!(error.to_string().contains(reason), "{reason}: {error}");
        };
        const RESULTS: &str = "callback results differ from the L2 static rolling hash";
        const ROW: &str = "callback does not match its L1 host row";

        let mut tampered = host.clone();
        // L1 would return a value that L2 does not compute.
        tampered.expectedL1ToL2Calls[0].returnData = Bytes::from_static(&[0xff]);
        rejected(&tampered, &source, RESULTS);
        let mut tampered = host.clone();
        tampered.expectedL1ToL2Calls[0].expectedL1toL2Hash = B256::with_last_byte(1);
        rejected(&tampered, &source, ROW);
        let mut tampered = host.clone();
        tampered.expectedL1ToL2Calls.clear();
        rejected(&tampered, &source, "callback has no L1 host row");
        let mut tampered = host.clone();
        let extra = tampered.expectedL1ToL2Calls[0].clone();
        tampered.expectedL1ToL2Calls.push(extra);
        rejected(&tampered, &source, "did not consume the complete");

        let mut tampered = source.clone();
        tampered.rollingHash = B256::with_last_byte(1);
        rejected(&host, &tampered, RESULTS);
        let mut tampered = source.clone();
        tampered.incomingCalls[0].data = Bytes::from_static(&[0xff]);
        rejected(&host, &tampered, ROW);
        let mut tampered = source.clone();
        tampered.incomingCalls.clear();
        tampered.rollingHash = B256::ZERO;
        rejected(&host, &tampered, "did not consume the complete");
    }

    /// Two identical callbacks in one L2 static read share a position key, and
    /// L1 answers both from the first matching row. Equal results verify;
    /// different results cannot both be returned and are rejected.
    #[test]
    fn repeated_static_callbacks_must_agree() {
        let l2 = RollupId(1);
        for (second_result, accepted) in [(vec![0x41], true), (vec![0x42], false)] {
            let mut calls = static_round_trip(l2, RollupId::MAINNET);
            let mut repeat = calls[1].clone();
            repeat.outcome = crate::ExecutionOutcome::Resolved {
                return_data: second_result,
                gas_used: 21_000,
                success: true,
            };
            calls.push(repeat);
            let mut batches =
                materialize_batches(&calls, l2).expect("materialize repeated callbacks");
            let mut host = batches.remove(&RollupId::MAINNET).expect("L1 host");
            let source = batches.remove(&l2).expect("L2 static source");
            host.batch.entries[0].rootUpdates.push(RootUpdateSol {
                rollupId: l2.0,
                currentRoot: B256::with_last_byte(0xd1),
                newRoot: B256::with_last_byte(0xd2),
                etherDelta: EtherDeltaSol::ZERO,
            });
            finalize_l1_entry_traces(&mut host.batch, &host.entry_traces).expect("finalize host");
            let [row_a, row_b] = host.batch.entries[0].expectedL1ToL2Calls.as_slice() else {
                panic!("expected one host row per callback");
            };
            assert_eq!(row_a.expectedL1toL2Hash, row_b.expectedL1toL2Hash);
            let verified =
                verify_l2_static_round_trip(&host.batch.entries[0], &source.l2_static_entries[0]);
            if accepted {
                verified.expect("equal repeated results verify");
            } else {
                let error = verified.expect_err("conflicting repeated results must be rejected");
                assert!(
                    error
                        .to_string()
                        .contains("repeat a lookup key with different results"),
                    "{error}"
                );
            }
        }
    }

    #[test]
    fn static_reads_reject_mutable_callbacks() {
        let l2 = RollupId(1);
        let unsupported = |calls: &[ExecutedAction], reason: &str| {
            let error = materialize_batches(calls, RollupId::MAINNET)
                .expect_err("unsupported static callback shape");
            assert!(error.to_string().contains(reason), "{error}");
        };

        let mut mutable_callback = static_round_trip(RollupId::MAINNET, l2);
        mutable_callback[1].call_mode = CallMode::Mutable;
        unsupported(&mutable_callback, "cannot make mutable callbacks");
    }

    #[test]
    fn terminal_l1_static_root_stays_in_the_l1_static_pool() {
        let l2 = RollupId(1);
        let mut call = action(0, None, RollupId::MAINNET, l2);
        call.call_mode = CallMode::Static;
        call.value = U256::ZERO;

        let mut batches = materialize_batches(std::slice::from_ref(&call), RollupId::MAINNET)
            .expect("materialize L1 static root");
        let l1 = batches.remove(&RollupId::MAINNET).expect("L1 source");
        assert!(batches.is_empty(), "a static root has no L2 delivery entry");
        assert!(l1.batch.entries.is_empty());
        assert_eq!(l1.batch.staticEntries.len(), 1);

        let entry = &l1.batch.staticEntries[0];
        assert_eq!(entry.proxyEntryHash, source_call_hash(&call));
        assert_eq!(entry.destinationRollupId, l2.0);
        assert!(entry.l2ToL1Calls.is_empty());
        assert_eq!(entry.rollingHash, B256::ZERO);
        assert_eq!(entry.returnData.as_ref(), [0x40]);
    }

    #[test]
    fn terminal_l2_static_root_is_bound_to_its_l1_host() {
        let l2 = RollupId(1);
        let mut call = action(0, None, l2, RollupId::MAINNET);
        call.call_mode = CallMode::Static;
        call.value = U256::ZERO;

        let mut batches = materialize_batches(std::slice::from_ref(&call), l2)
            .expect("materialize L2 static root");
        let mut l1 = batches.remove(&RollupId::MAINNET).expect("L1 host");
        let l2_output = batches.remove(&l2).expect("L2 static source");
        assert!(batches.is_empty());
        assert!(l2_output.batch.entries.is_empty());
        assert_eq!(l2_output.l2_static_entries.len(), 1);

        l1.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0xd1),
            newRoot: B256::with_last_byte(0xd2),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut l1.batch, &l1.entry_traces)
            .expect("finalize L2 static root host");

        let verified = crate::entries::verify_outbound_source_group(
            &l1.batch.entries[0],
            &[crate::system_tx::OutboundSourceEntry::Static(
                l2_output.l2_static_entries[0].clone(),
            )],
            l2,
        )
        .expect("verify L2 static root");
        assert!(verified.l2_outbound_hashes.is_empty());
        assert_eq!(verified.ether_delta, I256::ZERO);
    }

    #[test]
    fn mixed_static_results_are_bound_to_the_materialized_mutable_position() {
        use crate::system_tx::OutboundSourceEntry;
        let l2 = RollupId(1);
        let mut read = action(0, None, l2, RollupId::MAINNET);
        read.call_mode = CallMode::Static;
        let mut changed = read.clone();
        changed.outcome = crate::ExecutionOutcome::Resolved {
            return_data: vec![0x41],
            gas_used: 21_000,
            success: true,
        };
        let mut write = action(1, None, l2, RollupId::MAINNET);
        write.value = U256::ZERO;
        for has_write in [true, false] {
            let calls = if has_write {
                vec![
                    read.clone(),
                    read.clone(),
                    write.clone(),
                    changed.clone(),
                    changed.clone(),
                ]
            } else {
                vec![read.clone(), changed.clone()]
            };
            let mut batches = materialize_batches(&calls, l2).unwrap();
            let mut host = batches.remove(&RollupId::MAINNET).unwrap();
            let source = batches.remove(&l2).unwrap();
            assert!(source.batch.staticEntries.is_empty());
            let positions = source
                .l2_static_entries
                .iter()
                .map(|entry| entry.expectedEntryIndex)
                .collect::<Vec<_>>();
            assert_eq!(
                positions,
                if has_write {
                    vec![U256::ZERO, U256::ZERO, U256::from(1), U256::from(1)]
                } else {
                    vec![U256::ZERO, U256::ZERO]
                }
            );
            host.batch.entries[0].rootUpdates.push(RootUpdateSol {
                rollupId: 1,
                currentRoot: B256::with_last_byte(1),
                newRoot: B256::with_last_byte(2),
                etherDelta: EtherDeltaSol::ZERO,
            });
            finalize_l1_entry_traces(&mut host.batch, &host.entry_traces).unwrap();
            let sources = source
                .batch
                .entries
                .into_iter()
                .map(OutboundSourceEntry::Mutable)
                .chain(
                    source
                        .l2_static_entries
                        .into_iter()
                        .map(OutboundSourceEntry::Static),
                )
                .collect::<Vec<_>>();
            let verified =
                crate::entries::verify_outbound_source_group(&host.batch.entries[0], &sources, l2);
            if has_write {
                assert!(verified.is_ok(), "{verified:?}");
            } else {
                assert!(
                    verified
                        .unwrap_err()
                        .to_string()
                        .contains("conflicting results")
                );
            }
        }
    }

    #[test]
    fn mixed_outbound_groups_accept_nested_calls_but_reject_failed_and_rollback_sources() {
        use crate::system_tx::OutboundSourceEntry;
        let l2 = RollupId(1);
        let mut read = action(0, None, l2, RollupId::MAINNET);
        read.call_mode = CallMode::Static;
        let mut write = action(1, None, l2, RollupId::MAINNET);
        write.value = U256::ZERO;
        let calls = vec![read, write];
        let verify = |calls: &[ExecutedAction]| {
            let mut batches = materialize_batches(calls, l2).expect("materialize mixed source");
            let mut host = batches.remove(&RollupId::MAINNET).unwrap();
            let source = batches.remove(&l2).unwrap();
            host.batch.entries[0].rootUpdates.push(RootUpdateSol {
                rollupId: l2.0,
                currentRoot: B256::with_last_byte(1),
                newRoot: B256::with_last_byte(2),
                etherDelta: EtherDeltaSol::ZERO,
            });
            finalize_l1_entry_traces(&mut host.batch, &host.entry_traces)
                .expect("finalize mixed source");
            let sources = source
                .batch
                .entries
                .into_iter()
                .map(OutboundSourceEntry::Mutable)
                .chain(
                    source
                        .l2_static_entries
                        .into_iter()
                        .map(OutboundSourceEntry::Static),
                )
                .collect::<Vec<_>>();
            crate::entries::verify_outbound_source_group(&host.batch.entries[0], &sources, l2)
        };
        verify(&calls).expect("flat successful mutable source beside a static read");

        let mut nested = calls.clone();
        let mut callback = action(2, Some(1), RollupId::MAINNET, l2);
        callback.value = U256::ZERO;
        nested.push(callback);
        verify(&nested).expect("a static read and nested mutable write share one host");
        let mut failed = calls.clone();
        failed[1].outcome = crate::ExecutionOutcome::Resolved {
            return_data: vec![0x41],
            gas_used: 21_001,
            success: false,
        };
        let mut rollback = calls;
        rollback[1].revert_spans = vec![1];
        verify(&failed).expect("a failed mutable root beside a read retains its exact outcome");
        verify(&rollback).expect("a source rollback restores the mutable cursor beside reads");
    }

    #[test]
    fn nested_static_callback_is_position_bound_without_mutating_the_source_hash() {
        let l2 = RollupId(1);
        let mut calls = vec![
            action(0, None, RollupId::MAINNET, l2),
            action(1, Some(0), l2, RollupId::MAINNET),
        ];
        calls[1].call_mode = CallMode::Static;
        calls[1].value = U256::ZERO;

        let mut batches = materialize_batches(&calls, RollupId::MAINNET)
            .expect("materialize nested static callback");
        let mut l1 = batches.remove(&RollupId::MAINNET).expect("L1 source");
        let l2_output = batches.remove(&l2).expect("L2 delivery");
        l1.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(0xe1),
            newRoot: B256::with_last_byte(0xe2),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut l1.batch, &l1.entry_traces)
            .expect("finalize nested static callback");

        let row = &l2_output.batch.entries[0].expectedL1ToL2Calls[0];
        assert_eq!(row.revertedOrStaticRollingHash, B256::ZERO);
        assert!(row.l2ToL1Calls.is_empty());
        assert!(l1.batch.entries[0].l2ToL1Calls[0].isStatic);

        let verified = crate::entries::verify_nested_pair(
            &l1.batch.entries[0],
            &l2_output.batch.entries[0],
            crate::entries::NestedPairDirection::Inbound,
            l2,
        )
        .expect("verify nested static callback");
        assert!(verified.l2_outbound_hashes.is_empty());
    }

    #[test]
    fn rejects_non_contiguous_parent_preorder() {
        let calls = vec![
            action(0, None, RollupId(1), RollupId::MAINNET),
            action(1, None, RollupId(1), RollupId(2)),
            action(2, Some(0), RollupId::MAINNET, RollupId(1)),
        ];
        let error = materialize_batches(&calls, RollupId(1)).expect_err("not preorder");
        assert!(matches!(
            error.kind(),
            ProtocolErrorKind::InvalidEncoding(reason)
                if reason.contains("contiguous preorder")
        ));
    }
}
