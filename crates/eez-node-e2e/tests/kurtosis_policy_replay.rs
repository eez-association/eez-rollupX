//! Run with testing/kurtosis/scripts/verify-policy-replay.sh after the native
//! blob verifier. The follower receives L1 data and genesis only, never an
//! unsafe sequencer overlay or the composer's database.

use std::{path::PathBuf, time::Duration};

use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use alloy_provider::{Provider, ProviderBuilder};
use eez_testkit::{NodeBinary, NodeConfig, NodeHandle, wait_for};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the generated evidence and endpoints of a running Kurtosis enclave"]
async fn kurtosis_policy_batches_replay_independently_from_l1() {
    let evidence_path = PathBuf::from(std::env::var("EEZ_KURTOSIS_POLICY_EVIDENCE").unwrap());
    let evidence: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
    assert_eq!(evidence["verified"], true);
    let genesis = PathBuf::from(std::env::var("EEZ_KURTOSIS_L2_GENESIS").unwrap());
    let names = [
        "EEZ_L1_RPC_URL",
        "EEZ_L1_BEACON_URL",
        "EEZ_L1_CHAIN_ID",
        "EEZ_L1_CHAIN_CONFIG",
        "EEZ_REGISTRY_ADDRESS",
        "EEZ_REGISTRY_DEPLOY_BLOCK",
        "EEZ_ROLLUP_ID",
        "EEZ_L2_SYSTEM_KEY",
        "EEZL2_ADDRESS",
        "EEZ_L1_BLOCK_TIME_MS",
        "EEZ_L2_BLOCK_TIME_MS",
        "EEZ_PROOF_TIME_MS",
        "EEZ_SUBMISSION_SLACK_MS",
    ];
    let env: Vec<_> = names
        .into_iter()
        .map(|name| (name, std::env::var(name).unwrap()))
        .collect();
    let follower = NodeHandle::start(
        "kurtosis-policy-independent",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&genesis),
        },
        &env,
    )
    .await
    .unwrap();
    let rpc = ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap());
    let height = evidence["settlement"]["safe_l2_block"].as_u64().unwrap();
    let expected: B256 = evidence["settlement"]["safe_l2_block_hash"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    wait_for(Duration::from_secs(180), || async {
        let safe = rpc.get_block_by_number(BlockNumberOrTag::Safe).await?;
        Ok(safe.filter(|block| block.header.number >= height))
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "independent safe replay failed: {error:#}\n{}",
            follower.log_tail(80)
        )
    });
    assert_eq!(
        rpc.get_block_by_number(height.into())
            .await
            .unwrap()
            .unwrap()
            .header
            .hash,
        expected
    );
    let mut checked = std::collections::BTreeMap::new();
    for batch in evidence["verified_blob_batches"].as_array().unwrap() {
        for block in batch["payload"]["blocks"].as_array().unwrap() {
            let number = block["number"].as_u64().unwrap();
            let hash: B256 = block["hash"].as_str().unwrap().parse().unwrap();
            if let Some(previous) = checked.insert(number, hash) {
                assert_eq!(previous, hash);
            }
            let replayed = rpc
                .get_block_by_number(number.into())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(replayed.header.hash, hash, "reconstruction at L2 {number}");
        }
    }
    assert!(checked.contains_key(&height));
    for hash in evidence["l2_transaction_to_l1_blob_batch"]
        .as_object()
        .unwrap()
        .keys()
    {
        let receipt = rpc
            .get_transaction_receipt(hash.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        let expected_status = match evidence.get("l2_transaction_status") {
            Some(statuses) => statuses[hash]
                .as_bool()
                .expect("receipt status missing from complete history evidence"),
            None => true,
        };
        assert_eq!(receipt.status(), expected_status, "replayed receipt {hash}");
        assert_eq!(
            receipt.block_hash,
            checked.get(&receipt.block_number.unwrap()).copied()
        );
    }
    follower.assert_no_process_death();
    let output = evidence_path.with_file_name("independent-replay.json");
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&serde_json::json!({
            "verified": true, "source": "canonical_l1_only", "checked_blocks": checked.len(),
            "checked_receipts": evidence["l2_transaction_to_l1_blob_batch"].as_object().unwrap().len(),
            "safe_l2_block": height, "safe_l2_block_hash": expected,
        }))
        .unwrap(),
    )
    .unwrap();
}
