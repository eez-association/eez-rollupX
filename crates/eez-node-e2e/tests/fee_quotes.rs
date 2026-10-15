//! Unsigned application quote -> one signed source -> native settlement and
//! independent follower replay. No quoted bytes are submitted before signing.

use alloy_consensus::{TxEip1559, TxLegacy, Typed2718, TypedTransaction};
use alloy_eips::Encodable2718;
use alloy_network::TxSignerSync;
use alloy_primitives::{Address, Bytes, TxKind, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::SolCall;
use eez_testkit::{
    INBOUND_USER, INestedSetterInner, INestedSetterOuter, IValue, NodeBinary, NodeConfig,
    NodeHandle, OUTBOUND_USER, TARGET_DEPLOYER, deploy_nested_setter_inner,
    deploy_nested_setter_outer, l2_value, onchain_nonce, setup_cross_chain_with_env, sign_and_send,
    wait_for, wait_for_safe_prefix_convergence,
};
use serde_json::{Value, json};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_mins(3);

alloy_sol_types::sol! {
    interface FeeQuoteProbe {
        function gasSensitive() external view returns (uint256);
        function rejectsSettlementTip(address proxy) external;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsigned_nested_quote_matches_receipt_and_replays() {
    nested_quote_roundtrip(false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enforced_nested_quote_rejects_underpayment_and_replays() {
    nested_quote_roundtrip(true, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_rpc_quote_rejects_underpayment_settles_and_replays() {
    nested_quote_roundtrip(true, true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compact_nested_legacy_estimates_both_types_settles_and_replays() {
    nested_quote_roundtrip(true, true, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compact_nested_eip1559_estimates_both_types_settles_and_replays() {
    nested_quote_roundtrip(true, false, true).await;
}

async fn nested_quote_roundtrip(enforced: bool, legacy: bool, compact: bool) {
    // Exercise signed tariff admission above the old 5M quote-profile ceiling.
    // This covers submitted gas limits, not the Falcon verifier's execution cost.
    let mut source_gas_limit = if !enforced {
        1_200_000
    } else if legacy {
        6_050_247
    } else {
        16_777_216
    };
    let policy = crate::settlement_policy_cost::shadow_pricing_policy();
    let world = setup_cross_chain_with_env(&[
        (
            "EEZ_L2_FEE_RECIPIENT",
            policy["beneficiary"].as_str().unwrap().into(),
        ),
        ("EEZ_L2_FEE_SHADOW_POLICY", policy.to_string()),
        ("EEZ_L2_FEE_ENFORCE_TARIFF", enforced.to_string()),
        // Retain real ordinary deployment history until this cross-chain call.
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10000".into()),
        ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
        (
            "RUST_LOG",
            "info,eez_composer::composer::fees::quote=debug".into(),
        ),
    ])
    .await
    .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let inner = deploy_nested_setter_inner(
        &world.l2_rpc(),
        TARGET_DEPLOYER,
        world.l2_chain_id,
        world.outbound_proxy,
    )
    .await
    .unwrap();
    let outer =
        deploy_nested_setter_outer(&world.l2_rpc(), TARGET_DEPLOYER, world.l2_chain_id, inner)
            .await
            .unwrap();
    let signer: PrivateKeySigner = OUTBOUND_USER.parse().unwrap();
    let nonce = onchain_nonce(&world.l2_rpc(), OUTBOUND_USER).await.unwrap();
    let target_before = l2_value(&world.l1_rpc(), world.outbound_value)
        .await
        .unwrap();
    let data = Bytes::from(INestedSetterOuter::setViaInnerCall { v: U256::from(123) }.abi_encode());
    let intent = json!({"type":if legacy {"0x0"} else {"0x2"},"chainId":format!("{:#x}",world.l2_chain_id),"from":signer.address(),"to":outer,
        "nonce":format!("{nonce:#x}"),"gas":format!("{source_gas_limit:#x}"),"value":"0x0","data":data});
    let client = reqwest::Client::new();
    let mut wrong = intent.clone();
    wrong["chainId"] = json!("0xdead");
    let response = quote_rpc(&client, &world.l2_xchain(), "eez_quoteFee", json!([wrong]))
        .await
        .unwrap();
    assert_eq!(response["error"]["code"], -32602);
    let oversized = json!({"from":signer.address(),"to":outer,"data":"x".repeat(32 * 1024)});
    let response = quote_rpc(
        &client,
        &world.l2_xchain(),
        "eez_quoteFee",
        json!([oversized]),
    )
    .await
    .unwrap();
    assert_eq!(response["error"]["data"]["code"], "quote_limit");
    for rpc_url in [world.l1_xchain(), world.l2_xchain(), world.l2_rpc()] {
        let response = quote_rpc(&client, &rpc_url, "eez_quoteL2ToL1Fee", json!([intent]))
            .await
            .unwrap();
        assert_eq!(
            response["error"]["code"], -32601,
            "removed API must not remain active: {response}"
        );
    }
    // Exact-parent quotes can race a new L1 head. Retry only this documented
    // availability condition; unsupported or failed simulation is a test error.
    let last_error = std::sync::Mutex::new(Value::Null);
    let quote = wait_for(Duration::from_secs(30), || async {
        if compact {
            let mut input = intent.clone();
            for key in ["gas", "nonce", "chainId", "type"] {
                input.as_object_mut().unwrap().remove(key);
            }
            let params = json!([input]);
            let detailed = quote_rpc(
                &client,
                &world.l2_xchain(),
                "eez_debugQuoteFee",
                params.clone(),
            )
            .await?;
            if quote_retry(&detailed) {
                return Ok(None);
            }
            anyhow::ensure!(
                detailed.get("result").is_some(),
                "debug quote failed: {detailed}"
            );
            let detailed = &detailed["result"];
            let options = detailed["options"].as_array().unwrap();
            anyhow::ensure!(options.len() == 2, "missing fee variants: {detailed}");
            for (i, ty) in ["0x0", "0x2"].iter().enumerate() {
                let option = &options[i];
                let diagnostic = &detailed["diagnostics"]["options"][i];
                anyhow::ensure!(
                    option["type"] == *ty && option.get("error").is_none(),
                    "failed variant: {option}"
                );
                anyhow::ensure!(
                    option["intentHash"] == diagnostic["intentHash"],
                    "different binding"
                );
                anyhow::ensure!(
                    option["gas"] == diagnostic["transaction"]["gas"],
                    "different gas"
                );
                anyhow::ensure!(
                    option["intentHash"]
                        == json!(alloy_primitives::keccak256(
                            diagnostic["intent"].to_string()
                        )),
                    "invalid intent binding"
                );
                anyhow::ensure!(
                    diagnostic["context"] == detailed["diagnostics"]["options"][0]["context"],
                    "variants mixed chain state"
                );
            }
            let simple = quote_rpc(&client, &world.l2_xchain(), "eez_quoteFee", params).await?;
            if quote_retry(&simple) {
                return Ok(None);
            }
            anyhow::ensure!(
                simple.get("result").is_some(),
                "compact quote failed: {simple}"
            );
            let simple = &simple["result"];
            anyhow::ensure!(
                simple.get("diagnostics").is_none() && simple.get("context").is_none(),
                "verbose compact quote: {simple}"
            );
            anyhow::ensure!(
                simple["chainId"] == intent["chainId"] && simple["nonce"] == intent["nonce"],
                "incorrect defaults"
            );
            // Calls can legitimately select different heads. Compare only when
            // they produce the same options, then sign those compact fields.
            if simple["options"] != detailed["options"] {
                return Ok(None);
            }
            return Ok(Some(
                detailed["diagnostics"]["options"][usize::from(!legacy)].clone(),
            ));
        }
        let value = quote_rpc(
            &client,
            &world.l2_xchain(),
            "eez_debugQuoteFee",
            json!([intent]),
        )
        .await?;
        if quote_retry(&value) {
            *last_error.lock().unwrap() = value;
            return Ok(None);
        }
        anyhow::ensure!(
            value.get("result").is_some(),
            "quote failed: {value}{}",
            world.settlement_diagnostics()
        );
        let diagnostic = &value["result"]["diagnostics"]["options"][0];
        anyhow::ensure!(diagnostic.get("error").is_none(), "failed variant: {value}");
        Ok(Some(diagnostic.clone()))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "quote unavailable: {error:#}; last response: {}{}",
            last_error.lock().unwrap(),
            world.settlement_diagnostics()
        )
    });
    assert_eq!(quote["mode"], "advisory");
    assert_eq!(
        quote["estimate"]["basis"],
        "isolated_native_l2_and_l1_settlement"
    );
    assert_eq!(quote["estimate"]["proof"], "metered_assumed_valid");
    assert_eq!(quote["policy"]["assumptions"], policy);
    assert!(quote["estimate"]["iterations"].as_u64().unwrap() >= 2);
    assert!(
        quote["context"]["l2ParentNumber"].as_u64().unwrap()
            > quote["context"]["settledL2Number"].as_u64().unwrap()
    );
    assert_eq!(
        onchain_nonce(&world.l2_rpc(), OUTBOUND_USER).await.unwrap(),
        nonce,
        "quotes spend no nonce"
    );
    assert_eq!(
        l2_value(&world.l1_rpc(), world.outbound_value)
            .await
            .unwrap(),
        target_before,
        "quotes change no target state"
    );
    let quantity = |key: &str| {
        u128::from_str_radix(
            quote["transaction"][key]
                .as_str()
                .unwrap()
                .trim_start_matches("0x"),
            16,
        )
        .unwrap()
    };
    let base = quote["estimate"]["l2BaseFeePerGas"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .unwrap();
    let cap = quantity(if legacy { "gasPrice" } else { "maxFeePerGas" });
    let tip = if legacy {
        cap - base
    } else {
        quantity("maxPriorityFeePerGas")
    };
    if compact {
        source_gas_limit = u64::try_from(quantity("gas")).unwrap();
    }
    let sign = |priority, price| {
        let mut tx: TypedTransaction = if legacy {
            TxLegacy {
                chain_id: Some(world.l2_chain_id),
                nonce,
                gas_limit: source_gas_limit,
                gas_price: price,
                to: TxKind::Call(outer),
                input: data.clone(),
                ..Default::default()
            }
            .into()
        } else {
            TxEip1559 {
                chain_id: world.l2_chain_id,
                nonce,
                gas_limit: source_gas_limit,
                max_priority_fee_per_gas: priority,
                max_fee_per_gas: price,
                to: TxKind::Call(outer),
                input: data.clone(),
                ..Default::default()
            }
            .into()
        };
        let signature = signer.sign_transaction_sync(&mut tx).unwrap();
        tx.into_envelope(signature)
    };
    if enforced {
        let mut rejected_fees = vec![(0, if legacy { base } else { cap })];
        if legacy {
            let required = quote["estimate"]["requiredL2Tip"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap();
            let limit_based_tip = required.div_ceil(1_200_000);
            assert!(
                limit_based_tip < tip,
                "must exercise gas-limit underpayment"
            );
            rejected_fees.push((limit_based_tip, base + limit_based_tip));
        }
        for (rejected_tip, rejected_cap) in rejected_fees {
            let underfunded = sign(rejected_tip, rejected_cap);
            let raw = alloy_primitives::hex::encode_prefixed(underfunded.encoded_2718());
            // Availability races may defer validation; only the intended tariff
            // rejection establishes the regression, never a timeout or bad nonce.
            let error = wait_for(Duration::from_secs(30), || async {
                let response: Value = client
                    .post(world.l2_xchain())
                    .json(&json!({
                        "jsonrpc":"2.0", "id":1, "method":"eth_sendRawTransaction", "params":[raw]
                    }))
                    .send()
                    .await?
                    .json()
                    .await?;
                let error = &response["error"];
                match error["data"]["code"].as_str() {
                    Some("stale_context" | "quote_unavailable" | "quote_busy") => Ok(None),
                    Some("l2_tip_underfunded") => Ok(Some(error.clone())),
                    _ => anyhow::bail!("expected tariff rejection: {response}"),
                }
            })
            .await
            .unwrap();
            assert_eq!(error["code"], -32003);
            let status: Value = client.post(world.l2_xchain()).json(&json!({
            "jsonrpc":"2.0", "id":2, "method":"eez_getCrossChainTransaction", "params":[underfunded.tx_hash()]
        })).send().await.unwrap().json().await.unwrap();
            assert!(
                status["error"].is_null(),
                "queue lookup must reach the front: {status}"
            );
            assert!(
                status["result"].is_null(),
                "rejected source must not enter the queue: {status}"
            );
            assert_eq!(
                onchain_nonce(&world.l2_rpc(), OUTBOUND_USER).await.unwrap(),
                nonce
            );
            assert_eq!(
                l2_value(&world.l1_rpc(), world.outbound_value)
                    .await
                    .unwrap(),
                target_before
            );
        }
    }
    let signed = sign(tip, cap);
    assert_eq!(signed.ty(), if legacy { 0 } else { 2 });
    let hash = *signed.tx_hash();
    let raw = alloy_primitives::hex::encode_prefixed(signed.encoded_2718());
    wait_for(Duration::from_secs(30), || async {
        let response: Value = client
            .post(world.l2_xchain())
            .json(&json!({
                "jsonrpc":"2.0", "id":3, "method":"eth_sendRawTransaction", "params":[raw]
            }))
            .send()
            .await?
            .json()
            .await?;
        if matches!(
            response["error"]["data"]["code"].as_str(),
            Some("stale_context" | "quote_unavailable" | "quote_busy")
        ) {
            return Ok(None);
        }
        anyhow::ensure!(
            response["result"] == json!(hash),
            "signed source rejected: {response}"
        );
        Ok(Some(()))
    })
    .await
    .unwrap();
    let inbound = if enforced {
        Some(
            sign_and_send(
                &world.l1_xchain(),
                INBOUND_USER,
                l1.get_chain_id().await.unwrap(),
                onchain_nonce(&world.l1_rpc(), INBOUND_USER).await.unwrap(),
                Some(world.setter_proxy),
                U256::ZERO,
                IValue::setValueCall { v: U256::from(456) }.abi_encode(),
                600_000,
            )
            .await
            .unwrap(),
        )
    } else {
        None
    };
    let receipt = wait_for(TIMEOUT, || async {
        Ok(l2.get_transaction_receipt(hash).await?)
    })
    .await
    .unwrap();
    assert!(receipt.status());
    assert_eq!(
        receipt.gas_used,
        quote["estimate"]["sourceGasUsed"].as_u64().unwrap()
    );
    let source_block = l2
        .get_block_by_hash(receipt.block_hash.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source_block.header.beneficiary, Address::repeat_byte(0x44));
    let paid_tip = U256::from(receipt.gas_used)
        * U256::from(
            receipt.effective_gas_price - u128::from(source_block.header.base_fee_per_gas.unwrap()),
        );
    if legacy {
        // Legacy fixes the total gas price, so a lower inclusion base fee
        // increases the beneficiary's share without changing the user's price.
        assert_eq!(receipt.effective_gas_price, cap);
        assert_eq!(
            (U256::from(receipt.gas_used) * U256::from(cap - base)).to_string(),
            quote["estimate"]["grossL2Tip"]
        );
    } else {
        assert_eq!(paid_tip.to_string(), quote["estimate"]["grossL2Tip"]);
    }
    assert!(
        paid_tip
            >= U256::from_str_radix(quote["estimate"]["requiredL2Tip"].as_str().unwrap(), 10)
                .unwrap()
    );
    wait_for(TIMEOUT, || async {
        Ok(
            (l2_value(&world.l1_rpc(), world.outbound_value).await? == U256::from(123))
                .then_some(()),
        )
    })
    .await
    .unwrap();
    let completed = eez_testkit::read_state_word(
        &world.l2_rpc(),
        &eez_testkit::call_read(
            inner,
            "completedProxyCalls()",
            INestedSetterInner::completedProxyCallsCall {}.abi_encode(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(completed, U256::from(1));
    if let Some(hash) = inbound {
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l1.get_transaction_receipt(hash).await?)
        })
        .await
        .unwrap();
        assert!(receipt.status());
        wait_for(TIMEOUT, || async {
            Ok((l2_value(&world.l2_rpc(), world.value_l2).await? == U256::from(456)).then_some(()))
        })
        .await
        .unwrap();
    }
    let follower = NodeHandle::start(
        "fee-quote-follower",
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
        receipt.block_number.unwrap(),
        TIMEOUT,
    )
    .await
    .unwrap();
    let replayed = ProviderBuilder::new()
        .connect_http(follower.l2_rpc_url().parse().unwrap())
        .get_transaction_receipt(hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replayed.block_hash, receipt.block_hash);
    assert_eq!(replayed.gas_used, receipt.gas_used);
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
        if enforced {
            assert_eq!(
                observation["l1"]["companion_count"], 0,
                "unpaid inbound costs must be settled separately"
            );
        }
        let estimated_gas = quote["estimate"]["l1Gas"].as_u64().unwrap();
        assert!(
            estimated_gas.abs_diff(post.gas_used) <= 1_000,
            "the fee must price executed settlement, not admission reserves: quote={estimated_gas}, receipt={}",
            post.gas_used
        );
        assert_eq!(
            quote["estimate"]["blobs"].as_u64().unwrap() * alloy_eips::eip4844::DATA_GAS_PER_BLOB,
            post.blob_gas_used.unwrap()
        );
        checked_post = true;
    }
    assert!(checked_post, "independent canonical post receipt required");
    if let Ok(dir) = std::env::var("EEZ_TEST_LOG_DIR") {
        std::fs::write(
            std::path::Path::new(&dir).join(if compact && legacy {
                "compact-legacy-fee-quote.json"
            } else if compact {
                "compact-eip1559-fee-quote.json"
            } else if legacy {
                "legacy-fee-quote.json"
            } else if enforced {
                "enforced-fee-quote.json"
            } else {
                "unsigned-fee-quote.json"
            }),
            serde_json::to_vec_pretty(&quote).unwrap(),
        )
        .unwrap();
    }
}

async fn quote_rpc(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: Value,
) -> anyhow::Result<Value> {
    Ok(client
        .post(url)
        .json(&json!({"jsonrpc":"2.0","id":"compact", "method":method,"params":params}))
        .send()
        .await?
        .json()
        .await?)
}

fn quote_retry(response: &Value) -> bool {
    matches!(
        response["error"]["data"]["code"].as_str(),
        Some("stale_context" | "quote_unavailable" | "quote_busy")
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compact_ordinary_calls_boundaries_and_fee_sensitive_rejection() {
    let policy = crate::settlement_policy_cost::shadow_pricing_policy();
    let world = setup_cross_chain_with_env(&[
        (
            "EEZ_L2_FEE_RECIPIENT",
            policy["beneficiary"].as_str().unwrap().into(),
        ),
        ("EEZ_L2_FEE_SHADOW_POLICY", policy.to_string()),
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10000".into()),
    ])
    .await
    .unwrap();
    let client = reqwest::Client::new();
    let signer: PrivateKeySigner = OUTBOUND_USER.parse().unwrap();
    let intent = json!({"from":signer.address(),"to":world.value_l2,
        "data":Bytes::from(IValue::setValueCall {v:U256::from(999)}.abi_encode())});
    for method in ["eez_quoteFee", "eez_debugQuoteFee"] {
        let response = quote_rpc(&client, &world.l1_xchain(), method, json!([intent]))
            .await
            .unwrap();
        assert_eq!(response["error"]["data"]["code"], "unsupported_direction");
        let response = quote_rpc(&client, &world.l2_rpc(), method, json!([intent]))
            .await
            .unwrap();
        assert_eq!(response["error"]["code"], -32601);
    }
    let call = |params: Value| {
        let client = client.clone();
        let url = world.l2_xchain();
        async move {
            wait_for(Duration::from_secs(30), || {
                let params = params.clone();
                let client = &client;
                let url = &url;
                async move {
                    let response = quote_rpc(client, url, "eez_debugQuoteFee", params).await?;
                    Ok((!quote_retry(&response)).then_some(response))
                }
            })
            .await
            .unwrap()
        }
    };
    let before = l2_value(&world.l2_rpc(), world.value_l2).await.unwrap();
    let quote = call(json!([intent])).await;
    assert!(quote.get("result").is_some(), "{quote}");
    for diagnostic in quote["result"]["diagnostics"]["options"]
        .as_array()
        .unwrap()
    {
        assert_eq!(diagnostic["estimate"]["basis"], "ordinary_l2", "{quote}");
        assert_eq!(diagnostic["estimate"]["requiredL2Tip"], "0");
    }
    assert_eq!(
        l2_value(&world.l2_rpc(), world.value_l2).await.unwrap(),
        before
    );
    for (field, value, code) in [
        ("chainId", json!("0xdead"), "invalid_intent"),
        ("nonce", json!("0xff"), "invalid_intent"),
        ("gas", json!("0x1000001"), "invalid_intent"),
        ("gasPrice", json!("0x1"), "invalid_intent"),
    ] {
        let mut bad = intent.clone();
        bad[field] = value;
        let response = call(json!([bad])).await;
        assert_eq!(response["error"]["data"]["code"], code, "{response}");
    }
    let mut poor = intent.clone();
    poor["from"] = json!(Address::repeat_byte(0x19));
    poor["value"] = json!("0x1");
    let response = call(json!([poor])).await;
    for option in response["result"]["options"]
        .as_array()
        .expect("per-variant funding error")
    {
        assert_eq!(option["error"]["code"], "invalid_intent", "{response}");
    }
    let mut short = intent.clone();
    short["gas"] = json!("0x5208");
    let response = call(json!([short])).await;
    for option in response["result"]["options"]
        .as_array()
        .expect("per-variant execution error")
    {
        assert_eq!(option["error"]["code"], "unsupported_intent", "{response}");
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/out/FeeQuoteProbe.sol/FeeQuoteProbe.json");
    let artifact: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let bytecode: Bytes = artifact["bytecode"]["object"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let hash = sign_and_send(
        &world.l2_rpc(),
        TARGET_DEPLOYER,
        world.l2_chain_id,
        onchain_nonce(&world.l2_rpc(), TARGET_DEPLOYER)
            .await
            .unwrap(),
        None,
        U256::ZERO,
        bytecode.to_vec(),
        1_000_000,
    )
    .await
    .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let receipt = wait_for(TIMEOUT, || async {
        Ok(l2.get_transaction_receipt(hash).await?)
    })
    .await
    .unwrap();
    assert!(receipt.status());
    let probe = receipt.contract_address.unwrap();
    let sensitive = json!({"from":signer.address(),"to":probe,
        "data":Bytes::from(FeeQuoteProbe::gasSensitiveCall {}.abi_encode())});
    let response = call(json!([sensitive, {"transactionTypes":["0x2"]}])).await;
    assert_eq!(response["result"]["options"].as_array().unwrap().len(), 1);
    assert_eq!(
        response["result"]["options"][0]["gas"], "0x1000000",
        "gas-dependent output must retain verified ceiling: {response}"
    );
    let sensitive = json!({"from":signer.address(),"to":probe,
        "data":Bytes::from(FeeQuoteProbe::rejectsSettlementTipCall {proxy:world.outbound_proxy}.abi_encode())});
    let response = call(json!([sensitive])).await;
    for option in response["result"]["options"]
        .as_array()
        .expect("per-variant fee-dependent rejection")
    {
        assert_eq!(
            option["error"]["code"], "unsupported_intent",
            "must replay final price: {response}"
        );
    }
}
