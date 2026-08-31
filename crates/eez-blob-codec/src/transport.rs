//! KZG sidecar construction and EIP-4844 transaction encoding.
//!
//! This module intentionally requires callers to select the sidecar wrapper.
//! Fork selection belongs to chain configuration; silently guessing between
//! the legacy EIP-4844 proof wrapper and the Fulu EIP-7594 cell-proof wrapper
//! would make transaction propagation deployment-dependent.

use alloy_consensus::{
    SignableTransaction, Signed, TxEip4844, TxEip4844WithSidecar, transaction::TxHashRef as _,
};
use alloy_eips::{
    Decodable2718 as _, Encodable2718 as _,
    eip2930::AccessList,
    eip4844::{
        AsCkzg as _, Blob as AlloyBlob, env_settings::EnvKzgSettings, kzg_to_versioned_hash,
    },
    eip7594::{BlobTransactionSidecarEip7594, BlobTransactionSidecarVariant},
};
use alloy_network::TxSignerSync;
use alloy_primitives::{Address, B256, Bytes, Signature, TxHash, U256};

use crate::Blob;

/// EIP-7594 limits one type-3 transaction to six blobs.
///
/// Enforcing the current limit for both wrapper variants keeps batch sizing
/// deterministic across a fork transition and is stricter than older chains.
pub const MAX_BLOBS_PER_TRANSACTION: usize = 6;

/// Sidecar wire format used to propagate a type-3 transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlobSidecarEncoding {
    /// Original EIP-4844 wrapper: blobs, commitments, and one proof per blob.
    Eip4844,
    /// Fulu EIP-7594 wrapper: blobs, commitments, and cell proofs.
    Eip7594,
}

/// Fully specified, fee-resolved fields for a contract-calling blob transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobTransactionParams {
    pub chain_id: u64,
    pub nonce: u64,
    pub gas_limit: u64,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_blob_gas: u128,
    pub to: Address,
    pub value: U256,
    pub access_list: AccessList,
    pub input: Bytes,
}

/// Unsigned type-3 transaction carrying its validated propagation sidecar.
#[derive(Clone, Debug)]
pub struct PreparedBlobTransaction {
    transaction: TxEip4844WithSidecar<BlobTransactionSidecarVariant>,
    versioned_hashes: Vec<B256>,
    sidecar_encoding: BlobSidecarEncoding,
}

/// KZG material prepared before proving and reused when the final transaction
/// fields become available.
///
/// Preparing commitments and proofs can be expensive, especially for the
/// EIP-7594 cell-proof wrapper. Keeping the sidecar avoids doing that work once
/// for public-input hashes and again for submission.
#[derive(Clone, Debug)]
pub struct PreparedBlobSidecar {
    sidecar: BlobTransactionSidecarVariant,
    versioned_hashes: Vec<B256>,
    sidecar_encoding: BlobSidecarEncoding,
}

impl PreparedBlobSidecar {
    /// KZG versioned hashes in blob order.
    pub fn versioned_hashes(&self) -> &[B256] {
        &self.versioned_hashes
    }

    /// Selected propagation wrapper.
    pub const fn sidecar_encoding(&self) -> BlobSidecarEncoding {
        self.sidecar_encoding
    }
}

impl PreparedBlobTransaction {
    /// Transaction fields covered by the signature. The sidecar is not part of
    /// the signature, but its versioned hashes are.
    pub const fn transaction(&self) -> &TxEip4844 {
        &self.transaction.tx
    }

    /// KZG versioned hashes in transaction blob-index order.
    pub fn versioned_hashes(&self) -> &[B256] {
        &self.versioned_hashes
    }

    /// Selected propagation wrapper.
    pub const fn sidecar_encoding(&self) -> BlobSidecarEncoding {
        self.sidecar_encoding
    }
}

/// Signed pooled transaction ready for `eth_sendRawTransaction` or a builder
/// bundle API that accepts the EIP-4844 transaction-pool representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedBlobTransaction {
    /// Sidecar-independent Ethereum transaction hash.
    pub transaction_hash: TxHash,
    /// `0x03 || rlp([tx_payload_body, sidecar...])`.
    pub pooled_transaction: Bytes,
    /// KZG versioned hashes in transaction blob-index order.
    pub versioned_hashes: Vec<B256>,
    /// Propagation wrapper encoded in `pooled_transaction`.
    pub sidecar_encoding: BlobSidecarEncoding,
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("a blob transaction requires at least one blob")]
    EmptyBlobs,
    #[error(
        "blob transaction contains {actual} blobs; the supported transaction maximum is {maximum}"
    )]
    TooManyBlobs { actual: usize, maximum: usize },
    #[error("failed to construct or validate the KZG sidecar: {0}")]
    Kzg(#[from] c_kzg::Error),
    #[error("KZG sidecar validation failed: {0}")]
    InvalidSidecar(String),
    #[error("blob transaction signing failed: {0}")]
    Signing(String),
    #[error("invalid pooled blob transaction encoding: {0}")]
    InvalidEncoding(String),
}

/// Construct a KZG sidecar and its transaction-binding versioned hashes.
pub fn prepare_blob_sidecar(
    blobs: &[Blob],
    sidecar_encoding: BlobSidecarEncoding,
) -> Result<PreparedBlobSidecar, TransportError> {
    if blobs.is_empty() {
        return Err(TransportError::EmptyBlobs);
    }
    if blobs.len() > MAX_BLOBS_PER_TRANSACTION {
        return Err(TransportError::TooManyBlobs {
            actual: blobs.len(),
            maximum: MAX_BLOBS_PER_TRANSACTION,
        });
    }

    let alloy_blobs = blobs
        .iter()
        .map(|blob| AlloyBlob::from_slice(blob.as_ref()))
        .collect::<Vec<_>>();
    let settings = EnvKzgSettings::Default.get();
    let sidecar: BlobTransactionSidecarVariant = match sidecar_encoding {
        BlobSidecarEncoding::Eip4844 => {
            alloy_eips::eip4844::BlobTransactionSidecar::try_from_blobs_with_settings(
                alloy_blobs,
                settings,
            )?
            .into()
        }
        BlobSidecarEncoding::Eip7594 => {
            BlobTransactionSidecarEip7594::try_from_blobs_with_settings(alloy_blobs, settings)?
                .into()
        }
    };
    let versioned_hashes = sidecar.versioned_hashes().collect::<Vec<_>>();
    Ok(PreparedBlobSidecar {
        sidecar,
        versioned_hashes,
        sidecar_encoding,
    })
}

/// Compute the EIP-4844 versioned hash of every physical blob without
/// constructing propagation proofs.
///
/// This is the efficient verification path for blob bodies downloaded from a
/// beacon node: callers compare these independently derived hashes with the
/// hashes committed by the execution transaction.
pub fn blob_versioned_hashes(blobs: &[Blob]) -> Result<Vec<B256>, TransportError> {
    blob_kzg_commitments(blobs).map(|commitments| {
        commitments
            .iter()
            .map(|commitment| kzg_to_versioned_hash(commitment))
            .collect()
    })
}

/// Compute each physical blob's canonical 48-byte KZG commitment without
/// constructing propagation proofs.
pub fn blob_kzg_commitments(blobs: &[Blob]) -> Result<Vec<[u8; 48]>, TransportError> {
    let settings = EnvKzgSettings::Default.get();
    blobs
        .iter()
        .map(|blob| {
            let alloy_blob = AlloyBlob::from_slice(blob.as_ref());
            let commitment = settings.blob_to_kzg_commitment(alloy_blob.as_ckzg())?;
            Ok(commitment.to_bytes().into_inner())
        })
        .collect()
}

/// Bind a previously prepared sidecar to fully resolved transaction fields and
/// independently validate the resulting transaction/sidecar pair.
pub fn prepare_blob_transaction_with_sidecar(
    params: BlobTransactionParams,
    prepared: PreparedBlobSidecar,
) -> Result<PreparedBlobTransaction, TransportError> {
    let PreparedBlobSidecar {
        sidecar,
        versioned_hashes,
        sidecar_encoding,
    } = prepared;
    let settings = EnvKzgSettings::Default.get();

    let transaction = TxEip4844 {
        chain_id: params.chain_id,
        nonce: params.nonce,
        gas_limit: params.gas_limit,
        max_fee_per_gas: params.max_fee_per_gas,
        max_priority_fee_per_gas: params.max_priority_fee_per_gas,
        to: params.to,
        value: params.value,
        access_list: params.access_list,
        blob_versioned_hashes: versioned_hashes.clone(),
        max_fee_per_blob_gas: params.max_fee_per_blob_gas,
        input: params.input,
    };
    transaction
        .validate_blob(&sidecar, settings)
        .map_err(|error| TransportError::InvalidSidecar(error.to_string()))?;

    Ok(PreparedBlobTransaction {
        transaction: transaction.with_sidecar(sidecar),
        versioned_hashes,
        sidecar_encoding,
    })
}

/// Construct and independently validate a KZG sidecar, then bind its hashes to
/// an unsigned EIP-4844 transaction.
pub fn prepare_blob_transaction(
    params: BlobTransactionParams,
    blobs: &[Blob],
    sidecar_encoding: BlobSidecarEncoding,
) -> Result<PreparedBlobTransaction, TransportError> {
    let sidecar = prepare_blob_sidecar(blobs, sidecar_encoding)?;
    prepare_blob_transaction_with_sidecar(params, sidecar)
}

/// Sign a prepared type-3 transaction and encode the transaction-pool wrapper.
pub fn sign_blob_transaction<S>(
    mut prepared: PreparedBlobTransaction,
    signer: &S,
) -> Result<SignedBlobTransaction, TransportError>
where
    S: TxSignerSync<Signature>,
{
    let signature = signer
        .sign_transaction_sync(&mut prepared.transaction)
        .map_err(|error| TransportError::Signing(error.to_string()))?;
    let signed = prepared.transaction.into_signed(signature);
    let transaction_hash = *signed.tx_hash();
    let mut encoded = Vec::with_capacity(signed.encode_2718_len());
    signed.encode_2718(&mut encoded);

    Ok(SignedBlobTransaction {
        transaction_hash,
        pooled_transaction: encoded.into(),
        versioned_hashes: prepared.versioned_hashes,
        sidecar_encoding: prepared.sidecar_encoding,
    })
}

/// Recover the sidecar-independent Ethereum hash from an EIP-4844 pooled
/// transaction wrapper.
pub fn pooled_blob_transaction_hash(encoded: &[u8]) -> Result<TxHash, TransportError> {
    let mut input = encoded;
    let signed =
        Signed::<TxEip4844WithSidecar<BlobTransactionSidecarVariant>>::decode_2718(&mut input)
            .map_err(|error| TransportError::InvalidEncoding(error.to_string()))?;
    if !input.is_empty() {
        return Err(TransportError::InvalidEncoding(format!(
            "{} trailing bytes",
            input.len()
        )));
    }
    Ok(*signed.tx_hash())
}

#[cfg(test)]
mod tests {
    use alloy_primitives::address;
    use alloy_signer_local::PrivateKeySigner;

    use super::*;
    use crate::{Message, encode, pack};

    fn blob() -> Blob {
        let stream = encode(&[
            Message::ChainOperation {
                chain_id: 1,
                operations: Bytes::from_static(b"transport-test"),
            },
            Message::CloseBlobStream,
        ])
        .unwrap();
        pack(&stream.blob_portion).unwrap().remove(0)
    }

    fn params() -> BlobTransactionParams {
        BlobTransactionParams {
            chain_id: 31_337,
            nonce: 4,
            gas_limit: 500_000,
            max_fee_per_gas: 20,
            max_priority_fee_per_gas: 2,
            max_fee_per_blob_gas: 10,
            to: address!("1111111111111111111111111111111111111111"),
            value: U256::ZERO,
            access_list: AccessList::default(),
            input: Bytes::from_static(b"postBatch"),
        }
    }

    fn signer() -> PrivateKeySigner {
        "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
            .parse()
            .unwrap()
    }

    #[test]
    fn prepares_both_explicit_sidecar_encodings() {
        for encoding in [BlobSidecarEncoding::Eip4844, BlobSidecarEncoding::Eip7594] {
            let prepared = prepare_blob_transaction(params(), &[blob()], encoding).unwrap();
            assert_eq!(prepared.versioned_hashes().len(), 1);
            assert_eq!(
                prepared.transaction().blob_versioned_hashes,
                prepared.versioned_hashes()
            );
            assert_eq!(prepared.sidecar_encoding(), encoding);
        }
    }

    #[test]
    fn signs_and_round_trips_the_eip7594_pool_wrapper() {
        let signer = signer();
        let prepared =
            prepare_blob_transaction(params(), &[blob()], BlobSidecarEncoding::Eip7594).unwrap();
        let expected_hashes = prepared.versioned_hashes().to_vec();
        let signed = sign_blob_transaction(prepared, &signer).unwrap();

        assert_eq!(signed.pooled_transaction[0], 3);
        assert_eq!(signed.versioned_hashes, expected_hashes);
        let mut bytes = signed.pooled_transaction.as_ref();
        let decoded =
            Signed::<TxEip4844WithSidecar<BlobTransactionSidecarVariant>>::decode_2718(&mut bytes)
                .unwrap();
        assert!(bytes.is_empty());
        assert!(decoded.tx().sidecar.is_eip7594());
        assert_eq!(decoded.tx().tx.blob_versioned_hashes, expected_hashes);
        assert_eq!(*decoded.tx_hash(), signed.transaction_hash);
        assert_eq!(
            pooled_blob_transaction_hash(&signed.pooled_transaction).unwrap(),
            signed.transaction_hash
        );
    }

    #[test]
    fn rejects_empty_and_oversized_blob_sets() {
        assert!(matches!(
            prepare_blob_transaction(params(), &[], BlobSidecarEncoding::Eip7594),
            Err(TransportError::EmptyBlobs)
        ));

        let blobs = (0..=MAX_BLOBS_PER_TRANSACTION)
            .map(|_| blob())
            .collect::<Vec<_>>();
        assert!(matches!(
            prepare_blob_transaction(params(), &blobs, BlobSidecarEncoding::Eip7594),
            Err(TransportError::TooManyBlobs {
                actual: 7,
                maximum: MAX_BLOBS_PER_TRANSACTION
            })
        ));
    }

    #[test]
    fn transaction_hash_does_not_commit_to_sidecar_wrapper_version() {
        let signer = signer();
        let legacy = sign_blob_transaction(
            prepare_blob_transaction(params(), &[blob()], BlobSidecarEncoding::Eip4844).unwrap(),
            &signer,
        )
        .unwrap();
        let peer_das = sign_blob_transaction(
            prepare_blob_transaction(params(), &[blob()], BlobSidecarEncoding::Eip7594).unwrap(),
            &signer,
        )
        .unwrap();

        assert_eq!(legacy.transaction_hash, peer_das.transaction_hash);
        assert_ne!(legacy.pooled_transaction, peer_das.pooled_transaction);
        assert_ne!(
            legacy.pooled_transaction.len(),
            peer_das.pooled_transaction.len()
        );
        assert_ne!(legacy.sidecar_encoding, peer_das.sidecar_encoding);
    }
}
