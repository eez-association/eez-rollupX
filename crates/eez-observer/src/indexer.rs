//! Bounded canonical scanning. Chain evidence and receipt accounting never
//! depend on lifecycle delivery, blob availability, or a monitoring backend.
use alloy_primitives::{Address, B256, U256};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::RwLock;

use crate::{
    accounting::{Receipt, hash},
    config::{Config as ExporterConfig, Endpoint},
    database::{Database, DatabaseConfig},
    model::{Head, quantity, quantity_hex},
    rpc::{Failure, Rpc},
};

mod ledger;
pub mod lifecycle;
mod settlements;
pub mod source_receipts;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub bootstrap: u64,
    #[serde(default)]
    pub wallets: Vec<String>,
    #[serde(default)]
    pub contracts: Vec<String>,
    #[serde(default)]
    pub include_all_transactions: bool,
}

impl Selection {
    fn validate(&self) -> Result<(), &'static str> {
        if !self.include_all_transactions && self.wallets.is_empty() && self.contracts.is_empty() {
            return Err(
                "indexer selection must name wallets/contracts or explicitly include all transactions",
            );
        }
        if self.wallets.len() + self.contracts.len() > 256 {
            return Err("too many indexer selection addresses");
        }
        for list in [&self.wallets, &self.contracts] {
            let mut seen = HashSet::new();
            for value in list {
                let parsed = address(value).map_err(|_| "invalid indexer selection address")?;
                if !seen.insert(parsed) {
                    return Err("duplicate indexer selection address");
                }
            }
        }
        Ok(())
    }

    fn policy(&self) -> Value {
        let canonical = |values: &[String]| {
            let mut values: Vec<_> = values
                .iter()
                .map(|value| value.to_ascii_lowercase())
                .collect();
            values.sort();
            values
        };
        json!({"wallets":canonical(&self.wallets), "contracts":canonical(&self.contracts), "include_all_transactions":self.include_all_transactions})
    }

    fn includes(&self, transaction: &Transaction) -> bool {
        let sender = format!("{:#x}", transaction.sender);
        self.include_all_transactions
            || self
                .wallets
                .iter()
                .any(|value| value.eq_ignore_ascii_case(&sender))
            || transaction.recipient.is_some_and(|recipient| {
                let recipient = format!("{recipient:#x}");
                self.contracts
                    .iter()
                    .any(|value| value.eq_ignore_ascii_case(&recipient))
            })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub exporter: ExporterConfig,
    pub environment: String,
    pub deployment: String,
    pub rollup_id: u64,
    pub database_config: PathBuf,
    pub l1: Selection,
    pub l2: Selection,
    #[serde(default = "batch_default")]
    pub batch_blocks: u64,
    #[serde(default)]
    pub lifecycle: Vec<lifecycle::SourceConfig>,
}
fn batch_default() -> u64 {
    16
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, &'static str> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| "cannot read indexer config")?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read indexer config")?;
        if bytes.len() > 1024 * 1024 {
            return Err("indexer config too large");
        }
        let mut config: Self =
            serde_json::from_slice(&bytes).map_err(|_| "invalid indexer configuration")?;
        if config.database_config.is_relative() {
            config.database_config = path
                .parent()
                .unwrap_or(Path::new("."))
                .join(&config.database_config);
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        self.exporter.validate()?;
        self.l1.validate()?;
        self.l2.validate()?;
        lifecycle::validate_sources(self)?;
        for value in [&self.environment, &self.deployment] {
            if value.is_empty()
                || value.len() > 64
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            {
                return Err("invalid indexer deployment identity");
            }
        }
        if self.rollup_id == 0
            || !(1..=64).contains(&self.batch_blocks)
            || self.database_config.as_os_str().is_empty()
        {
            return Err("invalid rollup, batch bound or database config path");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Rpc(Failure),
    Database,
    Schema,
    SelectionChanged,
    SettlementFinality,
    SourceBudget,
    Inconsistent,
    ConflictingFact,
    Frozen,
    Bootstrap,
}
impl Error {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rpc(error) => error.label(),
            Self::Database => "database",
            Self::Schema => "schema_or_role",
            Self::SelectionChanged => "selection_changed",
            Self::SettlementFinality => "settled_l2_finality_contradiction",
            Self::SourceBudget => "source_receipt_budget_exhausted",
            Self::Inconsistent => "inconsistent_chain_facts",
            Self::ConflictingFact => "conflicting_fact",
            Self::Frozen => "finalized_contradiction_or_frozen",
            Self::Bootstrap => "history_before_bootstrap_unavailable",
        }
    }
}
impl From<Failure> for Error {
    fn from(error: Failure) -> Self {
        Self::Rpc(error)
    }
}
impl From<tokio_postgres::Error> for Error {
    fn from(error: tokio_postgres::Error) -> Self {
        if let Some(code) = error.code() {
            tracing::warn!(sqlstate = code.code(), "observer database operation failed");
        }
        if error
            .code()
            .is_some_and(|code| code.code().starts_with("23"))
        {
            Self::ConflictingFact
        } else {
            Self::Database
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Block {
    number: u64,
    hash: B256,
    parent: B256,
    timestamp: u64,
}
impl Block {
    fn parse(value: &Value) -> Result<Self, Error> {
        let head = Head::parse(value)?;
        Ok(Self {
            number: head.number,
            hash: hash(&value["hash"])?,
            parent: hash(&value["parentHash"])?,
            timestamp: head.timestamp,
        })
    }
}

#[derive(Debug, Clone)]
struct Transaction {
    hash: B256,
    sender: Address,
    recipient: Option<Address>,
    nonce: U256,
    kind: i16,
    selector: Option<Vec<u8>>,
    posting: crate::posting::Analysis,
    index: u64,
}
fn address(value: &str) -> Result<Address, Error> {
    if value.len() != 42 || !value.starts_with("0x") {
        return Err(Error::Rpc(Failure::Malformed));
    }
    value.parse().map_err(|_| Error::Rpc(Failure::Malformed))
}
impl Transaction {
    fn parse_receipt(&self, block: &Block, value: &Value) -> Result<Receipt, Error> {
        // Retain the scanner's existing error precedence: a known contradictory
        // type is rejected before interpreting any other receipt field.
        if quantity(&value["type"]).is_ok_and(|kind| i16::try_from(kind) != Ok(self.kind)) {
            return Err(Error::Inconsistent);
        }
        let receipt = Receipt::parse(value)?;
        if receipt.block_hash != block.hash
            || receipt.block_number != block.number
            || receipt.transaction_hash != self.hash
            || receipt.transaction_index != self.index
        {
            return Err(Error::Inconsistent);
        }
        Ok(receipt)
    }

    fn parse(value: &Value, block: &Block, index: usize) -> Result<Self, Error> {
        let tx_index = quantity(&value["transactionIndex"])?;
        if tx_index != index as u64
            || hash(&value["blockHash"])? != block.hash
            || quantity(&value["blockNumber"])? != block.number
        {
            return Err(Error::Inconsistent);
        }
        let input = value["input"].as_str().ok_or(Failure::Malformed)?;
        let bytes = input.strip_prefix("0x").ok_or(Failure::Malformed)?;
        if !bytes.len().is_multiple_of(2) || !bytes.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Failure::Malformed.into());
        }
        let selector = if bytes.len() >= 8 {
            Some(alloy_primitives::hex::decode(&bytes[..8]).map_err(|_| Failure::Malformed)?)
        } else {
            None
        };
        let posting = if selector.as_deref() == Some(crate::posting::SELECTOR.as_slice()) {
            crate::posting::Analysis::parse(
                &alloy_primitives::hex::decode(bytes).map_err(|_| Failure::Malformed)?,
            )
        } else {
            crate::posting::Analysis::other()
        };
        let kind = quantity(&value["type"])?;
        if kind > 255 {
            return Err(Failure::Malformed.into());
        }
        Ok(Self {
            hash: hash(&value["hash"])?,
            sender: address(value["from"].as_str().ok_or(Failure::Malformed)?)?,
            recipient: if value.get("to").ok_or(Failure::Malformed)?.is_null() {
                None
            } else {
                Some(address(value["to"].as_str().ok_or(Failure::Malformed)?)?)
            },
            nonce: U256::from_str_radix(quantity_hex(&value["nonce"])?, 16)
                .map_err(|_| Failure::Malformed)?,
            kind: kind as i16,
            selector,
            posting,
            index: tx_index,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub last_success: Option<u64>,
    pub error: Option<Error>,
    pub head: Option<u64>,
    pub cursor: Option<u64>,
    pub cursor_hash: Option<B256>,
    pub verified_cursors: Option<[B256; 2]>,
    pub gaps: Option<i64>,
    pub finalized: Option<u64>,
    pub finality_pending: Option<i64>,
}
#[derive(Debug, Clone, Default)]
pub struct RuntimeStatus {
    pub chains: [Status; 2],
    pub settlements: Option<Status>,
    pub lifecycle: Vec<lifecycle::Status>,
    pub source_receipts: Option<source_receipts::Status>,
}
pub type SharedStatus = Arc<RwLock<RuntimeStatus>>;

pub fn render_settlements(status: &RuntimeStatus, now: u64) -> String {
    use std::fmt::Write;
    let Some(state) = &status.settlements else {
        return String::new();
    };
    let fresh = |state: &Status| {
        state.error.is_none()
            && state
                .last_success
                .is_some_and(|t| t <= now.saturating_add(5000) && now.saturating_sub(t) <= 90000)
    };
    let same_cursors = state.verified_cursors.is_some_and(|expected| {
        status
            .chains
            .iter()
            .zip(expected)
            .all(|(chain, hash)| chain.cursor_hash == Some(hash))
    });
    let valid = fresh(state) && status.chains.iter().all(fresh) && same_cursors;
    let mut text = format!(
        "eez_observer_settlement_collection_success {}\n",
        u8::from(valid)
    );
    if let Some(time) = state.last_success {
        writeln!(
            text,
            "eez_observer_settlement_last_success_timestamp_seconds {}",
            time / 1000
        )
        .unwrap();
    }
    if let Some(error) = state.error {
        writeln!(
            text,
            "eez_observer_settlement_error{{reason=\"{}\"}} 1",
            error.label()
        )
        .unwrap();
    }
    if valid {
        if let Some(gaps) = state.gaps {
            writeln!(text, "eez_observer_settlement_pending_posts {gaps}").unwrap();
        }
        if let Some(pending) = state.finality_pending {
            writeln!(
                text,
                "eez_observer_settlement_finality_pending_ranges {pending}"
            )
            .unwrap();
        }
        if let Some(number) = state.finalized {
            writeln!(text, "eez_observer_settled_finalized_l2_number {number}").unwrap();
        }
    }
    text
}

pub fn render(status: &[Status; 2], now: u64) -> String {
    use std::fmt::Write;
    let mut text = String::new();
    for (index, state) in status.iter().enumerate() {
        let chain = if index == 0 { "l1" } else { "l2" };
        let valid = state.error.is_none()
            && state.last_success.is_some_and(|time| {
                time <= now.saturating_add(5000) && now.saturating_sub(time) <= 90000
            });
        writeln!(
            text,
            "eez_observer_index_collection_success{{chain=\"{chain}\"}} {}",
            u8::from(valid)
        )
        .unwrap();
        if let Some(time) = state.last_success {
            writeln!(
                text,
                "eez_observer_index_last_success_timestamp_seconds{{chain=\"{chain}\"}} {}",
                time / 1000
            )
            .unwrap();
        }
        if let Some(error) = state.error {
            writeln!(
                text,
                "eez_observer_index_error{{chain=\"{chain}\",reason=\"{}\"}} 1",
                error.label()
            )
            .unwrap();
        }
        if valid {
            if let (Some(head), Some(cursor)) = (state.head, state.cursor) {
                writeln!(
                    text,
                    "eez_observer_lag_blocks{{chain=\"{chain}\"}} {}",
                    head.saturating_sub(cursor)
                )
                .unwrap();
            }
            if let Some(gaps) = state.gaps {
                writeln!(
                    text,
                    "eez_observer_index_open_gaps{{chain=\"{chain}\"}} {gaps}"
                )
                .unwrap();
            }
            if let Some(finalized) = state.finalized {
                writeln!(
                    text,
                    "eez_observer_index_finalized_number{{chain=\"{chain}\"}} {finalized}"
                )
                .unwrap();
            }
        }
    }
    text
}

async fn remote_block(rpc: &Rpc, endpoint: &Endpoint, number: u64) -> Result<Block, Error> {
    let block = Block::parse(
        &rpc.block(endpoint, json!(format!("{number:#x}")), false)
            .await?,
    )?;
    if block.number != number {
        return Err(Error::Inconsistent);
    }
    Ok(block)
}

async fn cycle_role(config: &Config, rpc: &Rpc, index: usize) -> Result<Status, Error> {
    let db_config = DatabaseConfig::load(&config.database_config).map_err(|_| Error::Schema)?;
    let mut database = Database::connect(&db_config)
        .await
        .map_err(|_| Error::Database)?;
    database.verify_writer().await.map_err(|_| Error::Schema)?;
    let (endpoint, selection, role) = if index == 0 {
        (&config.exporter.l1, &config.l1, "l1")
    } else {
        (&config.exporter.l2, &config.l2, "l2")
    };
    scan(&mut database, config, rpc, endpoint, selection, role).await
}

async fn publish_result(
    status: &SharedStatus,
    index: usize,
    result: Result<Status, Error>,
) -> Result<(), Error> {
    let mut states = status.write().await;
    match result {
        Ok(state) => {
            states.chains[index] = state;
            Ok(())
        }
        Err(error) => {
            states.chains[index].error = Some(error);
            tracing::warn!(
                chain = if index == 0 { "l1" } else { "l2" },
                reason = error.label(),
                "canonical observer scan failed"
            );
            Err(error)
        }
    }
}

pub async fn once(config: &Config, rpc: &Rpc, status: &SharedStatus) -> Result<(), Error> {
    config.validate().map_err(|_| Error::SelectionChanged)?;
    let (l1, l2) = tokio::join!(cycle_role(config, rpc, 0), cycle_role(config, rpc, 1));
    let l1 = publish_result(status, 0, l1).await;
    let l2 = publish_result(status, 1, l2).await;
    if !config.l1.contracts.is_empty() {
        settlements::poll(config, rpc, status).await;
    }
    lifecycle::once(config, status).await;
    source_receipts::poll(config, rpc, status).await;
    l1.and(l2)
}

pub async fn run(config: Config, rpc: Rpc, status: SharedStatus) {
    async fn worker(config: &Config, rpc: &Rpc, status: &SharedStatus, index: usize) {
        let mut failures = 0u32;
        loop {
            let result = publish_result(status, index, cycle_role(config, rpc, index).await).await;
            failures = if result.is_ok() {
                0
            } else {
                failures.saturating_add(1)
            };
            tokio::time::sleep(Duration::from_secs(crate::retry_delay(failures))).await;
        }
    }
    // Slow history/reorg discovery on one chain cannot hold up the other chain.
    tokio::join!(
        worker(&config, &rpc, &status, 0),
        worker(&config, &rpc, &status, 1),
        settlements::run(&config, &rpc, &status),
        lifecycle::run(&config, &status),
        source_receipts::run(&config, &rpc, &status)
    );
}

async fn scan(
    database: &mut Database,
    config: &Config,
    rpc: &Rpc,
    endpoint: &Endpoint,
    selection: &Selection,
    role: &str,
) -> Result<Status, Error> {
    let existing = ledger::find_chain(database, config, endpoint, role).await?;
    if let Err(error) = rpc.validate_identity(endpoint).await {
        if let Some(id) = existing {
            ledger::mark_failure(database, id, error.into()).await?;
        }
        return Err(error.into());
    }
    let id = match ledger::register(database, config, endpoint, selection, role).await {
        Ok(id) => id,
        Err(error) => {
            if let Some(id) = existing {
                ledger::mark_failure(database, id, error).await?;
            }
            return Err(error);
        }
    };
    let result = scan_chain(
        database,
        id,
        config.batch_blocks,
        rpc,
        endpoint,
        selection,
        role,
    )
    .await;
    if let Err(error) = result {
        ledger::mark_failure(database, id, error).await?;
    }
    result
}

async fn scan_chain(
    database: &mut Database,
    id: i64,
    batch: u64,
    rpc: &Rpc,
    endpoint: &Endpoint,
    selection: &Selection,
    role: &str,
) -> Result<Status, Error> {
    let mut cursor = ledger::cursor(database, id).await?;
    if let Some(anchor) = ledger::finalized(database, id).await?
        && remote_block(rpc, endpoint, anchor.number).await?.hash != anchor.hash
    {
        // Repointing an endpoint during collection is an identity failure,
        // not evidence that this deployment's finalized chain contradicted itself.
        rpc.validate_identity(endpoint).await?;
        return Err(Error::Frozen);
    }
    let head = Block::parse(&rpc.block(endpoint, json!("latest"), false).await?)?;
    if head.number < selection.bootstrap {
        return Err(Error::Bootstrap);
    }
    if let Some(old) = cursor.as_ref() {
        let matches = if old.number <= head.number {
            remote_block(rpc, endpoint, old.number).await?.hash == old.hash
        } else {
            false
        };
        if !matches {
            let mut ancestor = Some(old.clone());
            let mut walked = 0;
            while let Some(block) = ancestor.as_ref() {
                if block.number <= head.number
                    && remote_block(rpc, endpoint, block.number).await?.hash == block.hash
                {
                    break;
                }
                walked += 1;
                // Discovery retains constant memory and yields after each bounded
                // chunk, so a deep unfinalized reorg remains recoverable without
                // an unbounded burst of history requests.
                if walked == batch {
                    walked = 0;
                    tokio::time::sleep(Duration::from_secs(15)).await;
                    rpc.validate_identity(endpoint).await?;
                    if remote_block(rpc, endpoint, block.number).await?.hash == block.hash {
                        return Err(Error::Inconsistent); // fork changed during discovery; restart it
                    }
                }
                let next = ledger::parent(database, id, block).await?;
                if next
                    .as_ref()
                    .is_some_and(|parent| parent.number.checked_add(1) != Some(block.number))
                    || (next.is_none() && block.number > selection.bootstrap)
                {
                    return Err(Error::ConflictingFact);
                }
                ancestor = next;
            }
            if let Some(anchor) = ancestor.as_ref()
                && remote_block(rpc, endpoint, anchor.number).await?.hash != anchor.hash
            {
                return Err(Error::Inconsistent);
            }
            ledger::retract(database, id, cursor.as_ref(), ancestor.as_ref()).await?;
            cursor = ancestor;
        }
    }
    let first = match cursor.as_ref() {
        Some(block) => block.number.checked_add(1),
        None => Some(selection.bootstrap),
    };
    if let Some(first) = first {
        for number in first..=head.number.min(first.saturating_add(batch - 1)) {
            let (block, observations) =
                match fetch_observations(rpc, endpoint, selection, number).await {
                    Ok(value) => value,
                    Err(error) => {
                        ledger::record_block_gap(database, id, number, error).await?;
                        return Err(error);
                    }
                };
            if cursor.as_ref().is_some_and(|parent| {
                block.parent != parent.hash || parent.number.checked_add(1) != Some(number)
            }) {
                return Err(Error::Inconsistent);
            }
            rpc.validate_identity(endpoint).await?;
            // Validate both ends immediately before committing the parent-linked slice.
            if remote_block(rpc, endpoint, block.number).await?.hash != block.hash {
                return Err(Error::Inconsistent);
            }
            if let Some(parent) = cursor.as_ref() {
                if remote_block(rpc, endpoint, parent.number).await?.hash != parent.hash {
                    return Err(Error::Inconsistent);
                }
            } else if block.number > 0
                && remote_block(rpc, endpoint, block.number - 1).await?.hash != block.parent
            {
                return Err(Error::Inconsistent);
            }
            ledger::commit_block(database, id, cursor.as_ref(), &block, &observations).await?;
            cursor = Some(block);
        }
    }
    // Retry a bounded, fair rotation of historical receipt gaps independently
    // of the new-block cursor. Pruned or missing receipts cannot starve headers.
    for pending in ledger::pending_blocks(database, id).await? {
        ledger::touch_receipt_attempt(database, id, &pending).await?;
        if let Ok((block, observations)) =
            fetch_observations(rpc, endpoint, selection, pending.number).await
            && block == pending
            && remote_block(rpc, endpoint, block.number).await?.hash == block.hash
        {
            rpc.validate_identity(endpoint).await?;
            ledger::repair_block(database, id, &block, &observations).await?;
        }
    }
    // Old schema rows may lack method evidence. A bounded fair retry lane
    // repairs classification independently from receipt/header progress.
    for (hash, selector) in ledger::pending_posting(database, id).await? {
        ledger::touch_posting_attempt(database, id, hash).await?;
        if selector.as_deref() != Some(crate::posting::SELECTOR.as_slice()) {
            ledger::save_posting(
                &database.client,
                id,
                hash,
                &crate::posting::Analysis::other(),
            )
            .await?;
            continue;
        }
        let transaction = match fetch_transaction(rpc, endpoint, hash).await {
            Ok(transaction) => transaction,
            Err(error) => {
                ledger::posting_failure(database, id, hash, error).await?;
                continue; // A failed method fetch never erases known receipt charges.
            }
        };
        rpc.validate_identity(endpoint).await?;
        ledger::repair_posting(database, id, &transaction).await?;
    }
    // L2 finality is deliberately NOT taken from its RPC tag. Applied settlement
    // hash/range verification supplies L2 finality in the correlation index.
    if role == "l1"
        && let Ok(value) = rpc.block(endpoint, json!("finalized"), false).await
        && !value.is_null()
    {
        let finalized = Block::parse(&value)?;
        if cursor.as_ref().is_some_and(|cursor| {
            finalized.number >= selection.bootstrap && finalized.number <= cursor.number
        }) {
            if remote_block(rpc, endpoint, finalized.number).await?.hash != finalized.hash {
                return Err(Error::Inconsistent);
            }
            ledger::finalize(database, id, &finalized).await?;
        }
    }
    if let Some(cursor) = cursor.as_ref()
        && remote_block(rpc, endpoint, cursor.number).await?.hash != cursor.hash
    {
        return Err(Error::Inconsistent);
    }
    ledger::mark_success(database, id).await?;
    Ok(Status {
        last_success: Some(crate::now_ms()),
        error: None,
        head: Some(head.number),
        cursor_hash: cursor.as_ref().map(|block| block.hash),
        cursor: cursor.map(|block| block.number),
        verified_cursors: None,
        gaps: Some(ledger::gaps(database, id).await?),
        finality_pending: None,
        finalized: ledger::finalized(database, id)
            .await?
            .map(|block| block.number),
    })
}

async fn fetch_transaction(
    rpc: &Rpc,
    endpoint: &Endpoint,
    expected: B256,
) -> Result<Transaction, Error> {
    let value = rpc.transaction(endpoint, expected).await?;
    let block = Block {
        number: quantity(&value["blockNumber"])?,
        hash: hash(&value["blockHash"])?,
        parent: B256::ZERO,
        timestamp: 0,
    };
    let index =
        usize::try_from(quantity(&value["transactionIndex"])?).map_err(|_| Failure::Malformed)?;
    let transaction = Transaction::parse(&value, &block, index)?;
    if transaction.hash != expected {
        return Err(Error::Inconsistent);
    }
    Ok(transaction)
}

async fn fetch_observations(
    rpc: &Rpc,
    endpoint: &Endpoint,
    selection: &Selection,
    number: u64,
) -> Result<(Block, Vec<(Transaction, Option<Receipt>)>), Error> {
    let value = rpc
        .block(endpoint, json!(format!("{number:#x}")), true)
        .await?;
    let block = Block::parse(&value)?;
    if block.number != number {
        return Err(Error::Inconsistent);
    }
    let values = value["transactions"].as_array().ok_or(Failure::Malformed)?;
    if values.len() > 10000 {
        return Err(Failure::Oversized.into());
    }
    let mut selected = Vec::new();
    let mut hashes = HashSet::new();
    for (index, value) in values.iter().enumerate() {
        let transaction = Transaction::parse(value, &block, index)?;
        if !hashes.insert(transaction.hash) {
            return Err(Error::Inconsistent);
        }
        if selection.includes(&transaction) {
            selected.push(transaction);
        }
    }
    let mut observations = Vec::with_capacity(selected.len());
    let mut receipts_unavailable = false;
    for transaction in selected {
        let observed = if receipts_unavailable {
            Ok(Value::Null)
        } else {
            rpc.call(
                &endpoint.url,
                "eth_getTransactionReceipt",
                json!([format!("{:#x}", transaction.hash)]),
            )
            .await
        };
        let receipt = match observed {
            Ok(value) if !value.is_null() => Some(transaction.parse_receipt(&block, &value)?),
            Err(Failure::Unsupported | Failure::Http | Failure::Transport) => {
                // A failed service must not incur thousands of sequential timeouts.
                // Keep every selected transaction as a retryable receipt gap.
                receipts_unavailable = true;
                None
            }
            Ok(_) | Err(Failure::Rpc) => None,
            Err(error) => return Err(error.into()),
        };

        observations.push((transaction, receipt));
    }
    Ok((block, observations))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_value() -> Value {
        let endpoint = json!({"url":"http://127.0.0.1:8545", "chain_id":1,
            "genesis_hash":format!("{:#x}", B256::ZERO)});
        json!({"exporter":{"l1":endpoint,"l2":endpoint},"environment":"test","deployment":"fixture",
            "rollup_id":1,"database_config":"writer.json","l1":{"bootstrap":1,"wallets":[format!("{:#x}",Address::repeat_byte(0xab))]},
            "l2":{"bootstrap":0,"include_all_transactions":true}})
    }

    #[test]
    fn receipt_reuse_preserves_type_precedence_and_exact_inclusion_checks() {
        let block = Block {
            number: 1,
            hash: B256::repeat_byte(1),
            parent: B256::ZERO,
            timestamp: 1,
        };
        let tx = Transaction {
            hash: B256::repeat_byte(2),
            sender: Address::ZERO,
            recipient: None,
            nonce: U256::ZERO,
            kind: 2,
            selector: None,
            posting: crate::posting::Analysis::other(),
            index: 3,
        };
        assert_eq!(
            tx.parse_receipt(&block, &json!({"type":"0x3"})),
            Err(Error::Inconsistent)
        );
        assert_eq!(
            tx.parse_receipt(&block, &json!({"type":"0x2"})),
            Err(Error::Rpc(Failure::Malformed))
        );
        let value = json!({"type":"0x2","transactionHash":format!("{:#x}",tx.hash),"blockHash":format!("{:#x}",block.hash),"blockNumber":"0x1","transactionIndex":"0x3","status":"0x1","gasUsed":"0x5208","effectiveGasPrice":"0x7"});
        let receipt = tx.parse_receipt(&block, &value).unwrap();
        assert!(receipt.success);
        assert!(receipt.cost.is_ok());
        for (key, invalid) in [
            ("blockNumber", json!("0x2")),
            ("transactionIndex", json!("0x4")),
            ("transactionHash", json!(format!("{:#x}", B256::ZERO))),
            ("blockHash", json!(format!("{:#x}", B256::ZERO))),
        ] {
            let mut changed = value.clone();
            changed[key] = invalid;
            assert_eq!(tx.parse_receipt(&block, &changed), Err(Error::Inconsistent));
        }
        let mut missing_type = value;
        missing_type.as_object_mut().unwrap().remove("type");
        let receipt = tx.parse_receipt(&block, &missing_type).unwrap();
        assert!(
            receipt.cost.is_err(),
            "missing type remains identified with unknown charges"
        );
    }

    #[test]
    fn configuration_defaults_bounds_and_selection_are_explicit() {
        let value = config_value();
        let config: Config = serde_json::from_value(value.clone()).unwrap();
        config.validate().unwrap();
        assert_eq!(config.batch_blocks, 16);
        assert!(!config.l1.include_all_transactions);
        assert!(config.l2.include_all_transactions);
        assert!(config.exporter.listen.ip().is_loopback());
        for (field, invalid) in [
            ("batch_blocks", json!(0)),
            ("batch_blocks", json!(65)),
            ("rollup_id", json!(0)),
            ("deployment", json!("unbounded label with spaces")),
            ("database_config", json!("")),
        ] {
            let mut changed = value.clone();
            changed[field] = invalid;
            assert!(
                serde_json::from_value::<Config>(changed)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        for bound in [1, 64] {
            let mut changed = value.clone();
            changed["batch_blocks"] = json!(bound);
            serde_json::from_value::<Config>(changed)
                .unwrap()
                .validate()
                .unwrap();
        }
        let mut selection = config.l1;
        selection.wallets.clear();
        assert!(selection.validate().is_err());
        selection.wallets = vec![
            format!("{:#x}", Address::repeat_byte(0xab)),
            format!("0x{}", "AB".repeat(20)),
        ];
        assert!(selection.validate().is_err());
        selection.wallets = vec!["0xab".into()];
        assert!(selection.validate().is_err());
        let mut changed = value;
        changed["unsupported"] = json!(true);
        assert!(serde_json::from_value::<Config>(changed).is_err());
    }

    #[test]
    fn selection_policy_is_order_and_case_independent() {
        let mut first: Config = serde_json::from_value(config_value()).unwrap();
        first
            .l1
            .wallets
            .push(format!("{:#x}", Address::repeat_byte(0xcd)));
        let mut second = first.l1.clone();
        second.wallets.reverse();
        second.wallets[0] = format!("0x{}", "CD".repeat(20));
        assert_eq!(first.l1.policy(), second.policy());
    }

    #[test]
    fn transaction_identity_and_optional_creation_are_checked() {
        let block = Block {
            number: 7,
            hash: B256::repeat_byte(1),
            parent: B256::ZERO,
            timestamp: 42,
        };
        let value = json!({"hash":format!("{:#x}",B256::repeat_byte(2)), "blockHash":format!("{:#x}",block.hash),
            "blockNumber":"0x7","transactionIndex":"0x0","from":format!("{:#x}",Address::repeat_byte(0xab)),
            "to":null,"nonce":"0x0","type":"0x2","input":"0x1234567800"});
        let tx = Transaction::parse(&value, &block, 0).unwrap();
        assert_eq!(tx.selector, Some(vec![0x12, 0x34, 0x56, 0x78]));
        assert!(tx.recipient.is_none());
        let config: Config = serde_json::from_value(config_value()).unwrap();
        assert!(config.l1.includes(&tx));
        for (field, invalid) in [
            ("transactionIndex", json!("0x1")),
            ("blockNumber", json!("0x8")),
            ("blockHash", json!(format!("{:#x}", B256::ZERO))),
            ("input", json!("0x0")),
            ("type", json!("0x100")),
        ] {
            let mut changed = value.clone();
            changed[field] = invalid;
            assert!(Transaction::parse(&changed, &block, 0).is_err());
        }
        let mut changed = value;
        changed.as_object_mut().unwrap().remove("to");
        assert!(Transaction::parse(&changed, &block, 0).is_err());
    }

    #[test]
    fn stale_failed_or_future_indexing_exposes_unknown_without_old_lag() {
        let mut states = [
            Status {
                last_success: Some(100_000),
                error: None,
                head: Some(10),
                cursor: Some(7),
                gaps: Some(2),
                finalized: Some(5),
                finality_pending: None,
                cursor_hash: None,
                verified_cursors: None,
            },
            Status::default(),
        ];
        assert!(render(&states, 190_000).contains("eez_observer_lag_blocks{chain=\"l1\"} 3"));
        for now in [190_001, 94_999] {
            let text = render(&states, now);
            assert!(!text.contains("eez_observer_lag_blocks"));
            assert!(
                text.contains(
                    "eez_observer_index_last_success_timestamp_seconds{chain=\"l1\"} 100"
                )
            );
        }
        states[0].error = Some(Error::Database);
        assert!(!render(&states, 100_000).contains("eez_observer_lag_blocks"));
    }
    #[test]
    fn settlement_finality_metrics_require_fresh_chain_and_correlation_sources() {
        let mut state = RuntimeStatus::default();
        assert!(render_settlements(&state, 100_000).is_empty());
        let fresh = Status {
            last_success: Some(100_000),
            cursor_hash: Some(B256::ZERO),
            ..Status::default()
        };
        state.chains = [fresh.clone(), fresh.clone()];
        state.settlements = Some(Status {
            finalized: Some(42),
            gaps: Some(0),
            verified_cursors: Some([B256::ZERO; 2]),
            ..fresh
        });
        assert!(
            render_settlements(&state, 100_000)
                .contains("eez_observer_settled_finalized_l2_number 42")
        );
        assert!(
            !render_settlements(&state, 190_001)
                .contains("eez_observer_settled_finalized_l2_number")
        );
        state.chains[1].cursor_hash = Some(B256::repeat_byte(1));
        assert!(
            !render_settlements(&state, 100_000)
                .contains("eez_observer_settled_finalized_l2_number")
        );
        state.chains[1].cursor_hash = Some(B256::ZERO);
        state.chains[1].error = Some(Error::Rpc(Failure::Identity));
        assert!(
            !render_settlements(&state, 100_000)
                .contains("eez_observer_settled_finalized_l2_number")
        );
        state.chains[1].error = None;
        state.settlements.as_mut().unwrap().error = Some(Error::SettlementFinality);
        let text = render_settlements(&state, 100_000);
        assert!(!text.contains("eez_observer_settled_finalized_l2_number"));
        assert!(text.contains("settled_l2_finality_contradiction"));
        assert!(text.contains("eez_observer_settlement_last_success_timestamp_seconds 100"));
    }
}
