//! `RemoteProver` — the composer-side [`Prover`] backed by one `Prove` RPC.
//!
//! The composer holds `Arc<dyn Prover>` and calls `prove(ctx)`; when that
//! `Arc` is a [`RemoteProver`], the call maps [`ProvingContext`] to a
//! `prove.v1` client-stream (a header then one chunk per window block), dials
//! the configured `eez-proof-signer`, awaits the attestation, verifies it recovers
//! to the registered attester over the exact request batch and physical blobs,
//! and returns the 65-byte signature. A shared channel retains the checked
//! capability snapshot; each proof still independently verifies its response.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

mod connection;

use alloy_primitives::{Address, B256, Bytes, Signature};
use alloy_sol_types::SolCall;
use async_trait::async_trait;
use eez_control_rpc::v1::{
    BlockWitness as WireBlockWitness, CachedOrdinaryRange, ExecutionWitness as WireWitness,
    PostBatch, PrepareChunk, PrepareHeader, PrepareResponse, ProveChunk, ProveHeader,
    RetainOrdinaryRangesRequest, prepare_chunk, prove_chunk, prove_failure,
};
use eez_prover::{
    ActionableProverFailure, Prover, ProverError, ProverResult, ProvingContext,
    RetryableProverError,
};
use prost::Message as _;
use tonic::{Code, Status};
use tracing::{Level, event};

/// A [`Prover`] that proves a window on a remote `eez-proof-signer` over the
/// `prove.v1.Prover` gRPC service. Cheap to clone (`Arc<Inner>`).
#[derive(Debug, Clone)]
pub struct RemoteProver {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    /// The `eez-proof-signer` endpoint, e.g. `http://127.0.0.1:50061`.
    url: String,
    /// The proof system's registered attester. The returned signature MUST
    /// recover to this over the returned `publicInputsHash`, or the proof is
    /// rejected (a wrong/malicious prover cannot forge an attestation).
    attester: Address,
    expected_identity: B256,
    connection: tokio::sync::Mutex<Option<Arc<connection::Connection>>>,
    agreed_capabilities: tokio::sync::Mutex<Option<eez_control_rpc::v1::CapabilitiesResponse>>,
    prepared_blocks: tokio::sync::RwLock<HashSet<B256>>,
    lease_blocks: tokio::sync::Mutex<BTreeMap<u64, (B256, B256)>>,
    preparation_lock: tokio::sync::Mutex<()>,
    lease_id: tokio::sync::Mutex<Option<B256>>,
}

impl RemoteProver {
    /// Build a remote prover pointing at `url`, verifying attestations against
    /// the registered `attester` and requiring agreement with locally computed
    /// `expected_identity` before any proof is dispatched.
    #[must_use]
    pub fn new(url: impl Into<String>, attester: Address, expected_identity: B256) -> Self {
        Self {
            inner: Arc::new(Inner {
                url: url.into(),
                attester,
                expected_identity,
                connection: tokio::sync::Mutex::new(None),
                agreed_capabilities: tokio::sync::Mutex::new(None),
                prepared_blocks: tokio::sync::RwLock::new(HashSet::new()),
                lease_blocks: tokio::sync::Mutex::new(BTreeMap::new()),
                preparation_lock: tokio::sync::Mutex::new(()),
                lease_id: tokio::sync::Mutex::new(None),
            }),
        }
    }

    /// The registered attester this prover verifies against.
    #[must_use]
    pub fn attester(&self) -> Address {
        self.inner.attester
    }

    /// Validate and retain one contiguous ordinary range without requesting a
    /// settlement signature. Final proving still performs all request-specific
    /// gates and always sends the terminal as a fresh full witness.
    pub async fn prepare_ordinary(
        &self,
        rollup_id: u64,
        blocks: &[eez_prover::BlockWitness],
    ) -> ProverResult<PrepareResponse> {
        let _preparation_guard = self.inner.preparation_lock.lock().await;
        let first = blocks
            .first()
            .ok_or_else(|| ProverError::Backend("empty preparation range".into()))?;
        let last = blocks.last().expect("nonempty preparation range");
        let connection = self.connection().await?;
        let limit =
            usize::try_from(connection.capabilities.preparation_max_blocks).map_err(|_| {
                ProverError::Backend("preparation limit exceeds platform capacity".into())
            })?;
        if limit == 0 || blocks.len() > limit {
            return Err(ProverError::Backend(
                "preparation range exceeds negotiated limit".into(),
            ));
        }
        for pair in blocks.windows(2) {
            if pair[0].number.checked_add(1) != Some(pair[1].number)
                || pair[1].parent_hash != pair[0].hash
            {
                return Err(ProverError::Backend(
                    "preparation range is discontinuous".into(),
                ));
            }
        }
        {
            let retained = self.inner.lease_blocks.lock().await;
            if blocks.iter().any(|block| {
                retained
                    .get(&block.number)
                    .is_some_and(|(hash, _)| *hash != block.hash)
            }) {
                return Err(ProverError::Backend(
                    "preparation branch changed before the previous lease was released".into(),
                ));
            }
        }
        let header = PrepareChunk {
            kind: Some(prepare_chunk::Kind::Header(PrepareHeader {
                rollup_id,
                expected_validation_identity: self.inner.expected_identity.to_vec(),
                from_block: first.number,
                to_block: last.number,
                lease_id: self
                    .inner
                    .lease_id
                    .lock()
                    .await
                    .map_or_else(Vec::new, |id| id.to_vec()),
            })),
        };
        let owned = blocks.to_vec();
        let chunks = std::iter::once(header).chain(owned.into_iter().map(|block| PrepareChunk {
            kind: Some(prepare_chunk::Kind::Block(wire_block(block))),
        }));
        let mut client = connection.client.clone();
        match client.prepare_ordinary(tokio_stream::iter(chunks)).await {
            Ok(response) => {
                let response = response.into_inner();
                if response.validation_identity.as_slice()
                    != self.inner.expected_identity.as_slice()
                    || response.first_number != first.number
                    || response.first_hash.as_slice() != first.hash.as_slice()
                    || response.last_number != last.number
                    || response.last_hash.as_slice() != last.hash.as_slice()
                    || response.prepared_count != blocks.len() as u64
                    || response.lease_id.len() != 32
                {
                    return Err(ProverError::Backend(
                        "preparation response does not match request".into(),
                    ));
                }
                self.inner
                    .prepared_blocks
                    .write()
                    .await
                    .extend(blocks.iter().map(|block| block.hash));
                self.inner.lease_blocks.lock().await.extend(
                    blocks
                        .iter()
                        .map(|block| (block.number, (block.hash, block.parent_hash))),
                );
                *self.inner.lease_id.lock().await = Some(B256::from_slice(&response.lease_id));
                Ok(response)
            }
            Err(status) => {
                if matches!(status.code(), Code::Unavailable | Code::ResourceExhausted)
                    || status.metadata().contains_key("eez-capabilities-changed")
                {
                    self.invalidate_connection(&connection).await;
                }
                Err(map_rpc_status(first.number, last.number, status))
            }
        }
    }

    /// Validate and retain an arbitrary contiguous ordinary range, splitting
    /// it into RPCs that satisfy every negotiated preparation quota. A single
    /// block that exceeds a byte, item, or message limit fails explicitly.
    pub async fn prepare_ordinary_range(
        &self,
        rollup_id: u64,
        blocks: &[eez_prover::BlockWitness],
    ) -> ProverResult<()> {
        if blocks.is_empty() {
            return Ok(());
        }
        for pair in blocks.windows(2) {
            if pair[0].number.checked_add(1) != Some(pair[1].number)
                || pair[1].parent_hash != pair[0].hash
            {
                return Err(ProverError::Backend(
                    "preparation range is discontinuous".into(),
                ));
            }
        }
        let mut offset = 0usize;
        while offset < blocks.len() {
            let connection = self.connection().await?;
            let count = preparation_prefix_len(
                &connection.capabilities,
                rollup_id,
                self.inner.expected_identity,
                &blocks[offset..],
            )?;
            self.prepare_ordinary(rollup_id, &blocks[offset..offset + count])
                .await?;
            offset += count;
        }
        Ok(())
    }

    /// Release the current pending-window reservation only after the caller's
    /// canonical reconciliation has advanced. Cached facts may remain eligible
    /// for reuse, but this client conservatively forgets all reference hints.
    pub async fn release_preparation_lease(&self) -> ProverResult<()> {
        let _preparation_guard = self.inner.preparation_lock.lock().await;
        let Some(lease_id) = *self.inner.lease_id.lock().await else {
            return Ok(());
        };
        let connection = self.connection().await?;
        let mut client = connection.client.clone();
        let response = match client
            .retain_ordinary_ranges(RetainOrdinaryRangesRequest {
                validation_identity: self.inner.expected_identity.to_vec(),
                lease_id: lease_id.to_vec(),
                ranges: Vec::new(),
                release: true,
            })
            .await
        {
            Ok(response) => response.into_inner(),
            Err(status) if status.code() == Code::FailedPrecondition => {
                // Expiry and signer restart both make a locally remembered
                // lease unknown. The reservation carries no validation
                // authority, so forgetting its hints is the only recovery
                // needed before durable witnesses are prepared again.
                *self.inner.lease_id.lock().await = None;
                self.inner.prepared_blocks.write().await.clear();
                self.inner.lease_blocks.lock().await.clear();
                return Ok(());
            }
            Err(status) => return Err(map_rpc_status(0, 0, status)),
        };
        if response.validation_identity.as_slice() != self.inner.expected_identity.as_slice()
            || response.lease_id.as_slice() != lease_id.as_slice()
            || response.retained_count != 0
        {
            return Err(ProverError::Backend(
                "lease-release response does not match request".into(),
            ));
        }
        *self.inner.lease_id.lock().await = None;
        self.inner.prepared_blocks.write().await.clear();
        self.inner.lease_blocks.lock().await.clear();
        Ok(())
    }

    /// Renew the current resource reservation over its exact prepared facts.
    /// Returns `false` when no lease exists or the signer has lost/expired it;
    /// local hints are cleared in the latter case so durable preparation can
    /// rebuild them. A lease is resource ownership only and carries no proof
    /// authority.
    pub async fn renew_preparation_lease(&self) -> ProverResult<bool> {
        let _preparation_guard = self.inner.preparation_lock.lock().await;
        let Some(lease_id) = *self.inner.lease_id.lock().await else {
            return Ok(false);
        };
        let retained = self.inner.lease_blocks.lock().await.clone();
        if retained.is_empty() {
            return Err(ProverError::Backend(
                "preparation lease has no locally retained facts".into(),
            ));
        }
        let ranges = lease_ranges(self.inner.expected_identity, &retained);
        let connection = self.connection().await?;
        let mut client = connection.client.clone();
        let response = match client
            .retain_ordinary_ranges(RetainOrdinaryRangesRequest {
                validation_identity: self.inner.expected_identity.to_vec(),
                lease_id: lease_id.to_vec(),
                ranges,
                release: false,
            })
            .await
        {
            Ok(response) => response.into_inner(),
            Err(status) if status.code() == Code::FailedPrecondition => {
                *self.inner.lease_id.lock().await = None;
                self.inner.prepared_blocks.write().await.clear();
                self.inner.lease_blocks.lock().await.clear();
                return Ok(false);
            }
            Err(status) => return Err(map_rpc_status(0, 0, status)),
        };
        if response.validation_identity.as_slice() != self.inner.expected_identity.as_slice()
            || response.lease_id.as_slice() != lease_id.as_slice()
            || response.retained_count != retained.len() as u64
        {
            return Err(ProverError::Backend(
                "lease-renewal response does not match request".into(),
            ));
        }
        Ok(true)
    }

    async fn retain_final_window(
        &self,
        connection: &connection::Connection,
        rollup_id: u64,
        ordinary: &[eez_prover::BlockWitness],
        from_block: u64,
        to_block: u64,
    ) -> ProverResult<()> {
        let mut repairs = 0usize;
        loop {
            let preparation_guard = self.inner.preparation_lock.lock().await;
            let current_lease = *self.inner.lease_id.lock().await;
            let Some(lease_id) = current_lease else {
                drop(preparation_guard);
                self.prepare_ordinary_range(rollup_id, ordinary).await?;
                continue;
            };
            let first = ordinary.first().expect("nonempty ordinary range");
            let last = ordinary.last().expect("nonempty ordinary range");
            let mut client = connection.client.clone();
            let request = RetainOrdinaryRangesRequest {
                validation_identity: self.inner.expected_identity.to_vec(),
                lease_id: lease_id.to_vec(),
                ranges: vec![CachedOrdinaryRange {
                    validation_identity: self.inner.expected_identity.to_vec(),
                    first_number: first.number,
                    first_hash: first.hash.to_vec(),
                    last_number: last.number,
                    last_hash: last.hash.to_vec(),
                }],
                release: false,
            };
            match client.retain_ordinary_ranges(request).await {
                Ok(response) => {
                    let response = response.into_inner();
                    if response.validation_identity.as_slice()
                        != self.inner.expected_identity.as_slice()
                        || response.lease_id.as_slice() != lease_id.as_slice()
                        || response.retained_count != ordinary.len() as u64
                    {
                        return Err(ProverError::Backend(
                            "retention response does not match final window".into(),
                        ));
                    }
                    let exact = ordinary
                        .iter()
                        .map(|block| (block.number, (block.hash, block.parent_hash)))
                        .collect();
                    *self.inner.lease_blocks.lock().await = exact;
                    let mut prepared = self.inner.prepared_blocks.write().await;
                    prepared.clear();
                    prepared.extend(ordinary.iter().map(|block| block.hash));
                    return Ok(());
                }
                Err(status)
                    if repairs == 0
                        && matches!(
                            status.code(),
                            Code::FailedPrecondition | Code::ResourceExhausted
                        ) =>
                {
                    repairs += 1;
                    *self.inner.lease_id.lock().await = None;
                    self.inner.prepared_blocks.write().await.clear();
                    self.inner.lease_blocks.lock().await.clear();
                    drop(preparation_guard);
                }
                Err(status) => return Err(map_rpc_status(from_block, to_block, status)),
            }
        }
    }
}

fn lease_ranges(identity: B256, blocks: &BTreeMap<u64, (B256, B256)>) -> Vec<CachedOrdinaryRange> {
    let mut ranges: Vec<CachedOrdinaryRange> = Vec::new();
    for (&number, &(hash, parent_hash)) in blocks {
        if let Some(last) = ranges.last_mut()
            && last.last_number.checked_add(1) == Some(number)
            && last.last_hash.as_slice() == parent_hash.as_slice()
        {
            last.last_number = number;
            last.last_hash = hash.to_vec();
            continue;
        }
        ranges.push(CachedOrdinaryRange {
            validation_identity: identity.to_vec(),
            first_number: number,
            first_hash: hash.to_vec(),
            last_number: number,
            last_hash: hash.to_vec(),
        });
    }
    ranges
}

fn preparation_prefix_len(
    capabilities: &eez_control_rpc::v1::CapabilitiesResponse,
    rollup_id: u64,
    identity: B256,
    blocks: &[eez_prover::BlockWitness],
) -> ProverResult<usize> {
    let first = blocks
        .first()
        .ok_or_else(|| ProverError::Backend("empty preparation range".into()))?;
    let block_limit = usize::try_from(capabilities.preparation_max_blocks)
        .map_err(|_| ProverError::Backend("preparation limit exceeds platform capacity".into()))?;
    let byte_limit = usize::try_from(capabilities.max_request_bytes)
        .map_err(|_| ProverError::Backend("request byte limit exceeds platform capacity".into()))?;
    let item_limit = usize::try_from(capabilities.max_request_witness_items).map_err(|_| {
        ProverError::Backend("request witness-item limit exceeds platform capacity".into())
    })?;
    let message_limit = usize::try_from(capabilities.max_message_bytes)
        .map_err(|_| ProverError::Backend("message limit exceeds platform capacity".into()))?;
    if block_limit == 0 {
        return Err(ProverError::Backend(
            "ordinary preparation is unavailable".into(),
        ));
    }

    let mut block_bytes = 0usize;
    let mut witness_items = 0usize;
    let mut accepted = 0usize;
    for block in blocks.iter().take(block_limit) {
        let wire = PrepareChunk {
            kind: Some(prepare_chunk::Kind::Block(wire_block(block.clone()))),
        };
        let message_bytes = wire.encoded_len();
        block_bytes = block_bytes
            .checked_add(message_bytes)
            .ok_or_else(|| ProverError::Backend("preparation byte accounting overflow".into()))?;
        let block_items = block
            .witness
            .state
            .len()
            .checked_add(block.witness.codes.len())
            .and_then(|count| count.checked_add(block.witness.keys.len()))
            .and_then(|count| count.checked_add(block.witness.headers.len()))
            .ok_or_else(|| {
                ProverError::Backend("preparation witness-item accounting overflow".into())
            })?;
        witness_items = witness_items.checked_add(block_items).ok_or_else(|| {
            ProverError::Backend("preparation witness-item accounting overflow".into())
        })?;
        // Use a full-width placeholder lease even for the first request. This
        // keeps grouping valid if another task establishes a lease before the
        // RPC is dispatched.
        let header = PrepareChunk {
            kind: Some(prepare_chunk::Kind::Header(PrepareHeader {
                rollup_id,
                expected_validation_identity: identity.to_vec(),
                from_block: first.number,
                to_block: block.number,
                lease_id: vec![0; 32],
            })),
        };
        let header_bytes = header.encoded_len();
        let aggregate = header_bytes
            .checked_add(block_bytes)
            .ok_or_else(|| ProverError::Backend("preparation byte accounting overflow".into()))?;
        if header_bytes > message_limit
            || message_bytes > message_limit
            || aggregate > byte_limit
            || witness_items > item_limit
        {
            break;
        }
        accepted += 1;
    }
    if accepted == 0 {
        return Err(ProverError::Backend(format!(
            "ordinary block {} cannot fit negotiated preparation quotas",
            first.number
        )));
    }
    Ok(accepted)
}

fn wire_block(block: eez_prover::BlockWitness) -> WireBlockWitness {
    WireBlockWitness {
        number: block.number,
        hash: block.hash.to_vec(),
        parent_hash: block.parent_hash.to_vec(),
        rlp: block.rlp.to_vec(),
        witness: Some(WireWitness {
            state: block
                .witness
                .state
                .into_iter()
                .map(|b| b.to_vec())
                .collect(),
            codes: block
                .witness
                .codes
                .into_iter()
                .map(|b| b.to_vec())
                .collect(),
            keys: block.witness.keys.into_iter().map(|b| b.to_vec()).collect(),
            headers: block
                .witness
                .headers
                .into_iter()
                .map(|b| b.to_vec())
                .collect(),
        }),
    }
}

/// Map a [`ProvingContext`] to the ordered `Prove` chunk stream: header first
/// (with the authoritative postBatch calldata), then one chunk per block.
fn chunks_for(
    ctx: &ProvingContext,
    expected_identity: B256,
    prepared: Arc<HashSet<B256>>,
) -> impl Iterator<Item = ProveChunk> + Send + 'static {
    // The authoritative on-chain payload (proofs[] empty — the proof isn't part
    // of the publicInputsHash). The prover decodes THIS to recompute the hash.
    let abi_calldata = eez_protocol::abi::postAndVerifyBatchCall {
        batch: ctx.batch.clone(),
    }
    .abi_encode();

    let header = ProveChunk {
        kind: Some(prove_chunk::Kind::Header(ProveHeader {
            rollup_id: ctx.rollup_id,
            expected_validation_identity: expected_identity.to_vec(),
            from_block: ctx.from_block,
            to_block: ctx.to_block,
            post_batch: Some(PostBatch {
                abi_calldata,
                // Empty: the prover recomputes the hash and returns it; we don't
                // pre-claim it (the batch determines it deterministically).
                public_inputs_hash: Vec::new(),
                l1_block_hash: ctx.l1_block_hash.map(|h| h.to_vec()).unwrap_or_default(),
                posting_context: Some(eez_control_rpc::v1::L1PostingContext {
                    chain_id: ctx.l1_posting.chain_id,
                    target_timestamp: ctx.l1_posting.target_timestamp,
                }),
                blobs: ctx
                    .blobs
                    .iter()
                    .map(|blob| blob.as_bytes().to_vec())
                    .collect(),
            }),
        })),
    };
    let blocks = Arc::clone(&ctx.blocks);
    let mut index = 0usize;
    std::iter::once(header).chain(std::iter::from_fn(move || {
        if index >= blocks.len() {
            return None;
        }
        // Even a previously prepared terminal hash is sent with its complete
        // fresh witness and observation evidence.
        if index + 1 < blocks.len() && prepared.contains(&blocks[index].hash) {
            let first = index;
            index += 1;
            while index + 1 < blocks.len()
                && prepared.contains(&blocks[index].hash)
                && blocks[index].number == blocks[index - 1].number + 1
                && blocks[index].parent_hash == blocks[index - 1].hash
            {
                index += 1;
            }
            let last = index - 1;
            return Some(ProveChunk {
                kind: Some(prove_chunk::Kind::CachedRange(CachedOrdinaryRange {
                    validation_identity: expected_identity.to_vec(),
                    first_number: blocks[first].number,
                    first_hash: blocks[first].hash.to_vec(),
                    last_number: blocks[last].number,
                    last_hash: blocks[last].hash.to_vec(),
                })),
            });
        }
        let block = blocks[index].clone();
        index += 1;
        Some(ProveChunk {
            kind: Some(prove_chunk::Kind::Block(wire_block(block))),
        })
    }))
}

#[async_trait]
impl Prover for RemoteProver {
    async fn prove(&self, ctx: ProvingContext) -> ProverResult<Bytes> {
        let n_blocks = ctx.blocks.len();
        let connection = self.connection().await?;
        let span = ctx
            .to_block
            .checked_sub(ctx.from_block)
            .and_then(|distance| distance.checked_add(1))
            .ok_or_else(|| ProverError::Backend("invalid proof window".into()))?;
        if span > connection.capabilities.max_window_blocks {
            return Err(ProverError::Backend(
                "proof window exceeds negotiated block limit".into(),
            ));
        }
        let preparation_limit = usize::try_from(connection.capabilities.preparation_max_blocks)
            .map_err(|_| {
                ProverError::Backend("preparation limit exceeds platform capacity".into())
            })?;
        if preparation_limit > 0 && ctx.blocks.len() > 1 {
            // This is the bounded catch-up path. The producer-side durable
            // preparation worker may have populated some or all hashes first;
            // remaining ordinary blocks are repaired in at most 64-block RPCs
            // before final proof dispatch.
            let ordinary = &ctx.blocks[..ctx.blocks.len() - 1];
            let mut start = 0usize;
            while start < ordinary.len() {
                while start < ordinary.len()
                    && self
                        .inner
                        .prepared_blocks
                        .read()
                        .await
                        .contains(&ordinary[start].hash)
                {
                    start += 1;
                }
                if start == ordinary.len() {
                    break;
                }
                let prepared = self.inner.prepared_blocks.read().await;
                let mut end = start + 1;
                while end < ordinary.len() && !prepared.contains(&ordinary[end].hash) {
                    end += 1;
                }
                drop(prepared);
                self.prepare_ordinary_range(ctx.rollup_id, &ordinary[start..end])
                    .await?;
                start = end;
            }
            self.retain_final_window(
                &connection,
                ctx.rollup_id,
                ordinary,
                ctx.from_block,
                ctx.to_block,
            )
            .await?;
        }
        let mut cache_repairs = 0usize;
        let resp = loop {
            let prepared = Arc::new(self.inner.prepared_blocks.read().await.clone());
            let expected_prepared = Arc::clone(&prepared);
            let chunks = chunks_for(&ctx, self.inner.expected_identity, prepared);
            let mut client = connection.client.clone();
            match client.prove(tokio_stream::iter(chunks)).await {
                Ok(response) => break response.into_inner(),
                Err(status) => {
                    if matches!(status.code(), Code::Unavailable | Code::ResourceExhausted)
                        || status.metadata().contains_key("eez-capabilities-changed")
                    {
                        self.invalidate_connection(&connection).await;
                    }
                    let error = map_request_status(
                        ctx.from_block,
                        ctx.to_block,
                        self.inner.expected_identity,
                        &expected_prepared,
                        &ctx.blocks,
                        status,
                    );
                    if let ProverError::CacheMiss { block_hash, .. } = error {
                        if cache_repairs >= 1 {
                            return Err(ProverError::Backend(
                                "prepared cache miss persisted after bounded repair".into(),
                            ));
                        }
                        ctx.blocks
                            .iter()
                            .find(|block| block.hash == block_hash)
                            .ok_or_else(|| {
                                ProverError::Backend(
                                    "cache-miss detail does not name a request block".into(),
                                )
                            })?;
                        let ordinary = &ctx.blocks[..ctx.blocks.len() - 1];
                        {
                            let mut prepared = self.inner.prepared_blocks.write().await;
                            for block in ordinary {
                                prepared.remove(&block.hash);
                            }
                        }
                        self.prepare_ordinary_range(ctx.rollup_id, ordinary).await?;
                        cache_repairs += 1;
                        continue;
                    }
                    return Err(error);
                }
            }
        };

        // Bind both the signing authority and the signed payload. A signature
        // from the registered attester over another batch is not our proof.
        let hash = verify_attestation(
            &resp.signature,
            &resp.public_inputs_hash,
            self.inner.attester,
        )?;
        let from_block = ctx.from_block;
        let to_block = ctx.to_block;
        let vkey = self.vkey();
        // KZG commitment calculation is CPU work; keep it off the async worker.
        let expected_hash = tokio::task::spawn_blocking(move || request_hash(&ctx, vkey))
            .await
            .map_err(|error| {
                ProverError::Backend(format!("public-input preflight worker: {error}"))
            })??;
        if hash != expected_hash {
            return Err(ProverError::Backend(format!(
                "attestation publicInputsHash {hash} != request hash {expected_hash}"
            )));
        }
        event!(
            name: "eez.prover_client.attested",
            Level::INFO,
            event_name = "eez.prover_client.attested",
            from = from_block,
            to = to_block,
            blocks = n_blocks,
            %hash,
            "remote prover attested the window",
        );
        Ok(Bytes::copy_from_slice(&resp.signature))
    }

    fn vkey(&self) -> B256 {
        // Left-zero-pad the 20-byte attester into a B256 (the registry vkey).
        self.inner.attester.into_word()
    }
}

/// Recompute the sole proof carrier from the exact request, deriving versioned
/// hashes from physical blobs instead of accepting a remote hash claim.
fn request_hash(ctx: &ProvingContext, vkey: B256) -> ProverResult<B256> {
    let blob_hashes = eez_blob_codec::blob_versioned_hashes(&ctx.blobs)
        .map_err(|error| ProverError::Backend(format!("request blob commitments: {error}")))?;
    let hashes =
        eez_protocol::public_inputs_hashes_with_blob_hashes(&ctx.batch, vkey, &blob_hashes)
            .map_err(|error| ProverError::Backend(format!("request public inputs: {error}")))?;
    match hashes.as_slice() {
        [hash] => Ok(*hash),
        _ => Err(ProverError::Backend(format!(
            "request has {} public-input hashes, expected one",
            hashes.len()
        ))),
    }
}

/// Preserve the Composer profile's closed retryable-status allowlist across
/// the transport-neutral [`Prover`] boundary. Every other non-OK status is a
/// non-retryable backend rejection for the unchanged request.
fn map_rpc_status(from_block: u64, to_block: u64, status: Status) -> ProverError {
    let kind = match status.code() {
        Code::Unavailable => Some(RetryableProverError::Unavailable),
        Code::DeadlineExceeded => Some(RetryableProverError::DeadlineExceeded),
        Code::Aborted => Some(RetryableProverError::Aborted),
        _ => None,
    };
    let message = format!("Prove {from_block}-{to_block}: {status}");
    match kind {
        Some(kind) => ProverError::Retryable { kind, message },
        None if status.code() == Code::FailedPrecondition => {
            if status.metadata().contains_key("eez-capabilities-changed") {
                return ProverError::Backend(message);
            }
            match decode_actionable_failure(status.details()) {
                Some(failure) => ProverError::Actionable { failure, message },
                None => ProverError::Backend(message),
            }
        }
        None => ProverError::Backend(message),
    }
}

fn map_request_status(
    from_block: u64,
    to_block: u64,
    expected_identity: B256,
    expected_prepared: &HashSet<B256>,
    expected_blocks: &[eez_prover::BlockWitness],
    status: Status,
) -> ProverError {
    if status.code() == Code::FailedPrecondition
        && let Ok(failure) = eez_control_rpc::decode_prove_failure(status.details())
        && let Some(prove_failure::ActionableFailure::CacheMiss(miss)) = failure.actionable_failure
        && let (Ok(identity), Ok(hash)) = (
            B256::try_from(miss.validation_identity.as_slice()),
            B256::try_from(miss.block_hash.as_slice()),
        )
        && identity == expected_identity
        && miss.block_number >= from_block
        && miss.block_number < to_block
        && expected_prepared.contains(&hash)
        && expected_blocks
            .iter()
            .any(|block| block.number == miss.block_number && block.hash == hash)
    {
        return ProverError::CacheMiss {
            validation_identity: identity,
            block_number: miss.block_number,
            block_hash: hash,
        };
    }
    map_rpc_status(from_block, to_block, status)
}

/// Decode only the exact candidate identities the Composer can act on.
/// Empty, malformed, or wrong-length details remain non-actionable.
fn decode_actionable_failure(details: &[u8]) -> Option<ActionableProverFailure> {
    if details.is_empty() {
        return None;
    }
    let failure = eez_control_rpc::decode_prove_failure(details)
        .ok()?
        .actionable_failure?;
    match failure {
        prove_failure::ActionableFailure::Outbound(outbound) => {
            Some(ActionableProverFailure::Outbound {
                transaction_index: outbound.transaction_index.try_into().ok()?,
                transaction_hash: exact_hash(&outbound.transaction_hash)?,
            })
        }
        prove_failure::ActionableFailure::Inbound(inbound) => {
            Some(ActionableProverFailure::Inbound {
                entry_index: inbound.entry_index.try_into().ok()?,
                entry_hash: exact_hash(&inbound.entry_hash)?,
            })
        }
        prove_failure::ActionableFailure::CacheMiss(_) => None,
    }
}

fn exact_hash(bytes: &[u8]) -> Option<B256> {
    (bytes.len() == B256::len_bytes()).then(|| B256::from_slice(bytes))
}

/// Verify a prover attestation: the 65-byte signature must recover to `attester`
/// over the 32-byte `public_inputs_hash`. Returns the bound hash on success.
///
/// # Errors
///
/// [`ProverError::Backend`] on a wrong-length signature or hash, a malformed
/// signature, non-canonical `s` or `v`, or a signer that isn't the registered attester.
fn verify_attestation(
    signature: &[u8],
    public_inputs_hash: &[u8],
    attester: Address,
) -> ProverResult<B256> {
    if signature.len() != 65 {
        return Err(ProverError::Backend(format!(
            "attestation is {} bytes, expected 65 (r||s||v)",
            signature.len()
        )));
    }
    if public_inputs_hash.len() != 32 {
        return Err(ProverError::Backend(format!(
            "publicInputsHash is {} bytes, expected 32",
            public_inputs_hash.len()
        )));
    }
    let hash = B256::from_slice(public_inputs_hash);
    if !matches!(signature[64], 27 | 28) {
        return Err(ProverError::Backend(
            "attestation v must be 27 or 28".into(),
        ));
    }
    let sig = Signature::try_from(signature)
        .map_err(|e| ProverError::Backend(format!("malformed attestation signature: {e}")))?;
    if sig.normalize_s().is_some() {
        return Err(ProverError::Backend(
            "attestation must use canonical low-s".into(),
        ));
    }
    let recovered = sig
        .recover_address_from_prehash(&hash)
        .map_err(|e| ProverError::Backend(format!("attestation recover failed: {e}")))?;
    if recovered != attester {
        return Err(ProverError::Backend(format!(
            "attestation signer {recovered} != registered attester {attester}"
        )));
    }
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;
    use eez_control_rpc::v1::{
        InboundFailure, OutboundFailure, ProveFailure, ProveResponse,
        prover_server::{Prover as ProverService, ProverServer},
    };
    use std::str::FromStr;
    use tonic::{Request, Response, Streaming, transport::Server};

    /// Stub `Prove` server: drains the window stream and returns a fixed
    /// `publicInputsHash` signed by `signer` (the r||s||v packing L1 expects).
    #[derive(Clone)]
    struct StubProver {
        signer: PrivateKeySigner,
        hash: B256,
        headers: tokio::sync::mpsc::UnboundedSender<ProveHeader>,
        retentions: Option<
            tokio::sync::mpsc::UnboundedSender<eez_control_rpc::v1::RetainOrdinaryRangesRequest>,
        >,
        capabilities: eez_control_rpc::v1::CapabilitiesResponse,
        capability_calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[tonic::async_trait]
    impl ProverService for StubProver {
        async fn retain_ordinary_ranges(
            &self,
            request: Request<eez_control_rpc::v1::RetainOrdinaryRangesRequest>,
        ) -> Result<Response<eez_control_rpc::v1::RetainOrdinaryRangesResponse>, Status> {
            if let Some(retentions) = &self.retentions {
                let request = request.into_inner();
                let retained_count = request
                    .ranges
                    .iter()
                    .map(|range| range.last_number - range.first_number + 1)
                    .sum();
                let _ = retentions.send(request.clone());
                return Ok(Response::new(
                    eez_control_rpc::v1::RetainOrdinaryRangesResponse {
                        validation_identity: request.validation_identity,
                        lease_id: request.lease_id,
                        retained_count,
                    },
                ));
            }
            Err(Status::failed_precondition("test lease is unavailable"))
        }

        async fn prepare_ordinary(
            &self,
            _request: Request<Streaming<eez_control_rpc::v1::PrepareChunk>>,
        ) -> Result<Response<eez_control_rpc::v1::PrepareResponse>, Status> {
            Err(Status::unimplemented("proof-only test double"))
        }

        async fn capabilities(
            &self,
            _request: Request<eez_control_rpc::v1::CapabilitiesRequest>,
        ) -> Result<Response<eez_control_rpc::v1::CapabilitiesResponse>, Status> {
            self.capability_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Response::new(self.capabilities.clone()))
        }

        async fn prove(
            &self,
            request: Request<Streaming<ProveChunk>>,
        ) -> Result<Response<ProveResponse>, Status> {
            let mut stream = request.into_inner();
            while let Some(chunk) = stream.message().await? {
                if let Some(prove_chunk::Kind::Header(header)) = chunk.kind {
                    let _ = self.headers.send(header);
                }
            }
            let sig = self.signer.sign_hash_sync(&self.hash).unwrap();
            let mut out = [0u8; 65];
            out[..32].copy_from_slice(&sig.r().to_be_bytes::<32>());
            out[32..64].copy_from_slice(&sig.s().to_be_bytes::<32>());
            out[64] = u8::from(sig.v()) + 27;
            Ok(Response::new(ProveResponse {
                public_inputs_hash: self.hash.to_vec(),
                signature: out.to_vec(),
            }))
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct UnavailableProver;

    #[tonic::async_trait]
    impl ProverService for UnavailableProver {
        async fn retain_ordinary_ranges(
            &self,
            _request: Request<eez_control_rpc::v1::RetainOrdinaryRangesRequest>,
        ) -> Result<Response<eez_control_rpc::v1::RetainOrdinaryRangesResponse>, Status> {
            Err(Status::unimplemented("proof-only test double"))
        }

        async fn prepare_ordinary(
            &self,
            _request: Request<Streaming<eez_control_rpc::v1::PrepareChunk>>,
        ) -> Result<Response<eez_control_rpc::v1::PrepareResponse>, Status> {
            Err(Status::unavailable("test prover is busy"))
        }

        async fn capabilities(
            &self,
            _request: Request<eez_control_rpc::v1::CapabilitiesRequest>,
        ) -> Result<Response<eez_control_rpc::v1::CapabilitiesResponse>, Status> {
            Ok(Response::new(test_capabilities()))
        }

        async fn prove(
            &self,
            _request: Request<Streaming<ProveChunk>>,
        ) -> Result<Response<ProveResponse>, Status> {
            Err(Status::unavailable("test prover is busy"))
        }
    }

    async fn spawn_stub(signer: PrivateKeySigner, hash: B256) -> String {
        spawn_capturing_stub(signer, hash).await.0
    }

    async fn spawn_capturing_stub(
        signer: PrivateKeySigner,
        hash: B256,
    ) -> (String, tokio::sync::mpsc::UnboundedReceiver<ProveHeader>) {
        let (url, received, _) = spawn_configured_stub(signer, hash, test_capabilities()).await;
        (url, received)
    }

    async fn spawn_configured_stub(
        signer: PrivateKeySigner,
        hash: B256,
        capabilities: eez_control_rpc::v1::CapabilitiesResponse,
    ) -> (
        String,
        tokio::sync::mpsc::UnboundedReceiver<ProveHeader>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (headers, received) = tokio::sync::mpsc::unbounded_channel();
        let capability_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = Arc::clone(&capability_calls);
        tokio::spawn(async move {
            Server::builder()
                .add_service(ProverServer::new(StubProver {
                    signer,
                    hash,
                    headers,
                    retentions: None,
                    capabilities,
                    capability_calls: calls,
                }))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        (format!("http://{addr}"), received, capability_calls)
    }

    async fn spawn_unavailable_stub() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            Server::builder()
                .add_service(ProverServer::new(UnavailableProver))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        format!("http://{addr}")
    }

    async fn spawn_retention_stub() -> (
        String,
        tokio::sync::mpsc::UnboundedReceiver<eez_control_rpc::v1::RetainOrdinaryRangesRequest>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (headers, _) = tokio::sync::mpsc::unbounded_channel();
        let (retentions, received) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            Server::builder()
                .add_service(ProverServer::new(StubProver {
                    signer: test_key(),
                    hash: B256::ZERO,
                    headers,
                    retentions: Some(retentions),
                    capabilities: test_capabilities(),
                    capability_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                }))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        (format!("http://{addr}"), received)
    }

    #[tokio::test]
    async fn successful_proofs_share_one_channel_and_capability_snapshot() {
        use std::sync::atomic::Ordering;
        let signer = test_key();
        let attester = signer.address();
        let ctx = test_context();
        let hash = request_hash(&ctx, attester.into_word()).unwrap();
        let (url, mut received, calls) =
            spawn_configured_stub(signer, hash, test_capabilities()).await;
        let prover = RemoteProver::new(url, attester, TEST_IDENTITY);
        prover.capabilities().await.unwrap();
        prover.prove(ctx.clone()).await.unwrap();
        prover.prove(ctx).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        for _ in 0..2 {
            assert_eq!(
                received.recv().await.unwrap().expected_validation_identity,
                TEST_IDENTITY.to_vec()
            );
        }
        assert!(received.try_recv().is_err());
    }

    #[tokio::test]
    async fn malformed_capabilities_and_lower_block_cap_prevent_proof_dispatch() {
        let base = test_capabilities();
        for capabilities in [
            eez_control_rpc::v1::CapabilitiesResponse {
                operations_tag: 2,
                ..base.clone()
            },
            eez_control_rpc::v1::CapabilitiesResponse {
                profile_id: 2,
                ..base.clone()
            },
            eez_control_rpc::v1::CapabilitiesResponse {
                validation_identity: vec![1; 32],
                ..base.clone()
            },
            eez_control_rpc::v1::CapabilitiesResponse {
                validation_identity: vec![0; 31],
                ..base.clone()
            },
            eez_control_rpc::v1::CapabilitiesResponse {
                max_request_witness_items: 0,
                ..base.clone()
            },
            eez_control_rpc::v1::CapabilitiesResponse {
                preparation_max_blocks: 0,
                ..base.clone()
            },
            eez_control_rpc::v1::CapabilitiesResponse {
                max_message_bytes: base.max_request_bytes + 1,
                ..base.clone()
            },
            eez_control_rpc::v1::CapabilitiesResponse {
                max_window_blocks: 1,
                ..base.clone()
            },
        ] {
            let signer = test_key();
            let attester = signer.address();
            let (url, mut headers, _) =
                spawn_configured_stub(signer, B256::ZERO, capabilities).await;
            let prover = RemoteProver::new(url, attester, TEST_IDENTITY);
            let error = prover.prove(context_with_witnesses()).await.unwrap_err();
            assert!(matches!(error, ProverError::Backend(_)), "{error}");
            assert_eq!(error.actionable_failure(), None);
            assert!(
                headers.try_recv().is_err(),
                "no proof may be dispatched before agreement"
            );
        }
    }

    #[tokio::test]
    async fn old_failure_cannot_invalidate_a_new_connection_snapshot() {
        use std::sync::atomic::Ordering;
        let signer = test_key();
        let attester = signer.address();
        let (url, _, calls) = spawn_configured_stub(signer, B256::ZERO, test_capabilities()).await;
        let prover = RemoteProver::new(url, attester, TEST_IDENTITY);
        let first = prover.connection().await.unwrap();
        prover.invalidate_connection(&first).await;
        let second = prover.connection().await.unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        prover.invalidate_connection(&first).await;
        assert!(Arc::ptr_eq(&second, &prover.connection().await.unwrap()));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn reconnect_with_changed_limits_fails_closed_before_proof_dispatch() {
        let signer = test_key();
        let attester = signer.address();
        let baseline = test_capabilities();
        let changed = eez_control_rpc::v1::CapabilitiesResponse {
            max_window_blocks: baseline.max_window_blocks - 1,
            ..baseline.clone()
        };
        let (url, mut headers, _) = spawn_configured_stub(signer, B256::ZERO, changed).await;
        let prover = RemoteProver::new(url, attester, TEST_IDENTITY);
        *prover.inner.agreed_capabilities.lock().await = Some(baseline);
        let error = prover.prove(test_context()).await.unwrap_err();
        assert!(matches!(error, ProverError::Backend(ref message)
            if message.contains("capabilities changed")));
        assert!(headers.try_recv().is_err());
    }

    #[tokio::test]
    async fn lost_release_lease_forgets_only_local_cache_hints() {
        let signer = test_key();
        let attester = signer.address();
        let (url, _, _) = spawn_configured_stub(signer, B256::ZERO, test_capabilities()).await;
        let prover = RemoteProver::new(url, attester, TEST_IDENTITY);
        prover.capabilities().await.unwrap();
        *prover.inner.lease_id.lock().await = Some(B256::repeat_byte(0x55));
        prover
            .inner
            .prepared_blocks
            .write()
            .await
            .insert(B256::repeat_byte(0x44));
        prover
            .inner
            .lease_blocks
            .lock()
            .await
            .insert(1, (B256::repeat_byte(0x44), B256::ZERO));

        prover.release_preparation_lease().await.unwrap();

        assert!(prover.inner.lease_id.lock().await.is_none());
        assert!(prover.inner.prepared_blocks.read().await.is_empty());
        assert!(prover.inner.lease_blocks.lock().await.is_empty());
    }

    #[tokio::test]
    async fn renewal_sends_exact_coalesced_ranges_and_preserves_local_hints() {
        let (url, mut requests) = spawn_retention_stub().await;
        let signer = test_key();
        let prover = RemoteProver::new(url, signer.address(), TEST_IDENTITY);
        let lease_id = B256::repeat_byte(0x55);
        *prover.inner.lease_id.lock().await = Some(lease_id);
        let blocks = BTreeMap::from([
            (1, (B256::repeat_byte(1), B256::ZERO)),
            (2, (B256::repeat_byte(2), B256::repeat_byte(1))),
            (4, (B256::repeat_byte(4), B256::repeat_byte(3))),
        ]);
        *prover.inner.lease_blocks.lock().await = blocks.clone();
        prover
            .inner
            .prepared_blocks
            .write()
            .await
            .extend(blocks.values().map(|(hash, _)| *hash));

        assert!(prover.renew_preparation_lease().await.unwrap());
        let request = requests.recv().await.unwrap();
        assert_eq!(request.lease_id, lease_id.to_vec());
        assert_eq!(request.ranges.len(), 2);
        assert_eq!(
            (
                request.ranges[0].first_number,
                request.ranges[0].last_number
            ),
            (1, 2)
        );
        assert_eq!(
            (
                request.ranges[1].first_number,
                request.ranges[1].last_number
            ),
            (4, 4)
        );
        assert_eq!(*prover.inner.lease_blocks.lock().await, blocks);
        assert_eq!(prover.inner.prepared_blocks.read().await.len(), 3);
    }

    #[tokio::test]
    async fn lost_renewal_clears_stale_lease_and_all_reference_hints() {
        let signer = test_key();
        let (url, _, _) =
            spawn_configured_stub(signer.clone(), B256::ZERO, test_capabilities()).await;
        let prover = RemoteProver::new(url, signer.address(), TEST_IDENTITY);
        *prover.inner.lease_id.lock().await = Some(B256::repeat_byte(0x55));
        prover
            .inner
            .prepared_blocks
            .write()
            .await
            .insert(B256::repeat_byte(0x44));
        prover
            .inner
            .lease_blocks
            .lock()
            .await
            .insert(1, (B256::repeat_byte(0x44), B256::ZERO));

        assert!(!prover.renew_preparation_lease().await.unwrap());
        assert!(prover.inner.lease_id.lock().await.is_none());
        assert!(prover.inner.prepared_blocks.read().await.is_empty());
        assert!(prover.inner.lease_blocks.lock().await.is_empty());
    }

    const TEST_IDENTITY: B256 = B256::repeat_byte(0x99);

    fn test_capabilities() -> eez_control_rpc::v1::CapabilitiesResponse {
        eez_control_rpc::v1::CapabilitiesResponse {
            operations_tag: 3,
            profile_id: 1,
            validation_identity: TEST_IDENTITY.to_vec(),
            max_window_blocks: 65_536,
            max_request_bytes: 536_870_912,
            max_request_witness_items: 1_000_000,
            max_message_bytes: 268_435_456,
            preparation_max_blocks: 64,
            stream_idle_timeout_millis: 120_000,
            request_timeout_millis: 600_000,
            cache_max_entries: 131_072,
            cache_max_bytes: 512 * 1024 * 1024,
            maximum_leases: 2,
            lease_lifetime_millis: 1_200_000,
        }
    }

    #[test]
    fn preparation_grouping_intersects_block_byte_item_and_message_limits() {
        let blocks = context_with_witnesses().blocks;
        let mut capabilities = test_capabilities();
        capabilities.preparation_max_blocks = 64;

        capabilities.max_request_witness_items = 4;
        assert_eq!(
            preparation_prefix_len(&capabilities, 1, TEST_IDENTITY, &blocks).unwrap(),
            1
        );

        capabilities.max_request_witness_items = 1_000_000;
        capabilities.preparation_max_blocks = 1;
        assert_eq!(
            preparation_prefix_len(&capabilities, 1, TEST_IDENTITY, &blocks).unwrap(),
            1
        );

        capabilities.preparation_max_blocks = 64;
        let block = PrepareChunk {
            kind: Some(prepare_chunk::Kind::Block(wire_block(blocks[0].clone()))),
        };
        let header = PrepareChunk {
            kind: Some(prepare_chunk::Kind::Header(PrepareHeader {
                rollup_id: 1,
                expected_validation_identity: TEST_IDENTITY.to_vec(),
                from_block: blocks[0].number,
                to_block: blocks[0].number,
                lease_id: vec![0; 32],
            })),
        };
        capabilities.max_request_bytes = (header.encoded_len() + block.encoded_len()) as u64;
        capabilities.max_message_bytes = capabilities.max_request_bytes;
        assert_eq!(
            preparation_prefix_len(&capabilities, 1, TEST_IDENTITY, &blocks).unwrap(),
            1
        );

        capabilities.max_message_bytes = (block.encoded_len() - 1) as u64;
        let error = preparation_prefix_len(&capabilities, 1, TEST_IDENTITY, &blocks).unwrap_err();
        assert!(matches!(error, ProverError::Backend(message)
            if message.contains("cannot fit negotiated preparation quotas")));
    }

    #[test]
    fn preparation_grouping_rejects_one_block_over_item_quota() {
        let blocks = context_with_witnesses().blocks;
        let mut capabilities = test_capabilities();
        capabilities.preparation_max_blocks = 64;
        capabilities.max_request_witness_items = 3;
        let error = preparation_prefix_len(&capabilities, 1, TEST_IDENTITY, &blocks).unwrap_err();
        assert!(matches!(error, ProverError::Backend(message)
            if message.contains("cannot fit negotiated preparation quotas")));
    }

    fn test_key() -> PrivateKeySigner {
        PrivateKeySigner::from_str(
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        )
        .unwrap()
    }

    // A batch with one proof carrier and one physical blob for response
    // preflight tests. Execution validation belongs to the real signer.
    fn test_context() -> ProvingContext {
        use eez_protocol::abi::{ExecutionEntrySol, RollupIdWithProofSystemsSol};
        ProvingContext {
            rollup_id: 1,
            from_block: 1,
            to_block: 1,
            batch: eez_protocol::EvmBatch {
                proofSystems: vec![address!("0000000000000000000000000000000000000055")],
                rollupIdsWithProofSystems: vec![RollupIdWithProofSystemsSol {
                    rollupId: 1,
                    proofSystemIndexes: vec![0],
                }],
                entries: vec![ExecutionEntrySol::default()],
                blobIndices: vec![alloy_primitives::U256::ZERO],
                ..Default::default()
            },
            blobs: eez_blob_codec::pack(&[0, 1]).unwrap(),
            blocks: Arc::from([]),
            l1_block_hash: None,
            l1_posting: eez_prover::L1PostingContext {
                chain_id: 31337,
                target_timestamp: 12345,
            },
        }
    }

    fn context_with_witnesses() -> ProvingContext {
        let mut ctx = test_context();
        ctx.to_block = 2;
        ctx.blocks = (1..=2)
            .map(|number| {
                let mut block = eez_prover::BlockWitness {
                    number,
                    hash: B256::repeat_byte(number as u8),
                    parent_hash: B256::repeat_byte(number as u8 - 1),
                    rlp: Bytes::from(vec![number as u8, 0xff]),
                    witness: Default::default(),
                };
                block.witness.state = vec![Bytes::from(vec![number as u8; 3])];
                block.witness.codes = vec![Bytes::from_static(&[0x60, 0x00])];
                block.witness.keys = vec![Bytes::from_static(&[0x11])];
                block.witness.headers = vec![Bytes::from_static(&[0xc0])];
                block
            })
            .collect::<Vec<_>>()
            .into();
        ctx
    }

    #[test]
    fn retries_share_immutable_witnesses_and_stream_exact_ordered_fields() {
        let ctx = context_with_witnesses();
        let retry = ctx.clone();
        assert!(Arc::ptr_eq(&ctx.blocks, &retry.blocks));
        let first = chunks_for(&ctx, TEST_IDENTITY, Arc::new(HashSet::new())).collect::<Vec<_>>();
        let second =
            chunks_for(&retry, TEST_IDENTITY, Arc::new(HashSet::new())).collect::<Vec<_>>();
        assert_eq!(first, second);
        assert_eq!(first.len(), 3);
        let Some(prove_chunk::Kind::Header(header)) = &first[0].kind else {
            panic!("header first")
        };
        assert_eq!((header.from_block, header.to_block), (1, 2));
        for (number, chunk) in first[1..].iter().enumerate() {
            let number = number as u8 + 1;
            let Some(prove_chunk::Kind::Block(block)) = &chunk.kind else {
                panic!("block")
            };
            assert_eq!(block.number, u64::from(number));
            assert_eq!(block.hash, vec![number; 32]);
            assert_eq!(block.parent_hash, vec![number - 1; 32]);
            assert_eq!(block.rlp, vec![number, 0xff]);
            let witness = block.witness.as_ref().unwrap();
            assert_eq!(witness.state, vec![vec![number; 3]]);
            assert_eq!(witness.codes, vec![vec![0x60, 0x00]]);
            assert_eq!(witness.keys, vec![vec![0x11]]);
            assert_eq!(witness.headers, vec![vec![0xc0]]);
        }
    }

    #[test]
    fn prepared_chunks_use_ranges_but_always_keep_terminal_witnessed() {
        let ctx = context_with_witnesses();
        let prepared = Arc::new(ctx.blocks.iter().map(|block| block.hash).collect());
        let chunks = chunks_for(&ctx, TEST_IDENTITY, prepared).collect::<Vec<_>>();
        assert_eq!(chunks.len(), 3);
        let Some(prove_chunk::Kind::CachedRange(range)) = &chunks[1].kind else {
            panic!("ordinary block must be referenced")
        };
        assert_eq!((range.first_number, range.last_number), (1, 1));
        assert_eq!(range.validation_identity, TEST_IDENTITY.to_vec());
        let Some(prove_chunk::Kind::Block(terminal)) = &chunks[2].kind else {
            panic!("terminal block must retain its full witness")
        };
        assert_eq!(terminal.number, 2);
        assert!(terminal.witness.is_some());
    }

    #[test]
    fn cache_miss_mapping_requires_exact_request_membership_and_identity() {
        use eez_control_rpc::v1::CacheMiss;
        let ctx = context_with_witnesses();
        let hash = ctx.blocks[0].hash;
        let prepared = HashSet::from([hash]);
        let detail = |identity: B256, number: u64, hash: B256| ProveFailure {
            actionable_failure: Some(prove_failure::ActionableFailure::CacheMiss(CacheMiss {
                validation_identity: identity.to_vec(),
                block_number: number,
                block_hash: hash.to_vec(),
            })),
        };
        let status = |failure: ProveFailure| {
            Status::with_details(
                Code::FailedPrecondition,
                "cache miss",
                eez_control_rpc::encode_prove_failure(&failure).into(),
            )
        };
        assert!(matches!(
            map_request_status(1, 2, TEST_IDENTITY, &prepared, &ctx.blocks,
                status(detail(TEST_IDENTITY, 1, hash))),
            ProverError::CacheMiss { block_number: 1, block_hash, .. } if block_hash == hash
        ));
        for failure in [
            detail(B256::repeat_byte(1), 1, hash),
            detail(TEST_IDENTITY, 2, hash),
            detail(TEST_IDENTITY, 1, B256::repeat_byte(2)),
        ] {
            let error =
                map_request_status(1, 2, TEST_IDENTITY, &prepared, &ctx.blocks, status(failure));
            assert!(matches!(error, ProverError::Backend(_)));
            assert_eq!(error.actionable_failure(), None);
        }
    }

    #[test]
    fn abandoning_a_partial_stream_releases_its_witness_owner() {
        let ctx = context_with_witnesses();
        let owner = Arc::downgrade(&ctx.blocks);
        let mut chunks = chunks_for(&ctx, TEST_IDENTITY, Arc::new(HashSet::new()));
        assert_eq!(owner.strong_count(), 2);
        drop(ctx);
        assert!(matches!(
            chunks.next().unwrap().kind,
            Some(prove_chunk::Kind::Header(_))
        ));
        assert!(matches!(
            chunks.next().unwrap().kind,
            Some(prove_chunk::Kind::Block(_))
        ));
        drop(chunks);
        assert_eq!(owner.strong_count(), 0);
    }

    #[tokio::test]
    async fn early_rpc_rejection_releases_streamed_witnesses() {
        let prover =
            RemoteProver::new(spawn_unavailable_stub().await, Address::ZERO, TEST_IDENTITY);
        let ctx = context_with_witnesses();
        let owner = Arc::downgrade(&ctx.blocks);
        assert_eq!(
            prover.prove(ctx).await.unwrap_err().retryable_kind(),
            Some(RetryableProverError::Unavailable)
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while owner.strong_count() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled transport must release its request body");
    }

    /// The required target and every physical blob survive RPC encoding.
    #[tokio::test]
    async fn rpc_preserves_required_l1_target_and_all_nine_physical_blobs() {
        let key = test_key();
        let attester = key.address();
        let mut ctx = test_context();
        let l1_posting = ctx.l1_posting;
        ctx.l1_block_hash = Some(B256::repeat_byte(0x42));
        ctx.blobs = eez_blob_codec::encode_native_payload(
            1,
            alloy_primitives::Bytes::from(vec![0; 8 * eez_blob_codec::DATA_BYTES_PER_BLOB + 100]),
            Vec::new(),
        )
        .unwrap();
        ctx.batch.blobIndices = (0..ctx.blobs.len())
            .map(alloy_primitives::U256::from)
            .collect();
        let expected = ctx
            .blobs
            .iter()
            .map(|blob| blob.as_bytes().to_vec())
            .collect::<Vec<_>>();
        let hash = request_hash(&ctx, attester.into_word()).unwrap();
        let (url, mut received) = spawn_capturing_stub(key, hash).await;
        RemoteProver::new(url, attester, TEST_IDENTITY)
            .prove(ctx)
            .await
            .unwrap();
        let header = received.recv().await.unwrap();
        let batch = header.post_batch.unwrap();
        assert_eq!(batch.blobs.len(), 9);
        assert_eq!(batch.blobs, expected);
        assert_eq!(batch.l1_block_hash, B256::repeat_byte(0x42).to_vec());
        assert_eq!(
            batch
                .posting_context
                .as_ref()
                .map(|context| (context.chain_id, context.target_timestamp)),
            Some((l1_posting.chain_id, l1_posting.target_timestamp))
        );
        assert!(received.try_recv().is_err());
    }

    /// A registered-attester signature over this request is returned verbatim.
    #[tokio::test]
    async fn attestation_recovering_to_registered_attester_is_accepted() {
        let key = test_key();
        let attester = key.address();
        let ctx = test_context();
        let hash = request_hash(&ctx, attester.into_word()).unwrap();
        let url = spawn_stub(key, hash).await;

        let prover = RemoteProver::new(url, attester, TEST_IDENTITY);
        let proof = prover
            .prove(ctx)
            .await
            .expect("attestation recovering to the registered attester must be accepted");
        assert_eq!(proof.len(), 65, "returned proof is the 65-byte r||s||v");
    }

    #[tokio::test]
    async fn registered_attester_signature_for_different_batch_or_blob_is_rejected() {
        let key = test_key();
        let attester = key.address();
        let original = test_context();
        let hash = request_hash(&original, attester.into_word()).unwrap();
        let prover = RemoteProver::new(spawn_stub(key, hash).await, attester, TEST_IDENTITY);

        let mut changed_entry = original.clone();
        changed_entry.batch.entries[0].rollingHash = B256::repeat_byte(0x42);
        let mut changed_blob = original;
        changed_blob.blobs = eez_blob_codec::pack(&[0, 2]).unwrap();
        for ctx in [changed_entry, changed_blob] {
            let error = prover.prove(ctx).await.unwrap_err();
            assert!(
                matches!(error, ProverError::Backend(ref message)
                if message.contains("!= request hash")),
                "{error:?}"
            );
        }
    }

    /// An attestation signed by a DIFFERENT key does not recover to the registered
    /// attester → fail-closed (a wrong/malicious prover cannot forge one).
    #[tokio::test]
    async fn attestation_from_wrong_signer_fails_closed() {
        let key = test_key();
        let hash = B256::repeat_byte(0x7c);
        let url = spawn_stub(key, hash).await;

        let wrong_attester = address!("0x00000000000000000000000000000000000000ff");
        let prover = RemoteProver::new(url, wrong_attester, TEST_IDENTITY);
        let err = prover
            .prove(test_context())
            .await
            .expect_err("attestation not recovering to the registered attester must be refused");
        assert!(
            matches!(err, ProverError::Backend(_)),
            "wrong-signer attestation must fail closed, got {err:?}"
        );
    }

    #[tokio::test]
    async fn connection_failure_is_retryable_unavailable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let prover = RemoteProver::new(format!("http://{addr}"), Address::ZERO, TEST_IDENTITY);

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            prover.prove(test_context()),
        )
        .await
        .expect("connection failure should be prompt")
        .unwrap_err();

        assert_eq!(
            error.retryable_kind(),
            Some(RetryableProverError::Unavailable)
        );
    }

    #[tokio::test]
    async fn unavailable_rpc_status_surfaces_as_retryable() {
        let prover =
            RemoteProver::new(spawn_unavailable_stub().await, Address::ZERO, TEST_IDENTITY);

        let error = prover.prove(test_context()).await.unwrap_err();

        assert_eq!(
            error.retryable_kind(),
            Some(RetryableProverError::Unavailable)
        );
    }

    /// Pack a signature the way the prover does (r||s||v, v+27).
    fn sign_65(signer: &PrivateKeySigner, hash: B256) -> Vec<u8> {
        let sig = signer.sign_hash_sync(&hash).unwrap();
        let mut out = [0u8; 65];
        out[..32].copy_from_slice(&sig.r().to_be_bytes::<32>());
        out[32..64].copy_from_slice(&sig.s().to_be_bytes::<32>());
        out[64] = u8::from(sig.v()) + 27;
        out.to_vec()
    }

    #[test]
    fn verify_attestation_accepts_valid() {
        let key = test_key();
        let hash = B256::repeat_byte(0x7c);
        let sig = sign_65(&key, hash);
        assert_eq!(
            verify_attestation(&sig, hash.as_slice(), key.address()).unwrap(),
            hash
        );
    }

    #[test]
    fn verify_attestation_rejects_wrong_signer() {
        let key = test_key();
        let hash = B256::repeat_byte(0x7c);
        let sig = sign_65(&key, hash);
        let wrong = address!("0x00000000000000000000000000000000000000ff");
        assert!(verify_attestation(&sig, hash.as_slice(), wrong).is_err());
    }

    #[test]
    fn verify_attestation_rejects_noncanonical_v_and_high_s() {
        use alloy_primitives::{U256, uint};
        let key = test_key();
        let hash = B256::repeat_byte(0x7c);
        let canonical = sign_65(&key, hash);
        for v in [0, 1, 26, 29, 35, 36, 255] {
            let mut sig = canonical.clone();
            sig[64] = v;
            let error = verify_attestation(&sig, hash.as_slice(), key.address()).unwrap_err();
            assert!(error.to_string().contains("v must be 27 or 28"));
        }

        let mut high_s = canonical;
        let order = uint!(0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141_U256);
        let s = order - U256::from_be_slice(&high_s[32..64]);
        high_s[32..64].copy_from_slice(&s.to_be_bytes::<32>());
        high_s[64] = if high_s[64] == 27 { 28 } else { 27 };
        // This is the same recoverable signature, but Solidity requires low-s.
        let error = verify_attestation(&high_s, hash.as_slice(), key.address()).unwrap_err();
        assert!(error.to_string().contains("canonical low-s"));
    }

    #[test]
    fn verify_attestation_rejects_bad_signature_length() {
        let hash = B256::repeat_byte(0x7c);
        assert!(verify_attestation(&[0u8; 64], hash.as_slice(), test_key().address()).is_err());
    }

    #[test]
    fn verify_attestation_rejects_bad_hash_length() {
        let key = test_key();
        let hash = B256::repeat_byte(0x7c);
        let sig = sign_65(&key, hash);
        assert!(verify_attestation(&sig, &[0u8; 31], key.address()).is_err());
    }

    #[test]
    fn verify_attestation_rejects_malformed_signature() {
        let hash = B256::repeat_byte(0x7c);
        // 65 bytes but not a valid signature over `hash` → recover fails or the
        // recovered address won't be the attester.
        assert!(verify_attestation(&[0u8; 65], hash.as_slice(), test_key().address()).is_err());
    }

    #[test]
    fn rpc_status_retryability_is_a_closed_allowlist() {
        let retryable = [
            (Code::Unavailable, RetryableProverError::Unavailable),
            (
                Code::DeadlineExceeded,
                RetryableProverError::DeadlineExceeded,
            ),
            (Code::Aborted, RetryableProverError::Aborted),
        ];
        for (code, expected) in retryable {
            let error = map_rpc_status(5, 9, Status::new(code, "test"));
            assert_eq!(error.retryable_kind(), Some(expected), "{code:?}");
        }

        for code in [
            Code::Cancelled,
            Code::Unknown,
            Code::InvalidArgument,
            Code::NotFound,
            Code::AlreadyExists,
            Code::PermissionDenied,
            Code::ResourceExhausted,
            Code::FailedPrecondition,
            Code::OutOfRange,
            Code::Unimplemented,
            Code::Internal,
            Code::DataLoss,
            Code::Unauthenticated,
        ] {
            let error = map_rpc_status(5, 9, Status::new(code, "test"));
            assert_eq!(error.retryable_kind(), None, "{code:?}");
            assert!(matches!(error, ProverError::Backend(_)), "{code:?}");
        }
    }

    #[test]
    fn failed_precondition_decodes_exact_actionable_candidate_identities() {
        let outbound = ProveFailure {
            actionable_failure: Some(prove_failure::ActionableFailure::Outbound(
                OutboundFailure {
                    transaction_index: 3,
                    transaction_hash: vec![0x44; 32],
                },
            )),
        };
        let error = map_rpc_status(
            5,
            9,
            Status::with_details(
                Code::FailedPrecondition,
                "rejected",
                eez_control_rpc::encode_prove_failure(&outbound).into(),
            ),
        );
        assert_eq!(
            error.actionable_failure(),
            Some(ActionableProverFailure::Outbound {
                transaction_index: 3,
                transaction_hash: B256::repeat_byte(0x44),
            })
        );

        let inbound = ProveFailure {
            actionable_failure: Some(prove_failure::ActionableFailure::Inbound(InboundFailure {
                entry_index: 4,
                entry_hash: vec![0x55; 32],
            })),
        };
        let error = map_rpc_status(
            5,
            9,
            Status::with_details(
                Code::FailedPrecondition,
                "rejected",
                eez_control_rpc::encode_prove_failure(&inbound).into(),
            ),
        );
        assert_eq!(
            error.actionable_failure(),
            Some(ActionableProverFailure::Inbound {
                entry_index: 4,
                entry_hash: B256::repeat_byte(0x55),
            })
        );
    }

    #[test]
    fn capability_mismatch_details_never_become_an_actionable_failure() {
        let detail = ProveFailure {
            actionable_failure: Some(prove_failure::ActionableFailure::Outbound(
                OutboundFailure {
                    transaction_index: 3,
                    transaction_hash: vec![0x44; 32],
                },
            )),
        };
        let mut status = Status::with_details(
            Code::FailedPrecondition,
            "configuration changed",
            eez_control_rpc::encode_prove_failure(&detail).into(),
        );
        status
            .metadata_mut()
            .insert("eez-capabilities-changed", "1".parse().unwrap());
        let error = map_rpc_status(5, 9, status);
        assert!(matches!(error, ProverError::Backend(_)));
        assert_eq!(error.actionable_failure(), None);
        assert_eq!(error.retryable_kind(), None);
    }

    #[test]
    fn malformed_or_misclassified_details_never_become_actionable() {
        let wrong_hash_length = ProveFailure {
            actionable_failure: Some(prove_failure::ActionableFailure::Outbound(
                OutboundFailure {
                    transaction_index: 3,
                    transaction_hash: vec![0x44; 31],
                },
            )),
        };
        let statuses = [
            Status::with_details(
                Code::FailedPrecondition,
                "rejected",
                eez_control_rpc::encode_prove_failure(&wrong_hash_length).into(),
            ),
            Status::with_details(Code::FailedPrecondition, "rejected", vec![0xff].into()),
            Status::with_details(
                Code::Internal,
                "internal",
                eez_control_rpc::encode_prove_failure(&ProveFailure {
                    actionable_failure: Some(prove_failure::ActionableFailure::Inbound(
                        InboundFailure {
                            entry_index: 4,
                            entry_hash: vec![0x55; 32],
                        },
                    )),
                })
                .into(),
            ),
        ];

        for status in statuses {
            let error = map_rpc_status(5, 9, status);
            assert!(matches!(error, ProverError::Backend(_)));
            assert_eq!(error.actionable_failure(), None);
        }
    }
}
