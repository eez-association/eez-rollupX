//! L1-event-driven L2 consensus: replays `BatchPosted` events into
//! local reth, advances safe/finalized via [`BlockCommitterHandle`],
//! and maintains a per-batch index so [`L1Event::Reorg`] retreats
//! the safe head and [`L1Event::Finalized`] advances finalized.
//!
//! STF-replay pattern adapted from `based-rollup`'s `build_derived_block`
//! at `/root/sync-rollups-composer/crates/based-rollup/src/driver/protocol_txs.rs:453`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use alloy_eips::{Decodable2718, Encodable2718};
use alloy_primitives::{Address, B256, Bytes};
use alloy_rpc_types_engine::ExecutionData;
use eez_driver::{BlockCommitterHandle, DeriveOutcome};
use eez_evm::EezEvmConfig;
use eez_l1::{BatchRecord, L1CanonicalHead, L1Event, L1Reader, ScannedBatch};
use eez_payload_codec::v0::Payload;
use eez_primitives::engine::EezEngineTypes;
use eez_primitives::{Block as EthereumBlock, EezTxEnvelope as TransactionSigned};
use eez_protocol::outbound_gate::OutboundCallObservation;
use reth_chainspec::ChainSpec;
use reth_evm::{ConfigureEvm, execute::BlockBuilder};
use reth_payload_primitives::PayloadTypes;
use reth_primitives_traits::{
    AlloyBlockHeader, Block, BlockBody, SealedHeader, SignedTransaction, SignerRecoverable,
};
use reth_provider::StateProviderFactory;
use reth_revm::database::StateProviderDatabase;
use reth_storage_api::{
    BlockReader, EvmStateProviderAdapter, ReceiptProvider, TransactionsProvider,
};
use revm::database::State;
use tokio::sync::broadcast;
use tracing::{Level, event};

use crate::correlation::{BatchCorrelation, CorrelationStore, L2BlockCorrelation};
use crate::error::{DeriverError, DeriverResult};

mod inputs;
mod source_retry;
use inputs::ExecutionInputs;

/// Watcher seed: the finalized block, kept inside the range this scan read.
/// Both bounds have wedged boot in the field. Separate fn so they stay
/// unit-testable without a provider.
fn choose_seed(floor: u64, end: u64, finalized: Option<u64>) -> u64 {
    // `floor.min(end)` is load-bearing: `clamp` PANICS when min > max, which a
    // rewound L1 can produce.
    finalized.unwrap_or(floor).clamp(floor.min(end), end)
}

/// L1-derived L2 consensus engine. Cheaply [`Clone`]able.
#[derive(Clone)]
pub struct Deriver<L2>
where
    L2: BlockReader,
{
    inner: Arc<Inner<L2>>,
}

struct Inner<L2>
where
    L2: BlockReader,
{
    committer: BlockCommitterHandle<EezEngineTypes>,
    l2_provider: Arc<L2>,
    l1_reader: L1Reader,
    derivation: eez_l1::derivation::DerivationSource,
    header_rules: eez_protocol::l1_history::HeaderRules,
    offline_profile: eez_protocol::l1_history::OfflineProfile,
    evm_config: EezEvmConfig,
    deploy_block: u64,
    /// Shared canonical-head state — cursor + per-batch index +
    /// `finalized_l2`. The Deriver is the sole writer; the Composer
    /// reads `cursor()` to compute the next batch's `from_block`.
    l1_head: Arc<L1CanonicalHead>,
    /// Persistent provenance index shared with the node's read-only RPC.
    correlations: Arc<CorrelationStore>,
    /// L2 block number currently reth `safe` head points at. Mirrors
    /// what we last passed to [`BlockCommitterHandle::advance_safe_finalized`];
    /// used to compute the FCU when advancing finalized without
    /// disturbing safe (and vice versa).
    safe_l2_block: AtomicU64,
    /// A failed/lagged live event must be repaired before a later scan marker
    /// can authorize source-transaction recovery.
    scan_needs_resync: std::sync::atomic::AtomicBool,
    /// Cross-chain system-tx reconstruction config. `Some` enables
    /// L1-entries → L2 system-tx prepending in `reconcile_batch_blocks`;
    /// `None` falls back to pure-user-tx STF. See [`Deriver::new`] docs.
    system_tx_cfg: Option<eez_protocol::system_tx::SystemTxContext>,
}

impl<L2> std::fmt::Debug for Deriver<L2>
where
    L2: BlockReader,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Deriver")
            .field("cursor", &self.inner.l1_head.cursor())
            .field(
                "safe_l2_block",
                &self.inner.safe_l2_block.load(Ordering::Acquire),
            )
            .field("finalized_l2_block", &self.inner.l1_head.finalized_l2())
            .field("committer", &self.inner.committer)
            .finish_non_exhaustive()
    }
}

impl<L2> Deriver<L2>
where
    L2: BlockReader<Header = alloy_consensus::Header>
        + StateProviderFactory
        + Send
        + Sync
        + 'static,
    <L2 as TransactionsProvider>::Transaction: Encodable2718,
    <L2 as ReceiptProvider>::Receipt: alloy_consensus::TxReceipt<Log = alloy_primitives::Log>,
{
    /// Builds a deriver. Cursor + per-batch index are populated lazily
    /// by `catch_up_to`, which walks historical `BatchPosted` events
    /// applying the same linearity check live events get — so losers
    /// (competing batches whose `currentRoot` no longer matches the
    /// cursor) don't pollute the index.
    ///
    /// `system_tx_cfg = Some(_)` enables the cross-chain STF path: the
    /// deriver reconstructs the same keyless native system transactions
    /// the composer produced (from the postBatch's `entries[]` /
    /// `l2_entries[]`) and prepends them to the batch's Sync block, so
    /// local replay is byte-identical. `None` is the pure-user-tx STF.
    pub async fn new(
        committer: BlockCommitterHandle<EezEngineTypes>,
        l2_provider: Arc<L2>,
        l1_reader: L1Reader,
        chain_spec: Arc<ChainSpec>,
        offline_rules: eez_l1::chain_config::OfflineL1Rules,
        deploy_block: u64,
        l1_head: Arc<L1CanonicalHead>,
        correlations: Arc<CorrelationStore>,
        system_tx_cfg: Option<eez_protocol::system_tx::SystemTxContext>,
    ) -> DeriverResult<Self> {
        let schedule =
            eez_evm::profile::schedule(chain_spec.as_ref()).map_err(DeriverError::l2_provider)?;
        // Readiness was established before construction, but the source can
        // drop out again (e.g. an embedded L1 restart); retry only that case.
        let derivation =
            source_retry::retry_unavailable(source_retry::POLL, source_retry::MAX_ATTEMPTS, || {
                l1_reader.derivation_source(schedule)
            })
            .await
            .map_err(DeriverError::l2_provider)?;
        let offline_profile = offline_rules
            .profile(schedule, chain_spec.chain().id())
            .map_err(DeriverError::l2_provider)?;
        l1_head.initialize_history(schedule);
        let evm_config = EezEvmConfig::new(chain_spec)
            .with_l1_history(l1_head.clone())
            .map_err(DeriverError::l2_provider)?;
        Ok(Self {
            inner: Arc::new(Inner {
                committer,
                l2_provider,
                l1_reader,
                derivation,
                header_rules: offline_rules.headers,
                offline_profile,
                evm_config,
                deploy_block,
                l1_head,
                correlations,
                safe_l2_block: AtomicU64::new(0),
                scan_needs_resync: std::sync::atomic::AtomicBool::new(true),
                system_tx_cfg,
            }),
        })
    }

    /// Bounded wait for the L1 head to reach `block`. Best effort: on timeout
    /// the caller reads whatever L1 serves and the usual validation decides.
    async fn await_l1_head(&self, block: u64) {
        if crate::l1_catch_up::wait_for_head(block, || async {
            self.inner.l1_reader.head_block_number().await.ok()
        })
        .await
        {
            return;
        }
        event!(
            name: "eez.deriver.correlation.l1_behind",
            Level::WARN,
            l1_block = block,
            "L1 head still below the indexed block; validating anyway",
        );
    }

    /// Validate and adopt the persistent correlation tail as the boot scan
    /// anchor. A tail from an orphaned L1 fork or another L2 datadir is removed
    /// one L1 block at a time until the newest surviving record proves both
    /// sides. Validating the tail pins every older L1 record through ancestry.
    async fn correlation_seed(&self) -> DeriverResult<Option<(u64, u64)>> {
        loop {
            let Some(tail) = self
                .inner
                .correlations
                .tail()
                .map_err(DeriverError::l2_provider)?
            else {
                return Ok(None);
            };
            self.await_l1_head(tail.l1_block_number).await;
            let canonical = self
                .inner
                .l1_reader
                .canonical_l1_hash(tail.l1_block_number)
                .await
                .map_err(DeriverError::l1_scan)?;
            let Some(canonical) = canonical else {
                return Err(DeriverError::l1_scan(eez_l1::L1Error::SourceIncomplete {
                    block: tail.l1_block_number,
                    tx_hash: tail.l1_transaction_hash,
                    detail: "L1 does not serve the persistent correlation tail".into(),
                }));
            };
            let last_l2 = tail.last_l2();
            let local_l2 = self.l2_hash_at(last_l2.number).ok();
            if canonical != tail.l1_block_hash || local_l2 != Some(last_l2.hash) {
                let ancestor = tail.l1_block_number.saturating_sub(1);
                let removed = self
                    .inner
                    .correlations
                    .remove_after_l1(ancestor)
                    .map_err(DeriverError::l2_provider)?;
                event!(
                    name: "eez.deriver.correlation.tail_rejected",
                    Level::WARN,
                    l1_block = tail.l1_block_number,
                    indexed_l1_hash = %tail.l1_block_hash,
                    canonical_l1_hash = %canonical,
                    l2_block = last_l2.number,
                    indexed_l2_hash = %last_l2.hash,
                    local_l2_hash = ?local_l2,
                    removed,
                    "persistent correlation tail does not match canonical L1/local L2; removing its L1-block suffix",
                );
                continue;
            }

            // Seed every record in the tail block. The subsequent inclusive
            // scan starts at this block, so this de-duplicates same-block peers
            // as well as the final transaction and preserves transaction order.
            let tail_block_records = self
                .inner
                .correlations
                .by_l1_number(tail.l1_block_number)
                .map_err(DeriverError::l2_provider)?;
            self.inner
                .l1_head
                .append_many(tail_block_records.iter().map(|record| BatchRecord {
                    l1_block: record.l1_block_number,
                    l1_block_hash: record.l1_block_hash,
                    tx_hash: record.l1_transaction_hash,
                    last_l2_block: record.last_l2().number,
                }));
            event!(
                name: "eez.deriver.correlation.seeded",
                Level::INFO,
                l1_block = tail.l1_block_number,
                l2_cursor = last_l2.number,
                records_in_tail_block = tail_block_records.len(),
                "boot seeded from the persistent L1/L2 correlation index",
            );
            return Ok(Some((tail.l1_block_number, last_l2.number)));
        }
    }

    fn correlation_record(
        &self,
        l1_block_number: u64,
        l1_block_hash: B256,
        tx_hash: B256,
        tx_index: u64,
        first_l2: u64,
        last_l2: u64,
        newest_hash: Option<B256>,
    ) -> DeriverResult<BatchCorrelation> {
        if last_l2 < first_l2 {
            return Err(DeriverError::l2_provider(format!(
                "invalid settled L2 correlation range {first_l2}..={last_l2} for {tx_hash}"
            )));
        }
        let mut l2_blocks = Vec::with_capacity(
            usize::try_from(last_l2 - first_l2 + 1)
                .map_err(|_| DeriverError::l2_provider("L2 correlation range is too large"))?,
        );
        for number in first_l2..=last_l2 {
            let hash = if number == last_l2 {
                match newest_hash {
                    Some(hash) => hash,
                    None => self.l2_hash_at(number)?,
                }
            } else {
                self.l2_hash_at(number)?
            };
            l2_blocks.push(L2BlockCorrelation { number, hash });
        }
        let endpoint_header = self
            .inner
            .l2_provider
            .sealed_header_by_hash(l2_blocks.last().expect("nonempty checked range").hash)
            .map_err(DeriverError::l2_provider)?
            .ok_or_else(|| DeriverError::l2_provider("accepted endpoint header is unavailable"))?
            .into_header();
        let provider = self
            .inner
            .l2_provider
            .state_by_block_hash(endpoint_header.hash_slow())
            .map_err(DeriverError::l2_provider)?;
        let mut db = StateProviderDatabase::new(EvmStateProviderAdapter(provider.as_ref()));
        let endpoint_history = self
            .inner
            .evm_config
            .history_state(&mut db)
            .map_err(DeriverError::l2_provider)?;
        Ok(BatchCorrelation {
            endpoint_header,
            endpoint_history,
            l1_block_number,
            l1_block_hash,
            l1_transaction_hash: tx_hash,
            l1_transaction_index: tx_index,
            l2_blocks,
        })
    }

    fn publish_correlation(
        &self,
        correlation: BatchCorrelation,
        evidence: eez_l1::ReconciledBatchSettlement,
        l1: &eez_l1::derivation::AuthenticatedL1Header,
    ) -> DeriverResult<()> {
        let origin = correlation.endpoint_history;
        self.inner
            .l1_head
            .validate_accepted(&evidence, l1, &correlation.endpoint_header, origin)
            .map_err(DeriverError::l2_provider)?;
        self.inner
            .correlations
            .put(&correlation)
            .map_err(DeriverError::l2_provider)?;
        self.inner
            .l1_head
            .publish_accepted(evidence, l1, &correlation.endpoint_header, origin)
            .map_err(DeriverError::l2_provider)?;
        Ok(())
    }

    /// Recheck the canonical history endpoint of every unsafe block. An L1
    /// reorg can change an imported hash without changing PREVRANDAO. The final
    /// selected hash commits all preceding imports through parent linkage.
    /// Replace the first stale child with an executed empty sibling so Reth can
    /// reinject ordinary transactions; the replacement remains unsafe.
    async fn discard_stale_history_suffix(
        &self,
        mut retained: SealedHeader<alloy_consensus::Header>,
    ) -> DeriverResult<()> {
        let head = self.inner.committer.last_header();
        while retained.number < head.number {
            let number = retained
                .number
                .checked_add(1)
                .ok_or_else(|| DeriverError::l2_provider("L2 height overflow"))?;
            let next = if number == head.number {
                head.clone()
            } else {
                self.l2_sealed_header_at(number)?
            };
            let (timestamp, _) = self
                .inner
                .l1_head
                .next_block_context(&retained)
                .map_err(DeriverError::l2_provider)?;
            if next.parent_hash != retained.hash()
                || next.timestamp != timestamp
                || !self.history_matches(&next)?
            {
                let environment = eez_driver::derivation::Environment {
                    fee_recipient: next.beneficiary,
                    extra_data: next.extra_data.clone(),
                };
                let transactions = {
                    let provider = self
                        .inner
                        .l2_provider
                        .state_by_block_hash(retained.hash())
                        .map_err(DeriverError::l2_provider)?;
                    let mut db =
                        StateProviderDatabase::new(EvmStateProviderAdapter(provider.as_ref()));
                    let (_, history) = self
                        .inner
                        .evm_config
                        .prepare_history(&mut db, &retained, timestamp)
                        .map_err(DeriverError::l2_provider)?;
                    history
                        .into_iter()
                        .map(|tx| Bytes::from(tx.encoded_2718()))
                        .collect::<Vec<_>>()
                };
                let (payload, header) = self.execute_block_on_parent(
                    &retained,
                    &transactions,
                    ExecutionInputs::Derived(&environment),
                )?;
                self.inner
                    .committer
                    .commit_derived(payload, header, true)
                    .await?;
                break;
            }
            retained = next;
        }
        Ok(())
    }

    /// Acquire outside the production lock, then atomically publish only if the
    /// canonical generation still matches. Execution never performs RPC reads.
    async fn prepare_history(
        &self,
        parent: &alloy_consensus::Header,
        through: eez_l1::L1BlockRef,
    ) -> DeriverResult<()> {
        use eez_protocol::l1_history::{ENDPOINT_SLOT, HISTORY_ADDRESS};
        let state = self
            .inner
            .l2_provider
            .state_by_block_hash(parent.hash_slow())
            .map_err(DeriverError::l2_provider)?;
        let first_number = u64::try_from(
            state
                .storage(
                    HISTORY_ADDRESS,
                    alloy_primitives::U256::from(ENDPOINT_SLOT).into(),
                )
                .map_err(DeriverError::l2_provider)?
                .unwrap_or_default(),
        )
        .map_err(DeriverError::l2_provider)?;
        self.prepare_history_from_endpoint(first_number, through)
            .await
    }

    async fn prepare_history_from_endpoint(
        &self,
        first_number: u64,
        through: eez_l1::L1BlockRef,
    ) -> DeriverResult<()> {
        let generation = self.inner.l1_head.policy_anchor_snapshot().generation;
        let previous = self
            .inner
            .l1_head
            .prepared_history()
            .map(|prepared| prepared.chain());
        let prepared = self
            .inner
            .derivation
            .prepare_history(first_number, through, self.inner.header_rules, previous)
            .await
            .map_err(|error| match error {
                eez_l1::derivation::DerivationError::Unavailable(detail) => {
                    DeriverError::l1_scan(eez_l1::L1Error::SourceIncomplete {
                        block: through.number,
                        tx_hash: B256::ZERO,
                        detail: detail.into(),
                    })
                }
                error => DeriverError::l2_provider(error),
            })?;
        self.inner
            .l1_head
            .publish_history(prepared, generation)
            .map_err(DeriverError::l2_provider)
    }

    /// Existing local blocks already passed the mandatory hook. Checking their
    /// selected endpoint against canonical evidence detects changed ancestors,
    /// including bridge-only hashes and branches with identical randomness.
    fn history_matches(
        &self,
        block: &SealedHeader<alloy_consensus::Header>,
    ) -> DeriverResult<bool> {
        use alloy_primitives::U256;
        use eez_protocol::l1_history::{ENDPOINT_SLOT, HISTORY_ADDRESS, hash_storage_key};
        let chain = self
            .inner
            .evm_config
            .l1_headers(block.timestamp)
            .map_err(DeriverError::l2_provider)?;
        let state = self
            .inner
            .l2_provider
            .state_by_block_hash(block.hash())
            .map_err(DeriverError::l2_provider)?;
        let number = state
            .storage(HISTORY_ADDRESS, B256::from(U256::from(ENDPOINT_SLOT)))
            .map_err(DeriverError::l2_provider)?
            .unwrap_or_default();
        let number = u64::try_from(number).map_err(DeriverError::l2_provider)?;
        let selected = chain.at(number).map_err(DeriverError::l2_provider)?;
        let hash = state
            .storage(HISTORY_ADDRESS, hash_storage_key(number).into())
            .map_err(DeriverError::l2_provider)?
            .unwrap_or_default();
        Ok(B256::from(hash) == selected.hash() && block.mix_hash == selected.header().mix_hash)
    }

    /// Current cursor — highest L2 block confirmed by any L1-landed
    /// batch. Reads through the shared [`L1CanonicalHead`].
    #[must_use]
    pub fn cursor(&self) -> u64 {
        self.inner.l1_head.cursor()
    }

    /// Reorg-aware catch-up from the latest canonical L1 batch already
    /// indexed locally, or from the registry deploy block if the index is
    /// empty. Scans historical `BatchPosted` events in chunks, replaying
    /// non-matching L2 blocks and populating `L1CanonicalHead`.
    ///
    /// # Errors
    ///
    /// `l1_scan` (scan failure), `l2_provider` (lookup failure),
    /// `local_diverged` (replay failure), `committer_closed`.
    ///
    /// # Panics
    ///
    /// If the `batches` mutex is poisoned.
    pub async fn catch_up(&self) -> DeriverResult<()> {
        self.catch_up_inner().await.map(|_| ())
    }

    /// [`Self::catch_up`], additionally returning the `L1Watcher::polling`
    /// seed: the finalized block, kept inside the range this scan read so
    /// the seed is immutable and always servable. Boot-only: discard any
    /// speculative local suffix whose in-memory ownership was lost on restart.
    ///
    /// # Errors
    ///
    /// As [`Self::catch_up`]; additionally `SourceIncomplete` while the
    /// L1 source cannot yet serve the seed block's hash.
    pub async fn catch_up_with_seed(&self) -> DeriverResult<(u64, B256)> {
        self.catch_up_with_seed_inner(false).await
    }

    /// Boot catch-up for a composer that may intentionally retain ordinary L2
    /// history between settlements. Preserve the canonical pure prefix above
    /// the validated anchor; an unowned cross-chain Sync and its descendants
    /// still require the existing boot rollback. Safe/finalized remain L1-derived.
    ///
    /// # Errors
    ///
    /// As [`Self::catch_up_with_seed`]; additionally unavailable or inconsistent
    /// local ancestry and malformed pending transactions prevent startup.
    pub async fn catch_up_with_seed_retaining_pure_l2(&self) -> DeriverResult<(u64, B256)> {
        self.catch_up_with_seed_inner(true).await
    }

    async fn catch_up_with_seed_inner(&self, retain_pure_l2: bool) -> DeriverResult<(u64, B256)> {
        let end = self.catch_up_inner().await?;
        // NOT canonicality-probed: at boot this tail is a batch THIS scan just
        // found, with a hash straight from `get_logs`. Cross-checked below.
        let indexed_tail = self.inner.l1_head.last_indexed();
        let floor = indexed_tail
            .as_ref()
            .map_or_else(|| self.inner.deploy_block.saturating_sub(1), |t| t.l1_block);
        let finalized = self
            .inner
            .l1_reader
            .finalized_block()
            .await
            .map_err(DeriverError::l1_scan)?;
        // No finality yet (a chain younger than two epochs) → the floor, which
        // this scan read and the watcher's ancestor backfill makes reorg-safe.
        let seed = choose_seed(floor, end, finalized.map(|(n, _)| n));
        let canonical = self
            .inner
            .l1_reader
            .canonical_l1_hash(seed)
            .await
            .map_err(DeriverError::l1_scan)?
            .ok_or_else(|| {
                // Not served yet, or rewound mid-scan — retryable, not fatal.
                DeriverError::l1_scan(eez_l1::L1Error::SourceIncomplete {
                    block: seed,
                    tx_hash: B256::ZERO,
                    detail: "catch-up seed block not served by the L1 source yet".into(),
                })
            })?;
        // A batch this scan indexed at the seed height sitting on another fork
        // means the chain moved under us: retry so `revalidate_index_tail` drops
        // it. (A finalized seed needs no such check — it cannot reorg.)
        if indexed_tail.is_some_and(|t| t.l1_block == seed && t.l1_block_hash != canonical) {
            return Err(DeriverError::l1_scan(eez_l1::L1Error::SourceIncomplete {
                block: seed,
                tx_hash: B256::ZERO,
                detail: "catch-up seed block reorged during the scan; retry".into(),
            }));
        }
        // No attempt ledger survives restart. Keeping an unposted cross-chain
        // Sync would leave a system-transaction barrier no attempt can recover.
        // A delayed-settlement composer must nevertheless retain its ordinary
        // history: losing volatile policy intent does not discard included txs.
        let _guard = self.inner.committer.begin_reconcile().await?;
        let cursor = self.inner.l1_head.cursor();
        let local_head = self
            .inner
            .l2_provider
            .best_block_number()
            .map_err(DeriverError::l2_provider)?;
        let retained = if retain_pure_l2 {
            self.pure_l2_boot_prefix(cursor, local_head)?
        } else {
            self.l2_sealed_header_at(cursor)?
        };
        if local_head > retained.number() {
            // catch_up_inner already reconciled safe/finalized. Only the
            // speculative head moves; retained pure blocks are not settlement.
            if retain_pure_l2 {
                self.inner.committer.reorg_to(retained.clone()).await?;
            } else {
                self.retreat_l2_to_cursor(cursor).await?;
            }
        }
        event!(name: "eez.deriver.catch_up.local_history", Level::INFO,
            event_name = "eez.deriver.catch_up.local_history",
            retain_pure_l2, settled_l2 = cursor, previous_head = local_head,
            retained_head = retained.number(), retained_hash = %retained.hash(),
            "reconciled pending local history at startup");
        event!(
            name: "eez.deriver.catch_up.seed",
            Level::INFO,
            floor,
            scan_end = end,
            finalized = ?finalized.map(|(n, _)| n),
            seed,
            seed_hash = %canonical,
            "watcher seed chosen",
        );
        Ok((seed, canonical))
    }

    fn pure_l2_boot_prefix(
        &self,
        cursor: u64,
        local_head: u64,
    ) -> DeriverResult<SealedHeader<alloy_consensus::Header>> {
        let mut retained = self.l2_sealed_header_at(cursor)?;
        self.inner.system_tx_cfg.as_ref().ok_or_else(|| {
            DeriverError::l2_provider(
                "retaining pure L2 history requires native reconstruction configuration",
            )
        })?;
        let system = eez_primitives::SYSTEM_ADDRESS;
        while retained.number() < local_head {
            let number = retained
                .number()
                .checked_add(1)
                .ok_or_else(|| DeriverError::l2_provider("pending L2 height overflow"))?;
            let block = self
                .inner
                .l2_provider
                .block_by_number(number)
                .map_err(DeriverError::l2_provider)?
                .ok_or_else(|| {
                    DeriverError::l2_provider(format!("pending L2 block {number} missing"))
                })?;
            if block.header().number != number || block.header().parent_hash != retained.hash() {
                return Err(DeriverError::l2_provider(
                    "pending L2 ancestry changed during boot",
                ));
            }
            self.inner
                .derivation
                .schedule()
                .validate_position(block.header())
                .map_err(DeriverError::l2_provider)?;
            if !self.history_matches(&SealedHeader::seal_slow(block.header().clone()))? {
                return Ok(retained);
            }
            for tx in block.body().transactions() {
                let encoded = tx.encoded_2718();
                let tx = TransactionSigned::decode_2718(&mut encoded.as_slice())
                    .map_err(DeriverError::l2_provider)?;
                if tx.recover_signer().map_err(DeriverError::l2_provider)? == system
                    && !tx.is_history()
                {
                    return Ok(retained);
                }
            }
            retained = SealedHeader::seal_slow(block.header().clone());
        }
        Ok(retained)
    }

    /// Shared body of [`Self::catch_up`] / [`Self::catch_up_with_seed`].
    /// Returns the inclusive L1 block covered: the scan's endpoint, or the
    /// tip when the range was empty (nothing below it can hold our events).
    async fn catch_up_inner(&self) -> DeriverResult<u64> {
        self.inner.l1_head.invalidate_scan();
        let old_cursor = self.inner.l1_head.cursor();
        let anchor = self.revalidate_index_tail().await?;
        let cursor = self.inner.l1_head.cursor();
        let accepted = self
            .inner
            .correlations
            .by_l2_number(cursor)
            .map_err(DeriverError::l2_provider)?;
        if cursor < old_cursor
            || accepted.as_ref().is_some_and(|accepted| {
                self.l2_hash_at(cursor).ok() != Some(accepted.l2_block.hash)
            })
        {
            let _guard = self.inner.committer.begin_reconcile().await?;
            self.retreat_l2_to_cursor(cursor).await?;
        }
        match anchor {
            Some(anchor_l1_block) => self.sync_batches_inner(anchor_l1_block, cursor).await,
            None => match self.correlation_seed().await? {
                Some((l1_block, l2_cursor)) => self.sync_batches_inner(l1_block, l2_cursor).await,
                None => self.sync_batches_inner(self.inner.deploy_block, 0).await,
            },
        }
    }

    /// Phase 1 of [`Self::catch_up`]: walk the index tail backward,
    /// dropping batches whose recorded L1 hash is no longer canonical.
    /// Returns the highest still-canonical batch's L1 block (the rescan
    /// lower bound), or `None` if the index is empty. Caller holds the
    /// reconcile lock.
    async fn revalidate_index_tail(&self) -> DeriverResult<Option<u64>> {
        while let Some(tail) = self.inner.l1_head.last_indexed() {
            let canonical = self
                .inner
                .l1_reader
                .canonical_l1_hash(tail.l1_block)
                .await
                .map_err(DeriverError::l1_scan)?;
            // `None` is not proof of a reorg — retreating would unwind L2 to
            // genesis, so retry. A head ABOVE this block means pruned/rewound,
            // which retrying won't fix; report it so that is diagnosable.
            let Some(canonical) = canonical else {
                let head = self.inner.l1_reader.head_block_number().await.ok();
                return Err(DeriverError::l1_scan(eez_l1::L1Error::SourceIncomplete {
                    block: tail.l1_block,
                    tx_hash: tail.tx_hash,
                    detail: format!(
                        "indexed batch's L1 block not served; cannot judge canonicality \
                         (source head: {head:?} — above this block means pruned or rewound, \
                         not lagging)"
                    ),
                }));
            };
            if canonical == tail.l1_block_hash {
                return Ok(Some(tail.l1_block));
            }
            let _guard = self.inner.committer.begin_reconcile().await?;
            let old_cursor = self.inner.l1_head.cursor();
            let (new_cursor, _new_finalized, dropped) = self
                .inner
                .l1_head
                .retreat_on_l1_reorg(tail.l1_block.saturating_sub(1));
            let removed_correlations = self
                .inner
                .correlations
                .remove_after_l1(tail.l1_block.saturating_sub(1))
                .map_err(DeriverError::l2_provider)?;
            event!(
                name: "eez.deriver.l1.reorg.retreated",
                Level::WARN,
                event_name = "eez.deriver.l1.reorg.retreated",
                l1_block = tail.l1_block,
                indexed_hash = %tail.l1_block_hash,
                canonical_hash = %canonical,
                old_cursor,
                new_cursor,
                dropped_batches = dropped,
                removed_correlations,
                "L1 reorg rolled out confirmed batches; L2 safe cursor retreated",
            );
        }
        Ok(None)
    }

    /// Scan `BatchPosted` from `from_l1_block`, reconciling and committing
    /// each successful L1 chunk before fetching the next. If a later chunk
    /// reports an incomplete source, the next catch-up retry can resume from
    /// the latest canonical batch already indexed in [`L1CanonicalHead`].
    /// Returns the inclusive L1 block the scan covered through.
    async fn sync_batches_inner(
        &self,
        from_l1_block: u64,
        cumulative_start: u64,
    ) -> DeriverResult<u64> {
        let local_head = self
            .inner
            .l2_provider
            .best_block_number()
            .map_err(DeriverError::l2_provider)?;
        let mut chunks = self
            .inner
            .l1_reader
            .batch_log_chunks(from_l1_block)
            .await
            .map_err(DeriverError::l1_scan)?;
        let to_l1_block = chunks.to_block();
        event!(
            name: "eez.deriver.catch_up.start",
            Level::INFO,
            local_head,
            from_l1_block,
            to_l1_block,
            cumulative_start,
            "starting batch scan to populate L1CanonicalHead and reconcile L2 chain",
        );

        if chunks.is_empty() {
            event!(
                name: "eez.deriver.catch_up.noop",
                Level::DEBUG,
                cursor = cumulative_start,
                "scan completed without replaying any blocks",
            );
            self.finish_scan(to_l1_block, None).await?;
            return Ok(to_l1_block);
        }

        let mut cumulative_l2 = cumulative_start;
        let mut total_replayed: u64 = 0;
        while let Some(scanned_batches) = self
            .inner
            .l1_reader
            .next_batch_log_chunk(&mut chunks)
            .await
            .map_err(DeriverError::l1_scan)?
        {
            total_replayed += self
                .reconcile_scanned_batches(&scanned_batches, &mut cumulative_l2)
                .await?;
        }

        if total_replayed > 0 {
            event!(
                name: "eez.deriver.catch_up.done",
                Level::INFO,
                local_head,
                replayed = total_replayed,
                cursor = cumulative_l2,
                "catch-up replay complete",
            );
        } else {
            event!(
                name: "eez.deriver.catch_up.noop",
                Level::DEBUG,
                cursor = cumulative_l2,
                "scan completed without replaying any blocks",
            );
        }
        self.finish_scan(to_l1_block, None).await?;
        Ok(to_l1_block)
    }

    /// Publish completion only after checking the scan's L1 identities and
    /// canonical L2 endpoint. The caller holds the engine reconciliation lock.
    async fn finish_scan(&self, number: u64, expected_hash: Option<B256>) -> DeriverResult<()> {
        let generation = self.inner.l1_head.policy_anchor_snapshot().generation;
        // A successful resync can run ahead of queued live notifications.
        // Never pair an older L1 snapshot with the newer canonical L2 state.
        let (number, expected_hash) = self
            .inner
            .l1_head
            .last_scanned_block()
            .filter(|previous| previous.number > number)
            .map_or((number, expected_hash), |previous| {
                (previous.number, Some(previous.hash))
            });
        let hash = scan_completion_hash(
            number,
            self.inner
                .l1_reader
                .canonical_l1_hash(number)
                .await
                .map_err(DeriverError::l1_scan)?,
        )?;
        if expected_hash.is_some_and(|expected| expected != hash) {
            return Err(DeriverError::l2_provider(
                "L1 changed before scan completion",
            ));
        }
        for block in self.inner.l1_head.pending_l1_blocks() {
            if block.number > number
                || self
                    .inner
                    .l1_reader
                    .canonical_l1_hash(block.number)
                    .await
                    .map_err(DeriverError::l1_scan)?
                    != Some(block.hash)
            {
                return Err(DeriverError::l2_provider(
                    "batch scan straddled an L1 reorg",
                ));
            }
        }
        if self
            .inner
            .l1_reader
            .canonical_l1_hash(number)
            .await
            .map_err(DeriverError::l1_scan)?
            != Some(hash)
        {
            return Err(DeriverError::l2_provider(
                "L1 changed while completing scan",
            ));
        }
        let cursor = self.inner.l1_head.cursor();
        let parent = self.l2_sealed_header_at(cursor)?;
        self.prepare_history(&parent, eez_l1::L1BlockRef { number, hash })
            .await?;
        let _guard = self.inner.committer.begin_reconcile().await?;
        if !self.inner.l1_head.generation_is(generation) || self.inner.l1_head.cursor() != cursor {
            return Err(DeriverError::l2_provider(
                "scan state changed during L1 acquisition",
            ));
        }
        self.discard_stale_history_suffix(parent).await?;
        self.sync_safe_to_cursor().await?;
        self.inner.l1_head.mark_scanned(
            eez_l1::L1BlockRef { number, hash },
            eez_l1::L2Endpoint {
                number: cursor,
                hash: self.l2_hash_at(cursor)?,
            },
        );
        self.inner.scan_needs_resync.store(false, Ordering::Release);
        Ok(())
    }

    fn batch_evidence(
        &self,
        l1_block: eez_l1::L1BlockRef,
        transaction: (B256, u64),
        calldata: &Bytes,
        semantic_transactions: &[eez_protocol::blob_semantics::SemanticTransaction],
        settlement: eez_l1::Settlement,
        endpoint: Option<eez_l1::L2Endpoint>,
    ) -> DeriverResult<eez_l1::ReconciledBatchSettlement> {
        let (skip, take) = settlement.producing_slice();
        let end = skip
            .checked_add(take)
            .ok_or_else(|| DeriverError::l2_provider("settlement effect range overflow"))?;
        let selection = if !settlement.is_empty()
            && let Some(cfg) = self.inner.system_tx_cfg.as_ref()
        {
            use alloy_sol_types::SolCall as _;
            let posted = eez_protocol::abi::postAndVerifyBatchCall::abi_decode(calldata)
                .map_err(DeriverError::l2_provider)?;
            let immediate = usize::try_from(posted.batch.immediateEntryCount)
                .map_err(DeriverError::l2_provider)?;
            if immediate == 0 || immediate > posted.batch.entries.len() {
                return Err(DeriverError::l2_provider(
                    "invalid immediate entry count in evidence",
                ));
            }
            let counts = eez_protocol::system_tx::outbound_settlement_root_counts(
                &posted.batch.entries[1..immediate],
            )
            .map_err(DeriverError::l2_provider)?;
            let actions = eez_protocol::blob_semantics::ReconstructionActions::from_semantics(
                semantic_transactions,
                eez_protocol::RollupId(cfg.this_rollup_id),
                cfg.l2_chain_id,
            )
            .map_err(DeriverError::l2_provider)?;
            actions
                .bind_settlement(&posted.batch)
                .map_err(DeriverError::l2_provider)?;
            let groups = actions
                .outbound()
                .iter()
                .map(|group| group.effects.len())
                .collect::<Vec<_>>();
            let inbound_count = posted.batch.entries[immediate..]
                .iter()
                .filter(|entry| {
                    !eez_protocol::settlement::is_lookup_only_inbound_entry(
                        entry,
                        cfg.this_rollup_id,
                    )
                })
                .count();
            selected_replay_groups(settlement, &counts, &groups, inbound_count)
                .map_err(DeriverError::l2_provider)?
        } else {
            eez_l1::ReplaySelection::default()
        };
        Ok(eez_l1::ReconciledBatchSettlement {
            l1_block,
            tx_hash: transaction.0,
            tx_index: transaction.1,
            calldata_hash: alloy_primitives::keccak256(calldata),
            applied_effects: skip..end,
            anchor_applied: !settlement.is_empty() && settlement.start == 0,
            endpoint,
            selection,
        })
    }

    async fn reconcile_scanned_batches(
        &self,
        scanned_batches: &[ScannedBatch],
        cumulative_l2: &mut u64,
    ) -> DeriverResult<u64> {
        let known_tx_hashes = self.inner.l1_head.known_tx_hashes();
        let mut total_replayed: u64 = 0;
        for batch in scanned_batches {
            let payload = Payload::decode(batch.operations_payload.as_ref())?;

            // `settled_count == 0` = nothing applied on L1 (the claimed
            // roots are phantoms). Skip the whole reconcile — no
            // cursor advance, no replay, no state check; the composer's
            // next slot re-attempts over the same range.
            if batch.settlement.is_empty() {
                let evidence = self.batch_evidence(
                    eez_l1::L1BlockRef {
                        number: batch.l1_block_number,
                        hash: batch.l1_block_hash,
                    },
                    (batch.tx_hash, batch.tx_index),
                    &batch.post_batch_input,
                    &batch.semantic_transactions,
                    batch.settlement,
                    None,
                )?;
                self.inner.l1_head.publish(evidence);
                event!(
                    name: "eez.deriver.catch_up.batch.unsettled",
                    Level::DEBUG,
                    l1_block_number = batch.l1_block_number,
                    tx_hash = %batch.tx_hash,
                    "catch_up: postBatch's L1 block has no L2ExecutionPerformed for our rollup; skipping (re-attempt expected)",
                );
                continue;
            }

            // Already indexed — processed by an earlier sync; its L2
            // range is accounted for in `cumulative_start`.
            let prior = self
                .inner
                .correlations
                .by_l1_number(batch.l1_block_number)
                .map_err(DeriverError::l2_provider)?
                .into_iter()
                .find(|record| {
                    record.l1_transaction_hash == batch.tx_hash
                        && record.l1_block_hash == batch.l1_block_hash
                });
            if let Some(prior) = prior.filter(|prior| prior.last_l2().number <= *cumulative_l2) {
                let endpoint = prior.last_l2();
                if batch.settlement.final_state != Some(endpoint.hash) {
                    return Err(DeriverError::l2_provider(
                        "persisted batch endpoint disagrees with canonical settlement",
                    ));
                }
                let evidence = self.batch_evidence(
                    eez_l1::L1BlockRef {
                        number: batch.l1_block_number,
                        hash: batch.l1_block_hash,
                    },
                    (batch.tx_hash, batch.tx_index),
                    &batch.post_batch_input,
                    &batch.semantic_transactions,
                    batch.settlement,
                    Some(eez_l1::L2Endpoint {
                        number: endpoint.number,
                        hash: endpoint.hash,
                    }),
                )?;
                let l1 = self
                    .inner
                    .derivation
                    .canonical_header(evidence.l1_block)
                    .await
                    .map_err(DeriverError::l2_provider)?;
                // The recorded origin came from this exact executed checkpoint.
                // Its state may have been replaced by a resumed same-height batch;
                // rebuild and authenticate L1 ancestry without reopening that state.
                self.prepare_history_from_endpoint(
                    prior.endpoint_history.endpoint,
                    evidence.l1_block,
                )
                .await?;
                let _guard = self.inner.committer.begin_reconcile().await?;
                self.inner
                    .l1_head
                    .publish_accepted(
                        evidence,
                        &l1,
                        &prior.endpoint_header,
                        prior.endpoint_history,
                    )
                    .map_err(DeriverError::l2_provider)?;
                continue;
            }
            if known_tx_hashes.contains(&batch.tx_hash) {
                continue;
            }

            let resumed = batch.settlement.start > 0;
            // Anchor on the batch's own start, not our cursor: a competing
            // composer's batch can cover a range that begins below it.
            let anchor = match batch.settlement.entry_state {
                Some(entry_state) => self
                    .batch_anchor(
                        *cumulative_l2,
                        entry_state,
                        u64::from(payload.block_count()),
                    )?
                    .unwrap_or(*cumulative_l2),
                None => *cumulative_l2,
            };
            let (batch_first_l2, batch_last_l2) =
                batch_l2_range(anchor, resumed, u64::from(payload.block_count()));

            // Cursor guard (as in on_batch_posted): a root this batch cannot leave at
            // the cursor means the scan is off L1, and replaying would fork us alone.
            if let Some(entry_state) = batch.settlement.entry_state {
                let local_root = self.l2_hash_at(anchor)?;
                if !cursor_root_accepted(local_root, entry_state, batch.settlement.final_state) {
                    event!(
                        name: "eez.deriver.catch_up.cursor.misaligned",
                        Level::ERROR,
                        l1_block_number = batch.l1_block_number,
                        tx_hash = %batch.tx_hash,
                        cumulative_l2 = *cumulative_l2,
                        anchor,
                        local_root = %local_root,
                        entry_state = %entry_state,
                        final_state = ?batch.settlement.final_state,
                        applied_start = batch.settlement.start,
                        "local block hash at the scan cursor is neither the commitment the batch's applied run started from nor its settled endpoint; refusing to replay",
                    );
                    return Err(DeriverError::local_diverged(batch_first_l2));
                }
            }

            let (replayed_here, scan_sync_hash, _reconcile_guard, authenticated_l1) = self
                .reconcile_batch_blocks(
                    batch_first_l2,
                    &payload,
                    batch.post_batch_input.clone(),
                    &batch.semantic_transactions,
                    eez_l1::L1BlockRef {
                        number: batch.l1_block_number,
                        hash: batch.l1_block_hash,
                    },
                    batch.tx_hash,
                    batch.settlement,
                )
                .await?;
            total_replayed += replayed_here;

            // Catch drift now, not at a live event. Both ends must be what L1
            // ACTUALLY ran, not the claimed endpoints. Pre-check skipped when
            // resumed: the cursor-alignment guard above already checked it, at
            // `cumulative_l2` — `batch_first_l2 - 1` would check the wrong
            // height now that a resumed batch doesn't advance past it.
            let settled_end = self.check_claimed_state(
                if resumed {
                    None
                } else {
                    batch.settlement.entry_state.or(batch.claimed_current_root)
                },
                batch.settlement.final_state.or(batch.claimed_new_root),
                anchor,
                batch_first_l2,
                batch_last_l2,
                scan_sync_hash,
                batch.l1_block_number,
                batch.tx_hash,
            )?;
            // Follow L1's real endpoint. A partial settlement stops early, and
            // the composer must not anchor past L1's stored root.
            let end = settled_end.unwrap_or(batch_last_l2);
            let correlation = self.correlation_record(
                batch.l1_block_number,
                batch.l1_block_hash,
                batch.tx_hash,
                batch.tx_index,
                batch_first_l2,
                end,
                (end == batch_last_l2).then_some(scan_sync_hash).flatten(),
            )?;
            let evidence = self.batch_evidence(
                eez_l1::L1BlockRef {
                    number: batch.l1_block_number,
                    hash: batch.l1_block_hash,
                },
                (batch.tx_hash, batch.tx_index),
                &batch.post_batch_input,
                &batch.semantic_transactions,
                batch.settlement,
                Some(eez_l1::L2Endpoint {
                    number: end,
                    hash: correlation.last_l2().hash,
                }),
            )?;
            self.publish_correlation(correlation, evidence, &authenticated_l1)?;
            *cumulative_l2 = end;
        }

        // Index every batch we walked (de-duped against startup
        // entries) so subsequent live `BatchPosted` events for any
        // of them are skipped as already-processed.

        // Advance safe once per chunk, not per batch: two batches in one L1 block
        // rewrite the same L2 height, so a per-batch hash gets orphaned.
        let _guard = self.inner.committer.begin_reconcile().await?;
        let old_safe_l2 = self.inner.safe_l2_block.load(Ordering::Acquire);
        if *cumulative_l2 > old_safe_l2 {
            let safe_header = self.l2_sealed_header_at(*cumulative_l2)?;
            let finalized_hash = self.l2_hash_at(self.inner.l1_head.finalized_l2())?;
            self.inner
                .committer
                .advance_safe_finalized(safe_header, finalized_hash)
                .await?;
            self.inner
                .safe_l2_block
                .store(*cumulative_l2, Ordering::Release);
        }

        Ok(total_replayed)
    }

    /// STF-replay one V0 block on top of `parent_block_number` using only its
    /// published execution inputs and the authenticated local parent.
    fn execute_block(
        &self,
        parent_block_number: u64,
        raw_txs: &[Bytes],
        inputs: ExecutionInputs<'_>,
    ) -> DeriverResult<(ExecutionData, SealedHeader<alloy_consensus::Header>)> {
        // Diagnostic: log parent context before touching reth so we can
        // pinpoint failing `state_by_block_hash` lookups.
        let local_best = self
            .inner
            .l2_provider
            .best_block_number()
            .map_err(DeriverError::l2_provider)?;
        event!(
            name: "eez.deriver.execute_block.start",
            Level::DEBUG,
            parent_block_number,
            local_best,
            tx_count = raw_txs.len(),
            "execute_block: looking up parent header",
        );

        let parent_header = self
            .inner
            .l2_provider
            .sealed_header(parent_block_number)
            .map_err(|e| {
                event!(
                    name: "eez.deriver.execute_block.parent_lookup_failed",
                    Level::ERROR,
                    parent_block_number,
                    local_best,
                    error = %e,
                    "sealed_header() failed",
                );
                DeriverError::l2_provider(e)
            })?
            .ok_or_else(|| {
                event!(
                    name: "eez.deriver.execute_block.parent_missing",
                    Level::ERROR,
                    parent_block_number,
                    local_best,
                    "sealed_header() returned None — parent header is not in canonical chain",
                );
                DeriverError::l2_provider(format!(
                    "local L2 header at parent block {parent_block_number} missing"
                ))
            })?;

        self.execute_block_on_parent(&parent_header, raw_txs, inputs)
    }

    /// Execute against the exact retained header during suffix repair, without
    /// resolving its number again through a possibly lagging canonical index.
    fn execute_block_on_parent(
        &self,
        parent_header: &SealedHeader<alloy_consensus::Header>,
        raw_txs: &[Bytes],
        inputs: ExecutionInputs<'_>,
    ) -> DeriverResult<(ExecutionData, SealedHeader<alloy_consensus::Header>)> {
        self.execute_block_with_plans(parent_header, raw_txs, &[], &[], inputs)
            .map(|(payload, header, _)| (payload, header))
    }

    /// Replay the exact retained prefix, then encode construction plans using
    /// the gas actually charged by every preceding transaction.
    fn execute_block_with_plans(
        &self,
        parent_header: &SealedHeader<alloy_consensus::Header>,
        raw_txs: &[Bytes],
        plans: &[eez_protocol::system_tx::PlannedTransaction],
        sources: &[eez_protocol::native_source::SourceEntry],
        inputs: ExecutionInputs<'_>,
    ) -> DeriverResult<(
        ExecutionData,
        SealedHeader<alloy_consensus::Header>,
        Vec<Bytes>,
    )> {
        let parent_block_number = parent_header.number;
        let parent_hash = parent_header.hash();
        let state_provider = self
            .inner
            .l2_provider
            .state_by_block_hash(parent_hash)
            .map_err(|e| {
                // reth has the *header* for this block (sealed_header
                // succeeded above) but refuses to give us its state.
                // Capture as much context as possible.
                event!(
                    name: "eez.deriver.execute_block.no_state",
                    Level::ERROR,
                    parent_block_number,
                    parent_hash = %parent_hash,
                    parent_timestamp = parent_header.timestamp(),
                    error = %e,
                    "state_by_block_hash() failed for a header that sealed_header() returned successfully — likely reth state retention timing under reorg churn",
                );
                DeriverError::l2_provider(e)
            })?;
        let state_db = StateProviderDatabase::new(EvmStateProviderAdapter(state_provider.as_ref()));
        let mut db = State::builder()
            .with_database(state_db)
            .with_bundle_update()
            .build();

        let attributes = inputs.attributes(&self.inner.evm_config, parent_header, raw_txs)?;
        let block_gas_limit = attributes.gas_limit;
        if plans
            .iter()
            .filter(|plan| matches!(plan, eez_protocol::system_tx::PlannedTransaction::Native(_)))
            .count()
            != sources.len()
        {
            return Err(DeriverError::l2_provider(
                "native source group count differs from plans",
            ));
        }
        let mut sources = sources.iter();

        let mut builder = self
            .inner
            .evm_config
            .builder_for_next_block(&mut db, parent_header, attributes)
            .map_err(|e| {
                DeriverError::l2_provider(format!("builder_for_next_block failed: {e}"))
            })?;

        builder
            .apply_pre_execution_changes()
            .map_err(|e| DeriverError::l2_provider(format!("pre-execution changes failed: {e}")))?;

        let mut gas_used = 0u64;
        let mut suffix = Vec::with_capacity(plans.len());
        for tx_idx in 0..raw_txs.len() + plans.len() {
            let tx_bytes = if let Some(raw) = raw_txs.get(tx_idx) {
                raw.clone()
            } else {
                let plan = &plans[tx_idx - raw_txs.len()];
                let source_hash =
                    if matches!(plan, eez_protocol::system_tx::PlannedTransaction::Native(_)) {
                        builder
                            .executor_mut()
                            .validate_prefix()
                            .map_err(DeriverError::l2_provider)?;
                        sources
                            .next()
                            .ok_or_else(|| {
                                DeriverError::l2_provider("missing native source group")
                            })?
                            .hash()
                    } else {
                        B256::ZERO
                    };
                let raw = plan
                    .at_position(block_gas_limit, gas_used, source_hash)
                    .map_err(DeriverError::l2_provider)?;
                suffix.push(raw.clone());
                raw
            };
            let tx = TransactionSigned::decode_2718_exact(&tx_bytes).map_err(|e| {
                DeriverError::local_diverged_with_msg(
                    parent_block_number + 1,
                    &format!("decode tx #{tx_idx}: {e}"),
                )
            })?;
            let recovered = SignedTransaction::try_into_recovered(tx).map_err(|_| {
                DeriverError::local_diverged_with_msg(
                    parent_block_number + 1,
                    &format!("could not recover signer for tx #{tx_idx}"),
                )
            })?;
            let gas = builder.execute_transaction(recovered).map_err(|e| {
                DeriverError::local_diverged_with_msg(
                    parent_block_number + 1,
                    &format!("execute tx #{tx_idx}: {e}"),
                )
            })?;
            gas_used = gas_used
                .checked_add(gas.tx_gas_used())
                .ok_or_else(|| DeriverError::l2_provider("reconstruction gas overflow"))?;
        }

        let outcome = builder
            .finish(state_provider.as_ref(), None)
            .map_err(|e| DeriverError::l2_provider(format!("block builder finish failed: {e}")))?;

        let sealed_block = outcome.block.sealed_block().clone();
        let sealed_header = sealed_block.sealed_header().clone();
        let execution_data = <EezEngineTypes as PayloadTypes>::block_to_payload(sealed_block, None);
        Ok((execution_data, sealed_header, suffix))
    }

    async fn replay_block(
        &self,
        parent_block_number: u64,
        raw_txs: &[Bytes],
        inputs: ExecutionInputs<'_>,
    ) -> DeriverResult<DeriveOutcome> {
        let (payload, header) = self.execute_block(parent_block_number, raw_txs, inputs)?;
        // feed_witness=false: follower / L1-reconcile re-derive — the producer
        // already fed this block to the prover witness capture; don't double-feed.
        Ok(self
            .inner
            .committer
            .commit_derived(payload, header, false)
            .await?)
    }

    /// Runs the deriver loop, processing each event on `rx` until the
    /// stream closes. `rx` must be subscribed before the `L1Watcher`
    /// starts so no event predates it.
    pub async fn run(self, mut rx: broadcast::Receiver<L1Event>) {
        // Defensive re-anchor: a cheap no-op in the normal boot order;
        // load-bearing for any caller that subscribed rx late.
        if let Err(err) = self.catch_up().await {
            event!(
                name: "eez.deriver.resync.failed",
                Level::ERROR,
                event_name = "eez.deriver.resync.failed",
                error = %err,
                "post-subscribe resync failed; deriver may have a gap",
            );
        }

        event!(
            name: "eez.deriver.started",
            Level::INFO,
            cursor = self.cursor(),
            "deriver loop started",
        );
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Err(err) = self.handle_event(event).await {
                        self.inner.scan_needs_resync.store(true, Ordering::Release);
                        self.inner.l1_head.invalidate_scan();
                        if err.is_committer_closed() {
                            event!(
                                name: "eez.deriver.committer.closed",
                                Level::ERROR,
                                event_name = "eez.deriver.committer.closed",
                                error = %err,
                                "block committer gone; deriver exiting",
                            );
                            return;
                        }
                        // A dropped event leaves `last_indexed_l2` behind
                        // L1, so later batches replay at the wrong heights —
                        // re-anchor from L1 first.
                        event!(
                            name: "eez.deriver.event.failed",
                            Level::WARN,
                            error = %err,
                            "deriver failed to handle event; resyncing from L1",
                        );
                        if !self.try_recover().await {
                            return;
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    self.inner.scan_needs_resync.store(true, Ordering::Release);
                    self.inner.l1_head.invalidate_scan();
                    event!(
                        name: "eez.deriver.l1_events.lagged",
                        Level::WARN,
                        skipped,
                        "L1 event stream lagged; resyncing from L1",
                    );
                    if !self.try_recover().await {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => {
                    event!(
                        name: "eez.deriver.l1_events.closed",
                        Level::ERROR,
                        "L1 event stream closed; deriver exiting",
                    );
                    return;
                }
            }
        }
    }

    /// Post-failure recovery via [`Self::catch_up`]. Returns `false` only
    /// when the committer is gone and the loop must exit; a failed resync is
    /// logged and retried at the next L1 event.
    async fn try_recover(&self) -> bool {
        match self.catch_up().await {
            Ok(()) => {
                event!(
                    name: "eez.deriver.resync.recovered",
                    Level::INFO,
                    cursor = self.cursor(),
                    "resync complete; cursor re-anchored to L1",
                );
                true
            }
            Err(err) if err.is_committer_closed() => {
                event!(
                    name: "eez.deriver.committer.closed",
                    Level::ERROR,
                    event_name = "eez.deriver.committer.closed",
                    error = %err,
                    "block committer gone; deriver exiting",
                );
                false
            }
            Err(err) => {
                event!(
                    name: "eez.deriver.resync.failed",
                    Level::ERROR,
                    event_name = "eez.deriver.resync.failed",
                    error = %err,
                    "resync failed; will retry after the next L1 event",
                );
                true
            }
        }
    }

    async fn handle_event(&self, event: L1Event) -> DeriverResult<()> {
        match event {
            L1Event::BatchPosted {
                l1_block_number,
                l1_block_hash,
                tx_hash,
                tx_index,
                submitter,
                operations_payload,
                semantic_transactions,
                post_batch_input,
                state_applied,
                settlement,
                claimed_current_root,
                claimed_new_root,
                last_in_l1_block,
            } => {
                self.on_batch_posted(
                    l1_block_number,
                    l1_block_hash,
                    tx_hash,
                    tx_index,
                    submitter,
                    operations_payload,
                    semantic_transactions,
                    post_batch_input,
                    state_applied,
                    settlement,
                    claimed_current_root,
                    claimed_new_root,
                    last_in_l1_block,
                )
                .await
            }
            L1Event::NewHead { .. } => Ok(()),
            L1Event::ScanComplete {
                block_number,
                block_hash,
            } => {
                if self.inner.scan_needs_resync.load(Ordering::Acquire) {
                    return self.catch_up().await;
                }
                self.finish_scan(block_number, Some(block_hash)).await
            }
            L1Event::Reorg {
                common_ancestor_number,
                old_head_hash,
                new_head_hash,
                new_head_number,
                ..
            } => {
                self.on_l1_reorg(
                    common_ancestor_number,
                    old_head_hash,
                    new_head_number,
                    new_head_hash,
                )
                .await
            }
            L1Event::Finalized { block_number, .. } => self.on_l1_finalized(block_number).await,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn on_batch_posted(
        &self,
        l1_block_number: u64,
        l1_block_hash: B256,
        tx_hash: B256,
        tx_index: u64,
        submitter: Address,
        operations_payload: Bytes,
        semantic_transactions: Arc<[eez_protocol::blob_semantics::SemanticTransaction]>,
        post_batch_input: Bytes,
        state_applied: bool,
        settlement: eez_l1::Settlement,
        claimed_current_root: Option<B256>,
        claimed_new_root: Option<B256>,
        last_in_l1_block: bool,
    ) -> DeriverResult<()> {
        let payload = Payload::decode(operations_payload.as_ref())?;
        let block_count = u64::from(payload.block_count());

        // Dedup by tx_hash — already-indexed = already processed by
        // catch_up; this is a stale live event.
        if self.inner.l1_head.contains_l1_tx(&tx_hash) {
            event!(
                name: "eez.deriver.batch_posted.skipped",
                Level::DEBUG,
                l1_block_number,
                tx_hash = %tx_hash,
                "live event for an already-indexed batch; skipping",
            );
            return self.flush_deferred_safe(last_in_l1_block).await;
        }

        // L1-finality gate: no `L2ExecutionPerformed` for our rollupId
        // means the bundle didn't settle — L1's stored root didn't
        // advance and the claimed roots are phantoms. Skip; the
        // composer's next slot re-attempts. Without this gate the
        // deriver would replay the unsettled range, reorg L2, then fail
        // `check_claimed_state` as a spurious divergence.
        if settlement.is_empty() {
            let _guard = self.inner.committer.begin_reconcile().await?;
            self.inner.l1_head.publish(self.batch_evidence(
                eez_l1::L1BlockRef {
                    number: l1_block_number,
                    hash: l1_block_hash,
                },
                (tx_hash, tx_index),
                &post_batch_input,
                &semantic_transactions,
                settlement,
                None,
            )?);
            event!(
                name: "eez.deriver.batch.unsettled",
                Level::INFO,
                l1_block_number,
                tx_hash = %tx_hash,
                submitter = %submitter,
                "postBatch's L1 block has no L2ExecutionPerformed for our rollup; bundle didn't settle, skipping (re-attempt expected)",
            );
            return self.flush_deferred_safe(last_in_l1_block).await;
        }

        event!(
            name: "eez.deriver.batch_posted.entered",
            Level::INFO,
            l1_block_number,
            tx_hash = %tx_hash,
            state_applied,
            "on_batch_posted entered; awaiting reconcile lock",
        );
        // Remote evidence is acquired before reconcile_batch_blocks takes the
        // commit lock; that guard remains owned through acceptance publication.
        event!(
            name: "eez.deriver.batch_posted.lock_acquired",
            Level::INFO,
            l1_block_number,
            tx_hash = %tx_hash,
            state_applied,
            "reconcile lock acquired",
        );

        // `state_applied` only catches the IMMEDIATE-entry path, where
        // `_applyRootUpdates` fires in the postBatch tx itself. In the
        // DEFERRED-entry path (our setter / deposit flow) it fires later
        // inside the user_tx calling `executeCrossChainCall` — a
        // different tx hash in the same L1 block — so the batch-log scanner
        // reports `state_applied=false`. The `settled_count` gate above
        // already confirmed something settled, so we still process.
        if !state_applied {
            event!(
                name: "eez.deriver.batch.deferred_path",
                Level::INFO,
                l1_block_number,
                tx_hash = %tx_hash,
                submitter = %submitter,
                "no L2ExecutionPerformed in postBatch tx — deferred-entry flow; bundled user_tx settled the state",
            );
        }

        // from_block is the next L2 block after the highest indexed
        // batch — the shared L1CanonicalHead is the source of truth.
        let last_indexed_l2 = self.inner.l1_head.last_indexed_l2();
        let resumed = settlement.start > 0;
        // Anchor on the batch's own start, not our cursor: a competing
        // composer's batch can cover a range that begins below it.
        let anchor = match settlement.entry_state {
            Some(entry_state) => self
                .batch_anchor(last_indexed_l2, entry_state, block_count)?
                .unwrap_or(last_indexed_l2),
            None => last_indexed_l2,
        };
        let (from_block, to_block) = batch_l2_range(anchor, resumed, block_count);

        // Cursor guard: a root this batch cannot leave at the cursor means the local
        // index is off L1 (e.g. a dropped event); let the run-loop resync re-anchor.
        if let Some(entry_state) = settlement.entry_state {
            let local_root = self.l2_hash_at(anchor)?;
            if !cursor_root_accepted(local_root, entry_state, settlement.final_state) {
                event!(
                    name: "eez.deriver.cursor.misaligned",
                    Level::ERROR,
                    l1_block_number,
                    tx_hash = %tx_hash,
                    last_indexed_l2,
                    anchor,
                    local_root = %local_root,
                    entry_state = %entry_state,
                    final_state = ?settlement.final_state,
                    applied_start = settlement.start,
                    "local block hash at cursor is neither the commitment the batch's applied run started from nor its settled endpoint; resync required",
                );
                return Err(DeriverError::local_diverged(from_block));
            }
        }

        // Per-block reconciliation: skip blocks whose tx lists already
        // match the batch, and STF-replay the rest (reth fork-switches
        // as needed).
        let (replayed, sync_block_hash, _reconcile_guard, authenticated_l1) = self
            .reconcile_batch_blocks(
                from_block,
                &payload,
                post_batch_input.clone(),
                &semantic_transactions,
                eez_l1::L1BlockRef {
                    number: l1_block_number,
                    hash: l1_block_hash,
                },
                tx_hash,
                settlement,
            )
            .await?;
        event!(
            name: "eez.deriver.reconcile.done",
            Level::DEBUG,
            l1_block_number,
            tx_hash = %tx_hash,
            from_block,
            to_block,
            replayed,
            "per-block reconciliation complete (pre-divergence check)",
        );

        // Both ends are what L1 ACTUALLY ran, never the claimed chain's endpoints.
        // Pre-check skipped when resumed: the cursor-alignment guard above
        // already checked it, at `last_indexed_l2` — `from_block - 1` would
        // check the wrong height now that a resumed batch doesn't advance past it.
        let settled_end = self.check_claimed_state(
            if resumed {
                None
            } else {
                settlement.entry_state.or(claimed_current_root)
            },
            settlement.final_state.or(claimed_new_root),
            anchor,
            from_block,
            to_block,
            sync_block_hash,
            l1_block_number,
            tx_hash,
        )?;

        let settled_last_l2 = settled_end.unwrap_or(to_block);
        let correlation = self.correlation_record(
            l1_block_number,
            l1_block_hash,
            tx_hash,
            tx_index,
            from_block,
            settled_last_l2,
            (settled_last_l2 == to_block)
                .then_some(sync_block_hash)
                .flatten(),
        )?;
        let evidence = self.batch_evidence(
            eez_l1::L1BlockRef {
                number: l1_block_number,
                hash: l1_block_hash,
            },
            (tx_hash, tx_index),
            &post_batch_input,
            &semantic_transactions,
            settlement,
            Some(eez_l1::L2Endpoint {
                number: settled_last_l2,
                hash: correlation.last_l2().hash,
            }),
        )?;
        self.publish_correlation(correlation, evidence, &authenticated_l1)?;
        let l1_settled_root = settlement.final_state.unwrap_or_default();
        // Safe moves once per L1 block, at its last batch: a resumed batch rewrites
        // the height its same-block predecessor settled, orphaning that safe hash.
        if !last_in_l1_block {
            event!(
                name: "eez.deriver.safe.deferred",
                Level::DEBUG,
                from_block,
                to_block,
                l1_block_number,
                tx_hash = %tx_hash,
                "more batches in this L1 block; safe advance waits for its last one",
            );
            return Ok(());
        }
        let Some(new_safe_hash) = self.sync_safe_to_cursor().await? else {
            return Ok(());
        };
        let l2_safe_state_root = self
            .l2_sealed_header_at(self.inner.l1_head.last_indexed_l2())?
            .state_root();
        event!(
            name: "eez.deriver.safe.advanced",
            Level::INFO,
            event_name = "eez.deriver.safe.advanced",
            from_block,
            to_block,
            applied_entries = settlement.len,
            l1_settled_root = %l1_settled_root,
            l1_block_number,
            tx_hash = %tx_hash,
            submitter = %submitter,
            new_safe_hash = %new_safe_hash,
            l2_safe_block_hash = %new_safe_hash,
            l2_safe_state_root = %l2_safe_state_root,
            "advanced L2 safe head from L1-confirmed batch",
        );
        Ok(())
    }

    /// Runs the safe advance a skipped last batch would have done, else safe
    /// stalls until the next posted batch.
    async fn flush_deferred_safe(&self, last_in_l1_block: bool) -> DeriverResult<()> {
        if last_in_l1_block {
            self.sync_safe_to_cursor().await?;
        }
        Ok(())
    }

    /// Moves `safe` to the L1-confirmed cursor, re-reading the header so a height
    /// a resume rewrote gives its current hash. Only L1 finality moves finalized.
    async fn sync_safe_to_cursor(&self) -> DeriverResult<Option<B256>> {
        let cursor = self.inner.l1_head.last_indexed_l2();
        if cursor <= self.inner.safe_l2_block.load(Ordering::Acquire) {
            return Ok(None);
        }
        let safe_header = match self
            .inner
            .correlations
            .by_l2_number(cursor)
            .map_err(DeriverError::l2_provider)?
        {
            Some(accepted) => SealedHeader::seal_slow(accepted.batch.endpoint_header),
            None if cursor == 0 => self.l2_sealed_header_at(0)?,
            None => {
                return Err(DeriverError::l2_provider(
                    "surviving accepted endpoint is missing",
                ));
            }
        };
        let safe_hash = safe_header.hash();
        let finalized_hash = self.l2_hash_at(self.inner.l1_head.finalized_l2())?;
        self.inner
            .committer
            .advance_safe_finalized(safe_header, finalized_hash)
            .await?;
        self.inner.safe_l2_block.store(cursor, Ordering::Release);
        Ok(Some(safe_hash))
    }

    async fn on_l1_reorg(
        &self,
        common_ancestor_number: u64,
        old_head_hash: B256,
        new_head_number: u64,
        new_head_hash: B256,
    ) -> DeriverResult<()> {
        // Delivered Reorg events already carry the surviving canonical L1
        // ancestor; hash-tail auditing is reserved for missed reorgs.
        let guard = self.inner.committer.begin_reconcile().await?;

        if common_ancestor_number
            < self
                .inner
                .derivation
                .schedule()
                .config()
                .l1_reference
                .number
        {
            return Err(DeriverError::l2_provider(
                "reorg crosses the finalized genesis reference",
            ));
        }
        let old_cursor = self.inner.l1_head.cursor();
        let (new_cursor, _new_finalized, dropped) = self
            .inner
            .l1_head
            .retreat_on_l1_reorg(common_ancestor_number);
        let removed_correlations = self
            .inner
            .correlations
            .remove_after_l1(common_ancestor_number)
            .map_err(DeriverError::l2_provider)?;
        let new_safe_hash = if dropped > 0 {
            self.retreat_l2_to_cursor(new_cursor).await?
        } else {
            self.l2_hash_at(new_cursor)?
        };
        let parent = self.l2_sealed_header_at(new_cursor)?;
        drop(guard);
        self.prepare_history(
            &parent,
            eez_l1::L1BlockRef {
                number: new_head_number,
                hash: new_head_hash,
            },
        )
        .await?;
        let _guard = self.inner.committer.begin_reconcile().await?;
        self.discard_stale_history_suffix(parent).await?;

        event!(
            name: "eez.deriver.l1.reorg.retreated",
            Level::WARN,
            event_name = "eez.deriver.l1.reorg.retreated",
            common_ancestor_number,
            old_head_hash = %old_head_hash,
            new_head_number,
            new_head_hash = %new_head_hash,
            old_cursor,
            new_cursor,
            dropped_batches = dropped,
            removed_correlations,
            new_safe_hash = %new_safe_hash,
            l2_safe_block_hash = %new_safe_hash,
            "L1 reorg rolled out confirmed batches; L2 head retreated to the surviving safe cursor",
        );
        Ok(())
    }

    /// Retreat reth's safe/finalized anchors and canonical head to the
    /// L1-derived cursor. Caller holds the reconcile lock so the Sequencer
    /// can't extend the branch between the two forkchoice updates.
    async fn retreat_l2_to_cursor(&self, cursor: u64) -> DeriverResult<B256> {
        let safe_header = self.l2_sealed_header_at(cursor)?;
        let safe_hash = safe_header.hash();
        let finalized_hash = self.l2_hash_at(self.inner.l1_head.finalized_l2())?;

        // Order matters: retreat safe/finalized first while the old head is
        // still canonical, then roll head back and repair the parent mirror.
        self.inner
            .committer
            .advance_safe_finalized(safe_header.clone(), finalized_hash)
            .await?;
        self.inner.committer.reorg_to(safe_header).await?;
        self.inner.safe_l2_block.store(cursor, Ordering::Release);
        Ok(safe_hash)
    }

    async fn on_l1_finalized(&self, l1_finalized_block: u64) -> DeriverResult<()> {
        // Find highest batch with l1_block <= l1_finalized_block.
        // That batch's last_l2_block is the new L2 finalized head.
        let new_finalized = self
            .inner
            .l1_head
            .highest_l2_at_or_below_l1(l1_finalized_block)
            .unwrap_or(0);
        let old_finalized = self.inner.l1_head.finalized_l2();
        if new_finalized <= old_finalized {
            return Ok(());
        }
        // Bound by safe — finalized never exceeds safe.
        let current_safe = self.inner.safe_l2_block.load(Ordering::Acquire);
        let bounded = new_finalized.min(current_safe);
        if bounded <= old_finalized {
            return Ok(());
        }

        let safe_header = self.l2_sealed_header_at(current_safe)?;
        let finalized_hash = self.l2_hash_at(bounded)?;
        self.inner
            .committer
            .advance_safe_finalized(safe_header, finalized_hash)
            .await?;
        self.inner.l1_head.set_finalized_l2(bounded);
        event!(
            name: "eez.deriver.finalized.advanced",
            Level::INFO,
            event_name = "eez.deriver.finalized.advanced",
            l1_finalized_block,
            l2_finalized = bounded,
            "advanced L2 finalized head from L1 finality",
        );
        Ok(())
    }

    fn l2_sealed_header_at(
        &self,
        l2_block: u64,
    ) -> DeriverResult<SealedHeader<alloy_consensus::Header>> {
        self.inner
            .l2_provider
            .sealed_header(l2_block)
            .map_err(DeriverError::l2_provider)?
            .ok_or_else(|| {
                DeriverError::l2_provider(format!("local L2 header at {l2_block} missing"))
            })
    }

    fn l2_hash_at(&self, l2_block: u64) -> DeriverResult<B256> {
        Ok(self.l2_sealed_header_at(l2_block)?.hash())
    }

    /// Block whose hash is `entry_state`: where this batch's run starts. A
    /// competing composer's batch can begin below our cursor, so search back.
    fn batch_anchor(
        &self,
        cursor: u64,
        entry_state: B256,
        window: u64,
    ) -> DeriverResult<Option<u64>> {
        anchor_with_overlap_log(cursor, entry_state, window, |block| self.l2_hash_at(block))
    }

    /// Per-block reconciliation against a decoded V0 payload beginning at
    /// `from_block`: for each block, skip if local reth already holds the same
    /// parent, execution inputs and transaction list; otherwise STF-replay it
    /// (reth fork-switches via `newPayload` + head-FCU). Once any block in the
    /// batch is replayed, replay every later block too so matching inputs and
    /// transactions are rebuilt on the new ancestry. Returns the count of
    /// blocks replayed.
    ///
    /// NOTE: not transactional. If a replay fails partway, earlier blocks
    /// are already committed to reth's canonical chain, leaving local L2
    /// in a half-state. The per-block `eez.deriver.reconcile.block` log
    /// records progress so a failure shows how far the loop got.
    /// [open: roll the canonical head back to the pre-loop snapshot on
    /// failure.]
    #[allow(clippy::too_many_arguments)]
    async fn reconcile_batch_blocks(
        &self,
        from_block: u64,
        payload: &Payload<'_>,
        post_batch_input: Bytes,
        semantic_transactions: &[eez_protocol::blob_semantics::SemanticTransaction],
        posting_block: eez_l1::L1BlockRef,
        tx_hash: B256,
        settlement: eez_l1::Settlement,
    ) -> DeriverResult<(
        u64,
        Option<B256>,
        tokio::sync::OwnedMutexGuard<()>,
        eez_l1::derivation::AuthenticatedL1Header,
    )> {
        let l1_block_number = posting_block.number;
        let terminal = payload.terminal();
        let terminal_prefix = terminal
            .transactions
            .clone()
            .map(Bytes::copy_from_slice)
            .collect::<Vec<_>>();
        let terminal_height = if settlement.start > 0 {
            from_block
        } else {
            from_block
                .checked_add(u64::from(payload.block_count()) - 1)
                .ok_or_else(|| DeriverError::l2_provider("candidate endpoint overflow"))?
        };
        // A resumed terminal prefix still belongs to the original complete
        // candidate. Check its original Sync parent/count against the posting
        // block, not against the locally consumed terminal variant.
        let original_parent_number = terminal_height
            .checked_sub(u64::from(payload.block_count()))
            .ok_or_else(|| DeriverError::l2_provider("candidate begins before genesis"))?;
        let generation = self.inner.l1_head.policy_anchor_snapshot().generation;
        let original_parent = self.l2_sealed_header_at(original_parent_number)?;
        let l1 = self
            .inner
            .derivation
            .canonical_header(posting_block)
            .await
            .map_err(DeriverError::l2_provider)?;
        use alloy_sol_types::SolCall as _;
        use eez_protocol::l1_history::EvidenceAnchor;
        let posted = eez_protocol::abi::postAndVerifyBatchCall::abi_decode(&post_batch_input)
            .map_err(DeriverError::l2_provider)?;
        if posted.abi_encode() != post_batch_input.as_ref() {
            return Err(DeriverError::l2_provider(
                "noncanonical settlement calldata",
            ));
        }
        // Fetch through inclusion to make the latest relevant cutoff available
        // even after missed L1 slots. The proof bridge itself ends at its anchor.
        self.prepare_history(&original_parent, posting_block)
            .await?;
        let reconcile_guard = self.inner.committer.begin_reconcile().await?;
        if !self.inner.l1_head.generation_is(generation)
            || self.l2_hash_at(original_parent_number)? != original_parent.hash()
        {
            return Err(DeriverError::l2_provider(
                "reconciliation predecessor changed during L1 acquisition",
            ));
        }
        let prepared = self
            .inner
            .l1_head
            .prepared_history()
            .ok_or_else(|| DeriverError::l2_provider("missing prepared follower history"))?;
        let chain = prepared.chain();
        let number = if posted.batch.blockNumber == u64::MAX {
            l1_block_number
                .checked_sub(1)
                .ok_or_else(|| DeriverError::l2_provider("settlement in L1 genesis"))?
        } else {
            posted.batch.blockNumber
        };
        let endpoint = chain.at(number).map_err(DeriverError::l2_provider)?;
        let historical = posted.batch.blockNumber != u64::MAX;
        let anchor = if historical {
            EvidenceAnchor::Historical {
                number,
                hash: endpoint.hash(),
            }
        } else {
            EvidenceAnchor::ActualParent {
                number,
                hash: endpoint.hash(),
                target_timestamp: l1.header().timestamp,
            }
        };
        self.inner
            .offline_profile
            .validate_inclusion(anchor, endpoint.header(), l1.header())
            .map_err(DeriverError::l2_provider)?;
        // Lookup-only and reverted/static observations can be present in the
        // settlement table even when no semantic L2 transaction was published.
        let has_actions = !semantic_transactions.is_empty()
            || posted.batch.entries.len() != 1
            || !posted.batch.staticEntries.is_empty()
            || posted.batch.entries.first().is_none_or(|entry| {
                !entry.l2ToL1Calls.is_empty() || !entry.expectedL1ToL2Calls.is_empty()
            });
        if historical && has_actions {
            return Err(DeriverError::l2_provider(
                "historical mode forbids every cross-chain action",
            ));
        }
        let schedule = self.inner.derivation.schedule();
        schedule
            .candidate(
                &original_parent,
                payload.block_count(),
                if historical {
                    endpoint.header().timestamp
                } else {
                    l1.header().timestamp
                },
                has_actions,
            )
            .map_err(DeriverError::l2_provider)?;
        let first = chain
            .headers()
            .first()
            .ok_or_else(|| DeriverError::l2_provider("missing follower L1 history"))?
            .header()
            .number;
        let bridge = chain
            .range(first, number)
            .map_err(DeriverError::l2_provider)?;
        // Authenticate the inherited endpoint and every recorded import against
        // this batch's exact anchor. Future evidence cannot authorize an import
        // beyond the anchor or replace the producer's chosen endpoint.
        let mut inherited = {
            let state = self
                .inner
                .l2_provider
                .state_by_block_hash(original_parent.hash_slow())
                .map_err(DeriverError::l2_provider)?;
            let mut db = StateProviderDatabase::new(EvmStateProviderAdapter(state.as_ref()));
            self.inner
                .evm_config
                .history_state(&mut db)
                .map_err(DeriverError::l2_provider)?
        };
        bridge
            .verify_origin(inherited.endpoint, inherited.endpoint_hash, anchor)
            .map_err(DeriverError::l2_provider)?;
        let history_schedule = schedule.history().map_err(DeriverError::l2_provider)?;
        for (position, mut plan) in payload.blocks().enumerate() {
            let timestamp = schedule
                .timestamp(original_parent.number + position as u64 + 1)
                .map_err(DeriverError::l2_provider)?;
            let first = plan
                .transactions
                .next()
                .map(TransactionSigned::decode_2718_exact)
                .transpose()
                .map_err(DeriverError::l2_provider)?;
            let hashes = match &first {
                Some(TransactionSigned::History(tx)) => Some(&tx.hashes),
                _ => None,
            };
            let selected = hashes
                .map(|hashes| {
                    inherited
                        .endpoint
                        .checked_add(hashes.len() as u64)
                        .ok_or_else(|| DeriverError::l2_provider("history endpoint overflow"))
                })
                .transpose()?;
            let transition = history_schedule
                .execution(timestamp, inherited, &bridge, selected)
                .map_err(DeriverError::l2_provider)?;
            let expected = transition
                .import
                .as_ref()
                .map(|input| eez_protocol::l1_history::decode_import(input).map(|call| call.hashes))
                .transpose()
                .map_err(DeriverError::l2_provider)?;
            if expected.as_ref() != hashes {
                return Err(DeriverError::l2_provider(
                    "published history update differs from anchored ancestry",
                ));
            }
            inherited.endpoint = transition.origin_number;
            inherited.endpoint_hash = transition.origin_hash;
        }
        // Reconstruct checked actions from the authenticated enclosing stream.
        // Settlement tables authorize applied effects; they cannot substitute
        // for L2 reconstruction inputs, including value-bearing/nested calls.
        event!(
            name: "eez.deriver.reconcile.start",
            Level::INFO,
            l1_block_number,
            tx_hash = %tx_hash,
            from_block,
            block_count = payload.block_count(),
            cross_chain = self.inner.system_tx_cfg.is_some(),
            "reconcile_batch_blocks entered",
        );
        // Reconstruct the Sync block's FULL tx list (system txs interleaved with
        // their user txs) via the SAME builder the composer uses → byte-identical
        // by construction. `None` when no cross-chain cfg (loop uses user txs
        // verbatim).
        //
        // Producing entries are ordered `[anchor, outbound…, inbound…]`:
        // `immediateEntryCount` frames the anchor and complete outbound hosts;
        // the remaining entries describe deferred L1-origin calls.
        // `Settlement::producing_slice` says WHICH of those L1 actually ran, read
        // off the settled block hashes — the anchor cannot be assumed to have run, since
        // a competing same-block batch can supply its hop.
        //
        // L1 entries whose L2-origin calls must match post-replay EEZL2 events.
        // This includes outbound roots and callbacks to L1 inside inbound roots.
        let mut gate_expected_hashes = Vec::new();
        let mut gate_should_verify = false;
        let mut sync_sources = Vec::new();
        let sync_block_txs: Option<Vec<eez_protocol::system_tx::PlannedTransaction>> = match self
            .inner
            .system_tx_cfg
            .as_ref()
        {
            Some(cfg) => {
                // Decode settlement entries independently from DA. They decide
                // which effects L1 applied and remain the authorization source
                // for outbound value. The semantic stream supplies the exact
                // L2 tables; settlement ABI cannot substitute for nested inputs.
                use alloy_sol_types::SolCall as _;
                let posted =
                    eez_protocol::abi::postAndVerifyBatchCall::abi_decode(&post_batch_input)
                        .map_err(|e| {
                            DeriverError::l2_provider(format!("decode postBatch({tx_hash}): {e}"))
                        })?;
                let immediate_count =
                    usize::try_from(posted.batch.immediateEntryCount).map_err(|_| {
                        DeriverError::l2_provider(format!(
                            "postBatch({tx_hash}) immediateEntryCount does not fit usize"
                        ))
                    })?;
                if immediate_count == 0 || immediate_count > posted.batch.entries.len() {
                    return Err(DeriverError::l2_provider(format!(
                        "postBatch({tx_hash}) has invalid immediateEntryCount {immediate_count} for {} entries",
                        posted.batch.entries.len(),
                    )));
                }
                let mut settlement_outbound = posted.batch.entries[1..immediate_count].to_vec();
                let mut settlement_inbound = posted.batch.entries[immediate_count..].to_vec();
                // Canonical lookup-only entries carry a proven L1-visible
                // result but authorize no L2 execution and consume no DA
                // sidecar. Their exact no-op shape is checked fail-closed;
                // arbitrary root-only deferred entries remain invalid effects.
                settlement_inbound.retain(|entry| {
                    !eez_protocol::settlement::is_lookup_only_inbound_entry(
                        entry,
                        cfg.this_rollup_id,
                    )
                });

                let actions = eez_protocol::blob_semantics::ReconstructionActions::from_semantics(
                    semantic_transactions,
                    eez_protocol::RollupId(cfg.this_rollup_id),
                    cfg.l2_chain_id,
                )
                .map_err(DeriverError::l2_provider)?;
                actions
                    .bind_settlement(&posted.batch)
                    .map_err(DeriverError::l2_provider)?;
                let outbound_group_sizes = actions
                    .outbound()
                    .iter()
                    .map(|group| group.effects.len())
                    .collect::<Vec<_>>();
                let all_sources = match anchor {
                    EvidenceAnchor::ActualParent { hash, .. } => {
                        let context = eez_protocol::native_source::LiveSourceContext::new(
                            cfg.l2_chain_id,
                            hash,
                        )
                        .map_err(DeriverError::l2_provider)?;
                        actions
                            .source_entries(&context)
                            .map_err(DeriverError::l2_provider)?
                    }
                    EvidenceAnchor::Historical { .. } => Vec::new(),
                };
                let all_outbound = actions.outbound().to_vec();
                let mut inbound = actions.inbound().to_vec();

                // Rebuild exactly the steps L1 ran; `skip > 0` = it resumed
                // mid-chain (see `ProducingSlice`).
                let outbound_settlement_root_counts =
                    eez_protocol::system_tx::outbound_settlement_root_counts(&settlement_outbound)
                        .map_err(|error| {
                            DeriverError::l2_provider(format!(
                                "postBatch({tx_hash}) has invalid outbound root partition: {error}"
                            ))
                        })?;
                let original_outbound_settlement_count = settlement_outbound.len();
                let slice = ProducingSlice::split(
                    settlement,
                    original_outbound_settlement_count,
                    inbound.len(),
                );
                if slice
                    .leaves_entries_unconsumed(original_outbound_settlement_count, inbound.len())
                {
                    event!(
                        name: "eez.deriver.reconcile.partial_consumption",
                        Level::WARN,
                        tx_hash = %tx_hash,
                        outbound = original_outbound_settlement_count,
                        inbound = inbound.len(),
                        outbound_skip = slice.outbound_skip,
                        outbound_take = slice.outbound_take,
                        inbound_skip = slice.inbound_skip,
                        inbound_take = slice.inbound_take,
                        anchor_applied = settlement.start == 0,
                        "L1 ran only part of this batch; reconstructing exactly that slice",
                    );
                }
                let selection = selected_replay_groups(
                    settlement,
                    &outbound_settlement_root_counts,
                    &outbound_group_sizes,
                    inbound.len(),
                )
                .map_err(|error| DeriverError::local_diverged_with_msg(from_block, &error))?;
                let selected_outbound_sources = all_sources
                    .get(selection.outbound_groups.clone())
                    .ok_or_else(|| {
                        DeriverError::l2_provider(
                            "outbound source rows do not cover replay selection",
                        )
                    })?;
                let inbound_start = all_outbound
                    .len()
                    .checked_add(slice.inbound_skip)
                    .ok_or_else(|| {
                        DeriverError::l2_provider("inbound source row offset overflow")
                    })?;
                let inbound_end =
                    inbound_start
                        .checked_add(slice.inbound_take)
                        .ok_or_else(|| {
                            DeriverError::l2_provider("inbound source row range overflow")
                        })?;
                let selected_inbound_sources =
                    all_sources.get(inbound_start..inbound_end).ok_or_else(|| {
                        DeriverError::l2_provider(
                            "inbound source rows do not cover replay selection",
                        )
                    })?;
                let selected_native_sources = selected_outbound_sources
                    .iter()
                    .chain(selected_inbound_sources)
                    .copied()
                    .collect::<Vec<_>>();
                let outbound = all_outbound[selection.outbound_groups].to_vec();
                inbound.drain(..slice.inbound_skip);
                inbound.truncate(slice.inbound_take);
                settlement_outbound.drain(..slice.outbound_skip);
                settlement_outbound.truncate(slice.outbound_take);
                settlement_inbound.drain(..slice.inbound_skip);
                settlement_inbound.truncate(slice.inbound_take);

                // Independently verify each producing L1 table against the L2
                // sidecar that re-execution will consume, then retain exactly
                // the committed L2-origin hashes for the post-replay event
                // gate. A naturally reverted proxy frame has no receipt log
                // and is intentionally omitted by `verify_nested_group`.
                gate_should_verify =
                    !settlement_outbound.is_empty() || !settlement_inbound.is_empty();
                let selected_root_counts =
                    eez_protocol::system_tx::outbound_settlement_root_counts(
                        &settlement_outbound,
                    )
                    .map_err(|error| {
                        DeriverError::l2_provider(format!(
                            "postBatch({tx_hash}) has invalid selected outbound partition: {error}"
                        ))
                    })?;
                let selected_sources = outbound
                    .iter()
                    .flat_map(|group| group.effects.iter())
                    .map(|effect| effect.source_entry.clone())
                    .collect::<Vec<_>>();
                if selected_root_counts.iter().sum::<usize>() != selected_sources.len() {
                    return Err(DeriverError::l2_provider(format!(
                        "postBatch({tx_hash}) selected outbound hosts cover {} roots, but reconstruction selected {} source entries",
                        selected_root_counts.iter().sum::<usize>(),
                        selected_sources.len(),
                    )));
                }
                // Static lookups share one pool for the whole signed user
                // transaction. Match the signer's whole-load boundary before
                // independently verifying the reconstructed source tables.
                let source_group_ranges = outbound
                    .iter()
                    .scan(0usize, |start, group| {
                        let end = *start + group.effects.len();
                        let has_static = group.effects.iter().any(|effect| {
                            matches!(
                                effect.source_entry,
                                eez_protocol::system_tx::OutboundSourceEntry::Static(_)
                            )
                        });
                        let range = (*start, (end, has_static));
                        *start = end;
                        Some(range)
                    })
                    .collect::<std::collections::BTreeMap<_, _>>();
                let mut source_cursor = 0usize;
                for (host, &count) in settlement_outbound.iter().zip(&selected_root_counts) {
                    let source_end = source_cursor + count;
                    let selected = &selected_sources[source_cursor..source_end];
                    let (&group_start, &(group_end, group_has_static)) = source_group_ranges
                        .range(..=source_cursor)
                        .next_back()
                        .ok_or_else(|| {
                            DeriverError::local_diverged_with_msg(
                                from_block,
                                "outbound host has no signed source group",
                            )
                        })?;
                    let host_has_static = selected.iter().any(|source| {
                        matches!(
                            source,
                            eez_protocol::system_tx::OutboundSourceEntry::Static(_)
                        )
                    });
                    if (group_has_static || host_has_static)
                        && (source_cursor != group_start || source_end != group_end)
                    {
                        return Err(DeriverError::local_diverged_with_msg(
                            from_block,
                            "mixed outbound host must cover one complete signed source group",
                        ));
                    }
                    let verification = eez_protocol::entries::verify_outbound_source_group(
                        host,
                        selected,
                        eez_protocol::RollupId(cfg.this_rollup_id),
                    )
                    .map_err(|error| {
                        DeriverError::local_diverged_with_msg(
                            from_block,
                            &format!(
                                "outbound settlement/source tables are inconsistent (tx={tx_hash}): {error}"
                            ),
                        )
                    })?;
                    gate_expected_hashes.extend(verification.l2_outbound_hashes);
                    source_cursor = source_end;
                }
                if settlement_inbound.len() != inbound.len() {
                    return Err(DeriverError::l2_provider(format!(
                        "postBatch({tx_hash}) selected inbound settlement/sidecars differ ({}, {})",
                        settlement_inbound.len(),
                        inbound.len(),
                    )));
                }
                for (host, sidecar) in settlement_inbound.iter().zip(&inbound) {
                    let verification = eez_protocol::entries::verify_nested_group(
                        host,
                        std::slice::from_ref(sidecar.as_abi_entry()),
                        eez_protocol::entries::NestedPairDirection::Inbound,
                        eez_protocol::RollupId(cfg.this_rollup_id),
                    )
                    .map_err(|error| {
                        DeriverError::local_diverged_with_msg(
                            from_block,
                            &format!(
                                "inbound settlement/sidecar tables are inconsistent (tx={tx_hash}): {error}"
                            ),
                        )
                    })?;
                    gate_expected_hashes.extend(verification.l2_outbound_hashes);
                }

                let pairs = eez_protocol::system_tx::build_cross_chain_sync_groups(
                    &outbound, &inbound, cfg,
                )
                .map_err(|e| {
                    DeriverError::l2_provider(format!(
                        "build_cross_chain_sync_groups(tx={tx_hash}): {e}"
                    ))
                })?;
                let full = eez_protocol::system_tx::interleave_sync_block_txs(&pairs);
                event!(
                    name: "eez.deriver.reconcile.sync_block_built",
                    Level::INFO,
                    event_name = "eez.deriver.reconcile.sync_block_built",
                    tx_hash = %tx_hash,
                    sync_height = terminal_height,
                    outbound = outbound.len(),
                    inbound = inbound.len(),
                    sync_block_txs = full.len(),
                    "rebuilt Sync block",
                );
                sync_sources = selected_native_sources;
                Some(full)
            }
            None => {
                if !semantic_transactions.is_empty() {
                    return Err(DeriverError::l2_provider(
                        "cross-chain actions require reconstruction configuration",
                    ));
                }
                None
            }
        };

        // Hash of the Sync block replayed this pass; the gate must read by hash.
        let mut sync_block_hash: Option<B256> = None;
        let mut gate_first_transaction = 0;
        let stale_boundary = !local_batch_boundary_matches(&self.inner.l2_provider, from_block)?;
        let mut suffix_replay = SuffixReplay::new(stale_boundary);
        let mut replayed: u64 = 0;
        let last_index = payload.block_count().saturating_sub(1);
        let resumed = settlement.start > 0;
        if resumed {
            let terminal_environment = self.derived_environment(
                from_block - 1,
                terminal.beneficiary,
                terminal.extra_data,
            )?;
            let terminal_inputs = ExecutionInputs::Derived(&terminal_environment);
            self.validate_execution_inputs(from_block - 1, terminal_inputs, &terminal_prefix)?;
            // The competing batch already committed this Sync block; only the
            // entries this batch settled are new. Append them to its EXISTING
            // content rather than a fresh block — see `batch_l2_range`.
            if stale_boundary {
                return Err(DeriverError::local_diverged_with_msg(
                    from_block,
                    "resumed batch's Sync block is missing or reorged; cannot append its \
                     settled entries without the existing content",
                ));
            }
            event!(
                name: "eez.deriver.resumed.placement",
                Level::INFO,
                l1_block_number,
                tx_hash = %tx_hash,
                from_block,
                applied_start = settlement.start,
                applied_len = settlement.len,
                entry_state = ?settlement.entry_state,
                claimed_block_count = payload.block_count(),
                "resumed batch placement: appending settled entries to Sync block {from_block}",
            );
            let mut block_txs: Vec<Bytes> = self
                .inner
                .l2_provider
                .block_by_number(from_block)
                .map_err(DeriverError::l2_provider)?
                .ok_or_else(|| {
                    DeriverError::l2_provider(format!("local L2 block at {from_block} missing"))
                })?
                .body()
                .transactions()
                .iter()
                .map(|transaction| Bytes::from(transaction.encoded_2718()))
                .collect();
            if !block_txs.starts_with(&terminal_prefix) {
                return Err(DeriverError::local_diverged_with_msg(
                    from_block,
                    "resumed terminal ordinary prefix differs from V0",
                ));
            }
            let plans = sync_block_txs.as_deref().unwrap_or_default();
            let parent = self.l2_sealed_header_at(from_block - 1)?;
            // Only authenticated settlement progress selects append versus
            // idempotent replay. A sourceHash identifies an input; competing
            // candidates can share one and it is never a deduplication key.
            let local_hash = self.l2_sealed_header_at(from_block)?.hash();
            let entry_state = settlement
                .entry_state
                .ok_or_else(|| DeriverError::l2_provider("resumed settlement lacks entry root"))?;
            let already_applied = !plans.is_empty() && settlement.final_state == Some(local_hash);
            let append_at = if already_applied {
                let start = block_txs
                    .len()
                    .checked_sub(plans.len())
                    .filter(|start| *start >= terminal_prefix.len())
                    .ok_or_else(|| {
                        DeriverError::local_diverged_with_msg(
                            from_block,
                            "settled suffix exceeds retained block",
                        )
                    })?;
                let (_, prefix) =
                    self.execute_block_on_parent(&parent, &block_txs[..start], terminal_inputs)?;
                if prefix.hash() != entry_state {
                    return Err(DeriverError::local_diverged_with_msg(
                        from_block,
                        "retained suffix does not start at authenticated entry root",
                    ));
                }
                start
            } else {
                if local_hash != entry_state {
                    return Err(DeriverError::local_diverged_with_msg(
                        from_block,
                        "resumed block matches neither entry nor final root",
                    ));
                }
                block_txs.len()
            };
            let (_, reconstructed_header, new_content) = self.execute_block_with_plans(
                &parent,
                &block_txs[..append_at],
                plans,
                &sync_sources,
                terminal_inputs,
            )?;
            if settlement.final_state != Some(reconstructed_header.hash()) {
                return Err(DeriverError::local_diverged_with_msg(
                    from_block,
                    "reconstructed resumed suffix differs from authenticated final root",
                ));
            }
            if already_applied && block_txs[append_at..] != new_content {
                return Err(DeriverError::local_diverged_with_msg(
                    from_block,
                    "retained suffix differs from exact reconstruction",
                ));
            }
            // This uses the actual canonical H-1 parent, never the resumed
            // entry-state prefix at H or the original ordinary count.
            terminal_inputs.attributes(&self.inner.evm_config, &parent, &block_txs)?;
            if !self.local_block_matches(from_block, &block_txs, terminal_inputs)? {
                return Err(DeriverError::local_diverged_with_msg(
                    from_block,
                    "resumed V0 prefix has different execution inputs",
                ));
            }
            // The preceding settlement already authorized the retained prefix.
            // This batch's table authorizes only its appended transactions,
            // including when replay is idempotently skipped after a crash.
            gate_first_transaction = if already_applied {
                block_txs.len() - new_content.len()
            } else {
                block_txs.len()
            };
            event!(
                name: "eez.deriver.reconcile.block",
                Level::DEBUG,
                l1_block_number,
                tx_hash = %tx_hash,
                l2_block = from_block,
                action = if already_applied { "skip" } else { "replay" },
                tx_count = block_txs.len() + new_content.len(),
                resumed_mid_chain = true,
                "appending settled entries to the existing Sync block",
            );
            if !already_applied {
                // Rewriting a height at or below `safe` orphans the stored safe hash
                // and the engine rejects the FCU, so retreat safe to the parent.
                if self.inner.safe_l2_block.load(Ordering::Acquire) >= from_block {
                    // Safe cannot retreat below finalized (finalized ahead of safe is
                    // an invalid forkchoice), so refuse loudly here (invariant 7).
                    let finalized_l2 = self.inner.l1_head.finalized_l2();
                    if from_block <= finalized_l2 {
                        return Err(DeriverError::local_diverged_with_msg(
                            from_block,
                            &format!(
                                "resumed batch would rewrite Sync block {from_block} at or below \
                                 the L1-finalized L2 head {finalized_l2}"
                            ),
                        ));
                    }
                    let parent = self.l2_sealed_header_at(from_block - 1)?;
                    // Only safe retreats; finalized tracks L1 finality, which a
                    // resume never reaches.
                    let finalized_hash = self.l2_hash_at(finalized_l2)?;
                    self.inner
                        .committer
                        .advance_safe_finalized(parent, finalized_hash)
                        .await?;
                    self.inner
                        .safe_l2_block
                        .store(from_block - 1, Ordering::Release);
                    event!(
                        name: "eez.deriver.safe.retreated_for_resume",
                        Level::WARN,
                        l2_block = from_block,
                        "safe retreated to the parent for a same-height resumed replacement (finalized unchanged)",
                    );
                }
                block_txs.extend(new_content);
                let outcome = self
                    .replay_block(from_block - 1, &block_txs, terminal_inputs)
                    .await?;
                // No later block follows this one to make it canonical, so
                // reading by number can still return the old block. Only an
                // issue here; a forward replay is extended by the next commit.
                sync_block_hash = Some(outcome.block_hash);
                replayed = 1;
            }
        } else {
            for (position, plan) in payload.blocks().enumerate() {
                let l2_block = from_block + position as u64;
                let is_sync_block = position as u32 == last_index;
                let mut block_txs = plan
                    .transactions
                    .map(Bytes::copy_from_slice)
                    .collect::<Vec<_>>();
                let environment =
                    self.derived_environment(l2_block - 1, plan.beneficiary, plan.extra_data)?;
                let inputs = ExecutionInputs::Derived(&environment);
                if is_sync_block && let Some(plans) = &sync_block_txs {
                    let parent = self.l2_sealed_header_at(l2_block - 1)?;
                    let (_, _, suffix) = self.execute_block_with_plans(
                        &parent,
                        &block_txs,
                        plans,
                        &sync_sources,
                        inputs,
                    )?;
                    block_txs.extend(suffix);
                }
                self.validate_execution_inputs(l2_block - 1, inputs, &block_txs)?;
                let matched = if stale_boundary {
                    false
                } else {
                    self.local_block_matches(l2_block, &block_txs, inputs)?
                };
                let should_replay = suffix_replay.required(matched);
                event!(
                    name: "eez.deriver.reconcile.block",
                    Level::DEBUG,
                    l1_block_number,
                    tx_hash = %tx_hash,
                    l2_block,
                    action = if should_replay { "replay" } else { "skip" },
                    tx_count = block_txs.len(),
                    replayed_so_far = replayed,
                    "reconciling batch block",
                );
                if !should_replay {
                    continue;
                }
                let outcome = self.replay_block(l2_block - 1, &block_txs, inputs).await?;
                if is_sync_block {
                    sync_block_hash = Some(outcome.block_hash);
                }
                replayed += 1;
            }
        }

        // L2-origin authorization gate (trace binding): every call to L1 in a
        // producing settlement table must match a real `CrossChainCallExecuted`
        // event this Sync block emitted on re-execution, including calls nested
        // under inbound roots. A phantom has no match. Runs post-replay because
        // the receipt events exist only after commit.
        // See `eez_protocol::outbound_gate` + docs/OUTBOUND-VIA-WRAPPER-GATE.md.
        if gate_should_verify {
            let cfg = self
                .inner
                .system_tx_cfg
                .as_ref()
                .expect("L2-origin gate only populated under system_tx_cfg = Some");
            let to_block = if resumed {
                from_block
            } else {
                from_block + u64::from(last_index)
            };
            // By hash after a replay: the number still resolves to the superseded
            // block until persistence catches up.
            let source: alloy_eips::BlockHashOrNumber =
                sync_block_hash.map_or_else(|| to_block.into(), Into::into);
            let observed =
                self.observed_outbound_calls(source, cfg.eezl2_address, gate_first_transaction)?;
            eez_protocol::outbound_gate::verify_expected_outbound_hashes(
                &gate_expected_hashes,
                &observed,
            )
            .map_err(|e| {
                DeriverError::local_diverged_with_msg(
                    from_block,
                    &format!("outbound authorization gate failed (tx={tx_hash}): {e}"),
                )
            })?;
        }
        Ok((replayed, sync_block_hash, reconcile_guard, l1))
    }

    fn derived_environment(
        &self,
        parent_number: u64,
        beneficiary: Address,
        extra_data: &[u8],
    ) -> DeriverResult<eez_driver::derivation::Environment> {
        // Validate the predecessor now; each reconstructed block selects its
        // own scheduled source through the shared EVM configuration.
        self.l2_sealed_header_at(parent_number)?;
        Ok(eez_driver::derivation::Environment {
            fee_recipient: beneficiary,
            extra_data: Bytes::copy_from_slice(extra_data),
        })
    }

    fn validate_execution_inputs(
        &self,
        parent_number: u64,
        inputs: ExecutionInputs<'_>,
        raw_txs: &[Bytes],
    ) -> DeriverResult<()> {
        let parent = self.l2_sealed_header_at(parent_number)?;
        let attributes = inputs.attributes(&self.inner.evm_config, &parent, raw_txs)?;
        let (_, timestamp) = self
            .inner
            .derivation
            .schedule()
            .next(&parent)
            .map_err(DeriverError::l2_provider)?;
        let state = self
            .inner
            .l2_provider
            .state_by_block_hash(parent.hash())
            .map_err(DeriverError::l2_provider)?;
        let mut db = StateProviderDatabase::new(EvmStateProviderAdapter(state.as_ref()));
        let first = raw_txs
            .first()
            .map(|raw| TransactionSigned::decode_2718_exact(raw))
            .transpose()
            .map_err(DeriverError::l2_provider)?;
        let transition = self
            .inner
            .evm_config
            .validate_history_input(&mut db, &parent, timestamp, first.as_ref())
            .map_err(DeriverError::l2_provider)?;
        if attributes.timestamp != timestamp || attributes.prev_randao != transition.prev_randao {
            return Err(DeriverError::l2_provider(
                "execution inputs differ from authenticated time/origin",
            ));
        }
        Ok(())
    }

    fn local_block_matches(
        &self,
        number: u64,
        expected_txs: &[Bytes],
        inputs: ExecutionInputs<'_>,
    ) -> DeriverResult<bool> {
        let Some(local) = self
            .inner
            .l2_provider
            .block_by_number(number)
            .map_err(DeriverError::l2_provider)?
        else {
            return Ok(false);
        };
        if local.body().transactions().len() != expected_txs.len()
            || local
                .body()
                .transactions()
                .iter()
                .zip(expected_txs)
                .any(|(actual, expected)| actual.encoded_2718().as_slice() != expected.as_ref())
        {
            return Ok(false);
        }
        let block = alloy_rlp::decode_exact::<EthereumBlock>(&alloy_rlp::encode(local))
            .map_err(DeriverError::l2_provider)?;
        let parent_number = number
            .checked_sub(1)
            .ok_or_else(|| DeriverError::l2_provider("cannot derive genesis"))?;
        let parent = self.l2_sealed_header_at(parent_number)?;
        Ok(
            inputs.matches_local(&self.inner.evm_config, &parent, &block)?
                && self.history_matches(&SealedHeader::seal_slow(block.header().clone()))?,
        )
    }

    /// Outbound calls emitted by the L2 manager from `first_transaction` onward.
    ///
    /// # Errors
    /// [`DeriverError::l2_provider`] if the block's receipts are missing locally.
    fn observed_outbound_calls(
        &self,
        block: alloy_eips::BlockHashOrNumber,
        eez_l2: Address,
        first_transaction: usize,
    ) -> DeriverResult<Vec<OutboundCallObservation>> {
        let receipts = self
            .inner
            .l2_provider
            .receipts_by_block(block)
            .map_err(DeriverError::l2_provider)?
            .ok_or_else(|| {
                DeriverError::l2_provider(format!("local receipts for Sync block {block} missing"))
            })?;
        extract_outbound_call_observations(&receipts, eez_l2, first_transaction)
    }

    /// Loud-fail if the batch's claimed commitment chain disagrees with
    /// our STF's actual L2 block hashes at the batch boundaries:
    ///
    /// - `claimed_current_root` (first root update's `currentRoot`) vs the
    ///   local block hash at `from_block - 1`.
    /// - `claimed_new_root` (last root update's `newRoot`) vs the local
    ///   block hash at `to_block`.
    ///
    /// Both ends are checked — the composer chains deltas across entries, so
    /// checking one would let a crafted chain pass. Halting here surfaces a
    /// mismatch at its origin rather than at our next post's
    /// `RootMismatch`.
    ///
    /// `entry_root` is [`eez_l1::Settlement::entry_state`], not the claimed chain
    /// head — the claimed head would contradict the cursor guard on a mid-chain resume.
    fn check_claimed_state(
        &self,
        entry_root: Option<B256>,
        claimed_new_root: Option<B256>,
        // Block the applied run started from. The endpoint is in
        // `[anchor, to_block]`, anchor included.
        anchor: u64,
        from_block: u64,
        to_block: u64,
        // Hash of the block replayed at `to_block`. Needed only when a
        // resumed batch rewrote the tip; see `sync_block_hash`.
        to_block_hash: Option<B256>,
        l1_block_number: u64,
        tx_hash: B256,
    ) -> DeriverResult<Option<u64>> {
        if let Some(claimed_curr) = entry_root {
            let pre = from_block.saturating_sub(1);
            let local_pre = self
                .inner
                .l2_provider
                .sealed_header(pre)
                .map_err(DeriverError::l2_provider)?
                .ok_or_else(|| {
                    DeriverError::l2_provider(format!("local L2 header at {pre} missing"))
                })?
                .hash();
            if local_pre != claimed_curr {
                event!(
                    name: "eez.deriver.state.diverged_pre",
                    Level::ERROR,
                    event_name = "eez.deriver.state.diverged_pre",
                    l1_block_number,
                    tx_hash = %tx_hash,
                    pre_block = pre,
                    local_root = %local_pre,
                    claimed = %claimed_curr,
                    "local L2 block hash at from_block-1 differs from the commitment the batch's applied run started from",
                );
                return Err(DeriverError::local_diverged(pre));
            }
        }
        // A partial settlement stops early, so find the block carrying the
        // settled block hash rather than assuming `to_block`. It becomes the cursor.
        if let Some(claimed_new) = claimed_new_root {
            // L1 can only apply a PREFIX of the claimed chain, so the settled
            // endpoint is at or below the range end — never above our tip.
            let tip = self.inner.committer.last_header().number();
            if to_block > tip {
                event!(
                    name: "eez.deriver.state.endpoint_above_tip",
                    Level::ERROR,
                    l1_block_number,
                    tx_hash = %tx_hash,
                    to_block,
                    tip,
                    "batch claims a range ending above the local tip; replay did not reach it",
                );
                return Err(DeriverError::local_diverged(to_block));
            }
            // A depth, not a block count. `from_block` is `anchor + 1` when
            // fresh and `anchor` when resumed, so measure from the anchor.
            let window = to_block.saturating_sub(anchor);
            if let Some(settled_end) = find_batch_anchor(to_block, claimed_new, window, |block| {
                self.l2_hash_at(block)
            })? {
                if settled_end != to_block {
                    event!(
                        name: "eez.deriver.state.settled_prefix",
                        Level::INFO,
                        l1_block_number,
                        tx_hash = %tx_hash,
                        to_block,
                        settled_end,
                        "L1 settled a prefix; the cursor follows its endpoint, not the claimed range end",
                    );
                }
                return Ok(Some(settled_end));
            }
            let local_post = if let Some(hash) = to_block_hash {
                hash
            } else {
                self.inner
                    .l2_provider
                    .sealed_header(to_block)
                    .map_err(DeriverError::l2_provider)?
                    .ok_or_else(|| {
                        DeriverError::l2_provider(format!("local L2 header at {to_block} missing"))
                    })?
                    .hash()
            };
            if local_post != claimed_new {
                event!(
                    name: "eez.deriver.state.diverged_post",
                    Level::ERROR,
                    event_name = "eez.deriver.state.diverged_post",
                    l1_block_number,
                    tx_hash = %tx_hash,
                    to_block,
                    local_root = %local_post,
                    claimed = %claimed_new,
                    "local L2 block hash at to_block differs from batch's claimed newRoot commitment",
                );
                return Err(DeriverError::local_diverged(to_block));
            }
            return Ok(Some(to_block));
        }
        Ok(None)
    }
}

/// [`find_batch_anchor`] plus the overlap log. Free fn so a test can drive it
/// with a closure and assert the event, with no live `Deriver`.
///
/// # Errors
/// Propagates `root_at` failures.
fn anchor_with_overlap_log(
    cursor: u64,
    entry_state: B256,
    window: u64,
    root_at: impl FnMut(u64) -> DeriverResult<B256>,
) -> DeriverResult<Option<u64>> {
    let anchor = find_batch_anchor(cursor, entry_state, window, root_at)?;
    // Overlapping range: a competing composer's batch starts below our cursor.
    if let Some(anchor) = anchor
        && anchor != cursor
    {
        event!(
            name: "eez.deriver.batch.anchored_below_cursor",
            Level::INFO,
            cursor,
            anchor,
            "batch anchored below the cursor; its range overlaps ours",
        );
    }
    Ok(anchor)
}

/// Walks back from `cursor` for the block whose root is `entry_state`. `None`
/// means the batch starts outside the window, which the caller treats as loud.
fn find_batch_anchor(
    cursor: u64,
    entry_state: B256,
    window: u64,
    mut root_at: impl FnMut(u64) -> DeriverResult<B256>,
) -> DeriverResult<Option<u64>> {
    let floor = cursor.saturating_sub(window);
    let mut block = cursor;
    loop {
        if root_at(block)? == entry_state {
            return Ok(Some(block));
        }
        if block == floor {
            return Ok(None);
        }
        block -= 1;
    }
}

/// L2 heights a batch's settled part covers. A resumed batch reuses the Sync
/// block at `cumulative_l2`; a fresh height would re-apply EIP-2935/4788.
/// The canonical hash at a scan's completion height. A source that does not
/// serve the height yet is incomplete, not invalid: an embedded L1 that has
/// just restarted can briefly report a lower head. Boot catch-up retries
/// [`DeriverError::is_source_incomplete`]; the live loop resyncs on any error.
fn scan_completion_hash(number: u64, hash: Option<B256>) -> DeriverResult<B256> {
    hash.ok_or_else(|| {
        DeriverError::l1_scan(eez_l1::L1Error::SourceIncomplete {
            block: number,
            tx_hash: B256::ZERO,
            detail: "scan completion block not served by the L1 source yet".into(),
        })
    })
}

const fn batch_l2_range(cumulative_l2: u64, resumed: bool, claimed_block_count: u64) -> (u64, u64) {
    if resumed {
        (cumulative_l2, cumulative_l2)
    } else {
        (cumulative_l2 + 1, cumulative_l2 + claimed_block_count)
    }
}

/// Takes `entry_state` (batch not applied yet) or `final_state` (already applied,
/// as a re-scan sees). Re-derivation is idempotent; any other root is divergence.
fn cursor_root_accepted(local_root: B256, entry_state: B256, final_state: Option<B256>) -> bool {
    local_root == entry_state || final_state == Some(local_root)
}

/// `true` iff the first local block in a batch is anchored to the current
/// local parent. `false` if the block is missing or sits on stale ancestry.
fn local_batch_boundary_matches<L2>(l2_provider: &Arc<L2>, from_block: u64) -> DeriverResult<bool>
where
    L2: BlockReader<Header = alloy_consensus::Header>,
{
    let Some(local_block) = l2_provider
        .block_by_number(from_block)
        .map_err(DeriverError::l2_provider)?
    else {
        return Ok(false);
    };
    let parent_block = from_block.checked_sub(1).ok_or_else(|| {
        DeriverError::l2_provider("cannot reconcile a batch starting at genesis block")
    })?;
    let expected_parent_hash = l2_provider
        .sealed_header(parent_block)
        .map_err(DeriverError::l2_provider)?
        .ok_or_else(|| {
            DeriverError::l2_provider(format!("local L2 header at {parent_block} missing"))
        })?
        .hash();

    Ok(local_block.header().parent_hash == expected_parent_hash)
}

/// Once one block in a batch must be replayed, every descendant in that batch
/// must be rebuilt on the new parent even when its transaction list matches.
#[derive(Debug, Clone, Copy)]
struct SuffixReplay {
    active: bool,
}

impl SuffixReplay {
    const fn new(stale_boundary: bool) -> Self {
        Self {
            active: stale_boundary,
        }
    }

    const fn required(&mut self, local_matches: bool) -> bool {
        self.active |= !local_matches;
        self.active
    }
}

#[cfg(test)]
mod scan_completion_tests {
    use super::*;

    #[test]
    fn unserved_scan_completion_block_is_retryable_source_incompleteness() {
        let error = scan_completion_hash(42, None).unwrap_err();
        assert!(error.is_source_incomplete(), "{error}");
        assert!(!error.is_l1_transport());
        let hash = B256::with_last_byte(7);
        assert_eq!(scan_completion_hash(42, Some(hash)).unwrap(), hash);
    }
}

#[cfg(test)]
mod suffix_replay_tests {
    use super::SuffixReplay;

    fn decisions(stale_boundary: bool, local_matches: &[bool]) -> Vec<bool> {
        let mut suffix = SuffixReplay::new(stale_boundary);
        local_matches
            .iter()
            .map(|matches| suffix.required(*matches))
            .collect()
    }

    #[test]
    fn mismatch_replays_every_later_descendant() {
        assert_eq!(
            decisions(false, &[true, false, true, true]),
            [false, true, true, true],
        );
    }

    #[test]
    fn stale_boundary_replays_the_complete_batch() {
        assert_eq!(decisions(true, &[true, true, true]), [true, true, true]);
    }
}

/// Which producing entries L1 ran, projected onto the partitioned
/// `[outbound…, inbound…]` list (that concatenation is the claimed chain minus
/// the anchor). A `skip` per list, not just a length: L1 resumes MID-CHAIN when
/// a competing same-block batch already made the leading hops, so those entries
/// drop from the FRONT — which prefix truncation cannot express.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProducingSlice {
    outbound_skip: usize,
    outbound_take: usize,
    inbound_skip: usize,
    inbound_take: usize,
}

impl ProducingSlice {
    /// Project `settlement`'s producing run onto lists of the given lengths.
    fn split(settlement: eez_l1::Settlement, outbound_len: usize, inbound_len: usize) -> Self {
        let (skip, take) = settlement.producing_slice();
        let outbound_skip = skip.min(outbound_len);
        let outbound_take = take.min(outbound_len - outbound_skip);
        // Any skip beyond the outbound list falls through into inbound.
        let inbound_skip = skip.saturating_sub(outbound_skip).min(inbound_len);
        let inbound_take = take
            .saturating_sub(outbound_take)
            .min(inbound_len - inbound_skip);
        Self {
            outbound_skip,
            outbound_take,
            inbound_skip,
            inbound_take,
        }
    }

    /// True when entries are left over at the TAIL (partial consumption). A pure
    /// mid-chain resume does NOT trip this — front skips are counted in `*_skip`.
    const fn leaves_entries_unconsumed(&self, outbound_len: usize, inbound_len: usize) -> bool {
        self.outbound_skip + self.outbound_take < outbound_len
            || self.inbound_skip + self.inbound_take < inbound_len
    }
}

/// Map settlement hosts to whole signed outbound groups. Used both by replay
/// and by evidence reconstructed from a previously validated correlation.
fn selected_replay_groups(
    settlement: eez_l1::Settlement,
    host_roots: &[usize],
    group_roots: &[usize],
    inbound_len: usize,
) -> Result<eez_l1::ReplaySelection, String> {
    let sum = |counts: &[usize]| {
        counts
            .iter()
            .try_fold(0usize, |sum, count| sum.checked_add(*count))
            .ok_or_else(|| "outbound root count overflow".to_string())
    };
    if host_roots.contains(&0) || group_roots.contains(&0) || sum(host_roots)? != sum(group_roots)?
    {
        return Err("outbound hosts and signed groups do not cover the same roots".into());
    }
    let (skip, take) = settlement.producing_slice();
    let total = host_roots
        .len()
        .checked_add(inbound_len)
        .ok_or("effect count overflow")?;
    if skip.checked_add(take).is_none_or(|end| end > total) {
        return Err("settlement exceeds available producing effects".into());
    }
    let slice = ProducingSlice::split(settlement, host_roots.len(), inbound_len);
    let start = sum(&host_roots[..slice.outbound_skip])?;
    let end = sum(&host_roots[..slice.outbound_skip + slice.outbound_take])?;
    let boundary = |root: usize| {
        let mut position = 0usize;
        if root == 0 {
            return Some(0);
        }
        for (index, size) in group_roots.iter().enumerate() {
            position += size; // bounded by the checked total above
            if position == root {
                return Some(index + 1);
            }
            if position > root {
                return None;
            }
        }
        None
    };
    let (Some(start), Some(end)) = (boundary(start), boundary(end)) else {
        return Err("partial L1 settlement splits one multi-root outbound user transaction".into());
    };
    Ok(eez_l1::ReplaySelection {
        outbound_groups: start..end,
        inbound_effects: slice.inbound_skip..slice.inbound_skip + slice.inbound_take,
    })
}

/// Decode outbound-call events emitted by the configured L2 manager.
fn extract_outbound_call_observations<R>(
    receipts: &[R],
    eez_l2: Address,
    first_transaction: usize,
) -> DeriverResult<Vec<OutboundCallObservation>>
where
    R: alloy_consensus::TxReceipt<Log = alloy_primitives::Log>,
{
    let receipts = receipts.get(first_transaction..).ok_or_else(|| {
        DeriverError::l2_provider(format!(
            "outbound authorization starts at transaction {first_transaction}, but only {} receipts exist",
            receipts.len(),
        ))
    })?;
    let logs: Vec<alloy_primitives::Log> = receipts
        .iter()
        .flat_map(alloy_consensus::TxReceipt::logs)
        .cloned()
        .collect();
    Ok(eez_protocol::outbound_gate::observations_from_logs(
        &logs, eez_l2,
    ))
}

#[cfg(test)]
mod producing_slice_tests {
    //! Two composers post into ONE L1 block: `pb1` claims `A→B` and lands, so
    //! `pb2`'s leading step(s) are refused as redundant and L1 resumes MID-CHAIN
    //! inside it. The deriver must rebuild exactly the steps that ran.

    use super::ProducingSlice;
    use alloy_primitives::B256;
    use eez_l1::Settlement;

    #[test]
    fn replay_selection_requires_whole_outbound_groups_and_bounded_effects() {
        use super::selected_replay_groups;
        // The first signed user has two hosts and cannot be replayed halfway.
        assert!(selected_replay_groups(settlement(0, 2), &[1, 1, 1], &[2, 1], 2).is_err());
        assert!(selected_replay_groups(settlement(2, 2), &[1, 1, 1], &[2, 1], 2).is_err());
        let prefix = selected_replay_groups(settlement(0, 3), &[1, 1, 1], &[2, 1], 2).unwrap();
        assert_eq!(prefix.outbound_groups, 0..1);
        assert_eq!(prefix.inbound_effects, 0..0);
        let suffix = selected_replay_groups(settlement(3, 3), &[1, 1, 1], &[2, 1], 2).unwrap();
        assert_eq!(suffix.outbound_groups, 1..2);
        assert_eq!(suffix.inbound_effects, 0..2);
        // One carrier can account for several roots without extra hosts.
        let carrier = selected_replay_groups(settlement(0, 2), &[3], &[3], 0).unwrap();
        assert_eq!(carrier.outbound_groups, 0..1);
        for (hosts, groups) in [
            (vec![0], vec![0]),
            (vec![2], vec![1]),
            (vec![usize::MAX, 1], vec![1]),
        ] {
            assert!(selected_replay_groups(settlement(0, 1), &hosts, &groups, 0).is_err());
        }
        assert!(selected_replay_groups(settlement(0, 9), &[1], &[1], 2).is_err());
        assert!(selected_replay_groups(settlement(usize::MAX, usize::MAX), &[1], &[1], 2).is_err());
    }

    /// Cursor guard and `check_claimed_state` must agree on entry root, else a
    /// mid-chain resume clears one and fails the other.
    #[test]
    fn state_check_entry_root_is_not_the_claimed_chain_head() {
        let (a, b) = (B256::repeat_byte(0x0A), B256::repeat_byte(0x0B));
        let claimed_head = Some(a);

        // Mid-chain resume: B is what both guards check.
        let resumed = Settlement {
            start: 1,
            len: 1,
            final_state: None,
            entry_state: Some(b),
        };
        assert_eq!(resumed.entry_state.or(claimed_head), Some(b));
        assert_ne!(resumed.entry_state.or(claimed_head), claimed_head);

        // Uncontested: unchanged.
        let plain = Settlement {
            start: 0,
            len: 1,
            final_state: None,
            entry_state: claimed_head,
        };
        assert_eq!(plain.entry_state.or(claimed_head), claimed_head);
    }

    /// The superseded reconstruction: keep `settled_count - 1` entries from the
    /// FRONT (outbound first, then inbound). Its skips are structurally 0 — the
    /// defect. Shaped as `(outbound_skip, outbound_take, inbound_skip, inbound_take)` to compare.
    fn old_prefix_split(
        settled_count: usize,
        outbound_len: usize,
        inbound_len: usize,
    ) -> (usize, usize, usize, usize) {
        let applied = settled_count.saturating_sub(1);
        let applied_outbound = applied.min(outbound_len);
        let consumed_inbound = (applied - applied_outbound).min(inbound_len);
        (0, applied_outbound, 0, consumed_inbound)
    }

    fn settlement(start: usize, len: usize) -> Settlement {
        Settlement {
            start,
            len,
            final_state: None,
            entry_state: None,
        }
    }

    /// Anchor skipped, one producing step ran → rebuild that one, not zero.
    #[test]
    fn second_batch_rebuilds_the_single_step_that_ran() {
        let s = ProducingSlice::split(settlement(1, 1), 0, 1);
        assert_eq!(
            s,
            ProducingSlice {
                outbound_skip: 0,
                outbound_take: 0,
                inbound_skip: 0,
                inbound_take: 1
            },
        );
        assert!(!s.leaves_entries_unconsumed(0, 1));
    }

    /// Competitor made the anchor AND the first producing hop, so L1 resumed at
    /// the second. The prefix formula keeps the already-settled entry and drops
    /// the one that ran — rebuilding a different tx than L1 executed.
    #[test]
    fn deeper_resume_skips_the_front_where_prefix_truncation_cannot() {
        // claimed [anchor, p0, p1]; competitor made anchor + p0; only p1 ran.
        let s = ProducingSlice::split(settlement(2, 1), 0, 2);
        assert_eq!(
            s,
            ProducingSlice {
                outbound_skip: 0,
                outbound_take: 0,
                inbound_skip: 1,
                inbound_take: 1,
            },
        );
        // Front skips are counted, so nothing reads as left over.
        assert!(!s.leaves_entries_unconsumed(0, 2));

        let old = old_prefix_split(1, 0, 2);
        assert_eq!(old, (0, 0, 0, 0), "prefix formula rebuilds nothing here");
        assert_ne!((old.2, old.3), (s.inbound_skip, s.inbound_take));
    }

    /// A skip longer than the outbound list falls through into inbound.
    #[test]
    fn skip_and_take_span_outbound_into_inbound() {
        let s = ProducingSlice::split(settlement(3, 2), 2, 3);
        assert_eq!((s.outbound_skip, s.outbound_take), (2, 0));
        assert_eq!((s.inbound_skip, s.inbound_take), (0, 2));
    }

    /// Anchor ran: producing entries are `len - 1`, nothing skipped. The ordinary
    /// single-composer path must derive exactly as the prefix formula did.
    #[test]
    fn anchor_applied_matches_the_prefix_formula() {
        let s = ProducingSlice::split(settlement(0, 3), 1, 1);
        assert_eq!(s.outbound_skip, 0);
        assert_eq!(s.inbound_skip, 0);
        assert_eq!((s.outbound_take, s.inbound_take), (1, 1));
        let old = old_prefix_split(3, 1, 1);
        assert_eq!((old.1, old.3), (s.outbound_take, s.inbound_take));
    }

    /// L1 stopped SHORT (a reverting user tx left its entry and the rest
    /// unconsumed) — truncate the tail, no skip.
    #[test]
    fn stopping_short_truncates_the_tail() {
        let s = ProducingSlice::split(settlement(0, 2), 0, 3);
        assert_eq!((s.inbound_skip, s.inbound_take), (0, 1));
        assert!(s.leaves_entries_unconsumed(0, 3));
    }

    /// A run longer than the available entries clamps instead of panicking.
    #[test]
    fn oversized_run_clamps_to_available_entries() {
        let s = ProducingSlice::split(settlement(0, 99), 1, 1);
        assert_eq!((s.outbound_take, s.inbound_take), (1, 1));
        let s = ProducingSlice::split(settlement(50, 99), 1, 1);
        assert_eq!(s.outbound_skip + s.outbound_take, 1);
        assert_eq!(s.inbound_skip + s.inbound_take, 1);
    }

    /// The trailing-append boundary must skip unconsumed tail entries too, not
    /// just `outbound_skip + outbound_take`.
    #[test]
    fn tail_truncation_boundary_must_skip_the_unconsumed_entry_too() {
        let original_outbound_len = 2; // [E0, E1]
        let s = ProducingSlice::split(settlement(0, 2), original_outbound_len, 0);
        assert_eq!((s.outbound_skip, s.outbound_take), (0, 1));
        assert!(s.leaves_entries_unconsumed(original_outbound_len, 0));

        let buggy_boundary = s.outbound_skip + s.outbound_take;
        assert_ne!(buggy_boundary, original_outbound_len);
    }

    /// Two composers built from cursor 100, both claiming 101..=110: batch 1
    /// (plain) spans that full range, advancing the cursor to 110; batch 2
    /// (resumed) settles within that SAME Sync block, not a new one.
    #[test]
    fn resumed_batch_settles_within_the_existing_sync_block_not_a_new_one() {
        let (first, last) = super::batch_l2_range(100, settlement(0, 2).start > 0, 10);
        assert_eq!((first, last), (101, 110));

        let (first, last) = super::batch_l2_range(110, settlement(2, 1).start > 0, 10);
        assert_eq!((first, last), (110, 110));
    }
}

#[cfg(test)]
mod outbound_wiring_tests {
    //! Wiring + attack-surface tests for the outbound authorization path: the
    //! event extraction ([`extract_outbound_call_observations`]) and its composition
    //! with [`eez_protocol::outbound_gate::verify_outbound_authorized`]. The pure gate
    //! logic is unit-tested in `eez-protocol`; here we exercise the DERIVER-side wiring
    //! — the address + event-signature filters that decide which events authorize
    //! — and the accept/reject decisions on synthetic receipts.

    use super::extract_outbound_call_observations;
    use alloy_consensus::Receipt;
    use alloy_primitives::{Address, B256, Bytes, Log, U256, address};
    use alloy_sol_types::SolEvent;
    use eez_protocol::RollupId;
    use eez_protocol::abi::eez_l2_events::CrossChainCallExecuted;
    use eez_protocol::abi::{ExecutionEntrySol, L2ToL1CallSol};
    use eez_protocol::action::{CallHashInput, l2_outbound_call_hash};
    use eez_protocol::outbound_gate::{OutboundCallObservation, verify_outbound_authorized};

    const EEZL2: Address = address!("4B2760F84B83d6b3829E30A7573B0172B6d6402C");
    const OTHER: Address = address!("00000000000000000000000000000000deadbeef");
    const L2_RID: u64 = 1;

    /// A canonical `CrossChainCallExecuted` log from `addr`.
    fn cc_log(addr: Address, call_hash: B256) -> Log {
        Log {
            address: addr,
            data: CrossChainCallExecuted {
                crossChainCallHash: call_hash,
                proxy: Address::ZERO,
                sourceAddress: Address::ZERO,
                callData: Bytes::new(),
                value: U256::ZERO,
                callGas: 0,
            }
            .encode_log_data(),
        }
    }

    fn receipt(logs: Vec<Log>) -> Receipt {
        Receipt {
            status: true.into(),
            cumulative_gas_used: 0,
            logs,
        }
    }

    fn outbound_call(source: Address, target: Address, value: u64, data: &[u8]) -> L2ToL1CallSol {
        L2ToL1CallSol {
            revertNextNCalls: 0,
            isStatic: false,
            gas: 0,
            sourceAddress: source,
            sourceRollupId: L2_RID,
            targetAddress: target,
            value: U256::from(value),
            data: Bytes::from(data.to_vec()),
        }
    }

    fn outbound_entry(call: L2ToL1CallSol) -> ExecutionEntrySol {
        ExecutionEntrySol {
            rootUpdates: Vec::new(),
            proxyEntryHash: B256::ZERO, // outbound immediate
            l2ToL1Calls: vec![call],
            expectedL1ToL2Calls: Vec::new(),
            rollingHash: B256::ZERO,
            destinationRollupId: L2_RID,
            success: true,
            returnData: Bytes::new(),
        }
    }

    /// The topic1 `EEZL2` emits for `call` on this L2 (`targetRollupId` =
    /// MAINNET(0), `sourceRollupId` = `L2_RID`) — what the gate recomputes.
    fn call_hash(call: &L2ToL1CallSol) -> B256 {
        l2_outbound_call_hash(
            CallHashInput {
                call_mode: eez_protocol::CallMode::Mutable,
                source_address: call.sourceAddress,
                source_rollup_id: RollupId(L2_RID),
                target_address: call.targetAddress,
                target_rollup_id: RollupId::MAINNET,
                value: call.value,
                data: &call.data,
            },
            0,
        )
    }

    fn eoa() -> Address {
        address!("00000000000000000000000000000000000000aa")
    }
    fn l1_target() -> Address {
        address!("dc64a140aa3e981100a9beca4e685f962f0cf6c9")
    }

    // ── extraction filters ──────────────────────────────────────────────

    #[test]
    fn extract_picks_eezl2_events_and_preserves_multiset() {
        let h1 = B256::repeat_byte(0x11);
        let h2 = B256::repeat_byte(0x22);
        // Empty receipts (reverted txs) and topicless logs carry no hash — ignored.
        let bare = Log::new_unchecked(EEZL2, Vec::new(), Bytes::new());
        let receipts = vec![
            receipt(vec![cc_log(EEZL2, h1), cc_log(EEZL2, h2)]),
            receipt(vec![cc_log(EEZL2, h1)]), // duplicate h1 → multiset keeps both
            receipt(vec![]),                  // reverted tx → no logs
            receipt(vec![bare]),              // topicless log → no hash
        ];
        assert_eq!(
            extract_outbound_call_observations(&receipts, EEZL2, 0).unwrap(),
            vec![
                OutboundCallObservation::new(h1, 0),
                OutboundCallObservation::new(h2, 0),
                OutboundCallObservation::new(h1, 0),
            ]
        );
    }

    // ── extraction ∘ gate: accept + attack rejections ───────────────────

    #[test]
    fn resumed_gate_checks_only_new_transactions_without_weakening_authorization() {
        use eez_protocol::outbound_gate::verify_expected_outbound_hashes;

        let prefix = B256::repeat_byte(1);
        let suffix = B256::repeat_byte(2);
        // The retained prefix's user and system receipts are already authorized.
        // A fresh append and an idempotent replay must select the same suffix.
        let receipts = vec![
            receipt(vec![cc_log(EEZL2, prefix)]),
            receipt(vec![]),
            receipt(vec![cc_log(EEZL2, suffix)]),
        ];
        let observed = extract_outbound_call_observations(&receipts, EEZL2, 2).unwrap();
        assert!(verify_expected_outbound_hashes(&[suffix], &observed).is_ok());
        assert!(verify_expected_outbound_hashes(&[prefix], &observed).is_err());
        assert!(verify_expected_outbound_hashes(&[], &observed).is_err());
        assert!(verify_expected_outbound_hashes(&[suffix, suffix], &observed).is_err());
        // An invalid receipt boundary fails closed rather than authorizing an
        // empty table or silently looking at unrelated transactions.
        assert!(extract_outbound_call_observations(&receipts, EEZL2, 4).is_err());
        assert!(
            extract_outbound_call_observations(&receipts, EEZL2, 3)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn wiring_accepts_contract_source_wrapper() {
        // Outbound-via-wrapper end to end through extraction: source is a CONTRACT.
        let wrapper = address!("cccccccccccccccccccccccccccccccccccccccc");
        let call = outbound_call(wrapper, l1_target(), 42, &[0xab]);
        let receipts = vec![receipt(vec![cc_log(EEZL2, call_hash(&call))])];
        let observed = extract_outbound_call_observations(&receipts, EEZL2, 0).unwrap();
        assert!(
            verify_outbound_authorized(&[outbound_entry(call)], &observed, L2_RID).is_ok(),
            "a contract-initiated (wrapper) outbound must be accepted"
        );
    }

    #[test]
    fn wiring_rejects_spoofed_foreign_event() {
        // ATTACK: the only event with the matching hash is emitted by a foreign
        // address; extraction drops it, so the gate sees a phantom.
        let call = outbound_call(eoa(), l1_target(), 7, &[0x12]);
        let receipts = vec![receipt(vec![cc_log(OTHER, call_hash(&call))])];
        let observed = extract_outbound_call_observations(&receipts, EEZL2, 0).unwrap();
        assert!(verify_outbound_authorized(&[outbound_entry(call)], &observed, L2_RID).is_err());
    }

    #[test]
    fn wiring_rejects_double_count() {
        // ATTACK: two identical settlement entries, one real event → the second is
        // unmatched (multiset consumption).
        let call = outbound_call(eoa(), l1_target(), 7, &[0x12]);
        let entries = vec![outbound_entry(call.clone()), outbound_entry(call.clone())];
        let one = vec![receipt(vec![cc_log(EEZL2, call_hash(&call))])];
        assert!(
            verify_outbound_authorized(
                &entries,
                &extract_outbound_call_observations(&one, EEZL2, 0).unwrap(),
                L2_RID,
            )
            .is_err()
        );
        // …but two events authorize both.
        let two = vec![receipt(vec![
            cc_log(EEZL2, call_hash(&call)),
            cc_log(EEZL2, call_hash(&call)),
        ])];
        assert!(
            verify_outbound_authorized(
                &entries,
                &extract_outbound_call_observations(&two, EEZL2, 0).unwrap(),
                L2_RID,
            )
            .is_ok()
        );
    }
}

#[cfg(test)]
mod batch_anchor_tests {
    //! Two composers on one rollup post OVERLAPPING ranges: B's cursor sits a
    //! block behind A's, so B's batch starts below A's cursor (live 2026-08-24).

    use super::{batch_l2_range, find_batch_anchor};
    use alloy_primitives::B256;

    /// Roots of the live stall: B's batch covered 174192..174197, A's cursor was
    /// 174192, and L1's stored root was block 174197's.
    fn root_at(block: u64) -> B256 {
        B256::with_last_byte(u8::try_from(block % 251).expect("fits"))
    }

    #[test]
    fn overlapping_batch_anchors_below_the_cursor_and_lands_on_the_settled_block() {
        let cursor = 174_192_u64;
        let block_count = 6_u64;
        // B's batch declares the root of 174191 as where its run starts.
        let entry_state = root_at(174_191);

        let anchor = find_batch_anchor(cursor, entry_state, block_count, |b| Ok(root_at(b)))
            .expect("lookup")
            .expect("anchor is inside the window");
        assert_eq!(anchor, 174_191);

        // Anchored: the range is B's real one and ends where L1 stored its root.
        assert_eq!(
            batch_l2_range(anchor, false, block_count),
            (174_192, 174_197)
        );
        // Cursor-relative (the bug): one block too high, so the endpoint check
        // compared L1's stored root against block 174198 and refused.
        assert_eq!(
            batch_l2_range(cursor, false, block_count),
            (174_193, 174_198)
        );
    }

    #[test]
    fn a_partial_settlement_endpoint_is_found_below_the_claimed_range_end() {
        // Live 2026-08-24: the batch claimed 174193..174198, but L1 stopped at
        // the hop whose newState is block 174197's root.
        let claimed_end = 174_198_u64;
        let settled = find_batch_anchor(claimed_end, root_at(174_197), 6, |b| Ok(root_at(b)))
            .expect("lookup")
            .expect("endpoint inside the window");
        assert_eq!(settled, 174_197);
        assert_ne!(settled, claimed_end, "cursor must not run past L1's root");
    }

    #[test]
    fn a_full_settlement_endpoint_is_the_range_end() {
        let claimed_end = 174_198_u64;
        let settled = find_batch_anchor(claimed_end, root_at(claimed_end), 6, |b| Ok(root_at(b)))
            .expect("lookup")
            .expect("endpoint");
        assert_eq!(settled, claimed_end);
    }

    #[test]
    fn disjoint_batch_still_anchors_at_the_cursor() {
        let cursor = 1_000_u64;
        let anchor = find_batch_anchor(cursor, root_at(cursor), 6, |b| Ok(root_at(b)))
            .expect("lookup")
            .expect("anchor");
        assert_eq!(anchor, cursor);
        assert_eq!(batch_l2_range(anchor, false, 6), (1_001, 1_006));
    }

    #[test]
    fn a_root_outside_the_window_stays_loud() {
        let cursor = 1_000_u64;
        let anchor =
            find_batch_anchor(cursor, B256::repeat_byte(0xEE), 6, |b| Ok(root_at(b))).expect("ok");
        assert!(anchor.is_none(), "unknown root must not be anchored");
    }

    /// Endpoint depth is measured from the anchor, not `from_block`, which is
    /// `anchor + 1` when fresh and `anchor` when resumed. Both from-relative
    /// forms are wrong in opposite directions.
    ///
    /// The anchor is a legitimate endpoint: a single-block batch's leading
    /// immediate can be a no-op (`newRoot == currentRoot == hash(anchor)`), so
    /// L1 can accept it and leave the block-hash commitment at the anchor.
    #[test]
    fn endpoint_depth_is_measured_from_the_anchor() {
        for count in [1_u64, 6] {
            let anchor = 174_192_u64;
            let (from, to) = batch_l2_range(anchor, false, count);
            let probe = |block| Ok(root_at(block));

            assert_eq!(
                find_batch_anchor(to, root_at(anchor), to - anchor, probe).expect("lookup"),
                Some(anchor),
                "count={count}: the anchor must stay reachable",
            );
            assert!(
                find_batch_anchor(to, root_at(anchor), to - from, probe)
                    .expect("lookup")
                    .is_none(),
                "count={count}: from-relative depth wrongly excludes the anchor",
            );
            for endpoint in from..=to {
                assert_eq!(
                    find_batch_anchor(to, root_at(endpoint), to - anchor, probe).expect("lookup"),
                    Some(endpoint),
                );
            }
            assert!(
                find_batch_anchor(to, root_at(anchor - 1), to - anchor, probe)
                    .expect("lookup")
                    .is_none(),
                "count={count}: a block hash predating the run must not be accepted",
            );
        }
    }

    /// A resumed batch collapses onto the anchor (`from == to == anchor`), so
    /// its only valid endpoint is that block. The old inclusive count also
    /// searched the anchor's parent and could accept a pre-batch commitment.
    #[test]
    fn resumed_batch_endpoint_is_its_own_sync_block_only() {
        let anchor = 174_197_u64;
        let (from, to) = batch_l2_range(anchor, true, 10);
        assert_eq!((from, to), (anchor, anchor));
        assert_eq!(to - anchor, 0);

        assert_eq!(
            find_batch_anchor(to, root_at(anchor), 0, |block| Ok(root_at(block))).expect("lookup"),
            Some(anchor),
        );
        assert!(
            find_batch_anchor(to, root_at(anchor - 1), 0, |block| Ok(root_at(block)))
                .expect("lookup")
                .is_none(),
            "must not settle at its predecessor's block hash",
        );
        assert_eq!(
            find_batch_anchor(to, root_at(anchor - 1), to - from + 1, |block| {
                Ok(root_at(block))
            })
            .expect("lookup"),
            Some(anchor - 1),
        );
    }

    #[test]
    fn search_is_bounded_by_the_window() {
        let cursor = 1_000_u64;
        let mut reads = 0_u32;
        let _ = find_batch_anchor(cursor, B256::repeat_byte(0xEE), 6, |b| {
            reads += 1;
            Ok(root_at(b))
        });
        assert_eq!(reads, 7, "cursor plus the window, nothing deeper");
    }
}

#[cfg(test)]
mod cursor_guard_tests {
    //! A re-scanned resume hits the guard twice: cursor at `entry_state` first,
    //! at `final_state` after. Calling the second divergence rolls back good state.

    use super::cursor_root_accepted;
    use alloy_primitives::B256;

    const ENTRY: B256 = B256::repeat_byte(0x0B);
    const FINAL: B256 = B256::repeat_byte(0x0C);
    const OTHER: B256 = B256::repeat_byte(0xFF);

    #[test]
    fn entry_state_passes_before_the_append() {
        assert!(cursor_root_accepted(ENTRY, ENTRY, Some(FINAL)));
    }

    #[test]
    fn settled_endpoint_passes_after_the_append() {
        assert!(cursor_root_accepted(FINAL, ENTRY, Some(FINAL)));
    }

    #[test]
    fn a_third_root_stays_loud() {
        assert!(!cursor_root_accepted(OTHER, ENTRY, Some(FINAL)));
        // No settled endpoint reported: only `entry_state` can clear the guard.
        assert!(!cursor_root_accepted(OTHER, ENTRY, None));
        assert!(!cursor_root_accepted(FINAL, ENTRY, None));
    }
}

#[cfg(test)]
mod seed_tests {
    use super::choose_seed;

    /// The seed must never leave `[floor, end]` — both bounds have been wrong
    /// in the field, wedging boot each time.
    #[test]
    fn choose_seed_stays_within_the_scanned_range() {
        assert_eq!(choose_seed(500, 1000, Some(968)), 968); // finality lags the tip
        assert_eq!(choose_seed(990, 1000, Some(900)), 990); // finalized below floor
        assert_eq!(choose_seed(500, 1000, Some(1010)), 1000); // finality past scan
        // Chain too young to finalize: the floor is what the scan read.
        assert_eq!(choose_seed(500, 1000, None), 500);
        // Floor above the endpoint (L1 rewound under an indexed batch).
        assert_eq!(choose_seed(999, 40, Some(20)), 40);
        assert_eq!(choose_seed(0, 0, None), 0);
    }
}
