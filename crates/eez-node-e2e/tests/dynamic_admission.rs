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

const TIMEOUT: Duration = Duration::from_secs(240);

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
/// execution fits. This exercises companion admission and retained queue cleanup.
async fn single_sync_wave(
    profile: NativeL1Profile,
    inbound_count: u64,
    outbound_count: u64,
    waves: u64,
    empty_inbound_return_data: bool,
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
    let registry = IDeferredQueue::new(world.cfg.eez_address, &l1);
    let outbound_nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap();
    let mut previous_sync = None;
    let mut previous_l1_block = None;
    for wave in 0..waves {
        // Every prior queue is fully consumed but remains in storage. The
        // following post must pay its deletion cost, including after a busy
        // predecessor; cursor == length does not mean there is no work.
        let previous_entries = if wave == 0 { 1 } else { inbound_count };
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
                        nonce + 1 + wave * inbound_count + i,
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
            nonce + 1 + (wave + 1) * inbound_count
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
    single_sync_wave(NativeL1Profile::EthereumOsaka, 17, 13, 1, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_fitting_thirty_request_wave_settles_in_one_sync() {
    single_sync_wave(NativeL1Profile::GnosisPrague, 17, 13, 1, false).await;
}

/// Two 40-source batches each fit one Sync. The second replaces a consumed but
/// nonempty deferred queue and can use proxies established by the first; all
/// 80 sources are independently replayed by the follower.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_fitting_forty_request_waves_replace_busy_queue_and_replay() {
    single_sync_wave(NativeL1Profile::EthereumOsaka, 24, 16, 2, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_fitting_forty_request_waves_replace_busy_queue_and_replay() {
    single_sync_wave(NativeL1Profile::GnosisPrague, 24, 16, 2, false).await;
}

/// Empty return data uses short Solidity byte storage. Both 45-source waves
/// must fit, including the second post's cleanup of all 45 consumed entries.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_short_result_forty_five_request_waves_replace_busy_queue_and_replay() {
    single_sync_wave(NativeL1Profile::EthereumOsaka, 45, 0, 2, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_short_result_forty_five_request_waves_replace_busy_queue_and_replay() {
    single_sync_wave(NativeL1Profile::GnosisPrague, 45, 0, 2, true).await;
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
    let world = setup_cross_chain_with_profile(NativeL1Profile::GnosisPrague, &config)
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
    if static_only {
        let dispatched = world
            .node
            .signals_since(cursor)
            .unwrap()
            .into_iter()
            .find(|e| e.name == "eez.composer.bundle.dispatched")
            .unwrap();
        let hash = dispatched.fields["sync_hash"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let block = wait_for(TIMEOUT, || async { Ok(l2.get_block_by_hash(hash).await?) })
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
async fn automatic_selection_shrinks_after_an_excluded_bundle_and_retries_every_source() {
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
    world.drop_next_bundle().unwrap();
    let mut hashes = Vec::new();
    for i in 1..=4 {
        hashes.push(send(&world, chain, nonce + i, i + 1).await);
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
    let dropped = world.dropped_bundle().unwrap().unwrap();
    assert_eq!(
        dropped["params"][0]["txs"].as_array().unwrap().len(),
        5,
        "the learned one-blob allowance must admit all four queued sources"
    );
    let events = world.node.signals_since(cursor).unwrap();
    let backoff = events
        .iter()
        .position(|e| {
            e.name == "eez.composer.admission.submission_backoff"
                && e.fields["submitted_groups"] == 4
                && e.fields["next_selection_max"] == 2
        })
        .unwrap_or_else(|| {
            panic!(
                "expected measured submission backoff{}",
                world.settlement_diagnostics()
            )
        });
    let next = events[backoff + 1..]
        .iter()
        .find(|e| e.name == "eez.composer.sync_slot.drain")
        .unwrap();
    assert_eq!(
        next.fields["selection_limit"], 2,
        "backoff must affect actual runtime selection"
    );
    let prepared = events[backoff + 1..]
        .iter()
        .find(|e| e.name == "eez.composer.admission.prepared")
        .unwrap();
    assert_eq!(
        prepared.fields["accepted"], 2,
        "the composer must enforce the reduced limit on complete groups"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn automatic_selection_survives_a_canonically_skipped_slot_without_shrinking() {
    // Leave time for the relay-capture poll and Engine API control handshake
    // before the targeted slot. The controller rejects requests for past slots.
    let mut config = env();
    config.extend([
        ("EEZ_PREPARATION_TIME_MS", "500".into()),
        ("EEZ_PROOF_TIME_MS", "500".into()),
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
    world.drop_next_bundle().unwrap();
    let mut hashes = Vec::new();
    for offset in 1..=4 {
        hashes.push(send(offset).await);
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
    assert_eq!(target["maxTimestamp"], target_timestamp);
    world
        .l1_reorg_control
        .as_mut()
        .unwrap()
        .skip_slot(target_timestamp)
        .await
        .unwrap();
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
    assert!(landed.header.timestamp > target_timestamp);
    let mut settled_block = None;
    for hash in hashes {
        let receipt = wait_for(TIMEOUT, || async {
            let owner: serde_json::Value = l2
                .client()
                .request("eez_getCrossChainTransaction", (hash,))
                .await?;
            if owner["ownership"]["reason"] != "Committed" {
                return Ok(None);
            }
            Ok(l2.get_transaction_receipt(hash).await?)
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "skipped source not recovered: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
        assert!(receipt.status());
        let block = receipt.block_number.unwrap();
        assert_eq!(
            *settled_block.get_or_insert(block),
            block,
            "a skipped slot must not fragment a previously fitting source group"
        );
    }
    let events = world.node.signals_since(cursor).unwrap();
    let skipped = events
        .iter()
        .position(|e| {
            e.name == "eez.composer.admission.skipped_slot"
                && e.fields["submitted_groups"] == 4
                && e.fields["target"] == format!("Some(({target_height}, {target_timestamp}))")
        })
        .unwrap_or_else(|| {
            panic!(
                "expected canonical skipped-slot decision{}",
                world.settlement_diagnostics()
            )
        });
    assert!(
        !events
            .iter()
            .any(|e| e.name == "eez.composer.admission.submission_backoff")
    );
    let next = events[skipped + 1..]
        .iter()
        .find(|e| e.name == "eez.composer.sync_slot.drain")
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
    wait_for_safe_prefix_convergence(&[&world.node, &follower], settled_block.unwrap(), TIMEOUT)
        .await
        .unwrap();
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
