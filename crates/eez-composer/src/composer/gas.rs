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

// Caller-frame gas (before refunds), pinned on Cancun/Prague/Osaka by
// contracts/test/PostBatchGasPins.t.sol. Intrinsic/calldata/target gas and the
// operator's percentage margin are separate. Unknown shapes keep the old pin.
const DEFERRED_ENTRY_GAS: u64 = 270_000;
const SHORT_DEFERRED_ENTRY_GAS: u64 = 230_000;
const WORD_DEFERRED_ENTRY_GAS: u64 = 250_000;
const EXISTING_PROXY_ENTRY_GAS: u64 = 70_000;
const MAX_SIMPLE_RESULT_BYTES: usize = 64;
pub(super) const MAX_SHAPED_ENTRIES: u64 = 128;

/// Evidence belongs to one pinned canonical parent, never a speculative proxy
/// creation earlier in the drain. Accepted outbound positions retain their
/// own eligibility, so identical calls with different outcomes cannot share
/// a discount. Root stitching may change roots/hashes, but not the call ABI.
pub(super) struct Projection {
    rollup_id: u64,
    pub(super) cleanup_gas: u64,
    immediate_calls: Vec<Option<B256>>,
    deferred_slots: Vec<bool>,
    deferred_count: usize,
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
        for index in 0..MAX_SHAPED_ENTRIES {
            let entry_slot =
                U256::from_be_bytes(keccak256((U256::from(index), queue_head).abi_encode()).0);
            let bounded = bounded_old_slot(entry_slot, |slot| {
                state
                    .storage(registry, B256::from(slot))
                    .map(Option::unwrap_or_default)
                    .map_err(|e| e.to_string())
            })?;
            deferred_slots.push(bounded);
        }
        Ok(Some(Self {
            rollup_id,
            cleanup_gas: 0,
            immediate_calls: Vec::new(),
            deferred_slots,
            deferred_count: 0,
            parent_proxies: Default::default(),
        }))
    }

    pub fn initial_gas(&self) -> u64 {
        21_000 + POSTBATCH_BASE_GAS_PIN + self.cleanup_gas
    }

    /// Only a single successful, non-reentrant L1 action with bounded input
    /// and output can use the established-proxy pin. In particular, entry
    /// success does not imply that the underlying target call succeeded.
    pub fn simple_immediate(
        &mut self,
        entries: &[ExecutionEntrySol],
        actions: &[ExecutedAction],
        parent: &mut L1TargetSession,
    ) -> Result<Option<B256>, String> {
        let ([entry], [action]) = (entries, actions) else {
            return Ok(None);
        };
        if !self.simple_immediate_action(entry, action) {
            return Ok(None);
        }
        let call = &entry.l2ToL1Calls[0];
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
        Ok(Some(keccak256(call.abi_encode())))
    }

    fn simple_immediate_action(&self, entry: &ExecutionEntrySol, action: &ExecutedAction) -> bool {
        if !self.simple_immediate_shape(entry) {
            return false;
        }
        let call = &entry.l2ToL1Calls[0];
        action.parent_index.is_none()
            && action.revert_span.is_none()
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
        POSTBATCH_ENTRY_GAS_PIN
    }

    fn deferred_entry_gas(&self, entry: &ExecutionEntrySol, index: usize) -> u64 {
        if self.deferred_slots.get(index) != Some(&true)
            || !self.common_shape(entry)
            || entry.proxyEntryHash.is_zero()
            || entry.destinationRollupId != self.rollup_id
            || !entry.l2ToL1Calls.is_empty()
        {
            return POSTBATCH_ENTRY_GAS_PIN;
        }
        match entry.returnData.len() {
            0..=31 => SHORT_DEFERRED_ENTRY_GAS,
            32 => WORD_DEFERRED_ENTRY_GAS,
            _ => DEFERRED_ENTRY_GAS,
        }
    }

    pub fn cost(
        &self,
        entries: &[ExecutionEntrySol],
        target_gas: u64,
        immediate: bool,
        established: Option<B256>,
    ) -> TxL1Gas {
        let mut cost = projected_tx_l1_gas(entries, target_gas);
        cost.entry_discount = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let priced = if immediate {
                    self.entry_gas(entry, true, established)
                } else {
                    self.deferred_entry_gas(entry, self.deferred_count + index)
                };
                POSTBATCH_ENTRY_GAS_PIN - priced
            })
            .fold(0, u64::saturating_add);
        cost
    }

    pub fn accept_immediate(&mut self, count: usize, established: Option<B256>) {
        self.immediate_calls
            .extend(std::iter::repeat_n(established, count));
    }

    pub fn accept_deferred(&mut self, count: usize) {
        self.deferred_count = self.deferred_count.saturating_add(count);
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
            || !batch.expectedRootPerRollup.is_empty()
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
    use super::super::{POSTBATCH_DRAIN_MARGIN, PostBatchGasBudget};
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
            deferred_count: 0,
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
            revert_span: None,
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
        p.accept_deferred(1);
        assert_eq!(
            p.cost(std::slice::from_ref(&word), 0, false, None)
                .entry_discount,
            0
        );
        p.accept_deferred(1);
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
        let cap = p.initial_gas() + 2 * cost.total() + POSTBATCH_DRAIN_MARGIN;
        let mut budget = PostBatchGasBudget::new(cap);
        budget.projected = p.initial_gas();
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
            |a| a.revert_span = Some(1),
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
        budget.projected = p.initial_gas();
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
    fn exceeding_measured_size_preserves_full_reserves_and_rejection_is_atomic() {
        let p = model(0);
        let cost = p.cost(&[deferred()], 0, false, None);
        let mut budget = PostBatchGasBudget::new(100_000_000);
        budget.projected = p.initial_gas();
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
        budget.projected = p.initial_gas();
        for _ in 0..30 {
            assert!(budget.try_accept(p.cost(&[deferred()], 0, false, None)));
        }
        assert!(budget.projected + POSTBATCH_DRAIN_MARGIN <= cap);
    }

    #[test]
    fn unsupported_batch_family_keeps_conservative_reserves() {
        let p = model(0);
        let b = batch(vec![anchor(), deferred()], 1);
        let conservative = projected_postbatch_gas(2, 100, 200);
        let changes: [fn(&mut eez_protocol::EvmBatch); 7] = [
            |b| b.blobIndices = vec![U256::ZERO; 65],
            |b| b.proofSystems.push(Address::ZERO),
            |b| b.staticEntries.push(Default::default()),
            |b| b.immediateStaticEntryCount = U256::from(1),
            |b| b.entries[0].success = false,
            |b| b.entries[0].rootUpdates.clear(),
            |b| b.immediateEntryCount = U256::ZERO,
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
    fn temporary_cleanup_pressure_does_not_label_a_fitting_source_unpostable() {
        let p = model(1_300_000);
        let cost = p.cost(&[deferred()], 0, false, None);
        let mut budget = PostBatchGasBudget::new(1_700_000);
        budget.projected = p.initial_gas();
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
