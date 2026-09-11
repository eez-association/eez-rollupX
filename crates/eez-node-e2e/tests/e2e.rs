//! End-to-end composer, follower, restart, outage, and reorg scenarios.

use std::time::Duration;

use alloy_primitives::{B256, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::BlockNumberOrTag;

use eez_testkit::signals;
use eez_testkit::{
    ANVIL_ADDR, ANVIL_ADDR_3, ANVIL_KEY, ANVIL_KEY_1, ANVIL_KEY_2, ANVIL_KEY_3, ANVIL_KEY_4,
    ANVIL_KEY_6, Harness, INVALID_PROOF_SELECTOR, INVALID_PROOF_SYSTEM_CONFIG_SELECTOR,
    L2_SYSTEM_KEY, NodeBinary, NodeConfig, NodeHandle, block_number_and_hash_at, override_env,
    safe_block_hash, send_l2_value_transfer, send_l2_value_transfer_confirmed, wait_for,
    wait_for_latest_height, wait_for_new_attested_safe_block, wait_for_safe_chain_contains,
    wait_for_safe_prefix_convergence, wait_for_safe_state,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_mins(5);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn node_fixture_ignores_parent_dotenv() {
    let harness = Harness::fresh().await.unwrap();
    let env = harness.env().await.unwrap();
    let parent = tempfile::tempdir().unwrap();
    // Either file would make startup fail if loaded from the parent directory.
    for name in [".env", "deployments.env"] {
        std::fs::write(
            parent.path().join(name),
            "EEZ_L2_SYSTEM_ADDRESS=not-an-address\n",
        )
        .unwrap();
    }
    let datadir = parent.path().join("node");
    std::fs::create_dir(&datadir).unwrap();
    let node =
        NodeHandle::start_with_datadir("isolated-env", &datadir, &NodeConfig::default(), &env)
            .await
            .expect("node must not read parent deployment files");
    harness
        .chain()
        .wait_for_batches(1, DEFAULT_TIMEOUT)
        .await
        .unwrap();
    node.assert_no_process_death();
}

/// A fixture reorg must replace history without shifting future submission
/// windows into the past. This needs no composer or proof signer to reproduce.
#[tokio::test]
async fn anvil_reorg_preserves_wall_clock() {
    let harness = Harness::for_reorg().await.unwrap();
    let chain = harness.chain();
    chain.set_interval_mining(0).await.unwrap();
    let provider = ProviderBuilder::new().connect_http(chain.rpc_url().parse().unwrap());
    let before = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    assert!(
        before.header.number >= 3,
        "fixture must have reorgable history"
    );
    let wall_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    harness.anvil.reorg(3).await.unwrap();
    let replacement = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replacement.header.number, before.header.number);
    assert_ne!(replacement.header.hash, before.header.hash);
    chain.mine().await.unwrap();
    let next = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.header.number, before.header.number + 1);
    assert!(next.header.timestamp > replacement.header.timestamp);
    assert!(
        next.header.timestamp >= wall_time,
        "Anvil's reorg left future blocks behind wall time: {} < {wall_time}",
        next.header.timestamp,
    );
}

/// A follower replaces the full divergent suffix of an intra-batch fork.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_composer_intra_batch_suffix_replay_converges() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let genesis = harness.l2_genesis_path();
    let composer_cfg = NodeConfig {
        genesis_path: Some(genesis),
        ..Default::default()
    };
    let follower_cfg = NodeConfig {
        binary: NodeBinary::Follower,
        genesis_path: Some(genesis),
    };

    let primary_dir = tempfile::tempdir().unwrap();
    let mirror_dir = tempfile::tempdir().unwrap();
    // The deterministic system key is not funded on Anvil. Both composers can
    // therefore sequence and prove, but neither can land a batch while the two
    // divergent local histories are being staged.
    let primary_stage_env = harness.env_for(L2_SYSTEM_KEY, true).await.unwrap();
    let mirror_stage_env = harness.env_for(L2_SYSTEM_KEY, true).await.unwrap();
    let (primary, mirror) = tokio::try_join!(
        NodeHandle::start_with_datadir(
            "intra-primary-stage",
            primary_dir.path(),
            &composer_cfg,
            &primary_stage_env,
        ),
        NodeHandle::start_with_datadir(
            "intra-mirror-stage",
            mirror_dir.path(),
            &composer_cfg,
            &mirror_stage_env,
        ),
    )
    .unwrap();

    let primary_rpc = primary.l2_rpc_url();
    let primary_provider = ProviderBuilder::new().connect_http(primary_rpc.parse().unwrap());
    let tx_hash = send_l2_value_transfer(&primary_rpc, ANVIL_KEY_1, ANVIL_ADDR, U256::from(1u64))
        .await
        .expect("submit divergent L2 transaction to primary composer");
    let receipt = wait_for(DEFAULT_TIMEOUT, || async {
        Ok(primary_provider.get_transaction_receipt(tx_hash).await?)
    })
    .await
    .unwrap_or_else(|err| panic!("wait for L2 tx {tx_hash} inclusion: {err:#}"));
    assert!(receipt.status(), "L2 tx {tx_hash} reverted");
    let included_block = receipt
        .block_number
        .unwrap_or_else(|| panic!("included L2 tx {tx_hash} missing block_number"));
    assert!(included_block > 0, "transaction must not land in genesis");

    let target = included_block + 3;
    let (primary_target, mirror_target) = tokio::try_join!(
        wait_for_latest_height(&primary, target, DEFAULT_TIMEOUT),
        wait_for_latest_height(&mirror, target, DEFAULT_TIMEOUT),
    )
    .expect("composers did not stage a multi-block suffix");
    assert_ne!(
        primary_target.hash, mirror_target.hash,
        "the staged suffix must actually diverge",
    );
    assert_eq!(
        chain.batches_posted().await.unwrap(),
        0,
        "unfunded staging composers must not settle either history",
    );

    drop(primary);
    drop(mirror);

    let follower_env = override_env(
        harness.follower_env(None).await.unwrap(),
        "RUST_LOG",
        "warn,eez_deriver=info,eez_l1=info",
    );
    let mirror = NodeHandle::start_with_datadir(
        "intra-mirror-follow",
        mirror_dir.path(),
        &follower_cfg,
        &follower_env,
    )
    .await
    .unwrap();
    let composer_env = override_env(
        harness.env_for(ANVIL_KEY, true).await.unwrap(),
        "RUST_LOG",
        "warn,eez_composer=info,eez_deriver=info,eez_l1=info,eez_prover_client=info",
    );
    let primary = NodeHandle::start_with_datadir(
        "intra-primary-compose",
        primary_dir.path(),
        &composer_cfg,
        &composer_env,
    )
    .await
    .unwrap();

    chain
        .wait_for_batches(1, DEFAULT_TIMEOUT)
        .await
        .expect("primary composer did not post its staged multi-block batch");
    wait_for_safe_prefix_convergence(&[&primary, &mirror], target, DEFAULT_TIMEOUT)
        .await
        .expect("follower did not replace the complete divergent suffix");

    primary.assert_no_divergence_failure_logs();
    mirror.assert_no_divergence_failure_logs();
}

/// Builder mode, sustained operation through a restart. Asserts every
/// observable invariant in one place:
///   - lockstep: `BatchPosted == L2ExecutionPerformed`, always;
///   - zero `L2TxSkipped` (no prestate/rolling-hash misfire);
///   - `latest_event.newRoot == rollups[rid].root` (event-state
///     consistency);
///   - state advances forward (≠ `B256::ZERO`, monotonic);
///   - across restart: the validated checkpoint seeds the L1 scan, counts
///     keep lockstep (no replay), and state keeps advancing.
///
/// Would have caught `immediateEntryCount = 0` (state never
/// advances) AND any future replay bug across the restart boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn happy_case_composer_sustained() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let datadir = tempfile::tempdir().unwrap();
    let mut env = harness.env().await.unwrap();
    env.iter_mut()
        .find(|(key, _)| *key == "RUST_LOG")
        .expect("test environment must configure RUST_LOG")
        .1 = "warn,eez_deriver=info".to_string();

    let n_before;
    let pre_restart_latest;
    {
        let node_before_restart = NodeHandle::start_with_datadir(
            "composer-before-restart",
            datadir.path(),
            &NodeConfig::default(),
            &env,
        )
        .await
        .unwrap();
        send_l2_value_transfer_confirmed(
            &node_before_restart.l2_rpc_url(),
            ANVIL_KEY_1,
            ANVIL_ADDR_3,
            U256::from(1u64),
            DEFAULT_TIMEOUT,
        )
        .await
        .expect("pre-restart L2 transfer did not land");
        wait_for_safe_state(
            &node_before_restart,
            &chain,
            harness.l2_genesis_block_hash().unwrap(),
            DEFAULT_TIMEOUT,
        )
        .await
        .expect("pre-restart state transition was not attested");
        chain.wait_for_batches(3, DEFAULT_TIMEOUT).await.unwrap();
        pre_restart_latest = wait_for_latest_height(&node_before_restart, 1, DEFAULT_TIMEOUT)
            .await
            .expect("pre-restart node did not produce L2 blocks");
        let before = chain.snapshot().await.unwrap();
        n_before = before.batches_posted;
        assert_eq!(before.executions_performed, n_before, "lockstep");
        assert_eq!(before.entries_skipped, 0, "no entry should revert");
        assert_ne!(before.state_root, harness.l2_genesis_block_hash().unwrap());
        assert_eq!(
            before.latest_execution_state.unwrap(),
            before.state_root,
            "latest event's newState == on-chain stateRoot",
        );
    }

    assert!(
        eez_deriver::ReconcileCheckpoint::load(datadir.path()).is_some(),
        "composer did not persist a reconciliation checkpoint before restart",
    );

    chain
        .wait_for_l1_blocks(2, Duration::from_secs(15))
        .await
        .unwrap();
    let node = NodeHandle::start_with_datadir(
        "composer-after-restart",
        datadir.path(),
        &NodeConfig::default(),
        &env,
    )
    .await
    .unwrap();
    wait_for(DEFAULT_TIMEOUT, || async {
        Ok((node
            .log_count_matching(&["boot seeded from the persistent L1/L2 correlation index"])?
            > 0)
        .then_some(()))
    })
    .await
    .expect("restarted composer did not seed its L1 scan from the persistent correlation index");
    assert_eq!(
        node.log_count_matching(&["boot seeded from the persistent L1/L2 correlation index"])
            .unwrap(),
        1,
        "restarted composer did not seed its L1 scan from the persistent correlation index",
    );

    chain
        .wait_for_batches(n_before + 1, DEFAULT_TIMEOUT)
        .await
        .expect("composer didn't post any new batch after restart");
    let after = chain.snapshot().await.unwrap();
    let n_after = after.batches_posted;
    assert!(
        n_after > n_before,
        "BatchPosted grew ({n_before} → {n_after})"
    );
    let post_restart_target_height = pre_restart_latest.number + 1;
    wait_for_latest_height(&node, post_restart_target_height, DEFAULT_TIMEOUT)
        .await
        .expect("restarted node did not advance L2 height after restart");
    assert_eq!(after.executions_performed, n_after, "no replay");
    assert_eq!(after.entries_skipped, 0, "no skipped entries after restart");
    assert_eq!(
        after.latest_execution_state.unwrap(),
        after.state_root,
        "event-state consistency holds across restart",
    );

    let follower_env = harness.follower_env(None).await.unwrap();
    let follower_cfg = NodeConfig {
        binary: NodeBinary::Follower,
        ..Default::default()
    };
    let follower = NodeHandle::start("follower", &follower_cfg, &follower_env)
        .await
        .unwrap();
    wait_for_safe_state(
        &follower,
        &chain,
        harness.l2_genesis_block_hash().unwrap(),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("follower did not catch up via L1 replay");
    wait_for_safe_prefix_convergence(
        &[&node, &follower],
        post_restart_target_height,
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("restarted node and replay follower did not converge on safe block hashes");
    follower.assert_no_process_death();
    node.assert_no_process_death();
}

/// Proofs for an unregistered rollup ID are signed but never posted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failure_wrong_rollup_id() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let env = harness.env_with_rollup_id(999).await.unwrap();
    let datadir = tempfile::tempdir().unwrap();
    let node = NodeHandle::start_with_datadir(
        "wrong-rollup",
        datadir.path(),
        &NodeConfig::default(),
        &env,
    )
    .await
    .unwrap();

    wait_for(Duration::from_mins(1), || async {
        Ok((harness.successful_attestations()? > 0).then_some(()))
    })
    .await
    .expect("wrong-rollup scenario never reached proof attestation");

    chain
        .wait_for_l1_blocks(5, Duration::from_secs(30))
        .await
        .unwrap();

    chain
        .assert_failed_post_and_verify_batch(999, INVALID_PROOF_SYSTEM_CONFIG_SELECTOR)
        .await
        .expect("wrong-rollup batch did not reach L1 structural validation");

    let snapshot = chain.snapshot().await.unwrap();
    assert_eq!(snapshot.batches_posted, 0);
    assert_eq!(snapshot.executions_performed, 0);
    assert_eq!(
        snapshot.state_root,
        harness.l2_genesis_block_hash().unwrap()
    );
    node.assert_no_process_death();
}

/// Posting resumes after an unfunded poster account is restored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failure_poster_funds_recovery() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let datadir = tempfile::tempdir().unwrap();
    let node = NodeHandle::start_with_datadir(
        "poster-funds",
        datadir.path(),
        &NodeConfig::default(),
        &harness.env().await.unwrap(),
    )
    .await
    .unwrap();

    chain
        .wait_for_batches(2, Duration::from_mins(1))
        .await
        .unwrap();

    harness
        .anvil
        .set_balance(ANVIL_ADDR, U256::ZERO)
        .await
        .unwrap();
    // Let work submitted before the balance change either land or fail before
    // measuring the outage interval.
    chain
        .wait_for_l1_blocks(1, Duration::from_secs(15))
        .await
        .unwrap();
    let outage_baseline = chain.batches_posted().await.unwrap();
    chain
        .wait_for_l1_blocks(5, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(
        chain.batches_posted().await.unwrap(),
        outage_baseline,
        "no progress during outage"
    );

    let restored = U256::from(10u64).pow(U256::from(21u64));
    harness
        .anvil
        .set_balance(ANVIL_ADDR, restored)
        .await
        .unwrap();
    chain
        .wait_for_batches(outage_baseline + 1, Duration::from_mins(1))
        .await
        .expect("composer did not recover after balance restored");
    node.assert_no_process_death();
}

/// A proof accepted by the composer but signed by an attester unauthorized
/// by the deployed L1 proof system reaches L1 and reverts with InvalidProof.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failure_prover_signer_mismatch() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let env = harness.env_with_proof_signer(ANVIL_KEY_1).await.unwrap();
    let datadir = tempfile::tempdir().unwrap();
    let node = NodeHandle::start_with_datadir(
        "signer-mismatch",
        datadir.path(),
        &NodeConfig::default(),
        &env,
    )
    .await
    .unwrap();

    wait_for(Duration::from_mins(1), || async {
        Ok((harness.successful_attestations()? > 0).then_some(()))
    })
    .await
    .expect("signer-mismatch scenario never reached proof attestation");

    chain
        .wait_for_l1_blocks(5, Duration::from_secs(30))
        .await
        .unwrap();

    chain
        .assert_failed_post_and_verify_batch(harness.dep.rollup_id, INVALID_PROOF_SELECTOR)
        .await
        .expect("unauthorized-attester batch did not reach L1 proof verification");

    let snapshot = chain.snapshot().await.unwrap();
    assert_eq!(snapshot.batches_posted, 0);
    assert_eq!(snapshot.executions_performed, 0);
    assert_eq!(
        snapshot.state_root,
        harness.l2_genesis_block_hash().unwrap()
    );
    node.assert_no_process_death();
}

/// Competing composers retreat across an L1 reorg and reconverge.
/// They must produce newly attested work before old-prefix agreement can pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn happy_case_two_composers_l1_reorg_recovers() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let genesis = harness.l2_genesis_path();
    let cfg = NodeConfig {
        genesis_path: Some(genesis),
        ..Default::default()
    };
    let env1 = harness.env_for(ANVIL_KEY, true).await.unwrap();
    let env2 = harness.env_for(ANVIL_KEY_4, true).await.unwrap();
    let (c1, c2) = tokio::try_join!(
        NodeHandle::start("c1", &cfg, &env1),
        NodeHandle::start("c2", &cfg, &env2),
    )
    .unwrap();

    c1.run_tx_spammer(ANVIL_KEY_1);
    c2.run_tx_spammer(ANVIL_KEY_2);

    let pre_batches = chain
        .wait_for_batches(4, DEFAULT_TIMEOUT)
        .await
        .expect("pre-reorg: ≥4 combined batches");
    let (c1_pre_reorg_latest, c2_pre_reorg_latest) = tokio::try_join!(
        wait_for_latest_height(&c1, 1, DEFAULT_TIMEOUT),
        wait_for_latest_height(&c2, 1, DEFAULT_TIMEOUT),
    )
    .expect("pre-reorg: both composers should have produced L2 blocks");
    let post_reorg_target_height = c1_pre_reorg_latest.number.min(c2_pre_reorg_latest.number) + 1;
    let pre_reorg_states = chain.executed_states().await.unwrap();

    // Depth three crosses the bundle target and retreats at least one posted batch.
    harness.anvil.reorg(3).await.unwrap();
    send_l2_value_transfer_confirmed(
        &c1.l2_rpc_url(),
        ANVIL_KEY_3,
        ANVIL_ADDR_3,
        U256::from(1u64),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("post-reorg L2 tx did not land on c1");

    // Require post-reorg progress before accepting convergence.
    chain
        .wait_for_batches_or_node_failure(pre_batches + 1, &[&c1, &c2], DEFAULT_TIMEOUT)
        .await
        .expect("no batches landed after reorg");
    tokio::try_join!(
        wait_for_latest_height(&c1, post_reorg_target_height, DEFAULT_TIMEOUT),
        wait_for_latest_height(&c2, post_reorg_target_height, DEFAULT_TIMEOUT),
    )
    .expect("composers did not advance L2 height after reorg");

    let c1_safe = wait_for_new_attested_safe_block(&c1, &chain, &pre_reorg_states, DEFAULT_TIMEOUT);
    let c2_safe = wait_for_new_attested_safe_block(&c2, &chain, &pre_reorg_states, DEFAULT_TIMEOUT);
    tokio::pin!(c1_safe);
    tokio::pin!(c2_safe);
    let (source_name, peer_name, peer, post_reorg_safe_number, post_reorg_safe_hash) = tokio::select! {
        c1_result = &mut c1_safe => match c1_result {
            Ok((number, hash)) => ("c1", "c2", &c2, number, hash),
            Err(c1_err) => {
                let (number, hash) = c2_safe.await.unwrap_or_else(|c2_err| {
                    panic!(
                        "neither composer imported a newly attested post-reorg safe block; \
                         c1: {c1_err:#}; c2: {c2_err:#}"
                    );
                });
                ("c2", "c1", &c1, number, hash)
            }
        },
        c2_result = &mut c2_safe => match c2_result {
            Ok((number, hash)) => ("c2", "c1", &c1, number, hash),
            Err(c2_err) => {
                let (number, hash) = c1_safe.await.unwrap_or_else(|c1_err| {
                    panic!(
                        "neither composer imported a newly attested post-reorg safe block; \
                         c1: {c1_err:#}; c2: {c2_err:#}"
                    );
                });
                ("c1", "c2", &c2, number, hash)
            }
        },
    };
    wait_for_safe_chain_contains(
        peer,
        post_reorg_safe_number,
        post_reorg_safe_hash,
        DEFAULT_TIMEOUT,
    )
    .await
    .unwrap_or_else(|err| {
        panic!("{peer_name} did not import {source_name}'s post-reorg safe block: {err:#}");
    });

    wait_for_safe_prefix_convergence(&[&c1, &c2], post_reorg_target_height, DEFAULT_TIMEOUT)
        .await
        .expect("composers did not converge on post-reorg safe block hashes");

    c1.wait_for_reorg_seen(DEFAULT_TIMEOUT).await.unwrap();
    c2.wait_for_reorg_seen(DEFAULT_TIMEOUT).await.unwrap();

    c1.assert_no_process_death();
    c2.assert_no_process_death();
}

/// Three composers racing on one L1, with no reorg or injected fault. Each
/// losing batch is redundant, and every composer must keep converging on block
/// hashes actually ratified by L1 instead of wedging after an overlap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_composer_steady_state_stays_converged_with_l1() {
    const COMPOSERS: usize = 3;
    const SETTLEMENTS: usize = 24;

    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let cfg = NodeConfig {
        genesis_path: Some(harness.l2_genesis_path()),
        ..Default::default()
    };

    // Distinct posters prevent L1 account nonces from serializing the race.
    let poster_keys = [ANVIL_KEY, ANVIL_KEY_4, ANVIL_KEY_6];
    let spam_keys = [ANVIL_KEY_1, ANVIL_KEY_2, ANVIL_KEY_3];
    let mut nodes = Vec::with_capacity(COMPOSERS);
    for (index, poster) in poster_keys.iter().take(COMPOSERS).enumerate() {
        let env = harness.env_for(poster, true).await.unwrap();
        let name: &'static str = ["mc1", "mc2", "mc3"][index];
        nodes.push((name, NodeHandle::start(name, &cfg, &env).await.unwrap()));
    }
    for ((_, node), key) in nodes.iter().zip(spam_keys) {
        node.run_tx_spammer(key);
    }

    // Reading L1's current commitment would race with the next settlement.
    // Instead, every observed safe block hash must appear in L1's complete
    // execution history, while safe heights must continue advancing.
    let mut last_safe = vec![0_u64; nodes.len()];
    let mut advanced = 0_usize;
    let mut compared = 0_usize;
    for settled in 1..=SETTLEMENTS {
        chain
            .wait_for_batches(settled, DEFAULT_TIMEOUT)
            .await
            .unwrap_or_else(|error| {
                panic!("stalled at settlement {settled} of {SETTLEMENTS}: {error:#}")
            });

        let recorded = chain.executed_states().await.unwrap();
        for (index, (name, node)) in nodes.iter().enumerate() {
            node.assert_no_process_death();
            let Some((number, _)) =
                block_number_and_hash_at(&node.l2_rpc_url(), BlockNumberOrTag::Safe)
                    .await
                    .unwrap_or_else(|error| {
                        panic!("{name}: safe block unreadable at {settled}: {error:#}")
                    })
            else {
                continue;
            };
            if number == 0 {
                continue;
            }
            let safe_hash = safe_block_hash(&node.l2_rpc_url())
                .await
                .unwrap()
                .unwrap_or_default();
            compared += 1;
            assert!(
                recorded.contains(&safe_hash),
                "{name} safe block hash {safe_hash} at L2 height {number} was never recorded by \
                 L1 (settlement {settled}); the composer derived a chain L1 did not ratify",
            );
            if number > last_safe[index] {
                advanced += 1;
                last_safe[index] = number;
            }
        }
    }

    eprintln!(
        "multi-composer: block-hash comparisons={compared} l1_recorded_hashes={} \
         final_safe_heights={last_safe:?}",
        chain.executed_states().await.unwrap().len(),
    );

    for (index, (name, _)) in nodes.iter().enumerate() {
        assert!(
            last_safe[index] > 0,
            "{name} never attested a safe block — it never caught up to L1",
        );
    }
    assert!(
        advanced >= SETTLEMENTS,
        "safe heads advanced only {advanced} times across {SETTLEMENTS} settlements and \
         {COMPOSERS} composers — at least one composer stopped tracking L1",
    );
    let floor = (SETTLEMENTS - 3) * COMPOSERS;
    assert!(
        compared >= floor,
        "only {compared} block-hash comparisons (floor {floor}) across {SETTLEMENTS} \
         settlements × {COMPOSERS} composers — the soundness check was mostly skipped",
    );
}

async fn spawn_follower(
    name: &str,
    harness: &Harness,
    seq_rpc: Option<&str>,
) -> anyhow::Result<NodeHandle> {
    let env = harness.follower_env(seq_rpc).await?;
    let cfg = NodeConfig {
        binary: NodeBinary::Follower,
        ..Default::default()
    };
    NodeHandle::start(name, &cfg, &env).await
}

/// An L1-only follower reconstructs an attested safe state without a sequencer RPC.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn happy_case_follower_l1_derived() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let seq = NodeHandle::start("seq", &NodeConfig::default(), &harness.env().await.unwrap())
        .await
        .unwrap();
    chain
        .wait_for_batches(2, DEFAULT_TIMEOUT)
        .await
        .expect("sequencer landed batches");

    let follower = spawn_follower("follower", &harness, None).await.unwrap();
    wait_for_safe_state(
        &follower,
        &chain,
        harness.l2_genesis_block_hash().unwrap(),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("follower did not catch up via L1 replay");
    wait_for_safe_prefix_convergence(&[&seq, &follower], 1, DEFAULT_TIMEOUT)
        .await
        .expect("follower safe chain did not converge with the sequencer");

    follower.assert_no_process_death();
    seq.assert_no_process_death();
}

/// Sequencer RPC advances unsafe state while L1 remains safe-authoritative.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn happy_case_follower_sequencer_rpc() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let seq = NodeHandle::start("seq", &NodeConfig::default(), &harness.env().await.unwrap())
        .await
        .unwrap();

    let seq_rpc = seq.l2_rpc_url();
    let follower_env = override_env(
        harness.follower_env(Some(&seq_rpc)).await.unwrap(),
        "RUST_LOG",
        "warn,eez_follower::unsafe_head=info",
    );
    let follower_cfg = NodeConfig {
        binary: NodeBinary::Follower,
        ..Default::default()
    };
    let follower = NodeHandle::start("follower", &follower_cfg, &follower_env)
        .await
        .unwrap();

    chain
        .wait_for_batches(2, DEFAULT_TIMEOUT)
        .await
        .expect("sequencer landed batches");
    wait_for_safe_state(
        &follower,
        &chain,
        harness.l2_genesis_block_hash().unwrap(),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("follower did not catch up via L1 replay");

    wait_for_safe_prefix_convergence(&[&seq, &follower], 1, DEFAULT_TIMEOUT)
        .await
        .expect("follower safe block never matched the sequencer chain");

    let follower_head_signals = [
        signals::FOLLOWER_HEAD_ADVANCED,
        signals::FOLLOWER_HEAD_SYNCING,
    ];
    let unsafe_head_events_before = follower.count_signals(&follower_head_signals).unwrap();
    seq.run_tx_spammer(ANVIL_KEY_1);
    wait_for(DEFAULT_TIMEOUT, || {
        std::future::ready(
            follower
                .count_signals(&follower_head_signals)
                .map(|n| (n > unsafe_head_events_before).then_some(())),
        )
    })
    .await
    .expect("follower never reported a sequencer-RPC unsafe-head FCU outcome");

    follower.assert_no_process_death();
    seq.assert_no_process_death();
}

/// An L1-only follower retreats, replays the replacement suffix, and converges
/// with the composer after an L1 reorg.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn happy_case_follower_l1_reorg_recovers() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let genesis = harness.l2_genesis_path();
    let seq_cfg = NodeConfig {
        genesis_path: Some(genesis),
        ..Default::default()
    };
    let follower_cfg = NodeConfig {
        binary: NodeBinary::Follower,
        genesis_path: Some(genesis),
    };
    let seq_env = harness.env().await.unwrap();
    let follower_env = harness.follower_env(None).await.unwrap();
    let (seq, follower) = tokio::try_join!(
        NodeHandle::start("seq", &seq_cfg, &seq_env),
        NodeHandle::start("follower", &follower_cfg, &follower_env),
    )
    .unwrap();
    seq.run_tx_spammer(ANVIL_KEY_1);

    let pre_batches = chain
        .wait_for_batches(2, DEFAULT_TIMEOUT)
        .await
        .expect("pre-reorg batches");
    let pre_reorg_states = chain.executed_states().await.unwrap();
    harness.anvil.reorg(3).await.unwrap();
    send_l2_value_transfer_confirmed(
        &seq.l2_rpc_url(),
        ANVIL_KEY_3,
        ANVIL_ADDR_3,
        U256::from(1u64),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("post-reorg L2 tx did not land on sequencer");
    chain
        .wait_for_batches_or_node_failure(pre_batches + 1, &[&seq, &follower], DEFAULT_TIMEOUT)
        .await
        .expect("no batches landed after reorg");
    let (post_reorg_safe_number, _) =
        wait_for_new_attested_safe_block(&seq, &chain, &pre_reorg_states, DEFAULT_TIMEOUT)
            .await
            .expect("sequencer did not import the post-reorg safe block");
    wait_for_safe_prefix_convergence(&[&seq, &follower], post_reorg_safe_number, DEFAULT_TIMEOUT)
        .await
        .expect("follower did not converge on the composer's replacement safe suffix");

    follower.wait_for_reorg_seen(DEFAULT_TIMEOUT).await.unwrap();
    follower.assert_no_process_death();
    seq.assert_no_process_death();
}

async fn settlement_by_l2(
    node: &NodeHandle,
    selector: serde_json::Value,
) -> anyhow::Result<Option<serde_json::Value>> {
    let provider = ProviderBuilder::new().connect_http(node.l2_rpc_url().parse()?);
    Ok(provider
        .client()
        .request("eez_getSettlementByL2Block", (selector,))
        .await?)
}

async fn settled_ranges_by_l1(
    node: &NodeHandle,
    selector: serde_json::Value,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let provider = ProviderBuilder::new().connect_http(node.l2_rpc_url().parse()?);
    Ok(provider
        .client()
        .request("eez_getSettledL2RangesByL1Block", (selector,))
        .await?)
}

fn correlation_hash(record: &serde_json::Value, field: &str) -> B256 {
    record[field]
        .as_str()
        .unwrap_or_else(|| panic!("correlation field {field} is not a hash"))
        .parse()
        .unwrap_or_else(|error| panic!("invalid correlation {field}: {error}"))
}

fn correlation_quantity(record: &serde_json::Value, field: &str) -> u64 {
    let raw = record[field]
        .as_str()
        .unwrap_or_else(|| panic!("correlation field {field} is not a quantity"));
    u64::from_str_radix(raw.strip_prefix("0x").unwrap_or(raw), 16)
        .unwrap_or_else(|error| panic!("invalid correlation {field}: {error}"))
}

async fn assert_blob_settlement_transaction(
    l1_rpc: &str,
    transaction_hash: B256,
) -> anyhow::Result<()> {
    let provider = ProviderBuilder::new().connect_http(l1_rpc.parse()?);
    let transaction: Option<serde_json::Value> = provider
        .client()
        .request(
            "eth_getTransactionByHash",
            (format!("{transaction_hash:#x}"),),
        )
        .await?;
    let transaction = transaction
        .unwrap_or_else(|| panic!("blob settlement transaction {transaction_hash} is missing"));
    assert_eq!(
        transaction["type"], "0x3",
        "settlement {transaction_hash} is not EIP-4844"
    );
    assert!(
        transaction["blobVersionedHashes"]
            .as_array()
            .is_some_and(|hashes| !hashes.is_empty()),
        "settlement {transaction_hash} has no blob versioned hashes"
    );
    Ok(())
}

/// A real EIP-4844 settlement is orphaned on L1 and replaced on the new fork.
/// The old L1-hash index must disappear, the same L2 height must be remapped to
/// a canonical blob transaction, and both correlation directions must agree.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn happy_case_blob_l1_reorg_replaces_correlations() {
    let harness = Harness::for_reorg().await.unwrap();
    let chain = harness.chain();
    let genesis = harness.l2_genesis_path();
    let cfg = NodeConfig {
        genesis_path: Some(genesis),
        ..Default::default()
    };
    let env = override_env(
        harness.blob_env().await.unwrap(),
        "RUST_LOG",
        "warn,eez_deriver=info,eez_l1=info",
    );
    let seq = NodeHandle::start("blob-reorg", &cfg, &env).await.unwrap();

    send_l2_value_transfer_confirmed(
        &seq.l2_rpc_url(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(1u64),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("pre-reorg L2 transaction did not land");
    wait_for_safe_state(
        &seq,
        &chain,
        harness.l2_genesis_block_hash().unwrap(),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("pre-reorg blob settlement was not adopted as safe");

    // Leave the deployment/registration blocks below the common ancestor,
    // then select a newer blob settlement that is still inside the exact
    // three-block suffix we will replace.
    chain
        .wait_for_l1_blocks(4, DEFAULT_TIMEOUT)
        .await
        .expect("L1 did not advance beyond the deployment window");
    let (old_l2_number, old_l2_hash, pre, old_l1_tip) = wait_for(DEFAULT_TIMEOUT, || async {
        let Some((l2_number, l2_hash)) =
            block_number_and_hash_at(&seq.l2_rpc_url(), BlockNumberOrTag::Safe).await?
        else {
            return Ok(None);
        };
        let Some(record) =
            settlement_by_l2(&seq, serde_json::json!(format!("{l2_hash:#x}"))).await?
        else {
            return Ok(None);
        };
        let tip = chain.block_number().await?;
        let settlement_block = correlation_quantity(&record, "l1BlockNumber");
        let common_ancestor = tip.saturating_sub(3);
        let registration_block = harness.dep.deploy_block + 3;
        Ok((record["canonicalL2"] == true
            && settlement_block > common_ancestor
            && common_ancestor >= registration_block)
            .then_some((l2_number, l2_hash, record, tip)))
    })
    .await
    .expect("no blob settlement entered a safe depth-3 reorg window");
    assert_eq!(
        correlation_hash(&pre["matchedL2Block"], "hash"),
        old_l2_hash,
        "pre-reorg correlation resolved a different L2 block"
    );
    let old_l1_number = correlation_quantity(&pre, "l1BlockNumber");
    let old_l1_hash = correlation_hash(&pre, "l1BlockHash");
    let old_l1_transaction = correlation_hash(&pre, "l1TransactionHash");
    assert_blob_settlement_transaction(&harness.anvil.rpc_url, old_l1_transaction)
        .await
        .expect("pre-reorg settlement was not a canonical blob transaction");
    let old_reverse = settled_ranges_by_l1(&seq, serde_json::json!(format!("{old_l1_hash:#x}")))
        .await
        .expect("pre-reorg reverse correlation RPC failed");
    assert!(
        old_reverse
            .iter()
            .any(|record| { correlation_hash(record, "l1TransactionHash") == old_l1_transaction }),
        "pre-reorg L1->L2 correlation is missing the blob settlement"
    );

    assert!(
        old_l1_number > old_l1_tip.saturating_sub(3),
        "latest safe blob settlement at L1 block {old_l1_number} would survive a depth-3 reorg from tip {old_l1_tip}"
    );
    let pre_reorg_states = chain.executed_states().await.unwrap();

    harness.anvil.reorg(3).await.unwrap();
    send_l2_value_transfer_confirmed(
        &seq.l2_rpc_url(),
        ANVIL_KEY_3,
        ANVIL_ADDR_3,
        U256::from(2u64),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("post-reorg L2 transaction did not land");
    let (new_safe_number, new_safe_hash) =
        wait_for_new_attested_safe_block(&seq, &chain, &pre_reorg_states, DEFAULT_TIMEOUT)
            .await
            .expect("no newly attested safe block was imported after the blob reorg");

    // The exact old L2 height must be backed by a settlement on the replacement
    // L1 fork. This prevents later-only progress from masking a gap at the
    // rolled-out height.
    let replacement = wait_for(DEFAULT_TIMEOUT, || async {
        Ok(settlement_by_l2(&seq, serde_json::json!(old_l2_number))
            .await?
            .filter(|record| {
                record["canonicalL2"] == true
                    && correlation_hash(record, "l1BlockHash") != old_l1_hash
            }))
    })
    .await
    .expect("old L2 height was not remapped to the replacement L1 fork");
    assert_eq!(
        correlation_quantity(&replacement["matchedL2Block"], "number"),
        old_l2_number
    );
    let replacement_l2_hash = correlation_hash(&replacement["matchedL2Block"], "hash");
    let replacement_l1_hash = correlation_hash(&replacement, "l1BlockHash");
    let replacement_l1_transaction = correlation_hash(&replacement, "l1TransactionHash");
    assert_ne!(replacement_l1_hash, old_l1_hash);
    assert_blob_settlement_transaction(&harness.anvil.rpc_url, replacement_l1_transaction)
        .await
        .expect("replacement settlement was not a canonical blob transaction");

    let orphan_removed = wait_for(DEFAULT_TIMEOUT, || async {
        let records =
            settled_ranges_by_l1(&seq, serde_json::json!(format!("{old_l1_hash:#x}"))).await?;
        Ok(records.is_empty().then_some(()))
    })
    .await;
    orphan_removed.expect("orphaned L1 block remained in the reverse correlation index");

    let replacement_reverse =
        settled_ranges_by_l1(&seq, serde_json::json!(format!("{replacement_l1_hash:#x}")))
            .await
            .expect("replacement reverse correlation RPC failed");
    assert!(
        replacement_reverse.iter().any(|record| {
            correlation_hash(record, "l1TransactionHash") == replacement_l1_transaction
                && record["canonicalL2"] == true
        }),
        "replacement L1->L2 correlation is absent or noncanonical"
    );

    if replacement_l2_hash != old_l2_hash {
        let stale = settlement_by_l2(&seq, serde_json::json!(format!("{old_l2_hash:#x}")))
            .await
            .expect("old L2-hash correlation query failed");
        assert!(stale.is_none(), "orphaned L2 hash remained indexed");
    }

    let l1_provider = ProviderBuilder::new().connect_http(harness.anvil.rpc_url.parse().unwrap());
    let canonical_at_old_height = l1_provider
        .get_block_by_number(BlockNumberOrTag::Number(old_l1_number))
        .await
        .unwrap()
        .expect("replacement L1 block is missing at the orphaned height");
    assert_ne!(
        canonical_at_old_height.header.hash, old_l1_hash,
        "anvil did not produce an alternative L1 fork"
    );
    if let Some(old_receipt) = l1_provider
        .get_transaction_receipt(old_l1_transaction)
        .await
        .unwrap()
    {
        assert_ne!(
            old_receipt.block_hash,
            Some(old_l1_hash),
            "orphaned blob receipt still names the old L1 block"
        );
    }

    let new_safe = settlement_by_l2(&seq, serde_json::json!(format!("{new_safe_hash:#x}")))
        .await
        .expect("new safe correlation RPC failed")
        .expect("newly attested safe block has no correlation");
    assert_eq!(
        correlation_quantity(&new_safe["matchedL2Block"], "number"),
        new_safe_number
    );
    assert_eq!(
        correlation_hash(&new_safe["matchedL2Block"], "hash"),
        new_safe_hash
    );
    assert_eq!(new_safe["canonicalL2"], true);

    seq.wait_for_reorg_seen(DEFAULT_TIMEOUT).await.unwrap();
    seq.assert_no_divergence_failure_logs();
}

/// Followers with different unsafe sources converge on the same safe prefix.
/// This proves L1 derivation, rather than the unsafe source, controls safe-head selection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn happy_case_follower_cross_safe_parity() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let seq = NodeHandle::start("seq", &NodeConfig::default(), &harness.env().await.unwrap())
        .await
        .unwrap();
    chain
        .wait_for_batches(2, DEFAULT_TIMEOUT)
        .await
        .expect("sequencer landed batches");

    let seq_rpc = seq.l2_rpc_url();
    let (f_l1, f_seq) = tokio::try_join!(
        spawn_follower("f_l1", &harness, None),
        spawn_follower("f_seq", &harness, Some(&seq_rpc)),
    )
    .unwrap();
    wait_for_safe_state(
        &f_l1,
        &chain,
        harness.l2_genesis_block_hash().unwrap(),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("f_l1 did not catch up");
    wait_for_safe_state(
        &f_seq,
        &chain,
        harness.l2_genesis_block_hash().unwrap(),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("f_seq did not catch up");

    wait_for_safe_prefix_convergence(&[&seq, &f_l1, &f_seq], 1, DEFAULT_TIMEOUT)
        .await
        .expect("followers never shared a sequencer safe block");

    f_l1.assert_no_process_death();
    f_seq.assert_no_process_death();
    seq.assert_no_process_death();
}

/// A rogue unsafe source cannot move the follower's L1-derived safe head.
/// A structured signal proves the follower polled the rogue source.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn happy_case_follower_rogue_sequencer_safe_head_holds() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();
    let genesis = harness.l2_genesis_path();

    let seq_cfg = NodeConfig {
        genesis_path: Some(genesis),
        ..Default::default()
    };
    let seq = NodeHandle::start("seq", &seq_cfg, &harness.env().await.unwrap())
        .await
        .unwrap();
    seq.run_tx_spammer(ANVIL_KEY_1);

    // The already-running L1 Anvil is an incompatible Ethereum chain and is
    // sufficient to exercise unsafe-head rejection without another L2 node.
    let rogue_rpc = harness.anvil.rpc_url.clone();
    let follower_env = override_env(
        harness.follower_env(Some(&rogue_rpc)).await.unwrap(),
        "RUST_LOG",
        "warn,eez_follower::unsafe_head=info",
    );
    let follower_cfg = NodeConfig {
        binary: NodeBinary::Follower,
        genesis_path: Some(genesis),
    };
    let follower = NodeHandle::start("follower", &follower_cfg, &follower_env)
        .await
        .unwrap();

    wait_for_safe_state(
        &follower,
        &chain,
        harness.l2_genesis_block_hash().unwrap(),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("follower safe head did not reach a non-genesis attested stateRoot while on the rogue");

    // Stop canonical batch production and let any already-submitted batch land
    // before fixing the safe anchor used by this assertion.
    drop(seq);
    chain
        .wait_for_l1_blocks(1, Duration::from_secs(15))
        .await
        .expect("L1 did not flush after stopping the composer");
    wait_for_safe_state(
        &follower,
        &chain,
        harness.l2_genesis_block_hash().unwrap(),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("follower did not derive the composer's final landed batch");
    let safe_before = block_number_and_hash_at(&follower.l2_rpc_url(), BlockNumberOrTag::Safe)
        .await
        .unwrap()
        .expect("follower has a safe head");

    // The rogue RPC is Anvil. Force its unrelated block number above the L2
    // safe height, making its ancestry locally unknown rather than a
    // timing-dependent equal-height/below-safe conflict.
    while chain.block_number().await.unwrap() <= safe_before.0 {
        chain.mine().await.unwrap();
    }
    let syncing_pattern = ["reth accepted sequencer head as a sync target"];
    let syncing_before = follower.log_count_matching(&syncing_pattern).unwrap();
    wait_for(DEFAULT_TIMEOUT, || {
        std::future::ready(
            follower
                .log_count_matching(&syncing_pattern)
                .map(|count| (count > syncing_before).then_some(())),
        )
    })
    .await
    .expect("follower did not offer the unknown rogue head as a sync target");

    let safe_after = block_number_and_hash_at(&follower.l2_rpc_url(), BlockNumberOrTag::Safe)
        .await
        .unwrap();
    assert_eq!(
        safe_after,
        Some(safe_before),
        "processing the rogue sync target must not move the L1-derived safe head",
    );

    follower.assert_no_process_death();
}

/// A late follower backfills the complete pre-existing batch history.
/// Matching a block captured before startup rules out a partial catch-up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn happy_case_follower_deep_backfill_late_join() {
    let harness = Harness::fresh().await.unwrap();
    let chain = harness.chain();

    let seq = NodeHandle::start("seq", &NodeConfig::default(), &harness.env().await.unwrap())
        .await
        .unwrap();
    seq.run_tx_spammer(ANVIL_KEY_1);

    chain
        .wait_for_batches(4, DEFAULT_TIMEOUT)
        .await
        .expect("sequencer did not build a deep backlog");

    let (backlog_depth, backlog_hash) =
        block_number_and_hash_at(&seq.l2_rpc_url(), BlockNumberOrTag::Safe)
            .await
            .unwrap()
            .expect("sequencer has a safe block");

    let follower = spawn_follower("follower", &harness, None).await.unwrap();

    wait_for_safe_state(
        &follower,
        &chain,
        harness.l2_genesis_block_hash().unwrap(),
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("late-joining follower did not backfill into an attested stateRoot");

    wait_for_safe_chain_contains(&follower, backlog_depth, backlog_hash, DEFAULT_TIMEOUT)
        .await
        .unwrap_or_else(|_| {
            panic!("follower did not replay full backlog to block {backlog_depth}")
        });

    follower.assert_no_process_death();
    seq.assert_no_process_death();
}
