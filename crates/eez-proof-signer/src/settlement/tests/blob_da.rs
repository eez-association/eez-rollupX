use super::*;

fn physical_blobs(payload: Bytes) -> Vec<Vec<u8>> {
    eez_blob_codec::encode_native_payload(expected_rollup_id().get(), payload, Vec::new())
        .unwrap()
        .into_iter()
        .map(|blob| blob.into_bytes().into_vec())
        .collect()
}

fn physical_native_blobs(
    payload: Bytes,
    semantic_messages: Vec<eez_blob_codec::Message>,
) -> Vec<Vec<u8>> {
    eez_blob_codec::encode_native_payload(expected_rollup_id().get(), payload, semantic_messages)
        .unwrap()
        .into_iter()
        .map(|blob| blob.into_bytes().into_vec())
        .collect()
}

#[test]
fn nonempty_abi_call_data_and_missing_physical_blobs_are_rejected() {
    let mut batch = carrier_batch();
    batch.callData = Bytes::from_static(b"unexpected inline payload");
    assert!(matches!(
        verify_da_carrier(&batch, &[], expected_rollup_id().get()),
        Err(BlobDaError::MissingBlobs)
    ));
}

#[test]
fn network_aware_carrier_accepts_nine_blobs_and_preserves_exact_payload() {
    use crate::settlement::blob_da::verify_da_carrier_with_limit;
    let payload = Bytes::from(vec![0; 8 * eez_blob_codec::DATA_BYTES_PER_BLOB + 100]);
    let blobs = physical_blobs(payload.clone());
    assert_eq!(blobs.len(), 9);
    let mut batch = carrier_batch();
    batch.blobIndices = (0..blobs.len()).map(U256::from).collect();
    let verified =
        verify_da_carrier_with_limit(&batch, &blobs, expected_rollup_id().get(), 9).unwrap();
    assert_eq!(verified.payload(), payload.as_ref());
    assert_eq!(verified.transaction_blob_hashes().len(), 9);
    assert!(matches!(
        verify_da_carrier_with_limit(&batch, &blobs, expected_rollup_id().get(), 6),
        Err(BlobDaError::TooManyBlobs {
            actual: 9,
            maximum: 6
        })
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
fn native_blob_profile_recovers_cross_chain_semantics() {
    use eez_blob_codec::Message;

    let payload = Bytes::from_static(b"native semantic payload");
    let submitted = physical_native_blobs(
        payload.clone(),
        vec![
            Message::InitiateCrossChainTransaction {
                chain_id: 1,
                tx_data: Bytes::from_static(b"signed L2 tx"),
            },
            Message::Call {
                to_chain: 0,
                from_address: Address::repeat_byte(0x11),
                to_address: Address::repeat_byte(0x22),
                value: U256::from(7),
                gas: 0,
                data: Bytes::from_static(b"call"),
            },
            Message::ReturnSuccess {
                return_data: Bytes::from_static(b"result"),
            },
            Message::FinishCrossChainTransaction,
        ],
    );
    let mut batch = carrier_batch();
    batch.blobIndices = (0..submitted.len()).map(U256::from).collect();
    let verified = verify_da_carrier(&batch, &submitted, expected_rollup_id().get()).unwrap();
    assert_eq!(verified.payload(), payload.as_ref());
    let [transaction] = verified.semantic_transactions() else {
        panic!("expected one semantic transaction");
    };
    assert_eq!(transaction.origin_chain, RollupId(1));
    assert_eq!(transaction.tx_data, Bytes::from_static(b"signed L2 tx"));
    assert_eq!(transaction.actions.len(), 1);
    assert_eq!(transaction.actions[0].target_rollup_id, RollupId::MAINNET);
}

#[test]
fn native_blob_profile_rejects_semantics_outside_deployment_gas_profile() {
    use eez_blob_codec::Message;

    let submitted = physical_native_blobs(
        Bytes::from_static(b"payload"),
        vec![
            Message::InitiateCrossChainTransaction {
                chain_id: 1,
                tx_data: Bytes::new(),
            },
            Message::Call {
                to_chain: 0,
                from_address: Address::ZERO,
                to_address: Address::ZERO,
                value: U256::ZERO,
                gas: 1,
                data: Bytes::new(),
            },
            Message::ReturnSuccess {
                return_data: Bytes::new(),
            },
            Message::FinishCrossChainTransaction,
        ],
    );
    let mut batch = carrier_batch();
    batch.blobIndices = (0..submitted.len()).map(U256::from).collect();
    assert!(matches!(
        verify_da_carrier(&batch, &submitted, expected_rollup_id().get()),
        Err(BlobDaError::InvalidProfile(reason)) if reason.contains("expected zero")
    ));
}

#[test]
fn blob_profile_rejects_mixed_noncanonical_and_wrong_rollup_carriers() {
    let submitted = physical_blobs(Bytes::from_static(b"blob payload"));

    let mut mixed = carrier_batch();
    mixed.blobIndices = vec![U256::ZERO];
    mixed.callData = Bytes::from_static(b"mixed");
    assert!(matches!(
        verify_da_carrier(&mixed, &submitted, expected_rollup_id().get()),
        Err(BlobDaError::NonEmptyBatchCallData)
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
