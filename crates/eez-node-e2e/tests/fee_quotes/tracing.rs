use super::*;
use eez_testkit::{
    IOverlayDriverL1, IOverlayProbeL2, setup_cross_chain_overlay_round_trip_with_env,
};

alloy_sol_types::sol! {
    interface TraceFactory {
        function obtain(bool fresh, bool deterministic, bytes32 salt, uint256 initial)
            external returns (address);
    }
}

fn frames(run: &Value) -> &[Value] {
    run["frames"].as_array().unwrap()
}

fn composition_runs(response: &Value) -> Vec<&Value> {
    response["result"]["diagnostics"]["trace"]["runs"]
        .as_array()
        .expect("debug response must retain the execution trace")
        .iter()
        .filter(|run| run["purpose"] == "composition")
        .collect()
}

fn validate_trace(response: &Value, ty: &str) {
    let trace = &response["result"]["diagnostics"]["trace"];
    assert_eq!(trace["format"], "eez-quote-call-trace", "{response}");
    assert_eq!(trace["version"], 1);
    assert_eq!(trace["truncated"], false, "{trace}");
    assert_eq!(trace["context"]["simulation"], true);
    assert!(trace["context"]["l1ParentHash"].is_string());
    assert!(trace["context"]["l2ParentHash"].is_string());
    let runs = trace["runs"].as_array().unwrap();
    assert!(!runs.is_empty());
    for run in runs {
        assert_eq!(run["type"], ty);
        assert_eq!(run["complete"], true, "{run}");
        for (id, frame) in frames(run).iter().enumerate() {
            assert_eq!(frame["id"], id);
            assert_ne!(frame["status"], "incomplete", "{frame}");
            if let Some(parent) = frame["parentId"].as_u64() {
                assert!((parent as usize) < id);
                assert_eq!(
                    frame["crossChain"],
                    frame["chainId"] != frames(run)[parent as usize]["chainId"]
                );
            } else {
                assert_eq!(frame["crossChain"], false);
            }
        }
    }
}

pub(super) fn save_trace(name: &str, response: &Value) {
    if let Some(directory) = std::env::var_os("EEZ_TEST_LOG_DIR") {
        let directory = std::path::PathBuf::from(directory).join("quote-traces");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(format!("{name}.json")),
            serde_json::to_vec_pretty(response).unwrap(),
        )
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nested_call_traces_link_both_directions_and_preserve_quote_results() {
    let policy = crate::settlement_policy_cost::shadow_pricing_policy();
    let settings = crate::settlement_policy_cost::fee_settings(
        &policy,
        &[
            ("EEZ_FEES_L1_TO_L2_ENABLED", "true".into()),
            ("EEZ_FEES_L2_TO_L1_ENABLED", "true".into()),
            ("EEZ_FEES_L1_REFUND_PERCENT", "90".into()),
            (
                "EEZ_FEES_L1_REFUND_RECIPIENT",
                Address::repeat_byte(0x97).to_string(),
            ),
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
            ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
        ],
    );
    let world = setup_cross_chain_overlay_round_trip_with_env(&settings)
        .await
        .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let client = reqwest::Client::new();
    let source_l1: PrivateKeySigner = INBOUND_USER.parse().unwrap();
    let source_l2: PrivateKeySigner = OUTBOUND_USER.parse().unwrap();
    let nonce_l1 = l1.get_transaction_count(source_l1.address()).await.unwrap();
    let nonce_l2 = l2.get_transaction_count(source_l2.address()).await.unwrap();
    let batch: Value = client
        .post(world.l2_xchain())
        .json(
            &json!([{"jsonrpc":"2.0","id":1,"method":"eez_debugQuoteFee",
            "params":[{"from":source_l2.address(),"to":world.probe},{"trace":"calls"}]}]),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(batch["error"]["code"], -32600, "{batch}");
    assert!(
        batch["error"]["message"]
            .as_str()
            .unwrap()
            .contains("individually")
    );
    for inbound in [true, false] {
        let (rpc, sender, target, data) = if inbound {
            (
                world.l1_xchain(),
                source_l1.address(),
                world.driver,
                IOverlayDriverL1::deliverThenBurnAndPingCall {
                    probe: world.probe_on_l1,
                    driver: world.driver_on_l2,
                    amount: U256::from(7),
                }
                .abi_encode(),
            )
        } else {
            (
                world.l2_xchain(),
                source_l2.address(),
                world.probe,
                IOverlayProbeL2::startDeliverThenBurnAndPingCall {
                    driver: world.driver_on_l2,
                    probe: world.probe_on_l1,
                    amount: U256::from(7),
                }
                .abi_encode(),
            )
        };
        for ty in ["0x0", "0x2"] {
            let mut tx = json!({"from":sender,"to":target,"data":Bytes::from(data.clone())});
            // Exercise both gas discovery and the explicit-gas path.
            if ty == "0x0" {
                tx["gas"] = json!("0x5b8d80");
            }
            let plain = quote_rpc(
                &client,
                &rpc,
                "eez_debugQuoteFee",
                json!([tx, {"transactionTypes":[ty]}]),
            )
            .await
            .unwrap();
            assert!(
                plain["result"]["options"][0].get("error").is_none(),
                "{plain}"
            );
            assert!(plain["result"]["diagnostics"].get("trace").is_none());
            let traced = quote_rpc(
                &client,
                &rpc,
                "eez_debugQuoteFee",
                json!([tx, {"transactionTypes":[ty],"trace":"calls"}]),
            )
            .await
            .unwrap();
            save_trace(
                &format!(
                    "nested-{}-{ty}",
                    if inbound { "inbound" } else { "outbound" }
                ),
                &traced,
            );
            assert!(
                traced["result"]["options"][0].get("error").is_none(),
                "{traced}"
            );
            validate_trace(&traced, ty);
            assert_eq!(
                plain["result"]["options"][0]["gas"],
                traced["result"]["options"][0]["gas"]
            );
            assert_eq!(
                plain["result"]["diagnostics"]["options"][0]["estimate"]["sourceGasUsed"],
                traced["result"]["diagnostics"]["options"][0]["estimate"]["sourceGasUsed"]
            );
            let runs = composition_runs(&traced)
                .into_iter()
                .filter(|run| run["phase"] != "gas_estimation")
                .collect::<Vec<_>>();
            assert!(!runs.is_empty());
            for run in runs {
                let calls = frames(run);
                assert_eq!(calls[0]["to"], json!(target));
                assert_eq!(calls[0]["status"], "success");
                let cross = calls
                    .iter()
                    .filter(|call| call["crossChain"] == true)
                    .collect::<Vec<_>>();
                assert!(cross.len() >= if inbound { 3 } else { 4 }, "{run}");
                // A callback reaches the source chain beneath a remote frame,
                // rather than appearing as a second unrelated transaction.
                assert!(
                    cross
                        .iter()
                        .any(|call| call["chainId"] == calls[0]["chainId"])
                );
                assert!(calls.iter().any(|call| call["to"] == json!(world.probe)));
                assert!(calls.iter().any(|call| call["to"] == json!(world.driver)));
            }
            let invalid = quote_rpc(
                &client,
                &rpc,
                "eez_quoteFee",
                json!([tx, {"trace":"calls"}]),
            )
            .await
            .unwrap();
            assert_eq!(invalid["error"]["data"]["code"], "invalid_transaction");
        }
    }

    // Ordinary source execution can create a contract and call it immediately.
    // Observe the actual CREATE/CREATE2 outcome and subsequent STATICCALL.
    let artifact: Value = serde_json::from_slice(
        &std::fs::read(
            eez_testkit::repo_root()
                .join("contracts/out/NestedContractCreation.sol/NestedCreationFactory.json"),
        )
        .unwrap(),
    )
    .unwrap();
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
        2_000_000,
    )
    .await
    .unwrap();
    let receipt = wait_for(TIMEOUT, || async {
        Ok(l2.get_transaction_receipt(hash).await?)
    })
    .await
    .unwrap();
    assert!(receipt.status());
    let factory = receipt.contract_address.unwrap();
    for deterministic in [false, true] {
        let tx = json!({"from":source_l2.address(),"to":factory,
            "data":Bytes::from(TraceFactory::obtainCall {
                fresh:true, deterministic, salt:alloy_primitives::B256::repeat_byte(7),
                initial:U256::from(41),
            }.abi_encode())});
        let response = quote_rpc(
            &client,
            &world.l2_xchain(),
            "eez_debugQuoteFee",
            json!([tx, {"transactionTypes":["0x2"],"trace":"calls"}]),
        )
        .await
        .unwrap();
        save_trace(&format!("creation-{deterministic}"), &response);
        assert!(
            response["result"]["options"][0].get("error").is_none(),
            "{response}"
        );
        let runs = composition_runs(&response)
            .into_iter()
            .filter(|run| run["phase"] != "gas_estimation")
            .collect::<Vec<_>>();
        assert!(!runs.is_empty(), "{response}");
        for run in runs {
            let created = frames(run)
                .iter()
                .find(|frame| frame["kind"] == if deterministic { "CREATE2" } else { "CREATE" })
                .unwrap();
            assert_eq!(created["status"], "success");
            assert!(created["to"].is_string());
            assert!(frames(run).iter().any(|frame| frame["kind"] == "STATICCALL"
                && frame["to"] == created["to"]
                && frame["status"] == "success"));
        }
    }

    // The L1 source makes a successful L2 callback and then propagates its own
    // revert. The successful descendant must remain visible under that revert.
    let propagated = json!({"from":source_l1.address(),"to":world.driver,
        "data":Bytes::from(IOverlayDriverL1::deliverThenRevertCall {
            probe:world.probe_on_l1, amount:U256::from(9),
        }.abi_encode())});
    for supplied_gas in [false, true] {
        let mut tx = propagated.clone();
        if supplied_gas {
            tx["gas"] = json!("0x5b8d80");
        }
        let response = quote_rpc(
            &client,
            &world.l1_xchain(),
            "eez_debugQuoteFee",
            json!([tx, {"trace":"calls"}]),
        )
        .await
        .unwrap();
        save_trace(&format!("propagated-{supplied_gas}"), &response);
        for option in response["result"]["options"].as_array().unwrap() {
            assert_eq!(
                option["error"]["code"], "unsupported_transaction",
                "{response}"
            );
        }
        for run in composition_runs(&response) {
            assert_eq!(frames(run)[0]["status"], "revert");
            assert!(
                frames(run)
                    .iter()
                    .any(|frame| frame["crossChain"] == true && frame["status"] == "success")
            );
            assert!(frames(run)[0]["output"]["length"].as_u64().unwrap() >= 4);
        }
    }

    // This topology deliberately exceeds the settlement profile: the remote
    // reverting frame has descendants. Its source execution succeeds because
    // the revert was caught. The trace must not misreport that root as reverted
    // merely because the quote subsequently rejects the unsupported effects.
    let caught = json!({"from":source_l2.address(),"to":world.probe,
        "data":Bytes::from(IOverlayProbeL2::startDeliverThenRevertCall {
            driver:world.driver_on_l2,probe:world.probe_on_l1,amount:U256::from(9),
        }.abi_encode())});
    let response = quote_rpc(
        &client,
        &world.l2_xchain(),
        "eez_debugQuoteFee",
        json!([caught, {"transactionTypes":["0x2"],"trace":"calls"}]),
    )
    .await
    .unwrap();
    save_trace("caught-remote", &response);
    let runs = composition_runs(&response);
    assert!(!runs.is_empty());
    for run in runs {
        assert_eq!(frames(run)[0]["status"], "success");
        assert!(
            frames(run)
                .iter()
                .any(|frame| frame["crossChain"] == true && frame["status"] == "revert")
        );
        assert!(
            frames(run)
                .iter()
                .any(|frame| frame["to"] == json!(world.probe)
                    && frame["parentId"].is_number()
                    && frame["status"] == "success")
        );
    }
    assert_eq!(
        l1.get_transaction_count(source_l1.address()).await.unwrap(),
        nonce_l1
    );
    assert_eq!(
        l2.get_transaction_count(source_l2.address()).await.unwrap(),
        nonce_l2
    );
    assert_eq!(
        l2_value(&world.l2_rpc(), world.probe).await.unwrap(),
        U256::ZERO
    );
}
