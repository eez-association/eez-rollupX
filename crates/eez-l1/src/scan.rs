use std::collections::{HashMap, HashSet};

use alloy_consensus::Transaction;
use alloy_consensus::transaction::TxHashRef;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_provider::Provider;
use alloy_rpc_types_eth::Filter;
use alloy_sol_types::{SolCall, SolEvent};
use eez_protocol::abi::{
    BatchPosted, L2ExecutionPerformed, L2TxSkipped, ProofSystemBatchPerVerificationEntriesSol,
    postAndVerifyBatchCall,
};
use tracing::{Level, event};

use crate::error::{L1Error, L1Result};

/// Initial block span for historical log scans. Wide catch-up gaps are
/// split before hitting RPCs that reject long `eth_getLogs` ranges.
pub(crate) const LOG_SCAN_CHUNK_BLOCKS: u64 = 100_000;

/// One `L2ExecutionPerformed.newRoot` block-hash commitment for our rollup,
/// tagged with where in the L1 block it was emitted so it can be attributed to
/// the owning batch.
#[derive(Debug, Clone, Copy)]
struct SettledRoot {
    tx_index: u64,
    log_index: u64,
    /// L1 block this commitment's log landed in — attribution requires it to
    /// match the batch's block hash (fork-pinning).
    block_hash: B256,
    root: B256,
}

/// A decoded `BatchPosted` log, before settlement attribution. Held between the
/// scan's two passes: windows need every batch in the block decoded first.
#[derive(Debug)]
struct DecodedBatchLog {
    l1_block_number: u64,
    l1_block_hash: B256,
    tx_hash: B256,
    tx_index: u64,
    submitter: Address,
    /// This batch lists our rollup, so it wiped our queue — a window boundary.
    verifies_our_rollup: bool,
    transaction_blob_hashes: Vec<B256>,
    post_batch_input: Bytes,
    claimed_current_root: Option<B256>,
    claimed_chain: Vec<B256>,
    /// Proven entry outcome, adjusted for observed `L2TxSkipped` events,
    /// aligned 1:1 with `claimed_chain`. Failed entries and skipped immediates do not leave
    /// `L2ExecutionPerformed` logs. Their checkpoints are carried by the
    /// preceding step only when the proven root chain assigns the SAME root.
    /// A dynamically skipped root-changing entry remains an unconsumed boundary.
    claimed_successes: Vec<bool>,
    /// Physical producing effects, parallel to the chain. The anchor and
    /// authenticated inbound lookups have zero replay weight.
    claimed_effects: Vec<bool>,
}

/// Stateful `BatchPosted` log chunks. Callers own when scanned ranges are
/// committed to their local cursors.
#[derive(Debug)]
pub struct BatchLogChunks {
    to_block: u64,
    ranges: Vec<(u64, u64)>,
}

impl BatchLogChunks {
    pub(crate) fn new(from_block: u64, to_block: u64) -> Self {
        let ranges = if from_block > to_block {
            Vec::new()
        } else {
            initial_log_scan_ranges(from_block, to_block)
        };
        Self { to_block, ranges }
    }

    /// L1 block these chunks were bounded to when created.
    #[must_use]
    pub const fn to_block(&self) -> u64 {
        self.to_block
    }

    /// Returns true when no scan chunks remain.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }
}

/// One decoded `BatchPosted` log: winner flag plus the claimed block hashes
/// from our rollup's legacy-named `RootUpdate`. The Deriver's catch-up scan and
/// the live [`L1Watcher`](crate::L1Watcher) poll consume the same shape.
#[derive(Debug, Clone)]
pub struct ScannedBatch {
    pub l1_block_number: u64,
    /// Hash of the L1 block the batch landed in — canonicality probe
    /// for the resync anchor walk.
    pub l1_block_hash: B256,
    pub tx_hash: B256,
    /// Position of the posting transaction in its L1 block. Together with
    /// `l1_block_number` this gives a stable total order for same-block batches.
    pub tx_index: u64,
    pub submitter: Address,
    /// Authenticated operations payload decoded from this transaction's native
    /// blobs. The initial log scan leaves this empty; blob hydration fills it
    /// before the batch leaves `eez-l1`.
    pub operations_payload: Bytes,
    /// EIP-4844 versioned hashes committed by the originating transaction.
    /// Internal derivation metadata used by the beacon retrieval path before
    /// this value leaves `eez-l1`.
    pub(crate) transaction_blob_hashes: Vec<B256>,
    /// The originating postBatch tx's full input (the `postAndVerifyBatch`
    /// calldata), captured from the tx fetched by (block, index). Carried
    /// so the Deriver's reconcile fallback decodes `batch.entries`
    /// from these bytes instead of re-fetching the tx by hash — that lookup
    /// fails on a pruned or still-resyncing embedded L1 and crashed boot
    /// catch_up on restart-after-post.
    pub post_batch_input: Bytes,
    pub state_applied: bool,
    /// Which of this batch's claimed steps L1 actually ran. See [`Settlement`].
    pub settlement: Settlement,
    pub claimed_current_root: Option<B256>,
    pub claimed_new_root: Option<B256>,
}

/// Scan-only metadata used to remove an L2-range ambiguity that the hash-only
/// L1 registry cannot resolve by itself. Two composers can submit from the
/// same cursor in one L1 block. If an anchor-only `A -> A` batch lands before
/// an `A -> B` batch, both updates execute, but both payloads describe the
/// same L2 range. Advancing the range for the idle batch makes the richer batch
/// replay at the wrong block numbers. Duplicate `A -> A` batches have the same
/// ambiguity even without a richer peer.
#[derive(Debug)]
struct AttributedBatch {
    scanned: ScannedBatch,
    tx_index: u64,
    anchor_only_root_noop: bool,
}

/// Canonicalize competing batches that started at the same commitment in the
/// same L1 block. Prefer any settled non-trivial batch over anchor-only no-ops;
/// when every settled candidate is a no-op, keep only the earliest one. This
/// preserves idle-chain cursor progress while ensuring a root-no-op can never
/// consume the range of a richer competing payload.
fn canonicalize_root_noops(batches: Vec<AttributedBatch>) -> Vec<ScannedBatch> {
    type CompetitionKey = (u64, B256, B256);

    let mut settled_non_noop: HashSet<CompetitionKey> = HashSet::new();
    let mut first_settled_noop: HashMap<CompetitionKey, (u64, usize)> = HashMap::new();

    for (index, batch) in batches.iter().enumerate() {
        if batch.scanned.settlement.is_empty() {
            continue;
        }
        let Some(current_root) = batch.scanned.claimed_current_root else {
            continue;
        };
        let key = (
            batch.scanned.l1_block_number,
            batch.scanned.l1_block_hash,
            current_root,
        );
        if batch.anchor_only_root_noop {
            first_settled_noop
                .entry(key)
                .and_modify(|first| {
                    if batch.tx_index < first.0 {
                        *first = (batch.tx_index, index);
                    }
                })
                .or_insert((batch.tx_index, index));
        } else {
            settled_non_noop.insert(key);
        }
    }

    batches
        .into_iter()
        .enumerate()
        .filter_map(|(index, mut batch)| {
            if !batch.anchor_only_root_noop || batch.scanned.settlement.is_empty() {
                return Some(batch.scanned);
            }
            let current_root = batch.scanned.claimed_current_root?;
            let key = (
                batch.scanned.l1_block_number,
                batch.scanned.l1_block_hash,
                current_root,
            );
            let superseded = settled_non_noop.contains(&key)
                || first_settled_noop
                    .get(&key)
                    .is_some_and(|(_, first_index)| *first_index != index);
            if superseded {
                event!(
                    name: "eez.l1_scan.root_noop_superseded",
                    Level::INFO,
                    l1_block_number = batch.scanned.l1_block_number,
                    tx_hash = %batch.scanned.tx_hash,
                    current_root = %current_root,
                    "ignoring an ambiguous anchor-only root-noop batch in favor of the canonical same-block payload",
                );
                // Preserve conclusive accounting for the posted transaction
                // without assigning the same L2 range to a second payload.
                batch.scanned.settlement = Settlement::NONE;
                Some(batch.scanned)
            } else {
                Some(batch.scanned)
            }
        })
        .collect()
}

/// Which of a batch's claimed L2 steps an observed L1 carrier committed. A
/// successful entry moves the stored block hash and emits `L2ExecutionPerformed`;
/// failed outbound entries emit nothing because their frames revert, so their
/// pre-carried L2 checkpoints belong to the preceding successful carrier.
/// Authenticated inbound lookups have no physical L2 checkpoint. The expanded
/// committed run remains contiguous, so `(start, len)` describes it fully in
/// the virtual `[anchor | producing effects...]` index space.
///
/// `start > 0` = leading steps skipped: a competing same-block batch already made
/// those hops (routine with two composers on a shared tx stream, since identical
/// Sync blocks produce identical hashes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Settlement {
    /// Index into the virtual chain where the applied run starts. Meaningless
    /// when `len == 0`.
    pub start: usize,
    /// How many consecutive steps ran. `0` = nothing settled → skip the batch.
    pub len: usize,
    /// `newRoot` of the last step that ran — L1's ACTUAL stored block hash after this
    /// batch (a prefix endpoint under partial consumption). Reconciliation
    /// validates against THIS, never the claimed full-chain end.
    pub final_state: Option<B256>,
    /// The stored block hash the applied run STARTED from — what reconciliation compares
    /// its local cursor against. The claimed `currentRoot` when `start == 0`,
    /// otherwise the root immediately before the first observed carrier;
    /// using the pre-batch value on a mid-chain resume reports a false divergence.
    pub entry_state: Option<B256>,
}

impl Settlement {
    /// Nothing of this batch applied on L1.
    pub const NONE: Self = Self {
        start: 0,
        len: 0,
        final_state: None,
        entry_state: None,
    };

    /// True when L1 ran none of this batch's steps — the claimed hashes are
    /// phantoms and the batch must be skipped entirely.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Which "producing" entries ran, as `(skip, take)` over that list. Producing
    /// entries carry L2 calls and map 1:1 to reconstructed system txs; claimed
    /// index 0 is the anchor (state-only, signs no system tx), so claimed `i` is
    /// producing `i - 1`:
    /// - `start == 0` — anchor ran → `(0, len - 1)`.
    /// - `start >= 1` — anchor plus `start - 1` producing entries skipped →
    ///   `(start - 1, len)`.
    ///
    /// `start > 1` is not an error: composer 1 posting `A→B→C` and composer 2
    /// posting `A→B→C→D` yield an identical `C`, so composer 2's anchor and `B→C`
    /// are refused as redundant while `C→D` still runs. Refusing to reconstruct
    /// that step would stall the cursor at `C` forever.
    #[must_use]
    pub const fn producing_slice(&self) -> (usize, usize) {
        match self.start {
            0 => (0, self.len.saturating_sub(1)),
            s => (s - 1, self.len),
        }
    }
}

fn initial_log_scan_ranges(from_block: u64, to_block: u64) -> Vec<(u64, u64)> {
    let mut ranges = Vec::new();
    let mut from = from_block;
    loop {
        let to = from
            .saturating_add(LOG_SCAN_CHUNK_BLOCKS.saturating_sub(1))
            .min(to_block);
        ranges.push((from, to));
        if to == to_block {
            break;
        }
        from = to + 1;
    }
    ranges.reverse();
    ranges
}

/// True when a `get_logs` failure means "matched too much", not "was wrong".
/// Providers cap RESULT COUNT, which no fixed block span can respect. Detected by
/// message, since the wording is client-specific but the remedy — narrow — isn't.
fn is_range_too_wide(err: &L1Error) -> bool {
    let L1Error::Provider(msg) = err else {
        return false;
    };
    let m = msg.to_ascii_lowercase();
    m.contains("exceeds max results")
        || m.contains("query returned more than")
        || m.contains("more than 10000 results")
        || m.contains("log response size exceeded")
        || m.contains("response size exceeded")
        || m.contains("query timeout exceeded")
        || m.contains("range is too large")
        || m.contains("block range too large")
        || m.contains("too many results")
}

/// Scan `[from, to]`, halving the upper bound until the provider accepts. Returns
/// the batches plus the block actually REACHED, which may be `< to` — callers must
/// advance their cursor by that, not by `to`. Propagating the refusal instead makes
/// no progress: the Watcher would retry the identical over-wide range forever.
pub(crate) async fn scan_batch_logs_range_adaptive(
    provider: &impl Provider,
    eez: Address,
    rollup_id: u64,
    from: u64,
    to: u64,
) -> L1Result<(Vec<ScannedBatch>, u64)> {
    let mut hi = to;
    loop {
        match scan_batch_logs_range(provider, eez, rollup_id, from, hi).await {
            Ok(scanned) => return Ok((scanned, hi)),
            Err(err) if is_range_too_wide(&err) && hi > from => {
                let mid = from + (hi - from) / 2;
                event!(
                    name: "eez.l1.scan_batch_logs.chunk_narrowed",
                    Level::WARN,
                    from,
                    requested_to = hi,
                    narrowed_to = mid,
                    error = %err,
                    "get_logs matched more than the provider serves; halving the range and retrying",
                );
                hi = mid;
            }
            // A single block that still exceeds the cap is genuinely unservable.
            Err(err) => return Err(err),
        }
    }
}

pub(crate) async fn scan_next_batch_log_chunk(
    provider: &impl Provider,
    eez: Address,
    rollup_id: u64,
    chunks: &mut BatchLogChunks,
) -> L1Result<Option<Vec<ScannedBatch>>> {
    let Some(&(from, to)) = chunks.ranges.last() else {
        return Ok(None);
    };

    let (scanned, reached) =
        scan_batch_logs_range_adaptive(provider, eez, rollup_id, from, to).await?;
    chunks.ranges.pop();
    if reached < to {
        // Narrowed: re-queue the tail so no range is silently skipped.
        chunks.ranges.push((reached + 1, to));
    }
    Ok(Some(scanned))
}

/// Fetch every `BatchPosted` log in `[from_block, to_block]` and cross-
/// reference each against `L2ExecutionPerformed` for our rollup — present
/// ⇔ this batch's commitment delta applied (winner; losers emit `BatchPosted`
/// only). For each, decode the originating tx for the submitter and our
/// rollup's claimed block hashes. The contract retains the legacy `currentRoot`
/// / `newRoot` field names.
pub(crate) async fn scan_batch_logs_range(
    provider: &impl Provider,
    eez: Address,
    rollup_id: u64,
    from_block: u64,
    to_block: u64,
) -> L1Result<Vec<ScannedBatch>> {
    let filter = Filter::new()
        .address(eez)
        .event_signature(BatchPosted::SIGNATURE_HASH)
        .from_block(from_block)
        .to_block(BlockNumberOrTag::Number(to_block));
    let logs = provider
        .get_logs(&filter)
        .await
        .map_err(|e| L1Error::Provider(format!("get_logs(BatchPosted): {e}")))?;

    // Keep settlement outcomes in one RPC query. Besides reducing scan work,
    // this guarantees that performed and skipped entries are observed over the
    // same block range. `L2TxSkipped.topic1` is the transient entry index (not
    // the rollup id), so filter the performed events for our rollup locally.
    let outcomes_filter = Filter::new()
        .address(eez)
        .event_signature(vec![
            L2ExecutionPerformed::SIGNATURE_HASH,
            L2TxSkipped::SIGNATURE_HASH,
        ])
        .from_block(from_block)
        .to_block(BlockNumberOrTag::Number(to_block));
    let outcome_logs = provider
        .get_logs(&outcomes_filter)
        .await
        .map_err(|e| L1Error::Provider(format!("get_logs(settlement outcomes): {e}")))?;
    let rollup_topic = B256::from(U256::from(rollup_id));
    let winner_logs = outcome_logs
        .iter()
        .filter(|log| {
            log.topic0() == Some(&L2ExecutionPerformed::SIGNATURE_HASH)
                && log.topics().get(1) == Some(&rollup_topic)
        })
        .collect::<Vec<_>>();
    let skipped_logs = outcome_logs
        .iter()
        .filter(|log| log.topic0() == Some(&L2TxSkipped::SIGNATURE_HASH))
        .collect::<Vec<_>>();
    let mut skipped_by_tx: HashMap<(B256, B256), HashSet<usize>> = HashMap::new();
    for log in &skipped_logs {
        let (Some(block_hash), Some(tx_hash)) = (log.block_hash, log.transaction_hash) else {
            continue;
        };
        let decoded = L2TxSkipped::decode_log(&alloy_primitives::Log {
            address: log.address(),
            data: log.data().clone(),
        })
        .map_err(|error| L1Error::Decode(format!("decode L2TxSkipped({tx_hash}): {error}")))?;
        let entry_index = usize::try_from(decoded.transientIdx).map_err(|error| {
            L1Error::Decode(format!(
                "L2TxSkipped({tx_hash}) index {} does not fit usize: {error}",
                decoded.transientIdx
            ))
        })?;
        skipped_by_tx
            .entry((block_hash, tx_hash))
            .or_default()
            .insert(entry_index);
    }
    // Pinned by block hash too: the same tx can sit on both sides of a fork.
    let winner_tx_hashes: HashSet<(B256, B256)> = winner_logs
        .iter()
        .filter_map(|l| Some((l.block_hash?, l.transaction_hash?)))
        .collect();
    // Settled roots per L1 block, in emission order and tagged with the emitting
    // tx index. Per-TX-INDEX because two composers can post for the same rollup
    // in one L1 block: crediting a batch with the whole block's roots would hand
    // a loser its rival's settlement. Ordered (not a set) because attribution
    // matches steps positionally.
    let mut settled_by_block: HashMap<u64, Vec<SettledRoot>> = HashMap::new();
    for l in &winner_logs {
        let (Some(bn), Some(tx_index)) = (l.block_number, l.transaction_index) else {
            continue;
        };
        let block_hash = l.block_hash.ok_or_else(|| {
            L1Error::Provider("L2ExecutionPerformed log missing block_hash".into())
        })?;
        let data = l.data().data.as_ref();
        if data.len() == 32 {
            settled_by_block.entry(bn).or_default().push(SettledRoot {
                tx_index,
                log_index: l.log_index.unwrap_or_default(),
                block_hash,
                root: B256::from_slice(data),
            });
        }
    }
    for roots in settled_by_block.values_mut() {
        roots.sort_by_key(|r| (r.tx_index, r.log_index));
    }

    // Pass 1: decode every postBatch. Windows can't be computed until we know
    // which OTHER batches in the same block touch our rollup — each of those
    // wipes our queue (`EEZ.sol:_markVerifiedBlockPerRollup`), ending the
    // previous batch's consumption window.
    let mut decoded_batches: Vec<DecodedBatchLog> = Vec::with_capacity(logs.len());
    for log in &logs {
        let l1_block_number = log
            .block_number
            .ok_or_else(|| L1Error::Provider("BatchPosted log missing block_number".into()))?;
        let l1_block_hash = log
            .block_hash
            .ok_or_else(|| L1Error::Provider("BatchPosted log missing block_hash".into()))?;
        let tx_hash = log
            .transaction_hash
            .ok_or_else(|| L1Error::Provider("BatchPosted log missing tx_hash".into()))?;
        // Fetch the postBatch tx by (block hash, index) — a minimal node has no
        // tx-hash index, and (block NUMBER, index) alone can straddle a reorg.
        let tx_index = log
            .transaction_index
            .ok_or_else(|| L1Error::Provider("BatchPosted log missing transaction_index".into()))?;
        let tx = fetch_log_transaction(provider, l1_block_number, l1_block_hash, tx_index, tx_hash)
            .await?;
        let submitter = tx.inner.signer();
        let input = tx.inner.input();
        // `BatchPosted` carries rollupCount, not rollupId, so we decode every
        // rollup's batch — and a peer posting via a router yields undecodable
        // input. Skip unless it settled OUR rollup (invariant 8).
        let decoded = match postAndVerifyBatchCall::abi_decode(input) {
            Ok(decoded) => decoded,
            Err(e) if !winner_tx_hashes.contains(&(l1_block_hash, tx_hash)) => {
                event!(
                    name: "eez.l1_scan.foreign_batch_undecodable",
                    Level::WARN,
                    l1_block_number,
                    tx_hash = %tx_hash,
                    error = %e,
                    "skipping an undecodable postBatch that did not settle our rollup",
                );
                continue;
            }
            Err(e) => {
                return Err(L1Error::Decode(format!("decode postBatch({tx_hash}): {e}")));
            }
        };
        let _decoded_event = BatchPosted::decode_log(&alloy_primitives::Log {
            address: log.address(),
            data: log.data().clone(),
        })
        .map_err(|e| L1Error::Decode(format!("decode BatchPosted({tx_hash}): {e}")))?;
        let skipped_indices = skipped_by_tx
            .get(&(l1_block_hash, tx_hash))
            .cloned()
            .unwrap_or_default();
        let (claimed_current_root, claimed_chain, claimed_successes, claimed_effects) =
            our_state_chain(&decoded.batch, rollup_id, &skipped_indices);
        decoded_batches.push(DecodedBatchLog {
            l1_block_number,
            l1_block_hash,
            tx_hash,
            tx_index,
            submitter,
            // A batch verifies (and therefore wipes) our rollup iff it lists it
            // — the same array `postAndVerifyBatch` loops over to mark verified.
            verifies_our_rollup: decoded
                .batch
                .rollupIdsWithProofSystems
                .iter()
                .any(|r| r.rollupId == rollup_id),
            transaction_blob_hashes: tx
                .inner
                .blob_versioned_hashes()
                .map_or_else(Vec::new, <[B256]>::to_vec),
            post_batch_input: input.clone(),
            claimed_current_root,
            claimed_chain,
            claimed_successes,
            claimed_effects,
        });
    }

    // Window boundaries: tx indices, per block, of the postBatches that verify
    // our rollup. A batch owns the settled block hashes emitted from its own tx up to
    // (excluding) the next such boundary — its entries are provably dead past
    // that point, queue wiped.
    let mut boundaries: HashMap<(u64, B256), Vec<u64>> = HashMap::new();
    for b in &decoded_batches {
        if b.verifies_our_rollup {
            boundaries
                .entry((b.l1_block_number, b.l1_block_hash))
                .or_default()
                .push(b.tx_index);
        }
    }
    for idxs in boundaries.values_mut() {
        idxs.sort_unstable();
    }

    // Pass 2: attribute each batch its own window, then match steps.
    let mut out: Vec<AttributedBatch> = Vec::with_capacity(decoded_batches.len());
    for b in decoded_batches {
        let window_end = boundaries
            .get(&(b.l1_block_number, b.l1_block_hash))
            .and_then(|idxs| idxs.iter().copied().find(|&i| i > b.tx_index))
            .unwrap_or(u64::MAX);
        // Roots at this height on a DIFFERENT hash mean the two log queries
        // straddled a reorg; retry rather than silently settling empty.
        let block_roots = settled_by_block.get(&b.l1_block_number);
        if block_roots.is_some_and(|roots| roots.iter().all(|r| r.block_hash != b.l1_block_hash)) {
            return Err(L1Error::SourceIncomplete {
                block: b.l1_block_number,
                tx_hash: b.tx_hash,
                detail: "settlement logs are from another fork of this block; retry".into(),
            });
        }
        let observed: Vec<B256> = block_roots
            .map(|roots| window_roots(roots, b.tx_index, window_end, b.l1_block_hash))
            .unwrap_or_default();
        let settlement = attribute_settlement_with_effects(
            b.claimed_current_root,
            &b.claimed_chain,
            &b.claimed_successes,
            &b.claimed_effects,
            &observed,
        );
        let claimed_new_root = b.claimed_chain.last().copied();
        let anchor_only_root_noop = b.claimed_chain.len() == 1
            && b.claimed_current_root.is_some()
            && b.claimed_current_root == claimed_new_root;
        out.push(AttributedBatch {
            scanned: ScannedBatch {
                l1_block_number: b.l1_block_number,
                l1_block_hash: b.l1_block_hash,
                tx_hash: b.tx_hash,
                tx_index: b.tx_index,
                submitter: b.submitter,
                operations_payload: Bytes::new(),
                transaction_blob_hashes: b.transaction_blob_hashes,
                post_batch_input: b.post_batch_input,
                state_applied: winner_tx_hashes.contains(&(b.l1_block_hash, b.tx_hash)),
                settlement,
                claimed_current_root: b.claimed_current_root,
                claimed_new_root,
            },
            tx_index: b.tx_index,
            anchor_only_root_noop,
        });
    }
    Ok(canonicalize_root_noops(out))
}

/// Our rollup's commitment chain in a batch: the first update's
/// `currentRoot` (pre-batch block hash) and the ordered per-update `newRoot`
/// block hashes. The function name retains protocol terminology.
fn our_state_chain(
    batch: &ProofSystemBatchPerVerificationEntriesSol,
    rollup_id: u64,
    skipped_entry_indices: &HashSet<usize>,
) -> (Option<B256>, Vec<B256>, Vec<bool>, Vec<bool>) {
    let mut first_curr: Option<B256> = None;
    let mut new_roots: Vec<B256> = Vec::new();
    let mut successes: Vec<bool> = Vec::new();
    let mut effects: Vec<bool> = Vec::new();
    for (entry_index, entry) in batch.entries.iter().enumerate() {
        let lookup = eez_protocol::settlement::is_lookup_only_inbound_entry(entry, rollup_id);
        for update in &entry.rootUpdates {
            if update.rollupId == rollup_id {
                effects.push(!new_roots.is_empty() && !lookup);
                if first_curr.is_none() {
                    first_curr = Some(update.currentRoot);
                }
                new_roots.push(update.newRoot);
                successes.push(entry.success && !skipped_entry_indices.contains(&entry_index));
            }
        }
    }
    (first_curr, new_roots, successes, effects)
}

/// Commitments settled inside this batch's tx-index window AND on this batch's
/// own L1 block hash — a same-numbered hash from a different fork never
/// attributes.
fn window_roots(
    roots: &[SettledRoot],
    tx_index: u64,
    window_end: u64,
    block_hash: B256,
) -> Vec<B256> {
    roots
        .iter()
        .filter(|r| r.tx_index >= tx_index && r.tx_index < window_end && r.block_hash == block_hash)
        .map(|r| r.root)
        .collect()
}

/// Which of this batch's claimed steps L1 ran (see [`Settlement`]). `observed` is
/// the ordered `newRoot` sequence emitted inside this batch's window, located as
/// a consecutive slice of `claimed_chain` — positional, not set membership, so
/// duplicate roots stay distinct and a coincidental match can't inflate the count.
/// [`Settlement::NONE`] when nothing matches (empty window or phantom roots).
#[cfg(test)]
fn attribute_settlement(
    claimed_current: Option<B256>,
    claimed_chain: &[B256],
    observed: &[B256],
) -> Settlement {
    attribute_settlement_with_successes(
        claimed_current,
        claimed_chain,
        &vec![true; claimed_chain.len()],
        observed,
    )
}

/// One root update whose successful L1 frame persists, plus the contiguous L2
/// effect range whose checkpoints that update carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SettlementCarrier {
    claimed_index: usize,
    root: B256,
    effect_start: usize,
    effect_end: usize,
    /// A successful lookup can still be rolled back by its source frame.
    /// Its no-op log is optional; producing commitments are not.
    optional: bool,
}

/// Collapse the claimed per-entry root chain into the steps that can actually
/// emit on L1. A failed entry belongs to the preceding carrier only when its
/// claimed root equals that carrier's root, as required by the composer's
/// proven carrier plan. A dynamically skipped root-changing entry does not
/// move its checkpoint into a previous carrier. Keeping it as a boundary makes
/// the observed prefix/suffix select only the effects actually committed.
fn settlement_carriers(
    claimed_chain: &[B256],
    claimed_successes: &[bool],
    claimed_effects: &[bool],
) -> Vec<SettlementCarrier> {
    if claimed_chain.is_empty()
        || claimed_chain.len() != claimed_successes.len()
        || claimed_chain.len() != claimed_effects.len()
        || claimed_effects[0]
    {
        return Vec::new();
    }

    // Keep zero-effect lookup roots in the event chain: a successful lookup
    // can emit a no-op root, while a reverted lookup's logs roll back. Neither
    // creates an L2 delivery or expands an outbound carrier's replay range.
    let mut effect_ends = Vec::with_capacity(claimed_chain.len());
    let mut effect_count = 0;
    for produces in claimed_effects {
        effect_count += usize::from(*produces); // bounded by the slice length
        effect_ends.push(effect_count);
    }
    let carrier_starts = claimed_successes[1..]
        .iter()
        .enumerate()
        .filter_map(|(effect_index, success)| {
            (*success || claimed_chain[effect_index + 1] != claimed_chain[effect_index])
                .then_some(effect_index)
        })
        .collect::<Vec<_>>();
    let leading_end = carrier_starts
        .first()
        .map_or(effect_count, |index| effect_ends[*index]);
    let mut carriers = Vec::with_capacity(1 + carrier_starts.len());
    carriers.push(SettlementCarrier {
        claimed_index: 0,
        root: claimed_chain[0],
        effect_start: 0,
        effect_end: leading_end,
        optional: false,
    });
    for (position, root_index) in carrier_starts.iter().copied().enumerate() {
        let effect_start = effect_ends[root_index];
        let effect_end = carrier_starts
            .get(position + 1)
            .map_or(effect_count, |index| effect_ends[*index]);
        carriers.push(SettlementCarrier {
            claimed_index: root_index + 1,
            root: claimed_chain[root_index + 1],
            effect_start,
            effect_end,
            optional: !claimed_effects[root_index + 1],
        });
    }
    carriers
}

#[cfg(test)]
fn attribute_settlement_with_successes(
    claimed_current: Option<B256>,
    claimed_chain: &[B256],
    claimed_successes: &[bool],
    observed: &[B256],
) -> Settlement {
    let effects = (0..claimed_chain.len())
        .map(|index| index != 0)
        .collect::<Vec<_>>();
    attribute_settlement_with_effects(
        claimed_current,
        claimed_chain,
        claimed_successes,
        &effects,
        observed,
    )
}

fn attribute_settlement_with_effects(
    claimed_current: Option<B256>,
    claimed_chain: &[B256],
    claimed_successes: &[bool],
    claimed_effects: &[bool],
    observed: &[B256],
) -> Settlement {
    let carriers = settlement_carriers(claimed_chain, claimed_successes, claimed_effects);
    if observed.is_empty() || carriers.is_empty() {
        return Settlement::NONE;
    }
    // Producing carriers must remain consecutive. A successful lookup's no-op
    // log may persist or may roll back with its source frame. Match those logs
    // optionally without discarding their positions or increasing replay weight.
    if observed.len() > carriers.len() {
        return Settlement::NONE;
    }
    // `starts[i]` is the earliest possible start of a run that has consumed
    // the observed prefix and is about to visit carrier i. Propagating only
    // through optional carriers retains strict ordering and multiplicity for
    // physical effects, even when adjacent roots are equal.
    let mut starts = (0..=carriers.len()).map(Some).collect::<Vec<_>>();
    for root in observed {
        let mut next = vec![None; carriers.len() + 1];
        for (index, carrier) in carriers.iter().enumerate() {
            let Some(start) = starts[index] else {
                continue;
            };
            if carrier.optional {
                starts[index + 1] = Some(starts[index + 1].map_or(start, |old| old.min(start)));
            }
            if carrier.root == *root {
                next[index + 1] = Some(start);
            }
        }
        starts = next;
    }
    let Some((carrier_start, carrier_end)) = starts
        .iter()
        .enumerate()
        .filter_map(|(end, start)| start.map(|start| (start, end)))
        .min()
    else {
        // The window settled block hashes this batch never claimed. Not attributable
        // to it — treat as "nothing of ours applied" and let the caller skip;
        // the cursor guard catches any real misalignment loudly.
        event!(
            name: "eez.l1.scan_batch_logs.settlement_unmatched",
            Level::WARN,
            observed = observed.len(),
            claimed = claimed_chain.len(),
            "settled block hashes in this batch's window match no consecutive run of its claimed chain",
        );
        return Settlement::NONE;
    };
    let first = carriers[carrier_start];
    let last = carriers[carrier_end - 1];

    // Preserve Settlement's virtual `[anchor | producing effects...]` index
    // space. A carrier expands to every failed effect checkpoint it commits,
    // so `producing_slice()` continues to return one contiguous effect range.
    let (start, len) = if first.claimed_index == 0 {
        (0, 1 + last.effect_end)
    } else {
        (first.effect_start + 1, last.effect_end - first.effect_start)
    };
    Settlement {
        start,
        len,
        final_state: observed.last().copied(),
        // Leading steps skipped ⇒ the run began at the last skipped step's
        // `newRoot` (a competing batch had already made that hop).
        entry_state: if first.claimed_index == 0 {
            claimed_current
        } else {
            claimed_chain.get(first.claimed_index - 1).copied()
        },
    }
}

/// Fetches the postBatch tx by (block hash, index) — never by tx hash, which
/// a minimal node can't serve. The fetched tx's own hash must match the
/// log's, else a reorg swapped the block between the log fetch and this call.
async fn fetch_log_transaction(
    provider: &impl Provider,
    l1_block_number: u64,
    l1_block_hash: B256,
    tx_index: u64,
    tx_hash: B256,
) -> L1Result<alloy_rpc_types_eth::Transaction> {
    let Some(tx) = provider
        .get_transaction_by_block_hash_and_index(l1_block_hash, tx_index as usize)
        .await
        .map_err(|e| {
            L1Error::Provider(format!(
                "get_tx({l1_block_hash}#{tx_index} for {tx_hash}): {e}"
            ))
        })?
    else {
        return Err(L1Error::SourceIncomplete {
            block: l1_block_number,
            tx_hash,
            detail: format!("block-hash/index lookup returned null at tx index {tx_index}"),
        });
    };

    if *tx.inner.tx_hash() != tx_hash {
        return Err(L1Error::SourceIncomplete {
            block: l1_block_number,
            tx_hash,
            detail: format!(
                "tx at ({l1_block_hash}, {tx_index}) does not match the log's tx hash — reorg during scan; retry"
            ),
        });
    }
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::{
        AttributedBatch, BatchLogChunks, LOG_SCAN_CHUNK_BLOCKS, ScannedBatch, SettledRoot,
        Settlement, attribute_settlement, attribute_settlement_with_effects,
        attribute_settlement_with_successes, canonicalize_root_noops, fetch_log_transaction,
        initial_log_scan_ranges, our_state_chain, scan_batch_logs_range, scan_next_batch_log_chunk,
        window_roots,
    };
    use crate::error::L1Error;
    use alloy_consensus::transaction::TxHashRef;
    use alloy_primitives::{Address, B256, Bytes, U256};
    use alloy_provider::ProviderBuilder;
    use alloy_sol_types::{SolCall, SolEvent};
    use alloy_transport::mock::Asserter;
    use eez_protocol::abi::{
        BatchPosted, EtherDeltaSol, ExecutionEntrySol, L2ExecutionPerformed,
        ProofSystemBatchPerVerificationEntriesSol, RollupIdWithProofSystemsSol, RootUpdateSol,
        postAndVerifyBatchCall,
    };
    use std::collections::HashSet;

    #[test]
    fn initial_log_scan_ranges_stack_order() {
        let c = LOG_SCAN_CHUNK_BLOCKS;
        struct Case {
            name: &'static str,
            from: u64,
            to: u64,
            stored_stack: Vec<(u64, u64)>,
            pop_order: Vec<(u64, u64)>,
        }

        let cases = vec![
            Case {
                name: "single block",
                from: 10,
                to: 10,
                stored_stack: vec![(10, 10)],
                pop_order: vec![(10, 10)],
            },
            Case {
                name: "exactly one chunk",
                from: 1,
                to: c,
                stored_stack: vec![(1, c)],
                pop_order: vec![(1, c)],
            },
            Case {
                name: "one block past a chunk",
                from: 1,
                to: c + 1,
                stored_stack: vec![(c + 1, c + 1), (1, c)],
                pop_order: vec![(1, c), (c + 1, c + 1)],
            },
            Case {
                name: "nonzero start exact chunks",
                from: 10,
                to: 10 + 2 * c - 1,
                stored_stack: vec![(10 + c, 10 + 2 * c - 1), (10, 10 + c - 1)],
                pop_order: vec![(10, 10 + c - 1), (10 + c, 10 + 2 * c - 1)],
            },
            Case {
                name: "multiple chunks with partial tail",
                from: 42,
                to: 42 + 2 * c + 6,
                stored_stack: vec![
                    (42 + 2 * c, 42 + 2 * c + 6),
                    (42 + c, 42 + 2 * c - 1),
                    (42, 42 + c - 1),
                ],
                pop_order: vec![
                    (42, 42 + c - 1),
                    (42 + c, 42 + 2 * c - 1),
                    (42 + 2 * c, 42 + 2 * c + 6),
                ],
            },
            Case {
                name: "near u64 max does not overflow",
                from: u64::MAX - 1,
                to: u64::MAX,
                stored_stack: vec![(u64::MAX - 1, u64::MAX)],
                pop_order: vec![(u64::MAX - 1, u64::MAX)],
            },
        ];

        for case in cases {
            let mut ranges = initial_log_scan_ranges(case.from, case.to);
            assert_eq!(ranges, case.stored_stack, "{}", case.name);

            let mut pop_order = Vec::new();
            while let Some(range) = ranges.pop() {
                pop_order.push(range);
            }
            assert_eq!(pop_order, case.pop_order, "{}", case.name);
        }
    }

    /// Idle `A→A` and rich `A→B` share an L1 block; each is judged against the
    /// roots settled in ITS OWN window, not the block's last root.
    #[test]
    fn same_block_batches_attributed_per_window_not_block_last() {
        let a = B256::repeat_byte(0xAA);
        let b = B256::repeat_byte(0xBB);
        assert_eq!(
            attribute_settlement(None, &[a], &[a]),
            Settlement {
                start: 0,
                len: 1,
                final_state: Some(a),
                entry_state: None
            },
        );
        assert_eq!(
            attribute_settlement(None, &[b], &[b]),
            Settlement {
                start: 0,
                len: 1,
                final_state: Some(b),
                entry_state: None
            },
        );
    }

    fn attributed_batch(
        tx_byte: u8,
        tx_index: u64,
        current_root: B256,
        new_root: B256,
        settled: bool,
        anchor_only_root_noop: bool,
    ) -> AttributedBatch {
        AttributedBatch {
            scanned: ScannedBatch {
                l1_block_number: 42,
                l1_block_hash: B256::repeat_byte(0x42),
                tx_hash: B256::repeat_byte(tx_byte),
                tx_index,
                submitter: Address::ZERO,
                operations_payload: Bytes::new(),
                transaction_blob_hashes: Vec::new(),
                post_batch_input: Bytes::new(),
                state_applied: settled,
                settlement: if settled {
                    Settlement {
                        start: 0,
                        len: 1,
                        final_state: Some(new_root),
                        entry_state: Some(current_root),
                    }
                } else {
                    Settlement::NONE
                },
                claimed_current_root: Some(current_root),
                claimed_new_root: Some(new_root),
            },
            tx_index,
            anchor_only_root_noop,
        }
    }

    /// Both updates execute on L1 because the first leaves the root at A, but
    /// their DA payloads cover the same L2 range. The richer A→B payload is the
    /// only canonical range; retaining A→A would replay A→B at later heights.
    #[test]
    fn same_block_rich_batch_supersedes_anchor_only_root_noop() {
        let (a, b) = (B256::repeat_byte(0xAA), B256::repeat_byte(0xBB));
        let canonical = canonicalize_root_noops(vec![
            attributed_batch(1, 3, a, a, true, true),
            attributed_batch(2, 7, a, b, true, false),
        ]);
        assert_eq!(canonical.len(), 2);
        assert!(canonical[0].settlement.is_empty());
        assert_eq!(canonical[1].tx_hash, B256::repeat_byte(2));
        assert!(!canonical[1].settlement.is_empty());
    }

    /// Exercise the complete RPC decode, per-window attribution, no-op
    /// classification, and canonicalization path for the exact race observed
    /// in the two-composer reorg E2E.
    #[tokio::test]
    async fn scan_prefers_same_block_rich_payload_over_settled_root_noop() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        let block_hash = B256::repeat_byte(0x42);
        let (a, b) = (B256::repeat_byte(0xAA), B256::repeat_byte(0xBB));
        let noop = batch_tx(1, Some((a, a)), 1);
        let rich = batch_tx(1, Some((a, b)), 2);
        let noop_hash = *noop.inner.tx_hash();
        let rich_hash = *rich.inner.tx_hash();

        asserter.push_success(&vec![
            batch_posted_log(42, block_hash, noop_hash, 3),
            batch_posted_log(42, block_hash, rich_hash, 7),
        ]);
        asserter.push_success(&vec![
            settled_root_log(1, a, 42, block_hash, noop_hash, 3, 0),
            settled_root_log(1, b, 42, block_hash, rich_hash, 7, 0),
        ]);
        asserter.push_success(&noop);
        asserter.push_success(&rich);

        let scanned = scan_batch_logs_range(&provider, Address::ZERO, 1, 42, 42)
            .await
            .expect("scan succeeds");
        assert_eq!(scanned.len(), 2);
        assert_eq!(scanned[0].tx_hash, noop_hash);
        assert!(scanned[0].settlement.is_empty());
        assert_eq!(scanned[1].tx_hash, rich_hash);
        assert_eq!(scanned[1].claimed_current_root, Some(a));
        assert_eq!(scanned[1].claimed_new_root, Some(b));
    }

    /// An idle chain still needs one no-op batch to advance its derived cursor,
    /// but competing duplicates must not advance the same range twice.
    #[test]
    fn duplicate_same_block_root_noops_keep_earliest_settled_payload() {
        let a = B256::repeat_byte(0xAA);
        let canonical = canonicalize_root_noops(vec![
            attributed_batch(2, 7, a, a, true, true),
            attributed_batch(1, 3, a, a, true, true),
        ]);
        assert_eq!(canonical.len(), 2);
        assert!(canonical[0].settlement.is_empty());
        assert_eq!(canonical[1].tx_hash, B256::repeat_byte(1));
        assert!(!canonical[1].settlement.is_empty());
    }

    /// A reverted/unsettled competitor consumes no L2 range and therefore
    /// cannot supersede the one settled idle payload.
    #[test]
    fn unsettled_rich_competitor_does_not_suppress_settled_root_noop() {
        let (a, b) = (B256::repeat_byte(0xAA), B256::repeat_byte(0xBB));
        let canonical = canonicalize_root_noops(vec![
            attributed_batch(1, 3, a, a, true, true),
            attributed_batch(2, 7, a, b, false, false),
        ]);
        assert_eq!(canonical.len(), 2);
        assert_eq!(canonical[0].tx_hash, B256::repeat_byte(1));
    }

    /// A loser gets an EMPTY window (its rival's events belong to the rival's
    /// window), so nothing is attributed to it ⇒ the deriver skips it. Crediting
    /// the loser with the winner's root let its phantom `currentRoot` reach the
    /// cursor guard and fail boot catch_up.
    #[test]
    fn loser_with_empty_window_is_skipped() {
        let b = B256::repeat_byte(0xBB);
        let y = B256::repeat_byte(0xCC);
        assert_eq!(attribute_settlement(None, &[y], &[]), Settlement::NONE);
        // Even when the loser's chain SHARES a root with the winner's (overlapping
        // ranges), an empty window keeps it unattributed.
        assert_eq!(attribute_settlement(None, &[y, b], &[]), Settlement::NONE);
    }

    /// Partial consumption: only a prefix ran → endpoint is the last root that
    /// ran, not the claimed end.
    #[test]
    fn partial_consumption_uses_last_root_that_ran() {
        let b = B256::repeat_byte(0x0B);
        let c = B256::repeat_byte(0x0C);
        let d = B256::repeat_byte(0x0D);
        assert_eq!(
            attribute_settlement(None, &[b, c, d], &[b, c]),
            Settlement {
                start: 0,
                len: 2,
                final_state: Some(c),
                entry_state: None
            },
        );
    }

    /// Full consumption: the claimed end ran → it's the endpoint.
    #[test]
    fn full_consumption_uses_claimed_end() {
        let b = B256::repeat_byte(0x0B);
        let c = B256::repeat_byte(0x0C);
        assert_eq!(
            attribute_settlement(None, &[b, c], &[b, c]),
            Settlement {
                start: 0,
                len: 2,
                final_state: Some(c),
                entry_state: None
            },
        );
    }

    /// Empty window (nothing settled for our rollup) → unsettled.
    #[test]
    fn empty_window_is_unsettled() {
        assert_eq!(
            attribute_settlement(None, &[B256::repeat_byte(1)], &[]),
            Settlement::NONE,
        );
    }

    /// Two composers in ONE L1 block: pb1 claims A→B and lands; pb2 claims
    /// A→B→C, so its A→B is refused as redundant and only B→C runs. pb2 must be
    /// credited with C alone and aligned on B — the claimed A reported a false
    /// `local_diverged`.
    #[test]
    fn two_composers_second_batch_resumes_after_first_hop() {
        let (a, b, c) = (
            B256::repeat_byte(0x0A),
            B256::repeat_byte(0x0B),
            B256::repeat_byte(0x0C),
        );

        let s1 = attribute_settlement(Some(a), &[b], &[b]);
        assert_eq!(
            s1,
            Settlement {
                start: 0,
                len: 1,
                final_state: Some(b),
                entry_state: Some(a),
            },
        );

        let s2 = attribute_settlement(Some(a), &[b, c], &[c]);
        assert_eq!(
            s2,
            Settlement {
                start: 1,
                len: 1,
                final_state: Some(c),
                entry_state: Some(b),
            },
        );
        assert_ne!(s2.entry_state, Some(a));
        assert_eq!(s2.producing_slice(), (0, 1));

        // Per-block attribution hands pb2 both roots, claiming pb1's hop as its
        // own — that disagreement was the bug.
        let per_block = attribute_settlement(Some(a), &[b, c], &[b, c]);
        assert_eq!((per_block.start, per_block.len), (0, 2));
        assert_ne!((per_block.start, per_block.len), (s2.start, s2.len));
    }

    /// Anchor skipped: a competing same-block batch already made the anchor's
    /// hop (A→B), so ours was refused as redundant while the producing steps
    /// still ran. The run starts at index 1 — counting producing entries as
    /// `len - 1` would under-count them and truncate a system tx.
    #[test]
    fn anchor_skipped_run_starts_at_one() {
        let b = B256::repeat_byte(0x0B); // anchor's newRoot
        let c = B256::repeat_byte(0x0C);
        let d = B256::repeat_byte(0x0D);
        let s = attribute_settlement(None, &[b, c, d], &[c, d]);
        assert_eq!(
            s,
            Settlement {
                start: 1,
                len: 2,
                final_state: Some(d),
                entry_state: Some(b)
            }
        );
        // Both producing steps ran — NOT `len - 1`.
        assert_eq!(s.producing_slice(), (0, 2));
    }

    /// Anchor ran: producing entries are `len - 1`.
    #[test]
    fn anchor_applied_excludes_itself_from_producing_count() {
        let b = B256::repeat_byte(0x0B);
        let c = B256::repeat_byte(0x0C);
        let s = attribute_settlement(None, &[b, c], &[b, c]);
        assert_eq!(s.start, 0);
        assert_eq!(s.producing_slice(), (0, 1));
    }

    /// Two composers on a SHARED tx stream: composer 1 posts `A→B→C` and lands
    /// first, composer 2 posts `A→B→C→D` over the same txs. Composer 2's anchor
    /// and `B→C` are refused as redundant (root is already `C`), but `C→D`
    /// matches the live root and RUNS. The run resumes mid-chain and must be
    /// reconstructed — refusing would stall the cursor at `C` forever.
    #[test]
    fn shared_tx_stream_run_resumes_mid_chain() {
        let b = B256::repeat_byte(0x0B); // anchor A→B
        let c = B256::repeat_byte(0x0C); // tx-1   B→C  (competitor already made this hop)
        let d = B256::repeat_byte(0x0D); // tx-2   C→D  (only this one ran)
        let s = attribute_settlement(None, &[b, c, d], &[d]);
        assert_eq!(
            s,
            Settlement {
                start: 2,
                len: 1,
                final_state: Some(d),
                entry_state: Some(c)
            }
        );
        // Skip the producing entry the competitor settled (tx-1), take tx-2.
        assert_eq!(s.producing_slice(), (1, 1));
        // Reconciliation compares its cursor against `C` — the root the run began
        // at — not the claimed `currentRoot` (`A`), which would look diverged.
        assert_eq!(s.entry_state, Some(c));
    }

    /// Deeper resume: a competitor supplied the anchor plus TWO producing hops.
    #[test]
    fn producing_slice_skips_every_step_before_the_run() {
        let r: Vec<B256> = (0x0Bu8..=0x0F).map(B256::repeat_byte).collect();
        let s = attribute_settlement(None, &r, &r[3..]);
        assert_eq!((s.start, s.len), (3, 2));
        // claimed [3,4] → producing [2,3] → skip 2, take 2
        assert_eq!(s.producing_slice(), (2, 2));
    }

    /// Matching is positional, so a duplicate root can't inflate the run and an
    /// out-of-order coincidence isn't credited.
    #[test]
    fn matching_is_positional_not_set_membership() {
        let b = B256::repeat_byte(0x0B);
        let c = B256::repeat_byte(0x0C);
        // Repeated root: the run is located, not counted twice.
        assert_eq!(
            attribute_settlement(None, &[b, b, c], &[b, b]),
            Settlement {
                start: 0,
                len: 2,
                final_state: Some(b),
                entry_state: None
            },
        );
        // Out of order → no consecutive run matches → unattributed.
        assert_eq!(
            attribute_settlement(None, &[b, c], &[c, b]),
            Settlement::NONE
        );
    }

    /// A failed source entry leaves no L1 execution event, but its successful
    /// L2 delivery is already committed by the anchor's carrier root. The
    /// deriver must therefore rebuild one producing effect, not an empty Sync
    /// block.
    #[test]
    fn anchor_event_carries_a_leading_failed_effect() {
        let root = B256::repeat_byte(0x0B);
        let settlement =
            attribute_settlement_with_successes(None, &[root, root], &[true, false], &[root]);
        assert_eq!((settlement.start, settlement.len), (0, 2));
        assert_eq!(settlement.producing_slice(), (0, 1));
        assert_eq!(settlement.final_state, Some(root));
    }

    /// Carrier groups partition `[failed, failed, success, failed, success]`
    /// as `anchor:[0,2)`, `success-2:[2,4)`, `success-4:[4,5)`.
    #[test]
    fn carrier_events_expand_over_failed_effect_groups() {
        let a = B256::repeat_byte(0x0A);
        let b = B256::repeat_byte(0x0B);
        let c = B256::repeat_byte(0x0C);
        let chain = [a, a, a, b, b, c];
        let successes = [true, false, false, true, false, true];

        let full = attribute_settlement_with_successes(None, &chain, &successes, &[a, b, c]);
        assert_eq!((full.start, full.len), (0, 6));
        assert_eq!(full.producing_slice(), (0, 5));

        let resumed = attribute_settlement_with_successes(None, &chain, &successes, &[b, c]);
        assert_eq!((resumed.start, resumed.len), (3, 3));
        assert_eq!(resumed.producing_slice(), (2, 3));
        assert_eq!(resumed.entry_state, Some(a));
    }

    /// Caught destination reverts and successful calls rolled back by their
    /// source frame are authenticated lookups, with no physical L2 delivery.
    /// Keep their optional logs distinct from the producing prefix/suffix.
    #[test]
    fn inbound_lookups_preserve_event_positions_without_producing_effects() {
        use eez_protocol::rolling_hash::EntryRollingHash;

        let rollup_id = 17;
        let [before, anchor, endpoint] = [1, 2, 3].map(B256::with_last_byte);
        let root_update = |current_root, new_root| RootUpdateSol {
            rollupId: rollup_id,
            currentRoot: current_root,
            newRoot: new_root,
            etherDelta: EtherDeltaSol::ZERO,
        };
        let lookup = |root, success| {
            let proxy = B256::with_last_byte(0xAB);
            ExecutionEntrySol {
                destinationRollupId: rollup_id,
                proxyEntryHash: proxy,
                rootUpdates: vec![root_update(root, root)],
                rollingHash: EntryRollingHash::seed_for_l1([(rollup_id, root)], proxy).current(),
                success,
                ..Default::default()
            }
        };
        for lookup_success in [false, true] {
            let batch = ProofSystemBatchPerVerificationEntriesSol {
                entries: vec![
                    ExecutionEntrySol {
                        success: true,
                        rootUpdates: vec![root_update(before, anchor)],
                        ..Default::default()
                    },
                    lookup(anchor, lookup_success),
                    ExecutionEntrySol {
                        success: true,
                        rootUpdates: vec![root_update(anchor, endpoint)],
                        ..Default::default()
                    },
                    lookup(endpoint, lookup_success),
                ],
                ..Default::default()
            };
            let (current, chain, successes, effects) =
                our_state_chain(&batch, rollup_id, &HashSet::new());
            assert_eq!(effects, [false, false, true, false]);
            let attribute = |observed: &[B256]| {
                attribute_settlement_with_effects(current, &chain, &successes, &effects, observed)
            };
            // Both failed entries and successful entries whose source later
            // reverts leave no lookup logs. Neither may expand the range.
            let full = attribute(&[anchor, endpoint]);
            assert_eq!((full.start, full.len), (0, 2));
            assert_eq!(full.producing_slice(), (0, 1));
            assert_eq!(full.entry_state, Some(before));
            assert_eq!(full.final_state, Some(endpoint));
            assert_eq!(attribute(&[anchor]).producing_slice(), (0, 0));
            let resumed = attribute(&[endpoint]);
            assert_eq!((resumed.start, resumed.len), (1, 1));
            assert_eq!(resumed.entry_state, Some(anchor));
            if lookup_success {
                // A lookup log can also persist. Test either optional log,
                // both, and a prefix ending at the leading lookup.
                for observed in [
                    vec![anchor, anchor, endpoint],
                    vec![anchor, endpoint, endpoint],
                    vec![anchor, anchor, endpoint, endpoint],
                ] {
                    assert_eq!(attribute(&observed), full);
                }
                assert_eq!(attribute(&[anchor, anchor]).producing_slice(), (0, 0));
                assert_eq!(attribute(&[endpoint, endpoint]), resumed);
                assert_eq!(attribute(&[anchor, anchor, anchor]), Settlement::NONE);
            }
        }
    }

    #[test]
    fn effect_projection_rejects_invalid_masks_and_keeps_producing_gaps_strict() {
        let chain = [1, 2, 3, 4].map(B256::with_last_byte);
        let successes = [true; 4];
        for effects in [&[false, true][..], &[true; 4][..]] {
            assert_eq!(
                attribute_settlement_with_effects(None, &chain, &successes, effects, &chain),
                Settlement::NONE,
            );
        }
        // Optional no-op matching must never skip a physical carrier.
        assert_eq!(
            attribute_settlement_with_effects(
                None,
                &chain,
                &successes,
                &[false, true, true, true],
                &[chain[0], chain[2], chain[3]],
            ),
            Settlement::NONE,
        );
    }

    /// A root-changing immediate skipped after proving leaves its L2 effect
    /// unconsumed. Only an already-proved equal-root carrier can cover it.
    #[test]
    fn skipped_immediate_entry_is_not_carried_without_a_matching_commitment() {
        let rollup_id = 17;
        let current = B256::repeat_byte(0x09);
        let anchor = B256::repeat_byte(0x0A);
        let effect = B256::repeat_byte(0x0B);
        let root_update = |current_root, new_root| RootUpdateSol {
            rollupId: rollup_id,
            currentRoot: current_root,
            newRoot: new_root,
            etherDelta: EtherDeltaSol::ZERO,
        };
        let batch = ProofSystemBatchPerVerificationEntriesSol {
            entries: vec![
                ExecutionEntrySol {
                    success: true,
                    rootUpdates: vec![root_update(current, anchor)],
                    ..Default::default()
                },
                ExecutionEntrySol {
                    success: true,
                    rootUpdates: vec![root_update(anchor, effect)],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let skipped = HashSet::from([1]);

        let (claimed_current, chain, persisted, effects) =
            our_state_chain(&batch, rollup_id, &skipped);
        assert_eq!(claimed_current, Some(current));
        assert_eq!(chain, vec![anchor, effect]);
        assert_eq!(persisted, vec![true, false]);
        assert_eq!(effects, vec![false, true]);

        let settlement =
            attribute_settlement_with_successes(claimed_current, &chain, &persisted, &[anchor]);
        assert_eq!((settlement.start, settlement.len), (0, 1));
        assert_eq!(settlement.producing_slice(), (0, 0));
        assert_eq!(settlement.final_state, Some(anchor));

        // A legitimately pre-carried reverted outcome repeats the carrier
        // commitment. This must still emit its valid L2 replay group.
        let carried = attribute_settlement_with_successes(
            Some(current),
            &[anchor, anchor],
            &persisted,
            &[anchor],
        );
        assert_eq!(carried.producing_slice(), (0, 1));
    }

    #[test]
    fn skipped_first_middle_last_outbounds_preserve_prefix_and_resume_boundaries() {
        let chain = [1, 2, 3, 4].map(B256::with_last_byte);
        for committed in 0..3 {
            let persisted = (0..4).map(|index| index <= committed).collect::<Vec<_>>();
            let prefix = attribute_settlement_with_successes(
                Some(B256::ZERO),
                &chain,
                &persisted,
                &chain[..=committed],
            );
            assert_eq!(prefix.producing_slice(), (0, committed));
            assert_eq!(prefix.final_state, Some(chain[committed]));

            let resumed = attribute_settlement_with_successes(
                Some(B256::ZERO),
                &chain,
                &(0..4).map(|index| index > committed).collect::<Vec<_>>(),
                &chain[committed + 1..],
            );
            assert_eq!(resumed.producing_slice(), (committed, 3 - committed));
            assert_eq!(resumed.entry_state, Some(chain[committed]));
            assert_eq!(resumed.final_state, Some(chain[3]));
        }
    }

    /// Roots this batch never claimed (window contamination) are not attributed.
    #[test]
    fn unclaimed_roots_are_not_attributed() {
        let b = B256::repeat_byte(0x0B);
        let z = B256::repeat_byte(0x7A);
        assert_eq!(attribute_settlement(None, &[b], &[z]), Settlement::NONE);
    }

    /// A root logged against a DIFFERENT fork of the same block NUMBER (hash B,
    /// not this batch's hash A) must not be attributed — it settles as empty,
    /// exactly like any other non-matching root.
    #[test]
    fn window_roots_excludes_a_different_forks_root_at_the_same_block_number() {
        let hash_a = B256::repeat_byte(0xA1);
        let hash_b = B256::repeat_byte(0xB2);
        let root = B256::repeat_byte(0x0D);
        let roots = [SettledRoot {
            tx_index: 0,
            log_index: 0,
            block_hash: hash_b,
            root,
        }];
        assert!(window_roots(&roots, 0, u64::MAX, hash_a).is_empty());
        assert_eq!(window_roots(&roots, 0, u64::MAX, hash_b), vec![root]);
    }

    /// A minimal, serializable RPC transaction for mocked provider
    /// responses. Signed with a fixed test vector, so its hash is
    /// deterministic — the identity check compares against it.
    fn mock_rpc_transaction() -> alloy_rpc_types_eth::Transaction {
        use alloy_consensus::{SignableTransaction, TxEnvelope, TxLegacy, transaction::Recovered};
        let tx = TxLegacy {
            chain_id: Some(1),
            nonce: 0,
            gas_price: 1,
            gas_limit: 21_000,
            to: alloy_primitives::TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input: Bytes::new(),
        };
        let signed = tx.into_signed(alloy_primitives::Signature::test_signature());
        alloy_rpc_types_eth::Transaction {
            inner: Recovered::new_unchecked(TxEnvelope::Legacy(signed), Address::ZERO),
            block_hash: None,
            block_number: None,
            block_timestamp: None,
            transaction_index: None,
            effective_gas_price: None,
        }
    }

    /// A `postAndVerifyBatch` tx carrying `input`, signed with a fixed test
    /// vector — its hash is a deterministic function of `input` alone, so
    /// distinct batches naturally get distinct hashes.
    /// A postBatch tx for `rollup_id`, optionally claiming one
    /// `current -> new` step. `salt` varies the encoding so each call yields a
    /// distinct tx hash.
    fn batch_tx(
        rollup_id: u64,
        step: Option<(B256, B256)>,
        salt: u64,
    ) -> alloy_rpc_types_eth::Transaction {
        let batch = ProofSystemBatchPerVerificationEntriesSol {
            rollupIdsWithProofSystems: vec![RollupIdWithProofSystemsSol {
                rollupId: rollup_id,
                proofSystemIndexes: vec![],
            }],
            immediateEntryCount: U256::from(salt),
            entries: step
                .map(|(current, new)| {
                    vec![ExecutionEntrySol {
                        rootUpdates: vec![RootUpdateSol {
                            rollupId: rollup_id,
                            currentRoot: current,
                            newRoot: new,
                            etherDelta: EtherDeltaSol::ZERO,
                        }],
                        ..Default::default()
                    }]
                })
                .unwrap_or_default(),
            ..Default::default()
        };
        mock_post_batch_tx(postAndVerifyBatchCall { batch }.abi_encode())
    }

    fn mock_post_batch_tx(input: Vec<u8>) -> alloy_rpc_types_eth::Transaction {
        use alloy_consensus::{SignableTransaction, TxEnvelope, TxLegacy, transaction::Recovered};
        let tx = TxLegacy {
            chain_id: Some(1),
            nonce: 0,
            gas_price: 1,
            gas_limit: 21_000,
            to: alloy_primitives::TxKind::Call(Address::ZERO),
            value: U256::ZERO,
            input: Bytes::from(input),
        };
        let signed = tx.into_signed(alloy_primitives::Signature::test_signature());
        alloy_rpc_types_eth::Transaction {
            inner: Recovered::new_unchecked(TxEnvelope::Legacy(signed), Address::ZERO),
            block_hash: None,
            block_number: None,
            block_timestamp: None,
            transaction_index: None,
            effective_gas_price: None,
        }
    }

    /// A `BatchPosted` log at the given (block, tx) coordinates.
    fn batch_posted_log(
        block_number: u64,
        block_hash: B256,
        tx_hash: B256,
        tx_index: u64,
    ) -> alloy_rpc_types_eth::Log {
        alloy_rpc_types_eth::Log {
            inner: alloy_primitives::Log {
                address: Address::ZERO,
                data: BatchPosted {
                    rollupCount: U256::from(1),
                }
                .encode_log_data(),
            },
            block_hash: Some(block_hash),
            block_number: Some(block_number),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: Some(tx_index),
            log_index: Some(0),
            removed: false,
        }
    }

    /// An `L2ExecutionPerformed` log settling `root` for `rollup_id` at the
    /// given (block, tx, log) coordinates.
    fn settled_root_log(
        rollup_id: u64,
        root: B256,
        block_number: u64,
        block_hash: B256,
        tx_hash: B256,
        tx_index: u64,
        log_index: u64,
    ) -> alloy_rpc_types_eth::Log {
        alloy_rpc_types_eth::Log {
            inner: alloy_primitives::Log {
                address: Address::ZERO,
                data: L2ExecutionPerformed {
                    rollupId: rollup_id,
                    newRoot: root,
                }
                .encode_log_data(),
            },
            block_hash: Some(block_hash),
            block_number: Some(block_number),
            block_timestamp: None,
            transaction_hash: Some(tx_hash),
            transaction_index: Some(tx_index),
            log_index: Some(log_index),
            removed: false,
        }
    }

    /// The boot-crash fix's linchpin: a tx the L1 serves at (block hash, index) is
    /// returned when its hash matches the log's; a null lookup classifies as
    /// `SourceIncomplete` (retryable) rather than a fatal provider error.
    ///
    /// The mock is a method-agnostic FIFO, so this pins response consumption
    /// counts and the classification behavior — not which RPC method was used.
    #[tokio::test]
    async fn tx_lookup_hits_by_hash_index_then_classifies_null_source_incomplete() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        let real_hash = *mock_rpc_transaction().inner.tx_hash();

        // (a) by-(block hash, index) hit, hash matches the log: accepted directly.
        asserter.push_success(&mock_rpc_transaction());
        fetch_log_transaction(&provider, 14, B256::ZERO, 0, real_hash)
            .await
            .expect("index lookup hit");

        // (b) null lookup → retryable SourceIncomplete carrying the block and
        // tx hash context.
        asserter.push_success(&serde_json::Value::Null);
        let err = fetch_log_transaction(&provider, 14, B256::ZERO, 7, real_hash)
            .await
            .expect_err("null lookup must not yield a tx");
        assert!(err.is_source_incomplete(), "unexpected error: {err}");
        match err {
            L1Error::SourceIncomplete {
                block, tx_hash: h, ..
            } => {
                assert_eq!(block, 14);
                assert_eq!(h, real_hash);
            }
            other => panic!("expected SourceIncomplete, got {other}"),
        }
    }

    /// A one-block reorg between the log fetch and this call can return a
    /// DIFFERENT tx at the same (block hash, index) slot; the tx's own hash must
    /// still match the log's, or it must be rejected — not laundered into the batch.
    #[tokio::test]
    async fn tx_lookup_rejects_a_tx_whose_hash_does_not_match_the_log() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        let real_hash = *mock_rpc_transaction().inner.tx_hash();
        let claimed_hash = B256::with_last_byte(0x51);
        assert_ne!(claimed_hash, real_hash, "test vector must actually differ");

        asserter.push_success(&mock_rpc_transaction());
        let err = fetch_log_transaction(&provider, 14, B256::ZERO, 0, claimed_hash)
            .await
            .expect_err("mismatched tx must be rejected, not accepted");
        assert!(err.is_source_incomplete(), "unexpected error: {err}");
    }

    /// A result-count refusal must NARROW the range, not abort the scan: the
    /// Watcher's catch-up aborts its tick before advancing its ring, so
    /// propagating means it retries the identical over-wide range forever, and
    /// boot `catch_up` propagating means the node never starts.
    #[tokio::test]
    async fn result_limit_refusal_narrows_and_requeues_the_tail() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        let mut chunks = BatchLogChunks::new(1_288, 33_632);
        assert_eq!(chunks.ranges.len(), 1, "one chunk (span < 100k)");

        // reth's wording; the retry of the narrowed half then succeeds.
        asserter
            .push_failure_msg("query exceeds max results 20000, retry with the range 1288-33632");
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([]));
        let scanned = scan_next_batch_log_chunk(&provider, Address::ZERO, 1, &mut chunks)
            .await
            .expect("must NOT propagate — narrowing is the remedy")
            .expect("chunk yielded");
        assert!(scanned.is_empty());

        // Covered only the lower half; the tail is re-queued so nothing is skipped.
        let mid = 1_288 + (33_632 - 1_288) / 2;
        assert_eq!(chunks.ranges.len(), 1, "tail re-queued");
        assert_eq!(*chunks.ranges.last().expect("tail"), (mid + 1, 33_632));
    }

    /// A single-block chunk that still exceeds the cap is genuinely unservable —
    /// propagate rather than split forever.
    #[tokio::test]
    async fn unsplittable_single_block_refusal_propagates() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        let mut chunks = BatchLogChunks::new(500, 500);
        asserter.push_failure_msg("query exceeds max results 20000");
        let err = scan_next_batch_log_chunk(&provider, Address::ZERO, 1, &mut chunks)
            .await
            .expect_err("a one-block range cannot be narrowed further");
        assert!(
            err.to_string().contains("exceeds max results"),
            "got: {err}"
        );
        assert_eq!(chunks.ranges.len(), 1, "range preserved for retry");
    }

    /// A failed chunk scan must NOT consume the range: the same range is
    /// retried on the next call. A successful scan consumes exactly the
    /// oldest range. This is the invariant the watcher's per-chunk
    /// catch-up (and the deriver's retry loop) rely on.
    #[tokio::test]
    async fn failed_chunk_scan_preserves_range() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        // Two ranges: [(0, 99_999), (100_000, 150_000)] stored reversed.
        let mut chunks = BatchLogChunks::new(0, 150_000);
        assert_eq!(chunks.ranges.len(), 2);

        // First get_logs of the oldest chunk fails.
        asserter.push_failure_msg("injected: range scan failed");
        let err = scan_next_batch_log_chunk(&provider, Address::ZERO, 1, &mut chunks)
            .await
            .expect_err("injected failure must propagate");
        assert!(
            err.to_string().contains("injected"),
            "unexpected error: {err}"
        );
        // Range NOT consumed.
        assert_eq!(chunks.ranges.len(), 2);
        assert_eq!(
            *chunks.ranges.last().expect("oldest range intact"),
            (0, 99_999)
        );

        // Retry succeeds (BatchPosted logs + winners logs, both empty).
        asserter.push_success(&serde_json::json!([]));
        asserter.push_success(&serde_json::json!([]));
        let scanned = scan_next_batch_log_chunk(&provider, Address::ZERO, 1, &mut chunks)
            .await
            .expect("retry succeeds")
            .expect("chunk yielded");
        assert!(scanned.is_empty());
        // Exactly the oldest range consumed; newer range still queued.
        assert_eq!(chunks.ranges.len(), 1);
        assert_eq!(
            *chunks.ranges.last().expect("tail range"),
            (100_000, 150_000)
        );
    }

    /// `state_applied` is pinned by `(block_hash, tx_hash)` — a settlement log
    /// carrying the SAME tx hash but a DIFFERENT block hash (as a reorg would
    /// produce) must not credit the batch.
    #[tokio::test]
    async fn state_applied_is_pinned_by_block_hash_not_tx_hash_alone() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());

        let hash_a = B256::with_last_byte(0xA1);
        let hash_b = B256::with_last_byte(0xB2);
        let tx = batch_tx(1, None, 0);
        let tx_hash = *tx.inner.tx_hash();

        asserter.push_success(&vec![batch_posted_log(100, hash_a, tx_hash, 0)]);
        asserter.push_success(&vec![
            // Unrelated settlement on our OWN hash — keeps this distinct from
            // the all-different-fork case (SourceIncomplete; tested separately).
            settled_root_log(
                1,
                B256::repeat_byte(0xD4),
                100,
                hash_a,
                B256::with_last_byte(0xC3),
                1,
                0,
            ),
            // Same tx hash as our postBatch tx, but a DIFFERENT block hash.
            settled_root_log(1, B256::repeat_byte(0xE5), 100, hash_b, tx_hash, 2, 0),
        ]);
        asserter.push_success(&tx);

        let scanned = scan_batch_logs_range(&provider, Address::ZERO, 1, 100, 100)
            .await
            .expect("scan succeeds");
        assert_eq!(scanned.len(), 1);
        assert!(
            !scanned[0].state_applied,
            "same tx hash on a different block hash must not mark state_applied"
        );
    }

    /// Window boundaries are keyed by `(block_number, block_hash)` — a rival
    /// batch on a DIFFERENT fork of the same block number must not truncate
    /// our window.
    #[tokio::test]
    async fn boundaries_are_keyed_by_block_hash_not_number_alone() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        let (hash_a, hash_b) = (B256::with_last_byte(0xA1), B256::with_last_byte(0xB2));
        let (c0, c1) = (B256::repeat_byte(0x50), B256::repeat_byte(0x51));

        let ours = batch_tx(1, Some((c0, c1)), 0);
        let rival = batch_tx(1, None, 1); // same block NUMBER, different HASH
        let boundary = batch_tx(1, None, 2); // our real next boundary, hash A
        let (ours_hash, rival_hash, boundary_hash) = (
            *ours.inner.tx_hash(),
            *rival.inner.tx_hash(),
            *boundary.inner.tx_hash(),
        );

        asserter.push_success(&vec![
            batch_posted_log(500, hash_a, ours_hash, 3),
            batch_posted_log(500, hash_b, rival_hash, 5),
            batch_posted_log(500, hash_a, boundary_hash, 9),
        ]);
        asserter.push_success(&vec![
            // tx_index 7: past the rival's 5, before our real boundary at 9.
            settled_root_log(1, c1, 500, hash_a, B256::with_last_byte(0xF0), 7, 0),
            // Decoy on the rival's hash so its own pass doesn't trip the
            // all-roots-on-a-foreign-fork guard first.
            settled_root_log(
                1,
                B256::repeat_byte(0x99),
                500,
                hash_b,
                B256::with_last_byte(0xF1),
                5,
                0,
            ),
        ]);
        asserter.push_success(&ours);
        asserter.push_success(&rival);
        asserter.push_success(&boundary);

        let scanned = scan_batch_logs_range(&provider, Address::ZERO, 1, 500, 500)
            .await
            .expect("scan succeeds");
        let ours = scanned
            .iter()
            .find(|b| b.tx_hash == ours_hash)
            .expect("our batch scanned");
        assert_eq!(
            ours.settlement,
            Settlement {
                start: 0,
                len: 1,
                final_state: Some(c1),
                entry_state: Some(c0),
            },
            "the rival on another fork must not cut our window at its tx_index"
        );
    }

    #[tokio::test]
    async fn cross_fork_settlement_logs_are_source_incomplete_not_empty() {
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());

        let hash_a = B256::with_last_byte(0xA1);
        let hash_b = B256::with_last_byte(0xB2);
        let tx = batch_tx(1, None, 0);
        let tx_hash = *tx.inner.tx_hash();

        asserter.push_success(&vec![batch_posted_log(700, hash_a, tx_hash, 0)]);
        asserter.push_success(&vec![settled_root_log(
            1,
            B256::repeat_byte(0x77),
            700,
            hash_b,
            B256::with_last_byte(0xC3),
            1,
            0,
        )]);
        asserter.push_success(&tx);

        let err = scan_batch_logs_range(&provider, Address::ZERO, 1, 700, 700)
            .await
            .expect_err("all settlement roots on a different fork must not settle empty");
        assert!(err.is_source_incomplete(), "unexpected error: {err}");
    }

    /// `BatchPosted` carries rollupCount, not rollupId, so a peer posting via a
    /// router yields an input we cannot decode. That must not halt us — unless
    /// the same tx settled OUR rollup, which we then genuinely cannot derive.
    #[tokio::test]
    async fn undecodable_foreign_batch_is_skipped_but_one_that_settled_us_is_fatal() {
        let block_hash = B256::with_last_byte(0xA1);
        let tx = mock_post_batch_tx(b"not a postAndVerifyBatch call".to_vec());
        let tx_hash = *tx.inner.tx_hash();

        // Did not settle our rollup: skipped, scan still succeeds.
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        asserter.push_success(&vec![batch_posted_log(700, block_hash, tx_hash, 0)]);
        asserter.push_success(&Vec::<alloy_rpc_types_eth::Log>::new());
        asserter.push_success(&tx);
        let scanned = scan_batch_logs_range(&provider, Address::ZERO, 1, 700, 700)
            .await
            .expect("a foreign undecodable batch must not fail the scan");
        assert!(scanned.is_empty());

        // Settled our rollup: we cannot derive it, so fail loudly.
        let asserter = Asserter::new();
        let provider = ProviderBuilder::new().connect_mocked_client(asserter.clone());
        asserter.push_success(&vec![batch_posted_log(700, block_hash, tx_hash, 0)]);
        asserter.push_success(&vec![settled_root_log(
            1,
            B256::repeat_byte(0x77),
            700,
            block_hash,
            tx_hash,
            0,
            0,
        )]);
        asserter.push_success(&tx);
        let err = scan_batch_logs_range(&provider, Address::ZERO, 1, 700, 700)
            .await
            .expect_err("an undecodable batch that moved our root must not be skipped");
        assert!(err.is_terminal(), "unexpected error: {err}");
    }
}
