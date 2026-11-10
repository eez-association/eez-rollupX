//! A poster may retune unproven dispatch counts after signing. The follower
//! must reconstruct the same execution from native blobs and canonical roots.
use super::*;
use alloy_consensus::{EthereumTxEnvelope, SignableTransaction, Transaction, TxEip4844WithSidecar};
use alloy_eips::{BlockNumberOrTag, Decodable2718, Encodable2718};
use alloy_network::TxSignerSync;
use eez_testkit::{
    NodeBinary, NodeConfig, NodeHandle, block_number_and_hash_at, deploy_setter_wrapper,
};

alloy_sol_types::sol! {
    function setMetaCall(address target, bytes data);
}

async fn retuned_dispatch(static_reads: bool, consume: bool) {
    let world = setup_cross_chain_with_env(&[
        (POST_BATCHER_ENV, "meta".into()),
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10000".into()),
    ])
    .await
    .unwrap();
    let wrapper = deploy_setter_wrapper(
        &world.l1_rpc(),
        TARGET_DEPLOYER,
        DEV_CHAIN_ID,
        world.setter_proxy,
    )
    .await
    .unwrap();
    let before = l2_value(&world.l2_rpc(), world.value_l2).await.unwrap();
    let data = if static_reads {
        ISetterWrapper::readWriteReadCall { v: U256::from(71) }.abi_encode()
    } else {
        ISetterWrapper::setViaProxyCall { v: U256::from(71) }.abi_encode()
    };
    let running = NodeHandle::start(
        "retuned-running-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let arm = sign_and_send(
        &world.l1_rpc(),
        TARGET_DEPLOYER,
        DEV_CHAIN_ID,
        onchain_nonce(&world.l1_rpc(), TARGET_DEPLOYER)
            .await
            .unwrap(),
        Some(world.post_batch_target),
        U256::ZERO,
        setMetaCallCall {
            target: if consume { wrapper } else { Address::ZERO },
            data: data.clone().into(),
        }
        .abi_encode(),
        500_000,
    )
    .await
    .unwrap();
    assert!(
        wait_for(SETTLE_TIMEOUT, || async {
            receipt_ok(&world.l1_rpc(), arm).await
        })
        .await
        .unwrap()
    );
    world.pause_bundles_after(1).unwrap();
    world.hold_next_bundle().unwrap();
    let source = sign_and_send(
        &world.l1_xchain(),
        INBOUND_USER,
        DEV_CHAIN_ID,
        onchain_nonce(&world.l1_rpc(), INBOUND_USER).await.unwrap(),
        Some(wrapper),
        U256::ZERO,
        data,
        1_000_000,
    )
    .await
    .unwrap();
    let mut request = wait_for(SETTLE_TIMEOUT, || async { world.held_bundle() })
        .await
        .unwrap();
    let raw: Bytes = request["params"][0]["txs"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let original =
        EthereumTxEnvelope::<TxEip4844WithSidecar>::decode_2718(&mut raw.as_ref()).unwrap();
    let mut call = eez_protocol::abi::postAndVerifyBatchCall::abi_decode(original.input()).unwrap();
    assert_eq!(call.batch.immediateEntryCount, U256::from(1));
    assert_eq!(call.batch.entries.len(), 2, "anchor and one inbound write");
    assert_eq!(!call.batch.staticEntries.is_empty(), static_reads);
    let expected_root = call.batch.entries[usize::from(consume)].rootUpdates[0].newRoot;
    let original_counts = (
        call.batch.immediateEntryCount,
        call.batch.immediateStaticEntryCount,
    );
    call.batch.immediateEntryCount = U256::from(call.batch.entries.len());
    call.batch.immediateStaticEntryCount = U256::from(call.batch.staticEntries.len());
    let mut tx = original.as_eip4844().unwrap().tx().clone();
    tx.tx.input = call.abi_encode().into();
    // Retuning changes the posting execution path. Budget for loading and
    // consuming transient entries instead of the original queue-only estimate.
    tx.tx.gas_limit += 3_000_000;
    let poster: alloy_signer_local::PrivateKeySigner = world.cfg.poster_key.parse().unwrap();
    let signature = poster.sign_transaction_sync(&mut tx).unwrap();
    let retuned = EthereumTxEnvelope::Eip4844(tx.into_signed(signature));
    assert_eq!(
        retuned.blob_versioned_hashes(),
        original.blob_versioned_hashes()
    );
    let mut unchanged = call.batch.clone();
    unchanged.immediateEntryCount = original_counts.0;
    unchanged.immediateStaticEntryCount = original_counts.1;
    assert_eq!(
        eez_protocol::entries::encode_postbatch(&unchanged),
        original.input().to_vec(),
        "only dispatch counts may change in the proved calldata"
    );
    // The source wrapper runs in the meta hook instead of a separate user tx.
    request["params"][0]["txs"] = serde_json::json!([Bytes::from(retuned.encoded_2718())]);
    world.release_bundle(&request).unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let receipt = wait_for(SETTLE_TIMEOUT, || async {
        Ok(l1.get_transaction_receipt(*retuned.tx_hash()).await?)
    })
    .await
    .unwrap();
    if !receipt.status() {
        let trace: serde_json::Value = l1
            .raw_request(
                "debug_traceTransaction".into(),
                (
                    retuned.tx_hash(),
                    serde_json::json!({"tracer": "callTracer"}),
                ),
            )
            .await
            .unwrap();
        panic!("retuned posting reverted: {trace}");
    }
    assert_eq!(
        state_root(&world.l1_rpc(), world.cfg.eez_address, world.cfg.rollup_id)
            .await
            .unwrap(),
        expected_root
    );
    eprintln!(
        "C13 static={static_reads} consume={consume} post={} endpoint={expected_root} gas_used={} original_gas_limit={}",
        retuned.tx_hash(),
        receipt.gas_used,
        original.gas_limit()
    );
    let follower = NodeHandle::start(
        "retuned-dispatch-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    for rpc in [world.l2_rpc(), running.l2_rpc_url(), follower.l2_rpc_url()] {
        wait_for(SETTLE_TIMEOUT, || async {
            let safe = block_number_and_hash_at(&rpc, BlockNumberOrTag::Safe).await?;
            Ok(safe.filter(|(_, hash)| *hash == expected_root))
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "retuned settlement not reconstructed: {error:#}{}\n{}",
                world.settlement_diagnostics(),
                follower.log_tail(60)
            )
        });
        // A sequencer may retain speculative pending work above its safe head.
        let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());
        let value: Bytes = provider.raw_request("eth_call".into(), (
            serde_json::json!({"to": world.value_l2, "data": Bytes::from(IValue::valueCall {}.abi_encode())}),
            "safe",
        )).await.unwrap();
        assert_eq!(
            U256::abi_decode(&value).unwrap(),
            if consume { U256::from(71) } else { before }
        );
    }
    assert_eq!(
        read_state_word(
            &world.l1_rpc(),
            &call_read(
                wrapper,
                "mixedReadSum()",
                ISetterWrapper::mixedReadSumCall {}.abi_encode()
            )
        )
        .await
        .unwrap(),
        if consume && static_reads {
            U256::from(66)
        } else {
            U256::ZERO
        }
    );
    if !consume {
        // A posted but unused transient entry must not release queue ownership.
        // Let the original source request retry through the normal queued path.
        world.resume_bundles().unwrap();
        assert!(
            wait_for(SETTLE_TIMEOUT, || async {
                receipt_ok(&world.l1_rpc(), source).await
            })
            .await
            .unwrap()
        );
        for rpc in [world.l2_rpc(), running.l2_rpc_url(), follower.l2_rpc_url()] {
            wait_for(SETTLE_TIMEOUT, || async {
                let root =
                    state_root(&world.l1_rpc(), world.cfg.eez_address, world.cfg.rollup_id).await?;
                let safe = safe_block_hash(&rpc).await?;
                Ok((root != expected_root && safe == Some(root)).then_some(()))
            })
            .await
            .unwrap();
            assert_eq!(
                l2_value(&rpc, world.value_l2).await.unwrap(),
                U256::from(71)
            );
        }
    }
    running.assert_no_divergence_failure_logs();
    follower.assert_no_divergence_failure_logs();
    world.node.assert_no_divergence_failure_logs();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn follower_replays_retuned_immediate_and_static_entries() {
    retuned_dispatch(true, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn follower_replays_retuned_immediate_entries() {
    retuned_dispatch(false, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn follower_does_not_deliver_unconsumed_immediate_entries() {
    retuned_dispatch(true, false).await;
}
