//! Native receipt coverage for bounded L1 fee-history priority selection.

use std::time::Duration;

use alloy_consensus::Transaction as _;
use alloy_primitives::{B256, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{SolCall as _, SolEvent as _};
use eez_testkit::{NativeL1Profile, wait_for};

const TIMEOUT: Duration = Duration::from_mins(3);

/// Synthetic allowances make the candidate underfunded while exercising a
/// non-parity conversion. This is test input, not a production tariff.
pub(super) fn shadow_pricing_policy() -> serde_json::Value {
    serde_json::json!({
        "version":1, "l1_chain_id":eez_testkit::DEV_CHAIN_ID, "l2_chain_id":1, "rollup_id":1,
        "beneficiary":alloy_primitives::Address::repeat_byte(0x44),
        "l1_per_l2_numerator":"2", "l1_per_l2_denominator":"3",
        "proving_cost_l1":"1000000000000000", "operations_cost_l1":"2000000000000000",
        "treasury_cost_l1":"3000000000000000", "risk_bps":1000, "profit_bps":500,
        "excluded_senders":[],
    })
}

/// Supply the cost assumptions used by receipt assertions through the real
/// scalar startup settings. Network identity comes from the test node itself.
pub(super) fn fee_settings(
    policy: &serde_json::Value,
    extra: &[(&'static str, String)],
) -> Vec<(&'static str, String)> {
    let mut settings = vec![(
        "EEZ_L2_FEE_RECIPIENT",
        policy["beneficiary"].as_str().unwrap().to_owned(),
    )];
    for (name, field) in [
        ("EEZ_FEES_L1_PER_L2_NUMERATOR", "l1_per_l2_numerator"),
        ("EEZ_FEES_L1_PER_L2_DENOMINATOR", "l1_per_l2_denominator"),
        ("EEZ_FEES_PROVING_COST_L1", "proving_cost_l1"),
        ("EEZ_FEES_OPERATIONS_COST_L1", "operations_cost_l1"),
        ("EEZ_FEES_TREASURY_COST_L1", "treasury_cost_l1"),
        ("EEZ_FEES_RISK_BPS", "risk_bps"),
        ("EEZ_FEES_PROFIT_BPS", "profit_bps"),
    ] {
        let value = &policy[field];
        settings.push((
            name,
            value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string()),
        ));
    }
    settings.push((
        "EEZ_FEES_EXCLUDED_SENDERS",
        policy["excluded_senders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>()
            .join(","),
    ));
    settings.extend_from_slice(extra);
    settings
}

/// Compare the production shadow observation against independently fetched
/// canonical receipts. Call after the scenario has established full settlement.
pub(super) async fn assert_l2_shadow_receipts(
    world: &eez_testkit::CrossChainWorld,
    source_count: u64,
    companion_count: u64,
    expected_policy: Option<&serde_json::Value>,
) {
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let events: Vec<serde_json::Value> = world
        .node
        .signals_since(0)
        .unwrap()
        .into_iter()
        .filter(|event| event.name == "eez.composer.admission.v1")
        .map(|event| {
            serde_json::from_str(event.fields["admission_event"].as_str().unwrap()).unwrap()
        })
        .collect();
    let mut checked = 0;
    for event in events.iter().filter(|event| {
        event["kind"] == "preflight"
            && event["detail"]["fee_observation"]["l2"]["source_count"] == source_count
    }) {
        let report = &event["detail"]["fee_observation"];
        let post_hash: B256 = report["l1"]["transaction_hash"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let Some(receipt) = l1.get_transaction_receipt(post_hash).await.unwrap() else {
            continue;
        };
        assert!(receipt.status());
        let post_block = l1
            .get_block_by_hash(receipt.block_hash.unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            report["l1"]["sample_hash"],
            serde_json::json!(post_block.header.parent_hash)
        );
        assert_eq!(report["l1"]["target_block"], post_block.header.number);
        assert_eq!(
            report["l1"]["target_timestamp"],
            post_block.header.timestamp
        );
        assert_eq!(report["l1"]["companion_count"], companion_count);
        let actual = eez_l1::posting_fees::ReceiptCost::from_receipt(&receipt).unwrap();
        assert_eq!(
            report["l1"]["estimated_execution_cost"],
            actual.paid.execution.to_string()
        );
        assert_eq!(
            report["l1"]["estimated_blob_cost"],
            actual.paid.blobs.to_string()
        );
        assert_eq!(
            report["l1"]["estimated_post_cost"],
            actual.paid.total.to_string()
        );
        assert_eq!(report["l1"]["gas_used"], receipt.gas_used);
        assert_eq!(
            report["l1"]["physical_blobs"].as_u64().unwrap()
                * alloy_eips::eip4844::DATA_GAS_PER_BLOB,
            receipt.blob_gas_used.unwrap()
        );

        let block_hash: B256 = report["l2"]["block_hash"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let block = l2.get_block_by_hash(block_hash).await.unwrap().unwrap();
        assert_eq!(
            l2.get_block_by_number(block.header.number.into())
                .await
                .unwrap()
                .unwrap()
                .header
                .hash,
            block_hash
        );
        assert_eq!(
            report["l2"]["beneficiary"],
            serde_json::json!(block.header.beneficiary)
        );
        assert_eq!(
            report["l2"]["parent_hash"],
            serde_json::json!(block.header.parent_hash)
        );
        assert_eq!(report["l2"]["chain_id"], world.l2_chain_id);
        // Generic Ethereum receipt enums do not include either native type.
        let receipts: Vec<serde_json::Value> = l2
            .raw_request("eth_getBlockReceipts".into(), [block_hash])
            .await
            .unwrap();
        let quantity = |value: &serde_json::Value| {
            U256::from_str_radix(value.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
        };
        let base = U256::from(block.header.base_fee_per_gas.unwrap());
        let mut ordinary_gas = U256::ZERO;
        let mut native_gas = U256::ZERO;
        let mut tips = U256::ZERO;
        let mut sources = 0;
        for receipt in receipts {
            let gas = quantity(&receipt["gasUsed"]);
            if receipt["type"] == "0x45" || receipt["type"] == "0x46" {
                native_gas += gas;
                assert_eq!(quantity(&receipt["effectiveGasPrice"]), U256::ZERO);
                continue;
            }
            ordinary_gas += gas;
            sources += 1;
            let tip = gas * (quantity(&receipt["effectiveGasPrice"]) - base);
            tips += tip;
            let source = events
                .iter()
                .find(|source| {
                    source["candidate_id"] == event["candidate_id"]
                        && source["kind"] == "source_preflight"
                        && source["detail"]["source_hash"] == receipt["transactionHash"]
                })
                .expect("canonical L2 source has native fee evidence in its candidate");
            assert_eq!(
                source["detail"]["preflight_basis"],
                "native_l2_terminal_receipt"
            );
            assert_eq!(source["detail"]["execution_chain_id"], world.l2_chain_id);
            assert_eq!(
                source["detail"]["preflight_used_gas"].as_u64().unwrap(),
                gas.to::<u64>()
            );
            assert!(source["detail"]["preflight_required_gas"].is_null());
            assert_eq!(
                source["detail"]["fee_observation"]["gross_priority_fee"],
                tip.to_string()
            );
            assert_eq!(
                source["detail"]["fee_observation"]["base_fee_burn"],
                (gas * base).to_string()
            );
        }
        assert_eq!(sources, source_count);
        assert!(native_gas > U256::ZERO, "exercise native loads/deliveries");
        assert!(
            tips > U256::ZERO,
            "exercise real ordinary priority payments"
        );
        assert_eq!(
            report["l2"]["ordinary_gas_used"].as_u64().unwrap(),
            ordinary_gas.to::<u64>()
        );
        assert_eq!(
            report["l2"]["native_gas_used"].as_u64().unwrap(),
            native_gas.to::<u64>()
        );
        assert_eq!(report["l2"]["gross_priority_fees"], tips.to_string());
        assert_eq!(
            report["l2"]["base_fee_burn"],
            (ordinary_gas * base).to_string()
        );
        assert_eq!(ordinary_gas + native_gas, U256::from(block.header.gas_used));
        assert!(report["qualified_recovery_l1"].is_null());
        if let Some(policy) = expected_policy {
            assert_eq!(report["version"], 2);
            assert_eq!(report["pricing_status"], "shadow_priced");
            let pricing = &report["pricing"];
            assert_eq!(pricing["assumptions"], *policy);
            assert_eq!(
                pricing["policy_id"],
                serde_json::json!(alloy_primitives::keccak256(policy.to_string()))
            );
            assert_eq!(pricing["eligible_source_count"], source_count);
            assert_eq!(pricing["eligible_priority_fees_l2"], tips.to_string());
            assert_eq!(pricing["excluded_priority_fees_l2"], "0");
            // Independent receipt-based oracle with fixture-sized u128 values.
            let subtotal = actual.paid.total.to::<u128>() + 6_000_000_000_000_000;
            let risk = (subtotal * 1000).div_ceil(10_000);
            let profit = (subtotal * 500).div_ceil(10_000);
            let required_l1 = subtotal + risk + profit;
            let required_l2 = (required_l1 * 3).div_ceil(2);
            let recovered = tips.to::<u128>() * 2 / 3;
            assert!(
                required_l1 > recovered,
                "underfunding must not stop shadow-mode execution"
            );
            for (field, expected) in [
                ("subtotal_l1", subtotal),
                ("risk_allowance_l1", risk),
                ("profit_allowance_l1", profit),
                ("required_service_payment_l1", required_l1),
                ("required_service_payment_l2", required_l2),
                ("projected_recovery_l1", recovered),
                ("shortfall_l1", required_l1 - recovered),
            ] {
                assert_eq!(pricing[field], expected.to_string(), "{field}");
            }
            assert_eq!(pricing["comparison"], "shortfall");
            assert_eq!(pricing["surplus_l1"], "0");
            if source_count == 1 && companion_count == 0 {
                let required_tip = required_l2.div_ceil(ordinary_gas.to::<u128>());
                assert_eq!(pricing["tip_status"], "single_source_candidate");
                assert_eq!(report["required_tip_per_gas"], required_tip.to_string());
                assert_eq!(
                    pricing["minimum_max_fee_per_gas_at_current_base"],
                    (required_tip + base.to::<u128>()).to_string()
                );
                assert!(required_tip * ordinary_gas.to::<u128>() * 2 / 3 >= required_l1);
            } else {
                assert_eq!(pricing["tip_status"], "standalone_estimate_required");
                assert!(report["required_tip_per_gas"].is_null());
            }
        } else {
            assert_eq!(report["pricing_status"], "unpriced");
            assert_eq!(report["pricing"]["status"], "not_configured");
            assert!(report["required_tip_per_gas"].is_null());
        }
        checked += 1;
    }
    assert_eq!(
        checked, 1,
        "one fully settled candidate must have receipt-backed fee evidence"
    );
}

/// Exercise the real estimator, bounded quote, signer, bundle preflight, and
/// canonical receipt. This native harness uses no PBS bid.
async fn check_bounded_fee_history_priority(profile: NativeL1Profile) {
    let world = eez_testkit::setup_cross_chain_with_fixed_fee_fixture(
        profile,
        &[
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
            ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
            ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10".into()),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "100000000000".into()),
            (
                "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS",
                "100000000000".into(),
            ),
            ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "0".into()),
            ("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "100000000000".into()),
            // These posts carry no cross-chain call. A 1 wei cross-chain
            // ceiling would visibly change their price if it leaked to them.
            (
                "EEZ_L1_POSTBATCH_CROSS_CHAIN_PRIORITY_PERCENTILE",
                "99".into(),
            ),
            ("EEZ_L1_POSTBATCH_CROSS_CHAIN_MAX_PRIORITY_FEE", "1".into()),
        ],
    )
    .await
    .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let posts = wait_for(TIMEOUT, || async {
        let logs = l1
            .get_logs(
                &alloy_rpc_types_eth::Filter::new()
                    .address(world.dep.eez_address)
                    .event_signature(eez_protocol::abi::BatchPosted::SIGNATURE_HASH)
                    .from_block(0),
            )
            .await?;
        Ok((logs.len() >= 2).then_some(logs))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "margin posting failed: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    let mut measured = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for post in posts.iter().take(2) {
        let hash = post.transaction_hash.unwrap();
        assert!(seen.insert(hash), "verify distinct included transactions");
        let tx = l1.get_transaction_by_hash(hash).await.unwrap().unwrap();
        let receipt = l1.get_transaction_receipt(hash).await.unwrap().unwrap();
        assert!(receipt.status());
        let repriced = world
            .node
            .signals_since(0)
            .unwrap()
            .into_iter()
            .filter(|event| event.name == "eez.composer.posting.v1")
            .map(|event| {
                serde_json::from_str::<serde_json::Value>(
                    event.fields["posting_event"].as_str().unwrap(),
                )
                .unwrap()
            })
            .find(|event| {
                event["stage"] == "repriced" && event["transaction_hash"] == serde_json::json!(hash)
            })
            .expect("included transaction must retain its final native pricing evidence");
        let required = repriced["detail"]["preflight_gas_required"][0]
            .as_u64()
            .unwrap();
        assert_eq!(
            tx.inner.gas_limit(),
            (required * 10_500).div_ceil(10_000),
            "the absent setting must reach final signing as the 5% default"
        );
        let block = l1
            .get_block_by_hash(receipt.block_hash.unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(block.header.base_fee_per_gas, Some(0));
        let quote = world
            .node
            .signals_since(0)
            .unwrap()
            .into_iter()
            .find(|event| {
                event.name == "eez.composer.postbatch.fee_quote"
                    && event.u64("sample_block").ok() == Some(block.header.number - 1)
                    && event.u64("priority_fee").ok().map(u128::from)
                        == tx.inner.max_priority_fee_per_gas()
            })
            .expect("included poster must match a fresh quote for its canonical parent");
        assert_eq!(quote.fields["priority_source"], "fee_history");
        assert_eq!(quote.fields["priority_profile"], "Base");
        assert_eq!(quote.u64("priority_reward_percentile").unwrap(), 90);
        assert_eq!(quote.u64("priority_history_blocks").unwrap(), 20);
        assert_eq!(quote.u64("priority_min").unwrap(), 0);
        assert_eq!(quote.u64("priority_max").unwrap(), 100_000_000_000);
        let estimate = quote.u64("estimated_priority_fee").unwrap();
        assert!(
            estimate > 0,
            "native estimator must exercise a nonzero quote"
        );
        let expected = u128::from(estimate).min(100_000_000_000);
        assert_eq!(tx.inner.max_priority_fee_per_gas(), Some(expected));
        assert_eq!(receipt.effective_gas_price, expected);

        let call = eez_protocol::abi::postAndVerifyBatchCall::abi_decode(tx.inner.input()).unwrap();
        let end = call
            .batch
            .entries
            .iter()
            .flat_map(|entry| &entry.rootUpdates)
            .rev()
            .find(|update| update.rollupId == world.cfg.rollup_id)
            .unwrap()
            .newRoot;
        let applied = receipt
            .inner
            .logs()
            .iter()
            .filter_map(|log| {
                log.log_decode::<eez_protocol::abi::L2ExecutionPerformed>()
                    .ok()
            })
            .any(|event| {
                event.inner.data.rollupId == world.cfg.rollup_id && event.inner.data.newRoot == end
            });
        assert!(applied, "fee repricing must preserve complete settlement");
        let endpoint = l2.get_block_by_hash(end).await.unwrap().unwrap();
        assert_eq!(
            l2.get_block_by_number(endpoint.header.number.into())
                .await
                .unwrap()
                .unwrap()
                .header
                .hash,
            end,
            "settlement must cover canonical L2 history"
        );
        let blobs = tx.inner.blob_versioned_hashes().unwrap().len() as u64;
        assert_eq!(
            receipt.blob_gas_used,
            Some(blobs * alloy_eips::eip4844::DATA_GAS_PER_BLOB)
        );
        let cost = eez_l1::posting_fees::ReceiptCost::from_receipt(&receipt).unwrap();
        let accounted = wait_for(TIMEOUT, || async {
            Ok(world.node.signals_since(0)?.into_iter().find(|event| {
                event.name == "eez.composer.postbatch.actual_cost"
                    && event.fields["post_batch_hash"] == hash.to_string()
                    && event.fields["action"] == "apply"
            }))
        })
        .await
        .unwrap();
        assert_eq!(
            accounted.fields["post_total_cost"],
            cost.paid.total.to_string()
        );
        measured.push(serde_json::json!({
            "transaction": hash, "l1_block": block.header.number,
            "fee_history_priority_estimate": estimate, "priority_max": 100_000_000_000_u64,
            "selected_priority_fee": expected, "effective_gas_price": receipt.effective_gas_price,
            "gas_used": receipt.gas_used, "blob_gas_used": receipt.blob_gas_used,
            "blob_gas_price": receipt.blob_gas_price, "paid": cost.paid.total.to_string(),
            "canonical_l2_endpoint": end,
        }));
    }
    if let Ok(directory) = std::env::var("EEZ_TEST_COST_EVIDENCE_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join(format!("{profile:?}-fee-history-priority.json")),
            serde_json::to_vec_pretty(&measured).unwrap(),
        )
        .unwrap();
    }
    world.node.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_receipts_use_bounded_fee_history_priority() {
    check_bounded_fee_history_priority(NativeL1Profile::EthereumOsaka).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_receipts_use_bounded_fee_history_priority() {
    check_bounded_fee_history_priority(NativeL1Profile::GnosisPrague).await;
}
