//! Optional fixture recording alongside the existing proving-stream capture.
//! Frozen prefix tests need RPC receipts and accepted L1 pins in addition to
//! witnesses; deriving those expectations from the verifier would be circular.
use alloy_consensus::Transaction as _;
use alloy_primitives::{B256, Bytes, U256};
use alloy_provider::{Provider as _, ProviderBuilder};
use alloy_sol_types::{SolCall as _, SolEvent as _};
use anyhow::Context as _;
use eez_testkit::{CrossChainWorld, IEEZ, IOfflineRollup};
use serde_json::{Value, json};

pub(super) async fn capture(world: &CrossChainWorld, terminal: B256) -> anyhow::Result<()> {
    let Some(directory) = std::env::var_os("EEZ_TEST_PROVE_FIXTURE_DIR") else {
        return Ok(());
    };
    let directory = std::path::PathBuf::from(directory);
    let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse()?);
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse()?);
    let front = ProviderBuilder::new().connect_http(world.l2_xchain().parse()?);
    let mut posted_pins = std::collections::BTreeSet::new();
    let mut accepted_pins = std::collections::BTreeSet::new();
    for log in l1
        .get_logs(
            &alloy_rpc_types_eth::Filter::new()
                .address(world.cfg.eez_address)
                .event_signature(IEEZ::L2ExecutionPerformed::SIGNATURE_HASH)
                .from_block(world.dep.deploy_block),
        )
        .await?
    {
        let event = IEEZ::L2ExecutionPerformed::decode_log(&log.inner)?;
        if event.data.rollupId == world.cfg.rollup_id {
            accepted_pins.insert(event.data.newRoot);
        }
    }
    let mut posts = Vec::new();
    for log in l1
        .get_logs(
            &alloy_rpc_types_eth::Filter::new()
                .address(world.cfg.eez_address)
                .event_signature(IEEZ::BatchPosted::SIGNATURE_HASH)
                .from_block(world.dep.deploy_block),
        )
        .await?
    {
        let hash = log
            .transaction_hash
            .ok_or_else(|| anyhow::anyhow!("post hash missing"))?;
        let transaction = l1
            .get_transaction_by_hash(hash)
            .await?
            .ok_or_else(|| anyhow::anyhow!("posted transaction missing"))?;
        let call =
            eez_protocol::abi::postAndVerifyBatchCall::abi_decode(transaction.inner.input())?;
        for entry in &call.batch.entries {
            for update in &entry.rootUpdates {
                if update.rollupId == world.cfg.rollup_id {
                    posted_pins.insert(update.currentRoot);
                    posted_pins.insert(update.newRoot);
                }
            }
        }
        posts.push(hash);
    }
    let profile_hash = IOfflineRollup::new(world.cfg.rollup_manager_address, &l1)
        .protocolConfigHash()
        .call()
        .await?;
    anyhow::ensure!(
        profile_hash == world.dep.profile.commitment(),
        "deployed profile differs"
    );
    let genesis: Value = serde_json::from_slice(&std::fs::read(&world.cfg.l2_genesis.0)?)?;
    let l1_genesis: Value = serde_json::from_slice(&std::fs::read(&world.cfg.l1_genesis.0)?)?;
    let chain_config: Value = l2
        .client()
        .request("debug_chainConfig", ())
        .await
        .context("capture L2 debug_chainConfig")?;
    let genesis_header = l2.get_block_by_number(0.into()).await?.unwrap().header;
    let mut hash = terminal;
    let mut imported = false;
    for index in 0..4096 {
        let rpc: Value = l2
            .client()
            .request("eth_getBlockByHash", (hash, true))
            .await?;
        let transactions = rpc["transactions"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("body missing"))?;
        let history_only = transactions.len() == 1 && transactions[0]["type"] == "0x46";
        let parent: B256 = serde_json::from_value(rpc["parentHash"].clone())?;
        if index == 0 || history_only {
            let history_address = eez_primitives::HISTORY_ADDRESS;
            let first: U256 = l2
                .client()
                .request("eth_getStorageAt", (history_address, "0x2", parent))
                .await?;
            let last: U256 = l2
                .client()
                .request("eth_getStorageAt", (history_address, "0x2", hash))
                .await?;
            let first = u64::try_from(first)?;
            let last = u64::try_from(last)?;
            anyhow::ensure!(
                last >= first && last - first <= 64,
                "unbounded history range"
            );
            let mut headers = Vec::<Bytes>::new();
            for number in first..=last {
                headers.push(
                    l1.client()
                        .request("debug_getRawHeader", (format!("0x{number:x}"),))
                        .await
                        .context("capture L1 debug_getRawHeader")?,
                );
            }
            let raw: Bytes = l2
                .client()
                .request("debug_getRawBlock", (hash,))
                .await
                .context("capture L2 debug_getRawBlock")?;
            let receipts: Vec<Bytes> = l2
                .client()
                .request("debug_getRawReceipts", (hash,))
                .await
                .context("capture L2 debug_getRawReceipts")?;
            let witness: Value = front
                .client()
                .request("eez_executionWitnessByBlockHash", (hash,))
                .await
                .context("capture L2 eez_executionWitnessByBlockHash")?;
            let input = json!({
                "block_hash": hash, "block_rlp": raw, "receipts_rlp": receipts, "witness": witness,
                "chain_config": chain_config, "genesis": genesis, "genesis_hash": genesis_header.hash,
                "l1_chain_config": l1_genesis, "l1_headers_rlp": headers, "protocol_config_hash": profile_hash,
                "accepted_root_pins": accepted_pins, "posted_root_pins": posted_pins, "accepted_posts": posts,
            });
            let name = if index == 0 {
                "prefix-history.json"
            } else {
                "prefix-history-import.json"
            };
            std::fs::write(directory.join(name), serde_json::to_vec_pretty(&input)?)?;
            if history_only && index != 0 {
                imported = true;
                break;
            }
        }
        if rpc["number"] == "0x0" {
            break;
        }
        hash = parent;
    }
    anyhow::ensure!(imported, "capture has no real history-only ancestor");
    Ok(())
}
