//! Independent test writer for the cold-path checkpoint.
//! This posts a real signer-authorized native blob batch and
//! starts a clean follower with L1/DA and genesis only.

use prost::Message as _;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use alloy_consensus::{Block, Header, TxEnvelope};
use alloy_eips::Encodable2718 as _;
use alloy_primitives::{Address, B256, Bytes, Signature, U256};
use alloy_provider::{Provider as _, ProviderBuilder};
use eez_blob_codec::{
    BlobSidecarEncoding, BlobTransactionParams, prepare_blob_sidecar,
    prepare_blob_transaction_with_sidecar, sign_blob_transaction,
};
use eez_control_rpc::v1::{
    BlockWitness, CachedOrdinaryRange, ExecutionWitness, L1PostingContext, PostBatch, PrepareChunk,
    PrepareHeader, ProveChunk, ProveHeader, RetainOrdinaryRangesRequest, prepare_chunk,
    prove_chunk, prove_failure, prover_client::ProverClient,
};
use eez_payload_codec::v0::{self, BlockRun};
use eez_protocol::abi::{
    EtherDeltaSol, ExecutionEntrySol, RollupIdWithProofSystemsSol, RootUpdateSol,
};
use eez_testkit::{
    NativeL1Profile, NodeBinary, NodeConfig, NodeHandle, batches_posted, receipt_ok,
    setup_cross_chain_with_profile, signer_address, state_root, wait_for, wait_for_latest_height,
    wait_for_safe_prefix_convergence,
};

const TIMEOUT: Duration = Duration::from_mins(3);

/// Independent dense test writer: only actual metadata and exact ordinary
/// transaction bytes. The signer must reconstruct all omitted outputs.
fn encode(_parent: &Header, blocks: &[Block<TxEnvelope>]) -> Vec<u8> {
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
        while !blocks[count - 1].body.transactions.is_empty() {
            count = count
                .checked_sub(k as usize)
                .filter(|count| *count != 0)
                .expect("an empty legal prefix must exist");
        }
        if let Err(error) = prove_native_window(
            &world,
            profile,
            &genesis,
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
}

async fn load_history(
    store: Arc<eez_node::WitnessDb>,
    terminal: u64,
    mut hash: B256,
    genesis: B256,
) -> Vec<Block<TxEnvelope>> {
    tokio::task::spawn_blocking(move || {
        let mut blocks = Vec::new();
        let deadline = Instant::now() + TIMEOUT;
        for number in (1..=terminal).rev() {
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
            let block: Block<TxEnvelope> = alloy_rlp::decode_exact(&witness.rlp).unwrap();
            assert_eq!(block.header.number, number);
            assert_eq!(block.header.hash_slow(), hash);
            hash = block.header.parent_hash;
            if number.is_multiple_of(4096) {
                eprintln!("native durable inputs remaining: {number}/{terminal}");
            }
            blocks.push(block);
        }
        assert_eq!(hash, genesis);
        blocks.reverse();
        blocks
    })
    .await
    .unwrap()
}

async fn prove_cold_window(
    context: &NativeProofContext<'_>,
    genesis: &Header,
    blocks: &[Block<TxEnvelope>],
    history_ceiling: Option<u64>,
    store: Arc<eez_node::WitnessDb>,
    release_after_proof: bool,
    producer_prepared: bool,
) -> Result<
    (
        eez_protocol::EvmBatch,
        eez_blob_codec::PreparedBlobSidecar,
        L1PostingContext,
    ),
    tonic::Status,
> {
    let terminal = blocks.last().unwrap().header.number;
    assert!(blocks.last().unwrap().body.transactions.is_empty());
    let terminal_hash = blocks.last().unwrap().header.hash_slow();
    let operations = encode(genesis, blocks);
    let operations_bytes = operations.len();
    let blobs = eez_blob_codec::encode_native_payload(context.rollup_id, operations.into(), vec![])
        .unwrap();
    assert_eq!(blobs.len(), 1);
    let mut batch = eez_protocol::EvmBatch {
        blockNumber: u64::MAX,
        entries: vec![ExecutionEntrySol {
            rootUpdates: vec![RootUpdateSol {
                rollupId: context.rollup_id,
                etherDelta: EtherDeltaSol::ZERO,
                currentRoot: genesis.hash_slow(),
                newRoot: terminal_hash,
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
    let l1 = ProviderBuilder::new().connect_http(context.l1_rpc.parse().unwrap());
    let encoding = if context.profile == NativeL1Profile::EthereumOsaka {
        BlobSidecarEncoding::Eip7594
    } else {
        BlobSidecarEncoding::Eip4844
    };
    // KZG preparation precedes the short-lived signing target selection.
    let sidecar = prepare_blob_sidecar(&blobs, encoding).unwrap();
    let mut prover_client = ProverClient::connect(context.endpoint.to_owned())
        .await
        .unwrap()
        .max_encoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES)
        .max_decoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES);
    let capabilities = prover_client
        .capabilities(eez_control_rpc::v1::CapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    let validation_identity = capabilities.validation_identity;
    let header = ProveHeader {
        parent_provenance: Some(eez_control_rpc::v1::ParentProvenance {
            parent_header_rlp: alloy_rlp::encode(genesis),
            accepted_l1_number: 0,
            live_slot_endpoint_headers_rlp: Vec::new(),
        }),
        expected_validation_identity: validation_identity.clone(),
        rollup_id: context.rollup_id,
        from_block: 1,
        to_block: terminal,
        post_batch: Some(PostBatch {
            abi_calldata: eez_protocol::entries::encode_postbatch(&batch),
            blobs: blobs
                .iter()
                .cloned()
                .map(|blob| blob.into_bytes().into_vec())
                .collect(),
            // Filled from canonical L1 immediately before each Prove.
            posting_context: None,
        }),
    };
    let header_chunk = ProveChunk {
        kind: Some(prove_chunk::Kind::Header(header)),
    };
    let identities = blocks
        .iter()
        .map(|block| (block.header.number, block.header.hash_slow()))
        .collect::<Vec<_>>();
    let proof_started = Instant::now();
    let mut request_bytes = header_chunk.encoded_len();
    let mut witness_items = 0usize;
    let mut streamed = 0usize;
    let preparation_limit = usize::try_from(capabilities.preparation_max_blocks).unwrap();
    assert_eq!(preparation_limit, 64);
    let mut lease_id = Vec::new();
    for group in identities[..identities.len().saturating_sub(1)]
        .chunks(preparation_limit)
        .filter(|_| !producer_prepared)
    {
        let range = group.to_vec();
        let source = Arc::clone(&store);
        let wire = tokio::task::spawn_blocking(move || {
            range
                .into_iter()
                .map(|(number, hash)| {
                    let source = source
                        .get(number, hash)
                        .unwrap()
                        .expect("native witness must remain durable");
                    let witness = source.witness;
                    let items = witness.state.len()
                        + witness.codes.len()
                        + witness.keys.len()
                        + witness.headers.len();
                    let block = BlockWitness {
                        number,
                        hash: hash.to_vec(),
                        parent_hash: source.parent_hash.to_vec(),
                        rlp: source.rlp.to_vec(),
                        witness: Some(ExecutionWitness {
                            state: witness.state.into_iter().map(|b| b.to_vec()).collect(),
                            codes: witness.codes.into_iter().map(|b| b.to_vec()).collect(),
                            keys: witness.keys.into_iter().map(|b| b.to_vec()).collect(),
                            headers: witness.headers.into_iter().map(|b| b.to_vec()).collect(),
                        }),
                    };
                    (block, items)
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap();
        let first = wire.first().unwrap().0.number;
        let last = wire.last().unwrap().0.number;
        let mut chunks = Vec::with_capacity(wire.len() + 1);
        let preparation_header = PrepareChunk {
            kind: Some(prepare_chunk::Kind::Header(PrepareHeader {
                rollup_id: context.rollup_id,
                expected_validation_identity: validation_identity.clone(),
                from_block: first,
                to_block: last,
                lease_id: lease_id.clone(),
            })),
        };
        request_bytes += preparation_header.encoded_len();
        chunks.push(preparation_header);
        for (block, items) in wire {
            witness_items += items;
            streamed += 1;
            let chunk = PrepareChunk {
                kind: Some(prepare_chunk::Kind::Block(block)),
            };
            request_bytes += chunk.encoded_len();
            chunks.push(chunk);
        }
        let prepared = prover_client
            .prepare_ordinary(tokio_stream::iter(chunks))
            .await?
            .into_inner();
        lease_id = prepared.lease_id;
        if last.is_multiple_of(4096) {
            eprintln!(
                "native preparation: {last}/{terminal}, bytes={request_bytes}, items={witness_items}"
            );
        }
    }

    let (terminal_number, terminal_identity) = *identities.last().unwrap();
    let terminal_source = {
        let source = Arc::clone(&store);
        tokio::task::spawn_blocking(move || {
            source
                .get(terminal_number, terminal_identity)
                .unwrap()
                .expect("terminal witness must remain durable")
        })
        .await
        .unwrap()
    };
    let witness = terminal_source.witness;
    witness_items +=
        witness.state.len() + witness.codes.len() + witness.keys.len() + witness.headers.len();
    streamed += 1;
    let terminal_chunk = ProveChunk {
        kind: Some(prove_chunk::Kind::Block(BlockWitness {
            number: terminal_number,
            hash: terminal_identity.to_vec(),
            parent_hash: terminal_source.parent_hash.to_vec(),
            rlp: terminal_source.rlp.to_vec(),
            witness: Some(ExecutionWitness {
                state: witness.state.into_iter().map(|b| b.to_vec()).collect(),
                codes: witness.codes.into_iter().map(|b| b.to_vec()).collect(),
                keys: witness.keys.into_iter().map(|b| b.to_vec()).collect(),
                headers: witness.headers.into_iter().map(|b| b.to_vec()).collect(),
            }),
        })),
    };
    request_bytes += terminal_chunk.encoded_len();
    let mut final_chunks = vec![header_chunk];
    if identities.len() > 1 {
        let first = identities.first().unwrap();
        let last = &identities[identities.len() - 2];
        let range = CachedOrdinaryRange {
            validation_identity: validation_identity.clone(),
            first_number: first.0,
            first_hash: first.1.to_vec(),
            last_number: last.0,
            last_hash: last.1.to_vec(),
        };
        if !producer_prepared {
            let retained = prover_client
                .retain_ordinary_ranges(RetainOrdinaryRangesRequest {
                    validation_identity: validation_identity.clone(),
                    lease_id: lease_id.clone(),
                    ranges: vec![range.clone()],
                    release: false,
                })
                .await?
                .into_inner();
            assert_eq!(retained.validation_identity, validation_identity);
            assert_eq!(retained.lease_id, lease_id);
            assert_eq!(retained.retained_count, (identities.len() - 1) as u64);
        }
        let range = ProveChunk {
            kind: Some(prove_chunk::Kind::CachedRange(range)),
        };
        request_bytes += range.encoded_len();
        final_chunks.push(range);
    }
    final_chunks.push(terminal_chunk);
    let final_repetitions = if history_ceiling == Some(65_536) {
        21
    } else {
        1
    };
    let mut final_times = Vec::with_capacity(final_repetitions);
    let mut first_response: Option<(L1PostingContext, eez_control_rpc::v1::ProveResponse)> = None;
    let mut signed_target = None;
    let mut outcome = None;
    let catch_up_deadline = Instant::now() + Duration::from_mins(2);
    let mut cache_misses = 0usize;
    for _ in 0..final_repetitions {
        let started = Instant::now();
        let result = loop {
            let head = l1
                .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
                .await
                .unwrap()
                .unwrap();
            let target = L1PostingContext {
                chain_id: context.l1_chain_id,
                parent_number: head.header.number,
                parent_hash: head.header.hash.to_vec(),
                target_timestamp: head.header.timestamp + context.l1_step,
            };
            let Some(prove_chunk::Kind::Header(header)) = &mut final_chunks[0].kind else {
                unreachable!()
            };
            header.post_batch.as_mut().unwrap().posting_context = Some(target.clone());
            let result = prover_client
                .prove(tokio_stream::iter(final_chunks.clone()))
                .await
                .map(tonic::Response::into_inner);
            match result {
                Ok(response) => {
                    if let Some((previous_target, previous_response)) = &first_response
                        && previous_target == &target
                    {
                        assert_eq!(
                            &response, previous_response,
                            "warm proof changed within the same authenticated context"
                        );
                    }
                    first_response = Some((target.clone(), response.clone()));
                    signed_target = Some(target);
                    break Ok(response);
                }
                Err(status) => {
                    let latest = l1
                        .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
                        .await
                        .unwrap()
                        .unwrap();
                    let stale = latest.header.hash != head.header.hash;
                    let cache_miss = structured_cache_miss(&status);
                    if Instant::now() < catch_up_deadline
                        && (stale
                            || (producer_prepared
                                && (status.code() == tonic::Code::Unavailable || cache_miss)))
                    {
                        cache_misses += usize::from(cache_miss);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                    break Err(status);
                }
            }
        };
        final_times.push(started.elapsed().as_millis() as u64);
        let failed = result.is_err();
        outcome = Some(result);
        if failed {
            break;
        }
    }
    let proof_ms = proof_started.elapsed().as_millis();
    let mut sorted_final_times = final_times.clone();
    sorted_final_times.sort_unstable();
    let percentile =
        |numerator: usize| sorted_final_times[(sorted_final_times.len() - 1) * numerator / 100];
    let outcome = outcome.expect("at least one final proof repetition");
    eprintln!(
        "{}",
        serde_json::json!({
            "checkpoint": "native_cold_history", "profile": format!("{:?}", context.profile),
            "requested_ceiling": history_ceiling, "actual_blocks": terminal, "k": context.k,
            "anchor_hash": genesis.hash_slow(), "terminal_hash": terminal_hash,
            "terminal_state_root": blocks.last().unwrap().header.state_root,
            "target_l1_timestamp": signed_target.as_ref().map(|target| target.target_timestamp),
            "l2_start_timestamp": genesis.timestamp,
            "l2_terminal_timestamp": blocks.last().unwrap().header.timestamp,
            "transactions": blocks.iter().map(|b| b.body.transactions.len()).sum::<usize>(),
            "public_inputs_hash": outcome.as_ref().ok().map(|r| alloy_primitives::hex::encode(&r.public_inputs_hash)),
            "operations_bytes": operations_bytes,
            "physical_blobs": blobs.len(), "streamed_blocks": streamed,
            "request_bytes_generated": request_bytes, "witness_items_generated": witness_items,
            "stream_and_proof_ms": proof_ms, "success": outcome.is_ok(),
            "warm_repetitions": final_repetitions.saturating_sub(1),
            "producer_prepared": producer_prepared, "cache_misses": cache_misses,
            "final_proof_median_ms": percentile(50), "final_proof_p95_ms": percentile(95),
            "final_proof_max_ms": sorted_final_times.last().copied().unwrap(),
            "error": outcome.as_ref().err().map(|e| format!("{}: {}", e.code(), e.message())),
            "workload": "native genesis with sparse setup transactions",
        })
    );
    let response = outcome?;
    let target = signed_target.expect("successful proof has an authenticated target");
    let attester = context.attester;
    let expected = eez_protocol::public_inputs::public_inputs_hashes_with_blob_hashes(
        &batch,
        attester.into_word(),
        sidecar.versioned_hashes(),
        eez_protocol::public_inputs::L1PostingContext {
            chain_id: target.chain_id,
            parent_number: target.parent_number,
            parent_hash: B256::from_slice(&target.parent_hash),
            target_timestamp: target.target_timestamp,
        },
    )
    .unwrap();
    assert_eq!(expected.len(), 1);
    assert_eq!(response.public_inputs_hash, expected[0].as_slice());
    assert_eq!(
        Signature::try_from(response.signature.as_slice())
            .unwrap()
            .recover_address_from_prehash(&expected[0])
            .unwrap(),
        attester
    );
    if release_after_proof && !lease_id.is_empty() {
        let released = prover_client
            .retain_ordinary_ranges(RetainOrdinaryRangesRequest {
                validation_identity: validation_identity.clone(),
                lease_id: lease_id.clone(),
                ranges: Vec::new(),
                release: true,
            })
            .await?
            .into_inner();
        assert_eq!(released.validation_identity, validation_identity);
        assert_eq!(released.lease_id, lease_id);
        assert_eq!(released.retained_count, 0);
    }
    batch.proofs = vec![response.signature.into()];
    Ok((batch, sidecar, target))
}

fn structured_cache_miss(status: &tonic::Status) -> bool {
    status.code() == tonic::Code::FailedPrecondition
        && eez_control_rpc::decode_prove_failure(status.details())
            .ok()
            .and_then(|failure| failure.actionable_failure)
            .is_some_and(|failure| {
                matches!(failure, prove_failure::ActionableFailure::CacheMiss(_))
            })
}

async fn prove_native_window(
    world: &eez_testkit::CrossChainWorld,
    profile: NativeL1Profile,
    genesis: &Block<TxEnvelope>,
    blocks: &[Block<TxEnvelope>],
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
        genesis.header.hash_slow()
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
    };
    let (batch, sidecar, target) = prove_cold_window(
        &context,
        &genesis.header,
        blocks,
        history_ceiling,
        store,
        !post_and_follow,
        true,
    )
    .await?;
    if !post_and_follow {
        return Ok(());
    }
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
    let builder = ProviderBuilder::new()
        .connect_http(read_setting("EEZ_L1_BUILDER_RPC_URL").parse().unwrap());
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
        // Offline fixtures must now supply a canonical L1 source explicitly.
        l1_rpc_url: l1_rpc.clone(),
        registry_address: metadata["registry_address"]
            .as_str()
            .expect("fixture registry address")
            .parse()
            .unwrap(),
        chain_config: &directory.join("l2-genesis.json"),
        l1_chain_config: Some(if profile == NativeL1Profile::GnosisPrague {
            format!("gnosis:{}", directory.join("l1-genesis.json").display())
        } else {
            directory.join("l1-genesis.json").display().to_string()
        }),
        rollup_id: metadata["rollup_id"].as_u64().unwrap(),
        signer_key: eez_testkit::ANVIL_ATTESTER_KEY,
        vkey: metadata["vkey"].as_str().unwrap().parse().unwrap(),
        proof_system: metadata["proof_system"].as_str().unwrap().parse().unwrap(),
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
    let context = NativeProofContext {
        endpoint: signer.endpoint(),
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
        while !blocks[count - 1].body.transactions.is_empty() {
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
            true,
            false,
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
