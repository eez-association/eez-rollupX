//! Stateful per-source-tx target-chain execution session.
//!
//! [`LocalExecutionSession`] accumulates target-chain state across
//! calls within one source transaction. It drives a direct call to the
//! destination contract with the proxy address as `msg.sender`
//! (CREATE2-derived).
//!
//! Also hosts the reth helper [`disable_checks`].

use alloy_primitives::{Address, Bytes, U256};
use alloy_sol_types::SolCall;

use super::state::SimState;
use alloy_consensus::{TxLegacy, transaction::Recovered};
use eez_evm::{EezEvmConfig, EezEvmTx};
use eez_evm_inspector::{OverlayChannelHandle, OverlaySnapshot, SessionInspectorFactory};
use evm2::{
    EvmFeatures, EvmTypesHost, Inspector, PendingState, evm::DynDatabase, interpreter::Interpreter,
};
use reth_evm::ConfigureEvm;

use eez_protocol::{
    CallMode, CompositionBuilder, ExecutionOutcome, ExecutionRequest, ExecutorError,
    ExecutorErrorKind, ExecutorResult, RollupId, TargetExecutionSession,
    abi::computeCrossChainProxyAddressCall,
};

use super::provider::ChainProvider;
use super::slot::transact_err;
use crate::composer::gas_trace;

/// Gas cap for simulated direct target calls; exhaustion is returned as an
/// unsuccessful execution outcome.
pub(super) const DIRECT_CALL_GAS_LIMIT: u64 = 30_000_000;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ForceStaticInspector;

impl<T: EvmTypesHost> Inspector<T> for ForceStaticInspector {
    fn initialize_interp(&mut self, interp: &mut Interpreter<'_, '_, T>) {
        interp.set_static(true);
    }
}

/// Stateful target-chain execution session.
///
/// Opened by `LocalChainClient::begin_execution_session` (the
/// `ChainClient` trait method, not an inherent fn); owned by the
/// `CompositionBuilder` through a `Rollup.session` slot for the
/// lifetime of one composition.
///
/// ## Limitation: direct call, not full manager path
///
/// The session calls the destination contract directly with the proxy
/// address as `msg.sender` (computed via CREATE2). This gives the
/// source simulation synchronous return data, but it does not reproduce
/// the full `executeIncomingCrossChainCall` path.
pub struct LocalExecutionSession {
    evm_config: EezEvmConfig,
    state: SimState,
    evm_env: reth_evm::EvmEnvFor<EezEvmConfig>,
    chain_id: u64,
    manager_address: Address,
    /// Optional factory for inspecting nested proxy calls. `None` disables
    /// nested-call detection for this session.
    inspector_factory: Option<SessionInspectorFactory>,
    /// This rollup's cache channel for propagating state through re-entry.
    overlay_channel: OverlayChannelHandle,
}

impl std::fmt::Debug for LocalExecutionSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalExecutionSession")
            .field("chain_id", &self.chain_id)
            .field("manager_address", &self.manager_address)
            .field("has_inspector_factory", &self.inspector_factory.is_some())
            .finish_non_exhaustive()
    }
}

impl LocalExecutionSession {
    /// Create from a `ChainProvider`. An explicit parent/environment pins lazy
    /// re-entry to a wallet probe; ordinary composition retains latest state.
    ///
    /// When supplied, `inspector_factory` detects nested proxy calls and routes
    /// them through the composition builder. `None` executes without nested
    /// call inspection.
    ///
    /// `cache` preloads in-flight state captured before a nested dispatch;
    /// `None` opens a fresh native cache. `overlay_channel` publishes cumulative
    /// state after execution so a suspended inspector can apply the diff.
    /// A quote context pins both cold state reads and block opcodes. Reentrant
    /// frames must not open `latest` after their source has begun. Normal
    /// composition supplies no override and keeps its existing latest-state path.
    ///
    /// # Errors
    ///
    /// Returns [`ExecutorErrorKind::Provider`] if reading the latest
    /// block number or header fails, or if opening the state provider
    /// fails.
    pub(crate) fn new(
        provider: &ChainProvider,
        manager_address: Address,
        inspector_factory: Option<SessionInspectorFactory>,
        cache: Option<OverlaySnapshot>,
        overlay_channel: OverlayChannelHandle,
        pinned: Option<(alloy_primitives::B256, reth_evm::EvmEnvFor<EezEvmConfig>)>,
    ) -> ExecutorResult<Self> {
        let (state_prov, mut evm_env) = if let Some((hash, env)) = pinned {
            (
                provider
                    .provider
                    .state_by_block_hash(hash)
                    .map_err(provider_err)?,
                env,
            )
        } else {
            let num = provider.headers.best_block_number().map_err(provider_err)?;
            tracing::debug!(block = num, "target session: best block number");

            let header = provider
                .headers
                .header_by_number(num)
                .map_err(provider_err)?
                .ok_or_else(|| ExecutorError::from(ExecutorErrorKind::Missing("target header")))?;
            tracing::debug!(
                block = num,
                state_root = %header.state_root,
                "target session: opened header"
            );

            let state_prov = provider.provider.latest().map_err(provider_err)?;

            let evm_env = provider.evm_config.evm_env(&header).map_err(evm_err)?;
            (state_prov, evm_env)
        };
        let chain_id = evm_env.version.chain_id;
        disable_checks(&mut evm_env);

        let state = SimState::new(state_prov, cache);

        tracing::debug!(
            chain_id,
            %manager_address,
            "target session: ready"
        );

        Ok(Self {
            evm_config: provider.evm_config.clone(),
            state,
            evm_env,
            chain_id,
            manager_address,
            inspector_factory,
            overlay_channel,
        })
    }

    /// Compute the target-chain proxy address for a given
    /// `(sourceAddress, sourceRollup)` via a static call to
    /// the chain-local manager's `computeCrossChainProxyAddress()`.
    fn compute_proxy_address(
        &mut self,
        source_address: Address,
        source_rollup: RollupId,
    ) -> ExecutorResult<Address> {
        let calldata = computeCrossChainProxyAddressCall {
            originalAddress: source_address,
            originalRollupId: source_rollup.0,
        }
        .abi_encode();

        let tx = Recovered::new_unchecked(
            EezEvmTx::Ethereum(evm2::ethereum::TxEnvelope::Legacy(TxLegacy {
                gas_limit: 1_000_000,
                to: alloy_primitives::TxKind::Call(self.manager_address),
                input: calldata.into(),
                chain_id: Some(self.chain_id),
                ..Default::default()
            })),
            Address::ZERO,
        );
        let result = self
            .state
            .transact(&self.evm_config, &self.evm_env, &tx, None)
            .map_err(transact_err)?
            .result;
        if !result.is_success() {
            return Err(ExecutorErrorKind::Unavailable(format!(
                "computeCrossChainProxyAddress did not succeed on manager {}",
                self.manager_address
            ))
            .into());
        }
        let output = result.output;
        // Revert bytes and noncanonical addresses must never select a caller.
        let malformed = || {
            ExecutorErrorKind::Unavailable(format!(
                "manager {} returned malformed computeCrossChainProxyAddress output",
                self.manager_address
            ))
        };
        if output.len() != 32 {
            return Err(malformed().into());
        }
        computeCrossChainProxyAddressCall::abi_decode_returns_validate(&output)
            .map_err(|_| malformed().into())
    }

    /// Uninspected direct-call path. Executes the call and restores the
    /// simulated proxy caller's nonce before committing the result.
    fn execute_internal(
        &mut self,
        destination: &Address,
        calldata: &Bytes,
        value: &U256,
        source_address: &Address,
        source_rollup: RollupId,
    ) -> ExecutorResult<eez_protocol::ExecutionOutcome> {
        let tx_env =
            self.build_tx_env(destination, calldata, value, source_address, source_rollup)?;
        let caller = tx_env.signer();
        let mut inspector = crate::composer::quote_trace::inspect(
            evm2::evm::inspector::NoopInspector::default(),
            self.chain_id,
            "composition",
            &tx_env,
        );
        let tracing = inspector.is_capturing();
        let result = self
            .state
            .transact(
                &self.evm_config,
                &self.evm_env,
                &tx_env.map(EezEvmTx::Funded),
                tracing.then_some(&mut inspector as &mut dyn Inspector<eez_evm::EezEvmTypes>),
            )
            .map_err(transact_err)?;
        Ok(self.commit_and_finish(
            result.result.output.clone(),
            result.result.tx_gas_used(),
            result.result.is_success(),
            result.pending_state,
            caller,
        ))
    }

    /// Inspected direct-call path. Runs the target-chain tx under the
    /// supplied [`eez_evm_inspector::SessionInspector`] so proxy CALLs detected
    /// during execution dispatch through the composition builder.
    ///
    /// The session owns the inspector and lends it to the synchronous EVM pass.
    /// Its error is checked before accepting any effects.
    fn execute_internal_with_inspector(
        &mut self,
        mut inspector: eez_evm_inspector::SessionInspector<'_>,
        destination: &Address,
        calldata: &Bytes,
        value: &U256,
        source_address: &Address,
        source_rollup: RollupId,
    ) -> ExecutorResult<eez_protocol::ExecutionOutcome> {
        let tx_env =
            self.build_tx_env(destination, calldata, value, source_address, source_rollup)?;
        let caller = tx_env.signer();
        let result = {
            let traced = gas_trace::maybe_trace(&mut inspector, self.chain_id);
            let mut traced = crate::composer::quote_trace::inspect(
                traced,
                self.chain_id,
                "composition",
                &tx_env,
            );
            self.state
                .transact(
                    &self.evm_config,
                    &self.evm_env,
                    &tx_env.map(EezEvmTx::Funded),
                    Some(&mut traced),
                )
                .map_err(transact_err)?
        };
        if let Some(error) = inspector.take_error() {
            return Err(error);
        }
        Ok(self.commit_and_finish(
            result.result.output.clone(),
            result.result.tx_gas_used(),
            result.result.is_success(),
            result.pending_state,
            caller,
        ))
    }

    fn execute_static(
        &mut self,
        req: &ExecutionRequest,
        dispatcher: &mut CompositionBuilder,
    ) -> ExecutorResult<ExecutionOutcome> {
        if !req.value.is_zero() {
            return Err(ExecutorErrorKind::Encoding(
                "static target execution carries non-zero value".into(),
            )
            .into());
        }
        let tx_env = self.build_tx_env(
            &req.target_address,
            &req.data,
            &req.value,
            &req.source_address,
            req.source_rollup_id,
        )?;
        if dispatcher.executing_static_read_callback() {
            self.require_deployed_callback_proxy(tx_env.signer())?;
        }
        let call = tx_env
            .clone()
            .map(|tx| EezEvmTx::Ethereum(evm2::ethereum::TxEnvelope::Legacy(tx)));
        let (result, inspector_error) = if let Some(factory) = self.inspector_factory.clone() {
            let mut inspector = factory.build_static(dispatcher, &self.state.snapshot());
            let result = {
                let traced = gas_trace::maybe_trace(&mut inspector, self.chain_id);
                let mut traced = crate::composer::quote_trace::inspect(
                    traced,
                    self.chain_id,
                    "composition",
                    &tx_env,
                );
                self.state
                    .transact(&self.evm_config, &self.evm_env, &call, Some(&mut traced))
                    .map_err(transact_err)?
                    .result
            };
            (result, inspector.take_error())
        } else {
            let mut inspector = ForceStaticInspector;
            let traced = gas_trace::maybe_trace(&mut inspector, self.chain_id);
            let mut traced = crate::composer::quote_trace::inspect(
                traced,
                self.chain_id,
                "composition",
                &tx_env,
            );
            (
                self.state
                    .transact(&self.evm_config, &self.evm_env, &call, Some(&mut traced))
                    .map_err(transact_err)?
                    .result,
                None,
            )
        };
        if let Some(error) = inspector_error {
            return Err(error);
        }
        Ok(ExecutionOutcome::Resolved {
            return_data: result.output.to_vec(),
            gas_used: result.tx_gas_used(),
            success: result.is_success(),
        })
    }

    /// Reject a static-read callback whose source proxy has no code here.
    /// Simulation executes the callback with the proxy's address as caller
    /// whether or not it exists, but the protocol cannot deploy it inside the
    /// read's static frame and reverts the read instead.
    fn require_deployed_callback_proxy(&mut self, proxy: Address) -> ExecutorResult<()> {
        let account = self
            .state
            .db
            .get_account(&proxy)
            .map_err(|error| ExecutorErrorKind::Provider(Box::new(error)))?;
        if account.is_some_and(|account| !account.is_empty_code_hash()) {
            return Ok(());
        }
        Err(ExecutorErrorKind::StaticCallbackProxyMissing {
            proxy,
            chain_id: self.chain_id,
        }
        .into())
    }

    /// Commit the pending state, publish the overlay cache, and return the
    /// execution outcome.
    fn commit_and_finish(
        &mut self,
        return_data: Bytes,
        gas_used: u64,
        success: bool,
        changes: PendingState,
        caller: Address,
    ) -> eez_protocol::ExecutionOutcome {
        self.state.commit_frame(changes, caller);

        // Publish the cumulative post-execute cache into the suspended
        // dispatch frame of this rollup. Its inspector applies the latest
        // result after the dispatch returns.
        if !self.overlay_channel.publish(self.state.snapshot()) {
            tracing::trace!("no suspended dispatch frame; overlay result dropped");
        }

        tracing::debug!(
            success = success,
            gas = gas_used,
            return_len = return_data.len(),
            "target call completed"
        );

        eez_protocol::ExecutionOutcome::Resolved {
            return_data: return_data.to_vec(),
            gas_used,
            success,
        }
    }

    /// Build the `TxEnv` for a direct call on the target chain. Shared
    /// between inspected and uninspected paths so the transaction input shape
    /// is identical regardless of inspection policy.
    fn build_tx_env(
        &mut self,
        destination: &Address,
        calldata: &Bytes,
        value: &U256,
        source_address: &Address,
        source_rollup: RollupId,
    ) -> ExecutorResult<Recovered<TxLegacy>> {
        let caller = self.compute_proxy_address(*source_address, source_rollup)?;

        tracing::trace!(
            dest = %destination,
            proxy_caller = %caller,
            source_addr = %source_address,
            %source_rollup,
            value = %value,
            calldata_len = calldata.len(),
            "executing direct call on target chain");

        Ok(Recovered::new_unchecked(
            TxLegacy {
                gas_limit: DIRECT_CALL_GAS_LIMIT,
                to: alloy_primitives::TxKind::Call(*destination),
                input: calldata.clone(),
                value: *value,
                chain_id: Some(self.chain_id),
                ..Default::default()
            },
            caller,
        ))
    }
}

/// Reject a cross-chain call whose destination is the executing chain's
/// manager. Protocol managers refuse it in every dispatched call array
/// (`EEZDestinationForbidden`). The check also covers static calls, including a
/// top-level static lookup the managers would resolve: this admission policy is
/// deliberately narrower, since reading a manager cross-chain has no supported
/// use. Every target session applies it before simulating the call.
pub(crate) fn reject_manager_destination(
    req: &ExecutionRequest,
    manager: Address,
    chain_id: u64,
) -> ExecutorResult<()> {
    if req.target_address == manager {
        return Err(ExecutorErrorKind::ManagerDestination { manager, chain_id }.into());
    }
    Ok(())
}

impl TargetExecutionSession for LocalExecutionSession {
    fn execute(
        &mut self,
        req: ExecutionRequest,
        dispatcher: &mut CompositionBuilder,
    ) -> ExecutorResult<ExecutionOutcome> {
        reject_manager_destination(&req, self.manager_address, self.chain_id)?;
        if req.call_mode == CallMode::Static {
            return self.execute_static(&req, dispatcher);
        }
        let outcome = if let Some(factory) = self.inspector_factory.clone() {
            let inspector = factory.build(dispatcher, &self.state.snapshot());
            self.execute_internal_with_inspector(
                inspector,
                &req.target_address,
                &req.data,
                &req.value,
                &req.source_address,
                req.source_rollup_id,
            )?
        } else {
            let _ = dispatcher;
            self.execute_internal(
                &req.target_address,
                &req.data,
                &req.value,
                &req.source_address,
                req.source_rollup_id,
            )?
        };

        Ok(outcome)
    }

    fn checkpoint(&mut self) -> ExecutorResult<eez_protocol::SessionSnapshot> {
        // A retained callback session may predate writes made by the resumed
        // source. Refresh before saving the rollback point, while preserving
        // cumulative sibling-callback state within the same dispatch frame.
        let mut snapshot = self.state.take_snapshot();
        self.overlay_channel.refresh_before_reentry(&mut snapshot);
        self.state.restore(snapshot.clone());
        // Synthetic target execution reads and writes through the cache. Keep
        // the exact pre-call cache so a source-frame revert can rewind this
        // chain before a later sibling dispatch observes it. Transition and
        // bundle bookkeeping are not consumed from these throwaway sessions;
        // accepted effects are propagated through `overlay_channel` and are
        // replayed canonically by the eventual system transaction.
        Ok(Box::new(snapshot) as Box<dyn std::any::Any + Send>)
    }

    fn rollback(&mut self, snapshot: &eez_protocol::SessionSnapshot) -> ExecutorResult<()> {
        let cache = snapshot.downcast_ref::<OverlaySnapshot>().ok_or_else(|| {
            ExecutorError::from(ExecutorErrorKind::Encoding(
                "LocalExecutionSession::rollback: snapshot type mismatch".into(),
            ))
        })?;
        self.state.restore(cache.clone());
        Ok(())
    }
}

pub(super) fn disable_checks(env: &mut reth_evm::EvmEnvFor<EezEvmConfig>) {
    // Intercepted composition frames are calls, not charged L2 transactions.
    // Only the separately executed native/user body distributes actual fees.
    eez_evm::disable_fee_charging(&mut env.version);
    env.version.features.remove(
        EvmFeatures::BASE_FEE_CHECK
            | EvmFeatures::NONCE_CHECK
            | EvmFeatures::EIP3607
            | EvmFeatures::BLOCK_GAS_LIMIT_CHECK,
    );
    env.version.tx_gas_limit_cap = u64::MAX;
    env.version.tx_total_gas_cap = u64::MAX;
}

pub(super) fn provider_err(
    e: impl Into<Box<dyn std::error::Error + Send + Sync>>,
) -> ExecutorError {
    ExecutorError::provider(e)
}

pub(super) fn evm_err(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> ExecutorError {
    ExecutorError::evm(e)
}

#[cfg(test)]
mod manager_destination_tests {
    use super::*;

    fn request(call_mode: CallMode, target_address: Address) -> ExecutionRequest {
        ExecutionRequest {
            call_mode,
            target_address,
            data: Bytes::new(),
            value: U256::ZERO,
            source_address: Address::repeat_byte(0x11),
            source_rollup_id: RollupId(0),
        }
    }

    #[test]
    fn manager_destination_is_rejected_for_mutable_and_static_calls() {
        let manager = Address::repeat_byte(0x4b);
        for call_mode in [CallMode::Mutable, CallMode::Static] {
            let error = reject_manager_destination(&request(call_mode, manager), manager, 7)
                .expect_err("a call to the destination manager is rejected");
            assert!(
                matches!(
                    error.kind(),
                    ExecutorErrorKind::ManagerDestination { manager: m, chain_id: 7 } if *m == manager
                ),
                "{error}"
            );
            reject_manager_destination(&request(call_mode, Address::repeat_byte(0x22)), manager, 7)
                .expect("other destinations are allowed");
        }
    }
}
