use alloy_primitives::{Address, B256, U256};
use alloy_sol_types::{SolEvent, sol};
use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::config::Config;

// Narrow mirror of the pinned ABI, avoiding a dependency on the node/Reth graph.
sol! { event L2ExecutionPerformed(uint64 indexed rollupId, bytes32 newRoot); }

pub fn quantity(v: &Value) -> Result<U256> {
    let s = v.as_str().context("missing hex quantity")?;
    let hex = s.strip_prefix("0x").context("quantity without 0x")?;
    ensure!(
        !hex.is_empty() && (hex.len() == 1 || !hex.starts_with('0')),
        "noncanonical quantity"
    );
    ensure!(
        hex.len() <= 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid quantity"
    );
    Ok(U256::from_str_radix(hex, 16)?)
}

pub fn number(v: &Value) -> Result<u64> {
    Ok(quantity(v)?.try_into()?)
}
pub fn hash(v: &Value) -> Result<B256> {
    Ok(v.as_str().context("missing hash")?.parse()?)
}

#[derive(Clone, Debug)]
pub struct Block {
    pub number: u64,
    pub hash: B256,
    pub parent: B256,
    pub timestamp: u64,
    pub transactions: Vec<B256>,
}

impl Block {
    pub fn parse(v: &Value) -> Result<Self> {
        let block = Self {
            number: number(&v["number"])?,
            hash: hash(&v["hash"])?,
            parent: hash(&v["parentHash"])?,
            timestamp: number(&v["timestamp"])?,
            transactions: v["transactions"]
                .as_array()
                .context("missing transactions")?
                .iter()
                .map(hash)
                .collect::<Result<_>>()?,
        };
        ensure!(
            block.number < i64::MAX as u64 && block.timestamp < i64::MAX as u64,
            "block outside database range"
        );
        Ok(block)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub block_hash: B256,
    pub block_number: u64,
    pub transaction: B256,
    pub transaction_index: u64,
    pub log_index: u64,
    pub endpoint: B256,
}

impl Event {
    pub fn parse(v: &Value, config: &Config) -> Result<Self> {
        ensure!(
            v["address"]
                .as_str()
                .context("missing emitter")?
                .parse::<Address>()?
                == config.registry,
            "wrong event emitter"
        );
        ensure!(v["removed"] == false, "removed or unclassified log");
        let topics = v["topics"].as_array().context("missing topics")?;
        ensure!(
            topics.len() == 2 && hash(&topics[0])? == L2ExecutionPerformed::SIGNATURE_HASH,
            "wrong event signature"
        );
        ensure!(
            hash(&topics[1])? == B256::from(U256::from(config.rollup_id)),
            "wrong rollup"
        );
        Ok(Self {
            block_hash: hash(&v["blockHash"])?,
            block_number: number(&v["blockNumber"])?,
            transaction: hash(&v["transactionHash"])?,
            transaction_index: number(&v["transactionIndex"])?,
            log_index: number(&v["logIndex"])?,
            endpoint: hash(&v["data"])?,
        })
    }
    pub fn validate_block(&self, block: &Block) -> Result<()> {
        ensure!(
            self.block_hash == block.hash && self.block_number == block.number,
            "log/block mismatch"
        );
        ensure!(
            block
                .transactions
                .get(usize::try_from(self.transaction_index)?)
                == Some(&self.transaction),
            "transaction position mismatch"
        );
        ensure!(
            self.log_index < i64::MAX as u64,
            "log index outside database range"
        );
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct Receipt {
    pub transaction: B256,
    pub index: u64,
    pub gas_used: U256,
    pub gas_price: U256,
    pub execution_fee: U256,
    pub blob_gas_used: U256,
    pub blob_gas_price: U256,
    pub blob_fee: U256,
}

impl Receipt {
    pub fn parse(v: &Value, block: &Block, expected: &[Event], config: &Config) -> Result<Self> {
        let first = expected.first().context("receipt without event")?;
        ensure!(
            hash(&v["transactionHash"])? == first.transaction
                && number(&v["transactionIndex"])? == first.transaction_index,
            "receipt transaction mismatch"
        );
        ensure!(
            hash(&v["blockHash"])? == block.hash && number(&v["blockNumber"])? == block.number,
            "receipt block mismatch"
        );
        ensure!(number(&v["status"])? == 1, "settlement receipt failed");
        let mut events = Vec::new();
        for log in v["logs"].as_array().context("missing receipt logs")? {
            let emitter = log["address"]
                .as_str()
                .context("missing receipt emitter")?
                .parse::<Address>()?;
            if emitter == config.registry
                && log["topics"]
                    .as_array()
                    .and_then(|topics| topics.first())
                    .map(hash)
                    .transpose()?
                    == Some(L2ExecutionPerformed::SIGNATURE_HASH)
                && hash(&log["topics"][1])? == B256::from(U256::from(config.rollup_id))
            {
                let event = Event::parse(log, config)?;
                event.validate_block(block)?;
                ensure!(
                    event.transaction == first.transaction
                        && event.transaction_index == first.transaction_index,
                    "receipt log transaction mismatch"
                );
                events.push(event);
            }
        }
        events.sort_by_key(|e| e.log_index);
        ensure!(
            events == expected,
            "receipt logs disagree with indexed events"
        );
        let gas_used = quantity(&v["gasUsed"])?;
        let gas_price = quantity(&v["effectiveGasPrice"])?;
        let is_blob = number(&v["type"])? == 3;
        let (blob_gas_used, blob_gas_price) =
            if is_blob || !v["blobGasUsed"].is_null() || !v["blobGasPrice"].is_null() {
                (quantity(&v["blobGasUsed"])?, quantity(&v["blobGasPrice"])?)
            } else {
                (U256::ZERO, U256::ZERO)
            };
        Ok(Self {
            transaction: first.transaction,
            index: first.transaction_index,
            gas_used,
            gas_price,
            execution_fee: gas_used
                .checked_mul(gas_price)
                .context("execution fee overflow")?,
            blob_gas_used,
            blob_gas_price,
            blob_fee: blob_gas_used
                .checked_mul(blob_gas_price)
                .context("blob fee overflow")?,
        })
    }
}
