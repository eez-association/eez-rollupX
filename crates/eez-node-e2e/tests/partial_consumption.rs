//! Valid native-blob partial settlement after proving. The test relay omits
//! one source user; later dependent users execute and revert on real EEZ.
//! Builder permission/omission semantics are tested separately on rbuilder.

use std::time::Duration;

use alloy_consensus::{
    EthereumTxEnvelope, SignableTransaction, Transaction, TxEip1559, TxEip4844WithSidecar,
};
use alloy_eips::{BlockNumberOrTag, Decodable2718, Encodable2718};
use alloy_network::TxSignerSync;
use alloy_primitives::{B256, Bytes, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::SolCall;
use eez_testkit::{
    ANVIL_KEY_1, ANVIL_KEY_2, ANVIL_KEY_4, DEV_CHAIN_ID, IValue, NodeBinary, NodeConfig,
    NodeHandle, TARGET_DEPLOYER, block_number_and_hash_at, create_l2_cross_chain_proxy,
    deploy_value, onchain_nonce, receipt_ok, setup_cross_chain_with_env, sign_and_send, signals,
    wait_for,
};

const TIMEOUT: Duration = Duration::from_mins(2);

async fn attempt_event(
    world: &eez_testkit::CrossChainWorld,
    name: &str,
    post_hash: B256,
) -> signals::NodeSignal {
    wait_for(TIMEOUT, || async {
        Ok(world.node.signals_since(0)?.into_iter().find(|event| {
            event.name == name && event.fields["post_batch_hash"] == serde_json::json!(post_hash)
        }))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "missing {name} for {post_hash}: {error:#}{}",
            world.settlement_diagnostics()
        )
    })
}

fn policy_env() -> Vec<(&'static str, String)> {
    vec![
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10000".into()),
        ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
        ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "100000000000".into()),
        (
            "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS",
            "100000000000".into(),
        ),
        // Native test miner ordering only; atomic builder ordering is exercised
        // by the separate Kurtosis fixture.
        ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "1000000001".into()),
        // Distinct from the base p90 / 100 gwei so a quote proves its profile.
        (
            "EEZ_L1_POSTBATCH_CROSS_CHAIN_PRIORITY_PERCENTILE",
            CROSS_CHAIN_PERCENTILE.to_string(),
        ),
        (
            "EEZ_L1_POSTBATCH_CROSS_CHAIN_MAX_PRIORITY_FEE",
            CROSS_CHAIN_MAX_PRIORITY_FEE.to_string(),
        ),
    ]
}

const CROSS_CHAIN_PERCENTILE: u64 = 77;
const CROSS_CHAIN_MAX_PRIORITY_FEE: u64 = 2_000_000_000;

/// The final poster was priced with the cross-chain profile, and any bundle
/// without cross-chain calls kept the base settings. `settlement_policy_cost`
/// checks base-only posts against a deliberately tiny cross-chain ceiling.
fn assert_priority_profiles(world: &eez_testkit::CrossChainWorld, priority_fee: u128) {
    let quotes = world
        .node
        .signals_since(0)
        .unwrap()
        .into_iter()
        .filter(|event| event.name == "eez.composer.postbatch.fee_quote")
        .collect::<Vec<_>>();
    let final_quote = quotes
        .iter()
        .find(|quote| {
            quote.fields["priority_profile"] == "CrossChain"
                && quote.u64("priority_fee").ok().map(u128::from) == Some(priority_fee)
        })
        .expect("the three-user poster must be priced with the cross-chain profile");
    assert_eq!(
        final_quote.u64("priority_reward_percentile").unwrap(),
        CROSS_CHAIN_PERCENTILE
    );
    assert_eq!(
        final_quote.u64("priority_max").unwrap(),
        CROSS_CHAIN_MAX_PRIORITY_FEE
    );
    let estimate = final_quote.u64("estimated_priority_fee").unwrap();
    assert_eq!(
        u128::from(estimate.clamp(1_000_000_001, CROSS_CHAIN_MAX_PRIORITY_FEE)),
        priority_fee
    );
    for quote in quotes
        .iter()
        .filter(|quote| quote.fields["priority_profile"] == "Base")
    {
        assert_eq!(quote.u64("priority_reward_percentile").unwrap(), 90);
        assert_eq!(quote.u64("priority_max").unwrap(), 100_000_000_000);
    }
}

async fn partial_after_proving(omitted: usize, restart_before_retry: bool) {
    let env = policy_env();
    let mut world = setup_cross_chain_with_env(&env).await.unwrap();
    world
        .omit_next_bundle_user(omitted, restart_before_retry)
        .unwrap();
    // Start the wave only after a completed empty drain, leaving the full
    // next slot for all three users. The relay additionally requires exactly
    // three proved users, so an accidental split cannot satisfy this test.
    let cursor = world.node.signal_cursor().unwrap();
    wait_for(TIMEOUT, || async {
        Ok(world
            .node
            .signals_since(cursor)?
            .iter()
            .any(|event| {
                event.name == "eez.composer.settlement.decision" && event.fields["due"] == false
            })
            .then_some(()))
    })
    .await
    .unwrap();
    let keys = [ANVIL_KEY_2, ANVIL_KEY_4, ANVIL_KEY_1];
    let mut hashes = Vec::new();
    let mut nonces = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        let nonce = onchain_nonce(&world.l1_rpc(), key).await.unwrap();
        nonces.push(nonce);
        hashes.push(
            sign_and_send(
                &world.l1_xchain(),
                key,
                DEV_CHAIN_ID,
                nonce,
                Some(world.setter_proxy),
                U256::ZERO,
                IValue::setValueCall {
                    v: U256::from(index + 1),
                }
                .abi_encode(),
                600_000,
            )
            .await
            .unwrap(),
        );
    }
    let fault = wait_for(TIMEOUT, || async { world.bundle_omission() })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "no fully proved three-user bundle: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
    let request = &fault["original"]["params"][0];
    assert_eq!(request["revertingTxHashes"], serde_json::json!(hashes));
    assert!(request.get("droppingTxHashes").is_none());
    assert_eq!(request["minTimestamp"], request["maxTimestamp"]);
    let raw: Bytes = request["txs"][0].as_str().unwrap().parse().unwrap();
    let post = EthereumTxEnvelope::<TxEip4844WithSidecar>::decode_2718(&mut raw.as_ref()).unwrap();
    let post_hash = *post.tx_hash();
    assert_priority_profiles(&world, post.max_priority_fee_per_gas().unwrap());
    let call = eez_protocol::abi::postAndVerifyBatchCall::abi_decode(post.input()).unwrap();
    let proposed_roots = call
        .batch
        .entries
        .iter()
        .flat_map(|entry| &entry.rootUpdates)
        .filter(|update| update.rollupId == world.cfg.rollup_id)
        .map(|update| update.newRoot)
        .collect::<Vec<_>>();
    assert_eq!(
        proposed_roots.len(),
        4,
        "anchor and all three users were proved"
    );
    // Final fee/gas selection re-signs only the poster. Match the immutable
    // user wave and root; receipt/reconciliation checks below use its final hash.
    let dispatched = wait_for(TIMEOUT, || async {
        Ok(world.node.signals_since(0)?.into_iter().find(|event| {
            event.name == signals::COMPOSER_BUNDLE_DISPATCHED
                && event.fields["user_hashes"] == format!("{hashes:?}")
        }))
    })
    .await
    .unwrap();
    let height = dispatched.u64("sync_height").unwrap();
    let full_hash = dispatched.b256("sync_hash").unwrap();
    assert_eq!(full_hash, *proposed_roots.last().unwrap());
    assert_eq!(dispatched.fields["user_hashes"], format!("{hashes:?}"));
    assert_eq!(dispatched.fields["attempt_counts"], "[0, 0, 0]");

    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let posted = wait_for(TIMEOUT, || async {
        Ok(l1.get_transaction_receipt(post_hash).await?)
    })
    .await
    .unwrap();
    assert!(
        posted.status(),
        "the required, blob-backed postBatch must succeed"
    );
    let inclusion_block = posted.block_number.unwrap();
    let registry_roots = |receipt: &alloy_rpc_types_eth::TransactionReceipt| {
        receipt
            .inner
            .logs()
            .iter()
            .filter(|log| log.address() == world.cfg.eez_address)
            .filter_map(|log| {
                log.log_decode::<eez_protocol::abi::L2ExecutionPerformed>()
                    .ok()
            })
            .filter(|event| event.inner.data.rollupId == world.cfg.rollup_id)
            .map(|event| event.inner.data.newRoot)
            .collect::<Vec<_>>()
    };
    let expected_prefix = proposed_roots[omitted];
    if omitted == 0 {
        assert_eq!(registry_roots(&posted).last(), Some(&expected_prefix));
    }
    // Before retrying, users before the omission completed; users after it
    // reverted because their expected prefix is absent. Reverted source logs
    // and nonces must never be mistaken for retryable, dropped users.
    for (index, hash) in hashes
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != omitted)
    {
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l1.get_transaction_receipt(*hash).await?)
        })
        .await
        .unwrap();
        assert_eq!(receipt.block_number, Some(inclusion_block));
        assert_eq!(receipt.status(), index < omitted);
        if index + 1 == omitted {
            assert_eq!(registry_roots(&receipt).last(), Some(&expected_prefix));
        }
    }
    let reconciled =
        attempt_event(&world, signals::COMPOSER_SETTLEMENT_RECONCILED, post_hash).await;
    let expected = (0..3)
        .map(|index| match index.cmp(&omitted) {
            std::cmp::Ordering::Less => "Terminal(Committed)".to_owned(),
            std::cmp::Ordering::Equal => {
                format!("Retryable {{ source_nonce: {} }}", nonces[index])
            }
            std::cmp::Ordering::Greater => "Terminal(SourceReverted)".to_owned(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    assert_eq!(reconciled.fields["outcomes"], format!("[{expected}]"));
    assert_eq!(reconciled.fields["charge_attempt"], true);
    let actual = attempt_event(&world, "eez.composer.postbatch.actual_cost", post_hash).await;
    let post_cost = U256::from(posted.gas_used) * U256::from(posted.effective_gas_price)
        + U256::from(posted.blob_gas_used.unwrap()) * U256::from(posted.blob_gas_price.unwrap());
    let mut companion_cost = U256::ZERO;
    for (index, hash) in hashes.iter().enumerate() {
        if index != omitted {
            let receipt = l1.get_transaction_receipt(*hash).await.unwrap().unwrap();
            companion_cost +=
                U256::from(receipt.gas_used) * U256::from(receipt.effective_gas_price);
        }
    }
    assert_eq!(actual.fields["post_total_cost"], post_cost.to_string());
    assert_eq!(
        actual.fields["companion_total_cost"],
        companion_cost.to_string()
    );
    assert_eq!(
        actual.fields["bundle_total_cost"],
        (post_cost + companion_cost).to_string()
    );
    assert!(
        companion_cost > U256::ZERO,
        "included source reverts still incur fees"
    );
    let mut retry = wait_for(TIMEOUT, || async {
        Ok(world.node.signals_since(cursor)?.into_iter().find(|event| {
            event.name == signals::COMPOSER_BUNDLE_DISPATCHED
                && event.fields["user_hashes"] == format!("{:?}", [hashes[omitted]])
        }))
    })
    .await
    .unwrap();
    assert_eq!(retry.fields["attempt_counts"], "[1]");
    if restart_before_retry {
        let held = wait_for(TIMEOUT, || async { world.held_bundle() })
            .await
            .unwrap();
        // The hold is one-shot. Keep later allowed replacements out of L1
        // while the original partial prefix crosses the persistence boundary.
        world.fail_rpc_methods(&["eth_sendBundle"]).unwrap();
        assert_eq!(held["params"][0]["txs"].as_array().unwrap().len(), 2);
        assert_eq!(held["params"][0]["txs"][1], request["txs"][omitted + 1]);
        let target = u64::from_str_radix(
            held["params"][0]["blockNumber"]
                .as_str()
                .unwrap()
                .trim_start_matches("0x"),
            16,
        )
        .unwrap();
        let held_raw: Bytes = held["params"][0]["txs"][0]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let held_post =
            EthereumTxEnvelope::<TxEip4844WithSidecar>::decode_2718(&mut held_raw.as_ref())
                .unwrap();
        let old_post = *held_post.tx_hash();
        let held_call =
            eez_protocol::abi::postAndVerifyBatchCall::abi_decode(held_post.input()).unwrap();
        assert_eq!(
            held_call
                .batch
                .entries
                .last()
                .unwrap()
                .rootUpdates
                .last()
                .unwrap()
                .newRoot,
            retry.b256("sync_hash").unwrap(),
            "repricing must preserve the proved retry endpoint"
        );
        // Killing the composer also kills this fixture's isolated L1. Establish
        // disk persistence of the settled prefix before testing lost retry ownership.
        world
            .wait_for_persisted_l1_block(
                alloy_rpc_types_eth::BlockNumHash::new(inclusion_block, posted.block_hash.unwrap()),
                TIMEOUT,
            )
            .await
            .unwrap();
        assert!(
            l1.get_transaction_receipt(hashes[omitted])
                .await
                .unwrap()
                .is_none(),
            "restart fixture must still own an unsettled source before the crash"
        );
        // Exercise Reth's normal masked persistence: block history has reached
        // disk while the state/trie frontier still lags. A graceful flush would
        // collapse these frontiers and miss this crash-recovery path.
        wait_for(TIMEOUT, || async {
            Ok(world
                .persisted_l2_frontiers()
                .await?
                .filter(|(blocks, state)| *state > 0 && state < blocks && *state < height))
        })
        .await
        .unwrap();
        // Leave the old relay request held until its bounded timeout. It must
        // never reach L1; the restarted composer has no record of that retry.
        let (persisted_blocks, persisted_state) = world.crash_and_restart_composer().await.unwrap();
        assert!(
            persisted_blocks > persisted_state,
            "crash must leave masked L2 state: blocks={persisted_blocks}, state={persisted_state}"
        );
        assert!(
            persisted_state > 0 && persisted_state < height,
            "crash must leave an intermediate L2 state prefix: {persisted_state}, settled={height}"
        );
        wait_for(TIMEOUT, || async {
            Ok((l1.get_block_number().await? > target).then_some(()))
        })
        .await
        .unwrap();
        assert_eq!(
            l1.get_transaction_receipt(post_hash)
                .await
                .unwrap()
                .unwrap()
                .block_hash,
            posted.block_hash,
            "the settled partial prefix must survive the fixture's L1 restart"
        );
        assert!(
            l1.get_transaction_receipt(hashes[omitted])
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            l1.get_transaction_receipt(old_post)
                .await
                .unwrap()
                .is_none()
        );
        let raw: Bytes = request["txs"][omitted + 1]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        world.fail_rpc_methods(&[]).unwrap();
        let front = ProviderBuilder::new().connect_http(world.l1_xchain().parse().unwrap());
        let resubmitted = wait_for(TIMEOUT, || async {
            match front.send_raw_transaction(&raw).await {
                Ok(pending) => Ok(Some(*pending.tx_hash())),
                Err(error) if error.to_string().contains("eez node is starting up") => Ok(None),
                Err(error) => Err(error.into()),
            }
        })
        .await
        .unwrap();
        assert_eq!(resubmitted, hashes[omitted]);
        retry = wait_for(TIMEOUT, || async {
            Ok(world.node.signals_since(0)?.into_iter().find(|event| {
                event.name == signals::COMPOSER_BUNDLE_DISPATCHED
                    && event.fields["user_hashes"] == format!("{:?}", [hashes[omitted]])
            }))
        })
        .await
        .unwrap();
        assert_eq!(
            retry.fields["attempt_counts"], "[0]",
            "restart does not retain retry counters"
        );
        assert_ne!(retry.b256("post_batch_hash").unwrap(), old_post);
    }
    let retried = wait_for(TIMEOUT, || async {
        Ok(l1.get_transaction_receipt(hashes[omitted]).await?)
    })
    .await
    .unwrap();
    // The final poster hash changes during fee/gas selection. Its proved
    // endpoint and the users' signed transactions remain unchanged.
    let retry_settlement = wait_for(TIMEOUT, || async {
        Ok(world.node.signals_since(0)?.into_iter().find(|event| {
            event.name == signals::COMPOSER_SETTLEMENT_RECONCILED
                && event.fields["canonical_l2_hash"] == retry.fields["sync_hash"]
        }))
    })
    .await
    .unwrap();
    assert_eq!(retry_settlement.fields["outcomes"], "[Terminal(Committed)]");
    assert!(retried.status());
    assert!(retried.block_number.unwrap() > inclusion_block);
    assert_ne!(expected_prefix, full_hash);
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let prefix_block = l2
        .get_block_by_hash(expected_prefix)
        .await
        .unwrap()
        .unwrap();
    let prefix_height = prefix_block.header.number;
    if omitted > 0 {
        assert_eq!(prefix_height, height);
    }
    assert!(prefix_height <= height);
    assert_eq!(
        block_number_and_hash_at(&world.l2_rpc(), BlockNumberOrTag::Number(prefix_height))
            .await
            .unwrap()
            .map(|(_, hash)| hash),
        Some(expected_prefix),
        "retry must preserve the confirmed partial prefix"
    );

    // A clean follower has no local proposal or sequencer input.
    let follower = NodeHandle::start(
        "partial-clean-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let target = block_number_and_hash_at(&world.l2_rpc(), BlockNumberOrTag::Safe)
        .await
        .unwrap()
        .unwrap();
    wait_for(TIMEOUT, || async {
        let actual =
            block_number_and_hash_at(&follower.l2_rpc_url(), BlockNumberOrTag::Number(target.0))
                .await?;
        let safe = block_number_and_hash_at(&follower.l2_rpc_url(), BlockNumberOrTag::Safe).await?;
        Ok((actual == Some(target) && safe.is_some_and(|head| head.0 >= target.0)).then_some(()))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "clean follower did not derive partial history: {error:#}\n{}",
            follower.log_tail(60)
        )
    });

    let healthy = sign_and_send(
        &world.l1_xchain(),
        keys[omitted],
        DEV_CHAIN_ID,
        onchain_nonce(&world.l1_rpc(), keys[omitted]).await.unwrap(),
        Some(world.setter_proxy),
        U256::ZERO,
        IValue::setValueCall { v: U256::from(99) }.abi_encode(),
        600_000,
    )
    .await
    .unwrap();
    assert!(
        wait_for(TIMEOUT, || async {
            receipt_ok(&world.l1_rpc(), healthy).await
        })
        .await
        .unwrap()
    );
    world.node.assert_no_divergence_failure_logs();
    follower.assert_no_divergence_failure_logs();
    println!(
        "partial user={omitted} post={post_hash} l1={inclusion_block} sync={height} full={full_hash} prefix={expected_prefix:?} follower={target:?} users={hashes:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_source_omission_keeps_the_registration_anchor() {
    partial_after_proving(0, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_prefix_keeps_unconsumed_users_and_receipt_costs() {
    partial_after_proving(1, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_source_omission_keeps_two_user_prefix() {
    partial_after_proving(2, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_after_partial_keeps_accounting_and_rebuilds_intent() {
    partial_after_proving(1, true).await;
}

/// Both posts carry the identical proof, calldata and valid blob sidecar. A
/// normal L1 setter changes the middle destination's return value after proving,
/// making the first post skip it. Restoring that value lets the second post
/// resume at the middle group, at the same L2 height and in the same L1 block.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_blob_batch_resumes_at_middle_group_in_same_l1_block() {
    valid_same_block_resume().await;
}

async fn valid_same_block_resume() {
    let env = policy_env();
    let world = setup_cross_chain_with_env(&env).await.unwrap();
    let middle = deploy_value(
        &world.l1_rpc(),
        TARGET_DEPLOYER,
        DEV_CHAIN_ID,
        U256::from(5),
    )
    .await
    .unwrap();
    let middle_proxy = create_l2_cross_chain_proxy(&world.l2_rpc(), TARGET_DEPLOYER, middle, 0)
        .await
        .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let setter_nonce = onchain_nonce(&world.l1_rpc(), TARGET_DEPLOYER)
        .await
        .unwrap();
    // Wait until the current empty bundle has passed the relay before arming
    // capture. The composer dispatch event precedes its HTTP request, so it
    // could capture that empty bundle instead of the following three-user slot.
    let cursor = world.node.signal_cursor().unwrap();
    wait_for(TIMEOUT, || async {
        Ok(world
            .node
            .signals_since(cursor)?
            .iter()
            .any(|event| {
                event.name == "eez.composer.settlement.decision" && event.fields["due"] == false
            })
            .then_some(()))
    })
    .await
    .unwrap();
    world.hold_next_bundle().unwrap();
    let mut users = Vec::new();
    for (index, (key, proxy)) in [
        (ANVIL_KEY_2, world.outbound_proxy),
        (ANVIL_KEY_4, middle_proxy),
        (ANVIL_KEY_1, world.outbound_proxy),
    ]
    .into_iter()
    .enumerate()
    {
        users.push(
            sign_and_send(
                &world.l2_xchain(),
                key,
                world.l2_chain_id,
                onchain_nonce(&world.l2_rpc(), key).await.unwrap(),
                Some(proxy),
                U256::ZERO,
                IValue::setValueCall {
                    v: U256::from(11 + index),
                }
                .abi_encode(),
                600_000,
            )
            .await
            .unwrap(),
        );
    }
    let mut request = wait_for(TIMEOUT, || async { world.held_bundle() })
        .await
        .unwrap();
    let raw: Bytes = request["params"][0]["txs"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let original =
        EthereumTxEnvelope::<TxEip4844WithSidecar>::decode_2718(&mut raw.as_ref()).unwrap();
    let call = eez_protocol::abi::postAndVerifyBatchCall::abi_decode(original.input()).unwrap();
    assert_eq!(
        call.batch.immediateEntryCount,
        U256::from(4),
        "must prove all three users together"
    );
    assert!(
        call.batch.expectedRootPerRollup.is_empty(),
        "resume cannot reuse a pre-state assertion"
    );
    assert!(!call.batch.blobIndices.is_empty());
    let priority = original.max_priority_fee_per_gas().unwrap();
    assert!(priority > 2);
    let setter: alloy_signer_local::PrivateKeySigner = TARGET_DEPLOYER.parse().unwrap();
    let change = |nonce, value, priority| {
        let mut tx = TxEip1559 {
            chain_id: DEV_CHAIN_ID,
            nonce,
            gas_limit: 100_000,
            max_fee_per_gas: 100_000_000_000,
            max_priority_fee_per_gas: priority,
            to: middle.into(),
            value: U256::ZERO,
            access_list: Default::default(),
            input: IValue::setValueCall {
                v: U256::from(value),
            }
            .abi_encode()
            .into(),
        };
        let sig = setter.sign_transaction_sync(&mut tx).unwrap();
        alloy_consensus::TxEnvelope::from(tx.into_signed(sig))
    };
    let before = change(setter_nonce, 12u64, priority + 1);
    let restore = change(setter_nonce + 1, 5u64, priority - 1);
    let poster: alloy_signer_local::PrivateKeySigner = world.cfg.poster_key.parse().unwrap();
    let mut resumed_tx = original.as_eip4844().unwrap().tx().clone();
    resumed_tx.tx.nonce += 1;
    resumed_tx.tx.max_priority_fee_per_gas = priority - 2;
    let sig = poster.sign_transaction_sync(&mut resumed_tx).unwrap();
    let resumed = EthereumTxEnvelope::Eip4844(resumed_tx.into_signed(sig));
    assert_eq!(
        resumed.input(),
        original.input(),
        "proof and complete ABI remain identical"
    );
    assert_eq!(
        resumed.blob_versioned_hashes(),
        original.blob_versioned_hashes()
    );
    request["params"][0]["txs"] = serde_json::json!([
        Bytes::from(before.encoded_2718()),
        raw,
        Bytes::from(restore.encoded_2718()),
        Bytes::from(resumed.encoded_2718()),
    ]);
    world.release_bundle(&request).unwrap();

    let mut receipts = Vec::new();
    for hash in [
        before.tx_hash(),
        original.tx_hash(),
        restore.tx_hash(),
        resumed.tx_hash(),
    ] {
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l1.get_transaction_receipt(*hash).await?)
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "missing fixture transaction {hash}: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
        assert!(receipt.status(), "fixture transaction {hash} must succeed");
        receipts.push(receipt);
    }
    for pair in receipts.windows(2) {
        assert_eq!(pair[0].block_hash, pair[1].block_hash);
        assert!(pair[0].transaction_index < pair[1].transaction_index);
    }
    let roots = |receipt: &alloy_rpc_types_eth::TransactionReceipt| {
        receipt
            .inner
            .logs()
            .iter()
            .filter(|log| log.address() == world.cfg.eez_address)
            .filter_map(|log| {
                log.log_decode::<eez_protocol::abi::L2ExecutionPerformed>()
                    .ok()
            })
            .filter(|event| event.inner.data.rollupId == world.cfg.rollup_id)
            .map(|event| event.inner.data.newRoot)
            .collect::<Vec<_>>()
    };
    let first_roots = roots(&receipts[1]);
    let resumed_roots = roots(&receipts[3]);
    assert_eq!(first_roots.len(), 2, "anchor and first user only");
    assert_eq!(
        resumed_roots.len(),
        2,
        "only the new middle and last groups"
    );
    let full_hash = *resumed_roots.last().unwrap();
    let dispatched = wait_for(TIMEOUT, || async {
        Ok(world.node.signals_since(0)?.into_iter().find(|event| {
            event.name == signals::COMPOSER_BUNDLE_DISPATCHED
                && event.fields["user_hashes"] == format!("{users:?}")
        }))
    })
    .await
    .unwrap();
    let height = dispatched.u64("sync_height").unwrap();
    assert_eq!(dispatched.b256("sync_hash").unwrap(), full_hash);
    assert_eq!(dispatched.fields["user_hashes"], format!("{users:?}"));
    assert_eq!(dispatched.fields["attempt_counts"], "[0, 0, 0]");
    let reconciled = attempt_event(
        &world,
        signals::COMPOSER_SETTLEMENT_RECONCILED,
        *original.tx_hash(),
    )
    .await;
    assert_eq!(
        reconciled.fields["outcomes"],
        "[Terminal(Committed), Terminal(Committed), Terminal(Committed)]"
    );
    let follower = NodeHandle::start(
        "valid-resume-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    for rpc in [world.l2_rpc(), follower.l2_rpc_url()] {
        wait_for(TIMEOUT, || async {
            let actual = block_number_and_hash_at(&rpc, BlockNumberOrTag::Number(height)).await?;
            let safe = block_number_and_hash_at(&rpc, BlockNumberOrTag::Safe).await?;
            Ok(
                (actual == Some((height, full_hash)) && safe.is_some_and(|head| head.0 >= height))
                    .then_some(()),
            )
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "valid resume not derived: {error:#}{}\n{}",
                world.settlement_diagnostics(),
                follower.log_tail(60)
            )
        });
        let provider = ProviderBuilder::new()
            .network::<alloy_network::AnyNetwork>()
            .connect_http(rpc.parse().unwrap());
        let block = provider
            .get_block_by_number(height.into())
            .full()
            .await
            .unwrap()
            .unwrap();
        let hashes: Vec<_> = block
            .transactions
            .txns()
            .map(alloy_network::TransactionResponse::tx_hash)
            .collect();
        for hash in &users {
            assert_eq!(
                hashes.iter().filter(|candidate| *candidate == hash).count(),
                1,
                "resumption must retain the first user and emit each signed user once"
            );
        }
    }
    world.node.assert_no_divergence_failure_logs();
    follower.assert_no_divergence_failure_logs();
    println!(
        "valid resume posts={:?}/{:?} l1={:?} sync={height} prefix={} full={full_hash}",
        original.tx_hash(),
        resumed.tx_hash(),
        receipts[1].block_hash,
        first_roots[1]
    );
}
