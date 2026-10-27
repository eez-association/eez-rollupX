//! History-sensitive Composer regression and opt-in replay of real Kurtosis captures.

use alloy_consensus::{Block, Transaction as _};
use alloy_primitives::{Address, B256, Bytes, Signature, U256, keccak256};
use alloy_provider::{Provider as _, ProviderBuilder};
use alloy_sol_types::SolValue as _;
use eez_control_rpc::v2::{self, client_frame, server_frame};
use eez_primitives::EezTxEnvelope;
use eez_protocol::{CallMode, ExecutionOutcome};
use eez_testkit::{ANVIL_KEY_1, ProofSignerConfig, ProofSignerHandle};
use prost::Message as _;
use std::{path::Path, time::Duration};

/// Reaches both Composer's local observation replay and the independent signer.
/// History must advance in Sync; a history-insensitive storage read would miss
/// the erroneous transaction-zero replay boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduled_history_static_and_lookup_settle() {
    use eez_testkit::{
        ANVIL_ATTESTER_KEY, OUTBOUND_USER, SETTLE_TIMEOUT, Scenario, ScenarioCall,
        assert_latest_batch_signature, call_read, deploy_combined_history_observation,
        onchain_nonce, read_state_word, safe_block_hash, setup_cross_chain_with_env, sign_and_send,
        signer_address, wait_for_receipt_or_failure,
    };
    let world = setup_cross_chain_with_env(&[("EEZ_TEST_HISTORY_UPDATE_OFFSET", "4".into())])
        .await
        .unwrap();
    let (probe, reader) = deploy_combined_history_observation(&world).await.unwrap();
    let l1 = world.l1_rpc();
    let l2 = world.l2_rpc();
    let ordinary = sign_and_send(
        &l2,
        OUTBOUND_USER,
        world.l2_chain_id,
        onchain_nonce(&l2, OUTBOUND_USER).await.unwrap(),
        Some(probe),
        U256::ZERO,
        keccak256("bump()")[..4].to_vec(),
        100_000,
    )
    .await
    .unwrap();
    assert!(
        wait_for_receipt_or_failure(&world.node, &l2, ordinary, SETTLE_TIMEOUT)
            .await
            .unwrap()
    );
    let read = |address, signature: &str| {
        call_read(address, signature, keccak256(signature)[..4].to_vec())
    };
    let mut run = keccak256("run(uint256)")[..4].to_vec();
    run.extend(U256::from(91).abi_encode());
    Scenario::new("Sync history, static and failed mutable lookup, delivery, static")
        .inbound(ScenarioCall::new(reader, run).with_gas_limit(3_000_000))
        .expect_l1_state(read(reader, "beforeMarker()"), 1u64)
        .expect_l1_state(read(reader, "afterMarker()"), 91u64)
        .expect_l1_state(read(reader, "runs()"), 1u64)
        .expect_l2_state(read(probe, "marker()"), 91u64)
        .expect_settled_fully()
        .run(&world)
        .await
        .unwrap();
    let provider = ProviderBuilder::new().connect_http(l2.parse().unwrap());
    let safe = safe_block_hash(&l2).await.unwrap().unwrap();
    let ordinary_height = provider
        .get_transaction_receipt(ordinary)
        .await
        .unwrap()
        .unwrap()
        .block_number
        .unwrap();
    let mut ancestor: serde_json::Value = provider
        .client()
        .request("eth_getBlockByHash", (safe, true))
        .await
        .unwrap();
    let height = |block: &serde_json::Value| {
        u64::from_str_radix(
            block["number"].as_str().unwrap().trim_start_matches("0x"),
            16,
        )
        .unwrap()
    };
    let count = height(&ancestor).checked_sub(ordinary_height).unwrap();
    assert!(count <= 4096, "bounded accepted ancestry");
    let native_type = format!("0x{:x}", eez_primitives::SYSTEM_TX_TYPE);
    let mut setter = keccak256("setMarker(uint256)")[..4].to_vec();
    setter.extend(U256::from(91).abi_encode());
    let mut delivery = None;
    for _ in 0..=count {
        let found = ancestor["transactions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tx| {
                if tx["type"] != native_type {
                    return false;
                }
                let native: eez_primitives::SystemTransaction =
                    serde_json::from_value(tx.clone()).unwrap();
                native.to == eez_primitives::EEZL2_ADDRESS
                    && eez_protocol::entries::decode_inbound(&native.input).is_some_and(|call| {
                        call.target == probe && call.data.as_ref() == setter && call.success
                    })
            });
        if found {
            delivery = Some(ancestor);
            break;
        }
        let parent = ancestor["parentHash"]
            .as_str()
            .unwrap()
            .parse::<B256>()
            .unwrap();
        ancestor = provider
            .client()
            .request("eth_getBlockByHash", (parent, true))
            .await
            .unwrap();
        assert_eq!(
            ancestor["hash"].as_str().unwrap().parse::<B256>().unwrap(),
            parent
        );
    }
    let block = delivery.expect("the real probe delivery must be in accepted safe ancestry");
    let first = &block["transactions"][0];
    assert_eq!(
        first["type"],
        format!("0x{:x}", eez_primitives::HISTORY_TX_TYPE)
    );
    let history: eez_primitives::HistoryTransaction =
        serde_json::from_value(first.clone()).unwrap();
    assert!(
        !history.hashes.is_empty(),
        "regression must observe an advancing history import"
    );
    let parent_number = u64::from_str_radix(
        block["number"].as_str().unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap()
        - 1;
    let before: Bytes = provider.client().request("eth_call", (
        serde_json::json!({"to":probe,"data":Bytes::copy_from_slice(&keccak256("observe()")[..4])}),
        format!("0x{parent_number:x}"),
    )).await.unwrap();
    let (_, old_number, _) = <(U256, u64, B256)>::abi_decode(&before).unwrap();
    let observed = read_state_word(&l1, &read(reader, "beforeNumber()"))
        .await
        .unwrap();
    assert_eq!(
        observed,
        U256::from(old_number + history.hashes.len() as u64)
    );
    let expected = (
        U256::from(1),
        observed.to::<u64>(),
        *history.hashes.last().unwrap(),
    )
        .abi_encode();
    assert_eq!(
        read_state_word(&l1, &read(reader, "lookupHash()"))
            .await
            .unwrap(),
        U256::from_be_bytes(keccak256(expected).0)
    );
    assert_latest_batch_signature(&l1, &world.dep, signer_address(ANVIL_ATTESTER_KEY).unwrap())
        .await
        .unwrap();
    world.node.assert_no_divergence_failure_logs();
}

fn decode_frames(path: &Path) -> Vec<v2::ClientFrame> {
    let encoded = std::fs::read(path).unwrap();
    let mut remaining = encoded.as_slice();
    let mut frames = Vec::new();
    while !remaining.is_empty() {
        frames.push(v2::ClientFrame::decode_length_delimited(&mut remaining).unwrap());
    }
    frames
}

fn settlement(frames: &mut [v2::ClientFrame]) -> &mut v2::SettlementInput {
    let Some(client_frame::Kind::Finalize(finalize)) = frames.last_mut().unwrap().kind.as_mut()
    else {
        panic!("capture must end in Finalize");
    };
    finalize.settlement.as_mut().unwrap()
}

fn write_frames(path: &Path, frames: &[v2::ClientFrame]) {
    let mut encoded = Vec::new();
    for frame in frames {
        frame.encode_length_delimited(&mut encoded).unwrap();
    }
    std::fs::write(path, encoded).unwrap();
}

/// A failed outcome must reach observation replay, so both the ABI and native
/// semantic carrier change together. Execution/witnesses/root pins stay exact.
fn replace_observation(frames: &mut [v2::ClientFrame], mode: CallMode, old: &[u8], new: &[u8]) {
    let wire = settlement(frames);
    let mut batch = eez_protocol::entries::decode_postbatch(&wire.abi_calldata).unwrap();
    let blobs = wire
        .blobs
        .iter()
        .enumerate()
        .map(|(i, blob)| eez_blob_codec::Blob::try_from_slice(i, blob).unwrap())
        .collect::<Vec<_>>();
    let native = eez_blob_codec::decode_native_payload(&blobs, 1).unwrap();
    let mut semantics =
        eez_protocol::blob_semantics::decode_semantic_transactions(&native.semantic_messages)
            .unwrap();
    let mut changed = 0;
    for action in semantics.iter_mut().flat_map(|tx| &mut tx.actions) {
        if action.call_mode == mode
            && let ExecutionOutcome::Resolved { return_data, .. } = &mut action.outcome
            && return_data.as_slice() == old
        {
            *return_data = new.to_vec();
            changed += 1;
        }
    }
    assert_eq!(
        changed, 1,
        "one selected ordinary-prefix semantic observation"
    );
    let mut changed = 0;
    if mode == CallMode::Static {
        for entry in &mut batch.staticEntries {
            if entry.returnData.as_ref() == old {
                entry.returnData = Bytes::copy_from_slice(new);
                changed += 1;
            }
        }
    } else {
        for entry in &mut batch.entries {
            if !entry.success && entry.returnData.as_ref() == old {
                entry.returnData = Bytes::copy_from_slice(new);
                changed += 1;
            }
        }
    }
    assert_eq!(changed, 1, "one selected ABI observation");
    let messages = semantics
        .iter()
        .flat_map(|tx| eez_protocol::blob_semantics::encode_semantic_transaction(tx).unwrap())
        .collect();
    wire.blobs = eez_blob_codec::encode_native_payload(1, native.operations, messages)
        .unwrap()
        .into_iter()
        .map(|blob| blob.into_bytes().into_vec())
        .collect();
    batch.blobIndices = (0..wire.blobs.len()).map(U256::from).collect();
    wire.abi_calldata = eez_protocol::entries::encode_postbatch(&batch);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an owned offset-5 Kurtosis capture and its retained genesis"]
async fn kurtosis_combined_history_observations_replay_offline() {
    let directory = std::path::PathBuf::from(std::env::var("EEZ_COMBINED_CAPTURE_DIR").unwrap());
    let identity: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("identity.json")).unwrap()).unwrap();
    let address = |field: &str| {
        identity[field]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap()
    };
    let probe = address("probe");
    let observe = &keccak256("observe()")[..4];
    let bump = &keccak256("bump()")[..4];
    let rpc = ProviderBuilder::new().connect_http(
        std::env::var("EEZ_COMBINED_L2_RPC")
            .unwrap()
            .parse()
            .unwrap(),
    );
    let safe: serde_json::Value = rpc
        .client()
        .request("eth_getBlockByNumber", ("safe", false))
        .await
        .unwrap();
    let safe_number = u64::from_str_radix(
        safe["number"].as_str().unwrap().trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let safe_hash = safe["hash"].as_str().unwrap().parse::<B256>().unwrap();
    let mut selected = None;
    for path in std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
    {
        if path.extension().is_none_or(|ext| ext != "protobuf") {
            continue;
        }
        let mut frames = decode_frames(&path);
        let blocks = frames
            .iter()
            .filter_map(|frame| match &frame.kind {
                Some(client_frame::Kind::Block(block)) => Some(block.data.as_ref().unwrap()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let Some(terminal) = blocks.last() else {
            continue;
        };
        let block: Block<EezTxEnvelope> = alloy_rlp::decode_exact(&terminal.rlp).unwrap();
        if !block
            .body
            .transactions
            .first()
            .is_some_and(EezTxEnvelope::is_history)
        {
            continue;
        }
        let Some(boundary) = block
            .body
            .transactions
            .iter()
            .position(EezTxEnvelope::is_cross_chain_system)
        else {
            continue;
        };
        let bumps = block.body.transactions[..boundary]
            .iter()
            .filter(|&tx| tx.to() == Some(probe) && tx.input().as_ref() == bump)
            .count();
        // Rich Sync production currently puts ordinary mempool writes in
        // preceding blocks. Do not manufacture an unproduced terminal body.
        let Some(first_bump) = blocks.iter().find_map(|wire| {
            let candidate: Block<EezTxEnvelope> = alloy_rlp::decode_exact(&wire.rlp).unwrap();
            candidate
                .body
                .transactions
                .iter()
                .any(|tx| tx.to() == Some(probe) && tx.input().as_ref() == bump)
                .then_some(candidate.header.number)
        }) else {
            continue;
        };
        let EezTxEnvelope::History(history) = &block.body.transactions[0] else {
            unreachable!("first transaction was checked above")
        };
        assert!(!history.hashes.is_empty());
        let parent_number = block.header.number - 1;
        let terminal_hash = B256::from_slice(&terminal.hash);
        let canonical: serde_json::Value = rpc
            .client()
            .request(
                "eth_getBlockByNumber",
                (format!("0x{:x}", block.header.number), false),
            )
            .await
            .unwrap();
        if canonical["hash"].as_str().unwrap().parse::<B256>().unwrap() != terminal_hash
            || block.header.number > safe_number
        {
            continue;
        }
        // Follow the accepted endpoint's parent hashes, not just a height
        // comparison with a possibly speculative captured terminal.
        assert!(
            safe_number - block.header.number <= 4096,
            "capture outside bounded replay ancestry"
        );
        let mut ancestor = safe.clone();
        for _ in block.header.number..safe_number {
            let parent = ancestor["parentHash"]
                .as_str()
                .unwrap()
                .parse::<B256>()
                .unwrap();
            ancestor = rpc
                .client()
                .request("eth_getBlockByHash", (parent, false))
                .await
                .unwrap();
            assert_eq!(
                ancestor["hash"].as_str().unwrap().parse::<B256>().unwrap(),
                parent
            );
        }
        if ancestor["hash"].as_str().unwrap().parse::<B256>().unwrap() != terminal_hash {
            continue;
        }
        let before: Bytes = rpc
            .client()
            .request(
                "eth_call",
                (
                    serde_json::json!({"to":probe,"data":Bytes::copy_from_slice(observe)}),
                    format!("0x{parent_number:x}"),
                ),
            )
            .await
            .unwrap();
        let before_tuple = <(U256, u64, B256)>::abi_decode(&before).unwrap();
        let wire = settlement(&mut frames);
        let blobs = wire
            .blobs
            .iter()
            .enumerate()
            .map(|(i, blob)| eez_blob_codec::Blob::try_from_slice(i, blob).unwrap())
            .collect::<Vec<_>>();
        let native = eez_blob_codec::decode_native_payload(&blobs, 1).unwrap();
        let semantics =
            eez_protocol::blob_semantics::decode_semantic_transactions(&native.semantic_messages)
                .unwrap();
        let observations = semantics
            .iter()
            .flat_map(|tx| &tx.actions)
            .filter(|action| {
                action.target_address == probe
                    && action.call_mode == CallMode::Static
                    && action.data.as_ref() == observe
            })
            .collect::<Vec<_>>();
        if observations.len() != 2 {
            continue;
        }
        let correct = observations[0].outcome.return_data().unwrap().to_vec();
        let after = observations[1].outcome.return_data().unwrap().to_vec();
        let correct_tuple = <(U256, u64, B256)>::abi_decode(&correct).unwrap();
        if correct_tuple.0 != before_tuple.0 + U256::from(bumps)
            || correct_tuple.1 <= before_tuple.1
        {
            continue;
        }
        assert_eq!(
            correct_tuple.1,
            before_tuple.1 + history.hashes.len() as u64
        );
        assert_eq!(Some(&correct_tuple.2), history.hashes.last());
        assert_ne!(correct, after, "real delivery changes observed state");
        let canonical_after: Bytes = rpc
            .client()
            .request(
                "eth_call",
                (
                    serde_json::json!({"to":probe,"data":Bytes::copy_from_slice(observe)}),
                    format!("0x{:x}", block.header.number),
                ),
            )
            .await
            .unwrap();
        assert_eq!(canonical_after.as_ref(), after.as_slice());
        let before_ordinary: Bytes = rpc
            .client()
            .request(
                "eth_call",
                (
                    serde_json::json!({"to":probe,"data":Bytes::copy_from_slice(observe)}),
                    format!("0x{:x}", first_bump - 1),
                ),
            )
            .await
            .unwrap();
        assert_ne!(before_ordinary.as_ref(), correct.as_slice());
        let metadata: serde_json::Value = serde_json::from_slice(
            &std::fs::read(path.with_extension("").with_extension("json")).unwrap(),
        )
        .unwrap();
        selected = Some((
            frames,
            before.to_vec(),
            before_ordinary.to_vec(),
            correct,
            after,
            metadata,
            serde_json::json!({"capture":path.file_name().unwrap(),"terminal_number":block.header.number,"terminal_hash":terminal_hash,
            "ordinary_prefix_transactions":boundary,"ordinary_bumps_in_terminal":bumps,"first_ordinary_bump_block":first_bump,
            "imported_headers":history.hashes.len(),"before":before_tuple,"correct":correct_tuple,
            "safe_number":safe_number,"safe_hash":safe_hash,"terminal_is_ancestor_of_safe":true}),
        ));
        break;
    }
    let (frames, before, before_ordinary, correct, after, metadata, selection) = selected.expect("Composer capture with ordinary window writes, advancing terminal history, observations and subsequent delivery");
    let output = directory.join("combined-regression");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::write(
        output.join("selection.json"),
        serde_json::to_vec_pretty(&selection).unwrap(),
    )
    .unwrap();
    let signer = ProofSignerHandle::spawn_network_denied(&ProofSignerConfig {
        rollup_manager_address: address("rollup_manager_address"),
        registry_address: address("registry_address"),
        chain_config: &directory.join("l2-genesis.json"),
        l1_chain_config: directory.join("l1-genesis.json").to_str().unwrap().into(),
        rollup_id: 1,
        signer_key: ANVIL_KEY_1,
        vkey: identity["vkey"].as_str().unwrap().parse().unwrap(),
        proof_system: address("proof_system"),
        max_request_blocks: Some(4096),
    })
    .await
    .unwrap();
    let mut client = v2::prover_client::ProverClient::connect(signer.endpoint().to_owned())
        .await
        .unwrap()
        .max_encoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES)
        .max_decoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES);
    let capabilities = client
        .capabilities(v2::CapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    let mut cases = vec![("correct".to_string(), frames.clone(), true)];
    for (name, mode) in [("static", CallMode::Static), ("lookup", CallMode::Mutable)] {
        for (state, outcome) in [
            ("before-history", &before),
            ("before-ordinary-window", &before_ordinary),
            ("after-effect", &after),
        ] {
            let mut modified = frames.clone();
            replace_observation(&mut modified, mode, &correct, outcome);
            cases.push((format!("{name}-{state}"), modified, false));
        }
    }
    let mut proofs = 0;
    let mut results = Vec::new();
    for (name, frames, accept) in cases {
        write_frames(&output.join(format!("{name}.v2.protobuf")), &frames);
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let mut request =
            tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(receiver));
        request.metadata_mut().insert_bin(
            "eez-validation-identity-bin",
            tonic::metadata::MetadataValue::from_bytes(&capabilities.validation_identity),
        );
        let mut responses = client.prove_stream(request).await.unwrap().into_inner();
        let mut session = Vec::new();
        let mut epoch = 0;
        let mut acks = 0;
        let mut finalized = false;
        for mut frame in frames {
            frame.session_id = session.clone();
            frame.epoch = epoch;
            let kind = frame.kind.clone().unwrap();
            let request_id = frame.request_id;
            sender.send(frame).await.unwrap();
            let response = tokio::time::timeout(Duration::from_secs(90), responses.message())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(response.request_id, request_id);
            match (kind, response.kind.unwrap()) {
                (client_frame::Kind::Begin(begin), server_frame::Kind::Ready(ready)) => {
                    assert_eq!(ready.validated_through, begin.anchor_number);
                    assert_eq!(ready.validated_hash, begin.anchor_hash);
                    session = response.session_id;
                    epoch = response.epoch;
                }
                (client_frame::Kind::Block(block), server_frame::Kind::Validated(ack)) => {
                    let block = block.data.unwrap();
                    assert_eq!(ack.hash, block.hash);
                    assert_eq!(ack.number, block.number);
                    acks += 1;
                }
                (client_frame::Kind::Finalize(_), server_frame::Kind::Proof(proof)) => {
                    let digest = B256::from_slice(&proof.public_inputs_hash);
                    assert!(accept, "wrong state signed: {name}");
                    assert_eq!(
                        format!("{digest:x}"),
                        metadata["public_inputs_hash"].as_str().unwrap()
                    );
                    assert_eq!(
                        Signature::try_from(proof.proof.as_slice())
                            .unwrap()
                            .recover_address_from_prehash(&digest)
                            .unwrap(),
                        address("attester")
                    );
                    proofs += 1;
                    finalized = true;
                }
                (client_frame::Kind::Finalize(_), server_frame::Kind::Rejected(rejection)) => {
                    assert!(!accept, "correct request rejected: {rejection:?}");
                    assert_eq!(rejection.code, i32::from(tonic::Code::FailedPrecondition));
                    finalized = true;
                }
                other => panic!("{name}: unexpected response: {other:?}"),
            }
        }
        assert!(finalized);
        assert!(acks > 0);
        assert_eq!(signer.streaming_attestations().unwrap(), proofs);
        results.push(serde_json::json!({"name":name,"accepted":accept,"validated_blocks":acks,"signatures_total":proofs}));
    }
    assert_eq!(proofs, 1);
    assert_eq!(
        signer
            .log_tail(5000)
            .lines()
            .filter(|line| line.contains("offline finalization rejected")
                && line.contains("observation_replay"))
            .count(),
        6
    );
    signer.assert_alive();
    std::fs::write(
        output.join("results.json"),
        serde_json::to_vec_pretty(&results).unwrap(),
    )
    .unwrap();
}
