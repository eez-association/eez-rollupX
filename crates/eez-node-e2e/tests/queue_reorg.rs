//! A real L1 fork while later cross-chain generations are still unresolved.
use alloy_primitives::U256;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::SolCall;
use eez_testkit::{
    ANVIL_KEY_4, IValue, NodeBinary, NodeConfig, NodeHandle, l2_value, onchain_nonce,
    setup_cross_chain_with_l1_reorg, sign_and_send, wait_for, wait_for_safe_prefix_convergence,
};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_mins(4);

/// A fork can invalidate imported history without removing any settlement.
/// Recovered ordinary transactions must retain local pool capacity and execute
/// once on the replacement history, without client resubmission.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn imported_history_reorg_recovers_twenty_ordinary_transactions_without_settlement() {
    use alloy_primitives::Bytes;
    use alloy_rpc_types_eth::{BlockNumberOrTag, TransactionRequest};
    use eez_protocol::l1_history::{HISTORY_ADDRESS, IL1BlockhashContract};
    use eez_testkit::{ANVIL_ADDR_3, ANVIL_KEY_1};

    let mut world = setup_cross_chain_with_l1_reorg(&[
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10000".into()),
        ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
    ])
    .await
    .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let setup_tip = l1.get_block_number().await.unwrap();
    let current = TransactionRequest::default().to(HISTORY_ADDRESS).input(
        Bytes::from(IL1BlockhashContract::getLatestImportedL1BlockCall {}.abi_encode()).into(),
    );
    // Fork only history produced after every fixture contract was installed.
    wait_for(TIMEOUT, || async {
        let source = l2.call(current.clone()).latest().await?;
        let source =
            IL1BlockhashContract::getLatestImportedL1BlockCall::abi_decode_returns(&source)?;
        Ok((source.number > setup_tip).then_some(()))
    })
    .await
    .unwrap();
    let before_chain = world.chain().snapshot().await.unwrap();
    assert_eq!(
        before_chain.batches_posted, 0,
        "fixture must have no accepted batch to remove"
    );
    assert_eq!(before_chain.state_root, world.cfg.initial_state);
    let balance = l2.get_balance(ANVIL_ADDR_3).await.unwrap();
    let nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1).await.unwrap();
    let mut hashes = Vec::new();
    // Twenty exceeds Reth's external per-sender capacity of sixteen. Losing
    // local origin during rollback would drop a suffix of this nonce chain.
    for offset in 0..20 {
        hashes.push(
            sign_and_send(
                &world.l2_rpc(),
                ANVIL_KEY_1,
                world.l2_chain_id,
                nonce + offset,
                Some(ANVIL_ADDR_3),
                U256::from(9),
                vec![],
                21_000,
            )
            .await
            .unwrap(),
        );
    }
    let mut before = Vec::new();
    for hash in &hashes {
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l2.get_transaction_receipt(*hash).await?)
        })
        .await
        .unwrap();
        assert!(receipt.status());
        before.push(receipt);
    }
    let original_origin = l2
        .call(TransactionRequest::default().to(HISTORY_ADDRESS).input(
            Bytes::from(IL1BlockhashContract::getLatestImportedL1BlockCall {}.abi_encode()).into(),
        ))
        .block(before[0].block_hash.unwrap().into())
        .await
        .unwrap();
    let original_origin =
        IL1BlockhashContract::getLatestImportedL1BlockCall::abi_decode_returns(&original_origin)
            .unwrap();
    assert!(
        original_origin.number > 1,
        "must fork an imported non-bootstrap source"
    );
    let old_source = l1
        .get_block_by_number(original_origin.number.into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old_source.header.hash, original_origin.hash);
    let ancestor = l1
        .get_block_by_number((original_origin.number - 1).into())
        .await
        .unwrap()
        .unwrap()
        .header;
    assert!(ancestor.number >= setup_tip);
    let replacement = world
        .l1_reorg_control
        .as_mut()
        .unwrap()
        .reorg_to(ancestor.number, ancestor.hash)
        .await
        .unwrap();
    assert_ne!(
        replacement["replacement"]["hash"],
        original_origin.hash.to_string()
    );
    world.node.wait_for_reorg_seen(TIMEOUT).await.unwrap();

    let mut repaired = Vec::new();
    for (hash, old) in hashes.iter().zip(&before) {
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l2.get_transaction_receipt(*hash).await?
                .filter(|receipt| receipt.block_hash != old.block_hash))
        }).await.unwrap_or_else(|error| panic!(
            "ordinary source {hash} was not recovered after imported-history reorg: {error:#}{}",
            world.settlement_diagnostics()));
        assert!(receipt.status());
        repaired.push(receipt);
    }
    assert_eq!(
        onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1).await.unwrap(),
        nonce + 20
    );
    assert_eq!(
        l2.get_balance(ANVIL_ADDR_3).await.unwrap(),
        balance + U256::from(180)
    );
    let after_chain = world.chain().snapshot().await.unwrap();
    assert_eq!(after_chain.batches_posted, 0);
    assert_eq!(after_chain.state_root, before_chain.state_root);

    let replaced = l1
        .get_block_by_number(original_origin.number.into())
        .await
        .unwrap()
        .unwrap()
        .header;
    assert_ne!(replaced.hash, original_origin.hash);
    let lookup = TransactionRequest::default().to(HISTORY_ADDRESS).input(
        Bytes::from(
            IL1BlockhashContract::getL1BlockHashCall {
                number: original_origin.number,
            }
            .abi_encode(),
        )
        .into(),
    );
    let history = l2.call(lookup.clone()).latest().await.unwrap();
    assert_eq!(
        IL1BlockhashContract::getL1BlockHashCall::abi_decode_returns(&history).unwrap(),
        replaced.hash
    );

    // A live outbound action makes the repaired ordinary prefix due. A fresh
    // follower has no access to the abandoned blocks or the producer's pool.
    let trigger = sign_and_send(
        &world.l2_xchain(),
        ANVIL_KEY_4,
        world.l2_chain_id,
        onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap(),
        Some(world.outbound_proxy),
        U256::ZERO,
        IValue::setValueCall { v: U256::from(73) }.abi_encode(),
        400_000,
    )
    .await
    .unwrap();
    let settled = wait_for(TIMEOUT, || async {
        let Some(receipt) = l2.get_transaction_receipt(trigger).await? else {
            return Ok(None);
        };
        let safe = l2.get_block_by_number(BlockNumberOrTag::Safe).await?;
        Ok(safe
            .filter(|safe| safe.header.number >= receipt.block_number.unwrap())
            .map(|_| receipt))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "repaired ordinary prefix was not settled: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    assert!(settled.status());
    let follower = NodeHandle::start(
        "history-reorg-ordinary-follower",
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
        settled.block_number.unwrap(),
        TIMEOUT,
    )
    .await
    .unwrap();
    let replay = ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap());
    for (hash, receipt) in hashes.iter().zip(&repaired) {
        let replayed = replay
            .get_transaction_receipt(*hash)
            .await
            .unwrap()
            .unwrap();
        assert!(replayed.status());
        assert_eq!(replayed.block_hash, receipt.block_hash);
    }
    assert_eq!(
        replay.get_balance(ANVIL_ADDR_3).await.unwrap(),
        balance + U256::from(180)
    );
    let replayed_history = replay.call(lookup).latest().await.unwrap();
    assert_eq!(
        IL1BlockhashContract::getL1BlockHashCall::abi_decode_returns(&replayed_history).unwrap(),
        replaced.hash
    );
    world.node.assert_no_divergence_failure_logs();
    follower.assert_no_divergence_failure_logs();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_l1_reorg_recovers_confirmed_and_unresolved_source_generations_in_nonce_order() {
    let env = [
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
    let composer = ProviderBuilder::new().connect_http(world.l2_xchain().parse().unwrap());
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
        let status: serde_json::Value = composer
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
    let owner: serde_json::Value = composer
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
            let owner: serde_json::Value = composer
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
            let status: serde_json::Value = composer
                .client()
                .request("eez_getCrossChainTransaction", (hash,))
                .await?;
            Ok((status["ownership"]["reason"] == "Committed").then_some(()))
        })
        .await
        .unwrap();
    }
}
