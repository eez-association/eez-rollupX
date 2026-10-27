//! Independent verification of L1/L2 entry pairs and outbound root groups.
//!
//! The verifier is the inverse of the protocol's `TableGenerator`: unified
//! rows are consumed in forward order, their position keys select the next
//! call in the opposite chain's current insertion array, and rolling hashes
//! advance on both chains exactly as execution alternates between them.

use alloy_primitives::{B256, I256, U256, keccak256};

use crate::abi::{ExecutionEntrySol, L2StaticExecutionEntrySol, L2ToL1CallSol};
use crate::action::{CallHashInput, CallMode, common_cross_chain_call_hash, l2_outbound_call_hash};
use crate::rolling_hash::{EntryRollingHash, StaticCallRollingHash};
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
    finish_facts(facts)
}

/// Verify a top-level L2 static source entry against the immediate L1 host
/// that executes the read against live L1 state.
pub fn verify_static_outbound_pair(
    l1_entry: &ExecutionEntrySol,
    l2_entry: &L2StaticExecutionEntrySol,
    l2_rollup_id: RollupId,
) -> ProtocolResult<NestedPairVerification> {
    verify_outbound_source_group(
        l1_entry,
        &[crate::system_tx::OutboundSourceEntry::Static(
            l2_entry.clone(),
        )],
        l2_rollup_id,
    )
}

fn finish_facts(facts: AccumulatedFacts) -> ProtocolResult<NestedPairVerification> {
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
    })
}

/// Verify a whole outbound host against the canonical load order (mutable
/// entries followed by static entries). Static roots are terminal lookups:
/// they do not advance the mutable cursor or emit outbound events.
/// Conflicting results for the same (position, proxy hash) key are rejected
/// because EEZL2 resolves that key using its first matching cached entry.
///
/// Mutable-only sources retain nested/revert verification. Groups containing
/// static sources (with or without static callbacks to L2) support successful
/// flat mutable roots and no root revert spans. The caller must bind the
/// complete load and receipt evidence before treating the returned facts as
/// authorization.
///
/// # Errors
///
/// Rejects unsupported table shapes, noncanonical source order, incomplete
/// coverage, conflicting static results, and mismatched identities or hashes.
pub fn verify_outbound_source_group(
    l1_entry: &ExecutionEntrySol,
    sources: &[crate::system_tx::OutboundSourceEntry],
    l2_rollup_id: RollupId,
) -> ProtocolResult<NestedPairVerification> {
    use crate::system_tx::OutboundSourceEntry;
    let mut mutable = Vec::new();
    let mut statics = Vec::new();
    for source in sources {
        match source {
            OutboundSourceEntry::Mutable(entry) if statics.is_empty() => {
                mutable.push(entry.clone())
            }
            OutboundSourceEntry::Static(entry) => statics.push(entry.clone()),
            OutboundSourceEntry::Mutable(_) => {
                return Err(ProtocolErrorKind::InvalidEncoding(
                    "outbound load must put mutable entries before static entries".into(),
                )
                .into());
            }
        }
    }
    verify_outbound_tables(l1_entry, &mutable, &statics, l2_rollup_id)
}

fn verify_outbound_group(
    l1_entry: &ExecutionEntrySol,
    l2_entries: &[ExecutionEntrySol],
    l2_rollup_id: RollupId,
) -> ProtocolResult<NestedPairVerification> {
    verify_outbound_tables(l1_entry, l2_entries, &[], l2_rollup_id)
}

fn verify_outbound_tables(
    l1_entry: &ExecutionEntrySol,
    l2_entries: &[ExecutionEntrySol],
    static_entries: &[L2StaticExecutionEntrySol],
    l2_rollup_id: RollupId,
) -> ProtocolResult<NestedPairVerification> {
    if l2_rollup_id.is_mainnet() {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "nested group L2 rollup id must be non-zero".into(),
        )
        .into());
    }
    if l2_entries.is_empty() && static_entries.is_empty() {
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
    // Host rows are allowed only for static callbacks: flat mutable sources
    // make no call that could consume a row, and `finish` rejects unused rows.
    if !static_entries.is_empty()
        && (!l1_entry.success
            || l2_entries.iter().any(|entry| {
                !entry.l2ToL1Calls.is_empty() || !entry.expectedL1ToL2Calls.is_empty()
            }))
    {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "mixed outbound groups require a successful flat host and flat mutable sources".into(),
        )
        .into());
    }
    let mut static_results = std::collections::BTreeMap::new();
    for entry in static_entries {
        crate::system_tx::check_l2_static_entry_shape(entry, "outbound static source")
            .map_err(ProtocolErrorKind::InvalidEncoding)?;
        let result = (entry.success, &entry.returnData);
        if static_results
            .insert((entry.expectedEntryIndex, entry.proxyEntryHash), result)
            .is_some_and(|first| first != result)
        {
            return Err(ProtocolErrorKind::InvalidEncoding(
                "outbound static sources have conflicting results for the same lookup key".into(),
            )
            .into());
        }
    }
    // Reverted spans reuse mutable cursor slots, whereas static pool rows are
    // never consumed. Keep that extension out of the mixed profile until its
    // source-count and replay partitioning are specified together.
    if !static_entries.is_empty()
        && (l1_entry
            .l2ToL1Calls
            .iter()
            .any(|call| call.revertNextNCalls != 0)
            || l2_entries.iter().any(|entry| !entry.success))
    {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "mixed outbound groups do not support reverted mutable roots or root spans".into(),
        )
        .into());
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
    if l2_entries.len() + static_entries.len() != source_count {
        return Err(ProtocolErrorKind::InvalidEncoding(format!(
            "outbound L1 host carries {source_count} source roots but the L2 table has {} entries",
            l2_entries.len() + static_entries.len(),
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
    let mut seen_sources = vec![false; l2_entries.len()];
    let mut static_cursor = 0usize;
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
            if l1.peek_current_call().is_some_and(|root| root.isStatic) {
                let source = static_entries.get(static_cursor).ok_or_else(|| {
                    crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                        "outbound static root has no source lookup".into(),
                    ))
                })?;
                let root = l1.take_current_call().ok_or_else(|| {
                    crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                        "outbound static root is missing".into(),
                    ))
                })?;
                if source.expectedEntryIndex != U256::from(source_cursor)
                    || root.revertNextNCalls != 0
                    || root.gas != 0
                    || !root.value.is_zero()
                    || source_call_hash(&root, l2_rollup_id, RollupId::MAINNET)?
                        != source.proxyEntryHash
                {
                    return Err(ProtocolErrorKind::InvalidEncoding(
                        "outbound static root does not match its source lookup".into(),
                    )
                    .into());
                }
                l1.rolling.call_begin(destination_call_hash(
                    &root,
                    l2_rollup_id,
                    RollupId::MAINNET,
                )?);
                resolve_static_callbacks(&mut l1, source, l2_rollup_id)?;
                l1.rolling.call_end(source.success, &source.returnData);
                static_cursor += 1;
                continue;
            }
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
    if seen_sources.iter().any(|seen| !seen) || static_cursor != static_entries.len() {
        return Err(ProtocolErrorKind::InvalidEncoding(format!(
            "outbound group did not exercise every source entry: {seen_sources:?}",
        ))
        .into());
    }
    l1.finish()?;
    finish_facts(facts)
}

/// Bind an L2 static source's callbacks to the L1 host rows that answer them.
///
/// While L1 executes the static root, each callback to L2 resolves the host's
/// next static row, keyed by the callback hash and the live host rolling hash
/// (which a static row does not advance). L2 executes the same callbacks from
/// `incomingCalls` and requires their results to fold to the source's
/// `rollingHash`. Matching that fold to the rows' results makes the values L1
/// returned the ones L2 computes.
///
/// Repeated identical callbacks share one position key, because static rows
/// leave the host rolling hash unchanged. L1 does not advance its cursor for a
/// static read and resolves every repeat to the first matching row, so the
/// repeats must carry the same result.
fn resolve_static_callbacks(
    l1: &mut ChainState<'_>,
    source: &L2StaticExecutionEntrySol,
    l2_rollup_id: RollupId,
) -> ProtocolResult<()> {
    let mut results = StaticCallRollingHash::new();
    let mut resolved = std::collections::HashMap::new();
    for callback in &source.incomingCalls {
        let callback = crate::system_tx::lift_call_from_l2(callback);
        let callback_hash = source_call_hash(&callback, RollupId::MAINNET, l2_rollup_id)?;
        let row = l1.next_row().cloned().ok_or_else(|| {
            crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                "outbound static callback has no L1 host row".into(),
            ))
        })?;
        if !callback.isStatic
            || row.expectedL1toL2Hash != position_key(callback_hash, l1.rolling.current())
            || !row.success
            || !row.l2ToL1Calls.is_empty()
            || row.revertedOrStaticRollingHash != B256::ZERO
        {
            return Err(ProtocolErrorKind::InvalidEncoding(
                "outbound static callback does not match its L1 host row".into(),
            )
            .into());
        }
        if resolved
            .insert(row.expectedL1toL2Hash, row.returnData.clone())
            .is_some_and(|first| first != row.returnData)
        {
            return Err(ProtocolErrorKind::InvalidEncoding(
                "outbound static callbacks repeat a lookup key with different results".into(),
            )
            .into());
        }
        l1.mark_static_row();
        results.append(row.success, &row.returnData);
    }
    if results.current() != source.rollingHash {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "outbound static callback results differ from the L2 static rolling hash".into(),
        )
        .into());
    }
    Ok(())
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
