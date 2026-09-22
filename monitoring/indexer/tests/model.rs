use alloy_primitives::{Address, B256, U256};
use alloy_sol_types::SolEvent;
use eez_indexer::{
    config::{Config, DetailsConfig},
    model::{Block, Event, L2ExecutionPerformed, Receipt, quantity},
};
use serde_json::json;

fn config() -> Config {
    Config {
        http_url: "http://localhost:8545".into(),
        ws_url: "ws://localhost:8546".into(),
        chain_id: 10200,
        genesis_hash: B256::repeat_byte(1),
        registry: Address::repeat_byte(2),
        rollup_id: 1,
        start_block: 1,
        details: None,
        posting_logs_dir: None,
    }
}

#[test]
fn config_rejects_invalid_identity_endpoints_and_unknown_fields() {
    let c = config();
    c.validate().unwrap();
    let mut bad = c.clone();
    bad.ws_url = "http://localhost".into();
    assert!(bad.validate().is_err());
    let mut bad = c.clone();
    bad.start_block = 0;
    assert!(bad.validate().is_err());
    let mut bad = c.clone();
    bad.registry = Address::ZERO;
    assert!(bad.validate().is_err());
    let mut bad = c;
    bad.genesis_hash = B256::ZERO;
    assert!(bad.validate().is_err());
    assert!(serde_json::from_value::<Config>(json!({"unused":true})).is_err());
    let mut c = config();
    c.details = Some(DetailsConfig {
        l2_http_url: "http://localhost:8547".into(),
        l2_chain_id: 2,
        l2_genesis_hash: B256::repeat_byte(7),
        beacon_url: "http://localhost:5052".into(),
    });
    c.validate().unwrap();
    c.details.as_mut().unwrap().beacon_url = "ws://localhost:5052".into();
    assert!(c.validate().is_err());
}

#[test]
fn settlement_details_validate_inclusion_ranges_and_blob_bytes() {
    use eez_indexer::details::{ranges, transaction, validate_blobs};
    let block = B256::repeat_byte(4);
    let tx = B256::repeat_byte(5);
    let blob = B256::repeat_byte(1);
    let mut value = json!({"hash":tx,"blockHash":block,"transactionIndex":"0x1","gas":"0x186a0","blobVersionedHashes":[blob]});
    assert_eq!(
        transaction(&value, block, tx, 1, 131072, 21000).unwrap()["blobCount"],
        1
    );
    assert!(transaction(&value, block, tx, 1, 262144, 21000).is_err());
    assert_eq!(
        transaction(&value, block, tx, 1, 131072, 100000).unwrap()["gasLimit"],
        "100000"
    );
    assert!(transaction(&value, block, tx, 1, 131072, 100001).is_err());
    let mut no_gas = value.clone();
    no_gas.as_object_mut().unwrap().remove("gas");
    assert!(transaction(&no_gas, block, tx, 1, 131072, 21000).is_err());
    value["blockHash"] = json!(B256::ZERO);
    assert!(transaction(&value, block, tx, 1, 131072, 21000).is_err());
    let first = B256::repeat_byte(6);
    let last = B256::repeat_byte(7);
    let entry = json!({"l1TransactionHash":tx,"l1BlockHash":block,"l1BlockNumber":"0x8","l1TransactionIndex":"0x1","canonicalL2":true,"l2Finalized":true,"l2Range":{"firstBlockNumber":"0xa","lastBlockNumber":"0xb","blockCount":"0x2","firstBlockHash":first,"lastBlockHash":last},"l2Blocks":[{"number":"0xa","hash":first},{"number":"0xb","hash":last}]});
    let mut unrelated = entry.clone();
    unrelated["l1TransactionHash"] = json!(B256::ZERO);
    let parsed = ranges(&json!([unrelated, entry.clone()]), block, tx, 1, 8).unwrap();
    assert_eq!(parsed["items"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["items"][0]["state"], "finalized");
    assert!(ranges(&json!([]), block, tx, 1, 8).is_err());
    let mut bad = entry.clone();
    bad["l2Blocks"][1]["number"] = json!("0xc");
    assert!(ranges(&json!([bad]), block, tx, 1, 8).is_err());
    let mut bad = entry;
    bad["canonicalL2"] = json!(false);
    assert!(ranges(&json!([bad]), block, tx, 1, 8).is_err());
    let full = json!({"data":[format!("0x{}","00".repeat(131072))]});
    assert_eq!(validate_blobs(&full, 131072).unwrap(), 1);
    assert!(validate_blobs(&full, 262144).is_err());
    assert!(validate_blobs(&json!({"data":["0x00"]}), 131072).is_err());
}

#[test]
fn blob_utilization_counts_occupied_stream_and_selects_transaction_blobs() {
    use eez_indexer::details::{blob_usage, select_transaction_blobs};
    // Wire fixture: reversed 31-byte field payload, including meaningful zeros.
    let mut physical = vec![0u8; 131072];
    physical[30] = 2;
    physical[27] = 1;
    assert_eq!(blob_usage(&[physical.clone()]).unwrap(), (5, 126976));
    physical[0] = 1;
    assert!(blob_usage(&[physical.clone()]).is_err());
    physical[0] = 0;
    physical[27] = 2;
    assert!(blob_usage(&[physical]).is_err());
    let mut a = vec![0u8; 131072];
    a[30] = 2;
    let mut b = vec![0u8; 131072];
    b[28] = 1;
    assert_eq!(blob_usage(&[a, b]).unwrap(), (126980, 253952));
    assert!(blob_usage(&[vec![0; 131072]]).is_err());
    assert_eq!(blob_usage(&[]).unwrap(), (0, 0));
    let zero: B256 = "0x01b0761f87b081d5cf10757ccc89f12be355c70e2e29df288b65b30710dcbcd1"
        .parse()
        .unwrap();
    let one: B256 = "0x01f90435cd45d0475cbb0ace7e7e3a672ed2a3a4984ad3e8624d8878725a9c2e"
        .parse()
        .unwrap();
    let commitments = json!([
        format!("0x{}", "00".repeat(48)),
        format!("0x{}", "01".repeat(48))
    ]);
    let full = json!({"data":["0xaa","0xbb"]});
    assert_eq!(
        select_transaction_blobs(&full, &commitments, &[one, zero]).unwrap(),
        vec![vec![0xbb], vec![0xaa]]
    );
    let sidecars = json!({"data":[{"index":"1","blob":"0xbb"},{"index":"0","blob":"0xaa"}]});
    assert_eq!(
        select_transaction_blobs(&sidecars, &commitments, &[one]).unwrap(),
        vec![vec![0xbb]]
    );
    assert!(select_transaction_blobs(&full, &commitments, &[B256::ZERO]).is_err());
    let duplicate = json!({"data":[{"index":"0","blob":"0xaa"},{"index":"0","blob":"0xbb"}]});
    assert!(select_transaction_blobs(&duplicate, &commitments, &[zero]).is_err());
}

#[test]
fn quantities_preserve_256_bits_and_reject_noncanonical_inputs() {
    assert_eq!(
        quantity(&json!(format!("0x{:x}", U256::MAX))).unwrap(),
        U256::MAX
    );
    for value in [
        json!(null),
        json!(12),
        json!("0x"),
        json!("12"),
        json!("0x00"),
        json!("0xgg"),
        json!(format!("0x1{}", "0".repeat(64))),
    ] {
        assert!(quantity(&value).is_err());
    }
}

#[test]
fn receipt_validation_covers_multiple_events_attribution_and_exact_fees() {
    let c = config();
    let tx = B256::repeat_byte(3);
    let block = Block {
        number: 1,
        hash: B256::repeat_byte(4),
        parent: c.genesis_hash,
        timestamp: 10,
        transactions: vec![tx],
    };
    let log = json!({"address":c.registry,"removed":false,"topics":[L2ExecutionPerformed::SIGNATURE_HASH,B256::from(U256::from(1))],"data":B256::repeat_byte(5),"blockHash":block.hash,"blockNumber":"0x1","transactionHash":tx,"transactionIndex":"0x0","logIndex":"0x0"});
    let mut second = log.clone();
    second["logIndex"] = json!("0x1");
    let events = vec![
        Event::parse(&log, &c).unwrap(),
        Event::parse(&second, &c).unwrap(),
    ];
    let mut receipt = json!({"blockHash":block.hash,"blockNumber":"0x1","transactionHash":tx,"transactionIndex":"0x0","status":"0x1","type":"0x3","gasUsed":"0x5208","effectiveGasPrice":"0x3b9aca00","blobGasUsed":"0x20000","blobGasPrice":"0x2","logs":[log,second]});
    let parsed = Receipt::parse(&receipt, &block, &events, &c).unwrap();
    assert_eq!(parsed.execution_fee, U256::from(21_000_000_000_000u64));
    assert_eq!(parsed.blob_fee, U256::from(262144));
    assert!(Receipt::parse(&receipt, &block, &events[..1], &c).is_err());
    for (field, value) in [
        ("status", json!("0x0")),
        ("blockHash", json!(B256::ZERO)),
        ("transactionHash", json!(B256::ZERO)),
        ("blobGasPrice", json!(null)),
    ] {
        let mut bad = receipt.clone();
        bad[field] = value;
        assert!(Receipt::parse(&bad, &block, &events, &c).is_err());
    }
    receipt["type"] = json!("0x2");
    receipt.as_object_mut().unwrap().remove("blobGasUsed");
    receipt.as_object_mut().unwrap().remove("blobGasPrice");
    assert_eq!(
        Receipt::parse(&receipt, &block, &events, &c)
            .unwrap()
            .blob_fee,
        U256::ZERO
    );
    receipt["gasUsed"] = json!(format!("0x{:x}", U256::MAX));
    assert!(Receipt::parse(&receipt, &block, &events, &c).is_err());
    let mut bad = events[0].clone();
    bad.transaction_index = 1;
    assert!(bad.validate_block(&block).is_err());
    for (field, value) in [
        ("removed", json!(true)),
        ("address", json!(Address::ZERO)),
        ("data", json!("0x01")),
    ] {
        let mut bad = log.clone();
        bad[field] = value;
        assert!(Event::parse(&bad, &c).is_err());
    }
}
