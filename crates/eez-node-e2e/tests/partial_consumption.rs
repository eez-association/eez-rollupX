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

const TIMEOUT: Duration = Duration::from_secs(120);

async fn partial_after_proving(omitted: usize) {
    let world = setup_cross_chain_with_env(&[]).await.unwrap();
    world.omit_next_bundle_user(omitted).unwrap();
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
                event.name == signals::COMPOSER_BUNDLE_DISPATCHED
                    || event.name == signals::COMPOSER_PHASE1_BUNDLE_DISPATCHED
            })
            .then_some(()))
    })
    .await
    .unwrap();
    let keys = [ANVIL_KEY_2, ANVIL_KEY_4, ANVIL_KEY_1];
    let mut hashes = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        hashes.push(
            sign_and_send(
                &world.l1_xchain(),
                key,
                DEV_CHAIN_ID,
                onchain_nonce(&world.l1_rpc(), key).await.unwrap(),
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
    let first_attempt = wait_for(TIMEOUT, || async {
        let journal = world.settlement_journal()?;
        Ok(journal["state"]["attempts"]
            .as_object()
            .unwrap()
            .values()
            .find(|attempt| attempt["id"]["post_batch_hash"] == serde_json::json!(post_hash))
            .cloned())
    })
    .await
    .unwrap();
    let height = first_attempt["sync_height"].as_u64().unwrap();
    let full_hash: B256 = first_attempt["manifest"]["sync_hash"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();

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
    let expected_prefix = if omitted == 0 {
        *registry_roots(&posted)
            .last()
            .expect("successful anchor commitment")
    } else {
        first_attempt["manifest"]["effects"][omitted - 1]["block_hash"]
            .as_str()
            .unwrap()
            .parse::<B256>()
            .unwrap()
    };
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
    let dispositions = wait_for(TIMEOUT, || async {
        let journal = world.settlement_journal()?;
        let users = &journal["state"]["users"];
        Ok(hashes
            .iter()
            .all(|hash| {
                users[hash.to_string()]["location"]
                    .get("Terminal")
                    .is_some()
            })
            .then_some(journal))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "unresolved partial ownership: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    for (index, hash) in hashes.iter().enumerate() {
        let user = &dispositions["state"]["users"][hash.to_string()];
        assert_eq!(
            user["location"]["Terminal"]["reason"],
            if index <= omitted {
                "Committed"
            } else {
                "SourceReverted"
            }
        );
        assert_eq!(user["tx"]["attempts"], u64::from(index == omitted));
    }
    let retried = l1
        .get_transaction_receipt(hashes[omitted])
        .await
        .unwrap()
        .unwrap();
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

    // A clean follower has no local proposal, journal or sequencer input.
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
async fn first_user_omission_preserves_anchor_and_retries_only_unspent_source() {
    partial_after_proving(0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn middle_user_omission_preserves_prefix_and_spent_source_dispositions() {
    partial_after_proving(1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_user_omission_preserves_two_user_prefix_and_clean_follower() {
    partial_after_proving(2).await;
}

/// Both posts carry the identical proof, calldata and valid blob sidecar. A
/// normal L1 setter changes the middle destination's return value after proving,
/// making the first post skip it. Restoring that value lets the second post
/// resume at the middle group, at the same L2 height and in the same L1 block.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_blob_batch_resumes_at_middle_group_in_same_l1_block() {
    let world = setup_cross_chain_with_env(&[]).await.unwrap();
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
            .any(|event| event.name == signals::BUNDLE_ACCEPTED)
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
    let journal = wait_for(TIMEOUT, || async {
        let journal = world.settlement_journal()?;
        Ok(users
            .iter()
            .all(|hash| {
                journal["state"]["users"][hash.to_string()]["location"]
                    .get("Terminal")
                    .is_some()
            })
            .then_some(journal))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "resumed ownership: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    let attempt = journal["state"]["attempts"]
        .as_object()
        .unwrap()
        .values()
        .find(|attempt| attempt["id"]["post_batch_hash"] == serde_json::json!(original.tx_hash()))
        .unwrap();
    let height = attempt["sync_height"].as_u64().unwrap();
    assert_eq!(
        attempt["manifest"]["sync_hash"],
        serde_json::json!(full_hash)
    );
    assert_eq!(
        attempt["manifest"]["effects"][0]["block_hash"],
        serde_json::json!(first_roots[1])
    );
    for hash in &users {
        let user = &journal["state"]["users"][hash.to_string()];
        assert_eq!(user["location"]["Terminal"]["reason"], "Committed");
        assert_eq!(
            user["tx"]["attempts"], 0,
            "a same-block resume requires no retry"
        );
    }
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
        let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());
        let block = provider
            .get_block_by_number(height.into())
            .full()
            .await
            .unwrap()
            .unwrap();
        let hashes: Vec<_> = block
            .transactions
            .txns()
            .map(|tx| *tx.inner.tx_hash())
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
