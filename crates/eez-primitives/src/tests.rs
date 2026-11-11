use super::*;
use alloy_consensus::{SignableTransaction, TxLegacy, TxReceipt};
use alloy_primitives::Signature;

fn system() -> EezTxEnvelope {
    SystemTransaction {
        chain_id: 1,
        source_hash: B256::repeat_byte(0x42),
        gas_limit: SYSTEM_TX_GAS_CAP,
        to: EEZL2_ADDRESS,
        value: U256::ZERO,
        input: Bytes::from_static(&[1, 2, 3, 4]),
    }
    .into()
}

#[test]
fn native_reference_vectors_are_canonical() {
    // Generated independently of the Rust implementation with ABI/RLP encoders
    // and Foundry. These prove the wire contract, not execution authorization.
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../eez-control-rpc/conformance/native-profile-v1.json"
    ))
    .unwrap();
    let mut checked = 0;
    for vector in corpus["vectors"].as_array().unwrap() {
        let id = vector["id"].as_str().unwrap();
        if !id.starts_with("native-history-") && !id.starts_with("native-source-") {
            continue;
        }
        let bytes =
            alloy_primitives::hex::decode(vector["expectedBytes"].as_str().unwrap()).unwrap();
        let tx = EezTxEnvelope::decode_2718_exact(&bytes)
            .unwrap_or_else(|error| panic!("{id}: {error}"));
        assert_eq!(tx.encoded_2718(), bytes, "{id}");
        assert_eq!(
            tx.tx_hash().to_string(),
            vector["expectedKeccak256"].as_str().unwrap(),
            "{id}"
        );
        assert_eq!(tx.recover_signer().unwrap(), SYSTEM_ADDRESS);
        checked += 1;
    }
    assert_eq!(checked, 4);
}

#[test]
fn engine_payloads_keep_native_transactions_and_empty_blob_bundles() {
    use alloy_rpc_types_engine::{
        BlobsBundleV1, BlobsBundleV2, ExecutionPayloadEnvelopeV3, ExecutionPayloadEnvelopeV4,
        ExecutionPayloadEnvelopeV5, ExecutionPayloadEnvelopeV6,
    };
    use reth_primitives_traits::SealedBlock;
    use std::sync::Arc;

    let block = Block::new(
        Default::default(),
        alloy_consensus::BlockBody {
            transactions: vec![system()],
            ..Default::default()
        },
    );
    let built = engine::EezBuiltPayload::new(
        Arc::new(SealedBlock::seal_slow(block).try_recover().unwrap()),
        U256::ZERO,
        None,
        Some(Bytes::from_static(&[0xc0])),
    );
    let v3 = ExecutionPayloadEnvelopeV3::try_from(built.clone()).unwrap();
    let v4 = ExecutionPayloadEnvelopeV4::try_from(built.clone()).unwrap();
    let v5 = ExecutionPayloadEnvelopeV5::try_from(built.clone()).unwrap();
    let v6 = ExecutionPayloadEnvelopeV6::try_from(built).unwrap();
    assert_eq!(v3.blobs_bundle, BlobsBundleV1::empty());
    assert_eq!(v4.envelope_inner.blobs_bundle, BlobsBundleV1::empty());
    assert_eq!(v5.blobs_bundle, BlobsBundleV2::empty());
    assert_eq!(v6.blobs_bundle, BlobsBundleV2::empty());
    assert_eq!(
        v3.execution_payload
            .payload_inner
            .payload_inner
            .transactions,
        vec![Bytes::from(system().encoded_2718())]
    );
    assert_eq!(v4.envelope_inner.execution_payload, v3.execution_payload);
    assert_eq!(v5.execution_payload, v3.execution_payload);
    assert_eq!(v6.execution_payload.payload_inner, v3.execution_payload);
}

#[test]
fn engine_block_conversion_preserves_prague_requests_hash() {
    use alloy_eips::eip7685::Requests;
    use reth_payload_primitives::PayloadTypes;
    use reth_primitives_traits::SealedBlock;

    for requests in [
        Requests::default(),
        Requests::new(vec![Bytes::from_static(&[0, 1])]),
    ] {
        let requests_hash = requests.requests_hash();
        let block = SealedBlock::seal_slow(Block::new(
            alloy_consensus::Header {
                base_fee_per_gas: Some(1),
                withdrawals_root: Some(alloy_consensus::proofs::calculate_withdrawals_root(&[])),
                blob_gas_used: Some(0),
                excess_blob_gas: Some(0),
                parent_beacon_block_root: Some(B256::ZERO),
                requests_hash: Some(requests_hash),
                ..Default::default()
            },
            alloy_consensus::BlockBody {
                withdrawals: Some(Default::default()),
                ..Default::default()
            },
        ));
        let expected_hash = block.hash();
        let data = engine::EezEngineTypes::block_to_payload(block, None);
        assert_eq!(data.sidecar.requests_hash(), Some(requests_hash));
        let restored = data
            .payload
            .try_into_block_with_sidecar::<EezTxEnvelope>(&data.sidecar)
            .unwrap();
        assert_eq!(restored.header.requests_hash, Some(requests_hash));
        assert_eq!(SealedBlock::seal_slow(restored).hash(), expected_hash);
    }
}

#[test]
fn native_wire_sender_hash_and_storage_round_trip() {
    let tx = system();
    let bytes = tx.encoded_2718();
    let expected = alloy_primitives::hex!(
        "45f84201a042424242424242424242424242424242424242424242424242424242424242428401000000944b2760f84b83d6b3829e30a7573b0172b6d6402c808401020304"
    );
    assert_eq!(bytes, expected);
    assert_eq!(tx.tx_hash(), &alloy_primitives::keccak256(expected));
    assert_eq!(tx.recover_signer().unwrap(), SYSTEM_ADDRESS);
    assert_eq!(tx.recover_signer_unchecked().unwrap(), SYSTEM_ADDRESS);
    assert_eq!(EezTxEnvelope::decode_2718_exact(&bytes).unwrap(), tx);
    assert_eq!(
        alloy_rlp::decode_exact::<EezTxEnvelope>(&alloy_rlp::encode(&tx)).unwrap(),
        tx
    );
    assert_eq!(
        <EezTxEnvelope as reth_codecs::Decompress>::decompress(
            &<EezTxEnvelope as reth_codecs::Compress>::compress(tx.clone())
        )
        .unwrap(),
        tx
    );
    assert!(reth_ethereum_primitives::TransactionSigned::decode_2718_exact(&bytes).is_err());
}

#[test]
fn rejects_malformed_and_trailing_native_bytes() {
    let bytes = system().encoded_2718();
    for len in 0..bytes.len() {
        assert!(EezTxEnvelope::decode_2718_exact(&bytes[..len]).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(EezTxEnvelope::decode_2718_exact(&trailing).is_err());
    let EezTxEnvelope::System(tx) = system() else {
        unreachable!()
    };
    let wrong_target = SystemTransaction {
        to: Address::ZERO,
        ..tx.into_inner()
    }
    .encoded_2718();
    assert!(EezTxEnvelope::decode_2718_exact(&wrong_target).is_err());
    assert!(<EezTxEnvelope as reth_codecs::Decompress>::decompress(&wrong_target).is_err());
}

#[test]
fn native_rejects_noncanonical_fields_extra_version_and_unknown_type() {
    let bytes = system().encoded_2718();
    let mut payload = &bytes[1..];
    let header = alloy_rlp::Header::decode(&mut payload).unwrap();
    assert!(header.list);
    let mut fields = Vec::<Bytes>::new();
    while !payload.is_empty() {
        fields.push(Bytes::decode(&mut payload).unwrap());
    }
    assert_eq!(fields.len(), 6);
    let encode = |fields: &[Bytes]| {
        let mut bytes = vec![SYSTEM_TX_TYPE];
        alloy_rlp::Header {
            list: true,
            payload_length: fields.iter().map(Encodable::length).sum(),
        }
        .encode(&mut bytes);
        for field in fields {
            field.encode(&mut bytes);
        }
        bytes
    };
    let reject = |bytes: &[u8]| {
        assert!(EezTxEnvelope::decode_2718_exact(bytes).is_err());
        assert!(<EezTxEnvelope as reth_codecs::Decompress>::decompress(bytes).is_err());
    };
    for (index, invalid) in [
        (0, vec![0, 1]),
        (0, vec![1; 9]),
        (1, vec![0; 31]),
        (1, vec![0; 33]),
        (2, vec![0, 1]),
        (2, vec![1; 9]),
        (2, vec![1, 0, 0, 1]),
        (3, vec![0; 19]),
        (3, vec![0; 21]),
        (4, vec![0]),
        (4, vec![1; 33]),
    ] {
        let mut malformed = fields.clone();
        malformed[index] = invalid.into();
        reject(&encode(&malformed));
    }
    reject(&encode(&fields[..5]));
    let mut versioned = fields.clone();
    versioned.insert(0, Bytes::from_static(&[1]));
    reject(&encode(&versioned));
    let mut extra = fields.clone();
    extra.push(Bytes::new());
    reject(&encode(&extra));
    let mut unknown_type = bytes;
    unknown_type[0] = 0x47;
    reject(&unknown_type);
}

#[test]
fn native_rpc_rejects_nonzero_placeholders_and_missing_consensus_fields() {
    let json = serde_json::to_value(system()).unwrap();
    for field in [
        "nonce",
        "gasPrice",
        "v",
        "r",
        "s",
        "yParity",
        "maxFeePerGas",
        "maxPriorityFeePerGas",
        "maxFeePerBlobGas",
    ] {
        let mut malformed = json.clone();
        malformed[field] = serde_json::json!("0x1");
        assert!(
            serde_json::from_value::<EezTxEnvelope>(malformed).is_err(),
            "{field}"
        );
    }
    for field in ["chainId", "sourceHash", "gas"] {
        let mut malformed = json.clone();
        malformed.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<EezTxEnvelope>(malformed).is_err(),
            "{field}"
        );
    }
    for field in ["accessList", "blobVersionedHashes", "authorizationList"] {
        let mut malformed = json.clone();
        malformed[field] = serde_json::json!([{}]);
        assert!(
            serde_json::from_value::<EezTxEnvelope>(malformed).is_err(),
            "{field}"
        );
    }
}

#[test]
fn native_receipt_matches_independent_reference() {
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../eez-control-rpc/conformance/native-profile-v1.json"
    ))
    .unwrap();
    let vector = corpus["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["id"] == "native-success-receipt")
        .unwrap();
    let bytes = alloy_primitives::hex::decode(vector["expectedBytes"].as_str().unwrap()).unwrap();
    let receipt = alloy_consensus::ReceiptWithBloom::<Receipt>::decode_2718_exact(&bytes).unwrap();
    assert_eq!(receipt.receipt.tx_type, EezTxType::System);
    assert!(receipt.receipt.success);
    assert_eq!(receipt.receipt.cumulative_gas_used, 21_000);
    assert_eq!(receipt.encoded_2718(), bytes);
    assert_eq!(
        alloy_primitives::keccak256(&bytes).to_string(),
        vector["expectedKeccak256"]
    );
}

#[test]
fn ethereum_bytes_and_sender_are_unchanged() {
    let eth: reth_ethereum_primitives::TransactionSigned = TxLegacy {
        chain_id: Some(1),
        nonce: 42,
        gas_price: 7,
        gas_limit: 21_000,
        to: TxKind::Call(Address::ZERO),
        value: U256::ONE,
        input: Bytes::new(),
    }
    .into_signed(Signature::new(U256::ONE, U256::ONE, false))
    .into();
    let tx = EezTxEnvelope::Ethereum(eth.clone());
    assert_eq!(tx.encoded_2718(), eth.encoded_2718());
    assert_eq!(alloy_rlp::encode(&tx), alloy_rlp::encode(&eth));
    assert_eq!(tx.recover_signer().unwrap(), eth.recover_signer().unwrap());
    assert_eq!(tx.tx_hash(), eth.tx_hash());
    assert_eq!(
        EezTxEnvelope::decode_2718_exact(&eth.encoded_2718()).unwrap(),
        tx
    );
}

#[test]
fn native_is_not_a_public_pooled_transaction() {
    type Pooled = alloy_consensus::EthereumTxEnvelope<alloy_consensus::TxEip4844WithSidecar>;
    assert!(Pooled::try_from(system()).is_err());
}

#[test]
fn rpc_serialization_has_native_execution_fields_and_zero_signature_placeholders() {
    let tx = system();
    let json = serde_json::to_value(&tx).unwrap();
    assert_eq!(json["type"], "0x45");
    assert!(json.get("version").is_none());
    assert_eq!(
        json["sourceHash"],
        serde_json::to_value(B256::repeat_byte(0x42)).unwrap()
    );
    assert_eq!(json["hash"], serde_json::to_value(tx.tx_hash()).unwrap());
    assert_eq!(json["chainId"], "0x1");
    assert_eq!(json["nonce"], "0x0");
    assert_eq!(json["gas"], "0x1000000");
    assert_eq!(json["gasPrice"], "0x0");
    assert_eq!(tx.gas_limit(), SYSTEM_TX_GAS_CAP);
    assert_eq!(tx.effective_gas_price(Some(100)), 0);
    let rpc = alloy_rpc_types_eth::Transaction::from_transaction(
        alloy_consensus::transaction::Recovered::new_unchecked(tx.clone(), SYSTEM_ADDRESS),
        alloy_consensus::transaction::TransactionInfo {
            base_fee: Some(100),
            ..Default::default()
        },
    );
    let rpc_json = serde_json::to_value(&rpc).unwrap();
    assert_eq!(rpc_json["gas"], "0x1000000");
    assert_eq!(rpc_json["gasPrice"], "0x0");
    assert_eq!(
        rpc_json["from"],
        serde_json::to_value(SYSTEM_ADDRESS).unwrap()
    );
    for field in ["v", "r", "s", "yParity"] {
        assert_eq!(json[field], "0x0");
        assert_eq!(rpc_json[field], "0x0");
    }
    for field in ["accessList", "blobVersionedHashes", "authorizationList"] {
        assert_eq!(json[field], serde_json::json!([]));
        assert_eq!(rpc_json[field], serde_json::json!([]));
    }
    for field in ["maxFeePerGas", "maxPriorityFeePerGas", "maxFeePerBlobGas"] {
        assert!(json.get(field).is_none());
        assert!(rpc_json.get(field).is_none());
    }
    assert_eq!(serde_json::from_value::<EezTxEnvelope>(json).unwrap(), tx);
}

#[test]
fn receipt_type_survives_consensus_and_database_encoding() {
    use reth_codecs::Compact;
    for ty in [
        EezTxType::System,
        EezTxType::History,
        EezTxType::Ethereum(alloy_consensus::TxType::Legacy),
        EezTxType::Ethereum(alloy_consensus::TxType::Eip1559),
    ] {
        for (success, logs) in [
            (false, vec![]),
            (
                true,
                vec![alloy_primitives::Log::new_unchecked(
                    EEZL2_ADDRESS,
                    vec![B256::repeat_byte(7)],
                    Bytes::from(vec![1; 256]),
                )],
            ),
        ] {
            let receipt = Receipt {
                tx_type: ty,
                success,
                cumulative_gas_used: 42_000,
                logs,
            };
            let mut stored = Vec::new();
            let len = receipt.to_compact(&mut stored);
            assert_eq!(Receipt::from_compact(&stored, len).0, receipt);
            let receipt = alloy_consensus::ReceiptWithBloom {
                logs_bloom: receipt.bloom(),
                receipt,
            };
            let encoded = receipt.encoded_2718();
            assert_eq!(
                alloy_consensus::ReceiptWithBloom::<Receipt>::decode_2718_exact(&encoded).unwrap(),
                receipt
            );
            if matches!(ty, EezTxType::System | EezTxType::History) {
                assert_eq!(encoded[0], ty.ty());
            }
        }
    }
}

#[test]
fn native_budget_is_the_remaining_block_gas_bounded_by_the_osaka_cap() {
    assert_eq!(SYSTEM_TX_GAS_CAP, 1 << 24);
    assert_eq!(system_tx_gas_limit(30_000_000, 0), SYSTEM_TX_GAS_CAP);
    assert_eq!(
        system_tx_gas_limit(30_000_000, 30_000_000 - SYSTEM_TX_GAS_CAP),
        SYSTEM_TX_GAS_CAP
    );
    assert_eq!(system_tx_gas_limit(30_000_000, 20_000_000), 10_000_000);
    assert_eq!(system_tx_gas_limit(30_000_000, 30_000_000), 0);
    // A corrupt cumulative figure above the limit leaves no budget, never wraps.
    assert_eq!(system_tx_gas_limit(30_000_000, u64::MAX), 0);
}

#[test]
fn history_has_a_distinct_bounded_envelope_and_fixed_rpc_context() {
    for count in [1, 64] {
        let tx = HistoryTransaction {
            version: HISTORY_TX_VERSION,
            chain_id: 1337,
            blocks: (1..=count)
                .map(|number| HistoryBlock {
                    number: u64::from(number),
                    block_hash: B256::with_last_byte(number),
                })
                .collect(),
        };
        let envelope = EezTxEnvelope::from(tx.clone());
        let raw = envelope.encoded_2718();
        let decoded = EezTxEnvelope::decode_2718_exact(&raw).unwrap();
        assert_eq!(decoded, envelope);
        assert!(decoded.is_history());
        assert!(!decoded.is_cross_chain_system());
        assert_eq!(decoded.recover_signer().unwrap(), SYSTEM_ADDRESS);
        assert_eq!(decoded.to(), Some(HISTORY_ADDRESS));
        assert_eq!(decoded.value(), U256::ZERO);
        assert!(
            <alloy_consensus::EthereumTxEnvelope<alloy_consensus::TxEip4844WithSidecar>>::try_from(
                decoded.clone()
            )
            .is_err()
        );
        let json = serde_json::to_value(&decoded).unwrap();
        assert!(json.get("sourceHash").is_none());
        assert_eq!(
            serde_json::from_value::<EezTxEnvelope>(json.clone()).unwrap(),
            decoded
        );
        for (field, value) in [
            ("sourceHash", serde_json::json!(B256::ZERO)),
            ("to", serde_json::json!(Address::ZERO)),
            ("value", serde_json::json!("0x1")),
            ("gas", serde_json::json!("0x1")),
            ("input", serde_json::json!("0x01")),
            ("nonce", serde_json::json!("0x1")),
            ("maxFeePerGas", serde_json::json!("0x0")),
        ] {
            let mut bad = json.clone();
            bad[field] = value;
            assert!(
                serde_json::from_value::<EezTxEnvelope>(bad).is_err(),
                "{field}"
            );
        }
        for len in 0..raw.len() {
            assert!(EezTxEnvelope::decode_2718_exact(&raw[..len]).is_err());
        }
        let mut trailing = raw.clone();
        trailing.push(0);
        assert!(EezTxEnvelope::decode_2718_exact(&trailing).is_err());
        assert_eq!(
            <EezTxEnvelope as reth_codecs::Decompress>::decompress(&raw).unwrap(),
            decoded
        );
    }
    for (version, hashes) in [
        (0, vec![B256::repeat_byte(1)]),
        (2, vec![B256::repeat_byte(1)]),
        (1, vec![]),
        (1, vec![B256::ZERO]),
        (1, vec![B256::repeat_byte(1); 65]),
    ] {
        let raw = HistoryTransaction {
            version,
            chain_id: 1,
            blocks: hashes
                .into_iter()
                .enumerate()
                .map(|(index, block_hash)| HistoryBlock {
                    number: index as u64 + 1,
                    block_hash,
                })
                .collect(),
        }
        .encoded_2718();
        assert!(EezTxEnvelope::decode_2718_exact(&raw).is_err());
    }
    // The former 0x45 history destination is invalid, even with zero value.
    let EezTxEnvelope::System(old) = system() else {
        unreachable!()
    };
    let mut old = old.into_inner();
    old.to = HISTORY_ADDRESS;
    old.gas_limit = HISTORY_TX_GAS_CAP;
    assert!(EezTxEnvelope::decode_2718_exact(&old.encoded_2718()).is_err());
}

#[test]
fn history_rejects_noncanonical_integers_hash_shapes_and_extra_fields() {
    let encode = |fields: &[Vec<u8>]| {
        let mut raw = vec![HISTORY_TX_TYPE];
        alloy_rlp::Header {
            list: true,
            payload_length: fields.iter().map(Vec::len).sum(),
        }
        .encode(&mut raw);
        for field in fields {
            raw.extend(field);
        }
        raw
    };
    let valid_hashes = alloy_rlp::encode(vec![HistoryBlock {
        number: 1,
        block_hash: B256::repeat_byte(1),
    }]);
    for fields in [
        vec![vec![0x81, 1], vec![1], valid_hashes.clone()],
        vec![vec![1], vec![0x82, 0, 1], valid_hashes.clone()],
        vec![
            vec![1],
            vec![0x89, 1, 1, 1, 1, 1, 1, 1, 1, 1],
            valid_hashes.clone(),
        ],
        vec![
            vec![1],
            vec![1],
            alloy_rlp::encode(vec![Bytes::from(vec![1; 31])]),
        ],
        vec![
            vec![1],
            vec![1],
            alloy_rlp::encode(vec![Bytes::from(vec![1; 33])]),
        ],
        vec![vec![1], vec![1], alloy_rlp::encode(B256::repeat_byte(1))],
        vec![vec![1], vec![1], valid_hashes, vec![0x80]],
    ] {
        assert!(EezTxEnvelope::decode_2718_exact(&encode(&fields)).is_err());
    }
}

#[test]
fn payload_identity_binds_exact_history_choice_even_when_randao_is_unchanged() {
    use reth_payload_primitives::PayloadAttributes as _;
    let mut attrs = engine::EezPayloadAttributes::from(alloy_rpc_types_engine::PayloadAttributes {
        timestamp: 100,
        prev_randao: B256::repeat_byte(1),
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(Vec::new()),
        parent_beacon_block_root: Some(B256::ZERO),
        slot_number: None,
        target_gas_limit: None,
    });
    let parent = B256::repeat_byte(2);
    let empty = attrs.payload_id(&parent);
    attrs.history_transaction = Some(Bytes::from_static(&[HISTORY_TX_TYPE, 1]));
    let first = attrs.payload_id(&parent);
    attrs.history_transaction = Some(Bytes::from_static(&[HISTORY_TX_TYPE, 2]));
    let other = attrs.payload_id(&parent);
    assert_ne!(empty, first);
    assert_ne!(first, other);
    attrs.no_tx_pool = true;
    assert_ne!(other, attrs.payload_id(&parent));
}

#[test]
fn history_pairs_reject_zero_nonincreasing_and_oversized_rpc_inputs() {
    for numbers in [vec![0], vec![1, 1], vec![2, 1], (1..=65).collect()] {
        let tx = HistoryTransaction {
            version: 1,
            chain_id: 1337,
            blocks: numbers
                .into_iter()
                .map(|number| HistoryBlock {
                    number,
                    block_hash: B256::repeat_byte(1),
                })
                .collect(),
        };
        assert!(EezTxEnvelope::decode_2718_exact(&tx.encoded_2718()).is_err());
        assert!(
            serde_json::from_value::<HistoryTransaction>(serde_json::to_value(tx).unwrap())
                .is_err()
        );
    }
    let tx = HistoryTransaction {
        version: 1,
        chain_id: 1337,
        blocks: vec![
            HistoryBlock {
                number: 100,
                block_hash: B256::repeat_byte(1),
            },
            HistoryBlock {
                number: u64::MAX,
                block_hash: B256::repeat_byte(2),
            },
        ],
    };
    assert_eq!(
        HistoryTransaction::decode_2718_exact(&tx.encoded_2718()).unwrap(),
        tx
    );
    assert_eq!(
        serde_json::from_value::<HistoryTransaction>(serde_json::to_value(&tx).unwrap()).unwrap(),
        tx
    );
}
