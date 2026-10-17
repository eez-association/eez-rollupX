//! Real Safe 1.4.1 factory deployments, including nested deployments from its
//! optional initializer. Fixtures are the published, unmodified creation code.

use alloy_consensus::{SignableTransaction, TxEip1559};
use alloy_eips::Encodable2718;
use alloy_network::TxSignerSync;
use alloy_primitives::{Address, B256, Bytes, TxKind, U256, keccak256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{SolCall, SolValue, sol};
use eez_testkit::{
    CrossChainWorld, NativeL1Profile, NodeBinary, NodeConfig, NodeHandle, OUTBOUND_USER,
    TARGET_DEPLOYER, create_l2_cross_chain_proxy, onchain_nonce, setup_cross_chain_with_profile,
    sign_and_send, wait_for, wait_for_safe_prefix_convergence,
};
use serde_json::{Value, json};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_mins(3);
const GAS_LIMIT: u64 = 5_000_000;

sol! {
    #[sol(rpc)]
    interface SafeAccount {
        function setup(address[] owners, uint256 threshold, address to, bytes data,
            address fallbackHandler, address paymentToken, uint256 payment,
            address payable paymentReceiver) external;
        function getOwners() external view returns (address[]);
        function getThreshold() external view returns (uint256);
    }
    #[sol(rpc)]
    interface SafeFactory {
        function createProxyWithNonce(address singleton, bytes initializer, uint256 saltNonce)
            external returns (address);
        function proxyCreationCode() external pure returns (bytes);
    }
    interface MultiSend {
        function multiSend(bytes transactions) external payable;
    }
}

fn quantity(value: &Value) -> u128 {
    u128::from_str_radix(value.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

fn setup(owner: Address, to: Address, data: Bytes) -> Bytes {
    SafeAccount::setupCall {
        owners: vec![owner],
        threshold: U256::from(1),
        to,
        data,
        fallbackHandler: Address::ZERO,
        paymentToken: Address::ZERO,
        payment: U256::ZERO,
        paymentReceiver: Address::ZERO,
    }
    .abi_encode()
    .into()
}

fn create(singleton: Address, initializer: Bytes, salt: u64) -> Bytes {
    SafeFactory::createProxyWithNonceCall {
        singleton,
        initializer,
        saltNonce: U256::from(salt),
    }
    .abi_encode()
    .into()
}

async fn deploy(rpc: &str, chain_id: u64, name: &str) -> Address {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/safe-1.4.1/bytecode.json")).unwrap();
    let fixture = &fixture["contracts"][name];
    let bytecode: Bytes = fixture["bytecode"].as_str().unwrap().parse().unwrap();
    let hash = sign_and_send(
        rpc,
        TARGET_DEPLOYER,
        chain_id,
        onchain_nonce(rpc, TARGET_DEPLOYER).await.unwrap(),
        None,
        U256::ZERO,
        bytecode.to_vec(),
        7_000_000,
    )
    .await
    .unwrap();
    let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());
    let receipt = wait_for(TIMEOUT, || async {
        Ok(provider.get_transaction_receipt(hash).await?)
    })
    .await
    .unwrap();
    assert!(receipt.status(), "deploy {name}");
    let address = receipt.contract_address.unwrap();
    if let Some(expected) = fixture["runtimeHash"].as_str() {
        assert_eq!(
            keccak256(provider.get_code_at(address).await.unwrap()),
            expected.parse::<B256>().unwrap(),
            "official {name} runtime"
        );
    }
    address
}

async fn predicted(
    rpc: &str,
    factory: Address,
    singleton: Address,
    initializer: &Bytes,
    salt: u64,
) -> Address {
    let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());
    let mut init = SafeFactory::new(factory, provider)
        .proxyCreationCode()
        .call()
        .await
        .unwrap()
        .to_vec();
    init.extend_from_slice(singleton.into_word().as_slice());
    let salt = keccak256((keccak256(initializer), U256::from(salt)).abi_encode());
    factory.create2_from_code(salt, init)
}

async fn quote(world: &CrossChainWorld, target: Address, data: &Bytes, nonce: u64) -> Value {
    let signer: PrivateKeySigner = OUTBOUND_USER.parse().unwrap();
    let transaction = json!({"type":"0x2", "chainId":format!("{:#x}",world.l2_chain_id),
        "from":signer.address(), "to":target, "nonce":format!("{nonce:#x}"),
        "gas":format!("{GAS_LIMIT:#x}"), "value":"0x0", "data":data});
    let client = reqwest::Client::new();
    wait_for(Duration::from_secs(40), || async {
        let response = client
            .post(world.l2_xchain())
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"eez_debugQuoteFee","params":[transaction]}))
            .send()
            .await?;
        let status = response.status();
        let value: Value = response.json().await?;
        if status.is_success() && value.get("result").is_some() {
            let detail = &value["result"]["diagnostics"]["options"][0];
            anyhow::ensure!(detail.get("error").is_none(), "Safe quote failed: {detail}");
            return Ok(Some(detail.clone()));
        }
        if matches!(
            value["error"]["data"]["code"].as_str(),
            Some("stale_context" | "quote_unavailable" | "quote_busy")
        ) {
            return Ok(None);
        }
        anyhow::bail!(
            "Safe quote failed: {status} {value}{}",
            world.settlement_diagnostics()
        );
    })
    .await
    .unwrap()
}

fn transaction(
    world: &CrossChainWorld,
    target: Address,
    data: Bytes,
    nonce: u64,
    quote: &Value,
) -> TxEip1559 {
    TxEip1559 {
        chain_id: world.l2_chain_id,
        nonce,
        gas_limit: GAS_LIMIT,
        max_priority_fee_per_gas: quantity(&quote["transaction"]["maxPriorityFeePerGas"]),
        max_fee_per_gas: quantity(&quote["transaction"]["maxFeePerGas"]),
        to: TxKind::Call(target),
        input: data,
        ..Default::default()
    }
}

async fn send(world: &CrossChainWorld, mut tx: TxEip1559, rejection: Option<&str>) -> B256 {
    let signer: PrivateKeySigner = OUTBOUND_USER.parse().unwrap();
    let signature = signer.sign_transaction_sync(&mut tx).unwrap();
    let signed = tx.into_signed(signature);
    let hash = *signed.hash();
    let raw = alloy_primitives::hex::encode_prefixed(signed.encoded_2718());
    let client = reqwest::Client::new();
    wait_for(Duration::from_secs(40), || async {
        let response: Value = client
            .post(world.l2_xchain())
            .json(&json!({
                "jsonrpc":"2.0", "id":1, "method":"eth_sendRawTransaction", "params":[raw]
            }))
            .send()
            .await?
            .json()
            .await?;
        if matches!(
            response["error"]["data"]["code"].as_str(),
            Some("stale_context" | "quote_unavailable" | "quote_busy")
        ) {
            return Ok(None);
        }
        if let Some(label) = rejection {
            anyhow::ensure!(
                response["error"]["code"] == -32003
                    && response["error"]["data"]["code"] == "l2_tip_underfunded",
                "{label}: expected tariff rejection, got {response}"
            );
        } else {
            anyhow::ensure!(
                response["result"] == json!(hash),
                "Safe source rejected: {response}"
            );
        }
        Ok(Some(()))
    })
    .await
    .unwrap();
    if let Some(label) = rejection {
        let status: Value = client
            .post(world.l2_xchain())
            .json(&json!({
                "jsonrpc":"2.0", "id":2, "method":"eez_getCrossChainTransaction", "params":[hash]
            }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(
            status["error"].is_null() && status["result"].is_null(),
            "{label}: {status}"
        );
    }
    hash
}

async fn assert_safe(rpc: &str, address: Address, owner: Address) {
    let provider = ProviderBuilder::new().connect_http(rpc.parse().unwrap());
    wait_for(TIMEOUT, || async {
        Ok((!provider.get_code_at(address).await?.is_empty()).then_some(()))
    })
    .await
    .unwrap();
    let safe = SafeAccount::new(address, provider);
    assert_eq!(safe.getOwners().call().await.unwrap(), vec![owner]);
    assert_eq!(safe.getThreshold().call().await.unwrap(), U256::from(1));
}

async fn funded_world(profile: NativeL1Profile) -> CrossChainWorld {
    let mut policy = crate::settlement_policy_cost::shadow_pricing_policy();
    // The deployed testnet tariff: parity conversion, 20% risk and no fixed
    // service allowances. Receipt assertions below independently recompute it.
    for field in ["proving_cost_l1", "operations_cost_l1", "treasury_cost_l1"] {
        policy[field] = json!("0");
    }
    policy["l1_per_l2_numerator"] = json!("1");
    policy["l1_per_l2_denominator"] = json!("1");
    policy["risk_bps"] = json!(2000);
    policy["profit_bps"] = json!(0);
    setup_cross_chain_with_profile(
        profile,
        &crate::settlement_policy_cost::fee_settings(
            &policy,
            &[
                ("EEZ_FEES_L2_TO_L1_ENABLED", "true".into()),
                ("EEZ_TEST_L2_DEBUG_RPC", "true".into()),
                ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
                ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "900000".into()),
                ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "10000".into()),
                ("EEZ_MAX_SPECULATIVE_DEPTH", "0".into()),
            ],
        ),
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn safe_factory_nested_initialization_is_funded_and_replays() {
    safe_roundtrip(NativeL1Profile::EthereumOsaka).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gnosis_safe_factory_nested_initialization_is_funded_and_replays() {
    safe_roundtrip(NativeL1Profile::GnosisPrague).await;
}

async fn safe_roundtrip(profile: NativeL1Profile) {
    let world = funded_world(profile).await;
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse().unwrap());
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let chain_id = l1.get_chain_id().await.unwrap();
    let singleton = deploy(&world.l1_rpc(), chain_id, "Safe").await;
    let factory = deploy(&world.l1_rpc(), chain_id, "SafeProxyFactory").await;
    let multisend = deploy(&world.l1_rpc(), chain_id, "MultiSend").await;
    let proxy = create_l2_cross_chain_proxy(&world.l2_rpc(), TARGET_DEPLOYER, factory, 0)
        .await
        .unwrap();
    let owner = OUTBOUND_USER.parse::<PrivateKeySigner>().unwrap().address();
    let initializer = setup(owner, Address::ZERO, Bytes::new());
    let data = create(singleton, initializer.clone(), 1);
    let safe = predicted(&world.l1_rpc(), factory, singleton, &initializer, 1).await;
    let nonce = onchain_nonce(&world.l2_rpc(), OUTBOUND_USER).await.unwrap();
    let cheap = quote(&world, proxy, &data, nonce).await;
    let template = transaction(&world, proxy, data.clone(), nonce, &cheap);
    let mut zero = template.clone();
    zero.max_priority_fee_per_gas = 0;
    send(&world, zero, Some("zero tip")).await;
    let mut reservation = template.clone();
    let required = cheap["estimate"]["requiredL2Tip"]
        .as_str()
        .unwrap()
        .parse::<u128>()
        .unwrap();
    reservation.max_priority_fee_per_gas = required.div_ceil(u128::from(GAS_LIMIT));
    assert!(reservation.max_priority_fee_per_gas < template.max_priority_fee_per_gas);
    send(&world, reservation, Some("gas limit is not charged gas")).await;
    let mut capped = template.clone();
    let base = l2
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .base_fee_per_gas
        .unwrap();
    capped.max_fee_per_gas = u128::from(base) + 1;
    capped.max_priority_fee_per_gas = capped.max_fee_per_gas;
    send(&world, capped, Some("effective tip after maxFee clipping")).await;
    assert_eq!(
        onchain_nonce(&world.l2_rpc(), OUTBOUND_USER).await.unwrap(),
        nonce
    );
    assert!(l1.get_code_at(safe).await.unwrap().is_empty());

    // Safe permits delegatecall initialization. Its MultiSend creates four
    // additional initialized Safes on L1 within the one quoted source call.
    let mut packed = Vec::new();
    let mut nested_safes = Vec::new();
    for salt in 10..14 {
        let call = create(singleton, initializer.clone(), salt);
        packed.push(0); // MultiSend CALL operation, not delegatecall.
        packed.extend_from_slice(factory.as_slice());
        packed.extend_from_slice(&U256::ZERO.to_be_bytes::<32>());
        packed.extend_from_slice(&U256::from(call.len()).to_be_bytes::<32>());
        packed.extend_from_slice(&call);
        nested_safes.push(predicted(&world.l1_rpc(), factory, singleton, &initializer, salt).await);
    }
    let expensive_initializer = setup(
        owner,
        multisend,
        MultiSend::multiSendCall {
            transactions: packed.into(),
        }
        .abi_encode()
        .into(),
    );
    let expensive_data = create(singleton, expensive_initializer.clone(), 2);
    let expensive_quote = quote(&world, proxy, &expensive_data, nonce).await;
    assert!(
        quantity(&expensive_quote["transaction"]["maxPriorityFeePerGas"])
            > quantity(&cheap["transaction"]["maxPriorityFeePerGas"]),
        "nested work must cost more"
    );
    let reused_quote = transaction(&world, proxy, expensive_data.clone(), nonce, &cheap);
    send(
        &world,
        reused_quote,
        Some("changed initializer reusing cheaper quote"),
    )
    .await;
    assert_eq!(
        onchain_nonce(&world.l2_rpc(), OUTBOUND_USER).await.unwrap(),
        nonce
    );
    for address in &nested_safes {
        assert!(l1.get_code_at(*address).await.unwrap().is_empty());
    }

    let mut evidence = Vec::new();
    for (data, addresses) in [
        (data, vec![safe]),
        (expensive_data, {
            nested_safes.push(
                predicted(
                    &world.l1_rpc(),
                    factory,
                    singleton,
                    &expensive_initializer,
                    2,
                )
                .await,
            );
            nested_safes
        }),
    ] {
        let nonce = onchain_nonce(&world.l2_rpc(), OUTBOUND_USER).await.unwrap();
        let quote = quote(&world, proxy, &data, nonce).await;
        let hash = send(
            &world,
            transaction(&world, proxy, data, nonce, &quote),
            None,
        )
        .await;
        for address in &addresses {
            assert_safe(&world.l1_rpc(), *address, owner).await;
        }
        let speculative = wait_for(TIMEOUT, || async {
            Ok(l2.get_transaction_receipt(hash).await?)
        })
        .await
        .unwrap();
        wait_for_safe_prefix_convergence(
            &[&world.node],
            speculative.block_number.unwrap(),
            TIMEOUT,
        )
        .await
        .unwrap();
        // Canonical derivation can replace the speculative receipt's block.
        let receipt = l2.get_transaction_receipt(hash).await.unwrap().unwrap();
        assert!(receipt.status());
        let block = l2
            .get_block_by_hash(receipt.block_hash.unwrap())
            .await
            .unwrap()
            .unwrap();
        let paid = u128::from(receipt.gas_used)
            * (receipt.effective_gas_price - u128::from(block.header.base_fee_per_gas.unwrap()));
        assert_eq!(block.header.beneficiary, Address::repeat_byte(0x44));
        let beneficiary_before = l2
            .get_balance(block.header.beneficiary)
            .block_id(block.header.parent_hash.into())
            .await
            .unwrap();
        let beneficiary_after = l2
            .get_balance(block.header.beneficiary)
            .block_id(block.header.hash.into())
            .await
            .unwrap();
        assert_eq!(
            beneficiary_after - beneficiary_before,
            U256::from(paid),
            "receipt tips must actually credit the configured beneficiary"
        );
        let mut found = None;
        for signal in world
            .node
            .signals_since(0)
            .unwrap()
            .into_iter()
            .filter(|s| s.name == "eez.composer.admission.v1")
        {
            let event: Value =
                serde_json::from_str(signal.fields["admission_event"].as_str().unwrap()).unwrap();
            let observation = &event["detail"]["fee_observation"];
            if event["kind"] != "preflight"
                || observation["l2"]["block_hash"] != json!(receipt.block_hash.unwrap())
            {
                continue;
            }
            let post_hash: B256 = observation["l1"]["transaction_hash"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let Some(post) = l1.get_transaction_receipt(post_hash).await.unwrap() else {
                continue;
            };
            assert!(post.status());
            let cost = eez_l1::posting_fees::ReceiptCost::from_receipt(&post)
                .unwrap()
                .paid
                .total
                .to::<u128>();
            let tariff = cost + (cost * 2000).div_ceil(10_000);
            assert!(
                paid >= tariff,
                "canonical Safe deployment underpaid: {paid} < {tariff}"
            );
            assert_eq!(observation["l1"]["companion_count"], 0);
            found = Some(json!({"source":hash,"sourceBlock":receipt.block_number,
                "sourceBlockHash":receipt.block_hash,"l2GasUsed":receipt.gas_used,
                "post":post_hash,"l1GasUsed":post.gas_used,"l1Cost":cost.to_string(),
                "requiredL2Tip":tariff.to_string(),"paidL2Tip":paid.to_string(),
                "safeAddresses":addresses,"quote":quote}));
        }
        evidence.push(found.expect("independent canonical post receipt required"));
    }
    let follower = NodeHandle::start(
        "safe-fee-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &world.partial_follower_env(),
    )
    .await
    .unwrap();
    let follower_rpc = ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap());
    for entry in &evidence {
        wait_for_safe_prefix_convergence(
            &[&world.node, &follower],
            entry["sourceBlock"].as_u64().unwrap(),
            TIMEOUT,
        )
        .await
        .unwrap();
        let hash = entry["source"].as_str().unwrap().parse::<B256>().unwrap();
        let replayed = follower_rpc
            .get_transaction_receipt(hash)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(json!(replayed.block_hash), entry["sourceBlockHash"]);
        assert_eq!(json!(replayed.gas_used), entry["l2GasUsed"]);
    }
    if let Ok(dir) = std::env::var("EEZ_TEST_LOG_DIR") {
        std::fs::write(
            std::path::Path::new(&dir).join(format!("{profile:?}-safe-fee-evidence.json")),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
    }
}

/// Documents the current scope: ordinary L2 deployments do not enter outbound
/// tariff admission. A positive receipt does not establish posting-cost recovery.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn entirely_l2_safe_keeps_ordinary_gas_rules() {
    let world = funded_world(NativeL1Profile::EthereumOsaka).await;
    let singleton = deploy(&world.l2_rpc(), world.l2_chain_id, "Safe").await;
    let factory = deploy(&world.l2_rpc(), world.l2_chain_id, "SafeProxyFactory").await;
    let owner = OUTBOUND_USER.parse::<PrivateKeySigner>().unwrap().address();
    let initializer = setup(owner, Address::ZERO, Bytes::new());
    let safe = predicted(&world.l2_rpc(), factory, singleton, &initializer, 100).await;
    let tx = TxEip1559 {
        chain_id: world.l2_chain_id,
        nonce: onchain_nonce(&world.l2_rpc(), OUTBOUND_USER).await.unwrap(),
        gas_limit: GAS_LIMIT,
        max_fee_per_gas: 100_000_000_000,
        max_priority_fee_per_gas: 0,
        to: TxKind::Call(factory),
        input: create(singleton, initializer, 100),
        ..Default::default()
    };
    let hash = send(&world, tx, None).await;
    assert_safe(&world.l2_rpc(), safe, owner).await;
    let provider = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let receipt = provider
        .get_transaction_receipt(hash)
        .await
        .unwrap()
        .unwrap();
    assert!(receipt.status());
    let block = provider
        .get_block_by_hash(receipt.block_hash.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.effective_gas_price,
        u128::from(block.header.base_fee_per_gas.unwrap())
    );
    let status: Value = reqwest::Client::new()
        .post(world.l2_xchain())
        .json(&json!({
            "jsonrpc":"2.0", "id":2, "method":"eez_getCrossChainTransaction", "params":[hash]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        status["error"].is_null() && status["result"].is_null(),
        "ordinary route: {status}"
    );
    if let Ok(dir) = std::env::var("EEZ_TEST_LOG_DIR") {
        let evidence = json!({"source":hash,"safe":safe,"receipt":receipt,
            "paidL2Tip":"0","outboundQueueOwned":false,"settlementRecoveryEnforced":false});
        std::fs::write(
            std::path::Path::new(&dir).join("ordinary-safe-fee-evidence.json"),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
    }
}
