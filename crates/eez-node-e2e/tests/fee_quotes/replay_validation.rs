use super::*;
use eez_testkit::{
    CrossChainWorld, IOverlayProbeL2, setup_cross_chain_overlay_round_trip_with_env,
};
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
    let data = Bytes::from(
        IOverlayProbeL2::startDeliverThenBurnAndPingCall {
            driver: world.driver_on_l2,
            probe: world.probe_on_l1,
            amount: U256::from(7),
        }
        .abi_encode(),
    );
    repeated_quotes(&world, world.probe, data, "quote-samples.json").await;
}

/// Exercise the same quote matrix for nested callbacks and real Safe creation.
/// Samples include the effective policy so their cost arithmetic can be checked
/// independently of timings, including across different fixture policies.
pub(crate) async fn repeated_quotes(
    world: &CrossChainWorld,
    target: Address,
    data: Bytes,
    filename: &str,
) {
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let client = reqwest::Client::new();
    let source: PrivateKeySigner = OUTBOUND_USER.parse().unwrap();
    let nonce = l2.get_transaction_count(source.address()).await.unwrap();
    let balance = l2.get_balance(source.address()).await.unwrap();
    let mut samples = Vec::new();
    // Interleave gas modes and formats so automatic quotes do not always pay
    // the fixture's cold-start cost while supplied-gas quotes run last.
    let variants = [(false, "0x0"), (true, "0x0"), (false, "0x2"), (true, "0x2")];
    for sample in 0..25 {
        for offset in 0..variants.len() {
            let index = if sample % 2 == 0 {
                offset
            } else {
                variants.len() - 1 - offset
            };
            let (supplied_gas, ty) = variants[index];
            let mut tx = json!({"from":source.address(),"to":target,"data":data});
            if supplied_gas {
                tx["gas"] = json!("0x5b8d80");
            }
            for trace in if sample % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
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
                let quantity =
                    |key: &str| -> U256 { option[key].as_str().unwrap().parse().unwrap() };
                let base: U256 = estimate["l2BaseFeePerGas"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap();
                // Legacy pays its full price; type 2 leaves room for the base
                // fee to double and is charged `base + tip`.
                let price = if ty == "0x0" {
                    quantity("gasPrice")
                } else {
                    let priority = quantity("maxPriorityFeePerGas");
                    assert_eq!(
                        quantity("maxFeePerGas"),
                        base * U256::from(2) + priority,
                        "{response}"
                    );
                    base + priority
                };
                let fee: U256 = option["estimatedFee"].as_str().unwrap().parse().unwrap();
                assert_eq!(fee, price * U256::from(source_gas));
                let runs = result["diagnostics"]["trace"]["runs"].as_array();
                if let Some(runs) = runs {
                    assert!(runs.iter().all(|run| run["complete"] == true));
                    let probes = runs
                        .iter()
                        .filter(|run| {
                            run["phase"] == "gas_estimation" && run["purpose"] == "native_source"
                        })
                        .collect::<Vec<_>>();
                    if supplied_gas {
                        assert!(probes.is_empty(), "{response}");
                    } else {
                        assert_eq!(probes.len(), 2, "high and selected probes: {response}");
                        assert_ne!(
                            probes[0]["transaction"]["gas"],
                            probes[1]["transaction"]["gas"]
                        );
                        assert_eq!(
                            probes[0]["transaction"]["nonce"],
                            probes[1]["transaction"]["nonce"]
                        );
                        assert_eq!(
                            probes[0]["frames"][0]["output"],
                            probes[1]["frames"][0]["output"]
                        );
                        assert_eq!(
                            probes[0]["frames"][0]["gasUsed"],
                            probes[1]["frames"][0]["gasUsed"]
                        );
                    }
                    let priced = runs
                        .iter()
                        .rfind(|run| {
                            run["phase"] == "fee_simulation" && run["purpose"] == "native_source"
                        })
                        .unwrap();
                    for field in ["gas", "gasPrice", "maxFeePerGas", "maxPriorityFeePerGas"] {
                        assert_eq!(priced["transaction"][field], option[field], "{response}");
                    }
                }
                samples.push(json!({
                        "type":ty,"suppliedGas":supplied_gas,"trace":trace,"sample":sample,
                        "elapsedMs":elapsed_ms,"runs":runs.map(Vec::len),
                        "estimate":estimate,"context":detail["context"],"policy":detail["policy"],"option":option
                    }));
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
            std::path::Path::new(&directory).join(filename),
            serde_json::to_vec_pretty(&samples).unwrap(),
        )
        .unwrap();
    }
}
