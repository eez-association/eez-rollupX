//! Exercise the decoded calldata boundary through real tariff admission,
//! multi-blob settlement, and independent follower reconstruction.
use super::*;
use alloy_primitives::keccak256;

alloy_sol_types::sol! {
    interface CalldataValue {
        function setCalldataHash() external returns (uint256);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_64_kib_boundary_settles_and_replays() {
    calldata_roundtrip(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn eip1559_64_kib_boundary_settles_and_replays() {
    calldata_roundtrip(false).await;
}

async fn calldata_roundtrip(legacy: bool) {
    let policy = crate::settlement_policy_cost::shadow_pricing_policy();
    let world = setup_cross_chain_with_env(&crate::settlement_policy_cost::fee_settings(
        &policy,
        &[
            ("EEZ_FEES_L2_TO_L1_ENABLED", "true".into()),
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
            ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
            ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10000".into()),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
        ],
    ))
    .await
    .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let signer: PrivateKeySigner = OUTBOUND_USER.parse().unwrap();
    let client = reqwest::Client::new();
    let ty = if legacy { "0x0" } else { "0x2" };
    let follower = NodeHandle::start(
        "calldata-boundary-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let replay = ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap());

    // Reject first, then settle the same nonce with valid input. All bytes are
    // nonzero after the selector, exercising the higher calldata gas cost.
    for length in [65_537, 13_000, 65_535, 65_536] {
        let mut data = CalldataValue::setCalldataHashCall {}.abi_encode();
        data.resize(length, 0xff);
        let data = Bytes::from(data);
        let nonce = onchain_nonce(&world.l2_rpc(), OUTBOUND_USER).await.unwrap();
        let input = json!({
            "type":ty, "chainId":format!("{:#x}", world.l2_chain_id),
            "from":signer.address(), "to":world.outbound_proxy,
            "nonce":format!("{nonce:#x}"), "gas":"0x1000000", "value":"0x0", "data":data
        });
        let mut quoted = Value::Null;
        let mut estimated_blobs = 0;
        for method in ["eez_quoteFee", "eez_debugQuoteFee"] {
            let response = wait_for(TIMEOUT, || async {
                let response =
                    quote_rpc(&client, &world.l2_xchain(), method, json!([input])).await?;
                Ok((!quote_retry(&response)).then_some(response))
            })
            .await
            .unwrap();
            if length > 65_536 {
                assert_eq!(response["error"]["code"], -32602, "{response}");
                assert_eq!(response["error"]["data"]["code"], "invalid_transaction");
                assert_eq!(
                    response["error"]["data"]["message"],
                    "invalid transaction: calldata must be hex and at most 64 KiB"
                );
                continue;
            }
            assert!(response.get("error").is_none(), "{response}");
            let options = response["result"]["options"].as_array().unwrap();
            assert_eq!(options.len(), 1);
            let option = &options[0];
            assert!(option.get("error").is_none(), "{option}");
            assert_eq!(option["type"], ty);
            assert_eq!(option["gas"], input["gas"]);
            if method == "eez_quoteFee" {
                quoted = option.clone();
            } else {
                let diagnostic = &response["result"]["diagnostics"]["options"][0];
                assert_eq!(diagnostic["request"]["data"], json!(data));
                estimated_blobs = diagnostic["estimate"]["blobs"].as_u64().unwrap();
                if length >= 65_535 {
                    // DA carries the signed source and the forwarded call.
                    assert!(estimated_blobs >= 2, "{diagnostic}");
                }
            }
        }
        let (tip, cap) = if length > 65_536 {
            (1_000_000_000, 10_000_000_000)
        } else {
            let quantity = |name: &str| {
                u128::from_str_radix(quoted[name].as_str().unwrap().trim_start_matches("0x"), 16)
                    .unwrap()
            };
            let cap = quantity(if legacy { "gasPrice" } else { "maxFeePerGas" });
            (
                if legacy {
                    cap
                } else {
                    quantity("maxPriorityFeePerGas")
                },
                cap,
            )
        };
        let mut tx: TypedTransaction = if legacy {
            TxLegacy {
                chain_id: Some(world.l2_chain_id),
                nonce,
                gas_limit: 16_777_216,
                gas_price: cap,
                to: TxKind::Call(world.outbound_proxy),
                input: data.clone(),
                ..Default::default()
            }
            .into()
        } else {
            TxEip1559 {
                chain_id: world.l2_chain_id,
                nonce,
                gas_limit: 16_777_216,
                max_priority_fee_per_gas: tip,
                max_fee_per_gas: cap,
                to: TxKind::Call(world.outbound_proxy),
                input: data.clone(),
                ..Default::default()
            }
            .into()
        };
        let signature = signer.sign_transaction_sync(&mut tx).unwrap();
        let signed = tx.into_envelope(signature);
        let response = wait_for(TIMEOUT, || async {
            let response = quote_rpc(
                &client,
                &world.l2_xchain(),
                "eth_sendRawTransaction",
                json!([Bytes::from(signed.encoded_2718())]),
            )
            .await?;
            Ok((!quote_retry(&response)).then_some(response))
        })
        .await
        .unwrap();
        if length > 65_536 {
            assert_eq!(response["error"]["code"], -32003, "{response}");
            assert_eq!(response["error"]["data"]["code"], "invalid_transaction");
            assert_eq!(
                response["error"]["data"]["message"],
                "invalid transaction: calldata must be hex and at most 64 KiB"
            );
            let status = quote_rpc(
                &client,
                &world.l2_xchain(),
                "eez_getCrossChainTransaction",
                json!([signed.tx_hash()]),
            )
            .await
            .unwrap();
            assert!(status["error"].is_null(), "{status}");
            assert!(
                status["result"].is_null(),
                "rejected source was queued: {status}"
            );
            assert_eq!(
                onchain_nonce(&world.l2_rpc(), OUTBOUND_USER).await.unwrap(),
                nonce
            );
            continue;
        }
        assert_eq!(response["result"], json!(signed.tx_hash()), "{response}");
        let hash = *signed.tx_hash();
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l2.get_transaction_receipt(hash).await?)
        })
        .await
        .unwrap();
        assert!(receipt.status());
        let expected = U256::from_be_bytes(keccak256(&data).0);
        wait_for(TIMEOUT, || async {
            Ok((l2_value(&world.l1_rpc(), world.outbound_value).await? == expected).then_some(()))
        })
        .await
        .unwrap();
        wait_for_safe_prefix_convergence(
            &[&world.node, &follower],
            receipt.block_number.unwrap(),
            TIMEOUT,
        )
        .await
        .unwrap();
        let replayed = replay.get_transaction_receipt(hash).await.unwrap().unwrap();
        assert!(replayed.status());
        assert_eq!(replayed.block_hash, receipt.block_hash);
        assert_eq!(replayed.gas_used, receipt.gas_used);
        let replayed_source = replay.get_transaction_by_hash(hash).await.unwrap().unwrap();
        assert_eq!(replayed_source.input(), &data);

        let mut checked_post = false;
        for signal in world
            .node
            .signals_since(0)
            .unwrap()
            .into_iter()
            .filter(|s| s.name == "eez.composer.admission.v1")
        {
            let event: Value =
                serde_json::from_str(signal.fields["admission_event"].as_str().unwrap()).unwrap();
            let observation = &event["detail"]["fee_observation"];
            if event["kind"] != "preflight"
                || observation["l2"]["block_hash"] != json!(receipt.block_hash.unwrap())
            {
                continue;
            }
            let post_hash = observation["l1"]["transaction_hash"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let Some(post) = l1.get_transaction_receipt(post_hash).await.unwrap() else {
                continue;
            };
            assert!(post.status());
            assert_eq!(
                post.blob_gas_used.unwrap(),
                estimated_blobs * alloy_eips::eip4844::DATA_GAS_PER_BLOB
            );
            checked_post = true;
        }
        assert!(
            checked_post,
            "canonical settlement receipt required for {length} bytes"
        );
    }
}
