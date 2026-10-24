//! Run with testing/kurtosis/scripts/verify-policy-replay.sh after the native
//! blob verifier. The follower receives L1 data and genesis only, never an
//! unsafe sequencer overlay or the composer's database.

use std::{path::PathBuf, time::Duration};

use alloy_eips::BlockNumberOrTag;
use alloy_network::{AnyTransactionReceipt, ReceiptResponse};
use alloy_primitives::{B256, Bytes, keccak256};
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
        "EEZL2_ADDRESS",
        "EEZ_L1_BLOCK_TIME_MS",
        "EEZ_L2_BLOCK_TIME_MS",
        "EEZ_PROOF_TIME_MS",
        "EEZ_SUBMISSION_SLACK_MS",
        "EEZ_SETTLEMENT_START_AFTER_BLOCK",
        "EEZ_SETTLEMENT_START_DELAY_MS",
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
    wait_for(Duration::from_mins(3), || async {
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
    let batches = evidence["verified_blob_batches"].as_array().unwrap();
    assert!(!batches.is_empty());
    let mut derived_blocks = 0;
    for batch in batches {
        assert_eq!(batch["verified"], true);
        let payload = &batch["payload"];
        let operations: Bytes = payload["operations"].as_str().unwrap().parse().unwrap();
        let plan = eez_payload_codec::v0::Payload::decode(&operations).unwrap();
        let pre_hash: B256 = batch["settlement_claim"]["pre_block_hash"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let post_hash: B256 = batch["settlement_claim"]["post_block_hash"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let mut parent = rpc
            .get_block_by_hash(pre_hash)
            .await
            .unwrap()
            .unwrap()
            .header;
        let terminal = rpc
            .get_block_by_hash(post_hash)
            .await
            .unwrap()
            .unwrap()
            .header;
        assert_eq!(
            terminal.number.checked_sub(parent.number).unwrap(),
            u64::from(plan.block_count())
        );
        assert_eq!(
            payload["block_count"].as_u64().unwrap(),
            u64::from(plan.block_count())
        );
        // Check the available canonical range before iterating a supplied empty
        // run. Decoding alone never authenticates its declared span or endpoints.
        assert!(terminal.number <= height);
        assert_eq!(
            rpc.get_block_by_number(parent.number.into())
                .await
                .unwrap()
                .unwrap()
                .header
                .hash,
            pre_hash
        );
        let mut transaction_hashes = Vec::new();
        for (index, block) in plan.blocks().enumerate() {
            let number = parent.number.checked_add(1).unwrap();
            let replayed = rpc
                .get_block_by_number(number.into())
                .await
                .unwrap()
                .unwrap();
            let header = &replayed.header;
            assert_eq!(header.number, number);
            assert_eq!(header.parent_hash, parent.hash, "ancestry at L2 {number}");
            assert_eq!(header.beneficiary, block.beneficiary);
            assert_eq!(header.extra_data.as_ref(), block.extra_data);
            let ordinary: Vec<_> = block.transactions.map(keccak256).collect();
            let actual: Vec<_> = replayed.transactions.hashes().collect();
            if index + 1 == plan.block_count() as usize {
                // Only the terminal can have a reconstructed native/action
                // suffix. The independent follower validates that entire suffix
                // against L1 semantics and the endpoint hash checked below.
                assert!(actual.starts_with(&ordinary), "terminal ordinary prefix");
            } else {
                assert_eq!(actual, ordinary, "ordinary transactions at L2 {number}");
            }
            transaction_hashes.extend(ordinary);
            derived_blocks += 1;
            if let Some(previous) = checked.insert(number, header.hash) {
                assert_eq!(previous, header.hash);
            }
            parent = replayed.header;
        }
        assert_eq!(
            parent.hash, post_hash,
            "independently replayed settlement endpoint"
        );
        let expected_transactions: Vec<B256> = payload["transaction_hashes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hash| hash.as_str().unwrap().parse().unwrap())
            .collect();
        assert_eq!(transaction_hashes, expected_transactions);
        assert_eq!(
            payload["transaction_count"].as_u64().unwrap(),
            transaction_hashes.len() as u64
        );
    }
    assert!(checked.contains_key(&height));
    let receipts = evidence["l2_transaction_to_l1_blob_batch"]
        .as_object()
        .unwrap();
    assert!(!receipts.is_empty());
    for hash in receipts.keys() {
        // L2 receipts include native type 0x45, outside Ethereum's closed receipt enum.
        let receipt: Option<AnyTransactionReceipt> = rpc
            .client()
            .request(
                "eth_getTransactionReceipt",
                (hash.parse::<B256>().unwrap(),),
            )
            .await
            .unwrap();
        let receipt = receipt.expect("replayed receipt missing");
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
            "checked_derived_blocks": derived_blocks,
            "checked_receipts": receipts.len(),
            "safe_l2_block": height, "safe_l2_block_hash": expected,
        }))
        .unwrap(),
    )
    .unwrap();
}
