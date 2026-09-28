//! `Prove` request orchestration and blocking-task lifecycle management.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use eez_control_rpc::v1::prover_server::Prover;
use eez_control_rpc::v1::{ProveChunk, ProveResponse};
use tokio::sync::OwnedSemaphorePermit;
use tokio::task::AbortHandle;
use tokio::time::{Instant, timeout_at};
use tonic::{Request, Response, Status, Streaming};
use tracing::{Level, Span, debug, error, event, info, trace, warn};

use super::ProveSvc;
use super::settlement_job::{AttestationMaterial, validate_and_settle};
use super::stream::read_window;
use crate::cancel::CancellationToken;
use crate::window;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Emits one event at a level chosen at runtime. `tracing` fixes each
/// callsite's level at compile time, so this expands to one callsite per level.
macro_rules! dyn_event {
    ($level:expr, $($field:tt)+) => {
        match $level {
            Level::ERROR => error!($($field)+),
            Level::WARN => warn!($($field)+),
            Level::INFO => info!($($field)+),
            Level::DEBUG => debug!($($field)+),
            Level::TRACE => trace!($($field)+),
        }
    };
}

/// Public rejection message shared by every deadline-exceeded path.
const DEADLINE_EXCEEDED_MESSAGE: &str = "Prove request deadline exceeded";

/// Start instant and absolute request deadline shared by every phase.
#[derive(Clone, Copy, Debug)]
struct RequestTiming {
    started: Instant,
    deadline: Instant,
}

/// Successful blocking-task result together with the still-held request slot.
struct CompletedPipeline {
    request_permit: OwnedSemaphorePermit,
    attestation_material: AttestationMaterial,
}

impl RequestTiming {
    /// Milliseconds since the request acquired the active slot, saturating at `u64::MAX`.
    fn elapsed_ms(self) -> u64 {
        elapsed_millis(self.started)
    }

    /// Reject with `DeadlineExceeded` once the absolute deadline has passed.
    /// Without a pipeline start instant no `pipeline_elapsed_ms` is recorded.
    fn ensure_before_deadline(
        self,
        phase: &'static str,
        message: &'static str,
        pipeline_started: Option<Instant>,
    ) -> Result<(), Status> {
        if Instant::now() < self.deadline {
            return Ok(());
        }
        Err(self.deadline_exceeded(phase, message, pipeline_started))
    }

    /// Log and build the shared deadline-exceeded rejection.
    fn deadline_exceeded(
        self,
        phase: &'static str,
        message: &'static str,
        pipeline_started: Option<Instant>,
    ) -> Status {
        warn!(
            phase,
            grpc_code = ?tonic::Code::DeadlineExceeded,
            pipeline_elapsed_ms = pipeline_started.map(elapsed_millis),
            elapsed_ms = self.elapsed_ms(),
            "{message}",
        );
        Status::deadline_exceeded(DEADLINE_EXCEEDED_MESSAGE)
    }
}

#[tonic::async_trait]
impl Prover for ProveSvc {
    async fn retain_ordinary_ranges(
        &self,
        request: Request<eez_control_rpc::v1::RetainOrdinaryRangesRequest>,
    ) -> Result<Response<eez_control_rpc::v1::RetainOrdinaryRangesResponse>, Status> {
        super::preparation::retain(self, request.into_inner()).await
    }

    async fn prepare_ordinary(
        &self,
        request: Request<Streaming<eez_control_rpc::v1::PrepareChunk>>,
    ) -> Result<Response<eez_control_rpc::v1::PrepareResponse>, Status> {
        super::preparation::prepare(self, request).await
    }

    async fn capabilities(
        &self,
        _request: Request<eez_control_rpc::v1::CapabilitiesRequest>,
    ) -> Result<Response<eez_control_rpc::v1::CapabilitiesResponse>, Status> {
        let limits = self.limits;
        let window = limits.window_limits();
        // Use the enforced limits, including the aggregate-byte reduction of
        // the per-message ceiling. No permit, witness, or attestable value is
        // acquired/constructed here.
        Ok(Response::new(eez_control_rpc::v1::CapabilitiesResponse {
            operations_tag: u32::from(eez_payload_codec::v0::VERSION),
            profile_id: 1,
            validation_identity: self.state.validation_identity.to_vec(),
            max_window_blocks: window.blocks as u64,
            max_request_bytes: window.payload_bytes as u64,
            max_request_witness_items: window.witness_items as u64,
            max_message_bytes: limits.max_decoding_message_bytes() as u64,
            preparation_max_blocks: super::preparation::MAX_PREPARATION_BLOCKS,
            stream_idle_timeout_millis: u64::try_from(limits.stream_idle_timeout().as_millis())
                .map_err(|_| Status::internal("stream idle timeout exceeds capability range"))?,
            request_timeout_millis: u64::try_from(limits.request_timeout().as_millis())
                .map_err(|_| Status::internal("request timeout exceeds capability range"))?,
            cache_max_entries: crate::prepared_cache::DEFAULT_MAX_ENTRIES as u64,
            cache_max_bytes: crate::prepared_cache::DEFAULT_MAX_BYTES as u64,
            maximum_leases: eez_control_rpc::MAX_PREPARATION_LEASES,
            lease_lifetime_millis: u64::try_from(limits.lease_lifetime().as_millis())
                .map_err(|_| Status::internal("lease lifetime exceeds capability range"))?,
        }))
    }

    #[tracing::instrument(
        name = "prove",
        skip_all,
        fields(
            request_id = next_request_id(),
            peer_addr = ?request.remote_addr(),
            validator = self.state.validator.label(),
            expected_rollup_id = self.state.expected_rollup_id.get(),
            wire_rollup_id = tracing::field::Empty,
            declared_from_block = tracing::field::Empty,
            declared_to_block = tracing::field::Empty,
            declared_blocks = tracing::field::Empty,
        )
    )]
    async fn prove(
        &self,
        request: Request<Streaming<ProveChunk>>,
    ) -> Result<Response<ProveResponse>, Status> {
        let request_permit = self.try_acquire_request_slot()?;
        let timing = self.request_timing()?;
        // Ingestion admits stream structure only: exactly the declared block
        // span under the expected rollup identity. Every execution and
        // settlement claim inside it is still unverified.
        let admitted_window = self.ingest_window(request.into_inner(), timing).await?;
        let (header, blocks) = admitted_window.into_parts();
        // The Composer claim already matched `expected_rollup_id`, so it is
        // deliberately not forwarded into validation or settlement.
        let window::AdmittedHeader {
            declared_from_block,
            declared_to_block,
            submitted_post_batch_calldata,
            submitted_blobs,
            posting_context,
            parent_provenance,
        } = header;
        let maximum_blobs = self.state.posting_blob_limit(&posting_context)?;
        let (candidate_context, target_recheck) = timeout_at(
            timing.deadline,
            self.state
                .l1_authority
                .authenticate(posting_context, parent_provenance),
        )
        .await
        .map_err(|_| {
            timing.deadline_exceeded(
                "l1_authentication",
                "Prove deadline exceeded authenticating L1",
                None,
            )
        })??;
        timing.ensure_before_deadline(
            "pipeline",
            "Prove request deadline exceeded before request pipeline",
            None,
        )?;

        let pipeline_started = Instant::now();
        let CompletedPipeline {
            request_permit,
            attestation_material,
        } = self
            .run_pipeline(
                blocks,
                submitted_post_batch_calldata,
                submitted_blobs,
                maximum_blobs,
                posting_context,
                candidate_context,
                request_permit,
                pipeline_started,
                timing,
            )
            .await?;
        // Keep the exclusive slot through signing and response construction.
        let _active_request_guard = request_permit;
        timeout_at(timing.deadline, target_recheck.revalidate())
            .await
            .map_err(|_| {
                timing.deadline_exceeded(
                    "l1_revalidation",
                    "Prove deadline exceeded rechecking L1",
                    Some(pipeline_started),
                )
            })??;
        let attestable_public_inputs_hash =
            super::settlement_job::AttestablePublicInputsHash::authorize(
                attestation_material.recomputed_public_inputs,
            );

        timing.ensure_before_deadline(
            "attestation",
            "Prove request deadline exceeded before attestation",
            Some(pipeline_started),
        )?;
        let attestation_signature = self
            .state
            .attester
            .sign(attestable_public_inputs_hash)
            .map_err(|signing_error| {
                error!(
                    phase = "attestation",
                    grpc_code = ?tonic::Code::Internal,
                    attester = %self.state.attester.address(),
                    error = %signing_error,
                    elapsed_ms = timing.elapsed_ms(),
                    "public-input hash signing failed",
                );
                Status::internal("attestation signing failed")
            })?;
        timing.ensure_before_deadline(
            "attestation",
            "Prove request deadline exceeded during attestation",
            None,
        )?;

        event!(
            name: "eez.proof_signer.window_signed",
            Level::INFO,
            event_name = "eez.proof_signer.window_signed",
            phase = "attestation",
            validated_from_block = declared_from_block,
            validated_to_block = declared_to_block,
            validated_window_pre_block_hash = %attestation_material.validated_window_pre_block_hash,
            validated_window_post_block_hash = %attestation_material.validated_window_post_block_hash,
            recomputed_public_inputs_hash = %attestable_public_inputs_hash.into_inner(),
            attester = %self.state.attester.address(),
            elapsed_ms = timing.elapsed_ms(),
            "window validated and signed",
        );
        Ok(Response::new(ProveResponse {
            public_inputs_hash: attestable_public_inputs_hash.into_inner().to_vec(),
            signature: attestation_signature.to_vec(),
        }))
    }
}

impl ProveSvc {
    /// Claim the single active-request slot, rejecting a second request instead of
    /// queueing its potentially open stream.
    fn try_acquire_request_slot(&self) -> Result<OwnedSemaphorePermit, Status> {
        let request_permit = Arc::clone(&self.active_request_slot)
            .try_acquire_owned()
            .map_err(|_| {
                warn!(
                    phase = "admission",
                    grpc_code = ?tonic::Code::Unavailable,
                    "Prove request rejected: another request is active",
                );
                Status::unavailable("another Prove request is already active")
            })?;
        debug!(phase = "admission", "Prove request admitted");
        Ok(request_permit)
    }

    /// Anchor the absolute deadline covering ingestion, validation,
    /// settlement, and signing.
    fn request_timing(&self) -> Result<RequestTiming, Status> {
        let started = Instant::now();
        let deadline = started
            .checked_add(self.limits.request_timeout())
            .ok_or_else(|| {
                error!(
                    phase = "admission",
                    grpc_code = ?tonic::Code::Internal,
                    "configured request timeout is out of range",
                );
                Status::internal("configured request timeout is out of range")
            })?;
        Ok(RequestTiming { started, deadline })
    }

    /// Stream one posted window within the idle and absolute deadlines.
    async fn ingest_window(
        &self,
        stream: Streaming<ProveChunk>,
        timing: RequestTiming,
    ) -> Result<window::AdmittedWindow, Status> {
        let ingestion = read_window(
            stream,
            self.state.expected_rollup_id,
            self.state.validation_identity,
            Arc::clone(&self.prepared_cache),
            self.limits.window_limits(),
            self.limits.stream_idle_timeout(),
        );
        match timeout_at(timing.deadline, ingestion).await {
            Ok(Ok(window_input)) => Ok(window_input),
            Ok(Err(status)) => {
                let (level, message) = if status.code() == tonic::Code::Cancelled {
                    (Level::DEBUG, "Prove request cancelled while reading window")
                } else {
                    (Level::WARN, "Prove request rejected while reading window")
                };
                dyn_event!(
                    level,
                    phase = "ingestion",
                    grpc_code = ?status.code(),
                    reason = %status.message(),
                    elapsed_ms = timing.elapsed_ms(),
                    "{message}",
                );
                Err(status)
            }
            Err(_) => {
                Err(timing.deadline_exceeded("ingestion", "Prove request deadline exceeded", None))
            }
        }
    }

    /// Validate and settle the window on one deadline-bounded blocking task.
    /// The request permit travels through the task so the slot stays held
    /// until the worker exits, then returns for the signing phase.
    async fn run_pipeline(
        &self,
        blocks: window::AdmittedBlocks,
        submitted_post_batch_calldata: Vec<u8>,
        submitted_blobs: Vec<Vec<u8>>,
        maximum_blobs: usize,
        posting_context: eez_protocol::public_inputs::L1PostingContext,
        candidate_context: super::l1::CandidateContext,
        request_permit: OwnedSemaphorePermit,
        pipeline_started: Instant,
        timing: RequestTiming,
    ) -> Result<CompletedPipeline, Status> {
        // Preparation releases this permit after each block. A final request
        // therefore waits for at most the currently executing preparation
        // block, then owns the worker through complete settlement/signing data.
        let validation_permit = timeout_at(
            timing.deadline,
            Arc::clone(&self.validation_worker).acquire_owned(),
        )
        .await
        .map_err(|_| {
            timing.deadline_exceeded(
                "validation_wait",
                "Prove request deadline exceeded waiting for validation worker",
                None,
            )
        })?
        .map_err(|_| Status::internal("validation worker closed"))?;
        let submitted_post_batch_calldata_bytes = submitted_post_batch_calldata.len();
        debug!(
            phase = "pipeline",
            ingestion_elapsed_ms = timing.elapsed_ms(),
            submitted_post_batch_calldata_bytes,
            "starting validation and settlement",
        );
        let state = Arc::clone(&self.state);
        // The worker's only cancellation signal is this token; it is created
        // here because no earlier phase has blocking work to stop.
        let cancellation = CancellationToken::default();
        let worker_cancellation = cancellation.clone();
        let deadline = timing.deadline;
        // Blocking tasks do not inherit the async request span automatically.
        let span = Span::current();
        let pipeline_work = tokio::task::spawn_blocking(move || {
            let _validation_guard = validation_permit;
            let result = span.in_scope(|| {
                validate_and_settle(
                    &state,
                    blocks,
                    submitted_post_batch_calldata,
                    submitted_blobs,
                    maximum_blobs,
                    posting_context,
                    candidate_context,
                    deadline,
                    &worker_cancellation,
                )
            });
            (request_permit, result)
        });
        // On early exit the guard aborts queued work or requests cooperative
        // cancellation from running work. The slot stays exclusive until the
        // worker's closure drops `request_permit`.
        let _pipeline_worker_guard = WorkerGuard::new(pipeline_work.abort_handle(), cancellation);
        let (request_permit, pipeline_result) = timeout_at(timing.deadline, pipeline_work)
            .await
            .map_err(|_| {
                timing.deadline_exceeded(
                    "pipeline",
                    "Prove request deadline exceeded during request pipeline",
                    Some(pipeline_started),
                )
            })?
            .map_err(|join_error| {
                error!(
                    phase = "pipeline",
                    grpc_code = ?tonic::Code::Internal,
                    error = %join_error,
                    pipeline_elapsed_ms = elapsed_millis(pipeline_started),
                    elapsed_ms = timing.elapsed_ms(),
                    "request pipeline worker failed",
                );
                Status::internal("request pipeline worker failed")
            })?;
        let attestation_material = pipeline_result.map_err(|pipeline_error| {
            let status = pipeline_error.status();
            let grpc_code = status.code();
            let (level, message) = match grpc_code {
                tonic::Code::Internal => (Level::ERROR, "request pipeline invariant failed"),
                tonic::Code::Cancelled => (
                    Level::DEBUG,
                    "request pipeline stopped after request cancellation",
                ),
                tonic::Code::DeadlineExceeded => (
                    Level::WARN,
                    "Prove request deadline exceeded before settlement",
                ),
                _ => (Level::WARN, "request pipeline rejected"),
            };
            dyn_event!(
                level,
                phase = pipeline_error.phase(),
                gate = pipeline_error.gate(),
                ?grpc_code,
                error = %pipeline_error,
                submitted_post_batch_calldata_bytes,
                pipeline_elapsed_ms = elapsed_millis(pipeline_started),
                elapsed_ms = timing.elapsed_ms(),
                "{message}",
            );
            status
        })?;
        Ok(CompletedPipeline {
            request_permit,
            attestation_material,
        })
    }
}

fn elapsed_millis(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

/// Request pipeline-worker cancellation when the RPC exits through any path.
///
/// A queued `spawn_blocking` job is aborted before it starts; a running job
/// cannot be aborted and instead observes the cancellation signal at its next
/// cooperative checkpoint.
#[must_use = "the guard must be held for the lifetime of the RPC pipeline work"]
pub(super) struct WorkerGuard {
    abort: AbortHandle,
    cancellation: CancellationToken,
}

impl WorkerGuard {
    pub(super) const fn new(abort: AbortHandle, cancellation: CancellationToken) -> Self {
        Self {
            abort,
            cancellation,
        }
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.abort.abort();
        self.cancellation.cancel();
    }
}
