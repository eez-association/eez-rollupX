//! Persistent, reorg-aware L1 settlement ↔ L2 block correlation index.
//!
//! The index is deliberately separate from reth's chain database. It is derived
//! entirely from authenticated L1 batches and local L2 block hashes, and can be
//! rebuilt by replaying L1. Four small MDBX tables provide constant-time lookup
//! by L2 number/hash and an ordered lookup by L1 block. An in-memory backend is
//! available when no node datadir was configured; it has identical semantics but
//! naturally does not survive restart.

use std::path::Path;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use alloy_primitives::B256;
use reth_libmdbx::{DatabaseFlags, Environment, Geometry, WriteFlags};
use serde::{Deserialize, Serialize};

const SCHEMA: &[u8] = b"eez-batch-correlation-v2";
const META_SCHEMA_KEY: &[u8] = b"schema";
const RECORDS_DB: &str = "records";
const L1_ORDER_DB: &str = "l1_order";
const L1_HASH_DB: &str = "l1_hash";
const L2_NUMBER_DB: &str = "l2_number";
const L2_HASH_DB: &str = "l2_hash";
const GIGABYTE: usize = 1024 * 1024 * 1024;

/// One exact L2 block committed by an accepted L1 batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L2BlockCorrelation {
    pub number: u64,
    pub hash: B256,
}

/// Canonical L1 provenance for the settled part of one batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchCorrelation {
    pub l1_block_number: u64,
    pub l1_block_hash: B256,
    pub l1_transaction_hash: B256,
    pub l1_transaction_index: u64,
    /// Exact canonical L2 hashes covered by the batch's settled portion.
    pub l2_blocks: Vec<L2BlockCorrelation>,
}

impl BatchCorrelation {
    fn validate(&self) -> Result<(), String> {
        if self.l2_blocks.is_empty() {
            return Err("correlation record has no L2 blocks".into());
        }
        for pair in self.l2_blocks.windows(2) {
            if pair[1].number != pair[0].number.saturating_add(1) {
                return Err(format!(
                    "correlation record has non-contiguous L2 numbers {} then {}",
                    pair[0].number, pair[1].number
                ));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn first_l2(&self) -> L2BlockCorrelation {
        self.l2_blocks[0]
    }

    #[must_use]
    pub fn last_l2(&self) -> L2BlockCorrelation {
        self.l2_blocks[self.l2_blocks.len() - 1]
    }

    const fn order(&self) -> (u64, u64) {
        (self.l1_block_number, self.l1_transaction_index)
    }
}

/// Result of an L2 lookup. `canonical_l2` is false when a later accepted batch
/// replaced the same L2 height with another hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorrelationMatch {
    pub batch: BatchCorrelation,
    pub l2_block: L2BlockCorrelation,
    pub canonical_l2: bool,
}

/// Query/write handle shared by the deriver and JSON-RPC server.
pub struct CorrelationStore {
    backend: Backend,
    ready: AtomicBool,
    /// Ephemeral execution evidence, deliberately absent from the MDBX schema.
    pub replay_evidence: crate::diagnostics::ReplayEvidence,
    pub required_blobs: std::sync::Arc<eez_l1::blob_diagnostics::RequiredBlobState>,
}

enum Backend {
    Persistent(Environment),
    Memory(RwLock<Vec<BatchCorrelation>>),
}

impl std::fmt::Debug for CorrelationStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CorrelationStore")
            .field(
                "persistent",
                &matches!(self.backend, Backend::Persistent(_)),
            )
            .finish_non_exhaustive()
    }
}

impl CorrelationStore {
    /// Open or create a dedicated MDBX index below `path`.
    pub fn open(path: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(path)
            .map_err(|error| format!("create correlation index {}: {error}", path.display()))?;
        let mut builder = Environment::builder();
        builder.set_max_dbs(6);
        builder.set_geometry(Geometry {
            size: Some(0..64 * GIGABYTE),
            growth_step: Some(64 * 1024 * 1024),
            shrink_threshold: None,
            page_size: None,
        });
        let env = builder
            .open(path)
            .map_err(|error| format!("open correlation index {}: {error}", path.display()))?;
        let txn = env
            .begin_rw_txn()
            .map_err(|error| format!("begin correlation schema transaction: {error}"))?;
        let meta = txn
            .create_db(None, DatabaseFlags::empty())
            .map_err(|error| format!("open correlation metadata: {error}"))?;
        for name in [
            RECORDS_DB,
            L1_ORDER_DB,
            L1_HASH_DB,
            L2_NUMBER_DB,
            L2_HASH_DB,
        ] {
            txn.create_db(Some(name), DatabaseFlags::empty())
                .map_err(|error| format!("open correlation table {name}: {error}"))?;
        }
        let stored: Option<Vec<u8>> = txn
            .get(meta.dbi(), META_SCHEMA_KEY)
            .map_err(|error| format!("read correlation schema: {error}"))?;
        match stored {
            Some(version) if version != SCHEMA => {
                return Err(format!(
                    "unsupported correlation schema {:?}",
                    String::from_utf8_lossy(&version)
                ));
            }
            None => txn
                .put(meta.dbi(), META_SCHEMA_KEY, SCHEMA, WriteFlags::UPSERT)
                .map_err(|error| format!("write correlation schema: {error}"))?,
            Some(_) => {}
        }
        txn.commit()
            .map_err(|error| format!("commit correlation schema: {error}"))?;
        Ok(Self {
            backend: Backend::Persistent(env),
            ready: AtomicBool::new(false),
            replay_evidence: crate::diagnostics::ReplayEvidence::default(),
            required_blobs: std::sync::Arc::default(),
        })
    }

    /// Non-persistent fallback used when the node has no configured datadir.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            backend: Backend::Memory(RwLock::new(Vec::new())),
            ready: AtomicBool::new(false),
            replay_evidence: crate::diagnostics::ReplayEvidence::default(),
            required_blobs: std::sync::Arc::default(),
        }
    }

    #[must_use]
    pub const fn is_persistent(&self) -> bool {
        matches!(self.backend, Backend::Persistent(_))
    }

    /// Allow public queries after boot catch-up has validated the persistent
    /// tail against canonical L1 and local L2.
    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    /// Insert one accepted batch atomically. Re-inserting the same transaction
    /// with different contents is rejected. L2 indices only move forward in L1
    /// transaction order, which makes catch-up replays idempotent.
    pub fn put(&self, record: &BatchCorrelation) -> Result<(), String> {
        record.validate()?;
        match &self.backend {
            Backend::Persistent(env) => put_persistent(env, record),
            Backend::Memory(records) => {
                let mut records = records
                    .write()
                    .map_err(|_| "correlation memory lock poisoned".to_string())?;
                if let Some(existing) = records
                    .iter()
                    .find(|item| item.l1_transaction_hash == record.l1_transaction_hash)
                {
                    return if existing == record {
                        Ok(())
                    } else {
                        Err(format!(
                            "L1 transaction {} already has different correlation data",
                            record.l1_transaction_hash
                        ))
                    };
                }
                records.push(record.clone());
                records.sort_by_key(BatchCorrelation::order);
                Ok(())
            }
        }
    }

    /// Remove all records above the surviving L1 ancestor and restore L2
    /// number/hash indices to the latest surviving transaction.
    pub fn remove_after_l1(&self, ancestor: u64) -> Result<usize, String> {
        self.replay_evidence.remove_after_l1(ancestor);
        self.required_blobs.clear();
        match &self.backend {
            Backend::Persistent(env) => remove_after_persistent(env, ancestor),
            Backend::Memory(records) => {
                let mut records = records
                    .write()
                    .map_err(|_| "correlation memory lock poisoned".to_string())?;
                let before = records.len();
                records.retain(|record| record.l1_block_number <= ancestor);
                Ok(before - records.len())
            }
        }
    }

    /// Exact immutable batch lookup without materializing every batch in its L1 block.
    pub fn by_l1_transaction_hash(&self, hash: B256) -> Result<Option<BatchCorrelation>, String> {
        match &self.backend {
            Backend::Persistent(env) => persistent_record(env, hash),
            Backend::Memory(records) => Ok(records
                .read()
                .map_err(|_| "correlation memory lock poisoned".to_string())?
                .iter()
                .find(|record| record.l1_transaction_hash == hash)
                .cloned()),
        }
    }

    /// Check a slice from an immutable batch against current canonical indices.
    /// Never decode the entire (up to 65,536-block) batch once per page member.
    pub fn canonical_l2_blocks(
        &self,
        transaction_hash: B256,
        blocks: &[L2BlockCorrelation],
    ) -> Result<bool, String> {
        match &self.backend {
            Backend::Persistent(env) => {
                let txn = env
                    .begin_ro_txn()
                    .map_err(|error| format!("begin correlation read: {error}"))?;
                let numbers = txn
                    .open_db(Some(L2_NUMBER_DB))
                    .map_err(|error| format!("open L2-number index: {error}"))?;
                let hashes = txn
                    .open_db(Some(L2_HASH_DB))
                    .map_err(|error| format!("open L2-hash index: {error}"))?;
                for block in blocks {
                    let number_owner: Option<Vec<u8>> = txn
                        .get(numbers.dbi(), &block.number.to_be_bytes())
                        .map_err(|error| format!("read canonical number: {error}"))?;
                    let hash_owner: Option<Vec<u8>> = txn
                        .get(hashes.dbi(), block.hash.as_slice())
                        .map_err(|error| format!("read canonical hash: {error}"))?;
                    if number_owner.as_deref() != Some(transaction_hash.as_slice())
                        || hash_owner.as_deref() != Some(transaction_hash.as_slice())
                    {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Backend::Memory(records) => {
                let records = records
                    .read()
                    .map_err(|_| "correlation memory lock poisoned".to_string())?;
                Ok(blocks.iter().all(|block| {
                    records
                        .iter()
                        .rev()
                        .find_map(|record| {
                            let offset = usize::try_from(
                                block.number.checked_sub(record.first_l2().number)?,
                            )
                            .ok()?;
                            record.l2_blocks.get(offset).map(|current| {
                                record.l1_transaction_hash == transaction_hash
                                    && current.hash == block.hash
                            })
                        })
                        .unwrap_or(false)
                }))
            }
        }
    }

    pub fn by_l2_number(&self, number: u64) -> Result<Option<CorrelationMatch>, String> {
        let record = match &self.backend {
            Backend::Persistent(env) => {
                let Some(tx_hash) = index_get(env, L2_NUMBER_DB, &number.to_be_bytes())? else {
                    return Ok(None);
                };
                Some(
                    persistent_record(env, db_hash(&tx_hash, "L2-number index value")?)?
                        .ok_or_else(|| {
                            "L2-number index points at a missing correlation record".to_string()
                        })?,
                )
            }
            Backend::Memory(records) => records
                .read()
                .map_err(|_| "correlation memory lock poisoned".to_string())?
                .iter()
                .rev()
                .find(|record| record.l2_blocks.iter().any(|block| block.number == number))
                .cloned(),
        };
        let Some(batch) = record else {
            return Ok(None);
        };
        let l2_block = batch
            .l2_blocks
            .iter()
            .find(|block| block.number == number)
            .copied()
            .ok_or_else(|| "L2-number index points at a record without that number".to_string())?;
        Ok(Some(CorrelationMatch {
            batch,
            l2_block,
            canonical_l2: true,
        }))
    }

    pub fn by_l2_hash(&self, hash: B256) -> Result<Option<CorrelationMatch>, String> {
        let record = match &self.backend {
            Backend::Persistent(env) => {
                let Some(tx_hash) = index_get(env, L2_HASH_DB, hash.as_slice())? else {
                    return Ok(None);
                };
                Some(
                    persistent_record(env, db_hash(&tx_hash, "L2-hash index value")?)?.ok_or_else(
                        || "L2-hash index points at a missing correlation record".to_string(),
                    )?,
                )
            }
            Backend::Memory(records) => records
                .read()
                .map_err(|_| "correlation memory lock poisoned".to_string())?
                .iter()
                .rev()
                .find(|record| record.l2_blocks.iter().any(|block| block.hash == hash))
                .cloned(),
        };
        let Some(batch) = record else {
            return Ok(None);
        };
        let l2_block = batch
            .l2_blocks
            .iter()
            .find(|block| block.hash == hash)
            .copied()
            .ok_or_else(|| "L2-hash index points at a record without that hash".to_string())?;
        let canonical_l2 = self
            .by_l2_number(l2_block.number)?
            .is_some_and(|current| current.l2_block.hash == hash);
        Ok(Some(CorrelationMatch {
            batch,
            l2_block,
            canonical_l2,
        }))
    }

    pub fn by_l1_number(&self, number: u64) -> Result<Vec<BatchCorrelation>, String> {
        match &self.backend {
            Backend::Persistent(env) => records_in_l1_block(env, number),
            Backend::Memory(records) => Ok(records
                .read()
                .map_err(|_| "correlation memory lock poisoned".to_string())?
                .iter()
                .filter(|record| record.l1_block_number == number)
                .cloned()
                .collect()),
        }
    }

    pub fn by_l1_hash(&self, hash: B256) -> Result<Vec<BatchCorrelation>, String> {
        match &self.backend {
            Backend::Persistent(env) => {
                let Some(raw_number) = index_get(env, L1_HASH_DB, hash.as_slice())? else {
                    return Ok(Vec::new());
                };
                let number = db_u64(&raw_number, "L1-hash index value")?;
                let records = records_in_l1_block(env, number)?;
                if records.iter().all(|record| record.l1_block_hash == hash) {
                    Ok(records)
                } else {
                    Err("L1-hash index resolves to records from another block hash".into())
                }
            }
            Backend::Memory(records) => Ok(records
                .read()
                .map_err(|_| "correlation memory lock poisoned".to_string())?
                .iter()
                .filter(|record| record.l1_block_hash == hash)
                .cloned()
                .collect()),
        }
    }

    /// Most recent record in canonical L1 transaction order.
    pub fn tail(&self) -> Result<Option<BatchCorrelation>, String> {
        match &self.backend {
            Backend::Persistent(env) => {
                let txn = env
                    .begin_ro_txn()
                    .map_err(|error| format!("begin correlation read: {error}"))?;
                let order_db = txn
                    .open_db(Some(L1_ORDER_DB))
                    .map_err(|error| format!("open correlation L1 index: {error}"))?;
                let mut cursor = txn
                    .cursor(order_db.dbi())
                    .map_err(|error| format!("open correlation cursor: {error}"))?;
                let Some((_key, tx_hash)): Option<(Vec<u8>, Vec<u8>)> = cursor
                    .last()
                    .map_err(|error| format!("read correlation tail: {error}"))?
                else {
                    return Ok(None);
                };
                persistent_record_in_txn(&txn, db_hash(&tx_hash, "L1 tail index value")?)?
                    .map(Some)
                    .ok_or_else(|| "L1 tail index points at a missing record".to_string())
            }
            Backend::Memory(records) => Ok(records
                .read()
                .map_err(|_| "correlation memory lock poisoned".to_string())?
                .last()
                .cloned()),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRecord {
    l1_block_number: u64,
    l1_block_hash: [u8; 32],
    l1_transaction_hash: [u8; 32],
    l1_transaction_index: u64,
    l2_blocks: Vec<StoredL2Block>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredL2Block {
    number: u64,
    hash: [u8; 32],
}

impl From<&BatchCorrelation> for StoredRecord {
    fn from(record: &BatchCorrelation) -> Self {
        Self {
            l1_block_number: record.l1_block_number,
            l1_block_hash: record.l1_block_hash.0,
            l1_transaction_hash: record.l1_transaction_hash.0,
            l1_transaction_index: record.l1_transaction_index,
            l2_blocks: record
                .l2_blocks
                .iter()
                .map(|block| StoredL2Block {
                    number: block.number,
                    hash: block.hash.0,
                })
                .collect(),
        }
    }
}

impl From<StoredRecord> for BatchCorrelation {
    fn from(record: StoredRecord) -> Self {
        Self {
            l1_block_number: record.l1_block_number,
            l1_block_hash: B256::from(record.l1_block_hash),
            l1_transaction_hash: B256::from(record.l1_transaction_hash),
            l1_transaction_index: record.l1_transaction_index,
            l2_blocks: record
                .l2_blocks
                .into_iter()
                .map(|block| L2BlockCorrelation {
                    number: block.number,
                    hash: B256::from(block.hash),
                })
                .collect(),
        }
    }
}

fn encode_record(record: &BatchCorrelation) -> Result<Vec<u8>, String> {
    postcard::to_allocvec(&StoredRecord::from(record))
        .map_err(|error| format!("encode correlation record: {error}"))
}

fn decode_record(raw: &[u8]) -> Result<BatchCorrelation, String> {
    let stored: StoredRecord =
        postcard::from_bytes(raw).map_err(|error| format!("decode correlation record: {error}"))?;
    let record = BatchCorrelation::from(stored);
    record.validate()?;
    Ok(record)
}

fn order_key(block: u64, tx_index: u64) -> [u8; 16] {
    let mut key = [0_u8; 16];
    key[..8].copy_from_slice(&block.to_be_bytes());
    key[8..].copy_from_slice(&tx_index.to_be_bytes());
    key
}

fn put_persistent(env: &Environment, record: &BatchCorrelation) -> Result<(), String> {
    let txn = env
        .begin_rw_txn()
        .map_err(|error| format!("begin correlation write: {error}"))?;
    let records_db = txn
        .open_db(Some(RECORDS_DB))
        .map_err(|error| format!("open correlation records: {error}"))?;
    let order_db = txn
        .open_db(Some(L1_ORDER_DB))
        .map_err(|error| format!("open correlation L1 index: {error}"))?;
    let l1_hash_db = txn
        .open_db(Some(L1_HASH_DB))
        .map_err(|error| format!("open correlation L1-hash index: {error}"))?;
    let number_db = txn
        .open_db(Some(L2_NUMBER_DB))
        .map_err(|error| format!("open correlation L2-number index: {error}"))?;
    let hash_db = txn
        .open_db(Some(L2_HASH_DB))
        .map_err(|error| format!("open correlation L2-hash index: {error}"))?;
    let tx_key = record.l1_transaction_hash.as_slice();
    let existing: Option<Vec<u8>> = txn
        .get(records_db.dbi(), tx_key)
        .map_err(|error| format!("read existing correlation record: {error}"))?;
    if let Some(raw) = existing {
        let existing = decode_record(&raw)?;
        return if existing == *record {
            Ok(())
        } else {
            Err(format!(
                "L1 transaction {} already has different correlation data",
                record.l1_transaction_hash
            ))
        };
    }

    let order = order_key(record.l1_block_number, record.l1_transaction_index);
    let occupied: Option<Vec<u8>> = txn
        .get(order_db.dbi(), &order)
        .map_err(|error| format!("read existing correlation L1 position: {error}"))?;
    if let Some(other) = occupied {
        return Err(format!(
            "L1 position ({}, {}) already belongs to {}",
            record.l1_block_number,
            record.l1_transaction_index,
            db_hash(&other, "occupied L1 index value")?
        ));
    }

    txn.put(
        records_db.dbi(),
        tx_key,
        encode_record(record)?,
        WriteFlags::UPSERT,
    )
    .map_err(|error| format!("write correlation record: {error}"))?;
    txn.put(order_db.dbi(), order, tx_key, WriteFlags::UPSERT)
        .map_err(|error| format!("write correlation L1 index: {error}"))?;
    let existing_l1_number: Option<Vec<u8>> = txn
        .get(l1_hash_db.dbi(), record.l1_block_hash.as_slice())
        .map_err(|error| format!("read correlation L1-hash index: {error}"))?;
    if let Some(existing) = existing_l1_number
        && db_u64(&existing, "L1-hash index value")? != record.l1_block_number
    {
        return Err("one L1 block hash is indexed at two block numbers".into());
    }
    txn.put(
        l1_hash_db.dbi(),
        record.l1_block_hash.as_slice(),
        record.l1_block_number.to_be_bytes(),
        WriteFlags::UPSERT,
    )
    .map_err(|error| format!("write correlation L1-hash index: {error}"))?;
    for block in &record.l2_blocks {
        update_l2_index_if_newer(
            &txn,
            records_db.dbi(),
            number_db.dbi(),
            block.number.to_be_bytes(),
            record,
        )?;
        update_l2_index_if_newer(
            &txn,
            records_db.dbi(),
            hash_db.dbi(),
            block.hash.as_slice(),
            record,
        )?;
    }
    txn.commit()
        .map_err(|error| format!("commit correlation record: {error}"))?;
    Ok(())
}

fn update_l2_index_if_newer(
    txn: &reth_libmdbx::Transaction<reth_libmdbx::RW>,
    records_dbi: reth_libmdbx::ffi::MDBX_dbi,
    index_dbi: reth_libmdbx::ffi::MDBX_dbi,
    key: impl AsRef<[u8]>,
    record: &BatchCorrelation,
) -> Result<(), String> {
    let key = key.as_ref();
    let existing: Option<Vec<u8>> = txn
        .get(index_dbi, key)
        .map_err(|error| format!("read correlation L2 index: {error}"))?;
    if let Some(existing_tx) = existing {
        let raw: Option<Vec<u8>> = txn
            .get(records_dbi, &existing_tx)
            .map_err(|error| format!("read indexed correlation record: {error}"))?;
        let Some(raw) = raw else {
            return Err("correlation L2 index points at a missing record".into());
        };
        if decode_record(&raw)?.order() > record.order() {
            return Ok(());
        }
    }
    txn.put(
        index_dbi,
        key,
        record.l1_transaction_hash.as_slice(),
        WriteFlags::UPSERT,
    )
    .map_err(|error| format!("write correlation L2 index: {error}"))
}

fn index_get(env: &Environment, table: &str, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let txn = env
        .begin_ro_txn()
        .map_err(|error| format!("begin correlation read: {error}"))?;
    let db = txn
        .open_db(Some(table))
        .map_err(|error| format!("open correlation table {table}: {error}"))?;
    txn.get(db.dbi(), key)
        .map_err(|error| format!("read correlation table {table}: {error}"))
}

fn persistent_record(env: &Environment, tx_hash: B256) -> Result<Option<BatchCorrelation>, String> {
    let txn = env
        .begin_ro_txn()
        .map_err(|error| format!("begin correlation read: {error}"))?;
    persistent_record_in_txn(&txn, tx_hash)
}

fn persistent_record_in_txn<K: reth_libmdbx::TransactionKind>(
    txn: &reth_libmdbx::Transaction<K>,
    tx_hash: B256,
) -> Result<Option<BatchCorrelation>, String> {
    let db = txn
        .open_db(Some(RECORDS_DB))
        .map_err(|error| format!("open correlation records: {error}"))?;
    let raw: Option<Vec<u8>> = txn
        .get(db.dbi(), tx_hash.as_slice())
        .map_err(|error| format!("read correlation record: {error}"))?;
    raw.map(|raw| decode_record(&raw)).transpose()
}

fn records_in_l1_block(env: &Environment, block: u64) -> Result<Vec<BatchCorrelation>, String> {
    let txn = env
        .begin_ro_txn()
        .map_err(|error| format!("begin correlation read: {error}"))?;
    let order_db = txn
        .open_db(Some(L1_ORDER_DB))
        .map_err(|error| format!("open correlation L1 index: {error}"))?;
    let mut cursor = txn
        .cursor(order_db.dbi())
        .map_err(|error| format!("open correlation cursor: {error}"))?;
    let prefix = block.to_be_bytes();
    let mut item: Option<(Vec<u8>, Vec<u8>)> = cursor
        .set_range(&order_key(block, 0))
        .map_err(|error| format!("seek correlation L1 index: {error}"))?;
    let mut out = Vec::new();
    while let Some((key, tx_hash)) = item {
        if key.get(..8) != Some(prefix.as_slice()) {
            break;
        }
        let record = persistent_record_in_txn(&txn, db_hash(&tx_hash, "L1 block index value")?)?
            .ok_or_else(|| "correlation L1 index points at a missing record".to_string())?;
        out.push(record);
        item = cursor
            .next()
            .map_err(|error| format!("read next correlation L1 index: {error}"))?;
    }
    Ok(out)
}

fn db_hash(raw: &[u8], context: &str) -> Result<B256, String> {
    if raw.len() != 32 {
        return Err(format!(
            "corrupt correlation {context}: expected 32 bytes, got {}",
            raw.len()
        ));
    }
    Ok(B256::from_slice(raw))
}

fn db_u64(raw: &[u8], context: &str) -> Result<u64, String> {
    let raw: [u8; 8] = raw.try_into().map_err(|_| {
        format!(
            "corrupt correlation {context}: expected 8 bytes, got {}",
            raw.len()
        )
    })?;
    Ok(u64::from_be_bytes(raw))
}

fn remove_after_persistent(env: &Environment, ancestor: u64) -> Result<usize, String> {
    let txn = env
        .begin_rw_txn()
        .map_err(|error| format!("begin correlation reorg write: {error}"))?;
    let records_db = txn
        .open_db(Some(RECORDS_DB))
        .map_err(|error| format!("open correlation records: {error}"))?;
    let order_db = txn
        .open_db(Some(L1_ORDER_DB))
        .map_err(|error| format!("open correlation L1 index: {error}"))?;
    let l1_hash_db = txn
        .open_db(Some(L1_HASH_DB))
        .map_err(|error| format!("open correlation L1-hash index: {error}"))?;
    let number_db = txn
        .open_db(Some(L2_NUMBER_DB))
        .map_err(|error| format!("open correlation L2-number index: {error}"))?;
    let hash_db = txn
        .open_db(Some(L2_HASH_DB))
        .map_err(|error| format!("open correlation L2-hash index: {error}"))?;

    let mut cursor = txn
        .cursor(order_db.dbi())
        .map_err(|error| format!("open correlation cursor: {error}"))?;
    let start = ancestor.saturating_add(1);
    let mut item: Option<(Vec<u8>, Vec<u8>)> = cursor
        .set_range(&order_key(start, 0))
        .map_err(|error| format!("seek correlation reorg boundary: {error}"))?;
    let mut doomed = Vec::new();
    while let Some((key, tx_hash)) = item {
        doomed.push((key, tx_hash));
        item = cursor
            .next()
            .map_err(|error| format!("read correlation reorg suffix: {error}"))?;
    }
    drop(cursor);
    for (key, tx_hash) in &doomed {
        txn.del(order_db.dbi(), key, None)
            .map_err(|error| format!("delete correlation L1 index: {error}"))?;
        txn.del(records_db.dbi(), tx_hash, None)
            .map_err(|error| format!("delete correlation record: {error}"))?;
    }

    // Rebuild both reverse indices in surviving canonical L1 transaction order.
    txn.clear_db(number_db.dbi())
        .map_err(|error| format!("clear correlation L2-number index: {error}"))?;
    txn.clear_db(hash_db.dbi())
        .map_err(|error| format!("clear correlation L2-hash index: {error}"))?;
    txn.clear_db(l1_hash_db.dbi())
        .map_err(|error| format!("clear correlation L1-hash index: {error}"))?;
    let mut cursor = txn
        .cursor(order_db.dbi())
        .map_err(|error| format!("open correlation rebuild cursor: {error}"))?;
    let mut item: Option<(Vec<u8>, Vec<u8>)> = cursor
        .first()
        .map_err(|error| format!("read first surviving correlation: {error}"))?;
    while let Some((_key, tx_hash)) = item {
        let raw: Option<Vec<u8>> = txn
            .get(records_db.dbi(), &tx_hash)
            .map_err(|error| format!("read surviving correlation record: {error}"))?;
        let record = decode_record(
            &raw.ok_or_else(|| "correlation L1 index points at a missing record".to_string())?,
        )?;
        txn.put(
            l1_hash_db.dbi(),
            record.l1_block_hash.as_slice(),
            record.l1_block_number.to_be_bytes(),
            WriteFlags::UPSERT,
        )
        .map_err(|error| format!("rebuild correlation L1-hash index: {error}"))?;
        for block in &record.l2_blocks {
            txn.put(
                number_db.dbi(),
                block.number.to_be_bytes(),
                &tx_hash,
                WriteFlags::UPSERT,
            )
            .map_err(|error| format!("rebuild correlation L2-number index: {error}"))?;
            txn.put(
                hash_db.dbi(),
                block.hash.as_slice(),
                &tx_hash,
                WriteFlags::UPSERT,
            )
            .map_err(|error| format!("rebuild correlation L2-hash index: {error}"))?;
        }
        item = cursor
            .next()
            .map_err(|error| format!("read next surviving correlation: {error}"))?;
    }
    txn.commit()
        .map_err(|error| format!("commit correlation reorg: {error}"))?;
    Ok(doomed.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(l1_block: u64, tx_index: u64, first_l2: u64, hashes: &[u8]) -> BatchCorrelation {
        BatchCorrelation {
            l1_block_number: l1_block,
            l1_block_hash: B256::with_last_byte(l1_block as u8),
            l1_transaction_hash: B256::with_last_byte((l1_block + tx_index + 1) as u8),
            l1_transaction_index: tx_index,
            l2_blocks: hashes
                .iter()
                .enumerate()
                .map(|(offset, hash)| L2BlockCorrelation {
                    number: first_l2 + offset as u64,
                    hash: B256::with_last_byte(*hash),
                })
                .collect(),
        }
    }

    fn exercise(store: &CorrelationStore) {
        let a = record(10, 1, 1, &[1, 2]);
        let b = record(11, 2, 3, &[3, 4]);
        store.put(&a).unwrap();
        store.put(&b).unwrap();
        store.put(&a).unwrap(); // idempotent replay

        assert_eq!(store.by_l2_number(3).unwrap().unwrap().batch, b);
        assert_eq!(
            store
                .by_l2_hash(B256::with_last_byte(2))
                .unwrap()
                .unwrap()
                .batch,
            a
        );
        assert_eq!(
            store.by_l1_transaction_hash(a.l1_transaction_hash).unwrap(),
            Some(a.clone())
        );
        assert!(
            store
                .by_l1_transaction_hash(B256::repeat_byte(0xff))
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .canonical_l2_blocks(a.l1_transaction_hash, &a.l2_blocks)
                .unwrap()
        );
        assert!(
            !store
                .canonical_l2_blocks(b.l1_transaction_hash, &a.l2_blocks)
                .unwrap()
        );
        assert_eq!(store.by_l1_number(10).unwrap(), vec![a.clone()]);
        assert_eq!(store.by_l1_hash(b.l1_block_hash).unwrap(), vec![b.clone()]);
        assert_eq!(store.tail().unwrap(), Some(b));
    }

    #[test]
    fn memory_queries_both_directions() {
        exercise(&CorrelationStore::in_memory());
    }

    #[test]
    fn persistent_queries_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = CorrelationStore::open(dir.path()).unwrap();
            exercise(&store);
        }
        let reopened = CorrelationStore::open(dir.path()).unwrap();
        assert_eq!(
            reopened.by_l2_number(4).unwrap().unwrap().l2_block.hash,
            B256::with_last_byte(4)
        );
    }

    fn exercise_same_height_replacement_and_reorg(store: &CorrelationStore) {
        let original = record(20, 1, 8, &[0x11]);
        let replacement = record(20, 2, 8, &[0x22]);
        store.put(&original).unwrap();
        store.put(&replacement).unwrap();

        assert_eq!(
            store.by_l2_number(8).unwrap().unwrap().l2_block.hash,
            B256::with_last_byte(0x22)
        );
        assert!(
            !store
                .by_l2_hash(B256::with_last_byte(0x11))
                .unwrap()
                .unwrap()
                .canonical_l2
        );

        assert!(
            !store
                .canonical_l2_blocks(original.l1_transaction_hash, &original.l2_blocks)
                .unwrap()
        );
        assert!(
            store
                .canonical_l2_blocks(replacement.l1_transaction_hash, &replacement.l2_blocks)
                .unwrap()
        );
        assert_eq!(store.remove_after_l1(19).unwrap(), 2);
        assert!(
            store
                .by_l1_transaction_hash(original.l1_transaction_hash)
                .unwrap()
                .is_none()
        );
        assert!(
            !store
                .canonical_l2_blocks(replacement.l1_transaction_hash, &replacement.l2_blocks)
                .unwrap()
        );
        assert!(store.by_l2_number(8).unwrap().is_none());
        assert!(store.tail().unwrap().is_none());
    }

    #[test]
    fn memory_replacement_and_reorg_are_hash_safe() {
        exercise_same_height_replacement_and_reorg(&CorrelationStore::in_memory());
    }

    #[test]
    fn persistent_replacement_and_reorg_are_hash_safe() {
        let dir = tempfile::tempdir().unwrap();
        exercise_same_height_replacement_and_reorg(&CorrelationStore::open(dir.path()).unwrap());
    }
}
