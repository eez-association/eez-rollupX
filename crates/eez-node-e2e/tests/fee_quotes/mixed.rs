//! Mixed native preparation, partial inclusion and independent replay.
//! The dev L1 adapter sorts by tip; refund-enabled success needs the real PBS
//! builder covered by Kurtosis's state-chaining and fee suites.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_fee_routes_prepare_together_and_replay_partial_inclusion() {
    mixed_roundtrip(eez_testkit::NativeL1Profile::EthereumOsaka, true, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_both_fee_routes_prepare_together_and_replay_partial_inclusion() {
    mixed_roundtrip(eez_testkit::NativeL1Profile::GnosisPrague, true, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn l2_tariff_can_share_a_post_with_unpriced_inbound_work() {
    mixed_roundtrip(eez_testkit::NativeL1Profile::EthereumOsaka, true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn l1_refund_mixed_post_replays_partial_inclusion() {
    mixed_roundtrip(eez_testkit::NativeL1Profile::EthereumOsaka, false, true).await;
}

async fn mixed_roundtrip(profile: eez_testkit::NativeL1Profile, l2_fees: bool, l1_fees: bool) {
    let policy = crate::settlement_policy_cost::shadow_pricing_policy();
    let settings = crate::settlement_policy_cost::fee_settings(
        &policy,
        &[
            ("EEZ_FEES_L2_TO_L1_ENABLED", l2_fees.to_string()),
            ("EEZ_FEES_L1_TO_L2_ENABLED", l1_fees.to_string()),
            ("EEZ_FEES_L1_REFUND_PERCENT", "90".into()),
            (
                "EEZ_FEES_L1_REFUND_RECIPIENT",
                Address::repeat_byte(0x97).to_string(),
            ),
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
            ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
            // Order unpriced L1 sources after the post in the dev mempool.
            // Priced L1 sources exceed this fixture's poster cap and exercise
            // partial inclusion instead; real PBS preserves bundle order.
            ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "1000000001".into()),
        ],
    );
    let world = eez_testkit::setup_cross_chain_with_profile(profile, &settings)
        .await
        .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let l1_chain = l1.get_chain_id().await.unwrap();
    let client = reqwest::Client::new();
    let sign = |key: &str, chain_id, nonce, to, value, tip| {
        let signer: PrivateKeySigner = key.parse().unwrap();
        let mut tx: TypedTransaction = TxEip1559 {
            chain_id,
            nonce,
            // Keep maximum gas payment below the fixture RPC's 1 ETH cap.
            gas_limit: 400_000,
            max_priority_fee_per_gas: tip,
            max_fee_per_gas: 2_000_000_000_000,
            to: TxKind::Call(to),
            input: IValue::setValueCall {
                v: U256::from(value),
            }
            .abi_encode()
            .into(),
            ..Default::default()
        }
        .into();
        let signature = signer.sign_transaction_sync(&mut tx).unwrap();
        tx.into_envelope(signature)
    };
    let inbound = sign(
        INBOUND_USER,
        l1_chain,
        onchain_nonce(&world.l1_rpc(), INBOUND_USER).await.unwrap(),
        world.setter_proxy,
        456u64,
        if l1_fees {
            1_000_000_000_000
        } else {
            1_000_000_000
        },
    );
    let outbound = sign(
        OUTBOUND_USER,
        world.l2_chain_id,
        onchain_nonce(&world.l2_rpc(), OUTBOUND_USER).await.unwrap(),
        world.outbound_proxy,
        123u64,
        1_000_000_000_000,
    );
    let cursor = world.node.signal_cursor().unwrap();
    // Queue both directions after a new native L1 parent, before the fixture's
    // T-2s composition window. Inbound is older even though execution is outbound first.
    let head = l1.get_block_number().await.unwrap();
    wait_for(TIMEOUT, || async {
        Ok((l1.get_block_number().await? > head).then_some(()))
    })
    .await
    .unwrap();
    for (front, tx) in [
        (&world.l1_xchain(), &inbound),
        (&world.l2_xchain(), &outbound),
    ] {
        let result = quote_rpc(
            &client,
            front,
            "eth_sendRawTransaction",
            json!([Bytes::from(tx.encoded_2718())]),
        )
        .await
        .unwrap();
        assert_eq!(result["result"], json!(tx.tx_hash()), "{result}");
    }
    let outbound_receipt = wait_for(TIMEOUT, || async {
        Ok(l2.get_transaction_receipt(*outbound.tx_hash()).await?)
    })
    .await
    .unwrap();
    let inbound_receipt = wait_for(TIMEOUT, || async {
        Ok(l1.get_transaction_receipt(*inbound.tx_hash()).await?)
    })
    .await
    .unwrap();
    assert!(outbound_receipt.status(), "{outbound_receipt:?}");
    assert_eq!(
        inbound_receipt.status(),
        !l1_fees,
        "outbound={outbound_receipt:?}; inbound={inbound_receipt:?}{}",
        world.settlement_diagnostics()
    );
    assert_eq!(
        l2_value(&world.l1_rpc(), world.outbound_value)
            .await
            .unwrap(),
        U256::from(123)
    );
    let mut planned_hash: Option<alloy_primitives::B256> = None;
    for signal in world.node.signals_since(cursor).unwrap() {
        if signal.name != "eez.composer.admission.v1" {
            continue;
        }
        let event: Value =
            serde_json::from_str(signal.fields["admission_event"].as_str().unwrap()).unwrap();
        let detail = &event["detail"];
        let observed = &detail["fee_observation"];
        if event["kind"] != "preflight" || observed["l2"]["source_count"] != 1 {
            continue;
        }
        let post_hash = observed["l1"]["transaction_hash"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let Some(post) = l1.get_transaction_receipt(post_hash).await.unwrap() else {
            continue;
        };
        assert!(post.status());
        assert_eq!(
            observed["l1"]["companion_count"],
            1,
            "both fee routes must share one native post: {event}{}",
            world.settlement_diagnostics()
        );
        assert_eq!(post.block_hash, inbound_receipt.block_hash);
        if l1_fees {
            assert!(
                inbound_receipt.transaction_index < post.transaction_index,
                "the dev mempool must expose source-before-post partial inclusion"
            );
        } else {
            assert!(post.transaction_index < inbound_receipt.transaction_index);
        }
        if l2_fees {
            assert_eq!(detail["l2_tariff"]["scope"], "standalone_outbound");
            let amount = |value: &Value| U256::from_str_radix(value.as_str().unwrap(), 10).unwrap();
            assert!(
                amount(&observed["l2"]["gross_priority_fees"])
                    >= amount(&detail["l2_tariff"]["minimum_l2_payment"])
            );
            assert!(
                amount(&detail["l2_tariff"]["complete_post_cost"])
                    > amount(&detail["l2_tariff"]["l1_post_cost"])
            );
        } else {
            assert!(detail["l2_tariff"].is_null());
        }
        if l1_fees {
            assert_eq!(detail["refund"]["source"], json!(inbound.tx_hash()));
        } else {
            assert!(detail["refund"].is_null());
        }
        planned_hash = Some(
            observed["l2"]["block_hash"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
        );
    }
    assert!(
        planned_hash.is_some(),
        "native mixed preflight and canonical post receipt required{}",
        world.settlement_diagnostics()
    );
    let follower = NodeHandle::start(
        "mixed-fee-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    wait_for_safe_prefix_convergence(
        &[&world.node, &follower],
        outbound_receipt.block_number.unwrap(),
        TIMEOUT,
    )
    .await
    .unwrap();
    let expected_inbound = U256::from(if l1_fees { 5 } else { 456 });
    assert_eq!(
        l2_value(&world.l2_rpc(), world.value_l2).await.unwrap(),
        expected_inbound,
        "only canonically consumed inbound effects may remain"
    );
    assert_eq!(
        l2_value(&follower.l2_rpc_url(), world.value_l2)
            .await
            .unwrap(),
        expected_inbound
    );
    // Partial inclusion replaces the speculative mixed block with the exact
    // consumed prefix; compare the follower against the canonical receipt.
    let canonical = l2
        .get_transaction_receipt(*outbound.tx_hash())
        .await
        .unwrap()
        .unwrap();
    assert!(canonical.status());
    if l1_fees {
        assert_ne!(canonical.block_hash, planned_hash);
    } else {
        assert_eq!(canonical.block_hash, planned_hash);
    }
    let replayed = ProviderBuilder::new()
        .connect_http(follower.l2_rpc_url().parse().unwrap())
        .get_transaction_receipt(*outbound.tx_hash())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replayed.block_hash, canonical.block_hash);
    assert_eq!(replayed.gas_used, canonical.gas_used);
    world.node.assert_no_divergence_failure_logs();
    follower.assert_no_divergence_failure_logs();
}
