//! Pending application simulation after the real EEZ pre-execution prefix.
//!
//! Reth's environment-only fallback reads the parent state. Keep that fallback
//! guarded, and supply executed prefix state to single-transaction simulation.
//! Their algorithms remain Reth's; this adapter supplies one request-local state
//! and environment and never publishes a pending block or modifies the pool.

#[cfg(test)]
mod tests;

use super::{Converter, EezEthApiBuilder};
use crate::node::EezNode;
use alloy_primitives::{Address, B256, Bytes, U256};
use alloy_rpc_types_eth::{
    AccessListResult, BlockId, BlockOverrides, TransactionRequest,
    state::{EvmOverrides, StateOverride},
};
use alloy_rpc_types_trace::geth::{GethDebugTracingCallOptions, GethTrace};
use eez_evm::EezEvmConfig;
use eez_protocol::l1_history::WitnessHistory;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};
use reth_errors::{ProviderError, RethError};
use reth_evm::{
    ConfigureEvm, EvmEnvFor, HaltReasonFor, NextBlockEnvAttributes, execute::BlockBuilder,
};
use reth_node_api::FullNodeComponents;
use reth_node_builder::rpc::RpcContext;
use reth_primitives_traits::{Account, Bytecode, SealedHeader, SignedTransaction};
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_rpc::{DebugApi, EthApi};
use reth_rpc_convert::RpcTxReq;
use reth_rpc_eth_api::{
    EthApiServer, EthApiTypes, RpcNodeCore,
    helpers::{
        Call, EthCall, GetBlockAccessList, LoadBlock, LoadPendingBlock, LoadState, LoadTransaction,
        SpawnBlocking, Trace, TraceExt, estimate::EstimateCall, pending_block::PendingEnvBuilder,
    },
    node::RpcNodeCoreExt,
};
use reth_rpc_eth_types::{EthApiError, EthApiSettings, EthStateCache, PendingBlock, StateCacheDb};
use reth_storage_api::{
    BlockReader, BlockReaderIdExt, EvmStateProvider, StateProvider, StateProviderFactory,
};
use reth_tasks::{
    Runtime,
    pool::{BlockingTaskGuard, BlockingTaskPool},
};
use revm::{DatabaseRef, context_interface::result::ResultAndState};
use std::sync::{Arc, Mutex};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(super) fn install<N>(
    ctx: RpcContext<'_, N, EthApi<N, Converter>>,
    settings: EezEthApiBuilder,
) -> eyre::Result<()>
where
    N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>,
{
    let module = SimulationRpc {
        inner: ctx.registry.eth_api().clone(),
        settings: settings.clone(),
    }
    .into_rpc();
    // Preserve namespace selection on each transport, including an explicitly
    // disabled eth namespace. The embedded L1 does not use these add-ons.
    ctx.modules
        .add_or_replace_if_module_configured("eth".parse()?, module)?;
    let eth = ctx.registry.eth_api().clone();
    let trace_guard = ctx.registry.blocking_pool_guard();
    let module = TraceCallRpc {
        inner: eth,
        settings,
        trace_guard,
    }
    .into_rpc();
    ctx.modules
        .add_or_replace_if_module_configured("debug".parse()?, module)?;
    Ok(())
}

// Match Reth's names as well as types: jsonrpsee preserves positional params,
// optional trailing params, and named params with snake_case/camelCase aliases.
#[rpc(server, namespace = "eth")]
trait Simulation {
    #[method(name = "call")]
    async fn call(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> RpcResult<Bytes>;
    #[method(name = "estimateGas")]
    async fn estimate_gas(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_override: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> RpcResult<U256>;
    #[method(name = "createAccessList")]
    async fn create_access_list(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_override: Option<StateOverride>,
    ) -> RpcResult<AccessListResult>;
}

struct SimulationRpc<N: RpcNodeCore> {
    inner: EthApi<N, Converter>,
    settings: EezEthApiBuilder,
}

#[jsonrpsee::core::async_trait]
impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> SimulationServer
    for SimulationRpc<N>
{
    async fn call(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_overrides: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> RpcResult<Bytes> {
        if let Some(api) = SimulationApi::prepare(
            self.inner.clone(),
            self.settings.clone(),
            block_number,
            None,
        )
        .await?
        {
            return Ok(EthCall::call(
                &api,
                request,
                block_number,
                EvmOverrides::new(state_overrides, block_overrides),
            )
            .await?);
        }
        EthApiServer::call(
            &self.inner,
            request,
            block_number,
            state_overrides,
            block_overrides,
        )
        .await
    }
    async fn estimate_gas(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_override: Option<StateOverride>,
        block_overrides: Option<Box<BlockOverrides>>,
    ) -> RpcResult<U256> {
        if let Some(api) = SimulationApi::prepare(
            self.inner.clone(),
            self.settings.clone(),
            block_number,
            None,
        )
        .await?
        {
            return Ok(EthCall::estimate_gas_at(
                &api,
                request,
                block_number.unwrap_or_default(),
                EvmOverrides::new(state_override, block_overrides),
            )
            .await?);
        }
        EthApiServer::estimate_gas(
            &self.inner,
            request,
            block_number,
            state_override,
            block_overrides,
        )
        .await
    }
    async fn create_access_list(
        &self,
        request: TransactionRequest,
        block_number: Option<BlockId>,
        state_override: Option<StateOverride>,
    ) -> RpcResult<AccessListResult> {
        if let Some(api) = SimulationApi::prepare(
            self.inner.clone(),
            self.settings.clone(),
            block_number,
            None,
        )
        .await?
        {
            return Ok(
                EthCall::create_access_list_at(&api, request, block_number, state_override).await?,
            );
        }
        EthApiServer::create_access_list(&self.inner, request, block_number, state_override).await
    }
}

// Keep the same parameter names and options as Reth's debug_traceCall.
#[rpc(server, namespace = "debug")]
trait TraceCall {
    #[method(name = "traceCall")]
    async fn trace_call(
        &self,
        request: TransactionRequest,
        block_id: Option<BlockId>,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<GethTrace>;
}

struct TraceCallRpc<N: RpcNodeCore> {
    inner: EthApi<N, Converter>,
    settings: EezEthApiBuilder,
    trace_guard: BlockingTaskGuard,
}

#[jsonrpsee::core::async_trait]
impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> TraceCallServer
    for TraceCallRpc<N>
{
    async fn trace_call(
        &self,
        request: TransactionRequest,
        block_id: Option<BlockId>,
        opts: Option<GethDebugTracingCallOptions>,
    ) -> RpcResult<GethTrace> {
        // This is the registry's aggregate tracing quota, shared with every
        // stock debug/trace/bundle method, not EthApi's distinct proof quota.
        let permit = Arc::new(
            self.trace_guard
                .clone()
                .acquire_owned()
                .await
                .map_err(|_| EthApiError::InternalEthError)?,
        );
        let opts = opts.unwrap_or_default();
        // txIndex addresses an actual block's transaction positions. Preserve
        // Reth's replay and bounds checks instead of inventing a pending block.
        let prepared = if opts.tx_index.is_none() {
            SimulationApi::prepare(
                self.inner.clone(),
                self.settings.clone(),
                block_id,
                Some(Arc::clone(&permit)),
            )
            .await?
        } else {
            None
        };
        let api = prepared.unwrap_or_else(|| SimulationApi {
            inner: self.inner.clone(),
            context: None,
            trace_permit: Some(permit),
        });
        // traceCall never reads the bad-block cache. Other debug methods keep
        // the registry's original instance and event subscription.
        let debug = DebugApi::new(
            api,
            self.trace_guard.clone(),
            self.inner.io_task_spawner(),
            futures_util::stream::empty(),
        );
        // The inherent method uses Reth's inspector and option handling;
        // its RPC wrapper would reacquire the permit already held here.
        Ok(execute_trace(debug, request, block_id, opts).await?)
    }
}

// Keep Reth's callback futures behind its generic RPC bound. Specializing them
// inside async_trait fails Send lifetime inference; prove Send before that
// boundary without adding a task or changing permit ownership.
#[expect(
    clippy::manual_async_fn,
    reason = "the explicit Send boundary is intentional"
)]
fn execute_trace<Eth: TraceExt>(
    debug: DebugApi<Eth>,
    request: RpcTxReq<Eth::NetworkTypes>,
    at: Option<BlockId>,
    opts: GethDebugTracingCallOptions,
) -> impl std::future::Future<Output = Result<GethTrace, Eth::Error>> + Send {
    async move { debug.debug_trace_call(request, at, opts).await }
}

struct PendingContext {
    parent_hash: B256,
    evm_config: EezEvmConfig,
    env: EvmEnvFor<EezEvmConfig>,
    state: Mutex<Option<StateCacheDb>>,
    // Every cloned adapter, including the blocking worker, retains this permit.
    // Dropping/timing out the RPC must not release it while execution continues.
    _permit: OwnedSemaphorePermit,
}

struct SimulationApi<N: RpcNodeCore> {
    inner: EthApi<N, Converter>,
    context: Option<Arc<PendingContext>>,
    // Retain this on the adapter, not just the prefix: tracing a stored block
    // or an executed pending payload can also outlive its cancelled RPC future.
    trace_permit: Option<Arc<OwnedSemaphorePermit>>,
}

impl<N: RpcNodeCore> Clone for SimulationApi<N> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            context: self.context.as_ref().map(Arc::clone),
            trace_permit: self.trace_permit.as_ref().map(Arc::clone),
        }
    }
}

impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> SimulationApi<N> {
    async fn prepare(
        inner: EthApi<N, Converter>,
        settings: EezEthApiBuilder,
        at: Option<BlockId>,
        trace_permit: Option<Arc<OwnedSemaphorePermit>>,
    ) -> Result<Option<Self>, EthApiError> {
        if !at.is_some_and(|at| at.is_pending()) {
            return Ok(None);
        }
        let permit = inner
            .acquire_owned_blocking_io()
            .await
            .map_err(|_| EthApiError::InternalEthError)?;
        inner
            .spawn_blocking_io(move |inner| {
                // An executed payload already includes its native prefix; let Reth
                // serve that payload's environment and state without applying it again.
                if inner.provider().pending_block_and_receipts()?.is_some() {
                    return Ok(None);
                }
                let (_, correlations) = settings.state.as_ref().ok_or_else(|| {
                    internal("L1 derivation is not configured for pending RPC simulation")
                })?;
                if !correlations.is_ready() {
                    return Err(internal("L1 derivation is still validating/catching up"));
                }
                let parent = inner
                    .provider()
                    .latest_header()?
                    .ok_or(EthApiError::HeaderNotFound(BlockId::latest()))?;
                let state = inner.provider().state_by_block_hash(parent.hash())?;
                let db = State::builder()
                    .with_database(StateProviderDatabase::new(state.into_evm_state_provider()))
                    .build();
                let (evm_config, env, db) = execute_prefix(inner.evm_config(), &parent, db)?;
                Ok(Some(Self {
                    inner,
                    context: Some(Arc::new(PendingContext {
                        parent_hash: parent.hash(),
                        evm_config,
                        env,
                        state: Mutex::new(Some(db)),
                        _permit: permit,
                    })),
                    trace_permit,
                }))
            })
            .await
    }
}

fn internal(message: &'static str) -> EthApiError {
    EthApiError::Internal(RethError::msg(message))
}

fn execution_error(error: impl std::error::Error + Send + Sync + 'static) -> EthApiError {
    EthApiError::Internal(RethError::other(error))
}

/// Build only the pre-application prefix. Parent state and the checked L1 header
/// snapshot are immutable throughout selection, execution and simulation.
pub(super) fn execute_prefix(
    evm_config: &EezEvmConfig,
    parent: &SealedHeader,
    mut db: StateCacheDb,
) -> Result<(EezEvmConfig, EvmEnvFor<EezEvmConfig>, StateCacheDb), EthApiError> {
    let (_, timestamp) = eez_evm::profile::schedule(evm_config.chain_spec())
        .and_then(|schedule| schedule.next(parent))
        .map_err(execution_error)?;
    let chain = evm_config.l1_headers(timestamp).map_err(execution_error)?;
    let evm_config = evm_config
        .clone()
        .with_l1_history(Arc::new(WitnessHistory(chain)))
        .map_err(execution_error)?;
    let (history, transaction) = evm_config
        .prepare_history(&mut db, parent, timestamp)
        .map_err(execution_error)?;
    let attributes = NextBlockEnvAttributes {
        timestamp,
        suggested_fee_recipient: parent.beneficiary,
        prev_randao: history.prev_randao,
        gas_limit: parent.gas_limit,
        parent_beacon_block_root: Some(B256::ZERO),
        withdrawals: Some(Default::default()),
        extra_data: parent.extra_data.clone(),
        slot_number: None,
    };
    let env = evm_config
        .next_evm_env(parent, &attributes)
        .map_err(execution_error)?;
    {
        let mut builder = evm_config
            .builder_for_next_block(&mut db, parent, attributes)
            .map_err(execution_error)?;
        builder
            .apply_pre_execution_changes()
            .map_err(execution_error)?;
        if let Some(transaction) = transaction {
            builder
                .execute_transaction(
                    transaction
                        .try_into_recovered()
                        .map_err(|_| internal("history signer recovery failed"))?,
                )
                .map_err(execution_error)?;
        }
        // As in Composer's open draft, drop without finish: the application
        // continues from this mid-block state, never from a synthetic persisted block.
    }
    Ok((evm_config, env, db))
}

impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> EthApiTypes for SimulationApi<N> {
    type Error = EthApiError;
    type NetworkTypes = super::EezRpcTypes;
    type RpcConvert = Converter;
    fn eth_api_settings(&self) -> &EthApiSettings {
        self.inner.eth_api_settings()
    }
    fn converter(&self) -> &Self::RpcConvert {
        self.inner.converter()
    }
}

impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> RpcNodeCore for SimulationApi<N> {
    type Primitives = <N as RpcNodeCore>::Primitives;
    type Provider = <N as RpcNodeCore>::Provider;
    type Pool = <N as RpcNodeCore>::Pool;
    type Evm = EezEvmConfig;
    type Network = <N as RpcNodeCore>::Network;
    fn pool(&self) -> &Self::Pool {
        self.inner.pool()
    }
    fn evm_config(&self) -> &Self::Evm {
        self.context
            .as_ref()
            .map_or_else(|| self.inner.evm_config(), |context| &context.evm_config)
    }
    fn network(&self) -> &Self::Network {
        self.inner.network()
    }
    fn provider(&self) -> &Self::Provider {
        self.inner.provider()
    }
}

impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> RpcNodeCoreExt
    for SimulationApi<N>
{
    fn cache(&self) -> &EthStateCache<Self::Primitives> {
        self.inner.cache()
    }
}

impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> SpawnBlocking
    for SimulationApi<N>
{
    fn io_task_spawner(&self) -> &Runtime {
        self.inner.io_task_spawner()
    }
    fn tracing_task_pool(&self) -> &BlockingTaskPool {
        self.inner.tracing_task_pool()
    }
    fn tracing_task_guard(&self) -> &BlockingTaskGuard {
        self.inner.tracing_task_guard()
    }
    fn blocking_io_task_guard(&self) -> &Arc<Semaphore> {
        self.inner.blocking_io_task_guard()
    }
}

impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> LoadPendingBlock
    for SimulationApi<N>
{
    fn pending_block(&self) -> &tokio::sync::Mutex<Option<PendingBlock<Self::Primitives>>> {
        self.inner.pending_block()
    }
    fn pending_env_builder(&self) -> &dyn PendingEnvBuilder<Self::Evm> {
        self.inner.pending_env_builder()
    }
}

impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> LoadState for SimulationApi<N> {
    async fn evm_env_at(
        &self,
        at: BlockId,
    ) -> Result<(EvmEnvFor<Self::Evm>, BlockId), Self::Error> {
        let Some(context) = &self.context else {
            return self.inner.evm_env_at(at).await;
        };
        if !at.is_pending() {
            return Err(internal("pending adapter received a historical request"));
        }
        Ok((context.env.clone(), context.parent_hash.into()))
    }
}

impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> Call for SimulationApi<N> {
    fn spawn_with_state_at_block<F, R>(
        &self,
        at: impl Into<BlockId>,
        f: F,
    ) -> impl std::future::Future<Output = Result<R, Self::Error>> + Send
    where
        F: FnOnce(Self, StateCacheDb) -> Result<R, Self::Error> + Send + 'static,
        R: Send + 'static,
    {
        let at = at.into();
        async move {
            let Some(context) = self.context.as_ref().map(Arc::clone) else {
                let this = self.clone();
                // Forward stored-state lookup to Reth, while the callback owns
                // the adapter/trace permit until the blocking worker exits.
                return self
                    .inner
                    .spawn_with_state_at_block(at, move |_, db| f(this, db))
                    .await;
            };
            if at != BlockId::from(context.parent_hash) {
                return Err(internal("pending adapter state/environment mismatch"));
            }
            self.spawn_blocking_io(move |this| {
                let db = context
                    .state
                    .lock()
                    .map_err(|_| internal("pending state lock poisoned"))?
                    .take()
                    .ok_or_else(|| internal("pending state already consumed"))?;
                f(this, db)
            })
            .await
        }
    }

    async fn transact_call_at(
        &self,
        request: RpcTxReq<Self::NetworkTypes>,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> Result<ResultAndState<HaltReasonFor<Self::Evm>>, Self::Error> {
        // Preparation already owns the request permit. Reacquiring here could
        // deadlock a server configured with one permit. spawn_blocking_io checks
        // cancellation before starting; its clone retains the permit until exit.
        let this = self.clone();
        self.spawn_with_call_at(request, at, overrides, move |db, env, tx| {
            this.transact(db, env, tx)
        })
        .await
    }
}

impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> EstimateCall for SimulationApi<N> {
    async fn estimate_gas_at(
        &self,
        request: RpcTxReq<Self::NetworkTypes>,
        at: BlockId,
        overrides: EvmOverrides,
    ) -> Result<U256, Self::Error> {
        let (env, at) = self.evm_env_at(at).await?;
        self.spawn_with_state_at_block(at, move |this, db| {
            this.estimate_gas_with(env, request, ExecutedPrefix(db), overrides)
        })
        .await
    }
}

impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> LoadBlock for SimulationApi<N> {}
impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> EthCall for SimulationApi<N> {}
impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> Trace for SimulationApi<N> {}
impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> LoadTransaction
    for SimulationApi<N>
{
}
impl<N: FullNodeComponents<Types = EezNode, Evm = EezEvmConfig>> GetBlockAccessList
    for SimulationApi<N>
{
}

/// Reth's estimator opens a fresh mutable cache on an EvmStateProvider. Adapt
/// the already executed prefix's read-only view, retaining zero/deleted slots.
struct ExecutedPrefix(StateCacheDb);
impl EvmStateProvider for ExecutedPrefix {
    fn basic_account(&self, address: &Address) -> Result<Option<Account>, ProviderError> {
        self.0
            .basic_ref(*address)
            .map(|info| info.map(Into::into))
            .map_err(ProviderError::other)
    }
    fn block_hash(&self, number: u64) -> Result<Option<B256>, ProviderError> {
        self.0
            .block_hash_ref(number)
            .map(Some)
            .map_err(ProviderError::other)
    }
    fn bytecode_by_hash(&self, hash: &B256) -> Result<Option<Bytecode>, ProviderError> {
        self.0
            .code_by_hash_ref(*hash)
            .map(|code| Some(Bytecode(code)))
            .map_err(ProviderError::other)
    }
    fn storage(&self, address: Address, key: B256) -> Result<Option<U256>, ProviderError> {
        self.0
            .storage_ref(address, key.into())
            .map(Some)
            .map_err(ProviderError::other)
    }
}
