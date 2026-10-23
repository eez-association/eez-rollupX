//! Capability discovery and bounded-worker cancellation shared by v2.
use super::ProveSvc;
use crate::cancel::CancellationToken;
use eez_control_rpc::v1::prover_server::Prover;
use tokio::task::AbortHandle;
use tonic::{Request, Response, Status};

#[tonic::async_trait]
impl Prover for ProveSvc {
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
            profile_id: eez_control_rpc::OFFLINE_PROFILE_ID,
            validation_identity: self.state.validation_identity.to_vec(),
            max_window_blocks: window.blocks as u64,
            max_request_bytes: window.payload_bytes as u64,
            max_request_witness_items: window.witness_items as u64,
            max_message_bytes: limits.max_decoding_message_bytes() as u64,
            stream_idle_timeout_millis: u64::try_from(limits.stream_idle_timeout().as_millis())
                .map_err(|_| Status::internal("stream idle timeout exceeds capability range"))?,
            request_timeout_millis: u64::try_from(limits.request_timeout().as_millis())
                .map_err(|_| Status::internal("request timeout exceeds capability range"))?,
        }))
    }
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
