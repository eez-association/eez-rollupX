use super::*;
use eez_testkit::{IOverlayProbeL2, setup_cross_chain_overlay_round_trip_with_env};
use std::time::Instant;

/// Retain serial latency samples without asserting a wall-clock threshold on
/// shared CI hosts. The same harness can exercise baseline/candidate runtimes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_nested_quotes_preserve_priced_execution_and_source_state() {
    let policy = crate::settlement_policy_cost::shadow_pricing_policy();
    let settings = crate::settlement_policy_cost::fee_settings(
        &policy,
        &[
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
            ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
        ],
    );
    let world = setup_cross_chain_overlay_round_trip_with_env(&settings)
        .await
        .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let client = reqwest::Client::new();
    let source: PrivateKeySigner = OUTBOUND_USER.parse().unwrap();
    let nonce = l2.get_transaction_count(source.address()).await.unwrap();
    let balance = l2.get_balance(source.address()).await.unwrap();
    let data = Bytes::from(
        IOverlayProbeL2::startDeliverThenBurnAndPingCall {
            driver: world.driver_on_l2,
            probe: world.probe_on_l1,
            amount: U256::from(7),
        }
        .abi_encode(),
    );
    let mut samples = Vec::new();
    for supplied_gas in [false, true] {
        for ty in ["0x0", "0x2"] {
            let mut tx = json!({"from":source.address(),"to":world.probe,"data":data});
            if supplied_gas {
                tx["gas"] = json!("0x5b8d80");
            }
            for sample in 0..5 {
                for trace in [false, true] {
                    let mut options = json!({"transactionTypes":[ty]});
                    if trace {
                        options["trace"] = json!("calls");
                    }
                    let started = Instant::now();
                    let response = quote_rpc(
                        &client,
                        &world.l2_xchain(),
                        "eez_debugQuoteFee",
                        json!([tx, options]),
                    )
                    .await
                    .unwrap();
                    let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
                    let result = &response["result"];
                    assert!(result["options"][0].get("gas").is_some(), "{response}");
                    let option = &result["options"][0];
                    let detail = &result["diagnostics"]["options"][0];
                    let estimate = &detail["estimate"];
                    let source_gas = estimate["sourceGasUsed"].as_u64().unwrap();
                    let tip = estimate["grossL2Tip"]
                        .as_str()
                        .unwrap()
                        .parse::<U256>()
                        .unwrap();
                    let required = estimate["requiredL2Tip"]
                        .as_str()
                        .unwrap()
                        .parse::<U256>()
                        .unwrap();
                    assert!(tip >= required && !required.is_zero(), "{response}");
                    let price: U256 = option[if ty == "0x0" {
                        "gasPrice"
                    } else {
                        "maxFeePerGas"
                    }]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap();
                    let fee: U256 = option["estimatedFee"].as_str().unwrap().parse().unwrap();
                    assert_eq!(fee, price * U256::from(source_gas));
                    let runs = result["diagnostics"]["trace"]["runs"].as_array();
                    if let Some(runs) = runs {
                        assert!(runs.iter().all(|run| run["complete"] == true));
                        let priced = runs
                            .iter()
                            .rfind(|run| {
                                run["phase"] == "fee_simulation"
                                    && run["purpose"] == "native_source"
                            })
                            .unwrap();
                        for field in ["gas", "gasPrice", "maxFeePerGas", "maxPriorityFeePerGas"] {
                            assert_eq!(priced["transaction"][field], option[field], "{response}");
                        }
                    }
                    samples.push(json!({
                        "type":ty,"suppliedGas":supplied_gas,"trace":trace,"sample":sample,
                        "elapsedMs":elapsed_ms,"runs":runs.map(Vec::len),
                        "estimate":estimate,"context":detail["context"],"option":option
                    }));
                }
            }
        }
    }
    assert_eq!(
        l2.get_transaction_count(source.address()).await.unwrap(),
        nonce
    );
    assert_eq!(l2.get_balance(source.address()).await.unwrap(), balance);
    if let Some(directory) = std::env::var_os("EEZ_TEST_LOG_DIR") {
        std::fs::write(
            std::path::Path::new(&directory).join("quote-samples.json"),
            serde_json::to_vec_pretty(&samples).unwrap(),
        )
        .unwrap();
    }
}
