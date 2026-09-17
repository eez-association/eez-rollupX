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
const PREVIOUS_ENTRY_CLEANUP_GAS: u64 = 65_000;
const SHORT_ENTRY_CLEANUP_GAS: u64 = 55_000;
const WORD_ENTRY_CLEANUP_GAS: u64 = 60_000;
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
    parent_proxies: std::collections::HashMap<(u64, Address), bool>,
}

impl Projection {
    pub fn open(
        state: &dyn StateProvider,
        registry: Address,
        rollup_id: u64,
    ) -> Result<Option<Self>, String> {
        let cleanup = previous_cleanup(rollup_id, |key| {
            state
                .storage(registry, key)
                .map(Option::unwrap_or_default)
                .map_err(|e| e.to_string())
        })?;
        Ok(cleanup.map(|cleanup_gas| Self {
            rollup_id,
            cleanup_gas,
            immediate_calls: Vec::new(),
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
        if immediate {
            if self.simple_immediate_shape(entry)
                && established == Some(keccak256(entry.l2ToL1Calls[0].abi_encode()))
            {
                return EXISTING_PROXY_ENTRY_GAS;
            }
        } else if self.common_shape(entry)
            && !entry.proxyEntryHash.is_zero()
            && entry.destinationRollupId == self.rollup_id
            && entry.l2ToL1Calls.is_empty()
        {
            // Solidity stores up to 31 bytes in the length slot; 32 bytes
            // needs one external slot and 33..=64 needs two. Charge only the
            // measured size class, with the same shape gates and final check.
            return match entry.returnData.len() {
                0..=31 => SHORT_DEFERRED_ENTRY_GAS,
                32 => WORD_DEFERRED_ENTRY_GAS,
                _ => DEFERRED_ENTRY_GAS,
            };
        }
        POSTBATCH_ENTRY_GAS_PIN
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
            .map(|entry| POSTBATCH_ENTRY_GAS_PIN - self.entry_gas(entry, immediate, established))
            .fold(0, u64::saturating_add);
        cost
    }

    pub fn accept_immediate(&mut self, count: usize, established: Option<B256>) {
        self.immediate_calls
            .extend(std::iter::repeat_n(established, count));
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
                    self.entry_gas(
                        entry,
                        index < self.immediate_calls.len(),
                        self.immediate_calls.get(index).copied().flatten(),
                    )
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

/// Pinned EEZ storage layout, asserted against populated and consumed queues
/// in the Solidity gas tests. Only the bounded no-call queue shape is read;
/// static/nested/large queues disable discounts for this candidate. This is
/// deliberately independent of entryQueueIndex: consumption leaves storage.
fn previous_cleanup<E>(
    rollup_id: u64,
    mut read: impl FnMut(B256) -> Result<U256, E>,
) -> Result<Option<u64>, E> {
    let head =
        U256::from_be_bytes(keccak256((U256::from(rollup_id), U256::from(3)).abi_encode()).0);
    let normal = head.wrapping_add(U256::from(1));
    let length = read(B256::from(normal))?;
    if length >= U256::from(MAX_SHAPED_ENTRIES)
        || !read(B256::from(head.wrapping_add(U256::from(2))))?.is_zero()
    {
        return Ok(None);
    }
    let start = U256::from_be_bytes(keccak256(normal.to_be_bytes::<32>()).0);
    let count = length.to::<u64>();
    let mut cleanup_gas = 0u64;
    for index in 0..count {
        let entry = start.wrapping_add(U256::from(index * 7));
        if read(B256::from(entry))? > U256::from(1)
            || !read(B256::from(entry.wrapping_add(U256::from(2))))?.is_zero()
            || !read(B256::from(entry.wrapping_add(U256::from(3))))?.is_zero()
        {
            return Ok(None);
        }
        let data = read(B256::from(entry.wrapping_add(U256::from(6))))?;
        // Solidity short bytes put length*2 in the low byte. Long results in
        // the measured range (32..=64 bytes) occupy one or two extra slots.
        let low = data.byte(0);
        let short = low & 1 == 0 && low <= 62;
        let long = low & 1 == 1
            && data >= U256::from(65)
            && data <= U256::from(MAX_SIMPLE_RESULT_BYTES * 2 + 1);
        let entry_gas = if short {
            SHORT_ENTRY_CLEANUP_GAS
        } else if long {
            if data == U256::from(65) {
                WORD_ENTRY_CLEANUP_GAS
            } else {
                PREVIOUS_ENTRY_CLEANUP_GAS
            }
        } else {
            return Ok(None);
        };
        cleanup_gas = cleanup_gas.saturating_add(entry_gas);
    }
    Ok(Some(cleanup_gas))
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
    fn deferred_discount_has_explicit_shape_boundaries() {
        let p = model(0);
        let entry = deferred();
        assert_eq!(p.entry_gas(&entry, false, None), DEFERRED_ENTRY_GAS);
        let changes: [fn(&mut ExecutionEntrySol); 6] = [
            |e| e.returnData = Bytes::from(vec![0xff; MAX_SIMPLE_RESULT_BYTES + 1]),
            |e| e.success = false,
            |e| e.destinationRollupId = 2,
            |e| e.proxyEntryHash = B256::ZERO,
            |e| e.rootUpdates[0].rollupId = 2,
            |e| e.rootUpdates.push(e.rootUpdates[0].clone()),
        ];
        for change in changes {
            let mut other = entry.clone();
            change(&mut other);
            assert_eq!(p.entry_gas(&other, false, None), POSTBATCH_ENTRY_GAS_PIN);
        }
        let mut with_call = entry;
        with_call.l2ToL1Calls = immediate().0.l2ToL1Calls;
        assert_eq!(
            p.entry_gas(&with_call, false, None),
            POSTBATCH_ENTRY_GAS_PIN
        );
    }

    #[test]
    fn deferred_reserves_follow_storage_boundaries_without_expanding_eligibility() {
        let p = model(0);
        for (length, expected) in [
            (0, 230_000),
            (1, 230_000),
            (30, 230_000),
            (31, 230_000),
            (32, 250_000),
            (33, 270_000),
            (63, 270_000),
            (64, 270_000),
            (65, 370_000),
        ] {
            let mut entry = deferred();
            entry.returnData = Bytes::from(vec![0xff; length]);
            assert_eq!(p.entry_gas(&entry, false, None), expected);
            // Return size alone never grants immediate or failed-entry savings.
            assert_eq!(p.entry_gas(&entry, true, None), POSTBATCH_ENTRY_GAS_PIN);
            entry.success = false;
            assert_eq!(p.entry_gas(&entry, false, None), POSTBATCH_ENTRY_GAS_PIN);
        }
    }

    #[test]
    fn mixed_result_reserves_admit_a_larger_complete_prefix_and_match_final_sizing() {
        // The same previous short/word/long queue costs 180k to clear, even
        // after consumption. Two mixed three-entry groups save 120k compared
        // with reserving 270k for every result. Use an exact budget boundary.
        let p = model(180_000);
        let entries: Vec<_> = [31, 32, 64]
            .into_iter()
            .map(|length| {
                let mut entry = deferred();
                entry.returnData = Bytes::from(vec![0xff; length]);
                entry
            })
            .collect();
        let cost = p.cost(&entries, 0, false, None);
        assert_eq!(cost.entry_discount, 360_000);
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
        let old_cost = TxL1Gas {
            entry_discount: 3 * (POSTBATCH_ENTRY_GAS_PIN - DEFERRED_ENTRY_GAS),
            ..cost
        };
        let mut old = PostBatchGasBudget::new(cap);
        old.projected = p.initial_gas();
        assert!(old.try_accept(old_cost));
        assert!(!old.try_accept(old_cost));
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
        let mut p = model(35 * PREVIOUS_ENTRY_CLEANUP_GAS);
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
        assert!(
            p.initial_gas() > 21_000 + POSTBATCH_BASE_GAS_PIN + POSTBATCH_ENTRY_GAS_PIN,
            "the old extra anchor pin cannot pay for 35 consumed prior entries"
        );
        assert_eq!(
            p.batch_gas(&batch(vec![anchor()], 1), 0, 0),
            p.initial_gas()
        );
    }

    #[test]
    fn exceeding_measured_size_restores_all_reserves_and_rejection_is_atomic() {
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
        budget.cap = budget.projected + cost.total();
        let before = budget;
        assert!(
            !budget.try_accept(cost),
            "128th source must restore the earlier discounts too"
        );
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
    fn cleanup_reads_persisted_lengths_not_the_consumed_cursor_and_bounds_reads() {
        let rid = 1;
        let head = U256::from_be_bytes(keccak256((U256::from(rid), U256::from(3)).abi_encode()).0);
        let normal = head + U256::from(1);
        let start = U256::from_be_bytes(keccak256(normal.to_be_bytes::<32>()).0);
        let mut storage = std::collections::HashMap::new();
        storage.insert(B256::from(normal), U256::from(2));
        storage.insert(B256::from(head), U256::from(2) << 64); // consumed both
        for (i, encoded) in [
            U256::from_be_bytes([0xfe; 32]) & !U256::from(193),
            U256::from(129),
        ]
        .into_iter()
        .enumerate()
        {
            let entry = start + U256::from(i * 7);
            storage.insert(B256::from(entry), U256::from(1));
            storage.insert(B256::from(entry + U256::from(6)), encoded);
        }
        let read = |map: &std::collections::HashMap<B256, U256>| {
            previous_cleanup(rid, |key| {
                Ok::<_, ()>(map.get(&key).copied().unwrap_or_default())
            })
            .unwrap()
        };
        assert_eq!(read(&storage), Some(120_000));
        for length in 0..=MAX_SIMPLE_RESULT_BYTES {
            let encoded = if length < 32 {
                U256::from(length * 2)
            } else {
                U256::from(length * 2 + 1)
            };
            storage.insert(B256::from(start + U256::from(6)), encoded);
            let expected = match length {
                0..=31 => 120_000,
                32 => 125_000,
                _ => 130_000,
            };
            assert_eq!(read(&storage), Some(expected), "result bytes {length}");
        }
        for malformed in [1, 63, 64, 66, 128, 131] {
            storage.insert(B256::from(start + U256::from(6)), U256::from(malformed));
            assert_eq!(
                read(&storage),
                None,
                "invalid or oversized result encoding {malformed}"
            );
        }
        storage.insert(B256::from(start + U256::from(6)), U256::from(131));
        assert_eq!(read(&storage), None, "65-byte old result needs fallback");
        storage.insert(B256::from(start + U256::from(6)), U256::ZERO);
        storage.insert(B256::from(start + U256::from(2)), U256::from(1));
        assert_eq!(read(&storage), None, "old inline calls need fallback");
        storage.insert(B256::from(normal), U256::ZERO);
        storage.insert(B256::from(head + U256::from(2)), U256::from(1));
        assert_eq!(
            read(&storage),
            None,
            "empty normal queue does not imply empty static queue"
        );
        let mut reads = 0;
        assert_eq!(
            previous_cleanup(rid, |_| {
                reads += 1;
                Ok::<_, ()>(U256::MAX)
            })
            .unwrap(),
            None
        );
        assert_eq!(
            reads, 1,
            "untrusted lengths cannot cause an unbounded state walk"
        );
        assert_eq!(
            previous_cleanup(rid, |_| Err::<U256, _>("unavailable")),
            Err("unavailable")
        );
    }

    #[test]
    fn fitted_deferred_work_can_exceed_the_old_thirty_four_entry_projection() {
        let p = model(35 * PREVIOUS_ENTRY_CLEANUP_GAS);
        let cap = eez_l1::posting_fees::PostingFeeConfig::parse(|name| {
            Ok(matches!(
                name,
                "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS" | "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS"
            )
            .then(|| "500000000000".into()))
        })
        .unwrap()
        .estimate_budget(16_777_216);
        let mut budget = PostBatchGasBudget::new(cap);
        budget.projected = p.initial_gas();
        for _ in 0..35 {
            assert!(budget.try_accept(p.cost(&[deferred()], 0, false, None)));
        }
        assert!(budget.projected + POSTBATCH_DRAIN_MARGIN <= cap);
    }

    #[test]
    fn unsupported_batch_family_keeps_conservative_reserves_and_known_cleanup() {
        let p = model(PREVIOUS_ENTRY_CLEANUP_GAS);
        let b = batch(vec![anchor(), deferred()], 1);
        let conservative = projected_postbatch_gas(2, 100, 200) + PREVIOUS_ENTRY_CLEANUP_GAS;
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
        for (name, expected) in [
            ("BOUNDED_DEFERRED_GAS_PIN", DEFERRED_ENTRY_GAS),
            ("SHORT_DEFERRED_GAS_PIN", SHORT_DEFERRED_ENTRY_GAS),
            ("WORD_DEFERRED_GAS_PIN", WORD_DEFERRED_ENTRY_GAS),
            ("EXISTING_PROXY_GAS_PIN", EXISTING_PROXY_ENTRY_GAS),
            ("SIMPLE_QUEUE_CLEANUP_GAS_PIN", PREVIOUS_ENTRY_CLEANUP_GAS),
            ("SHORT_QUEUE_CLEANUP_GAS_PIN", SHORT_ENTRY_CLEANUP_GAS),
            ("WORD_QUEUE_CLEANUP_GAS_PIN", WORD_ENTRY_CLEANUP_GAS),
        ] {
            let prefix = format!("uint256 private constant {name} = ");
            let value = contract
                .lines()
                .find_map(|line| line.trim().strip_prefix(&prefix))
                .expect("contract gas assertion constant")
                .trim_end_matches(';')
                .replace('_', "")
                .parse::<u64>()
                .unwrap();
            assert_eq!(
                value, expected,
                "Rust and executable contract bound {name} drifted"
            );
        }
    }

    #[test]
    fn temporary_cleanup_pressure_does_not_label_a_fitting_source_unpostable() {
        let p = model(20 * PREVIOUS_ENTRY_CLEANUP_GAS);
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
