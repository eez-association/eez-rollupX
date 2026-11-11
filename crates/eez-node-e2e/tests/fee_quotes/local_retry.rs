use super::*;

alloy_sol_types::sol! {
    interface LocalRetry {
        function incrementWithLocalRetry(uint256 expected) external;
        function caught() external view returns (bool);
        function lastNewValue() external view returns (uint256);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_local_revert_retry_settles_once_and_replays() {
    // The local development builder does not implement PBS refunds. Exercise
    // actual settlement separately under the supported fee-disabled policy.
    let world = eez_testkit::setup_cross_chain().await.unwrap();
    let follower = NodeHandle::start(
        "local-retry-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let replay = ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap());
    let hash = sign_and_send(
        &world.l1_xchain(),
        INBOUND_USER,
        l1.get_chain_id().await.unwrap(),
        onchain_nonce(&world.l1_rpc(), INBOUND_USER).await.unwrap(),
        Some(world.inbound_wrapper),
        U256::ZERO,
        LocalRetry::incrementWithLocalRetryCall {
            expected: U256::from(6),
        }
        .abi_encode(),
        1_000_000,
    )
    .await
    .unwrap();
    let receipt = wait_for(TIMEOUT, || async {
        Ok(l1.get_transaction_receipt(hash).await?)
    })
    .await
    .unwrap();
    assert!(
        receipt.status(),
        "source reverted{}",
        world.settlement_diagnostics()
    );
    let client = reqwest::Client::new();
    for (data, expected) in [
        (LocalRetry::caughtCall {}.abi_encode(), U256::from(1)),
        (LocalRetry::lastNewValueCall {}.abi_encode(), U256::from(6)),
    ] {
        let result = quote_rpc(
            &client,
            &world.l1_rpc(),
            "eth_call",
            json!([{"to":world.inbound_wrapper,"data":Bytes::from(data)},"latest"]),
        )
        .await
        .unwrap();
        assert_eq!(
            result["result"].as_str().unwrap().parse::<U256>().unwrap(),
            expected
        );
    }
    wait_for(TIMEOUT, || async {
        Ok(
            (l2_value(&follower.l2_rpc_url(), world.value_l2).await? == U256::from(6))
                .then_some(()),
        )
    })
    .await
    .unwrap();
    let height = replay.get_block_number().await.unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], height, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        l2_value(&world.l2_rpc(), world.value_l2).await.unwrap(),
        U256::from(6)
    );
    assert_eq!(
        l2_value(&follower.l2_rpc_url(), world.value_l2)
            .await
            .unwrap(),
        U256::from(6)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inbound_local_revert_retry_can_be_quoted() {
    let policy = crate::settlement_policy_cost::shadow_pricing_policy();
    let settings = crate::settlement_policy_cost::fee_settings(
        &policy,
        &[
            ("EEZ_FEES_L1_TO_L2_ENABLED", "true".into()),
            ("EEZ_FEES_L1_REFUND_PERCENT", "90".into()),
            (
                "EEZ_FEES_L1_REFUND_RECIPIENT",
                Address::repeat_byte(0x97).to_string(),
            ),
        ],
    );
    let world = setup_cross_chain_with_env(&settings).await.unwrap();
    let client = reqwest::Client::new();
    let source: PrivateKeySigner = INBOUND_USER.parse().unwrap();
    // The shared fixture's counter starts at five. Both attempts must return
    // six, and only the second attempt may survive its enclosing L1 frame.
    let data = Bytes::from(
        LocalRetry::incrementWithLocalRetryCall {
            expected: U256::from(6),
        }
        .abi_encode(),
    );
    let response = wait_for(TIMEOUT, || async {
        let response = quote_rpc(
            &client,
            &world.l1_xchain(),
            "eez_debugQuoteFee",
            json!([{"from":source.address(),"to":world.inbound_wrapper,"data":data},
                {"trace":"calls"}]),
        )
        .await?;
        Ok((!quote_retry(&response)).then_some(response))
    })
    .await
    .unwrap();
    super::tracing::save_trace("inbound-local-retry", &response);
    let options = response["result"]["options"]
        .as_array()
        .expect("quote options");
    assert_eq!(options.len(), 2, "{response}");
    for option in options {
        assert!(option.get("gas").is_some(), "{response}");
    }
}
