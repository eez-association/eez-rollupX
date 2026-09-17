//! Conservative size of one block in the existing `Vec<Vec<u8>>` payload.
//! Unknown execution header fields are bounded; transaction encodings are exact.

use alloy_consensus::Header;
use alloy_eips::Encodable2718 as _;
use alloy_primitives::{Address, B64, B256, Bloom, Bytes, U256};
use alloy_rlp::Encodable;

use crate::derivable::Error;

type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug)]
pub struct BlockDaSizer {
    header_bytes: usize,
    transaction_bytes: usize,
    transaction_items: usize,
}

fn prefix(bytes: usize) -> usize {
    alloy_rlp::Header {
        list: true,
        payload_length: bytes,
    }
    .length()
}

impl BlockDaSizer {
    /// Bound either the full integer-list block item or all opaque transaction
    /// items for an already built block. Record/environment framing is charged
    /// separately by the interval reserve. This avoids assuming that a legacy
    /// transaction's new byte-string prefix is free.
    pub fn retained_item_bytes(encoded: &[u8]) -> Result<usize> {
        let block = alloy_rlp::decode_exact::<reth_ethereum_primitives::Block>(encoded)?;
        let full_items = encoded.iter().try_fold(0usize, |sum, byte| {
            sum.checked_add(byte.length()).ok_or(Error::Overflow)
        })?;
        let full = full_items
            .checked_add(prefix(full_items))
            .ok_or(Error::Overflow)?;
        let transaction_items = block.body.transactions.iter().try_fold(0usize, |sum, tx| {
            sum.checked_add(Bytes::from(tx.encoded_2718()).length())
                .ok_or(Error::Overflow)
        })?;
        Ok(full.max(transaction_items))
    }

    /// Bound every supported header field, including optional fields that may
    /// activate before the next boundary. EEZ supplies its fixed extra data and
    /// never produces ommers or nonempty withdrawals through ordinary sequencing.
    pub fn new(extra_data: &[u8]) -> Self {
        let header = Header {
            parent_hash: B256::repeat_byte(255),
            ommers_hash: B256::repeat_byte(255),
            beneficiary: Address::repeat_byte(255),
            state_root: B256::repeat_byte(255),
            transactions_root: B256::repeat_byte(255),
            receipts_root: B256::repeat_byte(255),
            logs_bloom: Bloom::repeat_byte(255),
            difficulty: U256::MAX,
            number: u64::MAX,
            gas_limit: u64::MAX,
            gas_used: u64::MAX,
            timestamp: u64::MAX,
            extra_data: Bytes::copy_from_slice(extra_data),
            mix_hash: B256::repeat_byte(255),
            nonce: B64::repeat_byte(255),
            base_fee_per_gas: Some(u64::MAX),
            withdrawals_root: Some(B256::repeat_byte(255)),
            blob_gas_used: Some(u64::MAX),
            excess_blob_gas: Some(u64::MAX),
            parent_beacon_block_root: Some(B256::repeat_byte(255)),
            requests_hash: Some(B256::repeat_byte(255)),
            block_access_list_hash: Some(B256::repeat_byte(255)),
            slot_number: Some(u64::MAX),
        };
        Self {
            header_bytes: header.length(),
            transaction_bytes: 0,
            transaction_items: 0,
        }
    }

    /// The transaction's canonical **block-body RLP**, including a typed
    /// transaction's RLP string wrapper, rather than just its EIP-2718 envelope.
    pub fn with_transaction(&self, transaction_rlp: &[u8]) -> Result<Self> {
        let mut body = transaction_rlp;
        let header = alloy_rlp::Header::decode(&mut body)?;
        if header.payload_length != body.len() {
            return Err(Error::Rlp(alloy_rlp::Error::UnexpectedLength));
        }
        let opaque_item = if header.list {
            transaction_rlp
                .len()
                .checked_add(prefix(transaction_rlp.len()))
                .ok_or(Error::Overflow)?
        } else {
            // Typed transactions already have a byte-string wrapper in a block.
            transaction_rlp.len()
        };
        let items = transaction_rlp.iter().try_fold(0usize, |sum, byte| {
            sum.checked_add(byte.length()).ok_or(Error::Overflow)
        })?;
        Ok(Self {
            header_bytes: self.header_bytes,
            transaction_bytes: self
                .transaction_bytes
                .checked_add(transaction_rlp.len())
                .ok_or(Error::Overflow)?,
            transaction_items: self
                .transaction_items
                .checked_add(items.max(opaque_item))
                .ok_or(Error::Overflow)?,
        })
    }

    /// Upper bound including the enclosing byte-vector RLP list. Header and
    /// variable list-prefix bytes may each expand to two bytes in that format.
    pub fn item_bytes(&self) -> Result<usize> {
        let tx_prefix = prefix(self.transaction_bytes);
        let raw_body = self
            .header_bytes
            .checked_add(tx_prefix)
            .and_then(|n| n.checked_add(self.transaction_bytes))
            // Empty ommers and withdrawals lists. Pre-Shanghai omits the latter.
            .and_then(|n| n.checked_add(2))
            .ok_or(Error::Overflow)?;
        let framing = self
            .header_bytes
            .checked_add(tx_prefix)
            .and_then(|n| n.checked_add(prefix(raw_body)))
            .and_then(|n| n.checked_add(2))
            .and_then(|n| n.checked_mul(2))
            .ok_or(Error::Overflow)?;
        let items = framing
            .checked_add(self.transaction_items)
            .ok_or(Error::Overflow)?;
        items.checked_add(prefix(items)).ok_or(Error::Overflow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::SignableTransaction;
    use alloy_consensus::{
        Block, BlockBody, TxEip1559, TxEip2930, TxEip7702, TxEnvelope, TxLegacy,
    };
    use alloy_primitives::{Signature, TxKind};

    #[test]
    fn bounds_the_actual_codec_for_typed_and_legacy_transactions() {
        let signature = Signature::new(U256::MAX, U256::MAX, true);
        for byte in [0, 1, 0x7f, 0x80, 0xff] {
            for length in [0, 1, 54, 55, 56, 255, 256, 1024, 65535, 128000] {
                let input = Bytes::from(vec![byte; length]);
                let transactions: Vec<TxEnvelope> = vec![
                    TxLegacy {
                        input: input.clone(),
                        to: TxKind::Create,
                        ..Default::default()
                    }
                    .into_signed(signature)
                    .into(),
                    TxEip2930 {
                        input: input.clone(),
                        ..Default::default()
                    }
                    .into_signed(signature)
                    .into(),
                    TxEip1559 {
                        input: input.clone(),
                        ..Default::default()
                    }
                    .into_signed(signature)
                    .into(),
                    TxEip7702 {
                        input,
                        ..Default::default()
                    }
                    .into_signed(signature)
                    .into(),
                ];
                for count in 0..=transactions.len() {
                    let txs = transactions[..count].to_vec();
                    let mut bound = BlockDaSizer::new(b"eez");
                    for tx in &txs {
                        bound = bound.with_transaction(&alloy_rlp::encode(tx)).unwrap();
                    }
                    for shanghai in [false, true] {
                        let block = Block::new(
                            Header {
                                extra_data: Bytes::from_static(b"eez"),
                                logs_bloom: Bloom::repeat_byte(byte),
                                number: u64::MAX,
                                timestamp: u64::MAX,
                                withdrawals_root: shanghai.then_some(B256::repeat_byte(byte)),
                                ..Default::default()
                            },
                            BlockBody {
                                transactions: txs.clone(),
                                ommers: vec![],
                                withdrawals: shanghai.then_some(Default::default()),
                            },
                        );
                        let actual = alloy_rlp::encode(&block).length();
                        assert!(
                            actual <= bound.item_bytes().unwrap(),
                            "byte={byte} length={length} count={count}: {actual}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn arithmetic_overflow_fails_closed() {
        let mut bound = BlockDaSizer::new(&[]);
        bound.transaction_bytes = usize::MAX;
        assert!(bound.with_transaction(&[1]).is_err());
        assert!(bound.item_bytes().is_err());
        bound.transaction_bytes = 0;
        bound.transaction_items = usize::MAX;
        assert!(bound.with_transaction(&[1]).is_err());
        assert!(bound.item_bytes().is_err());
    }
}
