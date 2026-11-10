use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_sol_types::SolValue;
use eez_protocol::abi::{EtherDeltaSol, RootUpdateSol};
use eez_protocol::entries::{finalize_l1_entry_traces, verify_outbound_source_group};
use eez_protocol::system_tx::OutboundSourceEntry;
use eez_protocol::{CallMode, ExecutedAction, ExecutionOutcome, RollupId, materialize_actions};
#[test]
fn retry_matrix() {
    let mut lines = Vec::new();
    for len in 1..=4u32 {
        for code in 0..4usize.pow(len) {
            for scope in 0..=len as usize {
                let mut n = code;
                let calls = (0..len)
                    .map(|i| {
                        let k = n % 4;
                        n /= 4;
                        ExecutedAction {
                            parent_index: None,
                            call_mode: CallMode::Mutable,
                            source_rollup_id: RollupId(1),
                            target_rollup_id: RollupId::MAINNET,
                            source_address: Address::with_last_byte(1),
                            target_address: Address::with_last_byte(2),
                            data: Bytes::from(vec![(k / 2) as u8]),
                            value: U256::ZERO,
                            outcome: ExecutionOutcome::Resolved {
                                success: k % 2 == 0,
                                return_data: vec![(k % 2) as u8],
                                gas_used: 21000,
                            },
                            revert_spans: if i == 0 && scope > 0 {
                                vec![scope as u32]
                            } else {
                                vec![]
                            },
                        }
                    })
                    .collect();
                let result = (|| {
                    let mut c = match materialize_actions(RollupId(1), calls) {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("REJECT {len},{code},{scope}: {e}");
                            return None;
                        }
                    };
                    let h = &mut c.targets[0];
                    h.batch.entries[0].rootUpdates.push(RootUpdateSol {
                        rollupId: 1,
                        currentRoot: B256::with_last_byte(1),
                        newRoot: B256::with_last_byte(2),
                        etherDelta: EtherDeltaSol::ZERO,
                    });
                    finalize_l1_entry_traces(&mut h.batch, &h.entry_traces).ok()?;
                    let sources = c
                        .source
                        .batch
                        .entries
                        .iter()
                        .cloned()
                        .map(OutboundSourceEntry::Mutable)
                        .collect::<Vec<_>>();
                    verify_outbound_source_group(&h.batch.entries[0], &sources, RollupId(1))
                        .ok()?;
                    Some(format!(
                        "{len},{code},{scope}\t{}\t{}",
                        alloy_primitives::hex::encode(c.source.batch.abi_encode()),
                        alloy_primitives::hex::encode(h.batch.abi_encode())
                    ))
                })();
                if let Some(line) = result {
                    lines.push(line);
                }
            }
        }
    }
    eprintln!("accepted {} cases", lines.len());
    std::fs::write(std::env::var("MATRIX_OUTPUT").unwrap(), lines.join("\n")).unwrap();
}
