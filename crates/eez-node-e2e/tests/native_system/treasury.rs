//! Fee credits, recipient rotation and withdrawal must survive independent DA replay.
use super::*;
use alloy_sol_types::SolEvent;
use eez_protocol::fee_vault::{FEE_VAULT_ADDRESS as VAULT, IFeeVault};

async fn call<C: SolCall>(url: &str, call: C, block: &str) -> Result<C::Return> {
    let data: Bytes = serde_json::from_value(
        rpc(
            url,
            "eth_call",
            json!([
                {"to": VAULT, "data": Bytes::from(call.abi_encode())}, block
            ]),
        )
        .await?,
    )?;
    Ok(C::abi_decode_returns(&data)?)
}

async fn send<C: SolCall>(world: &CrossChainWorld, key: &str, call: C) -> Result<B256> {
    sign_and_send(
        &world.l2_rpc(),
        key,
        world.l2_chain_id,
        onchain_nonce(&world.l2_rpc(), key).await?,
        Some(VAULT),
        U256::ZERO,
        call.abi_encode(),
        150_000,
    )
    .await
}

async fn balance(url: &str, account: Address, block: &str) -> Result<U256> {
    Ok(serde_json::from_value(
        rpc(url, "eth_getBalance", json!([account, block])).await?,
    )?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recipient_rotation_withdrawal_and_offline_replay() {
    let world = setup_cross_chain_with_env(&[
        ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".into()),
        ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "1000".into()),
    ])
    .await
    .unwrap();
    let mut follower = None;
    let result: Result<()> = async {
        let url = world.l2_rpc();
        let recipient = Address::with_last_byte(0xf2);
        ensure!(
            world.dep.profile.feeCollector == VAULT,
            "profile collector differs from vault"
        );
        let genesis = rpc(&url, "eth_getBlockByNumber", json!(["0x0", false])).await?;
        ensure!(
            rpc(&url, "eth_getCode", json!([VAULT, "0x0"])).await? != "0x",
            "vault missing from genesis"
        );
        ensure!(
            call(&url, IFeeVault::ownerCall {}, "0x0").await? == ANVIL_ADDR,
            "genesis owner"
        );
        let initial_recipient = call(&url, IFeeVault::recipientCall {}, "0x0").await?;
        ensure!(initial_recipient == ANVIL_ADDR_3, "genesis recipient");
        ensure!(
            balance(&url, VAULT, "0x0").await? == U256::ZERO,
            "vault must start empty"
        );
        ensure!(
            l2_balance(&url, VAULT).await? > U256::ZERO,
            "application deployment did not accrue base fees"
        );

        let denied = send(
            &world,
            OUTBOUND_USER,
            IFeeVault::setRecipientCall {
                newRecipient: recipient,
            },
        )
        .await?;
        ensure!(
            !wait_for(SETTLE_TIMEOUT, || receipt_ok(&url, denied)).await?,
            "unauthorized recipient change succeeded"
        );
        ensure!(
            call(&url, IFeeVault::recipientCall {}, "latest").await? == initial_recipient,
            "reverted setter changed state"
        );
        let rotate = send(
            &world,
            ANVIL_KEY,
            IFeeVault::setRecipientCall {
                newRecipient: recipient,
            },
        )
        .await?;
        ensure!(
            wait_for(SETTLE_TIMEOUT, || receipt_ok(&url, rotate)).await?,
            "owner setter failed"
        );
        ensure!(
            call(&url, IFeeVault::recipientCall {}, "latest").await? == recipient,
            "recipient not rotated"
        );
        let old_balance = l2_balance(&url, initial_recipient).await?;
        let recipient_before = l2_balance(&url, recipient).await?;
        let withdrawal = send(&world, OUTBOUND_USER, IFeeVault::withdrawCall {}).await?;
        ensure!(
            wait_for(SETTLE_TIMEOUT, || receipt_ok(&url, withdrawal)).await?,
            "permissionless withdrawal failed"
        );
        let receipt = rpc(&url, "eth_getTransactionReceipt", json!([withdrawal])).await?;
        let height = receipt["blockNumber"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("receipt height"))?;
        let height_number = u64::from_str_radix(height.trim_start_matches("0x"), 16)?;
        let logs: Vec<alloy_primitives::Log> = receipt["logs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|log| -> Result<_> {
                Ok(alloy_primitives::Log::new_unchecked(
                    serde_json::from_value(log["address"].clone())?,
                    serde_json::from_value(log["topics"].clone())?,
                    serde_json::from_value(log["data"].clone())?,
                ))
            })
            .collect::<Result<_>>()?;
        ensure!(
            logs.len() == 1,
            "withdrawal must emit exactly one payment event"
        );
        ensure!(
            logs[0].address == VAULT,
            "payment event came from another contract"
        );
        let payment = IFeeVault::Withdrawal::decode_log_validate(&logs[0])?;
        ensure!(
            payment.recipient == recipient && payment.caller == signer_address(OUTBOUND_USER)?,
            "wrong withdrawal destination/caller"
        );
        ensure!(payment.amount > U256::ZERO, "empty fee payment");
        ensure!(
            balance(&url, recipient, height).await? == recipient_before + payment.amount,
            "recipient did not receive the exact payment"
        );
        ensure!(
            balance(&url, initial_recipient, height).await? == old_balance,
            "old recipient received fees after rotation"
        );
        // No ordinary transaction follows the withdrawal in this fixture. Its own
        // fee is credited after execution and remains in the vault for the next payment.
        let block = rpc(&url, "eth_getBlockByNumber", json!([height, true])).await?;
        let base_fee: U256 = serde_json::from_value(block["baseFeePerGas"].clone())?;
        let gas: U256 = serde_json::from_value(receipt["gasUsed"].clone())?;
        ensure!(
            balance(&url, VAULT, height).await? == base_fee * gas,
            "withdrawal's own base fee was lost or duplicated"
        );
        ensure!(
            genesis == rpc(&url, "eth_getBlockByNumber", json!(["0x0", false])).await?,
            "treasury rotation changed genesis identity"
        );

        wait_for(SETTLE_TIMEOUT, || async {
            let safe = rpc(&url, "eth_getBlockByNumber", json!(["safe", false])).await?;
            let Some(number) = safe["number"].as_str() else {
                return Ok(None);
            };
            Ok(
                (u64::from_str_radix(number.trim_start_matches("0x"), 16)? >= height_number)
                    .then_some(()),
            )
        })
        .await?;
        follower = Some(eez_testkit::native::spawn_follower(&world, "treasury-replay").await?);
        let replay = follower.as_ref().unwrap().l2_rpc_url();
        wait_for(SETTLE_TIMEOUT, || async {
            let imported = rpc(&replay, "eth_getBlockByNumber", json!([height, false])).await?;
            Ok((!imported.is_null()).then_some(imported))
        })
        .await?;
        let imported = rpc(&replay, "eth_getBlockByNumber", json!([height, true])).await?;
        ensure!(
            block == imported,
            "cold follower changed the canonical withdrawal block"
        );
        for hash in [denied, rotate, withdrawal] {
            ensure!(
                rpc(&url, "eth_getTransactionReceipt", json!([hash])).await?
                    == rpc(&replay, "eth_getTransactionReceipt", json!([hash])).await?,
                "replayed treasury receipt differs"
            );
        }
        for account in [VAULT, initial_recipient, recipient] {
            ensure!(
                balance(&url, account, height).await? == balance(&replay, account, height).await?,
                "replayed treasury balance differs"
            );
        }
        ensure!(
            call(&replay, IFeeVault::recipientCall {}, height).await? == recipient,
            "replayed treasury recipient differs"
        );
        ensure!(
            call(&replay, IFeeVault::ownerCall {}, height).await? == ANVIL_ADDR,
            "recipient rotation changed owner"
        );
        println!(
            "treasury replay evidence: {}",
            json!({"vault": VAULT, "recipient": recipient,
            "withdrawal": withdrawal, "blockHash": receipt["blockHash"], "amount": payment.amount,
            "retainedFee": base_fee * gas})
        );
        Ok(())
    }
    .await;
    result.unwrap();
}
