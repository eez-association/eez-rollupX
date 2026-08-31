//! Independent admission of legacy calldata or native semantic blob DA.

use alloy_primitives::{B256, Bytes, U256};
use eez_blob_codec::{
    Blob, MAX_BLOBS_PER_TRANSACTION, blob_versioned_hashes, decode_native_payload,
};
use eez_protocol::blob_semantics::{SemanticTransaction, decode_semantic_transactions};

use super::CanonicalPostBatch;

/// Data availability bound to physical blobs and the configured rollup profile.
pub(crate) struct VerifiedDa {
    payload: Bytes,
    semantic_transactions: Option<Vec<SemanticTransaction>>,
    transaction_blob_hashes: Vec<B256>,
}

impl VerifiedDa {
    pub(crate) fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub(crate) fn transaction_blob_hashes(&self) -> &[B256] {
        &self.transaction_blob_hashes
    }

    /// Native semantic transactions for blob DA, or `None` for the explicitly
    /// supported legacy calldata carrier.
    pub(crate) fn semantic_transactions(&self) -> Option<&[SemanticTransaction]> {
        self.semantic_transactions.as_deref()
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BlobDaError {
    #[error("legacy calldata batch declares blobIndices but supplies no physical blobs")]
    MissingBlobs,
    #[error("blob batch must keep batch.callData empty")]
    MixedCalldataAndBlobs,
    #[error("physical blobs were supplied but batch.blobIndices is empty")]
    UnexpectedBlobs,
    #[error("blob batch supplies {actual} blobs; maximum is {maximum}")]
    TooManyBlobs { actual: usize, maximum: usize },
    #[error("blobIndices must select every supplied transaction blob exactly once in order")]
    NonCanonicalBlobIndices,
    #[error("physical blob {index} is invalid: {reason}")]
    InvalidBlob { index: usize, reason: String },
    #[error("native blob profile is invalid: {0}")]
    InvalidProfile(String),
    #[error("KZG commitment derivation failed: {0}")]
    Kzg(String),
}

/// Validate the DA carrier and recover the existing `eez-payload-codec`
/// payload. Blob batches are deliberately restricted to the canonical native
/// profile and `[0, 1, ...]` blob indices. Legacy calldata has no standardized
/// semantic stream and is marked explicitly as such.
pub(crate) fn verify_da_carrier(
    batch: &CanonicalPostBatch,
    submitted_blobs: &[Vec<u8>],
    expected_rollup_id: u64,
) -> Result<VerifiedDa, BlobDaError> {
    let batch = batch.as_batch();
    if submitted_blobs.is_empty() {
        if !batch.blobIndices.is_empty() {
            return Err(BlobDaError::MissingBlobs);
        }
        return Ok(VerifiedDa {
            payload: batch.callData.clone(),
            semantic_transactions: None,
            transaction_blob_hashes: Vec::new(),
        });
    }
    if batch.blobIndices.is_empty() {
        return Err(BlobDaError::UnexpectedBlobs);
    }
    if !batch.callData.is_empty() {
        return Err(BlobDaError::MixedCalldataAndBlobs);
    }
    if submitted_blobs.len() > MAX_BLOBS_PER_TRANSACTION {
        return Err(BlobDaError::TooManyBlobs {
            actual: submitted_blobs.len(),
            maximum: MAX_BLOBS_PER_TRANSACTION,
        });
    }
    let expected_indices = (0..submitted_blobs.len()).map(U256::from);
    if !batch.blobIndices.iter().copied().eq(expected_indices) {
        return Err(BlobDaError::NonCanonicalBlobIndices);
    }

    let blobs = submitted_blobs
        .iter()
        .enumerate()
        .map(|(index, bytes)| {
            Blob::try_from_slice(index, bytes).map_err(|error| BlobDaError::InvalidBlob {
                index,
                reason: error.to_string(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let native = decode_native_payload(&blobs, expected_rollup_id)
        .map_err(|error| BlobDaError::InvalidProfile(error.to_string()))?;
    let semantic_transactions = decode_semantic_transactions(&native.semantic_messages)
        .map_err(|error| BlobDaError::InvalidProfile(error.to_string()))?;
    let transaction_blob_hashes =
        blob_versioned_hashes(&blobs).map_err(|error| BlobDaError::Kzg(error.to_string()))?;
    Ok(VerifiedDa {
        payload: native.operations,
        semantic_transactions: Some(semantic_transactions),
        transaction_blob_hashes,
    })
}
