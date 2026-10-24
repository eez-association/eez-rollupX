//! Producer projection into the sole direct V0 operations format.
//! Actions and their authorization remain in the enclosing native EEZ stream.

use alloy_eips::Encodable2718 as _;
use alloy_primitives::Bytes;
use eez_payload_codec::v0::{self, BlockRun, PayloadSizer};

use super::{Block, EezEvmConfig, SealedHeader, TransactionSigned, full_attributes};

/// The ordinary prefix ends at the first native transaction. Remaining signed
/// transactions are outbound origin actions, whose exact bytes are carried by
/// the semantic stream and independently bound by the signer and follower.
/// This projection does not authorize that suffix or its execution.
pub fn ordinary_prefix(block: &Block) -> &[TransactionSigned] {
    let end = block
        .body
        .transactions
        .iter()
        .position(TransactionSigned::is_cross_chain_system)
        .unwrap_or(block.body.transactions.len());
    &block.body.transactions[..end]
}

/// Exact V0 contribution of one block, including terminal ordinary metadata.
/// Execution validity and authorization of the omitted suffix are separate.
pub fn operations_size(block: &Block) -> eyre::Result<PayloadSizer> {
    let transactions = ordinary_prefix(block)
        .iter()
        .map(|tx| Bytes::from(tx.encoded_2718()))
        .collect::<Vec<_>>();
    Ok(PayloadSizer::run(&BlockRun {
        blocks: 1,
        beneficiary: block.header.beneficiary,
        extra_data: &block.header.extra_data,
        transactions: &transactions,
    })?)
}

/// Encode every position from its actual metadata and ordinary transactions.
/// No block headers, environment overrides, sidecars or group arrays are
/// transported. Consumers derive the fixed fields from authenticated context
/// and execute every block before checking its settlement commitment.
pub fn encode_operations(
    config: &EezEvmConfig,
    parent: &SealedHeader,
    block_rlps: &[Vec<u8>],
) -> eyre::Result<Vec<u8>> {
    eyre::ensure!(!block_rlps.is_empty(), "operations range has no terminal");
    u32::try_from(block_rlps.len())?;
    let mut parent = parent.clone();
    let mut blocks = Vec::with_capacity(block_rlps.len());
    let mut transactions = Vec::with_capacity(block_rlps.len());
    for (position, rlp) in block_rlps.iter().enumerate() {
        let block: Block = alloy_rlp::decode_exact(rlp)?;
        eyre::ensure!(
            alloy_rlp::encode(&block) == *rlp,
            "noncanonical producer block"
        );
        full_attributes(config, &parent, &block)?;
        eez_protocol::l2::validate_header(&block.header)?;
        eyre::ensure!(
            block.body.withdrawals.as_ref().is_none_or(|w| w.is_empty()),
            "nonempty L2 withdrawals"
        );
        let prefix = ordinary_prefix(&block);
        eyre::ensure!(
            position + 1 == block_rlps.len() || prefix.len() == block.body.transactions.len(),
            "native transaction before terminal position"
        );
        transactions.push(
            prefix
                .iter()
                .map(|tx| Bytes::from(tx.encoded_2718()))
                .collect::<Vec<_>>(),
        );
        parent = SealedHeader::seal_slow(block.header.clone());
        blocks.push(block);
    }
    let runs = blocks
        .iter()
        .zip(&transactions)
        .map(|(block, transactions)| BlockRun {
            blocks: 1,
            beneficiary: block.header.beneficiary,
            extra_data: &block.header.extra_data,
            transactions,
        })
        .collect::<Vec<_>>();
    Ok(v0::encode(&runs)?)
}
