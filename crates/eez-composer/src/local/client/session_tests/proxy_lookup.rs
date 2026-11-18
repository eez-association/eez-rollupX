//! Fail the inspector's live authorization read through real source simulation.
use super::*;
use alloy_consensus::TxLegacy;
use alloy_eips::eip2718::Encodable2718;
use alloy_network::TxSignerSync;
use alloy_primitives::{B256, TxKind};
use alloy_signer_local::PrivateKeySigner;
use evm2::evm::Database;
use std::sync::atomic::Ordering;

const BALANCE: u64 = 1_000_000;

fn signer() -> PrivateKeySigner {
    PrivateKeySigner::from_bytes(&B256::with_last_byte(7)).unwrap()
}

fn signed_proxy_call() -> Vec<u8> {
    let mut tx = TxLegacy {
        chain_id: Some(1),
        gas_price: 1,
        gas_limit: 100_000,
        to: TxKind::Call(PROXY),
        ..Default::default()
    };
    let signature = signer().sign_transaction_sync(&mut tx).unwrap();
    reth_ethereum_primitives::TransactionSigned::new_unhashed(
        reth_ethereum_primitives::Transaction::Legacy(tx),
        signature,
    )
    .encoded_2718()
}

fn source_fixture(
    account_fault: bool,
    failing: bool,
) -> (Arc<LocalChainClient>, provider::FaultProvider) {
    let (mut client, backing) = fixture(bytes!("00"), bytes!("00"));
    let env = &mut Arc::get_mut(&mut client)
        .unwrap()
        .session_context
        .as_mut()
        .unwrap()
        .1;
    *env = reth_evm_ethereum::EthEvmEnv::new(evm2::SpecId::OSAKA, env.block, env.version.chain_id);
    let key = eez_protocol::proxy_mapping_key(
        PROXY,
        client.proxy_lookup_config().authorized_proxies_slot,
    );
    let mut packed = [0u8; 32];
    packed[3..11].copy_from_slice(&7u64.to_be_bytes());
    packed[11..31].copy_from_slice(TARGET.as_slice());
    packed[31] = 1;
    backing.add_account(
        MANAGER,
        ExtendedAccount::new(0, U256::ZERO)
            .with_bytecode(bytes!("00"))
            .extend_storage([
                (key, U256::from_be_bytes(packed)),
                (
                    eez_protocol::proxy_mapping_key(
                        TARGET,
                        client.proxy_lookup_config().authorized_proxies_slot,
                    ),
                    U256::from_be_bytes(packed),
                ),
            ]),
    );
    // The fail-once provider is healthy again inside this nested CALL. The latched
    // error must suppress its dispatch as well as rejecting the later marker write.
    let mut fallback = bytes!("6000600060006000600073").to_vec();
    fallback.extend_from_slice(TARGET.as_slice());
    fallback.extend_from_slice(&bytes!("5af150602a60005500"));
    backing.add_account(
        PROXY,
        ExtendedAccount::new(0, U256::ZERO).with_bytecode(fallback.into()),
    );
    backing.add_account(
        signer().address(),
        ExtendedAccount::new(0, U256::from(BALANCE)),
    );
    let mut faults = with_faults(&mut client, backing, MANAGER);
    faults.fail_account_at = account_fault.then_some(MANAGER);
    // Account faults cannot also fail the later slot read.
    faults.fail_storage_at = if account_fault {
        Address::repeat_byte(0xee)
    } else {
        MANAGER
    };
    faults.fail_storage_key = Some(key);
    faults.once = true;
    faults.failing.store(failing, Ordering::Relaxed);
    Arc::get_mut(&mut client).unwrap().provider.provider = Arc::new(faults.clone());
    (client, faults)
}

fn builder() -> CompositionBuilder {
    let (mut remote, _) = fixture(returning_proxy(0xf3), bytes!("602a60005260206000f3"));
    Arc::get_mut(&mut remote).unwrap().rollup_id = RollupId(7);
    let config = eez_protocol::TargetConfig {
        proxy_lookup: remote.proxy_lookup_config(),
        dialect: eez_protocol::ChainDialect::EvmL1Style,
    };
    CompositionBuilder::new(
        RollupId::MAINNET,
        std::collections::HashMap::from([(
            RollupId(7),
            eez_protocol::Rollup {
                client: remote,
                session: None,
                config,
            },
        )]),
    )
}

fn simulate(
    client: &LocalChainClient,
    state: &mut SimState,
    dispatch: &mut CompositionBuilder,
    retry: bool,
) -> ExecutorResult<SourceSimulation> {
    client.simulate_source_tx_on_with_retries(
        signed_proxy_call(),
        dispatch,
        state,
        client.session_context.as_ref().unwrap().1.clone(),
        retry,
    )
}

fn assert_healthy(client: &LocalChainClient, state: &mut SimState, retry: bool) {
    let mut dispatch = builder();
    let outcome = simulate(client, state, &mut dispatch, retry).unwrap();
    assert!(outcome.success);
    // Measured on the unchanged production implementation with this healthy fixture.
    assert_eq!(outcome.gas_used, 21_000);
    assert_eq!(
        outcome.output,
        Bytes::copy_from_slice(&U256::from(42).to_be_bytes::<32>())
    );
    assert_eq!(dispatch.recorded_count(), 1);
    assert_eq!(
        state
            .db
            .get_account(&signer().address())
            .unwrap()
            .unwrap()
            .nonce,
        1
    );
    assert_eq!(
        state.db.get_storage(&PROXY, &U256::ZERO).unwrap(),
        U256::ZERO
    );
}

pub(crate) fn injected_error(account_fault: bool, retry: bool) -> ExecutorError {
    let (client, faults) = source_fixture(account_fault, true);
    let mut state = SimState::new(Box::new(faults.clone()), None);
    let before_sender = state.db.get_account(&signer().address()).unwrap();
    let before_statuses = state.snapshot().statuses;
    let mut dispatch = builder();
    let result = simulate(&client, &mut state, &mut dispatch, retry);
    assert_eq!(
        faults.injected.load(Ordering::Relaxed),
        1,
        "must reach the intended fault"
    );
    assert!(
        !faults.failing.load(Ordering::Relaxed),
        "fault is fail-once"
    );
    let error = result.expect_err("authorization read failure must not become an ordinary call");
    assert_eq!(dispatch.recorded_count(), 0);
    assert_eq!(faults.matched.load(Ordering::Relaxed), 1);
    assert_eq!(
        state.db.get_account(&signer().address()).unwrap(),
        before_sender
    );
    assert_eq!(state.snapshot().statuses, before_statuses);
    assert_eq!(
        state.db.get_storage(&PROXY, &U256::ZERO).unwrap(),
        U256::ZERO
    );
    // Reuse the retained source state: a failed pass must not commit nonce, fees or fallback storage.
    assert_healthy(&client, &mut state, retry);
    assert_eq!(faults.matched.load(Ordering::Relaxed), 2);
    error
}

#[test]
fn proxy_lookup_account_failure_is_typed_and_retryable() {
    for retry in [false, true] {
        assert_provider_error(injected_error(true, retry));
    }
}

#[test]
fn proxy_lookup_slot_failure_is_typed_and_retryable() {
    for retry in [false, true] {
        assert_provider_error(injected_error(false, retry));
    }
}

#[test]
fn proxy_lookup_healthy_control() {
    for retry in [false, true] {
        let (client, faults) = source_fixture(false, false);
        let mut state = SimState::new(Box::new(faults.clone()), None);
        assert_healthy(&client, &mut state, retry);
        assert_eq!(faults.injected.load(Ordering::Relaxed), 0);
        assert_eq!(faults.matched.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn proxy_lookup_target_session_failure_is_typed_and_recovers() {
    for mode in [CallMode::Mutable, CallMode::Static] {
        let (mut client, backing) = fixture(returning_proxy(0xf3), bytes!("3360005260206000f3"));
        let mut faults = with_faults(&mut client, backing, MANAGER);
        faults.fail_storage_key = Some(eez_protocol::proxy_mapping_key(
            TARGET,
            client.proxy_lookup_config().authorized_proxies_slot,
        ));
        faults.once = true;
        Arc::get_mut(&mut client).unwrap().provider.provider = Arc::new(faults.clone());
        let result = execute(&client, mode);
        assert_eq!(faults.injected.load(Ordering::Relaxed), 1);
        assert_eq!(faults.matched.load(Ordering::Relaxed), 1);
        assert_provider_error(result.unwrap_err());
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
        assert_eq!(faults.matched.load(Ordering::Relaxed), 2);
    }
}
