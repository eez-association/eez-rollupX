//! Independent test writer for the cold-path checkpoint.
//! This posts a real signer-authorized native blob batch and
//! starts a clean follower with L1/DA and genesis only.

use prost::Message as _;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use alloy_consensus::Header;
use alloy_eips::Encodable2718 as _;
use alloy_primitives::{Address, B256, Bytes, Signature, U256};
use alloy_provider::{Provider as _, ProviderBuilder};
use eez_blob_codec::{
    BlobSidecarEncoding, BlobTransactionParams, prepare_blob_sidecar,
    prepare_blob_transaction_with_sidecar, sign_blob_transaction,
};
use eez_control_rpc::v2::{self, ClientFrame, L1PostingContext, client_frame, server_frame};
use eez_control_rpc::v2::{BlockWitness, ExecutionWitness, prover_client::ProverClient};
use eez_payload_codec::v0::{self, BlockRun};
use eez_primitives::Block;
use eez_protocol::abi::{
    EtherDeltaSol, ExecutionEntrySol, RollupIdWithProofSystemsSol, RootUpdateSol,
};
use eez_protocol::l1_history::{
    EvidenceAnchor, HeaderChain, HeaderRules, OfflineContext, OfflineProfile,
};
use eez_testkit::{
    NativeL1Profile, NodeBinary, NodeConfig, NodeHandle, batches_posted, receipt_ok,
    setup_cross_chain_with_profile, signer_address, state_root, wait_for, wait_for_latest_height,
    wait_for_safe_prefix_convergence,
};

const TIMEOUT: Duration = Duration::from_mins(3);

mod multipart;

/// Independent dense test writer: only actual metadata and exact ordinary
/// transaction bytes. The signer must reconstruct all omitted outputs.
fn encode(_parent: &Header, blocks: &[Block]) -> Vec<u8> {
    let transactions = blocks
        .iter()
        .map(|block| {
            block
                .body
                .transactions
                .iter()
                .map(|tx| Bytes::from(tx.encoded_2718()))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    v0::encode(
        &blocks
            .iter()
            .zip(&transactions)
            .map(|(block, transactions)| BlockRun {
                blocks: 1,
                beneficiary: block.header.beneficiary,
                extra_data: &block.header.extra_data,
                transactions,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap()
}

async fn native_replay(profile: NativeL1Profile, history_ceiling: Option<u64>) {
    let requested_history = history_ceiling.map(|ceiling| ceiling / 5 * 5);
    let build_started = Instant::now();
    let world = setup_cross_chain_with_profile(
        profile,
        &[
            ("EEZ_TEST_L2_DEBUG_RPC", "true".into()),
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
            (
                "EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS",
                history_ceiling
                    .map_or(900_000, |count| (count + 3_600) * 1_000)
                    .to_string(),
            ),
            ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "65536".into()),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "65540".into()),
            (
                "EEZ_TEST_L2_GENESIS_AGE_SECONDS",
                requested_history.unwrap_or(0).to_string(),
            ),
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "100000000000".into()),
            (
                "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS",
                "100000000000".into(),
            ),
            ("EEZ_COMPOSER_EXPECT_EXTERNAL_BATCHES", "true".into()),
            ("RUST_LOG", "info".into()),
        ],
    )
    .await
    .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    wait_for_latest_height(
        &world.node,
        history_ceiling.unwrap_or(12),
        if requested_history.is_some() {
            Duration::from_mins(40)
        } else {
            TIMEOUT
        },
    )
    .await
    .unwrap();
    if history_ceiling.is_none() {
        assert_eq!(
            batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
                .await
                .unwrap(),
            0
        );
    }
    let genesis = l2
        .get_block_by_number(0.into())
        .full()
        .await
        .unwrap()
        .unwrap()
        .map_transactions(|tx| tx.inner.into_inner())
        .into_consensus();
    let follower_env = world.partial_follower_env();
    assert!(
        !follower_env
            .iter()
            .any(|(name, _)| *name == "EEZ_SEQUENCER_RPC")
    );
    let read_setting = |name: &str| {
        follower_env
            .iter()
            .rev()
            .find(|(key, _)| *key == name)
            .unwrap()
            .1
            .clone()
    };
    let step = read_setting("EEZ_L2_BLOCK_TIME_MS").parse::<u64>().unwrap() / 1000;
    let l1_step = read_setting("EEZ_L1_BLOCK_TIME_MS").parse::<u64>().unwrap() / 1000;
    let k = l1_step / step;
    let latest = l2.get_block_number().await.unwrap();
    let terminal = native_grid_boundary(history_ceiling.unwrap_or(latest), k);
    eprintln!(
        "native history produced: profile={profile:?}, ceiling={history_ceiling:?}, terminal={terminal}, build_ms={}",
        build_started.elapsed().as_millis()
    );
    let path = world.witness_db_path().to_owned();
    let store =
        tokio::task::spawn_blocking(move || Arc::new(eez_node::WitnessDb::open(&path).unwrap()))
            .await
            .unwrap();
    let hash = l2
        .get_block_by_number(terminal.into())
        .await
        .unwrap()
        .unwrap()
        .header
        .hash;
    let blocks = load_history(
        Arc::clone(&store),
        terminal,
        hash,
        genesis.header.hash_slow(),
    )
    .await;
    let ceilings = history_ceiling.map_or_else(
        || vec![None],
        |maximum| {
            let mut values = [512, 1_024, 4_096, 10_000, 43_200, 65_536]
                .into_iter()
                .filter(|value| *value <= maximum)
                .map(Some)
                .collect::<Vec<_>>();
            if values.last() != Some(&Some(maximum)) {
                values.push(Some(maximum));
            }
            values
        },
    );
    let mut failures = Vec::new();
    let last_checkpoint = ceilings.len().saturating_sub(1);
    for (checkpoint, ceiling) in ceilings.into_iter().enumerate() {
        let mut count = ceiling.map_or(blocks.len(), |count| {
            native_grid_boundary(count, k) as usize
        });
        while blocks[count - 1]
            .body
            .transactions
            .iter()
            .any(|tx| !tx.is_history())
        {
            count = count
                .checked_sub(k as usize)
                .filter(|count| *count != 0)
                .expect("an empty legal prefix must exist");
        }
        if let Err(error) = prove_native_window(
            &world,
            profile,
            &genesis.header,
            &blocks[..count],
            ceiling,
            &follower_env,
            l1_step,
            k,
            Arc::clone(&store),
            checkpoint == last_checkpoint,
        )
        .await
        {
            failures.push((ceiling, error.code(), error.message().to_owned()));
        }
    }
    assert!(
        failures.is_empty(),
        "native cold checkpoint failures: {failures:?}"
    );
}

struct NativeProofContext<'a> {
    endpoint: &'a str,
    profile: NativeL1Profile,
    k: u64,
    rollup_id: u64,
    proof_system: Address,
    attester: Address,
    l1_chain_id: u64,
    l1_rpc: &'a str,
    l1_step: u64,
    registry: Address,
    manager: Address,
    offline_profile: OfflineProfile,
    header_rules: HeaderRules,
}

async fn load_history(
    store: Arc<eez_node::WitnessDb>,
    terminal: u64,
    hash: B256,
    genesis: B256,
) -> Vec<Block> {
    load_range(store, 0, terminal, hash, genesis, TIMEOUT).await
}

async fn load_range(
    store: Arc<eez_node::WitnessDb>,
    parent_number: u64,
    terminal: u64,
    mut hash: B256,
    parent_hash: B256,
    capture_timeout: Duration,
) -> Vec<Block> {
    tokio::task::spawn_blocking(move || {
        let mut blocks = Vec::new();
        let deadline = Instant::now() + capture_timeout;
        for number in (parent_number + 1..=terminal).rev() {
            let witness = loop {
                if let Some(witness) = store.get(number, hash).unwrap() {
                    break witness;
                }
                assert!(
                    Instant::now() < deadline,
                    "native witness {number}/{hash} was not captured"
                );
                std::thread::sleep(Duration::from_millis(10));
            };
            let block: Block = alloy_rlp::decode_exact(&witness.rlp).unwrap();
            assert_eq!(block.header.number, number);
            assert_eq!(block.header.hash_slow(), hash);
            hash = block.header.parent_hash;
            if number.is_multiple_of(4096) {
                eprintln!("native durable inputs remaining: {number}/{terminal}");
            }
            blocks.push(block);
        }
        assert_eq!(hash, parent_hash);
        blocks.reverse();
        blocks
    })
    .await
    .unwrap()
}

async fn exchange(
    sender: &tokio::sync::mpsc::Sender<ClientFrame>,
    responses: &mut tonic::Streaming<v2::ServerFrame>,
    frame: ClientFrame,
) -> Result<v2::ServerFrame, tonic::Status> {
    let id = frame.request_id;
    sender
        .send(frame)
        .await
        .map_err(|_| tonic::Status::unavailable("test stream closed"))?;
    let response = tokio::time::timeout(Duration::from_mins(10), responses.message())
        .await
        .map_err(|_| tonic::Status::deadline_exceeded("native v2 response timeout"))??
        .ok_or_else(|| tonic::Status::unavailable("native v2 response missing"))?;
    assert_eq!(response.request_id, id);
    if let Some(server_frame::Kind::Rejected(rejection)) = &response.kind {
        return Err(tonic::Status::with_details(
            tonic::Code::from_i32(rejection.code),
            &rejection.message,
            rejection.details.clone().into(),
        ));
    }
    Ok(response)
}

async fn prove_cold_window(
    context: &NativeProofContext<'_>,
    parent: &Header,
    blocks: &[Block],
    history_ceiling: Option<u64>,
    store: Arc<eez_node::WitnessDb>,
    actual_target: Option<L1PostingContext>,
) -> Result<
    (
        eez_protocol::EvmBatch,
        eez_blob_codec::PreparedBlobSidecar,
        L1PostingContext,
    ),
    tonic::Status,
> {
    let terminal = &blocks.last().unwrap().header;
    let operations = encode(parent, blocks);
    let operations_bytes = operations.len();
    let blobs = eez_blob_codec::encode_native_payload(context.rollup_id, operations.into(), vec![])
        .unwrap();
    let l1 = ProviderBuilder::new().connect_http(context.l1_rpc.parse().unwrap());
    let head = wait_for(TIMEOUT, || async {
        Ok(l1
            .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
            .await?
            .filter(|head| head.header.timestamp >= terminal.timestamp && head.header.number != 0))
    })
    .await
    .unwrap();
    let anchor = if let Some(target) = &actual_target {
        assert_eq!(target.parent_number, head.header.number);
        assert_eq!(target.parent_hash.as_slice(), head.header.hash.as_slice());
        EvidenceAnchor::ActualParent {
            number: head.header.number,
            hash: head.header.hash,
            target_timestamp: target.target_timestamp,
        }
    } else {
        EvidenceAnchor::Historical {
            number: head.header.number,
            hash: head.header.hash,
        }
    };
    let offline = OfflineContext::new(
        &context.offline_profile,
        context.registry,
        context.manager,
        context.rollup_id,
        parent.hash_slow(),
        anchor,
        head.header.timestamp,
    )
    .unwrap();
    let mut batch = eez_protocol::EvmBatch {
        blockNumber: if actual_target.is_some() {
            u64::MAX
        } else {
            head.header.number
        },
        expectedRootPerRollup: vec![eez_protocol::abi::ExpectedRootPerRollupSol {
            rollupId: context.rollup_id,
            root: parent.hash_slow(),
        }],
        entries: vec![ExecutionEntrySol {
            rootUpdates: vec![RootUpdateSol {
                rollupId: context.rollup_id,
                etherDelta: EtherDeltaSol::ZERO,
                currentRoot: parent.hash_slow(),
                newRoot: terminal.hash_slow(),
            }],
            destinationRollupId: context.rollup_id,
            success: true,
            ..Default::default()
        }],
        immediateEntryCount: U256::from(1),
        proofSystems: vec![context.proof_system],
        rollupIdsWithProofSystems: vec![RollupIdWithProofSystemsSol {
            rollupId: context.rollup_id,
            proofSystemIndexes: vec![0],
        }],
        blobIndices: (0..blobs.len()).map(U256::from).collect(),
        ..Default::default()
    };
    eez_protocol::entries::finalize_l1_rolling_hashes(&mut batch).unwrap();
    let encoding = if context.profile == NativeL1Profile::EthereumOsaka {
        BlobSidecarEncoding::Eip7594
    } else {
        BlobSidecarEncoding::Eip4844
    };
    let sidecar = prepare_blob_sidecar(&blobs, encoding).unwrap();
    let capabilities = ProverClient::connect(context.endpoint.to_owned())
        .await
        .unwrap()
        .capabilities(eez_control_rpc::v2::CapabilitiesRequest {})
        .await?
        .into_inner();
    assert_eq!(capabilities.profile_id, eez_control_rpc::OFFLINE_PROFILE_ID);
    assert!(
        blocks.len() as u64 <= capabilities.max_window_blocks,
        "catch-up must use bounded parts"
    );
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let mut request = tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(receiver));
    request.metadata_mut().insert_bin(
        "eez-validation-identity-bin",
        tonic::metadata::MetadataValue::from_bytes(&capabilities.validation_identity),
    );
    let mut client = v2::prover_client::ProverClient::connect(context.endpoint.to_owned())
        .await
        .unwrap()
        .max_encoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES)
        .max_decoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES);
    let mut responses = client.prove_stream(request).await?.into_inner();
    let ready = exchange(
        &sender,
        &mut responses,
        ClientFrame {
            session_id: vec![],
            request_id: 1,
            epoch: 0,
            kind: Some(client_frame::Kind::Begin(v2::Begin {
                rollup_id: context.rollup_id,
                anchor_number: parent.number,
                anchor_hash: parent.hash_slow().to_vec(),
                anchor_state_root: parent.state_root.to_vec(),
            })),
        },
    )
    .await?;
    assert!(matches!(ready.kind, Some(server_frame::Kind::Ready(_))));
    let mut evidence = HeaderChain::default();
    let mut previous_raw: Option<Bytes> = None;
    let mut request_bytes = 0;
    let mut witness_items = 0;
    let started = Instant::now();
    for (index, block) in blocks.iter().enumerate() {
        let number = block.header.number;
        let hash = block.header.hash_slow();
        let source = store.clone();
        let source = tokio::task::spawn_blocking(move || {
            source
                .get(number, hash)
                .unwrap()
                .expect("durable witness missing")
        })
        .await
        .unwrap();
        let mut l1_headers = source.l1_headers_rlp;
        assert!(!l1_headers.is_empty());
        if let Some(previous) = &previous_raw {
            assert_eq!(
                previous,
                &l1_headers.remove(0),
                "durable evidence boundary mismatch"
            );
        }
        if let Some(last) = l1_headers.last() {
            previous_raw = Some(last.clone());
        }
        if number == terminal.number {
            let end: Header = alloy_rlp::decode_exact(previous_raw.as_ref().unwrap()).unwrap();
            for header in read_l1_headers(context.l1_rpc, end.number + 1, head.header.number).await
            {
                l1_headers.push(alloy_rlp::encode(header).into());
            }
        }
        evidence
            .append(&l1_headers, &context.header_rules, 131_072)
            .unwrap();
        let witness = source.witness;
        witness_items += witness.state.len()
            + witness.codes.len()
            + witness.keys.len()
            + witness.headers.len()
            + l1_headers.len();
        let wire = BlockWitness {
            number,
            hash: hash.to_vec(),
            parent_hash: source.parent_hash.to_vec(),
            rlp: source.rlp.to_vec(),
            witness: Some(ExecutionWitness {
                state: witness.state.into_iter().map(|b| b.to_vec()).collect(),
                codes: witness.codes.into_iter().map(|b| b.to_vec()).collect(),
                keys: witness.keys.into_iter().map(|b| b.to_vec()).collect(),
                headers: witness.headers.into_iter().map(|b| b.to_vec()).collect(),
                l1_headers_rlp: l1_headers.into_iter().map(|b| b.to_vec()).collect(),
            }),
        };
        request_bytes += wire.encoded_len();
        let ack = exchange(
            &sender,
            &mut responses,
            ClientFrame {
                session_id: ready.session_id.clone(),
                request_id: index as u64 + 2,
                epoch: ready.epoch,
                kind: Some(client_frame::Kind::Block(v2::Block { data: Some(wire) })),
            },
        )
        .await?;
        assert!(
            matches!(ack.kind, Some(server_frame::Kind::Validated(ref b)) if b.number == number && b.hash == hash.as_slice() && b.post_state_root == block.header.state_root.as_slice())
        );
    }
    evidence.verify_anchor(anchor).unwrap();
    let result = exchange(
        &sender,
        &mut responses,
        ClientFrame {
            session_id: ready.session_id,
            request_id: blocks.len() as u64 + 2,
            epoch: ready.epoch,
            kind: Some(client_frame::Kind::Finalize(v2::Finalize {
                from_block: parent.number + 1,
                to_block: terminal.number,
                terminal_hash: terminal.hash_slow().to_vec(),
                settlement: Some(v2::SettlementInput {
                    abi_calldata: eez_protocol::entries::encode_postbatch(&batch),
                    blobs: blobs
                        .iter()
                        .cloned()
                        .map(|blob| blob.into_bytes().into_vec())
                        .collect(),
                    posting_context: actual_target.clone(),
                }),
            })),
        },
    )
    .await?;
    let Some(server_frame::Kind::Proof(proof)) = result.kind else {
        panic!("expected offline v2 proof")
    };
    let expected = eez_protocol::public_inputs::public_inputs_hashes_with_blob_hashes(
        &batch,
        context.attester.into_word(),
        sidecar.versioned_hashes(),
        offline,
    )
    .unwrap();
    assert_eq!(expected.len(), 1);
    assert_eq!(proof.public_inputs_hash, expected[0].as_slice());
    assert_eq!(
        Signature::try_from(proof.proof.as_slice())
            .unwrap()
            .recover_address_from_prehash(&expected[0])
            .unwrap(),
        context.attester
    );
    eprintln!(
        "{}",
        serde_json::json!({"checkpoint": "native_offline_v2", "requested_ceiling": history_ceiling,
        "from": parent.number + 1, "to": terminal.number, "k": context.k,
        "mode": if actual_target.is_some() { "actual-parent" } else { "historical" },
        "l1_anchor": head.header.number,
        "l1_headers": evidence.headers().len(), "l2_terminal_hash": terminal.hash_slow(), "l2_state_root": terminal.state_root,
        "physical_blobs": blobs.len(), "operations_bytes": operations_bytes, "request_bytes": request_bytes,
        "witness_items": witness_items, "replay_and_proof_ms": started.elapsed().as_millis()})
    );
    batch.proofs = vec![proof.proof.into()];
    if let Some(target) = actual_target {
        return Ok((batch, sidecar, target));
    }
    // Operational submission target is separate from the historical proof anchor.
    let current = l1
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    let target = L1PostingContext {
        chain_id: context.l1_chain_id,
        parent_number: current.header.number,
        parent_hash: current.header.hash.to_vec(),
        target_timestamp: current.header.timestamp + context.l1_step,
    };
    Ok((batch, sidecar, target))
}

/// Bounded acquisition belongs to the request producer, never the signer.
/// Sort and check every returned height before using the canonical-chain checks.
async fn read_l1_headers(rpc: &str, from: u64, through: u64) -> Vec<Header> {
    let mut workers = tokio::task::JoinSet::new();
    for offset in 0..8 {
        let rpc = rpc.to_owned();
        workers.spawn(async move {
            let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());
            let mut headers = Vec::new();
            for number in (from + offset..=through).step_by(8) {
                let header = provider
                    .get_block_by_number(number.into())
                    .await
                    .unwrap()
                    .unwrap()
                    .header
                    .inner;
                assert_eq!(header.number, number);
                headers.push(header);
            }
            headers
        });
    }
    let mut headers = Vec::new();
    while let Some(result) = workers.join_next().await {
        headers.extend(result.unwrap());
    }
    headers.sort_by_key(|header| header.number);
    assert_eq!(
        headers.len() as u64,
        through.saturating_add(1).saturating_sub(from)
    );
    headers
}

async fn prove_native_window(
    world: &eez_testkit::NativeWorld,
    profile: NativeL1Profile,
    genesis: &Header,
    blocks: &[Block],
    history_ceiling: Option<u64>,
    follower_env: &[(&'static str, String)],
    l1_step: u64,
    k: u64,
    store: Arc<eez_node::WitnessDb>,
    post_and_follow: bool,
) -> Result<(), tonic::Status> {
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let read_setting = |name: &str| {
        follower_env
            .iter()
            .rev()
            .find(|(key, _)| *key == name)
            .unwrap()
            .1
            .clone()
    };
    let terminal = blocks.last().unwrap().header.number;
    let terminal_hash = blocks.last().unwrap().header.hash_slow();
    assert_eq!(
        state_root(&world.l1_rpc(), world.dep.eez_address, world.dep.rollup_id)
            .await
            .unwrap(),
        genesis.hash_slow()
    );
    let l1_rpc = world.l1_rpc();
    let context = NativeProofContext {
        endpoint: world.proof_signer.endpoint(),
        profile,
        k,
        rollup_id: world.dep.rollup_id,
        proof_system: world.dep.proof_system_address,
        attester: signer_address(world.cfg.attester_key).unwrap(),
        l1_chain_id: l1.get_chain_id().await.unwrap(),
        l1_rpc: &l1_rpc,
        l1_step,
        registry: world.dep.eez_address,
        manager: world.dep.rollup_manager_address,
        offline_profile: world.dep.profile.clone(),
        header_rules: eez_l1::chain_config::offline_l1_rules(
            &eez_l1::chain_config::load_l1_chain_spec(std::path::Path::new(&read_setting(
                "EEZ_L1_CHAIN_CONFIG",
            )))
            .unwrap(),
        )
        .unwrap()
        .headers,
    };
    let (batch, sidecar, target) =
        prove_cold_window(&context, genesis, blocks, history_ceiling, store, None).await?;
    if !post_and_follow {
        return Ok(());
    }
    post_native_proof(world, batch, sidecar, target, terminal_hash).await;
    let follower = NodeHandle::start(
        "derivable-clean-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        follower_env,
    )
    .await
    .unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], terminal, TIMEOUT)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "V0 replay failed: {error:#}{}\n{}",
                world.settlement_diagnostics(),
                follower.log_tail(100)
            )
        });
    let replayed = ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap());
    for original in blocks {
        let actual = replayed
            .get_block_by_number(original.header.number.into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.header.hash, original.header.hash_slow());
    }
    Ok(())
}

async fn post_native_proof(
    world: &eez_testkit::NativeWorld,
    batch: eez_protocol::EvmBatch,
    sidecar: eez_blob_codec::PreparedBlobSidecar,
    target: L1PostingContext,
    terminal_hash: B256,
) {
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let poster: alloy_signer_local::PrivateKeySigner = world.cfg.poster_key.parse().unwrap();
    let signed = sign_blob_transaction(
        prepare_blob_transaction_with_sidecar(
            BlobTransactionParams {
                chain_id: l1.get_chain_id().await.unwrap(),
                nonce: l1
                    .get_transaction_count(poster.address())
                    .pending()
                    .await
                    .unwrap(),
                gas_limit: 5_000_000,
                max_fee_per_gas: l1.get_gas_price().await.unwrap() * 2 + 1_000_000_000,
                max_priority_fee_per_gas: 1_000_000_000,
                max_fee_per_blob_gas: l1.get_blob_base_fee().await.unwrap().max(1) * 2,
                to: world.dep.eez_address,
                value: U256::ZERO,
                access_list: Default::default(),
                input: eez_protocol::entries::encode_postbatch(&batch).into(),
            },
            sidecar,
        )
        .unwrap(),
        &poster,
    )
    .unwrap();
    let current = l1
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    assert!(
        current.header.timestamp < target.target_timestamp,
        "cold prototype missed its explicit target"
    );
    assert_eq!(
        current.header.hash.as_slice(),
        target.parent_hash.as_slice(),
        "proof parent changed before submission"
    );
    let target_number = target.parent_number + 1;
    let builder = ProviderBuilder::new().connect_http(world.fault_rpc_url().parse().unwrap());
    let _: serde_json::Value = builder
        .client()
        .request(
            "eth_sendBundle",
            (serde_json::json!({
                "txs": [signed.pooled_transaction], "blockNumber": format!("0x{target_number:x}"),
                "minTimestamp": target.target_timestamp, "maxTimestamp": target.target_timestamp,
            }),),
        )
        .await
        .unwrap();
    assert!(
        wait_for(TIMEOUT, || async {
            receipt_ok(&world.l1_rpc(), signed.transaction_hash).await
        })
        .await
        .unwrap(),
        "native post reverted"
    );
    assert_eq!(
        state_root(&world.l1_rpc(), world.dep.eez_address, world.dep.rollup_id)
            .await
            .unwrap(),
        terminal_hash
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_native_v0_post_replays_from_blobs_without_sequencer_headers() {
    native_replay(NativeL1Profile::EthereumOsaka, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_native_v0_post_replays_from_blobs_without_sequencer_headers() {
    native_replay(NativeL1Profile::GnosisPrague, None).await;
}

fn history_ceiling() -> u64 {
    let ceiling = std::env::var("EEZ_TEST_DERIVABLE_HISTORY_CEILING").map_or(65_536, |value| {
        value.parse().expect("history ceiling must be an integer")
    });
    assert!((5..=65_536).contains(&ceiling));
    ceiling
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit large native cold feasibility checkpoint; capped production catch-up takes minutes"]
async fn ethereum_native_large_history_cold_checkpoint() {
    native_replay(NativeL1Profile::EthereumOsaka, Some(history_ceiling())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit large native cold feasibility checkpoint; capped production catch-up takes minutes"]
async fn gnosis_native_large_history_cold_checkpoint() {
    native_replay(NativeL1Profile::GnosisPrague, Some(history_ceiling())).await;
}

/// Repeat conditional proofs after shutting down the original producer. This
/// reads the retained native witnesses; it neither invents blocks nor requires
/// their old parent state to remain available in a live node.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the explicitly retained native history and metadata"]
async fn retained_native_history_cold_checkpoint() {
    let directory =
        std::path::PathBuf::from(std::env::var("EEZ_TEST_RETAINED_NATIVE_HISTORY").unwrap());
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("metadata.json")).unwrap()).unwrap();
    let genesis: alloy_genesis::Genesis =
        serde_json::from_slice(&std::fs::read(directory.join("l2-genesis.json")).unwrap()).unwrap();
    let chain = reth_chainspec::ChainSpec::from(genesis);
    let genesis_header = chain.genesis_header();
    assert_eq!(
        genesis_header.hash_slow(),
        metadata["genesis_hash"]
            .as_str()
            .unwrap()
            .parse::<B256>()
            .unwrap()
    );
    let profile = match metadata["profile"].as_str().unwrap() {
        "EthereumOsaka" => NativeL1Profile::EthereumOsaka,
        "GnosisPrague" => NativeL1Profile::GnosisPrague,
        other => panic!("unsupported native fixture profile: {other}"),
    };
    let l1_rpc = std::env::var("EEZ_TEST_L1_RPC_URL").expect("canonical fixture L1 RPC required");
    let signer = eez_testkit::ProofSignerHandle::spawn(&eez_testkit::ProofSignerConfig {
        rollup_manager_address: metadata["rollup_manager_address"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
        registry_address: metadata["registry_address"]
            .as_str()
            .expect("fixture registry address")
            .parse()
            .unwrap(),
        chain_config: &directory.join("l2-genesis.json"),
        l1_chain_config: if profile == NativeL1Profile::GnosisPrague {
            format!("gnosis:{}", directory.join("l1-genesis.json").display())
        } else {
            directory.join("l1-genesis.json").display().to_string()
        },
        rollup_id: metadata["rollup_id"].as_u64().unwrap(),
        signer_key: eez_testkit::ANVIL_ATTESTER_KEY,
        vkey: metadata["vkey"].as_str().unwrap().parse().unwrap(),
        proof_system: metadata["proof_system"].as_str().unwrap().parse().unwrap(),
        max_request_blocks: None,
    })
    .await
    .unwrap();
    let witness_path = directory.join("witness-db");
    let store = tokio::task::spawn_blocking(move || {
        Arc::new(eez_node::WitnessDb::open(&witness_path).unwrap())
    })
    .await
    .unwrap();
    let terminal = metadata["terminal_number"].as_u64().unwrap();
    let terminal_hash = if let Some(hash) = metadata["terminal_hash"].as_str() {
        hash.parse().unwrap()
    } else {
        assert_eq!(
            metadata["discover_terminal_in_stopped_store"], true,
            "missing terminal hash requires explicit offline discovery"
        );
        store
            .unique_hash_at_height(terminal)
            .unwrap()
            .expect("retained terminal is missing")
    };
    let blocks = load_history(
        Arc::clone(&store),
        terminal,
        terminal_hash,
        genesis_header.hash_slow(),
    )
    .await;
    if let Some(pin) = metadata.get("observed_parent_pin") {
        let number = pin["number"].as_u64().unwrap();
        assert_eq!(
            blocks[number as usize - 1].header.hash_slow(),
            pin["hash"].as_str().unwrap().parse::<B256>().unwrap()
        );
    }
    let l1: alloy_genesis::Genesis =
        serde_json::from_slice(&std::fs::read(directory.join("l1-genesis.json")).unwrap()).unwrap();
    let l1_spec = eez_l1::chain_config::load_l1_chain_spec(std::path::Path::new(&if profile
        == NativeL1Profile::GnosisPrague
    {
        format!("gnosis:{}", directory.join("l1-genesis.json").display())
    } else {
        directory.join("l1-genesis.json").display().to_string()
    }))
    .unwrap();
    let rules = eez_l1::chain_config::offline_l1_rules(&l1_spec).unwrap();
    let derivation: eez_protocol::l2::DerivationConfig =
        serde_json::from_value(chain.genesis.config.extra_fields["rollupX"].clone()).unwrap();
    let schedule = eez_protocol::l2::Schedule::new(derivation, genesis_header).unwrap();
    let context = NativeProofContext {
        endpoint: signer.endpoint(),
        registry: metadata["registry_address"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
        manager: metadata["rollup_manager_address"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
        offline_profile: rules.profile(schedule, chain.chain().id()).unwrap(),
        header_rules: rules.headers,
        profile,
        k: metadata["k"].as_u64().unwrap(),
        rollup_id: metadata["rollup_id"].as_u64().unwrap(),
        proof_system: metadata["proof_system"].as_str().unwrap().parse().unwrap(),
        attester: metadata["attester"].as_str().unwrap().parse().unwrap(),
        l1_chain_id: l1.config.chain_id,
        l1_rpc: &l1_rpc,
        l1_step: {
            let config: eez_protocol::l2::DerivationConfig = serde_json::from_value(
                chain
                    .genesis
                    .config
                    .extra_fields
                    .get("rollupX")
                    .unwrap()
                    .clone(),
            )
            .unwrap();
            eez_protocol::l2::Schedule::new(config, genesis_header)
                .unwrap()
                .l1_slot_seconds()
        },
    };
    let mut failures = Vec::new();
    for ceiling in [512, 1_024, 4_096, 10_000, 43_200, 65_536] {
        let mut count = native_grid_boundary(ceiling, context.k) as usize;
        while blocks[count - 1]
            .body
            .transactions
            .iter()
            .any(|tx| !tx.is_history())
        {
            count = count
                .checked_sub(context.k as usize)
                .filter(|count| *count != 0)
                .expect("an empty legal prefix must exist");
        }
        if let Err(error) = prove_cold_window(
            &context,
            genesis_header,
            &blocks[..count],
            Some(ceiling),
            Arc::clone(&store),
            None,
        )
        .await
        {
            failures.push((ceiling, error.code(), error.message().to_owned()));
        }
    }
    assert!(
        failures.is_empty(),
        "retained native cold checkpoint failures: {failures:?}"
    );
}

fn native_grid_boundary(ceiling: u64, k: u64) -> u64 {
    assert!((2..=65_536).contains(&k));
    let endpoint = ceiling / k * k;
    assert!(
        endpoint > 0,
        "no nonzero genesis-derived endpoint below ceiling"
    );
    endpoint
}

#[test]
fn native_capacity_uses_the_genesis_sync_grid() {
    assert_eq!(native_grid_boundary(65_536, 5), 65_535);
    assert_eq!(native_grid_boundary(65_536, 6), 65_532);
    for (ceiling, expected) in [
        (512, 510),
        (1024, 1020),
        (4096, 4095),
        (10_000, 10_000),
        (43_200, 43_200),
    ] {
        assert_eq!(native_grid_boundary(ceiling, 5), expected);
    }
}
