//! Authenticated replay of lookup/static calls at Sync-block prefixes.
//!
//! These calls deliberately do not appear as canonical L2 transactions. The
//! Composer uses this module while generating the terminal execution witness,
//! and the proof signer uses the same module over that witness. Sharing the
//! exact EVM input and prefix schedule prevents simulation/replay drift.

use alloy_consensus::{TxLegacy, transaction::Recovered};

use alloy_primitives::{Address, Bytes, TxKind};
use alloy_sol_types::SolCall as _;
use eez_evm::EezEvmConfig;
use eez_evm::{EezEvmTx, EezEvmTypes};
use eez_primitives::Block;
use eez_protocol::abi::computeCrossChainProxyAddressCall;
use eez_protocol::settlement::{FifoObservationCallback, FifoObservationClaim};
use eez_protocol::{
    CallMode, EEZL2_AUTHORIZED_PROXIES_SLOT, ProxyInfo, RollupId, decode_proxy_value,
    proxy_mapping_key,
};
use evm2::block::{BlockState, BundleSource, BundleState};
use evm2::evm::inspector::Inspector;
use evm2::evm::{CacheDB, DynDatabase};
use evm2::interpreter::{
    GasTracker, Host, InstrStop, Interpreter, Message, MessageKind, MessageResult,
};
use evm2::{Evm, EvmFeatures};
use reth_evm::{BlockExecutionOutput, BlockExecutor, ConfigureEvm, EvmEnvFor};
use reth_primitives_traits::RecoveredBlock;

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

impl Inspector<EezEvmTypes> for ForceStaticInspector {
    fn initialize_interp(&mut self, interp: &mut Interpreter<'_, '_, EezEvmTypes>) {
        interp.set_static(true);
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
    fn expected_callback(
        &self,
        interp: &Interpreter<'_, '_, EezEvmTypes>,
        inputs: &Message<EezEvmTypes>,
        proxy: &ProxyInfo,
    ) -> Option<&'a FifoObservationCallback> {
        let callback = self.callbacks.get(self.answered)?;
        (inputs.is_static()
            && inputs.value.is_zero()
            && inputs.caller == callback.source_address
            && proxy.original_address == callback.target_address
            && proxy.original_rollup_id == self.source_rollup_id
            && inputs.input.as_bytes(interp.memory()) == callback.data.as_ref())
        .then_some(callback)
    }
}

/// The proxy registered at `candidate`, or `None` for an ordinary account.
fn authorized_proxy(
    host: &mut Evm<'_, EezEvmTypes>,
    manager_address: Address,
    candidate: Address,
) -> Result<Option<ProxyInfo>, ObservationInspectionFailure> {
    let failed = ObservationInspectionFailure::ProxyLookup;
    host.load_account(&manager_address, false, false)
        .map_err(|_| failed)?;
    let key = proxy_mapping_key(candidate, EEZL2_AUTHORIZED_PROXIES_SLOT);
    let load = host
        .sload(&manager_address, &key.into(), false)
        .map_err(|_| failed)?;
    Ok(decode_proxy_value(load.value))
}

impl Inspector<EezEvmTypes> for ObservationInspector<'_> {
    fn initialize_interp(&mut self, interp: &mut Interpreter<'_, '_, EezEvmTypes>) {
        if self.force_static {
            interp.set_static(true);
        }
    }
    fn call(
        &mut self,
        interp: &mut Interpreter<'_, '_, EezEvmTypes>,
        inputs: &mut Message<EezEvmTypes>,
    ) -> Option<MessageResult<EezEvmTypes>> {
        if self.failure.is_some() {
            return None;
        }
        // Delegate/callcode frames do not represent cross-chain proxy dispatches.
        if !matches!(inputs.kind, MessageKind::Call | MessageKind::StaticCall) {
            return None;
        }
        let proxy = match authorized_proxy(interp.host(), self.manager_address, inputs.destination)
        {
            Ok(proxy) => proxy?,
            Err(failure) => {
                self.failure = Some(failure);
                return None;
            }
        };
        let Some(callback) = self.expected_callback(interp, inputs, &proxy) else {
            self.failure = Some(ObservationInspectionFailure::CrossChainCall(
                inputs.destination,
            ));
            return None;
        };
        self.answered += 1;
        Some(MessageResult::<EezEvmTypes> {
            stop: if callback.success {
                InstrStop::Return
            } else {
                InstrStop::Revert
            },
            output: callback.output.clone(),
            gas: GasTracker::new(inputs.gas_limit),
            ..Default::default()
        })
    }
}

/// Capture the actual root target frame while the authenticated manager
/// executes and verifies the derived table. Manager completion and target
/// completion are separate: a validation revert is never a target outcome.
struct DeliveryObservationInspector<'a> {
    claim: &'a FifoObservationClaim,
    proxy: Address,
    frames: Vec<bool>,
    started: bool,
    outcome: Option<ObservedOutcome>,
}

impl Inspector<EezEvmTypes> for DeliveryObservationInspector<'_> {
    fn call(
        &mut self,
        interp: &mut Interpreter<'_, '_, EezEvmTypes>,
        inputs: &mut Message<EezEvmTypes>,
    ) -> Option<MessageResult<EezEvmTypes>> {
        let root = !self.started
            && inputs.kind == MessageKind::Call
            && inputs.caller == self.proxy
            && inputs.destination == self.claim.target_address
            && inputs.value == self.claim.value
            && inputs.input.as_bytes(interp.memory()) == self.claim.data.as_ref();
        self.started |= root;
        self.frames.push(root);
        None
    }
    fn call_end(
        &mut self,
        _: &mut Interpreter<'_, '_, EezEvmTypes>,
        _: &Message<EezEvmTypes>,
        outcome: &mut MessageResult<EezEvmTypes>,
    ) {
        if self.frames.pop() == Some(true) {
            self.outcome = Some(ObservedOutcome {
                success: outcome.is_success(),
                output: outcome.output.clone(),
            });
        }
    }
}

fn simulation_env(env: &EvmEnvFor<EezEvmConfig>) -> EvmEnvFor<EezEvmConfig> {
    let mut env = env.clone();
    env.version.features.remove(
        EvmFeatures::BASE_FEE_CHECK
            | EvmFeatures::BALANCE_CHECK
            | EvmFeatures::NONCE_CHECK
            | EvmFeatures::EIP3607
            | EvmFeatures::BLOCK_GAS_LIMIT_CHECK,
    );
    env.version.tx_gas_limit_cap = u64::MAX;
    env
}

/// Execute one observation in disposable state. A surrounding source rollback
/// scope retains mutable changes for its later observations; otherwise they are
/// discarded immediately. Database reads still reach the witness recorder.
fn execute_observation<DB>(
    state: &mut CacheDB<DB>,
    evm_config: &EezEvmConfig,
    evm_env: &EvmEnvFor<EezEvmConfig>,
    observation: &BlockObservation,
    retained: Option<&mut BundleState>,
) -> eyre::Result<ObservedOutcome>
where
    DB: DynDatabase,
{
    let claim = &observation.claim;
    let proxy_calldata = computeCrossChainProxyAddressCall {
        originalAddress: claim.source_address,
        originalRollupId: claim.source_rollup_id.0,
    }
    .abi_encode();
    let proxy_tx = TxLegacy {
        gas_limit: PROXY_VIEW_GAS_LIMIT,
        to: TxKind::Call(observation.manager_address),
        input: proxy_calldata.into(),
        chain_id: Some(evm_env.version.chain_id),
        ..Default::default()
    };
    let proxy_output = {
        let mut evm = evm_config.evm_with_env(&mut *state, simulation_env(evm_env));
        let tx = Recovered::new_unchecked(
            EezEvmTx::Ethereum(evm2::ethereum::TxEnvelope::Legacy(proxy_tx)),
            Address::ZERO,
        );
        let result = evm
            .transact_with_inspector(&tx, &mut ForceStaticInspector)
            .map_err(|error| eyre::eyre!("proxy-address observation failed: {error}"))?
            .result;
        eyre::ensure!(
            result.status,
            "proxy-address observation reverted: {:?}",
            result.output
        );
        result.output
    };
    let proxy = computeCrossChainProxyAddressCall::abi_decode_returns(&proxy_output)
        .map_err(|error| eyre::eyre!("decode observed proxy address: {error}"))?;

    if let Some(delivery) = &claim.delivery {
        eyre::ensure!(
            claim.call_mode == CallMode::Mutable && claim.callbacks.is_empty(),
            "mutable table observation has inconsistent mode or static callbacks"
        );
        let tx = TxLegacy {
            gas_limit: eez_primitives::SYSTEM_TX_GAS_CAP,
            to: TxKind::Call(observation.manager_address),
            input: delivery.calldata.clone(),
            value: delivery.value,
            chain_id: Some(evm_env.version.chain_id),
            ..Default::default()
        };
        let mut inspector = DeliveryObservationInspector {
            claim,
            proxy,
            frames: Vec::new(),
            started: false,
            outcome: None,
        };
        let mut evm = evm_config.evm_with_env(&mut *state, simulation_env(evm_env));
        let tx = Recovered::new_unchecked(
            EezEvmTx::Native {
                ty: eez_primitives::SYSTEM_TX_TYPE,
                call: tx,
                access_list: Default::default(),
            },
            eez_primitives::SYSTEM_ADDRESS,
        );
        let result = evm
            .transact_with_inspector(&tx, &mut inspector)
            .map_err(|error| eyre::eyre!("lookup table observation failed: {error}"))?;
        eyre::ensure!(
            result.result.status,
            "lookup table validation reverted: {:?}",
            result.result.output
        );
        let outcome = inspector
            .outcome
            .take()
            .ok_or_else(|| eyre::eyre!("lookup table did not execute its claimed target frame"))?;
        drop(evm);
        if let Some(retained) = retained {
            let mut transition = BlockState::new();
            for changes in result.pending_state.account_changes() {
                transition.commit_account_changes(changes, None);
            }
            retained.extend(transition.into_bundle());
            // A CREATE starts fresh storage even at an existing empty account.
            state.commit_pending(&result.pending_state);
        }
        return Ok(outcome);
    }
    eyre::ensure!(
        claim.call_mode == CallMode::Static,
        "mutable observation has no derived delivery table"
    );
    let target_tx = TxLegacy {
        gas_limit: OBSERVATION_CALL_GAS_LIMIT,
        to: TxKind::Call(claim.target_address),
        input: claim.data.clone(),
        value: claim.value,
        chain_id: Some(evm_env.version.chain_id),
        ..Default::default()
    };
    let mut evm = evm_config.evm_with_env(&mut *state, simulation_env(evm_env));
    let tx = Recovered::new_unchecked(
        EezEvmTx::Ethereum(evm2::ethereum::TxEnvelope::Legacy(target_tx)),
        proxy,
    );
    let mut inspector = ObservationInspector::new(observation.manager_address, claim);
    let result = evm
        .transact_with_inspector(&tx, &mut inspector)
        .map_err(|error| eyre::eyre!("target observation failed: {error}"))?
        .result;
    inspector.result()?;
    Ok(ObservedOutcome {
        success: result.status,
        output: result.output,
    })
}

/// Execute one canonical prefix's observations in a disposable overlay. Reads
/// reach the canonical database and its witness recorder; writes never do.
fn execute_prefix_observations<DB>(
    canonical: &mut DB,
    config: &EezEvmConfig,
    env: &EvmEnvFor<EezEvmConfig>,
    observations: &[BlockObservation],
) -> eyre::Result<Vec<ObservedOutcome>>
where
    DB: DynDatabase,
{
    let mut scratch = CacheDB::new(canonical);
    // Journal only retained writes. Parent reads reach the canonical recorder;
    // opening nested scopes never copies their accumulated cache.
    let mut retained = BundleState::default();
    let mut scopes = Vec::new();
    let mut outcomes = Vec::with_capacity(observations.len());
    for (index, observation) in observations.iter().enumerate() {
        for &span in &observation.claim.rollback_spans {
            let end = index
                .checked_add(span)
                .filter(|&end| span != 0 && end <= observations.len())
                .ok_or_else(|| {
                    eyre::eyre!("observation rollback scope exceeds its canonical prefix")
                })?;
            if let Some((outer_end, _)) = scopes.last() {
                eyre::ensure!(end <= *outer_end, "crossing observation rollback scopes");
            }
            scopes.push((end, retained.reverts.len()));
        }
        outcomes.push(execute_observation(
            &mut scratch,
            config,
            env,
            observation,
            (!scopes.is_empty()).then_some(&mut retained),
        )?);
        while scopes.last().is_some_and(|(end, _)| *end == index + 1) {
            let (_, checkpoint) = scopes.pop().expect("scope checked above");
            let transitions = retained
                .reverts
                .len()
                .checked_sub(checkpoint)
                .ok_or_else(|| {
                    eyre::eyre!("observation rollback checkpoint is ahead of its journal")
                })?;
            if transitions != 0 {
                retained.revert(transitions);
                scratch.cache = Default::default();
                // BundleSource does not wipe storage for CREATE alone. A restored
                // created account must not read slots from the canonical parent.
                for (address, account) in &retained.state {
                    if account.status.is_storage_known() {
                        scratch.cache.storage.entry(*address).or_default().wipe();
                    }
                }
                scratch.commit_source(&BundleSource(&retained));
            }
        }
    }
    eyre::ensure!(scopes.is_empty(), "unclosed observation rollback scope");
    Ok(outcomes)
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
/// synthetic calls use a disposable overlay and therefore cannot change the
/// block result. Nested source rollback scopes retain and restore overlay state
/// only within one canonical prefix.
///
/// # Errors
///
/// Returns an error for an invalid schedule, block-execution failure, missing
/// witness/database data, or an invalid proxy-address response.
pub fn replay_block_with_observations<DB>(
    state: &mut CacheDB<DB>,
    evm_config: &EezEvmConfig,
    block: &RecoveredBlock<Block>,
    observations: &[BlockObservation],
) -> eyre::Result<(
    BlockExecutionOutput<eez_primitives::Receipt>,
    Vec<ObservedOutcome>,
)>
where
    DB: DynDatabase,
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
    let mut outcomes = Vec::with_capacity(observations.len());
    let mut executor = evm_config
        .executor_for_block(&mut *state, block.sealed_block())
        .map_err(|error| eyre::eyre!("open block observation executor: {error:?}"))?;
    executor
        .apply_pre_execution_changes()
        .map_err(|error| eyre::eyre!("apply block pre-execution changes: {error}"))?;

    let end = observations.partition_point(|observation| observation.after_transactions == 0);
    outcomes.extend(execute_prefix_observations(
        executor.evm_mut().overlay_db_mut(),
        evm_config,
        &evm_env,
        &observations[..end],
    )?);
    let mut observation_cursor = end;

    for (transaction_index, transaction) in block.transactions_recovered().enumerate() {
        executor.execute_transaction(transaction).map_err(|error| {
            eyre::eyre!("execute block transaction {transaction_index}: {error}")
        })?;
        let boundary = transaction_index + 1;
        let end = observation_cursor
            + observations[observation_cursor..]
                .partition_point(|observation| observation.after_transactions == boundary);
        outcomes.extend(execute_prefix_observations(
            executor.evm_mut().overlay_db_mut(),
            evm_config,
            &evm_env,
            &observations[observation_cursor..end],
        )?);
        observation_cursor = end;
    }
    let output = executor
        .finish()
        .map_err(|error| eyre::eyre!("apply block post-execution changes: {error}"))?;
    eyre::ensure!(
        observation_cursor == observations.len(),
        "observation executor consumed {observation_cursor}/{} claims",
        observations.len(),
    );
    Ok((output, outcomes))
}
