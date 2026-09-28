use alloy_primitives::{Address, B256};
use axum::{
    Json, Router,
    extract::{State, WebSocketUpgrade, ws::Message},
    response::Response,
    routing::{get, post},
};
use eez_indexer::{
    config::Config,
    heads,
    rpc::{Rpc, notifications},
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

const GENESIS: &str = "0xada44fd8d2ecab8b08f256af07ad3e777f17fb434f8f8e678b312f576212ba9a";
const RAW: &str = include_str!("fixtures/chiado-genesis-header.hex");

#[derive(Clone)]
struct Fixture {
    code: Option<i64>,
    raw: Value,
    calls: Arc<Mutex<Vec<String>>>,
}
fn reply(f: &Fixture, q: &Value) -> Value {
    let method = q["method"].as_str().unwrap();
    f.calls.lock().unwrap().push(method.into());
    let value = match method {
        "eth_chainId" => json!("0x27d8"),
        "eth_getBlockByNumber" => {
            if q["params"][0] == "0x0"
                && let Some(code) = f.code
            {
                return json!({"jsonrpc":"2.0","id":1,"error":{"code":code,"message":"fixture"}});
            }
            let n = if q["params"][0] == "0x0" {
                "0x0"
            } else {
                "0x1"
            };
            json!({"number":n,"hash":GENESIS,"parentHash":B256::ZERO,"timestamp":"0x1","transactions":[]})
        }
        "debug_getRawHeader" => f.raw.clone(),
        "eth_subscribe" => json!("fixture"),
        other => panic!("unexpected method {other}"),
    };
    json!({"jsonrpc":"2.0","id":1,"result":value})
}
async fn http(State(f): State<Fixture>, Json(q): Json<Value>) -> Json<Value> {
    Json(reply(&f, &q))
}
async fn ws(State(f): State<Fixture>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |mut socket| async move {
        while let Some(Ok(Message::Text(text))) = socket.recv().await {
            let q: Value = serde_json::from_str(&text).unwrap();
            if socket
                .send(Message::Text(reply(&f, &q).to_string().into()))
                .await
                .is_err()
            {
                return;
            }
        }
    })
}
async fn server(code: Option<i64>, raw: Value) -> (Config, Fixture, tokio::task::JoinHandle<()>) {
    let f = Fixture {
        code,
        raw,
        calls: Default::default(),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/", post(http))
        .route("/ws", get(ws))
        .with_state(f.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        Config {
            http_url: format!("http://{address}"),
            ws_url: format!("ws://{address}/ws"),
            chain_id: 10200,
            genesis_hash: GENESIS.parse().unwrap(),
            registry: Address::repeat_byte(1),
            rollup_id: 1,
            start_block: 1,
            details: None,
            posting_logs_dir: None,
        },
        f,
        task,
    )
}

#[tokio::test]
async fn pruned_http_identity_and_heads_verify_retained_genesis_header() {
    let (mut config, f, server) = server(Some(4444), json!(RAW.trim())).await;
    let rpc = Rpc::new(&config.http_url).unwrap();
    rpc.identity(&config).await.unwrap();
    assert_eq!(
        heads::collect(&rpc, config.chain_id, config.genesis_hash)
            .await
            .unwrap()["latest"],
        1
    );
    config.genesis_hash = B256::ZERO;
    assert!(
        rpc.identity(&config)
            .await
            .unwrap_err()
            .to_string()
            .contains("genesis mismatch")
    );
    assert_eq!(
        f.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|m| *m == "debug_getRawHeader")
            .count(),
        3
    );
    server.abort();
}

#[tokio::test]
async fn full_genesis_and_unrelated_errors_do_not_use_raw_fallback() {
    for code in [None, Some(-32601), Some(-32000)] {
        let (config, f, server) = server(code, json!(RAW.trim())).await;
        assert_eq!(
            Rpc::new(&config.http_url)
                .unwrap()
                .identity(&config)
                .await
                .is_ok(),
            code.is_none()
        );
        assert!(
            !f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|m| m == "debug_getRawHeader")
        );
        server.abort();
    }
}

#[tokio::test]
async fn pruned_genesis_rejects_missing_malformed_oversized_and_wrong_headers() {
    for raw in [
        Value::Null,
        json!("0x"),
        json!("0xzz"),
        json!("0x00"),
        json!(format!("0x{}", "00".repeat(8193))),
    ] {
        let (config, f, server) = server(Some(4444), raw).await;
        assert!(
            Rpc::new(&config.http_url)
                .unwrap()
                .identity(&config)
                .await
                .is_err()
        );
        assert!(
            f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|m| m == "debug_getRawHeader")
        );
        server.abort();
    }
}

#[tokio::test]
async fn pruned_websocket_checks_genesis_before_waking_scanner() {
    for valid in [true, false] {
        let (mut config, f, server) = server(Some(4444), json!(RAW.trim())).await;
        if !valid {
            config.genesis_hash = B256::ZERO;
        }
        let (tx, mut rx) = tokio::sync::watch::channel(0);
        let task = tokio::spawn(notifications(config, tx));
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(300), rx.changed())
                .await
                .is_ok(),
            valid
        );
        assert!(
            f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|m| m == "debug_getRawHeader")
        );
        assert_eq!(
            f.calls.lock().unwrap().iter().any(|m| m == "eth_subscribe"),
            valid
        );
        task.abort();
        server.abort();
    }
}
