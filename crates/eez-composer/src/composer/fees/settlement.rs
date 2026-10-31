//! Receipt-cost estimation for outbound settlement. No signed transaction or
//! proof leaves this module: all roots, blobs and proof acceptance are private
//! estimation inputs. Actual dispatch still uses independent proof validation
//! and `PostingRuntime::finalize` on the complete signed bundle.

use alloy_primitives::{Address, B256, Bytes, I256, TxKind, U256, b256};
use alloy_sol_types::{SolCall, SolEvent};
use eez_protocol::{
    EvmBatch,
    abi::{
        EtherDeltaSol, ExecutionEntrySol, RollupIdWithProofSystemsSol, RootUpdateSol,
        postAndVerifyBatchCall, rollupsCall,
    },
    blob_semantics::SemanticTransaction,
    entries::EntryTrace,
    system_tx::{OutboundSourceEntry, OutboundSyncGroup},
};
use reth_evm::{ConfigureEvm, Evm as _, execute::BlockExecutor as _};
use reth_primitives_traits::SealedHeader;
use reth_revm::{database::StateProviderDatabase, db::State};
use reth_storage_api::EvmStateProviderAdapter;
use revm::{
    Database, Inspector, Journal,
    context::TxEnv,
    context_interface::ContextTr,
    interpreter::{CallInputs, CallOutcome, CallScheme},
};

use super::super::posting::{EffectInspector, PostingQuote, PostingRuntime};
use super::quote::FeeQuoteError;

alloy_sol_types::sol! {
    function verify(bytes proof, bytes32 publicInputsHash) external view returns (bool);
    function checkProofSystemsAndGetVkeys(address[] proofSystems) external view returns (bytes32[]);
}

pub(in crate::composer) struct SettlementInput<'a> {
    pub rollup_id: u64,
    pub settled_root: B256,
    pub entries: &'a [ExecutionEntrySol],
    pub traces: &'a [EntryTrace],
    pub groups: &'a [OutboundSyncGroup],
    pub semantics: &'a [SemanticTransaction],
    pub blobs: usize,
}

fn unavailable(error: impl std::fmt::Display) -> FeeQuoteError {
    tracing::debug!(%error, "L1 settlement fee simulation unavailable");
    FeeQuoteError::Unavailable("L1 settlement execution")
}

/// Quote and cumulative signed admission use identical receipt-cost arithmetic.
/// Gas-limit headroom is used only to choose an executable transaction limit.
pub(in crate::composer) fn measured_payment(
    runtime: &PostingRuntime,
    posting: &PostingQuote,
    input: SettlementInput<'_>,
) -> Result<
    (
        u64,
        eez_l1::posting_fees::PostingCost,
        super::pricing::RequiredPayment,
    ),
    FeeQuoteError,
> {
    Estimator::new(runtime, posting).measured_payment(input)
}

/// Bound to one immutable posting context and one source envelope's pricing
/// loop. Source/native execution always reruns; only identical successful L1
/// replays can share a gas measurement. Never share this across quote requests.
pub(in crate::composer) struct Estimator<'a> {
    runtime: &'a PostingRuntime,
    posting: &'a PostingQuote,
    preparation: PreparationCache,
    cache: ReplayCache,
}

impl<'a> Estimator<'a> {
    pub fn new(runtime: &'a PostingRuntime, posting: &'a PostingQuote) -> Self {
        Self {
            runtime,
            posting,
            preparation: PreparationCache::default(),
            cache: ReplayCache::default(),
        }
    }

    pub fn reuses(&self) -> usize {
        self.cache.reuses
    }

    pub fn measured_payment(
        &mut self,
        input: SettlementInput<'_>,
    ) -> Result<
        (
            u64,
            eez_l1::posting_fees::PostingCost,
            super::pricing::RequiredPayment,
        ),
        FeeQuoteError,
    > {
        let snapshot = self
            .preparation
            .get_or_load(input.rollup_id, input.settled_root, || {
                prepare_snapshot(
                    self.runtime,
                    self.posting,
                    input.rollup_id,
                    input.settled_root,
                )
            })?;
        let (prepared, funding) = prepare(self.runtime, self.posting, snapshot, &input)?;
        let gas = self.cache.measure(prepared, |prepared| {
            estimate(self.runtime, self.posting, prepared, &funding)
        })?;
        let cost = self
            .posting
            .quote
            .estimate(gas, input.blobs as u64)
            .map_err(unavailable)?;
        let payment = self
            .runtime
            .collection
            .pricing
            .as_ref()
            .ok_or(FeeQuoteError::Disabled)?
            .required_payment(cost.total)
            .map_err(FeeQuoteError::Limit)?;
        Ok((gas, cost, payment))
    }
}

/// Validated deployment and account reads at the estimator's pinned L1 state.
/// No simulated state, blob reservation or execution outcome belongs here.
struct SnapshotPreparation {
    vkey: B256,
    poster_nonce: u64,
    poster_balance: U256,
}

#[derive(Default)]
struct PreparationCache {
    previous: Option<((u64, B256), SnapshotPreparation)>,
}

impl PreparationCache {
    fn get_or_load(
        &mut self,
        rollup_id: u64,
        settled_root: B256,
        load: impl FnOnce() -> Result<SnapshotPreparation, FeeQuoteError>,
    ) -> Result<&SnapshotPreparation, FeeQuoteError> {
        // The owning estimator fixes the state hash, target and runtime. A
        // different rollup or expected root must still pass deployment/root
        // validation, even if a previous pricing iteration loaded successfully.
        let key = (rollup_id, settled_root);
        if self
            .previous
            .as_ref()
            .is_none_or(|(previous, _)| *previous != key)
        {
            self.previous = Some((key, load()?));
        }
        // Both paths above establish a successful preparation for this key.
        Ok(&self
            .previous
            .as_ref()
            .expect("matching preparation loaded")
            .1)
    }
}

/// Complete variable inputs consumed by the L1 probe and its validation. The
/// enclosing Estimator pins state, block environment, gas policy and deployment.
/// Source fee bytes are absent only because L1 executes this prepared post, not
/// the L2 envelope. Blob count/fees and expected nested results remain bound.
#[derive(Clone, PartialEq, Eq)]
struct PreparedReplay {
    tx: TxEnv,
    verify_data: Bytes,
    endpoint: B256,
    rollup_id: u64,
    effects: EffectInspector,
}

#[derive(Default)]
struct ReplayCache {
    previous: Option<(PreparedReplay, u64)>,
    reuses: usize,
}

impl ReplayCache {
    fn measure(
        &mut self,
        prepared: PreparedReplay,
        execute: impl FnOnce(&PreparedReplay) -> Result<u64, FeeQuoteError>,
    ) -> Result<u64, FeeQuoteError> {
        if let Some((previous, gas)) = &self.previous
            && previous == &prepared
        {
            self.reuses += 1;
            return Ok(*gas);
        }
        let gas = execute(&prepared)?;
        self.previous = Some((prepared, gas));
        Ok(gas)
    }
}

/// Full-width hashes bound calldata pricing without pretending to know the
/// unsigned source's final block hash or KZG commitments. Distinct roots keep
/// the real nonzero-to-nonzero storage transition and carrier ordering.
fn placeholder(index: usize) -> Result<B256, FeeQuoteError> {
    let mut hash = B256::repeat_byte(0x11);
    hash[31] = u8::try_from(index + 1).map_err(|_| FeeQuoteError::Limit("settlement entries"))?;
    Ok(hash)
}

fn batch(runtime: &PostingRuntime, input: &SettlementInput<'_>) -> Result<EvmBatch, FeeQuoteError> {
    if input.entries.is_empty() || input.entries.len() != input.traces.len() {
        return Err(FeeQuoteError::Unsupported("settlement trace coverage"));
    }
    let counts = eez_protocol::system_tx::outbound_settlement_source_counts(
        input.entries,
        input.groups.iter().map(OutboundSyncGroup::source_shape),
    )
    .map_err(unavailable)?;
    let effects = input
        .groups
        .iter()
        .flat_map(|g| &g.effects)
        .collect::<Vec<_>>();
    let prefix = placeholder(0)?;
    let checkpoints = (0..input.entries.len())
        .map(|i| placeholder(i + 1))
        .collect::<Result<Vec<_>, _>>()?;
    let (anchor, carriers) = eez_protocol::settlement::settlement_root_carriers(
        prefix,
        checkpoints
            .iter()
            .copied()
            .zip(input.entries.iter().map(|e| e.success)),
    );
    let mut result = EvmBatch::default();
    result.entries.push(ExecutionEntrySol {
        rootUpdates: vec![RootUpdateSol {
            rollupId: input.rollup_id,
            currentRoot: input.settled_root,
            newRoot: anchor,
            etherDelta: EtherDeltaSol::ZERO,
        }],
        destinationRollupId: input.rollup_id,
        success: true,
        ..Default::default()
    });
    let mut live = anchor;
    let mut cursor = 0;
    for ((entry, count), new_root) in input.entries.iter().zip(counts).zip(carriers) {
        let outflow = eez_protocol::entries::entry_flat_value_sum(entry)
            .ok_or(FeeQuoteError::Unsupported("settlement value shape"))?;
        let inflow = effects[cursor..cursor + count]
            .iter()
            .try_fold(U256::ZERO, |total, effect| {
                let source = match &effect.source_entry {
                    OutboundSourceEntry::Mutable(entry) => {
                        eez_protocol::entries::entry_flat_value_sum(entry)
                    }
                    OutboundSourceEntry::Static(_) => Some(U256::ZERO),
                }?;
                total.checked_add(source)
            })
            .ok_or(FeeQuoteError::Unsupported("settlement inflow shape"))?;
        let delta = I256::try_from(inflow)
            .ok()
            .and_then(|inflow| {
                I256::try_from(outflow)
                    .ok()
                    .and_then(|outflow| inflow.checked_sub(outflow))
            })
            .filter(|delta| {
                *delta >= I256::from(EtherDeltaSol::MIN) && *delta <= I256::from(EtherDeltaSol::MAX)
            })
            .ok_or(FeeQuoteError::Unsupported("settlement value overflow"))?;
        let mut entry = entry.clone();
        entry.destinationRollupId = input.rollup_id;
        entry.rootUpdates = vec![RootUpdateSol {
            rollupId: input.rollup_id,
            currentRoot: live,
            newRoot: new_root,
            etherDelta: EtherDeltaSol::from(delta),
        }];
        result.entries.push(entry);
        live = new_root;
        cursor += count;
    }
    let mut traces = vec![EntryTrace::default()];
    traces.extend_from_slice(input.traces);
    eez_protocol::entries::finalize_l1_entry_traces(&mut result, &traces).map_err(unavailable)?;
    result.immediateEntryCount = U256::from(result.entries.len());
    result.blockNumber = u64::MAX;
    result.expectedRootPerRollup = vec![eez_protocol::abi::ExpectedRootPerRollupSol {
        rollupId: input.rollup_id,
        root: input.settled_root,
    }];
    result.proofSystems = vec![runtime.ctx.ecdsa_proof_system_address];
    result.rollupIdsWithProofSystems = vec![RollupIdWithProofSystemsSol {
        rollupId: input.rollup_id,
        proofSystemIndexes: vec![0],
    }];
    result.blobIndices = (0..input.blobs).map(U256::from).collect();
    result.proofs = vec![placeholder_proof()];
    Ok(result)
}

/// Full-width valid curve input executes the verifier's real recovery path.
pub(in crate::composer) fn placeholder_proof() -> Bytes {
    let mut proof =
        b256!("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798").to_vec();
    proof.extend([0x11; 32]);
    proof.push(27);
    proof.into()
}

fn read_call<C: SolCall>(
    runtime: &PostingRuntime,
    posting: &PostingQuote,
    to: Address,
    call: C,
) -> Result<C::Return, FeeQuoteError> {
    let provider = runtime
        .chain
        .provider
        .state_by_block_hash(posting.quote.sample.hash)
        .map_err(unavailable)?;
    let mut state = State::builder()
        .with_database(StateProviderDatabase::new(EvmStateProviderAdapter(
            provider,
        )))
        .build();
    let mut env = posting.env.clone();
    env.cfg_env.disable_nonce_check = true;
    env.cfg_env.disable_balance_check = true;
    env.cfg_env.disable_base_fee = true;
    env.cfg_env.disable_fee_charge = true;
    let mut evm = runtime.chain.evm_config.evm_with_env(&mut state, env);
    let result = evm
        .transact(TxEnv {
            kind: TxKind::Call(to),
            data: call.abi_encode().into(),
            gas_limit: 1_000_000,
            chain_id: Some(runtime.ctx.l1_chain_id),
            ..Default::default()
        })
        .map_err(unavailable)?;
    if !result.result.is_success() {
        return Err(FeeQuoteError::Unsupported("settlement deployment lookup"));
    }
    C::abi_decode_returns(
        result
            .result
            .output()
            .ok_or(FeeQuoteError::Unavailable("deployment output"))?,
    )
    .map_err(unavailable)
}

fn prepare_snapshot(
    runtime: &PostingRuntime,
    posting: &PostingQuote,
    rollup_id: u64,
    settled_root: B256,
) -> Result<SnapshotPreparation, FeeQuoteError> {
    let registry = read_call(
        runtime,
        posting,
        runtime.registry,
        rollupsCall {
            rollupId: rollup_id,
        },
    )?;
    if registry.root != settled_root {
        return Err(FeeQuoteError::Stale);
    }
    let provider = runtime
        .chain
        .provider
        .state_by_block_hash(posting.quote.sample.hash)
        .map_err(unavailable)?;
    super::super::estimate::deployment::validate_at(
        provider.as_ref(),
        runtime.registry,
        registry.rollupContract,
        runtime.ctx.ecdsa_proof_system_address,
        runtime.post_batch_target,
    )
    .map_err(unavailable)?;
    let keys = read_call(
        runtime,
        posting,
        registry.rollupContract,
        checkProofSystemsAndGetVkeysCall {
            proofSystems: vec![runtime.ctx.ecdsa_proof_system_address],
        },
    )?;
    let [vkey] = keys.as_slice() else {
        return Err(FeeQuoteError::Unsupported("settlement proof profile"));
    };
    let account = provider
        .basic_account(&runtime.ctx.l1_poster_signer.address())
        .map_err(unavailable)?
        .unwrap_or_default();
    Ok(SnapshotPreparation {
        vkey: *vkey,
        poster_nonce: account.nonce,
        poster_balance: account.balance,
    })
}

fn prepare(
    runtime: &PostingRuntime,
    posting: &PostingQuote,
    snapshot: &SnapshotPreparation,
    input: &SettlementInput<'_>,
) -> Result<(PreparedReplay, crate::PosterBalanceError), FeeQuoteError> {
    let batch = batch(runtime, input)?;
    let hashes = (0..input.blobs)
        .map(|i| {
            let mut hash = placeholder(i)?;
            hash[0] = 1;
            Ok(hash)
        })
        .collect::<Result<Vec<_>, FeeQuoteError>>()?;
    let context = eez_protocol::l1_history::ProofContext::new(
        U256::from(runtime.ctx.l2_chain_id),
        input.rollup_id,
        eez_protocol::l1_history::EvidenceAnchor::ActualParent {
            number: posting.parent.number,
            hash: posting.quote.sample.hash,
            target_timestamp: posting.attributes.timestamp,
        },
        posting.parent.timestamp,
    )
    .map_err(unavailable)?;
    let digests = eez_protocol::public_inputs::public_inputs_hashes_with_blob_hashes(
        &batch,
        snapshot.vkey,
        &hashes,
        context,
    )
    .map_err(unavailable)?;
    let [digest] = digests.as_slice() else {
        return Err(FeeQuoteError::Unsupported("settlement proof digest"));
    };
    let verify_data: Bytes = verifyCall {
        proof: batch.proofs[0].clone(),
        publicInputsHash: *digest,
    }
    .abi_encode()
    .into();
    let poster = runtime.ctx.l1_poster_signer.address();
    // Only the balance is immutable. A fee-dependent call can change the blob
    // count, so reserve blob fees and derive affordable gas for every candidate.
    let funding = posting
        .probe_funding(poster, snapshot.poster_balance, input.blobs as u64)
        .map_err(unavailable)?;
    if funding.affordable_gas < 21_000 {
        return Err(FeeQuoteError::PosterFunds(Box::new(funding)));
    }
    let tx = TxEnv {
        tx_type: 3,
        caller: poster,
        kind: TxKind::Call(runtime.post_batch_target),
        data: postAndVerifyBatchCall {
            batch: batch.clone(),
        }
        .abi_encode()
        .into(),
        nonce: snapshot.poster_nonce,
        gas_limit: funding.affordable_gas,
        gas_price: posting.quote.max_fee_per_gas,
        gas_priority_fee: Some(posting.quote.max_priority_fee_per_gas),
        max_fee_per_blob_gas: posting.quote.max_fee_per_blob_gas,
        blob_hashes: hashes,
        chain_id: Some(runtime.ctx.l1_chain_id),
        ..Default::default()
    };
    let endpoint = batch
        .entries
        .last()
        .and_then(|entry| entry.rootUpdates.last())
        .ok_or(FeeQuoteError::Unsupported("settlement endpoint"))?
        .newRoot;
    let effects = EffectInspector::new(runtime.registry, input.rollup_id, input.semantics)
        .map_err(unavailable)?;
    Ok((
        PreparedReplay {
            tx,
            verify_data,
            endpoint,
            rollup_id: input.rollup_id,
            effects,
        },
        funding,
    ))
}

fn estimate(
    runtime: &PostingRuntime,
    posting: &PostingQuote,
    prepared: &PreparedReplay,
    funding: &crate::PosterBalanceError,
) -> Result<u64, FeeQuoteError> {
    let execute = |tx| replay(runtime, posting, prepared, tx, funding);
    let (used, required) = execute(prepared.tx.clone())?;
    let mut selected = prepared.tx.clone();
    selected.gas_limit = runtime
        .fees
        .declared_gas(required, funding.affordable_gas)
        .map_err(|error| {
            if funding.limits_probe() {
                FeeQuoteError::PosterFunds(Box::new(funding.clone()))
            } else {
                unavailable(error)
            }
        })?;
    if selected.gas_limit == funding.affordable_gas {
        return Ok(used);
    }
    // As in final preflight, gas-sensitive code may need the larger verified
    // limit. Only a successful replay of all intended effects establishes cost.
    match execute(selected) {
        Ok((gas, _)) => Ok(gas),
        Err(FeeQuoteError::Unsupported(_)) => Ok(used),
        Err(error) => Err(error),
    }
}

fn replay(
    runtime: &PostingRuntime,
    posting: &PostingQuote,
    prepared: &PreparedReplay,
    tx: TxEnv,
    funding: &crate::PosterBalanceError,
) -> Result<(u64, u64), FeeQuoteError> {
    let gas_limit = tx.gas_limit;
    let provider = runtime
        .chain
        .provider
        .state_by_block_hash(posting.quote.sample.hash)
        .map_err(unavailable)?;
    let mut state = State::builder()
        .with_database(StateProviderDatabase::new(EvmStateProviderAdapter(
            provider,
        )))
        .with_bundle_update()
        .build();
    let mut inspector = MeteredProof {
        registry: runtime.registry,
        verifier: runtime.ctx.ecdsa_proof_system_address,
        calldata: prepared.verify_data.clone(),
        matches: 0,
        completed: 0,
        invalid: false,
        effects: prepared.effects.clone(),
    };
    macro_rules! execute {
        ($config:expr, $parent:expr) => {{
            let config = $config;
            let context = config
                .context_for_next_block(&$parent, posting.attributes.clone())
                .map_err(unavailable)?;
            let evm = config.evm_with_env(&mut state, posting.env.clone());
            config
                .create_executor(evm, context)
                .apply_pre_execution_changes()
                .map_err(unavailable)?;
            let traced = crate::composer::quote_trace::inspect(
                &mut inspector,
                posting.env.cfg_env.chain_id,
                "settlement",
                &tx,
            );
            let mut evm =
                config.evm_with_env_and_inspector(&mut state, posting.env.clone(), traced);
            evm.transact(tx)
                .map_err(|error| {
                    if funding.explains(gas_limit, &error) {
                        FeeQuoteError::PosterFunds(Box::new(funding.clone()))
                    } else {
                        unavailable(error)
                    }
                })?
                .result
        }};
    }
    let result = if let Some(config) = &runtime.chain.gnosis_evm_config {
        execute!(
            config,
            SealedHeader::new(posting.parent.clone().into(), posting.quote.sample.hash)
        )
    } else {
        let config =
            reth_evm_ethereum::EthEvmConfig::new(runtime.chain.evm_config.chain_spec().clone());
        execute!(
            &config,
            SealedHeader::new(posting.parent.clone(), posting.quote.sample.hash)
        )
    };
    if funding.limits_probe()
        && gas_limit == funding.affordable_gas
        && matches!(
            result,
            revm::context_interface::result::ExecutionResult::Halt {
                reason: revm::context_interface::result::HaltReason::OutOfGas(_),
                ..
            }
        )
    {
        return Err(FeeQuoteError::PosterFunds(Box::new(funding.clone())));
    }
    if !result.is_success()
        || inspector.invalid
        || inspector.matches != 1
        || inspector.completed != 1
    {
        tracing::debug!(
            ?result,
            matches = inspector.matches,
            completed = inspector.completed,
            "conditional settlement failed"
        );
        return Err(FeeQuoteError::Unsupported("conditional settlement failed"));
    }
    let mut last = None;
    for log in result
        .logs()
        .iter()
        .filter(|log| log.address == runtime.registry)
    {
        if log.data.topics().first() == Some(&eez_protocol::abi::L2TxSkipped::SIGNATURE_HASH) {
            return Err(FeeQuoteError::Unsupported("settlement skipped an entry"));
        }
        if log.data.topics().first()
            == Some(&eez_protocol::abi::L2ExecutionPerformed::SIGNATURE_HASH)
        {
            let event =
                eez_protocol::abi::L2ExecutionPerformed::decode_log(log).map_err(unavailable)?;
            if event.rollupId == prepared.rollup_id {
                last = Some(event.newRoot);
            }
        }
    }
    if last != Some(prepared.endpoint) {
        return Err(FeeQuoteError::Unsupported("settlement endpoint mismatch"));
    }
    inspector.effects.verify().map_err(|error| {
        tracing::debug!(%error, "settlement fee effects mismatch");
        FeeQuoteError::Unsupported("settlement effects mismatch")
    })?;
    let gas = result.gas();
    Ok((
        gas.tx_gas_used(),
        gas.total_gas_spent().max(gas.floor_gas()),
    ))
}

/// Execute the actual verifier bytecode and preserve all its gas consumption.
/// Only its final boolean is assumed valid, scoped to one exact registry call.
/// This inspector is never installed on signed preflight or canonical execution.
pub(in crate::composer) struct MeteredProof {
    registry: Address,
    verifier: Address,
    calldata: Bytes,
    matches: usize,
    completed: usize,
    invalid: bool,
    effects: EffectInspector,
}

impl MeteredProof {
    pub(in crate::composer) fn setup(
        registry: Address,
        verifier: Address,
        calldata: Bytes,
        rollup_id: u64,
    ) -> Result<Self, String> {
        Ok(Self {
            registry,
            verifier,
            calldata,
            matches: 0,
            completed: 0,
            invalid: false,
            effects: EffectInspector::new(registry, rollup_id, &[]).map_err(|e| e.to_string())?,
        })
    }

    pub(in crate::composer) fn valid(&self) -> bool {
        !self.invalid && self.matches == 1 && self.completed == 1
    }

    fn matches<DB: Database, C: ContextTr<Db = DB, Journal = Journal<DB>>>(
        &self,
        context: &C,
        inputs: &CallInputs,
    ) -> bool {
        inputs.scheme == CallScheme::StaticCall
            && inputs.is_static
            && inputs.caller == self.registry
            && inputs.target_address == self.verifier
            && inputs.bytecode_address == self.verifier
            && &*inputs.input.as_bytes(context) == self.calldata.as_ref()
    }
}

impl<DB: Database, C: ContextTr<Db = DB, Journal = Journal<DB>>> Inspector<C> for MeteredProof {
    fn call(&mut self, context: &mut C, inputs: &mut CallInputs) -> Option<CallOutcome> {
        if self.matches(context, inputs) {
            self.matches += 1;
        }
        self.effects.call(context, inputs)
    }

    fn call_end(&mut self, context: &mut C, inputs: &CallInputs, outcome: &mut CallOutcome) {
        if self.matches(context, inputs) {
            self.completed += 1;
            let zero = U256::ZERO.to_be_bytes::<32>();
            let one = U256::from(1).to_be_bytes::<32>();
            if self.matches != 1
                || self.completed != 1
                || !outcome.result.result.is_ok()
                || (outcome.result.output.as_ref() != zero && outcome.result.output.as_ref() != one)
            {
                self.invalid = true;
            } else {
                outcome.result.output = one.to_vec().into();
            }
        }
        self.effects.call_end(context, inputs, outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eez_protocol::{CallMode, ExecutedAction, ExecutionOutcome, RollupId};
    use revm::interpreter::{CallInput, CallValue, Gas, InstructionResult, InterpreterResult};
    use revm::{Context, MainContext, database::InMemoryDB};

    fn snapshot_fixture(nonce: u64) -> SnapshotPreparation {
        SnapshotPreparation {
            vkey: B256::repeat_byte(7),
            poster_nonce: nonce,
            poster_balance: U256::from(100),
        }
    }

    #[test]
    fn preparation_reuse_is_bound_to_rollup_root_and_estimator() {
        let root = B256::repeat_byte(1);
        let mut cache = PreparationCache::default();
        assert_eq!(
            cache
                .get_or_load(1, root, || Ok(snapshot_fixture(10)))
                .unwrap()
                .poster_nonce,
            10
        );
        let snapshot = cache
            .get_or_load(1, root, || panic!("same snapshot must reuse"))
            .unwrap();
        assert_eq!(snapshot.vkey, B256::repeat_byte(7));
        assert_eq!(snapshot.poster_balance, U256::from(100));

        // Equal roots do not authorize using another rollup's deployment/vkey.
        assert_eq!(
            cache
                .get_or_load(2, root, || Ok(snapshot_fixture(20)))
                .unwrap()
                .poster_nonce,
            20
        );
        // A changed expected root must reach the lookup and its stale check.
        assert!(matches!(
            cache.get_or_load(2, B256::repeat_byte(2), || Err(FeeQuoteError::Stale)),
            Err(FeeQuoteError::Stale)
        ));
        assert_eq!(
            cache
                .get_or_load(2, root, || panic!(
                    "failed lookup must not replace validated preparation"
                ))
                .unwrap()
                .poster_nonce,
            20
        );

        // A different quote/envelope starts with fresh deployment/account reads,
        // even when the rollup and root match the previous quote.
        assert_eq!(
            PreparationCache::default()
                .get_or_load(2, root, || Ok(snapshot_fixture(30)))
                .unwrap()
                .poster_nonce,
            30
        );
    }

    #[test]
    fn preparation_retries_failed_lookups() {
        let mut cache = PreparationCache::default();
        for _ in 0..2 {
            for error in [
                FeeQuoteError::Stale,
                FeeQuoteError::Unavailable("L1 settlement execution"),
                FeeQuoteError::Unsupported("settlement proof profile"),
            ] {
                assert!(cache.get_or_load(1, B256::ZERO, || Err(error)).is_err());
                assert!(cache.previous.is_none());
            }
        }
        assert_eq!(
            cache
                .get_or_load(1, B256::ZERO, || Ok(snapshot_fixture(1)))
                .unwrap()
                .poster_nonce,
            1
        );
    }

    fn prepared_fixture() -> PreparedReplay {
        PreparedReplay {
            tx: TxEnv {
                gas_limit: 1_000_000,
                data: Bytes::from_static(&[1, 2, 3]),
                ..Default::default()
            },
            verify_data: Bytes::from_static(&[4, 5, 6]),
            endpoint: B256::repeat_byte(7),
            rollup_id: 1,
            effects: callback_expectations(1, true),
        }
    }

    fn callback_expectations(output: u8, success: bool) -> EffectInspector {
        EffectInspector::new(
            Address::repeat_byte(1),
            1,
            &[SemanticTransaction {
                origin_chain: RollupId(0),
                tx_data: Bytes::new(),
                actions: vec![ExecutedAction {
                    parent_index: None,
                    source_rollup_id: RollupId(0),
                    target_rollup_id: RollupId(1),
                    source_address: Address::repeat_byte(2),
                    target_address: Address::repeat_byte(3),
                    call_mode: CallMode::Mutable,
                    data: Bytes::from_static(&[4]),
                    value: U256::ZERO,
                    outcome: ExecutionOutcome::Resolved {
                        return_data: vec![output],
                        success,
                        gas_used: 100,
                    },
                    revert_spans: vec![],
                }],
            }],
        )
        .unwrap()
    }

    #[test]
    fn settlement_reuse_requires_transaction_proof_endpoint_and_callback_identity() {
        let original = prepared_fixture();
        for variant in 0..14 {
            let mut cache = ReplayCache::default();
            assert_eq!(cache.measure(original.clone(), |_| Ok(123)).unwrap(), 123);
            let mut changed = original.clone();
            match variant {
                0 => changed.tx.data = Bytes::from_static(&[9]),
                1 => changed.tx.gas_limit -= 1,
                2 => changed.tx.gas_price += 1,
                3 => changed.tx.gas_priority_fee = Some(1),
                4 => changed.tx.blob_hashes.push(B256::repeat_byte(1)),
                5 => changed.tx.max_fee_per_blob_gas += 1,
                6 => changed.tx.caller = Address::repeat_byte(9),
                7 => changed.tx.nonce += 1,
                8 => changed.tx.kind = TxKind::Call(Address::repeat_byte(9)),
                9 => changed.verify_data = Bytes::from_static(&[9]),
                10 => changed.endpoint = B256::repeat_byte(9),
                11 => changed.rollup_id += 1,
                // Even identical posting bytes cannot reuse validation of
                // different expected nested results or success polarity.
                12 => changed.effects = callback_expectations(9, true),
                _ => changed.effects = callback_expectations(1, false),
            }
            let mut executed = false;
            let gas = cache
                .measure(changed.clone(), |_| {
                    executed = true;
                    Ok(456)
                })
                .unwrap();
            assert!(executed, "changed input {variant} reused a measurement");
            assert_eq!(gas, 456);
            assert_eq!(cache.reuses, 0);
            assert_eq!(
                cache
                    .measure(changed, |_| panic!("identical successful input must reuse"))
                    .unwrap(),
                456
            );
            assert_eq!(cache.reuses, 1);
        }
    }

    #[test]
    fn settlement_reuse_never_memoizes_failed_replay() {
        let mut cache = ReplayCache::default();
        for _ in 0..2 {
            let mut executed = false;
            assert!(matches!(
                cache.measure(prepared_fixture(), |_| {
                    executed = true;
                    Err(FeeQuoteError::Unsupported("settlement effects mismatch"))
                }),
                Err(FeeQuoteError::Unsupported("settlement effects mismatch"))
            ));
            assert!(executed);
            assert!(cache.previous.is_none());
        }
        assert_eq!(cache.measure(prepared_fixture(), |_| Ok(123)).unwrap(), 123);
        assert_eq!(cache.reuses, 0);
        // Another quote/envelope owns an empty cache even for equal inputs.
        assert_eq!(
            ReplayCache::default()
                .measure(prepared_fixture(), |_| Ok(456))
                .unwrap(),
            456
        );
    }

    fn fixture() -> (MeteredProof, CallInputs, CallOutcome) {
        let registry = Address::repeat_byte(1);
        let verifier = Address::repeat_byte(2);
        let calldata: Bytes = verifyCall {
            proof: vec![0x11; 65].into(),
            publicInputsHash: B256::repeat_byte(3),
        }
        .abi_encode()
        .into();
        let inspector = MeteredProof {
            registry,
            verifier,
            calldata: calldata.clone(),
            matches: 0,
            completed: 0,
            invalid: false,
            effects: EffectInspector::new(registry, 1, &[]).unwrap(),
        };
        let input = CallInputs {
            input: CallInput::Bytes(calldata),
            return_memory_offset: 0..32,
            gas_limit: 100_000,
            reservoir: 0,
            bytecode_address: verifier,
            known_bytecode: Default::default(),
            target_address: verifier,
            caller: registry,
            value: CallValue::Transfer(U256::ZERO),
            scheme: CallScheme::StaticCall,
            is_static: true,
            charged_new_account_state_gas: false,
        };
        let mut gas = Gas::new(100_000);
        assert!(gas.record_regular_cost(4_321));
        let outcome = CallOutcome::new(
            InterpreterResult::new(
                InstructionResult::Return,
                U256::ZERO.to_be_bytes::<32>().to_vec().into(),
                gas,
            ),
            0..32,
        );
        (inspector, input, outcome)
    }

    #[test]
    fn proof_assumption_runs_verifier_and_preserves_its_metered_gas() {
        let mut context = Context::mainnet().with_db(InMemoryDB::default());
        let (mut inspector, mut input, mut outcome) = fixture();
        let gas = outcome.result.gas;
        assert!(
            inspector.call(&mut context, &mut input).is_none(),
            "verifier must execute"
        );
        inspector.call_end(&mut context, &input, &mut outcome);
        assert_eq!(
            outcome.result.output.as_ref(),
            U256::from(1).to_be_bytes::<32>()
        );
        assert_eq!(
            outcome.result.gas, gas,
            "assumption cannot erase verifier cost"
        );
        assert_eq!(
            (inspector.matches, inspector.completed, inspector.invalid),
            (1, 1, false)
        );
        assert!(inspector.call(&mut context, &mut input).is_none());
        inspector.call_end(&mut context, &input, &mut outcome);
        assert!(
            inspector.invalid,
            "repeated proof calls cannot authorize an estimate"
        );
    }

    #[test]
    fn proof_assumption_rejects_wrong_scope_reverts_and_malformed_returns() {
        let mut context = Context::mainnet().with_db(InMemoryDB::default());
        for variant in 0..7 {
            let (mut inspector, mut input, mut outcome) = fixture();
            match variant {
                0 => input.caller = Address::ZERO,
                1 => input.target_address = Address::ZERO,
                2 => input.bytecode_address = Address::ZERO,
                3 => input.scheme = CallScheme::Call,
                4 => input.is_static = false,
                5 => input.input = CallInput::Bytes(Bytes::new()),
                _ => inspector.calldata = Bytes::new(),
            }
            assert!(inspector.call(&mut context, &mut input).is_none());
            inspector.call_end(&mut context, &input, &mut outcome);
            assert_eq!(inspector.matches, 0);
            assert_eq!(outcome.result.output.as_ref(), [0; 32]);
        }
        for variant in 0..3 {
            let (mut inspector, mut input, mut outcome) = fixture();
            match variant {
                0 => outcome.result.result = InstructionResult::Revert,
                1 => outcome.result.output = vec![1].into(),
                _ => outcome.result.output = U256::from(2).to_be_bytes::<32>().to_vec().into(),
            }
            let original = outcome.clone();
            inspector.call(&mut context, &mut input);
            inspector.call_end(&mut context, &input, &mut outcome);
            assert!(inspector.invalid);
            assert_eq!(outcome, original);
        }
    }
}
