//! Shared stateless sessions under a real native L1 reorg (#206 adaptation).
//! This fork's posting fixture keeps simulation and submission on one L1.
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::BlockNumberOrTag;
use eez_testkit::*;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_stateless_signer_composers_recover_from_native_l1_reorg() {
    let policy = [
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "always".to_owned()),
        ("EEZ_COMPOSER_EXPECT_EXTERNAL_BATCHES", "true".to_owned()),
    ];
    let mut world = setup_cross_chain_with_l1_reorg(&policy).await.unwrap();
    assert!(
        world.proof_signer.streaming_attestations().unwrap() > 0,
        "setup never completed a v2 proof"
    );
    let mirror = native::spawn_composer(&world, world.fault_rpc_url(), &policy)
        .await
        .unwrap();
    let peer = &mirror.node;
    let timeout = Duration::from_mins(3);
    world.node.run_tx_spammer(ANVIL_KEY_1);
    peer.run_tx_spammer(ANVIL_KEY_2);
    wait_for(timeout, || async {
        Ok((peer
            .log_count_matching(&["eez.prover_client.attested"])
            .unwrap()
            >= 2)
            .then_some(()))
    })
    .await
    .unwrap();
    let before = wait_for_safe_prefix_convergence(&[&world.node, peer], 1, timeout)
        .await
        .unwrap();
    let proofs_a = world
        .node
        .log_count_matching(&["eez.prover_client.attested"])
        .unwrap();
    let proofs_b = peer
        .log_count_matching(&["eez.prover_client.attested"])
        .unwrap();
    let signed = world.proof_signer.streaming_attestations().unwrap();
    let reorg_signals = [
        signals::L1_REORG_DETECTED,
        signals::DERIVER_REORG_RETREATED,
        signals::DERIVER_REORG_NOOP,
    ];
    let reorgs_a = world.node.count_signals(&reorg_signals).unwrap();
    let reorgs_b = peer.count_signals(&reorg_signals).unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let head = l1
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    let ancestor = l1
        .get_block_by_number((head.header.number - 2).into())
        .await
        .unwrap()
        .unwrap();
    world
        .l1_reorg_control
        .as_mut()
        .unwrap()
        .reorg_to(ancestor.header.number, ancestor.header.hash)
        .await
        .unwrap();
    wait_for(timeout, || async {
        Ok(
            (world.node.count_signals(&reorg_signals).unwrap() > reorgs_a
                && peer.count_signals(&reorg_signals).unwrap() > reorgs_b)
                .then_some(()),
        )
    })
    .await
    .unwrap();
    wait_for(timeout, || async {
        Ok((world
            .node
            .log_count_matching(&["eez.prover_client.attested"])
            .unwrap()
            > proofs_a
            && peer
                .log_count_matching(&["eez.prover_client.attested"])
                .unwrap()
                > proofs_b)
            .then_some(()))
    })
    .await
    .unwrap();
    let after = wait_for_safe_prefix_convergence(&[&world.node, peer], before.number + 5, timeout)
        .await
        .unwrap();
    assert!(
        after.number > before.number
            && world.proof_signer.streaming_attestations().unwrap() > signed,
        "no canonical progress after reorg"
    );
    world.node.assert_no_process_death();
    mirror.assert_healthy();
    eprintln!(
        "native shared streaming reorg: safe {} -> {}, signer proofs {} -> {}",
        before.number,
        after.number,
        signed,
        world.proof_signer.streaming_attestations().unwrap()
    );
}
