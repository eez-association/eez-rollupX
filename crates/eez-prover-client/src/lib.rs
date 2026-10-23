//! `RemoteProver` — Composer's remote validation and settlement client.
//!
//! The composer holds `Arc<dyn Prover>` and calls `prove(ctx)`; when that
//! `Arc` is a [`RemoteProver`], the call resumes or backfills a `prove.v2`
//! session after checking the offline profile and deployment identity.
//! It awaits the configured signer's attestation, verifies it recovers
//! to the registered attester over the exact request batch and physical blobs,
//! and returns the 65-byte signature. A shared channel retains the checked
//! capability snapshot; each proof still independently verifies its response.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use std::sync::Arc;

mod connection;
mod streaming;

use alloy_primitives::{Address, B256, Bytes, Signature};
use alloy_sol_types::SolCall;
use async_trait::async_trait;
use eez_control_rpc::v1::{
    BlockWitness as WireBlockWitness, ExecutionWitness as WireWitness, prove_failure,
};
use eez_prover::{
    ActionableProverFailure, Prover, ProverError, ProverResult, ProvingContext,
    RetryableProverError,
};
use tonic::{Code, Status};
use tracing::{Level, event};

/// A [`Prover`] that proves a window on a remote `eez-proof-signer` over v2,
/// with complete request-local evidence. Clones share checked session limits.
#[derive(Debug, Clone)]
pub struct RemoteProver {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    streaming: tokio::sync::Mutex<streaming::StreamingState>,
    exchange_timeout: std::time::Duration,
    /// The `eez-proof-signer` endpoint, e.g. `http://127.0.0.1:50061`.
    url: String,
    /// The proof system's registered attester. The returned signature MUST
    /// recover to this over the returned `publicInputsHash`, or the proof is
    /// rejected (a wrong/malicious prover cannot forge an attestation).
    attester: Address,
    expected_identity: B256,
    connection: tokio::sync::Mutex<Option<Arc<connection::Connection>>>,
    agreed_capabilities: tokio::sync::Mutex<Option<eez_control_rpc::v1::CapabilitiesResponse>>,
}

impl RemoteProver {
    /// Build a remote prover pointing at `url`, verifying attestations against
    /// the registered `attester` and requiring agreement with locally computed
    /// `expected_identity` before any proof is dispatched.
    #[must_use]
    pub fn new(url: impl Into<String>, attester: Address, expected_identity: B256) -> Self {
        Self {
            inner: Arc::new(Inner {
                streaming: tokio::sync::Mutex::new(streaming::StreamingState::default()),
                exchange_timeout: std::time::Duration::from_mins(10),
                url: url.into(),
                attester,
                expected_identity,
                connection: tokio::sync::Mutex::new(None),
                agreed_capabilities: tokio::sync::Mutex::new(None),
            }),
        }
    }

    /// The registered attester this prover verifies against.
    #[must_use]
    pub fn attester(&self) -> Address {
        self.inner.attester
    }
}

fn wire_block(block: eez_prover::BlockWitness) -> WireBlockWitness {
    WireBlockWitness {
        number: block.number,
        hash: block.hash.to_vec(),
        parent_hash: block.parent_hash.to_vec(),
        rlp: block.rlp.to_vec(),
        witness: Some(WireWitness {
            l1_headers_rlp: block
                .l1_headers_rlp
                .into_iter()
                .map(|b| b.to_vec())
                .collect(),
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

impl RemoteProver {
    async fn verify_response(
        &self,
        ctx: ProvingContext,
        public_inputs_hash: Vec<u8>,
        signature: Vec<u8>,
    ) -> ProverResult<Bytes> {
        let n_blocks = ctx.blocks.len();
        // Bind both the signing authority and the signed payload. A signature
        // from the registered attester over another batch is not our proof.
        let hash = verify_attestation(&signature, &public_inputs_hash, self.inner.attester)?;
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
        Ok(Bytes::copy_from_slice(&signature))
    }
}

#[async_trait]
impl Prover for RemoteProver {
    async fn prove(&self, ctx: ProvingContext) -> ProverResult<Bytes> {
        let connection = self.connection().await?;
        let count = ctx
            .to_block
            .checked_sub(ctx.from_block)
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| ProverError::Backend("invalid proof window".into()))?;
        if count > connection.capabilities.max_window_blocks || count != ctx.blocks.len() as u64 {
            return Err(ProverError::Backend(
                "proof window exceeds negotiated block limit or has missing blocks".into(),
            ));
        }
        let parent = &ctx.parent;
        let anchor = streaming::StreamAnchor {
            number: parent.number,
            hash: parent.hash_slow(),
            state_root: parent.state_root,
        };
        let settlement = eez_control_rpc::v2::SettlementInput {
            abi_calldata: eez_protocol::abi::postAndVerifyBatchCall {
                batch: ctx.batch.clone(),
            }
            .abi_encode(),
            blobs: ctx
                .blobs
                .iter()
                .map(|blob| blob.as_bytes().to_vec())
                .collect(),
            posting_context: ctx
                .l1_posting
                .map(|posting| eez_control_rpc::v2::L1PostingContext {
                    chain_id: posting.chain_id,
                    target_timestamp: posting.target_timestamp,
                    parent_number: posting.parent_number,
                    parent_hash: posting.parent_hash.to_vec(),
                }),
        };
        match self
            .stream_exchange(
                ctx.rollup_id,
                anchor,
                &ctx.blocks,
                Some((ctx.from_block, ctx.to_block, settlement)),
            )
            .await
        {
            Ok(Some((hash, proof))) => self.verify_response(ctx, hash, proof).await,
            Ok(None) => Err(ProverError::Backend("Finalize returned no proof".into())),
            Err(error) => Err(error),
        }
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
    let hashes = eez_protocol::public_inputs_hashes_with_blob_hashes(
        &ctx.batch,
        vkey,
        &blob_hashes,
        ctx.offline_context,
    )
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
    mod streaming;
    use super::*;
    use alloy_primitives::address;
    use alloy_signer::SignerSync;
    use alloy_signer_local::PrivateKeySigner;
    use eez_control_rpc::v1::{
        InboundFailure, OutboundFailure, ProveFailure,
        prover_server::{Prover as ProverService, ProverServer},
    };
    use std::str::FromStr;
    use tonic::{Request, Response, Streaming, transport::Server};

    /// Capability discovery has no proving behavior; the shared streaming model
    /// below handles every test exchange through the actual v2 service.
    #[derive(Clone)]
    struct CapabilityStub {
        capabilities: eez_control_rpc::v1::CapabilitiesResponse,
        capability_calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[tonic::async_trait]
    impl ProverService for CapabilityStub {
        async fn capabilities(
            &self,
            _request: Request<eez_control_rpc::v1::CapabilitiesRequest>,
        ) -> Result<Response<eez_control_rpc::v1::CapabilitiesResponse>, Status> {
            self.capability_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Response::new(self.capabilities.clone()))
        }
    }

    async fn spawn_stub(signer: PrivateKeySigner, hash: B256) -> String {
        spawn_capturing_stub(signer, hash).await.0
    }

    async fn spawn_capturing_stub(
        signer: PrivateKeySigner,
        hash: B256,
    ) -> (
        String,
        tokio::sync::mpsc::UnboundedReceiver<eez_control_rpc::v2::Finalize>,
    ) {
        let (url, received, _) = spawn_configured_stub(signer, hash, test_capabilities()).await;
        (url, received)
    }

    async fn spawn_configured_stub(
        signer: PrivateKeySigner,
        hash: B256,
        capabilities: eez_control_rpc::v1::CapabilitiesResponse,
    ) -> (
        String,
        tokio::sync::mpsc::UnboundedReceiver<eez_control_rpc::v2::Finalize>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (headers, received) = tokio::sync::mpsc::unbounded_channel();
        let capability_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = Arc::clone(&capability_calls);
        let stub = streaming::StreamStub::response(signer, hash, Some(headers));
        tokio::spawn(async move {
            Server::builder()
                .add_service(ProverServer::new(CapabilityStub {
                    capabilities,
                    capability_calls: calls,
                }))
                .add_service(eez_control_rpc::v2::prover_server::ProverServer::new(stub))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        (format!("http://{addr}"), received, capability_calls)
    }

    async fn spawn_unavailable_stub() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut stub = streaming::StreamStub::response(test_key(), B256::ZERO, None);
        stub.unavailable = true;
        tokio::spawn(async move {
            Server::builder()
                .add_service(ProverServer::new(CapabilityStub {
                    capabilities: test_capabilities(),
                    capability_calls: Arc::default(),
                }))
                .add_service(eez_control_rpc::v2::prover_server::ProverServer::new(stub))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        format!("http://{addr}")
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
            let finalize = received.recv().await.unwrap();
            assert_eq!((finalize.from_block, finalize.to_block), (1, 1));
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
                profile_id: 1,
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
                request_timeout_millis: 0,
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

    const TEST_IDENTITY: B256 = B256::repeat_byte(0x99);

    fn test_capabilities() -> eez_control_rpc::v1::CapabilitiesResponse {
        eez_control_rpc::v1::CapabilitiesResponse {
            operations_tag: 0,
            profile_id: eez_control_rpc::OFFLINE_PROFILE_ID,
            validation_identity: TEST_IDENTITY.to_vec(),
            max_window_blocks: 65_536,
            max_request_bytes: 536_870_912,
            max_request_witness_items: 1_000_000,
            max_message_bytes: 268_435_456,
            stream_idle_timeout_millis: 120_000,
            request_timeout_millis: 600_000,
        }
    }

    fn test_key() -> PrivateKeySigner {
        PrivateKeySigner::from_str(
            "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
        )
        .unwrap()
    }

    fn test_offline_context() -> eez_protocol::l1_history::OfflineContext {
        use eez_protocol::l1_history::{EvidenceAnchor, OfflineContext, OfflineProfile};
        let profile = OfflineProfile {
            l1ChainId: alloy_primitives::U256::from(31337),
            l2ChainId: alloy_primitives::U256::from(1),
            genesisHash: alloy_consensus::Header::default().hash_slow(),
            genesisTimestamp: 5,
            blockInterval: 1,
            syncInterval: 5,
            updateOffset: 2,
            l1ConfigHash: B256::repeat_byte(1),
            historyCodeHash: B256::repeat_byte(2),
            ruleTimes: vec![0],
        };
        OfflineContext::new(
            &profile,
            Address::repeat_byte(3),
            Address::repeat_byte(4),
            1,
            profile.genesisHash,
            EvidenceAnchor::ActualParent {
                number: 99,
                hash: B256::repeat_byte(0x42),
                target_timestamp: 12345,
            },
            12340,
        )
        .unwrap()
    }

    // A batch with one proof carrier and one physical blob for response
    // preflight tests. Execution validation belongs to the real signer.
    fn test_context() -> ProvingContext {
        use eez_protocol::abi::{ExecutionEntrySol, RollupIdWithProofSystemsSol};
        ProvingContext {
            parent: Default::default(),
            offline_context: test_offline_context(),
            rollup_id: 1,
            from_block: 1,
            to_block: 1,
            batch: eez_protocol::EvmBatch {
                blockNumber: u64::MAX,
                proofSystems: vec![address!("0000000000000000000000000000000000000055")],
                rollupIdsWithProofSystems: vec![RollupIdWithProofSystemsSol {
                    rollupId: 1,
                    proofSystemIndexes: vec![0],
                }],
                entries: vec![ExecutionEntrySol::default()],
                expectedRootPerRollup: vec![eez_protocol::abi::ExpectedRootPerRollupSol {
                    rollupId: 1,
                    root: alloy_consensus::Header::default().hash_slow(),
                }],
                blobIndices: vec![alloy_primitives::U256::ZERO],
                ..Default::default()
            },
            blobs: eez_blob_codec::pack(&[0, 1]).unwrap(),
            blocks: Arc::from([eez_prover::BlockWitness {
                l1_headers_rlp: vec![Bytes::from_static(&[0xc0])],
                number: 1,
                hash: alloy_consensus::Header {
                    number: 1,
                    parent_hash: alloy_consensus::Header::default().hash_slow(),
                    ..Default::default()
                }
                .hash_slow(),
                parent_hash: alloy_consensus::Header::default().hash_slow(),
                rlp: alloy_rlp::encode(eez_primitives::Block::new(
                    alloy_consensus::Header {
                        number: 1,
                        parent_hash: alloy_consensus::Header::default().hash_slow(),
                        ..Default::default()
                    },
                    Default::default(),
                ))
                .into(),
                witness: Default::default(),
            }]),
            l1_posting: Some(eez_prover::L1PostingContext {
                parent_number: 99,
                parent_hash: B256::repeat_byte(0x42),
                chain_id: 31337,
                target_timestamp: 12345,
            }),
        }
    }

    fn context_with_witnesses() -> ProvingContext {
        let mut ctx = test_context();
        ctx.to_block = 2;
        ctx.blocks = (1..=2)
            .map(|number| {
                let mut block = eez_prover::BlockWitness {
                    l1_headers_rlp: vec![Bytes::from_static(&[0xc1, 0x80])],
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
    fn retries_share_immutable_witnesses_and_preserve_both_ancestries() {
        let ctx = context_with_witnesses();
        let retry = ctx.clone();
        assert!(Arc::ptr_eq(&ctx.blocks, &retry.blocks));
        for (left, right) in ctx.blocks.iter().zip(retry.blocks.iter()) {
            let wire = wire_block(left.clone());
            assert_eq!(wire, wire_block(right.clone()));
            assert_eq!(wire.number, left.number);
            assert_eq!(wire.hash, left.hash.as_slice());
            assert_eq!(wire.parent_hash, left.parent_hash.as_slice());
            assert_eq!(wire.rlp, left.rlp.as_ref());
            let witness = wire.witness.unwrap();
            assert_eq!(witness.state, left.witness.state);
            assert_eq!(witness.codes, left.witness.codes);
            assert_eq!(witness.keys, left.witness.keys);
            assert_eq!(witness.headers, left.witness.headers);
            assert_eq!(witness.l1_headers_rlp, left.l1_headers_rlp);
            assert_ne!(witness.headers, witness.l1_headers_rlp);
        }
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
        let l1_posting = ctx.l1_posting.unwrap();
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
        let batch = header.settlement.unwrap();
        assert_eq!(batch.blobs.len(), 9);
        assert_eq!(batch.blobs, expected);
        assert_eq!(
            batch.posting_context.as_ref().unwrap().parent_hash,
            l1_posting.parent_hash.to_vec()
        );
        assert_eq!(
            batch.posting_context.as_ref().unwrap().parent_number,
            l1_posting.parent_number
        );
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
