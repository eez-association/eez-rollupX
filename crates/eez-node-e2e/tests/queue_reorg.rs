//! A real L1 fork while later cross-chain generations are still unresolved.
use alloy_primitives::U256;
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
    let env = [
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
    // Exact-target retries can use a different post hash from the initial
    // dispatch. The reconciled owner identifies the actual canonical L1 block.
    let owner: serde_json::Value = l2
        .client()
        .request("eez_getCrossChainTransaction", (sources[0],))
        .await
        .unwrap();
    let first_height = owner["ownership"]["canonical"]["number"].as_u64().unwrap();
    let original_l1_hash = owner["ownership"]["canonical"]["hash"]
        .as_str()
        .unwrap()
        .to_owned();
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
