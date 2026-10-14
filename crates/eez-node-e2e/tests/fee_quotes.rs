//! Unsigned application quote -> one signed source -> native settlement and
//! independent follower replay. No quoted bytes are submitted before signing.

use alloy_consensus::{SignableTransaction, TxEip1559};
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsigned_nested_quote_matches_receipt_and_replays() {
    nested_quote_roundtrip(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enforced_nested_quote_rejects_underpayment_and_replays() {
    nested_quote_roundtrip(true).await;
}

async fn nested_quote_roundtrip(enforced: bool) {
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
    let intent = json!({"type":"0x2","chainId":format!("{:#x}",world.l2_chain_id),"from":signer.address(),"to":outer,
        "nonce":format!("{nonce:#x}"),"gas":"0x124f80","value":"0x0","data":data});
    let client = reqwest::Client::new();
    let url = format!("{}/eez/fees/l2-to-l1", world.l2_xchain());
    let mut wrong = intent.clone();
    wrong["chainId"] = json!("0xdead");
    let response = client.post(&url).json(&wrong).send().await.unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["code"],
        "invalid_intent"
    );
    let oversized = client
        .post(&url)
        .body(" ".repeat(32 * 1024 + 1))
        .send()
        .await
        .unwrap();
    assert_eq!(oversized.status(), 413);
    let response = client
        .post(format!("{}/eez/fees/l2-to-l1", world.l1_xchain()))
        .json(&intent)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    // Exact-parent quotes can race a new L1 head. Retry only this documented
    // availability condition; unsupported or failed simulation is a test error.
    let last_error = std::sync::Mutex::new(Value::Null);
    let quote = wait_for(Duration::from_secs(30), || async {
        let response = client.post(&url).json(&intent).send().await?;
        let status = response.status();
        let value = response.json::<Value>().await?;
        if status.is_success() {
            return Ok(Some(value));
        }
        if status == 409 || status == 503 {
            *last_error.lock().unwrap() = value;
            return Ok(None);
        }
        anyhow::bail!(
            "quote failed: {status} {value}{}",
            world.settlement_diagnostics()
        );
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
    let tip = quantity("maxPriorityFeePerGas");
    if enforced {
        let mut underfunded = TxEip1559 {
            chain_id: world.l2_chain_id,
            nonce,
            gas_limit: 1_200_000,
            max_priority_fee_per_gas: 0,
            max_fee_per_gas: quantity("maxFeePerGas"),
            to: TxKind::Call(outer),
            input: data.clone(),
            ..Default::default()
        };
        let signature = signer.sign_transaction_sync(&mut underfunded).unwrap();
        let underfunded = underfunded.into_signed(signature);
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
            "jsonrpc":"2.0", "id":2, "method":"eez_getCrossChainTransaction", "params":[underfunded.hash()]
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
    let mut tx = TxEip1559 {
        chain_id: world.l2_chain_id,
        nonce,
        gas_limit: 1_200_000,
        max_priority_fee_per_gas: tip,
        max_fee_per_gas: quantity("maxFeePerGas"),
        to: TxKind::Call(outer),
        input: data,
        ..Default::default()
    };
    let signature = signer.sign_transaction_sync(&mut tx).unwrap();
    let signed = tx.into_signed(signature);
    let hash = *signed.hash();
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
    assert_eq!(paid_tip.to_string(), quote["estimate"]["grossL2Tip"]);
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
        assert!(quote["estimate"]["l1Gas"].as_u64().unwrap() >= post.gas_used);
        assert_eq!(
            quote["estimate"]["blobs"].as_u64().unwrap() * alloy_eips::eip4844::DATA_GAS_PER_BLOB,
            post.blob_gas_used.unwrap()
        );
        checked_post = true;
    }
    assert!(checked_post, "independent canonical post receipt required");
    if let Ok(dir) = std::env::var("EEZ_TEST_LOG_DIR") {
        std::fs::write(
            std::path::Path::new(&dir).join(if enforced {
                "enforced-fee-quote.json"
            } else {
                "unsigned-fee-quote.json"
            }),
            serde_json::to_vec_pretty(&quote).unwrap(),
        )
        .unwrap();
    }
}
