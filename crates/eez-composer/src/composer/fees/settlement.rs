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
    let gas = estimate(runtime, posting, &input)?;
    let cost = posting
        .quote
        .estimate(gas, input.blobs as u64)
        .map_err(unavailable)?;
    let payment = runtime
        .collection
        .pricing
        .as_ref()
        .ok_or(FeeQuoteError::Disabled)?
        .required_payment(cost.total)
        .map_err(FeeQuoteError::Limit)?;
    Ok((gas, cost, payment))
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
    let counts = eez_protocol::system_tx::outbound_settlement_root_counts(input.entries)
        .map_err(unavailable)?;
    let effects = input
        .groups
        .iter()
        .flat_map(|g| &g.effects)
        .collect::<Vec<_>>();
    if counts.iter().sum::<usize>() != effects.len() {
        return Err(FeeQuoteError::Unsupported("settlement effect coverage"));
    }
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
                effect.callback_entries.iter().try_fold(
                    total.checked_add(source)?,
                    |total, callback| {
                        total.checked_add(eez_protocol::entries::entry_flat_value_sum(
                            callback.as_abi_entry(),
                        )?)
                    },
                )
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

fn estimate(
    runtime: &PostingRuntime,
    posting: &PostingQuote,
    input: &SettlementInput<'_>,
) -> Result<u64, FeeQuoteError> {
    let registry = read_call(
        runtime,
        posting,
        runtime.registry,
        rollupsCall {
            rollupId: input.rollup_id,
        },
    )?;
    if registry.root != input.settled_root {
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
    let batch = batch(runtime, input)?;
    let hashes = (0..input.blobs)
        .map(|i| {
            let mut hash = placeholder(i)?;
            hash[0] = 1;
            Ok(hash)
        })
        .collect::<Result<Vec<_>, FeeQuoteError>>()?;
    let context = eez_protocol::public_inputs::L1PostingContext {
        chain_id: runtime.ctx.l1_chain_id,
        parent_number: posting.parent.number,
        parent_hash: posting.quote.sample.hash,
        target_timestamp: posting.attributes.timestamp,
    };
    let digests = eez_protocol::public_inputs::public_inputs_hashes_with_blob_hashes(
        &batch, *vkey, &hashes, context,
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
    let tx = TxEnv {
        tx_type: 3,
        caller: runtime.ctx.l1_poster_signer.address(),
        kind: TxKind::Call(runtime.post_batch_target),
        data: postAndVerifyBatchCall {
            batch: batch.clone(),
        }
        .abi_encode()
        .into(),
        nonce: provider
            .account_nonce(&runtime.ctx.l1_poster_signer.address())
            .map_err(unavailable)?
            .unwrap_or_default(),
        gas_limit: posting.gas_budget,
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
    let execute = |tx| replay(runtime, posting, input, tx, &verify_data, endpoint);
    let (used, required) = execute(tx.clone())?;
    let mut selected = tx;
    selected.gas_limit = runtime
        .fees
        .declared_gas(required, posting.gas_budget)
        .map_err(unavailable)?;
    if selected.gas_limit == posting.gas_budget {
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
    input: &SettlementInput<'_>,
    tx: TxEnv,
    verify_data: &Bytes,
    endpoint: B256,
) -> Result<(u64, u64), FeeQuoteError> {
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
        calldata: verify_data.clone(),
        matches: 0,
        completed: 0,
        invalid: false,
        effects: EffectInspector::new(runtime.registry, input.rollup_id, input.semantics)
            .map_err(unavailable)?,
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
            let mut evm =
                config.evm_with_env_and_inspector(&mut state, posting.env.clone(), &mut inspector);
            evm.transact(tx).map_err(unavailable)?.result
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
            if event.rollupId == input.rollup_id {
                last = Some(event.newRoot);
            }
        }
    }
    if last != Some(endpoint) {
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
    use revm::interpreter::{CallInput, CallValue, Gas, InstructionResult, InterpreterResult};
    use revm::{Context, MainContext, database::InMemoryDB};

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
