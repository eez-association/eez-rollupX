//! Bounded postBatch estimates for the settlement policy. These are admission
//! reserves, not execution guarantees: final ordered native execution still
//! validates the signed bundle and every intended effect.

use alloy_primitives::{Address, B256, U256, keccak256};
use alloy_sol_types::SolValue as _;
use eez_protocol::{CallMode, ExecutedAction, ExecutionOutcome, RollupId, abi::ExecutionEntrySol};
use reth_storage_api::StateProvider;

use super::{
    POSTBATCH_BASE_GAS_PIN, POSTBATCH_ENTRY_GAS_PIN, TxL1Gas, projected_postbatch_gas,
    projected_tx_l1_gas,
};
use crate::local::L1TargetSession;

mod deferred;
pub(super) mod framing;

#[derive(Clone, Copy, Debug)]
pub(super) struct CompanionEstimate {
    pub gas: u64,
    pub protocol_reserve: Option<u64>,
    pub basis: &'static str,
}

impl CompanionEstimate {
    pub fn declared(gas: u64) -> Self {
        Self {
            gas,
            protocol_reserve: None,
            basis: "declared_limit",
        }
    }

    fn bounded(simulated: u64, callback_gas: u64, protocol_reserve: u64, declared: u64) -> Self {
        // An admission prediction above the signed limit is not evidence that
        // this source is invalid. Native execution remains the acceptance gate.
        let predicted = simulated
            .saturating_add(callback_gas)
            .saturating_add(protocol_reserve);
        Self {
            gas: predicted.min(declared),
            protocol_reserve: Some(protocol_reserve),
            basis: if predicted > declared {
                "bounded_capped"
            } else {
                "bounded_protocol"
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum EstimateReason {
    ParentStateUnavailable,
    ImmediateRegistered,
    ImmediateEvidenceUnavailable,
    DeferredStorage,
    DeferredLegacy,
    DeferredOldSlot,
    DeferredShape,
    DeferredSizeBound,
    DeferredCallFlags,
    GeneralReserveLower,
}

const REASON_NAMES: [&str; 10] = [
    "parent_state_unavailable",
    "immediate_registered",
    "immediate_evidence_unavailable",
    "deferred_storage",
    "deferred_legacy",
    "deferred_old_slot",
    "deferred_shape",
    "deferred_size_bound",
    "deferred_call_flags",
    "general_reserve_lower",
];

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct EstimateReasons([u64; REASON_NAMES.len()]);

impl EstimateReasons {
    pub(super) fn unavailable(entries: u64) -> Self {
        let mut reasons = Self::default();
        reasons.0[EstimateReason::ParentStateUnavailable as usize] = entries;
        reasons
    }

    pub(super) fn json(self) -> serde_json::Value {
        serde_json::Value::Object(
            REASON_NAMES
                .into_iter()
                .zip(self.0)
                .filter(|(_, count)| *count != 0)
                .map(|(name, count)| (name.to_owned(), count.into()))
                .collect(),
        )
    }
}

// Caller-frame gas (before refunds), pinned on Cancun/Prague/Osaka by
// contracts/test/PostBatchGasPins.t.sol. Intrinsic/calldata/target gas and the
// operator's percentage margin are separate. Unknown shapes keep the old pin.
const DEFERRED_ENTRY_GAS: u64 = 270_000;
const SHORT_DEFERRED_ENTRY_GAS: u64 = 230_000;
const WORD_DEFERRED_ENTRY_GAS: u64 = 250_000;
const EXISTING_PROXY_ENTRY_GAS: u64 = 70_000;
// Bounded successful multi-call/nested immediate entries reuse registered
// proxies. Their reentrant table is transient; deferred queue storage uses a
// different model. Target execution and calldata remain separate charges.
const EXTRA_IMMEDIATE_CALL_GAS: u64 = 40_000;
const IMMEDIATE_RESULT_ROW_GAS: u64 = 35_000;
const MAX_IMMEDIATE_CALLS: usize = 4;
const MAX_IMMEDIATE_ROWS: usize = 4;
// A cold zero-to-nonzero SSTORE costs 17,100 more than overwriting an
// originally nonzero word. Reserve 2,100 of that saving for variation. Only
// fixed words observed nonzero in the exact parent earn this discount; dynamic
// result slots and speculative writes never do. Pinned transitions are exercised
// by PostBatchGasPins plus native repeated-wave preflight checks.
const RETAINED_WORD_DISCOUNT: u64 = 15_000;
const MAX_SIMPLE_RESULT_BYTES: usize = 64;
pub(super) const MAX_SHAPED_ENTRIES: u64 = 128;

// Cold protocol caller frames on Cancun/Prague/Osaka, including maximum queues,
// reentrant rows and reverted spans. Application work is charged separately.
// Matching stale identities retain more headroom because they read root pins.
const COMPANION_ROOT_GAS: u64 = 70_000;
const COMPANION_CALLBACK_GAS: u64 = 35_000;
const COMPANION_ROW_GAS: u64 = 25_000;
const COMPANION_WORD_GAS: u64 = 2_300;
const COMPANION_REVERT_SPAN_GAS: u64 = 25_000;
const COMPANION_SCAN_GAS: u64 = 2_600;
const COMPANION_MATCHING_SCAN_GAS: u64 = 12_500;

/// Minimum contribution under every supported admission pricing path. This is
/// a lower bound on the estimator, not on consensus execution gas. A valid
/// mutable source contributes at least one entry. Static-only inbound sources
/// require a separate lower bound with general pins. Malformed/empty sources
/// still reach simulation on a fresh candidate before any capacity deferral.
pub(super) const fn minimum_entry_gas(immediate: bool) -> u64 {
    if immediate {
        EXISTING_PROXY_ENTRY_GAS
    } else {
        // Seven entry slots plus the three words of its stitched root update.
        // The legacy retained-word path is more expensive than this minimum.
        deferred::ENTRY_GAS + 10 * deferred::ZERO_WORD_GAS
    }
}

/// Evidence belongs to one pinned canonical parent, never a speculative proxy
/// creation earlier in the drain. Accepted outbound positions retain their
/// own eligibility, so identical calls with different outcomes cannot share
/// a discount. Root stitching may change roots/hashes, but not the call ABI.
pub(super) struct Projection {
    rollup_id: u64,
    pub(super) cleanup_gas: u64,
    immediate_calls: Vec<Option<B256>>,
    deferred_slots: Vec<bool>,
    deferred_discounts: Vec<u64>,
    deferred_storage: Vec<Option<deferred::Slot>>,
    deferred_count: usize,
    deferred_hashes: Vec<B256>,
    parent_proxies: std::collections::HashMap<(u64, Address), bool>,
}

impl Projection {
    pub fn open(
        state: &dyn StateProvider,
        registry: Address,
        rollup_id: u64,
    ) -> Result<Option<Self>, String> {
        let queue_head =
            U256::from_be_bytes(keccak256((U256::from(rollup_id), U256::from(3)).abi_encode()).0)
                .wrapping_add(U256::from(1));
        // The mapping retains payloads beyond its active length. Inspect every
        // index we may discount, including inactive old slots. Any unknown
        // shape keeps the full per-entry reserve.
        let mut deferred_slots = Vec::with_capacity(MAX_SHAPED_ENTRIES as usize);
        let mut deferred_discounts = Vec::with_capacity(MAX_SHAPED_ENTRIES as usize);
        let mut deferred_storage = Vec::with_capacity(MAX_SHAPED_ENTRIES as usize);
        for index in 0..MAX_SHAPED_ENTRIES {
            let entry_slot =
                U256::from_be_bytes(keccak256((U256::from(index), queue_head).abi_encode()).0);
            let bounded = bounded_old_slot(entry_slot, |slot| {
                state
                    .storage(registry, B256::from(slot))
                    .map(Option::unwrap_or_default)
                    .map_err(|e| e.to_string())
            })?;
            let discount = if bounded {
                retained_word_discount(entry_slot, |slot| {
                    state
                        .storage(registry, B256::from(slot))
                        .map(Option::unwrap_or_default)
                        .map_err(|e| e.to_string())
                })?
            } else {
                0
            };
            deferred_discounts.push(discount);
            deferred_slots.push(bounded);
            deferred_storage.push(deferred::Slot::read(entry_slot, |slot| {
                state
                    .storage(registry, B256::from(slot))
                    .map(Option::unwrap_or_default)
                    .map_err(|e| e.to_string())
            })?);
        }
        Ok(Some(Self {
            rollup_id,
            cleanup_gas: 0,
            immediate_calls: Vec::new(),
            deferred_slots,
            deferred_discounts,
            deferred_storage,
            deferred_count: 0,
            deferred_hashes: Vec::new(),
            parent_proxies: Default::default(),
        }))
    }

    pub fn initial_gas(&self) -> u64 {
        21_000 + POSTBATCH_BASE_GAS_PIN + self.cleanup_gas
    }

    /// Source simulation intercepts cross-chain calls and omits queue decoding,
    /// proxy dispatch and rolling hashes. Cover only measured bounded shapes;
    /// other sources retain their entire signed limit as consumption reserve.
    pub fn companion_estimate(
        &mut self,
        entries: &[ExecutionEntrySol],
        actions: &[ExecutedAction],
        simulated: super::CompositionGas,
        declared: u64,
        parent: &mut L1TargetSession,
    ) -> Result<CompanionEstimate, String> {
        let Some(reserve) = self.companion_reserve(entries) else {
            return Ok(CompanionEstimate::declared(declared));
        };
        let Some(input_discount) = self.companion_action_discount(entries, actions) else {
            return Ok(CompanionEstimate::declared(declared));
        };
        // The shape reserve includes eight input words per nested row; validated
        // action evidence discounts only those words, never queue/rollback work.
        let reserve = reserve - input_discount;
        for call in entries.iter().flat_map(|entry| {
            entry.l2ToL1Calls.iter().chain(
                entry
                    .expectedL1ToL2Calls
                    .iter()
                    .flat_map(|row| &row.l2ToL1Calls),
            )
        }) {
            let key = (call.sourceRollupId, call.sourceAddress);
            let registered = if let Some(known) = self.parent_proxies.get(&key) {
                *known
            } else {
                let found = parent
                    .proxy_is_registered(call.sourceAddress, RollupId(call.sourceRollupId))
                    .map_err(|error| error.to_string())?;
                self.parent_proxies.insert(key, found);
                found
            };
            if !registered {
                return Ok(CompanionEstimate::declared(declared));
            }
        }
        // Intercepted cross-chain frames return gas to their source caller.
        // Every L1 callback's measured application work is therefore separate
        // from source simulation, including work in a reverted span.
        Ok(CompanionEstimate::bounded(
            simulated.source,
            simulated.l1_target,
            reserve,
            declared,
        ))
    }

    fn companion_action_discount(
        &self,
        entries: &[ExecutionEntrySol],
        actions: &[ExecutedAction],
    ) -> Option<u64> {
        self.companion_reserve(entries)?;
        let expected = entries
            .iter()
            .map(|entry| {
                1 + entry.l2ToL1Calls.len()
                    + entry
                        .expectedL1ToL2Calls
                        .iter()
                        .map(|row| 1 + row.l2ToL1Calls.len())
                        .sum::<usize>()
            })
            .sum::<usize>();
        if actions.len() != expected {
            return None;
        }
        let roots: Vec<_> = actions
            .iter()
            .enumerate()
            .filter_map(|(index, action)| action.parent_index.is_none().then_some(index))
            .collect();
        if roots.len() != entries.len() || roots.first() != Some(&0) {
            return None;
        }
        let mut discount = 0;
        for (entry_index, entry) in entries.iter().enumerate() {
            let start = roots[entry_index];
            let end = roots.get(entry_index + 1).copied().unwrap_or(actions.len());
            let root = &actions[start];
            if !matches!(&root.outcome, ExecutionOutcome::Resolved { success, return_data, .. }
                if *success == entry.success && return_data.as_slice() == entry.returnData.as_ref())
            {
                return None;
            }
            let mut calls: Vec<_> = entry
                .l2ToL1Calls
                .iter()
                .chain(
                    entry
                        .expectedL1ToL2Calls
                        .iter()
                        .flat_map(|row| &row.l2ToL1Calls),
                )
                .collect();
            let mut rows = entry.expectedL1ToL2Calls.iter();
            let mut ancestors = Vec::new();
            for (index, action) in actions.iter().enumerate().take(end).skip(start) {
                if let Some(parent) = action.parent_index {
                    let position = ancestors.iter().position(|p| *p == parent)?;
                    ancestors.truncate(position + 1);
                } else if index != start {
                    return None;
                }
                let depth = ancestors.len();
                if depth > 3
                    || action.revert_spans.len() > 1
                    || action.call_mode != CallMode::Mutable
                    || !action.value.is_zero()
                    || action.data.len() > 256
                {
                    return None;
                }
                let outbound = depth % 2 == 1;
                if outbound {
                    if action.source_rollup_id.0 != self.rollup_id
                        || action.target_rollup_id != RollupId::MAINNET
                    {
                        return None;
                    }
                    let call_index = calls.iter().position(|call| {
                        action.source_address == call.sourceAddress
                            && action.target_address == call.targetAddress
                            && action.data == call.data
                            && action.revert_spans.first().copied().unwrap_or(0)
                                == u32::from(call.revertNextNCalls)
                    })?;
                    calls.swap_remove(call_index);
                    if !matches!(&action.outcome, ExecutionOutcome::Resolved { success: true, return_data, .. }
                        if return_data.len() <= 64)
                    {
                        return None;
                    }
                } else {
                    if action.source_rollup_id != RollupId::MAINNET
                        || action.target_rollup_id.0 != self.rollup_id
                        || !action.revert_spans.is_empty()
                    {
                        return None;
                    }
                    if depth == 2 {
                        // Nested resolution copies the expected-call table on
                        // each reentry. Measured bounded shapes require at least
                        // four words of allowance even for empty call input.
                        let words = action.data.len().div_ceil(32).max(4);
                        discount += COMPANION_WORD_GAS * (8 - words) as u64;
                        let row = rows.next()?;
                        if !matches!(&action.outcome, ExecutionOutcome::Resolved { success, return_data, .. }
                            if *success == row.success && return_data.as_slice() == row.returnData.as_ref())
                        {
                            return None;
                        }
                    }
                }
                ancestors.push(index);
            }
            if !calls.is_empty() || rows.next().is_some() {
                return None;
            }
        }
        Some(discount)
    }

    fn companion_reserve(&self, entries: &[ExecutionEntrySol]) -> Option<u64> {
        if !(1..=4).contains(&entries.len())
            || self.deferred_count.saturating_add(entries.len()) > MAX_SHAPED_ENTRIES as usize
        {
            return None;
        }
        let mut reserve = 0;
        for (index, entry) in entries.iter().enumerate() {
            // Reject unmeasured array sizes before collecting or traversing
            // their contents. The action evidence then has at most nine nodes
            // per root (one root, four callbacks and four reentries).
            if entry.expectedL1ToL2Calls.len() > 4
                || entry.l2ToL1Calls.len() > 4
                || entry
                    .expectedL1ToL2Calls
                    .iter()
                    .any(|row| row.l2ToL1Calls.len() > 4)
            {
                return None;
            }
            let arrays: Vec<_> = std::iter::once(&entry.l2ToL1Calls)
                .chain(entry.expectedL1ToL2Calls.iter().map(|row| &row.l2ToL1Calls))
                .collect();
            let calls: Vec<_> = arrays.iter().flat_map(|calls| calls.iter()).collect();
            if entry.proxyEntryHash.is_zero()
                || entry.destinationRollupId != self.rollup_id
                || entry.rootUpdates.len() > 1
                || entry
                    .rootUpdates
                    .iter()
                    .any(|r| r.rollupId != self.rollup_id || !r.etherDelta.is_zero())
                || entry.expectedL1ToL2Calls.len() > 4
                || calls.len() > 4
                || entry.returnData.len() > 256
                || entry.expectedL1ToL2Calls.iter().any(|r| {
                    !r.success
                        || !r.revertedOrStaticRollingHash.is_zero()
                        || r.returnData.len() > 64
                })
                || arrays.iter().any(|array| {
                    array.iter().enumerate().any(|(i, c)| {
                        c.sourceRollupId != self.rollup_id
                            || c.data.len() > 256
                            || usize::from(c.revertNextNCalls) > array.len() - i
                            || c.isStatic
                            || c.gas != 0
                            || !c.value.is_zero()
                    })
                })
            {
                return None;
            }
            // Any outer source revert can restore the cursor. Charge the full
            // prior queue, including earlier roots in this same source group.
            let scan = self
                .deferred_hashes
                .iter()
                .copied()
                .chain(entries[..index].iter().map(|e| e.proxyEntryHash))
                .map(|hash| {
                    if hash == entry.proxyEntryHash {
                        COMPANION_MATCHING_SCAN_GAS
                    } else {
                        COMPANION_SCAN_GAS
                    }
                })
                .sum::<u64>();
            let words = entry.returnData.len().div_ceil(32)
                + calls
                    .iter()
                    .map(|c| c.data.len().div_ceil(32))
                    .sum::<usize>()
                + entry
                    .expectedL1ToL2Calls
                    .iter()
                    .map(|r| 8 + r.returnData.len().div_ceil(32))
                    .sum::<usize>();
            let spans = calls.iter().filter(|c| c.revertNextNCalls != 0).count();
            reserve += COMPANION_ROOT_GAS
                + COMPANION_CALLBACK_GAS * calls.len() as u64
                + COMPANION_WORD_GAS * words as u64
                + scan
                + COMPANION_ROW_GAS * entry.expectedL1ToL2Calls.len() as u64
                + COMPANION_REVERT_SPAN_GAS * spans as u64;
        }
        Some(reserve)
    }

    /// Evidence includes successful bounded execution and every L1 source
    /// proxy's registration at the canonical parent. Entry success alone does
    /// not establish success of its nested target calls.
    pub fn immediate_evidence(
        &mut self,
        entries: &[ExecutionEntrySol],
        actions: &[ExecutedAction],
        parent: &mut L1TargetSession,
    ) -> Result<Option<B256>, String> {
        let [entry] = entries else {
            return Ok(None);
        };
        let evidence = if let [action] = actions
            && self.simple_immediate_action(entry, action)
        {
            keccak256(entry.l2ToL1Calls[0].abi_encode())
        } else if self.bounded_immediate_actions(entry, actions) {
            Self::immediate_shape_key(entry)
        } else {
            return Ok(None);
        };
        for call in entry.l2ToL1Calls.iter().chain(
            entry
                .expectedL1ToL2Calls
                .iter()
                .flat_map(|row| &row.l2ToL1Calls),
        ) {
            let key = (call.sourceRollupId, call.sourceAddress);
            let registered = if let Some(known) = self.parent_proxies.get(&key) {
                *known
            } else {
                let found = parent
                    .proxy_is_registered(call.sourceAddress, RollupId(call.sourceRollupId))
                    .map_err(|e| e.to_string())?;
                self.parent_proxies.insert(key, found);
                found
            };
            if !registered {
                return Ok(None);
            }
        }
        Ok(Some(evidence))
    }

    fn bounded_immediate_gas(&self, entry: &ExecutionEntrySol) -> Option<u64> {
        let calls = entry.l2ToL1Calls.iter().chain(
            entry
                .expectedL1ToL2Calls
                .iter()
                .flat_map(|row| &row.l2ToL1Calls),
        );
        let count = calls.clone().count();
        if entry.rootUpdates.len() > 1
            || entry
                .rootUpdates
                .iter()
                .any(|u| u.rollupId != self.rollup_id || !u.etherDelta.is_zero())
            || !entry.proxyEntryHash.is_zero()
            || !entry.success
            || !entry.returnData.is_empty()
            || entry.destinationRollupId != self.rollup_id
            || entry.l2ToL1Calls.is_empty()
            || !(1..=MAX_IMMEDIATE_CALLS).contains(&count)
            || entry.expectedL1ToL2Calls.len() > MAX_IMMEDIATE_ROWS
            || (count == 1 && entry.expectedL1ToL2Calls.is_empty())
            || entry.expectedL1ToL2Calls.iter().any(|row| {
                !row.success
                    || !row.revertedOrStaticRollingHash.is_zero()
                    || row.returnData.len() > MAX_SIMPLE_RESULT_BYTES
            })
            || calls.into_iter().any(|call| {
                call.sourceRollupId != self.rollup_id
                    || call.data.len() > 256
                    || !call.value.is_zero()
                    || call.gas != 0
                    || call.isStatic
                    || call.revertNextNCalls != 0
            })
        {
            return None;
        }
        Some(
            EXISTING_PROXY_ENTRY_GAS
                + (count as u64 - 1) * EXTRA_IMMEDIATE_CALL_GAS
                + entry.expectedL1ToL2Calls.len() as u64 * IMMEDIATE_RESULT_ROW_GAS,
        )
    }

    fn bounded_immediate_actions(
        &self,
        entry: &ExecutionEntrySol,
        actions: &[ExecutedAction],
    ) -> bool {
        if self.bounded_immediate_gas(entry).is_none() {
            return false;
        }
        let mut calls: Vec<_> = entry
            .l2ToL1Calls
            .iter()
            .chain(
                entry
                    .expectedL1ToL2Calls
                    .iter()
                    .flat_map(|row| &row.l2ToL1Calls),
            )
            .collect();
        if actions.len() != calls.len() + entry.expectedL1ToL2Calls.len() {
            return false;
        }
        let mut rows = 0;
        for (index, action) in actions.iter().enumerate() {
            // Measured family: top-level L1 calls, their L2 reentries, and
            // leaf L1 callbacks. Deeper recursion retains the general pin.
            let depth = match action.parent_index {
                None => 0,
                Some(parent) if parent < index => match actions[parent].parent_index {
                    None => 1,
                    Some(root) if root < parent && actions[root].parent_index.is_none() => 2,
                    _ => return false,
                },
                _ => return false,
            };
            if (action.target_rollup_id == RollupId::MAINNET) != (depth % 2 == 0) {
                return false;
            }
            if !action.revert_spans.is_empty()
                || action.call_mode != CallMode::Mutable
                || !action.value.is_zero()
                || action.data.len() > 256
                || !matches!(&action.outcome, ExecutionOutcome::Resolved { success: true, return_data, .. } if return_data.len() <= MAX_SIMPLE_RESULT_BYTES)
            {
                return false;
            }
            if action.target_rollup_id == RollupId::MAINNET {
                let Some(index) = calls.iter().position(|call| {
                    action.source_rollup_id.0 == call.sourceRollupId
                        && action.source_address == call.sourceAddress
                        && action.target_address == call.targetAddress
                        && action.data == call.data
                }) else {
                    return false;
                };
                calls.swap_remove(index);
            } else if action.source_rollup_id == RollupId::MAINNET
                && action.target_rollup_id.0 == self.rollup_id
            {
                rows += 1;
            } else {
                return false;
            }
        }
        calls.is_empty() && rows == entry.expectedL1ToL2Calls.len()
    }

    /// Root stitching changes position keys and rolling hashes. Only those
    /// derived fields are excluded; call arrays, results and row ordering stay
    /// bound to this occurrence's checked execution evidence.
    fn immediate_shape_key(entry: &ExecutionEntrySol) -> B256 {
        let mut shape = entry.clone();
        shape.rootUpdates.clear();
        shape.rollingHash = B256::ZERO;
        for row in &mut shape.expectedL1ToL2Calls {
            row.expectedL1toL2Hash = B256::ZERO;
        }
        keccak256(shape.abi_encode())
    }

    fn simple_immediate_action(&self, entry: &ExecutionEntrySol, action: &ExecutedAction) -> bool {
        if !self.simple_immediate_shape(entry) {
            return false;
        }
        let call = &entry.l2ToL1Calls[0];
        action.parent_index.is_none()
            && action.revert_spans.is_empty()
            && action.call_mode == CallMode::Mutable
            && action.source_rollup_id.0 == call.sourceRollupId
            && action.source_address == call.sourceAddress
            && action.target_rollup_id == RollupId::MAINNET
            && action.target_address == call.targetAddress
            && action.data == call.data
            && action.value == call.value
            && matches!(&action.outcome, ExecutionOutcome::Resolved { success: true, return_data, .. } if return_data.len() <= MAX_SIMPLE_RESULT_BYTES)
    }

    fn common_shape(&self, entry: &ExecutionEntrySol) -> bool {
        // Before stitching there may be no update; the final batch has one.
        entry.rootUpdates.len() <= 1
            && entry
                .rootUpdates
                .iter()
                .all(|u| u.rollupId == self.rollup_id)
            && entry.expectedL1ToL2Calls.is_empty()
            && entry.success
            && entry.returnData.len() <= MAX_SIMPLE_RESULT_BYTES
    }

    fn simple_immediate_shape(&self, entry: &ExecutionEntrySol) -> bool {
        self.common_shape(entry)
            && entry.proxyEntryHash.is_zero()
            && entry.returnData.is_empty()
            && entry.rootUpdates.iter().all(|u| u.etherDelta.is_zero())
            && matches!(entry.l2ToL1Calls.as_slice(), [call]
                if call.sourceRollupId == self.rollup_id && call.data.len() <= 256
                && call.value.is_zero() && call.gas == 0 && !call.isStatic
                && call.revertNextNCalls == 0)
    }

    fn entry_gas(
        &self,
        entry: &ExecutionEntrySol,
        immediate: bool,
        established: Option<B256>,
    ) -> u64 {
        if immediate
            && self.simple_immediate_shape(entry)
            && established == Some(keccak256(entry.l2ToL1Calls[0].abi_encode()))
        {
            return EXISTING_PROXY_ENTRY_GAS;
        }
        if immediate
            && established.is_some()
            && let Some(gas) = self.bounded_immediate_gas(entry)
            && established == Some(Self::immediate_shape_key(entry))
        {
            return gas.min(POSTBATCH_ENTRY_GAS_PIN);
        }
        POSTBATCH_ENTRY_GAS_PIN
    }

    fn legacy_deferred_entry_gas(&self, entry: &ExecutionEntrySol, index: usize) -> u64 {
        if self.deferred_slots.get(index) != Some(&true)
            || !self.common_shape(entry)
            || entry.proxyEntryHash.is_zero()
            || entry.destinationRollupId != self.rollup_id
            || !entry.l2ToL1Calls.is_empty()
        {
            return POSTBATCH_ENTRY_GAS_PIN;
        }
        let fresh = match entry.returnData.len() {
            0..=31 => SHORT_DEFERRED_ENTRY_GAS,
            32 => WORD_DEFERRED_ENTRY_GAS,
            _ => DEFERRED_ENTRY_GAS,
        };
        fresh.saturating_sub(self.deferred_discounts.get(index).copied().unwrap_or(0))
    }

    fn deferred_price(&self, entry: &ExecutionEntrySol, index: usize) -> (u64, EstimateReason) {
        let legacy = self.legacy_deferred_entry_gas(entry, index);
        let priced = self
            .deferred_storage
            .get(index)
            .and_then(Option::as_ref)
            .ok_or(EstimateReason::DeferredOldSlot)
            .and_then(|slot| slot.gas(entry, self.rollup_id));
        match priced {
            Ok(gas) if gas < legacy => (gas, EstimateReason::DeferredStorage),
            _ if legacy < POSTBATCH_ENTRY_GAS_PIN => (legacy, EstimateReason::DeferredLegacy),
            Ok(_) => (legacy, EstimateReason::GeneralReserveLower),
            Err(reason) => (legacy, reason),
        }
    }

    fn deferred_entry_gas(&self, entry: &ExecutionEntrySol, index: usize) -> u64 {
        self.deferred_price(entry, index).0
    }

    pub fn cost(
        &self,
        entries: &[ExecutionEntrySol],
        target_gas: u64,
        immediate: bool,
        established: Option<B256>,
    ) -> TxL1Gas {
        let mut cost = projected_tx_l1_gas(entries, target_gas);
        cost.estimate_reasons = EstimateReasons::default();
        cost.entry_discount = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let (priced, reason) = if immediate {
                    let gas = self.entry_gas(entry, true, established);
                    (
                        gas,
                        if gas < POSTBATCH_ENTRY_GAS_PIN {
                            EstimateReason::ImmediateRegistered
                        } else {
                            EstimateReason::ImmediateEvidenceUnavailable
                        },
                    )
                } else {
                    self.deferred_price(entry, self.deferred_count + index)
                };
                cost.estimate_reasons.0[reason as usize] += 1;
                POSTBATCH_ENTRY_GAS_PIN - priced
            })
            .fold(0, u64::saturating_add);
        cost
    }

    pub fn accept_immediate(&mut self, count: usize, established: Option<B256>) {
        self.immediate_calls
            .extend(std::iter::repeat_n(established, count));
    }

    pub fn accept_deferred(&mut self, entries: &[ExecutionEntrySol]) {
        self.deferred_hashes.extend(
            entries
                .iter()
                .take((MAX_SHAPED_ENTRIES as usize).saturating_sub(self.deferred_count))
                .map(|entry| entry.proxyEntryHash),
        );
        self.deferred_count = self.deferred_count.saturating_add(entries.len());
    }

    pub fn batch_gas(&self, batch: &eez_protocol::EvmBatch, calldata: u64, target: u64) -> u64 {
        // This is the composer's native single-rollup ECDSA family. Keep the
        // conservative projection if another caller ever supplies a different one.
        let anchor = batch.entries.first().is_some_and(|entry| {
            entry.destinationRollupId == self.rollup_id
                && entry.proxyEntryHash.is_zero()
                && entry.success
                && entry.returnData.is_empty()
                && entry.l2ToL1Calls.is_empty()
                && entry.expectedL1ToL2Calls.is_empty()
                && matches!(entry.rootUpdates.as_slice(), [update]
                    if update.rollupId == self.rollup_id && update.etherDelta.is_zero())
        });
        if !anchor
            || batch.immediateEntryCount.is_zero()
            || batch.blobIndices.len() > 64
            || !batch.staticEntries.is_empty()
            || !matches!(batch.expectedRootPerRollup.as_slice(), [pin]
                if pin.rollupId == self.rollup_id && !pin.root.is_zero())
            || !batch.immediateStaticEntryCount.is_zero()
            || batch.proofSystems.len() != 1
            || batch.rollupIdsWithProofSystems.len() != 1
            || batch.rollupIdsWithProofSystems[0].rollupId != self.rollup_id
            || batch.rollupIdsWithProofSystems[0].proofSystemIndexes != [0]
            || !batch.callData.is_empty()
        {
            return projected_postbatch_gas(batch.entries.len() as u64, calldata, target)
                .saturating_add(self.cleanup_gas);
        }
        let discounted = batch.entries.len() as u64 <= MAX_SHAPED_ENTRIES
            && batch.staticEntries.is_empty()
            && batch.immediateEntryCount == U256::from(self.immediate_calls.len() + 1);
        let entries = batch
            .entries
            .iter()
            .skip(1)
            .enumerate()
            .map(|(index, entry)| {
                if discounted {
                    if index < self.immediate_calls.len() {
                        self.entry_gas(entry, true, self.immediate_calls[index])
                    } else {
                        self.deferred_entry_gas(entry, index - self.immediate_calls.len())
                    }
                } else {
                    POSTBATCH_ENTRY_GAS_PIN
                }
            })
            .fold(0, u64::saturating_add);
        self.initial_gas()
            .saturating_add(entries)
            .saturating_add(calldata)
            .saturating_add(target)
    }
}

/// Six fixed words only: proxy hash, rolling hash, destination/success and
/// the one retained root update's packed identity/delta and two roots. The
/// existing bounded-shape check is required before calling this helper.
fn retained_word_discount<E>(
    entry_slot: U256,
    mut read: impl FnMut(U256) -> Result<U256, E>,
) -> Result<u64, E> {
    if read(entry_slot)? != U256::from(1) {
        return Ok(0);
    }
    let root_data = U256::from_be_bytes(keccak256(entry_slot.abi_encode()).0);
    let mut words = 0;
    for slot in [
        entry_slot.wrapping_add(U256::from(1)),
        entry_slot.wrapping_add(U256::from(4)),
        entry_slot.wrapping_add(U256::from(5)),
        root_data,
        root_data.wrapping_add(U256::from(1)),
        root_data.wrapping_add(U256::from(2)),
    ] {
        words += u64::from(!read(slot)?.is_zero());
    }
    Ok(words * RETAINED_WORD_DISCOUNT)
}

/// Solidity 0.8.34 stores each mapping value at `keccak256(index, mappingSlot)`.
/// A prior empty or one-update/no-call entry with at most two result data words
/// has bounded overwrite work. The active queue length is not evidence here:
/// discarded mapping entries remain allocated for later replacement.
fn bounded_old_slot<E>(
    entry_slot: U256,
    mut read: impl FnMut(U256) -> Result<U256, E>,
) -> Result<bool, E> {
    let roots = read(entry_slot)?;
    if roots.is_zero() {
        for offset in 1..7 {
            if !read(entry_slot.wrapping_add(U256::from(offset)))?.is_zero() {
                return Ok(false);
            }
        }
        return Ok(true);
    }
    if roots != U256::from(1)
        || !read(entry_slot.wrapping_add(U256::from(2)))?.is_zero()
        || !read(entry_slot.wrapping_add(U256::from(3)))?.is_zero()
    {
        return Ok(false);
    }
    let data = read(entry_slot.wrapping_add(U256::from(6)))?;
    let low = data.byte(0);
    Ok((low & 1 == 0 && low <= 62)
        || (low & 1 == 1 && data >= U256::from(65) && data <= U256::from(129)))
}

#[cfg(test)]
mod tests {
    use super::super::PostBatchGasBudget;
    use super::*;
    use alloy_primitives::Bytes;
    use eez_protocol::abi::{
        EtherDeltaSol, L2ToL1CallSol, RollupIdWithProofSystemsSol, RootUpdateSol,
    };

    fn model(cleanup: u64) -> Projection {
        Projection {
            rollup_id: 1,
            cleanup_gas: cleanup,
            immediate_calls: Vec::new(),
            deferred_slots: vec![false; MAX_SHAPED_ENTRIES as usize],
            deferred_discounts: vec![0; MAX_SHAPED_ENTRIES as usize],
            deferred_storage: (0..MAX_SHAPED_ENTRIES).map(|_| None).collect(),
            deferred_count: 0,
            deferred_hashes: Vec::new(),
            parent_proxies: Default::default(),
        }
    }

    fn deferred() -> ExecutionEntrySol {
        ExecutionEntrySol {
            rootUpdates: vec![RootUpdateSol {
                rollupId: 1,
                etherDelta: EtherDeltaSol::ZERO,
                currentRoot: B256::repeat_byte(1),
                newRoot: B256::repeat_byte(2),
            }],
            proxyEntryHash: B256::repeat_byte(3),
            destinationRollupId: 1,
            success: true,
            returnData: Bytes::from(vec![0xff; MAX_SIMPLE_RESULT_BYTES]),
            ..Default::default()
        }
    }

    fn immediate() -> (ExecutionEntrySol, ExecutedAction) {
        let action = ExecutedAction {
            parent_index: None,
            call_mode: CallMode::Mutable,
            target_address: Address::repeat_byte(2),
            target_rollup_id: RollupId::MAINNET,
            source_rollup_id: RollupId(1),
            source_address: Address::repeat_byte(1),
            data: Bytes::from(vec![0xff; 256]),
            value: U256::ZERO,
            outcome: ExecutionOutcome::Resolved {
                return_data: vec![0xff; MAX_SIMPLE_RESULT_BYTES],
                gas_used: 100_000,
                success: true,
            },
            revert_spans: vec![],
        };
        let entry = ExecutionEntrySol {
            proxyEntryHash: B256::ZERO,
            returnData: Bytes::new(),
            l2ToL1Calls: vec![L2ToL1CallSol {
                revertNextNCalls: 0,
                isStatic: false,
                gas: 0,
                sourceAddress: action.source_address,
                sourceRollupId: 1,
                targetAddress: action.target_address,
                value: U256::ZERO,
                data: action.data.clone(),
            }],
            ..deferred()
        };
        (entry, action)
    }

    fn anchor() -> ExecutionEntrySol {
        ExecutionEntrySol {
            proxyEntryHash: B256::ZERO,
            returnData: Bytes::new(),
            ..deferred()
        }
    }

    fn batch(entries: Vec<ExecutionEntrySol>, immediates: usize) -> eez_protocol::EvmBatch {
        eez_protocol::EvmBatch {
            expectedRootPerRollup: vec![eez_protocol::abi::ExpectedRootPerRollupSol {
                rollupId: 1,
                root: B256::repeat_byte(1),
            }],
            blockNumber: u64::MAX,
            entries,
            immediateEntryCount: U256::from(immediates),
            proofSystems: vec![Address::repeat_byte(4)],
            rollupIdsWithProofSystems: vec![RollupIdWithProofSystemsSol {
                rollupId: 1,
                proofSystemIndexes: vec![0],
            }],
            ..Default::default()
        }
    }

    #[test]
    fn offline_root_pin_preserves_incremental_and_final_gas_agreement() {
        let mut projection = model(0);
        projection.deferred_slots[0] = true;
        let entry = deferred();
        let cost = projection.cost(std::slice::from_ref(&entry), 0, false, None);
        assert!(cost.entry_discount > 0);
        let mut budget = PostBatchGasBudget::new(10_000_000);
        budget.use_projection(projection.initial_gas());
        assert!(budget.try_accept(cost));
        projection.accept_deferred(std::slice::from_ref(&entry));
        let mut final_batch = batch(vec![anchor(), entry], 1);
        final_batch.expectedRootPerRollup = vec![eez_protocol::abi::ExpectedRootPerRollupSol {
            rollupId: 1,
            root: B256::repeat_byte(1),
        }];
        assert_eq!(
            projection.batch_gas(&final_batch, cost.calldata_gas, 0),
            budget.projected,
            "the mandatory offline root pin must not discard admitted entry evidence"
        );
    }

    #[test]
    fn deferred_reserves_do_not_depend_on_result_size() {
        let p = model(0);
        for length in [0, 1, 30, 31, 32, 33, 63, 64, 65] {
            let mut entry = deferred();
            entry.returnData = Bytes::from(vec![0xff; length]);
            assert_eq!(p.entry_gas(&entry, false, None), POSTBATCH_ENTRY_GAS_PIN);
            // Return size alone never grants immediate or failed-entry savings.
            assert_eq!(p.entry_gas(&entry, true, None), POSTBATCH_ENTRY_GAS_PIN);
            entry.success = false;
            assert_eq!(p.entry_gas(&entry, false, None), POSTBATCH_ENTRY_GAS_PIN);
        }
    }

    #[test]
    fn bounded_mapping_slots_enable_size_discount_at_the_correct_index() {
        let mut p = model(0);
        p.deferred_slots[0] = true;
        p.deferred_slots[2] = true;
        let mut short = deferred();
        short.returnData = Bytes::from(vec![0xff; 31]);
        let mut word = deferred();
        word.returnData = Bytes::from(vec![0xff; 32]);
        assert_eq!(
            p.cost(std::slice::from_ref(&short), 0, false, None)
                .entry_discount,
            140_000
        );
        p.accept_deferred(std::slice::from_ref(&short));
        assert_eq!(
            p.cost(std::slice::from_ref(&word), 0, false, None)
                .entry_discount,
            0
        );
        p.accept_deferred(std::slice::from_ref(&short));
        assert_eq!(
            p.cost(std::slice::from_ref(&word), 0, false, None)
                .entry_discount,
            120_000
        );
        let b = batch(vec![anchor(), short, word.clone(), word], 1);
        assert_eq!(
            p.batch_gas(&b, 0, 0),
            p.initial_gas() + 230_000 + 370_000 + 250_000
        );
        let changes: [fn(&mut ExecutionEntrySol); 7] = [
            |e: &mut ExecutionEntrySol| e.returnData = Bytes::from(vec![0xff; 65]),
            |e| e.success = false,
            |e| e.destinationRollupId = 2,
            |e| e.proxyEntryHash = B256::ZERO,
            |e| e.rootUpdates[0].rollupId = 2,
            |e| e.rootUpdates.push(e.rootUpdates[0].clone()),
            |e| e.l2ToL1Calls.push(immediate().0.l2ToL1Calls[0].clone()),
        ];
        for change in changes {
            let mut unsupported = deferred();
            change(&mut unsupported);
            assert_eq!(
                p.deferred_entry_gas(&unsupported, 0),
                POSTBATCH_ENTRY_GAS_PIN
            );
        }
    }

    #[test]
    fn retained_fixed_words_discount_only_the_exact_slot_and_keep_shape_fallbacks() {
        let slot = U256::from(101);
        let root = U256::from_be_bytes(keccak256(slot.abi_encode()).0);
        for mask in 0u8..64 {
            let slots = [
                slot + U256::from(1),
                slot + U256::from(4),
                slot + U256::from(5),
                root,
                root.wrapping_add(U256::from(1)),
                root.wrapping_add(U256::from(2)),
            ];
            let discount = retained_word_discount(slot, |key| {
                Ok::<_, ()>(
                    if key == slot
                        || slots
                            .iter()
                            .enumerate()
                            .any(|(i, s)| *s == key && mask & (1 << i) != 0)
                    {
                        U256::from(1)
                    } else {
                        U256::ZERO
                    },
                )
            })
            .unwrap();
            assert_eq!(
                discount,
                u64::from(mask.count_ones()) * RETAINED_WORD_DISCOUNT
            );
        }
        let mut projection = model(0);
        projection.deferred_slots[0] = true;
        projection.deferred_discounts[0] = 6 * RETAINED_WORD_DISCOUNT;
        let mut entry = deferred();
        assert_eq!(projection.deferred_entry_gas(&entry, 0), 180_000);
        assert_eq!(
            projection.deferred_entry_gas(&entry, 1),
            POSTBATCH_ENTRY_GAS_PIN
        );
        entry.returnData = Bytes::from(vec![0xff; 65]);
        assert_eq!(
            projection.deferred_entry_gas(&entry, 0),
            POSTBATCH_ENTRY_GAS_PIN
        );
    }

    #[test]
    fn old_mapping_slot_must_be_empty_or_bounded_before_discount() {
        let slot = U256::from(101);
        let mut values = std::collections::HashMap::<U256, U256>::new();
        let classify = |values: &std::collections::HashMap<U256, U256>| {
            bounded_old_slot(slot, |key| {
                Ok::<_, ()>(*values.get(&key).unwrap_or(&U256::ZERO))
            })
            .unwrap()
        };
        assert!(classify(&values));
        values.insert(slot + U256::from(1), U256::from(1));
        assert!(
            !classify(&values),
            "a nonempty zero-update slot is not fresh"
        );
        values.clear();
        values.insert(slot, U256::from(1));
        values.insert(slot + U256::from(6), U256::from(129));
        assert!(
            classify(&values),
            "one update and a 64-byte result are bounded"
        );
        for (offset, value) in [(0, 2), (2, 1), (3, 1), (6, 131)] {
            let key = slot + U256::from(offset);
            let original = values.insert(key, U256::from(value));
            assert!(
                !classify(&values),
                "unsupported prior shape at offset {offset}"
            );
            if let Some(original) = original {
                values.insert(key, original);
            } else {
                values.remove(&key);
            }
        }
    }

    #[test]
    fn mixed_result_reserves_match_final_sizing() {
        let p = model(0);
        let entries: Vec<_> = [31, 32, 64]
            .into_iter()
            .map(|length| {
                let mut entry = deferred();
                entry.returnData = Bytes::from(vec![0xff; length]);
                entry
            })
            .collect();
        let cost = p.cost(&entries, 0, false, None);
        assert_eq!(cost.entry_discount, 0);
        let cap = p.initial_gas() + 2 * (cost.total() + cost.framing_gas) + framing::base(64);
        let mut budget = PostBatchGasBudget::new(cap);
        budget.use_projection(p.initial_gas());
        assert!(budget.try_accept(cost));
        assert!(budget.try_accept(cost));
        let before = budget.projected;
        assert!(!budget.try_accept(cost));
        assert_eq!(budget.projected, before, "refused group spends no gas");
        let final_batch = batch(
            std::iter::once(anchor())
                .chain(entries.iter().cloned().cycle().take(6))
                .collect(),
            1,
        );
        assert_eq!(
            budget.projected,
            p.batch_gas(&final_batch, 2 * cost.calldata_gas, 0)
        );
    }

    #[test]
    fn immediate_discount_needs_exact_successful_bounded_leaf_and_parent_proxy_evidence() {
        let p = model(0);
        let (entry, action) = immediate();
        let key = Some(keccak256(entry.l2ToL1Calls[0].abi_encode()));
        assert!(p.simple_immediate_action(&entry, &action));
        assert_eq!(p.entry_gas(&entry, true, None), POSTBATCH_ENTRY_GAS_PIN);
        assert_eq!(p.entry_gas(&entry, true, key), EXISTING_PROXY_ENTRY_GAS);
        let changes: [fn(&mut ExecutedAction); 8] = [
            |a| a.parent_index = Some(0),
            |a| a.revert_spans = vec![1],
            |a| a.target_rollup_id = RollupId(1),
            |a| a.call_mode = CallMode::Static,
            |a| a.source_address = Address::ZERO,
            |a| a.data = Bytes::from(vec![0xff; 257]),
            |a| {
                a.outcome = ExecutionOutcome::Resolved {
                    success: false,
                    return_data: vec![],
                    gas_used: 1,
                }
            },
            |a| {
                a.outcome = ExecutionOutcome::Resolved {
                    success: true,
                    return_data: vec![0xff; MAX_SIMPLE_RESULT_BYTES + 1],
                    gas_used: 1,
                }
            },
        ];
        for change in changes {
            let mut other = action.clone();
            change(&mut other);
            assert!(!p.simple_immediate_action(&entry, &other));
        }
        let changes: [fn(&mut ExecutionEntrySol); 7] = [
            |e| e.l2ToL1Calls[0].revertNextNCalls = 1,
            |e| e.l2ToL1Calls[0].gas = 1,
            |e| e.l2ToL1Calls[0].isStatic = true,
            |e| e.l2ToL1Calls[0].value = U256::from(1),
            |e| e.returnData = Bytes::from(vec![1]),
            |e| e.rootUpdates[0].etherDelta = EtherDeltaSol::from(alloy_primitives::I256::ONE),
            |e| e.l2ToL1Calls.push(e.l2ToL1Calls[0].clone()),
        ];
        for change in changes {
            let mut other = entry.clone();
            change(&mut other);
            assert_eq!(p.entry_gas(&other, true, key), POSTBATCH_ENTRY_GAS_PIN);
        }
    }

    #[test]
    fn admission_and_final_projection_share_reserves_and_preserve_occurrence_evidence() {
        let mut p = model(0);
        let (entry, _) = immediate();
        let key = Some(keccak256(entry.l2ToL1Calls[0].abi_encode()));
        let mut budget = PostBatchGasBudget::new(16_777_216);
        budget.use_projection(p.initial_gas());
        let a = p.cost(std::slice::from_ref(&entry), 10_000, true, key);
        assert!(budget.try_accept(a));
        p.accept_immediate(1, key);
        // Identical call bytes with an unsupported second outcome stay dear.
        let b = p.cost(std::slice::from_ref(&entry), 20_000, true, None);
        assert!(budget.try_accept(b));
        p.accept_immediate(1, None);
        let c = p.cost(&[deferred()], 0, false, None);
        assert!(budget.try_accept(c));
        let final_batch = batch(vec![anchor(), entry.clone(), entry, deferred()], 3);
        assert_eq!(
            budget.projected,
            p.batch_gas(
                &final_batch,
                a.calldata_gas + b.calldata_gas + c.calldata_gas,
                30_000
            )
        );
        assert_eq!(p.initial_gas(), 21_000 + POSTBATCH_BASE_GAS_PIN);
        assert_eq!(
            p.batch_gas(&batch(vec![anchor()], 1), 0, 0),
            p.initial_gas()
        );
    }

    #[test]
    fn bounded_nested_evidence_binds_calls_results_and_occurrences_through_root_stitching() {
        use eez_protocol::abi::ExpectedL1ToL2CallSol;
        let mut p = model(0);
        let (mut entry, root) = immediate();
        let mut reentry = root.clone();
        reentry.parent_index = Some(0);
        reentry.source_rollup_id = RollupId::MAINNET;
        reentry.target_rollup_id = RollupId(1);
        let mut callback = root.clone();
        callback.parent_index = Some(1);
        entry.expectedL1ToL2Calls.push(ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: B256::repeat_byte(7),
            l2ToL1Calls: vec![entry.l2ToL1Calls[0].clone()],
            success: true,
            returnData: Bytes::from(vec![0xff; 64]),
            revertedOrStaticRollingHash: B256::ZERO,
        });
        let actions = vec![root, reentry, callback];
        assert!(p.bounded_immediate_actions(&entry, &actions));
        let key = Some(Projection::immediate_shape_key(&entry));
        assert_eq!(p.entry_gas(&entry, true, None), POSTBATCH_ENTRY_GAS_PIN);
        assert_eq!(p.entry_gas(&entry, true, key), 145_000);
        let cost = p.cost(std::slice::from_ref(&entry), 50_000, true, key);
        let mut budget = PostBatchGasBudget::new(16_777_216);
        budget.use_projection(p.initial_gas());
        assert!(budget.try_accept(cost));
        p.accept_immediate(1, key);
        entry.rootUpdates[0].currentRoot = B256::repeat_byte(8);
        entry.rootUpdates[0].newRoot = B256::repeat_byte(9);
        entry.rollingHash = B256::repeat_byte(10);
        entry.expectedL1ToL2Calls[0].expectedL1toL2Hash = B256::repeat_byte(11);
        assert_eq!(p.entry_gas(&entry, true, key), 145_000);
        assert_eq!(
            budget.projected,
            p.batch_gas(
                &batch(vec![anchor(), entry.clone()], 2),
                cost.calldata_gas,
                50_000
            )
        );
        for change in [
            |e: &mut ExecutionEntrySol| {
                e.expectedL1ToL2Calls[0].returnData = Bytes::from(vec![1; 64])
            },
            |e: &mut ExecutionEntrySol| {
                e.expectedL1ToL2Calls[0].l2ToL1Calls[0].targetAddress = Address::ZERO
            },
            |e: &mut ExecutionEntrySol| e.expectedL1ToL2Calls[0].success = false,
            |e: &mut ExecutionEntrySol| e.l2ToL1Calls[0].revertNextNCalls = 1,
        ] {
            let mut changed = entry.clone();
            change(&mut changed);
            assert_eq!(p.entry_gas(&changed, true, key), POSTBATCH_ENTRY_GAS_PIN);
        }
        for change in [
            |a: &mut Vec<ExecutedAction>| {
                a.pop();
            },
            |a: &mut Vec<ExecutedAction>| a[2].data = Bytes::new(),
            |a: &mut Vec<ExecutedAction>| a[1].parent_index = Some(2),
            |a: &mut Vec<ExecutedAction>| a[2].revert_spans = vec![1],
            |a: &mut Vec<ExecutedAction>| {
                a[2].outcome = ExecutionOutcome::Resolved {
                    success: false,
                    return_data: vec![],
                    gas_used: 1,
                }
            },
            |a: &mut Vec<ExecutedAction>| {
                a[2].outcome = ExecutionOutcome::Resolved {
                    success: true,
                    return_data: vec![0; 65],
                    gas_used: 1,
                }
            },
        ] {
            let mut changed = actions.clone();
            change(&mut changed);
            assert!(!p.bounded_immediate_actions(&entry, &changed));
        }
        for extra in 0..4 {
            let mut changed = entry.clone();
            changed
                .l2ToL1Calls
                .extend(std::iter::repeat_n(entry.l2ToL1Calls[0].clone(), extra));
            assert_eq!(p.bounded_immediate_gas(&changed).is_some(), extra <= 2);
        }
        for change in [
            |e: &mut ExecutionEntrySol| {
                e.expectedL1ToL2Calls[0].returnData = Bytes::from(vec![0; 65])
            },
            |e: &mut ExecutionEntrySol| {
                e.expectedL1ToL2Calls
                    .resize(5, e.expectedL1ToL2Calls[0].clone())
            },
            |e: &mut ExecutionEntrySol| {
                e.expectedL1ToL2Calls[0].l2ToL1Calls[0].data = Bytes::from(vec![0; 257])
            },
            |e: &mut ExecutionEntrySol| e.expectedL1ToL2Calls[0].l2ToL1Calls[0].gas = 1,
            |e: &mut ExecutionEntrySol| e.expectedL1ToL2Calls[0].l2ToL1Calls[0].isStatic = true,
            |e: &mut ExecutionEntrySol| e.expectedL1ToL2Calls[0].l2ToL1Calls[0].sourceRollupId = 2,
        ] {
            let mut changed = entry.clone();
            change(&mut changed);
            assert!(p.bounded_immediate_gas(&changed).is_none());
        }
    }

    #[test]
    fn complex_pin_literals_match_the_measured_contract_fixture() {
        let sol = include_str!("../../../../contracts/test/PostBatchGasPins.t.sol");
        for (name, gas) in [
            ("EXTRA_IMMEDIATE_CALL_GAS", EXTRA_IMMEDIATE_CALL_GAS),
            ("IMMEDIATE_RESULT_ROW_GAS", IMMEDIATE_RESULT_ROW_GAS),
        ] {
            assert!(sol.contains(&format!(
                "uint256 private constant {name} = {}_000;",
                gas / 1000
            )));
        }
    }

    #[test]
    fn exceeding_measured_size_preserves_full_reserves_and_rejection_is_atomic() {
        let p = model(0);
        let cost = p.cost(&[deferred()], 0, false, None);
        let mut budget = PostBatchGasBudget::new(100_000_000);
        budget.use_projection(p.initial_gas());
        for _ in 0..127 {
            assert!(budget.try_accept(cost));
        }
        let final_128 = batch(
            std::iter::once(anchor())
                .chain(std::iter::repeat_n(deferred(), 127))
                .collect(),
            1,
        );
        assert_eq!(
            budget.projected,
            p.batch_gas(&final_128, 127 * cost.calldata_gas, 0)
        );
        budget.cap = budget.projected + cost.total() - 1;
        let before = budget;
        assert!(!budget.try_accept(cost));
        assert_eq!(budget.projected, before.projected);
        assert_eq!(budget.entry_discount, before.entry_discount);
        budget.cap = 100_000_000;
        assert!(budget.try_accept(cost));
        let final_129 = batch(
            std::iter::once(anchor())
                .chain(std::iter::repeat_n(deferred(), 128))
                .collect(),
            1,
        );
        assert_eq!(budget.entry_discount, 0);
        assert_eq!(
            budget.projected,
            p.batch_gas(&final_129, 128 * cost.calldata_gas, 0)
        );
        budget.cap = u64::MAX;
        assert!(!budget.try_accept(TxL1Gas {
            target_gas: u64::MAX,
            ..cost
        }));
    }

    #[test]
    fn deferred_work_keeps_full_reserve_under_the_transaction_cap() {
        let p = model(0);
        let cap = eez_l1::posting_fees::PostingFeeConfig::parse(|name| {
            Ok(matches!(
                name,
                "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS"
                    | "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS"
                    | "EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE"
            )
            .then(|| "500000000000".into()))
        })
        .unwrap()
        .estimate_budget(16_777_216);
        let mut budget = PostBatchGasBudget::new(cap);
        budget.use_projection(p.initial_gas());
        for _ in 0..30 {
            assert!(budget.try_accept(p.cost(&[deferred()], 0, false, None)));
        }
        assert!(budget.full_estimate() <= cap);
    }

    #[test]
    fn unsupported_batch_family_keeps_conservative_reserves() {
        let p = model(0);
        let b = batch(vec![anchor(), deferred()], 1);
        let conservative = projected_postbatch_gas(2, 100, 200);
        let changes: [fn(&mut eez_protocol::EvmBatch); 11] = [
            |b| b.blobIndices = vec![U256::ZERO; 65],
            |b| b.proofSystems.push(Address::ZERO),
            |b| b.staticEntries.push(Default::default()),
            |b| b.immediateStaticEntryCount = U256::from(1),
            |b| b.entries[0].success = false,
            |b| b.entries[0].rootUpdates.clear(),
            |b| b.immediateEntryCount = U256::ZERO,
            |b| b.expectedRootPerRollup.clear(),
            |b| b.expectedRootPerRollup[0].rollupId = 2,
            |b| b.expectedRootPerRollup[0].root = B256::ZERO,
            |b| {
                b.expectedRootPerRollup
                    .push(b.expectedRootPerRollup[0].clone())
            },
        ];
        for change in changes {
            let mut other = b.clone();
            change(&mut other);
            assert_eq!(p.batch_gas(&other, 100, 200), conservative);
        }
    }

    #[test]
    fn reserves_match_executable_solidity_bounds() {
        let contract = include_str!("../../../../contracts/test/PostBatchGasPins.t.sol");
        let prefix = "uint256 private constant EXISTING_PROXY_GAS_PIN = ";
        let value = contract
            .lines()
            .find_map(|line| line.trim().strip_prefix(prefix))
            .expect("contract gas assertion constant")
            .trim_end_matches(';')
            .replace('_', "")
            .parse::<u64>()
            .unwrap();
        assert_eq!(value, EXISTING_PROXY_ENTRY_GAS);
    }

    #[test]
    fn deferred_storage_pricing_survives_stitching_and_rejected_groups() {
        let mut p = model(0);
        p.deferred_storage = (0..MAX_SHAPED_ENTRIES)
            .map(|i| {
                deferred::Slot::read(U256::from(i * 100), |_| Ok::<_, ()>(U256::ZERO)).unwrap()
            })
            .collect();
        let mut e = immediate().0;
        e.proxyEntryHash = B256::repeat_byte(7);
        e.l2ToL1Calls[0].data = Bytes::from(vec![1; 4]);
        let mut unstitched = e.clone();
        unstitched.rootUpdates.clear();
        let cost = p.cost(&[unstitched.clone(), unstitched], 0, false, None);
        assert_eq!(cost.estimate_reasons.json()["deferred_storage"], 2);
        let mut budget = PostBatchGasBudget::new(100_000_000);
        budget.use_projection(p.initial_gas());
        assert!(budget.try_accept(cost));
        p.accept_deferred(&[e.clone(), e.clone()]);
        let final_batch = batch(vec![anchor(), e.clone(), e.clone()], 1);
        assert_eq!(
            budget.projected,
            p.batch_gas(&final_batch, cost.calldata_gas, 0)
        );
        let prior = budget;
        budget.cap = budget.projected;
        assert!(!budget.try_accept(p.cost(&[e], 0, false, None)));
        assert_eq!(budget.projected, prior.projected);
        assert_eq!(p.deferred_count, 2);
    }

    #[test]
    fn companion_protocol_reserve_tracks_reverted_prefix_and_declared_cap() {
        let mut p = model(0);
        let mut e = deferred();
        e.returnData = Bytes::from(vec![0xa5; 100]);
        let one = std::slice::from_ref(&e);
        let reserve = p.companion_reserve(one).unwrap();
        assert_eq!(reserve, 79_200);
        let estimated = CompanionEstimate::bounded(40_000, 0, reserve, 200_000);
        assert_eq!(estimated.gas, 119_200);
        assert_eq!(estimated.basis, "bounded_protocol");
        let capped = CompanionEstimate::bounded(40_000, 0, reserve, 100_000);
        assert_eq!(capped.gas, 100_000);
        assert_eq!(capped.basis, "bounded_capped");
        let mut budget = PostBatchGasBudget::new(10_000_000);
        assert!(budget.try_accept_with_companion(p.cost(one, 0, false, None), 100_000, capped.gas));
        // Reverted metadata does not authorize assuming a cursor advance.
        e.success = false;
        p.accept_deferred(std::slice::from_ref(&e));
        assert_eq!(
            p.companion_reserve(std::slice::from_ref(&e)),
            Some(reserve + 12_500)
        );
        e.proxyEntryHash = B256::repeat_byte(99);
        assert_eq!(
            p.companion_reserve(std::slice::from_ref(&e)),
            Some(reserve + 2_600)
        );
        assert_eq!(
            p.companion_reserve(&[e.clone(), e.clone()]),
            Some(2 * (reserve + 2_600) + 12_500)
        );
        assert!(p.companion_reserve(&vec![e.clone(); 5]).is_none());
        e.returnData = Bytes::from(vec![0; 257]);
        assert!(p.companion_reserve(std::slice::from_ref(&e)).is_none());
        let fallback = CompanionEstimate::declared(500_000);
        assert_eq!(fallback.gas, 500_000);
        assert_eq!(fallback.protocol_reserve, None);
    }

    #[test]
    fn companion_evidence_rejects_unmeasured_results_and_nested_execution() {
        let p = model(0);
        let (mut entry, mut callback) = immediate();
        entry.proxyEntryHash = B256::repeat_byte(8);
        callback.parent_index = Some(0);
        let mut root = callback.clone();
        root.parent_index = None;
        root.source_rollup_id = RollupId::MAINNET;
        root.target_rollup_id = RollupId(1);
        root.outcome = ExecutionOutcome::Resolved {
            success: true,
            return_data: Vec::new(),
            gas_used: 1,
        };
        let actions = [root, callback];
        assert_eq!(
            p.companion_action_discount(std::slice::from_ref(&entry), &actions),
            Some(0)
        );
        for change in 0..4 {
            let mut invalid = actions.clone();
            match change {
                0 => {
                    invalid[1].outcome = ExecutionOutcome::Resolved {
                        success: true,
                        return_data: vec![0; 65],
                        gas_used: 1,
                    }
                }
                1 => invalid[1].outcome = ExecutionOutcome::Pending,
                2 => invalid[1].parent_index = Some(1),
                _ => invalid[0].call_mode = CallMode::Static,
            }
            assert!(
                p.companion_action_discount(std::slice::from_ref(&entry), &invalid)
                    .is_none()
            );
        }
        assert!(
            p.companion_action_discount(std::slice::from_ref(&entry), &actions[..1])
                .is_none()
        );
    }

    #[test]
    fn companion_execution_charges_callbacks_and_caps_overflow_without_rejection() {
        let estimate = CompanionEstimate::bounded(40_000, 1_000_000, 90_000, 2_000_000);
        assert_eq!(estimate.gas, 1_130_000);
        assert_eq!(estimate.basis, "bounded_protocol");
        assert_eq!(
            CompanionEstimate::bounded(u64::MAX, 1, 1, 500_000).gas,
            500_000
        );
    }

    #[test]
    fn companion_group_evidence_bounds_sequential_nested_and_reverted_actions() {
        use eez_protocol::abi::ExpectedL1ToL2CallSol;
        let mut p = model(0);
        let (mut entry, mut callback) = immediate();
        entry.proxyEntryHash = B256::repeat_byte(8);
        entry.l2ToL1Calls[0].revertNextNCalls = 1;
        callback.parent_index = Some(0);
        callback.revert_spans = vec![1];
        let mut root = callback.clone();
        root.parent_index = None;
        root.revert_spans = vec![];
        root.source_rollup_id = RollupId::MAINNET;
        root.target_rollup_id = RollupId(1);
        root.outcome = ExecutionOutcome::Resolved {
            success: true,
            return_data: Vec::new(),
            gas_used: 1,
        };
        let mut nested = root.clone();
        nested.parent_index = Some(1);
        nested.data = Bytes::new();
        nested.outcome = ExecutionOutcome::Resolved {
            success: true,
            return_data: vec![0xff; 64],
            gas_used: 1,
        };
        let mut leaf = callback.clone();
        leaf.parent_index = Some(2);
        leaf.revert_spans = vec![];
        let mut call = entry.l2ToL1Calls[0].clone();
        call.revertNextNCalls = 0;
        entry.expectedL1ToL2Calls.push(ExpectedL1ToL2CallSol {
            expectedL1toL2Hash: B256::repeat_byte(7),
            l2ToL1Calls: vec![call],
            success: true,
            returnData: Bytes::from(vec![0xff; 64]),
            revertedOrStaticRollingHash: B256::ZERO,
        });
        let actions = vec![root, callback, nested, leaf];
        let one = std::slice::from_ref(&entry);
        assert_eq!(p.companion_action_discount(one, &actions), Some(9_200));
        let reserve = p.companion_reserve(one).unwrap();
        for bytes in [0usize, 1, 31, 32, 33, 64, 100, 128, 129, 256] {
            let mut varied = actions.clone();
            varied[2].data = vec![0; bytes].into();
            assert_eq!(
                p.companion_action_discount(one, &varied),
                Some(2_300 * (8 - bytes.div_ceil(32).max(4)) as u64)
            );
        }
        let mut oversized = actions.clone();
        oversized[2].data = vec![0; 257].into();
        assert!(p.companion_action_discount(one, &oversized).is_none());
        for roots in 1..=4 {
            let entries = vec![entry.clone(); roots];
            let forest: Vec<_> = (0..roots)
                .flat_map(|index| {
                    actions.iter().cloned().map(move |mut action| {
                        action.parent_index = action.parent_index.map(|parent| parent + index * 4);
                        action
                    })
                })
                .collect();
            assert_eq!(
                p.companion_action_discount(&entries, &forest),
                Some(9_200 * roots as u64)
            );
            assert_eq!(
                p.companion_reserve(&entries),
                Some(reserve * roots as u64 + 12_500 * (roots * (roots - 1) / 2) as u64)
            );
        }
        for change in 0..6 {
            let mut invalid = actions.clone();
            match change {
                0 => invalid[3].parent_index = Some(3),
                1 => invalid[2].target_rollup_id = RollupId(2),
                2 => invalid[1].revert_spans = vec![],
                3 => invalid[2].outcome = ExecutionOutcome::Pending,
                4 => invalid[3].data = vec![1; 257].into(),
                _ => invalid[3].call_mode = CallMode::Static,
            }
            assert!(
                p.companion_action_discount(one, &invalid).is_none(),
                "change {change}"
            );
        }
        p.accept_deferred(&vec![entry.clone(); 127]);
        assert!(p.companion_reserve(one).is_some());
        assert!(
            p.companion_reserve(&[entry.clone(), entry.clone()])
                .is_none()
        );
        p.accept_deferred(one);
        assert!(p.companion_reserve(one).is_none());
    }

    #[test]
    fn deferred_storage_constants_match_executable_measurements() {
        let contract = include_str!("../../../../contracts/test/PostBatchGasPins.t.sol");
        for (name, value) in [
            ("DEFERRED_STORAGE_ENTRY_GAS", deferred::ENTRY_GAS),
            ("DEFERRED_STORAGE_WORD_GAS", deferred::WORD_GAS),
            ("DEFERRED_STORAGE_FRESH_WORD_GAS", deferred::FRESH_WORD_GAS),
            ("DEFERRED_STORAGE_ITEM_GAS", deferred::ITEM_GAS),
        ] {
            let line = format!("uint256 private constant {name} = ");
            let actual = contract
                .lines()
                .find_map(|l| l.trim().strip_prefix(&line))
                .unwrap()
                .trim_end_matches(';')
                .replace('_', "")
                .parse::<u64>()
                .unwrap();
            assert_eq!(actual, value, "{name}");
        }
    }

    #[test]
    fn temporary_cleanup_pressure_does_not_label_a_fitting_source_unpostable() {
        let p = model(1_300_000);
        let cost = p.cost(&[deferred()], 0, false, None);
        let mut budget = PostBatchGasBudget::new(1_700_000);
        budget.use_projection(p.initial_gas());
        assert!(!budget.try_accept_with_companion(cost, 200_000, 100_000));
        assert!(budget.blocked_by_cleanup(cost, 200_000, 100_000, p.cleanup_gas));
        assert!(!budget.blocked_by_cleanup(cost, 200_000, 100_000, 0));
        assert!(!budget.blocked_by_cleanup(
            TxL1Gas {
                target_gas: 5_000_000,
                ..cost
            },
            0,
            0,
            p.cleanup_gas
        ));
        budget.transaction_cap = 100_000;
        assert!(!budget.blocked_by_cleanup(cost, 200_000, 100_000, p.cleanup_gas));
    }
}
