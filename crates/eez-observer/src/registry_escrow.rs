//! Optional hash-pinned registry escrow observation, independent of wallet fees.
use alloy_primitives::{Address, U256};
use alloy_sol_types::SolCall;
use eez_protocol::abi::rollupsCall;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fmt::Write, sync::Arc, time::Duration};
use tokio::sync::RwLock;

use crate::{
    config::Endpoint,
    model::Head,
    rpc::{Failure, Rpc},
};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub registry_address: Address,
    pub rollup_id: u64,
}
impl Config {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.registry_address == Address::ZERO || self.rollup_id == 0 {
            return Err("registry escrow requires a nonzero registry address and rollup ID");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct Reading {
    balance: U256,
    head: Head,
}
#[derive(Debug)]
pub(crate) struct Status {
    rollup_id: u64,
    last_success: Option<u64>,
    value: Option<Reading>,
    error: Option<&'static str>,
}
pub(crate) type SharedStatus = Arc<RwLock<Option<Status>>>;

pub(crate) fn status(config: Option<&Config>) -> SharedStatus {
    Arc::new(RwLock::new(config.map(|config| Status {
        rollup_id: config.rollup_id,
        last_success: None,
        value: None,
        error: None,
    })))
}
fn fresh(at: u64, now: u64) -> bool {
    at <= now.saturating_add(5000) && now.saturating_sub(at) <= 90000
}
fn decode(value: &Value) -> Result<U256, &'static str> {
    let raw = value
        .as_str()
        .and_then(|value| value.strip_prefix("0x"))
        .ok_or("malformed")?;
    // Three fixed ABI words, no dynamic tail, trailing data or loose padding.
    if raw.len() != 192 {
        return Err("malformed");
    }
    let bytes = alloy_primitives::hex::decode(raw).map_err(|_| "malformed")?;
    let decoded = rollupsCall::abi_decode_returns_validate(&bytes).map_err(|_| "malformed")?;
    if rollupsCall::abi_encode_returns(&decoded) != bytes {
        return Err("malformed");
    }
    if decoded.rollupContract == Address::ZERO {
        return Err("unregistered");
    }
    Ok(decoded.etherBalance)
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
    Head::parse(&value).map_err(Failure::label)
}
async fn collect(config: &Config, endpoint: &Endpoint, rpc: &Rpc) -> Result<Reading, &'static str> {
    rpc.validate_identity(endpoint)
        .await
        .map_err(Failure::label)?;
    let head = block(rpc, endpoint, json!("latest")).await?;
    let timestamp = head.timestamp.checked_mul(1000).ok_or("malformed")?;
    if !fresh(timestamp, crate::now_ms()) {
        return Err("stale_head");
    }
    let call = rollupsCall {
        rollupId: config.rollup_id,
    };
    let value = rpc.call(&endpoint.url,"eth_call",json!([
        {"to":config.registry_address,"data":format!("0x{}",alloy_primitives::hex::encode(call.abi_encode()))},
        {"blockHash":head.hash,"requireCanonical":true}
    ])).await.map_err(Failure::label)?;
    let balance = decode(&value)?;
    rpc.validate_identity(endpoint)
        .await
        .map_err(Failure::label)?;
    let recheck = block(rpc, endpoint, json!(format!("0x{:x}", head.number))).await?;
    if recheck.number != head.number
        || !recheck.hash.eq_ignore_ascii_case(&head.hash)
        || recheck.timestamp != head.timestamp
    {
        return Err("noncanonical");
    }
    if !fresh(timestamp, crate::now_ms()) {
        return Err("stale_head");
    }
    Ok(Reading { balance, head })
}
impl Status {
    fn update(&mut self, result: Result<Reading, &'static str>, now: u64) {
        match result {
            Ok(value) => {
                self.value = Some(value);
                self.last_success = Some(now);
                self.error = None;
            }
            Err(error) => {
                self.value = None;
                self.error = Some(error);
            }
        }
    }
}
pub(crate) async fn run(config: Config, endpoint: Endpoint, rpc: Rpc, status: SharedStatus) {
    let mut failures = 0u32;
    loop {
        let result =
            tokio::time::timeout(Duration::from_secs(60), collect(&config, &endpoint, &rpc))
                .await
                .unwrap_or(Err("timeout"));
        failures = if result.is_ok() {
            0
        } else {
            failures.saturating_add(1)
        };
        if let Some(state) = status.write().await.as_mut() {
            state.update(result, crate::now_ms());
        }
        tokio::time::sleep(Duration::from_secs(crate::retry_delay(failures))).await;
    }
}
fn native_units(balance: U256) -> String {
    let scale = U256::from(1_000_000_000_000_000_000u64);
    let whole = balance / scale;
    let fraction = (balance % scale).to::<u64>();
    if fraction == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{fraction:018}")
            .trim_end_matches('0')
            .to_owned()
    }
}

pub(crate) fn render(state: Option<&Status>, now: u64) -> String {
    let mut text = format!(
        "eez_observer_registry_escrow_enabled {}\n",
        u8::from(state.is_some())
    );
    let Some(state) = state else {
        return text;
    };
    let labels = format!("chain=\"l1\",rollup=\"{}\"", state.rollup_id);
    let valid = state.error.is_none()
        && state.last_success.is_some_and(|at| fresh(at, now))
        && state.value.as_ref().is_some_and(|v| {
            v.head
                .timestamp
                .checked_mul(1000)
                .is_some_and(|at| fresh(at, now))
        });
    writeln!(
        text,
        "eez_observer_registry_escrow_evaluation_valid{{{labels}}} {}",
        u8::from(valid)
    )
    .unwrap();
    let reason = if valid {
        "none"
    } else {
        state.error.unwrap_or(if state.last_success.is_some() {
            "stale"
        } else {
            "pending"
        })
    };
    writeln!(
        text,
        "eez_observer_registry_escrow_state{{{labels},reason=\"{reason}\"}} 1"
    )
    .unwrap();
    if let Some(at) = state.last_success {
        writeln!(
            text,
            "eez_observer_registry_escrow_last_success_timestamp_seconds{{{labels}}} {}",
            at / 1000
        )
        .unwrap();
    }
    if valid && let Some(value) = &state.value {
        // Exact wei-to-native decimal conversion before Prometheus's approximate
        // floating-point storage. This gauge is never used for billing.
        writeln!(
            text,
            "eez_observer_registry_escrow_balance_native_units{{{labels}}} {}",
            native_units(value.balance)
        )
        .unwrap();
        writeln!(
            text,
            "eez_observer_registry_escrow_block_number{{{labels}}} {}",
            value.head.number
        )
        .unwrap();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use alloy_sol_types::SolValue;
    use axum::{Router, body::Bytes, extract::State, routing::post};
    use std::sync::{
        Mutex,
        atomic::{AtomicU8, Ordering},
    };

    fn raw_balance(address: Address, balance: U256) -> Value {
        json!(format!(
            "0x{}",
            alloy_primitives::hex::encode((address, B256::ZERO, balance).abi_encode())
        ))
    }
    fn config() -> Config {
        Config {
            registry_address: Address::repeat_byte(3),
            rollup_id: 7,
        }
    }
    fn exporter_value() -> Value {
        let endpoint = json!({"url":"http://127.0.0.1:1","chain_id":1,"genesis_hash":B256::ZERO});
        json!({"l1":endpoint,"l2":endpoint})
    }
    #[test]
    fn config_defaults_disabled_rejects_invalid_and_matches_indexed_rollup() {
        let mut value = exporter_value();
        let disabled: crate::config::Config = serde_json::from_value(value.clone()).unwrap();
        assert!(disabled.registry_escrow.is_none());
        disabled.validate().unwrap();
        value["registry_escrow"] = Value::Null;
        assert!(
            serde_json::from_value::<crate::config::Config>(value.clone())
                .unwrap()
                .registry_escrow
                .is_none()
        );
        let enabled = json!({"registry_address":Address::repeat_byte(3),"rollup_id":7});
        value["registry_escrow"] = enabled.clone();
        serde_json::from_value::<crate::config::Config>(value.clone())
            .unwrap()
            .validate()
            .unwrap();
        for invalid in [
            json!({}),
            json!({"registry_address":"bad","rollup_id":7}),
            json!({"registry_address":Address::repeat_byte(3),"rollup_id":-1}),
            json!({"registry_address":Address::repeat_byte(3),"rollup_id":7,"fallback":true}),
        ] {
            value["registry_escrow"] = invalid;
            assert!(serde_json::from_value::<crate::config::Config>(value.clone()).is_err());
        }
        for invalid in [
            json!({"registry_address":Address::ZERO,"rollup_id":7}),
            json!({"registry_address":Address::repeat_byte(3),"rollup_id":0}),
        ] {
            value["registry_escrow"] = invalid;
            assert!(
                serde_json::from_value::<crate::config::Config>(value.clone())
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        value["registry_escrow"] = enabled;
        let mut index: crate::indexer::Config = serde_json::from_value(json!({
            "exporter":value,"environment":"fixture","deployment":"escrow","rollup_id":7,
            "database_config":"unused","l1":{"bootstrap":0,"include_all_transactions":true},
            "l2":{"bootstrap":0,"include_all_transactions":true}
        }))
        .unwrap();
        index.validate().unwrap();
        index.rollup_id = 8;
        assert_eq!(
            index.validate(),
            Err("registry escrow rollup must match the indexed deployment")
        );
    }
    #[test]
    fn exact_abi_and_uint256_bounds_never_turn_missing_registration_into_zero() {
        assert_eq!(rollupsCall::SELECTOR, [0xef, 0x67, 0x8d, 0x27]); // independent `cast sig`
        for balance in [U256::ZERO, U256::from(1), U256::MAX] {
            assert_eq!(
                decode(&raw_balance(Address::repeat_byte(1), balance)),
                Ok(balance)
            );
        }
        assert_eq!(
            decode(&raw_balance(Address::ZERO, U256::ZERO)),
            Err("unregistered")
        );
        let canonical = raw_balance(Address::repeat_byte(1), U256::MAX);
        let raw = canonical.as_str().unwrap();
        for invalid in [
            json!(format!("{raw}00")),
            json!(&raw[..raw.len() - 2]),
            json!("0x"),
            Value::Null,
        ] {
            assert_eq!(decode(&invalid), Err("malformed"));
        }
        let mut bytes = alloy_primitives::hex::decode(&raw[2..]).unwrap();
        bytes[0] = 1; // dirty address padding is not a registered address
        assert_eq!(
            decode(&json!(format!(
                "0x{}",
                alloy_primitives::hex::encode(bytes)
            ))),
            Err("malformed")
        );
    }
    #[test]
    fn escrow_metric_uses_exact_whole_native_units() {
        for (wei, expected) in [
            (U256::ZERO, "0"),
            (U256::from(1), "0.000000000000000001"),
            (U256::from(10_000_000_000_000_000u64), "0.01"),
            (U256::from(1_000_000_000_000_000_000u64), "1"),
            (
                U256::MAX,
                "115792089237316195423570985008687907853269984665640564039457.584007913129639935",
            ),
        ] {
            let state = Status {
                rollup_id: 7,
                last_success: Some(100000),
                error: None,
                value: Some(Reading {
                    balance: wei,
                    head: Head {
                        number: 1,
                        timestamp: 100,
                        hash: format!("{:#x}", B256::ZERO),
                    },
                }),
            };
            assert!(render(Some(&state), 100000).contains(&format!(
                "eez_observer_registry_escrow_balance_native_units{{chain=\"l1\",rollup=\"7\"}} {expected}\n"
            )));
        }
    }

    #[test]
    fn invalid_failed_stale_and_future_readings_suppress_balance_without_fabricating_zero() {
        assert_eq!(
            render(None, 100000),
            "eez_observer_registry_escrow_enabled 0\n"
        );
        let mut state = Status {
            rollup_id: 7,
            last_success: None,
            value: None,
            error: None,
        };
        assert!(render(Some(&state), 100000).contains("reason=\"pending\""));
        let reading = Reading {
            balance: U256::ZERO,
            head: Head {
                number: 1,
                timestamp: 100,
                hash: format!("{:#x}", B256::repeat_byte(1)),
            },
        };
        state.update(Ok(reading.clone()), 100000);
        assert!(
            render(Some(&state), 100000)
                .contains("balance_native_units{chain=\"l1\",rollup=\"7\"} 0\n")
        );
        assert!(render(Some(&state), 190000).contains("balance_native_units"));
        for now in [190001, 94999] {
            assert!(!render(Some(&state), now).contains("balance_native_units"));
        }
        state.update(Err("rpc"), 100001);
        let failed = render(Some(&state), 100001);
        assert!(!failed.contains("balance_native_units"));
        assert!(failed.contains("last_success_timestamp_seconds{chain=\"l1\",rollup=\"7\"} 100"));
        state.update(
            Ok(Reading {
                balance: U256::MAX,
                ..reading
            }),
            100001,
        );
        assert!(render(Some(&state), 100001).contains(&native_units(U256::MAX)));
        // Fresh collection cannot make an old canonical head fresh.
        state.last_success = Some(200000);
        assert!(!render(Some(&state), 200000).contains("balance_native_units"));
    }

    struct Fixture {
        mode: AtomicU8,
        calls: Mutex<Vec<Value>>,
        timestamp: u64,
    }
    async fn serve(State(state): State<Arc<Fixture>>, body: Bytes) -> String {
        let request: Value = serde_json::from_slice(&body).unwrap();
        state.calls.lock().unwrap().push(request.clone());
        let mode = state.mode.load(Ordering::SeqCst);
        let result = match request["method"].as_str().unwrap() {
            "eth_chainId" => json!(if mode == 1 { "0x2" } else { "0x1" }),
            "eth_getBlockByNumber" => {
                let genesis = request["params"][0] == "0x0";
                let recheck = request["params"][0] == "0x1";
                let timestamp = if mode == 5 {
                    state.timestamp - 100
                } else if mode == 6 {
                    state.timestamp + 100
                } else {
                    state.timestamp
                };
                json!({"number":if genesis {"0x0"} else {"0x1"},
                    "hash":if genesis {B256::ZERO} else if mode==2 && recheck {B256::repeat_byte(2)} else {B256::repeat_byte(1)},
                    "timestamp":format!("0x{timestamp:x}")})
            }
            "eth_call" => {
                if mode == 3 {
                    return json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":"fixture revert"}}).to_string();
                }
                if mode == 7 {
                    json!("0x1234")
                } else {
                    raw_balance(
                        if mode == 4 {
                            Address::ZERO
                        } else {
                            Address::repeat_byte(1)
                        },
                        U256::MAX,
                    )
                }
            }
            _ => panic!("unexpected escrow method"),
        };
        json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string()
    }
    #[tokio::test]
    async fn actual_rpc_reads_registry_at_canonical_hash_and_rejects_failure_reorg_and_stale_head()
    {
        let fixture = Arc::new(Fixture {
            mode: AtomicU8::new(0),
            calls: Mutex::new(Vec::new()),
            timestamp: crate::now_ms() / 1000,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Endpoint {
            url: format!("http://{}", listener.local_addr().unwrap()),
            chain_id: 1,
            genesis_hash: format!("{:#x}", B256::ZERO),
        };
        let app = Router::new()
            .route("/", post(serve))
            .with_state(fixture.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        struct Stop(tokio::task::AbortHandle);
        impl Drop for Stop {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let _stop = Stop(server.abort_handle());
        let rpc = Rpc::new().unwrap();
        let reading = collect(&config(), &endpoint, &rpc).await.unwrap();
        assert_eq!(reading.balance, U256::MAX);
        let calls = std::mem::take(&mut *fixture.calls.lock().unwrap());
        assert_eq!(
            calls
                .iter()
                .filter(|r| r["method"] == "eth_chainId")
                .count(),
            2
        );
        let call = calls.iter().find(|r| r["method"] == "eth_call").unwrap();
        assert_eq!(
            call["params"],
            json!([
                {"to":config().registry_address,"data":format!("0xef678d27{:064x}",7)},
                {"blockHash":B256::repeat_byte(1),"requireCanonical":true}
            ])
        );
        assert_eq!(calls.last().unwrap()["params"], json!(["0x1", false]));
        for (mode, reason) in [
            (1, "identity_mismatch"),
            (2, "noncanonical"),
            (3, "rpc"),
            (4, "unregistered"),
            (5, "stale_head"),
            (6, "stale_head"),
            (7, "malformed"),
        ] {
            fixture.mode.store(mode, Ordering::SeqCst);
            assert_eq!(
                collect(&config(), &endpoint, &rpc).await.unwrap_err(),
                reason
            );
        }
        fixture.mode.store(0, Ordering::SeqCst);
        assert_eq!(
            collect(&config(), &endpoint, &rpc).await.unwrap().balance,
            U256::MAX
        );
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
    }
}
