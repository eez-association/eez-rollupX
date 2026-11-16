//! [`SessionInspector`] detects cross-chain proxy calls during EVM execution
//! and dispatches each one through a borrowed [`CompositionBuilder`].
//!
//! When the inspector sees a CALL opcode:
//! 1. `lookup_authorized_proxy_live` reads `authorizedProxies[addr]`
//!    from the **live EVM state** (EVM journal + DB) — so proxies
//!    registered by earlier calls in the same transaction, or by
//!    earlier transactions in the same in-progress block, are visible.
//! 2. If the address is a registered proxy,
//!    [`CompositionBuilder::dispatch_call`] forwards the call to the target
//!    session and returns the execution response.
//! 3. The inspector synthesizes the outcome back into the source EVM so
//!    the source transaction's control flow can continue.
//!
//! Errors surfaced by dispatch are stashed locally so the caller can
//! consult them after the EVM pass without unwinding the EVM machine.
//!
//! # Why live EVM state
//!
//! The lookup must see writes made earlier in the same transaction
//! (e.g. `Bridge.bridgeEther` deploys a CREATE2 proxy and calls it
//! inside one tx) and earlier in the same in-progress block
//! (multi-tx composition). Reading through the EVM journal captures
//! both; a pre-tx storage snapshot would not.
//!
use crate::overlay_state::OverlaySnapshot;
use alloy_primitives::{Address, Bytes};
use evm2::{
    EvmTypes, ExecutionError, HostError,
    evm::{Evm, inspector::Inspector},
    interpreter::{GasTracker, Host, InstrStop, Interpreter, Message, MessageKind, MessageResult},
};

use crate::overlay::OverlayChannelHandle;
use eez_protocol::{CallMode, CompositionBuilder, ExecutorError, ProxyLookupConfig, RollupId};
use eez_protocol::{ProxyInfo, decode_proxy_value, proxy_mapping_key};

/// Bytes of a cross-chain call's return or revert data kept in its log line.
const LOGGED_RETURN_BYTES: usize = 256;

/// Look up `authorizedProxies[addr]` from the live EVM state at the
/// hook site.
///
/// Goes through the EVM journal (so writes by earlier calls or earlier
/// txs in the same in-progress block are visible) and falls back to the
/// underlying DB for cold slots. Returns `None` for unregistered proxies
/// and propagates read errors so the caller can reject the simulation before
/// accepting effects. An unavailable mapping cannot authorize ordinary execution.
///
/// # Account warming
///
/// The host's `sload` requires the target account to already be present in
/// the journal; it's the normal precondition when the EVM does an SLOAD
/// while executing a contract. The inspector fires on every CALL —
/// including calls to destinations that have never had their state
/// touched before — so we can't assume the rollups contract is warm.
/// We explicitly load it; a no-op if it was already warm.
fn lookup_authorized_proxy_live<T: EvmTypes>(
    context: &mut Evm<'_, T>,
    lookup: &ProxyLookupConfig,
    addr: Address,
) -> Result<Option<ProxyInfo>, HostError> {
    context.load_account(&lookup.contract_address, false, false)?;
    let key = proxy_mapping_key(addr, lookup.authorized_proxies_slot);
    let load = context.sload(&lookup.contract_address, &key.into(), false)?;
    Ok(decode_proxy_value(load.value))
}

/// EVM inspector that detects proxy calls and dispatches them through
/// the borrowed [`CompositionBuilder`].
///
/// After the EVM pass, the caller consults `take_error()` to surface
/// any dispatch failure; the dispatcher already holds the recorded calls.
///
/// # Frame bracketing
///
/// The inspector brackets every EVM CALL frame with a snapshot of the
/// dispatcher's recorded-call count. On `call_end`, the popped value
/// pairs with the current count to compute `(start, span)` for any
/// reverted frame, and the bracket is forwarded to
/// [`CompositionBuilder::annotate_revert_span`](eez_protocol::CompositionBuilder::annotate_revert_span).
/// Calls are recorded in preorder because each slot is opened before its
/// session executes. Therefore, `span = end - start` covers the reverted
/// frame's recorded calls. Batch materialization lowers such spans to the
/// protocol's `revertNextNCalls` rollback marker.
pub struct SessionInspector<'a> {
    /// Combined proxy-lookup configuration: the contract address to
    /// read and which slot to read from.
    proxy_lookup: ProxyLookupConfig,
    /// Composition builder that routes and records detected calls.
    dispatcher: &'a mut CompositionBuilder,
    /// Rollup ID of the chain this inspector is running on. Written to each
    /// [`eez_protocol::ExecutionRequest`] as the source identity used for
    /// target execution, recording, and call hashing.
    caller_rollup_id: RollupId,
    /// First target execution error, if any.
    error: Option<ExecutorError>,
    /// Per-rollup cache channel for nested re-entry.
    overlay_channel: OverlayChannelHandle,
    /// Per-EVM-frame snapshot of the dispatcher's recorded-call count and
    /// whether this frame itself was an intercepted cross-chain proxy call.
    ///
    /// A reverting ordinary frame brackets remote calls whose effects must be
    /// lowered to `revertNextNCalls`. A cross-chain proxy frame that naturally
    /// returns the destination's revert data does not: that failure is already
    /// represented by the entry/call outcome. Only an enclosing source frame
    /// that subsequently reverts creates a rollback window.
    frame_boundaries: Vec<FrameBoundary>,
    /// Prior committed effects remain visible even when this EVM has not loaded them.
    snapshot_seed: OverlaySnapshot,
    /// Force the first synthetic target frame into EVM static context.
    force_first_frame_static: bool,
    first_frame_initialized: bool,
}

#[derive(Clone, Copy, Debug)]
struct FrameBoundary {
    start: usize,
    intercepted_cross_chain: bool,
}

fn reverted_dispatch_span(
    boundary: FrameBoundary,
    end: usize,
    result: InstrStop,
    has_error: bool,
) -> Option<(usize, u32)> {
    if has_error
        || boundary.intercepted_cross_chain
        || boundary.start >= end
        || result != InstrStop::Revert
    {
        return None;
    }

    let span = u32::try_from(end - boundary.start).unwrap_or(u32::MAX);
    Some((boundary.start, span))
}

impl std::fmt::Debug for SessionInspector<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionInspector")
            .field("proxy_lookup", &self.proxy_lookup)
            .field("caller_rollup_id", &self.caller_rollup_id)
            .field("has_error", &self.error.is_some())
            .finish_non_exhaustive()
    }
}

/// Shared construction surface for [`SessionInspector`] instances.
///
/// Holds the per-chain proxy lookup, caller rollup id, and overlay channel
/// shared by source and target-session inspectors.
///
/// Each call site produces an inspector via [`build`](Self::build).
#[derive(Debug, Clone)]
pub struct SessionInspectorFactory {
    /// Contract and storage slot for this chain's `authorizedProxies` map.
    proxy_lookup: ProxyLookupConfig,
    /// Source rollup ID written to every request dispatched by inspectors
    /// built from this factory.
    caller_rollup_id: RollupId,
    /// Per-rollup channel for propagating state through recursive re-entry.
    /// Source and target-session inspectors use the channel belonging to the
    /// rollup whose EVM state they inspect.
    overlay_channel: OverlayChannelHandle,
}

impl SessionInspectorFactory {
    /// Create a factory pinned to one chain's configuration.
    #[must_use]
    pub fn new(
        proxy_lookup: ProxyLookupConfig,
        caller_rollup_id: RollupId,
        overlay_channel: OverlayChannelHandle,
    ) -> Self {
        Self {
            proxy_lookup,
            caller_rollup_id,
            overlay_channel,
        }
    }

    /// Build an inspector for source simulation or a target session.
    ///
    /// Calls are opened before target execution, so nested dispatches are
    /// recorded as preorder children of their outer call.
    pub fn build<'a>(
        &self,
        dispatcher: &'a mut CompositionBuilder,
        snapshot: &OverlaySnapshot,
    ) -> SessionInspector<'a> {
        SessionInspector {
            proxy_lookup: self.proxy_lookup.clone(),
            dispatcher,
            caller_rollup_id: self.caller_rollup_id,
            error: None,
            overlay_channel: self.overlay_channel.clone(),
            frame_boundaries: Vec::new(),
            snapshot_seed: snapshot.clone(),
            force_first_frame_static: false,
            first_frame_initialized: false,
        }
    }

    /// Build an inspector whose first synthetic target frame executes with
    /// the EVM static flag set. Descendant calls inherit that flag normally.
    pub fn build_static<'a>(
        &self,
        dispatcher: &'a mut CompositionBuilder,
        snapshot: &OverlaySnapshot,
    ) -> SessionInspector<'a> {
        let mut inspector = self.build(dispatcher, snapshot);
        inspector.force_first_frame_static = true;
        inspector
    }
}

impl SessionInspector<'_> {
    /// Take the recorded error, if any.
    pub fn take_error(&mut self) -> Option<ExecutorError> {
        self.error.take()
    }

    fn has_error(&self) -> bool {
        self.error.is_some()
    }

    fn record_error(&mut self, error: ExecutorError) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }
}

impl<T: EvmTypes> Inspector<T> for SessionInspector<'_> {
    fn initialize_interp(&mut self, interp: &mut Interpreter<'_, '_, T>) {
        if !self.first_frame_initialized {
            if self.force_first_frame_static {
                interp.set_static(true);
            }
            self.first_frame_initialized = true;
        }
    }

    fn call(
        &mut self,
        interp: &mut Interpreter<'_, '_, T>,
        inputs: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        // Bracket every CALL frame with a recorded-count snapshot so
        // `call_end` can detect "frame reverted AFTER dispatching one
        // or more cross-chain calls" (revert-continue patterns —
        // see field doc).
        self.frame_boundaries.push(FrameBoundary {
            start: self.dispatcher.recorded_count(),
            intercepted_cross_chain: false,
        });

        if self.has_error() {
            return None;
        }

        // Only CALL / STATICCALL can represent cross-chain proxy calls. Leave
        // delegate/callcode shapes to the EVM.
        if !matches!(inputs.kind, MessageKind::Call | MessageKind::StaticCall) {
            return None;
        }

        let calldata = inputs.input.to_bytes(interp.memory());
        let context = interp.host();
        let calldata_len = calldata.len();

        tracing::trace!(
            depth = inputs.depth,
            target_addr = %inputs.destination,
            caller = %inputs.caller,
            calldata_len,
            "inspector: CALL"
        );

        // Look up `authorizedProxies[target_address]` on the configured
        // contract via the live EVM state. Sees in-tx / in-block writes.
        let info =
            match lookup_authorized_proxy_live(context, &self.proxy_lookup, inputs.destination) {
                Ok(Some(info)) => info,
                Ok(None) => {
                    tracing::trace!(addr = %inputs.destination, "inspector: not a proxy");
                    return None;
                }
                Err(error) => {
                    // Keep provider failures typed: the queue must abort/recover the slot,
                    // not classify a missing observation as permanent transaction poison.
                    self.record_error(match error {
                        HostError::Execution(ExecutionError::Database(error)) => {
                            ExecutorError::provider(error)
                        }
                        // These reads neither skip cold loads nor load code; current
                        // Host implementations can only return database errors here.
                        error => ExecutorError::evm(error),
                    });
                    return None;
                }
            };
        self.frame_boundaries
            .last_mut()
            .expect("the current CALL frame has a boundary")
            .intercepted_cross_chain = true;

        let call_mode = if inputs.is_static() {
            CallMode::Static
        } else {
            CallMode::Mutable
        };
        let call_value = inputs.value;
        if call_mode == CallMode::Static && !call_value.is_zero() {
            self.record_error(ExecutorError::evm(
                "static cross-chain proxy call carries non-zero value",
            ));
            return None;
        }

        let req = eez_protocol::ExecutionRequest {
            call_mode,
            target_address: info.original_address,
            data: calldata,
            value: call_value,
            source_address: inputs.caller,
            source_rollup_id: self.caller_rollup_id,
        };
        // Dispatch is synchronous, so this inspector can exchange cache
        // snapshots around the nested execution.
        // Open a frame with this rollup's in-flight cache. If the downstream
        // call re-enters this rollup, the new session preloads the snapshot
        // and publishes its updated cache into this frame.
        let snapshot = match self.snapshot_seed.capture(context.state_mut()) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.record_error(ExecutorError::evm(format!(
                    "live overlay snapshot failed: {error:?}"
                )));
                return Some(synthetic_result::<T>(
                    inputs,
                    InstrStop::Revert,
                    Bytes::new(),
                ));
            }
        };
        self.overlay_channel.open_frame(snapshot);
        let sim = self.dispatcher.dispatch_call(info.original_rollup_id, req);
        // Apply only a result published during this dispatch, before this EVM
        // frame continues. A failed target call reverts every callback it
        // made, so its result is discarded. The frame's snapshot is the exact
        // cache recorded above (same thread; re-entered sessions only peek),
        // so it serves as the diff baseline without a second clone.
        let frame = self.overlay_channel.close_frame();
        let succeeded = sim
            .as_ref()
            .is_ok_and(eez_protocol::ExecutionOutcome::is_success);
        let features = context.execution_config().version().features;
        if let Some(crate::overlay::DispatchFrame {
            pre_dispatch,
            result: Some(result),
        }) = frame
            && succeeded
            && let Err(e) = crate::overlay::apply_overlay_diff(
                context.state_mut(),
                &pre_dispatch,
                &result,
                features,
            )
        {
            self.record_error(ExecutorError::evm(format!(
                "overlay diff-apply failed: {e}"
            )));
        }
        let sim = match sim {
            Ok(response) => response,
            Err(e) => {
                // Expected source waits are reported once per transition by
                // the queue owner, with the transaction and pinned state.
                if !matches!(e.kind(), eez_protocol::ExecutorErrorKind::SourceBlocked(_)) {
                    tracing::error!(error = %e, "target execution failed");
                }
                self.record_error(e);
                return Some(synthetic_result::<T>(
                    inputs,
                    InstrStop::Revert,
                    Bytes::new(),
                ));
            }
        };

        let success = sim.is_success();
        let return_data: Bytes = sim
            .return_data()
            .map(<[u8]>::to_vec)
            .unwrap_or_default()
            .into();
        // A failed target's revert data is the only clue to why a source
        // later fails; keep a bounded prefix so the line stays log-sized.
        let logged = &return_data[..return_data.len().min(LOGGED_RETURN_BYTES)];
        tracing::info!(
            dest = %info.original_address,
            rollup_id = %info.original_rollup_id,
            caller = %inputs.caller,
            proxy = %inputs.destination,
            depth = inputs.depth,
            calldata_len,
            value = %call_value,
            target_result = if success { "ok" } else { "REVERT" },
            target_gas = sim.gas_used().unwrap_or(0),
            return_len = return_data.len(),
            return_data = %alloy_primitives::hex::encode_prefixed(logged),
            "cross-chain proxy call detected"
        );

        // Surface a failed cross-chain sim as `Revert` to the surrounding
        // EVM — the outer Solidity frame should see the call as reverted
        // so try/catch + reverted-frame accounting bracket the right range.
        let result = if success {
            InstrStop::Return
        } else {
            InstrStop::Revert
        };

        Some(synthetic_result::<T>(inputs, result, return_data))
    }

    fn call_end(
        &mut self,
        _interp: &mut Interpreter<'_, '_, T>,
        _inputs: &Message<T>,
        outcome: &mut MessageResult<T>,
    ) {
        // Pair with the count captured in `call`. A reverted frame with a
        // non-empty dispatch range records the span and queues the affected
        // session checkpoints for rollback before the next dispatch.
        //
        // Skip when an executor error already aborts composition, no calls
        // were dispatched in the frame, or the outcome is not an explicit
        // revert.
        let boundary = self.frame_boundaries.pop().unwrap_or(FrameBoundary {
            start: 0,
            intercepted_cross_chain: false,
        });
        let end = self.dispatcher.recorded_count();
        if let Some((start, span)) =
            reverted_dispatch_span(boundary, end, outcome.stop, self.has_error())
        {
            self.dispatcher.annotate_revert_span(start, span);
        }
    }
}

fn synthetic_result<T: EvmTypes>(
    message: &Message<T>,
    stop: InstrStop,
    output: Bytes,
) -> MessageResult<T> {
    MessageResult::<T> {
        stop,
        output,
        gas: GasTracker::new_with_execution_gas_and_reservoir(message.gas_limit, message.reservoir),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{U256, address};
    use eez_protocol::{EEZ_AUTHORIZED_PROXIES_SLOT, EEZL2_AUTHORIZED_PROXIES_SLOT};
    use evm2::{
        BaseEvmTypes, EvmFeatures, SpecId,
        bytecode::Bytecode,
        ethereum::ethereum_tx_registry,
        evm::{AccountInfo, InMemoryDB, State},
        precompiles::Precompiles,
    };

    fn fresh_context(db: InMemoryDB) -> Evm<'static, BaseEvmTypes> {
        Evm::new(
            SpecId::OSAKA,
            Default::default(),
            ethereum_tx_registry(SpecId::OSAKA),
            db,
            Precompiles::base(SpecId::OSAKA),
        )
    }
    #[test]
    fn live_snapshot_includes_created_code_and_constructor_storage_without_committing() {
        let child = Address::with_last_byte(0x42);
        let code = Bytecode::new_raw(alloy_primitives::bytes!("602a60005260206000f3"));
        let mut state = State::new(InMemoryDB::default());
        let outer = state.checkpoint();
        state
            .create_account(&child, &child, &U256::ZERO, EvmFeatures::EIP161)
            .unwrap()
            .unwrap();
        {
            let mut account = state.account(&child).unwrap();
            account.set_code_slow(code.clone());
            account.set_balance(U256::from(17));
            account.set_nonce(2);
        }
        state
            .storage_slot(&child, U256::ZERO)
            .unwrap()
            .set(U256::from(41));
        let original_cache = state.overlay_db().cache.clone();
        assert_eq!(original_cache.accounts[&child], None);
        let seed = OverlaySnapshot::default();
        let snapshot = seed.capture(&mut state).unwrap();
        let account = snapshot.cache.accounts[&child].as_ref().unwrap();
        assert_eq!(account.code_hash, code.hash_slow());
        assert_eq!(
            account
                .code
                .as_ref()
                .or_else(|| snapshot.cache.contracts.get(&account.code_hash)),
            Some(&code)
        );
        assert_eq!(account.nonce, 2);
        assert_eq!(account.balance, U256::from(17));
        assert_eq!(
            snapshot.cache.storage[&child].slots[&U256::ZERO],
            U256::from(41)
        );
        assert_eq!(snapshot.cache.contracts[&code.hash_slow()], code);
        assert_eq!(state.overlay_db().cache, original_cache);
        assert!(state.account(&child).unwrap().is_created());
        state
            .storage_slot(&child, U256::ZERO)
            .unwrap()
            .set(U256::from(42));
        assert_eq!(
            seed.capture(&mut state).unwrap().cache.storage[&child].slots[&U256::ZERO],
            U256::from(42)
        );
        state.rollback(outer, EvmFeatures::EIP161);
        let snapshot = seed.capture(&mut state).unwrap();
        assert_eq!(snapshot.cache.accounts[&child], None);
        assert!(!snapshot.cache.contracts.contains_key(&code.hash_slow()));
    }
    const EEZ_ADDRESS: Address = address!("0x1111111111111111111111111111111111111111");
    const EEZL2_ADDRESS: Address = address!("0x4444444444444444444444444444444444444444");
    const PROXY_ADDR: Address = address!("0x2222222222222222222222222222222222222222");
    const DESTINATION_ADDR: Address = address!("0x3333333333333333333333333333333333333333");
    const TARGET_ROLLUP: u64 = 42;

    fn lookup(contract_address: Address, authorized_proxies_slot: u8) -> ProxyLookupConfig {
        ProxyLookupConfig {
            contract_address,
            authorized_proxies_slot,
        }
    }

    fn packed_proxy_value(destination: Address, rollup_id: u64) -> U256 {
        let mut word = [0u8; 32];
        word[3..11].copy_from_slice(&rollup_id.to_be_bytes());
        word[11..31].copy_from_slice(destination.as_ref());
        word[31] = 1;
        U256::from_be_bytes(word)
    }

    #[test]
    fn proxy_code_context_calls_are_left_to_the_evm_without_dispatch() {
        for kind in [MessageKind::DelegateCall, MessageKind::CallCode] {
            let mut db = InMemoryDB::default();
            db.insert_account_info(&EEZ_ADDRESS, AccountInfo::empty());
            db.insert_account_info(&EEZL2_ADDRESS, AccountInfo::empty());
            for address in [PROXY_ADDR, DESTINATION_ADDR] {
                db.insert_account_storage(
                    &EEZ_ADDRESS,
                    &proxy_mapping_key(address, EEZ_AUTHORIZED_PROXIES_SLOT).into(),
                    &packed_proxy_value(DESTINATION_ADDR, TARGET_ROLLUP),
                );
            }
            let mut context = fresh_context(db);
            let mut dispatcher = CompositionBuilder::new(RollupId::MAINNET, Default::default());
            let factory = SessionInspectorFactory::new(
                lookup(EEZ_ADDRESS, EEZ_AUTHORIZED_PROXIES_SLOT),
                RollupId::MAINNET,
                crate::overlay::new_overlay_channel(),
            );
            let mut inspector = factory.build(&mut dispatcher, &OverlaySnapshot::default());
            let mut inputs = Message::<BaseEvmTypes> {
                kind,
                gas_limit: 100_000,
                code_address: PROXY_ADDR,
                call_target: PROXY_ADDR,
                destination: DESTINATION_ADDR,
                caller: Address::with_last_byte(7),
                ..Default::default()
            };
            let frame = inputs.clone();
            let mut interp = Interpreter::<BaseEvmTypes>::new(&frame);
            interp.prepare_run(&mut context);
            assert!(inspector.call(&mut interp, &mut inputs).is_none());
            assert!(inspector.take_error().is_none());
            assert_eq!(inspector.dispatcher.recorded_count(), 0);
            assert_eq!(inspector.frame_boundaries.len(), 1);
            assert!(!inspector.frame_boundaries[0].intercepted_cross_chain);
        }
    }
    #[test]
    fn registered_calls_reach_dispatch_and_keep_failure_and_frame_ownership() {
        for kind in [MessageKind::Call, MessageKind::StaticCall] {
            let mut db = InMemoryDB::default();
            db.insert_account_info(&EEZ_ADDRESS, AccountInfo::empty().with_nonce(1));
            db.insert_account_storage(
                &EEZ_ADDRESS,
                &proxy_mapping_key(PROXY_ADDR, EEZ_AUTHORIZED_PROXIES_SLOT).into(),
                &packed_proxy_value(DESTINATION_ADDR, TARGET_ROLLUP),
            );
            let mut context = fresh_context(db);
            let channel = crate::overlay::new_overlay_channel();
            let factory = SessionInspectorFactory::new(
                lookup(EEZ_ADDRESS, EEZ_AUTHORIZED_PROXIES_SLOT),
                RollupId::MAINNET,
                channel.clone(),
            );
            let mut dispatcher = CompositionBuilder::new(RollupId::MAINNET, Default::default());
            let mut inspector = factory.build(&mut dispatcher, &OverlaySnapshot::default());
            let mut message = Message::<BaseEvmTypes> {
                kind,
                destination: PROXY_ADDR,
                call_target: PROXY_ADDR,
                gas_limit: 100_000,
                ..Default::default()
            };
            let frame = message.clone();
            let mut interp = Interpreter::<BaseEvmTypes>::new(&frame);
            interp.prepare_run(&mut context);
            let mut outcome = inspector
                .call(&mut interp, &mut message)
                .expect("registered proxy must be intercepted");
            assert_eq!(outcome.stop, InstrStop::Revert);
            assert_eq!(outcome.gas.remaining(), 100_000);
            let error = inspector
                .take_error()
                .expect("missing target is an execution error");
            assert!(
                matches!(
                    error.kind(),
                    eez_protocol::ExecutorErrorKind::Unavailable(_)
                ),
                "{error}"
            );
            assert!(
                inspector
                    .frame_boundaries
                    .last()
                    .unwrap()
                    .intercepted_cross_chain
            );
            assert!(
                channel.peek_pre_snapshot().is_none(),
                "failed dispatch must close its own frame"
            );
            inspector.call_end(&mut interp, &message, &mut outcome);
            assert!(inspector.frame_boundaries.is_empty());
        }
    }

    /// A fixture target publishes a callback cache; the real source EVM then executes
    /// the installed child's SLOAD and must undo that installation on an outer revert.
    #[test]
    fn resumed_evm_reads_callback_child_storage_and_owns_outer_rollback() {
        use eez_protocol::{
            ChainClient, ChainDialect, ExecutionOutcome, ExecutionRequest, ExecutorResult, Rollup,
            SessionSnapshot, TargetConfig, TargetExecutionSession,
        };
        use std::sync::Arc;
        const CHILD: Address = Address::repeat_byte(0x66);
        const SOURCE: Address = Address::repeat_byte(0x55);
        struct Callback {
            channel: OverlayChannelHandle,
            succeeds: bool,
        }
        impl TargetExecutionSession for Callback {
            fn execute(
                &mut self,
                req: ExecutionRequest,
                _: &mut CompositionBuilder,
            ) -> ExecutorResult<ExecutionOutcome> {
                assert_eq!(req.source_address, SOURCE);
                assert_eq!(req.source_rollup_id, RollupId::MAINNET);
                assert_eq!(req.target_address, DESTINATION_ADDR);
                let mut snapshot = self.channel.peek_pre_snapshot().expect("live source frame");
                let code = Bytecode::new_raw(alloy_primitives::bytes!("5f545f5260205ff3"));
                let info = AccountInfo::empty().with_nonce(1).with_code(code.clone());
                snapshot.cache.contracts.insert(info.code_hash, code);
                snapshot.cache.accounts.insert(CHILD, Some(info));
                snapshot.statuses.insert(
                    CHILD,
                    evm2::block::bundle_state::AccountStatus::InMemoryChange,
                );
                snapshot
                    .cache
                    .storage
                    .entry(CHILD)
                    .or_default()
                    .slots
                    .insert(U256::ZERO, U256::from(42));
                assert!(self.channel.publish(snapshot));
                Ok(ExecutionOutcome::Resolved {
                    return_data: Vec::new(),
                    gas_used: 0,
                    success: self.succeeds,
                })
            }
            fn checkpoint(&mut self) -> ExecutorResult<SessionSnapshot> {
                Ok(Box::new(()))
            }
            fn rollback(&mut self, _: &SessionSnapshot) -> ExecutorResult<()> {
                Ok(())
            }
        }
        struct UnusedClient;
        impl ChainClient for UnusedClient {
            fn begin_execution_session(
                &self,
            ) -> ExecutorResult<Box<dyn TargetExecutionSession + Send>> {
                panic!("fixture session is preseeded")
            }
        }
        for (callback_success, outer_revert) in [(true, false), (true, true), (false, false)] {
            let channel = crate::overlay::new_overlay_channel();
            let lookup = lookup(EEZ_ADDRESS, EEZ_AUTHORIZED_PROXIES_SLOT);
            let target = Rollup {
                client: Arc::new(UnusedClient),
                session: Some(Box::new(Callback {
                    channel: channel.clone(),
                    succeeds: callback_success,
                })),
                config: TargetConfig {
                    proxy_lookup: lookup.clone(),
                    dialect: ChainDialect::EvmL2Style,
                },
            };
            let mut dispatcher = CompositionBuilder::new(
                RollupId::MAINNET,
                [(RollupId(TARGET_ROLLUP), target)].into(),
            );
            let factory = SessionInspectorFactory::new(lookup, RollupId::MAINNET, channel.clone());
            let mut inspector = factory.build(&mut dispatcher, &OverlaySnapshot::default());
            let mut db = InMemoryDB::default();
            db.insert_account_info(&EEZ_ADDRESS, AccountInfo::empty().with_nonce(1));
            db.insert_account_storage(
                &EEZ_ADDRESS,
                &proxy_mapping_key(PROXY_ADDR, EEZ_AUTHORIZED_PROXIES_SLOT).into(),
                &packed_proxy_value(DESTINATION_ADDR, TARGET_ROLLUP),
            );
            // CALL the proxy, then CALL the newly installed child into memory[0..32].
            let mut code = alloy_primitives::hex::decode("5f5f5f5f5f73").unwrap();
            code.extend_from_slice(PROXY_ADDR.as_slice());
            code.extend_from_slice(&alloy_primitives::hex::decode("5af15060205f5f5f5f73").unwrap());
            code.extend_from_slice(CHILD.as_slice());
            code.extend_from_slice(&alloy_primitives::hex::decode("5af15060205f").unwrap());
            code.push(if outer_revert { 0xfd } else { 0xf3 });
            db.insert_account_info(
                &SOURCE,
                AccountInfo::empty()
                    .with_nonce(1)
                    .with_code(Bytecode::new_raw(code.into())),
            );
            let user = Address::repeat_byte(0x77);
            db.insert_account_info(
                &user,
                AccountInfo::empty().with_balance(U256::from(1_000_000)),
            );
            let mut evm = fresh_context(db);
            let tx = evm2::ethereum::RecoveredTxEnvelope::new_unchecked(
                evm2::ethereum::TxEnvelope::Legacy(alloy_consensus::TxLegacy {
                    to: alloy_primitives::TxKind::Call(SOURCE),
                    gas_limit: 500_000,
                    ..Default::default()
                }),
                user,
            );
            let result = evm
                .transact_with_inspector(&tx, &mut inspector)
                .unwrap()
                .detach();
            assert_eq!(result.result.is_success(), !outer_revert);
            // Independent Reth2.7/REVM43 run of these exact source bytes; a cold
            // callback account/slot adds 4,500 gas while still returning the right data.
            assert_eq!(
                result.result.tx_gas_used(),
                if callback_success { 23_858 } else { 26_243 }
            );
            assert_eq!(
                U256::from_be_slice(&result.result.output),
                U256::from(if callback_success { 42 } else { 0 })
            );
            if callback_success && !outer_revert {
                let child = result.pending_state.account_changes_for(&CHILD).unwrap();
                assert!(child.created);
                assert_eq!(child.current.unwrap().nonce, 1);
            } else {
                assert!(result.pending_state.account_info(&CHILD).is_none());
            }
            println!(
                "callback-runtime callback_success={callback_success} outer_revert={outer_revert} gas={} output={}",
                result.result.tx_gas_used(),
                alloy_primitives::hex::encode(&result.result.output)
            );
            assert!(inspector.take_error().is_none());
            assert!(inspector.frame_boundaries.is_empty());
            assert_eq!(inspector.dispatcher.recorded_count(), 1);
            assert!(channel.peek_pre_snapshot().is_none());
        }
    }

    #[test]
    fn proxy_lookup_propagates_account_and_slot_database_errors() {
        struct FaultDb {
            account_fault: bool,
        }
        impl evm2::evm::Database for FaultDb {
            type Error = std::io::Error;
            fn get_account(&mut self, _: &Address) -> Result<Option<AccountInfo>, Self::Error> {
                if self.account_fault {
                    Err(std::io::Error::other("lookup account"))
                } else {
                    Ok(Some(AccountInfo::empty()))
                }
            }
            fn get_storage(&mut self, _: &Address, _: &U256) -> Result<U256, Self::Error> {
                Err(std::io::Error::other("lookup slot"))
            }
            fn get_code_by_hash(
                &mut self,
                _: &alloy_primitives::B256,
            ) -> Result<Bytecode, Self::Error> {
                panic!("lookup must not read code")
            }
            fn get_block_hash(&mut self, _: &U256) -> Result<alloy_primitives::B256, Self::Error> {
                panic!("lookup must not read history")
            }
        }
        for account_fault in [true, false] {
            let mut ctx = Evm::<BaseEvmTypes>::new(
                SpecId::OSAKA,
                Default::default(),
                ethereum_tx_registry(SpecId::OSAKA),
                evm2::evm::Db::new(FaultDb { account_fault }),
                Precompiles::base(SpecId::OSAKA),
            );
            let error = lookup_authorized_proxy_live(
                &mut ctx,
                &lookup(EEZ_ADDRESS, EEZ_AUTHORIZED_PROXIES_SLOT),
                PROXY_ADDR,
            )
            .unwrap_err();
            let HostError::Execution(ExecutionError::Database(error)) = error else {
                panic!("unexpected lookup error: {error:?}")
            };
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().to_string(),
                if account_fault {
                    "lookup account"
                } else {
                    "lookup slot"
                }
            );
        }
    }

    #[test]
    fn unregistered_address_returns_none() {
        let mut ctx = fresh_context(InMemoryDB::default());
        assert!(
            lookup_authorized_proxy_live(
                &mut ctx,
                &lookup(EEZ_ADDRESS, EEZ_AUTHORIZED_PROXIES_SLOT),
                PROXY_ADDR
            )
            .unwrap()
            .is_none()
        );
    }
    #[test]
    fn registered_in_db_returns_some_cold_read() {
        let key = proxy_mapping_key(PROXY_ADDR, EEZ_AUTHORIZED_PROXIES_SLOT);
        let mut db = InMemoryDB::default();
        db.insert_account_info(&EEZ_ADDRESS, AccountInfo::empty());
        db.insert_account_info(&EEZL2_ADDRESS, AccountInfo::empty());
        db.insert_account_storage(
            &EEZ_ADDRESS,
            &key.into(),
            &packed_proxy_value(DESTINATION_ADDR, TARGET_ROLLUP),
        );
        let mut ctx = fresh_context(db);
        let info = lookup_authorized_proxy_live(
            &mut ctx,
            &lookup(EEZ_ADDRESS, EEZ_AUTHORIZED_PROXIES_SLOT),
            PROXY_ADDR,
        )
        .unwrap()
        .unwrap();
        assert_eq!(info.original_address, DESTINATION_ADDR);
        assert_eq!(info.original_rollup_id, RollupId(TARGET_ROLLUP));
        assert!(
            !ctx.load_account(&EEZ_ADDRESS, false, false)
                .unwrap()
                .is_cold
        );
        assert!(!ctx.sload(&EEZ_ADDRESS, &key.into(), false).unwrap().is_cold);
    }
    #[test]
    fn registered_in_journal_returns_some_hot_read() {
        let mut ctx = fresh_context(InMemoryDB::default());
        let key = proxy_mapping_key(PROXY_ADDR, EEZ_AUTHORIZED_PROXIES_SLOT);
        ctx.state_mut()
            .storage_slot(&EEZ_ADDRESS, key.into())
            .unwrap()
            .set(packed_proxy_value(DESTINATION_ADDR, TARGET_ROLLUP));
        let info = lookup_authorized_proxy_live(
            &mut ctx,
            &lookup(EEZ_ADDRESS, EEZ_AUTHORIZED_PROXIES_SLOT),
            PROXY_ADDR,
        )
        .unwrap()
        .unwrap();
        assert_eq!(info.original_address, DESTINATION_ADDR);
        assert_eq!(info.original_rollup_id, RollupId(TARGET_ROLLUP));
    }
    #[test]
    fn wrong_source_contract_slot_returns_none() {
        let correct_slot = EEZ_AUTHORIZED_PROXIES_SLOT;
        let wrong_slot = 7;
        assert_ne!(correct_slot, wrong_slot);
        let mut db = InMemoryDB::default();
        db.insert_account_info(&EEZ_ADDRESS, AccountInfo::empty());
        db.insert_account_info(&EEZL2_ADDRESS, AccountInfo::empty());
        db.insert_account_storage(
            &EEZ_ADDRESS,
            &proxy_mapping_key(PROXY_ADDR, correct_slot).into(),
            &packed_proxy_value(DESTINATION_ADDR, TARGET_ROLLUP),
        );
        let mut ctx = fresh_context(db);
        assert!(
            lookup_authorized_proxy_live(&mut ctx, &lookup(EEZ_ADDRESS, wrong_slot), PROXY_ADDR)
                .unwrap()
                .is_none()
        );
    }
    fn assert_slot_path(config: ProxyLookupConfig) {
        assert_eq!(config.authorized_proxies_slot, 0);
        let mut db = InMemoryDB::default();
        db.insert_account_info(&EEZ_ADDRESS, AccountInfo::empty());
        db.insert_account_info(&EEZL2_ADDRESS, AccountInfo::empty());
        db.insert_account_storage(
            &config.contract_address,
            &proxy_mapping_key(PROXY_ADDR, config.authorized_proxies_slot).into(),
            &packed_proxy_value(DESTINATION_ADDR, TARGET_ROLLUP),
        );
        let mut ctx = fresh_context(db);
        let info = lookup_authorized_proxy_live(&mut ctx, &config, PROXY_ADDR)
            .unwrap()
            .unwrap();
        assert_eq!(info.original_rollup_id, RollupId(TARGET_ROLLUP));
        assert_eq!(info.original_address, DESTINATION_ADDR);
    }
    #[test]
    fn eez_slot_path_reads_slot_0() {
        assert_slot_path(lookup(EEZ_ADDRESS, EEZ_AUTHORIZED_PROXIES_SLOT));
    }
    #[test]
    fn eezl2_slot_path_reads_slot_0() {
        assert_slot_path(lookup(EEZL2_ADDRESS, EEZL2_AUTHORIZED_PROXIES_SLOT));
    }
    #[test]
    fn natural_cross_chain_revert_does_not_create_rollback_span() {
        let span = reverted_dispatch_span(
            FrameBoundary {
                start: 3,
                intercepted_cross_chain: true,
            },
            4,
            InstrStop::Revert,
            false,
        );

        assert_eq!(span, None);
    }

    #[test]
    fn enclosing_source_revert_creates_rollback_span() {
        let span = reverted_dispatch_span(
            FrameBoundary {
                start: 3,
                intercepted_cross_chain: false,
            },
            5,
            InstrStop::Revert,
            false,
        );

        assert_eq!(span, Some((3, 2)));
    }

    #[test]
    fn successful_source_frame_does_not_create_rollback_span() {
        let span = reverted_dispatch_span(
            FrameBoundary {
                start: 3,
                intercepted_cross_chain: false,
            },
            5,
            InstrStop::Return,
            false,
        );

        assert_eq!(span, None);
    }
}
