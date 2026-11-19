//! Inbound advisory and signed admission retain a fitting source while ordinary
//! history needs several real posts. The native mempool fixture has no PBS refunds.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_survives_history_drain_reorg_and_replays() {
    roundtrip(Scenario::LegacyReorg).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eip1559_is_held_through_multiple_history_posts_and_replays() {
    roundtrip(Scenario::Eip1559).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn externally_spent_nonce_during_history_drain_has_terminal_status() {
    roundtrip(Scenario::NonceConsumed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_eip1559_is_held_through_multiple_history_posts_and_replays() {
    roundtrip(Scenario::Gnosis).await;
}

#[derive(Debug)]
enum Scenario {
    Eip1559,
    Gnosis,
    LegacyReorg,
    NonceConsumed,
}

async fn roundtrip(scenario: Scenario) {
    let legacy = matches!(scenario, Scenario::LegacyReorg);
    let spend_nonce = matches!(scenario, Scenario::NonceConsumed);
    let mut policy = crate::settlement_policy_cost::shadow_pricing_policy();
    policy["risk_bps"] = json!(0);
    policy["profit_bps"] = json!(2000);
    for key in ["proving_cost_l1", "operations_cost_l1", "treasury_cost_l1"] {
        policy[key] = json!("0");
    }
    let settings = crate::settlement_policy_cost::fee_settings(
        &policy,
        &[
            ("EEZ_FEES_L1_TO_L2_ENABLED", "true".into()),
            ("EEZ_FEES_L1_REFUND_PERCENT", "90".into()),
            (
                "EEZ_FEES_L1_REFUND_RECIPIENT",
                Address::repeat_byte(0x97).to_string(),
            ),
            ("EEZ_L1_POSTBATCH_PRIORITY_FEE", "1000000000".into()),
            ("EEZ_MAX_BLOBS_PER_BATCH", "1".into()),
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
            ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
            ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10000".into()),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
        ],
    );
    let mut world = if legacy {
        eez_testkit::setup_cross_chain_with_l1_reorg(&settings).await
    } else if matches!(scenario, Scenario::Gnosis) {
        eez_testkit::setup_cross_chain_with_profile(
            eez_testkit::NativeL1Profile::GnosisPrague,
            &settings,
        )
        .await
    } else {
        setup_cross_chain_with_env(&settings).await
    }
    .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let chain = l1.get_chain_id().await.unwrap();
    let inner =
        deploy_nested_setter_inner(&world.l1_rpc(), TARGET_DEPLOYER, chain, world.setter_proxy)
            .await
            .unwrap();
    let outer = deploy_nested_setter_outer(&world.l1_rpc(), TARGET_DEPLOYER, chain, inner)
        .await
        .unwrap();
    let client = reqwest::Client::new();
    wait_for_l1_quote_parent(&world, &client, l1.get_block_number().await.unwrap()).await;
    for _ in 0..5 {
        let hash = sign_and_send(
            &world.l2_rpc(),
            TARGET_DEPLOYER,
            world.l2_chain_id,
            onchain_nonce(&world.l2_rpc(), TARGET_DEPLOYER)
                .await
                .unwrap(),
            Some(Address::repeat_byte(0x93)),
            U256::ZERO,
            vec![0x5a; 60_000],
            3_000_000,
        )
        .await
        .unwrap();
        assert!(
            wait_for(TIMEOUT, || async {
                Ok(l2.get_transaction_receipt(hash).await?)
            })
            .await
            .unwrap()
            .status()
        );
    }
    let signer: PrivateKeySigner = INBOUND_USER.parse().unwrap();
    let nonce = onchain_nonce(&world.l1_rpc(), INBOUND_USER).await.unwrap();
    let mut input = INestedSetterOuter::setViaInnerCall { v: U256::from(781) }.abi_encode();
    // Valid trailing calldata adds source gas without enlarging the nested call.
    // This makes the fixture's priority sorter put the post before the source;
    // receipt ordering below is asserted, not treated as PBS/refund coverage.
    input.resize(60_000, 0x5a);
    let input = Bytes::from(input);
    let quote = wait_for(TIMEOUT, || async {
        let response = quote_rpc(&client, &world.l1_xchain(), "eez_debugQuoteFee",
            json!([{"from":signer.address(),"to":outer,"nonce":format!("{nonce:#x}"),"data":input}])).await?;
        Ok((!quote_retry(&response)).then_some(response))
    }).await.unwrap();
    tracing::save_trace(&format!("inbound-backlog-{scenario:?}"), &quote);
    assert!(quote.get("error").is_none(), "{quote}");
    for option in quote["result"]["options"].as_array().unwrap() {
        assert!(
            option.get("error").is_none(),
            "inbound backlog quote: {quote}"
        );
    }
    let quote = &quote["result"];
    for detail in quote["diagnostics"]["options"].as_array().unwrap() {
        assert_eq!(detail["estimate"]["historyDeferred"], true, "{quote}");
        assert_eq!(detail["estimate"]["blobs"], 1);
        assert!(
            detail["estimate"]["daBytes"].as_u64().unwrap()
                > 2 * eez_blob_codec::DATA_BYTES_PER_BLOB as u64
        );
    }
    assert_eq!(
        onchain_nonce(&world.l1_rpc(), INBOUND_USER).await.unwrap(),
        nonce
    );
    let option = &quote["options"][usize::from(!legacy)];
    let quantity = |key: &str| {
        u128::from_str_radix(option[key].as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
    };
    let cap = quantity(if legacy { "gasPrice" } else { "maxFeePerGas" });
    let tip = if legacy {
        cap
    } else {
        quantity("maxPriorityFeePerGas")
    };
    let gas = quantity("gas") as u64;
    let sign = |tip, cap| {
        let mut tx: TypedTransaction = if legacy {
            TxLegacy {
                chain_id: Some(chain),
                nonce,
                gas_limit: gas,
                gas_price: cap,
                to: TxKind::Call(outer),
                input: input.clone(),
                ..Default::default()
            }
            .into()
        } else {
            TxEip1559 {
                chain_id: chain,
                nonce,
                gas_limit: gas,
                max_priority_fee_per_gas: tip,
                max_fee_per_gas: cap,
                to: TxKind::Call(outer),
                input: input.clone(),
                ..Default::default()
            }
            .into()
        };
        let signature = signer.sign_transaction_sync(&mut tx).unwrap();
        tx.into_envelope(signature)
    };
    let base: u128 =
        quote["diagnostics"]["options"][usize::from(!legacy)]["estimate"]["l1BaseFeePerGas"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
    let underpaid = sign(0, if legacy { base + 1 } else { cap });
    let rejection = quote_rpc(
        &client,
        &world.l1_xchain(),
        "eth_sendRawTransaction",
        json!([Bytes::from(underpaid.encoded_2718())]),
    )
    .await
    .unwrap();
    assert_eq!(
        rejection["error"]["data"]["code"], "l1_tip_underfunded",
        "{rejection}"
    );
    let source = sign(tip, cap);
    let hash = *source.tx_hash();
    let cursor = world.node.signal_cursor().unwrap();
    let batches_before = world.chain().snapshot().await.unwrap().batches_posted;
    let ancestor = l1
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header;
    let initial_value = l2_value(&world.l2_rpc(), world.value_l2).await.unwrap();
    if spend_nonce {
        // Block recompositions too, so no later post can race external nonce consumption.
        world.pause_bundles_after(1).unwrap();
    } else if legacy {
        world.hold_bundle_after(1).unwrap();
    }
    let admitted = quote_rpc(
        &client,
        &world.l1_xchain(),
        "eth_sendRawTransaction",
        json!([Bytes::from(source.encoded_2718())]),
    )
    .await
    .unwrap();
    assert_eq!(admitted["result"], json!(hash), "{admitted}");
    if legacy || spend_nonce {
        let mut held = wait_for(TIMEOUT, || async {
            if spend_nonce {
                world.paused_bundle()
            } else {
                world.held_bundle()
            }
        })
        .await
        .unwrap();
        assert!(world.chain().snapshot().await.unwrap().batches_posted > batches_before);
        assert!(l1.get_transaction_receipt(hash).await.unwrap().is_none());
        if spend_nonce {
            let pending = quote_rpc(
                &client,
                &world.l1_xchain(),
                "eez_getCrossChainTransaction",
                json!([hash]),
            )
            .await
            .unwrap();
            assert!(pending.get("error").is_none(), "{pending}");
            assert!(
                matches!(
                    pending["result"]["lifecycle"].as_str(),
                    Some("queued" | "in_flight" | "composing")
                ),
                "{pending}"
            );
            assert!(pending["result"]["ownerGeneration"].is_null(), "{pending}");
            let conflicting = sign_and_send(
                &world.l1_rpc(),
                INBOUND_USER,
                chain,
                nonce,
                Some(signer.address()),
                U256::ZERO,
                Vec::new(),
                21_000,
            )
            .await
            .unwrap();
            assert_ne!(conflicting, hash);
            let mined = wait_for(TIMEOUT, || async {
                Ok(l1.get_transaction_receipt(conflicting).await?)
            })
            .await
            .unwrap();
            assert!(mined.status());
            assert_eq!(
                onchain_nonce(&world.l1_rpc(), INBOUND_USER).await.unwrap(),
                nonce + 1
            );
            wait_for_l1_quote_parent(&world, &client, mined.block_number.unwrap()).await;
            world.resume_bundles().unwrap();
            let status = wait_for(TIMEOUT, || async {
                let response = quote_rpc(
                    &client,
                    &world.l1_xchain(),
                    "eez_getCrossChainTransaction",
                    json!([hash]),
                )
                .await?;
                Ok((response.get("error").is_some()
                    || response["result"].is_null()
                    || response["result"]["lifecycle"] == "terminal")
                    .then_some(response))
            })
            .await
            .unwrap();
            tracing::save_trace(
                "nonce-consumed-status",
                &json!({
                    "source": hash, "conflicting": conflicting, "consumingBlock": mined.block_hash,
                    "before": pending, "after": status,
                }),
            );
            assert!(l1.get_transaction_receipt(hash).await.unwrap().is_none());
            assert_eq!(
                l2_value(&world.l2_rpc(), world.value_l2).await.unwrap(),
                initial_value
            );
            assert!(status.get("error").is_none(), "{status}");
            assert!(
                !status["result"].is_null(),
                "accepted source disappeared after nonce consumption: {status}"
            );
            assert_eq!(
                status["result"]["ownership"]["reason"], "PoolDiscarded",
                "{status}"
            );
            assert!(
                status["result"]["ownership"]["canonical"].is_null(),
                "{status}"
            );
            assert!(status["result"]["ownerGeneration"].is_null(), "{status}");
            assert!(
                status["result"]["ownership"]["observedAtMs"]
                    .as_u64()
                    .is_some(),
                "{status}"
            );
            return;
        }
        world
            .l1_reorg_control
            .as_mut()
            .unwrap()
            .reorg_to(ancestor.number, ancestor.hash)
            .await
            .unwrap();
        // The old proof is excluded, not forwarded onto the replacement fork.
        held["params"][0]["txs"] = json!([]);
        world.release_bundle(&held).unwrap();
        world.node.wait_for_reorg_seen(TIMEOUT).await.unwrap();
    }
    let receipt = wait_for(TIMEOUT, || async {
        if let Some(receipt) = l1.get_transaction_receipt(hash).await? {
            return Ok(Some(receipt));
        }
        let status = quote_rpc(
            &client,
            &world.l1_xchain(),
            "eez_getCrossChainTransaction",
            json!([hash]),
        )
        .await?;
        assert!(
            status.get("error").is_none()
                && (matches!(
                    status["result"]["lifecycle"].as_str(),
                    Some("queued" | "in_flight" | "composing" | "awaiting_evidence")
                ) || status["result"]["ownership"]["reason"] == "Committed"),
            "accepted source lost while history drains: {status}"
        );
        Ok(None)
    })
    .await
    .unwrap();
    assert!(
        receipt.status(),
        "source failed after history drain: {receipt:?}{}",
        world.settlement_diagnostics()
    );
    let events = world.node.signals_since(cursor).unwrap();
    if legacy {
        assert!(
            events.iter().any(|s| {
                s.name == "eez.deriver.l1.reorg.retreated" && s.u64("dropped_batches").unwrap() > 0
            }),
            "the fork must remove a confirmed historical post"
        );
    } else {
        // A rollback can discard speculative history and change the post count.
        assert!(
            world.chain().snapshot().await.unwrap().batches_posted >= batches_before + 3,
            "the canonical chain must include multiple history posts and the source post"
        );
    }
    assert!(
        events
            .iter()
            .filter(|s| s.name == "eez.composer.emission.historical_chunk")
            .count()
            >= 2,
        "expected multiple historical drain emissions before the source{}",
        world.settlement_diagnostics()
    );
    let block = l1
        .get_block_by_hash(receipt.block_hash.unwrap())
        .full()
        .await
        .unwrap()
        .unwrap();
    let transactions = block.transactions.as_transactions().unwrap();
    let post_index = transactions
        .iter()
        .position(|tx| tx.inner.to() == Some(world.post_batch_target))
        .unwrap();
    assert!((post_index as u64) < receipt.transaction_index.unwrap());
    assert_eq!(
        transactions[post_index]
            .inner
            .blob_versioned_hashes()
            .unwrap()
            .len(),
        1
    );
    let follower = NodeHandle::start(
        "inbound-backlog-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    wait_for(TIMEOUT, || async {
        Ok(
            (l2_value(&world.l2_rpc(), world.value_l2).await? == U256::from(781)
                && l2_value(&follower.l2_rpc_url(), world.value_l2).await? == U256::from(781))
            .then_some(()),
        )
    })
    .await
    .unwrap();
    let safe = l2
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Safe)
        .await
        .unwrap()
        .unwrap()
        .header
        .number;
    wait_for_safe_prefix_convergence(&[&world.node, &follower], safe, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        l2_value(&follower.l2_rpc_url(), world.value_l2)
            .await
            .unwrap(),
        U256::from(781)
    );
}
