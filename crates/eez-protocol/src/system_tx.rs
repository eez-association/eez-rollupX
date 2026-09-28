//! Canonical keyless L2 system-transaction reconstruction.
//!
//! Composer, follower and independent proof validation use these builders with
//! the same checked execution tables and parent-state nonce. Loads, their user
//! transactions, and callback deliveries retain the fork's grouped ordering.
//! Native type `0x76` has a fixed sender, fixed gas budget and zero fees; no
//! participant needs a system private key.

use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, Bytes, U256};
use alloy_sol_types::SolCall;
use eez_primitives::{EEZL2_ADDRESS, SystemTransaction};

use crate::RollupId;
use crate::abi::{
    CrossChainCallSol, ExecutionEntrySol, ExpectedL1ToL2CallSol, ExpectedOutgoingCrossChainCallSol,
    L2ExecutionEntrySol, L2StaticExecutionEntrySol, L2ToL1CallSol, StaticExecutionEntrySol,
    loadExecutionTableCall,
};
use crate::entries::InboundSidecar;

/// Selector for `EEZL2.loadExecutionTable`.
pub const LOAD_EXECUTION_TABLE_SELECTOR: [u8; 4] = loadExecutionTableCall::SELECTOR;

/// Native value that must fund a system-loaded L2 execution table.
///
/// The defensive-checks protocol removes the redundant outer inbound-call
/// fields. `msg.value` now funds every call committed to execute on this L2:
/// the entry's flat incoming calls plus the incoming sub-calls carried by its
/// reentrant outgoing rows. The contract deliberately leaves equality to the
/// prover because table consumption can be partial, so builders and validators
/// must agree on this sum.
pub fn committed_l2_table_value(entries: &[L2ExecutionEntrySol]) -> Result<U256, String> {
    let mut total = U256::ZERO;
    for call in entries.iter().flat_map(|entry| {
        entry.incomingCalls.iter().chain(
            entry
                .expectedOutgoingCalls
                .iter()
                .flat_map(|row| row.incomingCalls.iter()),
        )
    }) {
        total = total
            .checked_add(call.value)
            .ok_or_else(|| "committed L2 incoming value overflows uint256".to_string())?;
    }
    Ok(total)
}

/// Per-follower configuration the system-tx builder needs. The
/// composer and deriver each build one from their own startup config.
#[derive(Clone, Debug)]
pub struct SystemTxContext {
    /// Address of the `EEZL2` contract on this L2.
    pub eezl2_address: Address,
    /// EIP-155 chain id of this L2.
    pub l2_chain_id: u64,
    /// This rollup's id — entries whose `destinationRollupId` doesn't
    /// match are skipped (they belong to a different L2).
    pub this_rollup_id: u64,
}

/// Build native L2 inbound system txs from a postBatch's entries.
///
/// For each entry whose `destinationRollupId == cfg.this_rollup_id`,
/// lowers the entry into its canonical L2 execution table and produces one
/// native tx invoking
/// `EEZL2.executeIncomingCrossChainCall(...)`. Entries for other
/// rollups are skipped.
///
/// `starting_nonce` is the SYSTEM_ADDRESS account nonce at the L2
/// parent block. The function advances locally by one per emitted
/// tx — callers don't need to thread nonces themselves.
///
/// # Errors
///
/// Rejects invalid entry shape, a noncanonical destination, or nonce overflow.
pub fn build_inbound_system_txs(
    entries: &[InboundSidecar],
    cfg: &SystemTxContext,
    starting_nonce: u64,
) -> Result<Vec<Bytes>, String> {
    let mut nonce = starting_nonce;
    let mut out = Vec::new();
    for entry in entries {
        if entry.destination_rollup_id().0 != cfg.this_rollup_id {
            continue;
        }
        let raw = encode_native_system_tx(
            nonce,
            cfg.eezl2_address,
            entry.calldata().to_vec(),
            entry.committed_value(),
            cfg.l2_chain_id,
        )?;
        nonce = nonce.checked_add(1).ok_or_else(|| {
            "SYSTEM_ADDRESS nonce overflow in build_inbound_system_txs".to_string()
        })?;
        out.push(raw);
    }
    Ok(out)
}

/// Build SYNC-block `loadExecutionTable` system txs for OUTBOUND L2→L1
/// deferred entries — ONE tx per entry, mirroring [`build_inbound_system_txs`]'s
/// one-tx-per-entry shape. Each tx stages a single entry that the
/// immediately-following user tx's `EEZL2.executeCrossChainCall` consumes,
/// so the SyncPair block layout is `[load_1 | user_1 | load_2 | user_2 | …]`.
/// Per-outbound-load is a deliberate FAILURE-ISOLATION choice, NOT forced: a
/// single `loadExecutionTable([all entries])` followed by N user txs also works
/// (`entryIndex` is persistent storage advanced by `_consumeAndExecute`). But
/// `_loadExecutionTable` replaces the table and resets `entryIndex` to zero, so
/// one load per entry isolates a reverting/desync'd withdrawal — it can't
/// cascade-desync the cursor for the rest. Given that choice, each load's user
/// tx must run before the next load wipes the table → the interleaved order.
///
/// `starting_nonce` is the SYSTEM_ADDRESS account nonce at the L2 parent
/// block; advanced by one per emitted tx (callers don't thread nonces).
///
/// # Errors
///
/// Rejects a noncanonical destination or SYSTEM_ADDRESS nonce overflow.
pub fn build_outbound_load_table_txs(
    entries: &[L2ExecutionEntrySol],
    cfg: &SystemTxContext,
    starting_nonce: u64,
) -> Result<Vec<Bytes>, String> {
    let mut nonce = starting_nonce;
    let mut out: Vec<Bytes> = Vec::new();
    for entry in entries {
        // One entry per `loadExecutionTable`: the SyncPair pairs each load
        // with its consuming user tx, so a single-element table is correct
        // (the self-clean only matters across pairs, not within one). This is
        // the canonical encoding boundary for an already-validated lean L2
        // entry.
        let calldata = loadExecutionTableCall {
            _entries: vec![entry.clone()],
            _staticEntries: Vec::new(),
        }
        .abi_encode();
        let committed_value = committed_l2_table_value(std::slice::from_ref(entry))?;
        let raw = encode_native_system_tx(
            nonce,
            cfg.eezl2_address,
            calldata,
            committed_value,
            cfg.l2_chain_id,
        )?;
        nonce = nonce.checked_add(1).ok_or_else(|| {
            "SYSTEM_ADDRESS nonce overflow in build_outbound_load_table_txs".to_string()
        })?;
        out.push(raw);
    }
    Ok(out)
}

/// A Sync-block tx pair: a system tx and its OPTIONAL immediately-following
/// user tx.
///
/// - Inbound delivery: `user_tx = None` — the `executeIncomingCrossChainCall`
///   system tx self-loads + consumes in one tx; no user tx follows.
/// - Outbound L2→L1: `user_tx = Some(_)` — the `loadExecutionTable` system tx
///   stages the deferred entry, immediately consumed by the user tx's
///   `executeCrossChainCall` (the SyncPair `[load_i | user_i]`).
#[derive(Clone, Debug)]
pub struct SyncPair {
    /// The system tx (a native SYSTEM_ADDRESS tx).
    pub system_tx: Bytes,
    /// The user tx that consumes `system_tx`'s effect in the SAME block,
    /// immediately after it. `None` for a self-contained system tx.
    pub user_tx: Option<Bytes>,
    /// System deliveries that are part of the same logical outbound effect.
    /// They execute after the consuming user tx and before the next outbound
    /// table load, so the effect endpoint includes every nested callback.
    pub followup_system_txs: Vec<Bytes>,
    /// Number of follow-up deliveries belonging to each settlement effect in
    /// this pair. Outbound multi-root transactions have one element per loaded
    /// source entry. Inbound pairs leave this empty and represent one effect.
    pub effect_followup_counts: Vec<usize>,
}

/// One logical L2-origin settlement effect within a signed user transaction.
///
/// The source entry is loaded before the user transaction. Any top-level
/// L1→L2 returns are separate callback delivery entries and execute directly
/// after that user transaction, while the L1 settlement still contains one
/// host entry for the complete alternating call tree.
#[derive(Clone, Debug)]
pub struct OutboundSyncEffect {
    /// Seed-only mutable or static source-table entry consumed by `user_tx`.
    pub source_entry: OutboundSourceEntry,
    /// Ordered top-level L1→L2 callback deliveries belonging to this effect.
    pub callback_entries: Vec<InboundSidecar>,
}

/// Source-table kind loaded before an L2-origin user transaction.
#[derive(Clone, Debug)]
pub enum OutboundSourceEntry {
    /// A state-changing proxy call consumed from the mutable entry table.
    Mutable(ExecutionEntrySol),
    /// A top-level read consumed from the same-block static entry pool.
    Static(L2StaticExecutionEntrySol),
}

impl OutboundSourceEntry {
    /// Borrow the mutable entry, if this effect consumes one.
    #[must_use]
    pub const fn as_mutable(&self) -> Option<&ExecutionEntrySol> {
        match self {
            Self::Mutable(entry) => Some(entry),
            Self::Static(_) => None,
        }
    }

    /// Borrow the static entry, if this effect consumes one.
    #[must_use]
    pub const fn as_static(&self) -> Option<&L2StaticExecutionEntrySol> {
        match self {
            Self::Mutable(_) => None,
            Self::Static(entry) => Some(entry),
        }
    }
}

/// All settlement effects consumed by one signed L2 user transaction.
#[derive(Clone, Debug)]
pub struct OutboundSyncGroup {
    /// Ordered roots loaded together and consumed sequentially by `user_tx`.
    pub effects: Vec<OutboundSyncEffect>,
    /// Original signed L2 user transaction, included exactly once.
    pub user_tx: Bytes,
}

/// Number of source-L2 roots represented by each outbound L1 settlement host.
///
/// Each live source cursor slot is backed by one seed-only execution-table
/// entry. Calls inside a marked revert span advance the cursor temporarily and
/// then restore it, so a later retry can consume the same source entry again.
/// Separate nested callback deliveries are partitioned downstream.
///
/// # Errors
///
/// Returns an error for a non-host entry or an empty host.
pub fn outbound_settlement_root_counts(
    entries: &[ExecutionEntrySol],
) -> Result<Vec<usize>, String> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            if entry.proxyEntryHash != alloy_primitives::B256::ZERO {
                return Err(format!(
                    "outbound settlement entry {index} has a non-zero proxy hash"
                ));
            }
            check_call_array_shape(
                &entry.l2ToL1Calls,
                &format!("outbound settlement entry {index} flat calls"),
            )?;
            let roots = call_array_cursor_slots(&entry.l2ToL1Calls);
            if roots == 0 {
                return Err(format!(
                    "outbound settlement entry {index} has no top-level L2-to-L1 root"
                ));
            }
            Ok(roots)
        })
        .collect()
}

fn call_array_cursor_slots(calls: &[crate::abi::L2ToL1CallSol]) -> usize {
    let mut call_index = 0usize;
    let mut cursor = 0usize;
    let mut slots = 0usize;
    while call_index < calls.len() {
        let span = usize::from(calls[call_index].revertNextNCalls);
        let group_len = span.max(1);
        let saved_cursor = cursor;
        cursor += group_len;
        slots = slots.max(cursor);
        call_index += group_len;
        if span != 0 {
            cursor = saved_cursor;
        }
    }
    slots
}

/// The canonical Sync-block tx order: each pair's system tx immediately
/// followed by its user tx (if any) — `[s_1, u_1?, s_2, u_2?, …]`.
///
/// INTERLEAVED, not system-first: GIVEN the per-outbound-load failure-isolation
/// choice (see `build_outbound_load_table_txs`), each `loadExecutionTable` wipes
/// + resets the cursor in `_loadExecutionTable`, so each load's user tx must run
/// before the next load. Both `build_sync_block` (the composer) and the
/// deriver's reconstruction build their tx list through THIS fn, so the order
/// is identical by construction (no system-first vs interleaved drift → no
/// `to_block` root divergence). For inbound (all `user_tx == None`) it reduces
/// to `[system_txs…]`, byte-identical to the prior system-first layout.
#[must_use]
pub fn interleave_sync_block_txs(pairs: &[SyncPair]) -> Vec<Bytes> {
    let mut out: Vec<Bytes> = Vec::with_capacity(pairs.len() * 2);
    for pair in pairs {
        out.push(pair.system_tx.clone());
        if let Some(user_tx) = &pair.user_tx {
            out.push(user_tx.clone());
        }
        out.extend(pair.followup_system_txs.iter().cloned());
    }
    out
}

/// Number of SYSTEM_ADDRESS transactions in a collection of sync pairs.
#[must_use]
pub fn sync_pair_system_tx_count(pairs: &[SyncPair]) -> usize {
    pairs
        .iter()
        .map(|pair| 1 + pair.followup_system_txs.len())
        .sum()
}

/// Inclusive transaction index at which each logical sync effect completes.
#[must_use]
pub fn sync_pair_end_positions(pairs: &[SyncPair]) -> Vec<usize> {
    let mut next = 0usize;
    let mut positions = Vec::new();
    for pair in pairs {
        let system = next;
        next += 1;
        let Some(_) = pair.user_tx else {
            positions.push(system);
            continue;
        };
        let user = next;
        next += 1;
        let mut endpoint = user;
        for &followups in &pair.effect_followup_counts {
            next += followups;
            if followups != 0 {
                endpoint = next - 1;
            }
            positions.push(endpoint);
        }
        debug_assert_eq!(
            pair.effect_followup_counts.iter().sum::<usize>(),
            pair.followup_system_txs.len(),
        );
    }
    positions
}

/// Reject entry semantics outside the runtime's mutable profile.
///
/// The composer calls this per entry at accept time and
/// [`build_cross_chain_sync_pairs`] calls it over the whole drained set, so
/// both sides gate on the exact same predicate.
///
/// `dir` names the direction (`"outbound"` / `"inbound"`) in the error.
///
/// # Errors
/// Returns a `String` error naming the offending shape.
pub fn check_entry_shape(entry: &ExecutionEntrySol, dir: &str) -> Result<(), String> {
    if !entry.success && (entry.l2ToL1Calls.len() > 1 || !entry.expectedL1ToL2Calls.is_empty()) {
        return Err(format!(
            "unsuccessful {dir} entry is nonterminal; failed top-level calls with descendants are not supported"
        ));
    }
    check_call_array_shape(&entry.l2ToL1Calls, &format!("{dir} entry flat calls"))?;
    for (row_index, row) in entry.expectedL1ToL2Calls.iter().enumerate() {
        check_call_array_shape(
            &row.l2ToL1Calls,
            &format!("{dir} entry nested row {row_index}"),
        )?;
        if row.success && row.revertedOrStaticRollingHash != alloy_primitives::B256::ZERO {
            return Err(format!(
                "{dir} entry nested row {row_index} is successful but carries a reverted/static rolling hash"
            ));
        }
        if !row.success {
            if row.revertedOrStaticRollingHash == alloy_primitives::B256::ZERO {
                return Err(format!(
                    "{dir} entry failed nested row {row_index} has no reverted rolling hash"
                ));
            }
            if !row.l2ToL1Calls.is_empty() {
                return Err(format!(
                    "{dir} entry failed nested row {row_index} contains sub-calls; failed rows with sub-calls are not supported"
                ));
            }
        }
    }
    Ok(())
}

fn check_call_array_shape(calls: &[crate::abi::L2ToL1CallSol], label: &str) -> Result<(), String> {
    let mut index = 0usize;
    while index < calls.len() {
        let call = &calls[index];
        if call.gas != 0 {
            return Err(format!("{label} call {index} uses explicit-gas semantics"));
        }
        if call.isStatic {
            if !call.value.is_zero() {
                return Err(format!(
                    "{label} static call {index} carries non-zero value"
                ));
            }
            if call.revertNextNCalls != 0 {
                return Err(format!("{label} static call {index} carries a revert span"));
            }
            index += 1;
            continue;
        }
        let span = usize::from(call.revertNextNCalls);
        if span == 0 {
            index += 1;
            continue;
        }
        let end = index
            .checked_add(span)
            .filter(|end| *end <= calls.len())
            .ok_or_else(|| {
                format!(
                    "{label} call {index} revert span {span} exceeds the {}-call array",
                    calls.len(),
                )
            })?;
        if let Some(nested) = calls[index + 1..end]
            .iter()
            .position(|call| call.revertNextNCalls != 0)
        {
            return Err(format!(
                "{label} call {} starts a nested revert span",
                index + 1 + nested,
            ));
        }
        index = end;
    }
    Ok(())
}

/// Reject malformed top-level static-entry semantics at transport boundaries.
pub fn check_static_entry_shape(
    entry: &StaticExecutionEntrySol,
    label: &str,
) -> Result<(), String> {
    if entry.proxyEntryHash == alloy_primitives::B256::ZERO {
        return Err(format!("{label} static entry has a zero proxy hash"));
    }
    for (index, call) in entry.l2ToL1Calls.iter().enumerate() {
        if !call.isStatic {
            return Err(format!(
                "{label} static sub-call {index} is not marked static"
            ));
        }
        if !call.value.is_zero() {
            return Err(format!(
                "{label} static sub-call {index} carries non-zero value"
            ));
        }
        if call.revertNextNCalls != 0 {
            return Err(format!(
                "{label} static sub-call {index} carries a revert span"
            ));
        }
    }
    Ok(())
}

/// Check the supported terminal L2 static-source shape, retaining its position.
/// Position authorization requires the complete ordered host in
/// `verify_outbound_source_group`; decoding and this shape check are insufficient.
pub fn check_l2_static_entry_shape(
    entry: &L2StaticExecutionEntrySol,
    label: &str,
) -> Result<(), String> {
    if entry.proxyEntryHash == alloy_primitives::B256::ZERO {
        return Err(format!("{label} static entry has a zero proxy hash"));
    }
    if !entry.incomingCalls.is_empty() || entry.rollingHash != alloy_primitives::B256::ZERO {
        return Err(format!(
            "{label} static source must be a terminal L1 lookup"
        ));
    }
    Ok(())
}

/// Lower the L1-shaped sidecar transport into the field-identical L2 ABI.
///
/// The sidecar intentionally uses `ExecutionEntrySol` so it can share the
/// settlement/DA codec. L2 ignores L1-only state and destination fields; every
/// flat call and every unified nested row is retained exactly.
#[must_use]
pub fn lower_entry_to_l2(entry: &ExecutionEntrySol) -> L2ExecutionEntrySol {
    let lower_call = |call: &crate::abi::L2ToL1CallSol| CrossChainCallSol {
        revertNextNCalls: call.revertNextNCalls,
        isStatic: call.isStatic,
        gas: call.gas,
        sourceAddress: call.sourceAddress,
        sourceRollupId: call.sourceRollupId,
        targetAddress: call.targetAddress,
        value: call.value,
        data: call.data.clone(),
    };
    L2ExecutionEntrySol {
        proxyEntryHash: entry.proxyEntryHash,
        incomingCalls: entry.l2ToL1Calls.iter().map(lower_call).collect(),
        expectedOutgoingCalls: entry
            .expectedL1ToL2Calls
            .iter()
            .map(|row| ExpectedOutgoingCrossChainCallSol {
                expectedOutgoingHash: row.expectedL1toL2Hash,
                incomingCalls: row.l2ToL1Calls.iter().map(lower_call).collect(),
                revertedOrStaticRollingHash: row.revertedOrStaticRollingHash,
                success: row.success,
                returnData: row.returnData.clone(),
            })
            .collect(),
        rollingHash: entry.rollingHash,
        success: entry.success,
        returnData: entry.returnData.clone(),
    }
}

/// Lift the field-identical L2 ABI into the shared sidecar transport shape.
///
/// `destination_rollup_id` supplies the transport-only routing metadata: it
/// is this L2 for an inbound target entry and MAINNET for an outbound source
/// entry. No execution-table field is discarded or synthesized.
#[must_use]
pub fn lift_entry_from_l2(
    entry: &L2ExecutionEntrySol,
    destination_rollup_id: RollupId,
) -> ExecutionEntrySol {
    let lift_call = |call: &CrossChainCallSol| L2ToL1CallSol {
        revertNextNCalls: call.revertNextNCalls,
        isStatic: call.isStatic,
        gas: call.gas,
        sourceAddress: call.sourceAddress,
        sourceRollupId: call.sourceRollupId,
        targetAddress: call.targetAddress,
        value: call.value,
        data: call.data.clone(),
    };
    ExecutionEntrySol {
        rootUpdates: Vec::new(),
        proxyEntryHash: entry.proxyEntryHash,
        l2ToL1Calls: entry.incomingCalls.iter().map(lift_call).collect(),
        expectedL1ToL2Calls: entry
            .expectedOutgoingCalls
            .iter()
            .map(|row| ExpectedL1ToL2CallSol {
                expectedL1toL2Hash: row.expectedOutgoingHash,
                l2ToL1Calls: row.incomingCalls.iter().map(lift_call).collect(),
                revertedOrStaticRollingHash: row.revertedOrStaticRollingHash,
                success: row.success,
                returnData: row.returnData.clone(),
            })
            .collect(),
        rollingHash: entry.rollingHash,
        destinationRollupId: destination_rollup_id.0,
        success: entry.success,
        returnData: entry.returnData.clone(),
    }
}

/// Translate an L1-shaped static call description at an explicit L2 position.
/// This conversion does not authorize the position or L1 routing/root metadata;
/// L2 DA and authorization retain the resulting native L2 type.
#[must_use]
pub fn lower_static_entry_to_l2(
    entry: &StaticExecutionEntrySol,
    expected_entry_index: U256,
) -> L2StaticExecutionEntrySol {
    L2StaticExecutionEntrySol {
        expectedEntryIndex: expected_entry_index,
        proxyEntryHash: entry.proxyEntryHash,
        incomingCalls: entry
            .l2ToL1Calls
            .iter()
            .map(|call| CrossChainCallSol {
                revertNextNCalls: call.revertNextNCalls,
                isStatic: call.isStatic,
                gas: call.gas,
                sourceAddress: call.sourceAddress,
                sourceRollupId: call.sourceRollupId,
                targetAddress: call.targetAddress,
                value: call.value,
                data: call.data.clone(),
            })
            .collect(),
        rollingHash: entry.rollingHash,
        success: entry.success,
        returnData: entry.returnData.clone(),
    }
}

/// Lower ONE outbound L2→L1 entry to its `[load | user]` Sync pairs, starting
/// at `starting_nonce`.
///
/// The per-entry unit [`build_cross_chain_sync_pairs`] builds its PHASE 1
/// from. The composer appends these pairs to the block as each survivor is
/// accepted and the post-drain canonical rebuild replays the same per-entry
/// calls in the same order, so the incremental block and the canonical layout
/// are byte-identical BY CONSTRUCTION.
///
/// # Errors
/// Rejects unsupported entry shapes and invalid native destinations.
pub fn build_outbound_pair(
    entry: &ExecutionEntrySol,
    user_tx: &Bytes,
    cfg: &SystemTxContext,
    starting_nonce: u64,
) -> Result<Vec<SyncPair>, String> {
    check_entry_shape(entry, "outbound")?;
    if entry.proxyEntryHash == alloy_primitives::B256::ZERO {
        return Err("outbound source-L2 entry has a zero proxyEntryHash".to_string());
    }
    let l2_entry = lower_entry_to_l2(entry);
    let loads =
        build_outbound_load_table_txs(std::slice::from_ref(&l2_entry), cfg, starting_nonce)?;
    Ok(loads
        .into_iter()
        .map(|load| SyncPair {
            system_tx: load,
            user_tx: Some(user_tx.clone()),
            followup_system_txs: Vec::new(),
            effect_followup_counts: vec![0],
        })
        .collect())
}

/// Build one canonical outbound group as
/// `[load(source), user, callback_delivery…]`.
///
/// All system nonces are consecutive within the group. This is required for
/// an L2-origin call that returns from L1 to L2: each return is its own
/// `executeIncomingCrossChainCall`, but remains part of the source user
/// transaction's single settlement effect.
pub fn build_outbound_group(
    group: &OutboundSyncGroup,
    cfg: &SystemTxContext,
    starting_nonce: u64,
) -> Result<SyncPair, String> {
    if group.effects.is_empty() {
        return Err("outbound user transaction has no source effects".to_string());
    }
    let mut lowered = Vec::with_capacity(group.effects.len());
    let mut lowered_static = Vec::with_capacity(group.effects.len());
    let mut callbacks = Vec::new();
    let mut effect_followup_counts = Vec::with_capacity(group.effects.len());
    for effect in &group.effects {
        match &effect.source_entry {
            OutboundSourceEntry::Mutable(entry) => {
                check_entry_shape(entry, "outbound")?;
                if entry.proxyEntryHash == alloy_primitives::B256::ZERO {
                    return Err("outbound source-L2 entry has a zero proxyEntryHash".to_string());
                }
                lowered.push(lower_entry_to_l2(entry));
            }
            OutboundSourceEntry::Static(entry) => {
                check_l2_static_entry_shape(entry, "outbound")?;
                if !effect.callback_entries.is_empty() {
                    return Err(
                        "outbound static source entry cannot carry callback deliveries".into(),
                    );
                }
                lowered_static.push(entry.clone());
            }
        }
        for entry in &effect.callback_entries {
            if entry.destination_rollup_id().0 != cfg.this_rollup_id {
                return Err(format!(
                    "outbound callback targets rollup {}, expected {}",
                    entry.destination_rollup_id().0,
                    cfg.this_rollup_id,
                ));
            }
        }
        effect_followup_counts.push(effect.callback_entries.len());
        callbacks.extend(effect.callback_entries.iter().cloned());
    }
    let committed_value = committed_l2_table_value(&lowered)?;
    let calldata = loadExecutionTableCall {
        _entries: lowered,
        _staticEntries: lowered_static,
    }
    .abi_encode();
    let system_tx = encode_native_system_tx(
        starting_nonce,
        cfg.eezl2_address,
        calldata,
        committed_value,
        cfg.l2_chain_id,
    )?;
    let followup_system_txs = build_inbound_system_txs(
        &callbacks,
        cfg,
        starting_nonce
            .checked_add(1)
            .ok_or_else(|| "SYSTEM_ADDRESS nonce overflow (outbound group)".to_string())?,
    )?;
    Ok(SyncPair {
        system_tx,
        user_tx: Some(group.user_tx.clone()),
        followup_system_txs,
        effect_followup_counts,
    })
}

/// THE canonical Sync-block `SyncPair` builder — the SINGLE source of truth
/// both the composer (to commit the Sync block) and the deriver (to
/// reconstruct it) call, so the two are byte-identical BY CONSTRUCTION: no
/// drain-order-vs-two-phase nonce drift, no interleaved-vs-system-first order
/// drift. (Replaces the two sides independently calling
/// `build_outbound_load_table_txs` / `build_inbound_system_txs` in their own
/// order with their own nonce sequencing — the source of the mixed-batch
/// fork.)
///
/// Canonical order (given the per-outbound-load failure-isolation choice +
/// `_loadExecutionTable`'s table replacement + cursor reset, which
/// `executeIncomingCrossChainCall` also triggers):
/// ALL outbound load+user pairs FIRST, THEN all inbound deliveries —
/// `[load_0,user_0, …, load_{K-1},user_{K-1}, deliver_0, …, deliver_{M-1}]`.
/// SYSTEM_ADDRESS nonces run strictly in that order from `starting_nonce`:
/// outbound loads `N..N+K-1`, inbound deliveries `N+K..` (the deriver's
/// canonical two-phase order, `deriver.rs`). For the value-free A2b target
/// K=1/M=1 the list is `[load(N), user, deliver(N+1)]`.
///
/// `outbound`: each `(source-L2 sidecar ExecutionEntrySol, its consuming user
/// tx)` in canonical order; the complete flat arrays and unified nested rows
/// are lowered field-for-field into the L2 ABI. `inbound`: L1-shaped deferred
/// entries whose single top-level call drives `executeIncomingCrossChainCall`;
/// entries for other rollups are skipped.
///
/// Single-direction degenerates EXACTLY: outbound-only → `[load,user,…]`;
/// inbound-only → `[deliver,…]` (all `user_tx == None`) — byte-identical to
/// the pre-refactor per-direction builds.
///
/// # Errors
/// Rejects unsupported entry shapes, invalid native destinations, and SYSTEM_ADDRESS
/// nonce overflow.
pub fn build_cross_chain_sync_pairs(
    outbound: &[(ExecutionEntrySol, Bytes)],
    inbound: &[InboundSidecar],
    cfg: &SystemTxContext,
    starting_nonce: u64,
) -> Result<Vec<SyncPair>, String> {
    let groups = outbound
        .iter()
        .map(|(source_entry, user_tx)| OutboundSyncGroup {
            effects: vec![OutboundSyncEffect {
                source_entry: OutboundSourceEntry::Mutable(source_entry.clone()),
                callback_entries: Vec::new(),
            }],
            user_tx: user_tx.clone(),
        })
        .collect::<Vec<_>>();
    build_cross_chain_sync_groups(&groups, inbound, cfg, starting_nonce)
}

/// Canonical grouped Sync-block builder.
///
/// Outbound groups are emitted first as
/// `[load, user, callback_delivery…]`; standalone inbound deliveries follow.
/// SYSTEM_ADDRESS nonces advance over loads and callbacks alike.
pub fn build_cross_chain_sync_groups(
    outbound: &[OutboundSyncGroup],
    inbound: &[InboundSidecar],
    cfg: &SystemTxContext,
    starting_nonce: u64,
) -> Result<Vec<SyncPair>, String> {
    let mut nonce = starting_nonce;
    let mut pairs: Vec<SyncPair> = Vec::with_capacity(outbound.len() + inbound.len());

    // ── PHASE 1 — outbound: each loadExecutionTable immediately paired with
    // its consuming user tx (the self-clean requires consume-before-next-load).
    for group in outbound {
        let built = build_outbound_group(group, cfg, nonce)?;
        nonce = nonce
            .checked_add(1 + built.followup_system_txs.len() as u64)
            .ok_or_else(|| "SYSTEM_ADDRESS nonce overflow (outbound groups)".to_string())?;
        pairs.push(built);
    }

    // ── PHASE 2 — inbound deliveries, continuing the SYSTEM_ADDRESS nonce
    // after ALL outbound loads (build_inbound_system_txs advances internally).
    let deliveries = build_inbound_system_txs(inbound, cfg, nonce)?;
    for d in deliveries {
        pairs.push(SyncPair {
            system_tx: d,
            user_tx: None,
            followup_system_txs: Vec::new(),
            effect_followup_counts: Vec::new(),
        });
    }

    Ok(pairs)
}

/// Encode the fixed native envelope, including the complete committed value.
/// Reject a misconfigured destination before producing privileged bytes.
fn encode_native_system_tx(
    nonce: u64,
    to: Address,
    calldata: Vec<u8>,
    value: U256,
    chain_id: u64,
) -> Result<Bytes, String> {
    if to != EEZL2_ADDRESS {
        return Err("native system transaction must target the EEZL2 predeploy".into());
    }
    Ok(SystemTransaction {
        chain_id,
        nonce,
        to,
        value,
        input: calldata.into(),
    }
    .encoded_2718()
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EvmBatch;
    use crate::abi::RollupIdWithProofSystemsSol;
    use crate::entries::{decode_postbatch, encode_postbatch};
    use alloy_primitives::{B256, address};
    use alloy_sol_types::SolValue as _;

    fn ctx() -> SystemTxContext {
        SystemTxContext {
            eezl2_address: address!("4200000000000000000000000000000000000007"),
            l2_chain_id: 1,
            this_rollup_id: 1,
        }
    }

    /// One inbound L1→L2 deferred entry (the call lives in `l2ToL1Calls[0]`,
    /// `destinationRollupId == this_rollup_id`), the shape `build_inbound_system_txs`
    /// lowers to an `executeIncomingCrossChainCall` system tx.
    fn inbound_entry() -> ExecutionEntrySol {
        inbound_entry_with(1, U256::ZERO)
    }

    fn inbound_entry_with(rollup_id: u64, value: U256) -> ExecutionEntrySol {
        InboundSidecar::new(crate::entries::IncomingEntry {
            target: address!("00000000000000000000000000000000000000bb"),
            source: address!("00000000000000000000000000000000000000cc"),
            value,
            data: Bytes::from_static(&[0x12, 0x34]),
            source_rollup_id: RollupId::MAINNET,
            l2_rollup_id: RollupId(rollup_id),
            return_data: Bytes::from_static(&[0xab, 0xcd]),
            success: true,
        })
        .unwrap()
        .as_abi_entry()
        .clone()
    }

    fn checked(entries: &[ExecutionEntrySol]) -> Result<Vec<InboundSidecar>, String> {
        entries
            .iter()
            .map(|entry| InboundSidecar::try_from(entry).map_err(|e| e.to_string()))
            .collect()
    }

    fn build_inbound_system_txs(
        entries: &[ExecutionEntrySol],
        cfg: &SystemTxContext,
        nonce: u64,
    ) -> Result<Vec<Bytes>, String> {
        super::build_inbound_system_txs(&checked(entries)?, cfg, nonce)
    }

    fn build_cross_chain_sync_pairs(
        outbound: &[(ExecutionEntrySol, Bytes)],
        inbound: &[ExecutionEntrySol],
        cfg: &SystemTxContext,
        nonce: u64,
    ) -> Result<Vec<SyncPair>, String> {
        super::build_cross_chain_sync_pairs(outbound, &checked(inbound)?, cfg, nonce)
    }

    fn round_tripped(entries: &[ExecutionEntrySol]) -> Vec<ExecutionEntrySol> {
        let mut batch = EvmBatch {
            entries: entries.to_vec(),
            ..Default::default()
        };
        batch.rollupIdsWithProofSystems = vec![RollupIdWithProofSystemsSol {
            rollupId: 1,
            proofSystemIndexes: vec![0],
        }];
        let calldata = encode_postbatch(&batch);
        decode_postbatch(&calldata)
            .expect("postBatch round-trips")
            .entries
    }

    /// One source-L2 outbound sidecar. It is seed-only for a root call; nested
    /// callbacks populate its flat and expected-row arrays.
    fn outbound_entry() -> ExecutionEntrySol {
        ExecutionEntrySol {
            rootUpdates: Vec::new(),
            proxyEntryHash: B256::repeat_byte(0xcd),
            l2ToL1Calls: Vec::new(),
            expectedL1ToL2Calls: Vec::new(),
            rollingHash: B256::repeat_byte(0xef),
            destinationRollupId: 0,
            success: true,
            returnData: Bytes::from(vec![0x77]),
        }
    }

    /// Decode a native system tx's nonce.
    fn nonce_of(raw: &Bytes) -> u64 {
        use alloy_consensus::Transaction as _;
        use alloy_eips::eip2718::Decodable2718 as _;
        let mut s: &[u8] = raw.as_ref();
        eez_primitives::EezTxEnvelope::decode_2718(&mut s)
            .expect("decode system tx")
            .nonce()
    }

    fn value_of(raw: &Bytes) -> U256 {
        use alloy_consensus::Transaction as _;
        use alloy_eips::eip2718::Decodable2718 as _;
        let mut s: &[u8] = raw.as_ref();
        eez_primitives::EezTxEnvelope::decode_2718(&mut s)
            .expect("decode system tx")
            .value()
    }

    #[test]
    fn payable_l2_table_calls_fund_outer_and_nested_incoming_values() {
        use alloy_consensus::Transaction as _;
        use alloy_eips::eip2718::Decodable2718 as _;

        let mut entry = inbound_entry_with(1, U256::from(3));
        let mut nested = entry.l2ToL1Calls[0].clone();
        nested.value = U256::from(5);
        entry.expectedL1ToL2Calls = vec![crate::abi::ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: B256::repeat_byte(0x31),
            l2ToL1Calls: vec![nested],
            revertedOrStaticRollingHash: B256::ZERO,
            success: true,
            returnData: Bytes::new(),
        }];

        let inbound = build_inbound_system_txs(&[entry.clone()], &ctx(), 0).unwrap();
        assert_eq!(value_of(&inbound[0]), U256::from(8));
        let mut encoded = inbound[0].as_ref();
        let signed = eez_primitives::EezTxEnvelope::decode_2718(&mut encoded).unwrap();
        let call = crate::abi::executeIncomingCrossChainCallCall::abi_decode(signed.input())
            .expect("new two-argument inbound ABI decodes");
        assert_eq!(call._entries.len(), 1);
        assert!(call._staticEntries.is_empty());

        let lowered = lower_entry_to_l2(&entry);
        assert_eq!(
            committed_l2_table_value(std::slice::from_ref(&lowered)).unwrap(),
            U256::from(8),
        );
        let loads = build_outbound_load_table_txs(&[lowered], &ctx(), 0).unwrap();
        assert_eq!(value_of(&loads[0]), U256::from(8));
    }

    /// THE A2b property: the canonical builder lays a MIXED slot out in
    /// deriver-canonical order (all outbound load+user pairs, THEN inbound
    /// deliveries) with strict two-phase SYSTEM_ADDRESS nonces — independent of
    /// the order the (outbound, inbound) args are constructed. K=1/M=1.
    #[test]
    fn cross_chain_sync_pairs_canonical_order_and_two_phase_nonces() {
        let cfg = ctx();
        let n = 5u64;
        let user_tx = Bytes::from(vec![0xde, 0xad]);
        let pairs = build_cross_chain_sync_pairs(
            &[(outbound_entry(), user_tx.clone())],
            &[inbound_entry()],
            &cfg,
            n,
        )
        .unwrap();

        assert_eq!(pairs.len(), 2, "K=1 outbound + M=1 inbound → 2 pairs");
        // Outbound first, paired with its user tx; inbound second, self-contained.
        assert_eq!(
            pairs[0].user_tx,
            Some(user_tx),
            "pair[0] = outbound load + its user tx",
        );
        assert_eq!(
            pairs[1].user_tx, None,
            "pair[1] = inbound delivery, no user tx"
        );
        // Two-phase nonces: outbound load = N, inbound delivery = N+1.
        assert_eq!(nonce_of(&pairs[0].system_tx), n, "outbound load nonce = N");
        assert_eq!(
            nonce_of(&pairs[1].system_tx),
            n + 1,
            "inbound delivery nonce = N+1 (strictly AFTER all outbound loads)",
        );
    }

    #[test]
    fn multi_root_outbound_uses_one_load_and_one_user() {
        use alloy_consensus::Transaction as _;
        use alloy_eips::eip2718::Decodable2718 as _;
        use alloy_sol_types::SolCall as _;

        let mut second = outbound_entry();
        second.proxyEntryHash = B256::repeat_byte(0xce);
        let group = OutboundSyncGroup {
            effects: vec![
                OutboundSyncEffect {
                    source_entry: OutboundSourceEntry::Mutable(outbound_entry()),
                    callback_entries: Vec::new(),
                },
                OutboundSyncEffect {
                    source_entry: OutboundSourceEntry::Mutable(second),
                    callback_entries: Vec::new(),
                },
            ],
            user_tx: Bytes::from_static(&[0xaa]),
        };
        let pairs = build_cross_chain_sync_groups(&[group], &[], &ctx(), 7).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].effect_followup_counts, [0, 0]);
        assert_eq!(interleave_sync_block_txs(&pairs).len(), 2);
        assert_eq!(sync_pair_end_positions(&pairs), [1, 1]);

        let mut raw = pairs[0].system_tx.as_ref();
        let signed = eez_primitives::EezTxEnvelope::decode_2718(&mut raw).unwrap();
        let load = loadExecutionTableCall::abi_decode(signed.input()).unwrap();
        assert_eq!(load._entries.len(), 2);
        assert_eq!(nonce_of(&pairs[0].system_tx), 7);
        assert!(pairs[0].followup_system_txs.is_empty());
    }

    #[test]
    fn static_outbound_uses_the_static_table_and_round_trips() {
        use alloy_consensus::Transaction as _;
        use alloy_eips::eip2718::Decodable2718 as _;

        let source = L2StaticExecutionEntrySol {
            expectedEntryIndex: U256::from(7),
            proxyEntryHash: B256::repeat_byte(0x5a),
            incomingCalls: Vec::new(),
            rollingHash: B256::ZERO,
            success: true,
            returnData: Bytes::from_static(&[0x42]),
        };
        let group = OutboundSyncGroup {
            effects: vec![OutboundSyncEffect {
                source_entry: OutboundSourceEntry::Static(source.clone()),
                callback_entries: Vec::new(),
            }],
            user_tx: Bytes::from_static(&[0xaa]),
        };

        let built = build_outbound_group(&group, &ctx(), 9).expect("build static group");
        assert_eq!(built.effect_followup_counts, [0]);
        assert!(built.followup_system_txs.is_empty());

        let mut raw = built.system_tx.as_ref();
        let signed =
            eez_primitives::EezTxEnvelope::decode_2718(&mut raw).expect("decode static load tx");
        let load = loadExecutionTableCall::abi_decode(signed.input()).expect("decode static load");
        assert!(load._entries.is_empty());
        assert_eq!(load._staticEntries.len(), 1);
        assert_eq!(load._staticEntries[0].abi_encode(), source.abi_encode());
    }

    #[test]
    fn multi_root_settlement_host_may_carry_nested_callbacks() {
        let mut host = inbound_entry();
        host.proxyEntryHash = B256::ZERO;
        host.destinationRollupId = 0;
        host.l2ToL1Calls.push(host.l2ToL1Calls[0].clone());
        host.expectedL1ToL2Calls = vec![crate::abi::ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: B256::repeat_byte(0x31),
            l2ToL1Calls: Vec::new(),
            revertedOrStaticRollingHash: B256::ZERO,
            success: true,
            returnData: Bytes::new(),
        }];

        assert_eq!(outbound_settlement_root_counts(&[host]).unwrap(), [2]);
    }

    #[test]
    fn reverted_root_retry_reuses_one_source_cursor_slot() {
        let mut host = inbound_entry();
        host.proxyEntryHash = B256::ZERO;
        host.destinationRollupId = 0;
        host.l2ToL1Calls[0].revertNextNCalls = 1;
        let mut retry = host.l2ToL1Calls[0].clone();
        retry.revertNextNCalls = 0;
        host.l2ToL1Calls.push(retry);

        assert_eq!(outbound_settlement_root_counts(&[host]).unwrap(), [1]);
    }

    /// Rich flat arrays and unified nested rows survive the sidecar-to-L2 ABI
    /// lowering exactly. A naturally failed empty nested frame is supported,
    /// while malformed failure commitments and failed frames with sub-calls
    /// still fail closed.
    #[test]
    fn entry_lowering_preserves_multicall_and_nested_rows() {
        let cfg = ctx();
        let user = Bytes::from(vec![0x01]);
        let mut outbound = outbound_entry();
        let call = inbound_entry().l2ToL1Calls[0].clone();
        outbound.l2ToL1Calls = vec![call.clone(), call.clone()];
        outbound.expectedL1ToL2Calls = vec![crate::abi::ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: B256::repeat_byte(0x31),
            l2ToL1Calls: vec![call],
            revertedOrStaticRollingHash: B256::ZERO,
            success: true,
            returnData: Bytes::from_static(&[0x32]),
        }];

        let lowered = lower_entry_to_l2(&outbound);
        assert_eq!(lowered.incomingCalls.len(), 2);
        assert_eq!(lowered.expectedOutgoingCalls.len(), 1);
        assert_eq!(lowered.expectedOutgoingCalls[0].incomingCalls.len(), 1);
        let lifted = lift_entry_from_l2(&lowered, RollupId(outbound.destinationRollupId));
        assert_eq!(lifted.abi_encode(), outbound.abi_encode());
        build_cross_chain_sync_pairs(&[(outbound.clone(), user)], &[], &cfg, 0)
            .expect("rich outbound sidecar builds");

        outbound.expectedL1ToL2Calls[0].success = false;
        let error = check_entry_shape(&outbound, "outbound")
            .expect_err("failed row without a sub-hash is malformed");
        assert!(error.contains("no reverted rolling hash"), "error: {error}");

        outbound.expectedL1ToL2Calls[0].revertedOrStaticRollingHash = B256::repeat_byte(0x33);
        let error = check_entry_shape(&outbound, "outbound")
            .expect_err("failed row with sub-calls is outside the initial safe profile");
        assert!(error.contains("contains sub-calls"), "error: {error}");

        outbound.expectedL1ToL2Calls[0].l2ToL1Calls.clear();
        check_entry_shape(&outbound, "outbound").expect("failed empty nested row is supported");
        let lowered = lower_entry_to_l2(&outbound);
        assert!(!lowered.expectedOutgoingCalls[0].success);
        assert_eq!(
            lowered.expectedOutgoingCalls[0].revertedOrStaticRollingHash,
            B256::repeat_byte(0x33)
        );
    }

    #[test]
    fn cross_chain_sync_pairs_rejects_zero_hash_outbound_entry() {
        let cfg = ctx();
        let mut outbound = outbound_entry();
        outbound.proxyEntryHash = B256::ZERO;

        let err =
            build_cross_chain_sync_pairs(&[(outbound, Bytes::from_static(&[0x01]))], &[], &cfg, 0)
                .expect_err("L2 entries cannot use the L1 host's zero proxy hash");

        assert!(
            err.contains("zero proxyEntryHash"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reverted_outbound_lookup_is_preserved_but_failed_inbound_is_not_delivered() {
        let cfg = ctx();

        let mut outbound = outbound_entry();
        outbound.success = false;
        let outbound_pairs =
            build_cross_chain_sync_pairs(&[(outbound, Bytes::from_static(&[0x01]))], &[], &cfg, 0)
                .expect("terminal unsuccessful outbound entries are representable");
        assert_eq!(outbound_pairs.len(), 1);

        let mut inbound = inbound_entry();
        inbound.success = false;
        let error = build_cross_chain_sync_pairs(&[], &[inbound], &cfg, 0)
            .expect_err("a failed inbound action must not produce a native delivery");
        assert!(error.contains("delivery entry must be successful"));
    }

    /// Single-direction degenerates EXACTLY to the per-direction builds, so
    /// e2e_outbound / e2e_inbound stay byte-identical through the refactor.
    #[test]
    fn cross_chain_sync_pairs_degenerate_to_single_direction() {
        let cfg = ctx();
        let n = 3u64;
        let user = Bytes::from(vec![0x01]);

        // Outbound-only == build_outbound_load_table_txs at the same nonce.
        let out_pairs =
            build_cross_chain_sync_pairs(&[(outbound_entry(), user.clone())], &[], &cfg, n)
                .unwrap();
        let l2e = lower_entry_to_l2(&outbound_entry());
        let direct_out =
            build_outbound_load_table_txs(std::slice::from_ref(&l2e), &cfg, n).unwrap();
        assert_eq!(out_pairs.len(), 1);
        assert_eq!(
            out_pairs[0].system_tx, direct_out[0],
            "outbound-only byte-identical"
        );

        // Inbound-only == build_inbound_system_txs at the same nonce.
        let in_pairs = build_cross_chain_sync_pairs(&[], &[inbound_entry()], &cfg, n).unwrap();
        let direct_in = build_inbound_system_txs(&[inbound_entry()], &cfg, n).unwrap();
        assert_eq!(in_pairs.len(), 1);
        assert_eq!(
            in_pairs[0].system_tx, direct_in[0],
            "inbound-only byte-identical"
        );
        assert!(in_pairs[0].user_tx.is_none());
    }

    /// Composer-emit == deriver-rebuild for the canonical mixed builder across
    /// the postBatch encode/decode round-trip (the byte-equality both sides rely
    /// on, now for BOTH directions in one call).
    #[test]
    fn cross_chain_sync_pairs_emit_equals_rebuild() {
        let cfg = ctx();
        let n = 9u64;
        let user = Bytes::from(vec![0x01, 0x02]);
        let emitted = build_cross_chain_sync_pairs(
            &[(outbound_entry(), user.clone())],
            &[inbound_entry()],
            &cfg,
            n,
        )
        .unwrap();
        let out_rt = round_tripped(&[outbound_entry()])[0].clone();
        let in_rt = round_tripped(&[inbound_entry()])[0].clone();
        let rebuilt = build_cross_chain_sync_pairs(&[(out_rt, user)], &[in_rt], &cfg, n).unwrap();
        assert_eq!(emitted.len(), rebuilt.len());
        for (e, r) in emitted.iter().zip(&rebuilt) {
            assert_eq!(
                e.system_tx, r.system_tx,
                "system tx byte-identical across round-trip"
            );
            assert_eq!(e.user_tx, r.user_tx);
        }
    }

    /// A mid-chain resume rebuilds only the tail, which must be byte-equal to the
    /// composer's suffix: offset the nonce by txs the prefix EMITS, not entry count.
    #[test]
    fn resumed_tail_matches_full_build_when_offset_by_prefix_emitted_txs() {
        let cfg = ctx();
        let n = 11u64;
        let (u0, u1) = (Bytes::from(vec![0xa0]), Bytes::from(vec![0xa1]));
        let o0 = outbound_entry();
        let mut o1 = outbound_entry();
        // The grouped protocol's root outbound fixture is intentionally
        // seed-only, so distinguish the second root through its commitment.
        o1.rollingHash = B256::repeat_byte(0x99);
        // An inbound entry for ANOTHER rollup: passes the shape gate, emits no
        // system tx on this L2.
        let foreign = inbound_entry_with(2, U256::ZERO);
        let in1 = inbound_entry();

        // Composer: the whole claimed chain, nonces N.. in emit order.
        let full = build_cross_chain_sync_pairs(
            &[(o0.clone(), u0.clone()), (o1.clone(), u1.clone())],
            &[foreign.clone(), in1.clone()],
            &cfg,
            n,
        )
        .unwrap();
        assert_eq!(
            full.len(),
            3,
            "2 outbound loads + 1 delivery (the foreign entry emits none)"
        );
        assert_eq!(
            full.iter()
                .map(|p| nonce_of(&p.system_tx))
                .collect::<Vec<_>>(),
            vec![n, n + 1, n + 2],
        );

        // Deriver resume: a competing batch already settled the prefix
        // (1 outbound + the foreign inbound); only the rest ran under this batch.
        let (skip_out, skip_in) = (1usize, 1usize);
        let prefix =
            build_cross_chain_sync_pairs(&[(o0, u0)], std::slice::from_ref(&foreign), &cfg, n)
                .unwrap();
        assert_eq!(
            prefix.len(),
            1,
            "the foreign entry contributes no system tx"
        );
        assert_ne!(
            prefix.len(),
            skip_out + skip_in,
            "the emitted count must differ from the entry count here, else the \
             offset assertion below is vacuous",
        );

        let tail_out = [(o1, u1)];
        let tail_in = [in1];
        let tail_at =
            |start: u64| build_cross_chain_sync_pairs(&tail_out, &tail_in, &cfg, start).unwrap();

        let tail = tail_at(n + prefix.len() as u64);
        assert_eq!(tail.len(), full.len() - prefix.len());
        for (t, f) in tail.iter().zip(&full[prefix.len()..]) {
            assert_eq!(
                t.system_tx, f.system_tx,
                "rebuilt tail system tx must be byte-equal to the committed one",
            );
            assert_eq!(t.user_tx, f.user_tx);
        }

        // Negative control: no offset, or the entry-count offset, re-nonces the
        // tail into different bytes and derivation halts.
        for wrong in [0u64, (skip_out + skip_in) as u64] {
            assert_ne!(
                tail_at(n + wrong)[0].system_tx,
                full[prefix.len()].system_tx,
                "offset {wrong} must NOT reproduce the committed tail",
            );
        }
    }

    /// Phase-C invariant (the migration plan's ★ HIGHEST-RISK item): the composer
    /// EMITS the inbound system tx from its in-memory entries; the standalone deriver
    /// REBUILDS it from the L1 `postBatch` (encode → on-chain → decode). The shared
    /// `build_inbound_system_txs` MUST produce BYTE-IDENTICAL native txs across that
    /// encode/decode round-trip — otherwise the derived L2 block forks from the
    /// composer's and the next postBatch root mismatches: a SILENT soundness failure
    /// (based has no deriver, so this equality was never exercised upstream).
    ///
    /// The native envelope has no signature or fee fields. Exact entry bytes,
    /// chain identity and nonce progression determine its canonical identity.
    #[test]
    fn composer_emit_equals_deriver_rebuild_byte_identical() {
        let cfg = ctx();
        let nonce = 7u64;
        let entries = vec![inbound_entry()];

        let emitted = build_inbound_system_txs(&entries, &cfg, nonce).unwrap();
        assert_eq!(emitted.len(), 1, "one inbound entry → one system tx");

        let rebuilt = build_inbound_system_txs(&round_tripped(&entries), &cfg, nonce).unwrap();
        assert_eq!(
            emitted, rebuilt,
            "composer-emit must equal deriver-rebuild byte-for-byte (Phase-C invariant)",
        );
    }

    /// Guards against a vacuous pass: the signed bytes MUST vary with the nonce (so the
    /// equality above is non-trivial), and emit==rebuild must still hold at that nonce.
    #[test]
    fn byte_identity_is_non_vacuous_and_holds_across_nonces() {
        let cfg = ctx();
        let entries = vec![inbound_entry()];
        let at0 = build_inbound_system_txs(&entries, &cfg, 0).unwrap();
        let at99 = build_inbound_system_txs(&entries, &cfg, 99).unwrap();
        assert_ne!(at0, at99, "different nonce must change the signed bytes");
        assert_eq!(
            build_inbound_system_txs(&round_tripped(&entries), &cfg, 99).unwrap(),
            at99,
            "emit==rebuild at the second nonce too",
        );
    }

    /// A2.1b: the outbound `loadExecutionTable` system tx is byte-identical
    /// whether built from the composer's in-memory entry or from the entry
    /// decoded back out of the `loadExecutionTable` calldata (the deriver's
    /// rebuild) — the Q4 shared-constructor / P-1 byte-equality. One tx per
    /// entry (the SyncPair per-pair load).
    #[test]
    fn outbound_load_table_emit_equals_deriver_rebuild() {
        use crate::entries::{OutboundEntry, build_l2_outbound_entry};

        let cfg = ctx();
        let nonce = 7u64;
        let entry = build_l2_outbound_entry(OutboundEntry {
            target: address!("00000000000000000000000000000000000000cc"),
            source: address!("00000000000000000000000000000000000000dd"),
            value: U256::ZERO,
            data: Bytes::from(vec![0x55, 0x24, 0x10, 0x77, 0x07]),
            l2_rollup_id: RollupId(cfg.this_rollup_id),
            return_data: Bytes::new(),
            success: true,
        })
        .unwrap();

        let emitted =
            build_outbound_load_table_txs(std::slice::from_ref(&entry), &cfg, nonce).unwrap();
        assert_eq!(
            emitted.len(),
            1,
            "one outbound entry → one loadExecutionTable tx"
        );

        // Deriver rebuild: decode the entry back out of the loadExecutionTable
        // calldata and re-emit — must be byte-identical.
        let calldata = loadExecutionTableCall {
            _entries: vec![entry.clone()],
            _staticEntries: Vec::new(),
        }
        .abi_encode();
        let decoded =
            loadExecutionTableCall::abi_decode(&calldata).expect("loadExecutionTable round-trips");
        let rebuilt = build_outbound_load_table_txs(&decoded._entries, &cfg, nonce).unwrap();
        assert_eq!(
            emitted, rebuilt,
            "composer-emit must equal deriver-rebuild byte-for-byte"
        );

        // Non-vacuous: the signed bytes vary with the nonce.
        let at99 = build_outbound_load_table_txs(&[entry], &cfg, 99).unwrap();
        assert_ne!(
            emitted, at99,
            "different nonce must change the signed bytes"
        );
    }

    /// A2.2: the canonical Sync-block order interleaves each system tx with its
    /// optional user tx — `[s, u?, …]` — so an outbound `[load | user]` pair
    /// stays adjacent (the load's self-clean can't wipe a not-yet-consumed
    /// entry), while inbound (user `None`) reduces to `[system_txs]` exactly as
    /// the prior system-first layout.
    #[test]
    fn interleave_sync_block_txs_orders_pairs() {
        let s1 = Bytes::from(vec![0x11]);
        let u1 = Bytes::from(vec![0xA1]);
        let s2 = Bytes::from(vec![0x22]);
        let s3 = Bytes::from(vec![0x33]);
        let u3 = Bytes::from(vec![0xA3]);
        let c3 = Bytes::from(vec![0xC3]);

        let out = interleave_sync_block_txs(&[
            SyncPair {
                system_tx: s1.clone(),
                user_tx: Some(u1.clone()),
                followup_system_txs: Vec::new(),
                effect_followup_counts: vec![0],
            },
            SyncPair {
                system_tx: s2.clone(),
                user_tx: None,
                followup_system_txs: Vec::new(),
                effect_followup_counts: Vec::new(),
            },
            SyncPair {
                system_tx: s3.clone(),
                user_tx: Some(u3.clone()),
                followup_system_txs: vec![c3.clone()],
                effect_followup_counts: vec![1],
            },
        ]);
        assert_eq!(
            out,
            vec![s1, u1, s2, s3, u3, c3],
            "each effect contains its load, optional user, and callbacks in order",
        );

        // All-inbound (every user_tx None) == [system_txs] — behavior-preserving.
        let sys = [Bytes::from(vec![1]), Bytes::from(vec![2])];
        assert_eq!(
            interleave_sync_block_txs(&[
                SyncPair {
                    system_tx: sys[0].clone(),
                    user_tx: None,
                    followup_system_txs: Vec::new(),
                    effect_followup_counts: Vec::new(),
                },
                SyncPair {
                    system_tx: sys[1].clone(),
                    user_tx: None,
                    followup_system_txs: Vec::new(),
                    effect_followup_counts: Vec::new(),
                },
            ]),
            sys.to_vec(),
            "inbound (no user txs) reduces to the system-only order",
        );

        assert!(interleave_sync_block_txs(&[]).is_empty());
    }
}
