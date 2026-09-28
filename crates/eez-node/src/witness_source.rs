//! Persistent per-block witness store for the composer-controlled prover
//! (remote mode).
//!
//! At commit, the committer feeds each produced block's hash to [`run_capture`],
//! which re-executes it (parent state still fresh) and persists the augmented
//! witness. [`NodeWitnessSource`] serves the committed window of
//! [`eez_prover::ProvingContext::blocks`] from the store, re-execing on demand
//! only for the newest blocks not yet drained.
//!
//! Persistent (mdbx), not RAM: the node is non-archival, so by settlement time an
//! older block's parent state is pruned — a RAM store lost on restart couldn't
//! re-derive it, stalling a restart with an unsettled backlog. Dedicated env
//! (never reth's node DB; path `EEZ_WITNESS_DB_PATH`), keyed by full block hash.
//!
//! Purge floor = L1-FINALIZED (not merely-posted) L2 height: an L1 reorg could
//! roll a posted batch out and need its witness back. The endpoint (uncommitted
//! Sync block) is captured by the composer's `endpoint_witness`, not here.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use alloy_consensus::Header;
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{B256, Bytes};
use alloy_rpc_types_debug::ExecutionWitness;
use eez_driver::witness::{ExecutionWitnessMode, block_witness};
use eez_primitives::{Block, EezPrimitives};
use eez_prover::{BlockWitness, ProvingWitnessSource};
use reth_evm::ConfigureEvm;
use reth_libmdbx::{DatabaseFlags, Environment, Geometry, WriteFlags};
use reth_storage_api::{BlockReader, HeaderProvider, StateProviderFactory, TransactionVariant};
use tokio::sync::mpsc;
use tracing::{Level, event};

const GIGABYTE: usize = 1024 * 1024 * 1024;
/// mdbx reserves this as sparse virtual address space (no preallocation) and
/// grows the file lazily. This is a hard upper bound, not a free-disk guarantee.
const MAP_MAX_BYTES: usize = 256 * GIGABYTE;
const GROWTH_STEP: isize = 1024 * 1024 * 1024;

const WITNESSES: &str = "witnesses_by_hash_v1";
const HEIGHTS: &str = "witnesses_by_height_v1";
const SCHEMA: &[u8] = b"hash-keyed-witness-v1";
const PURGE_BATCH: usize = 1024;

/// Persistent witness store keyed by full block hash. A transactional
/// height/hash index supports bounded finalized garbage collection while
/// preserving every competing unfinalized branch. MDBX MVCC needs no extra lock.
pub struct WitnessDb {
    env: Environment,
    path: PathBuf,
    capture_error: Mutex<Option<String>>,
    preparation_error: Mutex<Option<String>>,
}

/// Bounded handoff from durable capture to signer preparation. Overflow keeps
/// only the newest exact endpoint as a catch-up watermark; witnesses remain in
/// MDBX and are never copied into the watermark.
#[derive(Clone, Debug)]
pub(crate) struct PreparationNotifier {
    sender: mpsc::Sender<eez_l1::L2Endpoint>,
    watermark: Arc<Mutex<Option<eez_l1::L2Endpoint>>>,
}

impl PreparationNotifier {
    pub(crate) fn channel(capacity: usize) -> (Self, mpsc::Receiver<eez_l1::L2Endpoint>) {
        let (sender, receiver) = mpsc::channel(capacity);
        (
            Self {
                sender,
                watermark: Arc::new(Mutex::new(None)),
            },
            receiver,
        )
    }

    pub(crate) fn notify(&self, endpoint: eez_l1::L2Endpoint) {
        self.notify_after_lock(endpoint, || {});
    }

    fn notify_after_lock(&self, endpoint: eez_l1::L2Endpoint, after_lock: impl FnOnce()) {
        // Once overflow has established a watermark, every later endpoint must
        // replace it until the consumer takes it. Sending a later endpoint into
        // newly freed queue space would let the older watermark overwrite it
        // after the queue is drained. Keep the lock across try_send so cloned
        // producers cannot observe an empty watermark and publish out of order.
        let Ok(mut watermark) = self.watermark.lock() else {
            return;
        };
        after_lock();
        if watermark.is_some() {
            *watermark = Some(endpoint);
            return;
        }
        match self.sender.try_send(endpoint) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            Err(mpsc::error::TrySendError::Full(endpoint)) => {
                *watermark = Some(endpoint);
            }
        }
    }

    pub(crate) fn take_watermark(&self) -> Option<eez_l1::L2Endpoint> {
        self.watermark.lock().ok()?.take()
    }
}

impl std::fmt::Debug for WitnessDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WitnessDb").finish_non_exhaustive()
    }
}

impl WitnessDb {
    /// Open (creating if absent) the witness env at `path`.
    ///
    /// # Errors
    /// Filesystem or mdbx open failures.
    pub fn open(path: &Path) -> eyre::Result<Self> {
        std::fs::create_dir_all(path)?;
        let mut builder = Environment::builder();
        builder.set_max_dbs(2);
        builder.set_geometry(Geometry {
            size: Some(0..MAP_MAX_BYTES),
            growth_step: Some(GROWTH_STEP),
            shrink_threshold: None,
            page_size: None,
        });
        let env = builder
            .open(path)
            .map_err(|e| eyre::eyre!("open witness env at {}: {e}", path.display()))?;
        let txn = env.begin_rw_txn()?;
        let metadata = txn.create_db(None, DatabaseFlags::empty())?;
        match txn.get::<Vec<u8>>(metadata.dbi(), b"schema")? {
            Some(schema) => eyre::ensure!(schema == SCHEMA, "unsupported witness database schema"),
            None => {
                // This is a new-network schema. Never silently ignore an old
                // height-keyed database or pretend to migrate missing branches.
                let mut cursor = txn.cursor(metadata.dbi())?;
                eyre::ensure!(
                    cursor.first::<(), ()>()?.is_none(),
                    "witness database requires the new-network schema"
                );
                txn.put(metadata.dbi(), b"schema", SCHEMA, WriteFlags::UPSERT)?;
            }
        }
        txn.create_db(Some(WITNESSES), DatabaseFlags::empty())?;
        txn.create_db(Some(HEIGHTS), DatabaseFlags::empty())?;
        txn.commit()?;
        Ok(Self {
            env,
            path: path.to_path_buf(),
            capture_error: Mutex::new(None),
            preparation_error: Mutex::new(None),
        })
    }

    fn check_disk_capacity(&self, bytes: usize) -> eyre::Result<()> {
        let capacity = rustix::fs::statvfs(&self.path)?;
        let available = capacity
            .f_bavail
            .checked_mul(capacity.f_frsize)
            .ok_or_else(|| eyre::eyre!("witness free-disk accounting overflow"))?;
        require_disk_capacity(available, bytes)
    }

    fn ensure_ready(&self) -> eyre::Result<()> {
        if let Some(error) = &*self
            .capture_error
            .lock()
            .map_err(|_| eyre::eyre!("witness capture status poisoned"))?
        {
            eyre::bail!("durable witness capture unavailable: {error}");
        }
        if let Some(error) = &*self
            .preparation_error
            .lock()
            .map_err(|_| eyre::eyre!("witness preparation status poisoned"))?
        {
            eyre::bail!("ordinary witness preparation unavailable: {error}");
        }
        self.check_disk_capacity(0)
    }

    fn capture_status(&self, error: Option<String>) {
        // A poisoned status remains unavailable to callers; do not mask it by
        // replacing state after an internal panic.
        if let Ok(mut status) = self.capture_error.lock() {
            *status = error;
        }
    }

    pub(crate) fn preparation_status(&self, error: Option<String>) {
        if let Ok(mut status) = self.preparation_error.lock() {
            *status = error;
        }
    }

    /// Persist a witness without overwriting any competing block at its height.
    /// Primary record and finalized-GC index commit atomically. Repeated capture
    /// of the same hash may refresh its witness but cannot change its block.
    pub fn put(&self, bw: &BlockWitness) -> eyre::Result<()> {
        validate_identity(bw, bw.number, bw.hash)?;
        let bytes = postcard::to_allocvec(&StoredWitness::from(bw))
            .map_err(|e| eyre::eyre!("encode witness {} ({}): {e}", bw.number, bw.hash))?;
        self.check_disk_capacity(bytes.len())?;
        let txn = self.env.begin_rw_txn()?;
        let witnesses = txn.open_db(Some(WITNESSES))?;
        let heights = txn.open_db(Some(HEIGHTS))?;
        txn.put(witnesses.dbi(), bw.hash, &bytes, WriteFlags::UPSERT)?;
        txn.put(
            heights.dbi(),
            height_key(bw.number, bw.hash),
            [],
            WriteFlags::UPSERT,
        )?;
        txn.commit()?;
        Ok(())
    }

    /// Resolve the exact requested ancestry, including after a reorg or restart
    /// with pruned state. Height alone is never a witness identity.
    pub fn get(&self, number: u64, hash: B256) -> eyre::Result<Option<BlockWitness>> {
        let txn = self.env.begin_ro_txn()?;
        let witnesses = txn.open_db(Some(WITNESSES))?;
        let heights = txn.open_db(Some(HEIGHTS))?;
        let raw: Option<Vec<u8>> = txn.get(witnesses.dbi(), hash.as_slice())?;
        match raw {
            Some(bytes) => {
                let witness = decode_stored(&bytes, number, hash)?;
                eyre::ensure!(
                    txn.get::<()>(heights.dbi(), &height_key(number, hash))?
                        .is_some(),
                    "witness height/hash index is missing"
                );
                Ok(Some(witness))
            }
            None => {
                eyre::ensure!(
                    txn.get::<()>(heights.dbi(), &height_key(number, hash))?
                        .is_none(),
                    "witness primary record is missing"
                );
                Ok(None)
            }
        }
    }

    /// Inspect a stopped/offline store when an archive has exactly one block
    /// at a height. This is a diagnostic selector, not canonicality or execution
    /// evidence; proving still takes an explicit hash through `get` and replays it.
    /// Ambiguous branches and corrupt index/primary records fail explicitly.
    pub fn unique_hash_at_height(&self, number: u64) -> eyre::Result<Option<B256>> {
        let txn = self.env.begin_ro_txn()?;
        let heights = txn.open_db(Some(HEIGHTS))?;
        let witnesses = txn.open_db(Some(WITNESSES))?;
        let mut cursor = txn.cursor(heights.dbi())?;
        let prefix = number.to_be_bytes();
        let Some((key, ())): Option<([u8; 40], ())> = cursor.set_range(&prefix)? else {
            return Ok(None);
        };
        if key[..8] != prefix {
            return Ok(None);
        }
        if let Some((next, ())) = cursor.next::<[u8; 40], ()>()? {
            eyre::ensure!(
                next[..8] != prefix,
                "multiple retained branches at height {number}; an expected hash is required"
            );
        }
        let hash = B256::from_slice(&key[8..]);
        let raw: Vec<u8> = txn
            .get(witnesses.dbi(), hash.as_slice())?
            .ok_or_else(|| eyre::eyre!("witness index points to missing primary record"))?;
        decode_stored(&raw, number, hash)?;
        Ok(Some(hash))
    }

    /// Delete at most PURGE_BATCH witnesses strictly below the authenticated
    /// L1-finalized L2 floor. Keep every branch at the boundary. Repeating with
    /// the same floor drains a large prefix without an unbounded write txn.
    pub fn purge_settled(&self, below: u64) -> eyre::Result<usize> {
        let txn = self.env.begin_rw_txn()?;
        let witnesses = txn.open_db(Some(WITNESSES))?;
        let heights = txn.open_db(Some(HEIGHTS))?;
        let mut doomed = Vec::new();
        {
            let mut cursor = txn.cursor(heights.dbi())?;
            let mut entry: Option<([u8; 40], ())> = cursor.first()?;
            while let Some((key, ())) = entry {
                let number = u64::from_be_bytes(key[..8].try_into()?);
                if number >= below || doomed.len() == PURGE_BATCH {
                    break;
                }
                doomed.push((key, number, B256::from_slice(&key[8..])));
                entry = cursor.next()?;
            }
        }
        for (key, number, hash) in &doomed {
            let raw: Vec<u8> = txn.get(witnesses.dbi(), hash.as_slice())?.ok_or_else(|| {
                eyre::eyre!("finalized witness index points to missing primary record")
            })?;
            // Corrupt index metadata cannot authorize deleting a different
            // unfinalized block. Abort the whole transaction on inconsistency.
            decode_stored(&raw, *number, *hash)?;
            txn.del(witnesses.dbi(), hash.as_slice(), None)?;
            txn.del(heights.dbi(), key, None)?;
        }
        txn.commit()?;
        Ok(doomed.len())
    }
}

// Reserve one database growth step and a copy-on-write allowance. This is an
// admission check, not a promise against another process filling the disk;
// every actual MDBX write/commit error also stops capture and admission.
fn require_disk_capacity(available: u64, record_bytes: usize) -> eyre::Result<()> {
    let required = u64::try_from(record_bytes)?
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(GROWTH_STEP as u64))
        .ok_or_else(|| eyre::eyre!("witness disk reservation overflow"))?;
    eyre::ensure!(
        available >= required,
        "witness disk capacity exhausted: {available} available bytes, {required} required"
    );
    Ok(())
}

fn height_key(number: u64, hash: B256) -> [u8; 40] {
    let mut key = [0; 40];
    key[..8].copy_from_slice(&number.to_be_bytes());
    key[8..].copy_from_slice(hash.as_slice());
    key
}

fn validate_identity(bw: &BlockWitness, number: u64, hash: B256) -> eyre::Result<()> {
    let block: Block = alloy_rlp::decode_exact(&bw.rlp)?;
    eyre::ensure!(
        bw.number == number
            && bw.hash == hash
            && block.header.number == number
            && block.header.hash_slow() == hash
            && block.header.parent_hash == bw.parent_hash
            && alloy_rlp::encode(&block).as_slice() == bw.rlp.as_ref(),
        "witness block identity/RLP mismatch for {number} ({hash})"
    );
    Ok(())
}

fn decode_stored(bytes: &[u8], number: u64, hash: B256) -> eyre::Result<BlockWitness> {
    let stored: StoredWitness = postcard::from_bytes(bytes)
        .map_err(|e| eyre::eyre!("decode witness {number} ({hash}): {e}"))?;
    let witness = stored.into();
    validate_identity(&witness, number, hash)?;
    Ok(witness)
}

/// On-disk mirror of [`BlockWitness`] (postcard). Scalars are raw bytes to avoid
/// depending on alloy's optional `serde` feature; `ExecutionWitness` self-serdes.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredWitness {
    number: u64,
    hash: [u8; 32],
    parent_hash: [u8; 32],
    rlp: Vec<u8>,
    witness: ExecutionWitness,
}

impl From<&BlockWitness> for StoredWitness {
    fn from(b: &BlockWitness) -> Self {
        Self {
            number: b.number,
            hash: b.hash.0,
            parent_hash: b.parent_hash.0,
            rlp: b.rlp.to_vec(),
            witness: b.witness.clone(),
        }
    }
}

impl From<StoredWitness> for BlockWitness {
    fn from(s: StoredWitness) -> Self {
        Self {
            number: s.number,
            hash: B256::from(s.hash),
            parent_hash: B256::from(s.parent_hash),
            rlp: Bytes::from(s.rlp),
            witness: s.witness,
        }
    }
}

/// Shared handle to the persistent witness store.
pub type WitnessStore = Arc<WitnessDb>;

/// Open the dedicated witness environment at `path`.
///
/// # Errors
/// See [`WitnessDb::open`].
pub fn new_store(path: &Path) -> eyre::Result<WitnessStore> {
    Ok(Arc::new(WitnessDb::open(path)?))
}

/// The composer's [`ProvingWitnessSource`]: reads a captured witness from the
/// persistent store, falling back to on-demand re-exec for the newest blocks the
/// capture task hasn't drained yet (parent state still fresh).
#[derive(Clone)]
pub struct NodeWitnessSource<P, E> {
    store: WitnessStore,
    provider: P,
    evm_config: E,
    correlations: Arc<eez_deriver::CorrelationStore>,
}

impl<P, E> NodeWitnessSource<P, E> {
    #[must_use]
    pub const fn new(
        store: WitnessStore,
        provider: P,
        evm_config: E,
        correlations: Arc<eez_deriver::CorrelationStore>,
    ) -> Self {
        Self {
            store,
            provider,
            evm_config,
            correlations,
        }
    }
}

impl<P, E> std::fmt::Debug for NodeWitnessSource<P, E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeWitnessSource").finish_non_exhaustive()
    }
}

impl<P, E> ProvingWitnessSource for NodeWitnessSource<P, E>
where
    P: BlockReader<Block = Block>
        + StateProviderFactory
        + HeaderProvider<Header = Header>
        + Send
        + Sync,
    E: ConfigureEvm<Primitives = EezPrimitives> + Send + Sync,
{
    fn ensure_ready(&self) -> Result<(), String> {
        self.store.ensure_ready().map_err(|error| error.to_string())
    }

    fn parent_provenance(
        &self,
        parent: Header,
        live_slot: Option<(u64, B256)>,
    ) -> Result<eez_protocol::l2::ParentProvenance, String> {
        let hash = parent.hash_slow();
        let accepted_l1_number = if parent.number == 0 {
            0
        } else {
            let matched = self
                .correlations
                .by_endpoint_hash(hash)?
                .ok_or("accepted parent is missing from the endpoint correlation index")?;
            matched.l1_block_number
        };
        let live_slot_endpoints = if let Some((number, canonical_hash)) = live_slot {
            self.correlations
                .by_l1_number(number)?
                .into_iter()
                .map(|batch| {
                    if batch.l1_block_hash != canonical_hash {
                        return Err("live-slot correlation belongs to a replaced L1 block".into());
                    }
                    Ok(batch.endpoint_header)
                })
                .collect::<Result<Vec<_>, String>>()?
        } else {
            Vec::new()
        };
        Ok(eez_protocol::l2::ParentProvenance {
            parent,
            accepted_l1_number,
            live_slot_endpoints,
        })
    }

    fn block_witness(&self, number: u64, hash: B256) -> Result<BlockWitness, String> {
        // Corruption/I/O failures are not cache misses and must not silently
        // replace retained evidence with a witness for another branch.
        if let Some(witness) = self.store.get(number, hash).map_err(|e| e.to_string())? {
            return Ok(witness);
        }
        let witness = build_block_witness(
            &self.provider,
            &self.evm_config,
            BlockHashOrNumber::Hash(hash),
        )?;
        validate_identity(&witness, number, hash).map_err(|e| e.to_string())?;
        self.store
            .put(&witness)
            .map_err(|e| format!("persist on-demand witness: {e}"))?;
        Ok(witness)
    }
}

/// Re-execute a committed block on its parent state → augmented witness.
/// Blocking (trie walk); call under `spawn_blocking`.
fn build_block_witness<P, E>(
    provider: &P,
    evm_config: &E,
    id: BlockHashOrNumber,
) -> Result<BlockWitness, String>
where
    P: BlockReader<Block = Block> + StateProviderFactory + HeaderProvider<Header = Header>,
    E: ConfigureEvm<Primitives = EezPrimitives>,
{
    let block = provider
        .recovered_block(id, TransactionVariant::WithHash)
        .map_err(|e| format!("fetch block {id:?}: {e}"))?
        .ok_or_else(|| format!("witness for block {id:?} not captured yet"))?;
    // Legacy + removal-closure augmentation: the intermediate-per-tx-root nodes
    // the prover's re-execution needs (MPT completeness fix).
    block_witness(provider, evm_config, &block, ExecutionWitnessMode::Legacy)
        .map_err(|e| format!("witness for block {id:?}: {e}"))
}

/// Drain committed-block hashes, capture each block's witness at commit (parent
/// state fresh), persist it, and purge everything the L1 has FINALIZED
/// (`settled_floor`). Runs until the channel closes.
pub async fn run_capture<P, E, F>(
    mut rx: mpsc::UnboundedReceiver<eez_l1::L2Endpoint>,
    store: WitnessStore,
    provider: P,
    evm_config: E,
    settled_floor: F,
    preparation: Option<PreparationNotifier>,
) where
    P: BlockReader<Block = Block>
        + StateProviderFactory
        + HeaderProvider<Header = Header>
        + Clone
        + Send
        + Sync
        + 'static,
    E: ConfigureEvm<Primitives = EezPrimitives> + Clone + Send + Sync + 'static,
    F: Fn() -> u64 + Send + 'static,
{
    while let Some(endpoint) = rx.recv().await {
        let hash = endpoint.hash;
        // Do not drop a failed identity and continue capturing newer blocks.
        // Admission pauses while this exact block retries. All trie/database
        // work stays off the async runtime threads.
        loop {
            let provider = provider.clone();
            let evm_config = evm_config.clone();
            let capture_store = Arc::clone(&store);
            let floor = settled_floor();
            if endpoint.number < floor {
                // This exact failed/queued block is no longer required. Do not
                // recreate finalized witnesses or require pruned parent state.
                store.capture_status(None);
                break;
            }
            let result = tokio::task::spawn_blocking(move || -> Result<usize, String> {
                // Finalized pruning may free map space after a previous failure.
                let purged = capture_store
                    .purge_settled(floor)
                    .map_err(|e| e.to_string())?;
                if capture_store
                    .get(endpoint.number, hash)
                    .map_err(|e| e.to_string())?
                    .is_some()
                {
                    return Ok(purged);
                }
                let bw =
                    build_block_witness(&provider, &evm_config, BlockHashOrNumber::Hash(hash))?;
                validate_identity(&bw, endpoint.number, hash).map_err(|e| e.to_string())?;
                capture_store.put(&bw).map_err(|e| e.to_string())?;
                Ok(purged)
            })
            .await;
            let error = match result {
                Ok(Ok(purged)) => {
                    store.capture_status(None);
                    if let Some(preparation) = &preparation {
                        preparation.notify(endpoint);
                    }
                    if purged > 0 {
                        event!(name: "eez.node.witness_store.purged", Level::DEBUG,
                            removed = purged, "purged finalized witnesses");
                    }
                    break;
                }
                Ok(Err(error)) => error,
                Err(error) => format!("witness capture worker failed: {error}"),
            };
            store.capture_status(Some(error.clone()));
            event!(name: "eez.node.witness_capture.retry", Level::ERROR,
                %hash, %error, "durable witness capture failed; admission paused, retrying exact block");
            if rx.is_closed() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(number: u64) -> BlockWitness {
        with_header(Header {
            number,
            parent_hash: B256::repeat_byte(number.wrapping_sub(1) as u8),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn bounded_preparation_handoff_retains_only_latest_overflow_watermark() {
        let (notifier, mut receiver) = PreparationNotifier::channel(1);
        let first = eez_l1::L2Endpoint {
            number: 1,
            hash: B256::repeat_byte(1),
        };
        let second = eez_l1::L2Endpoint {
            number: 2,
            hash: B256::repeat_byte(2),
        };
        let latest = eez_l1::L2Endpoint {
            number: 3,
            hash: B256::repeat_byte(3),
        };
        notifier.notify(first);
        notifier.notify(second);
        notifier.notify(latest);
        assert_eq!(receiver.recv().await, Some(first));
        assert_eq!(notifier.take_watermark(), Some(latest));
        assert_eq!(notifier.take_watermark(), None);

        // If queue space opens while an overflow watermark still exists, a
        // newer endpoint must replace that watermark instead of entering the
        // queue and then being overwritten by the older value.
        notifier.notify(first);
        notifier.notify(second);
        assert_eq!(receiver.recv().await, Some(first));
        notifier.notify(latest);
        assert!(receiver.try_recv().is_err());
        assert_eq!(notifier.take_watermark(), Some(latest));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_notifiers_cannot_install_an_older_watermark() {
        let (notifier, mut receiver) = PreparationNotifier::channel(1);
        let first = eez_l1::L2Endpoint {
            number: 1,
            hash: B256::repeat_byte(1),
        };
        let older = eez_l1::L2Endpoint {
            number: 2,
            hash: B256::repeat_byte(2),
        };
        let latest = eez_l1::L2Endpoint {
            number: 3,
            hash: B256::repeat_byte(3),
        };
        notifier.notify(first);

        // Pause the older producer after it has observed the watermark while
        // holding the serialization lock. Free queue capacity, then start a
        // newer cloned producer. The older call must publish first; the newer
        // call then becomes the watermark rather than being overwritten.
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let older_notifier = notifier.clone();
        let older_thread = std::thread::spawn(move || {
            older_notifier.notify_after_lock(older, || {
                locked_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
        });
        locked_rx.recv().unwrap();
        assert_eq!(receiver.recv().await, Some(first));

        let latest_notifier = notifier.clone();
        let latest_thread = std::thread::spawn(move || latest_notifier.notify(latest));
        release_tx.send(()).unwrap();
        older_thread.join().unwrap();
        latest_thread.join().unwrap();

        assert_eq!(receiver.recv().await, Some(older));
        assert_eq!(notifier.take_watermark(), Some(latest));
    }

    #[test]
    fn offline_height_discovery_rejects_competing_branches() {
        let directory = tempfile::tempdir().unwrap();
        let store = WitnessDb::open(directory.path()).unwrap();
        assert_eq!(store.unique_hash_at_height(7).unwrap(), None);
        let first = sample(7);
        store.put(&first).unwrap();
        assert_eq!(store.unique_hash_at_height(7).unwrap(), Some(first.hash));
        assert_eq!(store.unique_hash_at_height(6).unwrap(), None);
        let other = with_header(Header {
            number: 7,
            extra_data: Bytes::from_static(b"branch"),
            ..Default::default()
        });
        store.put(&other).unwrap();
        assert!(
            store
                .unique_hash_at_height(7)
                .unwrap_err()
                .to_string()
                .contains("multiple retained branches")
        );
        assert_eq!(store.get(7, first.hash).unwrap().unwrap().hash, first.hash);
        assert_eq!(store.get(7, other.hash).unwrap().unwrap().hash, other.hash);
    }

    #[tokio::test]
    async fn queued_captures_reuse_durable_witnesses_and_skip_only_below_finalized() {
        use eez_evm::EezEvmConfig;
        use reth_chainspec::ChainSpecBuilder;
        use reth_provider::test_utils::MockEthProvider;
        for (number, stored, must_fail) in [(9, false, false), (10, true, false), (10, false, true)]
        {
            let directory = tempfile::tempdir().unwrap();
            let store = Arc::new(WitnessDb::open(directory.path()).unwrap());
            let block = sample(number);
            if stored {
                store.put(&block).unwrap();
            }
            let (tx, rx) = mpsc::unbounded_channel();
            tx.send(eez_l1::L2Endpoint {
                number,
                hash: block.hash,
            })
            .unwrap();
            drop(tx);
            run_capture(
                rx,
                Arc::clone(&store),
                MockEthProvider::<EezPrimitives>::new(),
                EezEvmConfig::new(Arc::new(
                    ChainSpecBuilder::mainnet().shanghai_activated().build(),
                )),
                || 10,
                None,
            )
            .await;
            assert_eq!(store.ensure_ready().is_err(), must_fail);
            assert_eq!(store.get(number, block.hash).unwrap().is_some(), stored);
        }
    }

    fn with_header(header: Header) -> BlockWitness {
        BlockWitness {
            number: header.number,
            hash: header.hash_slow(),
            parent_hash: header.parent_hash,
            rlp: alloy_rlp::encode(Block {
                header,
                ..Default::default()
            })
            .into(),
            witness: ExecutionWitness {
                state: vec![Bytes::from_static(&[1, 2, 3])],
                codes: vec![],
                keys: vec![],
                headers: vec![],
            },
        }
    }

    #[test]
    fn capacity_and_capture_health_fail_closed_without_deleting_retained_data() {
        let reserve = GROWTH_STEP as u64;
        assert!(require_disk_capacity(reserve, 0).is_ok());
        assert!(require_disk_capacity(reserve - 1, 0).is_err());
        assert!(require_disk_capacity(reserve + 199, 100).is_err());
        assert!(require_disk_capacity(reserve + 200, 100).is_ok());
        assert!(require_disk_capacity(u64::MAX, usize::MAX).is_err());
        let directory = tempfile::tempdir().unwrap();
        let db = WitnessDb::open(directory.path()).unwrap();
        let block = sample(10);
        db.put(&block).unwrap();
        db.capture_status(Some("injected write failure".into()));
        assert!(
            db.ensure_ready()
                .unwrap_err()
                .to_string()
                .contains("injected write failure")
        );
        // Existing evidence remains readable for diagnosis/recovery; a failed
        // capture never grants permission to delete an unsettled branch.
        assert_eq!(db.get(10, block.hash).unwrap().unwrap().rlp, block.rlp);
        db.capture_status(None);
        assert!(db.ensure_ready().is_ok());
    }

    #[test]
    fn persist_get_purge_restart() {
        let dir = tempfile::tempdir().unwrap();
        for n in [10u64, 11, 12] {
            WitnessDb::open(dir.path())
                .unwrap()
                .put(&sample(n))
                .unwrap();
        }
        let db = WitnessDb::open(dir.path()).unwrap();
        let got = db
            .get(11, sample(11).hash)
            .unwrap()
            .expect("11 survives restart");
        assert_eq!(got.rlp, sample(11).rlp);
        assert!(db.get(99, sample(99).hash).unwrap().is_none());
        assert_eq!(db.purge_settled(12).unwrap(), 2);
        assert!(db.get(10, sample(10).hash).unwrap().is_none());
        assert!(db.get(11, sample(11).hash).unwrap().is_none());
        assert!(db.get(12, sample(12).hash).unwrap().is_some());
        assert_eq!(db.purge_settled(12).unwrap(), 0);
    }

    #[test]
    fn stored_witness_roundtrip() {
        let bw = sample(7);
        let bytes = postcard::to_allocvec(&StoredWitness::from(&bw)).unwrap();
        let back = decode_stored(&bytes, bw.number, bw.hash).unwrap();
        assert_eq!(back.number, bw.number);
        assert_eq!(back.hash, bw.hash);
        assert_eq!(back.parent_hash, bw.parent_hash);
        assert_eq!(back.rlp, bw.rlp);
        assert_eq!(back.witness.state, bw.witness.state);
    }

    #[test]
    fn canonical_witness_hit_reorg_and_missing_input_preserve_retained_data() {
        use eez_evm::EezEvmConfig;
        use reth_chainspec::ChainSpecBuilder;
        use reth_provider::test_utils::MockEthProvider;
        let directory = tempfile::tempdir().unwrap();
        let original = sample(2);
        let replacement = with_header(Header {
            number: 2,
            parent_hash: original.parent_hash,
            extra_data: Bytes::from_static(b"replacement"),
            ..Default::default()
        });
        let config = EezEvmConfig::new(Arc::new(
            ChainSpecBuilder::mainnet().shanghai_activated().build(),
        ));
        {
            let store = Arc::new(WitnessDb::open(directory.path()).unwrap());
            let source = NodeWitnessSource::new(
                Arc::clone(&store),
                MockEthProvider::<EezPrimitives>::new(),
                config.clone(),
                Arc::new(eez_deriver::CorrelationStore::in_memory()),
            );
            store.put(&original).unwrap();
            assert_eq!(
                source.block_witness(2, original.hash).unwrap().rlp,
                original.rlp
            );
            // Selecting a different hash cannot silently return the old branch.
            assert!(source.block_witness(2, replacement.hash).is_err());
            assert!(source.block_witness(3, sample(3).hash).is_err());
            store.put(&replacement).unwrap();
            assert_eq!(
                source.block_witness(2, replacement.hash).unwrap().rlp,
                replacement.rlp
            );
        }
        // A -> B -> restart -> A: no headers, bodies or parent execution state
        // are available in this provider, so both successes require persistence.
        let store = Arc::new(WitnessDb::open(directory.path()).unwrap());
        let source = NodeWitnessSource::new(
            Arc::clone(&store),
            MockEthProvider::<EezPrimitives>::new(),
            config,
            Arc::new(eez_deriver::CorrelationStore::in_memory()),
        );
        assert_eq!(
            source.block_witness(2, original.hash).unwrap().rlp,
            original.rlp
        );
        assert_eq!(
            source.block_witness(2, replacement.hash).unwrap().rlp,
            replacement.rlp
        );
        assert!(source.block_witness(3, original.hash).is_err());
        assert_eq!(store.purge_settled(2).unwrap(), 0);
        assert_eq!(store.purge_settled(3).unwrap(), 2);
    }

    #[test]
    fn duplicate_capture_is_idempotent_and_invalid_identity_cannot_replace_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let db = WitnessDb::open(dir.path()).unwrap();
        let original = sample(7);
        db.put(&original).unwrap();
        db.put(&original).unwrap();
        for field in 0..4 {
            let mut invalid = original.clone();
            match field {
                0 => invalid.number += 1,
                1 => invalid.hash = B256::ZERO,
                2 => invalid.parent_hash = B256::ZERO,
                _ => invalid.rlp = Bytes::from_static(b"broken"),
            }
            assert!(db.put(&invalid).is_err());
        }
        assert_eq!(db.get(7, original.hash).unwrap().unwrap().rlp, original.rlp);
        assert_eq!(db.purge_settled(8).unwrap(), 1);
    }

    #[test]
    fn corrupted_primary_record_is_an_error_not_a_missing_witness() {
        let dir = tempfile::tempdir().unwrap();
        let db = WitnessDb::open(dir.path()).unwrap();
        let original = sample(7);
        db.put(&original).unwrap();
        let txn = db.env.begin_rw_txn().unwrap();
        let records = txn.open_db(Some(WITNESSES)).unwrap();
        txn.put(
            records.dbi(),
            original.hash,
            b"corrupted",
            WriteFlags::UPSERT,
        )
        .unwrap();
        txn.commit().unwrap();
        assert!(db.get(7, original.hash).is_err());
        assert!(db.purge_settled(8).is_err());
    }

    #[test]
    fn corrupt_height_index_cannot_delete_unfinalized_evidence_and_purge_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let db = WitnessDb::open(dir.path()).unwrap();
        let older = sample(5);
        let newer = sample(20);
        db.put(&older).unwrap();
        db.put(&newer).unwrap();
        let txn = db.env.begin_rw_txn().unwrap();
        let heights = txn.open_db(Some(HEIGHTS)).unwrap();
        txn.put(
            heights.dbi(),
            height_key(6, newer.hash),
            [],
            WriteFlags::UPSERT,
        )
        .unwrap();
        txn.commit().unwrap();
        // The transaction would delete height 5 first, then discovers the corrupt
        // height-6 entry. Neither deletion may become visible.
        assert!(db.purge_settled(10).is_err());
        assert!(db.get(5, older.hash).unwrap().is_some());
        assert!(db.get(20, newer.hash).unwrap().is_some());
    }

    #[test]
    fn finalized_purge_is_bounded_and_repeatable() {
        let dir = tempfile::tempdir().unwrap();
        let db = WitnessDb::open(dir.path()).unwrap();
        for number in 1..=PURGE_BATCH as u64 + 2 {
            db.put(&sample(number)).unwrap();
        }
        let floor = PURGE_BATCH as u64 + 2;
        assert_eq!(db.purge_settled(floor).unwrap(), PURGE_BATCH);
        assert_eq!(db.purge_settled(floor).unwrap(), 1);
        assert_eq!(db.purge_settled(floor).unwrap(), 0);
        assert!(db.get(floor, sample(floor).hash).unwrap().is_some());
    }

    #[test]
    fn old_or_unknown_schema_fails_explicitly_without_rewriting_records() {
        for old in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            {
                let env = Environment::builder().open(dir.path()).unwrap();
                let txn = env.begin_rw_txn().unwrap();
                let db = txn.create_db(None, DatabaseFlags::empty()).unwrap();
                if old {
                    txn.put(
                        db.dbi(),
                        1u64.to_be_bytes(),
                        b"old record",
                        WriteFlags::UPSERT,
                    )
                    .unwrap();
                } else {
                    txn.put(db.dbi(), b"schema", b"unknown", WriteFlags::UPSERT)
                        .unwrap();
                }
                txn.commit().unwrap();
            }
            assert!(WitnessDb::open(dir.path()).is_err());
        }
    }
}
