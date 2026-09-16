//! Slot-scoped target-execution contexts for chained-interstate composition.
//!
//! Both implement [`TargetExecutionSession`], but neither approximates the
//! protocol: each runs the real contract path the chain will run.
//!
//! - [`L1TargetSession`] replays `EEZ._processNCalls`' frames
//!   (`EEZ.sol:1149-1178`) over an [`L1SlotState`] pinned at drain start,
//!   committing every surviving effect so later simulations in the same slot
//!   observe it.
//! - [`InboundL2TargetSession`] executes the destination frame on a fork of the
//!   Sync block under construction, with the regular session inspector active
//!   so nested calls recurse through the same composition tree. The canonical
//!   delivery transaction is replayed and checked at accept time.

use std::sync::Arc;

use alloy_consensus::Header;
use alloy_primitives::{Address, Bytes, TxKind, U256};
use alloy_sol_types::SolCall;
use reth_evm::{ConfigureEvm, Evm as _};
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::SealedHeader;
use reth_revm::{database::StateProviderDatabase, db::State, db::bal::EvmDatabaseError};
use reth_storage_api::StateProviderBox;
use reth_storage_api::errors::provider::ProviderError;
use revm::context::result::EVMError;
use revm::context_interface::{ContextSetters, ContextTr, JournalTr};
use revm::database::CacheState;
use revm::interpreter::{CallInputs, CallOutcome, Interpreter};
use revm::{Database, DatabaseCommit, Inspector};

use eez_protocol::abi::{
    authorizedProxiesCall, computeCrossChainProxyAddressCall, createCrossChainProxyCall,
    executeOnBehalfCall,
};
use eez_protocol::{
    CallMode, CompositionBuilder, ExecutionOutcome, ExecutionRequest, ExecutorError,
    ExecutorErrorKind, ExecutorResult, RollupId, SessionSnapshot, TargetExecutionSession,
};

use super::build::{BuildError, ForkSnapshot, SyncBlockFork};
use super::client::LocalChainClient;
use super::reset_frame_caller_nonce;
use super::session::{DIRECT_CALL_GAS_LIMIT, evm_err, provider_err};

/// The concrete local clients slot-scoped composition needs, alongside the
/// type-erased `ChainClient` views on the wiring. `l1_entry` is the same
/// instance the wiring's rollups map holds (they must share one overlay
/// channel); `l2_entry` is the dedicated L2 ENTRY client (the map holds the
/// L2 follower). The erased trait hides the simulation surfaces
/// ([`L1SlotState`], [`LocalChainClient::simulate_source_tx_on`]) the drain
/// drives.
///
/// L2's two role views share one overlay channel, so a nested call from L1 can
/// return through the follower client into the suspended entry simulation's
/// exact in-flight state.
#[derive(Debug, Clone)]
pub struct LocalComposeClients {
    /// L1 entry client — the state the outbound manager frames run on.
    pub l1_entry: Arc<LocalChainClient>,
    /// L2 entry client — the chain whose Sync block is under construction.
    pub l2_entry: Arc<LocalChainClient>,
}

/// Gas cap for the manager's `computeCrossChainProxyAddress` /
/// `authorizedProxies` view frames.
const VIEW_CALL_GAS_LIMIT: u64 = 1_000_000;

/// `CrossChainProxy.executeOnBehalf`'s `callGas` argument. Zero forwards all
/// remaining gas — the only shape the protocol emits (`USE_GAS_LEFT` off).
const ZERO_CALL_GAS: u64 = 0;

fn encoding_err(msg: impl Into<String>) -> ExecutorError {
    ExecutorError::from(ExecutorErrorKind::Encoding(msg.into()))
}

/// Classify a `transact` failure. Everything but a database read is a property
/// of the transaction (`Evm` ⇒ poison); a database read failure is the backing
/// store being unreachable, which `Provider` marks transient so the slot aborts
/// and retries instead of evicting a sound tx.
fn transact_err(e: EVMError<EvmDatabaseError<ProviderError>>) -> ExecutorError {
    match e {
        EVMError::Database(db) => provider_err(db),
        other => evm_err(other),
    }
}

/// The same split for the block-fork path: `BuildError::Provider` is the store,
/// everything else is the tx.
fn fork_err(e: BuildError) -> ExecutorError {
    if e.is_provider() {
        provider_err(e)
    } else {
        evm_err(e)
    }
}

// ── L1 state ─────────────────────────────────────────────────────

/// Slot-scoped simulated L1: an anchor header pinned at drain start plus the
/// accumulated cache of every effect committed this slot (manager frames and
/// inbound source sims).
///
/// The anchor never moves during a drain — the bundle lands at least one L1
/// block later regardless, and a moving base would make claims depend on
/// wall-clock arrival order (design §5, "L1 base drift").
#[derive(Debug)]
pub struct L1SlotState {
    /// L1 head at drain start; every fork opens its state here.
    pub anchor: SealedHeader<Header>,
    /// Effects committed by surviving transactions, seeded into each fork.
    pub cache: CacheState,
    /// Enabled-policy target environment and actual poster frame context.
    /// Synthetic calls observe these values without charging transaction fees.
    pub posting: Option<(reth_evm::EvmEnvFor<EthEvmConfig>, Address, u128)>,
}

impl L1SlotState {
    /// Pin the client's best block and start with an empty effect cache.
    ///
    /// # Errors
    ///
    /// Returns [`ExecutorErrorKind::Provider`] when the head number or header
    /// cannot be read, [`ExecutorErrorKind::Missing`] when the header is absent.
    pub fn open(client: &LocalChainClient) -> ExecutorResult<Self> {
        let provider = client.chain_provider();
        let num = provider.headers.best_block_number().map_err(provider_err)?;
        let header = provider
            .headers
            .header_by_number(num)
            .map_err(provider_err)?
            .ok_or_else(|| ExecutorError::from(ExecutorErrorKind::Missing("L1 anchor header")))?;
        let anchor = SealedHeader::seal_slow(header);
        tracing::debug!(
            block = num,
            hash = %anchor.hash(),
            "L1 state anchored for this slot"
        );
        Ok(Self {
            anchor,
            cache: CacheState::default(),
            posting: None,
        })
    }

    /// Anchor post-state preloaded with `seed`, plus the anchor's plain EVM
    /// env. Every fork of this state opens through here.
    fn open_state(
        &self,
        client: &LocalChainClient,
        seed: CacheState,
    ) -> ExecutorResult<(
        State<StateProviderDatabase<StateProviderBox>>,
        reth_evm::EvmEnvFor<EthEvmConfig>,
    )> {
        let provider = client.chain_provider();
        let state_prov = provider
            .provider
            .state_by_block_hash(self.anchor.hash())
            .map_err(provider_err)?;
        let evm_env = if let Some((env, _, _)) = &self.posting {
            env.clone()
        } else {
            provider
                .evm_config
                .evm_env(self.anchor.header())
                .map_err(evm_err)?
        };
        let state = State::builder()
            .with_database(StateProviderDatabase::new(state_prov))
            .with_cached_prestate(seed)
            .with_bundle_update()
            .build();
        Ok((state, evm_env))
    }

    /// Open a fork of this state for an inbound SOURCE simulation: anchor
    /// post-state preloaded with the accumulated effect cache, plus the
    /// anchor's plain EVM env (`simulate_source_tx_on` applies its own
    /// source-sim cfg tweaks; the manager-frame tweaks stay out of it).
    ///
    /// # Errors
    ///
    /// Returns [`ExecutorErrorKind::Provider`] when the anchor state cannot be
    /// opened, [`ExecutorErrorKind::Evm`] when env construction fails.
    pub fn fork_state(
        &self,
        client: &LocalChainClient,
    ) -> ExecutorResult<(
        State<StateProviderDatabase<StateProviderBox>>,
        reth_evm::EvmEnvFor<EthEvmConfig>,
    )> {
        self.open_state(client, self.cache.clone())
    }
}

// ── L1 manager execution ─────────────────────────────────────────

/// Target-chain session that replays `EEZ._processNCalls`' frames on a fork of
/// the [`L1SlotState`].
///
/// ## Checkpoint payload contract
///
/// [`checkpoint`](TargetExecutionSession::checkpoint) returns a boxed
/// [`CacheState`] and nothing else. The drain relies on that: it reclaims the
/// session through `CompositionBuilder::take_sessions`, calls `checkpoint()`,
/// downcasts to `CacheState`, and commits it into `L1SlotState::cache` on
/// survivor-accept. Changing the payload type breaks that hand-off.
pub struct L1TargetSession {
    client: Arc<LocalChainClient>,
    evm_config: EthEvmConfig,
    state: State<StateProviderDatabase<StateProviderBox>>,
    evm_env: reth_evm::EvmEnvFor<EthEvmConfig>,
    manager: Address,
    chain_id: u64,
    posting_origin: Option<Address>,
    posting_gas_price: u128,
    defer_source_conditions: bool,
}

impl std::fmt::Debug for L1TargetSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("L1TargetSession")
            .field("manager", &self.manager)
            .field("chain_id", &self.chain_id)
            .finish_non_exhaustive()
    }
}

impl L1TargetSession {
    /// Fork the L1 state: open the anchor's post-state seeded with the state's
    /// accumulated cache, under the manager-frame EVM env.
    ///
    /// Transaction balance validation is disabled for these synthetic frames.
    /// The manager is a contract, not a fee-paying EOA, and revm otherwise
    /// treats `msg.value` as transaction cost before the composer can apply the
    /// authoritative per-rollup escrow check. The caller validates and debits
    /// `rollups[rollupId].etherBalance` before accepting the composition.
    ///
    /// # Errors
    ///
    /// Returns [`ExecutorErrorKind::Provider`] when the anchor state cannot be
    /// opened, [`ExecutorErrorKind::Evm`] when env construction fails.
    pub fn new(
        state: &L1SlotState,
        client: Arc<LocalChainClient>,
        defer_source_conditions: bool,
    ) -> ExecutorResult<Self> {
        let posting_origin = state.posting.as_ref().map(|(_, origin, _)| *origin);
        let posting_gas_price = state.posting.as_ref().map_or(0, |(_, _, price)| *price);
        let (state, mut evm_env) = state.open_state(&client, state.cache.clone())?;
        let chain_id = evm_env.cfg_env.chain_id;
        // Synthetic frames carry no fee market and no EOA sender. In
        // particular, the manager's `msg.value` is contract execution funded
        // from rollup escrow, not a transaction fee paid by `caller`.
        evm_env.cfg_env.disable_base_fee = true;
        evm_env.cfg_env.disable_balance_check = true;
        evm_env.cfg_env.disable_eip3607 = true;
        evm_env.cfg_env.disable_nonce_check = true;
        evm_env.cfg_env.tx_gas_limit_cap = Some(u64::MAX);
        evm_env.cfg_env.disable_fee_charge = posting_origin.is_some();

        let evm_config = client.chain_provider().evm_config.clone();
        let manager = client.manager_address();
        Ok(Self {
            client,
            evm_config,
            state,
            evm_env,
            manager,
            chain_id,
            posting_origin,
            posting_gas_price,
            defer_source_conditions,
        })
    }

    /// Clamp a frame's gas to the anchor block's gas limit — revm rejects a
    /// caller gas limit above it, and real L1 execution has no more either
    /// (chiado's block limit is well below `DIRECT_CALL_GAS_LIMIT`).
    fn frame_gas(&self, requested: u64) -> u64 {
        requested.min(self.evm_env.block_env.gas_limit)
    }

    /// Run one synthetic frame against the manager. `commit` keeps its writes
    /// on the fork; a view frame drops them.
    fn manager_frame(
        &mut self,
        data: Vec<u8>,
        gas_limit: u64,
        commit: bool,
    ) -> ExecutorResult<Bytes> {
        let tx_env = revm::context::TxEnv {
            caller: Address::ZERO,
            gas_limit: self.frame_gas(gas_limit),
            kind: TxKind::Call(self.manager),
            data: data.into(),
            chain_id: Some(self.chain_id),
            ..Default::default()
        };
        let result = {
            let mut evm = self
                .evm_config
                .evm_with_env(&mut self.state, self.evm_env.clone());
            evm.transact(tx_env).map_err(transact_err)?
        };
        if !result.result.is_success() {
            return Err(encoding_err(format!(
                "manager frame reverted on {}: {:?}",
                self.manager,
                result.result.output()
            )));
        }
        let output = result.result.output().cloned().unwrap_or_default();
        if commit {
            let mut changes = result.state;
            reset_frame_caller_nonce(&mut changes, Address::ZERO);
            self.state.commit(changes);
        }
        Ok(output)
    }

    /// Run a read-only manager frame and drop its state.
    fn view_call(&mut self, data: Vec<u8>) -> ExecutorResult<Bytes> {
        self.manager_frame(data, VIEW_CALL_GAS_LIMIT, false)
    }

    /// `EEZ.sol:1150` — `computeCrossChainProxyAddress(source, sourceRollup)`.
    fn proxy_address(
        &mut self,
        source: Address,
        source_rollup: RollupId,
    ) -> ExecutorResult<Address> {
        let out = self.view_call(
            computeCrossChainProxyAddressCall {
                originalAddress: source,
                originalRollupId: source_rollup.0,
            }
            .abi_encode(),
        )?;
        computeCrossChainProxyAddressCall::abi_decode_returns(&out)
            .map_err(|e| encoding_err(format!("decode computeCrossChainProxyAddress: {e}")))
    }

    /// `EEZ.sol:1151` — `authorizedProxies[proxy].isProxy`.
    fn is_authorized_proxy(&mut self, proxy: Address) -> ExecutorResult<bool> {
        let out = self.view_call(authorizedProxiesCall { proxy }.abi_encode())?;
        authorizedProxiesCall::abi_decode_returns(&out)
            .map(|info| info.isProxy)
            .map_err(|e| encoding_err(format!("decode authorizedProxies: {e}")))
    }

    /// Read-only registration lookup for a source identity. Admission opens
    /// a separate session at the canonical parent so speculative creations
    /// (including ones later reverted) cannot receive an existing-proxy price.
    pub(crate) fn proxy_is_registered(
        &mut self,
        source: Address,
        source_rollup: RollupId,
    ) -> ExecutorResult<bool> {
        let proxy = self.proxy_address(source, source_rollup)?;
        self.is_authorized_proxy(proxy)
    }

    /// `EEZ.sol:1152` — the permissionless CREATE2 deployment + registration
    /// the manager performs itself when the proxy is missing.
    fn create_proxy(&mut self, source: Address, source_rollup: RollupId) -> ExecutorResult<()> {
        let calldata = createCrossChainProxyCall {
            originalAddress: source,
            originalRollupId: source_rollup.0,
        }
        .abi_encode();
        self.manager_frame(calldata, DIRECT_CALL_GAS_LIMIT, true)
            .map_err(|e| {
                encoding_err(format!(
                    "createCrossChainProxy({source}, {source_rollup}): {e}"
                ))
            })?;
        tracing::debug!(%source, %source_rollup, "L1 state: proxy created");
        Ok(())
    }
}

impl TargetExecutionSession for L1TargetSession {
    fn execute(
        &mut self,
        req: ExecutionRequest,
        dispatcher: &mut CompositionBuilder,
    ) -> ExecutorResult<ExecutionOutcome> {
        // An unfunded manager frame fails before composition can materialize
        // the rollup's exact ether delta. Treat missing physical backing as a
        // source wait; the caller still checks the per-rollup escrow before
        // accepting any successful composition. Read this fork's state so the
        // accepted prefix and nested calls retain their normal value semantics.
        if self.defer_source_conditions && !req.value.is_zero() {
            let available = self
                .state
                .basic(self.manager)
                .map_err(provider_err)?
                .map_or(U256::ZERO, |account| account.balance);
            if req.value > available {
                return Err(ExecutorErrorKind::SourceBlocked(
                    eez_protocol::SourceCondition::Escrow {
                        required: req.value,
                        available,
                    },
                )
                .into());
            }
        }
        let proxy = self.proxy_address(req.source_address, req.source_rollup_id)?;
        if !self.is_authorized_proxy(proxy)? {
            // `_processNCalls` creates and registers a missing source proxy in
            // its mutable postBatch frame before dispatching the requested
            // call. `Static` constrains that dispatch frame, not this protocol
            // preparation, so the proxy creation must survive either mode.
            self.create_proxy(req.source_address, req.source_rollup_id)?;
        }

        // The manager frame: the proxy sees `msg.sender == manager` and
        // forwards value + data to the target (`CrossChainProxy.sol:50-64`).
        let calldata = executeOnBehalfCall {
            destination: req.target_address,
            callGas: ZERO_CALL_GAS,
            data: req.data.clone(),
        }
        .abi_encode();
        let tx_env = revm::context::TxEnv {
            caller: self.posting_origin.unwrap_or(self.manager),
            gas_limit: self.frame_gas(DIRECT_CALL_GAS_LIMIT),
            kind: TxKind::Call(proxy),
            data: calldata.into(),
            value: req.value,
            chain_id: Some(self.chain_id),
            ..Default::default()
        };

        // Inspected so nested proxy calls inside the target are recorded in
        // the same composition tree. The outermost frame is skipped: it
        // targets an authorized proxy itself, and intercepting it would
        // re-dispatch the very frame that IS the dispatch.
        let mut inspector = SkipTopFrame::new(if req.call_mode == CallMode::Static {
            self.client.inspector_factory().build_static(dispatcher)
        } else {
            self.client.inspector_factory().build(dispatcher)
        });
        inspector.top_caller = self.posting_origin.map(|_| self.manager);
        inspector.top_gas_price = self.posting_origin.map(|_| self.posting_gas_price);
        let (return_data, gas_used, success, mut changes, inspector_error) = {
            let mut evm = self.evm_config.evm_with_env_and_inspector(
                &mut self.state,
                self.evm_env.clone(),
                inspector,
            );
            let result = evm.transact(tx_env).map_err(transact_err)?;
            let inspector = evm.inspector_mut();
            let inspector_error = inspector
                .error
                .take()
                .or_else(|| inspector.inner.take_error());
            (
                result.result.output().cloned().unwrap_or_default(),
                result.result.tx_gas_used(),
                result.result.is_success(),
                result.state,
                inspector_error,
            )
        };
        if let Some(err) = inspector_error {
            return Err(err);
        }

        // A reverted frame's only state today is the caller's nonce bump, but
        // committing it would tie the slot-shared state to whatever a future
        // revm decides to return in `result.state` for a revert.
        if success && req.call_mode == CallMode::Mutable {
            reset_frame_caller_nonce(&mut changes, self.posting_origin.unwrap_or(self.manager));
            self.state.commit(changes);
        }

        tracing::debug!(
            target = %req.target_address,
            %proxy,
            value = %req.value,
            success,
            gas_used,
            "L1 manager frame executed"
        );

        // The frame's raw output IS what `_processNCalls` folds into CALL_END
        // (`EEZ.sol:1181`) — revert data included on failure.
        Ok(ExecutionOutcome::Resolved {
            return_data: return_data.to_vec(),
            gas_used,
            success,
        })
    }

    fn checkpoint(&mut self) -> ExecutorResult<SessionSnapshot> {
        // Cache-only is sound here: simulation reads state exclusively through
        // the cache, and the bundle/transition state is never consulted.
        Ok(Box::new(self.state.cache.clone()))
    }

    fn rollback(&mut self, snapshot: SessionSnapshot) -> ExecutorResult<()> {
        let cache = snapshot
            .downcast::<CacheState>()
            .map_err(|_e| encoding_err("L1TargetSession::rollback: snapshot type mismatch"))?;
        self.state.cache = *cache;
        Ok(())
    }
}

/// Delegating inspector that hides the OUTERMOST call frame from `inner`.
///
/// The manager frame's own target is an authorized proxy, so the session
/// inspector would intercept the top-level frame and re-dispatch the very
/// call this session is executing. Nested frames still forward, so proxy
/// calls made *inside* the target are recorded as usual. A revert of the top
/// frame itself is represented by the outer call's unsuccessful outcome; the
/// terminal failure is materialized without a separate revert-span marker.
struct SkipTopFrame<I> {
    inner: I,
    /// Open CALL frames; the top frame is the one entered at depth 0.
    depth: usize,
    top_caller: Option<Address>,
    error: Option<ExecutorError>,
    top_gas_price: Option<u128>,
    original_gas_price: Option<u128>,
}

impl<I> SkipTopFrame<I> {
    fn new(inner: I) -> Self {
        Self {
            inner,
            depth: 0,
            top_caller: None,
            error: None,
            top_gas_price: None,
            original_gas_price: None,
        }
    }
}

impl<CTX: ContextTr<Tx = revm::context::TxEnv> + ContextSetters, I: Inspector<CTX>> Inspector<CTX>
    for SkipTopFrame<I>
{
    fn initialize_interp(&mut self, interp: &mut Interpreter, context: &mut CTX) {
        self.inner.initialize_interp(interp, context);
    }

    fn call(&mut self, context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
        let top = self.depth == 0;
        self.depth += 1;
        if top {
            // In pinned REVM 38, disable_fee_charge skips upfront deduction
            // but reimburse_caller/reward_beneficiary still use GASPRICE.
            // Expose the actual price only inside execution and restore the
            // synthetic zero price before transaction-level fee accounting.
            if let Some(price) = self.top_gas_price {
                self.original_gas_price = Some(context.tx().gas_price);
                let mut tx = context.tx().clone();
                tx.gas_price = price;
                context.set_tx(tx);
            }
            if let Some(caller) = self.top_caller {
                // A synthetic top-level transaction initially loads its EOA.
                // The actual value-paying caller is the manager, already warm
                // in a real postBatch frame. Load it before REVM transfers value.
                if let Err(error) = context.journal_mut().load_account(caller) {
                    self.error = Some(provider_err(error));
                    return None;
                }
                inputs.caller = caller;
            }
            return None;
        }
        self.inner.call(context, inputs)
    }

    fn call_end(&mut self, context: &mut CTX, inputs: &CallInputs, outcome: &mut CallOutcome) {
        self.depth = self.depth.saturating_sub(1);
        if self.depth == 0
            && let Some(price) = self.original_gas_price.take()
        {
            let mut tx = context.tx().clone();
            tx.gas_price = price;
            context.set_tx(tx);
        }
        if self.depth > 0 {
            self.inner.call_end(context, inputs, outcome);
        }
    }
}

// ── L2 block-fork execution ──────────────────────────────────────

/// Target-chain session that resolves an inbound call by running the canonical
/// destination frame on a fork of the Sync block under construction.
pub struct InboundL2TargetSession {
    fork: SyncBlockFork,
    client: Arc<LocalChainClient>,
}

impl std::fmt::Debug for InboundL2TargetSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboundL2TargetSession")
            .field("manager", &self.client.manager_address())
            .finish_non_exhaustive()
    }
}

impl InboundL2TargetSession {
    /// Open over a fork of the current Sync-block prefix.
    #[must_use]
    pub fn new(fork: SyncBlockFork, client: Arc<LocalChainClient>) -> Self {
        Self { fork, client }
    }

    fn proxy_address(&mut self, req: &ExecutionRequest) -> ExecutorResult<Address> {
        let calldata = computeCrossChainProxyAddressCall {
            originalAddress: req.source_address,
            originalRollupId: req.source_rollup_id.0,
        }
        .abi_encode();
        let tx_env = revm::context::TxEnv {
            caller: Address::ZERO,
            gas_limit: VIEW_CALL_GAS_LIMIT,
            kind: TxKind::Call(self.client.manager_address()),
            data: calldata.into(),
            chain_id: Some(self.fork.chain_id()),
            ..Default::default()
        };
        let output = self.fork.view_call(tx_env).map_err(fork_err)?;
        computeCrossChainProxyAddressCall::abi_decode_returns(&output)
            .map_err(|error| encoding_err(format!("decode computeCrossChainProxyAddress: {error}")))
    }
}

impl TargetExecutionSession for InboundL2TargetSession {
    fn execute(
        &mut self,
        req: ExecutionRequest,
        dispatcher: &mut CompositionBuilder,
    ) -> ExecutorResult<ExecutionOutcome> {
        let proxy = self.proxy_address(&req)?;
        let tx_env = revm::context::TxEnv {
            caller: proxy,
            gas_limit: DIRECT_CALL_GAS_LIMIT,
            kind: TxKind::Call(req.target_address),
            data: req.data.clone(),
            value: req.value,
            chain_id: Some(self.fork.chain_id()),
            ..Default::default()
        };
        let factory = self.client.inspector_factory();
        let mut inspector = if req.call_mode == CallMode::Static {
            factory.build_static(dispatcher)
        } else {
            factory.build(dispatcher)
        };
        let outcome = if req.call_mode == CallMode::Static {
            self.fork
                .execute_static_call_inspected(tx_env, &mut inspector)
        } else {
            self.fork.execute_call_inspected(tx_env, &mut inspector)
        }
        .map_err(fork_err)?;
        if let Some(error) = inspector.take_error() {
            return Err(error);
        }

        tracing::debug!(
            target = %req.target_address,
            value = %req.value,
            success = outcome.success,
            return_len = outcome.output.len(),
            "inbound target frame simulated on the Sync-block fork"
        );

        Ok(ExecutionOutcome::Resolved {
            return_data: outcome.output.to_vec(),
            gas_used: outcome.gas_used,
            success: outcome.success,
        })
    }

    fn checkpoint(&mut self) -> ExecutorResult<SessionSnapshot> {
        Ok(Box::new(self.fork.snapshot()))
    }

    fn rollback(&mut self, snapshot: SessionSnapshot) -> ExecutorResult<()> {
        let snap = snapshot.downcast::<ForkSnapshot>().map_err(|_e| {
            encoding_err("InboundL2TargetSession::rollback: snapshot type mismatch")
        })?;
        self.fork.restore(*snap);
        Ok(())
    }
}

#[cfg(test)]
mod posting_frame_tests {
    use super::*;
    use revm::{
        context::{BlockEnv, CfgEnv, TxEnv},
        database::InMemoryDB,
        inspector::NoOpInspector,
        primitives::hardfork::SpecId,
        state::{AccountInfo, Bytecode},
    };

    #[test]
    fn priced_synthetic_frame_has_real_origin_and_caller_without_charging_fees() {
        let poster = Address::repeat_byte(1);
        let manager = Address::repeat_byte(2);
        let proxy = Address::repeat_byte(3);
        // Return ORIGIN, GASPRICE, CALLER, CALLVALUE, and SELFBALANCE.
        let code = Bytes::from_static(&[
            0x32, 0x5f, 0x52, 0x3a, 0x60, 32, 0x52, 0x33, 0x60, 64, 0x52, 0x34, 0x60, 96, 0x52,
            0x47, 0x60, 128, 0x52, 0x60, 160, 0x5f, 0xf3,
        ]);
        let mut database = InMemoryDB::default();
        database.insert_account_info(poster, AccountInfo::default());
        database.insert_account_info(
            manager,
            AccountInfo {
                balance: U256::from(20),
                ..Default::default()
            },
        );
        database.insert_account_info(
            proxy,
            AccountInfo {
                code: Some(Bytecode::new_raw(code)),
                ..Default::default()
            },
        );
        let mut cfg = CfgEnv::new().with_spec_and_mainnet_gas_params(SpecId::OSAKA);
        cfg.disable_fee_charge = true;
        cfg.disable_balance_check = true;
        cfg.disable_base_fee = true;
        let env = reth_evm::EvmEnv {
            cfg_env: cfg,
            block_env: BlockEnv {
                basefee: 10,
                gas_limit: 1_000_000,
                ..Default::default()
            },
        };
        let mut inspector = SkipTopFrame::new(NoOpInspector {});
        inspector.top_caller = Some(manager);
        inspector.top_gas_price = Some(37);
        let mut result = EthEvmConfig::mainnet()
            .evm_with_env_and_inspector(database, env, inspector)
            .transact(TxEnv {
                caller: poster,
                kind: TxKind::Call(proxy),
                gas_limit: 100_000,
                value: U256::from(7),
                ..Default::default()
            })
            .unwrap();
        assert!(result.result.is_success());
        let values = result
            .result
            .output()
            .unwrap()
            .chunks_exact(32)
            .map(U256::from_be_slice)
            .collect::<Vec<_>>();
        assert_eq!(
            values,
            vec![
                U256::from_be_slice(poster.as_slice()),
                U256::from(37),
                U256::from_be_slice(manager.as_slice()),
                U256::from(7),
                U256::from(7)
            ]
        );
        reset_frame_caller_nonce(&mut result.state, poster);
        assert_eq!(
            result.state[&poster].info.balance,
            U256::ZERO,
            "a synthetic frame must not pay or mint fees"
        );
        assert_eq!(result.state[&poster].info.nonce, 0);
        assert_eq!(result.state[&manager].info.balance, U256::from(13));
        assert_eq!(result.state[&proxy].info.balance, U256::from(7));
    }
}
