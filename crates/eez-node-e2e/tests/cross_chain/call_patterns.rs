//! The pinned combined-revert actor checks exact caught errors and continuation.

use alloy_primitives::{Address, Bytes, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{SolCall, SolError, sol};
use eez_testkit::{
    CrossChainWorld, DEV_CHAIN_ID, SETTLE_TIMEOUT, Scenario, ScenarioCall, TARGET_DEPLOYER,
    call_read, create_cross_chain_proxy, create_l2_cross_chain_proxy, onchain_nonce, repo_root,
    setup_cross_chain, sign_and_send, wait_for,
};

sol! {
    #[sol(rpc)]
    interface Actor {
        function configure(address proxy) external;
        function runCatchRemoteAndNestedRevertsAfterSuccessfulCallbacks() external;
        function runDeepStaticReadCallbacksUnderStaticAndMutableCalls() external;
        function firstRead() external view returns (uint256);
        function lastRead() external view returns (uint256);
        function total() external view returns (uint256);
        function completed() external view returns (uint256);
        function result() external view returns (uint256);
        function failure() external view returns (bytes);
        error FrameFailed(uint256 childResult);
    }
}

async fn deploy_actor(rpc: &str, chain_id: u64) -> Address {
    let path = repo_root().join("contracts/out/CallPatternContracts.sol/CallPatternActor.json");
    let artifact: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let bytecode: Bytes = artifact["bytecode"]["object"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let hash = sign_and_send(
        rpc,
        TARGET_DEPLOYER,
        chain_id,
        onchain_nonce(rpc, TARGET_DEPLOYER).await.unwrap(),
        None,
        U256::ZERO,
        bytecode.to_vec(),
        3_000_000,
    )
    .await
    .unwrap();
    let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());
    let receipt = wait_for(SETTLE_TIMEOUT, || async {
        Ok(provider.get_transaction_receipt(hash).await?)
    })
    .await
    .unwrap();
    assert!(receipt.status(), "pinned actor deployment reverted");
    receipt.contract_address.unwrap()
}

async fn configure(rpc: &str, chain_id: u64, actor: Address, proxy: Address) {
    let hash = sign_and_send(
        rpc,
        TARGET_DEPLOYER,
        chain_id,
        onchain_nonce(rpc, TARGET_DEPLOYER).await.unwrap(),
        Some(actor),
        U256::ZERO,
        Actor::configureCall { proxy }.abi_encode(),
        200_000,
    )
    .await
    .unwrap();
    let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());
    let receipt = wait_for(SETTLE_TIMEOUT, || async {
        Ok(provider.get_transaction_receipt(hash).await?)
    })
    .await
    .unwrap();
    assert!(receipt.status(), "pinned actor configuration reverted");
}

async fn setup_actors() -> (CrossChainWorld, Address, Address) {
    let w = setup_cross_chain().await.unwrap();
    let l1 = w.l1_rpc();
    let l2 = w.l2_rpc();
    let actor_l1 = deploy_actor(&l1, DEV_CHAIN_ID).await;
    let actor_l2 = deploy_actor(&l2, w.l2_chain_id).await;
    let proxy_l1 = create_cross_chain_proxy(
        &l1,
        TARGET_DEPLOYER,
        w.cfg.eez_address,
        actor_l2,
        w.cfg.rollup_id,
    )
    .await
    .unwrap();
    let proxy_l2 = create_l2_cross_chain_proxy(&l2, TARGET_DEPLOYER, actor_l1, 0)
        .await
        .unwrap();
    configure(&l1, DEV_CHAIN_ID, actor_l1, proxy_l1).await;
    configure(&l2, w.l2_chain_id, actor_l2, proxy_l2).await;
    (w, actor_l1, actor_l2)
}

async fn combined_reverts(inbound: bool) {
    let (w, actor_l1, actor_l2) = setup_actors().await;
    let l1 = w.l1_rpc();
    let l2 = w.l2_rpc();
    let source = if inbound { actor_l1 } else { actor_l2 };
    let call = ScenarioCall::new(
        source,
        Actor::runCatchRemoteAndNestedRevertsAfterSuccessfulCallbacksCall {}.abi_encode(),
    )
    .with_gas_limit(1_000_000);
    let scenario = Scenario::new("pinned combined remote and nested reverts");
    let scenario = if inbound {
        scenario.inbound(call)
    } else {
        scenario.outbound(call)
    };
    scenario
        .expect_l1_state(
            call_read(actor_l1, "total", Actor::totalCall {}.abi_encode()),
            if inbound { 0u64 } else { 5u64 },
        )
        .expect_l2_state(
            call_read(actor_l2, "total", Actor::totalCall {}.abi_encode()),
            if inbound { 5u64 } else { 0u64 },
        )
        .expect_l1_state(
            call_read(actor_l1, "completed", Actor::completedCall {}.abi_encode()),
            u64::from(inbound),
        )
        .expect_l2_state(
            call_read(actor_l2, "completed", Actor::completedCall {}.abi_encode()),
            u64::from(!inbound),
        )
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();
    let expected = Actor::FrameFailed {
        childResult: U256::from(2),
    }
    .abi_encode();
    for (rpc, actor) in [(&l1, actor_l1), (&l2, actor_l2)] {
        let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());
        assert_eq!(
            Actor::new(actor, &provider)
                .failure()
                .call()
                .await
                .unwrap()
                .as_ref(),
            expected
        );
    }
    let source_rpc = if inbound { &l1 } else { &l2 };
    let provider = ProviderBuilder::new().connect_http(source_rpc.parse().unwrap());
    assert_eq!(
        Actor::new(source, &provider).result().call().await.unwrap(),
        U256::from(5)
    );
    super::assert_follower_replays(
        &w,
        "combined-revert-follower",
        &call_read(actor_l2, "total", Actor::totalCall {}.abi_encode()),
        U256::from(if inbound { 5 } else { 0 }),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_combined_caught_reverts_preserve_continuation_and_replay() {
    combined_reverts(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_combined_caught_reverts_preserve_continuation_and_replay() {
    combined_reverts(false).await;
}

async fn recursive_static_reads(inbound: bool) {
    let (w, actor_l1, actor_l2) = setup_actors().await;
    let source = if inbound { actor_l1 } else { actor_l2 };
    let call = ScenarioCall::new(
        source,
        Actor::runDeepStaticReadCallbacksUnderStaticAndMutableCallsCall {}.abi_encode(),
    )
    .with_gas_limit(1_000_000);
    let scenario = Scenario::new("recursive static pool followed by a producing call");
    let scenario = if inbound {
        scenario.inbound(call)
    } else {
        scenario.outbound(call)
    };
    scenario
        .expect_l1_state(
            call_read(actor_l1, "total", Actor::totalCall {}.abi_encode()),
            u64::from(!inbound),
        )
        .expect_l2_state(
            call_read(actor_l2, "total", Actor::totalCall {}.abi_encode()),
            u64::from(inbound),
        )
        .expect_l1_state(
            call_read(actor_l1, "completed", Actor::completedCall {}.abi_encode()),
            u64::from(inbound),
        )
        .expect_l2_state(
            call_read(actor_l2, "completed", Actor::completedCall {}.abi_encode()),
            u64::from(!inbound),
        )
        .expect_settled_fully()
        .run(&w)
        .await
        .unwrap();
    let source_rpc = if inbound { w.l1_rpc() } else { w.l2_rpc() };
    let provider = ProviderBuilder::new().connect_http(source_rpc.parse().unwrap());
    let actor = Actor::new(source, &provider);
    assert_eq!(actor.firstRead().call().await.unwrap(), U256::from(3));
    assert_eq!(actor.lastRead().call().await.unwrap(), U256::from(2));
    super::assert_follower_replays(
        &w,
        "recursive-static-follower",
        &call_read(actor_l2, "total", Actor::totalCall {}.abi_encode()),
        U256::from(u64::from(inbound)),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_recursive_static_pool_preserves_manifest_and_replay() {
    recursive_static_reads(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn outbound_recursive_static_pool_preserves_manifest_and_replay() {
    recursive_static_reads(false).await;
}
