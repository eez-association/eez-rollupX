use alloy_primitives::{B256, U256};
use axum::{Json, Router, extract::State, routing::post};
use eez_indexer::{
    l2_fees::{block_fees, collect},
    rpc::Rpc,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn header(n: u64) -> Value {
    json!({"number":format!("0x{n:x}"),"hash":B256::with_last_byte(n as u8),"parentHash":B256::with_last_byte(n as u8-1),"timestamp":"0x1","baseFeePerGas":"0xa","gasUsed":"0x5","transactions":[B256::with_last_byte(10),B256::with_last_byte(11)]})
}
fn receipts(n: u64) -> Value {
    json!((0..2).map(|i| json!({"blockNumber":format!("0x{n:x}"),"blockHash":B256::with_last_byte(n as u8),"transactionHash":B256::with_last_byte(10+i),"transactionIndex":format!("0x{i:x}"),"status":format!("0x{i:x}"),"gasUsed":if i==0 {"0x2"} else {"0x3"},"cumulativeGasUsed":if i==0 {"0x2"} else {"0x5"},"effectiveGasPrice":if i==0 {"0xc"} else {"0xf"}})).collect::<Vec<_>>())
}
#[test]
fn exact_fees_include_reverts_and_reject_incomplete_or_wrong_receipts() {
    let b = header(1);
    let r = receipts(1);
    let (burn, tip) = block_fees(&b, &r).unwrap();
    assert_eq!(burn.to_string(), "50");
    assert_eq!(tip.to_string(), "19");
    for (field, value) in [
        ("blockHash", json!(B256::ZERO)),
        ("transactionHash", json!(B256::ZERO)),
        ("transactionIndex", json!("0x1")),
        ("effectiveGasPrice", json!("0x9")),
        ("gasUsed", json!("0x4")),
        ("status", json!("0x2")),
    ] {
        let mut bad = r.clone();
        bad[0][field] = value;
        assert!(block_fees(&b, &bad).is_err());
    }
    assert!(block_fees(&b, &json!([r[0]])).is_err());
    assert!(block_fees(&b, &json!([r[0], r[0]])).is_err());
    let mut empty = b.clone();
    empty["transactions"] = json!([]);
    empty["gasUsed"] = json!("0x0");
    assert_eq!(
        block_fees(&empty, &json!([])).unwrap(),
        (U256::ZERO, U256::ZERO)
    );
    empty["gasUsed"] = json!("0x1");
    assert!(block_fees(&empty, &json!([])).is_err());
    let mut huge = b;
    huge["baseFeePerGas"] = json!(format!("0x{}", "f".repeat(64)));
    let mut r = r;
    r[0]["effectiveGasPrice"] = huge["baseFeePerGas"].clone();
    assert!(block_fees(&huge, &r).is_err());
}

#[derive(Default)]
struct Mock {
    missing: AtomicBool,
    requests: AtomicUsize,
}
async fn handler(State(s): State<Arc<Mock>>, Json(request): Json<Value>) -> Json<Value> {
    s.requests.fetch_add(1, Ordering::SeqCst);
    let n = u64::from_str_radix(
        request["params"][0]
            .as_str()
            .unwrap()
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let result = match request["method"].as_str().unwrap() {
        "eth_getBlockByNumber" => header(n),
        "eth_getBlockReceipts" if s.missing.load(Ordering::SeqCst) => json!([]),
        "eth_getBlockReceipts" => receipts(n),
        other => panic!("unexpected {other}"),
    };
    Json(json!({"jsonrpc":"2.0","id":1,"result":result}))
}
#[tokio::test]
async fn range_collection_resumes_without_double_counting_and_caches_complete_results() {
    let state = Arc::new(Mock::default());
    let app = Router::new()
        .route("/", post(handler))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rpc = Rpc::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let ranges = json!({"items":[{"first":1,"last":2,"count":2,"firstHash":B256::with_last_byte(1),"lastHash":B256::with_last_byte(2),"state":"finalized"}]});
    let first = collect(&rpc, &ranges, &Value::Null, 1).await;
    assert_eq!(first["status"], "pending");
    assert_eq!(first["items"][0]["processed"], 1);
    // A stored checkpoint survives serialization/restart and a receipt outage.
    let saved: Value = serde_json::from_str(&first.to_string()).unwrap();
    state.missing.store(true, Ordering::SeqCst);
    let failed = collect(&rpc, &ranges, &saved, 1).await;
    assert_eq!(failed["items"][0]["processed"], 1);
    assert_eq!(failed["items"][0]["burnWei"], "50");
    state.missing.store(false, Ordering::SeqCst);
    let complete = collect(&rpc, &ranges, &failed, 1).await;
    assert_eq!(complete["status"], "available");
    assert_eq!(complete["items"][0]["burnWei"], "100");
    assert_eq!(complete["items"][0]["tipWei"], "38");
    let requests = state.requests.load(Ordering::SeqCst);
    assert_eq!(collect(&rpc, &ranges, &complete, 1).await, complete);
    assert_eq!(state.requests.load(Ordering::SeqCst), requests);
    let mut stale = saved;
    stale["items"][0]["lastHash"] = json!(B256::ZERO);
    let reset = collect(&rpc, &ranges, &stale, 1).await;
    assert_eq!(reset["status"], "pending");
    assert_eq!(reset["items"][0]["processed"], 0);
    let mut nonfinal = ranges;
    nonfinal["items"][0]["state"] = json!("canonical");
    assert_eq!(
        collect(&rpc, &nonfinal, &complete, 1).await["status"],
        "pending"
    );
    task.abort();
}
