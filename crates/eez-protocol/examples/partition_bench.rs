//! Release microbenchmark for source-group partitioning.
//! Run with `cargo run --locked --release -p eez-protocol --example partition_bench`.
//! This measures the partitioner, not end-to-end node throughput.
use alloy_primitives::{Address, Bytes, U256};
use eez_protocol::{
    abi::{ExecutionEntrySol, L2ToL1CallSol},
    system_tx::outbound_settlement_source_counts,
};
use std::{hint::black_box, time::Instant};

fn main() {
    for (name, hosts, calls, bytes, rollback, split, rounds) in [
        ("single", 1, 1, 0, false, false, 200_000),
        ("many_hosts", 64, 4, 32, false, false, 5_000),
        ("large_calldata", 8, 16, 4096, false, false, 1_000),
        ("static_pool", 8, 16, 128, false, false, 10_000),
        ("nested_rollback", 8, 32, 128, true, false, 5_000),
        ("split_mutable", 8, 8, 32, false, true, 10_000),
        ("split_rollback", 8, 32, 128, true, true, 5_000),
    ] {
        let mut entries = vec![ExecutionEntrySol::default(); hosts];
        for entry in &mut entries {
            entry.l2ToL1Calls = (0..calls)
                .map(|i| L2ToL1CallSol {
                    targetAddress: Address::with_last_byte((i + 1) as u8),
                    data: Bytes::from(vec![i as u8; bytes]),
                    revertNextNCalls: if rollback && i < calls / 2 {
                        (calls - 2 * i) as u16
                    } else {
                        0
                    },
                    isStatic: name == "static_pool",
                    gas: 0,
                    sourceAddress: Address::ZERO,
                    sourceRollupId: 1,
                    value: U256::ZERO,
                })
                .collect();
        }
        let source_count = if name == "static_pool" {
            calls * 2
        } else {
            calls
        };
        let groups = if split {
            vec![(hosts * calls, false)]
        } else {
            vec![(source_count, name == "static_pool"); hosts]
        };
        let expected = vec![source_count; hosts];
        assert_eq!(
            outbound_settlement_source_counts(&entries, groups.iter().copied()).unwrap(),
            expected
        );
        for _ in 0..100 {
            black_box(
                outbound_settlement_source_counts(
                    black_box(&entries),
                    black_box(&groups).iter().copied(),
                )
                .unwrap(),
            );
        }
        let start = Instant::now();
        for _ in 0..rounds {
            black_box(
                outbound_settlement_source_counts(
                    black_box(&entries),
                    black_box(&groups).iter().copied(),
                )
                .unwrap(),
            );
        }
        println!(
            "{name},{rounds},{:.2}",
            start.elapsed().as_secs_f64() * 1e9 / f64::from(rounds)
        );
    }
}
