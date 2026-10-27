//! Incremental V0 transaction bytes and a conservative per-block column bound.
//! Only actual metadata and transaction bytes contribute, never execution headers.

use crate::v0::{Error, MAX_TRANSACTIONS};
use alloy_eips::Encodable2718 as _;

type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug)]
pub struct BlockDaSizer {
    extra_data_bytes: usize,
    transaction_items: usize,
    transaction_count: usize,
}

fn item(bytes: usize) -> Result<usize> {
    let length = u32::try_from(bytes).map_err(|_| Error::TransactionLength)?;
    if length == 0 {
        return Err(Error::TransactionLength);
    }
    bytes
        .checked_add(crate::v0::varint_len(length))
        .ok_or(Error::Overflow)
}

fn ordinary(tx: &eez_primitives::EezTxEnvelope) -> Result<()> {
    use alloy_consensus::Typed2718;
    if tx.is_cross_chain_system() || tx.ty() == 3 {
        return Err(Error::UnsupportedTransaction);
    }
    Ok(())
}

impl BlockDaSizer {
    /// Bound one already-built ordinary block's contribution. Splitting every
    /// metadata/count run at block boundaries overestimates canonical merging.
    pub fn retained_item_bytes(encoded: &[u8]) -> Result<usize> {
        let block = alloy_rlp::decode_exact::<eez_primitives::Block>(encoded)
            .map_err(|_| Error::TransactionEncoding)?;
        let mut size = Self::new(&block.header.extra_data);
        for tx in &block.body.transactions {
            ordinary(tx)?;
            size = size.with_length(tx.encode_2718_len())?;
        }
        size.item_bytes()
    }

    pub fn new(extra_data: &[u8]) -> Self {
        Self {
            extra_data_bytes: extra_data.len(),
            transaction_items: 0,
            transaction_count: 0,
        }
    }

    /// Accept the builder's canonical block-body transaction RLP. Typed RLP
    /// wrappers are removed before charging V0's exact EIP-2718 length prefix.
    pub fn with_transaction(&self, transaction_rlp: &[u8]) -> Result<Self> {
        let tx = alloy_rlp::decode_exact::<eez_primitives::EezTxEnvelope>(transaction_rlp)
            .map_err(|_| Error::TransactionEncoding)?;
        ordinary(&tx)?;
        self.with_length(tx.encode_2718_len())
    }

    fn with_length(&self, bytes: usize) -> Result<Self> {
        if self.transaction_count >= MAX_TRANSACTIONS {
            return Err(Error::TransactionCount);
        }
        Ok(Self {
            transaction_count: self.transaction_count + 1,
            transaction_items: self
                .transaction_items
                .checked_add(item(bytes)?)
                .ok_or(Error::Overflow)?,
            ..*self
        })
    }

    /// Up to two count bytes, one beneficiary run and one extraData run, all
    /// of length one. Canonical empty/metadata merging can only reduce this.
    pub fn item_bytes(&self) -> Result<usize> {
        if self.extra_data_bytes > 32 {
            return Err(Error::ExtraData);
        }
        self.transaction_items
            .checked_add(25 + self.extra_data_bytes)
            .ok_or(Error::Overflow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope, TxLegacy};
    use alloy_primitives::{Address, Bytes, Signature, U256};

    #[test]
    fn bounds_actual_v0_columns_for_typed_and_legacy_transactions() {
        for length in [0, 1, 55, 56, 127, 128, 255, 256, 16_383, 126_976] {
            let signature = Signature::new(U256::from(1), U256::from(2), false);
            let txs: [TxEnvelope; 2] = [
                TxLegacy {
                    input: vec![0; length].into(),
                    ..Default::default()
                }
                .into_signed(signature)
                .into(),
                TxEip1559 {
                    input: vec![0; length].into(),
                    ..Default::default()
                }
                .into_signed(signature)
                .into(),
            ];
            let mut bound = BlockDaSizer::new(b"actual metadata");
            let mut encoded = Vec::<Bytes>::new();
            for tx in txs {
                bound = bound.with_transaction(&alloy_rlp::encode(&tx)).unwrap();
                encoded.push(tx.encoded_2718().into());
                let wire = crate::v0::encode(&[crate::v0::BlockRun {
                    blocks: 1,
                    beneficiary: Address::ZERO,
                    extra_data: b"actual metadata",
                    transactions: &encoded,
                }])
                .unwrap();
                assert!(
                    wire.len()
                        <= crate::v0::interval_payload_bound(1, bound.item_bytes().unwrap())
                            .unwrap()
                );
            }
        }
    }

    #[test]
    fn sequencing_reserve_enforces_pure_count_and_checked_arithmetic() {
        let mut size = BlockDaSizer::new(&[]);
        for _ in 0..MAX_TRANSACTIONS {
            size = size.with_length(100).unwrap();
        }
        assert_eq!(size.with_length(100).unwrap_err(), Error::TransactionCount);
        let mut size = BlockDaSizer::new(&[]);
        size.transaction_items = usize::MAX;
        assert_eq!(size.with_length(100).unwrap_err(), Error::Overflow);
        assert_eq!(size.item_bytes().unwrap_err(), Error::Overflow);
        assert_eq!(
            BlockDaSizer::new(&[0; 33]).item_bytes().unwrap_err(),
            Error::ExtraData
        );
        assert_eq!(
            BlockDaSizer::new(&[])
                .with_transaction(&[0xff])
                .unwrap_err(),
            Error::TransactionEncoding
        );
    }
}
