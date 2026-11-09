//! Upgrade history through ordinary L2 execution, then prove and independently replay it.
use super::*;
use eez_protocol::l1_history::{HISTORY_ADMIN_ADDRESS, IMPLEMENTATION_SLOT};

alloy_sol_types::sol! {
    interface IHistoryAdmin {
        function owner() external view returns (address);
        function upgradeAndCall(address proxy, address implementation, bytes data) external payable;
    }
}

async fn call<C: SolCall>(url: &str, to: Address, call: C, block: &str) -> Result<C::Return> {
    let data: Bytes = serde_json::from_value(
        rpc(
            url,
            "eth_call",
            json!([
                {"to": to, "data": Bytes::from(call.abi_encode())}, block
            ]),
        )
        .await?,
    )?;
    Ok(C::abi_decode_returns(&data)?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn history_proxy_upgrade_preserves_records_and_offline_replay() {
    let world = setup_cross_chain_with_env(&[
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "1000".into()),
    ])
    .await
    .unwrap();
    let mut follower = None;
    let result: Result<()> = async {
        let url = world.l2_rpc();
        let genesis = rpc(&url, "eth_getBlockByNumber", json!(["0x0", false])).await?;
        let empty = call(&url, HISTORY_ADDRESS, IL1BlockhashContract::getLatestImportedL1BlockCall {}, "0x0").await?;
        ensure!(empty.number == 0 && empty.hash.is_zero(), "genesis history is not empty");
        ensure!(call(&url, HISTORY_ADMIN_ADDRESS, IHistoryAdmin::ownerCall {}, "latest").await? == ANVIL_ADDR, "history upgrade owner");
        let previous = wait_for(SETTLE_TIMEOUT, || async {
            let value = call(&url, HISTORY_ADDRESS, IL1BlockhashContract::getLatestImportedL1BlockCall {}, "latest").await?;
            Ok((value.number > 0).then_some(value))
        }).await?;

        // A fresh deployment at a different address exercises the witnessed code
        // pointer. The Solidity suite also upgrades to a changed compatible V2.
        let artifact: Value = serde_json::from_slice(&std::fs::read(repo_root()
            .join("contracts/out/L1BlockhashContract.sol/L1BlockhashContract.json"))?)?;
        let creation: Bytes = serde_json::from_value(artifact["bytecode"]["object"].clone())?;
        let deploy = sign_and_send(&url, ANVIL_KEY, world.l2_chain_id,
            onchain_nonce(&url, ANVIL_KEY).await?, None, U256::ZERO, creation.to_vec(), 2_000_000).await?;
        ensure!(wait_for(SETTLE_TIMEOUT, || receipt_ok(&url, deploy)).await?, "implementation deployment failed");
        let receipt = rpc(&url, "eth_getTransactionReceipt", json!([deploy])).await?;
        let implementation: Address = serde_json::from_value(receipt["contractAddress"].clone())?;
        let upgrade_data = IHistoryAdmin::upgradeAndCallCall {
            proxy: HISTORY_ADDRESS, implementation, data: Bytes::new(),
        }.abi_encode();
        let denied = sign_and_send(&url, OUTBOUND_USER, world.l2_chain_id,
            onchain_nonce(&url, OUTBOUND_USER).await?, Some(HISTORY_ADMIN_ADDRESS),
            U256::ZERO, upgrade_data.clone(), 150_000).await?;
        ensure!(!wait_for(SETTLE_TIMEOUT, || receipt_ok(&url, denied)).await?, "unauthorized upgrade succeeded");
        let upgrade = sign_and_send(&url, ANVIL_KEY, world.l2_chain_id,
            onchain_nonce(&url, ANVIL_KEY).await?, Some(HISTORY_ADMIN_ADDRESS),
            U256::ZERO, upgrade_data, 150_000).await?;
        ensure!(wait_for(SETTLE_TIMEOUT, || receipt_ok(&url, upgrade)).await?, "authorized upgrade failed");
        let upgrade_receipt = rpc(&url, "eth_getTransactionReceipt", json!([upgrade])).await?;
        let upgrade_height: alloy_primitives::U64 = serde_json::from_value(upgrade_receipt["blockNumber"].clone())?;
        let at_upgrade = call(&url, HISTORY_ADDRESS, IL1BlockhashContract::getLatestImportedL1BlockCall {},
            upgrade_receipt["blockNumber"].as_str().context("upgrade height")?).await?;
        ensure!(at_upgrade.number >= previous.number, "upgrade lost the endpoint");
        ensure!(call(&url, HISTORY_ADDRESS, IL1BlockhashContract::getL1BlockHashCall {number: previous.number}, "latest").await? == previous.hash, "upgrade lost stored hash");
        ensure!(rpc(&url, "eth_getStorageAt", json!([HISTORY_ADDRESS, IMPLEMENTATION_SLOT, "latest"])).await? == serde_json::to_value(implementation.into_word())?, "implementation pointer unchanged");

        // Wait for a later actual import, then require its settled block on a
        // follower that has no speculative state from the producer.
        let (height, block, imported) = wait_for(SETTLE_TIMEOUT, || async {
            let block = rpc(&url, "eth_getBlockByNumber", json!(["latest", true])).await?;
            let height = block["number"].as_str().context("L2 height")?.to_owned();
            let imported = call(&url, HISTORY_ADDRESS, IL1BlockhashContract::getLatestImportedL1BlockCall {}, &height).await?;
            Ok((imported.number > at_upgrade.number).then_some((height, block, imported)))
        }).await?;
        let height_number = u64::from_str_radix(height.trim_start_matches("0x"), 16)?;
        ensure!(height_number > upgrade_height.to::<u64>(), "no post-upgrade execution");
        let l1 = rpc(&world.l1_rpc(), "eth_getBlockByNumber", json!([format!("0x{:x}", imported.number), false])).await?;
        ensure!(serde_json::to_value(imported.hash)? == l1["hash"], "stored L1 hash differs");
        ensure!(block["mixHash"] == l1["mixHash"], "PREVRANDAO differs from imported origin");
        // History-only blocks do not trigger the pure-L2 transaction policy.
        // A later ordinary transfer makes settlement cover this exact import.
        let trigger = sign_and_send(
            &url, ANVIL_KEY, world.l2_chain_id,
            onchain_nonce(&url, ANVIL_KEY).await?, Some(world.recipient),
            U256::ZERO, Vec::new(), 21_000,
        ).await?;
        ensure!(wait_for(SETTLE_TIMEOUT, || receipt_ok(&url, trigger)).await?, "settlement trigger failed");
        wait_for(SETTLE_TIMEOUT, || async {
            let safe = rpc(&url, "eth_getBlockByNumber", json!(["safe", false])).await?;
            let Some(number) = safe["number"].as_str() else { return Ok(None); };
            Ok((u64::from_str_radix(number.trim_start_matches("0x"), 16)? >= height_number).then_some(()))
        }).await?;
        follower = Some(eez_testkit::native::spawn_follower(&world, "history-upgrade-replay").await?);
        let replay = follower.as_ref().unwrap().l2_rpc_url();
        wait_for(SETTLE_TIMEOUT, || async {
            let value = rpc(&replay, "eth_getBlockByNumber", json!([height, false])).await?;
            Ok((!value.is_null()).then_some(()))
        }).await?;
        ensure!(block == rpc(&replay, "eth_getBlockByNumber", json!([height, true])).await?, "follower block differs");
        for hash in [deploy, denied, upgrade] {
            ensure!(rpc(&url, "eth_getTransactionReceipt", json!([hash])).await? == rpc(&replay, "eth_getTransactionReceipt", json!([hash])).await?, "upgrade replay receipt differs");
        }
        ensure!(rpc(&replay, "eth_getStorageAt", json!([HISTORY_ADDRESS, IMPLEMENTATION_SLOT, height])).await? == serde_json::to_value(implementation.into_word())?, "follower code pointer differs");
        for (number, hash) in [(previous.number, previous.hash), (imported.number, imported.hash)] {
            ensure!(call(&replay, HISTORY_ADDRESS, IL1BlockhashContract::getL1BlockHashCall {number}, &height).await? == hash, "follower lost history");
        }
        ensure!(genesis == rpc(&url, "eth_getBlockByNumber", json!(["0x0", false])).await?, "upgrade changed genesis identity");
        println!("history upgrade replay evidence: {}", json!({"upgrade": upgrade, "implementation": implementation,
            "blockHash": block["hash"], "retainedOrigin": previous.number, "newOrigin": imported.number}));
        Ok(())
    }.await;
    result.unwrap();
}
