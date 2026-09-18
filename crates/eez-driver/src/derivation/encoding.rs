//! Producer representation selection. This is not validation authority.

use alloy_eips::Encodable2718 as _;
use eez_payload_codec::derivable::{Environment, Payload, Record, RecordData};

use super::{Block, EthEvmConfig, SealedHeader, full_attributes, ordinary_environment};

/// Encode exact canonical history with explicit inputs and sparse empty gaps.
/// The parent is the block immediately before this range, including after an
/// anchor advance. The signer must independently execute and reconstruct it.
pub fn encode_operations(
    config: &EthEvmConfig,
    parent: &SealedHeader,
    block_rlps: &[Vec<u8>],
    entries: &[Vec<u8>],
    groups: &[u16],
) -> eyre::Result<Payload> {
    eyre::ensure!(!block_rlps.is_empty(), "operations range has no terminal");
    eyre::ensure!(
        block_rlps.len() <= 65_536,
        "operations range exceeds format ceiling"
    );
    let mut parent = parent.clone();
    let mut initial = None;
    let mut active = None;
    let mut gap = 0;
    let mut records = Vec::new();
    for (position, rlp) in block_rlps.iter().enumerate() {
        let block: Block = alloy_rlp::decode_exact(rlp)?;
        eyre::ensure!(
            alloy_rlp::encode(&block) == *rlp,
            "noncanonical producer block"
        );
        if position + 1 == block_rlps.len() {
            full_attributes(config, &parent, &block)?;
            break;
        }
        let data = match ordinary_environment(config, &parent, &block)? {
            Some(environment) => {
                let transactions = block
                    .body
                    .transactions
                    .iter()
                    .map(|tx| tx.encoded_2718().into())
                    .collect::<Vec<_>>();
                initial.get_or_insert_with(|| environment.clone());
                if active.as_ref().is_some_and(|old| *old != environment) {
                    active = Some(environment.clone());
                    Some(RecordData::EnvironmentChange {
                        environment,
                        transactions,
                    })
                } else {
                    active = Some(environment);
                    (!transactions.is_empty()).then_some(RecordData::Transactions(transactions))
                }
            }
            None => Some(RecordData::FullBlock(rlp.clone().into())),
        };
        if let Some(data) = data {
            records.push(Record {
                empty_gap: gap,
                data,
            });
            gap = 0;
        } else {
            gap += 1;
        }
        parent = SealedHeader::seal_slow(block.header);
    }
    Ok(Payload::new(
        block_rlps.len() as u64 - 1,
        initial.unwrap_or_else(Environment::unused),
        records,
        block_rlps.last().expect("nonempty range checked").clone(),
        entries.to_vec(),
        groups.to_vec(),
    )?)
}
