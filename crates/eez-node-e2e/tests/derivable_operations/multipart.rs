//! Catch-up beyond L1's blockhash window, settled in bounded ranges with a
//! fresh network-denied signer for every range and a fresh final follower.

use super::*;
use alloy_sol_types::{SolCall as _, SolError as _, sol};
use eez_testkit::{
    HISTORY_PROBE_ADDRESS, ProofSignerConfig, ProofSignerHandle, setup_native_history,
};
use reth_chainspec::EthChainSpec as _;

sol! {
    function observe() external view returns (uint64 number, bytes32 hash, uint256 randao);
    function hashAt(uint64 number) external view returns (bytes32);
    function record() external;
    function records(uint256 index) external view returns (uint256 l2Block, uint64 number, bytes32 hash, uint256 randao);

    interface HistoricalAnchorContext {
        error BlockHashUnavailable(uint64 blockNumber);
        function getCustomData(uint64 selector) external view returns (bytes);
    }
}

async fn application_call(rpc: &str, block: u64, data: Vec<u8>) -> Bytes {
    ProviderBuilder::new()
        .connect_http(rpc.parse().unwrap())
        .client()
        .request(
            "eth_call",
            (
                serde_json::json!({"to": HISTORY_PROBE_ADDRESS, "data": Bytes::from(data)}),
                format!("0x{block:x}"),
            ),
        )
        .await
        .unwrap()
}

async fn observe_source(rpc: &str, block: &Header, expected: &Header) {
    let result = application_call(rpc, block.number, observeCall {}.abi_encode()).await;
    let result = observeCall::abi_decode_returns(&result).unwrap();
    assert_eq!(
        result.number, expected.number,
        "origin number at L2 {}",
        block.number
    );
    assert_eq!(
        result.hash,
        expected.hash_slow(),
        "origin hash at L2 {}",
        block.number
    );
    assert_eq!(
        result.randao,
        U256::from_be_bytes(expected.mix_hash.0),
        "opcode RANDAO at L2 {}",
        block.number
    );
    assert_eq!(
        block.mix_hash, expected.mix_hash,
        "header RANDAO at L2 {}",
        block.number
    );
}

/// Reach the cold signer's source and ancestry gates with an otherwise complete
/// block. The omitted bridge header is later than the entire part, so no source
/// used by that part is removed. The same process then accepts complete evidence.
async fn reject_incomplete_evidence(
    signer: &ProofSignerHandle,
    rollup_id: u64,
    parent: &Header,
    first: &Header,
    terminal: &Header,
    historical_headers: &[Header],
    store: Arc<eez_node::WitnessDb>,
) {
    let endpoint = signer.endpoint();
    let capabilities = ProverClient::connect(endpoint.to_owned())
        .await
        .unwrap()
        .capabilities(eez_control_rpc::v2::CapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    let (sender, receiver) = tokio::sync::mpsc::channel(1);
    let mut request = tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(receiver));
    request.metadata_mut().insert_bin(
        "eez-validation-identity-bin",
        tonic::metadata::MetadataValue::from_bytes(&capabilities.validation_identity),
    );
    let mut client = v2::prover_client::ProverClient::connect(endpoint.to_owned())
        .await
        .unwrap()
        .max_encoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES);
    let mut responses = client.prove_stream(request).await.unwrap().into_inner();
    let ready = exchange(
        &sender,
        &mut responses,
        ClientFrame {
            session_id: vec![],
            request_id: 1,
            epoch: 0,
            kind: Some(client_frame::Kind::Begin(v2::Begin {
                rollup_id,
                anchor_number: parent.number,
                anchor_hash: parent.hash_slow().to_vec(),
                anchor_state_root: parent.state_root.to_vec(),
            })),
        },
    )
    .await
    .unwrap();
    let (number, hash) = (first.number, first.hash_slow());
    let source = tokio::task::spawn_blocking(move || store.get(number, hash).unwrap().unwrap())
        .await
        .unwrap();
    assert!(!source.l1_headers_rlp.is_empty());
    let first_source: Header = alloy_rlp::decode_exact(&source.l1_headers_rlp[0]).unwrap();
    let omitted = &historical_headers[historical_headers.len() - 2];
    assert!(omitted.timestamp > terminal.timestamp);
    let incomplete_bridge = historical_headers
        .iter()
        .filter(|header| header.number >= first_source.number && header.number != omitted.number)
        .map(alloy_rlp::encode)
        .collect::<Vec<_>>();
    let witness = source.witness;
    let execution = ExecutionWitness {
        state: witness.state.into_iter().map(|b| b.to_vec()).collect(),
        codes: witness.codes.into_iter().map(|b| b.to_vec()).collect(),
        keys: witness.keys.into_iter().map(|b| b.to_vec()).collect(),
        headers: witness.headers.into_iter().map(|b| b.to_vec()).collect(),
        l1_headers_rlp: vec![],
    };
    for (index, (l1_headers_rlp, diagnostic)) in [
        (vec![], "missing L1 header evidence"),
        (incomplete_bridge, "nonconsecutive L1 ancestry"),
    ]
    .into_iter()
    .enumerate()
    {
        let rejected = exchange(
            &sender,
            &mut responses,
            ClientFrame {
                session_id: ready.session_id.clone(),
                request_id: index as u64 + 2,
                epoch: ready.epoch,
                kind: Some(client_frame::Kind::Block(v2::Block {
                    data: Some(BlockWitness {
                        number,
                        hash: hash.to_vec(),
                        parent_hash: source.parent_hash.to_vec(),
                        rlp: source.rlp.to_vec(),
                        witness: Some(ExecutionWitness {
                            l1_headers_rlp,
                            ..execution.clone()
                        }),
                    }),
                })),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(rejected.code(), tonic::Code::FailedPrecondition);
        assert_eq!(rejected.message(), "window validation rejected");
        assert!(
            signer.log_tail(30).contains(diagnostic),
            "request did not reach {diagnostic}: {}",
            signer.log_tail(30)
        );
    }
    exchange(
        &sender,
        &mut responses,
        ClientFrame {
            session_id: ready.session_id,
            request_id: 4,
            epoch: ready.epoch,
            kind: Some(client_frame::Kind::Cancel(v2::Cancel {})),
        },
    )
    .await
    .unwrap();
}

async fn catch_up(
    history_seconds: u64,
    part_blocks: u64,
    part_timeout: Duration,
    replay_timeout: Duration,
) -> u64 {
    assert!(history_seconds.is_multiple_of(5));
    assert!(part_blocks.is_multiple_of(5) && part_blocks < history_seconds);
    let started = Instant::now();
    let mut world = setup_native_history(&[
        ("EEZ_TEST_L2_DEBUG_RPC", "true".into()),
        (
            "EEZ_TEST_L2_GENESIS_AGE_SECONDS",
            history_seconds.to_string(),
        ),
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
        (
            "EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS",
            ((history_seconds + 86_400) * 1000).to_string(),
        ),
        // This writer owns settlement. The normal automatic admission gate
        // waits at a one-wei fee cap (below this fixture's execution base fee),
        // while each manual post pays the actual fee independently.
        ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "1".into()),
        ("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "1".into()),
        (
            "EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS",
            part_blocks.to_string(),
        ),
        (
            eez_testkit::SIGNER_MAX_REQUEST_BLOCKS_ENV,
            part_blocks.to_string(),
        ),
        // Keep a finite live speculative-depth limit. Historical catch-up uses
        // its per-turn work bound and can produce ahead of this proof range.
        ("EEZ_MAX_SPECULATIVE_DEPTH", (part_blocks + 4).to_string()),
        ("EEZ_COMPOSER_EXPECT_EXTERNAL_BATCHES", "true".into()),
        ("RUST_LOG", "info,eez::witness=debug".into()),
    ])
    .await
    .unwrap();
    let l1_rpc = world.l1_rpc();
    let l1 = ProviderBuilder::new().connect_http(l1_rpc.parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let genesis = l2
        .get_block_by_number(0.into())
        .await
        .unwrap()
        .unwrap()
        .header
        .inner;
    // Ordinary applications commit the delayed source into their own state
    // while local production catches up and continues. Sequential receipts
    // spread these writes across blocks and exercise nonce/receipt/DA replay.
    let mut application_blocks = Vec::new();
    if history_seconds > 120 {
        let key = eez_testkit::ANVIL_KEY_2;
        let nonce = eez_testkit::onchain_nonce(&world.l2_rpc(), key)
            .await
            .unwrap();
        for index in 0..32 {
            if index == 31 {
                // Force an application past the original catch-up boundary so
                // this regression cannot pass by outrunning block production.
                wait_for_latest_height(&world.node, history_seconds, Duration::from_mins(90))
                    .await
                    .unwrap();
            }
            let hash = eez_testkit::sign_and_send(
                &world.l2_rpc(),
                key,
                world.l2_chain_id,
                nonce + index,
                Some(HISTORY_PROBE_ADDRESS),
                U256::ZERO,
                recordCall {}.abi_encode(),
                200_000,
            )
            .await
            .unwrap();
            let receipt = wait_for(TIMEOUT, || async {
                Ok(l2.get_transaction_receipt(hash).await?)
            })
            .await
            .unwrap();
            assert!(receipt.status());
            let number = receipt.block_number.unwrap();
            application_blocks.push(number);
        }
        assert!(application_blocks.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            application_blocks.last().copied().unwrap() > history_seconds,
            "final application did not execute after the catch-up boundary"
        );
    }
    // The local clock can finish catch-up before all sequential RPC writes do.
    // Fix one terminal that covers both the minimum history and every included
    // application; later live blocks cannot extend this proving range.
    let terminal_number = history_seconds
        .max(application_blocks.last().copied().unwrap_or(0))
        .next_multiple_of(5);
    let end_timestamp = genesis.timestamp + terminal_number;
    eprintln!(
        "{}",
        serde_json::json!({"checkpoint":"offline_catchup_range",
        "minimum_history_seconds":history_seconds, "terminal_number":terminal_number,
        "terminal_timestamp":end_timestamp, "application_blocks":application_blocks})
    );
    // The bridge for the FIRST bounded part must already reach the present.
    // Mining alongside each proof alone would only exercise short ancestry.
    let head = wait_for(Duration::from_mins(90), || async {
        world.node.assert_no_process_death();
        Ok(l1
            .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
            .await?
            .filter(|block| block.header.timestamp >= end_timestamp))
    })
    .await
    .unwrap();
    assert!(head.header.number >= history_seconds / 5);
    let historical_headers = read_l1_headers(&l1_rpc, 0, head.header.number).await;
    for pair in historical_headers
        .windows(2)
        .take((history_seconds / 5) as usize)
    {
        assert_eq!(
            pair[1].timestamp,
            pair[0].timestamp + 5,
            "dense L1 fixture skipped a slot"
        );
        assert_eq!(pair[1].parent_hash, pair[0].hash_slow());
    }
    eprintln!(
        "dense L1 history ready: produced={}, seconds={}, elapsed_ms={}",
        head.header.number,
        history_seconds,
        started.elapsed().as_millis()
    );

    let follower_env = world.partial_follower_env();
    assert!(
        !follower_env
            .iter()
            .any(|(name, _)| *name == "EEZ_SEQUENCER_RPC")
    );
    let l1_profile = follower_env
        .iter()
        .find(|(name, _)| *name == "EEZ_L1_CHAIN_CONFIG")
        .unwrap()
        .1
        .clone();
    let l1_spec =
        eez_l1::chain_config::load_l1_chain_spec(std::path::Path::new(&l1_profile)).unwrap();
    let rules = eez_l1::chain_config::offline_l1_rules(&l1_spec).unwrap();
    let header_rules = rules.headers;
    let path = world.witness_db_path().to_owned();
    let store =
        tokio::task::spawn_blocking(move || Arc::new(eez_node::WitnessDb::open(&path).unwrap()))
            .await
            .unwrap();
    let profile = &world.dep.profile;
    assert_eq!(profile.blockInterval, 1);
    assert_eq!(profile.syncInterval, 5);
    assert_eq!(profile.updateOffset, 2);
    let rule_change = l1_spec.genesis.config.bpo1_time.unwrap();
    assert!(genesis.timestamp < rule_change && rule_change < genesis.timestamp + history_seconds);
    assert!(rules.rule_times.contains(&rule_change));
    assert_ne!(
        profile.rules_id(rule_change - 1).unwrap(),
        profile.rules_id(rule_change).unwrap()
    );
    assert!(historical_headers.first().unwrap().timestamp < rule_change);
    assert!(historical_headers.last().unwrap().timestamp >= rule_change);
    assert_ne!(
        l1_spec
            .blob_params_at_timestamp(rule_change - 1)
            .unwrap()
            .max_blob_count,
        l1_spec
            .blob_params_at_timestamp(rule_change)
            .unwrap()
            .max_blob_count,
    );
    let bootstrap_source = historical_headers
        .iter()
        .rfind(|header| header.timestamp <= genesis.timestamp - 5)
        .unwrap();
    let mut sources = std::collections::BTreeMap::from([(0_u64, bootstrap_source.clone())]);
    let mut parent = genesis.clone();
    let mut expected = Vec::with_capacity(terminal_number as usize);
    let mut parts = 0;
    let mut first_source_age = None;
    while parent.number < terminal_number {
        let to = (parent.number + part_blocks).min(terminal_number);
        wait_for_latest_height(&world.node, to, part_timeout)
            .await
            .unwrap();
        let endpoint = l2
            .get_block_by_number(to.into())
            .await
            .unwrap()
            .unwrap()
            .header;
        let blocks = load_range(
            Arc::clone(&store),
            parent.number,
            to,
            endpoint.hash,
            parent.hash_slow(),
            // Durable capture replays each block separately from production;
            // large parts need a bounded budget that includes that work.
            part_timeout,
        )
        .await;
        for block in &blocks {
            let prior = &sources[&(block.header.number - 1)];
            let elapsed = block.header.timestamp - genesis.timestamp;
            let cutoff = if elapsed < 3 {
                genesis.timestamp - 5
            } else {
                genesis.timestamp + ((elapsed - 3) / 5) * 5
            };
            let recorded: Vec<_> = block
                .body
                .transactions
                .iter()
                .enumerate()
                .filter_map(|(position, tx)| match tx {
                    eez_primitives::EezTxEnvelope::System(tx)
                        if tx.to == eez_protocol::l1_history::HISTORY_ADDRESS =>
                    {
                        Some((position, tx))
                    }
                    _ => None,
                })
                .collect();
            let mut selected = prior.number;
            if elapsed % 5 == 3 {
                assert_eq!(recorded.len(), 1);
                assert_eq!(recorded[0].0, 0);
                let call =
                    eez_protocol::l1_history::IL1BlockhashContract::importHistoryCall::abi_decode(
                        &recorded[0].1.input,
                    )
                    .unwrap();
                assert_eq!(call.abi_encode(), recorded[0].1.input.as_ref());
                assert_eq!(call.cutoff, cutoff);
                assert!(call.hashes.len() <= 64);
                for hash in call.hashes {
                    selected += 1;
                    assert_eq!(hash, historical_headers[selected as usize].hash_slow());
                }
            } else {
                assert!(recorded.is_empty());
            }
            let source = &historical_headers[selected as usize];
            assert!(source.timestamp <= cutoff);
            assert_eq!(block.header.mix_hash, source.mix_hash);
            sources.insert(block.header.number, source.clone());
        }
        let source = |block: &Header| &sources[&block.number];
        assert_eq!(
            state_root(&l1_rpc, world.dep.eez_address, world.dep.rollup_id)
                .await
                .unwrap(),
            parent.hash_slow()
        );
        let cold = ProofSignerHandle::spawn_network_denied(&ProofSignerConfig {
            rollup_manager_address: world.dep.rollup_manager_address,
            registry_address: world.dep.eez_address,
            chain_config: &world.cfg.l2_genesis.0,
            l1_chain_config: l1_profile.clone(),
            rollup_id: world.dep.rollup_id,
            signer_key: world.cfg.attester_key,
            vkey: signer_address(world.cfg.attester_key).unwrap().into_word(),
            proof_system: world.dep.proof_system_address,
            max_request_blocks: Some(part_blocks),
        })
        .await
        .unwrap();
        assert_eq!(cold.streaming_attestations().unwrap(), 0);
        if parts == 0 {
            let terminal_source = source(&blocks.last().unwrap().header);
            let age = head.header.number - terminal_source.number;
            first_source_age = Some(age);
            if age > 256 {
                // Even the newest source used by this entire first part is too
                // old to be an L1 anchor. The proof must authenticate it through
                // the complete header bridge to a recent, available anchor.
                let rejected = eez_testkit::call_revert_data(
                    &l1_rpc,
                    Address::ZERO,
                    world.dep.rollup_manager_address,
                    HistoricalAnchorContext::getCustomDataCall {
                        selector: terminal_source.number,
                    }
                    .abi_encode(),
                )
                .await
                .unwrap();
                assert_eq!(
                    rejected.as_ref(),
                    HistoricalAnchorContext::BlockHashUnavailable {
                        blockNumber: terminal_source.number,
                    }
                    .abi_encode()
                );
                eprintln!(
                    "{}",
                    serde_json::json!({"checkpoint":"expired_l1_source",
                    "source_number":terminal_source.number, "source_hash":terminal_source.hash_slow(),
                    "observed_head_number":head.header.number, "age":age,
                    "manager_rejected_old_anchor":true})
                );
            }
            reject_incomplete_evidence(
                &cold,
                world.dep.rollup_id,
                &parent,
                &blocks[0].header,
                &blocks.last().unwrap().header,
                &historical_headers,
                Arc::clone(&store),
            )
            .await;
            assert_eq!(cold.streaming_attestations().unwrap(), 0);
        }
        let context = NativeProofContext {
            endpoint: cold.endpoint(),
            profile: NativeL1Profile::EthereumOsaka,
            k: 5,
            rollup_id: world.dep.rollup_id,
            proof_system: world.dep.proof_system_address,
            attester: signer_address(world.cfg.attester_key).unwrap(),
            l1_chain_id: l1.get_chain_id().await.unwrap(),
            l1_rpc: &l1_rpc,
            l1_step: 5,
            registry: world.dep.eez_address,
            manager: world.dep.rollup_manager_address,
            offline_profile: profile.clone(),
            header_rules,
        };
        let actual_target = if to == terminal_number {
            // Leave one exact future inclusion slot for the final ordinary
            // range. Skipped slots add no fabricated headers. Arm this only
            // after the cold signer build, so compilation cannot use its time.
            let head = wait_for(TIMEOUT, || async {
                let head = l1
                    .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
                    .await?
                    .unwrap();
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                Ok((head.header.timestamp + 3 > now).then_some(head))
            })
            .await
            .unwrap();
            let control = world.l1_reorg_control.as_mut().unwrap();
            for slot in 1..12 {
                control
                    .skip_slot(head.header.timestamp + slot * 5)
                    .await
                    .unwrap();
            }
            Some(L1PostingContext {
                chain_id: context.l1_chain_id,
                parent_number: head.header.number,
                parent_hash: head.header.hash.to_vec(),
                target_timestamp: head.header.timestamp + 60,
            })
        } else {
            None
        };
        let actual_parent = actual_target.is_some();
        let part_started = Instant::now();
        let (batch, sidecar, target) = prove_cold_window(
            &context,
            &parent,
            &blocks,
            Some(part_blocks),
            Arc::clone(&store),
            actual_target,
        )
        .await
        .unwrap_or_else(|error| {
            panic!(
                "offline part {}..{to}: {error}; {}",
                parent.number + 1,
                cold.log_tail(60)
            )
        });
        assert_eq!(batch.blockNumber == u64::MAX, actual_parent);
        assert_eq!(cold.streaming_attestations().unwrap(), 1);
        let terminal = &blocks.last().unwrap().header;
        post_native_proof(&world, batch, sidecar, target, terminal.hash_slow()).await;
        // Receipt success is not progress: wait for the runtime's canonical
        // consumption and compare the exact accepted block-hash commitment.
        wait_for_safe_prefix_convergence(&[&world.node], to, TIMEOUT)
            .await
            .unwrap();
        assert_eq!(
            state_root(&l1_rpc, world.dep.eez_address, world.dep.rollup_id)
                .await
                .unwrap(),
            terminal.hash_slow()
        );
        for block in [blocks.first().unwrap(), &blocks[2], blocks.last().unwrap()] {
            observe_source(&world.l2_rpc(), &block.header, source(&block.header)).await;
        }
        parent = terminal.clone();
        expected.extend(blocks.into_iter().map(|block| block.header));
        drop(cold); // Next part has a new process, working directory and session.
        parts += 1;
        eprintln!(
            "{}",
            serde_json::json!({"checkpoint":"offline_catchup_part", "part":parts,
            "to":to, "block_hash":parent.hash_slow(), "state_root":parent.state_root,
            "elapsed_ms":part_started.elapsed().as_millis()})
        );
    }
    let source = |block: &Header| &sources[&block.number];
    assert!(parts >= 3);
    assert_eq!(parent.timestamp - genesis.timestamp, terminal_number);
    let follower = NodeHandle::start(
        "multipart-clean-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &follower_env,
    )
    .await
    .unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], parent.number, replay_timeout)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "independent multipart replay: {error:#}\n{}",
                follower.log_tail(100)
            )
        });
    let replayed = ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap());
    for original in &expected {
        let actual = replayed
            .get_block_by_number(original.number.into())
            .await
            .unwrap()
            .unwrap()
            .header;
        assert_eq!(actual.hash, original.hash_slow());
        assert_eq!(actual.state_root, original.state_root);
        assert_eq!(actual.mix_hash, source(original).mix_hash);
        if original.number % part_blocks <= 3 {
            observe_source(&follower.l2_rpc_url(), original, source(original)).await;
        }
    }
    let selected = source(&parent);
    for (index, number) in application_blocks.iter().copied().enumerate() {
        let expected_source = source(&expected[number as usize - 1]);
        let data = recordsCall {
            index: U256::from(index),
        }
        .abi_encode();
        let original = application_call(&world.l2_rpc(), parent.number, data.clone()).await;
        let replayed = application_call(&follower.l2_rpc_url(), parent.number, data).await;
        assert_eq!(original, replayed, "ordinary application state diverged");
        let observation = recordsCall::abi_decode_returns(&original).unwrap();
        assert_eq!(observation.l2Block, U256::from(number));
        assert_eq!(observation.number, expected_source.number);
        assert_eq!(observation.hash, expected_source.hash_slow());
        assert_eq!(
            observation.randao,
            U256::from_be_bytes(expected_source.mix_hash.0)
        );
    }
    for header in &historical_headers[..=selected.number as usize] {
        let encoded = hashAtCall {
            number: header.number,
        }
        .abi_encode();
        let original = application_call(&world.l2_rpc(), parent.number, encoded.clone()).await;
        let replayed = application_call(&follower.l2_rpc_url(), parent.number, encoded).await;
        assert_eq!(
            hashAtCall::abi_decode_returns(&original).unwrap(),
            header.hash_slow()
        );
        assert_eq!(
            replayed, original,
            "permanent history diverged at L1 {}",
            header.number
        );
    }
    eprintln!(
        "{}",
        serde_json::json!({"checkpoint":"offline_catchup_complete", "parts":parts,
        "historical_parts":parts - 1, "actual_parent_parts":1,
        "l1_rule_change_timestamp":rule_change,
        "first_part_terminal_source_age":first_source_age,
        "l2_blocks":parent.number, "elapsed_seconds":terminal_number,
        "minimum_history_seconds":history_seconds, "imported_l1_headers":selected.number + 1,
        "ordinary_transactions":application_blocks.len(),
        "l2_hash":parent.hash_slow(), "l2_state_root":parent.state_root, "wall_ms":started.elapsed().as_millis()})
    );
    first_source_age.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bounded_parts_use_cold_offline_signers_and_public_replay() {
    catch_up(120, 40, Duration::from_mins(30), Duration::from_mins(45)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_sources_use_recent_anchors_with_cold_offline_signers_and_public_replay() {
    let age = catch_up(2_000, 500, Duration::from_mins(30), Duration::from_mins(45)).await;
    assert!(age > 256, "fixture did not leave the L1 blockhash window");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "optional four-day stress run: 69,120 produced L1 blocks and 345,600 L2 blocks"]
async fn four_days_settle_in_parts_with_cold_offline_provers_and_independent_replay() {
    catch_up(
        4 * 86_400,
        10_000,
        Duration::from_mins(90),
        Duration::from_hours(6),
    )
    .await;
}
