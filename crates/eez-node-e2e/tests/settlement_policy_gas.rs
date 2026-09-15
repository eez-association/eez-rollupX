//! Real native DA and ECDSA coverage where execution gas binds before blobs.

use std::time::Duration;

use alloy_consensus::Transaction as _;
use alloy_primitives::{Address, B256, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{SolCall as _, SolEvent as _};
use eez_testkit::{
    ANVIL_KEY_1, ANVIL_KEY_2, ANVIL_KEY_4, CrossChainWorld, NativeL1Profile, NodeBinary,
    NodeConfig, NodeHandle, TARGET_DEPLOYER, batches_posted, create_l2_cross_chain_proxy,
    onchain_nonce, setup_cross_chain_with_profile, sign_and_send, wait_for, wait_for_latest_height,
    wait_for_safe_prefix_convergence,
};

const TIMEOUT: Duration = Duration::from_secs(180);

fn env() -> Vec<(&'static str, String)> {
    [
        ("EEZ_SETTLEMENT_POLICY_ENABLED", "true"),
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval"),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000"),
        ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10000"),
        ("EEZ_MAX_SPECULATIVE_DEPTH", "0"),
        ("EEZ_MAX_POSTBATCH_GAS", "5000000"),
        ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "100000000000"),
        ("EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS", "100000000000"),
        // Explicit ordering aid for the disposable native builder fixture.
        ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "1000000001"),
        ("RUST_LOG", "info"),
    ]
    .into_iter()
    .map(|(key, value)| (key, value.into()))
    .collect()
}

/// Every call has four bytes of input and performs `writes` fresh SSTOREs.
/// The straight-line fixture bytecode avoids compiler/gas-optimizer drift:
/// repeated PUSH1 1; PUSH2 slot; SSTORE, then returns the ABI word 1.
fn fresh_storage_writes(writes: u16) -> Vec<u8> {
    let mut runtime = Vec::new();
    for slot in 0..writes {
        runtime.extend([0x60, 1, 0x61]);
        runtime.extend(slot.to_be_bytes());
        runtime.push(0x55);
    }
    runtime
}

async fn expensive_target(world: &CrossChainWorld, writes: u16) -> (Address, Address) {
    let mut runtime = fresh_storage_writes(writes);
    runtime.extend([0x60, 1, 0x5f, 0x52, 0x60, 32, 0x5f, 0xf3]);
    let target = deploy_fixture_code(world, runtime).await;
    let proxy = create_l2_cross_chain_proxy(&world.l2_rpc(), TARGET_DEPLOYER, target, 0)
        .await
        .unwrap();
    (target, proxy)
}

async fn deploy_fixture_code(world: &CrossChainWorld, runtime: Vec<u8>) -> Address {
    let size = u16::try_from(runtime.len()).unwrap().to_be_bytes();
    let mut creation = vec![
        0x61, size[0], size[1], 0x80, 0x61, 0, 11, 0x5f, 0x39, 0x5f, 0xf3,
    ];
    creation.extend(runtime);
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let deploy = sign_and_send(
        &world.l1_rpc(),
        TARGET_DEPLOYER,
        l1.get_chain_id().await.unwrap(),
        onchain_nonce(&world.l1_rpc(), TARGET_DEPLOYER)
            .await
            .unwrap(),
        None,
        U256::ZERO,
        creation,
        6_000_000,
    )
    .await
    .unwrap();
    let receipt = wait_for(TIMEOUT, || async {
        Ok(l1.get_transaction_receipt(deploy).await?)
    })
    .await
    .unwrap();
    assert!(receipt.status());
    receipt.contract_address.unwrap()
}

async fn quiet_sync(world: &CrossChainWorld) {
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
}

async fn outbound(world: &CrossChainWorld, key: &str, proxy: Address) -> B256 {
    sign_and_send(
        &world.l2_xchain(),
        key,
        world.l2_chain_id,
        onchain_nonce(&world.l2_rpc(), key).await.unwrap(),
        Some(proxy),
        U256::ZERO,
        vec![0x12, 0x34, 0x56, 0x78],
        600_000,
    )
    .await
    .unwrap()
}

async fn assert_gas_first(profile: NativeL1Profile) {
    let world = setup_cross_chain_with_profile(profile, &env())
        .await
        .unwrap();
    let (target_a, proxy_a) = expensive_target(&world, 100).await;
    let (target_b, proxy_b) = expensive_target(&world, 100).await;
    let (oversized, oversized_proxy) = expensive_target(&world, 180).await;
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let follower = NodeHandle::start(
        "policy-gas-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    quiet_sync(&world).await;
    let cursor = world.node.signal_cursor().unwrap();
    let (first, second) = tokio::join!(
        outbound(&world, ANVIL_KEY_2, proxy_a),
        outbound(&world, ANVIL_KEY_4, proxy_b),
    );
    let mut covered = 0;
    for hash in [first, second] {
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l2.get_transaction_receipt(hash).await?)
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "gas-limited user missing: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
        assert!(receipt.status());
        covered = covered.max(receipt.block_number.unwrap());
    }
    wait_for_safe_prefix_convergence(&[&world.node, &follower], covered, TIMEOUT)
        .await
        .unwrap();
    for target in [target_a, target_b] {
        assert_eq!(
            l1.get_storage_at(target, U256::from(99)).await.unwrap(),
            U256::from(1)
        );
    }
    let events = world.node.signals_since(cursor).unwrap();
    assert!(
        events.iter().any(
            |event| event.name == "eez.composer.sync_slot.gas_budget_cut"
                && event.fields["accepted_count"] == 1
                && event.fields["constraint"] == "l1_tx_gas_budget"
        ),
        "must defer the second whole group: {}",
        world.settlement_diagnostics()
    );
    let dispatches: Vec<_> = events
        .iter()
        .filter(|event| event.name == "eez.composer.bundle.dispatched")
        .collect();
    assert_eq!(dispatches.len(), 2);
    for event in dispatches {
        assert_eq!(
            event.fields["attempt_counts"], "[0]",
            "capacity deferral is not a failure"
        );
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
    assert_eq!(logs.len(), 2);
    for log in logs {
        let hash = log.transaction_hash.unwrap();
        let receipt = l1.get_transaction_receipt(hash).await.unwrap().unwrap();
        let tx = l1.get_transaction_by_hash(hash).await.unwrap().unwrap();
        assert!(receipt.status());
        assert!(
            receipt.gas_used > 2_000_000,
            "actual expensive target execution"
        );
        assert!(tx.inner.gas_limit() <= 5_000_000);
        assert_eq!(
            tx.inner.blob_versioned_hashes().unwrap().len(),
            1,
            "gas binds with little DA"
        );
    }

    // No prefix can repair one target that alone exceeds the operator budget.
    let cursor = world.node.signal_cursor().unwrap();
    let rejected = outbound(&world, ANVIL_KEY_1, oversized_proxy).await;
    wait_for(TIMEOUT, || async {
        Ok(world
            .node
            .signals_since(cursor)?
            .iter()
            .any(|event| {
                event.name == "eez.composer.cc_compose.poison_eviction_completed"
                    && event.fields["tx_hash"] == rejected.to_string()
            })
            .then_some(()))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "oversized unit not reported: {error:#}{}",
            world.settlement_diagnostics()
        )
    });
    let height = l2.get_block_number().await.unwrap();
    wait_for_latest_height(&world.node, height + 12, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        2
    );
    assert!(
        l2.get_transaction_receipt(rejected)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        l1.get_storage_at(oversized, U256::from(179)).await.unwrap(),
        U256::ZERO
    );
    assert!(
        !world
            .node
            .signals_since(cursor)
            .unwrap()
            .iter()
            .any(|event| { event.name == "eez.composer.emission.historical_chunk" }),
        "an oversized atomic call must not manufacture historical urgency"
    );
    world.node.assert_no_divergence_failure_logs();
    follower.assert_no_divergence_failure_logs();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_low_byte_expensive_new_proxies_split_only_complete_groups() {
    assert_gas_first(NativeL1Profile::EthereumOsaka).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_low_byte_expensive_new_proxies_split_only_complete_groups() {
    assert_gas_first(NativeL1Profile::GnosisPrague).await;
}

/// A low-byte L1 source performs expensive local storage writes before calling
/// the existing inbound proxy. Each source fits individually; two exceed the
/// actual block gas budget even though postBatch itself is cheap.
async fn expensive_inbound_source(world: &CrossChainWorld, value: u64) -> Address {
    let mut code = fresh_storage_writes(710);
    let calldata = eez_testkit::IValue::setValueCall {
        v: U256::from(value),
    }
    .abi_encode();
    code.push(0x7f); // PUSH32 selector and the first 28 argument bytes.
    code.extend_from_slice(&calldata[..32]);
    code.extend([0x5f, 0x52]); // MSTORE(0).
    code.push(0x7f);
    code.extend(U256::from(value).to_be_bytes::<32>());
    code.extend([0x60, 4, 0x52]); // MSTORE(4), completing the ABI argument.
    code.extend([0x60, 32, 0x5f, 0x60, 36, 0x5f, 0x5f, 0x73]);
    code.extend(world.setter_proxy.as_slice());
    code.extend([0x5a, 0xf1]); // CALL(gas(), proxy, 0, 0, 36, 0, 32).
    // Propagate failure instead of accepting an outer success after a bad call.
    code.push(0x15); // ISZERO.
    code.push(0x61);
    let label_at = code.len();
    code.extend([0, 0]);
    code.push(0x57); // JUMPI.
    code.extend([0x60, 32, 0x5f, 0xf3]);
    let label = u16::try_from(code.len()).unwrap();
    code[label_at..label_at + 2].copy_from_slice(&label.to_be_bytes());
    code.extend([0x5b, 0x5f, 0x5f, 0xfd]); // JUMPDEST; REVERT(0, 0).
    deploy_fixture_code(world, code).await
}

async fn assert_companion_gas_first(profile: NativeL1Profile) {
    let world = setup_cross_chain_with_profile(profile, &env())
        .await
        .unwrap();
    let first = expensive_inbound_source(&world, 41).await;
    let second = expensive_inbound_source(&world, 42).await;
    let follower = NodeHandle::start(
        "policy-companion-gas-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let chain_id = l1.get_chain_id().await.unwrap();
    quiet_sync(&world).await;
    let cursor = world.node.signal_cursor().unwrap();
    let mut users = Vec::new();
    for (key, target) in [(ANVIL_KEY_2, first), (ANVIL_KEY_4, second)] {
        users.push(
            sign_and_send(
                &world.l1_xchain(),
                key,
                chain_id,
                onchain_nonce(&world.l1_rpc(), key).await.unwrap(),
                Some(target),
                U256::ZERO,
                vec![0x12, 0x34, 0x56, 0x78],
                16_600_000,
            )
            .await
            .unwrap(),
        );
    }
    let mut total_source_gas = 0;
    let mut largest_block = 0;
    for hash in users {
        let receipt = wait_for(TIMEOUT, || async {
            Ok(l1.get_transaction_receipt(hash).await?)
        })
        .await
        .unwrap_or_else(|error| {
            panic!(
                "companion missing: {error:#}{}",
                world.settlement_diagnostics()
            )
        });
        assert!(receipt.status());
        assert!(receipt.gas_used > 15_000_000);
        total_source_gas += receipt.gas_used;
        largest_block = largest_block.max(
            l1.get_block_by_hash(receipt.block_hash.unwrap())
                .await
                .unwrap()
                .unwrap()
                .header
                .gas_limit,
        );
    }
    assert!(
        total_source_gas > largest_block,
        "both real source executions cannot share a block"
    );
    let events = world.node.signals_since(cursor).unwrap();
    assert!(
        events.iter().any(
            |event| event.name == "eez.composer.sync_slot.gas_budget_cut"
                && event.fields["accepted_count"] == 1
                && event.fields["constraint"] == "l1_bundle_gas_budget"
        ),
        "missing complete-group bundle cut: {}",
        world.settlement_diagnostics()
    );
    let dispatches: Vec<_> = events
        .iter()
        .filter(|event| event.name == "eez.composer.bundle.dispatched")
        .collect();
    assert_eq!(dispatches.len(), 2);
    for dispatch in &dispatches {
        assert_eq!(dispatch.fields["attempt_counts"], "[0]");
    }
    let height = dispatches
        .iter()
        .map(|event| event.fields["sync_height"].as_u64().unwrap())
        .max()
        .unwrap();
    wait_for_safe_prefix_convergence(&[&world.node, &follower], height, TIMEOUT)
        .await
        .unwrap();
    assert_eq!(
        batches_posted(&world.l1_rpc(), world.dep.eez_address, 0)
            .await
            .unwrap(),
        2
    );
    assert!(
        !events
            .iter()
            .any(|event| event.name == "eez.composer.emission.historical_chunk")
    );
    world.node.assert_no_divergence_failure_logs();
    follower.assert_no_divergence_failure_logs();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ethereum_actual_companion_gas_reduces_whole_groups_before_proving() {
    assert_companion_gas_first(NativeL1Profile::EthereumOsaka).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_actual_companion_gas_reduces_whole_groups_before_proving() {
    assert_companion_gas_first(NativeL1Profile::GnosisPrague).await;
}
