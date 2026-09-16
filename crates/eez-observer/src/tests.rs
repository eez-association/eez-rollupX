use axum::{Router, body::Bytes, extract::State, routing::post};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::task::JoinHandle;

use crate::{
    config::{Config, Endpoint},
    metrics::render,
    model::{Data, Head, Queue, Sample, Source, quantity},
    retry_delay,
    rpc::{Failure, Rpc},
};

fn hash(byte: &str) -> String {
    format!("0x{}", byte.repeat(32))
}

fn queue_json() -> Value {
    json!({"queued":6,"inFlight":2,"ready":1,"blocked":2,"unknown":3,
        "oldestPendingAgeMs":42000,"oldestServiceAgeMs":null,"evaluatedAtMs":100000,
        "sourceState":{"l1":{"number":1,"hash":hash("aa")},"l2":{"number":2,"hash":hash("bb")}}})
}

#[test]
fn queue_partition_rejects_inconsistent_and_overflowed_counts() {
    let queue = Queue::parse(queue_json()).unwrap();
    assert_eq!(queue.ready + queue.blocked + queue.unknown, queue.queued);
    assert_eq!(queue.queued + queue.in_flight, 8);
    for (field, value) in [
        ("queued", json!(7)),
        ("ready", json!(u64::MAX)),
        ("inFlight", json!(u64::MAX)),
        ("blocked", json!(-1)),
    ] {
        let mut raw = queue_json();
        raw[field] = value;
        assert_eq!(Queue::parse(raw).unwrap_err(), Failure::Malformed);
    }
}

#[test]
fn successful_rpc_does_not_make_readiness_fresh() {
    let mut queue = Queue::parse(queue_json()).unwrap();
    assert!(queue.evaluation_valid(105000));
    assert!(!queue.evaluation_valid(161000));
    assert!(!queue.evaluation_valid(90000));
    queue.source_state = None;
    assert!(!queue.evaluation_valid(105000));
    let mut raw = queue_json();
    raw["sourceState"] = json!({});
    assert_eq!(Queue::parse(raw).unwrap_err(), Failure::Malformed);
}

#[test]
fn restart_unknowns_and_null_ages_do_not_become_zero() {
    let raw = json!({"queued":0,"inFlight":0,"ready":0,"blocked":0,"unknown":0,
        "sourceState":null,"evaluatedAtMs":null,"oldestPendingAgeMs":null,"oldestServiceAgeMs":null});
    let mut sample = Sample::new(Source::Queue);
    sample.update(Ok(Data::Queue(Queue::parse(raw).unwrap())), 100000);
    let metrics = render(&[sample], 100000);
    assert!(metrics.contains("eez_observer_queue_transactions{state=\"queued_unknown\"} 0"));
    assert!(metrics.contains("eez_observer_queue_evaluation_valid 0\n"));
    assert!(
        !metrics
            .lines()
            .any(|l| l.starts_with("eez_observer_queue_oldest_age_seconds{"))
    );
    assert!(
        !metrics
            .lines()
            .any(|l| l.starts_with("eez_observer_queue_evaluated_timestamp_seconds "))
    );
}

#[test]
fn failure_and_stale_samples_omit_data_but_preserve_last_success() {
    let mut sample = Sample::new(Source::L1);
    let initial = render(&[sample.clone()], 100000);
    assert!(initial.contains("eez_observer_collection_success{source=\"l1\"} 0"));
    assert!(
        !initial
            .lines()
            .any(|l| l.starts_with("eez_observer_collection_last_success_timestamp_seconds{"))
    );
    sample.update(
        Ok(Data::Head(Head {
            number: 123,
            timestamp: 99,
            hash: hash("aa"),
        })),
        100000,
    );
    assert!(
        render(&[sample.clone()], 100001)
            .contains("eez_observer_chain_head_number{source=\"l1\"} 123")
    );
    assert!(
        !render(&[sample.clone()], 191000)
            .lines()
            .any(|l| l.starts_with("eez_observer_chain_head_number{"))
    );
    assert!(!sample.is_current(90000));
    sample.update(Err(Failure::Identity), 101000);
    let failed = render(&[sample], 101000);
    assert!(failed.contains("reason=\"identity_mismatch\""));
    assert!(
        failed.contains(
            "eez_observer_collection_last_success_timestamp_seconds{source=\"l1\"} 100.000"
        )
    );
    assert!(
        !failed
            .lines()
            .any(|l| l.starts_with("eez_observer_chain_head_number{"))
    );
}

#[test]
fn quantities_are_strict_and_backoff_is_bounded() {
    for value in ["1", "0x", "0x01", "0x+1", "0x10000000000000000"] {
        assert_eq!(quantity(&json!(value)), Err(Failure::Malformed));
    }
    assert_eq!(quantity(&json!("0x0")), Ok(0));
    assert_eq!(quantity(&json!("0xff")), Ok(255));
    assert_eq!(
        (0..5).map(retry_delay).collect::<Vec<_>>(),
        vec![15, 15, 30, 60, 60]
    );
    assert_eq!(retry_delay(u32::MAX), 60);
}

#[test]
fn config_fails_closed_and_debug_redacts_endpoint_credentials() {
    let raw = json!({"l1":{"url":"http://user:secret@localhost/?token=secret","chain_id":1,"genesis_hash":hash("aa")},
        "l2":{"url":"http://localhost","chain_id":2,"genesis_hash":hash("bb")}});
    let mut config: Config = serde_json::from_value(raw.clone()).unwrap();
    config.validate().unwrap();
    assert!(!format!("{config:?} {:?}", config.l1).contains("secret"));
    config.listen = "0.0.0.0:19100".parse().unwrap();
    assert!(config.validate().is_err());
    config.allow_non_loopback = true;
    config.validate().unwrap();
    config.l1.genesis_hash = "TODO".to_owned();
    assert!(config.validate().is_err());
    let mut extra = raw;
    extra["l1"]["secret_key"] = json!("unused");
    assert!(serde_json::from_value::<Config>(extra).is_err());
}

#[derive(Default)]
struct MockState {
    mode: AtomicU8,
    active: AtomicUsize,
    peak: AtomicUsize,
    calls: AtomicUsize,
}
struct Mock {
    url: String,
    state: Arc<MockState>,
    task: JoinHandle<()>,
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Mock {
    async fn start() -> Self {
        let state = Arc::new(MockState::default());
        let app = Router::new()
            .route("/", post(mock_rpc))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { url, state, task }
    }
    fn endpoint(&self) -> Endpoint {
        Endpoint {
            url: self.url.clone(),
            chain_id: 1,
            genesis_hash: hash("aa"),
        }
    }
}

async fn mock_rpc(State(state): State<Arc<MockState>>, body: Bytes) -> String {
    let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
    state.peak.fetch_max(active, Ordering::SeqCst);
    state.calls.fetch_add(1, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(10)).await;
    state.active.fetch_sub(1, Ordering::SeqCst);
    let request: Value = serde_json::from_slice(&body).unwrap();
    let mode = state.mode.load(Ordering::SeqCst);
    if mode == 5 {
        return "x".repeat(1024 * 1024 + 1);
    }
    let result = match request["method"].as_str().unwrap() {
        "eth_chainId" => json!(if mode == 1 { "0x2" } else { "0x1" }),
        "eth_getBlockByNumber" => {
            let genesis = request["params"][0] == "0x0";
            json!({"number":if genesis {"0x0"} else {"0xa"},"timestamp":"0x64","hash":hash(if mode==2 {"bb"} else {"aa"})})
        }
        "eez_getCrossChainQueueStatus" => {
            if mode == 3 {
                return json!({"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"secret must not reach metrics"}}).to_string();
            }
            queue_json()
        }
        _ => panic!("exporter attempted an unexpected RPC method"),
    };
    json!({"jsonrpc":"2.0","id":if mode==4 {2} else {1},"result":result}).to_string()
}

#[tokio::test]
async fn rpc_identity_is_checked_on_every_poll_and_blocks_wrong_chain_data() {
    let mock = Mock::start().await;
    let rpc = Rpc::new().unwrap();
    let endpoint = mock.endpoint();
    assert!(matches!(
        rpc.collect(Source::L1, &endpoint).await.unwrap(),
        Data::Head(Head { number: 10, .. })
    ));
    for mode in [1, 2] {
        mock.state.mode.store(mode, Ordering::SeqCst);
        assert_eq!(
            rpc.collect(Source::Queue, &endpoint).await.unwrap_err(),
            Failure::Identity
        );
    }
    mock.state.mode.store(0, Ordering::SeqCst);
    assert!(matches!(
        rpc.collect(Source::Queue, &endpoint).await.unwrap(),
        Data::Queue(_)
    ));
}

#[tokio::test]
async fn unsupported_queue_does_not_disable_heads_and_rpc_errors_are_bounded() {
    let mock = Mock::start().await;
    let rpc = Rpc::new().unwrap();
    let endpoint = mock.endpoint();
    mock.state.mode.store(3, Ordering::SeqCst);
    assert_eq!(
        rpc.collect(Source::Queue, &endpoint).await.unwrap_err(),
        Failure::Unsupported
    );
    assert!(rpc.collect(Source::L2, &endpoint).await.is_ok());
    for (mode, error) in [(4, Failure::Malformed), (5, Failure::Oversized)] {
        mock.state.mode.store(mode, Ordering::SeqCst);
        assert_eq!(rpc.collect(Source::L1, &endpoint).await.unwrap_err(), error);
    }
}

#[tokio::test]
async fn concurrency_limit_applies_across_all_sources() {
    let mock = Mock::start().await;
    let rpc = Rpc::new().unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let rpc = rpc.clone();
        let endpoint = mock.endpoint();
        tasks.spawn(async move { rpc.collect(Source::L1, &endpoint).await });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap().unwrap();
    }
    assert!(mock.state.peak.load(Ordering::SeqCst) <= 4);
    assert!(mock.state.peak.load(Ordering::SeqCst) >= 2);
    assert_eq!(mock.state.calls.load(Ordering::SeqCst), 24);
}
