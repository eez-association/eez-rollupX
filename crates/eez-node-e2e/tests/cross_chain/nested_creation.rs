//! A nested callback's newly created account must be callable by the original
//! suspended L2 source frame. The factory returns the actual child address.

use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_sol_types::{SolCall, SolError, sol};
use eez_testkit::{
    CrossChainWorld, NativeL1Profile, OUTBOUND_USER, SETTLE_TIMEOUT, TARGET_DEPLOYER, call_read,
    create_cross_chain_proxy, create_l2_cross_chain_proxy, onchain_nonce, repo_root,
    setup_cross_chain_with_profile, sign_and_send, wait_for, wait_for_safe_chain_contains,
};
use serde_json::{Value, json};

sol! {
    #[sol(rpc)]
    interface Factory {
        function obtain(bool fresh, bool deterministic, bytes32 salt, uint256 initial) external returns (address);
        function lastChild() external view returns (address);
    }
    #[sol(rpc)]
    interface Caller {
        function createAndUse(address relayProxy, address factoryProxy, bool fresh, bool deterministic, bytes32 salt) external returns (address);
        function lastChild() external view returns (address);
        function lastValue() external view returns (uint256);
        function secondChild() external view returns (address);
        function createUseAndReenter(address relayProxy, address factoryProxy, address localFactory, bool deterministic, bytes32 salt) external;
        error MissingChildCode(address child);
    }
    #[sol(rpc)]
    interface Child {
        function value() external view returns (uint256);
    }
}

fn artifact(name: &str) -> Vec<u8> {
    let file = repo_root().join(format!(
        "contracts/out/NestedContractCreation.sol/{name}.json"
    ));
    let artifact: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    artifact["bytecode"]["object"]
        .as_str()
        .unwrap()
        .parse::<Bytes>()
        .unwrap()
        .to_vec()
}

async fn deploy(rpc: &str, chain_id: u64, name: &str) -> Address {
    let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());
    let hash = sign_and_send(
        rpc,
        TARGET_DEPLOYER,
        chain_id,
        onchain_nonce(rpc, TARGET_DEPLOYER).await.unwrap(),
        None,
        U256::ZERO,
        artifact(name),
        2_000_000,
    )
    .await
    .unwrap();
    let receipt = wait_for(SETTLE_TIMEOUT, || async {
        Ok(provider.get_transaction_receipt(hash).await?)
    })
    .await
    .unwrap();
    assert!(receipt.status(), "fixture deployment: {name}");
    receipt.contract_address.unwrap()
}

async fn terminal_status(world: &CrossChainWorld, hash: B256) -> Value {
    let front = ProviderBuilder::new().connect_http(world.l2_xchain().parse().unwrap());
    wait_for(SETTLE_TIMEOUT, || async {
        let status: Value = front
            .client()
            .request("eez_getCrossChainTransaction", (hash,))
            .await?;
        Ok((status["lifecycle"] == "terminal").then_some(status))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "transaction {hash}: {error:#}{}",
            world.settlement_diagnostics()
        )
    })
}

async fn nested_creation(deterministic: bool) {
    let profile = NativeL1Profile::GnosisPrague;
    let world = setup_cross_chain_with_profile(profile, &[]).await.unwrap();
    let l1_rpc = world.l1_rpc();
    let l2_rpc = world.l2_rpc();
    let l1 = ProviderBuilder::new().connect_http(l1_rpc.parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(l2_rpc.parse().unwrap());
    let factory = deploy(&l2_rpc, world.l2_chain_id, "NestedCreationFactory").await;
    let caller = deploy(&l2_rpc, world.l2_chain_id, "NestedCreationCaller").await;
    let relay = deploy(
        &l1_rpc,
        l1.get_chain_id().await.unwrap(),
        "NestedCreationRelay",
    )
    .await;
    let factory_proxy = create_cross_chain_proxy(
        &l1_rpc,
        TARGET_DEPLOYER,
        world.cfg.eez_address,
        factory,
        world.cfg.rollup_id,
    )
    .await
    .unwrap();
    let relay_proxy = create_l2_cross_chain_proxy(&l2_rpc, TARGET_DEPLOYER, relay, 0)
        .await
        .unwrap();

    // The control traverses the same two proxies and returns an already
    // deployed child, so a routing, ABI or funding failure cannot masquerade
    // as the fresh-account propagation defect.
    let create_existing = sign_and_send(
        &l2_rpc,
        TARGET_DEPLOYER,
        world.l2_chain_id,
        onchain_nonce(&l2_rpc, TARGET_DEPLOYER).await.unwrap(),
        Some(factory),
        U256::ZERO,
        Factory::obtainCall {
            fresh: true,
            deterministic: false,
            salt: B256::ZERO,
            initial: U256::from(41),
        }
        .abi_encode(),
        1_000_000,
    )
    .await
    .unwrap();
    let receipt = wait_for(SETTLE_TIMEOUT, || async {
        Ok(l2.get_transaction_receipt(create_existing).await?)
    })
    .await
    .unwrap();
    assert!(receipt.status(), "ordinary child deployment");
    let existing = Factory::new(factory, &l2).lastChild().call().await.unwrap();
    assert!(!l2.get_code_at(existing).await.unwrap().is_empty());
    let label = format!(
        "{profile:?}-{}",
        if deterministic { "create2" } else { "create" }
    );
    let mut evidence = json!({"profile":format!("{profile:?}"),"deterministic":deterministic,
        "caller":caller,"relay":relay,"factory":factory,
        "relayProxy":relay_proxy,"factoryProxy":factory_proxy,"existingChild":existing});

    for fresh in [false, true] {
        let hash = sign_and_send(
            &world.l2_xchain(),
            OUTBOUND_USER,
            world.l2_chain_id,
            onchain_nonce(&l2_rpc, OUTBOUND_USER).await.unwrap(),
            Some(caller),
            U256::ZERO,
            Caller::createAndUseCall {
                relayProxy: relay_proxy,
                factoryProxy: factory_proxy,
                fresh,
                deterministic,
                salt: B256::repeat_byte(0x42),
            }
            .abi_encode(),
            2_000_000,
        )
        .await
        .unwrap();
        let status = terminal_status(&world, hash).await;
        let case = if fresh { "fresh" } else { "existing" };
        evidence[case] = status.clone();
        if let Some(dir) = std::env::var_os("EEZ_TEST_LOG_DIR") {
            std::fs::write(
                std::path::Path::new(&dir).join(format!("nested-creation-{label}.json")),
                serde_json::to_vec_pretty(&evidence).unwrap(),
            )
            .unwrap();
        }
        eprintln!("{label} {case}: {status}");
        if status["ownership"]["reason"] == "SimulationFailed" {
            let rejection = &status["ownership"]["rejection"];
            let output: Bytes = rejection["source"]["output"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let missing = Caller::MissingChildCode::abi_decode_validate(&output)
                .expect("unexpected source failure instead of the fresh-child code check");
            assert!(fresh, "an existing child's code must survive the same path");
            let calls = rejection["calls"].as_array().unwrap();
            assert_eq!(calls.len(), 2, "one L1 call and one L2 callback: {status}");
            assert_eq!(rejection["omittedCalls"], 0);
            assert!(calls.iter().all(|call| call["success"] == true), "{status}");
            assert_eq!(calls[0]["target"], json!(relay));
            assert_eq!(calls[1]["target"], json!(factory));
            assert_eq!(calls[1]["depth"], 2);
            let returned: Bytes = calls[1]["returnData"].as_str().unwrap().parse().unwrap();
            assert_eq!(
                Factory::obtainCall::abi_decode_returns_validate(&returned).unwrap(),
                missing.child
            );
            panic!(
                "{label}: both cross-chain calls succeeded, but resumed L2 source sees no code at factory-returned child {}",
                missing.child
            );
        }
        assert_eq!(status["ownership"]["reason"], "Committed", "{status}");
        let receipt = l2.get_transaction_receipt(hash).await.unwrap().unwrap();
        assert!(receipt.status(), "source transaction must succeed");
        wait_for_safe_chain_contains(
            &world.node,
            receipt.block_number.unwrap(),
            receipt.block_hash.unwrap(),
            SETTLE_TIMEOUT,
        )
        .await
        .unwrap();
        let child = Caller::new(caller, &l2).lastChild().call().await.unwrap();
        assert!(!l2.get_code_at(child).await.unwrap().is_empty());
        assert_eq!(
            Child::new(child, &l2).value().call().await.unwrap(),
            U256::from(42)
        );
        assert_eq!(
            Caller::new(caller, &l2).lastValue().call().await.unwrap(),
            U256::from(42)
        );
        if fresh {
            assert_ne!(child, existing);
        } else {
            assert_eq!(child, existing);
        }
    }

    let factory_nonce = l2.get_transaction_count(factory).await.unwrap();
    let expected_second = factory.create(factory_nonce + 1);
    let hash = sign_and_send(
        &world.l2_xchain(),
        OUTBOUND_USER,
        world.l2_chain_id,
        onchain_nonce(&l2_rpc, OUTBOUND_USER).await.unwrap(),
        Some(caller),
        U256::ZERO,
        Caller::createUseAndReenterCall {
            relayProxy: relay_proxy,
            factoryProxy: factory_proxy,
            localFactory: factory,
            deterministic,
            salt: B256::repeat_byte(0x43),
        }
        .abi_encode(),
        2_000_000,
    )
    .await
    .unwrap();
    let status = terminal_status(&world, hash).await;
    evidence["reentry"] = status.clone();
    evidence["expectedSecondChild"] = json!(expected_second);
    evidence["factoryNonceBeforeReentry"] = json!(factory_nonce);
    if let Some(dir) = std::env::var_os("EEZ_TEST_LOG_DIR") {
        std::fs::write(
            std::path::Path::new(&dir).join(format!("nested-creation-{label}.json")),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(status["ownership"]["reason"], "Committed", "{status}");
    let receipt = l2.get_transaction_receipt(hash).await.unwrap().unwrap();
    assert!(receipt.status());
    wait_for_safe_chain_contains(
        &world.node,
        receipt.block_number.unwrap(),
        receipt.block_hash.unwrap(),
        SETTLE_TIMEOUT,
    )
    .await
    .unwrap();
    let child = Caller::new(caller, &l2).lastChild().call().await.unwrap();
    let second = Caller::new(caller, &l2).secondChild().call().await.unwrap();
    assert_eq!(
        second, expected_second,
        "resumed CREATE must use the callback's advanced nonce"
    );
    assert_ne!(child, second);
    assert!(!l2.get_code_at(child).await.unwrap().is_empty());
    assert!(!l2.get_code_at(second).await.unwrap().is_empty());
    assert_eq!(
        Child::new(child, &l2).value().call().await.unwrap(),
        U256::from(42)
    );
    assert_eq!(
        Child::new(second, &l2).value().call().await.unwrap(),
        U256::from(77)
    );
    assert_eq!(
        l2.get_transaction_count(factory).await.unwrap(),
        factory_nonce + 2
    );
    crate::cross_chain::assert_follower_replays(
        &world,
        &format!("nested-creation-{label}-follower"),
        &call_read(second, "value()", Child::valueCall {}.abi_encode()),
        U256::from(77),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_create_visible_after_nested_return() {
    nested_creation(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_create2_visible_after_nested_return() {
    nested_creation(true).await;
}
