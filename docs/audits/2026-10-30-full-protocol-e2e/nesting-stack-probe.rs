use eez_protocol::{CallMode, ExecutedAction, ExecutionOutcome, RollupId, materialize_actions};
use eez_protocol::abi::{RootUpdateSol, EtherDeltaSol};
use eez_protocol::entries::{finalize_l1_entry_traces, verify_outbound_source_group};
use eez_protocol::system_tx::OutboundSourceEntry;
fn main() {
    let depth: usize = std::env::args().nth(1).unwrap().parse().unwrap();
    let mutable = std::env::args().nth(2).as_deref() == Some("mutable");
    std::thread::Builder::new().name("bounded-protocol-stack".into())
        .stack_size(2 * 1024 * 1024).spawn(move || {
            let calls = (0..=depth).map(|index| {
                let source = RollupId(if index % 2 == 0 {1} else {0});
                let target = RollupId(if index % 2 == 0 {0} else {1});
                ExecutedAction {
                    parent_index: index.checked_sub(1), call_mode: if mutable {CallMode::Mutable} else {CallMode::Static},
                    source_rollup_id: source, target_rollup_id: target,
                    source_address: Default::default(), target_address: Default::default(),
                    data: (index as u64).to_be_bytes().to_vec().into(), value: Default::default(),
                    outcome: ExecutionOutcome::Resolved {success:true, return_data: vec![0x77],gas_used:21_000},
                    revert_spans: Vec::new(),
                }
            }).collect();
            match materialize_actions(RollupId(1), calls) {
                Ok(mut value) => {
                    let host = &mut value.targets[0];
                    host.batch.entries[0].rootUpdates.push(RootUpdateSol {
                        rollupId: 1, currentRoot: Default::default(), newRoot: [1u8;32].into(),
                        etherDelta: EtherDeltaSol::ZERO,
                    });
                    finalize_l1_entry_traces(&mut host.batch, &host.entry_traces).unwrap();
                    let sources = value.source.batch.entries.into_iter().map(OutboundSourceEntry::Mutable)
                        .chain(value.source.l2_static_entries.into_iter().map(OutboundSourceEntry::Static)).collect::<Vec<_>>();
                    verify_outbound_source_group(&host.batch.entries[0], &sources, RollupId(1)).unwrap();
                    println!("depth={depth}, mutable={mutable}, materialized and independently verified");
                },
                Err(error) => println!("depth={depth}, rejected={error}"),
            }
        }).unwrap().join().unwrap();
}
