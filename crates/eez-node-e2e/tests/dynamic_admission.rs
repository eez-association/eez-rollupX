//! Automatic admission against native L1 execution, real DA/ECDSA and an
//! independent follower. Counts describe workloads, not production limits.

use alloy_primitives::U256;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{SolCall, SolEvent, sol};
use eez_testkit::{
    ANVIL_KEY_2, ANVIL_KEY_4, IValue, NativeL1Profile, NodeBinary, NodeConfig, NodeHandle,
    l2_value, onchain_nonce, setup_cross_chain_with_l1_reorg, setup_cross_chain_with_profile,
    sign_and_send, wait_for, wait_for_safe_prefix_convergence,
};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_mins(4);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_proxy_multicall_estimate_reaches_final_post_and_replays() {
    use eez_testkit::ISetterWrapper;
    let world = setup_cross_chain_with_profile(NativeL1Profile::GnosisPrague, &env())
        .await
        .unwrap();
    let follower = NodeHandle::start(
        "multi-gas-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap();
    for wave in 0..2 {
        let cursor = world.node.signal_cursor().unwrap();
        let value = U256::from(70 + wave);
        let hash = sign_and_send(
            &world.l2_xchain(),
            ANVIL_KEY_4,
            world.l2_chain_id,
            nonce + wave,
            Some(world.outbound_wrapper),
            U256::ZERO,
            ISetterWrapper::setSameValueTwiceCall { v: value }.abi_encode(),
            1_200_000,
        )
        .await
        .unwrap();
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l2.get_transaction_receipt(hash).await?)
        })
        .await
        .unwrap();
        assert!(receipt.status());
        wait_for_safe_prefix_convergence(
            &[&world.node, &follower],
            receipt.block_number.unwrap(),
            TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(
            l2_value(&world.l1_rpc(), world.outbound_value)
                .await
                .unwrap(),
            value
        );
        let completed = ISetterWrapper::new(
            world.outbound_wrapper,
            ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap()),
        )
        .completedProxyCalls()
        .call()
        .await
        .unwrap();
        assert_eq!(completed, U256::from(2 * (wave + 1)));
        let events: Vec<serde_json::Value> = world
            .node
            .signals_since(cursor)
            .unwrap()
            .iter()
            .filter(|e| e.name == "eez.composer.admission.v1")
            .map(|e| serde_json::from_str(e.fields["admission_event"].as_str().unwrap()).unwrap())
            .collect();
        let estimate = events
            .iter()
            .find(|e| {
                e["kind"] == "source_estimate" && e["detail"]["source_hash"] == hash.to_string()
            })
            .unwrap();
        assert_eq!(estimate["detail"]["l1_call_count"], 2);
        assert_eq!(
            estimate["detail"]["entry_reserve_gas"],
            if wave == 0 { 370_000 } else { 110_000 }
        );
        let cid = &estimate["candidate_id"];
        let summary = events
            .iter()
            .find(|e| &e["candidate_id"] == cid && e["kind"] == "summary")
            .unwrap();
        assert_eq!(summary["detail"]["reason"], "snapshot_exhausted");
        let projection = events
            .iter()
            .find(|e| &e["candidate_id"] == cid && e["kind"] == "full_projection")
            .unwrap();
        assert!(
            projection["detail"]["projected_gas"].as_u64().unwrap() < 1_000_000,
            "bounded projection reaches final sizing"
        );
    }
    world.node.assert_no_divergence_failure_logs();
    follower.assert_no_divergence_failure_logs();
}

sol! {
    #[sol(rpc)]
    interface IDeferredQueue {
        function queueLength(uint64 rollupId) external view returns (uint256);
        function entryQueueIndex(uint64 rollupId) external view returns (uint256);
    }
}

fn env() -> Vec<(&'static str, String)> {
    [
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "always"),
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "1800"),
        ("EEZ_MAX_SPECULATIVE_DEPTH", "0"),
        ("EEZ_MAX_USER_TXS_PER_BUNDLE", "auto"),
        ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "500000000000"),
        ("EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS", "100000000000"),
        ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "1000000001"),
        ("RUST_LOG", "info"),
    ]
    .into_iter()
    .map(|(k, v)| (k, v.into()))
    .collect()
}

async fn mixed_wave(profile: NativeL1Profile, explicit_preparation: bool) {
    let mut config = env();
    if explicit_preparation {
        config.extend([
            ("EEZ_PREPARATION_TIME_MS", "500".into()),
            ("EEZ_PROOF_TIME_MS", "500".into()),
            ("EEZ_SUBMISSION_SLACK_MS", "700".into()),
        ]);
    }
    let world = setup_cross_chain_with_profile(profile, &config)
        .await
        .unwrap();
    let follower = NodeHandle::start(
        "auto-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let chain_id = l1.get_chain_id().await.unwrap();
    if explicit_preparation {
        // Learn from one small post before a busy wave. A sparse sample must
        // not impose a one-user/doubling ramp on a later finite queue.
        let warmup = sign_and_send(
            &world.l1_xchain(),
            ANVIL_KEY_2,
            chain_id,
            onchain_nonce(&world.l1_rpc(), ANVIL_KEY_2).await.unwrap(),
            Some(world.setter_proxy),
            U256::ZERO,
            IValue::setValueCall { v: U256::ZERO }.abi_encode(),
            400_000,
        )
        .await
        .unwrap();
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l1.get_transaction_receipt(warmup).await?)
        })
        .await
        .unwrap();
        assert!(receipt.status());
    }
    let inbound_nonce = onchain_nonce(&world.l1_rpc(), ANVIL_KEY_2).await.unwrap();
    let outbound_nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap();
    let cursor = world.node.signal_cursor().unwrap();
    let mut hashes = Vec::new();
    for i in 0..64u64 {
        let data = IValue::setValueCall {
            v: U256::from(i + 1),
        }
        .abi_encode();
        let inbound = sign_and_send(
            &world.l1_xchain(),
            ANVIL_KEY_2,
            chain_id,
            inbound_nonce + i,
            Some(world.setter_proxy),
            U256::ZERO,
            data.clone(),
            400_000,
        )
        .await
        .unwrap();
        let outbound = sign_and_send(
            &world.l2_xchain(),
            ANVIL_KEY_4,
            world.l2_chain_id,
            outbound_nonce + i,
            Some(world.outbound_proxy),
            U256::ZERO,
            data,
            400_000,
        )
        .await
        .unwrap();
        hashes.push((inbound, outbound));
    }
    let mut covered = 0;
    for (inbound, outbound) in hashes {
        let a = wait_for(TIMEOUT, || async {
            Ok(l1.get_transaction_receipt(inbound).await?)
        })
        .await
        .unwrap_or_else(|e| {
            panic!(
                "missing inbound {inbound}: {e:#}{}",
                world.settlement_diagnostics()
            )
        });
        let b = wait_for(TIMEOUT, || async {
            Ok(l2.get_transaction_receipt(outbound).await?)
        })
        .await
        .unwrap_or_else(|e| {
            panic!(
                "missing outbound {outbound}: {e:#}{}",
                world.settlement_diagnostics()
            )
        });
        assert!(
            a.status() && b.status(),
            "every accepted source must succeed"
        );
        covered = covered.max(b.block_number.unwrap());
    }
    wait_for_safe_prefix_convergence(&[&world.node, &follower], covered, TIMEOUT)
        .await
        .unwrap();
    wait_for(TIMEOUT, || async {
        Ok(
            (l2_value(&follower.l2_rpc_url(), world.value_l2).await? == U256::from(64))
                .then_some(()),
        )
    })
    .await
    .unwrap();
    assert_eq!(
        l2_value(&world.l1_rpc(), world.outbound_value)
            .await
            .unwrap(),
        U256::from(64)
    );
    assert_eq!(
        onchain_nonce(&world.l1_rpc(), ANVIL_KEY_2).await.unwrap(),
        inbound_nonce + 64
    );
    assert_eq!(
        onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap(),
        outbound_nonce + 64
    );
    let events = world.node.signals_since(cursor).unwrap();
    if explicit_preparation {
        assert!(events.iter().any(|e| {
            e.name == "eez.composer.admission.prepared"
                && e.fields["blob_preparation_ms"]
                    .as_u64()
                    .is_some_and(|n| n > 0)
                && e.fields["blob_count"].as_u64().is_some_and(|n| n > 0)
        }));
    }
    assert!(
        events
            .iter()
            .any(|e| e.name == "eez.composer.admission.prepared"
                && e.fields["accepted"].as_u64().is_some_and(|n| n > 12)),
        "automatic packing must grow beyond the old Chiado count: {}",
        world.settlement_diagnostics()
    );
    for e in events
        .iter()
        .filter(|e| e.name == "eez.composer.bundle.dispatched")
    {
        let attempts: Vec<u64> =
            serde_json::from_str(e.fields["attempt_counts"].as_str().unwrap()).unwrap();
        assert!(
            attempts.iter().all(|n| *n == 0),
            "local deferral must not charge a failed attempt"
        );
    }
    for forbidden in [
        "eez.composer.phase2.canonical_mismatch",
        "eez.composer.phase2.final_receipt_failed",
        "eez.composer.cc_compose.poison_eviction_completed",
    ] {
        assert!(
            !events.iter().any(|e| e.name == forbidden),
            "{forbidden}{}",
            world.settlement_diagnostics()
        );
    }
    world.node.assert_no_process_death();
    follower.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_automatic_mixed_wave_exceeds_twelve_and_replays() {
    mixed_wave(NativeL1Profile::EthereumOsaka, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_automatic_mixed_wave_exceeds_twelve_and_replays() {
    mixed_wave(NativeL1Profile::GnosisPrague, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_explicit_preparation_mixed_wave_after_small_post_replays() {
    mixed_wave(NativeL1Profile::EthereumOsaka, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_explicit_preparation_mixed_wave_after_small_post_replays() {
    mixed_wave(NativeL1Profile::GnosisPrague, true).await;
}

/// A fitting wave must reach one canonical Sync even after learning from one
/// small post. Summed L1 user gas limits exceed a block, but their ordered real
/// execution fits. This exercises companion admission and retained queue replacement.
async fn single_sync_wave(
    profile: NativeL1Profile,
    inbound_count: u64,
    outbound_count: u64,
    waves: u64,
    empty_inbound_return_data: bool,
    warmup: bool,
) {
    let mut config = env();
    // These waves test gas/blob fit, not a 500-ms throughput guarantee on every
    // CI CPU. Keep the slot cadence unchanged but start preparation earlier.
    // mixed_wave(..., true) still exercises admission with the tight budget.
    config.extend([
        ("EEZ_PREPARATION_TIME_MS", "1500".into()),
        ("EEZ_PROOF_TIME_MS", "500".into()),
        ("EEZ_SUBMISSION_SLACK_MS", "700".into()),
    ]);
    let world = setup_cross_chain_with_profile(profile, &config)
        .await
        .unwrap();
    let follower = NodeHandle::start(
        "single-sync-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    // Both fixture contracts expose the same setValue/value selectors. Choose
    // the existing deployment with the required execution return shape.
    let (inbound_value, inbound_proxy) = if empty_inbound_return_data {
        (world.inbound_no_ret, world.inbound_no_ret_proxy)
    } else {
        (world.value_l2, world.setter_proxy)
    };
    let initial_outbound_value = l2_value(&world.l1_rpc(), world.outbound_value)
        .await
        .unwrap();
    let chain = l1.get_chain_id().await.unwrap();
    let nonce = onchain_nonce(&world.l1_rpc(), ANVIL_KEY_2).await.unwrap();
    if warmup {
        let warm = sign_and_send(
            &world.l1_xchain(),
            ANVIL_KEY_2,
            chain,
            nonce,
            Some(inbound_proxy),
            U256::ZERO,
            IValue::setValueCall { v: U256::ZERO }.abi_encode(),
            2_000_000,
        )
        .await
        .unwrap();
        assert!(
            wait_for(TIMEOUT, || async {
                Ok(l1.get_transaction_receipt(warm).await?)
            })
            .await
            .unwrap()
            .status()
        );
    }
    let registry = IDeferredQueue::new(world.cfg.eez_address, &l1);
    let outbound_nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap();
    let mut previous_sync = None;
    let mut previous_l1_block = None;
    for wave in 0..waves {
        // Every prior queue is fully consumed but remains in storage. The
        // following post replaces active bounds and may overwrite retained
        // slots; cursor == length does not mean there is no stored payload.
        let previous_entries = if wave == 0 {
            u64::from(warmup)
        } else {
            inbound_count
        };
        assert_eq!(
            registry
                .queueLength(world.cfg.rollup_id)
                .call()
                .await
                .unwrap(),
            U256::from(previous_entries)
        );
        assert_eq!(
            registry
                .entryQueueIndex(world.cfg.rollup_id)
                .call()
                .await
                .unwrap(),
            U256::from(previous_entries)
        );
        // Start after an observed idle opportunity so the finite wave reaches
        // the held queue before the next selection, as in the 30-source case.
        let idle = world.node.signal_cursor().unwrap();
        wait_for(TIMEOUT, || async {
            Ok(world
                .node
                .signals_since(idle)?
                .iter()
                .any(|e| e.name == "eez.composer.settlement.decision" && e.fields["due"] == false)
                .then_some(()))
        })
        .await
        .unwrap();
        let cursor = world.node.signal_cursor().unwrap();
        let mut inbound = Vec::new();
        let mut outbound = Vec::new();
        for i in 0..inbound_count.max(outbound_count) {
            if i < inbound_count {
                inbound.push(
                    sign_and_send(
                        &world.l1_xchain(),
                        ANVIL_KEY_2,
                        chain,
                        nonce + u64::from(warmup) + wave * inbound_count + i,
                        Some(inbound_proxy),
                        U256::ZERO,
                        IValue::setValueCall {
                            v: U256::from(wave * inbound_count + i + 1),
                        }
                        .abi_encode(),
                        2_000_000,
                    )
                    .await
                    .unwrap(),
                );
            }
            if i < outbound_count {
                outbound.push(
                    sign_and_send(
                        &world.l2_xchain(),
                        ANVIL_KEY_4,
                        world.l2_chain_id,
                        outbound_nonce + wave * outbound_count + i,
                        Some(world.outbound_proxy),
                        U256::ZERO,
                        IValue::setValueCall {
                            v: U256::from(wave * outbound_count + i + 1),
                        }
                        .abi_encode(),
                        400_000,
                    )
                    .await
                    .unwrap(),
                );
            }
        }
        let mut l1_blocks = std::collections::BTreeSet::new();
        let mut l1_block_hashes = std::collections::BTreeSet::new();
        let mut final_inbound_root = None;
        for hash in inbound {
            let receipt = wait_for(TIMEOUT, || async {
                Ok(l1.get_transaction_receipt(hash).await?)
            })
            .await
            .unwrap_or_else(|e| panic!("{e:#}{}", world.settlement_diagnostics()));
            assert!(receipt.status());
            l1_blocks.insert(receipt.block_number.unwrap());
            l1_block_hashes.insert(receipt.block_hash.unwrap());
            if outbound_count == 0 {
                final_inbound_root = Some(
                    receipt
                        .inner
                        .logs()
                        .iter()
                        .rev()
                        .filter(|log| log.address() == world.cfg.eez_address)
                        .find_map(|log| {
                            eez_protocol::abi::L2ExecutionPerformed::decode_log(&log.inner)
                                .ok()
                                .filter(|event| event.rollupId == world.cfg.rollup_id)
                                .map(|event| event.newRoot)
                        })
                        .expect("every inbound source must apply its L2 root update"),
                );
            }
        }
        let mut syncs = std::collections::BTreeSet::new();
        for hash in outbound {
            let receipt = wait_for(TIMEOUT, || async {
                Ok(l2.get_transaction_receipt(hash).await?)
            })
            .await
            .unwrap_or_else(|e| panic!("{e:#}{}", world.settlement_diagnostics()));
            assert!(receipt.status());
            syncs.insert(receipt.block_number.unwrap());
        }
        if outbound_count == 0 {
            // Without an outbound source receipt, identify the Sync from the
            // final inbound's canonical L1 root update, never a dispatch log.
            let root = final_inbound_root.expect("the wave has inbound sources");
            let block = wait_for(TIMEOUT, || async { Ok(l2.get_block_by_hash(root).await?) })
                .await
                .unwrap_or_else(|e| panic!("{e:#}{}", world.settlement_diagnostics()));
            assert_eq!(
                l2.get_block_by_number(block.header.number.into())
                    .await
                    .unwrap()
                    .unwrap()
                    .header
                    .hash,
                root,
                "the final source root must identify a canonical L2 block"
            );
            syncs.insert(block.header.number);
        }
        assert_eq!(l1_blocks.len(), 1, "{}", world.settlement_diagnostics());
        assert_eq!(syncs.len(), 1, "{}", world.settlement_diagnostics());
        let l1_block = *l1_blocks.first().unwrap();
        let block = l1
            .get_block_by_number(l1_block.into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            l1_block_hashes,
            std::collections::BTreeSet::from([block.header.hash]),
            "every source receipt must belong to the canonical L1 block"
        );
        assert!(
            inbound_count * 2_000_000 > block.header.gas_limit,
            "summed declarations must exceed the real block budget"
        );
        let sync = *syncs.first().unwrap();
        assert!(previous_sync.is_none_or(|height| height < sync));
        assert!(previous_l1_block.is_none_or(|height| height < l1_block));
        previous_sync = Some(sync);
        previous_l1_block = Some(l1_block);
        wait_for_safe_prefix_convergence(&[&world.node, &follower], sync, TIMEOUT)
            .await
            .unwrap();
        if outbound_count == 0 {
            // Prove that all inbound calls executed in this one canonical
            // Sync on both nodes; a matching final setter value alone could
            // conceal an omitted earlier call.
            for rpc in [world.l2_rpc(), follower.l2_rpc_url()] {
                let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());
                let logs = provider
                    .get_logs(
                        &alloy_rpc_types_eth::Filter::new()
                            .address(inbound_value)
                            .event_signature(IValue::ValueSet::SIGNATURE_HASH)
                            .from_block(sync)
                            .to_block(sync),
                    )
                    .await
                    .unwrap();
                let values: Vec<_> = logs
                    .iter()
                    .map(|log| IValue::ValueSet::decode_log(&log.inner).unwrap().newValue)
                    .collect();
                let expected: Vec<_> = (1..=inbound_count)
                    .map(|i| U256::from(wave * inbound_count + i))
                    .collect();
                assert_eq!(
                    values, expected,
                    "every inbound call must replay in one Sync"
                );
            }
        }
        let expected_inbound = U256::from((wave + 1) * inbound_count);
        assert_eq!(
            l2_value(&world.l2_rpc(), inbound_value).await.unwrap(),
            expected_inbound
        );
        assert_eq!(
            l2_value(&follower.l2_rpc_url(), inbound_value)
                .await
                .unwrap(),
            expected_inbound
        );
        assert_eq!(
            l2_value(&world.l1_rpc(), world.outbound_value)
                .await
                .unwrap(),
            if outbound_count == 0 {
                initial_outbound_value
            } else {
                U256::from((wave + 1) * outbound_count)
            }
        );
        assert_eq!(
            onchain_nonce(&world.l1_rpc(), ANVIL_KEY_2).await.unwrap(),
            nonce + u64::from(warmup) + (wave + 1) * inbound_count
        );
        assert_eq!(
            onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap(),
            outbound_nonce + (wave + 1) * outbound_count
        );
        assert_eq!(
            registry
                .queueLength(world.cfg.rollup_id)
                .call()
                .await
                .unwrap(),
            U256::from(inbound_count)
        );
        assert_eq!(
            registry
                .entryQueueIndex(world.cfg.rollup_id)
                .call()
                .await
                .unwrap(),
            U256::from(inbound_count)
        );
        let events = world.node.signals_since(cursor).unwrap();
        // The production caller must budget protocol work omitted by source
        // simulation while retaining the original signed transaction limit.
        let companion_estimates: Vec<serde_json::Value> = events
            .iter()
            .filter(|e| e.name == "eez.composer.admission.v1")
            .map(|e| serde_json::from_str(e.fields["admission_event"].as_str().unwrap()).unwrap())
            .filter(|e: &serde_json::Value| {
                e["kind"] == "source_estimate" && e["detail"]["direction"] == "inbound"
            })
            .collect();
        assert_eq!(companion_estimates.len(), inbound_count as usize);
        for estimate in &companion_estimates {
            assert_eq!(estimate["estimator_version"], "candidate-framing-10");
            let detail = &estimate["detail"];
            assert_eq!(detail["companion_estimate_basis"], "bounded_protocol");
            assert_eq!(detail["companion_sim_gas"], detail["source_sim_gas"]);
            let reserve = detail["companion_protocol_reserve_gas"].as_u64().unwrap();
            assert!(reserve >= 70_000);
            assert_eq!(
                detail["companion_estimate_gas"].as_u64().unwrap(),
                detail["companion_sim_gas"].as_u64().unwrap()
                    + detail["companion_target_sim_gas"].as_u64().unwrap()
                    + reserve
            );
            assert!(
                detail["companion_estimate_gas"].as_u64().unwrap()
                    < detail["companion_declared_gas"].as_u64().unwrap()
            );
        }
        if wave > 0 && inbound_count > 0 {
            let estimates: Vec<serde_json::Value> = events
                .iter()
                .filter(|e| e.name == "eez.composer.admission.v1")
                .map(|e| {
                    serde_json::from_str(e.fields["admission_event"].as_str().unwrap()).unwrap()
                })
                .filter(|e: &serde_json::Value| {
                    e["kind"] == "source_estimate" && e["detail"]["direction"] == "inbound"
                })
                .collect();
            assert_eq!(estimates.len(), inbound_count as usize);
            assert!(
                estimates
                    .iter()
                    .any(|e| e["detail"]["estimate_reasons"]["deferred_storage"]
                        .as_u64()
                        .is_some_and(|n| n > 0)),
                "retained storage pricing must reach native settlement"
            );
            for estimate in estimates {
                assert_eq!(estimate["estimator_version"], "candidate-framing-10");
                assert_eq!(estimate["detail"]["batch_discount_revoked"], false);
                assert!(estimate["detail"]["entry_reserve_gas"].as_u64().unwrap() < 370_000);
            }
        }
        let admission: Vec<serde_json::Value> = events
            .iter()
            .filter(|event| event.name == "eez.composer.admission.v1")
            .map(|event| {
                serde_json::from_str(event.fields["admission_event"].as_str().unwrap()).unwrap()
            })
            .collect();
        let summary = admission
            .iter()
            .find(|event| event["kind"] == "summary")
            .unwrap();
        let projection = admission
            .iter()
            .find(|event| event["kind"] == "full_projection")
            .unwrap();
        assert!(
            projection["detail"]["projected_gas"].as_u64().unwrap()
                <= summary["detail"]["full_estimate_gas"].as_u64().unwrap(),
            "incremental framing must cover the production batch's final sizing"
        );
        assert!(summary["detail"]["framing_reserve_gas"].as_u64().unwrap() < 600_000);
        let deliveries: Vec<serde_json::Value> = events
            .iter()
            .filter(|event| event.name == "eez.composer.admission.v1")
            .map(|event| {
                serde_json::from_str(event.fields["admission_event"].as_str().unwrap()).unwrap()
            })
            .filter(|event: &serde_json::Value| event["kind"] == "delivery")
            .collect();
        assert_eq!(deliveries.len(), 1);
        let delivery = &deliveries[0]["detail"];
        assert_eq!(delivery["acknowledged"], true);
        assert_eq!(
            delivery["transaction_count"].as_u64(),
            Some(inbound_count + 1)
        );
        assert!(delivery["ack_remaining_ms"].as_f64().unwrap() >= 0.0);
        assert!(delivery["target_block"].as_u64().unwrap() > 0);
        assert!(delivery["target_timestamp"].as_u64().unwrap() > 0);
        assert!(delivery["expected_parent_hash"].as_str().is_some());
        let dispatched: Vec<_> = events
            .iter()
            .filter(|e| e.name == "eez.composer.bundle.dispatched")
            .collect();
        assert_eq!(dispatched.len(), 1, "{}", world.settlement_diagnostics());
        assert_eq!(dispatched[0].fields["sync_height"], sync);
        let attempts: Vec<u64> =
            serde_json::from_str(dispatched[0].fields["attempt_counts"].as_str().unwrap()).unwrap();
        assert!(attempts.iter().all(|attempt| *attempt == 0));
        assert!(
            events.iter().any(|e| {
                e.name == "eez.composer.admission.prepared"
                    && e.fields["accepted"] == inbound_count + outbound_count
            }),
            "{}",
            world.settlement_diagnostics()
        );
        for forbidden in [
            "eez.composer.admission.cut",
            "eez.composer.phase2.canonical_mismatch",
            "eez.composer.phase2.final_receipt_failed",
            "eez.composer.cc_compose.poison_eviction_completed",
        ] {
            assert!(
                !events.iter().any(|e| e.name == forbidden),
                "{forbidden}{}",
                world.settlement_diagnostics()
            );
        }
    }
    world.node.assert_no_process_death();
    follower.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_fitting_thirty_request_wave_settles_in_one_sync() {
    single_sync_wave(NativeL1Profile::EthereumOsaka, 17, 13, 1, false, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_fitting_thirty_request_wave_settles_in_one_sync() {
    single_sync_wave(NativeL1Profile::GnosisPrague, 17, 13, 1, false, true).await;
}

/// Two 40-source batches each fit one Sync. The second replaces a consumed but
/// nonempty deferred queue and can use proxies established by the first; all
/// 80 sources are independently replayed by the follower.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_fitting_forty_request_waves_replace_busy_queue_and_replay() {
    single_sync_wave(NativeL1Profile::EthereumOsaka, 24, 16, 2, false, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_fitting_forty_request_waves_replace_busy_queue_and_replay() {
    single_sync_wave(NativeL1Profile::GnosisPrague, 24, 16, 2, false, true).await;
}

/// Empty return data uses short Solidity byte storage. Both 45-source waves
/// must fit, including the second post's overwrite of retained bounded slots.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_short_result_forty_five_request_waves_replace_busy_queue_and_replay() {
    single_sync_wave(NativeL1Profile::EthereumOsaka, 45, 0, 2, true, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_short_result_forty_five_request_waves_replace_busy_queue_and_replay() {
    single_sync_wave(NativeL1Profile::GnosisPrague, 45, 0, 2, true, true).await;
}

/// A source can depend on an ordinary L2 predecessor seen on the unsafe
/// branch. Excluding that branch's Sync must recover the predecessor and keep
/// the dependent source owned until its nonce becomes executable again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_admission_recovers_an_orphaned_ordinary_l2_predecessor() {
    orphaned_predecessor(false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_admission_reorg_keeps_outbound_sources_out_of_ordinary_sequencing() {
    orphaned_predecessor(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_admission_static_only_exclusion_preserves_ordinary_descendants() {
    orphaned_predecessor(false, true).await;
}

async fn orphaned_predecessor(initial_outbound: bool, static_only: bool) {
    let mut config = env();
    config.push(("RUST_LOG", "info,txpool=debug,engine::tree=debug".into()));
    let world = eez_testkit::setup_cross_chain_with_profile_and_rpc_faults(
        NativeL1Profile::GnosisPrague,
        &config,
    )
    .await
    .unwrap();
    let follower = NodeHandle::start(
        "auto-predecessor-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let initial_rpc = if initial_outbound {
        world.l2_rpc()
    } else {
        world.l1_rpc()
    };
    let initial_provider = ProviderBuilder::new().connect_http(initial_rpc.parse().unwrap());
    let mut inbound_nonce = onchain_nonce(&initial_rpc, ANVIL_KEY_2).await.unwrap();
    let mut initial_target = if initial_outbound {
        world.outbound_proxy
    } else {
        world.setter_proxy
    };
    let initial_data = if static_only {
        // Minimal forwarding fixture: copy calldata, STATICCALL the proxy,
        // require success, and return its 32-byte value. Building this tiny
        // wrapper locally keeps the test on the existing signing/deploy path.
        let mut runtime = vec![0x36, 0x5f, 0x5f, 0x37, 0x60, 0x20, 0x5f, 0x36, 0x5f, 0x73];
        runtime.extend_from_slice(initial_target.as_slice());
        let destination = runtime.len() + 3;
        runtime.extend_from_slice(&[0x5a, 0xfa, 0x60, 0, 0x57, 0x5f, 0x5f, 0xfd]);
        runtime[destination] = u8::try_from(runtime.len()).unwrap();
        runtime.extend_from_slice(&[0x5b, 0x60, 0x20, 0x5f, 0xf3]);
        let len = u8::try_from(runtime.len()).unwrap();
        let mut init = vec![0x60, len, 0x60, 10, 0x5f, 0x39, 0x60, len, 0x5f, 0xf3];
        init.extend(runtime);
        let deployed = sign_and_send(
            &initial_rpc,
            ANVIL_KEY_2,
            initial_provider.get_chain_id().await.unwrap(),
            inbound_nonce,
            None,
            U256::ZERO,
            init,
            100_000,
        )
        .await
        .unwrap();
        let receipt = wait_for(TIMEOUT, || async {
            Ok(initial_provider.get_transaction_receipt(deployed).await?)
        })
        .await
        .unwrap();
        assert!(receipt.status());
        initial_target = receipt.contract_address.unwrap();
        inbound_nonce += 1;
        IValue::valueCall {}.abi_encode()
    } else {
        IValue::setValueCall { v: U256::from(1) }.abi_encode()
    };
    let outbound_nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap();
    let data = |v| IValue::setValueCall { v: U256::from(v) }.abi_encode();
    let cursor = world.node.signal_cursor().unwrap();
    world.hold_next_bundle().unwrap();
    let inbound = sign_and_send(
        &if initial_outbound {
            world.l2_xchain()
        } else {
            world.l1_xchain()
        },
        ANVIL_KEY_2,
        initial_provider.get_chain_id().await.unwrap(),
        inbound_nonce,
        Some(initial_target),
        U256::ZERO,
        initial_data,
        400_000,
    )
    .await
    .unwrap();
    let mut held = wait_for(TIMEOUT, || async { world.held_bundle() })
        .await
        .unwrap();
    // A relay response can time out before the first ordinary descendant is
    // built. Hold canonical observations independently so this fixture really
    // creates a descendant of the unconfirmed Sync before authorizing recovery.
    world.fail_rpc_methods(&["eth_getBlockByNumber"]).unwrap();
    let dispatched = world
        .node
        .signals_since(cursor)
        .unwrap()
        .into_iter()
        .find(|e| e.name == "eez.composer.bundle.dispatched")
        .unwrap();
    let sync_hash = dispatched.fields["sync_hash"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let sync_height = dispatched.fields["sync_height"].as_u64().unwrap();
    if static_only {
        let block = wait_for(TIMEOUT, || async {
            Ok(l2.get_block_by_hash(sync_hash).await?)
        })
        .await
        .unwrap();
        assert!(
            block.transactions.is_empty(),
            "static-only source must produce an empty Sync"
        );
    }
    let predecessor = sign_and_send(
        &world.l2_rpc(),
        ANVIL_KEY_4,
        world.l2_chain_id,
        outbound_nonce,
        Some(world.value_l2),
        U256::ZERO,
        data(2u64),
        100_000,
    )
    .await
    .unwrap();
    let original = wait_for(TIMEOUT, || async {
        Ok(l2.get_transaction_receipt(predecessor).await?)
    })
    .await
    .unwrap();
    assert!(original.status());
    assert!(original.block_number.unwrap() > sync_height);
    assert_eq!(
        l2.get_block_by_number(sync_height.into())
            .await
            .unwrap()
            .unwrap()
            .header
            .hash,
        sync_hash,
        "the ordinary predecessor must descend from the still-unconfirmed Sync"
    );
    let outbound = sign_and_send(
        &world.l2_xchain(),
        ANVIL_KEY_4,
        world.l2_chain_id,
        outbound_nonce + 1,
        Some(world.outbound_proxy),
        U256::ZERO,
        data(3u64),
        400_000,
    )
    .await
    .unwrap();
    // Reject the held submission only after the ordinary predecessor has an
    // unsafe receipt. The native endpoint rejects this empty request; none of
    // the original post or its companions can enter L1 through it.
    held["params"][0]["txs"] = serde_json::json!([]);
    world.release_bundle(&held).unwrap();
    world.fail_rpc_methods(&[]).unwrap();
    let replayed = wait_for(TIMEOUT, || async {
        Ok(l2
            .get_transaction_receipt(predecessor)
            .await?
            .filter(|r| static_only || r.block_hash != original.block_hash))
    })
    .await
    .unwrap_or_else(|e| {
        panic!(
            "ordinary predecessor was not recovered: {e:#}{}",
            world.settlement_diagnostics()
        )
    });
    assert!(replayed.status());
    let source = wait_for(TIMEOUT, || async {
        Ok(l2.get_transaction_receipt(outbound).await?)
    })
    .await
    .unwrap_or_else(|e| {
        panic!(
            "dependent source was not recovered: {e:#}{}",
            world.settlement_diagnostics()
        )
    });
    assert!(source.status());
    assert!(
        wait_for(TIMEOUT, || async {
            Ok(initial_provider.get_transaction_receipt(inbound).await?)
        })
        .await
        .unwrap()
        .status()
    );
    let height = source
        .block_number
        .unwrap()
        .max(replayed.block_number.unwrap());
    wait_for_safe_prefix_convergence(&[&world.node, &follower], height, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap(),
        outbound_nonce + 2
    );
    assert_eq!(
        l2_value(&world.l1_rpc(), world.outbound_value)
            .await
            .unwrap(),
        U256::from(3)
    );
    let follower_rpc = ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap());
    for hash in [predecessor, outbound] {
        assert!(
            follower_rpc
                .get_transaction_receipt(hash)
                .await
                .unwrap()
                .unwrap()
                .status()
        );
    }
    if static_only {
        let canonical = l2
            .get_transaction_receipt(predecessor)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            canonical.block_hash, original.block_hash,
            "excluding an already empty Sync must not replace ordinary descendants"
        );
    }
    assert!(
        !world
            .node
            .signals_since(cursor)
            .unwrap()
            .iter()
            .any(|e| e.name == "eez.composer.cc_compose.poison_eviction_completed"),
        "a future nonce is not poison"
    );
    if initial_outbound {
        assert!(
            world
                .node
                .signals_since(cursor)
                .unwrap()
                .iter()
                .any(|e| e.name == "eez.node.pool.cross_chain_source_rejected"
                    && e.fields["tx_hash"] == inbound.to_string()),
            "the original outbound must remain owned during pool reinjection"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_selection_preserves_capacity_after_one_exclusion() {
    consecutive_exclusions(1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_selection_backs_off_after_five_exclusions_and_retries_every_source() {
    consecutive_exclusions(5).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_settlement_six_exclusions_keep_sources_eligible() {
    // Six landed incomplete settlements would exhaust a source; six excluded
    // target slots spend no postBatch gas and stay within their own budget.
    consecutive_exclusions(6).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_settlement_twelfth_exclusion_exhausts_sources_and_releases_nonce_suffix() {
    consecutive_exclusions(EXCLUDED_SLOT_BUDGET).await;
}

/// `eez_composer::optimistic::MAX_EXCLUDED_SLOT_ATTEMPTS`.
const EXCLUDED_SLOT_BUDGET: u32 = 12;

/// Groups submitted in the `index`th consecutive failed attempt of a wave of
/// four sources, and the selection limit that failure leaves for the next
/// one: from the fifth failure, automatic admission halves the selection.
fn backoff_shape(index: usize) -> (usize, Option<usize>) {
    match index {
        0..4 => (4, None),
        4 => (4, Some(2)),
        5 => (2, Some(1)),
        _ => (1, Some(1)),
    }
}

async fn consecutive_exclusions(failures: u32) {
    let world = setup_cross_chain_with_profile(NativeL1Profile::GnosisPrague, &env())
        .await
        .unwrap();
    let follower = NodeHandle::start(
        "auto-retry-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let chain = l1.get_chain_id().await.unwrap();
    let nonce = onchain_nonce(&world.l1_rpc(), ANVIL_KEY_2).await.unwrap();
    async fn send(
        world: &eez_testkit::CrossChainWorld,
        chain: u64,
        nonce: u64,
        value: u64,
    ) -> alloy_primitives::B256 {
        sign_and_send(
            &world.l1_xchain(),
            ANVIL_KEY_2,
            chain,
            nonce,
            Some(world.setter_proxy),
            U256::ZERO,
            IValue::setValueCall {
                v: U256::from(value),
            }
            .abi_encode(),
            400_000,
        )
        .await
        .unwrap()
    }
    let warm = send(&world, chain, nonce, 1).await;
    let receipt = wait_for(TIMEOUT, || async {
        Ok(l1.get_transaction_receipt(warm).await?)
    })
    .await
    .unwrap();
    assert!(receipt.status());
    wait_for(TIMEOUT, || async {
        Ok(
            (l2_value(&follower.l2_rpc_url(), world.value_l2).await? == U256::from(1))
                .then_some(()),
        )
    })
    .await
    .unwrap();
    // Start the bounded wave after an idle opportunity so all four sources
    // reach the queue before the next selection, independent of setup timing.
    let idle = world.node.signal_cursor().unwrap();
    wait_for(TIMEOUT, || async {
        Ok(world
            .node
            .signals_since(idle)?
            .iter()
            .any(|e| e.name == "eez.composer.settlement.decision" && e.fields["due"] == false)
            .then_some(()))
    })
    .await
    .unwrap();
    let cursor = world.node.signal_cursor().unwrap();
    world.drop_next_bundles(failures).unwrap();
    let mut hashes = Vec::new();
    for i in 1..=4 {
        hashes.push(send(&world, chain, nonce + i, i + 1).await);
    }
    if failures == EXCLUDED_SLOT_BUDGET {
        // The first source is in every attempt, so its excluded-slot budget
        // is exhausted first and its nonce gap releases the suffix.
        wait_for(TIMEOUT, || async {
            Ok(world
                .node
                .signals_since(cursor)?
                .iter()
                .any(|e| {
                    e.name == "eez.composer.admission.submission_failure"
                        && e.fields["consecutive_failures"] == failures
                        && e.fields["submitted_groups"] == 1
                })
                .then_some(()))
        })
        .await
        .unwrap();
        let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
        let queue: serde_json::Value = l2
            .client()
            .request("eez_getCrossChainQueueStatus", ())
            .await
            .unwrap();
        for field in ["queued", "inFlight", "blocked", "unknown"] {
            assert_eq!(
                queue[field], 0,
                "exhaustion must release the complete nonce suffix"
            );
        }
        for hash in &hashes {
            assert!(l1.get_transaction_receipt(*hash).await.unwrap().is_none());
        }
        assert_eq!(
            onchain_nonce(&world.l1_rpc(), ANVIL_KEY_2).await.unwrap(),
            nonce + 1
        );
        assert_eq!(
            l2_value(&follower.l2_rpc_url(), world.value_l2)
                .await
                .unwrap(),
            U256::from(1)
        );
        // Exhaustion is explicit disposition. Fresh user resubmission can
        // reacquire the same signed nonce without historical owners erasing it.
        let resubmitted = send(&world, chain, nonce + 1, 2).await;
        assert_eq!(resubmitted, hashes[0]);
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l1.get_transaction_receipt(resubmitted).await?)
        })
        .await
        .unwrap();
        assert!(receipt.status());
        wait_for(TIMEOUT, || async {
            Ok(
                (l2_value(&follower.l2_rpc_url(), world.value_l2).await? == U256::from(2))
                    .then_some(()),
            )
        })
        .await
        .unwrap();
        return;
    }
    for hash in hashes {
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l1.get_transaction_receipt(hash).await?)
        })
        .await
        .unwrap_or_else(|e| {
            panic!(
                "excluded source not recovered: {e:#}{}",
                world.settlement_diagnostics()
            )
        });
        assert!(receipt.status());
    }
    wait_for(TIMEOUT, || async {
        Ok(
            (l2_value(&follower.l2_rpc_url(), world.value_l2).await? == U256::from(5))
                .then_some(()),
        )
    })
    .await
    .unwrap();
    assert_eq!(
        onchain_nonce(&world.l1_rpc(), ANVIL_KEY_2).await.unwrap(),
        nonce + 5
    );
    let last = failures as usize - 1;
    let dropped = world.dropped_bundle().unwrap().unwrap();
    assert_eq!(
        dropped["params"][0]["txs"].as_array().unwrap().len(),
        1 + backoff_shape(last).0,
        "the poster and each submitted source of the last excluded attempt"
    );
    let events = world.node.signals_since(cursor).unwrap();
    let observed: Vec<_> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| e.name == "eez.composer.admission.submission_failure")
        .collect();
    assert_eq!(
        observed.len(),
        failures as usize,
        "{}",
        world.settlement_diagnostics()
    );
    for (index, (_, event)) in observed.iter().enumerate() {
        let (submitted, next) = backoff_shape(index);
        assert_eq!(event.fields["submitted_groups"], submitted);
        assert_eq!(event.fields["consecutive_failures"], index + 1);
        assert_eq!(event.fields["backoff_threshold"], 5);
        match next {
            None => assert!(
                event
                    .fields
                    .get("next_selection_max")
                    .is_none_or(serde_json::Value::is_null)
            ),
            Some(next) => assert_eq!(event.fields["next_selection_max"], next),
        }
    }
    let backoffs: Vec<_> = events
        .iter()
        .filter(|e| e.name == "eez.composer.admission.submission_backoff")
        .collect();
    assert_eq!(backoffs.len(), (failures as usize).saturating_sub(4));
    let backoff = observed.last().unwrap().0;
    let expected = backoff_shape(last).1.unwrap_or(4);
    let next = events[backoff + 1..]
        .iter()
        .find(|e| e.name == "eez.composer.sync_slot.drain")
        .unwrap();
    assert_eq!(
        next.fields["selection_limit"], expected,
        "only the fifth confirmed failure may reduce runtime selection"
    );
    let prepared = events[backoff + 1..]
        .iter()
        .find(|e| e.name == "eez.composer.admission.prepared")
        .unwrap();
    assert_eq!(
        prepared.fields["accepted"], expected,
        "the composer must enforce the reduced limit on complete groups"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_selection_survives_a_canonically_skipped_slot_without_shrinking() {
    next_slot_replacement(false, 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_original_settlement_replaces_the_timeout_branch_without_losing_users() {
    next_slot_replacement(true, 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn consecutive_skipped_slots_keep_the_full_group_and_reach_the_next_block() {
    next_slot_replacement(false, 2).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_original_after_multiple_replacements_retains_users_and_follower_parity() {
    next_slot_replacement(true, 4).await;
}

async fn next_slot_replacement(delayed_original: bool, skipped_slots: u64) {
    // Leave time for the relay-capture poll and Engine API control handshake
    // before the targeted slot. The controller rejects requests for past slots.
    let mut config = env();
    config.extend([
        ("EEZ_PREPARATION_TIME_MS", "500".into()),
        ("EEZ_PROOF_TIME_MS", "1000".into()),
        ("EEZ_SUBMISSION_SLACK_MS", "1500".into()),
    ]);
    let mut world = setup_cross_chain_with_l1_reorg(&config).await.unwrap();
    let follower = NodeHandle::start(
        "skipped-slot-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap();
    let send = |offset: u64| {
        let rpc = world.l2_xchain();
        let chain = world.l2_chain_id;
        let target = world.outbound_proxy;
        async move {
            sign_and_send(
                &rpc,
                ANVIL_KEY_4,
                chain,
                nonce + offset,
                Some(target),
                U256::ZERO,
                IValue::setValueCall {
                    v: U256::from(offset + 1),
                }
                .abi_encode(),
                400_000,
            )
            .await
            .unwrap()
        }
    };
    let warm = send(0).await;
    let warm_receipt = wait_for(TIMEOUT, || async {
        let owner: serde_json::Value = l2
            .client()
            .request("eez_getCrossChainTransaction", (warm,))
            .await?;
        if owner["ownership"]["reason"] != "Committed" {
            return Ok(None);
        }
        Ok(l2.get_transaction_receipt(warm).await?)
    })
    .await
    .unwrap();
    assert!(warm_receipt.status());
    wait_for_safe_prefix_convergence(
        &[&world.node, &follower],
        warm_receipt.block_number.unwrap(),
        TIMEOUT,
    )
    .await
    .unwrap();
    let idle = world.node.signal_cursor().unwrap();
    wait_for(TIMEOUT, || async {
        Ok(world
            .node
            .signals_since(idle)?
            .iter()
            .any(|e| e.name == "eez.composer.settlement.decision" && e.fields["due"] == false)
            .then_some(()))
    })
    .await
    .unwrap();
    let cursor = world.node.signal_cursor().unwrap();
    if delayed_original {
        world.hold_next_bundle().unwrap();
    } else {
        // This fixture forwards raw transactions into the EL pool, which does
        // not enforce relay timestamp pins. Drop every deliberately skipped
        // target so an older replacement cannot land in the final test slot.
        world
            .drop_next_bundles(u32::try_from(skipped_slots).unwrap())
            .unwrap();
    }
    let mut hashes = Vec::new();
    for offset in 1..=4 {
        hashes.push(send(offset).await);
    }
    let dropped = wait_for(TIMEOUT, || async {
        if delayed_original {
            world.held_bundle()
        } else {
            world.dropped_bundle()
        }
    })
    .await
    .unwrap();
    let target = &dropped["params"][0];
    let target_height = u64::from_str_radix(
        target["blockNumber"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let target_timestamp = target["minTimestamp"].as_u64().unwrap();
    assert_eq!(target["maxTimestamp"], target_timestamp);
    if delayed_original {
        world
            .l1_reorg_control
            .as_mut()
            .unwrap()
            .delay_slot(target_timestamp)
            .await
            .unwrap();
        world.release_bundle(&dropped).unwrap();
    } else {
        for slot in 0..skipped_slots {
            world
                .l1_reorg_control
                .as_mut()
                .unwrap()
                .skip_slot(target_timestamp + 5 * slot)
                .await
                .unwrap();
        }
    }
    let original_receipt = wait_for(TIMEOUT, || async {
        Ok(l2.get_transaction_receipt(hashes[0]).await?)
    })
    .await
    .unwrap();
    let ordinary_nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_2).await.unwrap();
    let ordinary_recipient = alloy_primitives::Address::repeat_byte(0xab);
    let ordinary = sign_and_send(
        &world.l2_rpc(),
        ANVIL_KEY_2,
        world.l2_chain_id,
        ordinary_nonce,
        Some(ordinary_recipient),
        U256::from(1),
        vec![],
        21_000,
    )
    .await
    .unwrap();
    wait_for(Duration::from_secs(5 * skipped_slots + 7), || async {
        let events = world.node.signals_since(cursor)?;
        Ok(events.into_iter().find(|event| {
            event.name == "eez.submitter.bundle.dispatch"
                && event.fields["target_block"] == target_height
                && event.fields["target_timestamp"] == target_timestamp + 5 * skipped_slots
        }))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "no next-slot replacement dispatch: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    if delayed_original {
        let speculative_ordinary = l2.get_transaction_receipt(ordinary).await.unwrap().unwrap();
        assert!(
            speculative_ordinary.block_number.unwrap() > original_receipt.block_number.unwrap()
        );
        world
            .l1_reorg_control
            .as_mut()
            .unwrap()
            .release_delayed_slot()
            .await
            .unwrap();
    }
    let landed = wait_for(TIMEOUT, || async {
        Ok(l1.get_block_by_number(target_height.into()).await?)
    })
    .await
    .unwrap();
    let parent = l1
        .get_block_by_number((target_height - 1).into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(landed.header.parent_hash, parent.header.hash);
    assert!(parent.header.timestamp < target_timestamp);
    assert_eq!(
        landed.header.timestamp,
        if delayed_original {
            target_timestamp
        } else {
            target_timestamp + 5 * skipped_slots
        }
    );
    let mut settled_block = None;
    for hash in &hashes {
        let receipt = wait_for(TIMEOUT, || async {
            let owner: serde_json::Value = l2
                .client()
                .request("eez_getCrossChainTransaction", (*hash,))
                .await?;
            if owner["ownership"]["reason"] != "Committed" {
                return Ok(None);
            }
            Ok(l2.get_transaction_receipt(*hash).await?)
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "skipped source not recovered: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
        assert!(receipt.status());
        if *hash == hashes[0] {
            if delayed_original {
                assert_eq!(receipt.block_hash, original_receipt.block_hash);
            } else {
                assert_ne!(receipt.block_hash, original_receipt.block_hash);
                assert_eq!(
                    receipt.block_number.unwrap(),
                    original_receipt.block_number.unwrap() + 5 * skipped_slots
                );
            }
        }
        let owner: serde_json::Value = l2
            .client()
            .request("eez_getCrossChainTransaction", (*hash,))
            .await
            .unwrap();
        // Ownership records the reconciliation snapshot, which can be newer
        // than inclusion when the fixture releases a delayed head and catches up.
        let observed = owner["ownership"]["canonical"]["number"].as_u64().unwrap();
        assert!(observed >= target_height);
        let anchor = l1
            .get_block_by_number(observed.into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            owner["ownership"]["canonical"]["hash"],
            serde_json::json!(anchor.header.hash)
        );
        let settlement: serde_json::Value = l2
            .client()
            .request("eez_getSettlementByL2Block", (receipt.block_hash.unwrap(),))
            .await
            .unwrap();
        assert_eq!(settlement["canonicalL2"], true);
        assert_eq!(settlement["l1BlockNumber"], format!("{target_height:#x}"));
        assert_eq!(
            settlement["l1BlockHash"],
            serde_json::json!(landed.header.hash)
        );
        let block = receipt.block_number.unwrap();
        assert_eq!(
            *settled_block.get_or_insert(block),
            block,
            "a skipped slot must not fragment a previously fitting source group"
        );
    }
    let events = world.node.signals_since(cursor).unwrap();
    let replacement = events
        .iter()
        .position(|event| {
            event.name == "eez.composer.settlement.replacement"
                && event.fields["original_timestamp"] == target_timestamp
        })
        .unwrap();
    assert!(
        !events
            .iter()
            .any(|event| event.name == "eez.composer.admission.submission_backoff")
    );
    let next = events[replacement + 1..]
        .iter()
        .find(|event| event.name == "eez.composer.sync_slot.drain")
        .unwrap();
    assert_eq!(next.fields["selection_limit"], 4);
    assert_eq!(
        onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap(),
        nonce + 5
    );
    assert_eq!(
        l2_value(&world.l1_rpc(), world.outbound_value)
            .await
            .unwrap(),
        U256::from(5)
    );
    let ordinary_receipt = wait_for(TIMEOUT, || async {
        Ok(l2.get_transaction_receipt(ordinary).await?)
    })
    .await
    .unwrap();
    assert!(ordinary_receipt.status());
    assert_eq!(
        onchain_nonce(&world.l2_rpc(), ANVIL_KEY_2).await.unwrap(),
        ordinary_nonce + 1
    );
    assert_eq!(
        l2.get_balance(ordinary_recipient).await.unwrap(),
        U256::from(1)
    );
    wait_for_safe_prefix_convergence(
        &[&world.node, &follower],
        settled_block
            .unwrap()
            .max(ordinary_receipt.block_number.unwrap()),
        TIMEOUT,
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn historical_blob_prefix_preserves_the_skipped_slot_replacement_target() {
    historical_prefix_replacement(None).await;
}

/// Sync blocks carry the configured coinbase. A deadline replacement must not
/// rebuild an already empty historical boundary with a different header and
/// unwind every descendant through a branch switch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn historical_prefix_replacement_keeps_the_empty_boundary_with_a_configured_coinbase() {
    historical_prefix_replacement(Some(alloy_primitives::Address::repeat_byte(0x3e))).await;
}

async fn historical_prefix_replacement(fee_recipient: Option<alloy_primitives::Address>) {
    let mut config = env();
    config.extend([
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "60000".into()),
        ("EEZ_MAX_BLOBS_PER_BATCH", "1".into()),
        ("EEZ_PREPARATION_TIME_MS", "500".into()),
        ("EEZ_PROOF_TIME_MS", "1000".into()),
        ("EEZ_SUBMISSION_SLACK_MS", "1500".into()),
    ]);
    if let Some(fee_recipient) = fee_recipient {
        config.push(("EEZ_L2_FEE_RECIPIENT", format!("{fee_recipient:#x}")));
    }
    let mut world = setup_cross_chain_with_l1_reorg(&config).await.unwrap();
    let follower = NodeHandle::start(
        "historical-prefix-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_2).await.unwrap();
    let recipient = alloy_primitives::Address::repeat_byte(0x95);
    let cursor = world.node.signal_cursor().unwrap();
    world.drop_next_bundle().unwrap();
    let mut hashes = Vec::new();
    // Accumulate more than one blob before the interval trigger. Separate the
    // transactions by empty Sync boundaries so a historical prefix fits alone.
    for offset in 0..3 {
        let hash = sign_and_send(
            &world.l2_rpc(),
            ANVIL_KEY_2,
            world.l2_chain_id,
            nonce + offset,
            Some(recipient),
            U256::from(1),
            vec![0xff; 50_000],
            4_000_000,
        )
        .await
        .unwrap();
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l2.get_transaction_receipt(hash).await?)
        })
        .await
        .unwrap();
        assert!(receipt.status());
        let boundary = receipt.block_number.unwrap() + 6;
        wait_for(TIMEOUT, || async {
            Ok((l2.get_block_number().await? >= boundary).then_some(()))
        })
        .await
        .unwrap();
        hashes.push(hash);
    }
    let dropped = wait_for(TIMEOUT, || async { world.dropped_bundle() })
        .await
        .unwrap();
    let target = &dropped["params"][0];
    let target_height = u64::from_str_radix(
        target["blockNumber"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let target_timestamp = target["minTimestamp"].as_u64().unwrap();
    let boundary = world
        .node
        .signals_since(cursor)
        .unwrap()
        .iter()
        .rev()
        .find(|event| {
            event.name == "eez.composer.emission.historical_chunk"
                && event.fields["split_reasons"]
                    .as_str()
                    .is_some_and(|s| s.contains("blob_limit"))
        })
        .and_then(|event| event.fields["boundary"].as_u64())
        .expect("fixture must reach the historical blob-prefix path");
    let boundary_block = l2
        .get_block_by_number(boundary.into())
        .await
        .unwrap()
        .expect("historical boundary block");
    if let Some(fee_recipient) = fee_recipient {
        assert_eq!(
            boundary_block.header.beneficiary, fee_recipient,
            "fixture must exercise a boundary carrying the configured coinbase"
        );
    }
    world
        .l1_reorg_control
        .as_mut()
        .unwrap()
        .skip_slot(target_timestamp)
        .await
        .unwrap();
    wait_for(Duration::from_secs(12), || async {
        Ok(world.node.signals_since(cursor)?.into_iter().find(|event| {
            event.name == "eez.submitter.bundle.dispatch"
                && event.fields["target_block"] == target_height
                && event.fields["target_timestamp"] == target_timestamp + 5
        }))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "historical prefix lost its next-slot target: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    // The replacement re-proves the unchanged prefix at the next slot. The
    // boundary it ends at, and therefore every descendant, stays canonical.
    assert_eq!(
        l2.get_block_by_number(boundary.into())
            .await
            .unwrap()
            .map(|block| block.header.hash),
        Some(boundary_block.header.hash),
        "deadline replacement rewrote the historical boundary{}",
        world.settlement_diagnostics()
    );
    let mut covered = 0;
    for hash in hashes {
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l2.get_transaction_receipt(hash).await?)
        })
        .await
        .unwrap();
        assert!(receipt.status());
        covered = covered.max(receipt.block_number.unwrap());
    }
    wait_for_safe_prefix_convergence(&[&world.node, &follower], covered, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        onchain_nonce(&world.l2_rpc(), ANVIL_KEY_2).await.unwrap(),
        nonce + 3
    );
    assert_eq!(l2.get_balance(recipient).await.unwrap(), U256::from(3));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_blob_cut_keeps_complete_nonce_ordered_groups_for_later_posts() {
    let mut config = env();
    config.push(("EEZ_MAX_BLOBS_PER_BATCH", "1".into()));
    let world = setup_cross_chain_with_profile(NativeL1Profile::GnosisPrague, &config)
        .await
        .unwrap();
    let follower = NodeHandle::start(
        "auto-blob-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap();
    let cursor = world.node.signal_cursor().unwrap();
    let mut hashes = Vec::new();
    for i in 0..6u64 {
        let mut data = IValue::setValueCall {
            v: U256::from(i + 1),
        }
        .abi_encode();
        // Valid ABI calls may have trailing bytes. Each group fits alone, but
        // two of these full semantic/derivation encodings exceed one blob.
        data.resize(22_000, 0);
        hashes.push(
            sign_and_send(
                &world.l2_xchain(),
                ANVIL_KEY_4,
                world.l2_chain_id,
                nonce + i,
                Some(world.outbound_proxy),
                U256::ZERO,
                data,
                600_000,
            )
            .await
            .unwrap(),
        );
    }
    let mut covered = 0;
    for hash in hashes {
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l2.get_transaction_receipt(hash).await?)
        })
        .await
        .unwrap_or_else(|e| {
            panic!(
                "blob deferred source {hash}: {e:#}{}",
                world.settlement_diagnostics()
            )
        });
        assert!(receipt.status());
        covered = covered.max(receipt.block_number.unwrap());
    }
    wait_for_safe_prefix_convergence(&[&world.node, &follower], covered, TIMEOUT)
        .await
        .unwrap();
    let events = world.node.signals_since(cursor).unwrap();
    assert!(
        events.iter().any(|e| e.name == "eez.composer.admission.cut"
            && e.fields["constraint"] == "composition_blob_limit"),
        "expected actual blob admission cut{}",
        world.settlement_diagnostics()
    );
    assert!(
        !events
            .iter()
            .any(|e| e.name == "eez.composer.settlement.group_capacity"),
        "incremental admission should avoid recomposing an oversized terminal{}",
        world.settlement_diagnostics()
    );
    assert_eq!(
        l2_value(&world.l1_rpc(), world.outbound_value)
            .await
            .unwrap(),
        U256::from(6)
    );
    for e in events
        .iter()
        .filter(|e| e.name == "eez.composer.bundle.dispatched")
    {
        let attempts: Vec<u64> =
            serde_json::from_str(e.fields["attempt_counts"].as_str().unwrap()).unwrap();
        assert!(attempts.iter().all(|n| *n == 0));
    }
}

/// No calibration post is sent. Each cold process must settle the complete
/// fitting wave in its first batch, then repeat with retained queue storage.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_cold_and_warm_fitting_waves_have_no_calibration_post() {
    single_sync_wave(NativeL1Profile::GnosisPrague, 17, 13, 2, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_cold_and_warm_fitting_waves_have_no_calibration_post() {
    single_sync_wave(NativeL1Profile::EthereumOsaka, 17, 13, 2, false, false).await;
}
