//! Real role processes and databases. The canonical reader, embedded executor,
//! offline signer profile and submission target are independently configured.

use std::{
    convert::Infallible,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use alloy_primitives::{B256, Bytes, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::BlockNumberOrTag;
use alloy_sol_types::SolCall;
use eez_testkit::{
    ANVIL_ADDR_3, ANVIL_KEY_1, Anvil, DEV_CHAIN_ID, IValue, NativeL1Profile, NativeWorld,
    NodeBinary, NodeConfig, NodeHandle, OUTBOUND_USER, ProofSignerConfig, ProofSignerHandle,
    bind_l2_genesis_reference, deploy_protocol_dev, genesis_block_hash, native, onchain_nonce,
    send_l2_value_transfer_confirmed, setup_cross_chain_with_env, setup_cross_chain_with_profile,
    setup_native_history, sign_and_send, signer_address, wait_for,
    wait_for_safe_prefix_convergence,
};

const TIMEOUT: Duration = Duration::from_mins(3);

// This proxy changes only eth_chainId. All headers, state, witnesses and
// execution still come from the real canonical node; it models endpoint drift.
struct ChainIdProxy {
    url: String,
    chain_id: Arc<AtomicU64>,
    task: tokio::task::JoinHandle<()>,
}

impl ChainIdProxy {
    async fn start(upstream: String) -> Self {
        use http_body_util::BodyExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let chain_id = Arc::new(AtomicU64::new(DEV_CHAIN_ID));
        let selected = chain_id.clone();
        let task = tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap();
            let mut connections = tokio::task::JoinSet::new();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let client = client.clone();
                let upstream = upstream.clone();
                let selected = selected.clone();
                while connections.try_join_next().is_some() {}
                connections.spawn(async move {
                    let service = hyper::service::service_fn(
                        move |request: hyper::Request<hyper::body::Incoming>| {
                            let client = client.clone();
                            let upstream = upstream.clone();
                            let selected = selected.clone();
                            async move {
                                let body = request.into_body().collect().await.unwrap().to_bytes();
                                let request: serde_json::Value =
                                    serde_json::from_slice(&body).unwrap_or_default();
                                let body = if request["method"] == "eth_chainId" {
                                    serde_json::to_vec(
                                        &serde_json::json!({"jsonrpc":"2.0", "id":request["id"],
                                    "result":format!("{:#x}", selected.load(Ordering::SeqCst))}),
                                    )
                                    .unwrap()
                                } else {
                                    let response = async {
                                        client.post(upstream).header("content-type", "application/json")
                                            .body(body).send().await?.error_for_status()?.bytes().await
                                    }.await;
                                    match response {
                                        Ok(bytes) => bytes.to_vec(),
                                        Err(_) => serde_json::to_vec(&serde_json::json!({
                                            "jsonrpc":"2.0", "id":request["id"],
                                            "error":{"code":-32000,"message":"identity fixture upstream unavailable"}
                                        })).unwrap(),
                                    }
                                };
                                Ok::<_, Infallible>(hyper::Response::new(
                                    http_body_util::Full::new(hyper::body::Bytes::from(body)),
                                ))
                            }
                        },
                    );
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Self {
            url,
            chain_id,
            task,
        }
    }
}

impl Drop for ChainIdProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

alloy_sol_types::sol! {
    function updateVerificationKey(address proofSystem, bytes32 newVkey) external;
}

fn composer_env(world: &NativeWorld) -> Vec<(&'static str, String)> {
    let mut env = world.partial_follower_env();
    env.extend([
        (
            "EEZ_L1_CHAIN",
            if world.cfg.l1_profile == NativeL1Profile::GnosisPrague {
                "testing-gnosis"
            } else {
                "testing"
            }
            .into(),
        ),
        ("EEZ_L1_POSTER_KEY", world.cfg.poster_key.into()),
        ("EEZ_PROVER_URL", world.proof_signer.endpoint().into()),
        (
            "EEZ_ATTESTER_ADDRESS",
            format!("{:#x}", signer_address(world.cfg.attester_key).unwrap()),
        ),
    ]);
    env
}

fn retain_fixture(label: &str, path: &Path) {
    if let Some(root) = std::env::var_os("EEZ_TEST_LOG_DIR") {
        let fixture = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        std::fs::copy(
            path,
            Path::new(&root).join(format!("{label}-{fixture}.json")),
        )
        .unwrap();
    }
}

fn changed_l1(world: &NativeWorld, path: &Path, wrong_chain: bool) {
    let mut genesis: alloy_genesis::Genesis =
        serde_json::from_slice(&std::fs::read(&world.cfg.l1_genesis.0).unwrap()).unwrap();
    if wrong_chain {
        genesis.config.chain_id += 1;
    } else {
        genesis.extra_data = Bytes::from_static(b"issue145-distinct-genesis");
        // Make the difference observable in execution as well as the header.
        genesis.alloc.get_mut(&ANVIL_ADDR_3).unwrap().balance += U256::from(123);
    }
    std::fs::write(path, serde_json::to_vec(&genesis).unwrap()).unwrap();
    retain_fixture(path.file_stem().unwrap().to_str().unwrap(), path);
}

fn l1_config(world: &NativeWorld, path: &Path) -> String {
    if world.cfg.l1_profile == NativeL1Profile::GnosisPrague {
        format!("gnosis:{}", path.display())
    } else {
        path.to_string_lossy().into_owned()
    }
}

async fn rejected(node: &mut NodeHandle, message: &str) {
    let status = wait_for(TIMEOUT, || async { Ok(node.exit_status()) })
        .await
        .unwrap_or_else(|error| panic!("no startup rejection: {error:#}\n{}", node.log_tail(80)));
    assert!(!status.success(), "startup unexpectedly succeeded");
    let log = std::fs::read_to_string(&node.log_path).unwrap();
    assert!(log.contains(message), "missing {message:?}:\n{log}");
    assert_eq!(
        node.count_signal(eez_testkit::signals::NODE_COMPOSER_SPAWNED)
            .unwrap(),
        0
    );
    assert_eq!(relay_dispatches(node, 0), 0);
    node.fixture_ready();
}

async fn spawned(node: &NodeHandle) {
    wait_for(TIMEOUT, || async {
        node.assert_no_process_death();
        Ok((node.count_signal(eez_testkit::signals::NODE_COMPOSER_SPAWNED)? > 0).then_some(()))
    })
    .await
    .unwrap();
}

fn postings(node: &NodeHandle, cursor: usize) -> Vec<serde_json::Value> {
    node.signals_since(cursor)
        .unwrap()
        .into_iter()
        .filter(|signal| signal.name == "eez.composer.posting.v1")
        .map(|signal| {
            serde_json::from_str(signal.fields["posting_event"].as_str().unwrap()).unwrap()
        })
        .collect()
}

fn relay_dispatches(node: &NodeHandle, cursor: usize) -> usize {
    postings(node, cursor)
        .iter()
        .filter(|event| event["stage"] == "dispatched")
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_authenticates_canonical_reference_and_local_profiles() {
    startup_matrix(NativeL1Profile::EthereumOsaka).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_ethereum_prague_authenticates_canonical_reference_and_local_profiles() {
    startup_matrix(NativeL1Profile::EthereumPrague).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn startup_gnosis_prague_authenticates_canonical_reference_and_local_profiles() {
    startup_matrix(NativeL1Profile::GnosisPrague).await;
}

async fn startup_matrix(profile: NativeL1Profile) {
    let world = setup_cross_chain_with_profile(profile, &[]).await.unwrap();
    retain_fixture("startup-canonical-genesis", &world.cfg.l1_genesis.0);
    retain_fixture("startup-l2-genesis", &world.cfg.l2_genesis.0);
    spawned(&world.node).await;
    let alternate = Anvil::fresh().await.unwrap();
    let canonical = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let other = ProviderBuilder::new().connect_http(alternate.rpc_url.parse().unwrap());
    assert_eq!(
        canonical.get_chain_id().await.unwrap(),
        other.get_chain_id().await.unwrap()
    );
    assert_ne!(
        canonical
            .get_block_by_number(0.into())
            .await
            .unwrap()
            .unwrap()
            .header
            .hash,
        other
            .get_block_by_number(0.into())
            .await
            .unwrap()
            .unwrap()
            .header
            .hash,
    );
    let files = tempfile::tempdir().unwrap();
    let wrong_chain = files.path().join("wrong-chain.json");
    let wrong_genesis = files.path().join("wrong-genesis.json");
    changed_l1(&world, &wrong_chain, true);
    changed_l1(&world, &wrong_genesis, false);
    for role in [NodeBinary::Composer, NodeBinary::Follower] {
        let cfg = NodeConfig {
            binary: role,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        };
        let base = match role {
            NodeBinary::Composer => composer_env(&world),
            NodeBinary::Follower => world.partial_follower_env(),
        };
        // Both providers execute real state. The canonical Anvil's different
        // genesis must fail at reference authentication, after chain readiness.
        let mut env = base.clone();
        env.push(("EEZ_L1_RPC_URL", alternate.rpc_url.clone()));
        let mut node = NodeHandle::spawn("identity-canonical-genesis", &cfg, &env)
            .await
            .unwrap();
        rejected(&mut node, "L1 genesis reference header").await;
        assert!(
            std::fs::read_to_string(&node.log_path)
                .unwrap()
                .contains("L1 chain and finality ready")
        );
        drop(node);

        let _: serde_json::Value = other
            .raw_request("anvil_setChainId".into(), (DEV_CHAIN_ID + 1,))
            .await
            .unwrap();
        let mut env = base.clone();
        env.push(("EEZ_L1_RPC_URL", alternate.rpc_url.clone()));
        let mut node = NodeHandle::spawn("identity-canonical-chain", &cfg, &env)
            .await
            .unwrap();
        rejected(
            &mut node,
            &format!(
                "EEZ_L1_RPC_URL serves chain {}, expected {DEV_CHAIN_ID}",
                DEV_CHAIN_ID + 1
            ),
        )
        .await;
        drop(node);
        let _: serde_json::Value = other
            .raw_request("anvil_setChainId".into(), (DEV_CHAIN_ID,))
            .await
            .unwrap();

        let mut env = base.clone();
        env.extend([
            ("EEZ_L1_CHAIN_CONFIG", l1_config(&world, &wrong_chain)),
            (
                "EEZ_L1_CHAIN_PATH",
                wrong_chain.to_string_lossy().into_owned(),
            ),
        ]);
        let mut node = NodeHandle::spawn("identity-local-chain", &cfg, &env)
            .await
            .unwrap();
        rejected(
            &mut node,
            match role {
                NodeBinary::Composer => "L1 reference/profile chain mismatch",
                NodeBinary::Follower => "L1 chain configuration ID differs from EEZ_L1_CHAIN_ID",
            },
        )
        .await;
        drop(node);

        if matches!(role, NodeBinary::Composer) {
            let mut env = base;
            env.push((
                "EEZ_L1_CHAIN_PATH",
                wrong_genesis.to_string_lossy().into_owned(),
            ));
            let mut node = NodeHandle::spawn("identity-signer-profile", &cfg, &env)
                .await
                .unwrap();
            rejected(
                &mut node,
                "prover validation identity differs from local execution rules",
            )
            .await;
        } else {
            let control = NodeHandle::start("identity-matching-follower", &cfg, &base)
                .await
                .unwrap();
            wait_for(TIMEOUT, || async {
                control.assert_no_process_death();
                // The follower starts this watcher only after reference
                // authentication and boot catch-up; its own INFO target is filtered.
                Ok(
                    (control.log_count_matching(&["poll cycle: fetched latest L1 block"])? > 0)
                        .then_some(()),
                )
            })
            .await
            .unwrap();
            control.assert_no_divergence_failure_logs();
        }
    }
    assert_eq!(world.proof_signer.streaming_attestations().unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_id_different_embedded_genesis_rejects_canonical_proof_anchor_before_signing() {
    let mut world = setup_native_history(&[]).await.unwrap();
    retain_fixture("simulation-canonical-genesis", &world.cfg.l1_genesis.0);
    retain_fixture("simulation-l2-genesis", &world.cfg.l2_genesis.0);
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("embedded-genesis.json");
    changed_l1(&world, &path, false);
    let l1 = eez_l1::chain_config::load_l1_chain_spec(&path).unwrap();
    let mut profile = world.dep.profile.clone();
    profile.l1ConfigHash = eez_l1::chain_config::offline_l1_rules(&l1)
        .unwrap()
        .config_hash;
    assert_ne!(profile.commitment(), world.dep.profile.commitment());
    let signer = ProofSignerHandle::spawn(&ProofSignerConfig {
        rollup_manager_address: world.cfg.rollup_manager_address,
        registry_address: world.cfg.eez_address,
        chain_config: &world.cfg.l2_genesis.0,
        l1_chain_config: path.to_string_lossy().into_owned(),
        rollup_id: world.cfg.rollup_id,
        signer_key: world.cfg.attester_key,
        vkey: profile.verification_key(
            world.dep.eez_address,
            world.dep.proof_system_address,
            signer_address(world.cfg.attester_key).unwrap(),
        ),
        proof_system: world.cfg.proof_system_address,
        max_request_blocks: None,
    })
    .await
    .unwrap();
    // Deliberately provision the same wrong local profile at both signer and
    // deployed key. Agreement must not authorize execution on another L1 state.
    let update = sign_and_send(
        &world.l1_rpc(),
        world.cfg.deployer_key,
        DEV_CHAIN_ID,
        onchain_nonce(&world.l1_rpc(), world.cfg.deployer_key)
            .await
            .unwrap(),
        Some(world.dep.rollup_manager_address),
        U256::ZERO,
        updateVerificationKeyCall {
            proofSystem: world.dep.proof_system_address,
            newVkey: profile.verification_key(
                world.dep.eez_address,
                world.dep.proof_system_address,
                signer_address(world.cfg.attester_key).unwrap(),
            ),
        }
        .abi_encode(),
        200_000,
    )
    .await
    .unwrap();
    let canonical = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let receipt = wait_for(TIMEOUT, || async {
        Ok(canonical.get_transaction_receipt(update).await?)
    })
    .await
    .unwrap();
    assert!(receipt.status());
    world
        .l1_reorg_control
        .as_mut()
        .unwrap()
        .hold()
        .await
        .unwrap();
    let held = canonical
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header;
    let mut env = composer_env(&world);
    env.extend([
        ("EEZ_L1_CHAIN_PATH", path.to_string_lossy().into_owned()),
        ("EEZ_PROVER_URL", signer.endpoint().into()),
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "always".into()),
    ]);
    let mut node = NodeHandle::spawn(
        "identity-paired-genesis",
        &NodeConfig {
            binary: NodeBinary::Composer,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &env,
    )
    .await
    .unwrap();
    spawned(&node).await;
    send_l2_value_transfer_confirmed(
        &node.l2_rpc_url(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(1),
        TIMEOUT,
    )
    .await
    .unwrap();
    let embedded = ProviderBuilder::new().connect_http(node.embedded_l1_rpc_url().parse().unwrap());
    // Align block heights before releasing the canonical miner. A missing
    // local height would not exercise the proof-anchor hash comparison.
    wait_for(TIMEOUT, || async {
        Ok((embedded.get_block_number().await? >= held.number).then_some(()))
    })
    .await
    .unwrap();
    assert_eq!(
        embedded.get_block_number().await.unwrap(),
        held.number,
        "executor missed the controlled height alignment"
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let timestamp = world.cfg.genesis_time + ((now - world.cfg.genesis_time) / 5 + 1) * 5;
    world
        .l1_reorg_control
        .as_mut()
        .unwrap()
        .resume_slot(held.hash, timestamp)
        .await
        .unwrap();
    assert_eq!(
        embedded.get_chain_id().await.unwrap(),
        canonical.get_chain_id().await.unwrap()
    );
    assert_ne!(
        embedded
            .get_block_by_number(0.into())
            .await
            .unwrap()
            .unwrap()
            .header
            .hash,
        canonical
            .get_block_by_number(0.into())
            .await
            .unwrap()
            .unwrap()
            .header
            .hash
    );
    assert_ne!(
        embedded
            .get_balance(ANVIL_ADDR_3)
            .block_id(0.into())
            .await
            .unwrap(),
        canonical
            .get_balance(ANVIL_ADDR_3)
            .block_id(0.into())
            .await
            .unwrap()
    );
    wait_for(TIMEOUT, || async {
        node.assert_no_process_death();
        Ok(
            (node.log_count_matching(&["terminal witness and canonical proof anchor disagree"])?
                >= 2)
                .then_some(()),
        )
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "state boundary was not reached: {error:#}\n{}",
            node.log_tail(100)
        )
    });
    let log = std::fs::read_to_string(&node.log_path).unwrap();
    assert!(log.contains("prover configuration agreement established"));
    assert_eq!(signer.streaming_attestations().unwrap(), 0);
    assert_eq!(relay_dispatches(&node, 0), 0);
    assert_eq!(
        node.count_signal(eez_testkit::signals::TX_POISON_EVICTED)
            .unwrap(),
        0
    );
    assert_eq!(world.chain().snapshot().await.unwrap().batches_posted, 0);
    node.fixture_ready();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nonzero_reference_with_separate_canonical_and_embedded_providers_settles_and_replays() {
    let mut world = setup_native_history(&[]).await.unwrap();
    let finalized = world
        .l1_reorg_control
        .as_mut()
        .unwrap()
        .finalize_head()
        .await
        .unwrap();
    let canonical = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let reference = wait_for(TIMEOUT, || async {
        Ok(canonical
            .get_block_by_number(BlockNumberOrTag::Finalized)
            .await?
            .filter(|block| block.header.number > 0))
    })
    .await
    .unwrap()
    .header;
    assert_eq!(
        (reference.number, reference.hash),
        (finalized.number, finalized.hash)
    );
    let files = tempfile::tempdir().unwrap();
    let l2_genesis = files.path().join("nonzero-l2.json");
    std::fs::copy(&world.cfg.l2_genesis.0, &l2_genesis).unwrap();
    bind_l2_genesis_reference(&l2_genesis, &reference.inner, 1, 5, None).unwrap();
    retain_fixture("nonzero-canonical-genesis", &world.cfg.l1_genesis.0);
    retain_fixture("nonzero-l2-genesis", &l2_genesis);
    let mut profile = world.dep.profile.clone();
    profile.genesisHash = genesis_block_hash(&l2_genesis).unwrap();
    profile.genesisTimestamp = reference.timestamp + 5;
    let dep = deploy_protocol_dev(
        &world.l1_rpc(),
        world.cfg.deployer_key,
        world.cfg.attester_key,
        profile,
    )
    .await
    .unwrap();
    let signer = ProofSignerHandle::spawn(&ProofSignerConfig {
        rollup_manager_address: dep.rollup_manager_address,
        registry_address: dep.eez_address,
        chain_config: &l2_genesis,
        l1_chain_config: world.cfg.l1_genesis.0.to_string_lossy().into_owned(),
        rollup_id: dep.rollup_id,
        signer_key: world.cfg.attester_key,
        vkey: dep.profile.verification_key(
            dep.eez_address,
            dep.proof_system_address,
            signer_address(world.cfg.attester_key).unwrap(),
        ),
        proof_system: dep.proof_system_address,
        max_request_blocks: None,
    })
    .await
    .unwrap();
    let settings = vec![
        ("EEZ_L1_RPC_URL", world.l1_rpc()),
        ("EEZ_L1_TARGET_RPC_URL", world.fault_rpc_url().to_owned()),
        ("EEZ_REGISTRY_ADDRESS", format!("{:#x}", dep.eez_address)),
        (
            "EEZ_ROLLUP_MANAGER_ADDRESS",
            format!("{:#x}", dep.rollup_manager_address),
        ),
        (
            "EEZ_ECDSA_PROOF_SYSTEM_ADDRESS",
            format!("{:#x}", dep.proof_system_address),
        ),
        ("EEZ_REGISTRY_DEPLOY_BLOCK", dep.deploy_block.to_string()),
        (
            "EEZ_TEST_L2_GENESIS_PATH",
            l2_genesis.to_string_lossy().into_owned(),
        ),
        ("EEZ_PROVER_URL", signer.endpoint().into()),
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "always".into()),
    ];
    let mirror = native::spawn_composer(&world, world.fault_rpc_url(), &settings)
        .await
        .unwrap();
    spawned(&mirror.node).await;
    let embedded =
        ProviderBuilder::new().connect_http(mirror.node.embedded_l1_rpc_url().parse().unwrap());
    assert_ne!(mirror.node.embedded_l1_rpc_url(), world.l1_rpc());
    assert_eq!(
        embedded
            .get_block_by_number(0.into())
            .await
            .unwrap()
            .unwrap()
            .header
            .hash,
        canonical
            .get_block_by_number(0.into())
            .await
            .unwrap()
            .unwrap()
            .header
            .hash
    );
    let mut env = world.partial_follower_env();
    env.extend_from_slice(&settings);
    let follower = NodeHandle::start(
        "identity-nonzero-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&l2_genesis),
        },
        &env,
    )
    .await
    .unwrap();
    let other_l1 = files.path().join("follower-other-genesis.json");
    changed_l1(&world, &other_l1, false);
    env.push((
        "EEZ_L1_CHAIN_CONFIG",
        other_l1.to_string_lossy().into_owned(),
    ));
    let different_genesis_follower = NodeHandle::start(
        "identity-nonzero-other-profile",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&l2_genesis),
        },
        &env,
    )
    .await
    .unwrap();
    let hash = send_l2_value_transfer_confirmed(
        &mirror.node.l2_rpc_url(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(7),
        TIMEOUT,
    )
    .await
    .unwrap();
    let l2 = ProviderBuilder::new().connect_http(mirror.node.l2_rpc_url().parse().unwrap());
    let receipt = l2.get_transaction_receipt(hash).await.unwrap().unwrap();
    wait_for_safe_prefix_convergence(
        &[&mirror.node, &follower, &different_genesis_follower],
        receipt.block_number.unwrap(),
        TIMEOUT,
    )
    .await
    .unwrap();
    assert!(signer.streaming_attestations().unwrap() > 0);
    assert!(relay_dispatches(&mirror.node, 0) > 0);
    let canonical_url = world.l1_rpc();
    assert!(
        eez_testkit::Chain::new(&canonical_url, &dep)
            .batches_posted()
            .await
            .unwrap()
            > 0
    );
    assert_eq!(
        l2.get_block_by_number(0.into())
            .await
            .unwrap()
            .unwrap()
            .header
            .hash,
        dep.profile.genesisHash
    );
    mirror.assert_healthy();
    follower.assert_no_divergence_failure_logs();
    different_genesis_follower.assert_no_divergence_failure_logs();
    assert_ne!(reference.hash, B256::ZERO);
    assert_eq!(canonical.get_chain_id().await.unwrap(), DEV_CHAIN_ID);

    let bad_genesis = files.path().join("wrong-nonzero-reference.json");
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&l2_genesis).unwrap()).unwrap();
    document["config"]["rollupX"]["l1Reference"]["hash"] =
        serde_json::json!(B256::repeat_byte(0x91));
    std::fs::write(&bad_genesis, serde_json::to_vec(&document).unwrap()).unwrap();
    retain_fixture("wrong-nonzero-reference", &bad_genesis);
    assert_eq!(
        genesis_block_hash(&bad_genesis).unwrap(),
        dep.profile.genesisHash
    );
    let bad_signer = ProofSignerHandle::spawn(&ProofSignerConfig {
        rollup_manager_address: dep.rollup_manager_address,
        registry_address: dep.eez_address,
        chain_config: &bad_genesis,
        l1_chain_config: world.cfg.l1_genesis.0.to_string_lossy().into_owned(),
        rollup_id: dep.rollup_id,
        signer_key: world.cfg.attester_key,
        vkey: dep.profile.verification_key(
            dep.eez_address,
            dep.proof_system_address,
            signer_address(world.cfg.attester_key).unwrap(),
        ),
        proof_system: dep.proof_system_address,
        max_request_blocks: None,
    })
    .await
    .unwrap();
    for binary in [NodeBinary::Composer, NodeBinary::Follower] {
        let mut env = match binary {
            NodeBinary::Composer => composer_env(&world),
            NodeBinary::Follower => world.partial_follower_env(),
        };
        env.extend_from_slice(&settings);
        env.push(("EEZ_PROVER_URL", bad_signer.endpoint().into()));
        let mut node = NodeHandle::spawn(
            "identity-wrong-nonzero-reference",
            &NodeConfig {
                binary,
                genesis_path: Some(&bad_genesis),
            },
            &env,
        )
        .await
        .unwrap();
        rejected(&mut node, "finalized genesis reference header").await;
        if matches!(binary, NodeBinary::Composer) {
            assert!(
                std::fs::read_to_string(&node.log_path)
                    .unwrap()
                    .contains("prover configuration agreement established")
            );
        }
    }
    assert_eq!(bad_signer.streaming_attestations().unwrap(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_embedded_database_rejects_changed_genesis_and_reopens_original() {
    let mut world = setup_cross_chain_with_env(&[]).await.unwrap();
    retain_fixture("database-original-genesis", &world.cfg.l1_genesis.0);
    retain_fixture("database-l2-genesis", &world.cfg.l2_genesis.0);
    spawned(&world.node).await;
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("other-genesis.json");
    changed_l1(&world, &path, false);
    let error = world
        .restart_composer_with_env(&[("EEZ_L1_CHAIN_PATH", path.to_string_lossy().into_owned())])
        .await
        .expect_err("existing database must reject another genesis");
    assert!(
        format!("{error:#}")
            .contains("genesis hash in the storage does not match the specified chainspec"),
        "{error:#}"
    );
    rejected(
        &mut world.node,
        "genesis hash in the storage does not match the specified chainspec",
    )
    .await;
    assert!(
        !std::fs::read_to_string(&world.node.log_path)
            .unwrap()
            .contains("prover configuration agreement established")
    );
    let original = world.cfg.l1_genesis.0.to_string_lossy().into_owned();
    world
        .restart_composer_with_env(&[("EEZ_L1_CHAIN_PATH", original)])
        .await
        .unwrap();
    spawned(&world.node).await;
    world.node.assert_no_divergence_failure_logs();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_rpc_chain_id_retains_owned_request_until_endpoint_recovers() {
    let mut world = setup_cross_chain_with_env(&[]).await.unwrap();
    retain_fixture("rpc-canonical-genesis", &world.cfg.l1_genesis.0);
    retain_fixture("rpc-l2-genesis", &world.cfg.l2_genesis.0);
    let proxy = ChainIdProxy::start(world.l1_rpc()).await;
    for setting in ["EEZ_L1_RPC_URL", "EEZ_L1_TARGET_RPC_URL"] {
        let canonical = world.l1_rpc();
        world
            .restart_composer_with_env(&[
                (
                    "EEZ_L1_RPC_URL",
                    if setting == "EEZ_L1_RPC_URL" {
                        proxy.url.clone()
                    } else {
                        canonical.clone()
                    },
                ),
                (
                    "EEZ_L1_TARGET_RPC_URL",
                    if setting == "EEZ_L1_TARGET_RPC_URL" {
                        proxy.url.clone()
                    } else {
                        canonical
                    },
                ),
            ])
            .await
            .unwrap();
        spawned(&world.node).await;
        let front = ProviderBuilder::new().connect_http(world.l2_xchain().parse().unwrap());
        let before = world.chain().snapshot().await.unwrap();
        let signatures = world.proof_signer.streaming_attestations().unwrap();
        let cursor = world.node.signal_cursor().unwrap();
        proxy.chain_id.store(DEV_CHAIN_ID + 1, Ordering::SeqCst);
        let hash = sign_and_send(
            &world.l2_xchain(),
            OUTBOUND_USER,
            world.l2_chain_id,
            onchain_nonce(&world.l2_rpc(), OUTBOUND_USER).await.unwrap(),
            Some(world.outbound_proxy),
            U256::ZERO,
            IValue::setValueCall { v: U256::from(81) }.abi_encode(),
            600_000,
        )
        .await
        .unwrap();
        let expected = if setting == "EEZ_L1_RPC_URL" {
            "submission RPC and selected L1 profile have different chain IDs"
        } else {
            "target_changed"
        };
        wait_for(TIMEOUT, || async {
            world.node.assert_no_process_death();
            let rejected = if setting == "EEZ_L1_RPC_URL" {
                std::fs::read_to_string(&world.node.log_path)?
                    .lines()
                    .filter(|line| {
                        line.contains(expected)
                            && line
                                .contains("cross-chain compose failed; retaining held transactions")
                    })
                    .count()
                    >= 2
            } else {
                let events = postings(&world.node, cursor);
                let targets: std::collections::HashSet<_> = events
                    .iter()
                    .filter(|event| {
                        event["stage"] == "excluded"
                            && event["detail"]["reason"] == "target_changed"
                            && events.iter().any(|earlier| {
                                earlier["attempt_id"] == event["attempt_id"]
                                    && earlier["stage"] == "repriced"
                                    && earlier["sequence"].as_u64().unwrap()
                                        < event["sequence"].as_u64().unwrap()
                            })
                    })
                    .map(|event| {
                        (
                            event["target_block"].to_string(),
                            event["target_timestamp"].to_string(),
                        )
                    })
                    .collect();
                targets.len() >= 2
            };
            Ok(rejected.then_some(()))
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{setting} did not reach its guard: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
        let status: serde_json::Value = front
            .client()
            .request("eez_getCrossChainTransaction", (hash,))
            .await
            .unwrap();
        assert!(!status.is_null(), "accepted request lost: {hash}");
        let state = status["ownership"]["state"]
            .as_str()
            .or_else(|| status["lifecycle"].as_str())
            .unwrap();
        assert!(
            ["queued", "composing", "in_flight", "awaiting_evidence"].contains(&state),
            "request lost live ownership: {status}"
        );
        let blocked = world.chain().snapshot().await.unwrap();
        assert_eq!(
            (
                blocked.batches_posted,
                blocked.executions_performed,
                blocked.state_root
            ),
            (
                before.batches_posted,
                before.executions_performed,
                before.state_root
            )
        );
        assert!(
            !world
                .node
                .signals_since(cursor)
                .unwrap()
                .iter()
                .any(|signal| [
                    eez_testkit::signals::TX_POISON_EVICTED,
                    eez_testkit::signals::TX_NONCE_CHAIN_EVICTED,
                ]
                .contains(&signal.name.as_str()))
        );
        assert_eq!(relay_dispatches(&world.node, cursor), 0);
        if setting == "EEZ_L1_RPC_URL" {
            assert_eq!(
                world.proof_signer.streaming_attestations().unwrap(),
                signatures
            );
        } else {
            assert!(
                world.proof_signer.streaming_attestations().unwrap() > signatures,
                "target rejection must follow actual proof and final simulation"
            );
        }
        proxy.chain_id.store(DEV_CHAIN_ID, Ordering::SeqCst);
        // No restart or resubmission: the same admission must survive the
        // inconclusive attempt and complete with canonical effect evidence.
        crate::transaction_references::committed_references(&world, hash).await;
        let dispatch = world
            .node
            .signals_since(cursor)
            .unwrap()
            .into_iter()
            .rev()
            .find(|signal| {
                signal.name == eez_testkit::signals::COMPOSER_BUNDLE_DISPATCHED
                    && signal.fields["user_hashes"]
                        .as_str()
                        .is_some_and(|users| users.contains(&hash.to_string()))
            })
            .expect("retained source must be recomposed without a new admission");
        assert_eq!(
            dispatch.fields["attempt_counts"], "[0]",
            "inconclusive observations charged the user"
        );
        world.node.assert_no_divergence_failure_logs();
    }
}
