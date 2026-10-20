use super::*;
use eez_testkit::{
    CrossChainOverlayRoundTripWorld, IOverlayDriverL1, IOverlayProbeL2,
    setup_cross_chain_overlay_round_trip_with_env,
};

// Public, local-only fixture key. This account starts unfunded and is distinct
// from the fixture's deployment wallets, source users and fee recipient.
const POSTER_KEY: &str = "3737373737373737373737373737373737373737373737373737373737373737";

async fn world(funded: bool) -> CrossChainOverlayRoundTripWorld {
    let policy = crate::settlement_policy_cost::shadow_pricing_policy();
    let settings = crate::settlement_policy_cost::fee_settings(
        &policy,
        &[
            ("EEZ_L1_POSTER_KEY", POSTER_KEY.into()),
            ("EEZ_L1_POSTBATCH_PRIORITY_FEE", "1000000000".into()),
            ("EEZ_MAX_POSTBATCH_GAS", "16777216".into()),
            ("EEZ_FEES_L1_TO_L2_ENABLED", "true".into()),
            ("EEZ_FEES_L2_TO_L1_ENABLED", "true".into()),
            ("EEZ_FEES_L1_REFUND_PERCENT", "90".into()),
            (
                "EEZ_FEES_L1_REFUND_RECIPIENT",
                Address::repeat_byte(0x97).to_string(),
            ),
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
            ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
            ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10000".into()),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
        ],
    );
    let world = setup_cross_chain_overlay_round_trip_with_env(&settings)
        .await
        .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let poster: PrivateKeySigner = POSTER_KEY.parse().unwrap();
    if funded {
        let hash = sign_and_send(
            &world.l1_rpc(),
            TARGET_DEPLOYER,
            l1.get_chain_id().await.unwrap(),
            onchain_nonce(&world.l1_rpc(), TARGET_DEPLOYER)
                .await
                .unwrap(),
            Some(poster.address()),
            U256::from(5_000_000_000_000_000u64),
            vec![],
            21_000,
        )
        .await
        .unwrap();
        assert!(
            wait_for(TIMEOUT, || async {
                Ok(l1.get_transaction_receipt(hash).await?)
            })
            .await
            .unwrap()
            .status()
        );
    }
    let minimum = l1.get_block_number().await.unwrap();
    // Observe completed quote context through an ordinary L2 transfer. A
    // cross-chain readiness quote would itself require the poster's funding.
    let client = reqwest::Client::new();
    let source: PrivateKeySigner = OUTBOUND_USER.parse().unwrap();
    wait_for(TIMEOUT, || async {
        let response = quote_rpc(
            &client,
            &world.l2_xchain(),
            "eez_debugQuoteFee",
            json!([{"from":source.address(),"to":Address::repeat_byte(0x89)},
                {"transactionTypes":["0x2"],"trace":"calls"}]),
        )
        .await?;
        Ok(
            response["result"]["diagnostics"]["trace"]["context"]["l1ParentNumber"]
                .as_u64()
                .filter(|number| *number >= minimum)
                .map(|_| ()),
        )
    })
    .await
    .unwrap();
    let balance = l1.get_balance(poster.address()).await.unwrap();
    assert_eq!(
        balance,
        if funded {
            U256::from(5_000_000_000_000_000u64)
        } else {
            U256::ZERO
        }
    );
    // Even the configured priority alone makes the old maximum-gas probe
    // unaffordable. The nested workload needs substantially less gas.
    assert!(balance < U256::from(16_777_216u64) * U256::from(1_000_000_000u64));
    world
}

fn request(world: &CrossChainOverlayRoundTripWorld, inbound: bool) -> (String, Value) {
    let (rpc, key, to, data) = if inbound {
        (
            world.l1_xchain(),
            INBOUND_USER,
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
            OUTBOUND_USER,
            world.probe,
            IOverlayProbeL2::startDeliverThenBurnAndPingCall {
                driver: world.driver_on_l2,
                probe: world.probe_on_l1,
                amount: U256::from(7),
            }
            .abi_encode(),
        )
    };
    let sender: PrivateKeySigner = key.parse().unwrap();
    (
        rpc,
        json!({"from":sender.address(),"to":to,"data":Bytes::from(data)}),
    )
}

async fn affordable_poster_nested_quotes(inbound: bool) {
    let world = world(true).await;
    let client = reqwest::Client::new();
    for explicit_gas in [false, true] {
        let (rpc, mut tx) = request(&world, inbound);
        if explicit_gas {
            tx["gas"] = json!("0x5b8d80");
        }
        let response = quote_rpc(
            &client,
            &rpc,
            "eez_debugQuoteFee",
            json!([tx,{"trace":"calls"}]),
        )
        .await
        .unwrap();
        tracing::save_trace(
            &format!("affordable-poster-{inbound}-{explicit_gas}"),
            &response,
        );
        assert!(response.get("error").is_none(), "{response}");
        let options = response["result"]["options"].as_array().unwrap();
        assert_eq!(options.len(), 2);
        for option in options {
            assert!(option.get("error").is_none(), "{response}");
        }
        let runs = response["result"]["diagnostics"]["trace"]["runs"]
            .as_array()
            .unwrap();
        assert!(runs.iter().any(|run| {
            run["frames"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|frame| frame["crossChain"] == true)
                .count()
                >= 3
        }));
        let priced_posts: Vec<_> = runs
            .iter()
            .filter(|run| {
                (run["purpose"] == "settlement" || run["purpose"] == "conditional_setup")
                    && run["phase"] == "fee_simulation"
            })
            .collect();
        assert!(!priced_posts.is_empty(), "{response}");
        for run in priced_posts {
            let gas = u64::from_str_radix(
                run["transaction"]["gas"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("0x"),
                16,
            )
            .unwrap();
            assert!(gas < 16_777_216, "{run}");
            assert_eq!(run["complete"], true);
        }
    }
    let poster: PrivateKeySigner = POSTER_KEY.parse().unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    assert_eq!(
        l1.get_balance(poster.address()).await.unwrap(),
        U256::from(5_000_000_000_000_000u64)
    );
    assert_eq!(l1.get_transaction_count(poster.address()).await.unwrap(), 0);
}

async fn unfunded_poster_quotes_preserve_partial_trace(inbound: bool) {
    let world = world(false).await;
    let client = reqwest::Client::new();
    let (rpc, tx) = request(&world, inbound);
    for ty in ["0x0", "0x2"] {
        for method in ["eez_quoteFee", "eez_debugQuoteFee"] {
            let options = if method == "eez_debugQuoteFee" {
                json!({"transactionTypes":[ty],"trace":"calls"})
            } else {
                json!({"transactionTypes":[ty]})
            };
            let response = quote_rpc(&client, &rpc, method, json!([tx, options]))
                .await
                .unwrap();
            assert!(response.get("result").is_none(), "{response}");
            let error = &response["error"]["data"];
            assert_eq!(error["code"], "poster_insufficient_funds", "{response}");
            assert_eq!(error["details"]["scope"], "poster");
            assert_eq!(error["details"]["availableBalance"], "0x0");
            if method == "eez_debugQuoteFee" {
                tracing::save_trace(&format!("unfunded-poster-{inbound}-{ty}"), &response);
                let trace = &error["trace"];
                assert_eq!(trace["format"], "eez-quote-call-trace");
                assert_eq!(trace["partial"], true);
                assert_eq!(trace["failure"]["code"], "poster_insufficient_funds");
                assert!(trace["runs"].as_array().unwrap().iter().any(|run| {
                    run["frames"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|frame| frame["crossChain"] == true)
                }));
            } else {
                assert!(error.get("trace").is_none());
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn affordable_poster_nested_l1_quotes() {
    affordable_poster_nested_quotes(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn affordable_poster_nested_l2_quotes() {
    affordable_poster_nested_quotes(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unfunded_poster_l1_quotes_preserve_partial_trace() {
    unfunded_poster_quotes_preserve_partial_trace(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unfunded_poster_l2_quotes_preserve_partial_trace() {
    unfunded_poster_quotes_preserve_partial_trace(false).await;
}
