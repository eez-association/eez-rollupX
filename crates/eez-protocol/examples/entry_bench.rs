//! Matched release benchmarks for materialization and independent verification.
//! Run with `cargo run --locked --release -p eez-protocol --example entry_bench`.
//! Materialization and finalization include input cloning; verification borrows prebuilt tables.
//! These component timings do not measure whole-node throughput.
use alloy_primitives::{Address, B256, Bytes, U256};
use eez_protocol::{
    CallMode, ExecutedAction, ExecutionOutcome, RollupId,
    abi::{EtherDeltaSol, RootUpdateSol},
    entries::{finalize_l1_entry_traces, verify_outbound_source_group},
    materialize_actions,
    system_tx::OutboundSourceEntry,
};
use std::{hint::black_box, time::Instant};

fn main() {
    let l2 = RollupId(1);
    for (name, roots, depth, bytes, static_calls, failed, rollback, rounds) in [
        ("single", 1usize, 1usize, 0, false, false, false, 20_000),
        ("many_roots", 64, 1, 32, false, false, false, 1_000),
        ("nested", 8, 8, 32, false, false, false, 1_000),
        ("large_results", 4, 8, 4096, false, false, false, 200),
        ("failed_frames", 8, 8, 32, false, true, false, 1_000),
        ("rollback_roots", 64, 1, 32, false, false, true, 1_000),
        ("recursive_static", 8, 8, 32, true, false, false, 1_000),
    ] {
        let mut calls = Vec::new();
        for root in 0..roots {
            for level in 0..depth {
                let source = if level % 2 == 0 {
                    l2
                } else {
                    RollupId::MAINNET
                };
                let target = if source == l2 { RollupId::MAINNET } else { l2 };
                let mut data = vec![0x55; bytes];
                data.extend_from_slice(&calls.len().to_le_bytes());
                calls.push(ExecutedAction {
                    parent_index: (level != 0).then(|| calls.len() - 1),
                    call_mode: if static_calls {
                        CallMode::Static
                    } else {
                        CallMode::Mutable
                    },
                    source_rollup_id: source,
                    target_rollup_id: target,
                    source_address: Address::with_last_byte(source.0 as u8 + 1),
                    target_address: Address::with_last_byte(target.0 as u8 + 1),
                    value: U256::ZERO,
                    data: Bytes::from(data),
                    outcome: ExecutionOutcome::Resolved {
                        success: !(failed && level == 1),
                        return_data: vec![0x77; bytes],
                        gas_used: 21_000,
                    },
                    revert_spans: if rollback && root == roots / 2 {
                        vec![u32::try_from(roots / 2).unwrap()]
                    } else {
                        vec![]
                    },
                });
            }
        }
        let mut composition = materialize_actions(l2, calls.clone()).unwrap();
        let target = &mut composition.targets[0];
        target.batch.entries[0].rootUpdates.push(RootUpdateSol {
            rollupId: l2.0,
            currentRoot: B256::with_last_byte(1),
            newRoot: B256::with_last_byte(2),
            etherDelta: EtherDeltaSol::ZERO,
        });
        finalize_l1_entry_traces(&mut target.batch, &target.entry_traces).unwrap();
        let sources: Vec<_> = composition
            .source
            .batch
            .entries
            .into_iter()
            .map(OutboundSourceEntry::Mutable)
            .chain(
                composition
                    .source
                    .l2_static_entries
                    .into_iter()
                    .map(OutboundSourceEntry::Static),
            )
            .collect();
        let host = &target.batch.entries[0];
        let expected = verify_outbound_source_group(host, &sources, l2).unwrap();
        assert_eq!(
            expected.l2_outbound_hashes.len(),
            if static_calls {
                0
            } else if rollback {
                roots / 2
            } else if failed {
                roots
            } else {
                roots * depth.div_ceil(2)
            }
        );
        for phase in ["materialize", "finalize", "verify"] {
            let run = || {
                if phase == "materialize" {
                    black_box(materialize_actions(l2, black_box(&calls).clone()).unwrap());
                } else if phase == "finalize" {
                    let mut batch = black_box(&target.batch).clone();
                    finalize_l1_entry_traces(&mut batch, black_box(&target.entry_traces)).unwrap();
                    black_box(batch);
                } else {
                    black_box(
                        verify_outbound_source_group(black_box(host), black_box(&sources), l2)
                            .unwrap(),
                    );
                }
            };
            for _ in 0..100 {
                run();
            }
            let start = Instant::now();
            for _ in 0..rounds {
                run();
            }
            println!(
                "{name},{phase},{rounds},{:.2}",
                start.elapsed().as_secs_f64() * 1e9 / f64::from(rounds)
            );
        }
    }
}
