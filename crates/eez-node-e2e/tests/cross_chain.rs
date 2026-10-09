//! Cross-chain integration and nonce-gap regression tests.

use alloy_primitives::{Address, Bytes, TxHash, U256, keccak256};
use alloy_provider::{Provider as _, ProviderBuilder};
use alloy_sol_types::{SolCall, SolError, SolEvent, SolValue};

use eez_protocol::EEZL2_ADDRESS;
use eez_testkit::signals;
use eez_testkit::{
    ANVIL_ATTESTER_KEY, CrossChainOverlayRoundTripWorld, CrossChainStaticRoundTripWorld,
    CrossChainWorld, DEV_CHAIN_ID, IEEZL2Direct, IEmptyCall, INBOUND_USER, INestedSetterInner,
    INestedSetterOuter, IOverlayDriverL1, IOverlayProbeL2, IReturnData, IReturnDataWrapper,
    IRevertBubbleWrapper, IRevertingTarget, ISetterWrapper, IStaticRoundTripReader, IValue,
    IValueNoRet, L1_ROLLUP_ID, OUTBOUND_USER, POST_BATCHER_ENV, ProverMutation,
    REJECTING_BUILDER_ENV, SETTLE_TIMEOUT, Scenario, ScenarioCall, StateRead, TARGET_DEPLOYER,
    account_code, assert_latest_batch_signature, batches_posted, call_read, call_revert_data,
    count_events, cross_chain_source_proxy, deploy_nested_setter_inner, deploy_nested_setter_outer,
    events_since, l2_balance, l2_value, last_proxy_result, onchain_nonce, read_state_word,
    receipt_ok, run_scenarios, safe_block_hash, setter_call, setup_cross_chain,
    setup_cross_chain_codeless, setup_cross_chain_empty_call, setup_cross_chain_nested_setter,
    setup_cross_chain_outbound_return_data, setup_cross_chain_overlay_round_trip,
    setup_cross_chain_proxied, setup_cross_chain_return_data, setup_cross_chain_reverting,
    setup_cross_chain_static_round_trip, setup_cross_chain_with_env, sign_and_send, signer_address,
    state_root, value_no_ret, value_read, wait_for, wait_for_receipt_or_failure,
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
        let landed = wait_for_receipt_or_failure(&w.node, rpc_url, hash, SETTLE_TIMEOUT).await;
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
    let attester = signer_address(ANVIL_ATTESTER_KEY).unwrap();
    let w = setup_cross_chain().await.unwrap();
    let l1_rpc = w.l1_rpc();
    let l2_rpc = w.l2_rpc();

    // Discovery is served by both cross-chain fronts, never by the L2 node RPC.
    // It advertises the proxy bindings and chain IDs the Composer uses; the
    // fixture configures no bridge, so optional bridge fields are absent.
    let expected_info = serde_json::json!({
        "eezContracts": {
            "eezRegistryAddress": format!("{:#x}", w.cfg.eez_address),
            "eezRollupManagerAddress": format!("{:#x}", w.cfg.rollup_manager_address),
            "eezL2Address": format!("{EEZL2_ADDRESS:#x}"),
        },
        "supportedNetworks": {"eezL1": DEV_CHAIN_ID, "eezL2": w.l2_chain_id},
        "version": env!("CARGO_PKG_VERSION"),
    });
    for (front, source_chain_id) in [
        (w.l1_xchain(), DEV_CHAIN_ID),
        (w.l2_xchain(), w.l2_chain_id),
    ] {
        let info: serde_json::Value = ProviderBuilder::new()
            .connect_http(front.parse().unwrap())
            .client()
            .request("eez_composerInfo", ())
            .await
            .unwrap_or_else(|error| panic!("composer discovery on {front}: {error}"));
        assert_eq!(info, expected_info, "{front}");
        let nonce = alloy_primitives::B256::repeat_byte(0xa5);
        let data = alloy_primitives::Bytes::from(
            eez_protocol::discovery::composerInfoCall { nonce }.abi_encode(),
        );
        let response: alloy_primitives::Bytes = ProviderBuilder::new()
            .connect_http(front.parse().unwrap())
            .client()
            .request(
                "eth_call",
                serde_json::json!([{
                "to":eez_protocol::discovery::ADDRESS, "data":data
            }, "latest"]),
            )
            .await
            .unwrap();
        let response =
            eez_protocol::discovery::composerInfoCall::abi_decode_returns_validate(&response)
                .unwrap();
        assert_eq!(response.marker, eez_protocol::discovery::MARKER);
        assert_eq!(response.schemaVersion, U256::from(1));
        assert_eq!(response.echoedNonce, nonce);
        assert_eq!(response.sourceChainId, U256::from(source_chain_id));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&response.info).unwrap(),
            info
        );
        // Moved methods are handled locally by either front, not forwarded to
        // the source chain's RPC (which has no eez namespace).
        for (method, params) in [
            ("eez_getCrossChainQueueStatus", serde_json::json!([])),
            ("eez_getCrossChainTransaction", serde_json::json!([nonce])),
            ("eez_getSettlementByL2Block", serde_json::json!([nonce])),
            (
                "eez_getSettledL2RangesByL1Block",
                serde_json::json!([nonce]),
            ),
        ] {
            let _: serde_json::Value = ProviderBuilder::new()
                .connect_http(front.parse().unwrap())
                .client()
                .request(method, params)
                .await
                .unwrap();
        }
    }
    assert_no_composer_methods(&l1_rpc).await;
    assert_no_composer_methods(&l2_rpc).await;

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

    let inbound_hashes = [inbound];
    let outbound_hashes = [outbound];
    tokio::join!(
        assert_all_transactions_succeeded(&w, &l1_rpc, &inbound_hashes, "inbound smoke"),
        assert_all_transactions_succeeded(&w, &l2_rpc, &outbound_hashes, "outbound smoke"),
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

async fn assert_no_composer_methods(rpc: &str) {
    let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());
    for method in [
        "eez_composerInfo",
        "eez_getCrossChainQueueStatus",
        "eez_getCrossChainTransaction",
        "eez_getSettlementByL2Block",
        "eez_getSettledL2RangesByL1Block",
        "eez_executionWitnessByBlockHash",
    ] {
        let result: Result<serde_json::Value, _> = provider.client().request(method, ()).await;
        assert!(
            result
                .unwrap_err()
                .as_error_resp()
                .is_some_and(|error| error.code == -32601),
            "{rpc} must not expose {method}"
        );
    }
    let result: alloy_primitives::Bytes = provider
        .client()
        .request(
            "eth_call",
            serde_json::json!([{
        "to":eez_protocol::discovery::ADDRESS,
        "data":alloy_primitives::Bytes::from(eez_protocol::discovery::composerInfoCall {
            nonce:alloy_primitives::B256::ZERO
        }.abi_encode())
    }, "latest"]),
        )
        .await
        .unwrap();
    assert!(
        result.is_empty(),
        "ordinary RPC must not synthesize runtime discovery"
    );
}

/// Two configured relays: the first rejects every bundle and the second
/// forwards it. One dispatch to both must still settle through the accepting
/// relay without a retry or a user failure charge.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejecting_builder_does_not_block_settlement() {
    let w = setup_cross_chain_with_env(&[(REJECTING_BUILDER_ENV, "1".to_owned())])
        .await
        .unwrap();
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
            v: U256::from(47u64),
        }
        .abi_encode(),
        600_000,
    )
    .await
    .expect("inbound transaction must be admitted");
    assert_all_transactions_succeeded(&w, &l1_rpc, &[inbound], "multi-relay inbound").await;
    let (eez, rollup_id) = (w.cfg.eez_address, w.cfg.rollup_id);
    let converged = wait_for(SETTLE_TIMEOUT, || {
        let (l1_rpc, l2_rpc) = (l1_rpc.clone(), l2_rpc.clone());
        async move {
            let applied = l2_value(&l2_rpc, w.value_l2).await? == U256::from(47u64);
            let l1_root = state_root(&l1_rpc, eez, rollup_id).await?;
            let l2_root = safe_block_hash(&l2_rpc).await?;
            Ok((applied && l2_root == Some(l1_root)).then_some(()))
        }
    })
    .await;
    if let Err(err) = converged {
        panic!(
            "multi-relay submission did not settle: {err:#}{}",
            w.settlement_diagnostics()
        );
    }
    assert!(
        batches_posted(&l1_rpc, w.cfg.eez_address, w.dep.deploy_block)
            .await
            .unwrap()
            >= 1,
        "multi-relay submission must post at least one batch",
    );
    // Every bundle also reached the rejecting relay; its rejection was
    // aggregated instead of failing the dispatch.
    assert!(
        w.rejected_bundle_requests().unwrap() >= 1,
        "the rejecting relay must receive the bundle",
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
        "a relay rejection must not evict a valid user",
    );
    w.node.assert_no_divergence_failure_logs();
}

/// `EEZ_L1_POSTBATCH_TARGET` sends every postBatch through the
/// `EEZPostBatcher` wrapper. Both directions must settle, the node's own
/// derivation must follow the wrapped posts to the settled root, and every
/// registry `BatchPosted` must come from a transaction sent to the wrapper.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_batch_target_wrapper_settles_both_directions() {
    let w = setup_cross_chain_with_env(&[(POST_BATCHER_ENV, "1".to_owned())])
        .await
        .unwrap();
    assert_ne!(w.post_batch_target, w.cfg.eez_address);
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
            v: U256::from(53u64),
        }
        .abi_encode(),
        600_000,
    )
    .await
    .expect("inbound transaction must be admitted");
    let outbound = sign_and_send(
        &w.l2_xchain(),
        OUTBOUND_USER,
        w.l2_chain_id,
        onchain_nonce(&l2_rpc, OUTBOUND_USER).await.unwrap(),
        Some(w.outbound_proxy),
        U256::ZERO,
        IValue::setValueCall {
            v: U256::from(59u64),
        }
        .abi_encode(),
        900_000,
    )
    .await
    .expect("outbound transaction must be admitted");
    let (inbound_hashes, outbound_hashes) = ([inbound], [outbound]);
    tokio::join!(
        assert_all_transactions_succeeded(&w, &l1_rpc, &inbound_hashes, "wrapped inbound"),
        assert_all_transactions_succeeded(&w, &l2_rpc, &outbound_hashes, "wrapped outbound"),
    );
    let (eez, rollup_id) = (w.cfg.eez_address, w.cfg.rollup_id);
    let converged = wait_for(SETTLE_TIMEOUT, || {
        let (l1_rpc, l2_rpc) = (l1_rpc.clone(), l2_rpc.clone());
        async move {
            let inbound_applied = l2_value(&l2_rpc, w.value_l2).await? == U256::from(53u64);
            let outbound_applied = l2_value(&l1_rpc, w.outbound_value).await? == U256::from(59u64);
            let l1_root = state_root(&l1_rpc, eez, rollup_id).await?;
            let l2_root = safe_block_hash(&l2_rpc).await?;
            Ok((inbound_applied && outbound_applied && l2_root == Some(l1_root)).then_some(()))
        }
    })
    .await;
    if let Err(err) = converged {
        panic!(
            "wrapped postBatch did not settle both directions: {err:#}{}",
            w.settlement_diagnostics()
        );
    }

    let l1 = ProviderBuilder::new().connect_http(l1_rpc.parse().unwrap());
    let posted = l1
        .get_logs(
            &alloy_rpc_types_eth::Filter::new()
                .address(eez)
                .event_signature(eez_protocol::abi::BatchPosted::SIGNATURE_HASH)
                .from_block(w.dep.deploy_block),
        )
        .await
        .unwrap();
    assert!(
        !posted.is_empty(),
        "the wrapper must post at least one batch"
    );
    for log in posted {
        let hash = log.transaction_hash.unwrap();
        let tx = l1.get_transaction_by_hash(hash).await.unwrap().unwrap();
        assert_eq!(
            alloy_consensus::Transaction::to(&tx),
            Some(w.post_batch_target),
            "{hash}: every postBatch is sent to the wrapper"
        );
        assert!(
            alloy_consensus::Transaction::input(&tx)
                .starts_with(&eez_protocol::abi::postAndVerifyBatchCall::SELECTOR),
            "{hash}: the wrapper call keeps the registry's calldata"
        );
    }
    assert!(
        w.node
            .count_signals(&["eez.composer.postbatch.target_validated"])
            .unwrap()
            >= 1,
        "the node must validate the wrapper before posting"
    );
    w.node.assert_no_divergence_failure_logs();
}

async fn assert_real_signer_rejects(mutation: ProverMutation, tampered_input: &str) {
    let attester = signer_address(ANVIL_ATTESTER_KEY).unwrap();
    let w = setup_cross_chain_proxied(mutation, attester).await.unwrap();
    let l1_rpc = w.l1_rpc();
    let proxy = w.prover_proxy.as_ref().expect("real signer proxy");
    let starting_nonce = onchain_nonce(&l1_rpc, INBOUND_USER).await.unwrap();
    let tx_hash = sign_and_send(
        &w.l1_xchain(),
        INBOUND_USER,
        DEV_CHAIN_ID,
        starting_nonce,
        Some(w.setter_proxy),
        U256::ZERO,
        IValue::setValueCall { v: U256::from(75) }.abi_encode(),
        600_000,
    )
    .await
    .expect("valid transaction must be admitted before mutating its proof");

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
    assert_eq!(
        receipt_ok(&l1_rpc, tx_hash).await.unwrap(),
        None,
        "transaction with tampered {tampered_input} proof unexpectedly landed",
    );
    assert_eq!(
        onchain_nonce(&l1_rpc, INBOUND_USER).await.unwrap(),
        starting_nonce,
        "rejected transaction must not consume its source nonce",
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
/// resource capacity is exceeded) or becomes opaque because its actionable
/// identity is unsafe, the Composer must retain source ownership for manual
/// recovery. In particular, an untrusted identity must never select a source
/// transaction for eviction.
async fn assert_opaque_prover_rejection_retains_the_candidate(mutation: ProverMutation) {
    let attester = signer_address(ANVIL_ATTESTER_KEY).unwrap();
    let w = setup_cross_chain_proxied(mutation, attester).await.unwrap();
    let l1_rpc = w.l1_rpc();
    let l2_rpc = w.l2_rpc();
    let starting_nonce = onchain_nonce(&l1_rpc, INBOUND_USER).await.unwrap();
    let starting_value = l2_value(&l2_rpc, w.value_l2).await.unwrap();
    let proxy = w.prover_proxy.as_ref().expect("rejection proxy");
    let starting_rejections = proxy.rejections();
    let cursor = w.node.signal_cursor().unwrap();
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

    wait_for(SETTLE_TIMEOUT, || async {
        Ok((w.node.log_count_matching(&[
            "proof unavailable or rejected; retaining the complete range and all source ownership",
        ])? >= 4)
            .then_some(()))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "composer did not retain and retry the opaque prover rejection beyond the old eviction bound: {error:#}{}",
            w.settlement_diagnostics(),
        );
    });

    assert!(
        proxy.rejections() > starting_rejections,
        "the candidate must exercise the injected opaque rejection"
    );
    assert!(
        !w.node
            .signals_since(cursor)
            .unwrap()
            .into_iter()
            .any(|event| {
                event.name == signals::TX_POISON_EVICTED
                    && event.b256("tx_hash").ok() == Some(tx_hash)
            }),
        "opaque prover rejection must retain the source transaction for manual recovery"
    );
    assert_eq!(
        receipt_ok(&l1_rpc, tx_hash).await.unwrap(),
        None,
        "the rejected and retained transaction must not appear to have landed"
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
async fn opaque_prover_rejection_retains_the_candidate() {
    assert_opaque_prover_rejection_retains_the_candidate(ProverMutation::ResourceExhausted).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mismatched_actionable_rejection_retains_the_candidate() {
    assert_opaque_prover_rejection_retains_the_candidate(ProverMutation::MismatchedActionable)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_signer_attester_mismatch_never_submits_a_batch() {
    let wrong_attester = Address::repeat_byte(0x44);
    let w = setup_cross_chain_proxied(ProverMutation::None, wrong_attester)
        .await
        .unwrap();
    let proxy = w.prover_proxy.as_ref().expect("real signer proxy");
    let l1_rpc = w.l1_rpc();
    let starting_nonce = onchain_nonce(&l1_rpc, INBOUND_USER).await.unwrap();
    let tx_hash = sign_and_send(
        &w.l1_xchain(),
        INBOUND_USER,
        DEV_CHAIN_ID,
        starting_nonce,
        Some(w.setter_proxy),
        U256::ZERO,
        IValue::setValueCall { v: U256::from(77) }.abi_encode(),
        600_000,
    )
    .await
    .expect("valid transaction must be admitted before checking attester identity");

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
        batches_posted(&l1_rpc, w.cfg.eez_address, w.dep.deploy_block)
            .await
            .unwrap(),
        0,
        "an attestation from an unexpected signer must never reach L1",
    );
    assert_eq!(
        state_root(&l1_rpc, w.cfg.eez_address, w.cfg.rollup_id)
            .await
            .unwrap(),
        w.cfg.initial_state,
        "signer mismatch must leave the registered state root unchanged",
    );
    assert_eq!(
        receipt_ok(&l1_rpc, tx_hash).await.unwrap(),
        None,
        "transaction attested by the unexpected signer must not land",
    );
    assert_eq!(
        onchain_nonce(&l1_rpc, INBOUND_USER).await.unwrap(),
        starting_nonce,
        "unexpected attestation must not consume the source nonce",
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

// Round trips in which L1 calls back into a suspended L2 execution. The probe's
// `value` stands in for a token balance; `completed` and `pings` show the
// scenario actually ran, since `value` is already zero beforehand.
const OVERLAY_AMOUNT: u64 = 800_000_000_000;
const OVERLAY_GAS: u64 = 3_000_000;

fn overlay_value(w: &CrossChainOverlayRoundTripWorld) -> StateRead {
    call_read(
        w.probe,
        "value()",
        IOverlayProbeL2::valueCall {}.abi_encode(),
    )
}

fn overlay_completed(w: &CrossChainOverlayRoundTripWorld) -> StateRead {
    call_read(
        w.probe,
        "completed()",
        IOverlayProbeL2::completedCall {}.abi_encode(),
    )
}

fn overlay_pings(w: &CrossChainOverlayRoundTripWorld) -> StateRead {
    call_read(
        w.driver,
        "pings()",
        IOverlayDriverL1::pingsCall {}.abi_encode(),
    )
}

/// A delivery callback, then a burn callback whose own L1 call does not re-enter
/// L2. The burn's state must survive that call; the earlier delivery's state
/// must not be applied over it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlay_burn_survives_nested_call_without_reentry() {
    let w = setup_cross_chain_overlay_round_trip().await.unwrap();
    Scenario::new("delivery, then burn with a non-reentrant L1 call")
        .outbound(
            ScenarioCall::new(
                w.probe,
                IOverlayProbeL2::startDeliverThenBurnAndPingCall {
                    driver: w.driver_on_l2,
                    probe: w.probe_on_l1,
                    amount: U256::from(OVERLAY_AMOUNT),
                }
                .abi_encode(),
            )
            .with_gas_limit(OVERLAY_GAS),
        )
        .expect_l2_state(overlay_value(&w), 0u64)
        .expect_l2_state(overlay_completed(&w), 1u64)
        .expect_l1_state(overlay_pings(&w), 1u64)
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();
}

/// Two sibling callbacks into one suspended L2 call, then a second L1 call that
/// does not re-enter L2. Only the latest callback state belongs to the first
/// call; the second call must not apply the earlier one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlay_sibling_callback_state_is_not_reused_by_later_call() {
    let w = setup_cross_chain_overlay_round_trip().await.unwrap();
    Scenario::new("delivery and burn callbacks, then a non-reentrant L1 call")
        .outbound(
            ScenarioCall::new(
                w.probe,
                IOverlayProbeL2::startDeliverBurnThenPingCall {
                    driver: w.driver_on_l2,
                    probe: w.probe_on_l1,
                    amount: U256::from(OVERLAY_AMOUNT),
                }
                .abi_encode(),
            )
            .with_gas_limit(OVERLAY_GAS),
        )
        .expect_l2_state(overlay_value(&w), 0u64)
        .expect_l2_state(overlay_completed(&w), 1u64)
        .expect_l1_state(overlay_pings(&w), 1u64)
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();
}

/// The same delivery and burn driven by an L1 source: the L2 callbacks run as
/// inbound targets, and the burn's L1 call re-enters the suspended L1 source.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlay_inbound_round_trip_reenters_suspended_l1() {
    let w = setup_cross_chain_overlay_round_trip().await.unwrap();
    Scenario::new("inbound delivery, then burn with a non-reentrant L1 call")
        .inbound(
            ScenarioCall::new(
                w.driver,
                IOverlayDriverL1::deliverThenBurnAndPingCall {
                    probe: w.probe_on_l1,
                    driver: w.driver_on_l2,
                    amount: U256::from(OVERLAY_AMOUNT),
                }
                .abi_encode(),
            )
            .with_gas_limit(OVERLAY_GAS),
        )
        .expect_l2_state(overlay_value(&w), 0u64)
        .expect_l1_state(overlay_pings(&w), 1u64)
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();
}

/// An L1 call makes a delivery callback and then reverts. The failed call
/// undoes the delivery, so the suspended L2 caller must not apply it, and its
/// source simulation succeeds. Settling a failed call with descendants is not
/// supported, so the Composer then rejects the transaction for that reason;
/// applying the delivery instead fails the source with `ValueNotCleared`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlay_failed_l1_call_discards_its_callback_state() {
    let w = setup_cross_chain_overlay_round_trip().await.unwrap();
    let l2_rpc = w.l2_rpc();
    let nonce = onchain_nonce(&l2_rpc, OUTBOUND_USER).await.unwrap();
    let hash = sign_and_send(
        &w.l2_xchain(),
        OUTBOUND_USER,
        w.l2_chain_id,
        nonce,
        Some(w.probe),
        U256::ZERO,
        IOverlayProbeL2::startDeliverThenRevertCall {
            driver: w.driver_on_l2,
            probe: w.probe_on_l1,
            amount: U256::from(OVERLAY_AMOUNT),
        }
        .abi_encode(),
        OVERLAY_GAS,
    )
    .await
    .unwrap();
    let outcome = wait_for_receipt_or_failure(&w.node, &l2_rpc, hash, SETTLE_TIMEOUT).await;
    assert!(
        outcome.is_err(),
        "a failed call with descendants must not be included: {outcome:?}"
    );

    let status: serde_json::Value = ProviderBuilder::new()
        .connect_http(w.l2_xchain().parse().unwrap())
        .client()
        .request("eez_getCrossChainTransaction", (hash,))
        .await
        .unwrap();
    assert_eq!(
        status["ownership"]["reason"], "SimulationFailed",
        "{status}"
    );
    let rejection = &status["ownership"]["rejection"];
    assert_eq!(
        rejection["source"],
        serde_json::Value::Null,
        "the source must not fail once the failed call's callback is discarded: {status}"
    );
    assert!(
        rejection["error"].as_str().is_some_and(|error| {
            error.contains("unsuccessful cross-chain calls with descendants are not supported")
        }),
        "{status}"
    );
    assert_eq!(
        read_state_word(&l2_rpc, &overlay_completed(&w))
            .await
            .unwrap(),
        U256::ZERO
    );
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_mixed_static_reads_and_write_settle_together() {
    assert_mixed_static_reads_and_write_settle(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_changing_static_reads_and_write_settle_and_replay() {
    assert_mixed_static_reads_and_write_settle(true).await;
}

async fn assert_mixed_static_reads_and_write_settle(changing: bool) {
    let w = setup_cross_chain().await.unwrap();
    let expected_sum = if changing { 168u64 } else { 66u64 };
    Scenario::new("two static reads, mutable write, two static reads")
        .outbound(
            ScenarioCall::new(
                w.outbound_wrapper,
                if changing {
                    ISetterWrapper::readChangingValueAroundWriteCall { v: U256::from(79) }
                        .abi_encode()
                } else {
                    ISetterWrapper::readWriteReadCall { v: U256::from(79) }.abi_encode()
                },
            )
            .with_gas_limit(2_000_000),
        )
        .expect_l1_state(value_read(w.outbound_value), 79u64)
        .expect_l2_state(
            call_read(
                w.outbound_wrapper,
                "mixedReadSum()",
                ISetterWrapper::mixedReadSumCall {}.abi_encode(),
            ),
            expected_sum,
        )
        .expect_l2_state(
            call_read(
                w.outbound_wrapper,
                "completedProxyCalls()",
                ISetterWrapper::completedProxyCallsCall {}.abi_encode(),
            ),
            1u64,
        )
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();
    assert_follower_replays(
        &w,
        "mixed-outbound-follower",
        &call_read(
            w.outbound_wrapper,
            "mixedReadSum()",
            ISetterWrapper::mixedReadSumCall {}.abi_encode(),
        ),
        U256::from(expected_sum),
    )
    .await;
}

/// Start a follower from canonical L1/blob data, with no Composer proposal
/// state, and require it to reach the node's safe head and derive `expected`
/// for the L2 `read`.
async fn assert_follower_replays(
    w: &CrossChainWorld,
    name: &str,
    read: &StateRead,
    expected: U256,
) {
    use alloy_rpc_types_eth::BlockNumberOrTag;
    use eez_testkit::{NodeBinary, NodeConfig, NodeHandle, block_number_and_hash_at};

    let target = block_number_and_hash_at(&w.l2_rpc(), BlockNumberOrTag::Safe)
        .await
        .unwrap()
        .unwrap();
    let follower = NodeHandle::start(
        name,
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&w.cfg.l2_genesis.0),
        },
        &w.partial_follower_env(),
    )
    .await
    .unwrap();
    assert_no_composer_methods(&follower.l2_rpc_url()).await;
    let replayed = wait_for(SETTLE_TIMEOUT, || async {
        let actual =
            block_number_and_hash_at(&follower.l2_rpc_url(), BlockNumberOrTag::Number(target.0))
                .await?;
        let safe = block_number_and_hash_at(&follower.l2_rpc_url(), BlockNumberOrTag::Safe).await?;
        if actual != Some(target) || safe.is_none_or(|head| head.0 < target.0) {
            return Ok(None);
        }
        // Replayed blocks can be visible before startup finishes validating L1
        // history. Pending eth_call remains gated until that validation completes.
        read_state_word(&follower.l2_rpc_url(), read)
            .await
            .map(Some)
    })
    .await
    .unwrap_or_else(|error| panic!("{name} failed: {error:#}\n{}", follower.log_tail(60)));
    assert_eq!(replayed, expected);
    follower.assert_no_divergence_failure_logs();
    w.node.assert_no_divergence_failure_logs();
}

const STATIC_ROUND_TRIP_GAS: u64 = 1_000_000;

fn static_reader_state(w: &CrossChainStaticRoundTripWorld) -> (StateRead, StateRead) {
    (
        call_read(
            w.reader,
            "lastRead()",
            IStaticRoundTripReader::lastReadCall {}.abi_encode(),
        ),
        call_read(
            w.reader,
            "reads()",
            IStaticRoundTripReader::readsCall {}.abi_encode(),
        ),
    )
}

/// A top-level static read whose remote target makes one static callback to
/// the reader's chain settles as one static entry carrying that callback. The
/// reader's own write records the round-trip result, and a follower replays
/// the settled chain.
async fn assert_static_round_trip_settles(inbound: bool) {
    let w = setup_cross_chain_static_round_trip(inbound, true)
        .await
        .unwrap();
    let (last_read, reads) = static_reader_state(&w);
    let call = ScenarioCall::new(w.reader, IStaticRoundTripReader::readCall {}.abi_encode())
        .with_gas_limit(STATIC_ROUND_TRIP_GAS);
    let scenario = if inbound {
        Scenario::new("L1 static read with a static L1 callback")
            .inbound(call)
            .expect_l1_state(last_read, 1u64)
            .expect_l1_state(reads, 1u64)
    } else {
        Scenario::new("L2 static read with a static L2 callback")
            .outbound(call)
            .expect_l2_state(last_read, 1u64)
            .expect_l2_state(reads, 1u64)
    };
    scenario.expect_settled_fully().run(&w).await.unwrap();

    // An L1 read is a lookup that changes no L2 state, so the follower reads
    // an immutable of the L2 forwarder. An L2 read is replayed by its result.
    let (replay_read, expected) = if inbound {
        (
            call_read(
                w.forwarder,
                "callback()",
                keccak256("callback()")[..4].to_vec(),
            ),
            U256::from_be_slice(w.callback.as_slice()),
        )
    } else {
        (static_reader_state(&w).0, U256::from(1))
    };
    assert_follower_replays(&w, "static-round-trip-follower", &replay_read, expected).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_static_read_with_static_callback_settles() {
    assert_static_round_trip_settles(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_static_read_with_static_callback_settles_and_replays() {
    assert_static_round_trip_settles(false).await;
}

/// A static read whose callback source proxy is not deployed on the reading
/// chain would revert on chain: the protocol cannot create the proxy inside
/// the read's static frame. Simulation alone would answer the callback, so the
/// Composer must reject the transaction at admission and name the proxy.
async fn assert_static_round_trip_without_callback_proxy_is_rejected(inbound: bool) {
    let w = setup_cross_chain_static_round_trip(inbound, false)
        .await
        .unwrap();
    let source_rpc = if inbound { w.l1_rpc() } else { w.l2_rpc() };
    assert!(
        account_code(&source_rpc, w.callback_source_proxy)
            .await
            .unwrap()
            .is_empty(),
        "the fixture must leave the callback source proxy undeployed"
    );
    let (front, key, chain_id) = if inbound {
        (w.l1_xchain(), INBOUND_USER, DEV_CHAIN_ID)
    } else {
        (w.l2_xchain(), OUTBOUND_USER, w.l2_chain_id)
    };
    let nonce = onchain_nonce(&source_rpc, key).await.unwrap();
    let hash = sign_and_send(
        &front,
        key,
        chain_id,
        nonce,
        Some(w.reader),
        U256::ZERO,
        IStaticRoundTripReader::readCall {}.abi_encode(),
        STATIC_ROUND_TRIP_GAS,
    )
    .await
    .unwrap();
    let outcome = wait_for_receipt_or_failure(&w.node, &source_rpc, hash, SETTLE_TIMEOUT).await;
    assert!(
        outcome.is_err(),
        "a read whose callback proxy is missing must not be included: {outcome:?}"
    );

    let status: serde_json::Value = ProviderBuilder::new()
        .connect_http(w.l2_xchain().parse().unwrap())
        .client()
        .request("eez_getCrossChainTransaction", (hash,))
        .await
        .unwrap();
    assert_eq!(
        status["ownership"]["reason"], "SimulationFailed",
        "{status}"
    );
    let expected = format!(
        "static callback source proxy {} is not deployed",
        w.callback_source_proxy
    );
    assert!(
        status["ownership"]["rejection"]["error"]
            .as_str()
            .is_some_and(|error| error.contains(&expected)),
        "{status}"
    );
    let (_, reads) = static_reader_state(&w);
    assert_eq!(
        read_state_word(&source_rpc, &reads).await.unwrap(),
        U256::ZERO
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_static_read_without_callback_proxy_is_rejected() {
    assert_static_round_trip_without_callback_proxy_is_rejected(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_static_read_without_callback_proxy_is_rejected() {
    assert_static_round_trip_without_callback_proxy_is_rejected(false).await;
}

/// Wallets obtain the source gas limit before they have a signed envelope.
#[tokio::test]
async fn wallet_gas_estimates_bidirectional() {
    wallet_gas_estimates_profile(eez_testkit::NativeL1Profile::EthereumOsaka, false).await;
}

#[tokio::test]
async fn wallet_gas_estimates_bidirectional_gnosis() {
    wallet_gas_estimates_profile(eez_testkit::NativeL1Profile::GnosisPrague, false).await;
}

#[tokio::test]
async fn wallet_gas_estimates_post_batch_wrapper() {
    wallet_gas_estimates_profile(eez_testkit::NativeL1Profile::EthereumOsaka, true).await;
}

async fn wallet_gas_estimates_profile(profile: eez_testkit::NativeL1Profile, wrapper: bool) {
    use alloy_rpc_types_eth::BlockNumberOrTag;
    use eez_testkit::{NodeBinary, NodeConfig, NodeHandle, block_number_and_hash_at};
    let extra = if wrapper {
        vec![(POST_BATCHER_ENV, "1".to_owned())]
    } else {
        vec![]
    };
    let w = eez_testkit::setup_cross_chain_with_profile(profile, &extra)
        .await
        .unwrap();
    for (front, source, key, chain_id, target, destination, value_contract, value) in [
        (
            w.l1_xchain(),
            w.l1_rpc(),
            INBOUND_USER,
            DEV_CHAIN_ID,
            w.setter_proxy,
            w.l2_rpc(),
            w.value_l2,
            141u64,
        ),
        (
            w.l2_xchain(),
            w.l2_rpc(),
            OUTBOUND_USER,
            w.l2_chain_id,
            w.outbound_proxy,
            w.l1_rpc(),
            w.outbound_value,
            143u64,
        ),
    ] {
        let from = signer_address(key).unwrap();
        let data = Bytes::from(
            IValue::setValueCall {
                v: U256::from(value),
            }
            .abi_encode(),
        );
        let provider = ProviderBuilder::new().connect_http(front.parse().unwrap());
        let initial_nonce = onchain_nonce(&source, key).await.unwrap();
        let ordinary = serde_json::json!([{"from":from,"to":Address::repeat_byte(0x99),"value":"0x1"}, "latest"]);
        let upstream = ProviderBuilder::new().connect_http(source.parse().unwrap());
        let expected: U256 = upstream
            .client()
            .request("eth_estimateGas", ordinary.clone())
            .await
            .unwrap();
        let actual: U256 = provider
            .client()
            .request("eth_estimateGas", ordinary.clone())
            .await
            .unwrap();
        assert_eq!(
            actual, expected,
            "ordinary estimates retain upstream results"
        );
        let mixed = wallet_estimate_rpc(&front, serde_json::json!([
            {"jsonrpc":"2.0","id":"estimate","method":"eth_estimateGas","params":[{"from":from,"to":target,"data":data},"pending"]},
            {"jsonrpc":"2.0","id":17,"method":"eth_chainId","params":[]},
            {"jsonrpc":"2.0","method":"eth_estimateGas","params":ordinary}
        ])).await;
        let items = mixed.as_array().expect("mixed batch response");
        assert_eq!(items.len(), 2, "notifications have no response");
        assert!(
            items
                .iter()
                .any(|i| i["id"] == "estimate" && i["result"].is_string()),
            "cross-chain batch estimate: {mixed}"
        );
        assert!(
            items
                .iter()
                .any(|i| i["id"] == 17 && i["result"] == format!("0x{chain_id:x}"))
        );
        let single = wallet_estimate_rpc(
            &front,
            serde_json::json!({
                "jsonrpc":"2.0","id":"estimate","method":"eth_estimateGas",
                "params":[{"from":from,"to":target,"data":data},"pending"]
            }),
        )
        .await;
        let gas: U256 = serde_json::from_value(single["result"].clone()).unwrap_or_else(|error| {
            panic!(
                "wallet estimate {single}: {error}{}",
                w.settlement_diagnostics()
            )
        });
        let gas = u64::try_from(gas).unwrap();
        assert!(
            (21_000..2_000_000).contains(&gas),
            "useful source estimate: {gas}"
        );
        assert_eq!(
            onchain_nonce(&source, key).await.unwrap(),
            initial_nonce,
            "estimation never spends or reserves the user's nonce"
        );
        for params in [
            serde_json::json!([{"from":from,"to":target,"data":data,"gas":"0x5208"}, "pending"]),
            serde_json::json!([{"from":from,"to":target,"data":data}, "pending", {}]),
            serde_json::json!([{"to":target,"data":data}, "pending"]),
        ] {
            let rejected = wallet_estimate_rpc(&front, serde_json::json!({"jsonrpc":"2.0","id":"estimate","method":"eth_estimateGas","params":params})).await;
            assert!(
                rejected.get("error").is_some() && rejected.get("result").is_none(),
                "unsupported/underfunded estimate must fail: {rejected}"
            );
        }
        let hash = sign_and_send(
            &front,
            key,
            chain_id,
            onchain_nonce(&source, key).await.unwrap(),
            Some(target),
            U256::ZERO,
            data.to_vec(),
            gas,
        )
        .await
        .unwrap();
        assert_all_transactions_succeeded(&w, &source, &[hash], "wallet estimate").await;
        let receipt = upstream
            .get_transaction_receipt(hash)
            .await
            .unwrap()
            .unwrap();
        assert!(
            gas >= receipt.gas_used && gas <= receipt.gas_used * 2 + 21_000,
            "simple wallet estimate {gas}, receipt {}",
            receipt.gas_used
        );
        eprintln!(
            "wallet {profile:?} source {chain_id}: estimate={gas}, receipt={}",
            receipt.gas_used
        );
        wait_for(SETTLE_TIMEOUT, || {
            let destination = destination.clone();
            async move {
                Ok(
                    (l2_value(&destination, value_contract).await? == U256::from(value))
                        .then_some(()),
                )
            }
        })
        .await
        .unwrap();
    }
    // Value transfers exercise the protocol's escrow/funding preparation, not
    // just a zero-value lookup. Each source uses its next canonical nonce.
    let amount = U256::from(1_000_000_000_000u64);
    let inbound_balance = l2_balance(&w.l2_rpc(), w.recipient).await.unwrap();
    for (front, source, key, chain_id, proxy, destination, recipient) in [
        (
            w.l1_xchain(),
            w.l1_rpc(),
            INBOUND_USER,
            DEV_CHAIN_ID,
            w.deposit_proxy,
            w.l2_rpc(),
            w.recipient,
        ),
        (
            w.l2_xchain(),
            w.l2_rpc(),
            OUTBOUND_USER,
            w.l2_chain_id,
            w.withdrawal_proxy,
            w.l1_rpc(),
            w.withdrawal_recipient,
        ),
    ] {
        let from = signer_address(key).unwrap();
        let before = l2_balance(&destination, recipient).await.unwrap();
        let reply = wallet_estimate_rpc(
            &front,
            serde_json::json!({
                "jsonrpc":"2.0","id":"estimate","method":"eth_estimateGas",
                "params":[{"from":from,"to":proxy,"value":amount,"data":"0x"},"pending"]
            }),
        )
        .await;
        let gas: U256 = serde_json::from_value(reply["result"].clone())
            .unwrap_or_else(|e| panic!("value estimate {reply}: {e}"));
        let gas = u64::try_from(gas).unwrap();
        assert!((21_000..2_000_000).contains(&gas));
        let tx = sign_and_send(
            &front,
            key,
            chain_id,
            onchain_nonce(&source, key).await.unwrap(),
            Some(proxy),
            amount,
            vec![],
            gas,
        )
        .await
        .unwrap();
        assert_all_transactions_succeeded(&w, &source, &[tx], "wallet value estimate").await;
        wait_for(SETTLE_TIMEOUT, || async {
            Ok((l2_balance(&destination, recipient).await? == before + amount).then_some(()))
        })
        .await
        .unwrap();
    }
    let target = wait_for(SETTLE_TIMEOUT, || async {
        let safe = block_number_and_hash_at(&w.l2_rpc(), BlockNumberOrTag::Safe).await?;
        let root = state_root(&w.l1_rpc(), w.cfg.eez_address, w.cfg.rollup_id).await?;
        Ok(safe.filter(|(_, hash)| *hash == root))
    })
    .await
    .unwrap();
    let follower = NodeHandle::start(
        "wallet-estimate-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&w.cfg.l2_genesis.0),
        },
        &w.partial_follower_env(),
    )
    .await
    .unwrap();
    wait_for(SETTLE_TIMEOUT, || async {
        let actual =
            block_number_and_hash_at(&follower.l2_rpc_url(), BlockNumberOrTag::Number(target.0))
                .await?;
        let safe = block_number_and_hash_at(&follower.l2_rpc_url(), BlockNumberOrTag::Safe).await?;
        Ok((actual == Some(target) && safe.is_some_and(|head| head.0 >= target.0)).then_some(()))
    })
    .await
    .unwrap();
    assert_eq!(
        l2_value(&follower.l2_rpc_url(), w.value_l2).await.unwrap(),
        U256::from(141)
    );
    assert_eq!(
        l2_balance(&follower.l2_rpc_url(), w.recipient)
            .await
            .unwrap(),
        inbound_balance + amount
    );
    follower.assert_no_divergence_failure_logs();
    w.node.assert_no_divergence_failure_logs();
}

// The fixture may still be settling deployments when readiness first opens.
// Exercise documented context retries, but never retry an execution or setup error.
async fn wallet_estimate_rpc(front: &str, request: serde_json::Value) -> serde_json::Value {
    wait_for(SETTLE_TIMEOUT, || async {
        let response: serde_json::Value = reqwest::Client::new()
            .post(front)
            .json(&request)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let estimate = if let Some(items) = response.as_array() {
            items.iter().find(|item| item["id"] == "estimate").unwrap()
        } else {
            &response
        };
        let context_wait = estimate["error"]["code"] == -32000
            && estimate["error"]["message"]
                .as_str()
                .is_some_and(|message| {
                    message.contains("settlement reconciliation pending")
                        || message.contains("chain context changed during estimation")
                });
        if context_wait {
            eprintln!("wallet context retry: {}", estimate["error"]);
            return Ok(None);
        }
        Ok(Some(response))
    })
    .await
    .unwrap()
}

/// Protocol rc.2 managers reject a cross-chain call whose destination is the
/// destination chain's own manager (`EEZDestinationForbidden`): it would
/// re-enter the manager under an authorized proxy's identity. The Composer
/// must reject it at admission and name the manager.
async fn assert_manager_destination_is_rejected(inbound: bool) {
    let w = setup_cross_chain().await.unwrap();
    let (source_rpc, front, key, chain_id, proxy, manager) = if inbound {
        let proxy = eez_testkit::create_cross_chain_proxy(
            &w.l1_rpc(),
            TARGET_DEPLOYER,
            w.cfg.eez_address,
            EEZL2_ADDRESS,
            w.cfg.rollup_id,
        )
        .await
        .unwrap();
        (
            w.l1_rpc(),
            w.l1_xchain(),
            INBOUND_USER,
            DEV_CHAIN_ID,
            proxy,
            EEZL2_ADDRESS,
        )
    } else {
        let proxy = eez_testkit::create_l2_cross_chain_proxy(
            &w.l2_rpc(),
            TARGET_DEPLOYER,
            w.cfg.eez_address,
            L1_ROLLUP_ID,
        )
        .await
        .unwrap();
        (
            w.l2_rpc(),
            w.l2_xchain(),
            OUTBOUND_USER,
            w.l2_chain_id,
            proxy,
            w.cfg.eez_address,
        )
    };
    let nonce = onchain_nonce(&source_rpc, key).await.unwrap();
    let hash = sign_and_send(
        &front,
        key,
        chain_id,
        nonce,
        Some(proxy),
        U256::ZERO,
        Vec::new(),
        1_000_000,
    )
    .await
    .unwrap();
    let outcome = wait_for_receipt_or_failure(&w.node, &source_rpc, hash, SETTLE_TIMEOUT).await;
    assert!(
        outcome.is_err(),
        "a call to the destination manager must not be included: {outcome:?}"
    );
    let status: serde_json::Value = ProviderBuilder::new()
        .connect_http(w.l2_rpc().parse().unwrap())
        .client()
        .request("eez_getCrossChainTransaction", (hash,))
        .await
        .unwrap();
    assert_eq!(
        status["ownership"]["reason"], "SimulationFailed",
        "{status}"
    );
    let expected = format!("manager {manager}");
    assert!(
        status["ownership"]["rejection"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("cross-chain call targets the chain")
                && error.contains(&expected)),
        "{status}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_call_to_the_l2_manager_is_rejected() {
    assert_manager_destination_is_rejected(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_call_to_the_l1_manager_is_rejected() {
    assert_manager_destination_is_rejected(false).await;
}
