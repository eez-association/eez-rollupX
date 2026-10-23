//! The `Prove` RPC service: bounded ingestion, validation, and attestation.
//!
//! V2 retains incremental execution in bounded sessions and uses
//! [`crate::validate`] and [`crate::settlement`] to bind the native batch to
//! checked execution. The attester signs only after every stage succeeds.

use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy_primitives::{Address, B256};
use eez_control_rpc::v1::prover_server::ProverServer;
use tokio::sync::Semaphore;

use crate::{attest::Attester, settlement, validate, window};

pub(crate) mod l1;
mod rpc;
mod sessions;
mod settlement_job;

pub(crate) use settlement_job::AttestablePublicInputsHash;

/// Absolute ceiling for one decoded v2 client frame; the configured aggregate
/// request-byte limit may lower the effective per-message limit.
const MAX_DECODING_MESSAGE_BYTES: usize = 256 * 1024 * 1024;

/// Capabilities, bounded acknowledgements and proofs carry no execution witness.
const MAX_ENCODING_MESSAGE_BYTES: usize = 1024;

/// Named construction inputs for [`ServiceLimits`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct ServiceLimitsParams {
    pub(crate) max_window_blocks: NonZeroUsize,
    pub(crate) max_window_bytes: NonZeroUsize,
    pub(crate) max_window_witness_items: NonZeroUsize,
    pub(crate) stream_idle_timeout: Duration,
    pub(crate) request_timeout: Duration,
}

/// Runtime limits shared by every service clone.
///
/// Deadlines cover each
/// ordered operation. Idle timeouts bound inactive input and output backpressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ServiceLimits {
    window_limits: window::WindowLimits,
    stream_idle_timeout: Duration,
    request_timeout: Duration,
}

impl ServiceLimits {
    pub(crate) fn new(params: ServiceLimitsParams) -> eyre::Result<Self> {
        let ServiceLimitsParams {
            max_window_blocks,
            max_window_bytes,
            max_window_witness_items,
            stream_idle_timeout,
            request_timeout,
        } = params;
        let now = Instant::now();
        for (name, timeout) in [
            ("stream idle timeout", stream_idle_timeout),
            ("request timeout", request_timeout),
        ] {
            eyre::ensure!(!timeout.is_zero(), "{name} must be non-zero");
            eyre::ensure!(now.checked_add(timeout).is_some(), "{name} is out of range");
            eyre::ensure!(
                u64::try_from(timeout.as_millis()).is_ok(),
                "{name} exceeds the capability millisecond range"
            );
        }
        Ok(Self {
            window_limits: window::WindowLimits {
                blocks: max_window_blocks.get().min(u32::MAX as usize),
                payload_bytes: max_window_bytes.get(),
                witness_items: max_window_witness_items.get(),
            },
            stream_idle_timeout,
            request_timeout,
        })
    }

    /// Aggregate window quotas enforced during stream admission.
    pub(crate) fn window_limits(self) -> window::WindowLimits {
        self.window_limits
    }

    pub(crate) fn stream_idle_timeout(self) -> Duration {
        self.stream_idle_timeout
    }

    pub(crate) fn request_timeout(self) -> Duration {
        self.request_timeout
    }

    pub(crate) fn max_decoding_message_bytes(self) -> usize {
        self.window_limits
            .payload_bytes
            .min(MAX_DECODING_MESSAGE_BYTES)
    }
}

/// Immutable dependencies shared by all service clones.
#[derive(Debug)]
pub(crate) struct ServiceState {
    validator: validate::Validator,
    validation_identity: B256,
    expected_rollup_id: NonZeroU64,
    expected_l2_system_address: Address,
    attester: Attester,
    system_transaction_reconstructor: settlement::SystemTransactionReconstructor,
    l1_chain_spec: Arc<reth_chainspec::ChainSpec>,
    offline_rules: eez_l1::chain_config::OfflineL1Rules,
    deployment: l1::OfflineDeployment,
}

impl ServiceState {
    /// Bind system-transaction reconstruction to the same configured L2 chain
    /// and expected rollup identities used by validation and settlement.
    pub(crate) fn new(
        registry: Address,
        manager: Address,
        validator: validate::Validator,
        expected_rollup_id: NonZeroU64,
        attester: Attester,
        l1_chain_spec: Arc<reth_chainspec::ChainSpec>,
    ) -> eyre::Result<Self> {
        let expected_l2_system_address = eez_primitives::SYSTEM_ADDRESS;
        eyre::ensure!(
            validator.expected_l2_system_address() == expected_l2_system_address,
            "validator does not use the native L2 system address"
        );
        eyre::ensure!(
            attester.expected_l2_system_address() == expected_l2_system_address,
            "attester does not use the native L2 system address"
        );
        let system_transaction_reconstructor = settlement::SystemTransactionReconstructor::new(
            validator.chain_id(),
            expected_rollup_id,
        );
        let offline_rules = eez_l1::chain_config::offline_l1_rules(&l1_chain_spec)?;
        let schedule = validator.schedule()?;
        let profile = offline_rules.profile(schedule, validator.chain_id())?;
        let deployment = l1::OfflineDeployment::new(registry, manager, profile, schedule)?;
        let validation_identity = validator.validation_identity(expected_rollup_id, &deployment);
        Ok(Self {
            validator,
            validation_identity,
            expected_rollup_id,
            expected_l2_system_address,
            attester,
            system_transaction_reconstructor,
            l1_chain_spec,
            offline_rules,
            deployment,
        })
    }

    fn posting_blob_limit(&self, timestamp: u64) -> Result<usize, tonic::Status> {
        use reth_chainspec::EthChainSpec as _;
        // The actual-parent witness ends before inclusion. Its header layout
        // may still be supported when the target is the first unsupported fork
        // block; validating supplied headers alone cannot close that boundary.
        if self
            .offline_rules
            .headers
            .unsupported_time
            .is_some_and(|at| timestamp >= at)
        {
            return Err(tonic::Status::invalid_argument(
                "unsupported L1 rule period at posting target",
            ));
        }
        let chain_spec = &self.l1_chain_spec;
        let params = chain_spec
            .blob_params_at_timestamp(timestamp)
            .ok_or_else(|| {
                tonic::Status::invalid_argument("posting target has no active blob rules")
            })?;
        let maximum = eez_blob_codec::BlobLimits::from_params(params)
            .and_then(|limits| limits.outgoing(0, u64::MAX, None))
            .map_err(|error| tonic::Status::invalid_argument(error.to_string()))?;
        usize::try_from(maximum).map_err(|_| {
            tonic::Status::invalid_argument("L1 blob allowance exceeds platform capacity")
        })
    }
}

/// The `Prove` service. All cheap clones share state and one active-request slot.
#[derive(Debug, Clone)]
pub(crate) struct ProveSvc {
    sessions: Arc<sessions::SessionRuntime>,
    state: Arc<ServiceState>,
    limits: ServiceLimits,
    active_request_slot: Arc<Semaphore>,
    validation_worker: Arc<Semaphore>,
}

impl ProveSvc {
    pub(crate) fn new(state: Arc<ServiceState>, limits: ServiceLimits) -> Self {
        let validation_worker = Arc::new(Semaphore::new(1));
        let active_request_slot = Arc::new(Semaphore::new(1));
        let sessions = sessions::SessionRuntime::new(
            Arc::clone(&state),
            limits,
            Arc::clone(&validation_worker),
            Arc::clone(&active_request_slot),
        );
        Self {
            sessions,
            state,
            limits,
            active_request_slot,
            validation_worker,
        }
    }

    /// Close long-lived v2 attachments before tonic drains its connections.
    pub(crate) fn stop_streams(&self) {
        self.sessions.stop_streams();
    }

    /// Wait until no admitted request or detached request worker holds the slot.
    ///
    /// Observes idleness by acquiring the active-request slot, so it must run only
    /// after the listener stops accepting requests; called while serving it
    /// would briefly occupy the slot and reject an incoming `Prove`.
    pub(crate) async fn wait_until_idle(&self) {
        self.sessions.wait_until_idle().await;
        drop(
            self.active_request_slot
                .acquire()
                .await
                .expect("the active-request semaphore is never closed"),
        );
        drop(
            self.validation_worker
                .acquire()
                .await
                .expect("the validation-worker semaphore is never closed"),
        );
    }

    pub(crate) fn streaming_server(
        &self,
    ) -> eez_control_rpc::v2::prover_server::ProverServer<Self> {
        eez_control_rpc::v2::prover_server::ProverServer::new(self.clone())
            .max_decoding_message_size(self.limits.max_decoding_message_bytes())
            .max_encoding_message_size(MAX_ENCODING_MESSAGE_BYTES)
    }

    /// Build the gRPC server with the configured request and response size limits.
    pub(crate) fn into_server(self) -> ProverServer<Self> {
        let message_bytes = self.limits.max_decoding_message_bytes();
        ProverServer::new(self)
            .max_decoding_message_size(message_bytes)
            .max_encoding_message_size(MAX_ENCODING_MESSAGE_BYTES)
    }
}

#[cfg(test)]
mod tests;
