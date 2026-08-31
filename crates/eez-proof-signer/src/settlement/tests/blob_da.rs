use super::*;

fn physical_blobs(payload: Bytes) -> Vec<Vec<u8>> {
    eez_blob_codec::encode_compatibility_payload(expected_rollup_id().get(), payload)
        .unwrap()
        .into_iter()
        .map(|blob| blob.into_bytes().into_vec())
        .collect()
}

#[test]
fn legacy_calldata_remains_byte_for_byte_and_rejects_blob_claims() {
    let payload = Bytes::from_static(b"legacy payload");
    let mut batch = carrier_batch();
    batch.callData = payload.clone();
    let verified = verify_da_carrier(&batch, &[], expected_rollup_id().get()).unwrap();
    assert_eq!(verified.payload(), payload.as_ref());
    assert!(verified.transaction_blob_hashes().is_empty());

    batch.blobIndices = vec![U256::ZERO];
    assert!(matches!(
        verify_da_carrier(&batch, &[], expected_rollup_id().get()),
        Err(BlobDaError::MissingBlobs)
    ));
}

#[test]
fn canonical_blob_profile_recovers_payload_and_kzg_hashes() {
    let payload = Bytes::from_static(b"authenticated blob payload");
    let submitted = physical_blobs(payload.clone());
    let validated = submitted
        .iter()
        .enumerate()
        .map(|(index, blob)| eez_blob_codec::Blob::try_from_slice(index, blob).unwrap())
        .collect::<Vec<_>>();
    let expected_hashes = eez_blob_codec::blob_versioned_hashes(&validated).unwrap();
    let mut batch = carrier_batch();
    batch.blobIndices = (0..submitted.len()).map(U256::from).collect();

    let verified = verify_da_carrier(&batch, &submitted, expected_rollup_id().get()).unwrap();
    assert_eq!(verified.payload(), payload.as_ref());
    assert_eq!(verified.transaction_blob_hashes(), expected_hashes);
}

#[test]
fn blob_profile_rejects_mixed_noncanonical_and_wrong_rollup_carriers() {
    let submitted = physical_blobs(Bytes::from_static(b"blob payload"));

    let mut mixed = carrier_batch();
    mixed.blobIndices = vec![U256::ZERO];
    mixed.callData = Bytes::from_static(b"mixed");
    assert!(matches!(
        verify_da_carrier(&mixed, &submitted, expected_rollup_id().get()),
        Err(BlobDaError::MixedCalldataAndBlobs)
    ));

    let mut noncanonical = carrier_batch();
    noncanonical.blobIndices = vec![U256::from(1)];
    assert!(matches!(
        verify_da_carrier(&noncanonical, &submitted, expected_rollup_id().get()),
        Err(BlobDaError::NonCanonicalBlobIndices)
    ));

    let mut no_indices = carrier_batch();
    assert!(matches!(
        verify_da_carrier(&no_indices, &submitted, expected_rollup_id().get()),
        Err(BlobDaError::UnexpectedBlobs)
    ));

    no_indices.blobIndices = vec![U256::ZERO];
    assert!(matches!(
        verify_da_carrier(&no_indices, &submitted, 2),
        Err(BlobDaError::InvalidProfile(_))
    ));
}

#[test]
fn malformed_and_oversized_physical_blob_sets_are_rejected() {
    let mut batch = carrier_batch();
    batch.blobIndices = vec![U256::ZERO];
    assert!(matches!(
        verify_da_carrier(&batch, &[vec![0; 31]], expected_rollup_id().get()),
        Err(BlobDaError::InvalidBlob { index: 0, .. })
    ));

    let oversized = vec![vec![0; eez_blob_codec::BLOB_BYTES]; 7];
    batch.blobIndices = (0..oversized.len()).map(U256::from).collect();
    assert!(matches!(
        verify_da_carrier(&batch, &oversized, expected_rollup_id().get()),
        Err(BlobDaError::TooManyBlobs {
            actual: 7,
            maximum: 6
        })
    ));
}
