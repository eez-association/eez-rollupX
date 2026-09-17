//! A real L1 fork while later cross-chain generations are still unresolved.
#[path = "common/observer.rs"]
mod observer;

use alloy_primitives::{B256, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::SolCall;
use eez_testkit::{
    ANVIL_KEY_4, IValue, NodeBinary, NodeConfig, NodeHandle, l2_value, onchain_nonce,
    setup_cross_chain_with_l1_reorg, sign_and_send, wait_for, wait_for_safe_prefix_convergence,
};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(240);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_l1_reorg_recovers_confirmed_and_unresolved_source_generations_in_nonce_order() {
    let telemetry = observer::Telemetry::new("native-queue-reorg").unwrap();
    let env = [
        telemetry.env(),
        ("EEZ_SETTLEMENT_POLICY_ENABLED", "true".into()),
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "always".into()),
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "1800".into()),
        ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
        ("EEZ_MAX_USER_TXS_PER_BUNDLE", "1".into()),
        ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "500000000000".into()),
        (
            "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS",
            "100000000000".into(),
        ),
        ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "1000000001".into()),
    ];
    let mut world = setup_cross_chain_with_l1_reorg(&env).await.unwrap();
    let observer = observer::Observer::start(&world, &telemetry.spool, telemetry.deployment)
        .await
        .unwrap();
    let follower = NodeHandle::start(
        "l1-reorg-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap();
    let mut sources = Vec::new();
    let send = |offset: u64| {
        let rpc = world.l2_xchain();
        let chain_id = world.l2_chain_id;
        let target = world.outbound_proxy;
        async move {
            sign_and_send(
                &rpc,
                ANVIL_KEY_4,
                chain_id,
                nonce + offset,
                Some(target),
                U256::ZERO,
                IValue::setValueCall {
                    v: U256::from(31 + offset),
                }
                .abi_encode(),
                400_000,
            )
            .await
        }
    };
    sources.push(send(0).await.unwrap());
    let original = wait_for(TIMEOUT, || async {
        let status: serde_json::Value = l2
            .client()
            .request("eez_getCrossChainTransaction", (sources[0],))
            .await?;
        if status["ownership"]["state"] != "terminal"
            || status["ownership"]["reason"] != "Committed"
        {
            return Ok(None);
        }
        Ok(l2.get_transaction_receipt(sources[0]).await?)
    })
    .await
    .unwrap_or_else(|e| {
        panic!(
            "first source did not settle: {e:#}{}",
            world.settlement_diagnostics()
        )
    });
    assert!(original.status());
    wait_for_safe_prefix_convergence(
        &[&world.node, &follower],
        original.block_number.unwrap(),
        TIMEOUT,
    )
    .await
    .unwrap();
    // The queue's canonical field is a reconciliation watermark, not posting
    // inclusion. Fork the exact actual post occurrence from block correlation.
    let original_settlement: serde_json::Value = l2
        .client()
        .request(
            "eez_getSettlementByL2Block",
            (original.block_hash.unwrap(),),
        )
        .await
        .unwrap();
    let first_height = u64::from_str_radix(
        original_settlement["l1BlockNumber"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let original_l1_hash = original_settlement["l1BlockHash"]
        .as_str()
        .unwrap()
        .to_owned();
    let original_post: B256 = original_settlement["l1TransactionHash"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        l1.get_block_by_number(first_height.into())
            .await
            .unwrap()
            .unwrap()
            .header
            .hash
            .to_string(),
        original_l1_hash
    );
    observer.outcome(sources[0], "committed").await.unwrap();
    observer
        .receipt(&world.l2_rpc(), "l2", sources[0])
        .await
        .unwrap();
    observer.allocation(sources[0]).await.unwrap();
    observer
        .receipt(&world.l1_rpc(), "l1", original_post)
        .await
        .unwrap();
    observer.paid_post(original_post, 1).await.unwrap();
    let original_block: B256 = original_l1_hash.parse().unwrap();
    observer.wait_sql(&format!("SELECT count(*)=1 FROM eez.blocks b JOIN eez.chain_instances c ON c.id=b.chain_instance_id WHERE c.role='l1' AND b.hash=decode('{original_block:x}','hex') AND b.canonical AND NOT b.finalized"), "t").await.unwrap();
    let ancestor = l1
        .get_block_by_number((first_height - 1).into())
        .await
        .unwrap()
        .unwrap();
    world.hold_next_bundle().unwrap();
    sources.push(send(1).await.unwrap());
    let mut held = wait_for(TIMEOUT, || async { world.held_bundle() })
        .await
        .unwrap();
    wait_for(TIMEOUT, || async {
        Ok(l2.get_transaction_receipt(sources[1]).await?)
    })
    .await
    .unwrap();
    sources.push(send(2).await.unwrap());
    let replacement = world
        .l1_reorg_control
        .as_mut()
        .unwrap()
        .reorg_to(ancestor.header.number, ancestor.header.hash)
        .await
        .unwrap();
    assert_eq!(
        replacement["replacement"]["number"],
        format!("0x{first_height:x}")
    );
    assert_ne!(replacement["replacement"]["hash"], original_l1_hash);
    // Release the obsolete later proof as an explicit exclusion only after
    // the confirmed predecessor's L1 ancestry has actually been replaced.
    held["params"][0]["txs"] = serde_json::json!([]);
    world.release_bundle(&held).unwrap();
    world.node.wait_for_reorg_seen(TIMEOUT).await.unwrap();
    follower.wait_for_reorg_seen(TIMEOUT).await.unwrap();
    observer.wait_sql(&format!("SELECT count(*)=1 FROM eez.blocks b JOIN eez.chain_instances c ON c.id=b.chain_instance_id WHERE c.role='l1' AND b.hash=decode('{original_block:x}','hex') AND NOT b.canonical"), "t").await.unwrap();
    observer.wait_sql(&format!("SELECT NOT EXISTS(SELECT FROM eez.canonical_receipt_facts WHERE role='l1' AND block_hash=decode('{original_block:x}','hex')) AND NOT EXISTS(SELECT FROM eez.source_posting_allocations WHERE post_block_hash=decode('{original_block:x}','hex'))"), "t").await.unwrap();
    let mut covered = 0;
    let mut blocks = Vec::new();
    for (index, hash) in sources.iter().enumerate() {
        let repaired = wait_for(TIMEOUT, || async {
            let Some(receipt) = l2.get_transaction_receipt(*hash).await? else {
                return Ok(None);
            };
            let safe = l2
                .get_block_by_number(alloy_rpc_types_eth::BlockNumberOrTag::Safe)
                .await?;
            let owner: serde_json::Value = l2
                .client()
                .request("eez_getCrossChainTransaction", (*hash,))
                .await?;
            // The orphaned post may itself be re-included, legitimately
            // restoring the same L2 block. Require fresh canonical L1 evidence
            // instead of assuming a reorg must change the L2 execution hash.
            Ok(safe
                .filter(|safe| safe.header.number >= receipt.block_number.unwrap())
                .filter(|_| owner["ownership"]["reason"] == "Committed")
                .filter(|_| {
                    index != 0 || owner["ownership"]["canonical"]["hash"] != original_l1_hash
                })
                .map(|_| receipt))
        })
        .await
        .unwrap_or_else(|e| {
            panic!(
                "source generation {index} not recovered: {e:#}{}",
                world.settlement_diagnostics()
            )
        });
        assert!(repaired.status());
        covered = covered.max(repaired.block_number.unwrap());
        blocks.push(repaired.block_number.unwrap());
    }
    assert!(
        blocks.windows(2).all(|pair| pair[0] < pair[1]),
        "limit-one sources must execute in nonce order"
    );
    wait_for_safe_prefix_convergence(&[&world.node, &follower], covered, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap(),
        nonce + 3
    );
    assert_eq!(
        l2_value(&world.l1_rpc(), world.outbound_value)
            .await
            .unwrap(),
        U256::from(33)
    );
    for hash in sources {
        observer.outcome(hash, "committed").await.unwrap();
        let source_receipt = observer.receipt(&world.l2_rpc(), "l2", hash).await.unwrap();
        let settled: serde_json::Value = l2
            .client()
            .request(
                "eez_getSettlementByL2Block",
                (source_receipt["blockHash"].as_str().unwrap(),),
            )
            .await
            .unwrap();
        let current_post: B256 = settled["l1TransactionHash"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        assert_ne!(settled["l1BlockHash"], original_l1_hash);
        observer
            .receipt(&world.l1_rpc(), "l1", current_post)
            .await
            .unwrap();
        observer.paid_post(current_post, 1).await.unwrap();
        observer.allocation(hash).await.unwrap();
        wait_for(TIMEOUT, || async {
            let status: serde_json::Value = l2
                .client()
                .request("eez_getCrossChainTransaction", (hash,))
                .await?;
            Ok((status["ownership"]["reason"] == "Committed").then_some(()))
        })
        .await
        .unwrap();
    }
}
