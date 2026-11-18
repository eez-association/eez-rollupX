use super::*;
use eez_testkit::{
    ANVIL_KEY_1, ANVIL_KEY_6, TestQuorumConfig, assert_quorum_batch_signatures,
    setup_cross_chain_quorum,
};

async fn exercise(config: TestQuorumConfig) {
    let pricing = crate::settlement_policy_cost::shadow_pricing_policy();
    let settings = crate::settlement_policy_cost::fee_settings(&pricing, &[]);
    let world = setup_cross_chain_quorum(&config, &settings).await.unwrap();
    let quorum = world.network.quorum.as_ref().unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let from_block = l1.get_block_number().await.unwrap() + 1;
    let input = serde_json::json!({
        "from": signer_address(OUTBOUND_USER).unwrap(), "to": world.outbound_proxy,
        "value": "0x0", "data": Bytes::from(IValue::setValueCall { v: U256::from(43) }.abi_encode()),
    });
    let client = reqwest::Client::new();
    let estimate = wait_for(SETTLE_TIMEOUT, || async {
        let response = crate::fee_quotes::quote_rpc(
            &client,
            &world.l2_xchain(),
            "eth_estimateGas",
            serde_json::json!([input]),
        )
        .await?;
        let error = &response["error"]["data"];
        if error["retryable"] == true
            && matches!(
                error["code"].as_str(),
                Some("settlement_reconciliation_pending" | "estimation_context_changed")
            )
        {
            return Ok(None);
        }
        let hex = response["result"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("quorum wallet estimate failed: {response}"))?;
        Ok(Some(u64::from_str_radix(hex.trim_start_matches("0x"), 16)?))
    })
    .await
    .unwrap();
    assert!(
        estimate > 21_000,
        "native wallet estimation must include cross-chain execution"
    );
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
    let cursor = world.node.signal_cursor().unwrap();
    let stalled = config.wrong_profile_member.is_none();
    if stalled {
        quorum
            .pause_member(quorum.signers.len() - 1, true)
            .await
            .unwrap();
    }
    let resume_after_miss = async {
        if stalled && quorum.members.len() == 2 {
            // Wait for a real user-bearing proof failure before releasing B.
            wait_for(SETTLE_TIMEOUT, || async {
                let events = world.node.signals_since(cursor)?;
                Ok(events
                    .iter()
                    .any(|e| {
                        e.name == "eez.composer.prover.range_retained"
                            && e.fields["selected_users"].as_u64().is_some_and(|n| n > 0)
                    })
                    .then_some(()))
            })
            .await
            .unwrap();
            quorum.pause_member(0, false).await.unwrap();
        }
    };
    let (settlement, ()) = tokio::join!(
        run_scenarios(
            &world,
            [Scenario::new("native attestation quorum")
                .inbound(setter_call(world.setter_proxy, 41u64))
                .outbound(setter_call(world.outbound_proxy, 43u64))
                .expect_l2_state(value_read(world.value_l2), 41u64)
                .expect_l1_state(value_read(world.outbound_value), 43u64)
                .expect_settled_fully()],
        ),
        resume_after_miss
    );
    settlement.unwrap();
    if stalled && quorum.members.len() == 3 {
        // Settlement completed with C still suspended.
        quorum.pause_member(1, false).await.unwrap();
    }
    let dispatches = world
        .node
        .signals_since(cursor)
        .unwrap()
        .into_iter()
        .filter(|e| e.name == "eez.composer.bundle.dispatched")
        .collect::<Vec<_>>();
    let attempts = dispatches
        .iter()
        .flat_map(|event| {
            serde_json::from_str::<Vec<u32>>(event.fields["attempt_counts"].as_str().unwrap())
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(
        !attempts.is_empty() && attempts.iter().all(|&n| n == 0),
        "expected zero attempts charged across actual user-bearing dispatches: {attempts:?}"
    );
    let posts =
        assert_quorum_batch_signatures(&world.l1_rpc(), &world.dep, &quorum.members, 2, from_block)
            .await
            .unwrap();
    assert!(
        posts.iter().all(|systems| systems.len() == 2),
        "every L1 post must carry two distinct native profile proofs"
    );
    if let Some(index) = config.wrong_profile_member {
        let ready = world
            .node
            .signals_since(0)
            .unwrap()
            .into_iter()
            .find(|event| event.name == "eez.node.prover.ready")
            .unwrap();
        let ceiling = ready.fields["maximum_blocks"].as_u64().unwrap();
        assert!(ceiling > 0);
        assert_eq!(Some(ceiling), ready.fields["sync_interval_blocks"].as_u64());
        assert!(
            posts
                .iter()
                .all(|systems| !systems.contains(&quorum.members[index + 1].0)),
            "wrong-profile member must be excluded"
        );
        assert_eq!(
            quorum.signers[index].streaming_attestations().unwrap(),
            0,
            "a member with a wrong registered key must not receive Finalize"
        );
    }
    if quorum.members.len() == 2 {
        let height = events_since(
            &world.l2_rpc(),
            world.value_l2,
            IValue::ValueSet::SIGNATURE_HASH,
            0,
        )
        .await
        .unwrap()
        .iter()
        .filter_map(|log| log.block_number)
        .max()
        .expect("settled inbound setter emitted its L2 event");
        let follower = eez_testkit::NodeHandle::start(
            "quorum-follower",
            &eez_testkit::NodeConfig {
                binary: eez_testkit::NodeBinary::Follower,
                genesis_path: Some(&world.cfg.l2_genesis.0),
            },
            &world.partial_follower_env(),
        )
        .await
        .unwrap();
        eez_testkit::wait_for_safe_prefix_convergence(
            &[&world.node, &follower],
            height,
            SETTLE_TIMEOUT,
        )
        .await
        .unwrap();
        assert_eq!(
            l2_value(&follower.l2_rpc_url(), world.value_l2)
                .await
                .unwrap(),
            U256::from(41)
        );
        follower.assert_no_process_death();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_of_three_settle_while_an_admitted_member_is_still_suspended() {
    exercise(TestQuorumConfig {
        additional_keys: vec![ANVIL_KEY_1, ANVIL_KEY_6],
        threshold: 2,
        wrong_profile_member: None,
    })
    .await;
}
