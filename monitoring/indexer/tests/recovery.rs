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
        "data":format!("{}{}", B256::repeat_byte(30+i as u8+branch*10), "00".repeat(32)),"blockHash":block_hash(2,branch),"blockNumber":"0x2",
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
        posting_logs_dir: None,
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
        "eth_getBlockReceipts" => {
            if s.bad_receipt {
                Value::Null
            } else {
                json!((0..2).map(|i| {
                    let tx = transaction(i, s.branch);
                    let logs = (0..if s.branch == 0 { 3 } else { 1 })
                        .map(|j| log(j, s.branch))
                        .filter(|l| l["transactionHash"] == json!(tx))
                        .collect::<Vec<_>>();
                    json!({"blockHash":block_hash(2,s.branch),"blockNumber":"0x2","transactionHash":tx,"transactionIndex":format!("0x{i:x}"),"status":"0x1","type":"0x3","gasUsed":"0xa","effectiveGasPrice":"0x2","blobGasUsed":"0x3","blobGasPrice":"0x4","logs":logs})
                }).collect::<Vec<_>>())
            }
        }
        method => panic!("unexpected method {method}"),
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
    admin.execute("INSERT INTO eez_indexer.details(block_hash,transaction_hash,data,retry_at) SELECT block_hash,transaction_hash,CASE WHEN transaction_index=0 THEN '{\"version\":5}'::jsonb ELSE '{\"version\":6}'::jsonb END,'infinity'::timestamptz FROM eez_indexer.transactions", &[]).await.unwrap();
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
    let upgrade = admin.query_one("SELECT count(*) FILTER(WHERE retry_at < 'infinity'), count(*) FILTER(WHERE data->>'version'='6' AND retry_at='infinity') FROM eez_indexer.details", &[]).await.unwrap();
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
    // Off-chain history is idempotent and commits its cursor atomically.
    {
        use eez_indexer::postings::{Batch, import};
        let event = json!({"version":1,"attempt_id":"123-456:1:0","posting_id":B256::repeat_byte(3),
            "chain_id":cfg.chain_id,"registry":cfg.registry,"rollup_id":cfg.rollup_id,"sync_height":10,
            "trigger_reasons":["cross_chain"],"sequence":0,"stage":"prepared",
            "transaction_hash":transaction(0,1),"timestamp_ms":1000,"elapsed_ms":0.0,
            "dispatch_elapsed_ms":null,"detail":{}});
        let line = |e: &Value| {
            format!("{}\n",json!({"fields":{"event_name":"eez.composer.posting.v1","posting_event":e.to_string()}})).into_bytes()
        };
        let mut batch = Batch {
            file_id: "fixture".into(),
            offset: 100,
            anchor: "fixture".into(),
            lines: vec![line(&event)],
        };
        import(&mut db.client, &cfg, &batch).await.unwrap();
        import(&mut db.client, &cfg, &batch).await.unwrap();
        assert_eq!(
            db.client
                .query_one("SELECT count(*) FROM eez_indexer.posting_events", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            1
        );
        let mut conflicting = event.clone();
        conflicting["timestamp_ms"] = json!(2000);
        batch.lines = vec![line(&conflicting)];
        batch.offset = 200;
        assert!(import(&mut db.client, &cfg, &batch).await.is_err());
        assert_eq!(
            db.client
                .query_one(
                    "SELECT byte_offset FROM eez_indexer.posting_files WHERE file_id='fixture'",
                    &[]
                )
                .await
                .unwrap()
                .get::<_, i64>(0),
            100
        );
        let mut next = event.clone();
        next["sequence"] = json!(1);
        next["stage"] = json!("repriced");
        batch.lines = vec![line(&next), b"invalid JSON\n".to_vec()];
        assert!(import(&mut db.client, &cfg, &batch).await.is_err());
        assert_eq!(
            db.client
                .query_one("SELECT count(*) FROM eez_indexer.posting_events", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            1
        );
        next["posting_id"] = json!(B256::repeat_byte(99));
        batch.lines = vec![line(&next)];
        assert!(import(&mut db.client, &cfg, &batch).await.is_err());
        let mut priced = event.clone();
        priced["sequence"] = json!(1);
        priced["stage"] = json!("repriced");
        let mut dispatched = event.clone();
        dispatched["sequence"] = json!(2);
        dispatched["stage"] = json!("dispatched");
        dispatched["elapsed_ms"] = json!(25.0);
        dispatched["timestamp_ms"] = json!(100500);
        dispatched["dispatch_elapsed_ms"] = json!(0.0);
        dispatched["detail"] = json!({"target_block":2,"target_timestamp":102});
        let mut included = event.clone();
        included["sequence"] = json!(3);
        included["stage"] = json!("included");
        included["elapsed_ms"] = json!(4025.0);
        included["timestamp_ms"] = json!(104500);
        included["dispatch_elapsed_ms"] = json!(4000.0);
        included["detail"] = json!({"l1_block":2,"l1_block_hash":block_hash(2,1)});
        // Rotated files can arrive out of order. Preserve the gap until replay fills it.
        batch.lines = vec![line(&included)];
        import(&mut db.client, &cfg, &batch).await.unwrap();
        assert_eq!(
            db.client
                .query_one("SELECT history FROM eez_indexer.posting_attempts", &[])
                .await
                .unwrap()
                .get::<_, String>(0),
            "Missing events"
        );
        batch.lines = vec![line(&priced), line(&dispatched)];
        import(&mut db.client, &cfg, &batch).await.unwrap();
        let row=db.client.query_one("SELECT history,chain_status,preparation_ms,observation_ms FROM eez_indexer.posting_attempts", &[]).await.unwrap();
        assert_eq!(row.get::<_, String>(0), "Observed sequence intact");
        assert_eq!(row.get::<_, String>(1), "Canonical commitment");
        assert_eq!(row.get::<_, f64>(2), 25.0);
        assert_eq!(row.get::<_, f64>(3), 4000.0);
        let timing = db.client.query_one("SELECT target_l1_block,inclusion_block_delta,dispatch_vs_target_seconds::float8,inclusion_vs_target_seconds,receipt_vs_target_seconds::float8,receipt_observation_seconds::float8,target_result FROM eez_indexer.posting_timing", &[]).await.unwrap();
        assert_eq!(timing.get::<_, i64>(0), 2);
        assert_eq!(timing.get::<_, i64>(1), 0);
        assert_eq!(timing.get::<_, f64>(2), -1.5);
        assert_eq!(timing.get::<_, f64>(3), 0.0);
        assert_eq!(timing.get::<_, f64>(4), 2.5);
        assert_eq!(timing.get::<_, f64>(5), 2.5);
        assert_eq!(timing.get::<_, String>(6), "Expected block");
        // Exact canonical headers distinguish a produced slot from a proven
        // skipped timestamp; missing history is not evidence of a network skip.
        for (mode, expected) in [
            (0, "Block produced without inclusion"),
            (1, "Network slot skipped"),
            (2, "Awaiting canonical evidence"),
        ] {
            let tx = db.client.transaction().await.unwrap();
            tx.execute("UPDATE eez_indexer.posting_events SET data=jsonb_set(data,'{transaction_hash}',to_jsonb($1::text))", &[&B256::repeat_byte(88).to_string()]).await.unwrap();
            tx.execute("UPDATE eez_indexer.posting_events SET data=jsonb_set(data,'{stage}','\"excluded\"'::jsonb) WHERE data->>'stage'='included'", &[]).await.unwrap();
            if mode > 0 {
                tx.execute(
                    "UPDATE eez_indexer.blocks SET timestamp=104 WHERE height=2",
                    &[],
                )
                .await
                .unwrap();
            }
            if mode == 2 {
                tx.execute("DELETE FROM eez_indexer.blocks WHERE height=1", &[])
                    .await
                    .unwrap();
            }
            let row = tx
                .query_one(
                    "SELECT outcome,explanation FROM eez_indexer.posting_diagnostics",
                    &[],
                )
                .await
                .unwrap();
            assert_eq!(row.get::<_, String>(0), expected);
            if mode == 0 {
                assert!(row.get::<_, String>(1).contains("reason unknown"));
            }
            tx.rollback().await.unwrap();
        }
        let tx = db.client.transaction().await.unwrap();
        tx.execute(
            "DELETE FROM eez_indexer.posting_events WHERE sequence>0",
            &[],
        )
        .await
        .unwrap();
        tx.execute("UPDATE eez_indexer.posting_events SET data=data || jsonb_build_object('stage','preparation_failed','transaction_hash',null,'target_block',2,'target_timestamp',102,'detail',jsonb_build_object('reason','preparation_deadline','preparation_ms',501))", &[]).await.unwrap();
        let row = tx.query_one("SELECT outcome,history,transaction_hash,target_l1_block,dispatch_vs_target_seconds::float8 FROM eez_indexer.posting_diagnostics", &[]).await.unwrap();
        assert_eq!(row.get::<_, String>(0), "Preparation failed");
        assert_eq!(row.get::<_, String>(1), "Observed sequence intact");
        assert!(row.get::<_, Option<String>>(2).is_none());
        assert_eq!(row.get::<_, i64>(3), 2);
        assert!(row.get::<_, Option<f64>>(4).is_none());
        tx.rollback().await.unwrap();
        // Different target heights must not be confused with receipt-observation delay.
        for (target, timestamp, result, delta, seconds) in [
            (1_i64, 97_i64, "Later block", 1_i64, 5.0_f64),
            (3, 107, "Earlier block", -1, -5.0),
        ] {
            let tx = db.client.transaction().await.unwrap();
            tx.execute("UPDATE eez_indexer.posting_events SET data=jsonb_set(data,'{detail}',jsonb_build_object('target_block',$1::bigint,'target_timestamp',$2::bigint)) WHERE data->>'stage'='dispatched'", &[&target,&timestamp]).await.unwrap();
            let row = tx.query_one("SELECT target_result,inclusion_block_delta,inclusion_vs_target_seconds FROM eez_indexer.posting_timing", &[]).await.unwrap();
            assert_eq!(row.get::<_, String>(0), result);
            assert_eq!(row.get::<_, i64>(1), delta);
            assert_eq!(row.get::<_, f64>(2), seconds);
            tx.rollback().await.unwrap();
        }
        // A stale receipt must not measure observation of a different canonical block.
        let tx = db.client.transaction().await.unwrap();
        tx.execute("UPDATE eez_indexer.posting_events SET data=jsonb_set(data,'{detail,l1_block_hash}',to_jsonb($1::text)) WHERE data->>'stage'='included'", &[&block_hash(2, 99).to_string()]).await.unwrap();
        let row = tx.query_one("SELECT inclusion_vs_target_seconds,receipt_vs_target_seconds::float8,receipt_observation_seconds::float8 FROM eez_indexer.posting_timing", &[]).await.unwrap();
        assert_eq!(row.get::<_, f64>(0), 0.0);
        assert!(row.get::<_, Option<f64>>(1).is_none());
        assert!(row.get::<_, Option<f64>>(2).is_none());
        tx.rollback().await.unwrap();
        // Older logs without a target timestamp still support a block-height comparison.
        let tx = db.client.transaction().await.unwrap();
        tx.execute("UPDATE eez_indexer.posting_events SET data=data #- '{detail,target_timestamp}' WHERE data->>'stage'='dispatched'", &[]).await.unwrap();
        let row = tx
            .query_one(
                "SELECT target_result,inclusion_vs_target_seconds FROM eez_indexer.posting_timing",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(row.get::<_, String>(0), "Expected block");
        assert!(row.get::<_, Option<f64>>(1).is_none());
        tx.rollback().await.unwrap();
        // Missing dispatch history has no guessed target or timing, even with a receipt.
        let tx = db.client.transaction().await.unwrap();
        tx.execute(
            "DELETE FROM eez_indexer.posting_events WHERE data->>'stage'='dispatched'",
            &[],
        )
        .await
        .unwrap();
        let row = tx.query_one("SELECT target_result,inclusion_block_delta,dispatch_vs_target_seconds::float8 FROM eez_indexer.posting_timing", &[]).await.unwrap();
        assert_eq!(row.get::<_, String>(0), "Target unavailable");
        assert!(row.get::<_, Option<i64>>(1).is_none());
        assert!(row.get::<_, Option<f64>>(2).is_none());
        tx.rollback().await.unwrap();
        let dashboard: Value =
            serde_json::from_str(include_str!("../../grafana/dashboards/composer-queue.json"))
                .unwrap();
        let panel = dashboard["panels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == 17)
            .unwrap();
        let sql = panel["targets"][0]["rawSql"]
            .as_str()
            .unwrap()
            .replace("$__timeFilter(p.time)", "true");
        let rows = db.client.query(&sql, &[]).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].get::<_, String>("Trigger reasons"),
            "Cross-chain work"
        );
        assert_eq!(
            rows[0].get::<_, String>("Latest stage"),
            "Included at target"
        );
        assert_eq!(rows[0].get::<_, i64>("L1 block"), 2);
        let panel = dashboard["panels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == 16)
            .unwrap();
        let sql = panel["targets"][0]["rawSql"]
            .as_str()
            .unwrap()
            .replace("$__timeFilter(p.time)", "true")
            .replace(
                "$__timeGroupAlias(p.time, '$__interval')",
                "to_timestamp(floor(extract(epoch FROM p.time)/60)*60) AS time",
            );
        let rows = db.client.query(&sql, &[]).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<_, String>("Trigger"), "Cross-chain work");
        assert_eq!(
            rows[0].get::<_, String>("Attempts in range"),
            "1/1 (100.0%)"
        );
        assert_eq!(
            rows[0].get::<_, String>("Included at target"),
            "1/1 (100.0%)"
        );

        let panel = dashboard["panels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == 18)
            .unwrap();
        let sql = panel["targets"][0]["rawSql"]
            .as_str()
            .unwrap()
            .replace("$__timeFilter(p.expected_l1_time)", "true");
        let rows = db.client.query(&sql, &[]).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<_, f64>("Prepared"), -101.0);
        assert_eq!(rows[0].get::<_, f64>("Dispatch"), -1.5);
        assert_eq!(rows[0].get::<_, f64>("Canonical inclusion"), 0.0);
        assert_eq!(rows[0].get::<_, f64>("Receipt observed"), 2.5);

        // Outcome markers and details share the captured target time, without
        // aggregating different attempts that happen to target the same slot.
        let outcome_sql = panel["targets"][1]["rawSql"]
            .as_str()
            .unwrap()
            .replace("$__timeFilter(p.expected_l1_time)", "true");
        assert_eq!(dashboard["annotations"]["list"][0]["enable"], false);
        assert_eq!(dashboard["annotations"]["list"][0]["hide"], true);
        let detail_sql = dashboard["annotations"]["list"][0]["target"]["rawSql"]
            .as_str()
            .unwrap()
            .replace("$__timeFilter(p.expected_l1_time)", "true");
        let tx = db.client.transaction().await.unwrap();
        tx.execute("INSERT INTO eez_indexer.posting_events(attempt_id,sequence,data) SELECT '2:1:99',sequence,jsonb_set(data,'{attempt_id}',to_jsonb('2:1:99'::text)) FROM eez_indexer.posting_events", &[]).await.unwrap();
        let outcomes = tx.query(&outcome_sql, &[]).await.unwrap();
        assert_eq!(outcomes.len(), 2);
        for row in &outcomes {
            assert_eq!(row.get::<_, String>("metric"), "Included at target");
            assert_eq!(row.get::<_, f64>("value"), 1.0);
        }
        let details = tx.query(&detail_sql, &[]).await.unwrap();
        assert_eq!(details.len(), 2);
        assert_ne!(
            details[0].get::<_, String>("text"),
            details[1].get::<_, String>("text")
        );
        for row in &details {
            assert!(
                row.get::<_, String>("text")
                    .contains("Expected L1 block: 2")
            );
        }
        tx.rollback().await.unwrap();

        // First seen can be a later stage when logs are incomplete; never call it Prepared.
        let tx = db.client.transaction().await.unwrap();
        tx.execute(
            "DELETE FROM eez_indexer.posting_events WHERE data->>'stage'='prepared'",
            &[],
        )
        .await
        .unwrap();
        let rows = tx.query(&sql, &[]).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].get::<_, Option<f64>>("Prepared").is_none());
        assert_eq!(rows[0].get::<_, f64>("Dispatch"), -1.5);
        tx.rollback().await.unwrap();

        // Exercise the actual optional worker, not only the import helper.
        let logs = std::env::temp_dir().join(format!("eez-posting-worker-{}", std::process::id()));
        std::fs::create_dir(&logs).unwrap();
        std::fs::write(logs.join("posting.log"), line(&event)).unwrap();
        let mut worker_cfg = cfg.clone();
        worker_cfg.posting_logs_dir = Some(logs.clone());
        worker_cfg.validate().unwrap();
        let worker = tokio::spawn(eez_indexer::postings::run(worker_cfg, url.clone()));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let row = db
                    .client
                    .query_one(
                        "SELECT count(*) FROM eez_indexer.posting_files WHERE file_id <> 'fixture'",
                        &[],
                    )
                    .await
                    .unwrap();
                if row.get::<_, i64>(0) == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        worker.abort();
        let _ = worker.await;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if db
                    .client
                    .query_one("SELECT pg_try_advisory_lock(519763212)", &[])
                    .await
                    .unwrap()
                    .get::<_, bool>(0)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        db.client
            .query_one("SELECT pg_advisory_unlock(519763212)", &[])
            .await
            .unwrap();
        std::fs::remove_dir_all(logs).unwrap();
        db.rewind(1).await.unwrap();
        assert_eq!(
            db.client
                .query_one("SELECT count(*) FROM eez_indexer.posting_events", &[])
                .await
                .unwrap()
                .get::<_, i64>(0),
            4
        );
    }
    assert_eq!(
        db.client
            .query_one("SELECT chain_status FROM eez_indexer.posting_attempts", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "Not in indexed canonical history"
    );
    let row = db.client.query_one("SELECT inclusion_block_delta,inclusion_vs_target_seconds,receipt_vs_target_seconds::float8,receipt_observation_seconds::float8 FROM eez_indexer.posting_timing", &[]).await.unwrap();
    assert!(row.get::<_, Option<i64>>(0).is_none());
    for column in 1..4 {
        assert!(row.get::<_, Option<f64>>(column).is_none());
    }
    // The latest card must skip newer non-posting blocks/transactions, retain all
    // updates of the selected posting, and fall back when its block is reorged out.
    {
        let dashboard: Value =
            serde_json::from_str(include_str!("../dashboards/settlements.json")).unwrap();
        let sql = dashboard["panels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == 1)
            .unwrap()["targets"][0]["rawSql"]
            .as_str()
            .unwrap();
        let tx = db.client.transaction().await.unwrap();
        tx.batch_execute("INSERT INTO eez_indexer.blocks(height,hash,parent_hash,timestamp) VALUES
            (100,'query-old','parent',1000),(101,'query-new','query-old',1005),(102,'query-tip','query-new',1010);
            INSERT INTO eez_indexer.transactions(block_hash,transaction_hash,transaction_index,gas_used,gas_price_wei,execution_fee_wei,blob_gas_used,blob_gas_price_wei,blob_fee_wei) VALUES
            ('query-old','old-posting',0,1,1,1,131072,1,1),
            ('query-new','first-posting',0,1,1,1,131072,1,1),
            ('query-new','last-posting',1,1,1,1,131072,1,1),
            ('query-new','later-nonposting',2,1,1,1,0,0,0),
            ('query-tip','newer-nonposting',0,1,1,1,0,0,0);
            INSERT INTO eez_indexer.settlements(block_hash,transaction_hash,log_index,l2_block_hash) VALUES
            ('query-old','old-posting',0,'l2-old'),
            ('query-new','first-posting',0,'l2-first'),
            ('query-new','last-posting',1,'l2-middle'),
            ('query-new','last-posting',2,'l2-last'),
            ('query-new','later-nonposting',3,'l2-nonposting'),
            ('query-tip','newer-nonposting',0,'l2-tip');").await.unwrap();
        let row = tx.query_one(sql, &[]).await.unwrap();
        assert_eq!(row.get::<_, String>("transaction_hash"), "last-posting");
        assert_eq!(row.get::<_, i64>("L1 block"), 101);
        assert_eq!(row.get::<_, i64>("State updates"), 2);
        tx.execute("DELETE FROM eez_indexer.blocks WHERE height=101", &[])
            .await
            .unwrap();
        let row = tx.query_one(sql, &[]).await.unwrap();
        assert_eq!(row.get::<_, String>("transaction_hash"), "old-posting");
        assert_eq!(row.get::<_, i64>("State updates"), 1);
        tx.execute("UPDATE eez_indexer.transactions SET blob_gas_used=0", &[])
            .await
            .unwrap();
        assert!(tx.query(sql, &[]).await.unwrap().is_empty());
        tx.rollback().await.unwrap();
    }
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
