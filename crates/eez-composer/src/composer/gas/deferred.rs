//! Storage-only pricing for the pinned mapping-backed deferred queue. Calls
//! execute later in companion transactions and are budgeted separately.
use super::EstimateReason;
use std::collections::BTreeMap;

use alloy_primitives::{U256, keccak256};
use alloy_sol_types::SolValue;
use eez_protocol::abi::{ExecutionEntrySol, L2ToL1CallSol};

// Cold caller-frame bounds, before refunds; calibrated by PostBatchGasPins.
pub(super) const ENTRY_GAS: u64 = 35_000;
pub(super) const WORD_GAS: u64 = 8_000;
pub(super) const FRESH_WORD_GAS: u64 = 15_000;
pub(super) const ITEM_GAS: u64 = 8_000;
const MAX_CALLS: usize = 4;
const MAX_ROWS: usize = 4;
const MAX_INPUT: usize = 256;
const MAX_RESULT: usize = 64;

pub(super) struct Slot {
    head: U256,
    words: BTreeMap<U256, U256>,
}

fn at(head: U256, offset: usize) -> U256 {
    head.wrapping_add(U256::from(offset))
}

fn data(head: U256) -> U256 {
    U256::from_be_bytes(keccak256(head.abi_encode()).0)
}

struct Reader<F> {
    read: F,
    words: BTreeMap<U256, U256>,
}

impl<E, F: FnMut(U256) -> Result<U256, E>> Reader<F> {
    fn word(&mut self, slot: U256) -> Result<U256, E> {
        let value = (self.read)(slot)?;
        self.words.insert(slot, value);
        Ok(value)
    }

    fn bytes(&mut self, head: U256, limit: usize) -> Result<bool, E> {
        let value = self.word(head)?;
        if value.bit(0) {
            // Bound the entire encoded length before narrowing or iterating.
            if value < U256::from(65) || value > U256::from(limit * 2 + 1) {
                return Ok(false);
            }
            let length = (value.to::<usize>() - 1) / 2;
            for i in 0..length.div_ceil(32) {
                self.word(at(data(head), i))?;
            }
            Ok(true)
        } else {
            Ok(value.byte(0) <= 62)
        }
    }

    fn calls(&mut self, head: U256, remaining: &mut usize) -> Result<bool, E> {
        let count = self.word(head)?;
        if count > U256::from(*remaining) {
            return Ok(false);
        }
        let count = count.to::<usize>();
        *remaining -= count;
        for i in 0..count {
            let call = at(data(head), i * 4);
            for j in 0..3 {
                self.word(at(call, j))?;
            }
            if !self.bytes(at(call, 3), MAX_INPUT)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

impl Slot {
    /// Capture only a bounded inventory. Inactive mapping values must be read:
    /// resetting the active queue length does not erase their storage.
    pub(super) fn read<E>(
        head: U256,
        read: impl FnMut(U256) -> Result<U256, E>,
    ) -> Result<Option<Self>, E> {
        let mut r = Reader {
            read,
            words: BTreeMap::new(),
        };
        let roots = r.word(head)?;
        if roots.is_zero() {
            for i in 1..7 {
                if !r.word(at(head, i))?.is_zero() {
                    return Ok(None);
                }
            }
            return Ok(Some(Self {
                head,
                words: r.words,
            }));
        }
        if roots != U256::from(1) {
            return Ok(None);
        }
        for i in 0..3 {
            r.word(at(data(head), i))?;
        }
        for i in [1, 4, 5] {
            r.word(at(head, i))?;
        }
        if !r.bytes(at(head, 6), MAX_RESULT)? {
            return Ok(None);
        }
        let mut remaining = MAX_CALLS;
        if !r.calls(at(head, 2), &mut remaining)? {
            return Ok(None);
        }
        let rows = r.word(at(head, 3))?;
        if rows > U256::from(MAX_ROWS) {
            return Ok(None);
        }
        for i in 0..rows.to::<usize>() {
            let row = at(data(at(head, 3)), i * 5);
            for j in [0, 2, 3] {
                r.word(at(row, j))?;
            }
            if !r.calls(at(row, 1), &mut remaining)? || !r.bytes(at(row, 4), MAX_RESULT)? {
                return Ok(None);
            }
        }
        Ok(Some(Self {
            head,
            words: r.words,
        }))
    }

    pub(super) fn gas(
        &self,
        entry: &ExecutionEntrySol,
        rollup: u64,
    ) -> Result<u64, EstimateReason> {
        if !entry.success {
            return Err(EstimateReason::DeferredReverted);
        }
        if entry.rootUpdates.len() > 1
            || entry
                .rootUpdates
                .iter()
                .any(|r| r.rollupId != rollup || !r.etherDelta.is_zero())
            || entry.proxyEntryHash.is_zero()
            || entry.destinationRollupId != rollup
        {
            return Err(EstimateReason::DeferredShape);
        }
        let calls = entry.l2ToL1Calls.len()
            + entry
                .expectedL1ToL2Calls
                .iter()
                .map(|r| r.l2ToL1Calls.len())
                .sum::<usize>();
        let rows = entry.expectedL1ToL2Calls.len();
        if calls > MAX_CALLS || rows > MAX_ROWS || entry.returnData.len() > MAX_RESULT {
            return Err(EstimateReason::DeferredSizeBound);
        }
        let mut writes = BTreeMap::new();
        // Roots/hashes change during stitching. Price every final root word as
        // nonzero, including the one update absent from intermediate entries.
        for i in [0, 1, 4, 5] {
            writes.insert(at(self.head, i), true);
        }
        for i in 0..3 {
            writes.insert(at(data(self.head), i), true);
        }
        write_bytes(&mut writes, at(self.head, 6), &entry.returnData);
        write_calls(&mut writes, at(self.head, 2), &entry.l2ToL1Calls, rollup)?;
        writes.insert(at(self.head, 3), rows != 0);
        for (i, row) in entry.expectedL1ToL2Calls.iter().enumerate() {
            if !row.success || !row.revertedOrStaticRollingHash.is_zero() {
                return Err(EstimateReason::DeferredRevertedOrStaticRow);
            }
            if row.returnData.len() > MAX_RESULT {
                return Err(EstimateReason::DeferredSizeBound);
            }
            let head = at(data(at(self.head, 3)), i * 5);
            // The position hash also binds the stitched rolling hash.
            writes.insert(head, true);
            writes.insert(at(head, 2), false);
            writes.insert(at(head, 3), true);
            write_calls(&mut writes, at(head, 1), &row.l2ToL1Calls, rollup)?;
            write_bytes(&mut writes, at(head, 4), &row.returnData);
        }
        let mut gas = ENTRY_GAS + (calls + rows) as u64 * ITEM_GAS;
        for (slot, nonzero) in &writes {
            gas += WORD_GAS;
            if *nonzero && self.words.get(slot).is_none_or(U256::is_zero) {
                gas += FRESH_WORD_GAS;
            }
        }
        // Solidity assignment clears truncated arrays/long byte tails. Charge
        // all obsolete words, including zero ones, and never credit refunds.
        gas += self
            .words
            .keys()
            .filter(|slot| !writes.contains_key(slot))
            .count() as u64
            * WORD_GAS;
        Ok(gas)
    }
}

fn write_bytes(writes: &mut BTreeMap<U256, bool>, head: U256, bytes: &[u8]) {
    writes.insert(head, !bytes.is_empty());
    if bytes.len() >= 32 {
        for (i, word) in bytes.chunks(32).enumerate() {
            writes.insert(at(data(head), i), word.iter().any(|b| *b != 0));
        }
    }
}

fn write_calls(
    writes: &mut BTreeMap<U256, bool>,
    head: U256,
    calls: &[L2ToL1CallSol],
    rollup: u64,
) -> Result<(), EstimateReason> {
    writes.insert(head, !calls.is_empty());
    for (i, call) in calls.iter().enumerate() {
        if call.sourceRollupId != rollup
            || call.revertNextNCalls != 0
            || call.isStatic
            || call.gas != 0
            || !call.value.is_zero()
        {
            return Err(EstimateReason::DeferredCallFlags);
        }
        if call.data.len() > MAX_INPUT {
            return Err(EstimateReason::DeferredSizeBound);
        }
        let call_head = at(data(head), i * 4);
        writes.insert(call_head, !call.sourceAddress.is_zero());
        writes.insert(
            at(call_head, 1),
            call.sourceRollupId != 0 || !call.targetAddress.is_zero(),
        );
        writes.insert(at(call_head, 2), false);
        write_bytes(writes, at(call_head, 3), &call.data);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{Address, B256, Bytes};
    use eez_protocol::abi::{ExpectedL1ToL2CallSol, RootUpdateSol};

    fn entry() -> ExecutionEntrySol {
        ExecutionEntrySol {
            rootUpdates: vec![RootUpdateSol {
                rollupId: 1,
                currentRoot: B256::repeat_byte(1),
                newRoot: B256::repeat_byte(2),
                etherDelta: eez_protocol::abi::EtherDeltaSol::ZERO,
            }],
            proxyEntryHash: B256::repeat_byte(3),
            destinationRollupId: 1,
            success: true,
            rollingHash: B256::repeat_byte(4),
            returnData: Bytes::from(vec![0xa5; 32]),
            l2ToL1Calls: vec![L2ToL1CallSol {
                sourceRollupId: 1,
                sourceAddress: Address::repeat_byte(1),
                targetAddress: Address::repeat_byte(2),
                data: Bytes::from(vec![0xa5; 4]),
                revertNextNCalls: 0,
                isStatic: false,
                gas: 0,
                value: U256::ZERO,
            }],
            ..Default::default()
        }
    }

    // Independent concrete layout of a retained entry with one short callback
    // and a 32-byte result. Solidity asserts these packed offsets against vm.load.
    fn retained() -> BTreeMap<U256, U256> {
        let head = U256::from(123);
        let roots = data(head);
        let calls = data(at(head, 2));
        [
            (head, 1),
            (at(head, 1), 3),
            (at(head, 2), 1),
            (at(head, 3), 0),
            (at(head, 4), 4),
            (at(head, 5), 1),
            (at(head, 6), 65),
            (roots, 1),
            (at(roots, 1), 1),
            (at(roots, 2), 2),
            (calls, 1),
            (at(calls, 1), 1),
            (at(calls, 2), 0),
            (at(calls, 3), 8),
            (data(at(head, 6)), 5),
        ]
        .into_iter()
        .map(|(k, v)| (k, U256::from(v)))
        .collect()
    }

    fn read(words: &BTreeMap<U256, U256>) -> Slot {
        Slot::read(U256::from(123), |slot| {
            Ok::<_, ()>(words.get(&slot).copied().unwrap_or_default())
        })
        .unwrap()
        .unwrap()
    }

    #[test]
    fn fresh_retained_and_truncated_storage_are_priced_before_refunds() {
        let fresh = read(&BTreeMap::new());
        let old = read(&retained());
        let e = entry();
        assert_eq!(fresh.gas(&e, 1).unwrap(), 358_000);
        assert_eq!(old.gas(&e, 1).unwrap(), 163_000);
        let mut shortened = e.clone();
        shortened.l2ToL1Calls.clear();
        shortened.returnData = Bytes::new();
        // Four old callback words plus the result tail are still charged.
        assert_eq!(old.gas(&shortened, 1).unwrap(), 155_000);
        let mut partial = retained();
        partial.insert(data(at(U256::from(123), 6)), U256::ZERO);
        assert_eq!(read(&partial).gas(&e, 1).unwrap(), 178_000);
        let mut zero_result = e;
        zero_result.returnData = Bytes::from(vec![0; 32]);
        assert_eq!(read(&partial).gas(&zero_result, 1).unwrap(), 163_000);
    }

    #[test]
    fn incomplete_roots_cannot_discount_final_stitched_roots() {
        let slot = read(&BTreeMap::new());
        let mut e = entry();
        let before = slot.gas(&e, 1).unwrap();
        e.rootUpdates.clear();
        e.rollingHash = B256::ZERO;
        assert_eq!(slot.gas(&e, 1).unwrap(), before);
    }

    #[test]
    fn malformed_parent_lengths_are_bounded_before_storage_walk() {
        let head = U256::from(123);
        for (slot, value) in [
            (head, U256::MAX),
            (at(head, 2), U256::MAX),
            (at(head, 3), U256::MAX),
            (at(head, 6), U256::MAX),
            (at(head, 6), U256::from(64)),
            (at(head, 6), U256::from(63)),
            (at(head, 6), U256::from(131)),
            (at(data(at(head, 2)), 3), U256::from(515)),
        ] {
            let mut words = retained();
            words.insert(slot, value);
            let mut reads = 0;
            assert!(
                Slot::read(head, |k| {
                    reads += 1;
                    Ok::<_, ()>(words.get(&k).copied().unwrap_or_default())
                })
                .unwrap()
                .is_none()
            );
            assert!(reads <= 128);
        }
        assert!(Slot::read(head, |_| Err::<U256, _>("unavailable")).is_err());
        let mut invalid_empty = BTreeMap::new();
        invalid_empty.insert(at(head, 4), U256::from(1));
        assert!(
            Slot::read(head, |k| Ok::<_, ()>(
                invalid_empty.get(&k).copied().unwrap_or_default()
            ))
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn current_shape_bounds_preserve_fallbacks() {
        let slot = read(&retained());
        let base = entry();
        let changes: [fn(&mut ExecutionEntrySol); 10] = [
            |e| e.success = false,
            |e| e.proxyEntryHash = B256::ZERO,
            |e| e.rootUpdates[0].rollupId = 2,
            |e| e.destinationRollupId = 2,
            |e| e.returnData = Bytes::from(vec![0; 65]),
            |e| e.l2ToL1Calls[0].data = Bytes::from(vec![0; 257]),
            |e| e.l2ToL1Calls[0].revertNextNCalls = 1,
            |e| e.l2ToL1Calls[0].isStatic = true,
            |e| e.l2ToL1Calls[0].value = U256::from(1),
            |e| e.l2ToL1Calls = vec![e.l2ToL1Calls[0].clone(); 5],
        ];
        for change in changes {
            let mut e = base.clone();
            change(&mut e);
            assert!(slot.gas(&e, 1).is_err());
        }
        let mut e = base;
        e.expectedL1ToL2Calls = vec![ExpectedL1ToL2CallSol {
            success: true,
            expectedL1toL2Hash: B256::repeat_byte(8),
            returnData: Bytes::from(vec![0xff; 64]),
            l2ToL1Calls: vec![e.l2ToL1Calls[0].clone(); 3],
            revertedOrStaticRollingHash: B256::ZERO,
        }];
        assert!(slot.gas(&e, 1).is_ok());
        e.expectedL1ToL2Calls[0]
            .l2ToL1Calls
            .push(e.l2ToL1Calls[0].clone());
        assert!(slot.gas(&e, 1).is_err());
    }
}
