use alloy_primitives::{B256, U256};
use axum::{Json, Router, extract::State, routing::post};
use eez_indexer::l1_fees::{collect_reference, reference, split};
use eez_indexer::rpc::Rpc;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn header() -> Value {
    json!({"number":"0x3","hash":B256::with_last_byte(3),"parentHash":B256::with_last_byte(2),"timestamp":"0x1","baseFeePerGas":"0xa","transactions":[]})
}
fn history() -> Value {
    json!({"oldestBlock":"0x0","gasUsedRatio":[0.0,0.4,0.7],"reward":[["0x0"],["0x2"],["0x6"]]})
}
#[test]
fn split_preserves_receipt_cost_and_rejects_invalid_identity_price_and_overflow() {
    let h = header();
    let b = B256::with_last_byte(3);
    let fee = split(&h, b, 3, U256::from(100), U256::from(12)).unwrap();
    assert_eq!(fee["baseCostWei"], "1000");
    assert_eq!(fee["tipCostWei"], "200");
    assert_eq!(fee["tipPerGasWei"], "2");
    assert!(split(&h, B256::ZERO, 3, U256::from(100), U256::from(12)).is_err());
    assert!(split(&h, b, 2, U256::from(100), U256::from(12)).is_err());
    assert!(split(&h, b, 3, U256::from(100), U256::from(9)).is_err());
    assert!(split(&h, b, 3, U256::MAX, U256::from(12)).is_err());
}
#[test]
fn median_excludes_empty_blocks_but_keeps_nonempty_zero_tip_blocks() {
    let mut h = history();
    let sample = reference(&h, 2, 3).unwrap();
    assert_eq!(sample["medianTipWei"], "4");
    assert_eq!(sample["sampleBlocks"], 2);
    h["gasUsedRatio"][0] = json!(0.1);
    assert_eq!(reference(&h, 2, 3).unwrap()["medianTipWei"], "2");
    h["gasUsedRatio"] = json!([0, 0, 0]);
    assert!(reference(&h, 2, 3).unwrap()["medianTipWei"].is_null());
    assert!(reference(&history(), 3, 3).is_err());
    assert!(reference(&history(), 2, 0).is_err());
    h = history();
    h["reward"] = json!([["0x1"]]);
    assert!(reference(&h, 2, 3).is_err());
    h = history();
    h["gasUsedRatio"][0] = json!(-1);
    assert!(reference(&h, 2, 3).is_err());
}
#[tokio::test]
async fn historical_rpc_window_is_pinned_and_reorg_is_rejected() {
    let calls = Arc::new(AtomicUsize::new(0));
    async fn handle(State(calls): State<Arc<AtomicUsize>>, Json(q): Json<Value>) -> Json<Value> {
        let result = match q["method"].as_str().unwrap() {
            "eth_getBlockByNumber" => {
                let mut h = header();
                if calls.fetch_add(1, Ordering::SeqCst) >= 2 {
                    h["hash"] = json!(B256::ZERO);
                }
                h
            }
            "eth_feeHistory" => {
                assert_eq!(q["params"], json!(["0x3", "0x2", [50]]));
                history()
            }
            other => panic!("unexpected {other}"),
        };
        Json(json!({"jsonrpc":"2.0","id":1,"result":result}))
    }
    let app = Router::new()
        .route("/", post(handle))
        .with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rpc = Rpc::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    assert_eq!(
        collect_reference(&rpc, 3, B256::with_last_byte(3))
            .await
            .unwrap()["medianTipWei"],
        "4"
    );
    calls.store(1, Ordering::SeqCst);
    assert!(
        collect_reference(&rpc, 3, B256::with_last_byte(3))
            .await
            .is_err()
    );
    task.abort();
}
