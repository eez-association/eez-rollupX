//! Conditional L1 setup belongs only to wallet estimation. No state or proof
//! produced here can enter the signed posting pipeline.
use super::*;
use alloy_sol_types::{SolCall, SolEvent};
use reth_evm::execute::BlockExecutor as _;
use revm::context_interface::ContextTr;
use revm::interpreter::{
    CallInputs, CallOutcome, CallScheme, Gas, InstructionResult, InterpreterResult,
};
use revm::{Database, Inspector, Journal};

alloy_sol_types::sol! {
    function verify(bytes proof, bytes32 publicInputsHash) external view returns (bool);
}

struct AssumedProof {
    registry: Address,
    verifier: Address,
    calldata: Bytes,
    matches: usize,
}

impl<DB: Database, C: ContextTr<Db = DB, Journal = Journal<DB>>> Inspector<C> for AssumedProof {
    fn call(&mut self, context: &mut C, inputs: &mut CallInputs) -> Option<CallOutcome> {
        if inputs.scheme != CallScheme::StaticCall
            || inputs.caller != self.registry
            || inputs.target_address != self.verifier
            || inputs.bytecode_address != self.verifier
            || !inputs.is_static
            || &*inputs.input.as_bytes(context) != self.calldata.as_ref()
        {
            return None;
        }
        self.matches += 1;
        if self.matches != 1 {
            return None;
        }
        Some(CallOutcome::new(
            InterpreterResult::new(
                InstructionResult::Return,
                U256::from(1).to_be_bytes::<32>().to_vec().into(),
                Gas::new(inputs.gas_limit),
            ),
            inputs.return_memory_offset.clone(),
        ))
    }
}

/// Both networks execute real transactions under their native configuration.
/// The only assumed result is scoped to setup's exact verification call.
pub(super) fn replay(
    estimator: &GasEstimator,
    snapshot: &Snapshot,
    batch: &eez_protocol::EvmBatch,
    composition: &eez_protocol::Composition,
    mut tx: revm::context::TxEnv,
    digest: B256,
) -> Result<Probe, EstimateError> {
    let chain = estimator.l1.chain_provider();
    let (mut state, _) = snapshot.l1.fork_state(&estimator.l1).map_err(unavailable)?;
    let mut proof = AssumedProof {
        registry: estimator.l1.manager_address(),
        verifier: estimator.verifier,
        calldata: verifyCall {
            proof: Bytes::new(),
            publicInputsHash: digest,
        }
        .abi_encode()
        .into(),
        matches: 0,
    };
    let setup = revm::context::TxEnv {
        caller: estimator.poster,
        kind: alloy_primitives::TxKind::Call(estimator.post_batch_target),
        data: eez_protocol::abi::postAndVerifyBatchCall {
            batch: batch.clone(),
        }
        .abi_encode()
        .into(),
        gas_limit: snapshot.l1_env.block_env.gas_limit,
        chain_id: Some(snapshot.l1_env.cfg_env.chain_id),
        ..Default::default()
    };
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
    let mut setup_env = snapshot.l1_env.clone();
    setup_env.cfg_env.disable_nonce_check = true;
    setup_env.cfg_env.disable_balance_check = true;
    setup_env.cfg_env.disable_base_fee = true;
    setup_env.cfg_env.disable_eip3607 = true;
    setup_env.cfg_env.disable_fee_charge = true;
    setup_env.cfg_env.tx_gas_limit_cap = Some(setup.gas_limit);

    macro_rules! execute {
        ($config:expr, $parent:expr) => {{
            let config = $config;
            let context = config
                .context_for_next_block(&$parent, snapshot.l1_attributes.clone())
                .map_err(unavailable)?;
            {
                let evm = config.evm_with_env(&mut state, snapshot.l1_env.clone());
                config
                    .create_executor(evm, context)
                    .apply_pre_execution_changes()
                    .map_err(unavailable)?;
            }
            {
                let evm = config.evm_with_env_and_inspector(&mut state, setup_env, &mut proof);
                let output = execute_user(evm, setup)?;
                if !output.success {
                    return Err(EstimateError::Unsupported(
                        "conditional L1 setup reverted".into(),
                    ));
                }
                if output.gas_required > snapshot.l1_env.cfg_env.tx_gas_limit_cap() {
                    return Err(unsupported("prepared L1 group exceeds native gas capacity"));
                }
                tx.gas_limit = tx.gas_limit.min(
                    snapshot
                        .l1_env
                        .block_env
                        .gas_limit
                        .saturating_sub(output.gas_used),
                );
                limit = tx.gas_limit;
                observe_roots(&output.logs, estimator, &mut last_root)?;
                if proof.matches != 1 {
                    return Err(EstimateError::Unsupported(
                        "conditional setup did not match exactly one proof call".into(),
                    ));
                }
            }
            let mut user_env = snapshot.l1_env.clone();
            user_env.cfg_env.disable_base_fee = true;
            let evm = config.evm_with_env_and_inspector(&mut state, user_env, &mut effects);
            execute_user(evm, tx)?
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
    use revm::interpreter::{CallInput, CallValue};
    use revm::{Context, MainContext, database::InMemoryDB};

    #[test]
    fn assumption_matches_only_one_exact_registry_verification() {
        let mut context = Context::mainnet().with_db(InMemoryDB::default());
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
        let mut wrong_caller = input.clone();
        wrong_caller.caller = Address::ZERO;
        assert!(inspector.call(&mut context, &mut wrong_caller).is_none());
        let mut wrong_target = input.clone();
        wrong_target.target_address = Address::ZERO;
        assert!(inspector.call(&mut context, &mut wrong_target).is_none());
        let mut wrong_digest = input.clone();
        wrong_digest.input = CallInput::Bytes(
            verifyCall {
                proof: Bytes::new(),
                publicInputsHash: B256::ZERO,
            }
            .abi_encode()
            .into(),
        );
        assert!(inspector.call(&mut context, &mut wrong_digest).is_none());
        let mut wrong_scheme = input.clone();
        wrong_scheme.scheme = CallScheme::Call;
        assert!(inspector.call(&mut context, &mut wrong_scheme).is_none());
        assert_eq!(inspector.matches, 0);
        let output = inspector.call(&mut context, &mut input.clone()).unwrap();
        assert_eq!(
            output.result.output.as_ref(),
            U256::from(1).to_be_bytes::<32>()
        );
        assert_eq!(inspector.matches, 1);
        assert!(inspector.call(&mut context, &mut input.clone()).is_none());
        assert_eq!(
            inspector.matches, 2,
            "setup must reject repeated verification"
        );
    }
}
