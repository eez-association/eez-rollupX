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
pub(super) struct RequestTiming {
    pub(super) started: Instant,
    pub(super) deadline: Instant,
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
    async fn prove(
        &self,
        request: Request<Streaming<ProveChunk>>,
    ) -> Result<Response<ProveResponse>, Status> {
        let span = tracing::info_span!(target: "eez_trace", "prove",
            otel.kind = "server",
            trace_id = tracing::field::Empty,
            request_id = next_request_id(),
            peer_addr = ?request.remote_addr(),
            validator = self.state.validator.label(),
            expected_rollup_id = self.state.expected_rollup_id.get(),
            wire_rollup_id = tracing::field::Empty,
            declared_from_block = tracing::field::Empty,
            declared_to_block = tracing::field::Empty,
            declared_blocks = tracing::field::Empty,
        );
        eez_telemetry::traces::attach_remote_parent(&span, request.metadata());
        eez_telemetry::traces::record_trace_id(&span);
        let mut timer = eez_telemetry::operational::StageTimer::start(
            eez_telemetry::operational::Stage::Signer,
        );
        let result = tracing::Instrument::instrument(self.prove_request(request), span).await;
        timer.finish(stage_outcome(result.as_ref().err()));
        result
    }
}

impl ProveSvc {
    async fn prove_request(
        &self,
        request: Request<Streaming<ProveChunk>>,
    ) -> Result<Response<ProveResponse>, Status> {
        let request_permit = self.try_acquire_request_slot()?;
        let timing = self.request_timing()?;
        // Ingestion admits stream structure only: exactly the declared block
        // span under the expected rollup identity. Every execution and
        // settlement claim inside it is still unverified.
        let mut ingestion_timer = eez_telemetry::operational::StageTimer::start(
            eez_telemetry::operational::Stage::SignerIngestion,
        );
        let admitted_window = self.ingest_window(request.into_inner(), timing).await;
        ingestion_timer.finish(stage_outcome(admitted_window.as_ref().err()));
        let admitted_window = admitted_window?;
        let (header, blocks) = admitted_window.into_parts();
        // The Composer claim already matched `expected_rollup_id`, so it is
        // deliberately not forwarded into validation or settlement.
        let window::AdmittedHeader {
            declared_from_block,
            declared_to_block,
            submitted_post_batch_calldata,
            submitted_blobs,
            posting_context,
        } = header;
        let maximum_blobs = self.state.posting_blob_limit(&posting_context)?;
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
                request_permit,
                pipeline_started,
                timing,
            )
            .await?;
        // Keep the exclusive slot through signing and response construction.
        let _active_request_guard = request_permit;
        self.finish_authorized_response(
            attestation_material,
            declared_from_block,
            declared_to_block,
            timing,
            pipeline_started,
        )
    }

    /// Only fully authorized material can enter the response observation. The
    /// caller retains its exclusive request permit through this synchronous tail.
    pub(super) fn finish_authorized_response(
        &self,
        attestation_material: AttestationMaterial,
        declared_from_block: u64,
        declared_to_block: u64,
        timing: RequestTiming,
        pipeline_started: Instant,
    ) -> Result<Response<ProveResponse>, Status> {
        use eez_telemetry::operational::{AuthorizedOutcome, AuthorizedRequest};
        let mut observation = AuthorizedRequest::start();
        let result = (|| {
            let attestable_public_inputs_hash = attestation_material.attestable_public_inputs;

            timing.ensure_before_deadline(
                "attestation",
                "Prove request deadline exceeded before attestation",
                Some(pipeline_started),
            )?;
            let mut signing_timer = eez_telemetry::operational::StageTimer::start(
                eez_telemetry::operational::Stage::SignerSigning,
            );
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
                });
            signing_timer.finish(stage_outcome(attestation_signature.as_ref().err()));
            let attestation_signature = attestation_signature?;
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
        })();
        observation.finish(match result.as_ref().err().map(Status::code) {
            None => AuthorizedOutcome::Success,
            Some(tonic::Code::DeadlineExceeded) => AuthorizedOutcome::Deadline,
            Some(tonic::Code::Cancelled) => AuthorizedOutcome::Cancelled,
            _ => AuthorizedOutcome::Error,
        });
        result
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
        request_permit: OwnedSemaphorePermit,
        pipeline_started: Instant,
        timing: RequestTiming,
    ) -> Result<CompletedPipeline, Status> {
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
            let result = span.in_scope(|| {
                validate_and_settle(
                    &state,
                    blocks,
                    submitted_post_batch_calldata,
                    submitted_blobs,
                    maximum_blobs,
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

/// Only transport status categories enter metrics; request text and evidence do not.
pub(super) fn stage_outcome(status: Option<&Status>) -> eez_telemetry::operational::StageOutcome {
    use eez_telemetry::operational::StageOutcome;
    match status.map(Status::code) {
        None | Some(tonic::Code::Ok) => StageOutcome::Success,
        Some(
            tonic::Code::InvalidArgument | tonic::Code::OutOfRange | tonic::Code::ResourceExhausted,
        ) => StageOutcome::Invalid,
        Some(
            tonic::Code::FailedPrecondition
            | tonic::Code::PermissionDenied
            | tonic::Code::Unauthenticated,
        ) => StageOutcome::Rejected,
        Some(tonic::Code::Unavailable) => StageOutcome::Unavailable,
        Some(tonic::Code::DeadlineExceeded) => StageOutcome::Deadline,
        Some(tonic::Code::Cancelled) => StageOutcome::Cancelled,
        _ => StageOutcome::Error,
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

#[cfg(test)]
mod metric_tests {
    use super::*;
    #[test]
    fn rejection_labels_follow_status_categories_without_error_text() {
        assert_eq!(stage_outcome(None).label(), "success");
        for (code, label) in [
            (tonic::Code::InvalidArgument, "invalid"),
            (tonic::Code::ResourceExhausted, "invalid"),
            (tonic::Code::FailedPrecondition, "rejected"),
            (tonic::Code::Unavailable, "unavailable"),
            (tonic::Code::DeadlineExceeded, "deadline"),
            (tonic::Code::Cancelled, "cancelled"),
            (tonic::Code::Internal, "internal"),
        ] {
            assert_eq!(
                stage_outcome(Some(&Status::new(code, "untrusted secret/tx evidence"))).label(),
                label
            );
        }
    }
}
