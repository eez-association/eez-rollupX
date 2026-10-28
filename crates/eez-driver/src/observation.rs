//! Authenticated replay of lookup/static calls at Sync-block prefixes.
//!
//! These calls deliberately do not appear as canonical L2 transactions. The
//! Composer uses this module while generating the terminal execution witness,
//! and the proof signer uses the same module over that witness. Sharing the
//! exact EVM input and prefix schedule prevents simulation/replay drift.

use std::{error::Error, fmt::Debug};

use alloy_consensus::BlockHeader as _;
use alloy_primitives::{Address, Bytes, TxKind};
use alloy_sol_types::SolCall as _;
use eez_evm::EezEvmConfig;
use eez_primitives::Block;
use eez_protocol::abi::computeCrossChainProxyAddressCall;
use eez_protocol::settlement::{FifoObservationCallback, FifoObservationClaim};
use eez_protocol::{
    CallMode, EEZL2_AUTHORIZED_PROXIES_SLOT, ProxyInfo, RollupId, decode_proxy_value,
    proxy_mapping_key,
};
use reth_evm::{ConfigureEvm, Evm as _, EvmEnvFor, block::BlockExecutor};
use reth_primitives_traits::RecoveredBlock;
use revm::context_interface::{ContextTr, Host, JournalTr};
use revm::database::State;
use revm::interpreter::{
    CallInputs, CallOutcome, CallScheme, Gas, InstructionResult, Interpreter, InterpreterResult,
};
use revm::{Database, Inspector};

/// Gas cap shared with live Composer target simulation.
pub const OBSERVATION_CALL_GAS_LIMIT: u64 = 30_000_000;
const PROXY_VIEW_GAS_LIMIT: u64 = 1_000_000;

/// One observation assigned to a transaction boundary in the terminal block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockObservation {
    /// Number of canonical block transactions already applied at the pin.
    pub after_transactions: usize,
    /// Chain-local manager used to derive the destination proxy caller.
    pub manager_address: Address,
    /// Complete semantic input and claimed output.
    pub claim: FifoObservationClaim,
}

/// Independently replayed EVM outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservedOutcome {
    /// Whether the target frame returned successfully.
    pub success: bool,
    /// Exact return data on success or revert data on failure.
    pub output: Bytes,
}

#[derive(Clone, Copy, Debug, Default)]
struct ForceStaticInspector;

impl<CTX> Inspector<CTX> for ForceStaticInspector {
    fn initialize_interp(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        interp.runtime_flag.is_static = true;
    }
}

/// Inspector for an observation target frame.
///
/// Besides enforcing static execution when requested, it independently checks
/// every nested call against the chain-local `authorizedProxies` mapping. A
/// registered proxy denotes another cross-chain dispatch, whose result cannot
/// be authenticated from this rollup's witness alone. Mapping reads happen
/// through the live journal and become part of the generated/verified witness.
///
/// The only dispatches accepted are a static observation's claimed callbacks
/// to the source chain, in order and with identical caller, target and
/// calldata. Each is answered with its claimed output, without charging gas,
/// as live Composer simulation answers it; the source chain enforces that
/// output (see [`FifoObservationCallback`]). Any other dispatch, or a claimed
/// callback that is never made, rejects the observation after execution.
#[derive(Debug)]
struct ObservationInspector<'a> {
    manager_address: Address,
    force_static: bool,
    /// The chain that receives the callbacks.
    source_rollup_id: RollupId,
    callbacks: &'a [FifoObservationCallback],
    answered: usize,
    failure: Option<ObservationInspectionFailure>,
}

#[derive(Clone, Copy, Debug)]
enum ObservationInspectionFailure {
    ProxyLookup,
    CrossChainCall(Address),
}

impl<'a> ObservationInspector<'a> {
    fn new(manager_address: Address, claim: &'a FifoObservationClaim) -> Self {
        Self {
            manager_address,
            force_static: claim.call_mode == CallMode::Static,
            source_rollup_id: claim.source_rollup_id,
            callbacks: &claim.callbacks,
            answered: 0,
            failure: None,
        }
    }

    fn result(&self) -> eyre::Result<()> {
        match self.failure {
            None => {}
            Some(ObservationInspectionFailure::ProxyLookup) => {
                eyre::bail!("authorized-proxy observation lookup failed")
            }
            Some(ObservationInspectionFailure::CrossChainCall(proxy)) => eyre::bail!(
                "observation attempted unsupported nested cross-chain call through proxy {proxy}"
            ),
        }
        eyre::ensure!(
            self.answered == self.callbacks.len(),
            "observation made {} of {} claimed static callbacks",
            self.answered,
            self.callbacks.len(),
        );
        Ok(())
    }

    /// The next claimed callback, if this proxy dispatch is exactly it.
    fn expected_callback<CTX: ContextTr>(
        &self,
        context: &mut CTX,
        inputs: &CallInputs,
        proxy: &ProxyInfo,
    ) -> Option<&'a FifoObservationCallback> {
        let callback = self.callbacks.get(self.answered)?;
        (inputs.is_static
            && inputs.value.get().is_zero()
            && inputs.caller == callback.source_address
            && proxy.original_address == callback.target_address
            && proxy.original_rollup_id == self.source_rollup_id
            && inputs.input.bytes(context) == callback.data)
            .then_some(callback)
    }
}

/// The proxy registered at `candidate`, or `None` for an ordinary account.
fn authorized_proxy<CTX>(
    context: &mut CTX,
    manager_address: Address,
    candidate: Address,
) -> Result<Option<ProxyInfo>, ObservationInspectionFailure>
where
    CTX: ContextTr + Host,
{
    let failed = ObservationInspectionFailure::ProxyLookup;
    context
        .journal_mut()
        .load_account(manager_address)
        .map_err(|_| failed)?;
    let key = proxy_mapping_key(candidate, EEZL2_AUTHORIZED_PROXIES_SLOT);
    let load = context.sload(manager_address, key.into()).ok_or(failed)?;
    Ok(decode_proxy_value(load.data))
}

impl<CTX> Inspector<CTX> for ObservationInspector<'_>
where
    CTX: ContextTr + Host,
{
    fn initialize_interp(&mut self, interp: &mut Interpreter, _context: &mut CTX) {
        if self.force_static {
            interp.runtime_flag.is_static = true;
        }
    }

    fn call(&mut self, context: &mut CTX, inputs: &mut CallInputs) -> Option<CallOutcome> {
        if self.failure.is_some() {
            return None;
        }
        // Match the protocol inspector: delegate/callcode frames are local
        // EVM implementation details and never represent a proxy dispatch.
        if !matches!(inputs.scheme, CallScheme::Call | CallScheme::StaticCall) {
            return None;
        }
        let proxy = match authorized_proxy(context, self.manager_address, inputs.target_address) {
            Ok(proxy) => proxy?,
            Err(failure) => {
                self.failure = Some(failure);
                return None;
            }
        };
        let Some(callback) = self.expected_callback(context, inputs, &proxy) else {
            self.failure = Some(ObservationInspectionFailure::CrossChainCall(
                inputs.target_address,
            ));
            return None;
        };
        self.answered += 1;
        Some(CallOutcome::new(
            InterpreterResult::new(
                InstructionResult::Return,
                callback.output.clone(),
                Gas::new(inputs.gas_limit),
            ),
            inputs.return_memory_offset.clone(),
        ))
    }
}

fn simulation_env(env: &EvmEnvFor<EezEvmConfig>) -> EvmEnvFor<EezEvmConfig> {
    let mut env = env.clone();
    env.cfg_env.disable_base_fee = true;
    env.cfg_env.disable_balance_check = true;
    env.cfg_env.disable_nonce_check = true;
    env.cfg_env.disable_eip3607 = true;
    env.cfg_env.disable_block_gas_limit = true;
    env.cfg_env.tx_gas_limit_cap = Some(u64::MAX);
    env
}

/// Execute one observation without committing its synthetic transaction state.
/// Database reads still warm `state`, which is what witness generation needs.
fn execute_observation<DB>(
    state: &mut State<DB>,
    evm_config: &EezEvmConfig,
    evm_env: &EvmEnvFor<EezEvmConfig>,
    observation: &BlockObservation,
) -> eyre::Result<ObservedOutcome>
where
    DB: Database + Debug,
    DB::Error: Error + Send + Sync + 'static,
{
    let claim = &observation.claim;
    let proxy_calldata = computeCrossChainProxyAddressCall {
        originalAddress: claim.source_address,
        originalRollupId: claim.source_rollup_id.0,
    }
    .abi_encode();
    let proxy_tx = revm::context::TxEnv {
        caller: Address::ZERO,
        gas_limit: PROXY_VIEW_GAS_LIMIT,
        kind: TxKind::Call(observation.manager_address),
        data: proxy_calldata.into(),
        chain_id: Some(evm_env.cfg_env.chain_id),
        ..Default::default()
    };
    let proxy_output = {
        let mut evm = evm_config.evm_with_env_and_inspector(
            &mut *state,
            simulation_env(evm_env),
            ForceStaticInspector,
        );
        let result = evm
            .transact(proxy_tx)
            .map_err(|error| eyre::eyre!("proxy-address observation failed: {error}"))?;
        eyre::ensure!(
            result.result.is_success(),
            "proxy-address observation reverted: {:?}",
            result.result.output(),
        );
        result.result.output().cloned().unwrap_or_default()
    };
    let proxy = computeCrossChainProxyAddressCall::abi_decode_returns(&proxy_output)
        .map_err(|error| eyre::eyre!("decode observed proxy address: {error}"))?;

    let target_tx = revm::context::TxEnv {
        caller: proxy,
        gas_limit: OBSERVATION_CALL_GAS_LIMIT,
        kind: TxKind::Call(claim.target_address),
        data: claim.data.clone(),
        value: claim.value,
        chain_id: Some(evm_env.cfg_env.chain_id),
        ..Default::default()
    };
    let mut evm = evm_config.evm_with_env_and_inspector(
        &mut *state,
        simulation_env(evm_env),
        ObservationInspector::new(observation.manager_address, claim),
    );
    let result = evm
        .transact(target_tx)
        .map_err(|error| eyre::eyre!("target observation failed: {error}"))?;
    evm.inspector_mut().result()?;
    Ok(ObservedOutcome {
        success: result.result.is_success(),
        output: result.result.output().cloned().unwrap_or_default(),
    })
}

/// Replay an exact block and execute observations at their in-block prefix.
///
/// Observations run before block-finalization cleanup, even at the final
/// transaction boundary. Their pin authenticates the closed prefix's body and
/// replay from its parent, not an `eth_call` on its post-finalization state root.
/// This preserves ordinary donations visible to the next canonical transaction.
///
/// The observation list must be nondecreasing by `after_transactions` and no
/// position may exceed the block body. Canonical transactions commit normally;
/// synthetic calls never commit and therefore cannot change the block result.
///
/// # Errors
///
/// Returns an error for an invalid schedule, block-execution failure, missing
/// witness/database data, or an invalid proxy-address response.
pub fn replay_block_with_observations<DB>(
    state: &mut State<DB>,
    evm_config: &EezEvmConfig,
    block: &RecoveredBlock<Block>,
    observations: &[BlockObservation],
) -> eyre::Result<Vec<ObservedOutcome>>
where
    DB: Database + Debug,
    DB::Error: Error + Send + Sync + 'static,
{
    let transaction_count = block.body().transactions.len();
    let mut previous = 0usize;
    for (index, observation) in observations.iter().enumerate() {
        eyre::ensure!(
            observation.after_transactions <= transaction_count,
            "observation {index} targets prefix {} of a {transaction_count}-transaction block",
            observation.after_transactions,
        );
        eyre::ensure!(
            index == 0 || observation.after_transactions >= previous,
            "observation schedule is not FIFO at index {index}",
        );
        previous = observation.after_transactions;
    }

    let evm_env = evm_config
        .evm_env(block.header())
        .map_err(|error| eyre::eyre!("build observation EVM environment: {error}"))?;
    let has_bal = block.header().block_access_list_hash().is_some();
    let mut outcomes = Vec::with_capacity(observations.len());
    let mut observation_cursor = 0usize;
    let mut executor = evm_config
        .executor_for_block(&mut *state, block.sealed_block())
        .map_err(|error| eyre::eyre!("open block observation executor: {error:?}"))?;
    executor
        .apply_pre_execution_changes()
        .map_err(|error| eyre::eyre!("apply block pre-execution changes: {error}"))?;
    if has_bal {
        executor.evm_mut().db_mut().bump_bal_index();
    }

    while observations
        .get(observation_cursor)
        .is_some_and(|observation| observation.after_transactions == 0)
    {
        outcomes.push(execute_observation(
            executor.evm_mut().db_mut(),
            evm_config,
            &evm_env,
            &observations[observation_cursor],
        )?);
        observation_cursor += 1;
    }

    for (transaction_index, transaction) in block.transactions_recovered().enumerate() {
        executor.execute_transaction(transaction).map_err(|error| {
            eyre::eyre!("execute block transaction {transaction_index}: {error}")
        })?;
        if has_bal {
            executor.evm_mut().db_mut().bump_bal_index();
        }
        let boundary = transaction_index + 1;
        while observations
            .get(observation_cursor)
            .is_some_and(|observation| observation.after_transactions == boundary)
        {
            outcomes.push(execute_observation(
                executor.evm_mut().db_mut(),
                evm_config,
                &evm_env,
                &observations[observation_cursor],
            )?);
            observation_cursor += 1;
        }
    }
    executor
        .apply_post_execution_changes()
        .map_err(|error| eyre::eyre!("apply block post-execution changes: {error}"))?;
    eyre::ensure!(
        observation_cursor == observations.len(),
        "observation executor consumed {observation_cursor}/{} claims",
        observations.len(),
    );
    Ok(outcomes)
}
