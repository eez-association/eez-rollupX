//! Canonical receipt comparison for equal, quiescent 40-block workloads. The
//! receiving chain supplies its native blob fee floor; execution base fee is
//! held at zero in the native fixture so fee-market drift cannot fake savings.

use std::time::Duration;

use alloy_consensus::Transaction as _;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{B256, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{SolCall, SolEvent};
use eez_testkit::{
    ANVIL_ADDR_3, ANVIL_KEY_1, CrossChainWorld, NativeL1Profile, onchain_nonce, sign_and_send,
    wait_for,
};

const TIMEOUT: Duration = Duration::from_secs(180);
const RANGE: u64 = 40;

#[derive(Debug, serde::Serialize)]
struct Measurement {
    from: u64,
    through: u64,
    included_l2_transactions: usize,
    batch_hashes: Vec<B256>,
    execution_gas: u64,
    physical_blob_gas: u64,
    poster_cost: U256,
    bundle_cost: U256,
}

async fn measure(world: &CrossChainWorld, traffic: bool, policy: bool) -> Measurement {
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    // Finish every setup transaction before choosing the measured anchor.
    let latest = l2.get_block_number().await.unwrap();
    let start = wait_for(TIMEOUT, || async {
        Ok(l2
            .get_block_by_number(BlockNumberOrTag::Safe)
            .await?
            .filter(|block| block.header.number >= latest))
    })
    .await
    .unwrap()
    .header
    .number;
    let through = start + RANGE;
    let mut workload = Vec::new();
    if traffic {
        let nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1).await.unwrap();
        for offset in 0..2 {
            workload.push(
                sign_and_send(
                    &world.l2_rpc(),
                    ANVIL_KEY_1,
                    world.l2_chain_id,
                    nonce + offset,
                    Some(ANVIL_ADDR_3),
                    U256::from(1),
                    vec![],
                    21_000,
                )
                .await
                .unwrap(),
            );
        }
    }
    wait_for(TIMEOUT, || async {
        Ok(l2
            .get_block_by_number(BlockNumberOrTag::Safe)
            .await?
            .filter(|block| block.header.number >= through))
    })
    .await
    .unwrap();
    let mut measurement = Measurement {
        from: start + 1,
        through,
        included_l2_transactions: 0,
        batch_hashes: vec![],
        execution_gas: 0,
        physical_blob_gas: 0,
        poster_cost: U256::ZERO,
        bundle_cost: U256::ZERO,
    };
    for height in measurement.from..=through {
        measurement.included_l2_transactions += l2
            .get_block_by_number(height.into())
            .await
            .unwrap()
            .unwrap()
            .transactions
            .len();
    }
    assert_eq!(
        measurement.included_l2_transactions,
        workload.len(),
        "compare exactly the specified workload"
    );
    for hash in workload {
        let receipt = l2.get_transaction_receipt(hash).await.unwrap().unwrap();
        assert!(receipt.status());
        assert!((measurement.from..=through).contains(&receipt.block_number.unwrap()));
    }
    let logs = l1
        .get_logs(
            &alloy_rpc_types_eth::Filter::new()
                .address(world.dep.eez_address)
                .event_signature(eez_protocol::abi::BatchPosted::SIGNATURE_HASH)
                .from_block(0),
        )
        .await
        .unwrap();
    let mut ends = Vec::new();
    for log in logs {
        let hash = log.transaction_hash.unwrap();
        let tx = l1.get_transaction_by_hash(hash).await.unwrap().unwrap();
        let call = eez_protocol::abi::postAndVerifyBatchCall::abi_decode(tx.inner.input()).unwrap();
        let updates: Vec<_> = call
            .batch
            .entries
            .iter()
            .flat_map(|entry| &entry.rootUpdates)
            .filter(|update| update.rollupId == world.cfg.rollup_id)
            .collect();
        let last = updates.last().unwrap();
        let endpoint = l2.get_block_by_hash(last.newRoot).await.unwrap().unwrap();
        if endpoint.header.number <= start || endpoint.header.number > through {
            continue;
        }
        let parent = l2
            .get_block_by_hash(updates[0].currentRoot)
            .await
            .unwrap()
            .unwrap();
        assert!(
            parent.header.number >= start,
            "never charge a batch covering work outside the measured range"
        );
        let receipt = l1.get_transaction_receipt(hash).await.unwrap().unwrap();
        assert!(receipt.status());
        assert!(
            receipt
                .inner
                .logs()
                .iter()
                .filter_map(|log| log
                    .log_decode::<eez_protocol::abi::L2ExecutionPerformed>()
                    .ok())
                .any(|event| event.inner.data.rollupId == world.cfg.rollup_id
                    && event.inner.data.newRoot == last.newRoot)
        );
        let block = l1
            .get_block_by_hash(receipt.block_hash.unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            block.header.base_fee_per_gas,
            Some(0),
            "controlled native fee fixture"
        );
        assert_eq!(
            receipt.effective_gas_price,
            if policy {
                1_000_000_000
            } else {
                10_000_000_000
            }
        );
        let blob_gas = receipt.blob_gas_used.unwrap();
        assert_eq!(
            blob_gas,
            tx.inner.blob_versioned_hashes().unwrap().len() as u64
                * alloy_eips::eip4844::DATA_GAS_PER_BLOB
        );
        measurement.execution_gas += receipt.gas_used;
        measurement.physical_blob_gas += blob_gas;
        measurement.poster_cost += U256::from(receipt.gas_used)
            * U256::from(receipt.effective_gas_price)
            + U256::from(blob_gas) * U256::from(receipt.blob_gas_price.unwrap());
        measurement.batch_hashes.push(hash);
        ends.push((parent.header.number, endpoint.header.number));
    }
    ends.sort_unstable();
    let mut covered = start;
    for (parent, end) in ends {
        assert_eq!(parent, covered);
        covered = end;
    }
    assert_eq!(
        covered, through,
        "compare complete identical-length settled ranges"
    );
    // These measured workloads have no L1 companions.
    measurement.bundle_cost = measurement.poster_cost;
    measurement
}

async fn compare(profile: NativeL1Profile) {
    let env = vec![
        ("EEZ_SETTLEMENT_POLICY_ENABLED", "true".into()),
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", RANGE.to_string()),
        ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
        ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "100000000000".into()),
        (
            "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS",
            "100000000000".into(),
        ),
        ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "1000000000".into()),
    ];
    let mut reports = Vec::new();
    for policy in [false, true] {
        let world = eez_testkit::setup_cross_chain_with_fixed_fee_fixture(
            profile,
            if policy { &env } else { &[] },
        )
        .await
        .unwrap();
        let idle = measure(&world, false, policy).await;
        let transactions = measure(&world, true, policy).await;
        reports.push((idle, transactions));
    }
    for (baseline, policy) in [
        (&reports[0].0, &reports[1].0),
        (&reports[0].1, &reports[1].1),
    ] {
        assert_eq!(
            baseline.through - baseline.from,
            policy.through - policy.from
        );
        assert_eq!(
            baseline.included_l2_transactions,
            policy.included_l2_transactions
        );
        assert!(policy.batch_hashes.len() < baseline.batch_hashes.len());
        assert_eq!(policy.batch_hashes.len(), 1);
        assert!(policy.execution_gas < baseline.execution_gas);
        assert!(policy.physical_blob_gas < baseline.physical_blob_gas);
        assert!(policy.poster_cost < baseline.poster_cost);
    }
    if let Ok(directory) = std::env::var("EEZ_TEST_COST_EVIDENCE_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join(format!("{profile:?}.json")),
            serde_json::to_vec_pretty(&serde_json::json!({
                "profile": format!("{profile:?}"), "execution_base_fee": 0,
                "range_blocks": RANGE, "baseline": {"idle": reports[0].0, "transactions": reports[0].1},
                "policy": {"idle": reports[1].0, "transactions": reports[1].1},
            })).unwrap()).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_receipts_compare_equal_idle_and_transaction_workloads() {
    compare(NativeL1Profile::EthereumOsaka).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_receipts_compare_equal_idle_and_transaction_workloads() {
    compare(NativeL1Profile::GnosisPrague).await;
}

/// Exercise the real estimator, quote, signer, bundle preflight, and canonical
/// receipt. A zero floor makes the 20% uplift observable instead of masking it
/// behind the local PBS profile's minimum. This native harness uses no PBS bid.
async fn check_network_priority_margin(profile: NativeL1Profile) {
    let world = eez_testkit::setup_cross_chain_with_fixed_fee_fixture(
        profile,
        &[
            ("EEZ_SETTLEMENT_POLICY_ENABLED", "true".into()),
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
            ("EEZ_L1_POSTBATCH_PRIORITY_MARGIN_BPS", "2000".into()),
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
        assert_eq!(quote.fields["priority_source"], "network_estimator");
        assert_eq!(quote.u64("priority_min").unwrap(), 0);
        assert_eq!(quote.u64("priority_margin_bps").unwrap(), 2000);
        let estimate = quote.u64("estimated_priority_fee").unwrap();
        assert!(
            estimate > 0,
            "native estimator must exercise a nonzero uplift"
        );
        let expected = (u128::from(estimate) * 6).div_ceil(5);
        assert!(expected > u128::from(estimate));
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
            "network_priority_estimate": estimate, "priority_margin_bps": 2000,
            "selected_priority_fee": expected, "effective_gas_price": receipt.effective_gas_price,
            "gas_used": receipt.gas_used, "blob_gas_used": receipt.blob_gas_used,
            "blob_gas_price": receipt.blob_gas_price, "paid": cost.paid.total.to_string(),
            "canonical_l2_endpoint": end,
        }));
    }
    if let Ok(directory) = std::env::var("EEZ_TEST_COST_EVIDENCE_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join(format!("{profile:?}-priority-margin.json")),
            serde_json::to_vec_pretty(&measured).unwrap(),
        )
        .unwrap();
    }
    world.node.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_receipts_use_network_priority_with_percentage_margin() {
    check_network_priority_margin(NativeL1Profile::EthereumOsaka).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_receipts_use_network_priority_with_percentage_margin() {
    check_network_priority_margin(NativeL1Profile::GnosisPrague).await;
}
