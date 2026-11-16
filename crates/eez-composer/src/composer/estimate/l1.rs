//! Conditional L1 setup belongs only to wallet estimation. No state or proof
//! produced here can enter the signed posting pipeline.
use super::{
    Address, B256, Bytes, ConfigureEvm, Database, EstimateError, EvmFeatures, GasEstimator, Probe,
    Recovered, RecoveredTxEnvelope, Snapshot, Transaction, TxEnvelope, TxLegacy, U256,
    execute_user, set_source_gas_limit, unavailable, unsupported,
};
use alloy_consensus::TxEip4844;
use alloy_sol_types::{SolCall, SolEvent};
use evm2::{
    EvmTypes, Inspector,
    evm::inspector::InspectorConfig,
    interpreter::{GasTracker, InstrStop, Interpreter, Message, MessageKind, MessageResult},
};
use reth_evm::BlockExecutorFactory;
use reth_evm::execute::BlockExecutor as _;
use reth_evm_ethereum::EthEvmEnv;

alloy_sol_types::sol! {
    function verify(bytes proof, bytes32 publicInputsHash) external view returns (bool);
}

struct AssumedProof {
    registry: Address,
    verifier: Address,
    calldata: Bytes,
    matches: usize,
}

impl<T: EvmTypes> Inspector<T> for AssumedProof {
    fn inspector_config(&self) -> InspectorConfig {
        InspectorConfig::without_steps()
    }
    fn call(
        &mut self,
        context: &mut Interpreter<'_, '_, T>,
        inputs: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        if inputs.kind != MessageKind::StaticCall
            || inputs.caller != self.registry
            || inputs.destination != self.verifier
            || inputs.code_address != self.verifier
            || !inputs.is_static()
            || *inputs.input.to_bytes(context.memory()) != self.calldata.as_ref()
        {
            return None;
        }
        self.matches += 1;
        if self.matches != 1 {
            return None;
        }
        Some(MessageResult::<T> {
            stop: InstrStop::Return,
            output: U256::from(1).to_be_bytes::<32>().to_vec().into(),
            gas: GasTracker::new_with_execution_gas_and_reservoir(
                inputs.gas_limit,
                inputs.reservoir,
            ),
            ..Default::default()
        })
    }
}

enum SetupProof {
    GasOnly(AssumedProof),
    Priced(Box<super::super::fees::settlement::MeteredProof>),
}
impl SetupProof {
    fn valid(&self) -> bool {
        match self {
            Self::GasOnly(p) => p.matches == 1,
            Self::Priced(p) => p.valid(),
        }
    }
}
impl<T: EvmTypes> Inspector<T> for SetupProof {
    fn inspector_config(&self) -> InspectorConfig {
        InspectorConfig::without_steps()
    }
    fn create(
        &mut self,
        context: &mut Interpreter<'_, '_, T>,
        inputs: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        match self {
            Self::GasOnly(_) => None,
            Self::Priced(p) => p.create(context, inputs),
        }
    }
    fn create_end(
        &mut self,
        context: &mut Interpreter<'_, '_, T>,
        inputs: &Message<T>,
        outcome: &mut MessageResult<T>,
    ) {
        if let Self::Priced(p) = self {
            p.create_end(context, inputs, outcome);
        }
    }
    fn call(
        &mut self,
        context: &mut Interpreter<'_, '_, T>,
        inputs: &mut Message<T>,
    ) -> Option<MessageResult<T>> {
        match self {
            Self::GasOnly(p) => p.call(context, inputs),
            Self::Priced(p) => p.call(context, inputs),
        }
    }
    fn call_end(
        &mut self,
        context: &mut Interpreter<'_, '_, T>,
        inputs: &Message<T>,
        outcome: &mut MessageResult<T>,
    ) {
        if let Self::Priced(p) = self {
            p.call_end(context, inputs, outcome);
        }
    }
}

/// Both networks execute real transactions under their native configuration.
/// The only assumed result is scoped to setup's exact verification call.
pub(super) struct Replay<'a> {
    pub batch: &'a eez_protocol::EvmBatch,
    pub composition: &'a eez_protocol::Composition,
    pub tx: RecoveredTxEnvelope,
    pub digest: B256,
    pub posting: Option<&'a super::super::posting::PostingQuote>,
    pub blob_hashes: &'a [B256],
    pub gas_limit: Option<u64>,
}

pub(super) fn replay(
    estimator: &GasEstimator,
    snapshot: &Snapshot,
    input: Replay<'_>,
) -> Result<Probe, EstimateError> {
    let Replay {
        batch,
        composition,
        mut tx,
        digest,
        posting,
        blob_hashes,
        gas_limit,
    } = input;
    let chain = estimator.l1.chain_provider();
    let (mut state, _) = snapshot.l1.fork_state(&estimator.l1).map_err(unavailable)?;
    let calldata = verifyCall {
        proof: batch.proofs[0].clone(),
        publicInputsHash: digest,
    }
    .abi_encode()
    .into();
    let mut proof = if posting.is_some() {
        SetupProof::Priced(Box::new(
            super::super::fees::settlement::MeteredProof::setup(
                estimator.l1.manager_address(),
                estimator.verifier,
                calldata,
                estimator.rollup_id,
            )
            .map_err(unavailable)?,
        ))
    } else {
        SetupProof::GasOnly(AssumedProof {
            registry: estimator.l1.manager_address(),
            verifier: estimator.verifier,
            calldata,
            matches: 0,
        })
    };
    let calldata = eez_protocol::abi::postAndVerifyBatchCall {
        batch: batch.clone(),
    }
    .abi_encode()
    .into();
    let mut funding = None;
    let setup = if let Some(posting) = posting {
        let account = state
            .db
            .get_account(&estimator.poster)
            .map_err(unavailable)?
            .unwrap_or_default();
        let funds = posting
            .probe_funding(estimator.poster, account.balance, blob_hashes.len() as u64)
            .map_err(unavailable)?;
        let limit = gas_limit.unwrap_or(funds.affordable_gas);
        if limit < 21_000 || limit > funds.affordable_gas {
            return Err(EstimateError::PosterFunds(Box::new(funds)));
        }
        funding = Some(funds);
        TxEnvelope::Eip4844(
            TxEip4844 {
                nonce: account.nonce,
                gas_limit: limit,
                to: estimator.post_batch_target,
                input: calldata,
                max_fee_per_gas: posting.quote.max_fee_per_gas,
                max_priority_fee_per_gas: posting.quote.max_priority_fee_per_gas,
                max_fee_per_blob_gas: posting.quote.max_fee_per_blob_gas,
                blob_versioned_hashes: blob_hashes.to_vec(),
                chain_id: snapshot.l1_env.version.chain_id,
                ..Default::default()
            }
            .into(),
        )
    } else {
        TxEnvelope::Legacy(TxLegacy {
            to: alloy_primitives::TxKind::Call(estimator.post_batch_target),
            input: calldata,
            gas_limit: snapshot.l1_env.block.gas_limit.saturating_to(),
            chain_id: Some(snapshot.l1_env.version.chain_id),
            ..Default::default()
        })
    };
    let setup = Recovered::new_unchecked(setup, estimator.poster);
    let mut effects = super::super::posting::EffectInspector::for_actions(
        estimator.l1.manager_address(),
        0,
        estimator.rollup_id,
        eez_protocol::EEZ_AUTHORIZED_PROXIES_SLOT,
        &composition.actions,
    )
    .map_err(unavailable)?;
    // Only fixture setup is fee-free. The user runs in a fresh EVM with normal
    // transaction accounting, so setup cannot leak warm accesses or refunds.
    let mut last_root = None;
    let limit;
    let post_used;
    let post_required;
    let post_limit = setup.gas_limit();
    let mut setup_env = snapshot.l1_env.clone();
    if posting.is_none() {
        setup_env
            .version
            .features
            .remove(EvmFeatures::NONCE_CHECK | EvmFeatures::BASE_FEE_CHECK | EvmFeatures::EIP3607);
        eez_evm::disable_fee_charging(&mut setup_env.version);
        setup_env.version.tx_gas_limit_cap = setup.gas_limit();
        setup_env.version.tx_total_gas_cap = setup.gas_limit();
    }

    macro_rules! execute {
        ($config:expr, $parent:expr) => {{
            let config = $config;
            let context = config
                .context_for_next_block(&$parent, snapshot.l1_attributes.clone())
                .map_err(unavailable)?;
            if snapshot.l1_prefix_gas == 0 {
                let env = EthEvmEnv::new_with_version(
                    snapshot.l1_env.spec,
                    snapshot.l1_env.block,
                    snapshot.l1_env.version,
                );
                let evm = config.evm_with_env(&mut state.db, env);
                let mut executor = config
                    .block_executor_factory()
                    .create_executor(evm, context);
                executor
                    .apply_pre_execution_changes()
                    .map_err(unavailable)?;
                let cache = std::mem::take(&mut executor.evm_mut().overlay_db_mut().cache);
                let statuses = executor
                    .block_state()
                    .clone()
                    .into_bundle()
                    .state
                    .into_iter()
                    .map(|(address, account)| (address, account.status))
                    .collect::<Vec<_>>();
                drop(executor);
                // The next EVM must inherit accepted pre-execution changes, without its journal.
                let mut accepted = state.take_snapshot();
                accepted.cache.merge(cache);
                accepted.statuses.extend(statuses);
                state.restore(accepted);
            }
            {
                let inspector = crate::composer::quote_trace::inspect(
                    &mut proof,
                    setup_env.version.chain_id,
                    "conditional_setup",
                    &setup,
                );
                let env =
                    EthEvmEnv::new_with_version(setup_env.spec, setup_env.block, setup_env.version);
                let mut evm = config.evm_with_env(&mut state.db, env);
                evm.set_inspector(inspector);
                let (output, pending) = execute_user(evm, &setup, funding.as_ref())?;
                state.commit(&pending);
                if !output.success {
                    return Err(EstimateError::Unsupported(
                        "conditional L1 setup reverted".into(),
                    ));
                }
                post_used = output.gas_used;
                post_required = output.gas_required;
                if output.gas_required > snapshot.l1_env.version.tx_gas_limit_cap {
                    return Err(unsupported("prepared L1 group exceeds native gas capacity"));
                }
                limit = tx.gas_limit().min(
                    snapshot
                        .l1_env
                        .block
                        .gas_limit
                        .saturating_to::<u64>()
                        .saturating_sub(output.gas_used)
                        .saturating_sub(snapshot.l1_prefix_gas),
                );
                set_source_gas_limit(&mut tx, limit)?;
                observe_roots(&output.logs, estimator, &mut last_root)?;
                if !proof.valid() {
                    return Err(EstimateError::Unsupported(
                        "conditional setup did not match exactly one proof call".into(),
                    ));
                }
            }
            let mut user_env = snapshot.l1_env.clone();
            user_env
                .version
                .features
                .set(EvmFeatures::BASE_FEE_CHECK, posting.is_some());
            let inspector = crate::composer::quote_trace::inspect(
                &mut effects,
                user_env.version.chain_id,
                "native_source",
                &tx,
            );
            let env = EthEvmEnv::new_with_version(user_env.spec, user_env.block, user_env.version);
            let mut evm = config.evm_with_env(&mut state.db, env);
            evm.set_inspector(inspector);
            let (output, pending) = execute_user(evm, &tx, None)?;
            state.commit(&pending);
            output
        }};
    }
    let result = if let Some(config) = &chain.gnosis_evm_config {
        execute!(
            config,
            reth_primitives_traits::SealedHeader::new(
                snapshot.l1.anchor.header().clone().into(),
                snapshot.l1.anchor.hash()
            )
        )
    } else {
        let config = reth_evm_ethereum::EthEvmConfig::new(chain.evm_config.chain_spec().clone());
        execute!(&config, snapshot.l1.anchor.clone())
    };
    if !result.success {
        return Err(EstimateError::Revert(result.output));
    }
    observe_roots(&result.logs, estimator, &mut last_root)?;
    let expected = batch
        .entries
        .iter()
        .flat_map(|e| &e.rootUpdates)
        .filter(|r| r.rollupId == estimator.rollup_id)
        .last()
        .map(|r| r.newRoot);
    if last_root != expected {
        return Err(EstimateError::Effects(
            "L1 replay did not reach the intended endpoint".into(),
        ));
    }
    effects
        .verify()
        .map_err(|e| EstimateError::Effects(e.to_string()))?;
    Ok(Probe {
        required: result.gas_required,
        limit,
        source_gas: result.gas_used,
        output: result.output,
        actions: composition.actions.clone(),
        settlement: posting.map(|_| (post_used, blob_hashes.len(), 0)),
        post_limit,
        post_funding: funding,
        post_required,
        retained_l1: Some(state.take_snapshot()),
        retained_l2: Vec::new(),
        retained_root: None,
    })
}

fn observe_roots(
    logs: &[alloy_primitives::Log],
    e: &GasEstimator,
    last: &mut Option<B256>,
) -> Result<(), EstimateError> {
    for log in logs.iter().filter(|l| l.address == e.l1.manager_address()) {
        if log.data.topics().first() == Some(&eez_protocol::abi::L2TxSkipped::SIGNATURE_HASH) {
            return Err(EstimateError::Effects(
                "L1 replay skipped an intended entry".into(),
            ));
        }
        if log.data.topics().first()
            == Some(&eez_protocol::abi::L2ExecutionPerformed::SIGNATURE_HASH)
        {
            let event =
                eez_protocol::abi::L2ExecutionPerformed::decode_log(log).map_err(unavailable)?;
            if event.rollupId == e.rollup_id {
                *last = Some(event.newRoot);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use evm2::{
        BaseEvmTypes, SpecId, ethereum::ethereum_tx_registry, evm::InMemoryDB,
        precompiles::Precompiles,
    };

    #[test]
    fn assumption_matches_only_one_exact_registry_verification() {
        let mut host = evm2::Evm::<BaseEvmTypes>::new(
            SpecId::OSAKA,
            Default::default(),
            ethereum_tx_registry(SpecId::OSAKA),
            InMemoryDB::default(),
            Precompiles::base(SpecId::OSAKA),
        );
        let frame = Message::<BaseEvmTypes>::default();
        let mut context = Interpreter::<BaseEvmTypes>::new(&frame);
        context.prepare_run(&mut host);
        let registry = Address::repeat_byte(1);
        let verifier = Address::repeat_byte(2);
        let calldata: Bytes = verifyCall {
            proof: Bytes::new(),
            publicInputsHash: B256::repeat_byte(3),
        }
        .abi_encode()
        .into();
        let mut inspector = AssumedProof {
            registry,
            verifier,
            calldata: calldata.clone(),
            matches: 0,
        };
        let input = Message::<BaseEvmTypes> {
            input: calldata.into(),
            gas_limit: 100_000,
            reservoir: 0,
            code_address: verifier,
            destination: verifier,
            call_target: verifier,
            caller: registry,
            value: U256::ZERO,
            kind: MessageKind::StaticCall,
            caller_is_static: true,
            ..Default::default()
        };
        let mut wrong_caller = input.clone();
        wrong_caller.caller = Address::ZERO;
        assert!(inspector.call(&mut context, &mut wrong_caller).is_none());
        let mut wrong_target = input.clone();
        wrong_target.destination = Address::ZERO;
        assert!(inspector.call(&mut context, &mut wrong_target).is_none());
        let mut wrong_digest = input.clone();
        wrong_digest.input = Bytes::from(
            verifyCall {
                proof: Bytes::new(),
                publicInputsHash: B256::ZERO,
            }
            .abi_encode(),
        )
        .into();
        assert!(inspector.call(&mut context, &mut wrong_digest).is_none());
        let mut wrong_scheme = input.clone();
        wrong_scheme.kind = MessageKind::Call;
        assert!(inspector.call(&mut context, &mut wrong_scheme).is_none());
        assert_eq!(inspector.matches, 0);
        let output = inspector.call(&mut context, &mut input.clone()).unwrap();
        assert_eq!(output.output.as_ref(), U256::from(1).to_be_bytes::<32>());
        assert_eq!(inspector.matches, 1);
        assert!(inspector.call(&mut context, &mut input.clone()).is_none());
        assert_eq!(
            inspector.matches, 2,
            "setup must reject repeated verification"
        );
    }
}
