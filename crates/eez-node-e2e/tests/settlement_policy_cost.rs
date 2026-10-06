//! Native receipt coverage for bounded L1 fee-history priority selection.

use std::time::Duration;

use alloy_consensus::Transaction as _;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{SolCall as _, SolEvent as _};
use eez_testkit::{NativeL1Profile, wait_for};

const TIMEOUT: Duration = Duration::from_mins(3);

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
