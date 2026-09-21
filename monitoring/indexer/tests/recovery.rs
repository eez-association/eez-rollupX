use alloy_primitives::{Address, B256, U256};
use alloy_sol_types::SolEvent;
use axum::{
    Json, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
    routing::{get, post},
};
use eez_indexer::{
    config::Config,
    database::Database,
    model::L2ExecutionPerformed,
    rpc::{Rpc, notifications},
    sync,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Chain {
    branch: u8,
    head: u64,
    bad_receipt: bool,
    switch_in_logs: bool,
    wrong_chain: bool,
    ws_sessions: u32,
}
type Shared = Arc<Mutex<Chain>>;
fn block_hash(n: u64, branch: u8) -> B256 {
    B256::from(U256::from(
        n + 1 + if n >= 2 { u64::from(branch) * 100 } else { 0 },
    ))
}
fn transaction(i: u8, branch: u8) -> B256 {
    B256::repeat_byte(10 + i + branch * 10)
}
fn log(i: u64, branch: u8) -> Value {
    json!({"address":Address::repeat_byte(2),"removed":false,"topics":[L2ExecutionPerformed::SIGNATURE_HASH,B256::from(U256::from(1))],
        "data":B256::repeat_byte(30+i as u8+branch*10),"blockHash":block_hash(2,branch),"blockNumber":"0x2",
        "transactionHash":transaction(if i<2 {0} else {1},branch),"transactionIndex":if i<2 {"0x0"} else {"0x1"},"logIndex":format!("0x{i:x}")})
}
fn block(n: u64, branch: u8) -> Value {
    json!({"number":format!("0x{n:x}"),"hash":block_hash(n,branch),"parentHash":if n==0 {B256::ZERO} else {block_hash(n-1,branch)},
        "timestamp":format!("0x{:x}",100+n),"transactions":if n==2 {vec![transaction(0,branch),transaction(1,branch)]} else {vec![]}})
}
fn config(url: &str) -> Config {
    Config {
        http_url: url.into(),
        ws_url: url.replacen("http", "ws", 1) + "/ws",
        chain_id: 10200,
        genesis_hash: block_hash(0, 0),
        registry: Address::repeat_byte(2),
        rollup_id: 1,
        start_block: 1,
        details: None,
    }
}

async fn handler(State(state): State<Shared>, Json(request): Json<Value>) -> Json<Value> {
    let mut s = state.lock().unwrap();
    let value = match request["method"].as_str().unwrap() {
        "eth_chainId" => json!(if s.wrong_chain { "0x1" } else { "0x27d8" }),
        "eth_blockNumber" => json!(format!("0x{:x}", s.head)),
        "eth_getBlockByNumber" => {
            let n = u64::from_str_radix(
                request["params"][0]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("0x"),
                16,
            )
            .unwrap();
            if n > s.head {
                Value::Null
            } else {
                block(n, s.branch)
            }
        }
        "eth_getLogs" => {
            let from = u64::from_str_radix(
                request["params"][0]["fromBlock"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("0x"),
                16,
            )
            .unwrap();
            let to = u64::from_str_radix(
                request["params"][0]["toBlock"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("0x"),
                16,
            )
            .unwrap();
            let result = if from <= 2 && to >= 2 {
                (0..if s.branch == 0 { 3 } else { 1 })
                    .map(|i| log(i, s.branch))
                    .collect::<Vec<_>>()
            } else {
                vec![]
            };
            if s.switch_in_logs {
                s.branch = 1;
                s.switch_in_logs = false;
            }
            json!(result)
        }
        "eth_getTransactionReceipt" => {
            if s.bad_receipt {
                Value::Null
            } else {
                let tx: B256 = serde_json::from_value(request["params"][0].clone()).unwrap();
                let logs = (0..if s.branch == 0 { 3 } else { 1 })
                    .map(|i| log(i, s.branch))
                    .filter(|l| l["transactionHash"] == json!(tx))
                    .collect::<Vec<_>>();
                json!({"blockHash":block_hash(2,s.branch),"blockNumber":"0x2","transactionHash":tx,"transactionIndex":if tx==transaction(0,s.branch) {"0x0"} else {"0x1"},"status":"0x1","type":"0x3","gasUsed":"0xa","effectiveGasPrice":"0x2","blobGasUsed":"0x3","blobGasPrice":"0x4","logs":logs})
            }
        }
        _ => panic!("unexpected method"),
    };
    Json(json!({"jsonrpc":"2.0","id":request["id"],"result":value}))
}

async fn ws(State(state): State<Shared>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| ws_session(socket, state))
}
async fn ws_session(mut socket: WebSocket, state: Shared) {
    state.lock().unwrap().ws_sessions += 1;
    while let Some(Ok(Message::Text(text))) = socket.recv().await {
        let request: Value = serde_json::from_str(&text).unwrap();
        let value = match request["method"].as_str().unwrap() {
            "eth_chainId" => json!(if state.lock().unwrap().wrong_chain {
                "0x1"
            } else {
                "0x27d8"
            }),
            "eth_getBlockByNumber" => block(0, 0),
            "eth_subscribe" => json!("fixture"),
            _ => panic!("unexpected WS method"),
        };
        if socket
            .send(Message::Text(
                json!({"jsonrpc":"2.0","id":1,"result":value})
                    .to_string()
                    .into(),
            ))
            .await
            .is_err()
        {
            return;
        }
        if request["method"] == "eth_subscribe" {
            let _=socket.send(Message::Text(json!({"jsonrpc":"2.0","method":"eth_subscription","params":{"subscription":"fixture","result":{"number":"0x2"}}}).to_string().into())).await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    }
}

async fn server(state: Shared) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/", post(handler))
        .route("/ws", get(ws))
        .with_state(state);
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, task)
}

#[tokio::test]
async fn websocket_reconnects_and_rejects_wrong_chain_without_waking_scanner() {
    let state = Arc::new(Mutex::new(Chain {
        head: 2,
        ..Default::default()
    }));
    let (url, server) = server(state.clone()).await;
    let (tx, mut rx) = tokio::sync::watch::channel(0);
    let task = tokio::spawn(notifications(config(&url), tx));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            rx.changed().await.unwrap();
            if state.lock().unwrap().ws_sessions >= 2 {
                break;
            }
        }
    })
    .await
    .unwrap();
    task.abort();
    task.await.unwrap_err();
    state.lock().unwrap().wrong_chain = true;
    let (tx, mut rx) = tokio::sync::watch::channel(0);
    let task = tokio::spawn(notifications(config(&url), tx));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), rx.changed())
            .await
            .is_err()
    );
    task.abort();
    server.abort();
}

#[tokio::test]
#[ignore = "requires a disposable INDEXER_TEST_DATABASE_URL; run explicitly with --ignored"]
async fn postgres_restart_atomicity_duplicates_reorg_and_identity() {
    let url = std::env::var("INDEXER_TEST_DATABASE_URL").expect("dedicated test database required");
    let settings: tokio_postgres::Config = url.parse().unwrap();
    assert_eq!(
        settings.get_dbname(),
        Some("eez_indexer_test"),
        "test must never target a deployment database"
    );
    let (admin, connection) = settings.connect(tokio_postgres::NoTls).await.unwrap();
    let conn = tokio::spawn(connection);
    admin
        .batch_execute("DROP SCHEMA IF EXISTS eez_indexer CASCADE")
        .await
        .unwrap();
    let state = Arc::new(Mutex::new(Chain {
        head: 2,
        bad_receipt: true,
        ..Default::default()
    }));
    let (http, server) = server(state.clone()).await;
    let cfg = config(&http);
    let rpc = Rpc::new(&http).unwrap();
    let mut db = Database::connect(&url, &cfg).await.unwrap();
    assert!(
        Database::connect(&url, &cfg).await.is_err(),
        "only one writer"
    );
    assert!(sync(&cfg, &rpc, &mut db).await.is_err());
    assert_eq!(
        db.cursor().await.unwrap().unwrap().0,
        1,
        "failed receipt must not checkpoint block 2"
    );
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM eez_indexer.transactions", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    state.lock().unwrap().bad_receipt = false;
    assert!(!sync(&cfg, &rpc, &mut db).await.unwrap());
    assert!(
        !sync(&cfg, &rpc, &mut db).await.unwrap(),
        "duplicate heads do not duplicate work"
    );
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM eez_indexer.settlements", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        3
    );
    let row=admin.query_one("SELECT count(*),sum(execution_fee_wei)::text,sum(blob_fee_wei)::text FROM eez_indexer.transactions",&[]).await.unwrap();
    assert_eq!(row.get::<_, i64>(0), 2);
    assert_eq!(row.get::<_, String>(1), "40");
    assert_eq!(row.get::<_, String>(2), "24");
    admin.execute("INSERT INTO eez_indexer.details(block_hash,transaction_hash,data,retry_at) SELECT block_hash,transaction_hash,CASE WHEN transaction_index=0 THEN '{\"version\":2}'::jsonb ELSE '{\"version\":3}'::jsonb END,'infinity'::timestamptz FROM eez_indexer.transactions", &[]).await.unwrap();
    // Exercise database rollback after insertion has begun (missing receipt FK).
    let mut bad = eez_indexer::model::Event::parse(&log(0, 0), &cfg).unwrap();
    let b = eez_indexer::model::Block::parse(&block(3, 0)).unwrap();
    bad.block_number = 3;
    bad.block_hash = b.hash;
    assert!(db.store(&b, &[bad], &[], 1).await.is_err());
    assert_eq!(db.cursor().await.unwrap().unwrap().0, 2);
    drop(db);
    // Wait for the dropped client's driver to release the session advisory lock.
    let mut db = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match Database::connect(&url, &cfg).await {
                Ok(db) => break db,
                Err(_) => tokio::task::yield_now().await,
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(db.cursor().await.unwrap().unwrap().0, 2);
    let upgrade = admin.query_one("SELECT count(*) FILTER(WHERE retry_at < 'infinity'), count(*) FILTER(WHERE data->>'version'='3' AND retry_at='infinity') FROM eez_indexer.details", &[]).await.unwrap();
    assert_eq!(
        upgrade.get::<_, i64>(0),
        1,
        "upgrade requeues old enrichment without resetting canonical history"
    );
    assert_eq!(
        upgrade.get::<_, i64>(1),
        1,
        "complete current-version enrichment stays cached"
    );
    {
        let mut s = state.lock().unwrap();
        s.branch = 1;
        s.head = 3;
    }
    sync(&cfg, &rpc, &mut db).await.unwrap();
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM eez_indexer.settlements", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        1
    );
    assert_eq!(db.cursor().await.unwrap(), Some((3, block_hash(3, 1))));
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM eez_indexer.details", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0,
        "reorg removes the replaced branch's enrichment"
    );
    state.lock().unwrap().head = 1;
    sync(&cfg, &rpc, &mut db).await.unwrap();
    assert_eq!(db.cursor().await.unwrap().unwrap().0, 1);
    assert_eq!(
        admin
            .query_one("SELECT count(*) FROM eez_indexer.transactions", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        0
    );
    {
        let mut s = state.lock().unwrap();
        s.head = 2;
        s.branch = 0;
        s.switch_in_logs = true;
    }
    assert!(
        sync(&cfg, &rpc, &mut db).await.is_err(),
        "reorg during log retrieval cannot checkpoint an incomplete block"
    );
    assert_eq!(db.cursor().await.unwrap().unwrap().0, 1);
    sync(&cfg, &rpc, &mut db).await.unwrap();
    state.lock().unwrap().wrong_chain = true;
    assert!(sync(&cfg, &rpc, &mut db).await.is_err());
    drop(db);
    let mut wrong = cfg.clone();
    wrong.rollup_id = 2;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match Database::connect(&url, &wrong).await {
                Err(error) if error.to_string().contains("another eez-indexer writer") => {
                    tokio::task::yield_now().await;
                }
                Err(error) => {
                    assert_eq!(error.to_string(), "database source/configuration mismatch");
                    break;
                }
                Ok(_) => panic!("incompatible database identity accepted"),
            }
        }
    })
    .await
    .unwrap();
    server.abort();
    drop(admin);
    conn.abort();
}
