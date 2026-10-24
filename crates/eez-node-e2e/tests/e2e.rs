//! Fresh-network startup checks use the same canonical native L1 as execution.
//! The standalone Anvil check tests only its reorg clock restoration.

use std::time::Duration;

use alloy_primitives::U256;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::BlockNumberOrTag;

use eez_testkit::{
    ANVIL_ADDR_3, ANVIL_KEY_1, Anvil, NodeBinary, NodeConfig, NodeHandle,
    send_l2_value_transfer_confirmed, setup_native_history, wait_for, wait_for_latest_height,
    wait_for_safe_prefix_convergence,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_mins(5);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_history_import_requires_executed_state() {
    use alloy_primitives::{Address, B256, Bytes, keccak256};
    use alloy_rpc_types_eth::{AccessListResult, TransactionRequest};
    use alloy_sol_types::{SolCall as _, SolValue as _};
    use eez_protocol::l1_history::{HISTORY_ADDRESS, IL1BlockhashContract};
    use serde_json::json;

    let world = setup_native_history(&[
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
    ])
    .await
    .unwrap();
    assert_eq!(
        (
            world.dep.profile.blockInterval,
            world.dep.profile.syncInterval,
            world.dep.profile.updateOffset,
        ),
        (1, 5, 2),
    );
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let request = TransactionRequest::default()
        .to(HISTORY_ADDRESS)
        .input(Bytes::from(IL1BlockhashContract::currentCall {}.abi_encode()).into());
    // Inject the existing observation fixture only into the RPC simulation; it
    // reads history and PREVRANDAO together without a deployment transaction.
    let artifact: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            eez_testkit::repo_root()
                .join("contracts/out/OfflineHistoryProbe.sol/OfflineHistoryProbe.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let probe = Address::with_last_byte(90);
    let observe = TransactionRequest::default()
        .to(probe)
        .input(Bytes::copy_from_slice(&keccak256(b"observe()")[..4]).into());
    let overrides = json!({probe.to_string(): {"code": artifact["deployedBytecode"]["object"]}});
    // nextCutoff is internal slot 3, without a public getter. Replace only its
    // reader code after the real prefix; the simulated history storage remains.
    let cutoff_request = TransactionRequest::default().to(HISTORY_ADDRESS);
    let cutoff_override =
        json!({HISTORY_ADDRESS.to_string(): {"code": "0x60035460005260206000f3"}});
    let counter: serde_json::Value = serde_json::from_slice(
        &std::fs::read(eez_testkit::repo_root().join("contracts/out/Counter.sol/Counter.json"))
            .unwrap(),
    )
    .unwrap();
    let counter_code: Bytes =
        serde_json::from_value(counter["bytecode"]["object"].clone()).unwrap();
    let deploy = TransactionRequest::default().input(counter_code.into());
    let (parent, pending_origin, pending_randao) = wait_for(DEFAULT_TIMEOUT, || async {
        let parent = l2
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .expect("latest header before pending observations")
            .unwrap()
            .header;
        if parent.number % 5 != 2 {
            return Ok(None);
        }
        let mut requests = Vec::new();
        for tag in [
            json!("pending"),
            json!("latest"),
            json!({"blockHash": parent.hash}),
        ] {
            for method in ["eth_call", "eth_estimateGas", "eth_createAccessList"] {
                let (snake, camel) = if method == "eth_call" {
                    ("state_overrides", "stateOverrides")
                } else {
                    ("state_override", "stateOverride")
                };
                requests.extend([
                    (method, json!([observe, tag, overrides])),
                    (
                        method,
                        json!({"request": observe, "block_number": tag, (snake): overrides}),
                    ),
                    (
                        method,
                        json!({"request": observe, "blockNumber": tag, (camel): overrides}),
                    ),
                ]);
            }
        }
        let hypothetical = B256::repeat_byte(42);
        let block_overrides =
            json!({"time": "0x2345", "number": "0x1234", "prevRandao": hypothetical});
        // NUMBER, TIMESTAMP and PREVRANDAO expose the counterfactual environment;
        // the history reads below must still see the already executed prefix.
        let env_probe = Address::with_last_byte(91);
        let env_request = TransactionRequest::default().to(env_probe);
        let env_override =
            json!({env_probe.to_string(): {"code": "0x43600052426020524460405260606000f3"}});
        requests.extend([
            (
                "eth_call",
                json!([cutoff_request, "pending", cutoff_override]),
            ),
            ("eth_call", json!([request])),
            ("eth_call", json!([request, "latest"])),
            (
                "eth_call",
                json!([observe, "pending", overrides, block_overrides]),
            ),
            (
                "eth_call",
                json!([cutoff_request, "pending", cutoff_override, block_overrides]),
            ),
            (
                "eth_call",
                json!([env_request, "pending", env_override, block_overrides]),
            ),
            ("eth_estimateGas", json!([deploy, "pending"])),
            ("eth_estimateGas", json!([deploy, "latest"])),
        ]);
        // Parallel requests fit within a normal L2 slot. wait_for retries Err,
        // so RPC errors deliberately panic instead; only a changed parent is
        // an inconclusive observation that authorizes another selection.
        let mut calls = tokio::task::JoinSet::new();
        let mut results = vec![None; requests.len()];
        for (index, (method, params)) in requests.into_iter().enumerate() {
            let l2 = l2.clone();
            calls.spawn(async move {
                (
                    index,
                    method,
                    l2.raw_request::<_, serde_json::Value>(method.into(), params)
                        .await,
                )
            });
        }
        while let Some(result) = calls.join_next().await {
            let (index, method, response) = result.expect("RPC observation task panicked");
            results[index] =
                Some(response.unwrap_or_else(|error| {
                    panic!("{method} observation {index} failed: {error}")
                }));
        }
        let after = l2
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .expect("latest header after pending observations")
            .unwrap()
            .header;
        if after.hash != parent.hash {
            return Ok(None);
        }
        let results: Vec<_> = results.into_iter().map(Option::unwrap).collect();
        for group in results[..27].chunks_exact(9) {
            for method in group.chunks_exact(3) {
                assert_eq!(method[0], method[1], "snake_case named/positional parity");
                assert_eq!(method[0], method[2], "camelCase named/positional parity");
            }
            let gas: U256 = serde_json::from_value(group[3].clone()).unwrap();
            assert!(gas > U256::from(21_000));
            let access: AccessListResult = serde_json::from_value(group[6].clone()).unwrap();
            assert!(access.error.is_none(), "{access:?}");
            assert!(
                access
                    .access_list
                    .0
                    .iter()
                    .any(|item| item.address == HISTORY_ADDRESS)
            );
        }
        let bytes = |index: usize| serde_json::from_value::<Bytes>(results[index].clone()).unwrap();
        let (number, hash, randao) = <(u64, B256, U256)>::abi_decode(&bytes(0)).unwrap();
        let expected_cutoff = U256::from(parent.timestamp - 2 + 5);
        assert_eq!(U256::abi_decode(&bytes(27)).unwrap(), expected_cutoff);
        assert_eq!(results[28], results[29], "default tag remains latest");
        assert_eq!(
            <(u64, B256, U256)>::abi_decode(&bytes(30)).unwrap(),
            (number, hash, U256::from_be_slice(hypothetical.as_slice()))
        );
        assert_eq!(
            U256::abi_decode(&bytes(31)).unwrap(),
            expected_cutoff,
            "block overrides do not reschedule or repeat history execution"
        );
        assert_eq!(
            <(U256, U256, U256)>::abi_decode(&bytes(32)).unwrap(),
            (
                U256::from(0x1234),
                U256::from(0x2345),
                U256::from_be_slice(hypothetical.as_slice())
            )
        );
        let creation_gas: U256 = serde_json::from_value(results[33].clone()).unwrap();
        assert!(creation_gas > U256::from(53_000));
        assert_eq!(
            results[33], results[34],
            "Counter creation estimate at the import boundary"
        );
        Ok(Some((parent, (number, hash), randao)))
    })
    .await
    .expect("pending simulation did not execute the scheduled import on a stable parent");
    let before = l2
        .call(request.clone())
        .block(parent.hash.into())
        .await
        .unwrap();
    let before = IL1BlockhashContract::currentCall::abi_decode_returns(&before).unwrap();
    let previous_source = l1
        .get_block_by_number(before.number.into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.hash, previous_source.header.hash);
    assert_eq!(parent.mix_hash, previous_source.header.mix_hash);
    let selected_pending = l1
        .get_block_by_number(pending_origin.0.into())
        .await
        .unwrap()
        .unwrap()
        .header;
    assert!(pending_origin.0 >= before.number);
    assert_eq!(pending_origin.1, selected_pending.hash);
    assert_eq!(
        pending_randao,
        U256::from_be_slice(selected_pending.mix_hash.as_slice())
    );
    assert!(selected_pending.timestamp <= parent.timestamp - 2);

    let next = wait_for(DEFAULT_TIMEOUT, || async {
        Ok(l2.get_block_by_number((parent.number + 1).into()).await?)
    })
    .await
    .unwrap()
    .header;
    assert_eq!(next.parent_hash, parent.hash);
    let after = l2.call(request).block(next.hash.into()).await.unwrap();
    let after = IL1BlockhashContract::currentCall::abi_decode_returns(&after).unwrap();
    let selected = l1
        .get_block_by_number(after.number.into())
        .await
        .unwrap()
        .unwrap()
        .header;
    let successor = wait_for(DEFAULT_TIMEOUT, || async {
        Ok(l1.get_block_by_number((after.number + 1).into()).await?)
    })
    .await
    .unwrap()
    .header;
    let cutoff = next.timestamp - 3;
    assert!(selected.timestamp <= cutoff && successor.timestamp > cutoff);
    assert_eq!(after.hash, selected.hash);
    assert_eq!(next.mix_hash, selected.mix_hash);

    let revert = TransactionRequest::default().to(probe);
    let reverting_code = json!({probe.to_string(): {"code": "0x60006000fd"}});
    for method in ["eth_call", "eth_estimateGas"] {
        let error = l2
            .raw_request::<_, serde_json::Value>(
                method.into(),
                json!([revert, "pending", reverting_code]),
            )
            .await
            .expect_err("application reverts must propagate through the pending adapter");
        assert_eq!(error.as_error_resp().unwrap().code, 3, "{method}: {error}");
    }
    let access: AccessListResult = l2
        .raw_request(
            "eth_createAccessList".into(),
            json!([revert, "pending", reverting_code]),
        )
        .await
        .expect("access-list RPC returns application reverts in its result");
    assert!(access.error.is_some_and(|error| error.contains("revert")));
    world.node.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn node_fixture_ignores_parent_dotenv() {
    let world = setup_native_history(&[("EEZ_SETTLEMENT_PURE_L2_MODE", "always".into())])
        .await
        .unwrap();
    wait_for_latest_height(&world.node, 1, DEFAULT_TIMEOUT)
        .await
        .expect("native Composer did not become ready to sequence");
    let hash = send_l2_value_transfer_confirmed(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(1u64),
        DEFAULT_TIMEOUT,
    )
    .await
    .unwrap();
    let provider = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let receipt = provider
        .get_transaction_receipt(hash)
        .await
        .unwrap()
        .unwrap();
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
    // Exercise the shared NodeHandle launcher with a keyless follower. Its
    // successful replay proves startup progressed beyond merely opening RPC.
    let node = NodeHandle::start_with_datadir(
        "isolated-env",
        &datadir,
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .expect("node must not read parent deployment files");
    wait_for_safe_prefix_convergence(
        &[&world.node, &node],
        receipt.block_number.unwrap(),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("isolated follower did not replay the settled transfer");
    let follower = ProviderBuilder::new().connect_http(node.l2_rpc_url().parse().unwrap());
    let replayed = follower
        .get_transaction_receipt(hash)
        .await
        .unwrap()
        .unwrap();
    assert!(replayed.status());
    assert_eq!(replayed.block_hash, receipt.block_hash);
    node.assert_no_process_death();
}

#[tokio::test]
async fn anvil_reorg_preserves_wall_clock() {
    let anvil = Anvil::fresh().await.unwrap();
    let provider = ProviderBuilder::new().connect_http(anvil.rpc_url.parse().unwrap());
    anvil.set_interval_mining(0).await.unwrap();
    // Put an actual state change in the replaced suffix, so hash inequality
    // cannot depend on Anvil choosing different timestamps for empty blocks.
    eez_testkit::sign_and_send(
        &anvil.rpc_url,
        ANVIL_KEY_1,
        eez_testkit::DEV_CHAIN_ID,
        0,
        Some(ANVIL_ADDR_3),
        U256::from(1),
        vec![],
        21_000,
    )
    .await
    .unwrap();
    for _ in 0..3 {
        anvil.mine().await.unwrap();
    }
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

    anvil.reorg(3).await.unwrap();
    let replacement = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replacement.header.number, before.header.number);
    assert_ne!(replacement.header.hash, before.header.hash);
    anvil.mine().await.unwrap();
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
    let world = setup_native_history(&[("EEZ_ROLLUP_ID", "999".into())])
        .await
        .unwrap();
    let chain = world.chain();
    let node = &world.node;

    wait_for(Duration::from_mins(1), || async {
        Ok((node.count_signal("eez.node.settlement.bootstrap_not_ready")? > 0).then_some(()))
    })
    .await
    .expect("unregistered rollup did not close composer readiness");

    chain
        .wait_for_l1_blocks(5, Duration::from_secs(30))
        .await
        .unwrap();

    let l2 = ProviderBuilder::new().connect_http(node.l2_rpc_url().parse().unwrap());
    assert_eq!(
        l2.get_block_number().await.unwrap(),
        0,
        "RPC availability must not imply sequencing readiness for an unregistered rollup"
    );
    let snapshot = chain.snapshot().await.unwrap();
    assert_eq!(snapshot.batches_posted, 0);
    assert_eq!(snapshot.executions_performed, 0);
    assert_eq!(snapshot.state_root, world.dep.profile.genesisHash);
    assert_eq!(
        world.proof_signer.streaming_attestations().unwrap(),
        0,
        "unregistered rollup must be rejected before proof attestation",
    );
    node.assert_no_process_death();
}
