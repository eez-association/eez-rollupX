//! Startup and Anvil-only harness checks that do not submit settlement batches.
//!
//! Posting tests use the native L1 fixtures in the other integration targets.
//! Their local execution provider and submission RPC share one canonical chain,
//! which is required by the mandatory exact bundle preflight. The historical
//! Anvil harness deliberately uses a distinct embedded L1 for source simulation
//! and therefore cannot soundly exercise posting.

use std::time::Duration;

use alloy_primitives::U256;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::BlockNumberOrTag;

use eez_testkit::{
    ANVIL_ADDR_3, ANVIL_KEY_1, Harness, NodeConfig, NodeHandle, send_l2_value_transfer_confirmed,
    wait_for, wait_for_latest_height,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_mins(5);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn node_fixture_ignores_parent_dotenv() {
    let harness = Harness::fresh().await.unwrap();
    let env = harness.env().await.unwrap();
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
    let node =
        NodeHandle::start_with_datadir("isolated-env", &datadir, &NodeConfig::default(), &env)
            .await
            .expect("node must not read parent deployment files");
    // RPC opens before the composer binds its DA guard and validates bootstrap.
    // This fresh chain has no history to replay: its first produced block proves
    // sequencing is ready. Do not retry the transfer or bypass the admission gate.
    wait_for_latest_height(&node, 1, DEFAULT_TIMEOUT)
        .await
        .expect("isolated fixture did not become ready to sequence");
    send_l2_value_transfer_confirmed(
        &node.l2_rpc_url(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(1u64),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("node did not become operational with isolated fixture environment");
    node.assert_no_process_death();
}

#[tokio::test]
async fn anvil_reorg_preserves_wall_clock() {
    let harness = Harness::for_reorg().await.unwrap();
    let chain = harness.chain();
    chain.set_interval_mining(0).await.unwrap();
    let provider = ProviderBuilder::new().connect_http(chain.rpc_url().parse().unwrap());
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

    harness.anvil.reorg(3).await.unwrap();
    let replacement = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replacement.header.number, before.header.number);
    assert_ne!(replacement.header.hash, before.header.hash);
    chain.mine().await.unwrap();
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
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let env = harness.env_with_rollup_id(999).await.unwrap();
    let datadir = tempfile::tempdir().unwrap();
    let node = NodeHandle::start_with_datadir(
        "wrong-rollup",
        datadir.path(),
        &NodeConfig::default(),
        &env,
    )
    .await
    .unwrap();

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
    assert_eq!(
        snapshot.state_root,
        harness.l2_genesis_block_hash().unwrap()
    );
    assert_eq!(
        harness.successful_attestations().unwrap(),
        0,
        "unregistered rollup must be rejected before proof attestation",
    );
    node.assert_no_process_death();
}
