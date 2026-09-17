//! Optional explicit-account balances and covered canonical spend-based runway.
//! Native candidate affordability is a separate signal from the actual signer.
use super::{
    Block, Config, SharedStatus,
    ledger::{self, Deadline},
};
use crate::{
    database::{Database, DatabaseConfig},
    model::quantity_hex,
    rpc::{Failure, Rpc},
};
use alloy_primitives::{Address, B256, U256};
use serde::Deserialize;
use serde_json::json;
use std::{fmt::Write as _, time::Duration};

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountRole {
    Poster,
    System,
}
impl AccountRole {
    const fn label(self) -> &'static str {
        match self {
            Self::Poster => "poster",
            Self::System => "system",
        }
    }
}
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Chain {
    L1,
    L2,
}
impl Chain {
    const fn index(self) -> usize {
        match self {
            Self::L1 => 0,
            Self::L2 => 1,
        }
    }
    const fn label(self) -> &'static str {
        match self {
            Self::L1 => "l1",
            Self::L2 => "l2",
        }
    }
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    pub role: AccountRole,
    pub chain: Chain,
    pub address: Address,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FundingConfig {
    pub accounts: Vec<AccountConfig>,
    #[serde(default = "default_window")]
    pub burn_window_seconds: u64,
}
const fn default_window() -> u64 {
    86400
}
impl FundingConfig {
    pub(super) fn validate(&self, config: &Config) -> Result<(), &'static str> {
        if self.accounts.is_empty()
            || self.accounts.len() > 2
            || !(60..=604800).contains(&self.burn_window_seconds)
        {
            return Err("funding requires one or two accounts and a 60..604800-second burn window");
        }
        for (index, account) in self.accounts.iter().enumerate() {
            if account.address == Address::ZERO
                || self.accounts[..index]
                    .iter()
                    .any(|previous| previous.role == account.role)
                || account.role == AccountRole::Poster && account.chain != Chain::L1
            {
                return Err("funding roles must be unique, nonzero accounts; poster must be on L1");
            }
            let selection = if account.chain == Chain::L1 {
                &config.l1
            } else {
                &config.l2
            };
            if !selection.include_all_transactions
                && !selection
                    .wallets
                    .iter()
                    .any(|value| value.parse::<Address>().ok() == Some(account.address))
            {
                return Err(
                    "funding account must be covered by an explicit wallet or all-transactions selection",
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct AccountStatus {
    role: AccountRole,
    chain: Chain,
    at: u64,
    balance: Option<U256>,
    head: Option<B256>,
    head_timestamp: Option<u64>,
    canonical_revision: Option<i64>,
    error: Option<&'static str>,
    burn_reason: Option<&'static str>,
    burn_native_per_second: Option<f64>,
    runway_seconds: Option<f64>,
    low_runway: Option<bool>,
}
#[derive(Debug, Clone, Default)]
pub struct Status {
    accounts: Vec<AccountStatus>,
}

fn fresh(at: u64, now: u64) -> bool {
    at <= now.saturating_add(5000) && now.saturating_sub(at) <= 90000
}
fn display(value: &str) -> Result<f64, &'static str> {
    let value: f64 = value.parse().map_err(|_| "numeric")?;
    if !value.is_finite() || value < 0. {
        return Err("numeric");
    }
    Ok(value)
}
pub fn render(runtime: &super::RuntimeStatus, now: u64) -> String {
    let Some(status) = &runtime.funding else {
        return String::new();
    };
    let mut text = String::new();
    for account in &status.accounts {
        let labels = format!(
            "account=\"{}\",chain=\"{}\"",
            account.role.label(),
            account.chain.label()
        );
        let chain = &runtime.chains[account.chain.index()];
        let revision_valid = account
            .canonical_revision
            .is_none_or(|revision| chain.canonical_revision == Some(revision));
        let valid = fresh(account.at, now)
            && account.error.is_none()
            && account.balance.is_some()
            && account.head.is_some()
            && revision_valid;
        writeln!(
            text,
            "eez_observer_funding_source_valid{{{labels}}} {}",
            u8::from(valid)
        )
        .unwrap();
        writeln!(
            text,
            "eez_observer_funding_last_attempt_timestamp_seconds{{{labels}}} {}",
            account.at / 1000
        )
        .unwrap();
        if let Some(reason) = account.error {
            writeln!(
                text,
                "eez_observer_funding_error{{{labels},reason=\"{reason}\"}} 1"
            )
            .unwrap();
        }
        if valid {
            if let Some(timestamp) = account.head_timestamp {
                writeln!(
                    text,
                    "eez_observer_funding_balance_block_timestamp_seconds{{{labels}}} {timestamp}"
                )
                .unwrap();
            }
            if let Some(balance) = account
                .balance
                .and_then(|value| display(&value.to_string()).ok())
            {
                writeln!(
                    text,
                    "eez_observer_funding_balance_native_units{{{labels}}} {}",
                    balance / 1e18
                )
                .unwrap();
            }
        }
        let burn_valid = valid
            && chain.error.is_none()
            && chain.last_success.is_some_and(|at| fresh(at, now))
            && account.canonical_revision.is_some()
            && account.burn_reason.is_none()
            && account.runway_seconds.is_some();
        writeln!(
            text,
            "eez_observer_funding_burn_valid{{{labels}}} {}",
            u8::from(burn_valid)
        )
        .unwrap();
        if burn_valid {
            if let (Some(burn), Some(runway), Some(low)) = (
                account.burn_native_per_second,
                account.runway_seconds,
                account.low_runway,
            ) {
                writeln!(text, "eez_observer_funding_burn_native_units_per_second{{{labels}}} {burn}\neez_observer_funding_runway_seconds{{{labels}}} {runway}\neez_observer_funding_runway_below_day{{{labels}}} {}", u8::from(low)).unwrap();
            }
        } else {
            let reason = if !revision_valid {
                "canonical_changed"
            } else if !valid {
                "balance_unavailable"
            } else {
                account.burn_reason.unwrap_or("stale_source")
            };
            writeln!(
                text,
                "eez_observer_funding_burn_unknown{{{labels},reason=\"{reason}\"}} 1"
            )
            .unwrap();
        }
    }
    text
}

pub(super) async fn run(config: &Config, rpc: &Rpc, status: &SharedStatus) {
    if config.funding.is_none() {
        return;
    }
    let mut failures = 0u32;
    loop {
        failures = if poll(config, rpc, status).await {
            0
        } else {
            failures.saturating_add(1)
        };
        tokio::time::sleep(Duration::from_secs(crate::retry_delay(failures))).await;
    }
}
pub(super) async fn poll(config: &Config, rpc: &Rpc, status: &SharedStatus) -> bool {
    let Some(funding) = &config.funding else {
        return true;
    };
    async fn collect(
        config: &Config,
        rpc: &Rpc,
        account: &AccountConfig,
        window: u64,
        cursor: Option<(u64, B256, i64)>,
    ) -> AccountStatus {
        match tokio::time::timeout(
            Duration::from_secs(30),
            account_cycle(config, rpc, account, window, cursor),
        )
        .await
        {
            Ok(Ok(state)) => state,
            result => AccountStatus {
                role: account.role,
                chain: account.chain,
                at: crate::now_ms(),
                balance: None,
                head: None,
                head_timestamp: None,
                canonical_revision: None,
                error: Some(match result {
                    Ok(Err(error)) => error,
                    _ => "timeout",
                }),
                burn_reason: None,
                burn_native_per_second: None,
                runway_seconds: None,
                low_runway: None,
            },
        }
    }
    let chains = status.read().await.chains.clone();
    let cursor = |account: &AccountConfig| {
        let chain = &chains[account.chain.index()];
        (chain.error.is_none()
            && chain
                .last_success
                .is_some_and(|at| fresh(at, crate::now_ms())))
        .then(|| {
            chain
                .cursor
                .zip(chain.cursor_hash)
                .zip(chain.canonical_revision)
                .map(|((number, hash), revision)| (number, hash, revision))
        })
        .flatten()
    };
    let first = collect(
        config,
        rpc,
        &funding.accounts[0],
        funding.burn_window_seconds,
        cursor(&funding.accounts[0]),
    );
    let second = async {
        match funding.accounts.get(1) {
            Some(account) => Some(
                collect(
                    config,
                    rpc,
                    account,
                    funding.burn_window_seconds,
                    cursor(account),
                )
                .await,
            ),
            None => None,
        }
    };
    let (first, second) = tokio::join!(first, second);
    let accounts: Vec<_> = std::iter::once(first).chain(second).collect();
    let successful = accounts.iter().all(|account| account.error.is_none());
    status.write().await.funding = Some(Status { accounts });
    successful
}

async fn account_cycle(
    config: &Config,
    rpc: &Rpc,
    account: &AccountConfig,
    window: u64,
    cursor: Option<(u64, B256, i64)>,
) -> Result<AccountStatus, &'static str> {
    let endpoint = if account.chain == Chain::L1 {
        &config.exporter.l1
    } else {
        &config.exporter.l2
    };
    rpc.validate_identity(endpoint)
        .await
        .map_err(Failure::label)?;
    // A fresh canonical cursor provides an attainable common balance/burn
    // snapshot even while a fast chain advances independently of the indexer.
    // Missing/old history still permits a current balance, but no invented burn.
    let indexed = if let Some((number, expected, _)) = cursor {
        let block = super::remote_block(rpc, endpoint, number)
            .await
            .map_err(|_| "head_recheck")?;
        if block.hash != expected {
            return Err("head_changed");
        }
        let now = crate::now_ms() / 1000;
        (block.timestamp <= now.saturating_add(5) && now.saturating_sub(block.timestamp) <= 90)
            .then_some(block)
    } else {
        None
    };
    let head = match indexed {
        Some(block) => block,
        None => Block::parse(
            &rpc.block(endpoint, json!("latest"), false)
                .await
                .map_err(Failure::label)?,
        )
        .map_err(|_| "invalid_head")?,
    };
    let value = rpc
        .call(
            &endpoint.url,
            "eth_getBalance",
            json!([account.address, {"blockHash":head.hash,"requireCanonical":true}]),
        )
        .await
        .map_err(Failure::label)?;
    let balance = U256::from_str_radix(quantity_hex(&value).map_err(|_| "malformed")?, 16)
        .map_err(|_| "malformed")?;
    let mut result = AccountStatus {
        role: account.role,
        chain: account.chain,
        at: crate::now_ms(),
        balance: Some(balance),
        head: Some(head.hash),
        head_timestamp: Some(head.timestamp),
        canonical_revision: cursor.map(|(_, _, revision)| revision),
        error: None,
        burn_reason: None,
        burn_native_per_second: None,
        runway_seconds: None,
        low_runway: None,
    };
    match burn(config, account, &head, balance, window).await {
        Ok((rate, runway, low, revision)) => {
            result.burn_native_per_second = Some(rate);
            result.runway_seconds = Some(runway);
            result.low_runway = Some(low);
            result.canonical_revision = Some(revision);
        }
        Err(reason) => result.burn_reason = Some(reason),
    }
    let current = super::remote_block(rpc, endpoint, head.number)
        .await
        .map_err(|_| "head_recheck")?;
    rpc.validate_identity(endpoint)
        .await
        .map_err(Failure::label)?;
    if current != head {
        return Err("head_changed");
    }
    result.at = crate::now_ms();
    Ok(result)
}

// One SQL snapshot checks window coverage and computes exact numeric spend,
// runway and the 24h decision. Display floats never decide the warning boundary.
const BURN_SQL: &str = "WITH chain AS (SELECT c.*,s.block_hash,tip.number AS cursor_number FROM eez.chain_instances c JOIN eez.scan_cursors s ON s.chain_instance_id=c.id JOIN eez.blocks tip ON tip.chain_instance_id=c.id AND tip.hash=s.block_hash WHERE c.id=$1), paid AS (SELECT count(*) FILTER(WHERE accounting_gap IS NOT NULL) AS gaps,COALESCE(sum(execution_paid+blob_paid),0) AS total FROM eez.canonical_receipts WHERE chain_instance_id=$1 AND sender=$2::bytea AND timestamp_seconds>$3::text::numeric AND timestamp_seconds<=$4::text::numeric) SELECT (SELECT frozen_reason IS NULL AND collection_error IS NULL AND last_success_at BETWEEN now()-interval '90 seconds' AND now()+interval '5 seconds' AND EXISTS(SELECT FROM eez.blocks anchor WHERE anchor.chain_instance_id=chain.id AND anchor.canonical AND anchor.hash=$5::bytea AND anchor.timestamp_seconds=$4::text::numeric AND anchor.number<=chain.cursor_number) FROM chain) AND EXISTS(SELECT FROM eez.blocks b JOIN chain c ON b.chain_instance_id=c.id WHERE b.canonical AND b.number=c.bootstrap_number AND b.timestamp_seconds<=$3::text::numeric) AND NOT EXISTS(SELECT FROM eez.blocks WHERE chain_instance_id=$1 AND canonical AND timestamp_seconds>$3::text::numeric AND timestamp_seconds<=$4::text::numeric AND receipt_coverage<>'complete') AND NOT EXISTS(SELECT FROM eez.collection_gaps WHERE chain_instance_id=$1 AND state IN ('open','frozen')) AND paid.gaps=0,paid.total::text,CASE WHEN paid.total>0 THEN (paid.total/$6::text::numeric/1e18)::text END,CASE WHEN paid.total>0 THEN ($7::text::numeric*$6::text::numeric/paid.total)::text END,CASE WHEN paid.total>0 THEN $7::text::numeric*$6::text::numeric<paid.total*86400 END,(SELECT canonical_revision FROM chain) FROM paid";
async fn burn(
    config: &Config,
    account: &AccountConfig,
    head: &Block,
    balance: U256,
    window: u64,
) -> Result<(f64, f64, bool, i64), &'static str> {
    let now = crate::now_ms() / 1000;
    if head.timestamp > now.saturating_add(5) || now.saturating_sub(head.timestamp) > 90 {
        return Err("stale_head");
    }
    let since = head
        .timestamp
        .checked_sub(window)
        .ok_or("insufficient_history")?;
    let db_config = DatabaseConfig::load(&config.database_config).map_err(|_| "database")?;
    let db = Database::connect(&db_config)
        .await
        .map_err(|_| "database")?;
    db.verify_writer().await.map_err(|_| "schema_or_role")?;
    let endpoint = if account.chain == Chain::L1 {
        &config.exporter.l1
    } else {
        &config.exporter.l2
    };
    let id = ledger::find_chain(&db, config, endpoint, account.chain.label())
        .await
        .map_err(|_| "database")?
        .ok_or("unindexed")?;
    let row = db
        .client
        .query_one(
            BURN_SQL,
            &[
                &id,
                &account.address.as_slice(),
                &since.to_string(),
                &head.timestamp.to_string(),
                &head.hash.as_slice(),
                &window.to_string(),
                &balance.to_string(),
            ],
        )
        .bounded()
        .await
        .map_err(|_| "database")?;
    if row.try_get::<_, Option<bool>>(0).map_err(|_| "database")? != Some(true) {
        return Err("incomplete_history");
    }
    if row.try_get::<_, String>(1).map_err(|_| "database")? == "0" {
        return Err("zero_burn");
    }
    let rate = row
        .try_get::<_, Option<String>>(2)
        .map_err(|_| "database")?
        .ok_or("zero_burn")?;
    let runway = row
        .try_get::<_, Option<String>>(3)
        .map_err(|_| "database")?
        .ok_or("zero_burn")?;
    let low = row
        .try_get::<_, Option<bool>>(4)
        .map_err(|_| "database")?
        .ok_or("zero_burn")?;
    let revision = row.try_get::<_, i64>(5).map_err(|_| "database")?;
    Ok((display(&rate)?, display(&runway)?, low, revision))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        serde_json::from_value(json!({"exporter":{"l1":{"url":"http://localhost:1","chain_id":1,"genesis_hash":format!("{:#x}",B256::ZERO)},"l2":{"url":"http://localhost:2","chain_id":2,"genesis_hash":format!("{:#x}",B256::ZERO)}},"environment":"test","deployment":"funding","rollup_id":1,"database_config":"unused.json","l1":{"bootstrap":0,"include_all_transactions":true},"l2":{"bootstrap":0,"include_all_transactions":true}})).unwrap()
    }
    #[test]
    fn account_configuration_is_opt_in_bounded_and_requires_complete_selection() {
        let mut config = config();
        assert!(config.funding.is_none());
        config.funding = Some(serde_json::from_value(json!({"accounts":[{"role":"poster","chain":"l1","address":Address::repeat_byte(1)}]})).unwrap());
        assert_eq!(config.funding.as_ref().unwrap().burn_window_seconds, 86400);
        config.validate().unwrap();
        config.l1.include_all_transactions = false;
        config.l1.contracts = vec![format!("{:#x}", Address::repeat_byte(2))];
        assert!(
            config.validate().is_err(),
            "recipient selection cannot establish complete sender spend"
        );
        config.l1.wallets = vec![format!("{:#x}", Address::repeat_byte(1))];
        config.validate().unwrap();
        for window in [0, 59, 604801, u64::MAX] {
            let mut invalid = config.clone();
            invalid.funding.as_mut().unwrap().burn_window_seconds = window;
            assert!(invalid.validate().is_err());
        }
        for (role, chain, address) in [
            (AccountRole::Poster, Chain::L2, Address::repeat_byte(1)),
            (AccountRole::Poster, Chain::L1, Address::ZERO),
        ] {
            let mut invalid = config.clone();
            invalid.funding.as_mut().unwrap().accounts = vec![AccountConfig {
                role,
                chain,
                address,
            }];
            assert!(invalid.validate().is_err());
        }
        let funding = config.funding.as_mut().unwrap();
        funding.accounts.push(funding.accounts[0].clone());
        assert!(config.validate().is_err());
    }

    #[test]
    fn fresh_balance_and_runway_are_separate_and_reorg_staleness_retracts_burn() {
        let mut runtime = super::super::RuntimeStatus::default();
        assert!(render(&runtime, 100000).is_empty());
        runtime.chains[0].last_success = Some(100000);
        runtime.chains[0].canonical_revision = Some(0);
        runtime.chains[0].cursor_hash = Some(B256::repeat_byte(1));
        runtime.funding = Some(Status {
            accounts: vec![AccountStatus {
                role: AccountRole::Poster,
                chain: Chain::L1,
                at: 100000,
                balance: Some(U256::ZERO),
                head: Some(B256::repeat_byte(1)),
                head_timestamp: Some(100),
                canonical_revision: Some(0),
                error: None,
                burn_reason: None,
                burn_native_per_second: Some(1.),
                runway_seconds: Some(0.),
                low_runway: Some(true),
            }],
        });
        let known = render(&runtime, 100000);
        assert!(known.contains(
            "eez_observer_funding_balance_native_units{account=\"poster\",chain=\"l1\"} 0"
        ));
        assert!(
            known.contains(
                "eez_observer_funding_runway_below_day{account=\"poster\",chain=\"l1\"} 1"
            )
        );
        runtime.chains[0].cursor_hash = Some(B256::repeat_byte(2));
        assert!(
            render(&runtime, 100000).contains("eez_observer_funding_runway_seconds"),
            "append-only cursor progress preserves an anchored historical window"
        );
        runtime.chains[0].canonical_revision = Some(1);
        let changed = render(&runtime, 100000);
        assert!(!changed.contains("eez_observer_funding_runway_seconds"));
        assert!(changed.contains("reason=\"canonical_changed\""));
        assert!(!changed.contains("eez_observer_funding_balance_native_units"));
        let stale = render(&runtime, 190001);
        assert!(!stale.contains("eez_observer_funding_balance_native_units"));
        assert!(!stale.contains("eez_observer_funding_runway_seconds"));
        assert!(!render(&runtime, 94999).contains("eez_observer_funding_balance_native_units"));
    }

    #[test]
    fn missing_zero_or_incomplete_burn_never_becomes_infinite_runway() {
        let mut runtime = super::super::RuntimeStatus::default();
        runtime.chains[0].last_success = Some(100000);
        runtime.chains[0].canonical_revision = Some(0);
        runtime.chains[0].cursor_hash = Some(B256::repeat_byte(1));
        runtime.funding = Some(Status {
            accounts: vec![AccountStatus {
                role: AccountRole::Poster,
                chain: Chain::L1,
                at: 100000,
                balance: Some(U256::MAX),
                head: Some(B256::repeat_byte(1)),
                head_timestamp: Some(100),
                canonical_revision: Some(0),
                error: None,
                burn_reason: Some("zero_burn"),
                burn_native_per_second: None,
                runway_seconds: None,
                low_runway: None,
            }],
        });
        for reason in ["zero_burn", "incomplete_history", "database", "stale_head"] {
            runtime.funding.as_mut().unwrap().accounts[0].burn_reason = Some(reason);
            let text = render(&runtime, 100000);
            assert!(
                text.contains(
                    "eez_observer_funding_source_valid{account=\"poster\",chain=\"l1\"} 1"
                )
            );
            assert!(!text.contains("eez_observer_funding_runway_seconds"));
            assert!(!text.contains("eez_observer_funding_runway_below_day"));
        }
        runtime.funding.as_mut().unwrap().accounts[0].error = Some("identity_mismatch");
        assert!(!render(&runtime, 100000).contains("eez_observer_funding_balance_native_units"));
        for value in ["NaN", "inf", "-1", "invalid"] {
            assert!(display(value).is_err());
        }
    }

    #[tokio::test]
    async fn actual_rpc_balance_is_hash_pinned_rechecked_and_partial_account_failure_isolated() {
        use axum::{Router, body::Bytes, extract::State, routing::post};
        use std::sync::{
            Arc,
            atomic::{AtomicBool, AtomicU8, Ordering},
        };
        use tokio::sync::RwLock;
        #[derive(Clone)]
        struct Fixture {
            phase: Arc<AtomicU8>,
            pinned: Arc<AtomicBool>,
            now: u64,
        }
        async fn serve(State(fixture): State<Fixture>, body: Bytes) -> String {
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let phase = fixture.phase.load(Ordering::Relaxed);
            let value = match request["method"].as_str().unwrap() {
                "eth_chainId" => json!(if phase == 2 { "0x99" } else { "0x1" }),
                "eth_getBlockByNumber" => {
                    let genesis = request["params"][0] == "0x0";
                    let changed = phase == 1 && request["params"][0] == "0x1";
                    let newer = phase == 4 && request["params"][0] == "latest";
                    json!({"number":if genesis {"0x0"}else if newer {"0x2"}else{"0x1"},"hash":if genesis {B256::ZERO}else if changed || newer {B256::repeat_byte(2)}else{B256::repeat_byte(1)},"parentHash":B256::ZERO,"timestamp":if phase == 4 {format!("{:#x}",fixture.now)}else{"0x64".into()}})
                }
                "eth_getBalance" => {
                    fixture.pinned.store(
                        request["params"][1]
                            == json!({"blockHash":B256::repeat_byte(1),"requireCanonical":true}),
                        Ordering::Relaxed,
                    );
                    json!(if phase == 3 {
                        "0xinvalid".into()
                    } else {
                        format!("{:#x}", U256::MAX)
                    })
                }
                _ => panic!("unexpected fixture method"),
            };
            json!({"jsonrpc":"2.0","id":request["id"],"result":value}).to_string()
        }
        let fixture = Fixture {
            phase: Arc::new(AtomicU8::new(0)),
            pinned: Arc::new(AtomicBool::new(false)),
            now: crate::now_ms() / 1000,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/", post(serve))
            .with_state(fixture.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut config = config();
        config.exporter.l1.url = format!("http://{address}");
        config.exporter.l2.url = config.exporter.l1.url.clone();
        let account = AccountConfig {
            role: AccountRole::Poster,
            chain: Chain::L1,
            address: Address::repeat_byte(1),
        };
        let rpc = Rpc::new().unwrap();
        let result = account_cycle(&config, &rpc, &account, 60, None)
            .await
            .unwrap();
        assert_eq!(result.balance, Some(U256::MAX));
        assert_eq!(result.burn_reason, Some("stale_head"));
        assert!(fixture.pinned.load(Ordering::Relaxed));
        for (phase, reason) in [
            (1, "head_changed"),
            (2, "identity_mismatch"),
            (3, "malformed"),
        ] {
            fixture.phase.store(phase, Ordering::Relaxed);
            assert_eq!(
                account_cycle(&config, &rpc, &account, 60, None)
                    .await
                    .unwrap_err(),
                reason
            );
        }
        fixture.phase.store(4, Ordering::Relaxed);
        fixture.pinned.store(false, Ordering::Relaxed);
        let indexed = account_cycle(
            &config,
            &rpc,
            &account,
            60,
            Some((1, B256::repeat_byte(1), 0)),
        )
        .await
        .unwrap();
        assert_eq!(
            indexed.head,
            Some(B256::repeat_byte(1)),
            "use fresh canonical cursor instead of racing the newer tip"
        );
        assert!(fixture.pinned.load(Ordering::Relaxed));
        assert_eq!(
            account_cycle(
                &config,
                &rpc,
                &account,
                60,
                Some((1, B256::repeat_byte(9), 0))
            )
            .await
            .unwrap_err(),
            "head_changed"
        );
        fixture.phase.store(0, Ordering::Relaxed);
        config.funding = Some(FundingConfig {
            accounts: vec![
                account,
                AccountConfig {
                    role: AccountRole::System,
                    chain: Chain::L2,
                    address: Address::repeat_byte(2),
                },
            ],
            burn_window_seconds: 60,
        });
        let status = Arc::new(RwLock::new(super::super::RuntimeStatus::default()));
        assert!(!poll(&config, &rpc, &status).await);
        let state = status.read().await;
        let accounts = &state.funding.as_ref().unwrap().accounts;
        assert_eq!(accounts[0].balance, Some(U256::MAX));
        assert_eq!(accounts[1].error, Some("identity_mismatch"));
        assert!(render(&state, crate::now_ms()).contains(
            "eez_observer_funding_balance_native_units{account=\"poster\",chain=\"l1\"}"
        ));
        server.abort();
    }
}
