//! Shared channel and locally checked capability snapshot.

use std::sync::Arc;
use std::time::Duration;

use eez_control_rpc::v1::{CapabilitiesRequest, CapabilitiesResponse, prover_client::ProverClient};
use eez_prover::{ProverError, ProverResult, RetryableProverError};
use tonic::transport::{Channel, Endpoint};

use super::RemoteProver;

#[derive(Debug)]
pub(super) struct Connection {
    pub(super) client: ProverClient<Channel>,
    pub(super) capabilities: CapabilitiesResponse,
}

impl RemoteProver {
    /// Connect and compare the endpoint's rules with the locally loaded chain
    /// document before any producer work relies on its resource limits.
    pub async fn capabilities(&self) -> ProverResult<CapabilitiesResponse> {
        Ok(self.connection().await?.capabilities.clone())
    }

    #[tracing::instrument(name = "prover_capabilities", target = "eez_trace", skip_all, fields(trace_id = tracing::field::Empty, otel.kind = "client"))]
    pub(super) async fn connection(&self) -> ProverResult<Arc<Connection>> {
        let mut slot = self.inner.connection.lock().await;
        if let Some(connection) = slot.as_ref() {
            return Ok(Arc::clone(connection));
        }
        let endpoint = Endpoint::from_shared(self.inner.url.clone())
            .map_err(|error| ProverError::Backend(format!("invalid prover endpoint: {error}")))?
            .connect_timeout(Duration::from_secs(10));
        let channel = endpoint
            .connect()
            .await
            .map_err(|error| unavailable(error.to_string()))?;
        let mut client = ProverClient::new(channel)
            .max_encoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES)
            .max_decoding_message_size(1024);
        let capabilities = tokio::time::timeout(
            Duration::from_secs(10),
            client.capabilities(super::traced_request(CapabilitiesRequest {})),
        )
        .await
        .map_err(|_| unavailable("capability discovery timed out".into()))?
        .map_err(|status| {
            super::map_rpc_status(0, 0, tonic::Status::new(status.code(), status.message()))
        })?
        .into_inner();
        validate(&capabilities, self.inner.expected_identity)?;
        let mut agreed = self.inner.agreed_capabilities.lock().await;
        if let Some(previous) = agreed.as_ref() {
            if previous != &capabilities {
                return Err(ProverError::Backend(
                    "prover capabilities changed; producer restart is required to re-plan limits"
                        .into(),
                ));
            }
        } else {
            *agreed = Some(capabilities.clone());
        }
        drop(agreed);
        let maximum = usize::try_from(capabilities.max_message_bytes).map_err(|_| {
            ProverError::Backend("prover message limit exceeds platform capacity".into())
        })?;
        let connection = Arc::new(Connection {
            client: client.max_encoding_message_size(maximum),
            capabilities,
        });
        *slot = Some(Arc::clone(&connection));
        Ok(connection)
    }

    /// An older in-flight request must not discard a newer successful handshake.
    pub(super) async fn invalidate_connection(&self, failed: &Arc<Connection>) {
        let mut slot = self.inner.connection.lock().await;
        if slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, failed))
        {
            *slot = None;
        }
    }
}

fn unavailable(message: String) -> ProverError {
    ProverError::Retryable {
        kind: RetryableProverError::Unavailable,
        message,
    }
}

fn validate(
    capabilities: &CapabilitiesResponse,
    expected: alloy_primitives::B256,
) -> ProverResult<()> {
    if capabilities.operations_tag != 3 || capabilities.profile_id != 1 {
        return Err(ProverError::Backend(
            "prover operations tag/profile mismatch".into(),
        ));
    }
    if capabilities.validation_identity.as_slice() != expected.as_slice() {
        return Err(ProverError::Backend(
            "prover validation identity differs from local execution rules".into(),
        ));
    }
    if !(1..=65_536).contains(&capabilities.max_window_blocks)
        || capabilities.max_request_bytes == 0
        || capabilities.max_request_witness_items == 0
        || capabilities.max_message_bytes == 0
        || capabilities.max_message_bytes > capabilities.max_request_bytes
        || capabilities.max_message_bytes > 268_435_456
        || !(1..=64).contains(&capabilities.preparation_max_blocks)
        || capabilities.stream_idle_timeout_millis == 0
        || capabilities.request_timeout_millis == 0
        || capabilities.cache_max_entries == 0
        || capabilities.cache_max_bytes == 0
        || capabilities.maximum_leases != eez_control_rpc::MAX_PREPARATION_LEASES
        || capabilities.lease_lifetime_millis
            < capabilities
                .request_timeout_millis
                .saturating_mul(2)
                .max(120_000)
    {
        return Err(ProverError::Backend(
            "prover returned invalid effective limits".into(),
        ));
    }
    Ok(())
}
