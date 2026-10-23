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
    use alloy_primitives::Bytes;
    use alloy_rpc_types_eth::TransactionRequest;
    use alloy_sol_types::SolCall as _;
    use eez_protocol::l1_history::{HISTORY_ADDRESS, IL1BlockhashContract};

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
    let parent = wait_for(DEFAULT_TIMEOUT, || async {
        let parent = l2
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await?
            .unwrap()
            .header;
        if parent.number % 5 != 2 {
            return Ok(None);
        }
        let pending = l2.call(request.clone()).pending().await;
        let after = l2
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await?
            .unwrap()
            .header;
        if after.hash != parent.hash {
            return Ok(None);
        }
        // This specific error is emitted only after the source evidence has
        // resolved. Missing evidence or another RPC error does not pass.
        Ok(pending
            .err()
            .filter(|error| {
                error
                    .to_string()
                    .contains("pending L1 history import has not executed")
            })
            .map(|_| parent))
    })
    .await
    .expect("pending call did not reach the scheduled-import gate");
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
