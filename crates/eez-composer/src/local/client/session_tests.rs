//! Real target sessions over an in-memory provider; no network or node process.
mod provider;
use super::*;
use alloy_consensus::Header;
use alloy_primitives::{U256, bytes};
use eez_protocol::{CallMode, ExecutionOutcome, ExecutionRequest};
use reth_chainspec::ChainSpecBuilder;
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};

const MANAGER: Address = Address::repeat_byte(0x11);
const PROXY: Address = Address::repeat_byte(0x22);
const TARGET: Address = Address::repeat_byte(0x33);

fn returning_proxy(opcode: u8) -> Bytes {
    // PUSH20 proxy; MSTORE(0); RETURN/REVERT(0, 32).
    let mut code = vec![0x73];
    code.extend_from_slice(PROXY.as_slice());
    code.extend_from_slice(&[0x60, 0, 0x52, 0x60, 32, 0x60, 0, opcode]);
    code.into()
}

fn fixture(manager_code: Bytes, target_code: Bytes) -> (Arc<LocalChainClient>, MockEthProvider) {
    let provider: MockEthProvider = MockEthProvider::new().with_genesis_block();
    let header = Header {
        number: 1,
        gas_limit: 30_000_000,
        ..Default::default()
    };
    provider.add_header(header.hash_slow(), header);
    provider.add_account(
        MANAGER,
        ExtendedAccount::new(0, U256::ZERO).with_bytecode(manager_code),
    );
    provider.add_account(
        TARGET,
        ExtendedAccount::new(0, U256::ZERO).with_bytecode(target_code),
    );
    let mut client = LocalChainClient::new_entry(
        provider.clone(),
        EezEvmConfig::for_l1_simulation(Arc::new(ChainSpecBuilder::mainnet().build())),
        RollupId::MAINNET,
        MANAGER,
        eez_protocol::ChainDialect::EvmL1Style,
    );
    // Enable REVERT independently of mainnet's historical block schedule.
    let mut env = client
        .provider
        .evm_config
        .evm_env(&Header::default())
        .unwrap();
    env.cfg_env
        .set_spec_and_mainnet_gas_params(revm::primitives::hardfork::SpecId::BYZANTIUM);
    env.block_env.gas_limit = 30_000_000;
    Arc::get_mut(&mut client).unwrap().session_context = Some((alloy_primitives::B256::ZERO, env));
    (client, provider)
}

fn execute(client: &Arc<LocalChainClient>, mode: CallMode) -> ExecutorResult<ExecutionOutcome> {
    execute_value(client, mode, U256::ZERO)
}

fn execute_value(
    client: &Arc<LocalChainClient>,
    mode: CallMode,
    value: U256,
) -> ExecutorResult<ExecutionOutcome> {
    let mut builder = CompositionBuilder::new(
        RollupId(7),
        std::collections::HashMap::from([(
            RollupId::MAINNET,
            eez_protocol::Rollup {
                client: client.clone(),
                session: None,
                config: eez_protocol::TargetConfig {
                    proxy_lookup: client.proxy_lookup_config(),
                    dialect: eez_protocol::ChainDialect::EvmL1Style,
                },
            },
        )]),
    );
    let outcome = builder.dispatch_call(
        RollupId::MAINNET,
        ExecutionRequest {
            call_mode: mode,
            target_address: TARGET,
            data: Bytes::new(),
            value,
            source_address: Address::repeat_byte(0x44),
            source_rollup_id: RollupId(7),
        },
    )?;
    builder
        .finalize()
        .expect("successful target result composes");
    Ok(outcome)
}

#[test]
fn manager_lookup_rejects_revert_halt_and_malformed_output() {
    for code in [
        returning_proxy(0xfd), // Valid-looking address in revert data must not authorize a call.
        bytes!("fe"),
        Bytes::new(),
        bytes!("6001600052601f6000f3"), // Short output.
        bytes!("600160005260216000f3"), // Overlong output.
    ] {
        for mode in [CallMode::Mutable, CallMode::Static] {
            let (mut client, provider) = fixture(code.clone(), bytes!("3360005260206000f3"));
            let faults = with_faults(&mut client, provider, MANAGER);
            faults
                .failing
                .store(false, std::sync::atomic::Ordering::Relaxed);
            let result = execute(&client, mode);
            assert!(
                matches!(result, Err(ref e) if matches!(e.kind(), ExecutorErrorKind::Unavailable(_))),
                "manager code {code}: {result:?}"
            );
            assert_eq!(
                faults
                    .target_reads
                    .load(std::sync::atomic::Ordering::Relaxed),
                0
            );
        }
    }
}

#[test]
fn manager_lookup_valid_result_executes_with_proxy_caller() {
    for mode in [CallMode::Mutable, CallMode::Static] {
        let (client, _) = fixture(returning_proxy(0xf3), bytes!("3360005260206000f3"));
        let ExecutionOutcome::Resolved {
            success,
            return_data,
            ..
        } = execute(&client, mode).unwrap()
        else {
            panic!("unresolved target")
        };
        assert!(success);
        assert_eq!(return_data, PROXY.into_word().to_vec());
    }
}

fn with_faults(
    client: &mut Arc<LocalChainClient>,
    provider: MockEthProvider,
    at: Address,
) -> provider::FaultProvider {
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    let faults = provider::FaultProvider {
        inner: provider,
        fail_storage_at: at,
        failing: Arc::new(AtomicBool::new(true)),
        target_reads: Arc::new(AtomicUsize::new(0)),
    };
    Arc::get_mut(client).unwrap().provider.provider = Arc::new(faults.clone());
    faults
}

fn provider_error(at: Address, mode: CallMode) -> ExecutorError {
    let mut code = bytes!("60005450").to_vec(); // Read storage, discard value.
    code.extend_from_slice(&returning_proxy(0xf3));
    let (mut client, provider) = fixture(code.into(), bytes!("600054503360005260206000f3"));
    let faults = with_faults(&mut client, provider, at);
    let error = execute(&client, mode).unwrap_err();
    if at == MANAGER {
        assert_eq!(
            faults
                .target_reads
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "failed lookup must not reach the target"
        );
    }
    faults
        .failing
        .store(false, std::sync::atomic::Ordering::Relaxed);
    assert!(matches!(
        execute(&client, mode).unwrap(),
        ExecutionOutcome::Resolved { success: true, .. }
    ));
    error
}

/// Errors used by the scheduler regression come from actual production sessions.
pub(crate) fn manager_lookup_errors() -> Vec<ExecutorError> {
    let mut errors = Vec::new();
    for mode in [CallMode::Mutable, CallMode::Static] {
        for at in [MANAGER, TARGET] {
            errors.push(provider_error(at, mode));
        }
    }
    let (client, _) = fixture(returning_proxy(0xfd), bytes!("00"));
    errors.push(execute(&client, CallMode::Mutable).unwrap_err());
    errors
}

fn assert_provider_error(error: ExecutorError) {
    use reth_revm::db::bal::EvmDatabaseError;
    use reth_storage_api::errors::provider::ProviderError;
    let ExecutorErrorKind::Provider(source) = error.kind() else {
        panic!("{error:?}")
    };
    assert!(
        matches!(source.downcast_ref::<EvmDatabaseError<ProviderError>>(),
        Some(EvmDatabaseError::Database(ProviderError::StateForHashNotFound(hash))) if *hash == alloy_primitives::B256::repeat_byte(0xfa))
    );
}

#[test]
fn manager_lookup_provider_errors_preserve_type_and_recover() {
    for mode in [CallMode::Mutable, CallMode::Static] {
        assert_provider_error(provider_error(MANAGER, mode));
    }
}

#[test]
fn manager_lookup_target_provider_errors_preserve_type_and_recover() {
    for mode in [CallMode::Mutable, CallMode::Static] {
        assert_provider_error(provider_error(TARGET, mode));
    }
}

#[test]
fn manager_lookup_target_revert_and_halt_remain_outcomes() {
    for code in [returning_proxy(0xfd), bytes!("fe")] {
        for mode in [CallMode::Mutable, CallMode::Static] {
            let (client, _) = fixture(returning_proxy(0xf3), code.clone());
            assert!(matches!(
                execute(&client, mode).unwrap(),
                ExecutionOutcome::Resolved { success: false, .. }
            ));
        }
    }
}

#[test]
fn manager_lookup_rejects_noncanonical_address_padding() {
    // A successful 32-byte return with an address and nonzero high padding.
    let mut code = vec![0x7f];
    let mut word = PROXY.into_word();
    word[0] = 1;
    code.extend_from_slice(word.as_slice());
    code.extend_from_slice(&[0x60, 0, 0x52, 0x60, 32, 0x60, 0, 0xf3]);
    let (client, _) = fixture(code.into(), bytes!("00"));
    assert!(
        matches!(execute(&client, CallMode::Mutable), Err(e) if matches!(e.kind(), ExecutorErrorKind::Unavailable(_)))
    );
}

pub(crate) fn manager_lookup_execution_error() -> ExecutorError {
    let (client, provider) = fixture(returning_proxy(0xf3), bytes!("00"));
    provider.add_account(PROXY, ExtendedAccount::new(0, U256::MAX));
    execute_value(&client, CallMode::Mutable, U256::from(1)).unwrap_err()
}

#[test]
fn manager_lookup_keeps_nonprovider_execution_errors_distinct() {
    let error = manager_lookup_execution_error();
    assert!(
        matches!(error.kind(), ExecutorErrorKind::Evm(_)),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("call value credit balance overflow")
    );
}

#[test]
fn manager_lookup_snapshot_failure_is_retryable_for_latest_and_pinned_sessions() {
    use crate::local::provider::StateSnapshotProvider;
    use reth_storage_api::errors::provider::{ProviderError, ProviderResult};
    struct UnavailableSnapshot;
    impl StateSnapshotProvider for UnavailableSnapshot {
        fn latest(&self) -> ProviderResult<StateProviderBox> {
            Err(ProviderError::StateForHashNotFound(
                alloy_primitives::B256::ZERO,
            ))
        }
        fn state_by_block_hash(
            &self,
            _: alloy_primitives::B256,
        ) -> ProviderResult<StateProviderBox> {
            self.latest()
        }
    }
    for pinned in [false, true] {
        let (mut client, _) = fixture(returning_proxy(0xf3), bytes!("00"));
        if !pinned {
            Arc::get_mut(&mut client).unwrap().session_context = None;
        }
        let available = client.provider.provider.clone();
        Arc::get_mut(&mut client).unwrap().provider.provider = Arc::new(UnavailableSnapshot);
        let error = execute(&client, CallMode::Mutable).unwrap_err();
        assert!(
            matches!(error.kind(), ExecutorErrorKind::Provider(_)),
            "{error:?}"
        );
        Arc::get_mut(&mut client).unwrap().provider.provider = available;
        assert!(matches!(
            execute(&client, CallMode::Mutable).unwrap(),
            ExecutionOutcome::Resolved { success: true, .. }
        ));
    }
}
