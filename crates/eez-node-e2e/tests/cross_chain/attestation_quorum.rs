use super::*;
use alloy_network::TransactionBuilder;
use alloy_rpc_types_eth::TransactionRequest;
use eez_testkit::{
    ANVIL_KEY_1, ANVIL_KEY_6, IValue, TestQuorumConfig, assert_latest_batch_signatures,
    setup_cross_chain_quorum, value_read,
};

async fn exercise(config: TestQuorumConfig) {
    let pricing = crate::settlement_policy_cost::shadow_pricing_policy();
    let settings = crate::settlement_policy_cost::fee_settings(&pricing, &[]);
    let world = setup_cross_chain_quorum(&config, &settings).await.unwrap();
    let quorum = world.network.quorum.as_ref().unwrap();
    let front = ProviderBuilder::new().connect_http(world.l2_xchain().parse().unwrap());
    let estimate = front
        .estimate_gas(
            TransactionRequest::default()
                .with_from(signer_address(OUTBOUND_USER).unwrap())
                .with_to(world.outbound_proxy)
                .with_input(IValue::setValueCall { v: U256::from(43) }.abi_encode()),
        )
        .await
        .unwrap();
    assert!(
        estimate > 21_000,
        "native wallet estimation must include cross-chain execution"
    );
    let input = serde_json::json!({
        "from": signer_address(OUTBOUND_USER).unwrap(), "to": world.outbound_proxy,
        "value": "0x0", "data": Bytes::from(IValue::setValueCall { v: U256::from(43) }.abi_encode()),
    });
    let client = reqwest::Client::new();
    let quote = wait_for(SETTLE_TIMEOUT, || async {
        let response = crate::fee_quotes::quote_rpc(
            &client,
            &world.l2_xchain(),
            "eez_quoteFee",
            serde_json::json!([input.clone()]),
        )
        .await?;
        if crate::fee_quotes::quote_retry(&response) {
            return Ok(None);
        }
        anyhow::ensure!(
            response.get("result").is_some(),
            "quorum fee quote failed: {response}"
        );
        Ok(Some(response))
    })
    .await
    .unwrap();
    assert!(
        quote["result"].is_object(),
        "fee quote must execute against the usable member set"
    );
    run_scenarios(
        &world,
        [Scenario::new("native attestation quorum")
            .inbound(setter_call(world.setter_proxy, 41u64))
            .outbound(setter_call(world.outbound_proxy, 43u64))
            .expect_l2_state(value_read(world.value_l2), 41u64)
            .expect_l1_state(value_read(world.outbound_value), 43u64)
            .expect_settled_fully()],
    )
    .await
    .unwrap();
    let systems = assert_latest_batch_signatures(&world.l1_rpc(), &world.dep, &quorum.members, 2)
        .await
        .unwrap();
    assert_eq!(
        systems.len(),
        2,
        "L1 must accept two distinct native profile proofs"
    );
    if let Some(index) = config.wrong_profile_member {
        assert!(
            !systems.contains(&quorum.members[index + 1].0),
            "wrong-profile member must be excluded"
        );
        assert_eq!(
            quorum.signers[index].streaming_attestations().unwrap(),
            0,
            "a member with a wrong registered key must not receive Finalize"
        );
    }
    world.node.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_of_two_native_provers_settle_both_directions_on_l1() {
    exercise(TestQuorumConfig {
        additional_keys: vec![ANVIL_KEY_1],
        threshold: 2,
        wrong_profile_member: None,
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_of_three_native_provers_keep_estimation_and_settlement_live_with_a_wrong_profile() {
    exercise(TestQuorumConfig {
        additional_keys: vec![ANVIL_KEY_1, ANVIL_KEY_6],
        threshold: 2,
        wrong_profile_member: Some(1),
    })
    .await;
}
