use super::*;
use alloy_consensus::{SignableTransaction, TxLegacy};
use alloy_primitives::{Bytes, Signature, TxKind};
use eez_protocol::system_tx::LOAD_EXECUTION_TABLE_SELECTOR;
use reth_primitives_traits::Recovered;
use revm::inspector::NoOpInspector;

const USER: Address = Address::with_last_byte(77);
const TARGET: Address = Address::with_last_byte(88);
type Executor<'a> = EezBlockExecutor<
    'a,
    crate::evm::EezEvm<&'a mut State<InMemoryDB>, NoOpInspector>,
    &'a Arc<reth_chainspec::ChainSpec>,
>;

struct Fixture {
    factory: EezBlockExecutorFactory<reth_chainspec::ChainSpec>,
    state: State<InMemoryDB>,
}

impl Fixture {
    fn new(manager: Bytes, target: Bytes) -> Self {
        let (history, mut db) = crate::history::tests::fixture();
        db.insert_account_info(USER, AccountInfo::from_balance(U256::from(1_000_000)));
        for (address, raw) in [(EEZL2_ADDRESS, manager), (TARGET, target)] {
            let code = Bytecode::new_raw(raw);
            db.insert_account_info(
                address,
                AccountInfo {
                    code_hash: code.hash_slow(),
                    code: Some(code),
                    nonce: 1,
                    ..Default::default()
                },
            );
        }
        let spec = Arc::new(ChainSpecBuilder::mainnet().osaka_activated().build());
        let mut factory = EezBlockExecutorFactory::new(spec, ExecutionLayer::Rollup);
        factory.history = Some(history);
        factory.evm = crate::tests::factory();
        Self {
            factory,
            state: State::builder()
                .with_database(db)
                .with_bundle_update()
                .build(),
        }
    }

    fn executor(&mut self) -> Executor<'_> {
        let evm = self.factory.evm_factory().create_evm(
            &mut self.state,
            EvmEnv {
                cfg_env: CfgEnv::new_with_spec(SpecId::OSAKA).with_chain_id(1337),
                block_env: BlockEnv {
                    number: U256::ONE,
                    timestamp: U256::from(5),
                    gas_limit: 30_000_000,
                    prevrandao: Some(B256::ZERO),
                    ..Default::default()
                },
            },
        );
        let mut executor = self.factory.create_executor(
            evm,
            EthBlockExecutionCtx {
                parent_hash: B256::repeat_byte(5),
                parent_beacon_block_root: Some(B256::ZERO),
                ommers: &[],
                withdrawals: None,
                extra_data: Default::default(),
                tx_count_hint: None,
                slot_number: None,
            },
        );
        executor.apply_pre_execution_changes().unwrap();
        executor
    }
}

fn ordinary(nonce: u64, to: Address, value: u64) -> Recovered<EezTxEnvelope> {
    let tx = TxLegacy {
        chain_id: Some(1337),
        nonce,
        gas_limit: 100_000,
        gas_price: 0,
        to: TxKind::Call(to),
        value: U256::from(value),
        input: Bytes::new(),
    }
    .into_signed(Signature::test_signature());
    Recovered::new_unchecked(EezTxEnvelope::Ethereum(tx.into()), USER)
}

fn native(executor: &Executor<'_>, load: bool, value: u64) -> Recovered<EezTxEnvelope> {
    let selector = if load {
        LOAD_EXECUTION_TABLE_SELECTOR
    } else {
        eez_protocol::entries::EXECUTE_INCOMING_SELECTOR
    };
    let tx = eez_primitives::SystemTransaction {
        version: 1,
        chain_id: 1337,
        source_hash: B256::repeat_byte(99),
        gas_limit: system_tx_gas_limit(30_000_000, executor.cumulative_tx_gas_used),
        to: EEZL2_ADDRESS,
        value: U256::from(value),
        input: Bytes::copy_from_slice(&selector),
    };
    Recovered::new_unchecked(tx.into(), SYSTEM_ADDRESS)
}

fn balance(executor: &mut Executor<'_>, account: Address) -> U256 {
    executor
        .evm_mut()
        .db_mut()
        .basic(account)
        .unwrap()
        .map_or(U256::ZERO, |a| a.balance)
}

#[test]
fn complete_groups_clear_only_protocol_balances_after_user_and_inbound_execution() {
    // Record the manager balance seen during each transaction. The load and
    // paired user must see their shared funding; cleanup must preserve storage.
    let mut f = Fixture::new(bytes!("4760005500"), bytes!("00"));
    let mut executor = f.executor();
    executor
        .execute_transaction(ordinary(0, SYSTEM_ADDRESS, 9))
        .unwrap();
    assert_eq!(balance(&mut executor, SYSTEM_ADDRESS), U256::from(9));
    executor
        .execute_transaction(native(&executor, true, 37))
        .unwrap();
    assert_eq!(balance(&mut executor, EEZL2_ADDRESS), U256::from(37));
    assert_eq!(balance(&mut executor, SYSTEM_ADDRESS), U256::from(9));
    executor
        .execute_transaction(ordinary(1, EEZL2_ADDRESS, 11))
        .unwrap();
    for address in PROTOCOL_BALANCE_ACCOUNTS {
        assert_eq!(balance(&mut executor, address), U256::ZERO);
    }
    assert_eq!(
        executor
            .evm_mut()
            .db_mut()
            .storage(EEZL2_ADDRESS, U256::ZERO)
            .unwrap(),
        U256::from(48)
    );
    executor
        .execute_transaction(native(&executor, false, 19))
        .unwrap();
    assert_eq!(balance(&mut executor, EEZL2_ADDRESS), U256::ZERO);
    assert_eq!(
        executor
            .evm_mut()
            .db_mut()
            .storage(EEZL2_ADDRESS, U256::ZERO)
            .unwrap(),
        U256::from(19)
    );
    assert_eq!(
        executor
            .evm_mut()
            .db_mut()
            .basic(EEZL2_ADDRESS)
            .unwrap()
            .unwrap()
            .nonce,
        1
    );
    // Ordinary work after a group is not itself a boundary. The block-start
    // fallback, covered separately, clears these later donations next block.
    executor
        .execute_transaction(ordinary(2, SYSTEM_ADDRESS, 7))
        .unwrap();
    assert_eq!(balance(&mut executor, SYSTEM_ADDRESS), U256::from(7));
    assert_eq!(balance(&mut executor, USER), U256::from(1_000_000 - 27));
    let gas = executor.cumulative_tx_gas_used;
    let (_, output) = executor.finish().unwrap();
    assert_eq!(output.receipts.len(), 5);
    assert!(
        output
            .receipts
            .iter()
            .all(|r| r.success && r.logs.is_empty())
    );
    assert_eq!(output.gas_used, gas);
}

#[test]
fn reverted_and_halted_users_clean_on_commit_but_discarded_previews_do_not() {
    for target in [bytes!("60006000fd"), bytes!("fe")] {
        let mut f = Fixture::new(bytes!("00"), target);
        let mut executor = f.executor();
        executor
            .execute_transaction(native(&executor, true, 37))
            .unwrap();
        let pending = executor.group_state();
        let preview = executor
            .execute_transaction_without_commit(ordinary(0, TARGET, 0))
            .unwrap();
        assert!(!preview.result().result.is_success());
        assert_eq!(
            preview.result().state[&EEZL2_ADDRESS].info.balance,
            U256::ZERO
        );
        assert_eq!(balance(&mut executor, EEZL2_ADDRESS), U256::from(37));
        assert_eq!(executor.group_state(), pending);
        drop(preview);
        let result = executor
            .execute_transaction_without_commit(ordinary(0, TARGET, 0))
            .unwrap();
        let gas = result.result().result.tx_gas_used();
        executor.commit_transaction(result);
        assert_eq!(balance(&mut executor, EEZL2_ADDRESS), U256::ZERO);
        assert!(executor.group_state().ensure_complete().is_ok());
        assert_eq!(
            executor.receipts().last().unwrap().cumulative_gas_used
                - executor.receipts()[0].cumulative_gas_used,
            gas
        );
        executor
            .execute_transaction(native(&executor, false, 23))
            .unwrap();
        assert_eq!(balance(&mut executor, EEZL2_ADDRESS), U256::ZERO);
        executor.finish().unwrap();
    }
}

#[test]
fn failed_inbound_and_load_keep_failure_receipts_without_leaking_inventory() {
    // Empty calldata accepts ordinary donations; nonempty native calldata reverts.
    let mut f = Fixture::new(bytes!("3615600a5760006000fd5b00"), bytes!("00"));
    let mut executor = f.executor();
    executor
        .execute_transaction(ordinary(0, EEZL2_ADDRESS, 17))
        .unwrap();
    executor
        .execute_transaction(ordinary(1, SYSTEM_ADDRESS, 9))
        .unwrap();
    executor
        .execute_transaction(native(&executor, false, 31))
        .unwrap();
    assert!(!executor.receipts().last().unwrap().success);
    for address in PROTOCOL_BALANCE_ACCOUNTS {
        assert_eq!(balance(&mut executor, address), U256::ZERO);
    }
    executor
        .execute_transaction(native(&executor, true, 37))
        .unwrap();
    assert!(!executor.receipts().last().unwrap().success);
    assert!(executor.group_state().ensure_complete().is_err());
    executor
        .execute_transaction(ordinary(2, SYSTEM_ADDRESS, 11))
        .unwrap();
    assert_eq!(balance(&mut executor, SYSTEM_ADDRESS), U256::ZERO);
    executor.finish().unwrap();
}

#[test]
fn incomplete_groups_reject_native_interleaving_and_invalid_users_without_advancing() {
    let mut f = Fixture::new(bytes!("00"), bytes!("00"));
    let mut executor = f.executor();
    executor
        .execute_transaction(native(&executor, true, 37))
        .unwrap();
    let pending = executor.group_state();
    for load in [false, true] {
        let error = executor
            .execute_transaction(native(&executor, load, 9))
            .unwrap_err();
        assert!(error.to_string().contains("followed immediately"));
        assert_eq!(executor.group_state(), pending);
    }
    assert!(
        executor
            .execute_transaction(ordinary(1, TARGET, 0))
            .is_err()
    );
    assert_eq!(executor.group_state(), pending);
    assert_eq!(balance(&mut executor, EEZL2_ADDRESS), U256::from(37));
    assert!(
        executor
            .finish()
            .unwrap_err()
            .to_string()
            .contains("ends inside")
    );
}
