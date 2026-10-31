//! Independent verification of L1/L2 entry pairs and outbound root groups.
//!
//! The verifier is the inverse of the protocol's `TableGenerator`: unified
//! rows are consumed in forward order, their position keys select the next
//! call in the opposite chain's current insertion array, and rolling hashes
//! advance on both chains exactly as execution alternates between them.

use alloy_primitives::{B256, Bytes, I256, U256, keccak256};

use crate::abi::{ExecutionEntrySol, L2StaticExecutionEntrySol, L2ToL1CallSol};
use crate::action::{CallHashInput, CallMode, common_cross_chain_call_hash, l2_outbound_call_hash};
use crate::rolling_hash::{EntryRollingHash, StaticCallRollingHash};
use crate::{ProtocolErrorKind, ProtocolResult, RollupId};

use super::MAX_NESTING_DEPTH;

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
/// entries followed by static entries). Static roots are read-only lookups:
/// they do not advance the mutable cursor or emit outbound events.
/// Conflicting results for the same (position, proxy hash, callback hash) key are rejected
/// because EEZL2 resolves that key using its first matching cached entry.
///
/// Mutable-only sources retain nested/revert verification. Groups containing
/// static sources support nested reads and successful mutable roots, with no
/// root revert spans. The caller must bind the
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
    if !l1_entry.success {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "outbound groups require a successful settlement host".into(),
        )
        .into());
    }
    let mut static_results = std::collections::BTreeMap::new();
    for entry in static_entries {
        crate::system_tx::check_l2_static_entry_shape(entry, "outbound static source")
            .map_err(ProtocolErrorKind::InvalidEncoding)?;
        let result = (entry.success, &entry.returnData);
        if static_results
            .insert(
                (
                    entry.expectedEntryIndex,
                    entry.proxyEntryHash,
                    entry.rollingHash,
                ),
                result,
            )
            .is_some_and(|first| first != result)
        {
            return Err(ProtocolErrorKind::InvalidEncoding(
                "outbound static sources have conflicting results for the same lookup key".into(),
            )
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
    if l1_entry.l2ToL1Calls.is_empty() {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "outbound settlement host has no root calls".into(),
        )
        .into());
    }
    // Root attempts need not have distinct source slots: a failed resolver
    // restores its cursor and an identical retry selects the same entry.
    // The walk below binds each attempt and requires coverage of every slot.
    let source_count = l2_entries.len();
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
    let mut root_scopes = Vec::new();
    while call_index < l1_entry.l2ToL1Calls.len() {
        let root_index = call_index;
        let marker = l1.peek_current_call().ok_or_else(|| {
            crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(format!(
                "outbound L1 host has no root call at index {call_index}",
            )))
        })?;
        let span = usize::from(marker.revertNextNCalls);
        if span != 0 {
            let end = call_index
                .checked_add(span)
                .filter(|end| *end <= l1_entry.l2ToL1Calls.len())
                .ok_or_else(|| {
                    ProtocolErrorKind::InvalidEncoding(
                        "outbound root rollback exceeds its array".into(),
                    )
                })?;
            if root_scopes.last().is_some_and(|(outer, _, _)| end > *outer) {
                return Err(ProtocolErrorKind::InvalidEncoding(
                    "outbound root rollback crosses its enclosing scope".into(),
                )
                .into());
            }
            root_scopes.push((end, source_cursor, facts.clone()));
        }
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
            static_cursor += 1;
            resolve_static_callbacks(
                &mut l1,
                source,
                static_entries,
                &mut static_cursor,
                source_cursor,
                l2_rollup_id,
                0,
            )?;
            l1.rolling.call_end(source.success, &source.returnData);
        } else {
            let root = l1.peek_current_call().expect("root checked above");
            let expected_hash = source_call_hash(root, l2_rollup_id, RollupId::MAINNET)?;
            let original_source_cursor = source_cursor;
            source_cursor = source_entries
                .iter()
                .enumerate()
                .skip(source_cursor)
                .find(|(_, entry)| entry.proxyEntryHash == expected_hash)
                .map(|(index, _)| index)
                .ok_or_else(|| {
                    ProtocolErrorKind::InvalidEncoding(format!(
                        "outbound root {root_index} has no forward source candidate"
                    ))
                })?;
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
            source_cursor = if source_entry.success {
                source_cursor + 1
            } else {
                original_source_cursor
            };
        }
        call_index += 1;
        while root_scopes
            .last()
            .is_some_and(|(end, _, _)| *end == call_index)
        {
            let (_, saved_cursor, saved_facts) = root_scopes.pop().expect("scope checked above");
            facts = saved_facts;
            source_cursor = saved_cursor;
        }
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
/// repeats with the same callback commitment must carry the same result.
fn resolve_static_callbacks(
    l1: &mut ChainState<'_>,
    source: &L2StaticExecutionEntrySol,
    static_entries: &[L2StaticExecutionEntrySol],
    static_cursor: &mut usize,
    source_cursor: usize,
    l2_rollup_id: RollupId,
    depth: usize,
) -> ProtocolResult<()> {
    if depth > MAX_NESTING_DEPTH {
        return Err(ProtocolErrorKind::Unsupported(
            "static callback depth exceeds the verifier limit",
        )
        .into());
    }
    let mut results = StaticCallRollingHash::new();
    for callback in &source.incomingCalls {
        let callback = crate::system_tx::lift_call_from_l2(callback);
        let callback_hash = source_call_hash(&callback, RollupId::MAINNET, l2_rollup_id)?;
        let row_index = l1
            .next_matching_row(position_key(callback_hash, l1.rolling.current()))
            .ok_or_else(|| {
                crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                    if l1.entry.expectedL1ToL2Calls.is_empty() {
                        "outbound static callback has no L1 host row"
                    } else {
                        "outbound static callback does not match its L1 host row"
                    }
                    .into(),
                ))
            })?;
        let row = l1.entry.expectedL1ToL2Calls[row_index].clone();
        if !callback.isStatic
            || row.expectedL1toL2Hash != position_key(callback_hash, l1.rolling.current())
        {
            return Err(ProtocolErrorKind::InvalidEncoding(
                "outbound static callback does not match its L1 host row".into(),
            )
            .into());
        }
        l1.mark_static_row(row_index)?;
        let mut callback_results = StaticCallRollingHash::new();
        for nested in &row.l2ToL1Calls {
            let lookup = static_entries.get(*static_cursor).ok_or_else(|| {
                crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                    "recursive static callback has no source pool entry".into(),
                ))
            })?;
            if !nested.isStatic
                || !nested.value.is_zero()
                || nested.gas != 0
                || nested.revertNextNCalls != 0
                || lookup.expectedEntryIndex != U256::from(source_cursor)
                || lookup.proxyEntryHash
                    != source_call_hash(nested, l2_rollup_id, RollupId::MAINNET)?
            {
                return Err(ProtocolErrorKind::InvalidEncoding(
                    "recursive static callback does not match its source pool entry".into(),
                )
                .into());
            }
            *static_cursor += 1;
            l1.call_cursors[row_index + 1] += 1;
            resolve_static_callbacks(
                l1,
                lookup,
                static_entries,
                static_cursor,
                source_cursor,
                l2_rollup_id,
                depth + 1,
            )?;
            callback_results.append(lookup.success, &lookup.returnData);
        }
        if callback_results.current() != row.revertedOrStaticRollingHash {
            return Err(ProtocolErrorKind::InvalidEncoding(
                "static row callback commitment does not match its source results".into(),
            )
            .into());
        }
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
    let failed_frame = (!success).then(|| {
        (
            destination.rolling.clone(),
            destination.next_row,
            facts.clone(),
        )
    });

    let slot = source.cursor_slot();
    let mut scopes = Vec::new();
    while let Some(next_call) = source.peek_current_call().cloned() {
        let index = source.call_cursors[slot];
        let span = usize::from(next_call.revertNextNCalls);
        if span != 0 {
            let end = index
                .checked_add(span)
                .filter(|end| *end <= source.current_array().len())
                .ok_or_else(|| {
                    ProtocolErrorKind::InvalidEncoding(
                        "nested rollback exceeds its current call array".into(),
                    )
                })?;
            if scopes.last().is_some_and(|(outer, _, _, _)| end > *outer) {
                return Err(ProtocolErrorKind::InvalidEncoding(
                    "nested rollback crosses its enclosing scope".into(),
                )
                .into());
            }
            scopes.push((
                end,
                destination.rolling.clone(),
                destination.next_row,
                facts.clone(),
            ));
        }
        if !execute_next_nested_call(destination, source, facts, depth)? {
            if !scopes.is_empty() {
                return Err(ProtocolErrorKind::InvalidEncoding(
                    "revert span call has no matching reentrant row".into(),
                )
                .into());
            }
            // A remaining unmarked call must be matched by a later outer
            // frame; the final table checks reject any unconsumed remainder.
            break;
        }
        while scopes
            .last()
            .is_some_and(|(end, _, _, _)| *end == source.call_cursors[slot])
        {
            let (_, hash, cursor, saved_facts) = scopes.pop().expect("scope checked above");
            destination.rolling = hash;
            destination.next_row = cursor;
            *facts = saved_facts;
        }
    }
    if !scopes.is_empty() {
        return Err(ProtocolErrorKind::InvalidEncoding(
            "nested call ended with an open rollback scope".into(),
        )
        .into());
    }

    // Restore protocol state, not proof of the call arrays already executed.
    // Each array invocation must finish; reopening a row starts a fresh copy
    // of that row's array, just as the manager does on a retry.
    if let Some((hash, cursor, saved_facts)) = failed_frame {
        destination.rolling = hash;
        destination.next_row = cursor;
        *facts = saved_facts;
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
    let source_hash = source_call_hash(&next_call, destination.chain, source.chain)?;
    let recomputed_position = position_key(source_hash, destination.rolling.current());
    let saved_cursor = destination.next_row;
    let row_index = destination.next_matching_row(recomputed_position);
    let Some(row_index) = row_index else {
        return Ok(false);
    };
    let row = destination.entry.expectedL1ToL2Calls[row_index].clone();

    if next_call.isStatic {
        if !next_call.value.is_zero() || next_call.gas != 0 {
            return Err(ProtocolErrorKind::InvalidEncoding(
                "nested static row uses an unsupported table shape".into(),
            )
            .into());
        }
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
        let selected =
            resolve_static_row_pair(destination, source, recomputed_position, depth + 1)?;
        let row = &destination.entry.expectedL1ToL2Calls[selected];
        source.rolling.call_end(row.success, &row.returnData);
        return Ok(true);
    }

    destination.next_row = row_index;
    let row_index = destination.open_next_row(source_hash, saved_cursor);
    let next_call = source.take_current_call().ok_or_else(|| {
        crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
            "nested pair call cursor changed while opening a row".into(),
        ))
    })?;
    let saved_facts = (!row.success).then(|| facts.clone());
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
    if let Some(saved_facts) = saved_facts {
        *facts = saved_facts;
    }
    Ok(true)
}

/// Resolve a pending read against fixed host positions. Static callbacks change
/// neither tagged hashes nor mutable cursors, so the result for a (chain, key)
/// is stable for this read. Memoizing it bounds candidate exploration by the
/// table's rows and callbacks rather than by the number of recursive paths.
/// A new resolver is required after every direct call's CALL_BEGIN fold.
struct StaticRowResolver<'a> {
    entries: [&'a ExecutionEntrySol; 2],
    chains: [RollupId; 2],
    positions: [B256; 2],
    cursors: [usize; 2],
    resolutions: std::collections::HashMap<(usize, B256), StaticResolution>,
    children: std::collections::HashMap<(usize, usize), Vec<usize>>,
}

#[derive(Clone, Copy)]
enum StaticResolution {
    Pending,
    Resolved(Option<ResolvedStaticRow>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResolvedStaticRow {
    row: usize,
    /// Longest callback path below this row. Cached suffixes still count
    /// against the nesting bound when reached through a different prefix.
    height: usize,
}

impl StaticRowResolver<'_> {
    fn resolve(
        &mut self,
        side: usize,
        key: B256,
        depth: usize,
    ) -> ProtocolResult<Option<ResolvedStaticRow>> {
        if depth > MAX_NESTING_DEPTH {
            return Err(ProtocolErrorKind::Unsupported(
                "static callback depth exceeds the verifier limit",
            )
            .into());
        }
        if let Some(resolution) = self.resolutions.get(&(side, key)) {
            return match resolution {
                StaticResolution::Resolved(Some(row)) if row.height > MAX_NESTING_DEPTH - depth => {
                    Err(ProtocolErrorKind::Unsupported(
                        "static callback depth exceeds the verifier limit",
                    )
                    .into())
                }
                StaticResolution::Resolved(row) => Ok(*row),
                StaticResolution::Pending => Err(ProtocolErrorKind::InvalidEncoding(
                    "static callback table contains a resolution cycle".into(),
                )
                .into()),
            };
        }
        self.resolutions
            .insert((side, key), StaticResolution::Pending);
        let entry = self.entries[side];
        let other = 1 - side;
        for (index, row) in entry
            .expectedL1ToL2Calls
            .iter()
            .enumerate()
            .skip(self.cursors[side])
        {
            if row.expectedL1toL2Hash != key {
                continue;
            }
            let mut results = StaticCallRollingHash::new();
            let mut children = Vec::with_capacity(row.l2ToL1Calls.len());
            let mut height = 0;
            for callback in &row.l2ToL1Calls {
                if !callback.isStatic
                    || !callback.value.is_zero()
                    || callback.gas != 0
                    || callback.revertNextNCalls != 0
                {
                    return Err(ProtocolErrorKind::InvalidEncoding(
                        "nested static callback does not match its host row".into(),
                    )
                    .into());
                }
                let child_key = position_key(
                    source_call_hash(callback, self.chains[other], self.chains[side])?,
                    self.positions[other],
                );
                let child = self.resolve(other, child_key, depth + 1)?.ok_or_else(|| {
                    crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                        "nested static callback has no matching row".into(),
                    ))
                })?;
                let result = &self.entries[other].expectedL1ToL2Calls[child.row];
                results.append(result.success, &result.returnData);
                height = height.max(child.height + 1);
                children.push(child.row);
            }
            if results.current() == row.revertedOrStaticRollingHash {
                self.children.insert((side, index), children);
                let resolved = ResolvedStaticRow { row: index, height };
                self.resolutions
                    .insert((side, key), StaticResolution::Resolved(Some(resolved)));
                return Ok(Some(resolved));
            }
        }
        self.resolutions
            .insert((side, key), StaticResolution::Resolved(None));
        Ok(None)
    }
}

fn resolve_static_row_pair(
    lookup: &mut ChainState<'_>,
    executing: &mut ChainState<'_>,
    key: B256,
    depth: usize,
) -> ProtocolResult<usize> {
    let mut resolver = StaticRowResolver {
        entries: [lookup.entry, executing.entry],
        chains: [lookup.chain, executing.chain],
        positions: [lookup.rolling.current(), executing.rolling.current()],
        cursors: [lookup.next_row, executing.next_row],
        resolutions: Default::default(),
        children: Default::default(),
    };
    let selected = resolver
        .resolve(0, key, depth)?
        .ok_or_else(|| {
            crate::ProtocolError::from(ProtocolErrorKind::InvalidEncoding(
                "nested static callback results differ from the row rolling hash".into(),
            ))
        })?
        .row;
    // Only successfully resolved rows in the selected callback graph establish
    // coverage. Candidate probing cannot mark an unchecked row as validated.
    // Shared subgraphs need one coverage update, not exponential traversal.
    let mut pending = vec![(0, selected)];
    let mut covered = std::collections::HashSet::new();
    while let Some((side, row)) = pending.pop() {
        if !covered.insert((side, row)) {
            continue;
        }
        if side == 0 {
            lookup.mark_static_row(row)?;
            lookup.call_cursors[row + 1] = lookup.entry.expectedL1ToL2Calls[row].l2ToL1Calls.len();
        } else {
            executing.mark_static_row(row)?;
            executing.call_cursors[row + 1] =
                executing.entry.expectedL1ToL2Calls[row].l2ToL1Calls.len();
        }
        pending.extend(
            resolver.children[&(side, row)]
                .iter()
                .map(|&child| (1 - side, child)),
        );
    }
    Ok(selected)
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
    static_results: std::collections::HashMap<(B256, B256), (bool, Bytes)>,
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
            static_results: std::collections::HashMap::new(),
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

    fn take_current_call(&mut self) -> Option<L2ToL1CallSol> {
        let slot = self.cursor_slot();
        let call = self.current_array().get(self.call_cursors[slot]).cloned();
        if call.is_some() {
            self.call_cursors[slot] += 1;
        }
        call
    }

    /// Lookup starts at the mutable cursor; coverage does not consume a row.
    /// The caller advances the cursor only after selecting a mutable call.
    fn next_matching_row(&self, key: B256) -> Option<usize> {
        self.entry
            .expectedL1ToL2Calls
            .iter()
            .enumerate()
            .skip(self.next_row)
            .find(|(_, row)| row.expectedL1toL2Hash == key)
            .map(|(index, _)| index)
    }

    fn open_next_row(&mut self, call_hash: B256, saved_cursor: usize) -> usize {
        let row_index = self.next_row;
        self.next_row += 1;
        self.seen_rows[row_index] = true;
        self.call_cursors[row_index + 1] = 0;
        self.frame_cursors.push(saved_cursor);
        self.frame_hashes.push(self.rolling.clone());
        self.rolling.nested_begin(call_hash);
        self.frame_rows.push(row_index);
        row_index
    }

    fn mark_static_row(&mut self, row_index: usize) -> ProtocolResult<()> {
        let row = &self.entry.expectedL1ToL2Calls[row_index];
        // Static resolution can revisit the same key across different pending
        // reads. Compare the complete lookup commitment over the whole host,
        // not just siblings in one callback frame.
        if self
            .static_results
            .insert(
                (row.expectedL1toL2Hash, row.revertedOrStaticRollingHash),
                (row.success, row.returnData.clone()),
            )
            .is_some_and(|first| first != (row.success, row.returnData.clone()))
        {
            return Err(ProtocolErrorKind::InvalidEncoding(
                "static callbacks repeat a lookup key with different results".into(),
            )
            .into());
        }
        self.seen_rows[row_index] = true;
        self.call_cursors[row_index + 1] = 0;
        Ok(())
    }

    fn close_row(&mut self, row_index: usize, success: bool) -> ProtocolResult<()> {
        if self.call_cursors[row_index + 1]
            != self.entry.expectedL1ToL2Calls[row_index].l2ToL1Calls.len()
        {
            return Err(ProtocolErrorKind::InvalidEncoding(
                "nested row invocation did not execute every call".into(),
            )
            .into());
        }
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
            if row.revertedOrStaticRollingHash != B256::ZERO {
                return Err(ProtocolErrorKind::InvalidEncoding(
                    "successful mutable row carries a reverted/static rolling hash".into(),
                )
                .into());
            }
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
                 seen_rows={:?}, call_cursors={:?}, call_lengths={call_lengths:?}",
                self.chain, self.next_row, self.seen_rows, self.call_cursors,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn static_callback(level: u8) -> L2ToL1CallSol {
        L2ToL1CallSol {
            isStatic: true,
            sourceRollupId: u64::from(level % 2),
            data: Bytes::from(vec![level]),
            sourceAddress: alloy_primitives::Address::ZERO,
            targetAddress: alloy_primitives::Address::ZERO,
            value: U256::ZERO,
            gas: 0,
            revertNextNCalls: 0,
        }
    }

    fn callback_key(level: u8) -> B256 {
        position_key(
            source_call_hash(
                &static_callback(level),
                RollupId(u64::from(level % 2)),
                RollupId(u64::from(1 - level % 2)),
            )
            .unwrap(),
            B256::ZERO,
        )
    }

    #[test]
    fn static_resolution_memoizes_shared_callback_subgraphs() {
        let mut entries = [ExecutionEntrySol::default(), ExecutionEntrySol::default()];
        const DEPTH: u8 = 40;
        for level in 0..=DEPTH {
            let callbacks = if level == DEPTH {
                vec![]
            } else {
                vec![static_callback(level + 1); 2]
            };
            let mut results = StaticCallRollingHash::new();
            for _ in &callbacks {
                results.append(true, &[level + 1]);
            }
            entries[usize::from(level % 2)].expectedL1ToL2Calls.push(
                crate::abi::ExpectedL1ToL2CallSol {
                    expectedL1toL2Hash: callback_key(level),
                    l2ToL1Calls: callbacks,
                    revertedOrStaticRollingHash: results.current(),
                    success: true,
                    returnData: Bytes::from(vec![level]),
                },
            );
        }
        let mut resolver = StaticRowResolver {
            entries: [&entries[0], &entries[1]],
            chains: [RollupId::MAINNET, RollupId(1)],
            positions: [B256::ZERO; 2],
            cursors: [0; 2],
            resolutions: Default::default(),
            children: Default::default(),
        };
        assert_eq!(
            resolver.resolve(0, callback_key(0), 0).unwrap(),
            Some(ResolvedStaticRow {
                row: 0,
                height: usize::from(DEPTH)
            })
        );
        // Expanding this graph as a tree would visit more than 2^40 frames.
        assert_eq!(resolver.resolutions.len(), usize::from(DEPTH) + 1);
        assert_eq!(resolver.children.len(), usize::from(DEPTH) + 1);
    }

    #[test]
    fn static_resolution_rejects_cycles_before_claiming_coverage() {
        let mut entries = [ExecutionEntrySol::default(), ExecutionEntrySol::default()];
        for side in 0..2_u8 {
            entries[usize::from(side)].expectedL1ToL2Calls.push(
                crate::abi::ExpectedL1ToL2CallSol {
                    expectedL1toL2Hash: callback_key(side),
                    l2ToL1Calls: vec![static_callback(1 - side)],
                    success: true,
                    revertedOrStaticRollingHash: B256::ZERO,
                    returnData: Bytes::new(),
                },
            );
        }
        let mut resolver = StaticRowResolver {
            entries: [&entries[0], &entries[1]],
            chains: [RollupId::MAINNET, RollupId(1)],
            positions: [B256::ZERO; 2],
            cursors: [0; 2],
            resolutions: Default::default(),
            children: Default::default(),
        };
        let error = resolver.resolve(0, callback_key(0), 0).unwrap_err();
        assert!(error.to_string().contains("resolution cycle"), "{error}");
        assert!(
            resolver.children.is_empty(),
            "no callback graph was validated"
        );
    }

    #[test]
    fn static_resolution_cached_suffix_retains_its_depth_bound() {
        // Resolve a long suffix through a shortcut first, then reach that
        // cached suffix through a longer prefix. Memoization must not hide
        // the latter path's depth from the bounded verifier.
        let callback = |level: usize| {
            let mut call = static_callback((level % 2) as u8);
            call.data = Bytes::copy_from_slice(&level.to_be_bytes());
            call
        };
        let key = |level: usize| {
            position_key(
                source_call_hash(
                    &callback(level),
                    RollupId((level % 2) as u64),
                    RollupId((1 - level % 2) as u64),
                )
                .unwrap(),
                B256::ZERO,
            )
        };
        let mut entries = [ExecutionEntrySol::default(), ExecutionEntrySol::default()];
        for level in 0..=MAX_NESTING_DEPTH + 1 {
            let next = if level == 0 {
                vec![3, 1]
            } else if level == MAX_NESTING_DEPTH + 1 {
                vec![]
            } else {
                vec![level + 1]
            };
            let mut results = StaticCallRollingHash::new();
            for child in &next {
                results.append(true, &child.to_be_bytes());
            }
            entries[level % 2]
                .expectedL1ToL2Calls
                .push(crate::abi::ExpectedL1ToL2CallSol {
                    expectedL1toL2Hash: key(level),
                    l2ToL1Calls: next.into_iter().map(callback).collect(),
                    revertedOrStaticRollingHash: results.current(),
                    success: true,
                    returnData: Bytes::copy_from_slice(&level.to_be_bytes()),
                });
        }
        let mut resolver = StaticRowResolver {
            entries: [&entries[0], &entries[1]],
            chains: [RollupId::MAINNET, RollupId(1)],
            positions: [B256::ZERO; 2],
            cursors: [0; 2],
            resolutions: Default::default(),
            children: Default::default(),
        };
        let error = resolver
            .resolve(0, key(0), 0)
            .expect_err("a cached suffix cannot bypass the nesting bound");
        assert!(error.to_string().contains("depth exceeds"), "{error}");
        assert!(!resolver.children.contains_key(&(0, 0)));
    }
}
