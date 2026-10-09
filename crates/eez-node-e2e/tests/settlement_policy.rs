//! Observable policy behavior using the existing native-blob node harness.

use std::time::Duration;

use alloy_consensus::Transaction as _;
use alloy_primitives::U256;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{SolCall as _, SolEvent as _};
use eez_testkit::{
    ANVIL_ADDR_3, ANVIL_KEY_1, Harness, NodeBinary, NodeConfig, NodeHandle, batches_posted,
    send_l2_value_transfer_confirmed, setup_cross_chain_with_env, wait_for_latest_height,
    wait_for_safe_prefix_convergence,
};

const TIMEOUT: Duration = Duration::from_mins(3);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conditional_position_paces_late_pool_transactions_and_replays() {
    // The native fixture has K=5 and one-second L2 blocks. P=2 leaves
    // ordinary positions 3/4 available to a transaction arriving after cutoff.
    let mut env = policy_env(Vec::new(), "interval");
    env.extend([
        ("EEZ_SETTLEMENT_START_AFTER_BLOCK", "2".into()),
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "25".into()),
    ]);
    let world = setup_cross_chain_with_env(&env).await.unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let target = eez_testkit::wait_for(TIMEOUT, || async {
        let head = l2.get_block_number().await?;
        Ok(world
            .node
            .signals_since(0)?
            .into_iter()
            .rev()
            .find_map(|signal| {
                if signal.name != "eez.composer.slot.plan" || signal.fields["prepare"] != false {
                    return None;
                }
                signal.fields["target_height"]
                    .as_u64()
                    .filter(|target| head + 3 == *target)
            }))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "no ordinary cutoff: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    let hash = send_l2_value_transfer_confirmed(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(9),
        TIMEOUT,
    )
    .await
    .unwrap();
    let receipt = l2.get_transaction_receipt(hash).await.unwrap().unwrap();
    assert!(receipt.status());
    let included = receipt.block_number.unwrap();
    assert!(
        included > target - 3 && included < target,
        "late transaction must land in the paced suffix, got {included} for target {target}{}",
        world.settlement_diagnostics()
    );
    wait_for_latest_height(&world.node, target, TIMEOUT)
        .await
        .unwrap();
    let boundary = l2
        .get_block_by_number(target.into())
        .await
        .unwrap()
        .unwrap();
    assert!(
        boundary.transactions.is_empty(),
        "skipped checkpoint stays empty"
    );
    assert!(
        world.node.signals_since(0).unwrap().iter().any(|signal| {
            signal.name == "eez.composer.sync_slot.invoked"
                && signal.fields["mode"] == "Ordinary"
                && signal.fields["timestamp"] == boundary.header.timestamp
        }),
        "the successful ordinary decision must reach the non-posting Composer path"
    );
    let follower = NodeHandle::start(
        "conditional-position-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], target, TIMEOUT)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "conditional history did not settle/replay: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
    let replay = ProviderBuilder::new()
        .connect_http(follower.l2_rpc_url().parse().unwrap())
        .get_transaction_receipt(hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.block_hash, replay.block_hash);
    assert_eq!(receipt.status(), replay.status());
    world.node.assert_no_process_death();
    follower.assert_no_process_death();
}

fn full_history_payload_len(blocks: &[Vec<u8>]) -> usize {
    use alloy_eips::Encodable2718;
    use eez_payload_codec::v0::{self, BlockRun};
    let blocks = blocks
        .iter()
        .map(|bytes| alloy_rlp::decode_exact::<eez_primitives::Block>(bytes).unwrap())
        .collect::<Vec<_>>();
    let transactions = blocks
        .iter()
        .map(|block| {
            block
                .body
                .transactions
                .iter()
                .map(|tx| tx.encoded_2718().into())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    v0::encode(
        &blocks
            .iter()
            .zip(&transactions)
            .map(|(block, transactions)| BlockRun {
                blocks: 1,
                beneficiary: block.header.beneficiary,
                extra_data: &block.header.extra_data,
                transactions,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap()
    .len()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retained_backlog_replays_old_witnesses_when_the_capture_store_is_empty() {
    let mut world = setup_cross_chain_with_env(&policy_env(Vec::new(), "interval"))
        .await
        .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let hash = send_l2_value_transfer_confirmed(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(9),
        TIMEOUT,
    )
    .await
    .unwrap();
    let receipt = l2.get_transaction_receipt(hash).await.unwrap().unwrap();
    wait_for_latest_height(&world.node, receipt.block_number.unwrap() + 12, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        0
    );
    // Switch only the optional capture cache; keep the real canonical Reth
    // chain/state. Capture subscribes to new commits, so old blocks must be
    // reexecuted through NodeWitnessSource's existing on-demand fallback.
    let empty_store = tempfile::tempdir().unwrap();
    assert!(
        std::fs::read_dir(empty_store.path())
            .unwrap()
            .next()
            .is_none()
    );
    world
        .restart_composer_with_env(&[
            (
                "EEZ_WITNESS_DB_PATH",
                empty_store.path().to_string_lossy().into_owned(),
            ),
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "always".into()),
        ])
        .await
        .unwrap();
    let follower = NodeHandle::start(
        "replayed-witness-follower",
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
        receipt.block_number.unwrap(),
        TIMEOUT,
    )
    .await
    .unwrap_or_else(|error| {
        panic!(
            "old witness replay failed: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    let replayed = ProviderBuilder::new()
        .connect_http(follower.l2_rpc_url().parse().unwrap())
        .get_transaction_receipt(hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replayed.block_hash, receipt.block_hash);
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        world
            .node
            .log_count_matching(&["eez.composer.emission.historical_chunk"])
            .unwrap(),
        0,
        "a missing cached witness requires replay, never a capacity split"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_sync_settles_due_history_within_the_original_target_window() {
    let mut env = policy_env(Vec::new(), "interval");
    env.extend([
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "5".into()),
        ("EEZ_PROOF_TIME_MS", "1500".into()),
        ("EEZ_SUBMISSION_SLACK_MS", "700".into()),
    ]);
    let world = setup_cross_chain_with_env(&env).await.unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let cursor = world.node.signal_cursor().unwrap();
    // The signer is ready before inducing lateness. Pause between the L1
    // watcher's scan and the +2s trigger, then resume after its +2.8s proof
    // allocation but before the real submission deadline at +4.3s. The
    // remaining window may still complete preparation, proof and submission.
    let base = eez_testkit::wait_for(TIMEOUT, || async {
        let block = l1
            .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
            .await?
            .unwrap();
        let age = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis()
            .saturating_sub(u128::from(block.header.timestamp) * 1000);
        Ok((1200..1700)
            .contains(&age)
            .then_some(block.header.timestamp))
    })
    .await
    .unwrap();
    let resume = std::time::UNIX_EPOCH + Duration::from_millis(base * 1000 + 3500);
    world
        .node
        .pause_for(resume.duration_since(std::time::SystemTime::now()).unwrap())
        .await
        .unwrap();
    let (old_timestamp, endpoint) = eez_testkit::wait_for(TIMEOUT, || async {
        let mut late_slot = None;
        for event in world.node.signals_since(cursor)? {
            if event.name == "eez.composer.sync_slot.invoked" {
                late_slot = (event.fields["mode"] == "Empty")
                    .then(|| event.fields["timestamp"].as_u64().unwrap());
            }
            if let Some(timestamp) = late_slot
                && event.name == "eez.composer.phase1.bundle.dispatched"
            {
                return Ok(Some((
                    timestamp,
                    event.fields["sync_height"].as_u64().unwrap(),
                )));
            }
        }
        Ok(None)
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "late Sync must reach submission despite Empty mode: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    assert!(endpoint < 300);
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let header = l2
        .get_block_by_number(endpoint.into())
        .await
        .unwrap()
        .unwrap()
        .header;
    assert_eq!(header.timestamp, old_timestamp);
    let follower = NodeHandle::start(
        "late-sync-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], endpoint, TIMEOUT)
        .await
        .unwrap();
    let logs = l1
        .get_logs(
            &alloy_rpc_types_eth::Filter::new()
                .address(world.dep.eez_address)
                .event_signature(eez_protocol::abi::L2ExecutionPerformed::SIGNATURE_HASH)
                .from_block(0),
        )
        .await
        .unwrap();
    let applied = logs
        .iter()
        .find(|log| {
            log.log_decode::<eez_protocol::abi::L2ExecutionPerformed>()
                .is_ok_and(|decoded| decoded.inner.data.newRoot == header.hash)
        })
        .expect("the late candidate's exact root must be applied on L1");
    let included = l1
        .get_block_by_number(applied.block_number.unwrap().into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        included.header.timestamp, old_timestamp,
        "crossing the nominal proof-start allocation must not change a usable target"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn underpriced_companion_keeps_its_nonce_until_the_user_authorizes_replacement() {
    assert_source_deferral_recovery(
        false,
        eez_testkit::NativeL1Profile::EthereumOsaka,
        "1",
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn underfunded_companion_keeps_its_signed_bytes_until_external_funding_arrives() {
    assert_source_deferral_recovery(
        true,
        eez_testkit::NativeL1Profile::EthereumOsaka,
        "1",
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_blocked_fee_sender_does_not_starve_independent_auto_work() {
    assert_source_deferral_recovery(
        false,
        eez_testkit::NativeL1Profile::GnosisPrague,
        "auto",
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_blocked_balance_sender_does_not_starve_independent_auto_work() {
    assert_source_deferral_recovery(
        true,
        eez_testkit::NativeL1Profile::GnosisPrague,
        "auto",
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_blocked_outbound_balance_allows_independent_limit_one_work() {
    assert_source_deferral_recovery(true, eez_testkit::NativeL1Profile::EthereumOsaka, "1", true)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_blocked_outbound_balance_allows_independent_auto_work() {
    assert_source_deferral_recovery(
        true,
        eez_testkit::NativeL1Profile::GnosisPrague,
        "auto",
        true,
    )
    .await;
}

async fn assert_source_deferral_recovery(
    funding: bool,
    profile: eez_testkit::NativeL1Profile,
    limit: &str,
    outbound: bool,
) {
    use alloy_consensus::{SignableTransaction as _, TxEip1559};
    use alloy_eips::Encodable2718 as _;
    use alloy_network::TxSignerSync as _;
    use alloy_primitives::{B256, TxKind};
    use alloy_signer_local::PrivateKeySigner;
    use eez_testkit::{IValue, onchain_nonce, sign_and_send, wait_for};
    let mut env = policy_env(Vec::new(), "interval");
    env.push(("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "1000000001".into()));
    env.push(("EEZ_MAX_USER_TXS_PER_BUNDLE", limit.into()));
    let world = eez_testkit::setup_cross_chain_with_profile(profile, &env)
        .await
        .unwrap();
    let source_rpc = if outbound {
        world.l2_rpc()
    } else {
        world.l1_rpc()
    };
    let source_front = if outbound {
        world.l2_xchain()
    } else {
        world.l1_xchain()
    };
    let source = ProviderBuilder::new().connect_http(source_rpc.parse().unwrap());
    let queue_rpc = ProviderBuilder::new().connect_http(world.l2_xchain().parse().unwrap());
    let front = ProviderBuilder::new().connect_http(source_front.parse().unwrap());
    let source_chain_id = source.get_chain_id().await.unwrap();
    let target = if outbound {
        world.outbound_proxy
    } else {
        world.setter_proxy
    };
    let signer = PrivateKeySigner::from_bytes(&B256::with_last_byte(100)).unwrap();
    // Use the real source-chain transfer, not a state override. Both users
    // individually pass ingress, but cumulative execution can exhaust funding.
    let value = U256::from(if funding {
        1_210_000_000_000_000u64
    } else {
        100_000_000_000_000_000u64
    });
    let nonce = onchain_nonce(&source_rpc, ANVIL_KEY_1).await.unwrap();
    let funded = sign_and_send(
        &source_rpc,
        ANVIL_KEY_1,
        source_chain_id,
        nonce,
        Some(signer.address()),
        value,
        Vec::new(),
        21_000,
    )
    .await
    .unwrap();
    wait_for(TIMEOUT, || async {
        eez_testkit::receipt_ok(&source_rpc, funded).await
    })
    .await
    .unwrap();
    let signed = |nonce, price: u128| {
        let mut tx = TxEip1559 {
            chain_id: source_chain_id,
            nonce,
            gas_limit: 600_000,
            max_fee_per_gas: price,
            max_priority_fee_per_gas: price.min(1_000_000_000),
            to: TxKind::Call(target),
            input: IValue::setValueCall {
                v: U256::from(41 + nonce),
            }
            .abi_encode()
            .into(),
            ..Default::default()
        };
        let signature = signer.sign_transaction_sync(&mut tx).unwrap();
        alloy_consensus::TxEnvelope::from(tx.into_signed(signature))
    };
    let mut authorized = Vec::new();
    let cursor = world.node.signal_cursor().unwrap();
    wait_for(TIMEOUT, || async {
        Ok(world
            .node
            .signals_since(cursor)?
            .iter()
            .any(|event| {
                event.name == "eez.composer.settlement.decision" && event.fields["due"] == false
            })
            .then_some(()))
    })
    .await
    .unwrap();
    let first = signed(0, if funding { 2_000_000_000 } else { 0 });
    assert_eq!(
        front
            .send_raw_transaction(&first.encoded_2718())
            .await
            .unwrap()
            .tx_hash(),
        first.tx_hash()
    );
    if funding {
        authorized.push(first.clone());
        let second = signed(1, 2_000_000_000);
        assert_eq!(
            front
                .send_raw_transaction(&second.encoded_2718())
                .await
                .unwrap()
                .tx_hash(),
            second.tx_hash()
        );
        authorized.push(second);
    }
    wait_for(TIMEOUT, || async {
        let queue: serde_json::Value = queue_rpc
            .client()
            .request("eez_getCrossChainQueueStatus", ())
            .await?;
        let expected_nonce = u64::from(funding);
        // Outbound nonce advancement and the blocked suffix can become visible
        // with the optimistic Sync block, before its L1 target is mined.
        // Observe the funding split's canonical post before checking its count.
        Ok((queue["blocked"].as_u64().is_some_and(|n| n > 0)
            && source.get_transaction_count(signer.address()).await? == expected_nonce
            && batches_posted(&world.l1_rpc(), world.dep.eez_address, 0).await?
                == usize::from(funding))
        .then_some(()))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "temporary source failure not retained: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        usize::from(funding)
    );
    assert_eq!(
        source
            .get_transaction_count(signer.address())
            .await
            .unwrap(),
        u64::from(funding)
    );
    assert_eq!(
        world
            .node
            .log_count_matching(&["eez.composer.emission.historical_chunk"])
            .unwrap(),
        0
    );
    // An independent source must settle while the original sender is still
    // blocked, including when the configured selection limit is exactly one.
    // Fee cases cross the phase boundary: fee eligibility must be resolved
    // before the finite window so outbound B can use this opportunity.
    let independent_outbound = if funding { outbound } else { !outbound };
    let (independent_rpc, independent_front, independent_target) = if independent_outbound {
        (world.l2_rpc(), world.l2_xchain(), world.outbound_proxy)
    } else {
        (world.l1_rpc(), world.l1_xchain(), world.setter_proxy)
    };
    let independent_chain_id = ProviderBuilder::new()
        .connect_http(independent_rpc.parse().unwrap())
        .get_chain_id()
        .await
        .unwrap();
    let independent_nonce = onchain_nonce(&independent_rpc, eez_testkit::ANVIL_KEY_2)
        .await
        .unwrap();
    let independent = sign_and_send(
        &independent_front,
        eez_testkit::ANVIL_KEY_2,
        independent_chain_id,
        independent_nonce,
        Some(independent_target),
        U256::ZERO,
        IValue::setValueCall { v: U256::from(99) }.abi_encode(),
        400_000,
    )
    .await
    .unwrap();
    assert!(
        wait_for(TIMEOUT, || async {
            eez_testkit::receipt_ok(&independent_rpc, independent).await
        })
        .await
        .unwrap_or_else(|e| panic!(
            "independent sender stalled: {e:#}{}",
            world.settlement_diagnostics()
        ))
    );
    assert_eq!(
        source
            .get_transaction_count(signer.address())
            .await
            .unwrap(),
        u64::from(funding)
    );
    let blocked_hash = if funding {
        *authorized[1].tx_hash()
    } else {
        *first.tx_hash()
    };
    assert!(
        source
            .get_transaction_receipt(blocked_hash)
            .await
            .unwrap()
            .is_none()
    );

    if funding {
        let nonce = onchain_nonce(&source_rpc, ANVIL_KEY_1).await.unwrap();
        let funded = sign_and_send(
            &source_rpc,
            ANVIL_KEY_1,
            source_chain_id,
            nonce,
            Some(signer.address()),
            U256::from(100_000_000_000_000_000u64),
            Vec::new(),
            21_000,
        )
        .await
        .unwrap();
        assert!(
            wait_for(TIMEOUT, || async {
                eez_testkit::receipt_ok(&source_rpc, funded).await
            })
            .await
            .unwrap()
        );
    } else {
        let replacement = signed(0, 100_000_000_000);
        assert_eq!(
            front
                .send_raw_transaction(&replacement.encoded_2718())
                .await
                .unwrap()
                .tx_hash(),
            replacement.tx_hash()
        );
        authorized.push(replacement);
        assert!(
            source
                .get_transaction_receipt(*first.tx_hash())
                .await
                .unwrap()
                .is_none()
        );
    }
    for tx in &authorized {
        assert!(
            wait_for(TIMEOUT, || async {
                eez_testkit::receipt_ok(&source_rpc, *tx.tx_hash()).await
            })
            .await
            .unwrap_or_else(|error| panic!(
                "authorized source not recovered: {error:#}{}",
                world.settlement_diagnostics()
            ))
        );
        let included = source
            .get_transaction_by_hash(*tx.tx_hash())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(included.inner.encoded_2718(), tx.encoded_2718());
    }
    let dispatched: Vec<_> = world
        .node
        .signals_since(0)
        .unwrap()
        .into_iter()
        .filter(|event| event.name == "eez.composer.bundle.dispatched")
        .collect();
    for event in &dispatched {
        let attempts: Vec<u32> =
            serde_json::from_str(event.fields["attempt_counts"].as_str().unwrap()).unwrap();
        assert!(
            attempts.iter().all(|n| *n == 0),
            "source deferral must not charge an attempt"
        );
    }
    let covered = dispatched
        .iter()
        .filter_map(|event| event.fields["sync_height"].as_u64())
        .max()
        .unwrap();
    let follower = NodeHandle::start(
        "source-fee-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], covered, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        if funding { 3 } else { 2 }
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fee_rpc_outage_then_dropped_bundle_preserves_due_activity_until_canonical_replay() {
    let mut world = setup_cross_chain_with_env(&policy_env(Vec::new(), "interval"))
        .await
        .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let hash = send_l2_value_transfer_confirmed(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(7),
        TIMEOUT,
    )
    .await
    .unwrap();
    let receipt = l2.get_transaction_receipt(hash).await.unwrap().unwrap();
    wait_for_latest_height(&world.node, receipt.block_number.unwrap() + 12, TIMEOUT)
        .await
        .unwrap();
    world.fail_rpc_methods(&["eth_feeHistory"]).unwrap();
    let rpc = world.fault_rpc_url().to_owned();
    world
        .restart_composer_with_env(&[
            ("EEZ_L1_RPC_URL", rpc),
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "always".into()),
        ])
        .await
        .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    eez_testkit::wait_for(TIMEOUT, || async {
        Ok((world.node.log_count_matching(&["priority fee history"])? >= 2).then_some(()))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "fee outage was not reported: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        world
            .node
            .log_count_matching(&["eez.composer.emission.historical_chunk"])
            .unwrap(),
        0
    );
    assert_eq!(
        l2.get_transaction_receipt(hash)
            .await
            .unwrap()
            .unwrap()
            .block_hash,
        receipt.block_hash
    );

    world.drop_next_bundle().unwrap();
    world.fail_rpc_methods(&[]).unwrap();
    let dropped = eez_testkit::wait_for(TIMEOUT, || async { world.dropped_bundle() })
        .await
        .unwrap();
    let dropped_raw: alloy_primitives::Bytes = dropped["params"][0]["txs"][0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let dropped_hash = eez_blob_codec::pooled_blob_transaction_hash(&dropped_raw).unwrap();
    let follower = NodeHandle::start(
        "fee-rpc-retry-follower",
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
        receipt.block_number.unwrap(),
        TIMEOUT,
    )
    .await
    .unwrap_or_else(|error| {
        panic!(
            "dropped due batch did not recover: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    assert!(
        l1.get_transaction_receipt(dropped_hash)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        1
    );
    let replay = ProviderBuilder::new()
        .connect_http(follower.l2_rpc_url().parse().unwrap())
        .get_transaction_receipt(hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.block_hash, receipt.block_hash);
    let height = l2.get_block_number().await.unwrap();
    wait_for_latest_height(&world.node, height + 12, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        1,
        "confirmation clears the finite obligation even after several empty retry slots"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_data_admission_tracks_operator_capacity_without_losing_valid_work() {
    use eez_testkit::{onchain_nonce, sign_and_send, wait_for};
    let mut env = policy_env(Vec::new(), "always");
    env.push(("EEZ_MAX_BLOBS_PER_BATCH", "1".into()));
    let mut world = setup_cross_chain_with_env(&env).await.unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1).await.unwrap();
    let first = sign_and_send(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        world.l2_chain_id,
        nonce,
        Some(ANVIL_ADDR_3),
        U256::from(1),
        vec![],
        21_000,
    )
    .await
    .unwrap();
    // The old behavior admitted this indivisible block and stranded it. Reject
    // it before sequencing; the valid work already queued must still complete.
    let rejected = sign_and_send(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        world.l2_chain_id,
        nonce + 1,
        Some(ANVIL_ADDR_3),
        U256::ZERO,
        vec![0; 128_000],
        2_000_000,
    )
    .await
    .unwrap_err();
    assert!(
        format!("{rejected:#}").contains("cannot fit a settlement interval"),
        "{rejected:#}"
    );
    let second = sign_and_send(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        world.l2_chain_id,
        nonce + 1,
        Some(ANVIL_ADDR_3),
        U256::from(2),
        vec![],
        21_000,
    )
    .await
    .unwrap();
    let mut receipts = Vec::new();
    for hash in [first, second] {
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l2.get_transaction_receipt(hash).await?)
        })
        .await
        .unwrap();
        assert!(receipt.status());
        receipts.push((hash, receipt));
    }
    wait_for_safe_prefix_convergence(
        &[&world.node],
        receipts.last().unwrap().1.block_number.unwrap(),
        TIMEOUT,
    )
    .await
    .unwrap();
    // Increasing an operator cap within the network allowance lets fresh data
    // use the additional space. This is not recovery of an oversized block.
    world
        .restart_composer_with_env(&[("EEZ_MAX_BLOBS_PER_BATCH", "2".into())])
        .await
        .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let hash = sign_and_send(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        world.l2_chain_id,
        nonce + 2,
        Some(ANVIL_ADDR_3),
        U256::ZERO,
        vec![0; 128_000],
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
    let covered = receipt.block_number.unwrap();
    receipts.push((hash, receipt));
    let follower = NodeHandle::start(
        "policy-admission-cap-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], covered, TIMEOUT)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "valid data did not settle: {e:#}{}",
                world.settlement_diagnostics()
            )
        });
    let replay = ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap());
    for (hash, receipt) in receipts {
        assert_eq!(
            replay
                .get_transaction_receipt(hash)
                .await
                .unwrap()
                .unwrap()
                .block_hash,
            receipt.block_hash
        );
    }
    assert_eq!(
        onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1).await.unwrap(),
        nonce + 3
    );
    assert_eq!(
        world
            .node
            .log_count_matching(&["unpostable_range"])
            .unwrap(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fee_ceiling_blocks_posting_without_losing_activity_or_splitting() {
    let mut env = policy_env(Vec::new(), "always");
    env.push(("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "1".into()));
    // Keep startup configuration valid while making base fee plus any
    // selected priority exceed the execution ceiling at runtime.
    env.push(("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "1".into()));
    let mut world = setup_cross_chain_with_env(&env).await.unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let hash = send_l2_value_transfer_confirmed(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(1),
        TIMEOUT,
    )
    .await
    .unwrap();
    let receipt = l2.get_transaction_receipt(hash).await.unwrap().unwrap();
    wait_for_latest_height(&world.node, receipt.block_number.unwrap() + 12, TIMEOUT)
        .await
        .unwrap();
    assert!(world.node.log_count_matching(&["fee_cap_blocked"]).unwrap() > 0);
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        0
    );
    assert!(
        !world
            .node
            .signals_since(0)
            .unwrap()
            .iter()
            .any(|event| { event.name == "eez.composer.emission.historical_chunk" })
    );
    world
        .restart_composer_with_env(&[("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "100000000000".into())])
        .await
        .unwrap();
    let follower = NodeHandle::start(
        "policy-fee-recovery-follower",
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
        receipt.block_number.unwrap(),
        TIMEOUT,
    )
    .await
    .unwrap_or_else(|error| {
        panic!(
            "fee recovery failed: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    let replay = ProviderBuilder::new()
        .connect_http(follower.l2_rpc_url().parse().unwrap())
        .get_transaction_receipt(hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.block_hash, receipt.block_hash);
    assert!(replay.status());
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lowering_blob_cap_rejects_retained_unsplittable_history_without_losing_it() {
    let mut env = policy_env(Vec::new(), "interval");
    env.push(("EEZ_MAX_BLOBS_PER_BATCH", "2".into()));
    let mut world = setup_cross_chain_with_env(&env).await.unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    // Below the existing transaction-size limit, but the single ordinary block
    // exceeds one usable blob. Fullness remains a soft posting trigger.
    let hash = eez_testkit::sign_and_send(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        world.l2_chain_id,
        eez_testkit::onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1)
            .await
            .unwrap(),
        Some(ANVIL_ADDR_3),
        U256::ZERO,
        vec![0; 128_000],
        2_000_000,
    )
    .await
    .unwrap();
    let receipt = eez_testkit::wait_for(TIMEOUT, || async {
        Ok(l2.get_transaction_receipt(hash).await?)
    })
    .await
    .unwrap();
    assert!(receipt.status());
    wait_for_latest_height(&world.node, receipt.block_number.unwrap() + 12, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        0
    );
    // The RPC may briefly open before startup validation terminates the node.
    let restart = world
        .restart_composer_with_env(&[("EEZ_MAX_BLOBS_PER_BATCH", "1".into())])
        .await;
    match restart {
        Ok(()) => {
            let status = eez_testkit::wait_for(TIMEOUT, || async { Ok(world.node.exit_status()) })
                .await
                .unwrap();
            assert!(!status.success());
            assert!(
                world
                    .node
                    .log_count_matching(&["retained unsplittable history"])
                    .unwrap()
                    > 0
            );
        }
        Err(error) => assert!(
            format!("{error:#}").contains("retained unsplittable history"),
            "unexpected startup error: {error:#}"
        ),
    }
    // The startup failure is actionable and does not revise accepted L2 bytes.
    // Retrying the supported cap must recover and settle that exact block.
    world
        .restart_composer_with_env(&[
            ("EEZ_MAX_BLOBS_PER_BATCH", "2".into()),
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "always".into()),
        ])
        .await
        .unwrap();
    let follower = NodeHandle::start(
        "policy-cap-recovery-follower",
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
        receipt.block_number.unwrap(),
        TIMEOUT,
    )
    .await
    .unwrap_or_else(|error| {
        panic!(
            "cap recovery failed: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    let replay = ProviderBuilder::new()
        .connect_http(follower.l2_rpc_url().parse().unwrap())
        .get_transaction_receipt(hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.block_hash, receipt.block_hash);
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prover_resource_and_transport_failures_preserve_the_whole_due_range() {
    use eez_testkit::{ProverMutation, onchain_nonce, sign_and_send};
    let world =
        eez_testkit::setup_cross_chain_with_prover_faults(&policy_env(Vec::new(), "always"))
            .await
            .unwrap();
    let follower = NodeHandle::start(
        "policy-proof-retry-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    // A snapshot of `safe` can precede an already-proved setup batch still
    // awaiting inclusion. Settle a marker after all setup transactions before
    // taking the baseline or blocking the prover. `always` posts this activity
    // but deliberately leaves later empty blocks unsettled.
    let setup_tx = send_l2_value_transfer_confirmed(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(1),
        TIMEOUT,
    )
    .await
    .unwrap();
    let height = l2
        .get_transaction_receipt(setup_tx)
        .await
        .unwrap()
        .unwrap()
        .block_number
        .unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], height, TIMEOUT)
        .await
        .unwrap();
    let baseline = batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
        .await
        .unwrap();
    let proxy = world.prover_proxy.as_ref().unwrap();
    proxy.set_mutation(ProverMutation::AllResourceExhausted);
    let attempts = proxy.attempts();
    let successes = proxy.successes();
    let cursor = world.node.signal_cursor().unwrap();
    // Included reverted ordinary transactions are activity too. Calling the
    // Value contract with an unknown selector reverts on L2 without effects.
    let reverted = sign_and_send(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        world.l2_chain_id,
        onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1).await.unwrap(),
        Some(world.value_l2),
        U256::ZERO,
        vec![0xff; 4],
        100_000,
    )
    .await
    .unwrap();
    let receipt = eez_testkit::wait_for(TIMEOUT, || async {
        Ok(l2.get_transaction_receipt(reverted).await?)
    })
    .await
    .unwrap();
    assert!(!receipt.status());
    let included_height = receipt.block_number.unwrap();
    eez_testkit::wait_for(TIMEOUT, || async {
        Ok((proxy.attempts() >= attempts + 2).then_some(()))
    })
    .await
    .unwrap();
    assert_eq!(proxy.successes(), successes);
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        baseline
    );
    assert!(l2.get_block_number().await.unwrap() > included_height);
    proxy.set_mutation(ProverMutation::Unavailable);
    let attempts = proxy.attempts();
    eez_testkit::wait_for(TIMEOUT, || async {
        Ok((proxy.attempts() >= attempts + 2).then_some(()))
    })
    .await
    .unwrap();
    assert_eq!(proxy.successes(), successes);
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        baseline
    );
    assert!(
        !world
            .node
            .signals_since(cursor)
            .unwrap()
            .iter()
            .any(|event| { event.name == "eez.composer.emission.historical_chunk" }),
        "proof/resource failures never authorize capacity splitting"
    );
    proxy.set_mutation(ProverMutation::None);
    wait_for_safe_prefix_convergence(&[&world.node, &follower], included_height, TIMEOUT)
        .await
        .unwrap();
    let reproduced = ProviderBuilder::new()
        .connect_http(follower.l2_rpc_url().parse().unwrap())
        .get_transaction_receipt(reverted)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reproduced.block_hash, receipt.block_hash);
    assert!(!reproduced.status());
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        baseline + 1
    );
    let height = l2.get_block_number().await.unwrap();
    wait_for_latest_height(&world.node, height + 12, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        baseline + 1
    );
    world.node.assert_no_divergence_failure_logs();
    follower.assert_no_divergence_failure_logs();
}

/// The general threshold is a trigger, not a batch-size cap. This also
/// exercises old persistent witnesses through the unchanged ECDSA path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn more_than_three_hundred_blocks_settle_in_one_native_batch() {
    let mut env = policy_env(Vec::new(), "interval");
    env.push(("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "315".into()));
    let world = setup_cross_chain_with_env(&env).await.unwrap();
    let follower = NodeHandle::start(
        "policy-long-range-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], 315, Duration::from_secs(450))
        .await
        .unwrap_or_else(|error| {
            panic!(
                "large range not settled: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let logs = l1
        .get_logs(
            &alloy_rpc_types_eth::Filter::new()
                .address(world.dep.eez_address)
                .event_signature(eez_protocol::abi::BatchPosted::SIGNATURE_HASH)
                .from_block(0),
        )
        .await
        .unwrap();
    assert_eq!(
        logs.len(),
        1,
        "a fitting range must not split at 300 blocks"
    );
    let post = l1
        .get_transaction_by_hash(logs[0].transaction_hash.unwrap())
        .await
        .unwrap()
        .unwrap();
    let call = eez_protocol::abi::postAndVerifyBatchCall::abi_decode(post.inner.input()).unwrap();
    let update = &call.batch.entries[0].rootUpdates[0];
    assert_eq!(update.currentRoot, world.cfg.initial_state);
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let terminal = l2.get_block_by_hash(update.newRoot).await.unwrap().unwrap();
    assert!(terminal.header.number >= 315);
    assert_eq!(
        post.inner.blob_versioned_hashes().unwrap().len(),
        1,
        "derivable ordinary blocks must not force a second physical blob"
    );
    assert!(
        !world
            .node
            .signals_since(0)
            .unwrap()
            .iter()
            .any(|event| { event.name == "eez.composer.emission.historical_chunk" }),
        "the old size limit must not schedule a historical prefix"
    );
    world.node.assert_no_divergence_failure_logs();
    follower.assert_no_divergence_failure_logs();
}

/// A backlog above the batch block cap settles through consecutive historical
/// prefixes. Each landed prefix advances the canonical anchor, and the next
/// prefix must size its range from that new anchor rather than stopping after
/// the first one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn backlog_above_the_block_cap_settles_through_consecutive_historical_prefixes() {
    use eez_testkit::{IValue, onchain_nonce, sign_and_send, wait_for};
    const CAP: u64 = 10;
    let env = policy_env(
        vec![(eez_testkit::SIGNER_MAX_REQUEST_BLOCKS_ENV, CAP.to_string())],
        "interval",
    );
    let world = setup_cross_chain_with_env(&env).await.unwrap();
    let follower = NodeHandle::start(
        "policy-block-cap-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    // The window limit keeps a healthy chain below the cap. Exclude
    // consecutive bundles so the backlog exceeds twice the cap: after the
    // first historical prefix lands, the next one starts above a newer anchor.
    let cursor = world.node.signal_cursor().unwrap();
    world.drop_next_bundles(6).unwrap();
    wait_for(TIMEOUT, || async {
        Ok(world
            .node
            .signals_since(cursor)?
            .iter()
            .any(|event| {
                event.name == "eez.composer.emission.historical_chunk"
                    && event.fields["backlog"].as_u64() > Some(2 * CAP)
            })
            .then_some(()))
    })
    .await
    .unwrap();
    // Slots without held cross-chain work evaluate policy first. A held user
    // request selects the block-cap path directly, as on a busy network, and
    // can only be included once that path drains the backlog below the cap.
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let request = sign_and_send(
        &world.l1_xchain(),
        ANVIL_KEY_1,
        l1.get_chain_id().await.unwrap(),
        onchain_nonce(&world.l1_rpc(), ANVIL_KEY_1).await.unwrap(),
        Some(world.setter_proxy),
        U256::ZERO,
        IValue::setValueCall { v: U256::from(77) }.abi_encode(),
        600_000,
    )
    .await
    .unwrap();
    let succeeded = wait_for(TIMEOUT, || async {
        eez_testkit::receipt_ok(&world.l1_rpc(), request).await
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "held cross-chain request was never settled: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    assert!(succeeded, "held cross-chain request reverted");
    wait_for_safe_prefix_convergence(&[&world.node, &follower], 8 * CAP, TIMEOUT)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "historical prefixes stopped advancing: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
    let signals = world.node.signals_since(0).unwrap();
    let prefix_cursors = signals
        .iter()
        .filter(|event| {
            event.name == "eez.composer.emission.historical_chunk"
                && event.fields["split_reasons"]
                    .as_str()
                    .is_some_and(|reasons| reasons.contains("block_limit"))
        })
        .filter_map(|event| event.fields["cursor"].as_u64())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        prefix_cursors.len() >= 2,
        "expected block-cap prefixes above at least two anchors, saw {prefix_cursors:?}"
    );
    assert!(
        !signals
            .iter()
            .any(|event| event.name == "eez.composer.emission.historical_failed"),
        "a historical prefix failed after canonical advancement{}",
        world.settlement_diagnostics()
    );
    world.node.assert_no_divergence_failure_logs();
    follower.assert_no_divergence_failure_logs();
}

async fn registration(
    world: &eez_testkit::CrossChainWorld,
) -> eez_l1::l1_reader::RegistrationAnchor {
    eez_l1::L1Reader::new(eez_l1::L1ReaderConfig {
        rpc_url: world.l1_rpc().parse().unwrap(),
        beacon_url: None,
        eez: world.dep.eez_address,
        rollup_id: world.dep.rollup_id,
        deploy_block: 0,
    })
    .registration_anchor(world.cfg.initial_state)
    .await
    .unwrap()
}

async fn latest_post(world: &eez_testkit::CrossChainWorld) -> alloy_rpc_types_eth::Log {
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    eez_testkit::wait_for(TIMEOUT, || async {
        Ok(l1
            .get_logs(
                &alloy_rpc_types_eth::Filter::new()
                    .address(world.dep.eez_address)
                    .event_signature(eez_protocol::abi::BatchPosted::SIGNATURE_HASH)
                    .from_block(0),
            )
            .await?
            .pop())
    })
    .await
    .unwrap_or_else(|error| panic!("post missing: {error:#}{}", world.settlement_diagnostics()))
}

fn policy_env(mut env: Vec<(&'static str, String)>, mode: &str) -> Vec<(&'static str, String)> {
    env.retain(|(name, _)| {
        !matches!(
            *name,
            "EEZ_MAX_BLOCKS_PER_BATCH" | "EEZ_MAX_SPECULATIVE_DEPTH" | "RUST_LOG"
        )
    });
    env.extend([
        ("EEZ_SETTLEMENT_PURE_L2_MODE", mode.into()),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10000".into()),
        ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
        ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "100000000000".into()),
        (
            "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS",
            "100000000000".into(),
        ),
        ("RUST_LOG", "info".into()),
    ]);
    env
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn always_skips_idle_syncs_then_settles_included_activity_and_replays() {
    assert_always_profile(eez_testkit::NativeL1Profile::EthereumOsaka).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn native_gnosis_always_settles_with_native_blob_fees_and_independent_replay() {
    assert_always_profile(eez_testkit::NativeL1Profile::GnosisPrague).await;
}

async fn assert_always_profile(profile: eez_testkit::NativeL1Profile) {
    let world =
        eez_testkit::setup_cross_chain_with_profile(profile, &policy_env(Vec::new(), "always"))
            .await
            .unwrap();
    let node = &world.node;
    let follower = NodeHandle::start(
        "policy-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let provider = ProviderBuilder::new().connect_http(node.l2_rpc_url().parse().unwrap());
    // The full-state fixture deploys L2 contracts during setup. Wait for that
    // activity to settle before measuring an actually idle range.
    let setup_tx = send_l2_value_transfer_confirmed(
        &node.l2_rpc_url(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(1),
        TIMEOUT,
    )
    .await
    .unwrap();
    let setup_height = provider
        .get_transaction_receipt(setup_tx)
        .await
        .unwrap()
        .unwrap()
        .block_number
        .unwrap();
    wait_for_safe_prefix_convergence(&[node, &follower], setup_height, TIMEOUT)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "setup activity did not settle: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
    let initial_batches = batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
        .await
        .unwrap();
    let idle_start = provider.get_block_number().await.unwrap();
    wait_for_latest_height(node, idle_start + 12, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        initial_batches,
        "empty scheduled Syncs must not post"
    );

    let hash = send_l2_value_transfer_confirmed(
        &node.l2_rpc_url(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(1),
        TIMEOUT,
    )
    .await
    .expect("ordinary L2 transaction must still execute while settlement is idle");
    let included = provider
        .get_transaction_receipt(hash)
        .await
        .unwrap()
        .unwrap();
    wait_for_safe_prefix_convergence(&[node, &follower], included.block_number.unwrap(), TIMEOUT)
        .await
        .unwrap();
    assert!(included.status());
    let settled = provider
        .get_block_by_number(alloy_rpc_types_eth::BlockNumberOrTag::Safe)
        .await
        .unwrap()
        .unwrap();
    assert!(settled.header.number >= included.block_number.unwrap());
    let batches = batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
        .await
        .unwrap();
    let height = provider.get_block_number().await.unwrap();
    wait_for_latest_height(node, height + 12, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        batches,
        "covered activity must not become perpetual empty posting"
    );
    let signals = node.signals_since(0).unwrap();
    assert!(
        signals
            .iter()
            .any(|signal| signal.name == "eez.composer.postbatch.preflight"),
        "actual full-bundle preflight must run"
    );
    assert!(
        signals
            .iter()
            .any(|signal| signal.name == "eez.composer.postbatch.fee_quote"
                && signal.fields["priority_source"] == "fee_history"),
        "no implicit smoke-test tip"
    );
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let log = l1
        .get_logs(
            &alloy_rpc_types_eth::Filter::new()
                .address(world.dep.eez_address)
                .event_signature(eez_protocol::abi::BatchPosted::SIGNATURE_HASH)
                .from_block(0),
        )
        .await
        .unwrap()
        .pop()
        .unwrap();
    let post_hash = log.transaction_hash.unwrap();
    let receipt = l1
        .get_transaction_receipt(post_hash)
        .await
        .unwrap()
        .unwrap();
    let posted = l1
        .get_transaction_by_hash(post_hash)
        .await
        .unwrap()
        .unwrap();
    assert!(receipt.status());
    assert!(
        posted.inner.gas_limit() < 16_777_216,
        "declare candidate gas, not the transaction maximum"
    );
    assert!(receipt.gas_used <= posted.inner.gas_limit());
    assert!(receipt.effective_gas_price > 0);
    let blobs = posted.inner.blob_versioned_hashes().unwrap().len() as u64;
    assert_eq!(
        receipt.blob_gas_used,
        Some(blobs * alloy_eips::eip4844::DATA_GAS_PER_BLOB)
    );
    assert!(receipt.blob_gas_price.unwrap() > 0);
    let measured = U256::from(receipt.gas_used) * U256::from(receipt.effective_gas_price)
        + U256::from(receipt.blob_gas_used.unwrap()) * U256::from(receipt.blob_gas_price.unwrap());
    let cost = eez_testkit::wait_for(TIMEOUT, || async {
        Ok(node.signals_since(0)?.into_iter().find(|signal| {
            signal.name == "eez.composer.postbatch.actual_cost"
                && signal.fields["post_batch_hash"] == post_hash.to_string()
                && signal.fields["action"] == "apply"
        }))
    })
    .await
    .unwrap();
    assert_eq!(cost.fields["post_total_cost"], measured.to_string());
    assert_eq!(cost.fields["companion_total_cost"], "0");
    assert_eq!(cost.fields["bundle_total_cost"], measured.to_string());
    assert_eq!(
        cost.fields["native_currency"],
        if profile == eez_testkit::NativeL1Profile::GnosisPrague {
            "xDAI"
        } else {
            "ETH"
        }
    );
    if profile == eez_testkit::NativeL1Profile::GnosisPrague {
        assert!(
            receipt.blob_gas_price.unwrap() >= 1_000_000_000,
            "native Gnosis blob floor"
        );
    }
    assert!(cost.fields["settled_l2_blocks"].as_u64().unwrap() > 0);
    assert!(cost.fields["included_l2_transactions"].as_u64().unwrap() > 0);
    node.assert_no_process_death();
    follower.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interval_preserves_execution_but_waits_below_all_thresholds() {
    let harness = Harness::fresh().await.unwrap();
    let env = policy_env(harness.env().await.unwrap(), "interval");
    let node = NodeHandle::start("policy-interval", &NodeConfig::default(), &env)
        .await
        .unwrap();
    // L2 RPC binds before the Composer installs its ordinary-transaction
    // sequencing limits. Wait for the later startup event before sending.
    eez_testkit::wait_for(TIMEOUT, || async {
        Ok((node.count_signal(eez_testkit::signals::NODE_COMPOSER_SPAWNED)? > 0).then_some(()))
    })
    .await
    .unwrap();
    send_l2_value_transfer_confirmed(
        &node.l2_rpc_url(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(1),
        TIMEOUT,
    )
    .await
    .unwrap();
    let provider = ProviderBuilder::new().connect_http(node.l2_rpc_url().parse().unwrap());
    let height = provider.get_block_number().await.unwrap();
    wait_for_latest_height(&node, height + 12, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        harness.chain().batches_posted().await.unwrap(),
        0,
        "included pure activity waits for an enabled trigger"
    );
    let signals = node.signals_since(0).unwrap();
    assert!(
        signals
            .iter()
            .any(|signal| signal.name == "eez.composer.settlement.decision"
                && signal.fields["due"] == false),
        "test must observe actual policy evaluation, not a broken posting path"
    );
    assert!(
        !signals
            .iter()
            .any(|signal| signal.name == "eez.composer.postbatch.fee_quote"),
        "ordinary not-due ticks must not request posting fees"
    );
    for event in [
        "eez.composer.settlement.encoding",
        "eez.composer.postbatch.anchors",
        "eez.composer.postbatch.preflight",
        "eez.composer.phase1.bundle.dispatched",
        "eez.composer.bundle.dispatched",
    ] {
        assert!(
            !signals.iter().any(|signal| signal.name == event),
            "not-due history must not enter expensive posting preparation: {event}"
        );
    }
    node.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interval_posts_accumulated_activity_using_the_registration_l1_timestamp() {
    let mut env = policy_env(Vec::new(), "interval");
    env.push(("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "90000".into()));
    let world = setup_cross_chain_with_env(&env).await.unwrap();
    let anchor = registration(&world).await;
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let post = latest_post(&world).await;
    let block = l1
        .get_block_by_hash(post.block_hash.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(block.header.timestamp >= anchor.timestamp + 90);
    assert!(world.node.signals_since(0).unwrap().iter().any(|signal| {
        signal.name == "eez.composer.settlement.decision"
            && signal.fields["due"] == true
            && signal.fields["settlement_age_ms"]
                .as_u64()
                .is_some_and(|age| age >= 90_000)
    }));
    let follower = NodeHandle::start(
        "interval-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], 10, TIMEOUT)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_reevaluates_current_interval_without_resetting_the_l1_timer() {
    let mut world = setup_cross_chain_with_env(&policy_env(Vec::new(), "interval"))
        .await
        .unwrap();
    let anchor = registration(&world).await;
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        0
    );
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let before = l1
        .get_block_by_number(alloy_rpc_types_eth::BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;
    world
        .restart_composer_with_env(&[("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "45000".into())])
        .await
        .unwrap();
    let post = latest_post(&world).await;
    let timestamp = l1
        .get_block_by_hash(post.block_hash.unwrap())
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;
    assert!(timestamp >= anchor.timestamp + 45);
    assert!(
        timestamp < before + 45,
        "restart must not become a new interval anchor"
    );
    let receipt = l1
        .get_transaction_receipt(post.transaction_hash.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(receipt.status());
    world.node.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn general_block_rule_consolidates_empty_history_before_the_pure_interval() {
    let mut env = policy_env(Vec::new(), "interval");
    env.push(("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "30".into()));
    let world = setup_cross_chain_with_env(&env).await.unwrap();
    let cost = eez_testkit::wait_for(TIMEOUT, || async {
        Ok(world.node.signals_since(0)?.into_iter().find(|signal| {
            signal.name == "eez.composer.postbatch.actual_cost"
                && signal.fields["action"] == "apply"
                && signal.fields["post_included"] == true
                && signal.fields["included_l2_transactions"] == 0
                && signal.fields["settled_l2_blocks"]
                    .as_u64()
                    .is_some_and(|blocks| blocks >= 30)
        }))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "empty consolidation missing: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    assert!(
        cost.fields["post_cost_per_l2_transaction"]
            .as_str()
            .is_some_and(|value| value == "None")
    );
    assert!(cost.fields["post_blob_gas_used"].as_u64().unwrap() > 0);
    assert!(world.node.signals_since(0).unwrap().iter().any(|signal| {
        signal.name == "eez.composer.settlement.decision"
            && signal.fields["due"] == true
            && signal.fields["settlement_age_ms"]
                .as_u64()
                .is_some_and(|age| age < 900_000)
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retained_backlog_posts_more_than_six_blobs_in_one_prague_batch_and_replays() {
    use eez_testkit::{NativeL1Profile, onchain_nonce, receipt_ok, sign_and_send, wait_for};
    let mut world = eez_testkit::setup_cross_chain_with_profile(
        NativeL1Profile::EthereumPrague,
        &policy_env(Vec::new(), "interval"),
    )
    .await
    .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1).await.unwrap();
    let mut hashes = Vec::new();
    // Eight individually valid transactions force a physical allowance above
    // six. Zero bytes are real transaction data, never discardable padding.
    for index in 0..8 {
        hashes.push(
            sign_and_send(
                &world.l2_rpc(),
                ANVIL_KEY_1,
                world.l2_chain_id,
                nonce + index,
                Some(ANVIL_ADDR_3),
                U256::ZERO,
                vec![0; 115_000],
                3_000_000,
            )
            .await
            .unwrap(),
        );
    }
    for &hash in &hashes {
        assert!(
            wait_for(TIMEOUT, || async {
                receipt_ok(&world.l2_rpc(), hash).await
            })
            .await
            .unwrap()
        );
    }
    let covered = l2
        .get_transaction_receipt(*hashes.last().unwrap())
        .await
        .unwrap()
        .unwrap()
        .block_number
        .unwrap();
    // Include an empty tail in the retained backlog. The graceful policy
    // restart below flushes it; elapsed blocks alone do not prove persistence.
    wait_for_latest_height(&world.node, covered + 12, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        0
    );
    // Start before posting to exercise live historical-fork admission as well.
    let follower = NodeHandle::start(
        "multiblob-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    // Only the existing persistent witness store survives this restart.
    world
        .restart_composer_with_env(&[("EEZ_SETTLEMENT_PURE_L2_MODE", "always".into())])
        .await
        .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let post = latest_post(&world).await;
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let transaction = l1
        .get_transaction_by_hash(post.transaction_hash.unwrap())
        .await
        .unwrap()
        .unwrap();
    let count = transaction.inner.blob_versioned_hashes().unwrap().len();
    assert!(
        count > 6 && count <= 9,
        "entire fitting backlog must use one multiblob post: {count}"
    );
    wait_for_safe_prefix_convergence(&[&world.node, &follower], covered, TIMEOUT)
        .await
        .unwrap();
    let replay = ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap());
    for hash in hashes {
        let original = l2.get_transaction_receipt(hash).await.unwrap().unwrap();
        let derived = replay.get_transaction_receipt(hash).await.unwrap().unwrap();
        assert_eq!(original.block_hash, derived.block_hash);
        assert_eq!(original.status(), derived.status());
    }
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        1
    );
    assert!(
        !world
            .node
            .signals_since(0)
            .unwrap()
            .iter()
            .any(|signal| signal.name == "eez.composer.emission.historical_chunk")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_chain_overrides_interval_and_validates_two_ordered_companions() {
    use eez_testkit::{
        ANVIL_KEY_2, DEV_CHAIN_ID, IValue, l2_value, onchain_nonce, receipt_ok, sign_and_send,
        wait_for,
    };
    let mut env = policy_env(Vec::new(), "interval");
    // The native fixture relay forwards in-order into a fee-sorted dev pool.
    // Its explicit minimum keeps the poster ahead of the 1-gwei user tips.
    env.push(("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "1000000001".into()));
    let world = setup_cross_chain_with_env(&env).await.unwrap();
    let cursor = world.node.signal_cursor().unwrap();
    wait_for(TIMEOUT, || async {
        Ok(world
            .node
            .signals_since(cursor)?
            .into_iter()
            .any(|signal| {
                signal.name == "eez.composer.settlement.decision" && signal.fields["due"] == false
            })
            .then_some(()))
    })
    .await
    .unwrap();
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        0
    );
    let nonce = onchain_nonce(&world.l1_rpc(), ANVIL_KEY_2).await.unwrap();
    let first = sign_and_send(
        &world.l1_xchain(),
        ANVIL_KEY_2,
        DEV_CHAIN_ID,
        nonce,
        Some(world.setter_proxy),
        U256::ZERO,
        IValue::setValueCall { v: U256::from(41) }.abi_encode(),
        800_000,
    )
    .await
    .unwrap();
    let second = sign_and_send(
        &world.l1_xchain(),
        ANVIL_KEY_2,
        DEV_CHAIN_ID,
        nonce + 1,
        Some(world.setter_proxy),
        U256::ZERO,
        IValue::setValueCall { v: U256::from(42) }.abi_encode(),
        800_000,
    )
    .await
    .unwrap();
    for hash in [first, second] {
        let success = wait_for(TIMEOUT, || async {
            receipt_ok(&world.l1_rpc(), hash).await
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "cross-chain source was not included: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
        assert!(
            success,
            "admitted source must complete the actual deferred entries"
        );
    }
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let a = l1.get_transaction_receipt(first).await.unwrap().unwrap();
    let b = l1.get_transaction_receipt(second).await.unwrap().unwrap();
    assert_eq!(
        a.block_hash, b.block_hash,
        "both admitted groups must share the ordered L1 bundle"
    );
    assert!(a.transaction_index < b.transaction_index);
    for receipt in [&a, &b] {
        assert!(
            receipt
                .inner
                .logs()
                .iter()
                .any(|log| log.address() == world.dep.eez_address
                    && log.topic0() == Some(&eez_protocol::abi::ExecutionConsumed::SIGNATURE_HASH)),
            "each source transaction must consume its own proven entry"
        );
    }
    wait_for(TIMEOUT, || async {
        Ok((l2_value(&world.l2_rpc(), world.value_l2).await? == U256::from(42)).then_some(()))
    })
    .await
    .unwrap();
    let signals = world.node.signals_since(cursor).unwrap();
    assert!(
        signals
            .iter()
            .any(|signal| signal.name == "eez.composer.postbatch.preflight"
                && signal.fields["companion_gas_used"]
                    .as_str()
                    .is_some_and(|gas| gas.contains(','))),
        "preflight must validate both companions, not just the poster"
    );
    world.node.assert_no_process_death();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blob_overflow_drains_largest_empty_prefixes_without_resetting_the_interval() {
    assert_capacity_drain(false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_after_intermediate_prefix_discards_old_intent_and_uses_current_rules() {
    assert_capacity_drain(true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_chain_group_alone_makes_its_finite_historical_predecessor_due() {
    assert_capacity_drain(false, true).await;
}

async fn assert_capacity_drain(restart_after_prefix: bool, cross_chain_trigger: bool) {
    use eez_testkit::{onchain_nonce, receipt_ok, sign_and_send, wait_for};
    let mut env = policy_env(vec![], "interval");
    env.push(("EEZ_MAX_BLOBS_PER_BATCH", "2".into()));
    if cross_chain_trigger {
        env.push(("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "1000000001".into()));
    }
    let mut world = eez_testkit::setup_cross_chain_with_env(&env).await.unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1).await.unwrap();
    let mut hashes = Vec::new();
    let mut covered = 0;
    for index in 0..4 {
        let hash = sign_and_send(
            &world.l2_rpc(),
            ANVIL_KEY_1,
            world.l2_chain_id,
            nonce + index,
            Some(ANVIL_ADDR_3),
            U256::ZERO,
            vec![0; 80_000],
            3_000_000,
        )
        .await
        .unwrap();
        assert!(
            wait_for(TIMEOUT, || async {
                receipt_ok(&world.l2_rpc(), hash).await
            })
            .await
            .unwrap()
        );
        covered = l2
            .get_transaction_receipt(hash)
            .await
            .unwrap()
            .unwrap()
            .block_number
            .unwrap();
        // Each unit is below the hard allowance and has an empty Sync after it.
        wait_for_latest_height(&world.node, covered + 6, TIMEOUT)
            .await
            .unwrap();
        hashes.push(hash);
    }
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        0
    );
    if restart_after_prefix {
        // Let the first drain settlement through and hold the second, armed
        // before the drain starts so the hold cannot miss the second bundle.
        world.hold_bundle_after(1).unwrap();
    }
    if !cross_chain_trigger {
        world
            .restart_composer_with_env(&[("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "45000".into())])
            .await
            .unwrap();
    }
    let follower = NodeHandle::start(
        "split-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let mut planning_signals = Vec::new();
    if cross_chain_trigger {
        let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
        let source = sign_and_send(
            &world.l1_xchain(),
            eez_testkit::ANVIL_KEY_2,
            l1.get_chain_id().await.unwrap(),
            onchain_nonce(&world.l1_rpc(), eez_testkit::ANVIL_KEY_2)
                .await
                .unwrap(),
            Some(world.setter_proxy),
            U256::ZERO,
            eez_testkit::IValue::setValueCall { v: U256::from(17) }.abi_encode(),
            600_000,
        )
        .await
        .unwrap();
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l1.get_transaction_receipt(source).await?)
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "cross-chain predecessor did not drain: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
        assert!(receipt.status());
        let events = world.node.signals_since(0).unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.name == "eez.composer.settlement.predecessor")
        );
        let dispatched = events.iter().find(|event| {
            event.name == "eez.composer.bundle.dispatched"
                && event.fields["user_hashes"] == format!("[{source:?}]")
        });
        let dispatched = dispatched.expect("the re-composed source must be dispatched once");
        assert_eq!(dispatched.fields["attempt_counts"], "[0]");
        covered = covered.max(dispatched.fields["sync_height"].as_u64().unwrap());
    }
    if restart_after_prefix {
        let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
        let first = wait_for(TIMEOUT, || async {
            Ok(l2
                .get_block_by_number(alloy_eips::BlockNumberOrTag::Safe)
                .await?
                .filter(|block| block.header.number > 0))
        })
        .await
        .unwrap();
        assert!(
            first.header.number < covered,
            "fixture must restart during the drain"
        );
        // The second drain bundle is held (armed before the drain), so the
        // graceful policy restart occurs before a second canonical settlement.
        wait_for(TIMEOUT, || async { world.held_bundle() })
            .await
            .unwrap();
        let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
        let posts = l1
            .get_logs(
                &alloy_rpc_types_eth::Filter::new()
                    .address(world.dep.eez_address)
                    .event_signature(eez_protocol::abi::BatchPosted::SIGNATURE_HASH)
                    .from_block(0),
            )
            .await
            .unwrap();
        assert_eq!(posts.len(), 1, "fixture must hold the second settlement");
        // Restart also stops the isolated L1 miner. Its canonical RPC suffix
        // can precede disk persistence, and no peer can restore that suffix.
        world
            .wait_for_persisted_l1_block(
                alloy_rpc_types_eth::BlockNumHash::new(
                    posts[0].block_number.unwrap(),
                    posts[0].block_hash.unwrap(),
                ),
                TIMEOUT,
            )
            .await
            .unwrap();
        planning_signals = world.node.signals_since(0).unwrap();
        world
            .restart_composer_with_env(&[
                ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
                ("EEZ_SETTLEMENT_BLOB_FULLNESS_BPS", "off".into()),
            ])
            .await
            .unwrap();
        let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
        let height = l2.get_block_number().await.unwrap();
        wait_for_latest_height(&world.node, height + 12, TIMEOUT)
            .await
            .unwrap();
        assert_eq!(
            batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
                .await
                .unwrap(),
            1,
            "restarting deliberately loses the old drain; current rules are not due"
        );
        assert_eq!(
            l2.get_block_by_number(alloy_eips::BlockNumberOrTag::Safe)
                .await
                .unwrap()
                .unwrap()
                .header
                .hash,
            first.header.hash,
            "process start must not replace the canonical anchor"
        );
        world
            .restart_composer_with_env(&[("EEZ_SETTLEMENT_PURE_L2_MODE", "always".into())])
            .await
            .unwrap();
    }
    wait_for_safe_prefix_convergence(&[&world.node, &follower], covered, TIMEOUT)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "capacity drain did not finish: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let posts = l1
        .get_logs(
            &alloy_rpc_types_eth::Filter::new()
                .address(world.dep.eez_address)
                .event_signature(eez_protocol::abi::BatchPosted::SIGNATURE_HASH)
                .from_block(0),
        )
        .await
        .unwrap();
    if cross_chain_trigger {
        // A non-steady opportunity can finish the finite predecessor while
        // holding the fresh group for a subsequent steady opportunity.
        assert!((2..=3).contains(&posts.len()));
    } else {
        assert_eq!(
            posts.len(),
            2,
            "four blobs of separated history need two complete two-blob posts"
        );
    }
    let mut times = Vec::new();
    let mut maximum_posted_blobs = 0;
    for post in &posts {
        let tx = l1
            .get_transaction_by_hash(post.transaction_hash.unwrap())
            .await
            .unwrap()
            .unwrap();
        let blobs = tx.inner.blob_versioned_hashes().unwrap().len();
        assert!((1..=2).contains(&blobs));
        maximum_posted_blobs = maximum_posted_blobs.max(blobs);
        let receipt = l1
            .get_transaction_receipt(post.transaction_hash.unwrap())
            .await
            .unwrap()
            .unwrap();
        assert!(receipt.status());
        times.push(
            l1.get_block_by_hash(post.block_hash.unwrap())
                .await
                .unwrap()
                .unwrap()
                .header
                .timestamp,
        );
    }
    assert_eq!(
        maximum_posted_blobs, 2,
        "the largest valid prefix must use the available two-blob allowance"
    );
    if !restart_after_prefix {
        assert!(
            times[1] - times[0] < 45,
            "an intermediate anchor must not restart the unfinished drain's timer"
        );
    }
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let replay = ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap());
    for hash in hashes {
        assert_eq!(
            l2.get_transaction_receipt(hash)
                .await
                .unwrap()
                .unwrap()
                .block_hash,
            replay
                .get_transaction_receipt(hash)
                .await
                .unwrap()
                .unwrap()
                .block_hash
        );
    }
    planning_signals.extend(world.node.signals_since(0).unwrap());
    assert!(planning_signals.iter().any(|event| {
        event.name == "eez.composer.emission.historical_chunk"
            && event.fields["split_reasons"]
                .as_str()
                .is_some_and(|reasons| reasons.contains("blob_limit"))
    }));
    let head = l2.get_block_number().await.unwrap();
    wait_for_latest_height(&world.node, head + 10, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        posts.len(),
        "covering the finite drain must stop empty follow-up posting"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fullness_overshoot_posts_all_fitting_blobs_without_waiting_for_other_rules() {
    use eez_testkit::{onchain_nonce, receipt_ok, sign_and_send, wait_for};
    let mut world = eez_testkit::setup_cross_chain_with_env(&policy_env(vec![], "interval"))
        .await
        .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1).await.unwrap();
    let mut hashes = Vec::new();
    for index in 0..2 {
        hashes.push(
            sign_and_send(
                &world.l2_rpc(),
                ANVIL_KEY_1,
                world.l2_chain_id,
                nonce + index,
                Some(ANVIL_ADDR_3),
                U256::ZERO,
                vec![0; 90_000],
                3_000_000,
            )
            .await
            .unwrap(),
        );
    }
    for &hash in &hashes {
        assert!(
            wait_for(TIMEOUT, || async {
                receipt_ok(&world.l2_rpc(), hash).await
            })
            .await
            .unwrap()
        );
    }
    let covered = l2
        .get_transaction_receipt(*hashes.last().unwrap())
        .await
        .unwrap()
        .unwrap()
        .block_number
        .unwrap();
    // Include an empty tail in the retained backlog. The graceful policy
    // restart below flushes it; elapsed blocks alone do not prove persistence.
    wait_for_latest_height(&world.node, covered + 12, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        0
    );
    world
        .restart_composer_with_env(&[("EEZ_SETTLEMENT_BLOB_FULLNESS_BPS", "1000".into())])
        .await
        .unwrap();
    let post = latest_post(&world).await;
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let tx = l1
        .get_transaction_by_hash(post.transaction_hash.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        tx.inner.blob_versioned_hashes().unwrap().len(),
        2,
        "a soft threshold must not cut a fitting two-blob range into one-blob posts"
    );
    let follower = NodeHandle::start(
        "fullness-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], covered, TIMEOUT)
        .await
        .unwrap();
    let signals = world.node.signals_since(0).unwrap();
    assert!(signals.iter().any(|event| {
        event.name == "eez.composer.settlement.decision"
            && event.fields["due"] == true
            && event.fields["settlement_age_ms"]
                .as_u64()
                .is_some_and(|age| age < 900_000)
            && event.fields["unsettled_blocks"]
                .as_u64()
                .is_some_and(|blocks| blocks < 10_000)
    }));
    assert!(
        !signals
            .iter()
            .any(|event| event.name == "eez.composer.emission.historical_chunk")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_ordinary_burst_is_budgeted_across_sync_intervals() {
    ordinary_burst_is_budgeted(eez_testkit::NativeL1Profile::EthereumOsaka, 6).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_ordinary_burst_is_budgeted_across_sync_intervals() {
    ordinary_burst_is_budgeted(eez_testkit::NativeL1Profile::GnosisPrague, 2).await;
}

async fn ordinary_burst_is_budgeted(profile: eez_testkit::NativeL1Profile, max_blobs: usize) {
    use eez_testkit::{onchain_nonce, sign_and_send, wait_for};
    let mut env = policy_env(vec![], "interval");
    env.push(("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "120000".into()));
    // Six-blob KZG preparation plus final preflight exceeds the default
    // two-second window on CI. Start earlier in the same L1 slot; the proof
    // still binds its known parent and submission keeps the original cutoff.
    env.push(("EEZ_PREPARATION_TIME_MS", "2500".into()));
    // The native relay inserts the ordered bundle into a fee-sorted dev pool.
    // Keep the post ahead of its 1-gwei companion, as in the other bundle tests.
    env.push(("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "1000000001".into()));
    let world = eez_testkit::setup_cross_chain_with_profile_and_rpc_faults(profile, &env)
        .await
        .unwrap();
    // Delay the canonical reader, not the whole process: the native fixture's
    // embedded L1 must keep mining at its normal cadence during the outage.
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l1_before = l1.get_block_number().await.unwrap();
    let nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1).await.unwrap();
    let timing_env = world.partial_follower_env();
    let milliseconds = |name| {
        timing_env
            .iter()
            .find(|(key, _)| *key == name)
            .unwrap()
            .1
            .parse::<u64>()
            .unwrap()
    };
    let l2_seconds = milliseconds("EEZ_L2_BLOCK_TIME_MS") / 1000;
    let k = milliseconds("EEZ_L1_BLOCK_TIME_MS") / milliseconds("EEZ_L2_BLOCK_TIME_MS");
    world.fail_rpc_methods(&["eth_getBlockByNumber"]).unwrap();
    let mut hashes = Vec::new();
    // Each fits by itself on both networks. The calldata alone must exceed the
    // network's whole interval capacity even without any encoding overhead.
    let input_bytes = 70_000;
    let transaction_count =
        (max_blobs * eez_blob_codec::DATA_BYTES_PER_BLOB / input_bytes + 1) as u64;
    for offset in 0..transaction_count {
        hashes.push(
            sign_and_send(
                &world.l2_rpc(),
                ANVIL_KEY_1,
                world.l2_chain_id,
                nonce + offset,
                Some(ANVIL_ADDR_3),
                U256::from(1),
                vec![0xff; input_bytes],
                4_000_000,
            )
            .await
            .unwrap(),
        );
    }
    // Cached scheduling can continue briefly after the reader loses RPC.
    // Recover only after L1 has actually passed the stalled L2 tip by two
    // slots; a fixed sleep can leave an empty recovery window on Gnosis.
    let gap_seconds = 2 * milliseconds("EEZ_L1_BLOCK_TIME_MS") / 1000;
    let (stalled_l2_timestamp, recovery_l1_timestamp) = wait_for(TIMEOUT, || async {
        let l2_timestamp = l2
            .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| anyhow::anyhow!("missing L2 tip during RPC outage"))?
            .header
            .timestamp;
        let l1_tip = l1
            .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| anyhow::anyhow!("missing L1 tip during RPC outage"))?;
        let l1_timestamp = l1_tip.header.timestamp;
        Ok(
            (l1_tip.header.number >= l1_before + 2 && l1_timestamp >= l2_timestamp + gap_seconds)
                .then_some((l2_timestamp, l1_timestamp)),
        )
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "RPC outage did not create a historical gap: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    assert!(l1.get_block_number().await.unwrap() >= l1_before + 2);
    let recovery_cursor = world.node.signal_cursor().unwrap();
    world.fail_rpc_methods(&[]).unwrap();
    let mut receipts = Vec::new();
    for &hash in &hashes {
        receipts.push(
            wait_for(TIMEOUT, || async {
                Ok(l2.get_transaction_receipt(hash).await?)
            })
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "ordinary overflow never drained: {e:#}{}",
                    world.settlement_diagnostics()
                )
            }),
        );
    }
    assert!(
        receipts
            .iter()
            .all(alloy_rpc_types_eth::TransactionReceipt::status)
    );
    let first = receipts.first().unwrap().block_number.unwrap();
    let covered = receipts.last().unwrap().block_number.unwrap();
    let mut previous_l1_timestamp = l1
        .get_block_by_number(l1_before.into())
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;
    for height in (l1_before + 1)..=l1.get_block_number().await.unwrap() {
        let timestamp = l1
            .get_block_by_number(height.into())
            .await
            .unwrap()
            .unwrap()
            .header
            .timestamp;
        assert_eq!(
            (timestamp - previous_l1_timestamp) % (k * l2_seconds),
            0,
            "the native L1 fixture must preserve its configured slot alignment"
        );
        previous_l1_timestamp = timestamp;
    }
    wait_for_latest_height(&world.node, covered + 2 * k, TIMEOUT)
        .await
        .unwrap();
    let genesis = l2
        .get_block_by_number(0.into())
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;
    // The scheduler can have a nonzero residue relative to L2 genesis. Inspect
    // actual empty Sync construction, never assume height % K == 0.
    // Recovery may finish in Steady mode once it reaches a usable future slot.
    // Prove that a missing historical boundary was rebuilt after the outage,
    // rather than requiring a structural-only Catchup terminal. Cached retry
    // slots can already satisfy the receipt/height waits before RPC recovers;
    // wait for this recovery event and its committed block explicitly.
    let (rebuilt_timestamp, rebuilt) = wait_for(TIMEOUT, || async {
        let rebuilt_timestamp = world
            .node
            .signals_since(recovery_cursor)?
            .iter()
            .filter(|event| {
                event.name == "eez.composer.sync_slot.invoked" && event.fields["mode"] == "Empty"
            })
            .filter_map(|event| event.fields["timestamp"].as_u64())
            .find(|timestamp| *timestamp > stalled_l2_timestamp && *timestamp <= recovery_l1_timestamp);
        let Some(timestamp) = rebuilt_timestamp else {
            return Ok(None);
        };
        let number = (timestamp - genesis) / l2_seconds;
        Ok(l2
            .get_block_by_number(number.into())
            .full()
            .await?
            .map(|block| (timestamp, block)))
    })
    .await
    .unwrap_or_else(|error| panic!(
        "RPC recovery did not rebuild a missing historical Sync boundary in ({stalled_l2_timestamp}, {recovery_l1_timestamp}]: {error:#}{}",
        world.settlement_diagnostics(),
    ));
    assert_eq!(rebuilt.header.timestamp, rebuilt_timestamp);
    assert!(
        rebuilt.transactions.is_empty(),
        "recovered Sync boundary must be empty on the canonical L2"
    );
    let boundaries = world
        .node
        .signals_since(0)
        .unwrap()
        .into_iter()
        .filter(|event| event.name == "eez.composer.sync_slot.invoked")
        .map(|event| (event.fields["timestamp"].as_u64().unwrap() - genesis) / l2_seconds)
        .collect::<std::collections::BTreeSet<_>>();
    let base = boundaries.range(..first).next_back().copied().unwrap_or(0);
    let terminal = *boundaries
        .range(covered..)
        .next()
        .expect("a final empty Sync must be produced");
    let mut intervals = Vec::new();
    let mut encoded = Vec::new();
    for number in (base + 1)..=terminal {
        let block = l2
            .get_block_by_number(number.into())
            .full()
            .await
            .unwrap()
            .unwrap();
        let block = block
            .map_transactions(|tx| tx.inner.into_inner())
            .into_consensus();
        if boundaries.contains(&number) {
            assert!(
                block.body.transactions.is_empty(),
                "ordinary data occupied a required settlement boundary {number}"
            );
        }
        encoded.push(alloy_rlp::encode(&block));
        if boundaries.contains(&number) {
            let bytes =
                eez_blob_codec::native_stream_len(full_history_payload_len(&encoded), 0).unwrap();
            assert!(
                bytes <= max_blobs * eez_blob_codec::DATA_BYTES_PER_BLOB,
                "unpostable interval ending {number}: {bytes}"
            );
            intervals.push(bytes);
            encoded.clear();
        }
    }
    assert!(intervals.len() > 1, "overflow must cross a usable boundary");
    assert_eq!(
        onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1).await.unwrap(),
        nonce + transaction_count
    );
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        0,
        "DA budgeting must not override the settlement timer"
    );
    // Keep the same L1 running while the configured interval becomes due.
    let follower = NodeHandle::start(
        "ordinary-budget-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], covered, TIMEOUT)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "postable intervals failed to settle: {e:#}{}",
                world.settlement_diagnostics()
            )
        });
    let replay = ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap());
    for (hash, receipt) in hashes.iter().zip(&receipts) {
        let replayed = replay
            .get_transaction_receipt(*hash)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replayed.block_hash, receipt.block_hash);
        assert_eq!(replayed.status(), receipt.status());
    }
    // The original incident starved queued bridges behind ordinary history.
    // Verify a cross-chain source still executes after draining this burst.
    let source = sign_and_send(
        &world.l1_xchain(),
        eez_testkit::ANVIL_KEY_2,
        ProviderBuilder::new()
            .connect_http(world.l1_rpc().parse().unwrap())
            .get_chain_id()
            .await
            .unwrap(),
        onchain_nonce(&world.l1_rpc(), eez_testkit::ANVIL_KEY_2)
            .await
            .unwrap(),
        Some(world.setter_proxy),
        U256::ZERO,
        eez_testkit::IValue::setValueCall { v: U256::from(777) }.abi_encode(),
        400_000,
    )
    .await
    .unwrap();
    assert!(
        wait_for(TIMEOUT, || async {
            Ok(l1.get_transaction_receipt(source).await?)
        })
        .await
        .unwrap()
        .status()
    );
    wait_for(TIMEOUT, || async {
        Ok(
            (eez_testkit::l2_value(&follower.l2_rpc_url(), world.value_l2).await?
                == U256::from(777))
            .then_some(()),
        )
    })
    .await
    .unwrap();
    let logs = l1
        .get_logs(
            &alloy_rpc_types_eth::Filter::new()
                .address(world.dep.eez_address)
                .event_signature(eez_protocol::abi::BatchPosted::SIGNATURE_HASH)
                .from_block(0),
        )
        .await
        .unwrap();
    let mut maximum_posted = 0;
    for log in logs {
        let post = l1
            .get_transaction_by_hash(log.transaction_hash.unwrap())
            .await
            .unwrap()
            .unwrap();
        let count = post.inner.blob_versioned_hashes().unwrap().len();
        assert!(count <= max_blobs);
        maximum_posted = maximum_posted.max(count);
    }
    assert!(
        (2..=max_blobs).contains(&maximum_posted),
        "the burst must exercise multi-blob posting within the configured cap"
    );
    let signals = world.node.signals_since(0).unwrap();
    assert!(
        signals.iter().any(|event| {
            event.name == "eez.composer.settlement.preparation_time"
                && event.fields["blob_count"]
                    .as_u64()
                    .is_some_and(|count| count >= 2)
                && event.fields["preparation_ms"].as_u64().is_some()
        }),
        "multi-blob preparation must record a timing sample"
    );
    // Proofs bind a known canonical parent. A late preparation estimate must
    // not choose a later execution height whose parent has not been produced.
    let selections = signals
        .iter()
        .filter(|event| event.name == "eez.submitter.policy.target_selected")
        .collect::<Vec<_>>();
    assert!(
        !selections.is_empty(),
        "recovery must select a usable policy target"
    );
    for event in selections {
        let value = |key: &str| event.fields[key].as_u64().unwrap();
        assert_eq!(value("head_number") + 1, value("target_block"));
        assert!(
            (1..=eez_l1::submitter::MAX_MISSING_SLOT_REPLACEMENTS + 1)
                .contains(&value("slot_offset"))
        );
        assert_eq!(
            value("head_timestamp") * 1000
                + value("slot_offset") * milliseconds("EEZ_L1_BLOCK_TIME_MS"),
            value("target_timestamp") * 1000
        );
        assert!(
            value("selected_at_ms") + value("submission_slack_ms")
                < value("target_timestamp") * 1000
        );
    }
    assert_eq!(
        world
            .node
            .log_count_matching(&["unpostable_range"])
            .unwrap(),
        0
    );
    world.node.assert_no_divergence_failure_logs();
    follower.assert_no_divergence_failure_logs();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_rpc_rejects_unpostable_data_without_consuming_nonce() {
    let mut env = policy_env(vec![], "always");
    env.push(("EEZ_MAX_BLOBS_PER_BATCH", "1".into()));
    let world = setup_cross_chain_with_env(&env).await.unwrap();
    let nonce = eez_testkit::onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1)
        .await
        .unwrap();
    let error = eez_testkit::sign_and_send(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        world.l2_chain_id,
        nonce,
        Some(ANVIL_ADDR_3),
        U256::ZERO,
        // The calldata alone fills the blob; signed-transaction and interval
        // framing make it unpostable regardless of byte-string compression.
        vec![0xff; eez_blob_codec::DATA_BYTES_PER_BLOB],
        8_000_000,
    )
    .await
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("cannot fit a settlement interval"),
        "{error:#}"
    );
    let hash = send_l2_value_transfer_confirmed(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(1),
        TIMEOUT,
    )
    .await
    .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let receipt = l2.get_transaction_receipt(hash).await.unwrap().unwrap();
    assert!(receipt.status());
    assert_eq!(
        eez_testkit::onchain_nonce(&world.l2_rpc(), ANVIL_KEY_1)
            .await
            .unwrap(),
        nonce + 1
    );
    wait_for_safe_prefix_convergence(&[&world.node], receipt.block_number.unwrap(), TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        world
            .node
            .log_count_matching(&["unpostable_range"])
            .unwrap(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_fee_waits_do_not_consume_the_first_outbound_selection() {
    assert_fee_waits_do_not_consume_selection(eez_testkit::NativeL1Profile::EthereumOsaka).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_fee_waits_do_not_consume_the_first_outbound_selection() {
    assert_fee_waits_do_not_consume_selection(eez_testkit::NativeL1Profile::GnosisPrague).await;
}

async fn assert_fee_waits_do_not_consume_selection(profile: eez_testkit::NativeL1Profile) {
    use alloy_consensus::{SignableTransaction as _, TxEip1559};
    use alloy_eips::Encodable2718 as _;
    use alloy_network::TxSignerSync as _;
    use alloy_primitives::TxKind;
    use alloy_signer_local::PrivateKeySigner;
    use eez_testkit::{ANVIL_KEY_2, ANVIL_KEY_4, IValue, onchain_nonce, sign_and_send, wait_for};

    let mut env = policy_env(Vec::new(), "interval");
    env.extend([
        ("EEZ_MAX_USER_TXS_PER_BUNDLE", "3".into()),
        ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "1000000001".into()),
    ]);
    let world = eez_testkit::setup_cross_chain_with_profile(profile, &env)
        .await
        .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let front = ProviderBuilder::new().connect_http(world.l1_xchain().parse().unwrap());
    let inbound_nonce = onchain_nonce(&world.l1_rpc(), ANVIL_KEY_2).await.unwrap();
    let outbound_nonce = onchain_nonce(&world.l2_rpc(), ANVIL_KEY_4).await.unwrap();
    let chain_id = l1.get_chain_id().await.unwrap();
    let signer: PrivateKeySigner = ANVIL_KEY_2.parse().unwrap();
    // Construct a complete signed wave first. Queue it immediately after an idle
    // opportunity so its arrival cannot straddle the next finite snapshot.
    let blocked = (0..3)
        .map(|offset| {
            let mut tx = TxEip1559 {
                chain_id,
                nonce: inbound_nonce + offset,
                gas_limit: 400_000,
                max_fee_per_gas: 0,
                max_priority_fee_per_gas: 0,
                to: TxKind::Call(world.setter_proxy),
                input: IValue::setValueCall {
                    v: U256::from(offset + 1),
                }
                .abi_encode()
                .into(),
                ..Default::default()
            };
            let signature = signer.sign_transaction_sync(&mut tx).unwrap();
            alloy_consensus::TxEnvelope::from(tx.into_signed(signature))
        })
        .collect::<Vec<_>>();
    let idle = world.node.signal_cursor().unwrap();
    wait_for(TIMEOUT, || async {
        Ok(world
            .node
            .signals_since(idle)?
            .iter()
            .any(|e| e.name == "eez.composer.settlement.decision" && e.fields["due"] == false)
            .then_some(()))
    })
    .await
    .unwrap();
    let cursor = world.node.signal_cursor().unwrap();
    for tx in &blocked {
        assert_eq!(
            front
                .send_raw_transaction(&tx.encoded_2718())
                .await
                .unwrap()
                .tx_hash(),
            tx.tx_hash()
        );
    }
    let mut ready = Vec::new();
    for offset in 0..2 {
        ready.push(
            sign_and_send(
                &world.l2_xchain(),
                ANVIL_KEY_4,
                world.l2_chain_id,
                outbound_nonce + offset,
                Some(world.outbound_proxy),
                U256::ZERO,
                IValue::setValueCall {
                    v: U256::from(10 + offset),
                }
                .abi_encode(),
                400_000,
            )
            .await
            .unwrap(),
        );
    }
    let mut settled_block = None;
    for hash in ready {
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l2.get_transaction_receipt(hash).await?)
        })
        .await
        .unwrap_or_else(|e| {
            panic!(
                "ready outbound stalled: {e:#}{}",
                world.settlement_diagnostics()
            )
        });
        assert!(receipt.status());
        if let Some(block) = settled_block {
            assert_eq!(receipt.block_number, Some(block));
        }
        settled_block = receipt.block_number;
    }
    let events = world.node.signals_since(cursor).unwrap();
    let drain = events
        .iter()
        .position(|e| e.name == "eez.composer.sync_slot.drain")
        .unwrap();
    assert_eq!(
        events[drain].fields["candidate_count"], 5,
        "wave must reach one snapshot"
    );
    assert_eq!(events[drain].fields["selection_limit"], 3);
    let next = events[drain + 1..]
        .iter()
        .find(|e| {
            matches!(
                e.name.as_str(),
                "eez.composer.sync_slot.drain" | "eez.composer.admission.prepared"
            )
        })
        .unwrap();
    assert_eq!(
        next.name, "eez.composer.admission.prepared",
        "ready users must not wait for another selection"
    );
    assert_eq!(next.fields["accepted"], 2);
    assert_eq!(
        l1.get_transaction_count(signer.address()).await.unwrap(),
        inbound_nonce
    );
    for tx in &blocked {
        assert!(
            l1.get_transaction_receipt(*tx.tx_hash())
                .await
                .unwrap()
                .is_none()
        );
    }
    let follower = NodeHandle::start(
        "fee-selection-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], settled_block.unwrap(), TIMEOUT)
        .await
        .unwrap();
    let queue: serde_json::Value = front
        .client()
        .request("eez_getCrossChainQueueStatus", ())
        .await
        .unwrap();
    assert_eq!(queue["blocked"], 3);
}
