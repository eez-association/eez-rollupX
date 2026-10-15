//! Unsigned application quote -> one signed source -> native settlement and
//! independent follower replay. No quoted bytes are submitted before signing.

use alloy_consensus::{Transaction as _, TxEip1559, TxLegacy, Typed2718, TypedTransaction};
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
    nested_quote_roundtrip(false, false, false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enforced_nested_quote_rejects_underpayment_and_replays() {
    nested_quote_roundtrip(true, false, false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_rpc_quote_rejects_underpayment_settles_and_replays() {
    nested_quote_roundtrip(true, true, false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compact_nested_legacy_estimates_both_types_settles_and_replays() {
    nested_quote_roundtrip(true, true, true, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn compact_nested_eip1559_estimates_both_types_settles_and_replays() {
    nested_quote_roundtrip(true, false, true, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simplified_five_percent_legacy_payment_settles_below_twenty_percent_quote() {
    nested_quote_roundtrip(true, true, true, Some(500)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simplified_one_percent_eip1559_payment_settles_below_twenty_percent_quote() {
    nested_quote_roundtrip(true, false, true, Some(100)).await;
}

async fn nested_quote_roundtrip(enforced: bool, legacy: bool, compact: bool, margin: Option<u64>) {
    // Exercise signed tariff admission above the old 5M quote-profile ceiling.
    // This covers submitted gas limits, not the Falcon verifier's execution cost.
    let mut source_gas_limit = if !enforced {
        1_200_000
    } else if legacy {
        6_050_247
    } else {
        16_777_216
    };
    let mut policy = crate::settlement_policy_cost::shadow_pricing_policy();
    if margin.is_some() {
        policy["risk_bps"] = json!(0);
        policy["profit_bps"] = json!(2000);
    }
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
    let transaction = json!({"type":if legacy {"0x0"} else {"0x2"},"chainId":format!("{:#x}",world.l2_chain_id),"from":signer.address(),"to":outer,
        "nonce":format!("{nonce:#x}"),"gas":format!("{source_gas_limit:#x}"),"value":"0x0","data":data});
    let client = reqwest::Client::new();
    let mut wrong = transaction.clone();
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
        let response = quote_rpc(
            &client,
            &rpc_url,
            "eez_quoteL2ToL1Fee",
            json!([transaction]),
        )
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
            let mut input = transaction.clone();
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
            anyhow::ensure!(
                detailed.get("recoveryMode").is_none()
                    && detailed["diagnostics"]["recoveryMode"] == "enforced_tariff",
                "tariff mode must be diagnostic only: {detailed}"
            );
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
                    option["requestHash"] == diagnostic["requestHash"],
                    "different binding"
                );
                anyhow::ensure!(
                    option.get("intentHash").is_none() && diagnostic.get("intent").is_none(),
                    "retired transaction terminology"
                );
                anyhow::ensure!(
                    option["gas"] == diagnostic["transaction"]["gas"],
                    "different gas"
                );
                anyhow::ensure!(
                    option["requestHash"]
                        == json!(alloy_primitives::keccak256(
                            diagnostic["request"].to_string()
                        )),
                    "invalid transaction binding"
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
                simple
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    == ["chainId", "expiresAt", "nonce", "options", "version"],
                "verbose compact quote: {simple}"
            );
            anyhow::ensure!(
                simple["chainId"] == transaction["chainId"]
                    && simple["nonce"] == transaction["nonce"],
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
            json!([transaction]),
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
        anyhow::ensure!(
            value["result"].get("recoveryMode").is_none()
                && value["result"]["diagnostics"]["recoveryMode"]
                    == if enforced {
                        "enforced_tariff"
                    } else {
                        "advisory"
                    },
            "incorrect diagnostic tariff mode: {value}"
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
    let mut cap = quantity(if legacy { "gasPrice" } else { "maxFeePerGas" });
    let mut tip = if legacy {
        cap - base
    } else {
        quantity("maxPriorityFeePerGas")
    };
    if compact {
        source_gas_limit = u64::try_from(quantity("gas")).unwrap();
    }
    if let Some(margin) = margin {
        let minimum = quote["estimate"]["minimumL2Tip"]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap();
        let gas = u128::from(quote["estimate"]["sourceGasUsed"].as_u64().unwrap());
        let reduced = (minimum * u128::from(10_000 + margin))
            .div_ceil(10_000)
            .div_ceil(gas);
        assert!(
            reduced < tip,
            "the signed fee must be below the recommended quote"
        );
        tip = reduced;
        cap = base + tip;
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
    if margin.is_some() {
        // Reach the pending-attempt condition that previously disabled every
        // quote. Another sender can still estimate independently at a completed
        // snapshot. No retries hide quote_busy or reconciliation failures here.
        wait_for(Duration::from_secs(30), || async {
            let status = quote_rpc(
                &client,
                &world.l2_xchain(),
                "eez_getCrossChainTransaction",
                json!([hash]),
            )
            .await?;
            Ok((status["result"]["ownership"]["state"] == "awaiting_evidence").then_some(()))
        })
        .await
        .unwrap();
        let independent: PrivateKeySigner = TARGET_DEPLOYER.parse().unwrap();
        let params = json!([{"from":independent.address(), "to":outer, "data":data}, {"transactionTypes":["0x2"]}]);
        let mut burst = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let client = client.clone();
            let url = world.l2_xchain();
            let params = params.clone();
            burst.spawn(async move { quote_rpc(&client, &url, "eez_quoteFee", params).await });
        }
        let mut count = 0;
        while let Some(result) = burst.join_next().await {
            let response = result.unwrap().unwrap();
            assert!(
                response.get("error").is_none(),
                "first-attempt quote must succeed: {response}"
            );
            assert!(
                response["result"]["options"][0].get("error").is_none(),
                "{response}"
            );
            count += 1;
        }
        assert_eq!(count, 8);
    }
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
        if margin.is_none() {
            assert_eq!(
                (U256::from(receipt.gas_used) * U256::from(cap - base)).to_string(),
                quote["estimate"]["grossL2Tip"]
            );
        }
    } else if margin.is_none() {
        assert_eq!(paid_tip.to_string(), quote["estimate"]["grossL2Tip"]);
    }
    let target =
        U256::from_str_radix(quote["estimate"]["requiredL2Tip"].as_str().unwrap(), 10).unwrap();
    let minimum =
        U256::from_str_radix(quote["estimate"]["minimumL2Tip"].as_str().unwrap(), 10).unwrap();
    assert!(paid_tip >= minimum);
    if margin.is_some() {
        assert!(
            paid_tip < target,
            "lower margin must survive admission and final dispatch"
        );
    } else {
        assert!(paid_tip >= target);
    }
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
        if margin.is_some() {
            assert_eq!(
                observation["pricing"]["cost_coverage"], "covered",
                "{observation}"
            );
            assert_eq!(
                observation["pricing"]["comparison"], "shortfall",
                "target profit may be missed: {observation}"
            );
        }
        checked_post = true;
    }
    assert!(checked_post, "independent canonical post receipt required");
    if let Ok(dir) = std::env::var("EEZ_TEST_LOG_DIR") {
        std::fs::write(
            std::path::Path::new(&dir).join(if margin.is_some() && legacy {
                "simplified-legacy-fee-quote.json"
            } else if margin.is_some() {
                "simplified-eip1559-fee-quote.json"
            } else if compact && legacy {
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
async fn l1_refund_quote_legacy_continues_after_missing_payment() {
    l1_refund_roundtrip(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn l1_refund_quote_eip1559_continues_after_missing_payment() {
    l1_refund_roundtrip(false).await;
}

async fn l1_refund_roundtrip(legacy: bool) {
    let mut policy = crate::settlement_policy_cost::shadow_pricing_policy();
    policy["risk_bps"] = json!(0);
    policy["profit_bps"] = json!(2000);
    let recipient = Address::repeat_byte(0x97);
    let world = setup_cross_chain_with_env(&[
        (
            "EEZ_L2_FEE_RECIPIENT",
            policy["beneficiary"].as_str().unwrap().into(),
        ),
        ("EEZ_L2_FEE_SHADOW_POLICY", policy.to_string()),
        (
            "EEZ_L1_FEE_REFUND_POLICY",
            json!({"version":1,"recipient":recipient,"percent":90}).to_string(),
        ),
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
        ("RUST_LOG", "info,eez_composer::composer::fees=debug".into()),
    ])
    .await
    .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let chain = l1.get_chain_id().await.unwrap();
    let inner =
        deploy_nested_setter_inner(&world.l1_rpc(), TARGET_DEPLOYER, chain, world.setter_proxy)
            .await
            .unwrap();
    let outer = deploy_nested_setter_outer(&world.l1_rpc(), TARGET_DEPLOYER, chain, inner)
        .await
        .unwrap();
    let signer: PrivateKeySigner = INBOUND_USER.parse().unwrap();
    let nonce = onchain_nonce(&world.l1_rpc(), INBOUND_USER).await.unwrap();
    let data = Bytes::from(INestedSetterOuter::setViaInnerCall { v: U256::from(781) }.abi_encode());
    let client = reqwest::Client::new();
    let transaction = json!({"from":signer.address(),"to":outer,"data":data});
    let quote = wait_for(TIMEOUT, || async {
        let response = quote_rpc(
            &client,
            &world.l1_xchain(),
            "eez_debugQuoteFee",
            json!([transaction]),
        )
        .await?;
        if quote_retry(&response) {
            return Ok(None);
        }
        anyhow::ensure!(
            response.get("error").is_none(),
            "L1 quote: {response}{}",
            world.settlement_diagnostics()
        );
        let result = &response["result"];
        anyhow::ensure!(
            result["diagnostics"]["recoveryMode"] == "builder_refund",
            "{result}"
        );
        for option in result["options"].as_array().unwrap() {
            anyhow::ensure!(
                option.get("error").is_none(),
                "L1 quote variant: {response}{}",
                world.settlement_diagnostics()
            );
            anyhow::ensure!(
                option.get("recoveryMode").is_none() && option.get("estimate").is_none(),
                "compact response leaked diagnostics"
            );
        }
        Ok(Some(result.clone()))
    })
    .await
    .unwrap();
    assert_eq!(quote["options"].as_array().unwrap().len(), 2);
    assert_eq!(
        onchain_nonce(&world.l1_rpc(), INBOUND_USER).await.unwrap(),
        nonce
    );
    assert_ne!(
        l2_value(&world.l2_rpc(), world.value_l2).await.unwrap(),
        U256::from(781)
    );
    let index = usize::from(!legacy);
    let option = &quote["options"][index];
    let detail = &quote["diagnostics"]["options"][index]["estimate"];
    let quantity = |name: &str| {
        u128::from_str_radix(option[name].as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
    };
    let cap = quantity(if legacy { "gasPrice" } else { "maxFeePerGas" });
    let tip = if legacy {
        cap
    } else {
        quantity("maxPriorityFeePerGas")
    };
    let gas = quantity("gas") as u64;
    let sign = |nonce, tip, cap| {
        let mut tx: TypedTransaction = if legacy {
            TxLegacy {
                chain_id: Some(chain),
                nonce,
                gas_limit: gas,
                gas_price: cap,
                to: TxKind::Call(outer),
                input: data.clone(),
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
                input: data.clone(),
                ..Default::default()
            }
            .into()
        };
        let sig = signer.sign_transaction_sync(&mut tx).unwrap();
        tx.into_envelope(sig)
    };
    let base: u128 = detail["l1BaseFeePerGas"].as_str().unwrap().parse().unwrap();
    let underpaid = sign(nonce, 0, if legacy { base + 1 } else { cap });
    let rejected = quote_rpc(
        &client,
        &world.l1_xchain(),
        "eth_sendRawTransaction",
        json!([Bytes::from(underpaid.encoded_2718())]),
    )
    .await
    .unwrap();
    assert_eq!(
        rejected["error"]["data"]["code"], "l1_tip_underfunded",
        "{rejected}"
    );
    world.hold_next_bundle().unwrap();
    let paid = sign(nonce, tip, cap);
    let hash = *paid.tx_hash();
    let admitted = quote_rpc(
        &client,
        &world.l1_xchain(),
        "eth_sendRawTransaction",
        json!([Bytes::from(paid.encoded_2718())]),
    )
    .await
    .unwrap();
    assert_eq!(
        admitted["result"],
        json!(hash),
        "{admitted}{}",
        world.settlement_diagnostics()
    );
    let request = wait_for(TIMEOUT, || async { world.held_bundle() })
        .await
        .unwrap();
    assert_eq!(request["params"][0]["refundTxHashes"], json!([hash]));
    assert_eq!(request["params"][0]["refundRecipient"], json!(recipient));
    assert_eq!(request["params"][0]["refundPercent"], 90);
    assert!(
        !request["params"][0]["revertingTxHashes"]
            .as_array()
            .is_some_and(|v| v.contains(&json!(hash)))
    );
    world.release_bundle(&request).unwrap();
    let receipt = wait_for(TIMEOUT, || async {
        Ok(l1.get_transaction_receipt(hash).await?)
    })
    .await
    .unwrap();
    // The development adapter is intentionally NOT a bundle-aware builder.
    // Its priority sorter mines the expensive source before its required post.
    // Preserve this negative test; real success belongs to the PBS test lane.
    assert!(!receipt.status());
    let block = l1
        .get_block_by_hash(receipt.block_hash.unwrap())
        .full()
        .await
        .unwrap()
        .unwrap();
    let post_index = block
        .transactions
        .as_transactions()
        .unwrap()
        .iter()
        .position(|tx| tx.inner.to() == Some(world.post_batch_target))
        .unwrap();
    assert!(
        receipt.transaction_index.unwrap() < post_index as u64,
        "expected the native mempool adapter's ordering failure"
    );
    // This process fixture deliberately has no refund-capable PBS builder.
    // Inclusion must not be mistaken for payment. Missing payment is now
    // observation-only: another quote, admission and dispatch must still work.
    wait_for(TIMEOUT, || async {
        let signals = world.node.signals_since(0)?;
        Ok(signals
            .iter()
            .any(|s| {
                s.name == "eez.composer.postbatch.actual_cost"
                    && s.fields.get("refund_status").and_then(Value::as_str) == Some("missing")
            })
            .then_some(()))
    })
    .await
    .unwrap();
    let next = wait_for(TIMEOUT, || async {
        let response = quote_rpc(
            &client,
            &world.l1_xchain(),
            "eez_quoteFee",
            json!([transaction]),
        )
        .await?;
        if quote_retry(&response) {
            return Ok(None);
        }
        anyhow::ensure!(response.get("error").is_none(), "{response}");
        let option = &response["result"]["options"][index];
        anyhow::ensure!(option.get("error").is_none(), "{response}");
        Ok(Some(option.clone()))
    })
    .await
    .unwrap();
    assert_eq!(
        onchain_nonce(&world.l1_rpc(), INBOUND_USER).await.unwrap(),
        nonce + 1
    );
    let next_quantity = |name: &str| {
        u128::from_str_radix(next[name].as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
    };
    let cap = next_quantity(if legacy { "gasPrice" } else { "maxFeePerGas" });
    let tip = if legacy {
        cap
    } else {
        next_quantity("maxPriorityFeePerGas")
    };
    let next_signed = sign(nonce + 1, tip, cap);
    world.hold_next_bundle().unwrap();
    let admitted = quote_rpc(
        &client,
        &world.l1_xchain(),
        "eth_sendRawTransaction",
        json!([Bytes::from(next_signed.encoded_2718())]),
    )
    .await
    .unwrap();
    assert_eq!(
        admitted["result"],
        json!(next_signed.tx_hash()),
        "{admitted}"
    );
    let next_bundle = wait_for(TIMEOUT, || async { world.held_bundle() })
        .await
        .unwrap();
    assert_eq!(
        next_bundle["params"][0]["refundTxHashes"],
        json!([next_signed.tx_hash()])
    );
    let follower = NodeHandle::start(
        "l1-refund-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let height = ProviderBuilder::new()
        .connect_http(world.l2_rpc().parse().unwrap())
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Safe)
        .await
        .unwrap()
        .unwrap()
        .header
        .number;
    wait_for_safe_prefix_convergence(&[&world.node, &follower], height, TIMEOUT)
        .await
        .unwrap();
    assert_ne!(
        l2_value(&follower.l2_rpc_url(), world.value_l2)
            .await
            .unwrap(),
        U256::from(781),
        "failed source cannot create a canonical remote effect"
    );
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
    let transaction = json!({"from":signer.address(),"to":world.value_l2,
        "data":Bytes::from(IValue::setValueCall {v:U256::from(999)}.abi_encode())});
    for method in ["eez_quoteFee", "eez_debugQuoteFee"] {
        let response = quote_rpc(&client, &world.l1_xchain(), method, json!([transaction]))
            .await
            .unwrap();
        assert_eq!(response["error"]["data"]["code"], "quotes_disabled");
        let response = quote_rpc(&client, &world.l2_rpc(), method, json!([transaction]))
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
    let quote = call(json!([transaction])).await;
    assert!(quote.get("result").is_some(), "{quote}");
    assert!(quote["result"].get("recoveryMode").is_none());
    assert_eq!(quote["result"]["diagnostics"]["recoveryMode"], "advisory");
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
        ("chainId", json!("0xdead"), "invalid_transaction"),
        ("nonce", json!("0xff"), "invalid_transaction"),
        ("gas", json!("0x1000001"), "invalid_transaction"),
        ("gasPrice", json!("0x1"), "invalid_transaction"),
    ] {
        let mut bad = transaction.clone();
        bad[field] = value;
        let response = call(json!([bad])).await;
        assert_eq!(response["error"]["data"]["code"], code, "{response}");
    }
    let mut poor = transaction.clone();
    poor["from"] = json!(Address::repeat_byte(0x19));
    poor["value"] = json!("0x1");
    let response = call(json!([poor])).await;
    for option in response["result"]["options"]
        .as_array()
        .expect("per-variant funding error")
    {
        assert_eq!(option["error"]["code"], "invalid_transaction", "{response}");
    }
    let mut short = transaction.clone();
    short["gas"] = json!("0x5208");
    let response = call(json!([short])).await;
    for option in response["result"]["options"]
        .as_array()
        .expect("per-variant execution error")
    {
        assert_eq!(
            option["error"]["code"], "unsupported_transaction",
            "{response}"
        );
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
            option["error"]["code"], "unsupported_transaction",
            "must replay final price: {response}"
        );
    }
}
