//! [`SessionInspector`] detects cross-chain proxy calls during EVM execution
//! and dispatches each one through a borrowed [`CompositionBuilder`].
//!
//! When the inspector sees a CALL opcode:
//! 1. `lookup_authorized_proxy_live` reads `authorizedProxies[addr]`
//!    from the **live EVM state** (revm journal + DB) — so proxies
//!    registered by earlier calls in the same transaction, or by
//!    earlier transactions in the same in-progress block, are visible.
//! 2. If the address is a registered proxy,
//!    [`CompositionBuilder::dispatch_call`] forwards the call to the target
//!    session and returns the execution response.
//! 3. The inspector synthesizes the outcome back into the source EVM so
//!    the source transaction's control flow can continue.
//!
//! Errors surfaced by dispatch are stashed locally so the caller can
//! consult them after the EVM pass without unwinding the revm machine.
//!
//! # Why live EVM state
//!
//! The lookup must see writes made earlier in the same transaction
//! (e.g. `Bridge.bridgeEther` deploys a CREATE2 proxy and calls it
//! inside one tx) and earlier in the same in-progress block
//! (multi-tx composition). Reading through revm's journal captures
//! both; a pre-tx storage snapshot would not.
//!
use alloy_primitives::{Address, Bytes};
use revm::Inspector;
use revm::context_interface::{ContextTr, Host, JournalTr};
use revm::database::{CacheState, State};
use revm::interpreter::{
    CallInputs, CallOutcome, CallScheme, Gas, InstructionResult, Interpreter, InterpreterResult,
};

use crate::overlay::OverlayChannelHandle;
use eez_protocol::{CallMode, CompositionBuilder, ExecutorError, ProxyLookupConfig, RollupId};
use eez_protocol::{ProxyInfo, decode_proxy_value, proxy_mapping_key};

/// Bytes of a cross-chain call's return or revert data kept in its log line.
const LOGGED_RETURN_BYTES: usize = 256;

/// Look up `authorizedProxies[addr]` from the live EVM state at the
/// hook site.
///
/// Goes through revm's journal (so writes by earlier calls or earlier
/// txs in the same in-progress block are visible) and falls back to the
/// underlying DB for cold slots. Returns `None` for unregistered proxies
/// and for DB errors — the caller treats both as "not a proxy."
///
/// # Account warming
///
/// revm's `sload` requires the target account to already be present in
/// the journal; it's the normal precondition when the EVM does an SLOAD
/// while executing a contract. The inspector fires on every CALL —
/// including calls to destinations that have never had their state
/// touched before — so we can't assume the rollups contract is warm.
/// We explicitly load it; a no-op if it was already warm.
fn lookup_authorized_proxy_live<CTX: ContextTr + Host>(
    ctx: &mut CTX,
    lookup: &ProxyLookupConfig,
    addr: Address,
) -> Option<ProxyInfo> {
    // Warm the rollups account before the SLOAD (see fn-level doc).
    ctx.journal_mut()
        .load_account(lookup.contract_address)
        .ok()?;
    let key = proxy_mapping_key(addr, lookup.authorized_proxies_slot);
    let load = ctx.sload(lookup.contract_address, key.into())?;
    decode_proxy_value(load.data)
}

/// Clone this rollup's in-flight cache with journal-live values.
///
/// revm keeps mid-transaction writes in its journal, while loaded cache
/// entries can still contain their pre-write values. Refresh every loaded
/// account and slot through `Host` before sharing the cache. An `SSTORE`
/// first loads its slot, so the cache already identifies every storage key
/// that needs refreshing.
fn live_cache_snapshot<'db, DB, CTX>(context: &mut CTX) -> CacheState
where
    CTX: ContextTr<Db = &'db mut State<DB>> + Host,
    DB: 'db,
{
    let mut cache = context.db_mut().cache.clone();
    // Collect keys first: the refresh loops below mutate `cache` while
    // reading live values through `context`.
    let mut addrs: Vec<Address> = Vec::new();
    let mut slots: Vec<(Address, alloy_primitives::U256)> = Vec::new();
    for (addr, acc) in &cache.accounts {
        addrs.push(*addr);
        if let Some(plain) = acc.account.as_ref() {
            for slot in plain.storage.keys() {
                slots.push((*addr, *slot));
            }
        }
    }
    // Read live storage values through the journal-aware host.
    for (addr, slot) in slots {
        if let Some(load) = context.sload(addr, slot)
            && let Some(plain) = cache
                .accounts
                .get_mut(&addr)
                .and_then(|a| a.account.as_mut())
        {
            plain.storage.insert(slot, load.data);
        }
    }
    // Read live balances and nonces through the host.
    for addr in addrs {
        if let Ok(info) = context.load_account_info_skip_cold_load(addr, false, true)
            && let Some(plain) = cache
                .accounts
                .get_mut(&addr)
                .and_then(|a| a.account.as_mut())
        {
            plain.info.balance = info.balance;
            plain.info.nonce = info.nonce;
        }
    }
    cache
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
    result: InstructionResult,
    has_error: bool,
) -> Option<(usize, u32)> {
    if has_error
        || boundary.intercepted_cross_chain
        || boundary.start >= end
        || result != InstructionResult::Revert
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
    pub fn build<'a>(&self, dispatcher: &'a mut CompositionBuilder) -> SessionInspector<'a> {
        SessionInspector {
            proxy_lookup: self.proxy_lookup.clone(),
            dispatcher,
            caller_rollup_id: self.caller_rollup_id,
            error: None,
            overlay_channel: self.overlay_channel.clone(),
            frame_boundaries: Vec::new(),
            force_first_frame_static: false,
            first_frame_initialized: false,
        }
    }

    /// Build an inspector whose first synthetic target frame executes with
    /// the EVM static flag set. Descendant calls inherit that flag normally.
    pub fn build_static<'a>(&self, dispatcher: &'a mut CompositionBuilder) -> SessionInspector<'a> {
        let mut inspector = self.build(dispatcher);
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

impl<'db, DB, CTX> Inspector<CTX> for SessionInspector<'_>
where
    CTX: ContextTr<Db = &'db mut State<DB>> + Host,
    DB: 'db,
{
    fn initialize_interp(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        if !self.first_frame_initialized {
            if self.force_first_frame_static {
                interp.runtime_flag.is_static = true;
            }
            self.first_frame_initialized = true;
        }
    }

    fn call(&mut self, context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
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
        if !matches!(inputs.scheme, CallScheme::Call | CallScheme::StaticCall) {
            return None;
        }

        let calldata = inputs.input.bytes(context);
        let calldata_len = calldata.len();

        tracing::trace!(
            depth = context.journal_ref().depth(),
            target_addr = %inputs.target_address,
            caller = %inputs.caller,
            calldata_len,
            "inspector: CALL"
        );

        // Look up `authorizedProxies[target_address]` on the configured
        // contract via the live EVM state. Sees in-tx / in-block writes.
        let Some(info) =
            lookup_authorized_proxy_live(context, &self.proxy_lookup, inputs.target_address)
        else {
            tracing::trace!(addr = %inputs.target_address, "inspector: not a proxy");
            return None;
        };
        self.frame_boundaries
            .last_mut()
            .expect("the current CALL frame has a boundary")
            .intercepted_cross_chain = true;

        let call_mode = if inputs.is_static || matches!(inputs.scheme, CallScheme::StaticCall) {
            CallMode::Static
        } else {
            CallMode::Mutable
        };
        let call_value = inputs.value.get();
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
        self.overlay_channel
            .open_frame(live_cache_snapshot(context));
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
        if let Some(crate::overlay::DispatchFrame {
            pre_dispatch,
            result: Some(result),
        }) = frame
            && succeeded
            && let Err(e) = crate::overlay::apply_overlay_diff(context, &pre_dispatch, &result)
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
                return Some(CallOutcome::new(
                    InterpreterResult::new(
                        InstructionResult::Revert,
                        Bytes::new(),
                        Gas::new(inputs.gas_limit),
                    ),
                    inputs.return_memory_offset.clone(),
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
            proxy = %inputs.target_address,
            depth = context.journal_ref().depth(),
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
            InstructionResult::Return
        } else {
            InstructionResult::Revert
        };

        Some(CallOutcome::new(
            InterpreterResult::new(result, return_data, Gas::new(inputs.gas_limit)),
            inputs.return_memory_offset.clone(),
        ))
    }

    fn call_end(&mut self, _context: &mut CTX, _inputs: &CallInputs, outcome: &mut CallOutcome) {
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
            reverted_dispatch_span(boundary, end, outcome.result.result, self.has_error())
        {
            self.dispatcher.annotate_revert_span(start, span);
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for [`lookup_authorized_proxy_live`] + the two per-variant
    //! slot paths exercised via [`ProxyLookupConfig`].
    //!
    //! These target the core reason this helper exists: reading from
    //! revm's live state (journal + DB) instead of a pre-tx `StateProvider`
    //! snapshot.

    use super::*;
    use alloy_primitives::{U256, address};
    use eez_protocol::{EEZ_AUTHORIZED_PROXIES_SLOT, EEZL2_AUTHORIZED_PROXIES_SLOT};
    use revm::MainContext;
    use revm::context::Context;
    use revm::database::{CacheDB, EmptyDB};

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

    fn fresh_context() -> Context<
        revm::context::BlockEnv,
        revm::context::TxEnv,
        revm::context::CfgEnv,
        CacheDB<EmptyDB>,
        revm::context::Journal<CacheDB<EmptyDB>>,
        (),
    > {
        Context::mainnet().with_db(CacheDB::<EmptyDB>::default())
    }

    #[test]
    fn unregistered_address_returns_none() {
        let mut ctx = fresh_context();
        ctx.journal_mut()
            .load_account(EEZ_ADDRESS)
            .expect("load EEZ account");

        let info = lookup_authorized_proxy_live(
            &mut ctx,
            &lookup(EEZ_ADDRESS, EEZ_AUTHORIZED_PROXIES_SLOT),
            PROXY_ADDR,
        );
        assert!(info.is_none());
    }

    #[test]
    fn registered_in_db_returns_some_cold_read() {
        let key = proxy_mapping_key(PROXY_ADDR, EEZ_AUTHORIZED_PROXIES_SLOT);
        let value = packed_proxy_value(DESTINATION_ADDR, TARGET_ROLLUP);

        let mut cache_db = CacheDB::<EmptyDB>::default();
        cache_db
            .insert_account_storage(EEZ_ADDRESS, key.into(), value)
            .expect("populate storage");

        let mut ctx = Context::mainnet().with_db(cache_db);
        ctx.journal_mut()
            .load_account(EEZ_ADDRESS)
            .expect("load EEZ account");

        let info = lookup_authorized_proxy_live(
            &mut ctx,
            &lookup(EEZ_ADDRESS, EEZ_AUTHORIZED_PROXIES_SLOT),
            PROXY_ADDR,
        )
        .expect("proxy present in DB");

        assert_eq!(info.original_address, DESTINATION_ADDR);
        assert_eq!(info.original_rollup_id, RollupId(TARGET_ROLLUP));
    }

    #[test]
    fn registered_in_journal_returns_some_hot_read() {
        let mut ctx = fresh_context();
        let key = proxy_mapping_key(PROXY_ADDR, EEZ_AUTHORIZED_PROXIES_SLOT);
        let value = packed_proxy_value(DESTINATION_ADDR, TARGET_ROLLUP);

        ctx.journal_mut()
            .load_account(EEZ_ADDRESS)
            .expect("load EEZ account");
        ctx.journal_mut()
            .sstore(EEZ_ADDRESS, key.into(), value)
            .expect("journal sstore");

        let info = lookup_authorized_proxy_live(
            &mut ctx,
            &lookup(EEZ_ADDRESS, EEZ_AUTHORIZED_PROXIES_SLOT),
            PROXY_ADDR,
        )
        .expect("journal must expose in-tx writes to the inspector");

        assert_eq!(info.original_address, DESTINATION_ADDR);
        assert_eq!(info.original_rollup_id, RollupId(TARGET_ROLLUP));
    }

    #[test]
    fn wrong_source_contract_slot_returns_none() {
        // Populate the correct slot; reading an arbitrary other slot
        // must miss, not silently decode garbage.
        let correct_slot = EEZ_AUTHORIZED_PROXIES_SLOT;
        let wrong_slot: u8 = 7;
        assert_ne!(
            correct_slot, wrong_slot,
            "test premise: the two slots must differ"
        );

        let key = proxy_mapping_key(PROXY_ADDR, correct_slot);
        let value = packed_proxy_value(DESTINATION_ADDR, TARGET_ROLLUP);

        let mut cache_db = CacheDB::<EmptyDB>::default();
        cache_db
            .insert_account_storage(EEZ_ADDRESS, key.into(), value)
            .expect("populate storage");

        let mut ctx = Context::mainnet().with_db(cache_db);
        ctx.journal_mut()
            .load_account(EEZ_ADDRESS)
            .expect("load EEZ account");

        let info =
            lookup_authorized_proxy_live(&mut ctx, &lookup(EEZ_ADDRESS, wrong_slot), PROXY_ADDR);
        assert!(
            info.is_none(),
            "reading with the wrong slot must miss, not silently decode garbage"
        );
    }

    #[test]
    fn eez_slot_path_reads_slot_0() {
        // ProxyLookupConfig with source_contract=EEZ routes to slot 0
        // (authorizedProxies declared on EEZBase, first storage slot
        // of every child).
        let config = ProxyLookupConfig {
            contract_address: EEZ_ADDRESS,
            authorized_proxies_slot: EEZ_AUTHORIZED_PROXIES_SLOT,
        };
        assert_eq!(config.authorized_proxies_slot, 0u8);

        let key = proxy_mapping_key(PROXY_ADDR, config.authorized_proxies_slot);
        let value = packed_proxy_value(DESTINATION_ADDR, TARGET_ROLLUP);

        let mut cache_db = CacheDB::<EmptyDB>::default();
        cache_db
            .insert_account_storage(config.contract_address, key.into(), value)
            .expect("populate storage");
        let mut ctx = Context::mainnet().with_db(cache_db);
        ctx.journal_mut()
            .load_account(config.contract_address)
            .expect("load");

        let info = lookup_authorized_proxy_live(&mut ctx, &config, PROXY_ADDR)
            .expect("EEZ config path must find proxy");
        assert_eq!(info.original_rollup_id, RollupId(TARGET_ROLLUP));
        assert_eq!(info.original_address, DESTINATION_ADDR);
    }

    #[test]
    fn eezl2_slot_path_reads_slot_0() {
        // Keep the EEZL2 constant path distinct so each dialect's configured
        // slot is tested.
        let config = ProxyLookupConfig {
            contract_address: EEZL2_ADDRESS,
            authorized_proxies_slot: EEZL2_AUTHORIZED_PROXIES_SLOT,
        };
        assert_eq!(config.authorized_proxies_slot, 0u8);

        let key = proxy_mapping_key(PROXY_ADDR, config.authorized_proxies_slot);
        let value = packed_proxy_value(DESTINATION_ADDR, TARGET_ROLLUP);

        let mut cache_db = CacheDB::<EmptyDB>::default();
        cache_db
            .insert_account_storage(config.contract_address, key.into(), value)
            .expect("populate storage");
        let mut ctx = Context::mainnet().with_db(cache_db);
        ctx.journal_mut()
            .load_account(config.contract_address)
            .expect("load");

        let info = lookup_authorized_proxy_live(&mut ctx, &config, PROXY_ADDR)
            .expect("EEZL2 config path must find proxy");
        assert_eq!(info.original_rollup_id, RollupId(TARGET_ROLLUP));
        assert_eq!(info.original_address, DESTINATION_ADDR);
    }

    #[test]
    fn natural_cross_chain_revert_does_not_create_rollback_span() {
        let span = reverted_dispatch_span(
            FrameBoundary {
                start: 3,
                intercepted_cross_chain: true,
            },
            4,
            InstructionResult::Revert,
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
            InstructionResult::Revert,
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
            InstructionResult::Return,
            false,
        );

        assert_eq!(span, None);
    }
}
