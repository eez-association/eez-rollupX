//! Startup checks and native-L1 follower observability regressions.
//!
//! Posting tests below use the shared native L1 fixture, like the other integration targets.
//! Their local execution provider and submission RPC share one canonical chain,
//! which is required by the mandatory exact bundle preflight. The historical
//! Anvil harness deliberately uses a distinct embedded L1 for source simulation
//! and therefore cannot soundly exercise posting.

use std::time::Duration;

use alloy_primitives::{B256, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::BlockNumberOrTag;

use eez_testkit::signals;
use eez_testkit::{
    ANVIL_ADDR_3, ANVIL_KEY_1, Harness, NodeBinary, NodeConfig, NodeHandle, batches_posted,
    block_number_and_hash_at, override_env, send_l2_value_transfer_confirmed,
    setup_cross_chain_with_env, wait_for, wait_for_safe_prefix_convergence,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_mins(5);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn node_fixture_ignores_parent_dotenv() {
    let harness = Harness::fresh().await.unwrap();
    let env = harness.env().await.unwrap();
    let parent = tempfile::tempdir().unwrap();
    // Either file would make startup fail if loaded from the parent directory.
    for name in [".env", "deployments.env"] {
        std::fs::write(
            parent.path().join(name),
            "EEZ_L2_SYSTEM_ADDRESS=not-an-address\n",
        )
        .unwrap();
    }
    let datadir = parent.path().join("node");
    std::fs::create_dir(&datadir).unwrap();
    let node =
        NodeHandle::start_with_datadir("isolated-env", &datadir, &NodeConfig::default(), &env)
            .await
            .expect("node must not read parent deployment files");
    send_l2_value_transfer_confirmed(
        &node.l2_rpc_url(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(1u64),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("node did not become operational with isolated fixture environment");
    node.assert_no_process_death();
}

#[tokio::test]
async fn anvil_reorg_preserves_wall_clock() {
    let harness = Harness::for_reorg().await.unwrap();
    let chain = harness.chain();
    chain.set_interval_mining(0).await.unwrap();
    let provider = ProviderBuilder::new().connect_http(chain.rpc_url().parse().unwrap());
    let before = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    assert!(
        before.header.number >= 3,
        "fixture must have reorgable history"
    );
    let wall_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    harness.anvil.reorg(3).await.unwrap();
    let replacement = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replacement.header.number, before.header.number);
    assert_ne!(replacement.header.hash, before.header.hash);
    chain.mine().await.unwrap();
    let next = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.header.number, before.header.number + 1);
    assert!(next.header.timestamp > replacement.header.timestamp);
    assert!(
        next.header.timestamp >= wall_time,
        "Anvil's reorg left future blocks behind wall time: {} < {wall_time}",
        next.header.timestamp,
    );
}

/// An unregistered rollup ID keeps composer readiness closed before sequencing,
/// proof generation, or posting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failure_wrong_rollup_id() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let env = harness.env_with_rollup_id(999).await.unwrap();
    let datadir = tempfile::tempdir().unwrap();
    let node = NodeHandle::start_with_datadir(
        "wrong-rollup",
        datadir.path(),
        &NodeConfig::default(),
        &env,
    )
    .await
    .unwrap();

    wait_for(Duration::from_mins(1), || async {
        Ok((node.count_signal("eez.node.settlement.bootstrap_not_ready")? > 0).then_some(()))
    })
    .await
    .expect("unregistered rollup did not close composer readiness");

    chain
        .wait_for_l1_blocks(5, Duration::from_secs(30))
        .await
        .unwrap();

    let snapshot = chain.snapshot().await.unwrap();
    assert_eq!(snapshot.batches_posted, 0);
    assert_eq!(snapshot.executions_performed, 0);
    assert_eq!(
        snapshot.state_root,
        harness.l2_genesis_block_hash().unwrap()
    );
    assert_eq!(
        harness.successful_attestations().unwrap(),
        0,
        "unregistered rollup must be rejected before proof attestation",
    );
    node.assert_no_process_death();
}

/// Private fault proxy: only this follower's actual Beacon body requests fail.
async fn follower_beacon_fault_proxy(
    upstream: String,
) -> (
    String,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    tokio::task::JoinHandle<()>,
) {
    use http_body_util::Full;
    use hyper::{
        Request, Response,
        body::{Bytes, Incoming},
        service::service_fn,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let deny = Arc::new(AtomicBool::new(false));
    let control = Arc::clone(&deny);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let task = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let upstream = upstream.clone();
            let client = client.clone();
            let deny = Arc::clone(&deny);
            tokio::spawn(async move {
                let handler = service_fn(move |request: Request<Incoming>| {
                    let upstream = upstream.clone();
                    let client = client.clone();
                    let deny = Arc::clone(&deny);
                    async move {
                        let path = request.uri().path();
                        let (status, body) = if deny.load(Ordering::SeqCst)
                            && (path.contains("/blobs/") || path.contains("/blob_sidecars/"))
                        {
                            (
                                503,
                                Bytes::from_static(b"fixture required blob unavailable"),
                            )
                        } else {
                            match client
                                .get(format!(
                                    "{}{}",
                                    upstream.trim_end_matches('/'),
                                    request.uri()
                                ))
                                .send()
                                .await
                            {
                                Ok(response) => {
                                    let status = response.status().as_u16();
                                    let body = response.bytes().await.unwrap();
                                    assert!(body.len() <= 4 * 1024 * 1024);
                                    (status, body)
                                }
                                Err(_) => {
                                    (502, Bytes::from_static(b"fixture upstream unavailable"))
                                }
                            }
                        };
                        Ok::<_, std::convert::Infallible>(
                            Response::builder()
                                .status(status)
                                .header("content-type", "application/json")
                                .body(Full::new(body))
                                .unwrap(),
                        )
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(hyper_util::rt::TokioIo::new(socket), handler)
                    .await;
            });
        }
    });
    (url, control, task)
}

// Observe actual canonical native L1 receipts before accepting follower parity.
async fn wait_for_native_batches(world: &eez_testkit::CrossChainWorld, minimum: usize) {
    wait_for(DEFAULT_TIMEOUT, || async {
        world.node.assert_no_process_death();
        Ok(
            (batches_posted(&world.l1_rpc(), world.dep.eez_address, 0).await? >= minimum)
                .then_some(()),
        )
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "native settlements did not progress: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
}

/// An L1-only follower reconstructs an attested safe state without a sequencer RPC.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn happy_case_follower_l1_derived() {
    // Exact bundle preflight must read the same canonical native L1 that
    // receives submission; the legacy Anvil Harness cannot post these batches.
    let world = setup_cross_chain_with_env(&[("EEZ_SETTLEMENT_PURE_L2_MODE", "always".into())])
        .await
        .unwrap();
    let seq = &world.node;
    // Policy "always" posts included activity, not idle empty blocks. Keep
    // producing real work through the required-blob fault and recovery window.
    seq.run_tx_spammer(ANVIL_KEY_1);
    wait_for_native_batches(&world, 2).await;

    let datadir = tempfile::tempdir().unwrap();
    let mut env = world.partial_follower_env();
    let beacon = env
        .iter()
        .find(|(key, _)| *key == "EEZ_L1_BEACON_URL")
        .unwrap()
        .1
        .clone();
    let (fault_beacon, deny_blobs, beacon_task) = follower_beacon_fault_proxy(beacon).await;
    env = override_env(env, "EEZ_L1_BEACON_URL", &fault_beacon);
    let cfg = NodeConfig {
        binary: NodeBinary::Follower,
        genesis_path: Some(&world.cfg.l2_genesis.0),
    };
    let follower = NodeHandle::start_with_datadir("follower", datadir.path(), &cfg, &env)
        .await
        .unwrap();
    wait_for_safe_prefix_convergence(&[seq, &follower], 1, DEFAULT_TIMEOUT)
        .await
        .expect("follower safe chain did not converge with the sequencer");

    let (reference, evidence) = wait_for(DEFAULT_TIMEOUT, || async {
        let Some((number, _)) =
            block_number_and_hash_at(&follower.l2_rpc_url(), BlockNumberOrTag::Safe).await?
        else {
            return Ok(None);
        };
        let Some(record) =
            settlement_by_l2(&follower, serde_json::json!(format!("0x{number:x}"))).await?
        else {
            return Ok(None);
        };
        let reference = derivation_reference(&record);
        let evidence = derivation_evidence(&follower, reference.clone()).await?;
        Ok((evidence["evidence"]["outcome"] == "settled"
            && evidence["ready"] == true
            && !evidence["through"].is_null())
        .then_some((reference, evidence)))
    })
    .await
    .expect("fresh follower must expose actual STF evidence");
    assert_eq!(evidence["identity"]["role"], "follower");
    assert_eq!(evidence["identity"]["mode"], "l1_only");
    assert_eq!(evidence["evidence"]["reference"], reference);
    let executed: alloy_consensus::Header =
        serde_json::from_value(evidence["evidence"]["header"].clone()).unwrap();
    let canonical = ProviderBuilder::new()
        .connect_http(seq.l2_rpc_url().parse().unwrap())
        .get_block_by_number(executed.number.into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(executed.hash_slow(), canonical.header.hash);
    assert_eq!(executed.state_root, canonical.header.state_root);
    assert_eq!(
        derivation_evidence(seq, reference.clone()).await.unwrap()["identity"]["role"],
        "composer"
    );
    // The current native watcher, not the historical observer audit, must
    // identify the exact live dependency after repeated failed retrievals.
    deny_blobs.store(true, std::sync::atomic::Ordering::SeqCst);
    let blocked = wait_for(DEFAULT_TIMEOUT, || async {
        let value = derivation_evidence(&follower, reference.clone()).await?;
        Ok((value["requiredBlob"]["attempts"]
            .as_u64()
            .is_some_and(|count| count >= 3))
        .then_some(value))
    })
    .await
    .expect("live required blob failure must be affirmative and repeated");
    assert_eq!(blocked["requiredBlob"]["consumer"], "watcher");
    assert_eq!(blocked["requiredBlob"]["reason"], "beacon_unavailable");
    let missing_tx: B256 = blocked["requiredBlob"]["transactionHash"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let l1_provider = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let missing_receipt: serde_json::Value = l1_provider
        .client()
        .request("eth_getTransactionReceipt", (missing_tx,))
        .await
        .unwrap();
    assert_eq!(missing_receipt["status"], "0x1");
    assert_eq!(
        missing_receipt["blockHash"],
        blocked["requiredBlob"]["l1BlockHash"]
    );
    assert_eq!(missing_receipt["to"], blocked["identity"]["eez"]);
    let blocked_block = blocked["requiredBlob"]["l1BlockNumber"].as_u64().unwrap();
    deny_blobs.store(false, std::sync::atomic::Ordering::SeqCst);
    wait_for(DEFAULT_TIMEOUT, || async {
        let value = derivation_evidence(&follower, reference.clone()).await?;
        Ok((value["requiredBlob"].is_null()
            && value["through"]["number"]
                .as_u64()
                .is_some_and(|through| through >= blocked_block))
        .then_some(()))
    })
    .await
    .expect("retrieval recovery must clear blocker after reconciliation");
    wait_for_safe_prefix_convergence(&[seq, &follower], 1, DEFAULT_TIMEOUT)
        .await
        .unwrap();
    let previous_epoch = evidence["epoch"].clone();
    // A hard fixture kill may lose the newest in-memory Reth suffix. That
    // suffix legitimately re-executes at restart. Test reuse on the older first
    // settled block, whose ancestry is below the two observed settlements.
    let reused_record = settlement_by_l2(&follower, serde_json::json!("0x1"))
        .await
        .unwrap()
        .expect("first settled block correlation");
    let reused_reference = derivation_reference(&reused_record);
    assert_eq!(
        derivation_evidence(&follower, reused_reference.clone())
            .await
            .unwrap()["evidence"]["outcome"],
        "settled"
    );
    follower.assert_no_process_death();
    drop(follower);
    let restarted =
        NodeHandle::start_with_datadir("follower-restarted", datadir.path(), &cfg, &env)
            .await
            .unwrap();
    let reused = wait_for(DEFAULT_TIMEOUT, || async {
        let evidence = derivation_evidence(&restarted, reused_reference.clone()).await?;
        Ok((evidence["ready"] == true).then_some(evidence))
    })
    .await
    .unwrap();
    assert_ne!(previous_epoch, reused["epoch"]);
    assert!(
        restarted
            .log_count_matching(&["boot seeded from the persistent L1/L2 correlation index"])
            .unwrap()
            > 0,
        "restart must actually reuse surviving persistent history"
    );
    assert!(
        reused["evidence"].is_null(),
        "persisted/reused blocks must not fabricate new independent execution"
    );
    restarted.assert_no_process_death();
    seq.assert_no_process_death();
    beacon_task.abort();
}

/// Sequencer RPC advances unsafe state while L1 remains safe-authoritative.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn happy_case_follower_sequencer_rpc() {
    // Exact bundle preflight must read the same canonical native L1 that
    // receives submission; the legacy Anvil Harness cannot post these batches.
    let world = setup_cross_chain_with_env(&[("EEZ_SETTLEMENT_PURE_L2_MODE", "always".into())])
        .await
        .unwrap();
    let seq = &world.node;
    // Policy "always" posts included activity, not idle empty blocks. Keep
    // producing real work through the required-blob fault and recovery window.
    seq.run_tx_spammer(ANVIL_KEY_1);

    let seq_rpc = seq.l2_rpc_url();
    let mut follower_env = override_env(
        world.partial_follower_env(),
        "RUST_LOG",
        "warn,eez_follower::unsafe_head=info",
    );
    // The L1-only fixture has no sequencer entry for override_env to replace.
    follower_env.push(("EEZ_SEQUENCER_RPC", seq_rpc));
    let follower_cfg = NodeConfig {
        binary: NodeBinary::Follower,
        genesis_path: Some(&world.cfg.l2_genesis.0),
    };
    let follower = NodeHandle::start("follower", &follower_cfg, &follower_env)
        .await
        .unwrap();

    wait_for_native_batches(&world, 2).await;

    wait_for_safe_prefix_convergence(&[seq, &follower], 1, DEFAULT_TIMEOUT)
        .await
        .expect("follower safe block never matched the sequencer chain");

    let capability = derivation_evidence(&follower,serde_json::json!({"l1BlockNumber":0,"l1BlockHash":B256::ZERO,"transactionHash":B256::ZERO,"transactionIndex":0,"l2BlockNumber":0})).await.unwrap();
    assert_eq!(capability["identity"]["role"], "follower");
    assert_eq!(capability["identity"]["mode"], "sequencer_overlay");

    let follower_head_signals = [
        signals::FOLLOWER_HEAD_ADVANCED,
        signals::FOLLOWER_HEAD_SYNCING,
    ];
    let unsafe_head_events_before = follower.count_signals(&follower_head_signals).unwrap();
    wait_for(DEFAULT_TIMEOUT, || {
        std::future::ready(
            follower
                .count_signals(&follower_head_signals)
                .map(|n| (n > unsafe_head_events_before).then_some(())),
        )
    })
    .await
    .expect("follower never reported a sequencer-RPC unsafe-head FCU outcome");

    follower.assert_no_process_death();
    seq.assert_no_process_death();
}

fn derivation_reference(record: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({"l1BlockNumber":correlation_quantity(record,"l1BlockNumber"),
        "l1BlockHash":record["l1BlockHash"],"transactionHash":record["l1TransactionHash"],
        "transactionIndex":correlation_quantity(record,"l1TransactionIndex"),
        "l2BlockNumber":correlation_quantity(&record["matchedL2Block"],"number")})
}
async fn derivation_evidence(
    node: &NodeHandle,
    reference: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let provider = ProviderBuilder::new().connect_http(node.l2_rpc_url().parse()?);
    Ok(provider
        .client()
        .request("eez_getDerivationEvidence", (reference,))
        .await?)
}

async fn settlement_by_l2(
    node: &NodeHandle,
    selector: serde_json::Value,
) -> anyhow::Result<Option<serde_json::Value>> {
    let provider = ProviderBuilder::new().connect_http(node.l2_rpc_url().parse()?);
    Ok(provider
        .client()
        .request("eez_getSettlementByL2Block", (selector,))
        .await?)
}

fn correlation_quantity(record: &serde_json::Value, field: &str) -> u64 {
    let raw = record[field]
        .as_str()
        .unwrap_or_else(|| panic!("correlation field {field} is not a quantity"));
    u64::from_str_radix(raw.strip_prefix("0x").unwrap_or(raw), 16)
        .unwrap_or_else(|error| panic!("invalid correlation {field}: {error}"))
}
