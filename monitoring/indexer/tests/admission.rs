use alloy_primitives::{Address, B256};
use eez_indexer::{admission::parse, config::Config};
use serde_json::{Value, json};

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
fn event(sequence: u64, kind: &str, detail: Value) -> Value {
    json!({"version":1,"candidate_id":"123-456:1:0","chain_id":10200,"registry":Address::repeat_byte(2),
        "rollup_id":1,"l2_parent":B256::repeat_byte(3),"target_block":10,"target_timestamp":100,
        "rebuild_of":null,"policy_version":"shared-deadline-1","estimator_version":"shaped-1",
        "sequence":sequence,"timestamp_ms":1000+sequence,"elapsed_ms":sequence,"kind":kind,"detail":detail})
}
fn line(event: &Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"fields":{"event_name":"eez.composer.admission.v1","admission_event":event.to_string()}})).unwrap()
}
#[test]
fn admission_schema_rejects_wrong_context_and_malformed_gas() {
    let good = event(0, "started", json!({"queued":5,"selected":5}));
    assert_eq!(parse(&line(&good), &config()).unwrap(), Some(good.clone()));
    assert!(
        eez_indexer::postings::parse(&line(&good), &config())
            .unwrap()
            .is_none()
    );
    for (key, value) in [
        ("version", json!(2)),
        ("chain_id", json!(1)),
        ("rollup_id", json!(2)),
        ("candidate_id", json!("bad ID")),
        ("target_timestamp", json!(-1)),
        ("l2_parent", json!(B256::ZERO)),
        ("sequence", json!(u64::MAX)),
        ("kind", json!("success")),
        ("elapsed_ms", json!(-1)),
    ] {
        let mut bad = good.clone();
        bad[key] = value;
        assert!(
            parse(&line(&bad), &config()).is_err(),
            "accepted invalid {key}"
        );
    }
    let mut source = event(
        1,
        "source_estimate",
        json!({"source_hash":B256::repeat_byte(4),"direction":"inbound",
        "gas_gate_accepted":true,"projected_before":200000,"projected_after":500000,"source_sim_gas":50000}),
    );
    assert!(parse(&line(&source), &config()).is_ok());
    source["detail"]["projected_after"] = json!("not a number");
    assert!(parse(&line(&source), &config()).is_err());
    let oversized = event(2, "finished", json!({"extra":"x".repeat(64*1024)}));
    assert!(parse(&line(&oversized), &config()).is_err());
}

#[tokio::test]
#[ignore = "requires a disposable INDEXER_TEST_DATABASE_URL; execute serially with recovery"]
async fn admission_replay_gaps_conflicts_and_dashboard_sql() {
    let settings: tokio_postgres::Config = std::env::var("INDEXER_TEST_DATABASE_URL")
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(settings.get_dbname(), Some("eez_indexer_test"));
    let (mut client, connection) = settings.connect(tokio_postgres::NoTls).await.unwrap();
    let connection = tokio::spawn(connection);
    client
        .batch_execute("DROP SCHEMA IF EXISTS eez_indexer CASCADE")
        .await
        .unwrap();
    client
        .batch_execute(include_str!("../schema.sql"))
        .await
        .unwrap();
    let source = B256::repeat_byte(4);
    let records = [
        event(0, "started", json!({"queued":2,"selected":2})),
        event(
            1,
            "source_selected",
            json!({"source_hash":source,"direction":"inbound","position":0}),
        ),
        event(
            2,
            "source_estimate",
            json!({"source_hash":source,"direction":"inbound","gas_gate_accepted":true,
            "projected_before":200000,"projected_after":500000,"source_sim_gas":50000}),
        ),
        event(
            3,
            "source_accepted",
            json!({"source_hash":source,"position":0,"projected_gas":500000}),
        ),
        event(
            4,
            "summary",
            json!({"accepted":1,"deferred":1,"projected_gas":500000,"planning_cap":1000000}),
        ),
        event(
            5,
            "proof_requested",
            json!({"proof_estimate_ms":125.0,"proof_estimate_basis":"scaled_sample","post_reserve_ms":150.0}),
        ),
        event(6, "finished", json!({})),
    ];
    let batch = eez_indexer::postings::Batch {
        file_id: "admission-test".into(),
        offset: 100,
        anchor: "fixture".into(),
        lines: records.iter().map(line).collect(),
    };
    eez_indexer::postings::import(&mut client, &config(), &batch)
        .await
        .unwrap();
    eez_indexer::postings::import(&mut client, &config(), &batch)
        .await
        .unwrap();
    let row=client.query_one("SELECT admission_gas::bigint,history,canonical_receipt_gas::text FROM eez_indexer.admission_candidates",&[]).await.unwrap();
    assert_eq!(row.get::<_, i64>(0), 500000);
    assert_eq!(row.get::<_, String>(1), "Complete sequence");
    assert!(row.get::<_, Option<String>>(2).is_none());
    assert_eq!(
        client
            .query_one(
                "SELECT accepted_delta_gas::bigint FROM eez_indexer.admission_sources",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        300000
    );
    // A conflicting duplicate cannot advance the shared file checkpoint.
    let mut bad = records[0].clone();
    bad["detail"]["selected"] = json!(99);
    let conflict = eez_indexer::postings::Batch {
        offset: 200,
        lines: vec![line(&bad)],
        ..batch
    };
    assert!(
        eez_indexer::postings::import(&mut client, &config(), &conflict)
            .await
            .is_err()
    );
    assert_eq!(
        client
            .query_one("SELECT byte_offset FROM eez_indexer.posting_files", &[])
            .await
            .unwrap()
            .get::<_, i64>(0),
        100
    );
    client
        .execute(
            "DELETE FROM eez_indexer.admission_events WHERE sequence=2",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        client
            .query_one("SELECT history FROM eez_indexer.admission_candidates", &[])
            .await
            .unwrap()
            .get::<_, String>(0),
        "Missing events"
    );
    // Repricing must join the final signed hash. Reorg removes canonical cost
    // while preserving observations; a rebuilt candidate must not inherit it.
    let old = B256::repeat_byte(5);
    let signed = B256::repeat_byte(6);
    let block = B256::repeat_byte(7);
    let mut postings = Vec::new();
    for (sequence, stage, hash, detail) in [
        (0, "prepared", old, json!({"candidate_id":"123-456:1:0"})),
        (1, "repriced", signed, json!({"candidate_id":"123-456:1:0"})),
        (
            2,
            "dispatched",
            signed,
            json!({"target_block":10,"target_timestamp":100}),
        ),
        (
            3,
            "included",
            signed,
            json!({"l1_block":10,"l1_block_hash":block}),
        ),
    ] {
        let data = json!({"version":1,"attempt_id":"123-456:1:9","posting_id":old,"chain_id":10200,
            "registry":Address::repeat_byte(2),"rollup_id":1,"sync_height":12,"trigger_reasons":["cross_chain"],
            "sequence":sequence,"stage":stage,"transaction_hash":hash,"timestamp_ms":2000+sequence,
            "elapsed_ms":sequence,"dispatch_elapsed_ms":null,"target_block":10,"target_timestamp":100,"detail":detail});
        postings.push(serde_json::to_vec(&json!({"fields":{"event_name":"eez.composer.posting.v1","posting_event":data.to_string()}})).unwrap());
    }
    let linked = eez_indexer::postings::Batch {
        file_id: "linked-test".into(),
        offset: 400,
        anchor: "fixture".into(),
        lines: postings,
    };
    eez_indexer::postings::import(&mut client, &config(), &linked)
        .await
        .unwrap();
    client
        .execute(
            "INSERT INTO eez_indexer.blocks VALUES(10,$1,'parent',100)",
            &[&block.to_string()],
        )
        .await
        .unwrap();
    client
        .execute(
            "INSERT INTO eez_indexer.transactions VALUES($1,$2,0,400000,1,400000,131072,1,131072)",
            &[&block.to_string(), &signed.to_string()],
        )
        .await
        .unwrap();
    client
        .execute(
            "INSERT INTO eez_indexer.settlements VALUES($1,$2,0,'root')",
            &[&block.to_string(), &signed.to_string()],
        )
        .await
        .unwrap();
    assert_eq!(
        client
            .query_one(
                "SELECT canonical_receipt_gas::bigint FROM eez_indexer.admission_candidates",
                &[]
            )
            .await
            .unwrap()
            .get::<_, i64>(0),
        400000
    );
    client
        .execute("DELETE FROM eez_indexer.blocks WHERE height=10", &[])
        .await
        .unwrap();
    assert!(
        client
            .query_one(
                "SELECT canonical_receipt_gas::text FROM eez_indexer.admission_candidates",
                &[]
            )
            .await
            .unwrap()
            .get::<_, Option<String>>(0)
            .is_none()
    );
    let mut rebuilt = event(0, "started", json!({"selected":1,"queued":1}));
    rebuilt["candidate_id"] = json!("123-456:1:1");
    rebuilt["rebuild_of"] = json!("123-456:1:0");
    let rebuilt_batch = eez_indexer::postings::Batch {
        file_id: "rebuilt-test".into(),
        offset: 100,
        anchor: "fixture".into(),
        lines: vec![line(&rebuilt)],
    };
    eez_indexer::postings::import(&mut client, &config(), &rebuilt_batch)
        .await
        .unwrap();
    assert!(client.query_one("SELECT transaction_hash FROM eez_indexer.admission_candidates WHERE candidate_id='123-456:1:1'",&[]).await.unwrap().get::<_,Option<String>>(0).is_none());

    let dashboard: Value =
        serde_json::from_str(include_str!("../dashboards/admission.json")).unwrap();
    for panel in dashboard["panels"].as_array().unwrap() {
        let sql = panel["targets"][0]["rawSql"]
            .as_str()
            .unwrap()
            .replace("${candidate:sqlstring}", "'*'")
            .replace("${source:sqlstring}", "'*'")
            .replace("$__timeFilter(time)", "true")
            .replace("$__timeFilter(c.time)", "true")
            .replace("$__timeFilter(p.time)", "true")
            .replace(
                "$__timeFilter(to_timestamp((data->>'timestamp_ms')::bigint/1000.0))",
                "true",
            );
        let rows = client
            .query(&sql, &[])
            .await
            .unwrap_or_else(|e| panic!("dashboard {}: {e:?}", panel["title"]));
        if panel["id"] == 2 {
            let prediction = rows
                .iter()
                .find(|r| r.get::<_, String>("candidate_id") == "123-456:1:0")
                .unwrap();
            assert_eq!(
                prediction.get::<_, Option<f64>>("proof_estimate_ms"),
                Some(125.0)
            );
            assert_eq!(
                prediction
                    .get::<_, Option<String>>("proof_estimate_basis")
                    .as_deref(),
                Some("scaled_sample")
            );
            let legacy = rows
                .iter()
                .find(|r| r.get::<_, String>("candidate_id") == "123-456:1:1")
                .unwrap();
            assert!(legacy.get::<_, Option<f64>>("proof_estimate_ms").is_none());
        }
    }
    connection.abort();
}
