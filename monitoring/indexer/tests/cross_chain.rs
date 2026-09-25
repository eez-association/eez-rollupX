use alloy_primitives::{Address, B256, U256, address, keccak256};
use axum::{Json, Router, extract::State, routing::post};
use eez_indexer::{
    cross_chain::{collect, inbound, outbound},
    model::Block,
    rpc::Rpc,
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
const MANAGER: Address = address!("4200000000000000000000000000000000000007");
const REGISTRY: Address = address!("1111111111111111111111111111111111111111");
fn hash(n: u64) -> B256 {
    B256::from(U256::from(n))
}
fn header(n: u64) -> Value {
    json!({"number":format!("0x{n:x}"),"hash":hash(n),"parentHash":hash(n-1),"timestamp":"0x1","transactions":if n==1 {vec![hash(10001),hash(10002)]}else{vec![]}})
}
fn log(tx: u64, index: u64, position: u64, emitter: Address, topics: Vec<B256>) -> Value {
    json!({"address":emitter,"topics":topics,"data":"0x","blockHash":hash(1),"blockNumber":"0x1","transactionHash":hash(tx),"transactionIndex":format!("0x{index:x}"),"logIndex":format!("0x{position:x}"),"removed":false})
}
fn consumed(tx: u64, index: u64, position: u64) -> Value {
    log(
        tx,
        index,
        position,
        MANAGER,
        vec![
            keccak256("ExecutionConsumed(bytes32,uint256)"),
            hash(20),
            hash(position),
        ],
    )
}
fn receipt(tx: u64, index: u64, logs: Vec<Value>) -> Value {
    json!({"transactionHash":hash(tx),"transactionIndex":format!("0x{index:x}"),"blockHash":hash(1),"blockNumber":"0x1","status":"0x1","logs":logs})
}
#[test]
fn unique_inbound_source_ignores_outbound_entries_other_rollups_and_foreign_emitters() {
    let block = Block::parse(&header(1)).unwrap();
    let event = |p, key, rid, addr| {
        log(
            10001,
            0,
            p,
            addr,
            vec![
                keccak256("ExecutionConsumed(bytes32,uint64,uint256)"),
                hash(key),
                hash(rid),
                hash(p),
            ],
        )
    };
    let r = receipt(
        10001,
        0,
        vec![event(0, 20, 1, REGISTRY), event(1, 20, 1, REGISTRY)],
    );
    assert!(inbound(&r, &block, hash(10001), REGISTRY, 1).unwrap());
    for l in [
        event(0, 0, 1, REGISTRY),
        event(0, 20, 2, REGISTRY),
        event(0, 20, 1, MANAGER),
    ] {
        assert!(
            !inbound(
                &receipt(10001, 0, vec![l]),
                &block,
                hash(10001),
                REGISTRY,
                1
            )
            .unwrap()
        );
    }
    let mut bad = r.clone();
    bad["status"] = json!("0x0");
    assert!(inbound(&bad, &block, hash(10001), REGISTRY, 1).is_err());
    let mut bad = r.clone();
    bad["logs"][1]["logIndex"] = json!("0x0");
    assert!(inbound(&bad, &block, hash(10001), REGISTRY, 1).is_err());
    let mut bad = r;
    bad["logs"][0]["blockHash"] = json!(B256::ZERO);
    assert!(inbound(&bad, &block, hash(10001), REGISTRY, 1).is_err());
}
fn incoming() -> Value {
    log(
        10002,
        1,
        3,
        MANAGER,
        vec![
            keccak256(
                "IncomingCrossChainCallExecuted(bytes32,bool,address,uint64,address,uint256,uint64,bytes)",
            ),
            hash(21),
        ],
    )
}
#[test]
fn outbound_counts_source_once_and_excludes_inbound_delivery_callbacks() {
    let block = Block::parse(&header(1)).unwrap();
    assert!(
        outbound(
            &receipt(10001, 0, vec![consumed(10001, 0, 0), consumed(10001, 0, 1)]),
            &block,
            hash(10001)
        )
        .unwrap()
    );
    assert!(
        !outbound(
            &receipt(10002, 1, vec![consumed(10002, 1, 2), incoming()]),
            &block,
            hash(10002)
        )
        .unwrap()
    );
    assert!(!outbound(&receipt(10001, 0, vec![]), &block, hash(10001)).unwrap());
    let mut foreign = consumed(10001, 0, 0);
    foreign["address"] = json!(REGISTRY);
    assert!(!outbound(&receipt(10001, 0, vec![foreign]), &block, hash(10001)).unwrap());
    let mut bad = receipt(10001, 0, vec![consumed(10001, 0, 0)]);
    bad["transactionIndex"] = json!("0x1");
    assert!(outbound(&bad, &block, hash(10001)).is_err());
}
#[derive(Default)]
struct Mock {
    fail: AtomicBool,
    calls: AtomicUsize,
    reorg_after_logs: AtomicBool,
    changed: AtomicBool,
}
async fn handle(State(s): State<Arc<Mock>>, Json(q): Json<Value>) -> Json<Value> {
    s.calls.fetch_add(1, Ordering::SeqCst);
    let result = match q["method"].as_str().unwrap() {
        "eth_getBlockByNumber" => {
            let height = u64::from_str_radix(
                q["params"][0].as_str().unwrap().trim_start_matches("0x"),
                16,
            )
            .unwrap();
            let mut block = header(height);
            if height == 9000 && s.changed.load(Ordering::SeqCst) {
                block["hash"] = json!(hash(9001));
            }
            block
        }
        "eth_getLogs" => {
            if s.reorg_after_logs.load(Ordering::SeqCst) {
                s.changed.store(true, Ordering::SeqCst);
            }
            let start = u64::from_str_radix(
                q["params"][0]["fromBlock"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("0x"),
                16,
            )
            .unwrap();
            assert_eq!(q["params"][0]["address"], json!(MANAGER));
            if start > 1 && s.fail.load(Ordering::SeqCst) {
                return Json(
                    json!({"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"unavailable"}}),
                );
            }
            if start == 1 {
                json!([
                    consumed(10001, 0, 0),
                    consumed(10001, 0, 1),
                    consumed(10002, 1, 2)
                ])
            } else {
                json!([])
            }
        }
        "eth_getTransactionReceipt" => {
            if q["params"][0] == json!(hash(10001)) {
                receipt(10001, 0, vec![consumed(10001, 0, 0), consumed(10001, 0, 1)])
            } else {
                receipt(10002, 1, vec![consumed(10002, 1, 2), incoming()])
            }
        }
        other => panic!("unexpected {other}"),
    };
    Json(json!({"jsonrpc":"2.0","id":1,"result":result}))
}
#[tokio::test]
async fn chunked_history_recovers_without_duplicates_and_rejects_changed_ranges() {
    let state = Arc::new(Mock::default());
    let app = Router::new()
        .route("/", post(handle))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rpc = Rpc::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let ranges = json!({"items":[{"first":1,"last":9000,"count":9000,"firstHash":hash(1),"lastHash":hash(9000),"state":"finalized"}]});
    state.fail.store(true, Ordering::SeqCst);
    let failed = collect(&rpc, &ranges, &Value::Null).await;
    assert_eq!(failed["status"], "pending");
    assert_eq!(failed["items"][0]["processed"], 2048);
    assert_eq!(failed["items"][0]["transactions"], json!([hash(10001)]));
    state.fail.store(false, Ordering::SeqCst);
    let restored: Value = serde_json::from_str(&failed.to_string()).unwrap();
    let complete = collect(&rpc, &ranges, &restored).await;
    assert_eq!(complete["status"], "available");
    assert_eq!(complete["items"][0]["transactions"], json!([hash(10001)]));
    let calls = state.calls.load(Ordering::SeqCst);
    assert_eq!(collect(&rpc, &ranges, &complete).await, complete);
    assert_eq!(state.calls.load(Ordering::SeqCst), calls);
    let mut changed = ranges.clone();
    changed["items"][0]["lastHash"] = json!(B256::ZERO);
    let rejected = collect(&rpc, &changed, &complete).await;
    assert_eq!(rejected["status"], "pending");
    assert_eq!(rejected["items"][0]["transactions"], json!([]));
    state.reorg_after_logs.store(true, Ordering::SeqCst);
    let raced = collect(&rpc, &ranges, &Value::Null).await;
    assert_eq!(raced["status"], "pending");
    assert_eq!(raced["items"][0]["processed"], 0);
    assert_eq!(raced["items"][0]["transactions"], json!([]));
    assert_eq!(
        raced["items"][0]["error"],
        "cross-chain range changed during collection"
    );
    state.reorg_after_logs.store(false, Ordering::SeqCst);
    state.changed.store(false, Ordering::SeqCst);
    let mut canonical = ranges.clone();
    canonical["items"][0]["state"] = json!("canonical");
    let partial = collect(&rpc, &canonical, &Value::Null).await;
    assert_eq!(partial["status"], "pending");
    let ready = collect(&rpc, &canonical, &partial).await;
    assert_eq!(ready["status"], "available");
    assert_eq!(ready["items"][0]["transactions"], json!([hash(10001)]));
    let calls = state.calls.load(Ordering::SeqCst);
    assert_eq!(collect(&rpc, &canonical, &ready).await, ready);
    assert!(state.calls.load(Ordering::SeqCst) > calls);
    let promoted = collect(&rpc, &ranges, &ready).await;
    assert_eq!(promoted["status"], "available");
    assert_eq!(promoted["items"][0]["range"]["state"], "finalized");
    state.changed.store(true, Ordering::SeqCst);
    assert_eq!(collect(&rpc, &canonical, &ready).await["status"], "pending");
    assert_eq!(collect(&rpc, &ranges, &ready).await["status"], "pending");
    canonical["items"][0]["state"] = json!("non-canonical");
    assert_eq!(collect(&rpc, &canonical, &ready).await["status"], "pending");
    task.abort();
}
