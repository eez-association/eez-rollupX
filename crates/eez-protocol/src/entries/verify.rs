//! Independent verification of mutable L1/L2 entry pairs and root groups.
//!
//! The verifier is the inverse of the protocol's `TableGenerator`: unified
//! rows are consumed in forward order, their position keys select the next
//! call in the opposite chain's current insertion array, and rolling hashes
//! advance on both chains exactly as execution alternates between them.

use alloy_primitives::{B256, I256, U256, keccak256};

use crate::abi::{ExecutionEntrySol, L2ToL1CallSol, StaticExecutionEntrySol};
use crate::action::{CallHashInput, CallMode, common_cross_chain_call_hash, l2_outbound_call_hash};
use crate::rolling_hash::EntryRollingHash;
use crate::{ProtocolErrorKind, ProtocolResult, RollupId};

const MAX_NESTING_DEPTH: usize = 1024;

/// Which chain originated the root call represented by the pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NestedPairDirection {
    /// An L2 user transaction called L1. The L1 entry is its zero-hash host.
    Outbound,
    /// An L1 user transaction called this L2. The L1 entry is the source entry.
    Inbound,
}

/// Facts recomputed while verifying a nested pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NestedPairVerification {
    /// Every committed call that left L2, in execution order. These hashes
    /// must match the L2 manager's emitted events for the corresponding user
    /// transaction. Calls inside a reverted proxy frame are deliberately
    /// omitted because their receipt events and value transfers roll back.
    pub l2_outbound_hashes: Vec<B256>,
    /// Net L1→L2 value minus L2→L1 value for this rollup.
    pub ether_delta: I256,
    /// Number of separate L1→L2 callback deliveries belonging to each
    /// source-L2 root. Canonical mutable outbound entries contain callbacks
    /// directly, so each count is zero. Empty for inbound verification.
    pub outbound_callback_counts: Vec<usize>,
}

/// Verify one settlement entry against the complete ordered L2 execution
/// group that realizes it.
///
/// Inbound (L1-origin) effects have exactly one L2 delivery entry. Outbound
/// (L2-origin) effects have one L2 source entry per live cursor slot and one L1
/// host for the whole L2 user transaction. Calls returning to L2 are contained
/// by the corresponding source entry rather than represented as later
/// callback-delivery transactions.
pub fn verify_nested_group(
    l1_entry: &ExecutionEntrySol,
    l2_entries: &[ExecutionEntrySol],
    direction: NestedPairDirection,
    l2_rollup_id: RollupId,
) -> ProtocolResult<NestedPairVerification> {
    match direction {
        NestedPairDirection::Inbound => {
            let [l2_entry] = l2_entries else {
                return Err(ProtocolErrorKind::InvalidEncoding(format!(
                    "inbound nested effect requires exactly one L2 delivery entry; found {}",
                    l2_entries.len(),
                ))
                .into());
            };
            verify_nested_pair(l1_entry, l2_entry, direction, l2_rollup_id)
        }
        NestedPairDirection::Outbound => verify_outbound_group(l1_entry, l2_entries, l2_rollup_id),
    }
}

/// Verify one L1 settlement entry against its exact L2 derivation sidecar.
///
/// This supports the runtime's mutable two-chain profile, including terminal
/// failed nested rows. It checks every position key, flat/sub-array partition, return value, final
/// rolling hash, call identity, and net ether delta without trusting the
/// composer that produced either table.
pub fn verify_nested_pair(
    l1_entry: &ExecutionEntrySol,
    l2_entry: &ExecutionEntrySol,
    direction: NestedPairDirection,
    l2_rollup_id: RollupId,
) -> ProtocolResult<NestedPairVerification> {
    if l2_rollup_id.is_mainnet() {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "nested pair L2 rollup id must be non-zero".into(),
        )
        .into());
    }
    validate_entry_shape(l1_entry, "L1")?;
    validate_entry_shape(l2_entry, "L2")?;
    if l1_entry.rootUpdates.is_empty() {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "nested pair L1 entry has no root updates".into(),
        )
        .into());
    }
    if !l2_entry.rootUpdates.is_empty() {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "nested pair L2 sidecar carries root updates".into(),
        )
        .into());
    }

    let l1_seed = EntryRollingHash::seed_for_l1(
        l1_entry
            .rootUpdates
            .iter()
            .map(|update| (update.rollupId, update.currentRoot)),
        l1_entry.proxyEntryHash,
    );
    let l2_seed = EntryRollingHash::seed_for_l2(l2_entry.proxyEntryHash);
    let mut l1 = ChainState::new(l1_entry, RollupId::MAINNET, l1_seed);
    let mut l2 = ChainState::new(l2_entry, l2_rollup_id, l2_seed);
    let mut facts = AccumulatedFacts::default();

    match direction {
        NestedPairDirection::Outbound => {
            if l1_entry.proxyEntryHash != B256::ZERO
                || l1_entry.destinationRollupId != l2_rollup_id.0
                || l2_entry.destinationRollupId != RollupId::MAINNET.0
            {
                return Err(ProtocolErrorKind::InvalidEncoding(
                    "outbound nested pair has inconsistent routing metadata".into(),
                )
                .into());
            }
            if !l1_entry.returnData.is_empty() {
                return Err(ProtocolErrorKind::InvalidEncoding(
                    "outbound L1 host return data must be empty".into(),
                )
                .into());
            }
            let root = l1.take_current_call().ok_or_else(|| {
                crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                    "outbound L1 host has no root flat call".into(),
                ))
            })?;
            let root_hash = source_call_hash(&root, l2_rollup_id, RollupId::MAINNET)?;
            if root_hash != l2_entry.proxyEntryHash {
                return Err(ProtocolErrorKind::InvalidEncoding(
                    "outbound root hash does not match the source-L2 proxy hash".into(),
                )
                .into());
            }
            let reverted_facts = (!l2_entry.success).then(|| facts.clone());
            facts.record_value(l2_rollup_id, root.value)?;
            facts.l2_outbound_hashes.push(root_hash);
            execute_call(
                &mut l1,
                &mut l2,
                root,
                l2_entry.success,
                &l2_entry.returnData,
                &mut facts,
                0,
            )?;
            if let Some(saved) = reverted_facts {
                facts = saved;
            }
        }
        NestedPairDirection::Inbound => {
            if l1_entry.proxyEntryHash == B256::ZERO
                || l1_entry.destinationRollupId != l2_rollup_id.0
                || l2_entry.destinationRollupId != l2_rollup_id.0
                || l1_entry.proxyEntryHash != l2_entry.proxyEntryHash
            {
                return Err(ProtocolErrorKind::InvalidEncoding(
                    "inbound nested pair has inconsistent routing metadata".into(),
                )
                .into());
            }
            let root = l2.take_current_call().ok_or_else(|| {
                crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                    "inbound L2 entry has no root incoming call".into(),
                ))
            })?;
            let root_hash = source_call_hash(&root, RollupId::MAINNET, l2_rollup_id)?;
            if root_hash != l1_entry.proxyEntryHash {
                return Err(ProtocolErrorKind::InvalidEncoding(
                    "inbound root hash does not match the entry proxy hash".into(),
                )
                .into());
            }
            facts.record_value(RollupId::MAINNET, root.value)?;
            execute_call(
                &mut l2,
                &mut l1,
                root,
                l1_entry.success,
                &l1_entry.returnData,
                &mut facts,
                0,
            )?;
            let delivery_matches = l2_entry.success
                && if l1_entry.success {
                    l2_entry.returnData == l1_entry.returnData
                } else {
                    l2_entry.returnData.is_empty()
                };
            if !delivery_matches {
                return Err(ProtocolErrorKind::InvalidEncoding(
                    "inbound L2 delivery outcome does not match the L1 root outcome".into(),
                )
                .into());
            }
        }
    }

    l1.finish()?;
    l2.finish()?;
    finish_facts(facts, Vec::new())
}

/// Verify a top-level L2 static source entry against the immediate L1 host
/// that executes the read against live L1 state.
pub fn verify_static_outbound_pair(
    l1_entry: &ExecutionEntrySol,
    l2_entry: &StaticExecutionEntrySol,
    l2_rollup_id: RollupId,
) -> ProtocolResult<NestedPairVerification> {
    if l2_rollup_id.is_mainnet() {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "static outbound L2 rollup id must be non-zero".into(),
        )
        .into());
    }
    validate_entry_shape(l1_entry, "L1 static outbound host")?;
    crate::system_tx::check_static_entry_shape(l2_entry, "L2 static outbound source")
        .map_err(ProtocolErrorKind::InvalidEncoding)?;
    if l1_entry.rootUpdates.is_empty()
        || l1_entry.proxyEntryHash != B256::ZERO
        || l1_entry.destinationRollupId != l2_rollup_id.0
        || !l1_entry.success
        || !l1_entry.returnData.is_empty()
        || !l1_entry.expectedL1ToL2Calls.is_empty()
    {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "static outbound host has inconsistent routing or outcome metadata".into(),
        )
        .into());
    }
    let [call] = l1_entry.l2ToL1Calls.as_slice() else {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "static outbound host must contain exactly one L2-to-L1 call".into(),
        )
        .into());
    };
    if !call.isStatic
        || call.sourceRollupId != l2_rollup_id.0
        || !call.value.is_zero()
        || call.revertNextNCalls != 0
        || call.gas != 0
        || !l2_entry.expectedRoots.is_empty()
        || l2_entry.destinationRollupId != RollupId::MAINNET.0
        || !l2_entry.l2ToL1Calls.is_empty()
        || l2_entry.rollingHash != B256::ZERO
    {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "static outbound source/host uses an unsupported table shape".into(),
        )
        .into());
    }
    let call_hash = source_call_hash(call, l2_rollup_id, RollupId::MAINNET)?;
    if call_hash != l2_entry.proxyEntryHash {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "static outbound source hash does not match the L1 call".into(),
        )
        .into());
    }
    let mut rolling = EntryRollingHash::seed_for_l1(
        l1_entry
            .rootUpdates
            .iter()
            .map(|update| (update.rollupId, update.currentRoot)),
        B256::ZERO,
    );
    rolling.call_begin(destination_call_hash(
        call,
        l2_rollup_id,
        RollupId::MAINNET,
    )?);
    rolling.call_end(l2_entry.success, &l2_entry.returnData);
    if rolling.current() != l1_entry.rollingHash {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "static outbound host rolling hash does not bind the cached result".into(),
        )
        .into());
    }
    Ok(NestedPairVerification {
        l2_outbound_hashes: Vec::new(),
        ether_delta: I256::ZERO,
        outbound_callback_counts: vec![0],
    })
}

fn finish_facts(
    facts: AccumulatedFacts,
    outbound_callback_counts: Vec<usize>,
) -> ProtocolResult<NestedPairVerification> {
    let inflow = I256::try_from(facts.l1_to_l2_value).map_err(|_| {
        crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
            "nested pair L1-to-L2 value exceeds I256".into(),
        ))
    })?;
    let outflow = I256::try_from(facts.l2_to_l1_value).map_err(|_| {
        crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
            "nested pair L2-to-L1 value exceeds I256".into(),
        ))
    })?;
    let ether_delta = inflow.checked_sub(outflow).ok_or_else(|| {
        crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
            "nested pair net ether delta exceeds I256".into(),
        ))
    })?;
    Ok(NestedPairVerification {
        l2_outbound_hashes: facts.l2_outbound_hashes,
        ether_delta,
        outbound_callback_counts,
    })
}

fn verify_outbound_group(
    l1_entry: &ExecutionEntrySol,
    l2_entries: &[ExecutionEntrySol],
    l2_rollup_id: RollupId,
) -> ProtocolResult<NestedPairVerification> {
    if l2_rollup_id.is_mainnet() {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "nested group L2 rollup id must be non-zero".into(),
        )
        .into());
    }
    if l2_entries.is_empty() {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "outbound effect has no L2 source entries".into(),
        )
        .into());
    }
    validate_entry_shape(l1_entry, "L1")?;
    for (index, entry) in l2_entries.iter().enumerate() {
        validate_entry_shape(entry, &format!("L2 group entry {index}"))?;
        if !entry.rootUpdates.is_empty() {
            return Err(ProtocolErrorKind::InvalidEncoding(format!(
                "L2 group entry {index} carries root updates"
            ))
            .into());
        }
    }
    if l1_entry.rootUpdates.is_empty() {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "outbound nested L1 entry has no root updates".into(),
        )
        .into());
    }
    if l1_entry.proxyEntryHash != B256::ZERO || l1_entry.destinationRollupId != l2_rollup_id.0 {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "outbound group has inconsistent L1 routing metadata".into(),
        )
        .into());
    }
    if !l1_entry.returnData.is_empty() {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "outbound L1 host return data must be empty".into(),
        )
        .into());
    }
    let source_count = crate::system_tx::outbound_settlement_root_counts(std::slice::from_ref(
        l1_entry,
    ))
    .map_err(|reason| crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(reason)))?[0];
    if l2_entries.len() != source_count {
        return Err(ProtocolErrorKind::InvalidEncoding(format!(
            "outbound L1 host carries {source_count} source roots but the L2 table has {} entries",
            l2_entries.len(),
        ))
        .into());
    }
    let source_entries = l2_entries;

    let l1_seed = EntryRollingHash::seed_for_l1(
        l1_entry
            .rootUpdates
            .iter()
            .map(|update| (update.rollupId, update.currentRoot)),
        l1_entry.proxyEntryHash,
    );
    let mut l1 = ChainState::new(l1_entry, RollupId::MAINNET, l1_seed);
    let mut facts = AccumulatedFacts::default();
    let mut call_index = 0usize;
    let mut source_cursor = 0usize;
    let mut seen_sources = vec![false; source_count];
    while call_index < l1_entry.l2ToL1Calls.len() {
        let marker = l1.peek_current_call().ok_or_else(|| {
            crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(format!(
                "outbound L1 host has no root call at index {call_index}",
            )))
        })?;
        let revert_span = usize::from(marker.revertNextNCalls);
        let group_len = revert_span.max(1);
        let group_end = call_index
            .checked_add(group_len)
            .filter(|end| *end <= l1_entry.l2ToL1Calls.len())
            .ok_or_else(|| {
                crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(format!(
                    "outbound root {call_index} revert span {revert_span} exceeds the root group"
                )))
            })?;
        let saved_facts = (revert_span != 0).then(|| facts.clone());
        let saved_source_cursor = source_cursor;

        for root_index in call_index..group_end {
            let source_entry = source_entries.get(source_cursor).ok_or_else(|| {
                crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(format!(
                    "outbound root {root_index} advances beyond {source_count} source cursor slots"
                )))
            })?;
            seen_sources[source_cursor] = true;
            if source_entry.destinationRollupId != RollupId::MAINNET.0 {
                return Err(ProtocolErrorKind::InvalidEncoding(format!(
                    "outbound L2 source entry {root_index} has inconsistent routing metadata",
                ))
                .into());
            }
            let root = l1.take_current_call().ok_or_else(|| {
                crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(format!(
                    "outbound L1 host has no root call at index {root_index}",
                )))
            })?;
            if root_index != call_index && root.revertNextNCalls != 0 {
                return Err(ProtocolErrorKind::InvalidEncoding(format!(
                    "outbound root {root_index} starts a nested revert span",
                ))
                .into());
            }
            let root_hash = source_call_hash(&root, l2_rollup_id, RollupId::MAINNET)?;
            if root_hash != source_entry.proxyEntryHash {
                return Err(ProtocolErrorKind::InvalidEncoding(format!(
                    "outbound root {root_index} does not match its L2 source proxy hash",
                ))
                .into());
            }

            let reverted_facts = (!source_entry.success).then(|| facts.clone());
            facts.record_value(l2_rollup_id, root.value)?;
            facts.l2_outbound_hashes.push(root_hash);
            let mut source_state = ChainState::new(
                source_entry,
                l2_rollup_id,
                EntryRollingHash::seed_for_l2(source_entry.proxyEntryHash),
            );
            execute_call(
                &mut l1,
                &mut source_state,
                root,
                source_entry.success,
                &source_entry.returnData,
                &mut facts,
                0,
            )?;
            source_state.finish()?;
            if let Some(saved) = reverted_facts {
                facts = saved;
            }
            source_cursor += 1;
        }
        if let Some(saved) = saved_facts {
            facts = saved;
            source_cursor = saved_source_cursor;
        }
        call_index = group_end;
    }
    if seen_sources.iter().any(|seen| !seen) {
        return Err(ProtocolErrorKind::InvalidEncoding(format!(
            "outbound group did not exercise every source entry: {seen_sources:?}",
        ))
        .into());
    }
    l1.finish()?;
    finish_facts(facts, vec![0; source_count])
}

fn execute_call(
    destination: &mut ChainState<'_>,
    source: &mut ChainState<'_>,
    call: L2ToL1CallSol,
    success: bool,
    return_data: &[u8],
    facts: &mut AccumulatedFacts,
    depth: usize,
) -> ProtocolResult<()> {
    if depth > MAX_NESTING_DEPTH {
        return Err(
            ProtocolErrorKind::Unsupported("nested call depth exceeds the verifier limit").into(),
        );
    }
    let destination_hash = destination_call_hash(&call, source.chain, destination.chain)?;
    destination.rolling.call_begin(destination_hash);

    loop {
        let Some(next_call) = source.peek_current_call().cloned() else {
            break;
        };
        let revert_span = usize::from(next_call.revertNextNCalls);
        let group_len = revert_span.max(1);
        if group_len > source.remaining_current_calls() {
            return Err(ProtocolErrorKind::InvalidEncoding(format!(
                "nested pair {} revert span {revert_span} exceeds its current call array",
                source.chain,
            ))
            .into());
        }
        let saved_region_state = (revert_span != 0).then(|| {
            (
                destination.rolling.clone(),
                destination.next_row,
                destination.call_cursors.clone(),
            )
        });
        let saved_region_facts = (revert_span != 0).then(|| facts.clone());
        let mut matched_all = true;
        for offset in 0..group_len {
            let current = source.peek_current_call().ok_or_else(|| {
                crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                    "nested revert span ended before all calls were consumed".into(),
                ))
            })?;
            if offset != 0 && current.revertNextNCalls != 0 {
                return Err(ProtocolErrorKind::InvalidEncoding(
                    "nested revert spans within one call array overlap".into(),
                )
                .into());
            }
            if !execute_next_nested_call(destination, source, facts, depth)? {
                if revert_span != 0 {
                    return Err(ProtocolErrorKind::InvalidEncoding(
                        "revert span call has no matching reentrant row".into(),
                    )
                    .into());
                }
                matched_all = false;
                break;
            }
        }
        if let Some((saved_hash, saved_row_cursor, saved_call_cursors)) = saved_region_state {
            destination.rolling = saved_hash;
            destination.next_row = saved_row_cursor;
            destination.call_cursors = saved_call_cursors;
            *facts = saved_region_facts.expect("saved with region hash");
        } else if !matched_all {
            // The unmarked call did not match the next row. Preserve the old
            // behavior: leave both cursors in place so `finish` reports the
            // complete mismatch with its recomputed position.
            break;
        }
    }

    destination.rolling.call_end(success, return_data);
    Ok(())
}

fn execute_next_nested_call(
    destination: &mut ChainState<'_>,
    source: &mut ChainState<'_>,
    facts: &mut AccumulatedFacts,
    depth: usize,
) -> ProtocolResult<bool> {
    let Some(next_call) = source.peek_current_call().cloned() else {
        return Ok(false);
    };
    let Some(row) = destination.next_row().cloned() else {
        return Ok(false);
    };
    let source_hash = source_call_hash(&next_call, destination.chain, source.chain)?;
    let recomputed_position = position_key(source_hash, destination.rolling.current());
    if row.expectedL1toL2Hash != recomputed_position {
        destination.last_position_mismatch = Some((row.expectedL1toL2Hash, recomputed_position));
        return Ok(false);
    }

    if next_call.isStatic {
        if !next_call.value.is_zero()
            || next_call.revertNextNCalls != 0
            || next_call.gas != 0
            || !row.l2ToL1Calls.is_empty()
            || row.revertedOrStaticRollingHash != B256::ZERO
        {
            return Err(ProtocolErrorKind::InvalidEncoding(
                "nested static row uses an unsupported table shape".into(),
            )
            .into());
        }
        let row_index = destination.mark_static_row();
        let next_call = source.take_current_call().ok_or_else(|| {
            crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                "nested pair call cursor changed while resolving a static row".into(),
            ))
        })?;
        source.rolling.call_begin(destination_call_hash(
            &next_call,
            destination.chain,
            source.chain,
        )?);
        source.rolling.call_end(row.success, &row.returnData);
        debug_assert_eq!(row_index + 1, destination.next_row);
        return Ok(true);
    }

    let row_index = destination.open_next_row(source_hash);
    let next_call = source.take_current_call().ok_or_else(|| {
        crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
            "nested pair call cursor changed while opening a row".into(),
        ))
    })?;
    let saved_facts = facts.clone();
    facts.record_value(destination.chain, next_call.value)?;
    if !destination.chain.is_mainnet() {
        facts.l2_outbound_hashes.push(source_hash);
    }
    execute_call(
        source,
        destination,
        next_call,
        row.success,
        &row.returnData,
        facts,
        depth + 1,
    )?;
    destination.close_row(row_index, row.success)?;
    if !row.success {
        *facts = saved_facts;
    }
    Ok(true)
}

struct ChainState<'a> {
    entry: &'a ExecutionEntrySol,
    chain: RollupId,
    rolling: EntryRollingHash,
    next_row: usize,
    frame_rows: Vec<usize>,
    frame_cursors: Vec<usize>,
    frame_hashes: Vec<EntryRollingHash>,
    seen_rows: Vec<bool>,
    call_cursors: Vec<usize>,
    last_position_mismatch: Option<(B256, B256)>,
}

impl<'a> ChainState<'a> {
    fn new(entry: &'a ExecutionEntrySol, chain: RollupId, rolling: EntryRollingHash) -> Self {
        Self {
            entry,
            chain,
            rolling,
            next_row: 0,
            frame_rows: Vec::new(),
            frame_cursors: Vec::new(),
            frame_hashes: Vec::new(),
            seen_rows: vec![false; entry.expectedL1ToL2Calls.len()],
            call_cursors: vec![0; entry.expectedL1ToL2Calls.len() + 1],
            last_position_mismatch: None,
        }
    }

    fn current_array(&self) -> &[L2ToL1CallSol] {
        match self.frame_rows.last().copied() {
            Some(row) => &self.entry.expectedL1ToL2Calls[row].l2ToL1Calls,
            None => &self.entry.l2ToL1Calls,
        }
    }

    fn cursor_slot(&self) -> usize {
        self.frame_rows.last().map_or(0, |row| row + 1)
    }

    fn peek_current_call(&self) -> Option<&L2ToL1CallSol> {
        let cursor = self.call_cursors[self.cursor_slot()];
        self.current_array().get(cursor)
    }

    fn remaining_current_calls(&self) -> usize {
        let cursor = self.call_cursors[self.cursor_slot()];
        self.current_array().len().saturating_sub(cursor)
    }

    fn take_current_call(&mut self) -> Option<L2ToL1CallSol> {
        let slot = self.cursor_slot();
        let call = self.current_array().get(self.call_cursors[slot]).cloned();
        if call.is_some() {
            self.call_cursors[slot] += 1;
        }
        call
    }

    fn next_row(&self) -> Option<&crate::abi::ExpectedL1ToL2CallSol> {
        self.entry.expectedL1ToL2Calls.get(self.next_row)
    }

    fn open_next_row(&mut self, call_hash: B256) -> usize {
        let row_index = self.next_row;
        let saved_cursor = self.next_row;
        self.next_row += 1;
        self.seen_rows[row_index] = true;
        self.frame_cursors.push(saved_cursor);
        self.frame_hashes.push(self.rolling.clone());
        self.rolling.nested_begin(call_hash);
        self.frame_rows.push(row_index);
        row_index
    }

    fn mark_static_row(&mut self) -> usize {
        let row_index = self.next_row;
        self.next_row += 1;
        self.seen_rows[row_index] = true;
        row_index
    }

    fn close_row(&mut self, row_index: usize, success: bool) -> ProtocolResult<()> {
        let closed_row = self.frame_rows.pop();
        debug_assert_eq!(closed_row, Some(row_index));
        let saved_cursor = self.frame_cursors.pop().ok_or_else(|| {
            crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                "nested verifier frame has no saved cursor".into(),
            ))
        })?;
        let saved_hash = self.frame_hashes.pop().ok_or_else(|| {
            crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                "nested verifier frame has no saved rolling hash".into(),
            ))
        })?;
        let row = &self.entry.expectedL1ToL2Calls[row_index];
        if success {
            self.rolling.nested_end();
        } else {
            if self.rolling.current() != row.revertedOrStaticRollingHash {
                return Err(ProtocolErrorKind::InvalidEncoding(format!(
                    "nested pair {} failed row {row_index} reverted rolling hash mismatch",
                    self.chain,
                ))
                .into());
            }
            // Solidity's terminal revert restores both the host rolling hash
            // and its forward-scan cursor. Keep `seen_rows` as verification
            // evidence that this otherwise-unconsumed row was exercised.
            self.rolling = saved_hash;
            self.next_row = saved_cursor;
        }
        Ok(())
    }

    fn finish(&self) -> ProtocolResult<()> {
        let call_lengths = std::iter::once(self.entry.l2ToL1Calls.len())
            .chain(
                self.entry
                    .expectedL1ToL2Calls
                    .iter()
                    .map(|row| row.l2ToL1Calls.len()),
            )
            .collect::<Vec<_>>();
        if !self.frame_rows.is_empty()
            || !self.frame_cursors.is_empty()
            || !self.frame_hashes.is_empty()
            || self.seen_rows.iter().any(|seen| !seen)
            || self.call_cursors != call_lengths
        {
            return Err(ProtocolErrorKind::InvalidEncoding(format!(
                "nested pair did not consume the complete {} table: cursor={}, \
                 seen_rows={:?}, call_cursors={:?}, call_lengths={call_lengths:?}, last_position_mismatch={:?}",
                self.chain,
                self.next_row,
                self.seen_rows,
                self.call_cursors,
                self.last_position_mismatch,
            ))
            .into());
        }
        if self.rolling.current() != self.entry.rollingHash {
            return Err(ProtocolErrorKind::InvalidEncoding(format!(
                "nested pair {} rolling hash mismatch",
                self.chain
            ))
            .into());
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
struct AccumulatedFacts {
    l2_outbound_hashes: Vec<B256>,
    l1_to_l2_value: U256,
    l2_to_l1_value: U256,
}

impl AccumulatedFacts {
    fn record_value(&mut self, source: RollupId, value: U256) -> ProtocolResult<()> {
        let total = if source.is_mainnet() {
            &mut self.l1_to_l2_value
        } else {
            &mut self.l2_to_l1_value
        };
        *total = total.checked_add(value).ok_or_else(|| {
            crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                "nested pair value sum overflow".into(),
            ))
        })?;
        Ok(())
    }
}

fn validate_entry_shape(entry: &ExecutionEntrySol, label: &str) -> ProtocolResult<()> {
    crate::system_tx::check_entry_shape(entry, label)
        .map_err(|reason| ProtocolErrorKind::InvalidEncoding(reason).into())
}

fn checked_call_input(
    call: &L2ToL1CallSol,
    expected_source: RollupId,
    target: RollupId,
) -> ProtocolResult<CallHashInput<'_>> {
    if call.sourceRollupId != expected_source.0 {
        return Err(ProtocolErrorKind::InvalidEncoding(format!(
            "nested pair call source {} does not match executing chain {expected_source}",
            call.sourceRollupId
        ))
        .into());
    }
    Ok(CallHashInput {
        call_mode: CallMode::from_is_static(call.isStatic),
        source_address: call.sourceAddress,
        source_rollup_id: expected_source,
        target_address: call.targetAddress,
        target_rollup_id: target,
        value: call.value,
        data: &call.data,
    })
}

fn destination_call_hash(
    call: &L2ToL1CallSol,
    expected_source: RollupId,
    target: RollupId,
) -> ProtocolResult<B256> {
    Ok(common_cross_chain_call_hash(checked_call_input(
        call,
        expected_source,
        target,
    )?))
}

fn source_call_hash(
    call: &L2ToL1CallSol,
    expected_source: RollupId,
    target: RollupId,
) -> ProtocolResult<B256> {
    let input = checked_call_input(call, expected_source, target)?;
    Ok(if expected_source.is_mainnet() {
        common_cross_chain_call_hash(input)
    } else {
        l2_outbound_call_hash(input, 0)
    })
}

fn position_key(call_hash: B256, rolling_hash: B256) -> B256 {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(call_hash.as_slice());
    input[32..].copy_from_slice(rolling_hash.as_slice());
    keccak256(input)
}
