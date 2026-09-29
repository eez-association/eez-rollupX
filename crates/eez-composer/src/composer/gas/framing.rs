//! Calldata omitted by source-local admission encoding. The producer adds one
//! root update per mutable entry, one pin per static entry and native ECDSA
//! batch metadata. Charge those bytes incrementally, without encoding an
//! accepted prefix again or computing blobs/KZG. Final native execution still
//! validates effects and gas at the exact signed limits.

use alloy_primitives::{Address, B256, Bytes};
use alloy_sol_types::{SolCall, SolValue};
use eez_protocol::abi::{
    EtherDeltaSol, ExecutionEntrySol, RollupIdWithProofSystemsSol, RootUpdateSol,
    StaticExecutionEntrySol, postAndVerifyBatchCall,
};
use std::sync::LazyLock;

use super::super::{MAX_PROOF_BYTES, calldata_gas};

// ABI offsets are byte positions in a Rust allocation. Only usize-width bytes
// can become nonzero when roots or preceding entries change their positions.
const OFFSET_GROWTH: u64 = size_of::<usize>() as u64 * 12;
const ARRAY_OFFSET_GAS: u64 = 32 * 4 + OFFSET_GROWTH;

/// Upper bound for the anchor, enclosing ABI, proof and blob index words. All
/// bytes in the small metadata template are charged nonzero so array lengths,
/// offsets, roots, block number and sender binding can change independently.
/// The native payload remains in blobs, never in `batch.callData`.
pub(in super::super) fn base(maximum_blobs: usize) -> u64 {
    static BASE: LazyLock<u64> = LazyLock::new(|| {
        let batch = eez_protocol::EvmBatch {
            entries: vec![ExecutionEntrySol {
                rootUpdates: vec![RootUpdateSol {
                    rollupId: 0,
                    etherDelta: EtherDeltaSol::ZERO,
                    currentRoot: B256::ZERO,
                    newRoot: B256::ZERO,
                }],
                ..Default::default()
            }],
            proofSystems: vec![Address::ZERO],
            rollupIdsWithProofSystems: vec![RollupIdWithProofSystemsSol {
                rollupId: 0,
                proofSystemIndexes: vec![0],
            }],
            proofs: vec![Bytes::from(vec![0; MAX_PROOF_BYTES])],
            ..Default::default()
        };
        (postAndVerifyBatchCall { batch }.abi_encode().len() as u64).saturating_mul(16)
    });
    BASE.saturating_add((maximum_blobs as u64).saturating_mul(32 * 16))
}

fn hash_growth(hash: B256) -> u64 {
    32 * 16 - calldata_gas(hash.as_slice())
}

/// Bytes whose values/positions change in `prepare_post_batch_raw` and
/// `finalize_l1_entry_traces`. Existing payload and nested ABI tails are already
/// charged by `entry.abi_encode()`. Charging the complete new root even if one
/// is already present is conservative for replacement; malformed prefilled
/// source roots still fail the existing preparation checks.
pub(in super::super) fn mutable(entry: &ExecutionEntrySol) -> u64 {
    // Four root words; four dynamic entry-head offsets; destinationRollupId
    // can change from MAINNET to the registry id (uint64); outer array offset.
    let mut gas = 4 * 32 * 16 + 4 * OFFSET_GROWTH + 8 * 12 + ARRAY_OFFSET_GAS;
    gas += hash_growth(entry.rollingHash);
    for row in &entry.expectedL1ToL2Calls {
        gas = gas
            .saturating_add(hash_growth(row.expectedL1toL2Hash))
            .saturating_add(hash_growth(row.revertedOrStaticRollingHash));
    }
    gas
}

pub(in super::super) fn static_entry(entry: &StaticExecutionEntrySol) -> (u64, u64) {
    let encoded = calldata_gas(&entry.abi_encode());
    let framing = 2 * 32 * 16
        + 3 * OFFSET_GROWTH
        + 8 * 12
        + ARRAY_OFFSET_GAS
        + hash_growth(entry.rollingHash);
    (encoded, framing)
}

/// Standard calldata gas is four times EIP-7623's token count. Saturation
/// refuses unrepresentable candidates at the existing transaction budget.
pub(in super::super) fn floor(calldata: u64) -> u64 {
    21_000_u64.saturating_add(calldata.div_ceil(4).saturating_mul(10))
}

#[cfg(test)]
mod tests {
    use super::super::super::{
        PostBatchGasBudget, TxL1Gas, calldata_floor_gas, projected_tx_l1_gas,
    };
    use super::*;
    use alloy_primitives::U256;
    use eez_protocol::abi::{ExpectedL1ToL2CallSol, ExpectedRootPerRollupSol, L2ToL1CallSol};

    fn root() -> RootUpdateSol {
        RootUpdateSol {
            rollupId: u64::MAX,
            etherDelta: EtherDeltaSol::MINUS_ONE,
            currentRoot: B256::repeat_byte(0xff),
            newRoot: B256::repeat_byte(0xff),
        }
    }

    fn source(bytes: usize, rows: usize, fill: u8) -> ExecutionEntrySol {
        ExecutionEntrySol {
            proxyEntryHash: B256::repeat_byte(fill),
            l2ToL1Calls: vec![L2ToL1CallSol {
                revertNextNCalls: 1,
                isStatic: false,
                gas: 0,
                sourceAddress: Address::repeat_byte(fill),
                sourceRollupId: 1,
                targetAddress: Address::repeat_byte(fill),
                value: U256::MAX,
                data: Bytes::from(vec![fill; bytes]),
            }],
            expectedL1ToL2Calls: (0..rows)
                .map(|index| ExpectedL1ToL2CallSol {
                    expectedL1toL2Hash: B256::repeat_byte(fill),
                    l2ToL1Calls: vec![],
                    revertedOrStaticRollingHash: B256::repeat_byte(fill),
                    success: index % 2 == 0,
                    returnData: Bytes::from(vec![fill; bytes % 67]),
                })
                .collect(),
            rollingHash: B256::repeat_byte(fill),
            returnData: Bytes::from(vec![fill; bytes % 97]),
            ..Default::default()
        }
    }

    fn final_entry(mut entry: ExecutionEntrySol) -> ExecutionEntrySol {
        entry.rootUpdates = vec![root()];
        entry.destinationRollupId = u64::MAX;
        entry.rollingHash = B256::repeat_byte(0xff);
        for row in &mut entry.expectedL1ToL2Calls {
            row.expectedL1toL2Hash = B256::repeat_byte(0xff);
            row.revertedOrStaticRollingHash = B256::repeat_byte(0xff);
        }
        entry
    }

    fn verify_bound(
        count: usize,
        blobs: usize,
        bytes: usize,
        rows: usize,
        statics: usize,
        fill: u8,
    ) {
        let mut estimate = base(blobs);
        let mut batch = eez_protocol::EvmBatch {
            entries: vec![ExecutionEntrySol {
                rootUpdates: vec![root()],
                rollingHash: B256::repeat_byte(0xff),
                destinationRollupId: u64::MAX,
                success: true,
                ..Default::default()
            }],
            immediateEntryCount: U256::from(count + 1),
            proofSystems: vec![Address::repeat_byte(0xff)],
            rollupIdsWithProofSystems: vec![RollupIdWithProofSystemsSol {
                rollupId: u64::MAX,
                proofSystemIndexes: vec![0],
            }],
            blobIndices: (0..blobs).map(U256::from).collect(),
            proofs: vec![Bytes::from(vec![0xff; MAX_PROOF_BYTES])],
            blockNumber: u64::MAX,
            bindMsgSenderInPublicInput: true,
            ..Default::default()
        };
        for _ in 0..count {
            let entry = source(bytes, rows, fill);
            estimate += calldata_gas(&entry.abi_encode()) + mutable(&entry);
            batch.entries.push(final_entry(entry));
        }
        for _ in 0..statics {
            let mut entry = StaticExecutionEntrySol {
                proxyEntryHash: B256::repeat_byte(fill),
                rollingHash: B256::repeat_byte(fill),
                returnData: Bytes::from(vec![fill; bytes]),
                ..Default::default()
            };
            let (raw, framing) = static_entry(&entry);
            estimate += raw + framing;
            entry.expectedRoots = vec![ExpectedRootPerRollupSol {
                rollupId: u64::MAX,
                root: B256::repeat_byte(0xff),
            }];
            entry.destinationRollupId = u64::MAX;
            entry.rollingHash = B256::repeat_byte(0xff);
            batch.staticEntries.push(entry);
        }
        let encoded = postAndVerifyBatchCall { batch }.abi_encode();
        let actual = calldata_gas(&encoded);
        assert!(
            actual <= estimate,
            "{count}/{blobs}/{bytes}/{rows}/{statics}/{fill}: {actual} > {estimate}"
        );
        assert!(calldata_floor_gas(&encoded) <= floor(estimate));
    }

    #[test]
    fn framing_bounds_full_abi_across_entry_blob_and_offset_boundaries() {
        for count in [0, 1, 2, 31, 32, 127, 128, 129] {
            for blobs in [0, 1, 6, 64] {
                for fill in [0, 0x7f, 0xff] {
                    verify_bound(count, blobs, 257, 4, 2, fill);
                }
            }
        }
        for bytes in [0, 31, 32, 33, 255, 256, 65535, 65536, 100_000] {
            verify_bound(2, 9, bytes, 0, 1, 0xff);
        }
        verify_bound(2, 64, 31, 129, 0, 0);
    }

    #[test]
    fn framing_releases_capacity_without_dropping_the_fork_calldata_floor() {
        let entry = source(100_000, 0, 0xff);
        let cost = projected_tx_l1_gas(&[entry], 0);
        let mut pre_prague = PostBatchGasBudget::with_calldata_limits(3_000_000, 1, false);
        assert!(pre_prague.try_accept(cost));
        let mut prague = PostBatchGasBudget::with_calldata_limits(3_000_000, 1, true);
        let before = prague;
        assert!(!prague.try_accept(cost));
        assert_eq!(prague.rejected_constraint, Some("l1_tx_gas_budget"));
        assert_eq!(prague.projected, before.projected);
        assert_eq!(prague.calldata_gas, before.calldata_gas);
        assert_eq!(prague.framing_gas, before.framing_gas);

        let small = projected_tx_l1_gas(&[source(36, 1, 0)], 0);
        let mut budget = PostBatchGasBudget::with_calldata_limits(1_000_000, 6, true);
        // The old fixed reserve alone would make this complete group exceed
        // the cap. The encoded bound below must still cover actual final ABI.
        assert!(budget.projected + small.total() + 600_000 > budget.cap);
        assert!(budget.try_accept(small));
        assert!(budget.full_estimate() < budget.cap);
        verify_bound(1, 6, 36, 1, 0, 0);
    }

    #[test]
    fn static_pool_revokes_prior_execution_discounts_transactionally() {
        let mut budget = PostBatchGasBudget::with_calldata_limits(2_000_000, 1, true);
        budget.use_projection(181_000);
        let cost = TxL1Gas {
            entries: 1,
            entry_discount: 300_000,
            ..Default::default()
        };
        assert!(budget.try_accept(cost));
        let static_cost = TxL1Gas {
            general: true,
            calldata_gas: 2_000,
            framing_gas: 2_000,
            ..Default::default()
        };
        budget.cap = budget.full_estimate() + 400_000;
        let before = budget;
        assert!(!budget.try_accept(static_cost));
        assert_eq!(budget.entry_discount, before.entry_discount);
        assert_eq!(budget.anchor_discount, before.anchor_discount);
        assert!(!budget.general);
        budget.cap = 2_000_000;
        assert!(budget.try_accept(static_cost));
        assert_eq!(budget.projected, 181_000 + 370_000 + 370_000 + 2_000);
        assert_eq!(budget.entry_discount, 0);
        assert_eq!(budget.anchor_discount, 0);
        assert!(budget.general);
        assert!(budget.try_accept(cost));
        assert_eq!(budget.entry_discount, 0);
        assert_eq!(budget.projected, 181_000 + 3 * 370_000 + 2_000);
    }

    #[test]
    fn static_only_minimum_does_not_require_mutable_entry_capacity() {
        let mut budget = PostBatchGasBudget::with_calldata_limits(1_000_000, 6, true);
        // A previous static source already selected general execution pins.
        budget.general = true;
        budget.entries = 1;
        budget.cap = budget.full_estimate() + 10_000;
        assert_eq!(budget.minimum_rejection(true, 100_000), None);
        assert_eq!(budget.minimum_rejection(false, 0), Some("l1_tx_gas_budget"));
        let (calldata_gas, framing_gas) = static_entry(&StaticExecutionEntrySol::default());
        assert!(budget.try_accept_with_companion(
            TxL1Gas {
                calldata_gas,
                framing_gas,
                general: true,
                ..Default::default()
            },
            100_000,
            50_000,
        ));
    }

    #[test]
    fn minimum_inbound_checks_static_discount_revocation_before_rejecting() {
        let mut budget = PostBatchGasBudget::with_calldata_limits(2_000_000, 6, true);
        budget.use_projection(181_000);
        let minimum = super::super::minimum_entry_gas(false);
        assert!(budget.try_accept(TxL1Gas {
            entries: 1,
            entry_discount: 370_000 - minimum,
            ..Default::default()
        }));
        let before = budget;
        budget.cap = budget.full_estimate() + minimum - 1;
        assert_eq!(
            budget.minimum_rejection(true, 100_000),
            Some("l1_tx_gas_budget")
        );
        budget.cap += 1;
        assert_eq!(budget.minimum_rejection(true, 100_000), None);
        assert_eq!(budget.projected, before.projected);
        assert_eq!(budget.anchor_discount, before.anchor_discount);
        assert_eq!(budget.entry_discount, before.entry_discount);
    }

    #[test]
    fn minimum_inbound_never_rejects_a_fitting_static_group() {
        for general in [false, true] {
            for entries in [1, 127, 128] {
                for remaining in [0, 5_000, 70_000, 500_000, 4_000_000] {
                    let mut budget = PostBatchGasBudget::with_calldata_limits(16_777_216, 6, true);
                    budget.use_projection(181_000);
                    budget.entries = entries;
                    budget.general = general;
                    budget.entry_discount = if general { 0 } else { 2_000_000 };
                    budget.anchor_discount = if general { 0 } else { 370_000 };
                    budget.cap = budget.full_estimate() + remaining;
                    for calldata in [0, 5_000, 100_000] {
                        let mut next = budget;
                        if next.try_accept_with_companion(
                            TxL1Gas {
                                general: true,
                                calldata_gas: calldata,
                                framing_gas: 2_000,
                                ..Default::default()
                            },
                            100_000,
                            50_000,
                        ) {
                            assert_eq!(budget.minimum_rejection(true, 100_000), None);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn blob_count_outside_shaped_profile_preserves_general_pins() {
        let mut budget = PostBatchGasBudget::with_calldata_limits(2_000_000, 65, true);
        budget.use_projection(181_000 + 50_000);
        assert_eq!(budget.projected, 551_000 + 50_000);
        assert!(budget.try_accept(TxL1Gas {
            entries: 1,
            entry_discount: 300_000,
            ..Default::default()
        }));
        assert_eq!(budget.projected, 921_000 + 50_000);
        assert_eq!(budget.entry_discount, 0);
        verify_bound(1, 65, 32, 1, 0, 0);
    }

    #[test]
    fn framing_and_floor_overflow_cannot_admit_an_unrepresentable_candidate() {
        let mut budget = PostBatchGasBudget::with_calldata_limits(u64::MAX, 1, true);
        let before = budget;
        assert!(!budget.try_accept(TxL1Gas {
            framing_gas: u64::MAX,
            ..Default::default()
        }));
        assert_eq!(budget.framing_gas, before.framing_gas);
        assert!(!budget.try_accept(TxL1Gas {
            calldata_gas: u64::MAX / 2,
            ..Default::default()
        }));
        assert_eq!(budget.calldata_gas, before.calldata_gas);
    }
}
