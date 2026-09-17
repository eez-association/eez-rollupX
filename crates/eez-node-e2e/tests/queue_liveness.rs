//! Queue liveness with actual execution, native blobs, ECDSA, and follower replay.

use std::time::Duration;

use alloy_primitives::U256;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::SolCall;
use eez_testkit::{
    ANVIL_KEY_2, ANVIL_KEY_3, ANVIL_KEY_4, IValue, NativeL1Profile, NodeBinary, NodeConfig,
    NodeHandle, l2_balance, onchain_nonce, receipt_ok, setup_cross_chain_with_profile,
    sign_and_send, wait_for, wait_for_safe_prefix_convergence,
};

const TIMEOUT: Duration = Duration::from_secs(180);

async fn escrow_recovery(profile: NativeL1Profile, limit: &str) {
    let env = [
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10000".into()),
        ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
        ("EEZ_MAX_USER_TXS_PER_BUNDLE", limit.into()),
        ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "500000000000".into()),
        (
            "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS",
            "100000000000".into(),
        ),
        ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "1000000001".into()),
        ("RUST_LOG", "info".into()),
    ];
    let world = setup_cross_chain_with_profile(profile, &env).await.unwrap();
    let follower = NodeHandle::start(
        "queue-follower",
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
    let amount = U256::from(100_000_000_000_000_000u64);
    let recipient_before = l2_balance(&world.l1_rpc(), world.withdrawal_recipient)
        .await
        .unwrap();
    let nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap();
    let withdrawal = sign_and_send(
        &world.l2_xchain(),
        ANVIL_KEY_4,
        world.l2_chain_id,
        nonce,
        Some(world.withdrawal_proxy),
        amount,
        Vec::new(),
        600_000,
    )
    .await
    .unwrap();
    wait_for(TIMEOUT, || async {
        let queue: serde_json::Value = l2
            .client()
            .request("eez_getCrossChainQueueStatus", ())
            .await?;
        Ok((queue["blockedReasons"]["escrow"].as_u64().unwrap_or(0) > 0).then_some(()))
    })
    .await
    .unwrap_or_else(|e| {
        panic!(
            "withdrawal was not retained: {e:#}{}",
            world.settlement_diagnostics()
        )
    });

    let independent_nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_3).await.unwrap();
    let independent = sign_and_send(
        &world.l2_xchain(),
        ANVIL_KEY_3,
        world.l2_chain_id,
        independent_nonce,
        Some(world.outbound_proxy),
        U256::ZERO,
        IValue::setValueCall { v: U256::from(73) }.abi_encode(),
        400_000,
    )
    .await
    .unwrap();
    assert!(
        wait_for(TIMEOUT, || async {
            receipt_ok(&world.l2_rpc(), independent).await
        })
        .await
        .unwrap()
    );
    assert!(
        l2.get_transaction_receipt(withdrawal)
            .await
            .unwrap()
            .is_none()
    );

    let deposit_nonce = onchain_nonce(&world.l1_rpc(), ANVIL_KEY_2).await.unwrap();
    let deposit = sign_and_send(
        &world.l1_xchain(),
        ANVIL_KEY_2,
        l1.get_chain_id().await.unwrap(),
        deposit_nonce,
        Some(world.deposit_proxy),
        amount,
        Vec::new(),
        600_000,
    )
    .await
    .unwrap();
    assert!(
        wait_for(TIMEOUT, || async {
            receipt_ok(&world.l1_rpc(), deposit).await
        })
        .await
        .unwrap()
    );
    let receipt = wait_for(TIMEOUT, || async {
        Ok(l2.get_transaction_receipt(withdrawal).await?)
    })
    .await
    .unwrap_or_else(|e| {
        panic!(
            "funded withdrawal did not resume: {e:#}{}",
            world.settlement_diagnostics()
        )
    });
    assert!(receipt.status());
    wait_for_safe_prefix_convergence(
        &[&world.node, &follower],
        receipt.block_number.unwrap(),
        TIMEOUT,
    )
    .await
    .unwrap();
    assert_eq!(
        l2_balance(&world.l1_rpc(), world.withdrawal_recipient)
            .await
            .unwrap(),
        recipient_before + amount
    );
    assert_eq!(
        onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap(),
        nonce + 1
    );
    assert_eq!(
        world.node.log_count_matching(&["source blocked"]).unwrap(),
        0,
        "expected source waits must not flood the error log"
    );
    assert_eq!(
        world
            .node
            .signals_since(0)
            .unwrap()
            .iter()
            .filter(|event| event.name == "eez.held_pool.source_blocked"
                && event.fields["tx_hash"] == withdrawal.to_string())
            .count(),
        1,
        "an unchanged source condition is reported only once"
    );
    for event in world
        .node
        .signals_since(0)
        .unwrap()
        .iter()
        .filter(|e| e.name == "eez.composer.bundle.dispatched")
    {
        let attempts: Vec<u32> =
            serde_json::from_str(event.fields["attempt_counts"].as_str().unwrap()).unwrap();
        assert!(attempts.iter().all(|n| *n == 0));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_escrow_wait_allows_independent_senders_and_later_inbound_funding() {
    escrow_recovery(NativeL1Profile::EthereumOsaka, "1").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_escrow_wait_allows_independent_senders_and_later_inbound_funding() {
    escrow_recovery(NativeL1Profile::GnosisPrague, "auto").await;
}
