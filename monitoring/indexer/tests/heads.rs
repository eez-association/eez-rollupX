use alloy_primitives::{B256, U256};
use axum::{Json, Router, extract::State, routing::post};
use eez_indexer::{heads::collect, rpc::Rpc};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
fn hash(n: u64) -> B256 {
    B256::from(U256::from(n + 100))
}
async fn handle(State(mode): State<Arc<AtomicU8>>, Json(q): Json<Value>) -> Json<Value> {
    let mode = mode.load(Ordering::SeqCst);
    let value = match q["method"].as_str().unwrap() {
        "eth_chainId" => json!(if mode == 1 { "0x2" } else { "0x1" }),
        "eth_getBlockByNumber" => {
            let tag = q["params"][0].as_str().unwrap();
            let n = match tag {
                "latest" => 10,
                "safe" => {
                    if mode == 2 {
                        11
                    } else {
                        8
                    }
                }
                "finalized" => 6,
                _ => u64::from_str_radix(tag.trim_start_matches("0x"), 16).unwrap(),
            };
            if mode == 3 && tag == "safe" {
                Value::Null
            } else {
                json!({"number":format!("0x{n:x}"),"hash":hash(if mode==4 && tag=="0x8"{99}else{n}),"parentHash":hash(n.saturating_sub(1)),"timestamp":"0x1","transactions":[]})
            }
        }
        m => panic!("unexpected {m}"),
    };
    Json(json!({"jsonrpc":"2.0","id":1,"result":value}))
}
#[tokio::test]
async fn heads_validate_identity_order_availability_and_canonical_branch() {
    let mode = Arc::new(AtomicU8::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rpc = Rpc::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let app = Router::new()
        .route("/", post(handle))
        .with_state(mode.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let heads = collect(&rpc, 1, hash(0)).await.unwrap();
    assert_eq!(heads["latest"], 10);
    assert_eq!(heads["safe"], 8);
    assert_eq!(heads["finalized"], 6);
    assert_eq!(heads["unsettled"], 2);
    assert_eq!(heads["safeHash"], json!(hash(8)));
    assert!(
        collect(&rpc, 1, B256::ZERO)
            .await
            .unwrap_err()
            .to_string()
            .contains("identity")
    );
    for (value, expected) in [
        (1, "identity"),
        (2, "ordering"),
        (3, "missing hex quantity"),
        (4, "branch changed"),
    ] {
        mode.store(value, Ordering::SeqCst);
        assert!(
            collect(&rpc, 1, hash(0))
                .await
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
    }
    mode.store(0, Ordering::SeqCst);
    assert_eq!(collect(&rpc, 1, hash(0)).await.unwrap(), heads);
    task.abort();
}
