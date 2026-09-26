use alloy_primitives::B256;
use eez_indexer::{admission_receipts::observe, model::Block};
use serde_json::json;

#[test]
fn receipt_identity_position_and_absence_are_separate_from_consumption() {
    let post = B256::repeat_byte(1);
    let source = B256::repeat_byte(2);
    let block = Block {
        number: 10,
        hash: B256::repeat_byte(3),
        parent: B256::repeat_byte(4),
        timestamp: 100,
        transactions: vec![post, source],
    };
    let receipt = json!({"blockNumber":"0xa","blockHash":block.hash,"transactionHash":source,
        "transactionIndex":"0x1","status":"0x1","gasUsed":"0x12345","logs":[]});
    let actual = observe(&block, post, source, Some(&receipt)).unwrap();
    assert_eq!(actual.gas, Some(0x12345));
    assert_eq!(actual.success, Some(true));
    for (field, value) in [
        ("blockHash", json!(B256::ZERO)),
        ("blockNumber", json!("0xb")),
        ("transactionHash", json!(post)),
        ("transactionIndex", json!("0x0")),
        ("status", json!("0x2")),
        ("gasUsed", json!("0x0123")),
    ] {
        let mut bad = receipt.clone();
        bad[field] = value;
        assert!(
            observe(&block, post, source, Some(&bad)).is_err(),
            "{field}"
        );
    }
    assert!(observe(&block, post, source, None).is_err());
    assert!(observe(&block, post, source, Some(&json!(null))).is_err());
    let mut reverted = receipt.clone();
    reverted["status"] = json!("0x0");
    assert_eq!(
        observe(&block, post, source, Some(&reverted))
            .unwrap()
            .success,
        Some(false)
    );
    let absent = observe(&block, post, B256::repeat_byte(8), Some(&receipt)).unwrap();
    assert_eq!(absent.coverage, "absent");
    assert_eq!(absent.gas, None);
    let mut before = block.clone();
    before.transactions.reverse();
    assert_eq!(
        observe(&before, post, source, Some(&receipt))
            .unwrap()
            .coverage,
        "before_postbatch"
    );
    assert!(observe(&block, B256::ZERO, source, None).is_err());
}
