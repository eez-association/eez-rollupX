//! Cross-chain integration and nonce-gap regression tests.

#[path = "common/observer.rs"]
mod observer;

#[path = "common/traces.rs"]
mod traces;

#[path = "common/quota.rs"]
mod quota;

#[path = "common/loss_alerts.rs"]
mod loss_alerts;

use alloy_primitives::{Address, Bytes, TxHash, U256, keccak256};
use alloy_sol_types::{SolCall, SolError, SolEvent, SolValue};

use eez_protocol::EEZL2_ADDRESS;
use eez_testkit::signals;
use eez_testkit::{
    ANVIL_ATTESTER_KEY, DEV_CHAIN_ID, IEEZL2Direct, IEmptyCall, INBOUND_USER, INestedSetterInner,
    INestedSetterOuter, IReturnData, IReturnDataWrapper, IRevertBubbleWrapper, IRevertingTarget,
    ISetterWrapper, IValue, IValueNoRet, L1_ROLLUP_ID, OUTBOUND_USER, ProverMutation,
    SETTLE_TIMEOUT, Scenario, ScenarioCall, StateRead, TARGET_DEPLOYER, account_code,
    assert_latest_batch_signature, batches_posted, call_read, call_revert_data, count_events,
    cross_chain_source_proxy, deploy_nested_setter_inner, deploy_nested_setter_outer, events_since,
    l2_balance, l2_value, last_proxy_result, onchain_nonce, read_state_word, receipt_ok,
    run_scenarios, safe_block_hash, setter_call, setup_cross_chain, setup_cross_chain_codeless,
    setup_cross_chain_empty_call, setup_cross_chain_nested_setter,
    setup_cross_chain_outbound_return_data, setup_cross_chain_proxied,
    setup_cross_chain_return_data, setup_cross_chain_reverting, sign_and_send, signer_address,
    state_root, value_no_ret, value_read, wait_for,
};

const WAVE_SETTERS: &[u64] = &[7, 11, 17];
const WAVE_DEPOSITS: &[u128] = &[
    1_000_000_000_000_000,
    2_000_000_000_000_000,
    3_000_000_000_000_000,
];

// The embedded bundle path preserves ordering but not all-or-nothing inclusion,
// so a wave must check every source receipt independently.
async fn assert_all_transactions_succeeded(
    w: &eez_testkit::CrossChainWorld,
    rpc_url: &str,
    hashes: &[TxHash],
    label: &str,
) {
    assert!(!hashes.is_empty(), "no {label} transactions were submitted");
    for &hash in hashes {
        let landed = wait_for(
            SETTLE_TIMEOUT,
            || async move { receipt_ok(rpc_url, hash).await },
        )
        .await;
        let status = match landed {
            Ok(status) => status,
            Err(err) => {
                panic!(
                    "{label} transaction {hash} did not land: {err:#}{}",
                    w.settlement_diagnostics()
                );
            }
        };
        assert!(status, "{label} transaction {hash} reverted");
    }
}

// Reject-policy scenarios must be mined failures, not ingress rejections.
async fn assert_transaction_reverted(rpc_url: &str, hash: TxHash, label: &str) {
    let status = wait_for(SETTLE_TIMEOUT, || {
        let rpc_url = rpc_url.to_owned();
        async move { receipt_ok(&rpc_url, hash).await }
    })
    .await
    .unwrap_or_else(|err| panic!("{label} transaction {hash} did not land: {err:#}"));
    assert!(!status, "{label} transaction {hash} unexpectedly succeeded");
}

fn calls_read(target: Address) -> StateRead {
    call_read(target, "calls()", IEmptyCall::callsCall {}.abi_encode())
}

fn received_read(target: Address) -> StateRead {
    call_read(
        target,
        "received()",
        IEmptyCall::receivedCall {}.abi_encode(),
    )
}

fn last_value_read(target: Address) -> StateRead {
    call_read(
        target,
        "lastValue()",
        IEmptyCall::lastValueCall {}.abi_encode(),
    )
}

fn return_length_read(wrapper: Address) -> StateRead {
    call_read(
        wrapper,
        "lastReturnDataLength()",
        IReturnDataWrapper::lastReturnDataLengthCall {}.abi_encode(),
    )
}

fn return_hash_read(wrapper: Address) -> StateRead {
    call_read(
        wrapper,
        "lastReturnDataHash()",
        IReturnDataWrapper::lastReturnDataHashCall {}.abi_encode(),
    )
}

// Derive the source address observed on the destination chain.
async fn attributed_sender(
    destination_rpc: &str,
    manager: Address,
    source: Address,
    source_rollup_id: u64,
) -> Address {
    cross_chain_source_proxy(destination_rpc, manager, source, source_rollup_id)
        .await
        .expect("derive destination-side source proxy")
}

// Assert the number and sender of newly emitted `ValueSet` events.
async fn assert_value_set_attribution(
    rpc: &str,
    target: Address,
    before: usize,
    expected_new: usize,
    expected_sender: Address,
    label: &str,
) {
    let logs = wait_for(SETTLE_TIMEOUT, || {
        let rpc = rpc.to_owned();
        async move {
            let logs = events_since(&rpc, target, IValue::ValueSet::SIGNATURE_HASH, 0).await?;
            Ok((logs.len() >= before + expected_new).then_some(logs))
        }
    })
    .await
    .unwrap_or_else(|err| panic!("{label}: destination events were not retained: {err:#}"));
    assert_eq!(
        logs.len(),
        before + expected_new,
        "{label} must emit exactly {expected_new} new ValueSet event(s)",
    );
    for log in &logs[before..] {
        let event = IValue::ValueSet::decode_log(&log.inner).expect("decode ValueSet");
        assert_eq!(
            event.by, expected_sender,
            "{label} must attribute the destination write to the originating source proxy",
        );
    }
}

/// Exercise one signed and derived transaction in each direction, including
/// verification of the posted proof's recomputed public-input hash.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn minimal_bidirectional_cross_chain_smoke() {
    let _ = bidirectional_cross_chain_smoke(&[]).await;
}

async fn bidirectional_cross_chain_smoke(
    extra_env: &[(&'static str, String)],
) -> (eez_testkit::CrossChainWorld, [TxHash; 2]) {
    let w = eez_testkit::setup_cross_chain_with_env(extra_env)
        .await
        .unwrap();
    let hashes = submit_bidirectional_smoke(&w).await;
    assert_bidirectional_smoke(&w, hashes).await;
    (w, hashes)
}

async fn submit_bidirectional_smoke(w: &eez_testkit::CrossChainWorld) -> [TxHash; 2] {
    let l1_rpc = w.l1_rpc();
    let l2_rpc = w.l2_rpc();
    let inbound = sign_and_send(
        &w.l1_xchain(),
        INBOUND_USER,
        DEV_CHAIN_ID,
        onchain_nonce(&l1_rpc, INBOUND_USER).await.unwrap(),
        Some(w.setter_proxy),
        U256::ZERO,
        IValue::setValueCall {
            v: U256::from(41u64),
        }
        .abi_encode(),
        600_000,
    )
    .await
    .expect("inbound smoke transaction must be admitted");
    let outbound = sign_and_send(
        &w.l2_xchain(),
        OUTBOUND_USER,
        w.l2_chain_id,
        onchain_nonce(&l2_rpc, OUTBOUND_USER).await.unwrap(),
        Some(w.outbound_proxy),
        U256::ZERO,
        IValue::setValueCall {
            v: U256::from(43u64),
        }
        .abi_encode(),
        900_000,
    )
    .await
    .expect("outbound smoke transaction must be admitted");

    [inbound, outbound]
}

async fn assert_bidirectional_smoke(
    w: &eez_testkit::CrossChainWorld,
    [inbound, outbound]: [TxHash; 2],
) {
    let attester = signer_address(ANVIL_ATTESTER_KEY).unwrap();
    let l1_rpc = w.l1_rpc();
    let l2_rpc = w.l2_rpc();
    let inbound_hashes = [inbound];
    let outbound_hashes = [outbound];
    tokio::join!(
        assert_all_transactions_succeeded(w, &l1_rpc, &inbound_hashes, "inbound smoke"),
        assert_all_transactions_succeeded(w, &l2_rpc, &outbound_hashes, "outbound smoke"),
    );

    let (eez, rollup_id) = (w.cfg.eez_address, w.cfg.rollup_id);
    let converged = wait_for(SETTLE_TIMEOUT, || {
        let (l1_rpc, l2_rpc) = (l1_rpc.clone(), l2_rpc.clone());
        async move {
            let inbound_applied = l2_value(&l2_rpc, w.value_l2).await? == U256::from(41u64);
            let outbound_applied = l2_value(&l1_rpc, w.outbound_value).await? == U256::from(43u64);
            let l1_root = state_root(&l1_rpc, eez, rollup_id).await?;
            let l2_root = safe_block_hash(&l2_rpc).await?;
            Ok((inbound_applied && outbound_applied && l2_root == Some(l1_root)).then_some(()))
        }
    })
    .await;
    if let Err(err) = converged {
        panic!(
            "minimal smoke did not settle both directions: {err:#}{}",
            w.settlement_diagnostics(),
        );
    }

    assert!(
        batches_posted(&l1_rpc, w.cfg.eez_address, w.dep.deploy_block)
            .await
            .unwrap()
            >= 1,
        "minimal smoke must post at least one batch",
    );
    assert_latest_batch_signature(&l1_rpc, &w.dep, attester)
        .await
        .expect("posted proof must recover over the recomputed public-input hash");
    assert_eq!(
        w.node
            .count_signals(&[
                signals::COMPOSER_POISON_EVICTION_COMPLETED,
                signals::TX_POISON_EVICTED,
                signals::TX_NONCE_CHAIN_EVICTED,
            ])
            .unwrap(),
        0,
        "valid smoke transactions must not be evicted",
    );
    w.node.assert_no_divergence_failure_logs();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lifecycle_bidirectional_cross_chain_smoke() {
    lifecycle_bidirectional_smoke(&[]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lifecycle_policy_bidirectional_cross_chain_smoke() {
    lifecycle_bidirectional_smoke(&[
        ("EEZ_SETTLEMENT_POLICY_ENABLED", "true".into()),
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "always".into()),
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10000".into()),
        ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
        ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "100000000000".into()),
        (
            "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS",
            "100000000000".into(),
        ),
    ])
    .await;
}

/// Reachable OTLP export from real Composer/client/signer processes, including
/// a multi-source batch and a retryable transport failure before valid signing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn traced_bidirectional_cross_chain_smoke() {
    let directory = tempfile::tempdir().unwrap();
    let spool = directory.path().join("lifecycle");
    let collector = traces::Collector::start().await.unwrap();
    let composer_config = directory.path().join("composer-telemetry.json");
    let signer_config = directory.path().join("signer-telemetry.json");
    for (service, path) in [
        ("eez-composer", &composer_config),
        ("eez-proof-signer", &signer_config),
    ] {
        let mut config = serde_json::json!({
            "identity": {"environment": "devnet", "deployment": "native-trace-pilot", "service": service, "instance": "fixture"},
            "traces": {"receiver": collector.address, "sample_ratio": 1.0, "queue_capacity": 2048, "export_timeout_ms": 1000}
        });
        if service == "eez-composer" {
            config["lifecycle"] = serde_json::json!({"directory": spool});
        }
        std::fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
    }
    let world = eez_testkit::setup_cross_chain_with_telemetry(
        &[(
            "EEZ_TELEMETRY_CONFIG",
            composer_config.to_string_lossy().into_owned(),
        )],
        &signer_config,
    )
    .await
    .unwrap();
    let proxy = world.prover_proxy.as_ref().unwrap();
    proxy.set_mutation(ProverMutation::Unavailable);
    let hashes = submit_bidirectional_smoke(&world).await;
    // Recover only after actual exported retries reference a preparation that
    // contains both admitted sources. A sleep or proxy attempt count could match
    // unrelated historical/pure-L2 work and falsely claim batch retry coverage.
    wait_for(SETTLE_TIMEOUT, || async {
        Ok(collector.retried_batch(&spool, &hashes)?.then_some(()))
    })
    .await
    .unwrap_or_else(|error| {
        collector.report().unwrap();
        panic!(
            "missing multi-source native retry: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    proxy.set_mutation(ProverMutation::None);
    assert_bidirectional_smoke(&world, hashes).await;
    let trace_ids = wait_for(SETTLE_TIMEOUT, || async {
        collector.connected_sources(&spool, &hashes)
    })
    .await
    .unwrap_or_else(|error| {
        collector.report().unwrap();
        panic!(
            "missing native trace ancestry/source identities: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    let composer_logs = std::fs::read_to_string(&world.node.log_path).unwrap();
    let signer_logs = world.proof_signer.log_tail(10_000);
    let correlated_event = |logs: &str, event: &str, trace_id: &str| {
        logs.lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .any(|record| {
                record["fields"]["event_name"] == event
                    && (record["fields"]["trace_id"] == trace_id
                        || record["span"]["trace_id"] == trace_id)
            })
    };
    for trace_id in trace_ids {
        assert!(
            correlated_event(&composer_logs, "eez.prover_client.attested", &trace_id),
            "Composer JSON log correlation missing"
        );
        assert!(
            correlated_event(&signer_logs, "eez.proof_signer.window_signed", &trace_id),
            "signer JSON log correlation missing"
        );
    }
    collector.report().unwrap();
    world.node.assert_no_process_death();
    world.proof_signer.assert_alive();
}

/// The required quota runner supplies a private 3GiB ext4/FUSE mount and a
/// stopped Alloy reader. Missing controls fail; ordinary native suites leave
/// this test ignored because they do not own that physical filesystem.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the isolated physical ENOSPC runner"]
async fn dynamic_enospc_bidirectional_cross_chain_smoke() {
    let control = quota::Control::required().unwrap();
    let metrics_lease = eez_testkit::PortLease::tcp();
    let metrics_address = format!("127.0.0.1:{}", metrics_lease.port());
    let configuration = control.directory.join("telemetry.json");
    std::fs::write(&configuration, serde_json::to_vec(&serde_json::json!({
        "identity":{"environment":"devnet","deployment":"native-quota","service":"eez-composer","instance":"fixture"},
        "lifecycle":{"directory":control.spool,"segment_bytes":65536,"retained_bytes":131072,"retention_seconds":3600},
        "metrics":{"listen":metrics_address}
    })).unwrap()).unwrap();
    drop(metrics_lease);
    let world = eez_testkit::setup_cross_chain_with_env(&[(
        "EEZ_TELEMETRY_CONFIG",
        configuration.to_string_lossy().into_owned(),
    )])
    .await
    .unwrap();
    let healthy = wait_for(SETTLE_TIMEOUT, || async {
        Ok(control.stats()?.filter(|stats| {
            stats.written_records > 0 && stats.io_errors == 0 && stats.dropped_records == 0
        }))
    })
    .await
    .expect("writer must be healthy before the physical fault");
    control
        .signal("healthy", &serde_json::to_value(healthy).unwrap())
        .unwrap();
    let full = control.wait("full").await.unwrap();
    assert_eq!(full["enospc"], true);
    assert_eq!(full["held_unlinked"], true);
    wait_for(SETTLE_TIMEOUT, || async {
        let metrics = scrape_native_metrics(&metrics_address).await?;
        Ok(metric_sample(
            &metrics,
            "reth_eez_telemetry_writer_errors_total{reason=\"io\"}",
        )
        .filter(|count| *count > 0.))
    })
    .await
    .expect("the actual native writer must fail before submitting protocol work");
    let hashes = submit_bidirectional_smoke(&world).await;
    assert_bidirectional_smoke(&world, hashes).await;
    let metrics = scrape_native_metrics(&metrics_address).await.unwrap();
    let errors = metric_sample(
        &metrics,
        "reth_eez_telemetry_writer_errors_total{reason=\"io\"}",
    )
    .unwrap();
    let drops = metric_sample(
        &metrics,
        "reth_eez_telemetry_dropped_events_total{signal=\"lifecycle\",reason=\"io\"}",
    )
    .unwrap();
    assert!(errors > 0. && drops > 0.);
    assert!(
        !control.directory.join("released.json").exists(),
        "disk was released before semantic acceptance"
    );
    control.signal("completed_full", &serde_json::json!({"writer_errors":errors,"dropped_io":drops,"source_count":hashes.len()})).unwrap();
    let released = control.wait("released").await.unwrap();
    let released_at = released["observed_at_unix_ms"].as_u64().unwrap();
    wait_for(SETTLE_TIMEOUT, || async {
        control.stats_after(released_at)
    })
    .await
    .expect("a new control record must be written after physical capacity release");
    let fresh_hashes = submit_bidirectional_smoke(&world).await;
    assert!(
        fresh_hashes.iter().all(|hash| !hashes.contains(hash)),
        "post-release admissions must use fresh signed source identities"
    );
    assert_bidirectional_smoke(&world, fresh_hashes).await;
    wait_for(SETTLE_TIMEOUT, || async {
        Ok(control.persisted_commits(&fresh_hashes)?.then_some(()))
    })
    .await
    .expect("fresh post-release source admission and committed events must be persisted");
    let recovered = wait_for(SETTLE_TIMEOUT, || async {
        Ok(control.stats()?.filter(|stats| {
            stats.written_records > healthy.written_records
                && stats.io_errors > 0
                && stats.dropped_records > 0
                && stats.dropped_sequence_min.is_some()
                && stats.dropped_sequence_max.is_some()
        }))
    })
    .await
    .expect("writer must resume with cumulative explicit loss without a node restart");
    world.node.assert_no_process_death();
    world.node.assert_no_divergence_failure_logs();
    control
        .signal("recovered", &{
            let mut report = serde_json::to_value(recovered).unwrap();
            report["fresh_post_release_persisted_commits"] = serde_json::json!(fresh_hashes.len());
            report
        })
        .unwrap();
}

/// Storage and collector availability must not gate admitted protocol work.
/// This tests startup spool unavailability and a live but non-serving collector;
/// dynamic ENOSPC recovery and comparative overhead have separate acceptance gates.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unavailable_telemetry_bidirectional_cross_chain_smoke() {
    let directory = tempfile::tempdir().unwrap();
    let spool = directory.path().join("private-spool-sentinel");
    let private_body = "PRIVATE_SPOOL_BODY_MUST_NOT_APPEAR_IN_LOGS";
    // A regular file cannot become a directory, even when tests run as root.
    // This deterministic fixture fault is storage unavailability, not ENOSPC.
    std::fs::write(&spool, private_body).unwrap();
    let configuration = directory.path().join("telemetry.json");
    let metrics_lease = eez_testkit::PortLease::tcp();
    let metrics_address = format!("127.0.0.1:{}", metrics_lease.port());
    // Retain the listening socket but never accept/respond. This exercises the
    // configured exporter timeout without racing another process for a free port.
    let stalled_collector = eez_testkit::PortLease::tcp();
    let config = serde_json::json!({
        "identity": {
            "environment": "devnet", "deployment": "telemetry-failure",
            "service": "eez-composer", "instance": "fixture"
        },
        "lifecycle": {"directory": spool},
        "metrics": {"listen": metrics_address},
        "traces": {
            "receiver": format!("127.0.0.1:{}", stalled_collector.port()),
            "sample_ratio": 1.0, "queue_capacity": 16, "export_timeout_ms": 20
        }
    });
    std::fs::write(&configuration, serde_json::to_vec(&config).unwrap()).unwrap();
    drop(metrics_lease);
    let (world, hashes) = bidirectional_cross_chain_smoke(&[(
        "EEZ_TELEMETRY_CONFIG",
        configuration.to_string_lossy().into_owned(),
    )])
    .await;
    // The shared helper verifies source receipts, both destination effects,
    // canonical L1-root/L2-safe convergence and the independently recomputed
    // posted proof signature, plus absence of poison eviction/divergence.
    let metrics = wait_for(SETTLE_TIMEOUT, || {
        let address = metrics_address.clone();
        async move {
            let metrics = scrape_native_metrics(&address).await?;
            let completed = ["inbound", "outbound"].into_iter().all(|direction| {
                metric_sample(&metrics, &format!("reth_eez_native_admissions_total{{direction=\"{direction}\"}}")) == Some(1.)
                    && metric_sample(&metrics, &format!("reth_eez_native_terminal_observations_total{{direction=\"{direction}\",outcome=\"committed\"}}")) == Some(1.)
                    && metric_sample(&metrics, &format!("reth_eez_request_duration_seconds_count{{direction=\"{direction}\",outcome=\"committed\"}}")) == Some(1.)
                    && ["queued_ready", "queued_blocked", "queued_unknown", "in_flight"].into_iter().all(|state| {
                        metric_sample(&metrics, &format!("reth_eez_native_queue_transactions{{direction=\"{direction}\",state=\"{state}\"}}")) == Some(0.)
                    })
            });
            let loss_visible = metric_sample(&metrics, "reth_eez_lifecycle_writer_enabled") == Some(0.)
                && metric_sample(&metrics, "reth_eez_trace_export_enabled") == Some(1.)
                && metric_sample(&metrics, "reth_eez_telemetry_dropped_events_total{signal=\"trace\",reason=\"export_failure\"}").is_some_and(|count| count > 0.)
                && metric_sample(&metrics, "reth_eez_trace_exported_spans_total") == Some(0.);
            Ok((completed && loss_visible).then_some(metrics))
        }
    }).await.expect("protocol work must complete while storage/collector loss stays explicit");
    assert!(spool.is_file());
    assert_eq!(std::fs::read_to_string(&spool).unwrap(), private_body);
    // This notification precedes subscriber initialization and is plain stderr,
    // so the testkit's JSON-event matcher deliberately does not include it.
    let logs = std::fs::read_to_string(&world.node.log_path).unwrap();
    assert_eq!(
        logs.lines()
            .filter(|line| *line
                == "EEZ lifecycle exporter disabled: storage or writer lock unavailable")
            .count(),
        1,
        "configured exporter failure must be explicit once at startup"
    );
    assert!(!logs.contains(private_body));
    assert!(!logs.contains("private-spool-sentinel"));
    for hash in hashes {
        assert!(
            !metrics.contains(&format!("{hash:x}")),
            "transaction identity leaked into metric labels"
        );
    }
    world.node.assert_no_process_death();
    world.node.assert_no_divergence_failure_logs();
    // Keep both the fault and its owned port alive until all semantic checks end.
    drop(world);
    drop(stalled_collector);
}

/// Actual native drops must reach the operator and recover without restarting
/// the process or interrupting successful execution in either direction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_trace_loss_alert_delivery_and_recovery() {
    let metrics_lease = eez_testkit::PortLease::tcp();
    let metrics_address = format!("127.0.0.1:{}", metrics_lease.port());
    let mut controller = loss_alerts::Controller::start(&metrics_address)
        .await
        .expect("start owned native alert backends");
    drop(metrics_lease);
    let (world, hashes) = bidirectional_cross_chain_smoke(&[(
        "EEZ_TELEMETRY_CONFIG",
        controller.config.to_string_lossy().into_owned(),
    )])
    .await;
    let firing = controller
        .wait("firing")
        .await
        .expect("native loss notification");
    let drops = firing["drops"]
        .as_f64()
        .expect("actual native drop counter");
    assert!(drops > 0.);
    world.node.assert_no_process_death();

    // A new nonce alone does not prove a new destination effect when both waves
    // set the same value. Require an additional event from each destination too.
    let destinations = [
        (world.l2_rpc(), world.value_l2),
        (world.l1_rpc(), world.outbound_value),
    ];
    let mut before = [0; 2];
    for (index, (rpc, target)) in destinations.iter().enumerate() {
        before[index] = count_events(rpc, *target, IValue::ValueSet::SIGNATURE_HASH, 0)
            .await
            .unwrap();
    }
    controller
        .signal("recover")
        .expect("restore owned collector");
    let fresh = submit_bidirectional_smoke(&world).await;
    assert!(
        fresh
            .iter()
            .zip(hashes)
            .all(|(next, previous)| *next != previous)
    );
    assert_bidirectional_smoke(&world, fresh).await;
    for (index, (rpc, target)) in destinations.iter().enumerate() {
        assert_eq!(
            count_events(rpc, *target, IValue::ValueSet::SIGNATURE_HASH, 0)
                .await
                .unwrap(),
            before[index] + 1,
            "recovered wave must execute one new destination effect"
        );
    }
    let recovered = controller
        .wait("resolved")
        .await
        .expect("native recovery notification");
    assert!(recovered["drops"].as_f64().unwrap() >= drops);
    let metrics = scrape_native_metrics(&metrics_address).await.unwrap();
    assert!(
        metric_sample(&metrics, "reth_eez_trace_exported_spans_total")
            .is_some_and(|count| count > 0.)
    );
    world.node.assert_no_process_death();
    world.node.assert_no_divergence_failure_logs();
    controller
        .finish()
        .await
        .expect("verify and clean up owned alert fixture");
    drop(world);
}

async fn lifecycle_bidirectional_smoke(extra_env: &[(&'static str, String)]) {
    use alloy_consensus::Transaction as _;
    use alloy_provider::{Provider as _, ProviderBuilder};
    use eez_telemetry::{
        reader,
        schema::{Direction, Event, Record},
    };
    let directory = tempfile::tempdir().unwrap();
    let spool = directory.path().join("lifecycle");
    let configuration = directory.path().join("telemetry.json");
    let metrics_lease = eez_testkit::PortLease::tcp();
    let metrics_address = format!("127.0.0.1:{}", metrics_lease.port());
    let config = serde_json::json!({
        "identity": {
            "environment": "devnet",
            "deployment": "native-capture",
            "service": "eez-composer",
            "instance": "fixture"
        },
        "lifecycle": {"directory": spool},
        "metrics": {"listen": metrics_address}
    });
    std::fs::write(&configuration, serde_json::to_vec(&config).unwrap()).unwrap();
    let mut env = extra_env.to_vec();
    env.push((
        "EEZ_TELEMETRY_CONFIG",
        configuration.to_string_lossy().into_owned(),
    ));
    // Keep testkit's process-wide port identity reserved; release its socket
    // before the helper starts the node child that binds the configured endpoint.
    drop(metrics_lease);
    let (world, hashes) = bidirectional_cross_chain_smoke(&env).await;
    let observer = observer::Observer::start(&world, &spool, "native-capture")
        .await
        .unwrap();
    let genesis = alloy_rpc_types_eth::BlockNumberOrTag::Number(0);
    let expected_genesis = [
        eez_testkit::block_number_and_hash_at(&world.l1_rpc(), genesis)
            .await
            .unwrap()
            .unwrap()
            .1,
        eez_testkit::block_number_and_hash_at(&world.l2_rpc(), genesis)
            .await
            .unwrap()
            .unwrap()
            .1,
    ];
    let observed = wait_for(SETTLE_TIMEOUT, || {
        let spool = spool.clone();
        async move {
            let mut chunks = std::collections::BTreeMap::new();
            for id in reader::inventory(&spool)? {
                let mut offset = 0;
                // Controlled fixture budget, including idle preparation history.
                for _ in 0..64 {
                    let batch = reader::read_segment(&spool, id, offset)?;
                    let Record::Header {
                        identity,
                        epoch,
                        segment_id,
                        ..
                    } = serde_json::from_slice(&batch.header)?
                    else {
                        panic!("native lifecycle header required")
                    };
                    assert_eq!(identity.deployment, "native-capture");
                    assert_eq!(identity.service, "eez-composer");
                    assert_eq!(epoch, id.epoch);
                    assert_eq!(segment_id, id.segment);
                    for line in batch.lines {
                        if let Record::Data(record) = serde_json::from_slice(&line.bytes)? {
                            chunks
                                .entry((record.epoch, record.sequence))
                                .or_insert_with(Vec::new)
                                .push(record);
                        }
                    }
                    offset = batch.next_offset;
                    if batch.snapshot_complete {
                        break;
                    }
                }
            }
            let mut events = Vec::new();
            for (id, chunks) in chunks {
                let present = chunks
                    .iter()
                    .map(|chunk| chunk.chunk_index)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len();
                if present < chunks[0].chunk_count as usize {
                    continue;
                }
                let payload = eez_telemetry::assemble(&chunks).map_err(anyhow::Error::msg)?;
                events.push((id, payload.event));
            }
            let admitted = hashes.iter().all(|hash| {
                events.iter().any(|(_, event)| {
                    matches!(event, Event::Admitted { source, .. }
                        if source.transaction_hash.0 == hash.0)
                })
            });
            let ready = hashes.iter().all(|hash| {
                events.iter().any(|(_, event)| {
                    matches!(event, Event::Readiness { member, blocked: None, .. }
                        if member.source.transaction_hash.0 == hash.0)
                })
            });
            let dispatched = hashes.iter().all(|hash| {
                events.iter().any(|(_, event)| {
                    let Event::AttemptPrepared {
                        attempt,
                        members,
                        details: Some(_),
                        ..
                    } = event
                    else {
                        return false;
                    };
                    if !members
                        .iter()
                        .any(|member| member.source.transaction_hash.0 == hash.0)
                    {
                        return false;
                    }
                    events.iter().any(|(_, event)| {
                        let Event::Dispatched { attempt: sent, .. } = event else {
                            return false;
                        };
                        sent == attempt
                            || events.iter().any(|(_, event)| {
                                matches!(event, Event::AttemptRepriced { previous, replacement }
                                if previous == attempt && replacement == sent)
                            })
                    })
                })
            });
            let terminal = hashes.iter().all(|hash| {
                events.iter().any(|(_, event)| {
                    matches!(event, Event::Terminal { member, outcome: eez_telemetry::schema::Outcome::Committed, assurance: eez_telemetry::schema::Assurance::NodeReconciled, reconciliation: Some(_), .. }
                        if member.source.transaction_hash.0 == hash.0)
                })
            });
            Ok((admitted && ready && dispatched && terminal).then_some(events))
        }
    })
    .await;
    let events = match observed {
        Ok(events) => events,
        Err(error) => {
            let retained = directory.keep();
            panic!(
                "both native admission/readiness/attempt/terminal paths must emit complete lifecycle events: {error:#}; retained lifecycle fixture at {}",
                retained.display()
            );
        }
    };
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    for (index, hash) in hashes.into_iter().enumerate() {
        let matches = events
            .iter()
            .filter(|(_, event)| {
                matches!(event, Event::Admitted { source, .. }
                    if source.transaction_hash.0 == hash.0)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            matches.len(),
            1,
            "one admitted identity per accepted source"
        );
        let (id, Event::Admitted { source, .. }) = matches[0] else {
            unreachable!()
        };
        assert_eq!(source.genesis_hash.0, expected_genesis[index].0);
        assert_eq!(
            source.chain_id,
            if index == 0 {
                DEV_CHAIN_ID
            } else {
                world.l2_chain_id
            }
        );
        assert_eq!(
            source.rollup_id,
            if index == 0 { 0 } else { world.cfg.rollup_id }
        );
        assert_eq!(
            source.direction,
            if index == 0 {
                Direction::Inbound
            } else {
                Direction::Outbound
            }
        );
        for (_, event) in &events {
            if let Event::Readiness { member, .. } = event
                && member.source.transaction_hash.0 == hash.0
            {
                assert_eq!((member.admission.epoch, member.admission.sequence), *id);
            }
            if let Event::AttemptPrepared {
                members,
                details: Some(details),
                sync_height,
                encoded_bytes,
                blob_count,
                blob_allowance,
                ..
            } = event
            {
                assert_eq!(members.len(), details.plans.len());
                assert_eq!(details.l2_chain_id, world.l2_chain_id);
                assert_eq!(details.l2_genesis_hash.0, expected_genesis[1].0);
                assert_eq!(details.rollup_id, world.cfg.rollup_id);
                let preparation = details.preparation.expect("native pre-signing identity");
                let starts = events
                    .iter()
                    .filter(|((epoch, sequence), event)| {
                        *epoch == preparation.epoch
                            && *sequence == preparation.sequence
                            && matches!(event, Event::PreparationStarted {sync, members: proposed, l2_chain_id, l2_genesis_hash, rollup_id}
                            if sync.number == *sync_height && sync.hash == details.sync_hash
                            && proposed == members && *l2_chain_id == details.l2_chain_id
                            && *l2_genesis_hash == details.l2_genesis_hash && *rollup_id == details.rollup_id)
                    })
                    .count();
                assert_eq!(
                    starts, 1,
                    "attempt must link to its exact pre-signing operation"
                );
                let finishes = events
                    .iter()
                    .filter(|(_, event)| {
                        matches!(event,
                    Event::PreparationFinished {preparation: finished, ..}
                    if *finished == preparation)
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    finishes.len(),
                    1,
                    "successful preparation finishes once before attempt registration"
                );
                assert!(matches!(
                    finishes[0].1,
                    Event::PreparationFinished {
                        outcome: eez_telemetry::schema::PreparationOutcome::Prepared,
                        ..
                    }
                ));
                assert!(*encoded_bytes > 0);
                assert_eq!(
                    encoded_bytes.div_ceil(eez_blob_codec::DATA_BYTES_PER_BLOB as u64),
                    u64::from(*blob_count)
                );
                assert!(blob_count <= blob_allowance);
                for member in members
                    .iter()
                    .filter(|member| member.source.transaction_hash.0 == hash.0)
                {
                    assert_eq!((member.admission.epoch, member.admission.sequence), *id);
                }
            }
        }
        let terminals = events
            .iter()
            .filter_map(|(_, event)| match event {
                Event::Terminal {
                    member,
                    attempt: Some(attempt),
                    outcome,
                    assurance,
                    reconciliation: Some(reconciliation),
                    anchor,
                    elapsed_ns,
                } if member.source.transaction_hash.0 == hash.0 => Some((
                    member,
                    attempt,
                    outcome,
                    assurance,
                    reconciliation,
                    anchor,
                    elapsed_ns,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            terminals.len(),
            1,
            "one native terminal decision per admitted canary source"
        );
        let (member, attempt, outcome, assurance, reconciliation, anchor, elapsed_ns) =
            terminals[0];
        assert_eq!((member.admission.epoch, member.admission.sequence), *id);
        assert_eq!(*outcome, eez_telemetry::schema::Outcome::Committed);
        assert_eq!(*assurance, eez_telemetry::schema::Assurance::NodeReconciled);
        assert!(
            anchor.is_none(),
            "receipt references belong to the reconciliation, not the scan watermark"
        );
        let parent = events
            .iter()
            .find(|((epoch, sequence), _)| {
                *epoch == reconciliation.epoch && *sequence == reconciliation.sequence
            })
            .expect("linked reconciliation event");
        let Event::Reconciled {
            attempt: reconciled_attempt,
            anchor: scan,
            details: Some(details),
        } = &parent.1
        else {
            panic!("complete reconciliation evidence")
        };
        assert_eq!(attempt, reconciled_attempt);
        assert_eq!(details.l1.genesis_hash.0, expected_genesis[0].0);
        assert_eq!(details.l2.genesis_hash.0, expected_genesis[1].0);
        assert_eq!(details.l2.rollup_id, world.cfg.rollup_id);
        let resolution = details
            .resolutions
            .iter()
            .find(|resolution| resolution.member == *member)
            .expect("exact admission resolution");
        assert_eq!(resolution.owner, *attempt);
        assert_eq!(resolution.elapsed_ns, *elapsed_ns);
        assert_eq!(
            resolution.disposition,
            eez_telemetry::schema::ResolutionDisposition::Terminal {
                outcome: *outcome,
                assurance: *assurance
            }
        );
        // Feed actual native records through the observer's evidence boundary,
        // while retaining the independent receipt/hash checks below.
        fn observer_record(
            ((epoch, sequence), event): &((eez_telemetry::schema::Epoch, u64), Event),
        ) -> eez_observer::outcomes::Record<'_> {
            eez_observer::outcomes::Record {
                source_id: 1,
                epoch: *epoch,
                sequence: *sequence,
                event,
            }
        }
        let terminal_record=events.iter().find(|(_, event)| matches!(event,Event::Terminal {member: candidate,..} if candidate==member)).expect("terminal transport identity");
        let mut lineage = events
            .iter()
            .filter(|((epoch, sequence), event)| {
                *epoch == attempt.epoch
                    && *sequence < parent.0.1
                    && match event {
                        Event::AttemptPrepared {
                            attempt: candidate, ..
                        } => candidate.generation == attempt.generation,
                        Event::AttemptRepriced { replacement, .. } => {
                            replacement.generation == attempt.generation
                        }
                        _ => false,
                    }
            })
            .map(observer_record)
            .collect::<Vec<_>>();
        lineage.sort_by_key(|record| record.sequence);
        let scopes = [
            eez_telemetry::schema::ChainScope {
                chain_id: l1.get_chain_id().await.unwrap(),
                genesis_hash: eez_telemetry::schema::Hash32(expected_genesis[0].0),
                rollup_id: 0,
            },
            eez_telemetry::schema::ChainScope {
                chain_id: l2.get_chain_id().await.unwrap(),
                genesis_hash: eez_telemetry::schema::Hash32(expected_genesis[1].0),
                rollup_id: world.cfg.rollup_id,
            },
        ];
        let decision = eez_observer::outcomes::native_decision(
            &scopes,
            observer_record(terminal_record),
            observer_record(parent),
            &lineage,
        )
        .expect("native decision passes observer coherence checks");
        assert_eq!(decision.member, *member);
        assert_eq!(decision.outcome, *outcome);
        assert_eq!(decision.l1_snapshot, *scan);
        let paid = eez_observer::costs::paid_attempt(&scopes, observer_record(parent), &lineage)
            .expect("captured paid native attempt passes shared allocation membership checks");
        assert_eq!(paid.owner, *attempt);
        assert_eq!(paid.snapshot, *scan);
        assert!(paid.membership.members.contains(member));
        observer
            .paid_post(
                alloy_primitives::B256::from(paid.owner.post_batch_hash.0),
                paid.membership.members.len(),
            )
            .await
            .unwrap();
        let canonical_scan = eez_testkit::block_number_and_hash_at(
            &world.l1_rpc(),
            alloy_rpc_types_eth::BlockNumberOrTag::Number(scan.number),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(canonical_scan.1.0, scan.hash.0);
        let post = l1
            .get_transaction_receipt(TxHash::from(attempt.post_batch_hash.0))
            .await
            .unwrap()
            .expect("canonical post receipt");
        let captured_post = details.post.expect("native post receipt reference");
        assert_eq!(paid.post, captured_post);
        assert_eq!(post.block_hash.unwrap().0, captured_post.block.hash.0);
        assert_eq!(post.block_number.unwrap(), captured_post.block.number);
        assert_eq!(
            post.transaction_index.unwrap(),
            captured_post.transaction_index
        );
        assert_eq!(post.status(), captured_post.success);
        let applied = details.applied.as_ref().expect("applied settlement");
        assert_eq!(applied.transaction_hash, attempt.post_batch_hash);
        assert_eq!(applied.block, captured_post.block);
        assert_eq!(applied.transaction_index, captured_post.transaction_index);
        let applied_endpoint = applied.endpoint.expect("applied L2 endpoint");
        let canonical_endpoint = eez_testkit::block_number_and_hash_at(
            &world.l2_rpc(),
            alloy_rpc_types_eth::BlockNumberOrTag::Number(applied_endpoint.number),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(canonical_endpoint.1.0, applied_endpoint.hash.0);
        let observation = details
            .sources
            .iter()
            .find(|source| source.transaction_hash.0 == hash.0)
            .expect("exact source observation");
        assert_eq!(observation.direction, source.direction);
        let source_rpc = if index == 0 {
            world.l1_rpc()
        } else {
            world.l2_rpc()
        };
        let source_provider = ProviderBuilder::new().connect_http(source_rpc.parse().unwrap());
        let source_receipt = source_provider
            .get_transaction_receipt(hash)
            .await
            .unwrap()
            .expect("canonical source receipt");
        let source_block = observation
            .source_block
            .expect("source inclusion or applied group anchor");
        assert_eq!(source_receipt.block_hash.unwrap().0, source_block.hash.0);
        assert_eq!(source_receipt.block_number.unwrap(), source_block.number);
        if index == 0 {
            let captured_source = observation.receipt.expect("inbound source receipt");
            assert_eq!(captured_source.block, source_block);
            assert_eq!(
                captured_source.transaction_index,
                source_receipt.transaction_index.unwrap()
            );
            assert_eq!(captured_source.success, source_receipt.status());
            assert!(captured_source.transaction_index > captured_post.transaction_index);
            assert!(!observation.consumed.is_empty());
        }
        let mut included_dispatches = 0;
        for (_, event) in &events {
            let Event::AttemptPrepared {
                attempt,
                members,
                blob_count,
                ..
            } = event
            else {
                continue;
            };
            if !members
                .iter()
                .any(|member| member.source.transaction_hash.0 == hash.0)
            {
                continue;
            }
            for (_, event) in &events {
                let Event::Dispatched {
                    attempt: sent,
                    transaction_hash,
                    nonce,
                    ..
                } = event
                else {
                    continue;
                };
                let linked = sent == attempt
                    || events.iter().any(|(_, event)| {
                        matches!(event, Event::AttemptRepriced { previous, replacement }
                        if previous == attempt && replacement == sent)
                    });
                if !linked {
                    continue;
                }
                if let Some(transaction) = l1
                    .get_transaction_by_hash(TxHash::from(transaction_hash.0))
                    .await
                    .unwrap()
                    && transaction.block_hash.is_some()
                {
                    assert_eq!(transaction.inner.nonce(), *nonce);
                    assert_eq!(
                        transaction.inner.blob_versioned_hashes().unwrap().len(),
                        *blob_count as usize
                    );
                    included_dispatches += 1;
                }
            }
        }
        assert!(
            included_dispatches > 0,
            "source membership must reach a mined dispatch with the captured nonce/blob count"
        );
    }
    let metrics = wait_for(std::time::Duration::from_secs(45), || {
        let metrics_address = metrics_address.clone();
        async move {
            let metrics = scrape_native_metrics(&metrics_address).await?;
            let terminal_and_empty = ["inbound", "outbound"].into_iter().all(|direction| {
                let admissions = format!("reth_eez_native_admissions_total{{direction=\"{direction}\"}}");
                let terminal = format!("reth_eez_native_terminal_observations_total{{direction=\"{direction}\",outcome=\"committed\"}}");
                let completed = format!("reth_eez_request_duration_seconds_count{{direction=\"{direction}\",outcome=\"committed\"}}");
                let pending = format!("reth_eez_native_queue_age_count{{direction=\"{direction}\",kind=\"pending\"}}");
                metric_sample(&metrics, &admissions) == Some(1.)
                    && metric_sample(&metrics, &terminal) == Some(1.)
                    && metric_sample(&metrics, &completed) == Some(1.)
                    && metric_sample(&metrics, &pending) == Some(0.)
                    && ["queued_ready", "queued_blocked", "queued_unknown", "in_flight"].into_iter().all(|state| {
                        metric_sample(&metrics, &format!("reth_eez_native_queue_transactions{{direction=\"{direction}\",state=\"{state}\"}}")) == Some(0.)
                    })
            });
            Ok((terminal_and_empty && metric_sample(&metrics, "reth_eez_native_queue_sample_valid") == Some(1.)).then_some(metrics))
        }
    }).await.expect("real native metrics must retain both completed durations while current pending populations empty");
    assert!(metrics.contains("# TYPE reth_eez_stage_duration_seconds histogram"));
    assert!(metrics.contains("# TYPE reth_eez_request_duration_seconds histogram"));
    assert!(metrics.contains("# TYPE reth_eez_request_readiness_duration_seconds histogram"));
    for direction in ["inbound", "outbound"] {
        let total = metric_sample(&metrics, &format!("reth_eez_request_duration_seconds_sum{{direction=\"{direction}\",outcome=\"committed\"}}"))
            .expect("completed admission clock");
        let mut partition_sum = 0.;
        for kind in ["blocked", "serviceable", "unknown"] {
            assert_eq!(
                metric_sample(
                    &metrics,
                    &format!(
                        "reth_eez_request_readiness_duration_seconds_count{{direction=\"{direction}\",outcome=\"committed\",kind=\"{kind}\"}}"
                    )
                ),
                Some(1.),
                "one retained partition per native terminal observation"
            );
            let part = metric_sample(&metrics, &format!("reth_eez_request_readiness_duration_seconds_sum{{direction=\"{direction}\",outcome=\"committed\",kind=\"{kind}\"}}"))
                .expect("observed readiness portion");
            assert!(part.is_finite() && part >= 0.);
            partition_sum += part;
        }
        assert!(
            (partition_sum - total).abs() < 0.000_001,
            "readiness parts conserve the same native admission interval"
        );
    }
    assert!(
        metric_sample(
            &metrics,
            "reth_eez_stage_duration_seconds_count{stage=\"preparation\",outcome=\"prepared\"}"
        )
        .is_some_and(|count| count >= 1.)
    );
    for (stage, outcome) in [
        ("simulation", "success"),
        ("witness", "success"),
        ("prover_client", "success"),
        ("submission", "included"),
    ] {
        assert!(metric_sample(&metrics, &format!("reth_eez_stage_duration_seconds_count{{stage=\"{stage}\",outcome=\"{outcome}\"}}"))
            .is_some_and(|count| count >= 1.), "missing completed native stage {stage}: {metrics}");
    }
    let policy_enabled = extra_env
        .iter()
        .any(|(key, value)| *key == "EEZ_SETTLEMENT_POLICY_ENABLED" && value == "true");
    assert_eq!(
        metric_sample(&metrics, "reth_eez_settlement_policy_enabled"),
        Some(f64::from(u8::from(policy_enabled)))
    );
    assert!(
        metric_sample(
            &metrics,
            "reth_eez_settlement_policy_setting{setting=\"user_count_cap\"}"
        )
        .is_some_and(|cap| cap > 0.)
    );
    let inventory = metric_family_inventory(&metrics);
    assert!(
        inventory
            .keys()
            .any(|name| name.starts_with("reth_transaction_pool_")),
        "native fixture must exercise upstream client metrics"
    );
    if let Some(root) = std::env::var_os("EEZ_TEST_LOG_DIR") {
        let root = std::path::PathBuf::from(root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "eez-client-metrics-{}-{}.json",
            std::process::id(),
            if policy_enabled { "policy" } else { "legacy" }
        ));
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "scope": "actual combined embedded L1/L2 process; no per-chain inference",
                "format": "metric sample names and label keys only; no values",
                "families": inventory,
            }))
            .unwrap(),
        )
        .unwrap();
    }
    for hash in hashes {
        assert!(
            !metrics.contains(&format!("{hash:x}")),
            "transaction hash leaked into metrics"
        );
    }
    for (index, hash) in hashes.iter().enumerate() {
        let source_rpc = if index == 0 {
            world.l1_rpc()
        } else {
            world.l2_rpc()
        };
        observer.outcome(*hash, "committed").await.unwrap();
        observer
            .receipt(&source_rpc, if index == 0 { "l1" } else { "l2" }, *hash)
            .await
            .unwrap();
        observer.allocation(*hash).await.unwrap();
    }
    for (_, event) in &events {
        if let Event::Reconciled {
            attempt,
            details: Some(details),
            ..
        } = event
            && details.post.is_some()
        {
            observer
                .receipt(
                    &world.l1_rpc(),
                    "l1",
                    alloy_primitives::B256::from(attempt.post_batch_hash.0),
                )
                .await
                .unwrap();
        }
    }
    let before = [
        observer.admissions(hashes[0]).await.unwrap(),
        observer.admissions(hashes[1]).await.unwrap(),
    ];
    observer
        .canonical_commits(&["inbound", "outbound"], "before-restart")
        .await
        .unwrap();
    observer.restart().await.unwrap();
    for (index, hash) in hashes.iter().enumerate() {
        let source_rpc = if index == 0 {
            world.l1_rpc()
        } else {
            world.l2_rpc()
        };
        observer.outcome(*hash, "committed").await.unwrap();
        assert_eq!(
            observer.admissions(*hash).await.unwrap(),
            before[index],
            "observer restart duplicated admission history"
        );
        observer
            .receipt(&source_rpc, if index == 0 { "l1" } else { "l2" }, *hash)
            .await
            .unwrap();
        observer.allocation(*hash).await.unwrap();
    }
    observer
        .canonical_commits(&["inbound", "outbound"], "after-restart")
        .await
        .unwrap();
    world.node.assert_no_divergence_failure_logs();
}

/// Inventory only syntax-owned names/keys. Never copy label values, URLs,
/// account identifiers, hashes, chain metadata or command-line flags.
fn metric_family_inventory(
    metrics: &str,
) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
    let mut result = std::collections::BTreeMap::new();
    for line in metrics
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let end = line
            .find(['{', ' '])
            .expect("Prometheus sample name boundary");
        let name = &line[..end];
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':'))
        );
        let keys = result
            .entry(name.to_owned())
            .or_insert_with(std::collections::BTreeSet::new);
        if line.as_bytes()[end] != b'{' {
            continue;
        }
        let mut rest = &line[end + 1..];
        while !rest.starts_with('}') {
            let equals = rest.find('=').expect("Prometheus label key");
            let key = rest[..equals].trim();
            assert!(
                !key.is_empty()
                    && key
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            );
            keys.insert(key.to_owned());
            rest = &rest[equals + 1..];
            assert!(rest.starts_with('"'));
            let mut escaped = false;
            let mut consumed = None;
            for (index, character) in rest[1..].char_indices() {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    consumed = Some(index + 2);
                    break;
                }
            }
            rest = rest[consumed.expect("terminated Prometheus label value")..].trim_start();
            if let Some(next) = rest.strip_prefix(',') {
                rest = next.trim_start();
            } else {
                assert!(rest.starts_with('}'));
            }
        }
    }
    result
}

#[test]
fn metric_inventory_excludes_values_even_when_they_contain_prometheus_syntax() {
    let value = metric_family_inventory(
        r#"# TYPE demo gauge
demo{role="composer",opaque="secret,pretend_key=\"account\",}tail"} 7
plain_counter 1
"#,
    );
    assert_eq!(
        value["demo"],
        ["opaque".to_owned(), "role".to_owned()]
            .into_iter()
            .collect()
    );
    assert!(value["plain_counter"].is_empty());
    let encoded = serde_json::to_string(&value).unwrap();
    for private in ["secret", "pretend_key", "account", "composer"] {
        assert!(!encoded.contains(private));
    }
}

fn metric_sample(metrics: &str, identity: &str) -> Option<f64> {
    metrics.lines().find_map(|line| {
        let (name, value) = line.split_once(' ')?;
        (name == identity).then(|| value.parse().ok()).flatten()
    })
}

async fn scrape_native_metrics(address: &str) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let mut response = client
        .get(format!("http://{address}/metrics"))
        .send()
        .await?
        .error_for_status()?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(
            body.len().saturating_add(chunk.len()) <= 8 * 1024 * 1024,
            "native scrape exceeds fixture budget"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8(body)?)
}

async fn assert_real_signer_rejects(mutation: ProverMutation, tampered_input: &str) {
    let attester = signer_address(ANVIL_ATTESTER_KEY).unwrap();
    let w = setup_cross_chain_proxied(mutation, attester).await.unwrap();
    let l1_rpc = w.l1_rpc();
    let proxy = w.prover_proxy.as_ref().expect("real signer proxy");

    let rejected = wait_for(SETTLE_TIMEOUT, || async {
        Ok((proxy.rejections() >= 1).then_some(()))
    })
    .await;
    if rejected.is_err() {
        w.proof_signer.assert_alive();
        assert_ne!(
            proxy.attempts(),
            0,
            "composer never proved a window, so tampered {tampered_input} was never exercised",
        );
        panic!(
            "signer accepted tampered {tampered_input}: {} attempts, {} attested",
            proxy.attempts(),
            proxy.successes(),
        );
    }
    let batches_after_rejection = batches_posted(&l1_rpc, w.cfg.eez_address, w.dep.deploy_block)
        .await
        .unwrap();
    let root_after_rejection = state_root(&l1_rpc, w.cfg.eez_address, w.cfg.rollup_id)
        .await
        .unwrap();
    wait_for(SETTLE_TIMEOUT, || async {
        Ok((proxy.rejections() >= 2).then_some(()))
    })
    .await
    .expect("composer did not retry the rejected proof window");
    assert_eq!(
        batches_posted(&l1_rpc, w.cfg.eez_address, w.dep.deploy_block)
            .await
            .unwrap(),
        batches_after_rejection,
        "batch count advanced while the signer repeatedly rejected tampered {tampered_input}",
    );
    assert_eq!(
        state_root(&l1_rpc, w.cfg.eez_address, w.cfg.rollup_id)
            .await
            .unwrap(),
        root_after_rejection,
        "state root advanced while the signer repeatedly rejected tampered {tampered_input}",
    );
    w.proof_signer.assert_alive();
    w.node.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_signer_rejects_tampered_post_batch_calldata() {
    assert_real_signer_rejects(ProverMutation::PostBatch, "PostBatch calldata").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_signer_rejects_tampered_witness() {
    assert_real_signer_rejects(ProverMutation::Witness, "witness").await;
}

/// Whether the response is intrinsically opaque (`ResourceExhausted` when
/// resource capacity is exceeded) or becomes opaque because its actionable identity is unsafe, a valid
/// transaction may be sacrificed but must leave the retry loop at the bound.
async fn assert_opaque_prover_rejection_eventually_evicts_the_candidate(mutation: ProverMutation) {
    let attester = signer_address(ANVIL_ATTESTER_KEY).unwrap();
    let w = setup_cross_chain_proxied(mutation, attester).await.unwrap();
    let l1_rpc = w.l1_rpc();
    let l2_rpc = w.l2_rpc();
    let starting_nonce = onchain_nonce(&l1_rpc, INBOUND_USER).await.unwrap();
    let starting_value = l2_value(&l2_rpc, w.value_l2).await.unwrap();
    let tx_hash = sign_and_send(
        &w.l1_xchain(),
        INBOUND_USER,
        DEV_CHAIN_ID,
        starting_nonce,
        Some(w.setter_proxy),
        U256::ZERO,
        IValue::setValueCall { v: U256::from(76) }.abi_encode(),
        600_000,
    )
    .await
    .expect("valid transaction must be admitted before the prover rejects its batch");

    let evicted = wait_for(SETTLE_TIMEOUT, || async {
        let logs = w
            .node
            .log_lines_matching(&["potentially valid user_tx evicted"], 20);
        Ok(logs
            .lines()
            .any(|line| line.contains("ERROR") && line.contains(&tx_hash.to_string()))
            .then_some(()))
    })
    .await;
    if let Err(error) = evicted {
        panic!(
            "opaque prover rejection did not reach bounded eviction: {error:#}{}",
            w.settlement_diagnostics(),
        );
    }

    let proxy = w.prover_proxy.as_ref().expect("rejection proxy");
    assert!(
        proxy.rejections() >= 3,
        "eviction must require three distinct rejected proof episodes"
    );
    assert_eq!(
        receipt_ok(&l1_rpc, tx_hash).await.unwrap(),
        None,
        "a sacrificed valid transaction must not appear to have landed"
    );
    assert_eq!(
        onchain_nonce(&l1_rpc, INBOUND_USER).await.unwrap(),
        starting_nonce,
        "the rejected transaction must not burn its source nonce"
    );
    assert_eq!(
        l2_value(&l2_rpc, w.value_l2).await.unwrap(),
        starting_value,
        "the rejected cross-chain call must not change its target value"
    );
    assert!(
        proxy.successes() > 0,
        "anchor-only fallback proofs must remain healthy so recovery can make progress"
    );
    if matches!(mutation, ProverMutation::MismatchedActionable) {
        let logs = w.node.log_lines_matching(
            &["actionable failure details that do not match the current request"],
            20,
        );
        assert!(
            logs.lines().any(|line| line.contains("ERROR")),
            "mismatched actionable details must produce a severe diagnostic log"
        );
    }
    w.node.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opaque_prover_rejection_eventually_evicts_the_candidate() {
    assert_opaque_prover_rejection_eventually_evicts_the_candidate(
        ProverMutation::ResourceExhausted,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mismatched_actionable_rejection_eventually_evicts_the_candidate() {
    assert_opaque_prover_rejection_eventually_evicts_the_candidate(
        ProverMutation::MismatchedActionable,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_signer_attester_mismatch_never_submits_a_batch() {
    let wrong_attester = Address::repeat_byte(0x44);
    let w = setup_cross_chain_proxied(ProverMutation::None, wrong_attester)
        .await
        .unwrap();
    let proxy = w.prover_proxy.as_ref().expect("real signer proxy");

    let attested = wait_for(SETTLE_TIMEOUT, || async {
        Ok((proxy.successes() >= 2).then_some(()))
    })
    .await;
    if let Err(err) = attested {
        w.proof_signer.assert_alive();
        panic!("signer never returned the mismatched attestations: {err:#}");
    }
    assert_eq!(
        proxy.rejections(),
        0,
        "the signer must have attested; only the composer may refuse here",
    );
    assert_eq!(
        batches_posted(&w.l1_rpc(), w.cfg.eez_address, w.dep.deploy_block)
            .await
            .unwrap(),
        0,
        "an attestation from an unexpected signer must never reach L1",
    );
    assert_eq!(
        state_root(&w.l1_rpc(), w.cfg.eez_address, w.cfg.rollup_id)
            .await
            .unwrap(),
        w.cfg.initial_state,
        "signer mismatch must leave the registered state root unchanged",
    );
    w.node.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_directions_zero_value_direct_proxy_success_single_call() {
    let w = setup_cross_chain().await.unwrap();
    run_scenarios(
        &w,
        [Scenario::new("Bidirectional zero-value direct proxy")
            .inbound(setter_call(w.setter_proxy, 41u64))
            .outbound(setter_call(w.outbound_proxy, 43u64))
            .expect_l2_state(value_read(w.value_l2), 41u64)
            .expect_l1_state(value_read(w.outbound_value), 43u64)
            .expect_settled_fully()],
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_empty_calldata_and_zero_value_is_not_skipped() {
    // Verify that an all-zero call is materialized without being rewritten.
    let w = setup_cross_chain_empty_call().await.unwrap();
    Scenario::new("empty-calldata zero-value inbound")
        .inbound(ScenarioCall::new(w.empty_call_proxy, Vec::new()).with_gas_limit(600_000))
        .expect_l2_state(calls_read(w.empty_call_l2), 1u64)
        .expect_l2_state(received_read(w.empty_call_l2), 0u64)
        .expect_l2_state(last_value_read(w.empty_call_l2), 0u64)
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_deposit_to_payable_contract_runs_fallback() {
    // `EmptyCall` has no `receive`, so empty calldata exercises its fallback.
    let w = setup_cross_chain_empty_call().await.unwrap();
    let value = U256::from(123_456u64);
    Scenario::new("payable inbound deposit")
        .inbound(
            ScenarioCall::new(w.empty_call_proxy, Vec::new())
                .with_value(value)
                .with_gas_limit(600_000),
        )
        .expect_l2_state(calls_read(w.empty_call_l2), 1u64)
        .expect_l2_state(received_read(w.empty_call_l2), value)
        .expect_l2_state(last_value_read(w.empty_call_l2), 0u64)
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_value_and_calldata_apply_atomically() {
    // Bind the transferred value and decoded argument to one destination call.
    let w = setup_cross_chain_empty_call().await.unwrap();
    let value = U256::from(456_789u64);
    let next = U256::from(91u64);
    Scenario::new("value-and-calldata inbound")
        .inbound(
            ScenarioCall::new(
                w.empty_call_proxy,
                IEmptyCall::setValueCall { next }.abi_encode(),
            )
            .with_value(value)
            .with_gas_limit(600_000),
        )
        .expect_l2_state(calls_read(w.empty_call_l2), 1u64)
        .expect_l2_state(received_read(w.empty_call_l2), value)
        .expect_l2_state(last_value_read(w.empty_call_l2), next)
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_ccm_l2_outbound_call_is_rejected() {
    // Direct CCM-L2 access is mined, then rejected during execution.
    let w = setup_cross_chain().await.unwrap();
    let l2_rpc = w.l2_rpc();
    let caller: Address = signer_address(OUTBOUND_USER).unwrap();
    let tx = sign_and_send(
        &l2_rpc,
        OUTBOUND_USER,
        w.l2_chain_id,
        onchain_nonce(&l2_rpc, OUTBOUND_USER).await.unwrap(),
        Some(EEZL2_ADDRESS),
        U256::ZERO,
        IEEZL2Direct::executeCrossChainCallCall {
            sourceAddress: caller,
            callData: Bytes::new(),
        }
        .abi_encode(),
        600_000,
    )
    .await
    .expect("direct CCM-L2 transaction must be accepted for execution");

    // Admission is expected; only execution must fail for an unauthorized caller.
    assert_transaction_reverted(&l2_rpc, tx, "direct CCM-L2 outbound").await;

    // Pin the failure to `EEZL2`'s authorization check.
    let revert_data = call_revert_data(
        &l2_rpc,
        caller,
        EEZL2_ADDRESS,
        IEEZL2Direct::executeCrossChainCallCall {
            sourceAddress: caller,
            callData: Bytes::new(),
        }
        .abi_encode(),
    )
    .await
    .expect("direct CCM-L2 call must revert with data");
    assert_eq!(
        revert_data.as_ref(),
        IEEZL2Direct::UnauthorizedProxy::SELECTOR.as_slice(),
        "direct CCM-L2 access must be rejected as an unauthorized proxy",
    );
    w.node.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_directions_return_value_and_wrapper_success_repeated_waves() {
    // Sustained matrix: direct, no-return, wrapper, and value-transfer calls
    // in both directions.
    let w = setup_cross_chain().await.unwrap();
    let l1_rpc = w.l1_rpc();
    let l1_xchain = w.l1_xchain();
    let l2_xchain = w.l2_xchain();
    let l2_rpc = w.l2_rpc();

    let recipient_before = l2_balance(&l2_rpc, w.recipient).await.unwrap();
    let withdrawal_before = l2_balance(&l1_rpc, w.withdrawal_recipient).await.unwrap();
    let deposit_sum: u128 = WAVE_DEPOSITS.iter().sum();
    // Deltas, not totals: batches and bundles counted from deployment include
    // the background minimal batches the composer posts on an idle chain, so an
    // absolute threshold can be met without this wave contributing anything.
    let batches_before = batches_posted(&l1_rpc, w.cfg.eez_address, w.dep.deploy_block)
        .await
        .unwrap();
    let bundles_before = w.node.count_signal(signals::BUNDLE_ACCEPTED).unwrap();

    let mut inbound_hashes = Vec::new();
    let mut outbound_hashes = Vec::new();

    for (set_v, dep_v) in WAVE_SETTERS.iter().zip(WAVE_DEPOSITS.iter()) {
        // Each wave settles before the next source nonces are derived. A compose
        // tick can leave the prior wave in flight, so deriving from chain state
        // before its receipts land would reuse those reserved nonces.
        let mut l1_nonce = onchain_nonce(&l1_rpc, INBOUND_USER).await.unwrap();
        let mut l2_nonce = onchain_nonce(&l2_rpc, OUTBOUND_USER).await.unwrap();
        let inbound_wave_start = inbound_hashes.len();
        let outbound_wave_start = outbound_hashes.len();

        let set_input = IValue::setValueCall {
            v: U256::from(*set_v),
        }
        .abi_encode();
        let no_ret_input = IValueNoRet::setValueCall {
            v: U256::from(*set_v + 200),
        }
        .abi_encode();
        let wrapper_input = ISetterWrapper::setViaProxyCall {
            v: U256::from(*set_v + 100),
        }
        .abi_encode();
        inbound_hashes.push(
            sign_and_send(
                &l1_xchain,
                INBOUND_USER,
                DEV_CHAIN_ID,
                l1_nonce,
                Some(w.setter_proxy),
                U256::ZERO,
                set_input.clone(),
                600_000,
            )
            .await
            .unwrap(),
        );
        l1_nonce += 1;
        inbound_hashes.push(
            sign_and_send(
                &l1_xchain,
                INBOUND_USER,
                DEV_CHAIN_ID,
                l1_nonce,
                Some(w.deposit_proxy),
                U256::from(*dep_v),
                Vec::new(),
                600_000,
            )
            .await
            .unwrap(),
        );
        l1_nonce += 1;
        for (to, input, value) in [
            (w.inbound_no_ret_proxy, no_ret_input.clone(), U256::ZERO),
            (w.inbound_wrapper, wrapper_input.clone(), U256::ZERO),
        ] {
            inbound_hashes.push(
                sign_and_send(
                    &l1_xchain,
                    INBOUND_USER,
                    DEV_CHAIN_ID,
                    l1_nonce,
                    Some(to),
                    value,
                    input,
                    900_000,
                )
                .await
                .unwrap(),
            );
            l1_nonce += 1;
        }

        for (to, input, value) in [
            (w.outbound_proxy, set_input, U256::ZERO),
            (w.outbound_no_ret_proxy, no_ret_input, U256::ZERO),
            (w.withdrawal_proxy, Vec::new(), U256::from(*dep_v)),
            (w.outbound_wrapper, wrapper_input, U256::ZERO),
        ] {
            outbound_hashes.push(
                sign_and_send(
                    &l2_xchain,
                    OUTBOUND_USER,
                    w.l2_chain_id,
                    l2_nonce,
                    Some(to),
                    value,
                    input,
                    900_000,
                )
                .await
                .unwrap(),
            );
            l2_nonce += 1;
        }

        tokio::join!(
            assert_all_transactions_succeeded(
                &w,
                &l1_rpc,
                &inbound_hashes[inbound_wave_start..],
                "inbound wave",
            ),
            assert_all_transactions_succeeded(
                &w,
                &l2_rpc,
                &outbound_hashes[outbound_wave_start..],
                "outbound wave",
            ),
        );
    }

    let expected_per_direction = WAVE_SETTERS.len() * 4;
    assert_eq!(
        inbound_hashes.len(),
        expected_per_direction,
        "every inbound wave operation must be submitted",
    );
    assert_eq!(
        outbound_hashes.len(),
        expected_per_direction,
        "every outbound wave operation must be submitted",
    );

    // Source receipts prove admission; destination effects arrive after settlement.
    let setter_final = U256::from(*WAVE_SETTERS.last().unwrap() + 100);
    let no_ret_final = U256::from(*WAVE_SETTERS.last().unwrap() + 200);
    let recipient_final = recipient_before + U256::from(deposit_sum);
    let withdrawal_final = withdrawal_before + U256::from(deposit_sum);
    let effects_converged = wait_for(SETTLE_TIMEOUT, || {
        let (l1_rpc, l2_rpc) = (l1_rpc.clone(), l2_rpc.clone());
        async move {
            let converged = l2_value(&l2_rpc, w.value_l2).await? == setter_final
                && l2_value(&l1_rpc, w.outbound_value).await? == setter_final
                && value_no_ret(&l2_rpc, w.inbound_no_ret).await? == no_ret_final
                && value_no_ret(&l1_rpc, w.outbound_no_ret).await? == no_ret_final
                && l2_balance(&l2_rpc, w.recipient).await? == recipient_final
                && l2_balance(&l1_rpc, w.withdrawal_recipient).await? == withdrawal_final;
            Ok(converged.then_some(()))
        }
    })
    .await;
    if let Err(err) = effects_converged {
        panic!(
            "cross-chain wave effects did not converge: {err:#}{}",
            w.settlement_diagnostics(),
        );
    }

    assert_eq!(
        last_proxy_result(&l1_rpc, w.inbound_wrapper).await.unwrap(),
        (true, setter_final),
        "inbound wrapper must receive the exact destination return",
    );
    assert_eq!(
        last_proxy_result(&l2_rpc, w.outbound_wrapper)
            .await
            .unwrap(),
        (true, setter_final),
        "outbound wrapper must receive the exact destination return",
    );

    let (eez, rollup_id) = (w.cfg.eez_address, w.cfg.rollup_id);
    wait_for(SETTLE_TIMEOUT, || {
        let (l1_rpc, l2_rpc) = (l1_rpc.clone(), l2_rpc.clone());
        async move {
            let l1_root = state_root(&l1_rpc, eez, rollup_id).await?;
            let l2_root = safe_block_hash(&l2_rpc).await?;
            Ok(l2_root.filter(|r| *r == l1_root).map(|_| ()))
        }
    })
    .await
    .expect("L1 stored commitment never matched the L2 safe block hash");

    let batches = batches_posted(&l1_rpc, w.cfg.eez_address, w.dep.deploy_block)
        .await
        .unwrap();
    assert!(
        batches >= batches_before + WAVE_SETTERS.len(),
        "expected ≥{} new BatchPosted events, got {}",
        WAVE_SETTERS.len(),
        batches - batches_before,
    );
    assert_eq!(
        w.node
            .count_signals(&[
                signals::DERIVER_STATE_DIVERGED_PRE,
                signals::DERIVER_STATE_DIVERGED_POST,
            ])
            .unwrap(),
        0,
        "zero state-root divergence events",
    );
    assert_eq!(
        w.node
            .count_signals(&[
                signals::COMPOSER_POISON_EVICTION_COMPLETED,
                signals::TX_POISON_EVICTED,
                signals::TX_NONCE_CHAIN_EVICTED,
            ])
            .unwrap(),
        0,
        "all non-poison wave transactions must settle without eviction",
    );

    assert!(
        w.node.count_signal(signals::BUNDLE_ACCEPTED).unwrap() > bundles_before,
        "embedded dev L1 eth_sendBundle was exercised by this wave",
    );
    w.node.assert_no_divergence_failure_logs();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_destination_events_are_retained_in_the_l2_block() {
    let w = setup_cross_chain().await.unwrap();
    let l2_rpc = w.l2_rpc();
    // Capture the event count before submitting the scenario.
    let before = count_events(&l2_rpc, w.value_l2, IValue::ValueSet::SIGNATURE_HASH, 0)
        .await
        .unwrap();
    // The destination must see the L1 sender's source proxy.
    let expected_sender = attributed_sender(
        &l2_rpc,
        EEZL2_ADDRESS,
        signer_address(INBOUND_USER).unwrap(),
        L1_ROLLUP_ID,
    )
    .await;

    Scenario::new("event-emitting inbound")
        .inbound(setter_call(w.setter_proxy, 61u64).with_gas_limit(600_000))
        .expect_l2_state(value_read(w.value_l2), 61u64)
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();

    assert_value_set_attribution(
        &l2_rpc,
        w.value_l2,
        before,
        1,
        expected_sender,
        "event-emitting inbound",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn codeless_registered_targets_complete_deterministically_in_both_directions() {
    let w = setup_cross_chain_codeless().await.unwrap();
    let l1_rpc = w.l1_rpc();
    let l2_rpc = w.l2_rpc();
    let data = IReturnDataWrapper::callAndRecordCall { data: Bytes::new() }.abi_encode();

    assert!(
        account_code(&l2_rpc, w.recipient).await.unwrap().is_empty(),
        "inbound target must be codeless",
    );
    assert!(
        account_code(&l1_rpc, w.withdrawal_recipient)
            .await
            .unwrap()
            .is_empty(),
        "outbound target must be codeless",
    );

    // The nonzero empty-buffer hash proves the wrapper ran.
    let empty_hash = keccak256(Bytes::new());
    Scenario::new("codeless registered targets")
        .inbound(ScenarioCall::new(w.inbound_wrapper, data.clone()))
        .outbound(ScenarioCall::new(w.outbound_wrapper, data))
        .expect_l1_state(return_length_read(w.inbound_wrapper), 0u64)
        .expect_l1_state(return_hash_read(w.inbound_wrapper), empty_hash)
        .expect_l2_state(return_length_read(w.outbound_wrapper), 0u64)
        .expect_l2_state(return_hash_read(w.outbound_wrapper), empty_hash)
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_dynamic_and_empty_bytes_returns_are_preserved_distinctly() {
    // Length and hash distinguish dynamic bytes from ABI-encoded empty bytes.
    let w = setup_cross_chain_outbound_return_data().await.unwrap();
    let payload = Bytes::from(vec![0x5a; 96]);
    let wrapper = w.return_data_wrapper;
    run_scenarios(
        &w,
        [
            Scenario::new("outbound dynamic return")
                .outbound(
                    ScenarioCall::new(
                        wrapper,
                        IReturnDataWrapper::callAndRecordCall {
                            data: IReturnData::echoCall {
                                value: payload.clone(),
                            }
                            .abi_encode()
                            .into(),
                        }
                        .abi_encode(),
                    )
                    .with_gas_limit(1_200_000),
                )
                .expect_l2_state(return_length_read(wrapper), 160u64)
                .expect_l2_state(return_hash_read(wrapper), keccak256(payload.abi_encode()))
                .expect_settled_fully(),
            Scenario::new("outbound empty-bytes return")
                .outbound(
                    ScenarioCall::new(
                        wrapper,
                        IReturnDataWrapper::callAndRecordCall {
                            data: IReturnData::emptyBytesCall {}.abi_encode().into(),
                        }
                        .abi_encode(),
                    )
                    .with_gas_limit(1_200_000),
                )
                .expect_l2_state(return_length_read(wrapper), 64u64)
                .expect_l2_state(
                    return_hash_read(wrapper),
                    keccak256(Bytes::new().abi_encode()),
                )
                .expect_settled_fully(),
        ],
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_multiple_proxy_calls_in_one_transaction_preserve_ordered_results() {
    let w = setup_cross_chain().await.unwrap();
    let l1_rpc = w.l1_rpc();
    let l2_rpc = w.l2_rpc();
    let value = U256::from(79u64);
    let before = events_since(
        &l1_rpc,
        w.outbound_value,
        IValue::ValueSet::SIGNATURE_HASH,
        0,
    )
    .await
    .unwrap()
    .len();
    Scenario::new("two identical outbound calls in one source transaction")
        .outbound(
            ScenarioCall::new(
                w.outbound_wrapper,
                ISetterWrapper::setSameValueTwiceCall { v: value }.abi_encode(),
            )
            .with_gas_limit(1_200_000),
        )
        .expect_l1_state(value_read(w.outbound_value), value)
        .expect_l2_state(
            call_read(
                w.outbound_wrapper,
                "completedProxyCalls()",
                ISetterWrapper::completedProxyCallsCall {}.abi_encode(),
            ),
            2u64,
        )
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();

    let logs = events_since(
        &l2_rpc,
        w.outbound_wrapper,
        ISetterWrapper::Wrapped::SIGNATURE_HASH,
        0,
    )
    .await
    .unwrap();
    assert_eq!(
        logs.len(),
        2,
        "both source calls must complete exactly once"
    );
    assert_eq!(
        logs[0].transaction_hash, logs[1].transaction_hash,
        "both results must belong to the same source transaction"
    );
    let results = logs
        .iter()
        .map(|log| {
            let event = ISetterWrapper::Wrapped::decode_log(&log.inner).unwrap();
            (event.input, event.ok, event.changed, event.newValue)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        results,
        [(value, true, true, value), (value, true, false, value)],
        "the second identical call must observe the first destination write"
    );
    let sender = attributed_sender(
        &l1_rpc,
        w.cfg.eez_address,
        w.outbound_wrapper,
        w.cfg.rollup_id,
    )
    .await;
    assert_value_set_attribution(
        &l1_rpc,
        w.outbound_value,
        before,
        2,
        sender,
        "outbound multicall",
    )
    .await;
    w.node.assert_no_divergence_failure_logs();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_destination_events_are_retained_in_the_l1_block() {
    let w = setup_cross_chain().await.unwrap();
    let l1_rpc = w.l1_rpc();
    let before = count_events(
        &l1_rpc,
        w.outbound_value,
        IValue::ValueSet::SIGNATURE_HASH,
        0,
    )
    .await
    .unwrap();
    let expected_sender = attributed_sender(
        &l1_rpc,
        w.cfg.eez_address,
        signer_address(OUTBOUND_USER).unwrap(),
        w.cfg.rollup_id,
    )
    .await;

    Scenario::new("event-emitting outbound")
        .outbound(setter_call(w.outbound_proxy, 67u64))
        .expect_l1_state(value_read(w.outbound_value), 67u64)
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();

    assert_value_set_attribution(
        &l1_rpc,
        w.outbound_value,
        before,
        1,
        expected_sender,
        "event-emitting outbound",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_dynamic_and_empty_bytes_returns_are_preserved_distinctly() {
    // Mirror the outbound return-data assertions.
    let w = setup_cross_chain_return_data().await.unwrap();
    let payload = Bytes::from(vec![0xa5; 96]);
    let wrapper = w.return_data_wrapper;
    run_scenarios(
        &w,
        [
            Scenario::new("inbound dynamic return")
                .inbound(
                    ScenarioCall::new(
                        wrapper,
                        IReturnDataWrapper::callAndRecordCall {
                            data: IReturnData::echoCall {
                                value: payload.clone(),
                            }
                            .abi_encode()
                            .into(),
                        }
                        .abi_encode(),
                    )
                    .with_gas_limit(1_200_000),
                )
                .expect_l1_state(return_length_read(wrapper), 160u64)
                .expect_l1_state(return_hash_read(wrapper), keccak256(payload.abi_encode()))
                .expect_settled_fully(),
            Scenario::new("inbound empty-bytes return")
                .inbound(
                    ScenarioCall::new(
                        wrapper,
                        IReturnDataWrapper::callAndRecordCall {
                            data: IReturnData::emptyBytesCall {}.abi_encode().into(),
                        }
                        .abi_encode(),
                    )
                    .with_gas_limit(1_200_000),
                )
                .expect_l1_state(return_length_read(wrapper), 64u64)
                .expect_l1_state(
                    return_hash_read(wrapper),
                    keccak256(Bytes::new().abi_encode()),
                )
                .expect_settled_fully(),
        ],
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_nested_contract_to_contract_to_proxy_preserves_source_attribution() {
    // Only the inner frame invokes the proxy, so it must own the source identity.
    let w = setup_cross_chain_nested_setter().await.unwrap();
    let l2_rpc = w.l2_rpc();
    let value_l2 = w.world.value_l2;
    let value = U256::from(89u64);
    let before = count_events(&l2_rpc, value_l2, IValue::ValueSet::SIGNATURE_HASH, 0)
        .await
        .unwrap();
    let inner_sender =
        attributed_sender(&l2_rpc, EEZL2_ADDRESS, w.nested_setter_inner, L1_ROLLUP_ID).await;
    let outer_sender =
        attributed_sender(&l2_rpc, EEZL2_ADDRESS, w.nested_setter_outer, L1_ROLLUP_ID).await;
    let eoa_sender = attributed_sender(
        &l2_rpc,
        EEZL2_ADDRESS,
        signer_address(INBOUND_USER).unwrap(),
        L1_ROLLUP_ID,
    )
    .await;
    assert!(
        inner_sender != outer_sender && inner_sender != eoa_sender,
        "the attribution candidates must be distinguishable for this assertion to mean anything",
    );

    Scenario::new("nested inbound proxy call")
        .inbound(
            ScenarioCall::new(
                w.nested_setter_outer,
                INestedSetterOuter::setViaInnerCall { v: value }.abi_encode(),
            )
            .with_gas_limit(1_200_000),
        )
        .expect_l2_state(value_read(value_l2), value)
        // The return must reach the inner caller.
        .expect_l1_state(
            call_read(
                w.nested_setter_inner,
                "completedProxyCalls()",
                INestedSetterInner::completedProxyCallsCall {}.abi_encode(),
            ),
            1u64,
        )
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();

    assert_value_set_attribution(
        &l2_rpc,
        value_l2,
        before,
        1,
        inner_sender,
        "nested inbound proxy call",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_nested_contract_to_contract_to_proxy_preserves_source_attribution() {
    // The L1 destination must see the inner contract's source proxy.
    let w = setup_cross_chain().await.unwrap();
    let l1_rpc = w.l1_rpc();
    let l2_rpc = w.l2_rpc();
    let inner =
        deploy_nested_setter_inner(&l2_rpc, TARGET_DEPLOYER, w.l2_chain_id, w.outbound_proxy)
            .await
            .unwrap();
    let outer = deploy_nested_setter_outer(&l2_rpc, TARGET_DEPLOYER, w.l2_chain_id, inner)
        .await
        .unwrap();
    let value = U256::from(97u64);
    let before = count_events(
        &l1_rpc,
        w.outbound_value,
        IValue::ValueSet::SIGNATURE_HASH,
        0,
    )
    .await
    .unwrap();
    let inner_sender = attributed_sender(&l1_rpc, w.cfg.eez_address, inner, w.cfg.rollup_id).await;
    let outer_sender = attributed_sender(&l1_rpc, w.cfg.eez_address, outer, w.cfg.rollup_id).await;
    let eoa_sender = attributed_sender(
        &l1_rpc,
        w.cfg.eez_address,
        signer_address(OUTBOUND_USER).unwrap(),
        w.cfg.rollup_id,
    )
    .await;
    assert!(
        inner_sender != outer_sender && inner_sender != eoa_sender,
        "the attribution candidates must be distinguishable for this assertion to mean anything",
    );

    Scenario::new("nested outbound proxy call")
        .outbound(
            ScenarioCall::new(
                outer,
                INestedSetterOuter::setViaInnerCall { v: value }.abi_encode(),
            )
            .with_gas_limit(1_200_000),
        )
        .expect_l1_state(value_read(w.outbound_value), value)
        .expect_l2_state(
            call_read(
                inner,
                "completedProxyCalls()",
                INestedSetterInner::completedProxyCallsCall {}.abi_encode(),
            ),
            1u64,
        )
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();

    assert_value_set_attribution(
        &l1_rpc,
        w.outbound_value,
        before,
        1,
        inner_sender,
        "nested outbound proxy call",
    )
    .await;
}

fn failures_read(wrapper: Address) -> StateRead {
    call_read(
        wrapper,
        "failures()",
        IRevertBubbleWrapper::failuresCall {}.abi_encode(),
    )
}

fn successes_read(wrapper: Address) -> StateRead {
    call_read(
        wrapper,
        "successes()",
        IRevertBubbleWrapper::successesCall {}.abi_encode(),
    )
}

fn target_calls_read(target: Address) -> StateRead {
    call_read(
        target,
        "calls()",
        IRevertingTarget::callsCall {}.abi_encode(),
    )
}

fn target_last_value_read(target: Address) -> StateRead {
    call_read(
        target,
        "lastValue()",
        IRevertingTarget::lastValueCall {}.abi_encode(),
    )
}

fn record_call(wrapper: Address, inner: Vec<u8>) -> ScenarioCall {
    ScenarioCall::new(
        wrapper,
        IRevertBubbleWrapper::callAndRecordCall { data: inner.into() }.abi_encode(),
    )
    .with_gas_limit(1_200_000)
}

fn revert_cases(value: U256) -> [(&'static str, Vec<u8>); 3] {
    [
        (
            "custom error",
            IRevertingTarget::revertCustomCall { v: value }.abi_encode(),
        ),
        (
            "string reason",
            IRevertingTarget::revertStringCall { v: value }.abi_encode(),
        ),
        (
            // Writes two slots before reverting, so a surviving write is visible.
            "write then revert",
            IRevertingTarget::writeThenRevertCall { v: value }.abi_encode(),
        ),
    ]
}

async fn assert_destination_untouched(rpc: &str, target: Address, label: &str) {
    for read in [target_calls_read(target), target_last_value_read(target)] {
        let observed = read_state_word(rpc, &read).await.unwrap();
        assert_eq!(
            observed,
            U256::ZERO,
            "{label}: reverted destination must not persist state",
        );
    }
}

/// A caught destination revert is authenticated lookup data in this protocol.
/// The source records it; reverted destination writes never enter either chain.
async fn assert_reverting_destination_preserves_caught_result(inbound: bool) {
    let w = setup_cross_chain_reverting().await.unwrap();
    let (source_rpc, destination_rpc, manager, wrapper, target, source_rollup) = if inbound {
        (
            w.l1_rpc(),
            w.l2_rpc(),
            EEZL2_ADDRESS,
            w.inbound_wrapper,
            w.reverting_target_l2,
            L1_ROLLUP_ID,
        )
    } else {
        (
            w.l2_rpc(),
            w.l1_rpc(),
            w.cfg.eez_address,
            w.outbound_wrapper,
            w.reverting_target_l1,
            w.cfg.rollup_id,
        )
    };
    let proxy = attributed_sender(&destination_rpc, manager, wrapper, source_rollup).await;
    let value = U256::from(37u64);
    for (index, (label, inner)) in revert_cases(value).into_iter().enumerate() {
        let expected = call_revert_data(&destination_rpc, proxy, target, inner.clone())
            .await
            .unwrap();
        assert!(
            !expected.is_empty(),
            "{label} must have distinguishable revert data"
        );
        let scenario = Scenario::new(format!("caught destination {label}"));
        let scenario = if inbound {
            scenario.inbound(record_call(wrapper, inner))
        } else {
            scenario.outbound(record_call(wrapper, inner))
        };
        scenario.expect_settled_fully().run(&w).await.unwrap();
        assert_eq!(
            read_state_word(&source_rpc, &failures_read(wrapper))
                .await
                .unwrap(),
            U256::from(index + 1),
            "{label}: source must record the caught failure once"
        );
        assert_eq!(
            read_state_word(&source_rpc, &successes_read(wrapper))
                .await
                .unwrap(),
            U256::ZERO
        );
        let length = call_read(
            wrapper,
            "lastRevertLength()",
            IRevertBubbleWrapper::lastRevertLengthCall {}.abi_encode(),
        );
        let hash = call_read(
            wrapper,
            "lastRevertHash()",
            IRevertBubbleWrapper::lastRevertHashCall {}.abi_encode(),
        );
        assert_eq!(
            read_state_word(&source_rpc, &length).await.unwrap(),
            U256::from(expected.len())
        );
        assert_eq!(
            read_state_word(&source_rpc, &hash).await.unwrap(),
            U256::from_be_bytes(keccak256(&expected).0),
            "{label}: exact revert bytes must survive"
        );
        assert_destination_untouched(&destination_rpc, target, label).await;
    }
    let call = record_call(
        wrapper,
        IRevertingTarget::succeedCall { v: value }.abi_encode(),
    );
    let scenario = Scenario::new("valid call after caught destination reverts");
    let scenario = if inbound {
        scenario.inbound(call)
    } else {
        scenario.outbound(call)
    };
    scenario.expect_settled_fully().run(&w).await.unwrap();
    assert_eq!(
        read_state_word(&source_rpc, &successes_read(wrapper))
            .await
            .unwrap(),
        U256::from(1)
    );
    assert_eq!(
        read_state_word(&source_rpc, &failures_read(wrapper))
            .await
            .unwrap(),
        U256::from(3)
    );
    assert_eq!(
        read_state_word(&destination_rpc, &target_calls_read(target))
            .await
            .unwrap(),
        U256::from(1)
    );
    assert_eq!(
        read_state_word(&destination_rpc, &target_last_value_read(target))
            .await
            .unwrap(),
        value
    );
    w.node.assert_no_divergence_failure_logs();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_reverting_destination_preserves_caught_result_without_destination_writes() {
    assert_reverting_destination_preserves_caught_result(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_reverting_destination_preserves_caught_result_without_destination_writes() {
    assert_reverting_destination_preserves_caught_result(false).await;
}
