//! Independent admission of mandatory native semantic blob DA.

use alloy_primitives::{B256, Bytes, U256};
use eez_blob_codec::{Blob, blob_versioned_hashes, decode_native_payload};
use eez_protocol::blob_semantics::{SemanticTransaction, decode_semantic_transactions};

use super::CanonicalPostBatch;

/// Data availability bound to physical blobs and the configured rollup profile.
pub(crate) struct VerifiedDa {
    payload: Bytes,
    semantic_transactions: Vec<SemanticTransaction>,
    transaction_blob_hashes: Vec<B256>,
}

impl VerifiedDa {
    pub(crate) fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub(crate) fn transaction_blob_hashes(&self) -> &[B256] {
        &self.transaction_blob_hashes
    }

    pub(crate) fn semantic_transactions(&self) -> &[SemanticTransaction] {
        &self.semantic_transactions
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum BlobDaError {
    #[error("native blob DA requires at least one physical blob")]
    MissingBlobs,
    #[error("native blob batch must keep the ABI batch.callData field empty")]
    NonEmptyBatchCallData,
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

/// Verify legacy fixtures with the default transaction blob limit.
#[cfg(test)]
pub(crate) fn verify_da_carrier(
    batch: &CanonicalPostBatch,
    submitted_blobs: &[Vec<u8>],
    expected_rollup_id: u64,
) -> Result<VerifiedDa, BlobDaError> {
    verify_da_carrier_with_limit(
        batch,
        submitted_blobs,
        expected_rollup_id,
        eez_blob_codec::MAX_BLOBS_PER_TRANSACTION,
    )
}

/// Validate the DA carrier and recover the existing `eez-payload-codec`
/// payload. Batches must use the canonical native profile and `[0, 1, ...]`
/// blob indices; `batch.callData` is never an accepted DA carrier.
///
/// The caller resolves the maximum from its independently configured L1
/// chain profile and target context, never from a composer-supplied maximum.
pub(crate) fn verify_da_carrier_with_limit(
    batch: &CanonicalPostBatch,
    submitted_blobs: &[Vec<u8>],
    expected_rollup_id: u64,
    maximum: usize,
) -> Result<VerifiedDa, BlobDaError> {
    let batch = batch.as_batch();
    if submitted_blobs.is_empty() {
        return Err(BlobDaError::MissingBlobs);
    }
    if batch.blobIndices.is_empty() {
        return Err(BlobDaError::UnexpectedBlobs);
    }
    if !batch.callData.is_empty() {
        return Err(BlobDaError::NonEmptyBatchCallData);
    }
    if submitted_blobs.len() > maximum {
        return Err(BlobDaError::TooManyBlobs {
            actual: submitted_blobs.len(),
            maximum,
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
        semantic_transactions,
        transaction_blob_hashes,
    })
}
