//! Batch payload encode/decode per Rollup-1 spec §8.1.
//!
//! ```text
//!   payload := tagByte ‖ rlp([
//!     blockTxCounts,    # legacy tags: list of uint16, length ==
//!                       # toBlock - fromBlock + 1
//!                       # entry i = number of retained txs in block
//!                       # 0 means empty block
//!     transactions,     # list of bytes, block-major order
//!                       # the first blockTxCounts[0] entries belong to block fromBlock,
//!                       # the next blockTxCounts[1] entries to block fromBlock+1, etc.
//!                       # each entry is a standard RLP-encoded EVM tx
//!     l2_entries,       # list of bytes — ABI-encoded L2-shape
//!                       # ExecutionEntry the L2 system tx
//!                       # (executeIncomingCrossChainCall) consumes.
//!                       # Empty for arbitrary-call batches; populated
//!                       # for value-bearing (deposit/withdrawal) ones.
//!     outboundGroupSizes, # tag 0x01 only: number of source entries
//!                         # consumed by each outbound user transaction
//!   ])
//!
//!   tagByte:
//!     0x00  legacy three-field body
//!     0x01  grouped four-field body
//!     0x02  self-contained blocks: full canonical block RLP values replace
//!           the separate block counts and transactions fields; decoded counts
//!           and transactions are derived from those exact block bodies
//! ```
//!
//! `fromBlock` and `toBlock` are **not** in this encoding. Callers supply the
//! absolute block range from their surrounding protocol context; this codec
//! only preserves the number of covered blocks and each block's transaction
//! count.
//!
//! Decode invariants enforced (§8.3):
//!  - `sum(blockTxCounts) == len(transactions)`
//!  - `blockTxCounts[i]` fits in `u16`
//!
//! The codec does **not** validate per-tx fields (§8.4 — that's STF-level).

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use alloy_rlp::{Decodable, Encodable};

/// A raw EIP-2718 signed transaction. Opaque to this crate.
pub type RawTx = Vec<u8>;

/// Tag byte for the legacy three-field calldata format.
pub const TAG_CALLDATA: u8 = 0x00;
/// Calldata format carrying explicit outbound-root counts per signed user tx.
pub const TAG_GROUPED_CALLDATA: u8 = 0x01;
/// Self-contained format carrying each complete L2 block as consensus RLP.
pub const TAG_SELF_CONTAINED_BLOCKS: u8 = 0x02;

/// Convenience [`Result`] alias.
pub type CodecResult<T> = Result<T, CodecError>;

/// Error returned by encode / decode.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    /// Payload was empty or missing the tag byte.
    #[error("payload too short ({0} bytes)")]
    TooShort(usize),
    /// Tag byte didn't match the current format.
    #[error(
        "unsupported tag byte 0x{0:02x}; expected 0x{TAG_CALLDATA:02x}, \
         0x{TAG_GROUPED_CALLDATA:02x}, or 0x{TAG_SELF_CONTAINED_BLOCKS:02x}"
    )]
    UnsupportedTag(u8),
    /// A self-contained block is not exact canonical Ethereum block RLP.
    #[error("invalid self-contained block {index}: {reason}")]
    InvalidBlock { index: usize, reason: String },
    /// Adjacent self-contained blocks do not form one hash-linked sequence.
    #[error("self-contained block {index} is not contiguous with its predecessor")]
    NonContiguousBlock { index: usize },
    /// RLP decoding failed.
    #[error("rlp decode failed: {0}")]
    Rlp(#[from] alloy_rlp::Error),
    /// Bytes remained after the single tagged RLP body.
    #[error("payload has {0} trailing bytes")]
    TrailingBytes(usize),
    /// A `blockTxCounts[i]` didn't fit in `u16`.
    #[error("block tx count {0} at index {1} exceeds u16::MAX")]
    BlockTxCountOverflow(u64, usize),
    /// `sum(blockTxCounts) != transactions.len()`.
    #[error("tx-count mismatch: sum(blockTxCounts) = {expected}, transactions.len() = {got}")]
    TxCountMismatch { expected: u64, got: u64 },
}

/// A decoded batch payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBatch {
    /// Per-block transaction counts. Length = number of L2 blocks covered.
    ///
    /// For legacy tags these are the retained payload transactions. For the
    /// self-contained tag they count every transaction in the exact block body,
    /// including system transactions.
    pub block_tx_counts: Vec<u16>,
    /// Flat list of raw EIP-2718 signed transactions in block-major order.
    pub transactions: Vec<RawTx>,
    /// L2-shape `ExecutionEntry` bytes (ABI-encoded). Empty for
    /// arbitrary-call batches; populated for value-bearing ones
    /// (deposits/withdrawals), where the L1 settlement entry does not
    /// contain the exact L2 execution-table shape followers must rebuild.
    pub l2_entries: Vec<Vec<u8>>,
    /// Number of outbound source roots consumed by each terminal-block user
    /// transaction. Empty for legacy tag-0 payloads.
    pub outbound_group_sizes: Vec<u16>,
    /// Canonical full-block RLP, one value per block. Empty for legacy tags.
    ///
    /// This commits every original header field, transaction, ommer and
    /// withdrawal needed to reproduce and hash the exact block. Receipts and
    /// execution requests are deterministic STF outputs and are not duplicated.
    pub block_rlps: Vec<Vec<u8>>,
}

impl DecodedBatch {
    /// Number of L2 blocks covered.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.block_tx_counts.len()
    }
}

/// Encode the calldata payload: per-block tx vectors plus L2-shape
/// entries.
///
/// `blocks[i]` is the list of raw signed transactions for the i-th L2
/// block in the range. `l2_entries[i]` is one ABI-encoded
/// `ExecutionEntrySol`; pass `&[]` for arbitrary-call batches that
/// don't need follower-side L2 system-tx reconstruction.
///
/// # Errors
///
/// - [`CodecError::BlockTxCountOverflow`] if any block has > `u16::MAX` txs.
pub fn encode(blocks: &[Vec<RawTx>], l2_entries: &[Vec<u8>]) -> CodecResult<Vec<u8>> {
    let mut block_tx_counts: Vec<u16> = Vec::with_capacity(blocks.len());
    for (i, b) in blocks.iter().enumerate() {
        let n = u64::try_from(b.len()).unwrap_or(u64::MAX);
        let cnt = u16::try_from(b.len()).map_err(|_| CodecError::BlockTxCountOverflow(n, i))?;
        block_tx_counts.push(cnt);
    }
    let transactions: Vec<RawTx> = blocks.iter().flatten().cloned().collect();
    let body = Body {
        block_tx_counts,
        transactions,
        l2_entries: l2_entries.to_vec(),
    };
    let mut buf = Vec::with_capacity(1 + body.length());
    buf.push(TAG_CALLDATA);
    body.encode(&mut buf);
    Ok(buf)
}

/// Encode the grouped calldata format used when one signed outbound user
/// transaction consumes one or more source roots.
pub fn encode_grouped(
    blocks: &[Vec<RawTx>],
    l2_entries: &[Vec<u8>],
    outbound_group_sizes: &[u16],
) -> CodecResult<Vec<u8>> {
    let mut block_tx_counts: Vec<u16> = Vec::with_capacity(blocks.len());
    for (i, b) in blocks.iter().enumerate() {
        let n = u64::try_from(b.len()).unwrap_or(u64::MAX);
        block_tx_counts
            .push(u16::try_from(b.len()).map_err(|_| CodecError::BlockTxCountOverflow(n, i))?);
    }
    let body = GroupedBody {
        block_tx_counts,
        transactions: blocks.iter().flatten().cloned().collect(),
        l2_entries: l2_entries.to_vec(),
        outbound_group_sizes: outbound_group_sizes.to_vec(),
    };
    let mut buf = Vec::with_capacity(1 + body.length());
    buf.push(TAG_GROUPED_CALLDATA);
    body.encode(&mut buf);
    Ok(buf)
}

/// Encode the self-contained chain-operations profile.
///
/// Every element is one complete canonical Ethereum block RLP. Blocks must be
/// number- and parent-hash-contiguous. Transactions are recovered from those
/// block bodies during decoding rather than duplicated in a parallel field.
pub fn encode_self_contained(
    block_rlps: &[Vec<u8>],
    l2_entries: &[Vec<u8>],
    outbound_group_sizes: &[u16],
) -> CodecResult<Vec<u8>> {
    decode_self_contained_blocks(block_rlps)?;
    let body = SelfContainedBody {
        blocks: block_rlps.to_vec(),
        l2_entries: l2_entries.to_vec(),
        outbound_group_sizes: outbound_group_sizes.to_vec(),
    };
    let mut buf = Vec::with_capacity(1 + body.length());
    buf.push(TAG_SELF_CONTAINED_BLOCKS);
    body.encode(&mut buf);
    Ok(buf)
}

/// Decode a calldata payload.
///
/// # Errors
///
/// - [`CodecError::TooShort`] if `payload` is empty.
/// - [`CodecError::UnsupportedTag`] if the leading byte is not a supported tag.
/// - [`CodecError::Rlp`] if the RLP body is malformed.
/// - [`CodecError::TrailingBytes`] if bytes remain after the RLP body.
/// - [`CodecError::TxCountMismatch`] if `sum(blockTxCounts) != transactions.len()`.
pub fn decode(payload: &[u8]) -> CodecResult<DecodedBatch> {
    let Some((&tag, rest)) = payload.split_first() else {
        return Err(CodecError::TooShort(0));
    };
    let mut encoded_body = rest;
    let (block_tx_counts, transactions, l2_entries, outbound_group_sizes, block_rlps) = match tag {
        TAG_CALLDATA => {
            let body = Body::decode(&mut encoded_body)?;
            (
                body.block_tx_counts,
                body.transactions,
                body.l2_entries,
                Vec::new(),
                Vec::new(),
            )
        }
        TAG_GROUPED_CALLDATA => {
            let body = GroupedBody::decode(&mut encoded_body)?;
            (
                body.block_tx_counts,
                body.transactions,
                body.l2_entries,
                body.outbound_group_sizes,
                Vec::new(),
            )
        }
        TAG_SELF_CONTAINED_BLOCKS => {
            let body = SelfContainedBody::decode(&mut encoded_body)?;
            let (block_tx_counts, transactions) = decode_self_contained_blocks(&body.blocks)?;
            (
                block_tx_counts,
                transactions,
                body.l2_entries,
                body.outbound_group_sizes,
                body.blocks,
            )
        }
        _ => return Err(CodecError::UnsupportedTag(tag)),
    };
    if !encoded_body.is_empty() {
        return Err(CodecError::TrailingBytes(encoded_body.len()));
    }
    let expected: u64 = block_tx_counts.iter().map(|n| u64::from(*n)).sum();
    let got = transactions.len() as u64;
    if expected != got {
        return Err(CodecError::TxCountMismatch { expected, got });
    }
    Ok(DecodedBatch {
        block_tx_counts,
        transactions,
        l2_entries,
        outbound_group_sizes,
        block_rlps,
    })
}

#[derive(Debug, alloy_rlp::RlpEncodable, alloy_rlp::RlpDecodable)]
struct Body {
    block_tx_counts: Vec<u16>,
    transactions: Vec<RawTx>,
    l2_entries: Vec<Vec<u8>>,
}

#[derive(Debug, alloy_rlp::RlpEncodable, alloy_rlp::RlpDecodable)]
struct GroupedBody {
    block_tx_counts: Vec<u16>,
    transactions: Vec<RawTx>,
    l2_entries: Vec<Vec<u8>>,
    outbound_group_sizes: Vec<u16>,
}

#[derive(Debug, alloy_rlp::RlpEncodable, alloy_rlp::RlpDecodable)]
struct SelfContainedBody {
    blocks: Vec<Vec<u8>>,
    l2_entries: Vec<Vec<u8>>,
    outbound_group_sizes: Vec<u16>,
}

fn decode_self_contained_blocks(block_rlps: &[Vec<u8>]) -> CodecResult<(Vec<u16>, Vec<RawTx>)> {
    let mut block_tx_counts = Vec::with_capacity(block_rlps.len());
    let mut transactions = Vec::new();
    let mut previous: Option<(u64, alloy_primitives::B256)> = None;
    for (index, encoded) in block_rlps.iter().enumerate() {
        let block = alloy_rlp::decode_exact::<reth_ethereum_primitives::Block>(encoded).map_err(
            |error| CodecError::InvalidBlock {
                index,
                reason: error.to_string(),
            },
        )?;
        if alloy_rlp::encode(&block) != *encoded {
            return Err(CodecError::InvalidBlock {
                index,
                reason: "non-canonical RLP".to_owned(),
            });
        }
        if let Some((previous_number, previous_hash)) = previous
            && (block.header.number != previous_number.saturating_add(1)
                || block.header.parent_hash != previous_hash)
        {
            return Err(CodecError::NonContiguousBlock { index });
        }
        let count = u16::try_from(block.body.transactions.len()).map_err(|_| {
            CodecError::BlockTxCountOverflow(block.body.transactions.len() as u64, index)
        })?;
        block_tx_counts.push(count);
        transactions.extend(
            block
                .body
                .transactions
                .iter()
                .map(alloy_eips::Encodable2718::encoded_2718),
        );
        previous = Some((block.header.number, block.header.hash_slow()));
    }
    Ok((block_tx_counts, transactions))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(byte: u8, n: usize) -> RawTx {
        vec![byte; n]
    }

    #[test]
    fn roundtrip_single_block_with_two_txs() {
        let blocks = vec![vec![tx(0xa1, 64), tx(0xa2, 32)]];
        let bytes = encode(&blocks, &[]).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.block_tx_counts, vec![2u16]);
        assert_eq!(decoded.transactions.len(), 2);
        assert!(decoded.l2_entries.is_empty());
    }

    #[test]
    fn roundtrip_multi_block_with_empties() {
        let blocks = vec![
            vec![tx(0x01, 8)],
            vec![],
            vec![tx(0x02, 16), tx(0x03, 16), tx(0x04, 16)],
            vec![],
            vec![tx(0x05, 24)],
        ];
        let bytes = encode(&blocks, &[]).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.block_tx_counts, vec![1, 0, 3, 0, 1]);
        assert_eq!(decoded.transactions.len(), 5);
        assert_eq!(decoded.block_count(), 5);
    }

    #[test]
    fn tag_byte_is_first_byte() {
        let bytes = encode(&[vec![]], &[]).unwrap();
        assert_eq!(bytes[0], TAG_CALLDATA);
    }

    #[test]
    fn block_overflow_rejected_on_encode() {
        let blocks = vec![vec![tx(0, 1); usize::from(u16::MAX) + 1]];
        let err = encode(&blocks, &[]).unwrap_err();
        assert!(matches!(err, CodecError::BlockTxCountOverflow(_, 0)));
    }

    #[test]
    fn too_short_rejected_on_decode() {
        assert!(matches!(decode(&[]).unwrap_err(), CodecError::TooShort(_)));
    }

    #[test]
    fn unsupported_tag_rejected_on_decode() {
        assert!(matches!(
            decode(&[0xff]).unwrap_err(),
            CodecError::UnsupportedTag(0xff)
        ));
    }

    #[test]
    fn corrupt_rlp_rejected_on_decode() {
        assert!(matches!(
            decode(&[TAG_CALLDATA, 0x99, 0x99]).unwrap_err(),
            CodecError::Rlp(_)
        ));
    }

    #[test]
    fn trailing_bytes_rejected_on_decode() {
        for mut bytes in [
            encode(&[vec![]], &[]).unwrap(),
            encode_grouped(&[vec![]], &[], &[]).unwrap(),
        ] {
            bytes.extend_from_slice(&[0xde, 0xad]);
            assert!(matches!(
                decode(&bytes).unwrap_err(),
                CodecError::TrailingBytes(2)
            ));
        }
    }

    #[test]
    fn roundtrip_with_l2_entries() {
        // Value-bearing batch: L2 entries survive the round-trip
        // alongside per-block tx data.
        let blocks = vec![vec![], vec![tx(0xa1, 32)], vec![]];
        let l2_entries: Vec<Vec<u8>> = vec![
            vec![0xde, 0xad, 0xbe, 0xef],
            vec![0xca, 0xfe, 0xba, 0xbe, 0x00, 0x11, 0x22, 0x33],
        ];
        let bytes = encode(&blocks, &l2_entries).unwrap();
        assert_eq!(bytes[0], TAG_CALLDATA);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.block_tx_counts, vec![0u16, 1u16, 0u16]);
        assert_eq!(decoded.transactions.len(), 1);
        assert_eq!(decoded.l2_entries, l2_entries);
    }

    #[test]
    fn grouped_roundtrip_preserves_outbound_root_counts() {
        let blocks = vec![vec![tx(0xa1, 32), tx(0xa2, 32)]];
        let bytes =
            encode_grouped(&blocks, &[vec![0x01], vec![0x02], vec![0x03]], &[2, 1]).unwrap();
        assert_eq!(bytes[0], TAG_GROUPED_CALLDATA);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded.outbound_group_sizes, [2, 1]);
        assert_eq!(decoded.l2_entries.len(), 3);
    }

    fn self_contained_blocks() -> Vec<Vec<u8>> {
        use alloy_consensus::{BlockBody, Header};

        let first = reth_ethereum_primitives::Block {
            header: Header {
                number: 10,
                timestamp: 123,
                beneficiary: alloy_primitives::Address::repeat_byte(0x11),
                extra_data: alloy_primitives::Bytes::from_static(b"eez-test"),
                ..Default::default()
            },
            body: BlockBody::default(),
        };
        let second = reth_ethereum_primitives::Block {
            header: Header {
                parent_hash: first.header.hash_slow(),
                number: 11,
                timestamp: 125,
                beneficiary: alloy_primitives::Address::repeat_byte(0x22),
                ..Default::default()
            },
            body: BlockBody::default(),
        };
        vec![alloy_rlp::encode(first), alloy_rlp::encode(second)]
    }

    #[test]
    fn self_contained_roundtrip_preserves_exact_blocks_and_metadata() {
        let blocks = self_contained_blocks();
        let entries = vec![vec![0xde, 0xad]];
        let encoded = encode_self_contained(&blocks, &entries, &[2]).unwrap();
        assert_eq!(encoded[0], TAG_SELF_CONTAINED_BLOCKS);
        let decoded = decode(&encoded).unwrap();
        assert_eq!(decoded.block_rlps, blocks);
        assert_eq!(decoded.block_tx_counts, [0, 0]);
        assert!(decoded.transactions.is_empty());
        assert_eq!(decoded.l2_entries, entries);
        assert_eq!(decoded.outbound_group_sizes, [2]);
    }

    #[test]
    fn self_contained_blocks_must_be_hash_linked() {
        let mut blocks = self_contained_blocks();
        let mut second =
            alloy_rlp::decode_exact::<reth_ethereum_primitives::Block>(&blocks[1]).unwrap();
        second.header.parent_hash = alloy_primitives::B256::repeat_byte(0xff);
        blocks[1] = alloy_rlp::encode(second);
        assert!(matches!(
            encode_self_contained(&blocks, &[], &[]),
            Err(CodecError::NonContiguousBlock { index: 1 })
        ));
    }
}
