//! Receipt-backed checks of per-source transaction references, independent of
//! the manifest's boundary calculation. Shared by nested and multi-user cases.

use alloy_network::{AnyNetwork, ReceiptResponse};
use alloy_primitives::{B256, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::SolCall;
use eez_testkit::{
    CrossChainWorld, DEV_CHAIN_ID, INBOUND_USER, IOverlayDriverL1, IOverlayProbeL2, OUTBOUND_USER,
    SETTLE_TIMEOUT, l2_value, onchain_nonce, setup_cross_chain_overlay_round_trip, sign_and_send,
    wait_for,
};
use serde_json::{Value, json};

pub(crate) async fn committed_references(world: &CrossChainWorld, source: B256) -> Value {
    let front = ProviderBuilder::new().connect_http(world.l2_xchain().parse().unwrap());
    wait_for(SETTLE_TIMEOUT, || async {
        let status: Value = front
            .client()
            .request("eez_getCrossChainTransaction", (source,))
            .await?;
        if status["ownership"]["reason"] == "Committed" {
            assert!(
                status["transactions"].is_object(),
                "missing transaction references: {status}"
            );
        }
        Ok(
            (status["ownership"]["reason"] == "Committed")
                .then_some(status["transactions"].clone()),
        )
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "references did not settle: {error:#}{}",
            world.settlement_diagnostics()
        )
    })
}

/// Compare against independently identified physical transactions, then verify
/// every advertised identity has a successful canonical receipt on its chain.
pub(crate) async fn assert_references(
    world: &CrossChainWorld,
    source: B256,
    inbound: bool,
    expected_l2: &[B256],
) -> Value {
    let references = committed_references(world, source).await;
    assert_eq!(references["basis"], "attempt");
    assert_eq!(references["complete"], true);
    assert!(
        references["attemptGeneration"]
            .as_str()
            .unwrap()
            .starts_with("0x")
    );
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new()
        .network::<AnyNetwork>()
        .connect_http(world.l2_rpc().parse().unwrap());
    let l1_id = format!("{:#x}", l1.get_chain_id().await.unwrap());
    assert_eq!(references["postBatch"]["chainId"], l1_id);
    let post: B256 = references["postBatch"]["hash"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        references["l1"],
        json!({"chainId": l1_id, "hash": if inbound {source} else {post}})
    );
    assert_eq!(
        references["l2"],
        json!(
            expected_l2
                .iter()
                .map(|hash| json!({
                    "chainId":format!("{:#x}",world.l2_chain_id), "hash":hash
                }))
                .collect::<Vec<_>>()
        )
    );
    for hash in [post, if inbound { source } else { post }] {
        let receipt = l1
            .get_transaction_receipt(hash)
            .await
            .unwrap()
            .expect("L1 reference receipt");
        assert!(receipt.status());
        let block = l1
            .get_block_by_number(receipt.block_number.unwrap().into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(Some(block.header.hash), receipt.block_hash);
    }
    for hash in expected_l2 {
        let receipt = l2
            .get_transaction_receipt(*hash)
            .await
            .unwrap()
            .expect("L2 reference receipt");
        assert!(receipt.status());
        let block = l2
            .get_block_by_number(receipt.block_number.unwrap().into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(Some(block.header.hash), receipt.block_hash);
    }
    let other: Value = ProviderBuilder::new()
        .connect_http(world.l1_xchain().parse().unwrap())
        .client()
        .request("eez_getCrossChainTransaction", (source,))
        .await
        .unwrap();
    assert_eq!(
        other["transactions"], references,
        "both fronts expose the same group"
    );
    references
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nested_inbound_references_include_both_deliveries() {
    nested_references(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nested_outbound_references_keep_inline_callbacks_in_the_source_transaction() {
    nested_references(false).await;
}

async fn nested_references(inbound: bool) {
    let world = setup_cross_chain_overlay_round_trip().await.unwrap();
    let (front, source_rpc, key, chain_id, target, data) = if inbound {
        (
            world.l1_xchain(),
            world.l1_rpc(),
            INBOUND_USER,
            DEV_CHAIN_ID,
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
            world.l2_rpc(),
            OUTBOUND_USER,
            world.l2_chain_id,
            world.probe,
            IOverlayProbeL2::startDeliverBurnThenPingCall {
                driver: world.driver_on_l2,
                probe: world.probe_on_l1,
                amount: U256::from(7),
            }
            .abi_encode(),
        )
    };
    let source = sign_and_send(
        &front,
        key,
        chain_id,
        onchain_nonce(&source_rpc, key).await.unwrap(),
        Some(target),
        U256::ZERO,
        data,
        3_000_000,
    )
    .await
    .unwrap();
    let references = committed_references(&world, source).await;
    let first: B256 = references["l2"][0]["hash"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let l2 = ProviderBuilder::new()
        .network::<AnyNetwork>()
        .connect_http(world.l2_rpc().parse().unwrap());
    let receipt = l2.get_transaction_receipt(first).await.unwrap().unwrap();
    let block = l2
        .get_block_by_hash(receipt.block_hash.unwrap())
        .await
        .unwrap()
        .unwrap();
    let hashes = block.transactions.hashes().collect::<Vec<_>>();
    // This fixture submits exactly one cross-chain source. Every transaction
    // in its Sync block belongs to it; omitted loads/deliveries must fail here.
    // Inbound produces two self-loading deliveries. This outbound fixture runs
    // its reentrant callbacks inside the source transaction: one load + source.
    // Those internal calls must not be advertised as extra transaction hashes.
    assert_eq!(hashes.len(), 2);
    if !inbound {
        assert_eq!(hashes[1], source);
        let completed = eez_testkit::read_state_word(
            &world.l2_rpc(),
            &eez_testkit::call_read(
                world.probe,
                "completed()",
                IOverlayProbeL2::completedCall {}.abi_encode(),
            ),
        )
        .await
        .unwrap();
        assert_eq!(completed, U256::from(1));
    }
    assert_references(&world, source, inbound, &hashes).await;
    assert_eq!(
        l2_value(&world.l2_rpc(), world.probe).await.unwrap(),
        U256::ZERO
    );
    let pings = eez_testkit::read_state_word(
        &world.l1_rpc(),
        &eez_testkit::call_read(
            world.driver,
            "pings()",
            IOverlayDriverL1::pingsCall {}.abi_encode(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(pings, U256::from(1));
}
