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
use alloy_primitives::{Address, Bytes, TxKind};
use alloy_sol_types::SolCall;
use reth_evm::{ConfigureEvm, Evm as _};
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::SealedHeader;
use reth_revm::{database::StateProviderDatabase, db::State, db::bal::EvmDatabaseError};
use reth_storage_api::StateProviderBox;
use reth_storage_api::errors::provider::ProviderError;
use revm::context::result::EVMError;
use revm::database::CacheState;
use revm::interpreter::{CallInputs, CallOutcome};
use revm::{DatabaseCommit, Inspector};

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
        let evm_env = provider
            .evm_config
            .evm_env(self.anchor.header())
            .map_err(evm_err)?;
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
    /// The balance check stays ON — escrowed value must really be payable from
    /// the manager's balance, exactly as on-chain.
    ///
    /// # Errors
    ///
    /// Returns [`ExecutorErrorKind::Provider`] when the anchor state cannot be
    /// opened, [`ExecutorErrorKind::Evm`] when env construction fails.
    pub fn new(state: &L1SlotState, client: Arc<LocalChainClient>) -> ExecutorResult<Self> {
        let (state, mut evm_env) = state.open_state(&client, state.cache.clone())?;
        let chain_id = evm_env.cfg_env.chain_id;
        // Synthetic frames carry no fee market and no EOA sender; the balance
        // check is deliberately left on so escrow value is real.
        evm_env.cfg_env.disable_base_fee = true;
        evm_env.cfg_env.disable_eip3607 = true;
        evm_env.cfg_env.disable_nonce_check = true;
        evm_env.cfg_env.tx_gas_limit_cap = Some(u64::MAX);

        let evm_config = client.chain_provider().evm_config.clone();
        let manager = client.manager_address();
        Ok(Self {
            client,
            evm_config,
            state,
            evm_env,
            manager,
            chain_id,
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
        // `Encoding` because `sim_error_is_poison` classifies it POISON: a call
        // mode is fixed by the tx, so retrying it re-fails forever.
        if req.call_mode == CallMode::Static {
            return Err(ExecutorErrorKind::Encoding(
                "static target execution is not supported".into(),
            )
            .into());
        }

        let proxy = self.proxy_address(req.source_address, req.source_rollup_id)?;
        if !self.is_authorized_proxy(proxy)? {
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
            caller: self.manager,
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
        let inspector = SkipTopFrame::new(self.client.inspector_factory().build(dispatcher));
        let (return_data, gas_used, success, mut changes, inspector_error) = {
            let mut evm = self.evm_config.evm_with_env_and_inspector(
                &mut self.state,
                self.evm_env.clone(),
                inspector,
            );
            let result = evm.transact(tx_env).map_err(transact_err)?;
            let inspector_error = evm.inspector_mut().inner.take_error();
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
        if success {
            reset_frame_caller_nonce(&mut changes, self.manager);
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
}

impl<I> SkipTopFrame<I> {
    fn new(inner: I) -> Self {
        Self { inner, depth: 0 }
    }
}

impl<CTX, I: Inspector<CTX>> Inspector<CTX> for SkipTopFrame<I> {
    fn call(&mut self, context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
        let top = self.depth == 0;
        self.depth += 1;
        if top {
            return None;
        }
        self.inner.call(context, inputs)
    }

    fn call_end(&mut self, context: &mut CTX, inputs: &CallInputs, outcome: &mut CallOutcome) {
        self.depth = self.depth.saturating_sub(1);
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
        if req.call_mode == CallMode::Static {
            return Err(ExecutorErrorKind::Encoding(
                "static target execution is not supported".into(),
            )
            .into());
        }

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
        let mut inspector = factory.build(dispatcher);
        let outcome = self
            .fork
            .execute_call_inspected(tx_env, &mut inspector)
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
