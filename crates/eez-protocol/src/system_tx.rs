//! Canonical native L2 reconstruction from checked execution tables and source identities.
//!
//! Outbound brackets have one load followed by their exact signed origin.
//! Inbound roots have one delivery; callbacks execute inline. Local plans bind
//! gas only after prior transactions have established the block-position budget.

use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_sol_types::SolCall;
use eez_primitives::{EEZL2_ADDRESS, EezTxEnvelope, SystemTransaction, system_tx_gas_limit};

use crate::RollupId;
use crate::abi::{
    CrossChainCallSol, ExecutionEntrySol, ExpectedL1ToL2CallSol, ExpectedOutgoingCrossChainCallSol,
    L2ExecutionEntrySol, L2StaticExecutionEntrySol, L2ToL1CallSol, StaticExecutionEntrySol,
    loadExecutionTableCall,
};
use crate::entries::InboundSidecar;

/// Selector for `EEZL2.loadExecutionTable`.
pub const LOAD_EXECUTION_TABLE_SELECTOR: [u8; 4] = loadExecutionTableCall::SELECTOR;

/// Execution cursor for complete cross-chain groups in one L2 block.
///
/// The transaction body determines boundaries; request metadata never does.
/// Native loads and their immediately following ordinary transaction form one
/// group, regardless of receipt status. Inbound deliveries are one-transaction
/// groups. History and unpaired ordinary transactions do not end a group.
/// Keep this cursor with the executed prefix and restore it with speculative
/// state. A proposed transition takes effect only when its transaction commits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExecutionGroupState {
    awaiting_user: bool,
}

impl ExecutionGroupState {
    /// Propose the next cursor and whether this transaction completes a group.
    /// This classifies already decoded envelopes, not source/effect authority.
    pub fn after(self, tx: &EezTxEnvelope) -> Result<(Self, bool), &'static str> {
        match tx {
            EezTxEnvelope::Ethereum(_) => Ok(self.after_ordinary()),
            EezTxEnvelope::System(_) | EezTxEnvelope::History(_) if self.awaiting_user => {
                Err("native table load must be followed immediately by its user transaction")
            }
            EezTxEnvelope::History(_) => Ok((self, false)),
            EezTxEnvelope::System(native) if native.to == EEZL2_ADDRESS => {
                if native.input.starts_with(&LOAD_EXECUTION_TABLE_SELECTOR) {
                    Ok((
                        Self {
                            awaiting_user: true,
                        },
                        false,
                    ))
                } else if native
                    .input
                    .starts_with(&crate::entries::EXECUTE_INCOMING_SELECTOR)
                {
                    Ok((Self::default(), true))
                } else {
                    Err("unsupported native group selector")
                }
            }
            EezTxEnvelope::System(_) => Err("unsupported native group destination"),
        }
    }

    /// The ordinary branch, also used for unsigned advisory executions. It
    /// neither validates a signature nor authorizes the paired origin identity.
    pub const fn after_ordinary(self) -> (Self, bool) {
        (
            Self {
                awaiting_user: false,
            },
            self.awaiting_user,
        )
    }

    /// A published block or effect checkpoint cannot end after an unpaired load.
    pub fn ensure_complete(self) -> Result<(), &'static str> {
        if self.awaiting_user {
            Err("block ends inside an outbound load/user group")
        } else {
            Ok(())
        }
    }
}

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
    /// Local rollup identity. Foreign entries are rejected before construction.
    pub this_rollup_id: u64,
}

/// Build one native delivery per checked inbound root, in the supplied order.
/// Rejects foreign destinations and invalid deployment configuration.
pub fn build_inbound_system_txs(
    entries: &[InboundSidecar],
    cfg: &SystemTxContext,
) -> Result<Vec<NativeTransactionPlan>, String> {
    entries
        .iter()
        .map(|entry| {
            if entry.destination_rollup_id().0 != cfg.this_rollup_id {
                return Err("inbound delivery targets a foreign rollup".into());
            }
            native_plan(entry.calldata(), entry.committed_value(), cfg)
        })
        .collect()
}

/// Build one native load per supplied mutable table. Each consuming signed
/// transaction must execute immediately after its load, before another load
/// replaces the table. Grouped origins use build_outbound_group instead.
/// Rejects committed-value overflow.
pub fn build_outbound_load_table_txs(
    entries: &[L2ExecutionEntrySol],
    cfg: &SystemTxContext,
) -> Result<Vec<NativeTransactionPlan>, String> {
    entries
        .iter()
        .map(|entry| {
            let calldata = loadExecutionTableCall {
                _entries: vec![entry.clone()],
                _staticEntries: Vec::new(),
            }
            .abi_encode();
            native_plan(
                calldata.into(),
                committed_l2_table_value(std::slice::from_ref(entry))?,
                cfg,
            )
        })
        .collect()
}

/// Checked native call contents. Source identity and gas are resolved only once
/// preceding whole groups have executed. This is a construction plan, never a received wire transaction.
/// Validators must reject a supplied gas mismatch, not turn supplied bytes into
/// a plan and normalize them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeTransactionPlan {
    chain_id: u64,
    value: U256,
    input: Bytes,
}

impl NativeTransactionPlan {
    /// Construct canonical bytes after deriving source identity from the original
    /// settlement row and independently executed pre-group prefix. Never copy a
    /// received envelope's sourceHash into this construction path.
    /// The caller must execute the exact returned transaction and retain it in
    /// the body; later capacity probing cannot silently alter its budget.
    pub fn at_position(
        &self,
        block_gas_limit: u64,
        gas_used: u64,
        source_hash: B256,
    ) -> Result<SystemTransaction, String> {
        if gas_used > block_gas_limit {
            return Err("native position exceeds block gas limit".into());
        }
        Ok(SystemTransaction {
            chain_id: self.chain_id,
            source_hash,
            gas_limit: system_tx_gas_limit(block_gas_limit, gas_used),
            to: EEZL2_ADDRESS,
            value: self.value,
            input: self.input.clone(),
        })
    }
}

/// A local reconstruction input. Ordinary signed bytes are already fixed;
/// native gas is bound when preceding execution has established its budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlannedTransaction {
    /// A native load or delivery reconstructed from checked semantics.
    Native(NativeTransactionPlan),
    /// The exact original signed L2 transaction following its native load.
    User(Bytes),
}

impl PlannedTransaction {
    /// Resolve a locally constructed input at its actual block position.
    pub fn at_position(
        &self,
        block_gas_limit: u64,
        gas_used: u64,
        source_hash: B256,
    ) -> Result<Bytes, String> {
        match self {
            Self::Native(plan) => Ok(plan
                .at_position(block_gas_limit, gas_used, source_hash)?
                .encoded_2718()
                .into()),
            Self::User(raw) => Ok(raw.clone()),
        }
    }
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
    pub system_tx: NativeTransactionPlan,
    /// The user tx that consumes `system_tx`'s effect in the SAME block,
    /// immediately after it. `None` for a self-contained system tx.
    pub user_tx: Option<Bytes>,
    /// Number of loaded source effects. All outbound effects end at the one
    /// user transaction; nested callbacks execute inline. Inbound pairs use one.
    pub effect_count: usize,
}

/// One source root loaded before its signed L2 origin. Nested callbacks are
/// encoded in this table and execute inline within the same user transaction.
#[derive(Clone, Debug)]
pub struct OutboundSyncEffect {
    /// Seed-only mutable or static source-table entry consumed by `user_tx`.
    pub source_entry: OutboundSourceEntry,
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
/// Nested callbacks are part of the loaded tables and execute inline.
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
pub fn interleave_sync_block_txs(pairs: &[SyncPair]) -> Vec<PlannedTransaction> {
    let mut out = Vec::with_capacity(pairs.len() * 2);
    for pair in pairs {
        out.push(PlannedTransaction::Native(pair.system_tx.clone()));
        if let Some(user_tx) = &pair.user_tx {
            out.push(PlannedTransaction::User(user_tx.clone()));
        }
    }
    out
}

/// Number of SYSTEM_ADDRESS transactions in a collection of sync pairs.
#[must_use]
pub fn sync_pair_system_tx_count(pairs: &[SyncPair]) -> usize {
    pairs.len()
}

/// Inclusive transaction index at which each logical sync effect completes.
#[must_use]
pub fn sync_pair_end_positions(pairs: &[SyncPair]) -> Vec<usize> {
    let mut next = 0;
    let mut positions = Vec::new();
    for pair in pairs {
        next += 1 + usize::from(pair.user_tx.is_some());
        positions.extend(std::iter::repeat_n(next - 1, pair.effect_count));
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
    check_static_callbacks_shape(
        entry.l2ToL1Calls.iter().map(StaticCallbackFields::from),
        entry.rollingHash,
        label,
    )
}

/// Static callbacks run with STATICCALL under an untagged result fold that
/// covers neither value nor revert spans, so a call outside that profile would
/// silently change meaning. Zero gas keeps the identity gas-independent.
/// Without callbacks the fold is zero.
fn check_static_callbacks_shape<'a>(
    calls: impl ExactSizeIterator<Item = StaticCallbackFields<'a>>,
    rolling_hash: alloy_primitives::B256,
    label: &str,
) -> Result<(), String> {
    if calls.len() == 0 && rolling_hash != alloy_primitives::B256::ZERO {
        return Err(format!(
            "{label} static entry without callbacks has a nonzero rolling hash"
        ));
    }
    for (index, call) in calls.enumerate() {
        if !call.is_static {
            return Err(format!(
                "{label} static sub-call {index} is not marked static"
            ));
        }
        if !call.value.is_zero() {
            return Err(format!(
                "{label} static sub-call {index} carries non-zero value"
            ));
        }
        if call.revert_next_n_calls != 0 {
            return Err(format!(
                "{label} static sub-call {index} carries a revert span"
            ));
        }
        if call.gas != 0 {
            return Err(format!(
                "{label} static sub-call {index} uses explicit-gas semantics"
            ));
        }
    }
    Ok(())
}

/// The static-callback fields shared by the L1 and L2 call ABIs.
struct StaticCallbackFields<'a> {
    is_static: bool,
    value: &'a U256,
    revert_next_n_calls: u16,
    gas: u64,
}

impl<'a> From<&'a L2ToL1CallSol> for StaticCallbackFields<'a> {
    fn from(call: &'a L2ToL1CallSol) -> Self {
        Self {
            is_static: call.isStatic,
            value: &call.value,
            revert_next_n_calls: call.revertNextNCalls,
            gas: call.gas,
        }
    }
}

impl<'a> From<&'a CrossChainCallSol> for StaticCallbackFields<'a> {
    fn from(call: &'a CrossChainCallSol) -> Self {
        Self {
            is_static: call.isStatic,
            value: &call.value,
            revert_next_n_calls: call.revertNextNCalls,
            gas: call.gas,
        }
    }
}

/// Check the supported L2 static-source shape, retaining its position: an L1
/// lookup, optionally with static callbacks to this L2.
/// Position authorization and the callback results require the complete
/// ordered host in `verify_outbound_source_group`; decoding and this shape
/// check are insufficient.
pub fn check_l2_static_entry_shape(
    entry: &L2StaticExecutionEntrySol,
    label: &str,
) -> Result<(), String> {
    if entry.proxyEntryHash == alloy_primitives::B256::ZERO {
        return Err(format!("{label} static entry has a zero proxy hash"));
    }
    check_static_callbacks_shape(
        entry.incomingCalls.iter().map(StaticCallbackFields::from),
        entry.rollingHash,
        label,
    )
}

/// Lower the L1-shaped sidecar transport into the field-identical L2 ABI.
///
/// The sidecar intentionally uses `ExecutionEntrySol` so it can share the
/// settlement/DA codec. L2 ignores L1-only state and destination fields; every
/// flat call and every unified nested row is retained exactly.
#[must_use]
pub fn lower_entry_to_l2(entry: &ExecutionEntrySol) -> L2ExecutionEntrySol {
    let lower_call = lower_call_to_l2;
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

/// Lower one L1-shaped call to the field-identical L2 ABI.
#[must_use]
pub(crate) fn lower_call_to_l2(call: &L2ToL1CallSol) -> CrossChainCallSol {
    CrossChainCallSol {
        revertNextNCalls: call.revertNextNCalls,
        isStatic: call.isStatic,
        gas: call.gas,
        sourceAddress: call.sourceAddress,
        sourceRollupId: call.sourceRollupId,
        targetAddress: call.targetAddress,
        value: call.value,
        data: call.data.clone(),
    }
}

/// Lift one L2 call to the field-identical L1-shaped ABI.
#[must_use]
pub(crate) fn lift_call_from_l2(call: &CrossChainCallSol) -> L2ToL1CallSol {
    L2ToL1CallSol {
        revertNextNCalls: call.revertNextNCalls,
        isStatic: call.isStatic,
        gas: call.gas,
        sourceAddress: call.sourceAddress,
        sourceRollupId: call.sourceRollupId,
        targetAddress: call.targetAddress,
        value: call.value,
        data: call.data.clone(),
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
    let lift_call = lift_call_from_l2;
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
        incomingCalls: entry.l2ToL1Calls.iter().map(lower_call_to_l2).collect(),
        rollingHash: entry.rollingHash,
        success: entry.success,
        returnData: entry.returnData.clone(),
    }
}

/// Lower a single-root outbound origin to its [load, user] group, retaining the
/// exact signed origin bytes.
///
/// # Errors
/// Rejects unsupported entry shapes and invalid native destinations.
pub fn build_outbound_pair(
    entry: &ExecutionEntrySol,
    user_tx: &Bytes,
    cfg: &SystemTxContext,
) -> Result<Vec<SyncPair>, String> {
    Ok(vec![build_outbound_group(
        &OutboundSyncGroup {
            effects: vec![OutboundSyncEffect {
                source_entry: OutboundSourceEntry::Mutable(entry.clone()),
            }],
            user_tx: user_tx.clone(),
        },
        cfg,
    )?])
}

/// Build one canonical outbound bracket as [load, user]. Every root is staged
/// by the same load; nested callbacks execute inline in the user transaction.
pub fn build_outbound_group(
    group: &OutboundSyncGroup,
    cfg: &SystemTxContext,
) -> Result<SyncPair, String> {
    if group.effects.is_empty() {
        return Err("outbound user transaction has no source effects".into());
    }
    let mut lowered = Vec::with_capacity(group.effects.len());
    let mut lowered_static = Vec::new();
    for effect in &group.effects {
        match &effect.source_entry {
            OutboundSourceEntry::Mutable(entry) => {
                check_entry_shape(entry, "outbound")?;
                if entry.proxyEntryHash == B256::ZERO {
                    return Err("outbound source-L2 entry has a zero proxyEntryHash".into());
                }
                lowered.push(lower_entry_to_l2(entry));
            }
            OutboundSourceEntry::Static(entry) => {
                check_l2_static_entry_shape(entry, "outbound")?;
                lowered_static.push(entry.clone());
            }
        }
    }
    let value = committed_l2_table_value(&lowered)?;
    let calldata = loadExecutionTableCall {
        _entries: lowered,
        _staticEntries: lowered_static,
    }
    .abi_encode();
    Ok(SyncPair {
        system_tx: native_plan(calldata.into(), value, cfg)?,
        user_tx: Some(group.user_tx.clone()),
        effect_count: group.effects.len(),
    })
}

/// Construct single-root outbound origins followed by checked inbound roots.
/// The source identities come from the complete original semantic stream.
/// Rejects unsupported entries and foreign destinations.
pub fn build_cross_chain_sync_pairs(
    outbound: &[(ExecutionEntrySol, Bytes)],
    inbound: &[InboundSidecar],
    cfg: &SystemTxContext,
) -> Result<Vec<SyncPair>, String> {
    let groups = outbound
        .iter()
        .map(|(entry, user_tx)| OutboundSyncGroup {
            effects: vec![OutboundSyncEffect {
                source_entry: OutboundSourceEntry::Mutable(entry.clone()),
            }],
            user_tx: user_tx.clone(),
        })
        .collect::<Vec<_>>();
    build_cross_chain_sync_groups(&groups, inbound, cfg)
}

/// Construct complete outbound brackets as [load, user], then inbound deliveries.
/// Source identity and gas are resolved at execution. Callbacks cannot add
/// separate transactions.
pub fn build_cross_chain_sync_groups(
    outbound: &[OutboundSyncGroup],
    inbound: &[InboundSidecar],
    cfg: &SystemTxContext,
) -> Result<Vec<SyncPair>, String> {
    let mut pairs = outbound
        .iter()
        .map(|group| build_outbound_group(group, cfg))
        .collect::<Result<Vec<_>, _>>()?;
    for system_tx in build_inbound_system_txs(inbound, cfg)? {
        pairs.push(SyncPair {
            system_tx,
            user_tx: None,
            effect_count: 1,
        });
    }
    Ok(pairs)
}

/// Encode the fixed native envelope, including the complete committed value.
/// Reject a misconfigured destination before producing privileged bytes.
fn native_plan(
    input: Bytes,
    value: U256,
    cfg: &SystemTxContext,
) -> Result<NativeTransactionPlan, String> {
    if cfg.eezl2_address != EEZL2_ADDRESS || cfg.l2_chain_id == 0 || cfg.this_rollup_id == 0 {
        return Err("invalid native transaction deployment context".into());
    }
    Ok(NativeTransactionPlan {
        chain_id: cfg.l2_chain_id,
        value,
        input,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EvmBatch;
    use crate::abi::RollupIdWithProofSystemsSol;
    use crate::entries::{decode_postbatch, encode_postbatch};
    use alloy_eips::Decodable2718 as _;
    use alloy_primitives::{address, keccak256};
    use alloy_sol_types::SolValue as _;

    fn ctx() -> SystemTxContext {
        SystemTxContext {
            eezl2_address: EEZL2_ADDRESS,
            l2_chain_id: 1,
            this_rollup_id: 1,
        }
    }
    fn inbound(rollup_id: u64, value: U256) -> InboundSidecar {
        InboundSidecar::new(crate::entries::IncomingEntry {
            target: Address::with_last_byte(0xbb),
            source: Address::with_last_byte(0xcc),
            value,
            data: Bytes::from_static(&[0x12, 0x34]),
            source_rollup_id: RollupId::MAINNET,
            l2_rollup_id: RollupId(rollup_id),
            return_data: Bytes::from_static(&[0xab, 0xcd]),
            success: true,
        })
        .unwrap()
    }
    fn outbound_entry() -> ExecutionEntrySol {
        ExecutionEntrySol {
            rootUpdates: Vec::new(),
            proxyEntryHash: B256::repeat_byte(0xcd),
            l2ToL1Calls: Vec::new(),
            expectedL1ToL2Calls: Vec::new(),
            rollingHash: B256::repeat_byte(0xef),
            destinationRollupId: 0,
            success: true,
            returnData: Bytes::from_static(&[0x77]),
        }
    }
    fn group(entries: Vec<OutboundSourceEntry>, raw: Bytes) -> OutboundSyncGroup {
        OutboundSyncGroup {
            effects: entries
                .into_iter()
                .map(|source_entry| OutboundSyncEffect { source_entry })
                .collect(),
            user_tx: raw,
        }
    }
    fn sources(count: usize) -> Vec<B256> {
        (0..count)
            .map(|i| keccak256(U256::from(i).to_be_bytes::<32>()))
            .collect()
    }
    fn encoded(plan: &NativeTransactionPlan) -> Bytes {
        plan.at_position(crate::l2::GAS_LIMIT, 0, sources(1)[0])
            .unwrap()
            .encoded_2718()
            .into()
    }
    fn decoded(plan: &NativeTransactionPlan) -> SystemTransaction {
        let raw = encoded(plan);
        let eez_primitives::EezTxEnvelope::System(tx) =
            eez_primitives::EezTxEnvelope::decode_2718_exact(&raw).unwrap()
        else {
            panic!("native")
        };
        (*tx).clone()
    }
    fn round_tripped(entries: &[ExecutionEntrySol]) -> Vec<ExecutionEntrySol> {
        let batch = EvmBatch {
            entries: entries.to_vec(),
            rollupIdsWithProofSystems: vec![RollupIdWithProofSystemsSol {
                rollupId: 1,
                proofSystemIndexes: vec![0],
            }],
            ..Default::default()
        };
        decode_postbatch(&encode_postbatch(&batch)).unwrap().entries
    }

    #[test]
    fn payable_tables_fund_outer_and_nested_incoming_values_and_reject_overflow() {
        let mut entry = inbound(1, U256::from(3)).as_abi_entry().clone();
        let mut nested = entry.l2ToL1Calls[0].clone();
        nested.value = U256::from(5);
        entry.expectedL1ToL2Calls = vec![ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: B256::repeat_byte(0x31),
            l2ToL1Calls: vec![nested],
            revertedOrStaticRollingHash: B256::ZERO,
            success: true,
            returnData: Bytes::new(),
        }];
        let checked = InboundSidecar::try_from(&entry).unwrap();
        let plans = build_inbound_system_txs(&[checked], &ctx()).unwrap();
        let tx = decoded(&plans[0]);
        assert_eq!(tx.value, U256::from(8));
        let call = crate::abi::executeIncomingCrossChainCallCall::abi_decode(&tx.input).unwrap();
        assert_eq!(call._entries.len(), 1);
        assert!(call._staticEntries.is_empty());
        let mut lowered = lower_entry_to_l2(&entry);
        assert_eq!(
            committed_l2_table_value(std::slice::from_ref(&lowered)).unwrap(),
            U256::from(8)
        );
        let load = build_outbound_load_table_txs(&[lowered.clone()], &ctx()).unwrap();
        assert_eq!(decoded(&load[0]).value, U256::from(8));
        lowered.incomingCalls[0].value = U256::MAX;
        assert!(committed_l2_table_value(&[lowered.clone()]).is_err());
        assert!(build_outbound_load_table_txs(&[lowered], &ctx()).is_err());
    }

    #[test]
    fn canonical_order_retains_source_identity_and_exact_signed_origin() {
        let user = Bytes::from_static(&[0xde, 0xad]);
        let identities = sources(2);
        let pairs = build_cross_chain_sync_pairs(
            &[(outbound_entry(), user.clone())],
            &[inbound(1, U256::ZERO)],
            &ctx(),
        )
        .unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].user_tx, Some(user.clone()));
        assert_eq!(pairs[1].user_tx, None);
        for (pair, source) in pairs.iter().zip(identities) {
            let tx = pair
                .system_tx
                .at_position(crate::l2::GAS_LIMIT, 0, source)
                .unwrap();
            assert_eq!(tx.source_hash, source);
        }
        let flat = interleave_sync_block_txs(&pairs);
        assert_eq!(flat.len(), 3);
        assert_eq!(flat[1], PlannedTransaction::User(user));
        assert_eq!(sync_pair_end_positions(&pairs), [1, 2]);
        assert_eq!(sync_pair_system_tx_count(&pairs), 2);
        assert!(interleave_sync_block_txs(&[]).is_empty());
    }

    #[test]
    fn multi_root_outbound_uses_one_load_and_one_user_with_inline_callbacks() {
        let mut first = outbound_entry();
        first.l2ToL1Calls = inbound(1, U256::from(3)).as_abi_entry().l2ToL1Calls.clone();
        let mut second = outbound_entry();
        second.proxyEntryHash = B256::repeat_byte(0xce);
        let origin = group(
            vec![
                OutboundSourceEntry::Mutable(first),
                OutboundSourceEntry::Mutable(second),
            ],
            Bytes::from_static(&[0xaa]),
        );
        let pairs = build_cross_chain_sync_groups(&[origin], &[], &ctx()).unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].effect_count, 2);
        assert_eq!(interleave_sync_block_txs(&pairs).len(), 2);
        assert_eq!(sync_pair_end_positions(&pairs), [1, 1]);
        let tx = decoded(&pairs[0].system_tx);
        let load = loadExecutionTableCall::abi_decode(&tx.input).unwrap();
        assert_eq!(load._entries.len(), 2);
        assert_eq!(load._entries[0].incomingCalls.len(), 1);
        assert_eq!(tx.value, U256::from(3));
    }

    #[test]
    fn static_outbound_uses_the_static_table_and_round_trips() {
        let source = L2StaticExecutionEntrySol {
            expectedEntryIndex: U256::from(7),
            proxyEntryHash: B256::repeat_byte(0x5a),
            incomingCalls: Vec::new(),
            rollingHash: B256::ZERO,
            success: true,
            returnData: Bytes::from_static(&[0x42]),
        };
        let origin = group(
            vec![OutboundSourceEntry::Static(source.clone())],
            Bytes::from_static(&[0xaa]),
        );
        let pair = build_outbound_group(&origin, &ctx()).unwrap();
        assert_eq!(pair.effect_count, 1);
        let tx = decoded(&pair.system_tx);
        let load = loadExecutionTableCall::abi_decode(&tx.input).unwrap();
        assert!(load._entries.is_empty());
        assert_eq!(load._staticEntries.len(), 1);
        assert_eq!(load._staticEntries[0].abi_encode(), source.abi_encode());
    }

    #[test]
    fn multi_root_settlement_host_and_reverted_retry_cursor_counts() {
        let mut host = inbound(1, U256::ZERO).as_abi_entry().clone();
        host.proxyEntryHash = B256::ZERO;
        host.destinationRollupId = 0;
        host.l2ToL1Calls.push(host.l2ToL1Calls[0].clone());
        host.expectedL1ToL2Calls = vec![ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: B256::repeat_byte(0x31),
            l2ToL1Calls: Vec::new(),
            revertedOrStaticRollingHash: B256::ZERO,
            success: true,
            returnData: Bytes::new(),
        }];
        assert_eq!(
            outbound_settlement_root_counts(&[host.clone()]).unwrap(),
            [2]
        );
        host.l2ToL1Calls[0].revertNextNCalls = 1;
        assert_eq!(outbound_settlement_root_counts(&[host]).unwrap(), [1]);
    }

    #[test]
    fn entry_lowering_preserves_multicall_nested_rows_and_failure_guards() {
        let mut entry = outbound_entry();
        let call = inbound(1, U256::ZERO).as_abi_entry().l2ToL1Calls[0].clone();
        entry.l2ToL1Calls = vec![call.clone(), call.clone()];
        entry.expectedL1ToL2Calls = vec![ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: B256::repeat_byte(0x31),
            l2ToL1Calls: vec![call],
            revertedOrStaticRollingHash: B256::ZERO,
            success: true,
            returnData: Bytes::from_static(&[0x32]),
        }];
        let lowered = lower_entry_to_l2(&entry);
        assert_eq!(lowered.incomingCalls.len(), 2);
        assert_eq!(lowered.expectedOutgoingCalls[0].incomingCalls.len(), 1);
        assert_eq!(
            lift_entry_from_l2(&lowered, RollupId(0)).abi_encode(),
            entry.abi_encode()
        );
        build_cross_chain_sync_pairs(&[(entry.clone(), Bytes::from_static(&[1]))], &[], &ctx())
            .unwrap();
        entry.expectedL1ToL2Calls[0].success = false;
        assert!(
            check_entry_shape(&entry, "outbound")
                .unwrap_err()
                .contains("no reverted rolling hash")
        );
        entry.expectedL1ToL2Calls[0].revertedOrStaticRollingHash = B256::repeat_byte(0x33);
        assert!(
            check_entry_shape(&entry, "outbound")
                .unwrap_err()
                .contains("contains sub-calls")
        );
        entry.expectedL1ToL2Calls[0].l2ToL1Calls.clear();
        check_entry_shape(&entry, "outbound").unwrap();
        let row = &lower_entry_to_l2(&entry).expectedOutgoingCalls[0];
        assert!(!row.success);
        assert_eq!(row.revertedOrStaticRollingHash, B256::repeat_byte(0x33));
    }

    #[test]
    fn invalid_destinations_identities_and_delivery_shapes_are_rejected() {
        let mut entry = outbound_entry();
        entry.proxyEntryHash = B256::ZERO;
        assert!(
            build_cross_chain_sync_pairs(&[(entry, Bytes::new())], &[], &ctx())
                .unwrap_err()
                .contains("zero proxyEntryHash")
        );
        assert!(
            build_inbound_system_txs(&[inbound(2, U256::ZERO)], &ctx())
                .unwrap_err()
                .contains("foreign")
        );
        let mut failed = inbound(1, U256::ZERO).as_abi_entry().clone();
        failed.success = false;
        assert!(InboundSidecar::try_from(&failed).is_err());
        let mut outbound = outbound_entry();
        outbound.success = false;
        build_cross_chain_sync_pairs(&[(outbound, Bytes::from_static(&[1]))], &[], &ctx()).unwrap();
        for wrong in [
            SystemTxContext {
                eezl2_address: Address::ZERO,
                ..ctx()
            },
            SystemTxContext {
                l2_chain_id: 0,
                ..ctx()
            },
            SystemTxContext {
                this_rollup_id: 0,
                ..ctx()
            },
        ] {
            assert!(build_inbound_system_txs(&[inbound(1, U256::ZERO)], &wrong).is_err());
        }
    }

    #[test]
    fn directional_and_mixed_builds_are_byte_identical_after_table_round_trip() {
        let out = outbound_entry();
        let input = inbound(1, U256::ZERO);
        let user = Bytes::from_static(&[1, 2]);
        let emitted = build_cross_chain_sync_pairs(
            &[(out.clone(), user.clone())],
            std::slice::from_ref(&input),
            &ctx(),
        )
        .unwrap();
        let out_rt = round_tripped(std::slice::from_ref(&out)).remove(0);
        let in_rt =
            InboundSidecar::try_from(&round_tripped(&[input.as_abi_entry().clone()])[0]).unwrap();
        let rebuilt = build_cross_chain_sync_pairs(&[(out_rt, user)], &[in_rt], &ctx()).unwrap();
        assert_eq!(
            interleave_sync_block_txs(&emitted),
            interleave_sync_block_txs(&rebuilt)
        );
        let direct_out = build_outbound_load_table_txs(&[lower_entry_to_l2(&out)], &ctx()).unwrap();
        let direct_in = build_inbound_system_txs(&[input], &ctx()).unwrap();
        assert_eq!(encoded(&direct_out[0]), encoded(&emitted[0].system_tx));
        assert_eq!(encoded(&direct_in[0]), encoded(&emitted[1].system_tx));
    }

    #[test]
    fn partial_suffix_retains_original_source_and_position_gas() {
        let origins = [
            (outbound_entry(), Bytes::from_static(&[1])),
            (outbound_entry(), Bytes::from_static(&[2])),
        ];
        let incoming = [inbound(1, U256::ZERO)];
        let full = build_cross_chain_sync_pairs(&origins, &incoming, &ctx()).unwrap();
        let tail = build_cross_chain_sync_pairs(&origins[1..], &incoming, &ctx()).unwrap();
        for (expected, actual) in full[1..].iter().zip(&tail) {
            assert_eq!(expected.system_tx, actual.system_tx);
            let gas_used = 20_000_000;
            assert_eq!(
                expected
                    .system_tx
                    .at_position(crate::l2::GAS_LIMIT, gas_used, sources(1)[0])
                    .unwrap()
                    .encoded_2718(),
                actual
                    .system_tx
                    .at_position(crate::l2::GAS_LIMIT, gas_used, sources(1)[0])
                    .unwrap()
                    .encoded_2718()
            );
        }
        let source = crate::native_source::LiveSourceContext::new(1, B256::repeat_byte(7)).unwrap();
        let original = source.entry(2).hash();
        let renumbered = source.entry(1).hash();
        assert_ne!(
            tail[0]
                .system_tx
                .at_position(crate::l2::GAS_LIMIT, 20_000_000, original)
                .unwrap(),
            tail[0]
                .system_tx
                .at_position(crate::l2::GAS_LIMIT, 20_000_000, renumbered)
                .unwrap()
        );
    }

    #[test]
    fn native_bytes_commit_source_and_exact_remaining_gas_without_normalization() {
        let plan = build_inbound_system_txs(&[inbound(1, U256::ZERO)], &ctx())
            .unwrap()
            .remove(0);
        let full = plan
            .at_position(crate::l2::GAS_LIMIT, 0, sources(1)[0])
            .unwrap();
        let tail = plan
            .at_position(crate::l2::GAS_LIMIT, 20_000_000, sources(1)[0])
            .unwrap();
        assert_eq!(full.gas_limit, eez_primitives::SYSTEM_TX_GAS_CAP);
        assert_eq!(tail.gas_limit, 10_000_000);
        assert_ne!(full.encoded_2718(), tail.encoded_2718());
        assert!(
            plan.at_position(
                crate::l2::GAS_LIMIT,
                crate::l2::GAS_LIMIT + 1,
                sources(1)[0]
            )
            .is_err()
        );
        let other = plan
            .at_position(crate::l2::GAS_LIMIT, 0, B256::repeat_byte(99))
            .unwrap();
        assert_ne!(full.encoded_2718(), other.encoded_2718());
        assert_eq!(
            decoded(&plan).to,
            address!("4B2760F84B83d6b3829E30A7573B0172B6d6402C")
        );
    }
}
