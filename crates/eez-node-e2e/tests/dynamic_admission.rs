//! Automatic admission against native L1 execution, real DA/ECDSA and an
//! independent follower. Counts describe workloads, not production limits.

use alloy_primitives::U256;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::SolCall;
use eez_testkit::{
    ANVIL_KEY_2, ANVIL_KEY_4, IValue, NativeL1Profile, NodeBinary, NodeConfig, NodeHandle,
    l2_value, onchain_nonce, setup_cross_chain_with_profile, sign_and_send, wait_for,
    wait_for_safe_prefix_convergence,
};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(240);

fn env() -> Vec<(&'static str, String)> {
    [
        ("EEZ_SETTLEMENT_POLICY_ENABLED", "true"),
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

async fn mixed_wave(profile: NativeL1Profile) {
    let world = setup_cross_chain_with_profile(profile, &env())
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
    mixed_wave(NativeL1Profile::EthereumOsaka).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_automatic_mixed_wave_exceeds_twelve_and_replays() {
    mixed_wave(NativeL1Profile::GnosisPrague).await;
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
    assert_eq!(dropped["params"][0]["txs"].as_array().unwrap().len(), 3);
    let events = world.node.signals_since(cursor).unwrap();
    let backoff = events
        .iter()
        .position(|e| {
            e.name == "eez.composer.admission.submission_backoff"
                && e.fields["submitted_groups"] == 2
                && e.fields["next_selection_max"] == 1
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
        next.fields["selection_limit"], 1,
        "backoff must affect actual runtime selection"
    );
    let prepared = events[backoff + 1..]
        .iter()
        .find(|e| e.name == "eez.composer.admission.prepared")
        .unwrap();
    assert_eq!(
        prepared.fields["accepted"], 1,
        "the composer must enforce the reduced limit on complete groups"
    );
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
