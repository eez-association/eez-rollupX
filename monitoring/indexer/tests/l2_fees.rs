use alloy_primitives::{Address, B256, U256};
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
    json!({"number":format!("0x{n:x}"),"hash":B256::with_last_byte(n as u8),"parentHash":B256::with_last_byte(n as u8-1),"timestamp":"0x1","miner":Address::repeat_byte(n as u8),"baseFeePerGas":"0xa","gasUsed":"0x5","transactions":[B256::with_last_byte(10),B256::with_last_byte(11)]})
}
fn receipts(n: u64) -> Value {
    json!((0..2).map(|i| json!({"blockNumber":format!("0x{n:x}"),"blockHash":B256::with_last_byte(n as u8),"transactionHash":B256::with_last_byte(10+i),"transactionIndex":format!("0x{i:x}"),"type":if i==0 {"0x0"} else {"0x2"},"status":format!("0x{i:x}"),"gasUsed":if i==0 {"0x2"} else {"0x3"},"cumulativeGasUsed":if i==0 {"0x2"} else {"0x5"},"effectiveGasPrice":if i==0 {"0xc"} else {"0xf"}})).collect::<Vec<_>>())
}
#[test]
fn native_system_receipts_meter_gas_without_charging_fees() {
    let b = header(1);
    let mut r = receipts(1);
    r[0]["type"] = json!("0x76");
    r[0]["effectiveGasPrice"] = json!("0x0");

    // Both transactions contribute to cumulative/block gas. Only the ordinary
    // transaction pays the positive base fee and the beneficiary's priority fee.
    let fees = block_fees(&b, &r).unwrap();
    assert_eq!(fees.burn, U256::from(30));
    assert_eq!(fees.tip, U256::from(15));
    assert_eq!(fees.ordinary_gas_used, 3);
    assert_eq!(fees.native_gas_used, 2);
    assert_eq!(fees.beneficiary, Address::repeat_byte(1));

    r[0]["effectiveGasPrice"] = json!("0xc");
    assert_eq!(
        block_fees(&b, &r).unwrap_err().to_string(),
        "nonzero native L2 transaction price"
    );
}

#[test]
fn native_only_blocks_have_gas_but_no_fee_revenue() {
    let mut b = header(1);
    let mut r = receipts(1);
    for receipt in r.as_array_mut().unwrap() {
        receipt["type"] = json!("0x76");
        receipt["effectiveGasPrice"] = json!("0x0");
    }
    for base in ["0x0", "0xa"] {
        b["baseFeePerGas"] = json!(base);
        let fees = block_fees(&b, &r).unwrap();
        assert_eq!(fees.ordinary_gas_used, 0);
        assert_eq!(fees.native_gas_used, 5);
        assert_eq!(fees.burn, U256::ZERO);
        assert_eq!(fees.tip, U256::ZERO);
    }
    r[1]["cumulativeGasUsed"] = json!("0x4");
    assert_eq!(
        block_fees(&b, &r).unwrap_err().to_string(),
        "L2 cumulative gas mismatch"
    );
}

#[test]
fn missing_or_unknown_types_cannot_claim_native_fee_exemption() {
    let b = header(1);
    let mut r = receipts(1);
    r[0]["effectiveGasPrice"] = json!("0x0");
    for (kind, message) in [
        (json!("0x0"), "L2 effective price below base fee"),
        (json!("0x7f"), "unsupported L2 receipt type"),
        (json!("0x076"), "noncanonical quantity"),
        (Value::Null, "missing hex quantity"),
    ] {
        r[0]["type"] = kind;
        assert_eq!(block_fees(&b, &r).unwrap_err().to_string(), message);
    }
    let mut b = b;
    b.as_object_mut().unwrap().remove("miner");
    assert_eq!(
        block_fees(&b, &receipts(1)).unwrap_err().to_string(),
        "missing L2 fee beneficiary"
    );
}

#[test]
fn exact_fees_include_reverts_and_reject_incomplete_or_wrong_receipts() {
    let b = header(1);
    let r = receipts(1);
    let fees = block_fees(&b, &r).unwrap();
    assert_eq!(fees.burn.to_string(), "50");
    assert_eq!(fees.tip.to_string(), "19");
    assert_eq!(fees.ordinary_gas_used, 5);
    assert_eq!(fees.native_gas_used, 0);
    let mut zero_recipient = b.clone();
    zero_recipient["miner"] = json!(Address::ZERO);
    let fees = block_fees(&zero_recipient, &r).unwrap();
    assert_eq!(fees.beneficiary, Address::ZERO);
    assert_eq!(fees.tip.to_string(), "19");
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
    let fees = block_fees(&empty, &json!([])).unwrap();
    assert_eq!(fees.burn, U256::ZERO);
    assert_eq!(fees.tip, U256::ZERO);
    assert_eq!(fees.ordinary_gas_used + fees.native_gas_used, 0);
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
    reorg: AtomicBool,
    native: AtomicBool,
    same_beneficiary: AtomicBool,
    receipts: AtomicUsize,
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
        "eth_getBlockByNumber" => {
            let mut h = header(n);
            if s.reorg.load(Ordering::SeqCst) {
                h["hash"] = json!(B256::ZERO);
            }
            if s.same_beneficiary.load(Ordering::SeqCst) {
                h["miner"] = json!(Address::repeat_byte(1));
            }
            h
        }
        "eth_getBlockReceipts" if s.missing.load(Ordering::SeqCst) => json!([]),
        "eth_getBlockReceipts" => {
            s.receipts.fetch_add(1, Ordering::SeqCst);
            let mut r = receipts(n);
            if s.native.load(Ordering::SeqCst) {
                r[0]["type"] = json!("0x76");
                r[0]["effectiveGasPrice"] = json!("0x0");
            }
            r
        }
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
    let mut premature = saved.clone();
    premature["items"][0]["status"] = json!("available");
    let incomplete = collect(&rpc, &ranges, &premature, 0).await;
    assert_eq!(incomplete["status"], "pending");
    assert_eq!(incomplete["items"][0]["processed"], 1);
    state.missing.store(true, Ordering::SeqCst);
    let failed = collect(&rpc, &ranges, &saved, 1).await;
    assert_eq!(failed["items"][0]["processed"], 1);
    assert_eq!(failed["items"][0]["burnWei"], "50");
    state.missing.store(false, Ordering::SeqCst);
    let complete = collect(&rpc, &ranges, &failed, 1).await;
    assert_eq!(complete["status"], "available");
    assert_eq!(complete["items"][0]["burnWei"], "100");
    assert_eq!(complete["items"][0]["tipWei"], "38");
    assert_eq!(complete["items"][0]["ordinaryGasUsed"], "10");
    assert_eq!(complete["items"][0]["nativeGasUsed"], "0");
    assert_eq!(
        complete["items"][0]["tipsByBeneficiary"],
        json!({format!("{:#x}", Address::repeat_byte(1)):"19", format!("{:#x}", Address::repeat_byte(2)):"19"})
    );
    let requests = state.requests.load(Ordering::SeqCst);
    assert_eq!(collect(&rpc, &ranges, &complete, 1).await, complete);
    assert_eq!(state.requests.load(Ordering::SeqCst), requests);
    let mut stale = saved;
    stale["items"][0]["lastHash"] = json!(B256::ZERO);
    let reset = collect(&rpc, &ranges, &stale, 1).await;
    assert_eq!(reset["status"], "pending");
    assert_eq!(reset["items"][0]["processed"], 0);
    let mut canonical = ranges.clone();
    canonical["items"][0]["state"] = json!("canonical");
    // Upgrade legacy pending rows and expose only a complete exact sum.
    let legacy = json!({"items":[{"range":canonical["items"][0],"status":"pending","error":"L2 range not finalized"}]});
    let partial = collect(&rpc, &canonical, &legacy, 1).await;
    assert_eq!(partial["status"], "pending");
    let ready = collect(&rpc, &canonical, &partial, 1).await;
    assert_eq!(ready["status"], "available");
    assert_eq!(ready["items"][0]["tipWei"], "38");
    let receipt_calls = state.receipts.load(Ordering::SeqCst);
    assert_eq!(collect(&rpc, &canonical, &ready, 0).await, ready);
    let finalized = collect(&rpc, &ranges, &ready, 0).await;
    assert_eq!(finalized["status"], "available");
    assert_eq!(finalized["items"][0]["range"]["state"], "finalized");
    assert_eq!(state.receipts.load(Ordering::SeqCst), receipt_calls);
    state.reorg.store(true, Ordering::SeqCst);
    assert_eq!(
        collect(&rpc, &canonical, &ready, 0).await["status"],
        "pending"
    );
    assert_eq!(collect(&rpc, &ranges, &ready, 0).await["status"], "pending");
    canonical["items"][0]["state"] = json!("non-canonical");
    assert_eq!(
        collect(&rpc, &canonical, &ready, 1).await["status"],
        "pending"
    );
    task.abort();
}

#[tokio::test]
async fn native_fee_checkpoints_upgrade_resume_and_reject_unattributed_totals() {
    let state = Arc::new(Mock::default());
    state.native.store(true, Ordering::SeqCst);
    state.same_beneficiary.store(true, Ordering::SeqCst);
    let app = Router::new()
        .route("/", post(handler))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rpc = Rpc::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let ranges = json!({"items":[{"first":1,"last":2,"count":2,"firstHash":B256::with_last_byte(1),"lastHash":B256::with_last_byte(2),"state":"finalized"}]});
    // Completed legacy totals cannot supply the new attribution. Even finalized
    // history is rescanned, within the normal block budget, without mixing sums.
    let old = json!({"status":"available","items":[{"range":ranges["items"][0],"status":"available","processed":2,"lastHash":B256::with_last_byte(2),"burnWei":"999","tipWei":"888"}]});
    let first = collect(&rpc, &ranges, &old, 1).await;
    assert_eq!(first["status"], "pending");
    assert_eq!(first["items"][0]["version"], 1);
    assert_eq!(first["items"][0]["processed"], 1);
    assert_eq!(first["items"][0]["burnWei"], "30");
    assert_eq!(first["items"][0]["tipWei"], "15");
    assert_eq!(first["items"][0]["ordinaryGasUsed"], "3");
    assert_eq!(first["items"][0]["nativeGasUsed"], "2");

    let saved: Value = serde_json::from_str(&first.to_string()).unwrap();
    state.missing.store(true, Ordering::SeqCst);
    let failed = collect(&rpc, &ranges, &saved, 1).await;
    assert_eq!(failed["items"][0]["processed"], 1);
    assert_eq!(failed["items"][0]["nativeGasUsed"], "2");
    assert_eq!(failed["items"][0]["error"], "incomplete L2 receipts");
    state.missing.store(false, Ordering::SeqCst);
    let complete = collect(&rpc, &ranges, &failed, 1).await;
    assert_eq!(complete["status"], "available");
    assert_eq!(complete["items"][0]["burnWei"], "60");
    assert_eq!(complete["items"][0]["tipWei"], "30");
    assert_eq!(complete["items"][0]["ordinaryGasUsed"], "6");
    assert_eq!(complete["items"][0]["nativeGasUsed"], "4");
    assert_eq!(
        complete["items"][0]["tipsByBeneficiary"],
        json!({format!("{:#x}", Address::repeat_byte(1)):"30"})
    );
    let requests = state.requests.load(Ordering::SeqCst);
    assert_eq!(collect(&rpc, &ranges, &complete, 1).await, complete);
    assert_eq!(state.requests.load(Ordering::SeqCst), requests);

    let mut corrupt = complete.clone();
    corrupt["items"][0]["tipsByBeneficiary"] = json!({});
    let rejected = collect(&rpc, &ranges, &corrupt, 0).await;
    assert_eq!(rejected["status"], "pending");
    assert_eq!(
        rejected["items"][0]["error"],
        "L2 beneficiary tips do not match total"
    );

    let mut overflow = saved;
    overflow["items"][0]["nativeGasUsed"] = json!(U256::MAX.to_string());
    let rejected = collect(&rpc, &ranges, &overflow, 1).await;
    assert_eq!(rejected["status"], "pending");
    assert_eq!(rejected["items"][0]["processed"], 1);
    assert_eq!(rejected["items"][0]["burnWei"], "30");
    assert_eq!(rejected["items"][0]["tipWei"], "15");
    assert_eq!(
        rejected["items"][0]["error"],
        "L2 range native gas overflow"
    );
    task.abort();
}
