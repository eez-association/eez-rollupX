//! Optional, independently fenced safe-head and explorer observations.
use std::{fmt::Write, sync::Arc, time::Duration};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::RwLock;

use crate::{
    config::Endpoint,
    model::Head,
    rpc::{Failure, GetResponse, Rpc},
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub l2_safe_head: bool,
    #[serde(default)]
    pub explorers: Vec<Explorer>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Explorer {
    pub chain: Chain,
    pub profile: Profile,
    pub url: String,
}
impl std::fmt::Debug for Explorer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Explorer")
            .field("chain", &self.chain)
            .field("profile", &self.profile)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Chain {
    L1,
    L2,
}
impl Chain {
    fn label(self) -> &'static str {
        match self {
            Self::L1 => "l1",
            Self::L2 => "l2",
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    BlockscoutV2,
}

impl Config {
    pub fn validate(&self) -> Result<(), &'static str> {
        if (!self.l2_safe_head && self.explorers.is_empty()) || self.explorers.len() > 2 {
            return Err("network requires a capability and at most two explorers");
        }
        for (i, explorer) in self.explorers.iter().enumerate() {
            crate::config::validate_url(&explorer.url)?;
            let url = reqwest::Url::parse(&explorer.url).map_err(|_| "invalid explorer URL")?;
            if url.query().is_some()
                || self.explorers[..i]
                    .iter()
                    .any(|e| e.chain == explorer.chain)
            {
                return Err("explorer base URLs cannot have queries or duplicate chains");
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct Worker {
    chain: Chain,
    endpoint: Endpoint,
    explorer: Option<String>,
}
impl Worker {
    fn kind(&self) -> &'static str {
        if self.explorer.is_some() {
            "blockscout_v2"
        } else {
            "node_safe"
        }
    }
}
#[derive(Clone, Debug)]
struct Reading {
    number: u64,
    lag: u64,
}
#[derive(Clone)]
pub(crate) struct Status {
    chain: Chain,
    kind: &'static str,
    last_attempt: Option<u64>,
    last_success: Option<u64>,
    value: Option<Reading>,
    error: Option<&'static str>,
}
pub(crate) type SharedStatus = Arc<RwLock<Vec<Status>>>;

pub(crate) fn workers(config: &crate::config::Config) -> Vec<Worker> {
    let mut workers = Vec::new();
    if let Some(network) = &config.network {
        if network.l2_safe_head {
            workers.push(Worker {
                chain: Chain::L2,
                endpoint: config.l2.clone(),
                explorer: None,
            });
        }
        for explorer in &network.explorers {
            workers.push(Worker {
                chain: explorer.chain,
                endpoint: match explorer.chain {
                    Chain::L1 => config.l1.clone(),
                    Chain::L2 => config.l2.clone(),
                },
                explorer: Some(match explorer.profile {
                    Profile::BlockscoutV2 => explorer.url.clone(),
                }),
            });
        }
    }
    workers
}
pub(crate) fn status(workers: &[Worker]) -> SharedStatus {
    Arc::new(RwLock::new(
        workers
            .iter()
            .map(|worker| Status {
                chain: worker.chain,
                kind: worker.kind(),
                last_attempt: None,
                last_success: None,
                value: None,
                error: None,
            })
            .collect(),
    ))
}

async fn block(rpc: &Rpc, endpoint: &Endpoint, selector: Value) -> Result<Head, &'static str> {
    let value = rpc
        .call(
            &endpoint.url,
            "eth_getBlockByNumber",
            json!([selector, false]),
        )
        .await
        .map_err(Failure::label)?;
    if value.is_null() {
        return Err("unavailable");
    }
    Head::parse(&value).map_err(Failure::label)
}
async fn explorer_get(rpc: &Rpc, base: &str, path: &str) -> Result<Value, &'static str> {
    let url = reqwest::Url::parse(&format!("{}/{path}", base.trim_end_matches('/')))
        .map_err(|_| "malformed")?;
    match rpc
        .get_bounded(url, 1024 * 1024)
        .await
        .map_err(Failure::label)?
    {
        GetResponse::Value(value) => Ok(value),
        GetResponse::Absent | GetResponse::Pruned => Err("unavailable"),
    }
}
fn explorer_block(value: &Value) -> Result<(u64, &str), &'static str> {
    let number = value["height"].as_u64().ok_or("malformed")?;
    let hash = value["hash"]
        .as_str()
        .filter(|v| crate::config::valid_hash(v))
        .ok_or("malformed")?;
    if value["type"] != "block" {
        return Err("noncanonical");
    }
    Ok((number, hash))
}
async fn collect(worker: &Worker, rpc: &Rpc) -> Result<Reading, &'static str> {
    rpc.validate_identity(&worker.endpoint)
        .await
        .map_err(Failure::label)?;
    let (number, hash) = if let Some(base) = &worker.explorer {
        let genesis = explorer_get(rpc, base, "api/v2/blocks/0").await?;
        let (height, hash) = explorer_block(&genesis)?;
        if height != 0 || !hash.eq_ignore_ascii_case(&worker.endpoint.genesis_hash) {
            return Err("identity_mismatch");
        }
        let list = explorer_get(rpc, base, "api/v2/blocks?type=block").await?;
        let items = list["items"]
            .as_array()
            .filter(|items| !items.is_empty() && items.len() <= 50)
            .ok_or("malformed")?;
        // The reviewed v2 profile orders canonical blocks descending. Reject an
        // incompatible response instead of choosing a convenient height.
        let mut previous = None;
        for item in items {
            let (height, _) = explorer_block(item)?;
            if previous.is_some_and(|prior| height >= prior) {
                return Err("malformed");
            }
            previous = Some(height);
        }
        let (number, hash) = explorer_block(&items[0])?;
        (number, hash.to_owned())
    } else {
        let safe = block(rpc, &worker.endpoint, json!("safe")).await?;
        (safe.number, safe.hash)
    };
    let canonical = block(rpc, &worker.endpoint, json!(format!("0x{number:x}"))).await?;
    if canonical.number != number || !canonical.hash.eq_ignore_ascii_case(&hash) {
        return Err("noncanonical");
    }
    let latest = block(rpc, &worker.endpoint, json!("latest")).await?;
    let lag = latest.number.checked_sub(number).ok_or("ahead")?;
    rpc.validate_identity(&worker.endpoint)
        .await
        .map_err(Failure::label)?;
    // Unrelated appends are allowed; the exact observed canonical reference
    // must survive the pass. Each scrape is still an observation, not finality.
    let recheck = block(rpc, &worker.endpoint, json!(format!("0x{number:x}"))).await?;
    if recheck.number != number || !recheck.hash.eq_ignore_ascii_case(&hash) {
        return Err("noncanonical");
    }
    Ok(Reading { number, lag })
}

pub(crate) async fn run(worker: Worker, rpc: Rpc, status: SharedStatus, index: usize) {
    let mut failures = 0u32;
    loop {
        let result = tokio::time::timeout(Duration::from_secs(60), collect(&worker, &rpc))
            .await
            .unwrap_or(Err("timeout"));
        let now = crate::now_ms();
        let mut states = status.write().await;
        let state = &mut states[index];
        state.last_attempt = Some(now);
        match result {
            Ok(value) => {
                state.value = Some(value);
                state.error = None;
                state.last_success = Some(now);
                failures = 0;
            }
            Err(error) => {
                state.value = None;
                state.error = Some(error);
                failures = failures.saturating_add(1);
            }
        }
        drop(states);
        tokio::time::sleep(Duration::from_secs(crate::retry_delay(failures))).await;
    }
}

pub(crate) fn render(states: &[Status], now: u64) -> String {
    let mut text = String::new();
    for (chain, kind) in [
        (Chain::L2, "node_safe"),
        (Chain::L1, "blockscout_v2"),
        (Chain::L2, "blockscout_v2"),
    ] {
        let state = states.iter().find(|s| s.chain == chain && s.kind == kind);
        let labels = format!("chain=\"{}\",profile=\"{kind}\"", chain.label());
        writeln!(
            text,
            "eez_observer_network_enabled{{{labels}}} {}",
            u8::from(state.is_some())
        )
        .unwrap();
        let valid = state.is_some_and(|s| {
            s.value.is_some()
                && s.last_success.is_some_and(|at| {
                    at <= now.saturating_add(5000) && now.saturating_sub(at) <= 90000
                })
        });
        writeln!(
            text,
            "eez_observer_network_evaluation_valid{{{labels}}} {}",
            u8::from(valid)
        )
        .unwrap();
        let reason = match state {
            None => "disabled",
            Some(_) if valid => "none",
            Some(s) => s.error.unwrap_or(if s.last_attempt.is_some() {
                "stale"
            } else {
                "pending"
            }),
        };
        writeln!(
            text,
            "eez_observer_network_state{{{labels},reason=\"{reason}\"}} 1"
        )
        .unwrap();
        if let Some(state) = state {
            if let Some(at) = state.last_success {
                writeln!(
                    text,
                    "eez_observer_network_last_success_timestamp_seconds{{{labels}}} {}",
                    at / 1000
                )
                .unwrap();
            }
            if valid && let Some(value) = &state.value {
                writeln!(
                    text,
                    "eez_observer_network_height{{{labels}}} {}",
                    value.number
                )
                .unwrap();
                writeln!(
                    text,
                    "eez_observer_network_lag_blocks{{{labels}}} {}",
                    value.lag
                )
                .unwrap();
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Bytes,
        extract::State,
        routing::{get, post},
    };
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

    fn hash(n: u8) -> String {
        format!("0x{n:02x}").to_owned() + &format!("{n:02x}").repeat(31)
    }
    fn endpoint(url: String) -> Endpoint {
        Endpoint {
            url,
            chain_id: 1,
            genesis_hash: hash(0),
        }
    }
    fn raw_block(number: u64, byte: u8) -> Value {
        json!({"number":format!("0x{number:x}"),"timestamp":"0x1","hash":hash(byte)})
    }

    #[test]
    fn configuration_is_optional_bounded_and_strict() {
        let raw = json!({"l1":{"url":"http://localhost:1","chain_id":1,"genesis_hash":hash(0)},"l2":{"url":"http://localhost:2","chain_id":1,"genesis_hash":hash(0)}});
        let config: crate::config::Config = serde_json::from_value(raw.clone()).unwrap();
        config.validate().unwrap();
        assert!(workers(&config).is_empty());
        for network in [
            json!({}),
            json!({"l2_safe_head":1}),
            json!({"explorers":[{"chain":"l1","profile":"unknown","url":"http://localhost"}]}),
            json!({"explorers":[{"chain":"l1","profile":"blockscout_v2","url":"http://localhost?secret=value"}]}),
            json!({"explorers":[{"chain":"l1","profile":"blockscout_v2","url":"http://localhost"},{"chain":"l1","profile":"blockscout_v2","url":"http://localhost"}]}),
        ] {
            let mut value = raw.clone();
            value["network"] = network;
            assert!(
                serde_json::from_value::<crate::config::Config>(value)
                    .map_err(|_| ())
                    .and_then(|c| c.validate().map_err(|_| ()))
                    .is_err()
            );
        }
        let mut value = raw;
        value["network"] = json!({"l2_safe_head":true,"explorers":[{"chain":"l1","profile":"blockscout_v2","url":"http://localhost"}]});
        let config: crate::config::Config = serde_json::from_value(value).unwrap();
        config.validate().unwrap();
        let workers = workers(&config);
        assert_eq!(workers.len(), 2);
        assert_eq!(workers[0].endpoint.url, "http://localhost:2");
        assert_eq!(workers[1].endpoint.url, "http://localhost:1");
    }

    #[test]
    fn disabled_error_and_stale_are_unknown_without_height_or_zero_lag() {
        assert!(render(&[], 1000).contains("reason=\"disabled\""));
        let mut state = Status {
            chain: Chain::L2,
            kind: "node_safe",
            last_attempt: Some(1000),
            last_success: Some(1000),
            value: Some(Reading { number: 8, lag: 2 }),
            error: None,
        };
        assert!(render(&[state.clone()], 1000).contains("eez_observer_network_height"));
        for now in [92000, 0] {
            if now == 0 {
                state.last_success = Some(6000);
            }
            assert!(!render(&[state.clone()], now).contains("eez_observer_network_height"));
        }
        state.value = None;
        state.error = Some("noncanonical");
        let text = render(&[state], 1000);
        assert!(text.contains("reason=\"noncanonical\""));
        assert!(!text.contains("eez_observer_network_lag_blocks"));
    }

    #[tokio::test]
    async fn actual_wire_checks_identity_canonical_fencing_and_independent_sources() {
        #[derive(Clone)]
        struct Fixture {
            mode: Arc<AtomicU8>,
            references: Arc<AtomicUsize>,
        }
        async fn rpc(State(f): State<Fixture>, body: Bytes) -> String {
            let request: Value = serde_json::from_slice(&body).unwrap();
            let mode = f.mode.load(Ordering::SeqCst);
            let result = if request["method"] == "eth_chainId" {
                json!(if mode == 1 { "0x2" } else { "0x1" })
            } else {
                match request["params"][0].as_str().unwrap() {
                    "0x0" => raw_block(0, 0),
                    "latest" => raw_block(if mode == 6 { 7 } else { 10 }, 10),
                    "safe" if mode == 2 => Value::Null,
                    "safe" => raw_block(8, 8),
                    "0x8" => {
                        let n = f.references.fetch_add(1, Ordering::SeqCst);
                        raw_block(
                            8,
                            if mode == 3 || (mode == 4 && n > 0) {
                                9
                            } else {
                                8
                            },
                        )
                    }
                    _ => Value::Null,
                }
            };
            json!({"jsonrpc":"2.0","id":1,"result":result}).to_string()
        }
        async fn genesis(State(f): State<Fixture>) -> String {
            json!({"height":0,"hash":hash(u8::from(f.mode.load(Ordering::SeqCst) == 5)),"type":"block"}).to_string()
        }
        async fn blocks() -> String {
            json!({"items":[{"height":8,"hash":hash(8),"type":"block"}]}).to_string()
        }
        let fixture = Fixture {
            mode: Arc::new(AtomicU8::new(0)),
            references: Arc::new(AtomicUsize::new(0)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/", post(rpc))
            .route("/api/v2/blocks/0", get(genesis))
            .route("/api/v2/blocks", get(blocks))
            .with_state(fixture.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let rpc = Rpc::new().unwrap();
        let mut worker = Worker {
            chain: Chain::L2,
            endpoint: endpoint(url.clone()),
            explorer: None,
        };
        for (mode, expected) in [
            (0, None),
            (1, Some("identity_mismatch")),
            (2, Some("unavailable")),
            (3, Some("noncanonical")),
            (4, Some("noncanonical")),
            (6, Some("ahead")),
        ] {
            fixture.mode.store(mode, Ordering::SeqCst);
            fixture.references.store(0, Ordering::SeqCst);
            let result = collect(&worker, &rpc).await;
            match expected {
                Some(reason) => assert_eq!(result.unwrap_err(), reason),
                None => {
                    let reading = result.unwrap();
                    assert_eq!((reading.number, reading.lag), (8, 2));
                }
            }
        }
        fixture.mode.store(5, Ordering::SeqCst);
        worker.explorer = Some(url);
        assert_eq!(
            collect(&worker, &rpc).await.unwrap_err(),
            "identity_mismatch"
        );
        worker.explorer = None;
        assert!(collect(&worker, &rpc).await.is_ok());
        worker.explorer = Some(worker.endpoint.url.clone());
        fixture.mode.store(0, Ordering::SeqCst);
        assert_eq!(collect(&worker, &rpc).await.unwrap().lag, 2);
        server.abort();
    }
}
