use alloy_primitives::{Address, B256};
use eez_indexer::{
    config::Config,
    postings::{parse, read_batch},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::Write,
    sync::atomic::{AtomicU64, Ordering},
};

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
fn event() -> Value {
    json!({"version":1,"attempt_id":"123-456:1:0","posting_id":B256::repeat_byte(3),
        "chain_id":10200,"registry":Address::repeat_byte(2),"rollup_id":1,"sync_height":10,
        "trigger_reasons":["cross_chain"],"sequence":0,"stage":"prepared",
        "transaction_hash":B256::repeat_byte(4),"timestamp_ms":1000,"elapsed_ms":0.0,
        "dispatch_elapsed_ms":null,"detail":{}})
}
fn line(event: &Value) -> Vec<u8> {
    format!(
        "{}\n",
        json!({"fields":{"event_name":"eez.composer.posting.v1","posting_event":event.to_string()}})
    )
    .into_bytes()
}

#[test]
fn schema_filters_unrelated_logs_and_rejects_invalid_events() {
    let cfg = config();
    assert!(
        parse(b"{\"fields\":{\"message\":\"other\"}}", &cfg)
            .unwrap()
            .is_none()
    );
    assert_eq!(parse(&line(&event()), &cfg).unwrap(), Some(event()));
    for (key, value) in [
        ("version", json!(2)),
        ("rollup_id", json!(2)),
        ("chain_id", json!(1)),
        ("registry", json!(Address::repeat_byte(9))),
        ("stage", json!("success")),
        ("sequence", json!(999)),
        ("elapsed_ms", json!(-1)),
        ("transaction_hash", json!("bad")),
        ("attempt_id", json!("")),
        ("trigger_reasons", json!(["invented"])),
    ] {
        let mut bad = event();
        bad[key] = value;
        assert!(parse(&line(&bad), &cfg).is_err(), "accepted invalid {key}");
    }
    for stage in [
        "repriced",
        "dispatched",
        "included",
        "excluded",
        "uncertain",
        "preflight_failed",
    ] {
        let mut e = event();
        e["stage"] = json!(stage);
        e["detail"] = json!({"target_block":20,"target_timestamp":200,"l1_block":20,"l1_block_hash":B256::repeat_byte(5)});
        assert!(parse(&line(&e), &cfg).is_ok());
    }
    let mut included = event();
    included["stage"] = json!("included");
    assert!(parse(&line(&included), &cfg).is_err());
}

#[test]
fn retained_files_resume_across_partial_writes_rename_rotation_and_restart() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "eez-posting-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join("posting.log");
    let first = line(&event());
    std::fs::write(&path, &first[..10]).unwrap();
    assert!(read_batch(&path, &HashMap::new()).unwrap().is_none());
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&first[10..])
        .unwrap();
    let batch = read_batch(&path, &HashMap::new()).unwrap().unwrap();
    assert_eq!(batch.lines.len(), 1);
    let checkpoints =
        HashMap::from([(batch.file_id.clone(), (batch.offset, batch.anchor.clone()))]);
    assert!(
        read_batch(&path, &checkpoints)
            .unwrap()
            .unwrap()
            .lines
            .is_empty()
    );
    let mut next = event();
    next["sequence"] = json!(1);
    next["stage"] = json!("repriced");
    let next = line(&next);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&next[..20])
        .unwrap();
    assert_eq!(
        read_batch(&path, &checkpoints).unwrap().unwrap().offset,
        batch.offset
    );
    let rotated = dir.join("posting.log.1");
    std::fs::rename(&path, &rotated).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&rotated)
        .unwrap()
        .write_all(&next[20..])
        .unwrap();
    let resumed = read_batch(&rotated, &checkpoints).unwrap().unwrap();
    assert_eq!(resumed.file_id, batch.file_id);
    assert_eq!(resumed.lines, vec![next]);
    std::fs::write(&path, &first).unwrap();
    let new = read_batch(&path, &checkpoints).unwrap().unwrap();
    assert_ne!(new.file_id, batch.file_id);
    assert_eq!(new.lines.len(), 1);
    let checkpoints = HashMap::from([(resumed.file_id, (resumed.offset, resumed.anchor))]);
    std::fs::write(&rotated, &first).unwrap();
    assert!(read_batch(&rotated, &checkpoints).is_err());
    std::fs::write(&path, vec![b'x'; 65537]).unwrap();
    assert!(read_batch(&path, &HashMap::new()).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn collection_is_opt_in_and_bad_paths_fail_before_startup() {
    let mut cfg = config();
    cfg.validate().unwrap();
    for path in ["", "relative", "/definitely-missing-eez-posting-directory"] {
        cfg.posting_logs_dir = Some(path.into());
        assert!(cfg.validate().is_err());
    }
    cfg.posting_logs_dir = Some(std::env::temp_dir());
    cfg.validate().unwrap();
}

#[test]
fn preparation_failures_and_early_targets_are_validated_without_fabricated_transactions() {
    let cfg = config();
    let mut e = event();
    e["target_block"] = json!(20);
    e["target_timestamp"] = json!(200);
    assert_eq!(parse(&line(&e), &cfg).unwrap(), Some(e.clone()));
    let mut dispatched = e.clone();
    dispatched["stage"] = json!("dispatched");
    dispatched["detail"] = json!({"target_block":20,"target_timestamp":200});
    assert!(parse(&line(&dispatched), &cfg).is_ok());
    dispatched["detail"]["target_timestamp"] = json!(205);
    assert!(parse(&line(&dispatched), &cfg).is_err());
    e["stage"] = json!("preparation_failed");
    e["transaction_hash"] = Value::Null;
    e["detail"] = json!({"reason":"preparation_deadline","preparation_ms":501.0});
    assert_eq!(parse(&line(&e), &cfg).unwrap(), Some(e.clone()));
    for (field, value) in [
        ("sequence", json!(1)),
        ("target_block", json!(0)),
        ("target_timestamp", json!(-1)),
        ("transaction_hash", json!(B256::repeat_byte(4))),
        ("stage", json!("prepared")),
    ] {
        let mut bad = e.clone();
        bad[field] = value;
        assert!(parse(&line(&bad), &cfg).is_err(), "accepted {field}");
    }
    let mut bad = e.clone();
    bad.as_object_mut().unwrap().remove("target_timestamp");
    assert!(parse(&line(&bad), &cfg).is_err());
    for (key, value) in [("reason", json!("invented")), ("preparation_ms", json!(-1))] {
        let mut bad = e.clone();
        bad["detail"][key] = value;
        assert!(parse(&line(&bad), &cfg).is_err());
    }
}
