//! `RemoteProver` — the composer-side [`Prover`] backed by one `Prove` RPC.
//!
//! The composer holds `Arc<dyn Prover>` and calls `prove(ctx)`; when that
//! `Arc` is a [`RemoteProver`], the call maps [`ProvingContext`] to a
//! `prove.v1` client-stream (a header then one chunk per window block), dials
//! the configured `eez-proof-signer`, awaits the attestation, verifies it recovers
//! to the registered attester over the exact request batch and physical blobs,
//! and returns the 65-byte signature. Stateless:
//! one round-trip, no feed/cursor/sink.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use std::sync::Arc;

use alloy_primitives::{Address, B256, Bytes, Signature};
use alloy_sol_types::SolCall;
use async_trait::async_trait;
use eez_control_rpc::v1::{
    BlockWitness as WireBlockWitness, ExecutionWitness as WireWitness, PostBatch, ProveChunk,
    ProveHeader, prove_chunk, prove_failure, prover_client::ProverClient,
};
use eez_prover::{
    ActionableProverFailure, Prover, ProverError, ProverResult, ProvingContext,
    RetryableProverError,
};
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
}

impl RemoteProver {
    /// Build a remote prover pointing at `url`, verifying attestations against
    /// the registered `attester`.
    #[must_use]
    pub fn new(url: impl Into<String>, attester: Address) -> Self {
        Self {
            inner: Arc::new(Inner {
                url: url.into(),
                attester,
            }),
        }
    }

    /// The registered attester this prover verifies against.
    #[must_use]
    pub fn attester(&self) -> Address {
        self.inner.attester
    }
}

/// Map a [`ProvingContext`] to the ordered `Prove` chunk stream: header first
/// (with the authoritative postBatch calldata), then one chunk per block.
fn chunks_for(ctx: &ProvingContext) -> Vec<ProveChunk> {
    // The authoritative on-chain payload (proofs[] empty — the proof isn't part
    // of the publicInputsHash). The prover decodes THIS to recompute the hash.
    let abi_calldata = eez_protocol::abi::postAndVerifyBatchCall {
        batch: ctx.batch.clone(),
    }
    .abi_encode();

    let mut chunks = Vec::with_capacity(1 + ctx.blocks.len());
    chunks.push(ProveChunk {
        kind: Some(prove_chunk::Kind::Header(ProveHeader {
            rollup_id: ctx.rollup_id,
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
    });
    for bw in &ctx.blocks {
        chunks.push(ProveChunk {
            kind: Some(prove_chunk::Kind::Block(WireBlockWitness {
                number: bw.number,
                hash: bw.hash.to_vec(),
                parent_hash: bw.parent_hash.to_vec(),
                rlp: bw.rlp.to_vec(),
                witness: Some(WireWitness {
                    state: bw.witness.state.iter().map(|b| b.to_vec()).collect(),
                    codes: bw.witness.codes.iter().map(|b| b.to_vec()).collect(),
                    keys: bw.witness.keys.iter().map(|b| b.to_vec()).collect(),
                    headers: bw.witness.headers.iter().map(|b| b.to_vec()).collect(),
                }),
            })),
        });
    }
    chunks
}

#[async_trait]
impl Prover for RemoteProver {
    #[tracing::instrument(name = "prover_client", target = "eez_trace", skip_all, fields(trace_id = tracing::field::Empty, otel.kind = "client", from_block = ctx.from_block, to_block = ctx.to_block, rollup_id = ctx.rollup_id))]
    async fn prove(&self, ctx: ProvingContext) -> ProverResult<Bytes> {
        eez_telemetry::traces::record_trace_id(&tracing::Span::current());
        let chunks = chunks_for(&ctx);
        let n_blocks = chunks.len().saturating_sub(1);

        // Raise the message-size cap on both directions: a single block's witness
        // can exceed tonic's 4 MiB default → `ResourceExhausted`. Server matches.
        let mut client = ProverClient::connect(self.inner.url.clone())
            .await
            .map_err(|error| ProverError::Retryable {
                kind: RetryableProverError::Unavailable,
                message: format!("connect {}: {error}", self.inner.url),
            })?
            .max_encoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES)
            .max_decoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES);
        let mut request = tonic::Request::new(tokio_stream::iter(chunks));
        eez_telemetry::traces::inject(request.metadata_mut());
        let resp = client
            .prove(request)
            .await
            .map_err(|status| map_rpc_status(ctx.from_block, ctx.to_block, status))?
            .into_inner();

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
            match decode_actionable_failure(status.details()) {
                Some(failure) => ProverError::Actionable { failure, message },
                None => ProverError::Backend(message),
            }
        }
        None => ProverError::Backend(message),
    }
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
    }

    #[tonic::async_trait]
    impl ProverService for StubProver {
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
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (headers, received) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            Server::builder()
                .add_service(ProverServer::new(StubProver {
                    signer,
                    hash,
                    headers,
                }))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        (format!("http://{addr}"), received)
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
            blocks: Vec::new(),
            l1_block_hash: None,
            l1_posting: eez_prover::L1PostingContext {
                chain_id: 31337,
                target_timestamp: 12345,
            },
        }
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
        RemoteProver::new(url, attester).prove(ctx).await.unwrap();
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

        let prover = RemoteProver::new(url, attester);
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
        let prover = RemoteProver::new(spawn_stub(key, hash).await, attester);

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
        let prover = RemoteProver::new(url, wrong_attester);
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
        let prover = RemoteProver::new(format!("http://{addr}"), Address::ZERO);

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
        let prover = RemoteProver::new(spawn_unavailable_stub().await, Address::ZERO);

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
