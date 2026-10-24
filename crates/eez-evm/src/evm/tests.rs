use super::*;
use alloy_primitives::{B256, bytes};
use revm::{
    Database as _, DatabaseCommit,
    database::InMemoryDB,
    inspector::CountInspector,
    state::{AccountInfo, Bytecode},
};

const USER: Address = Address::repeat_byte(0x11);
const TARGET: Address = Address::repeat_byte(0x22);
const BENEFICIARY: Address = Address::repeat_byte(0x33);
const COLLECTOR: Address = Address::repeat_byte(0x44);
const FUNDS: u64 = 100_000_000;

fn environment() -> EvmEnv {
    EvmEnv {
        cfg_env: CfgEnv::new_with_spec(SpecId::OSAKA).with_chain_id(901),
        block_env: BlockEnv {
            number: U256::from(1),
            timestamp: U256::from(1),
            basefee: 10,
            beneficiary: BENEFICIARY,
            gas_limit: 30_000_000,
            prevrandao: Some(B256::ZERO),
            ..Default::default()
        },
    }
}

fn database(code: Bytes, collector: Address) -> InMemoryDB {
    let mut db = InMemoryDB::default();
    let code = Bytecode::new_raw(code);
    db.insert_account_info(
        TARGET,
        AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        },
    );
    db.insert_account_info(
        USER,
        AccountInfo {
            balance: U256::from(FUNDS),
            ..Default::default()
        },
    );
    if collector != USER && collector != BENEFICIARY {
        // A credit must not invoke the collector's code.
        let code = Bytecode::new_raw(bytes!("fe"));
        db.insert_account_info(
            collector,
            AccountInfo {
                code_hash: code.hash_slow(),
                code: Some(code),
                ..Default::default()
            },
        );
    }
    db
}

fn ordinary() -> TxEnv {
    TxEnv {
        caller: USER,
        kind: TxKind::Call(TARGET),
        gas_limit: 100_000,
        gas_price: 13,
        chain_id: Some(901),
        ..Default::default()
    }
}

fn simulation_environment() -> EvmEnv {
    let mut env = environment();
    env.cfg_env.disable_fee_charge = true;
    env.cfg_env.disable_nonce_check = true;
    env.cfg_env.disable_eip3607 = true;
    env.cfg_env.disable_balance_check = true;
    env
}

#[test]
fn funded_simulation_preserves_proxy_funds_returns_and_rollback_without_fees() {
    for balance in [0, 11] {
        for (code, success, returned) in [
            (bytes!("00"), true, 0),
            // Return three wei to the caller using CALL, then succeed.
            (bytes!("5f5f5f5f6003335af15000"), true, 3),
            (bytes!("5f5ffd"), false, 0),
            (bytes!("fe"), false, 0),
        ] {
            let mut db = database(code, COLLECTOR);
            db.insert_account_info(
                USER,
                AccountInfo {
                    balance: U256::from(balance),
                    ..Default::default()
                },
            );
            let mut evm = EezEvmFactory::for_rollup(COLLECTOR)
                .unwrap()
                .create_evm(db, simulation_environment());
            let tx = TxEnv {
                value: U256::from(7),
                ..ordinary()
            };
            let result = evm.transact_funded_call(tx.clone()).unwrap();
            assert_eq!(result.result.is_success(), success);
            assert_eq!(
                result.state[&USER].info.balance,
                U256::from(balance + returned)
            );
            assert_eq!(
                result.state[&TARGET].info.balance,
                U256::from(if success { 7 - returned } else { 0 })
            );
            assert!(!result.state.contains_key(&COLLECTOR));
            assert!(
                result
                    .state
                    .get(&BENEFICIARY)
                    .is_none_or(|account| account.info.balance.is_zero())
            );
            assert_eq!(
                evm.db_mut().basic(USER).unwrap().unwrap().balance,
                U256::from(balance)
            );
            // A discarded frame cannot leave credit in the next execution.
            assert_eq!(evm.transact_funded_call(tx).unwrap(), result);
        }
    }
}

#[test]
fn funded_simulation_rejects_invalid_context_and_clears_credit_on_errors() {
    let factory = EezEvmFactory::for_rollup(COLLECTOR).unwrap();
    let db = database(bytes!("00"), COLLECTOR);
    let mut charged = factory.create_evm(db.clone(), environment());
    assert!(
        charged
            .transact_funded_call(ordinary())
            .unwrap_err()
            .to_string()
            .contains("invalid funded simulation call")
    );
    let mut evm = factory.create_evm(db, simulation_environment());
    let tx = TxEnv {
        value: U256::from(7),
        ..ordinary()
    };
    for invalid in [
        TxEnv {
            kind: TxKind::Create,
            ..tx.clone()
        },
        TxEnv {
            caller: SYSTEM_ADDRESS,
            ..tx.clone()
        },
        TxEnv {
            tx_type: SYSTEM_TX_TYPE,
            ..tx.clone()
        },
    ] {
        assert!(
            evm.transact_funded_call(invalid)
                .unwrap_err()
                .to_string()
                .contains("invalid funded simulation call")
        );
    }
    assert!(
        evm.transact_funded_call(TxEnv {
            chain_id: Some(902),
            ..tx.clone()
        })
        .is_err()
    );
    let result = evm.transact_funded_call(tx.clone()).unwrap();
    assert_eq!(result.state[&USER].info.balance, U256::from(FUNDS));
    assert_eq!(result.state[&TARGET].info.balance, U256::from(7));
    evm.db_mut().insert_account_info(
        USER,
        AccountInfo {
            balance: U256::MAX,
            ..Default::default()
        },
    );
    assert!(
        evm.transact_funded_call(tx)
            .unwrap_err()
            .to_string()
            .contains("credit balance overflow")
    );
    let zero = evm.transact_funded_call(ordinary()).unwrap();
    assert_eq!(zero.state[&USER].info.balance, U256::MAX);
}

#[test]
fn collector_receives_final_base_fee_for_success_revert_halt_refund_and_floor() {
    for collector in [COLLECTOR, USER, BENEFICIARY] {
        for (code, data, refund, floor, success) in [
            (bytes!("00"), Bytes::new(), false, false, true),
            (bytes!("5f5ffd"), Bytes::new(), false, false, false),
            (bytes!("fe"), Bytes::new(), false, false, false),
            (bytes!("600060005500"), Bytes::new(), true, false, true),
            (bytes!("00"), Bytes::from(vec![1; 100]), false, true, true),
        ] {
            let mut db = database(code, collector);
            if refund {
                db.insert_account_storage(TARGET, U256::ZERO, U256::ONE)
                    .unwrap();
            }
            let factory = EezEvmFactory::for_rollup(collector).unwrap();
            let mut plain = factory.create_evm(db.clone(), environment());
            let mut inspected =
                factory.create_evm_with_inspector(db, environment(), CountInspector::default());
            let tx = TxEnv { data, ..ordinary() };
            let result = plain.transact_raw(tx.clone()).unwrap();
            assert_eq!(result, inspected.transact_raw(tx).unwrap());
            assert_eq!(result.result.is_success(), success);
            assert!(inspected.components().1.step_count() > 0);
            let gas = result.result.tx_gas_used();
            if refund {
                assert!(result.result.gas().final_refunded() > 0);
            }
            if floor {
                assert_eq!(gas, 25_000);
            }
            let mut expected = std::collections::BTreeMap::from([
                (USER, U256::from(FUNDS) - U256::from(13 * gas)),
                (BENEFICIARY, U256::from(3 * gas)),
            ]);
            *expected.entry(collector).or_default() += U256::from(10 * gas);
            for (address, balance) in expected {
                assert_eq!(
                    result.state[&address].info.balance, balance,
                    "collector={collector} address={address}"
                );
            }
            assert_eq!(result.state[&USER].info.nonce, 1);
            // Output is provisional until its owner commits. A discarded preview
            // must not credit again when replayed from the same pre-state.
            assert_eq!(
                plain
                    .transact_raw(TxEnv {
                        data: if floor {
                            Bytes::from(vec![1; 100])
                        } else {
                            Bytes::new()
                        },
                        ..ordinary()
                    })
                    .unwrap(),
                result
            );
        }
    }
}

#[test]
fn collector_credit_is_visible_to_the_next_transaction_without_running_its_code() {
    let mut evm = EezEvmFactory::for_rollup(COLLECTOR)
        .unwrap()
        .create_evm(database(bytes!("00"), COLLECTOR), environment());
    let first = evm.transact_raw(ordinary()).unwrap();
    let credited = U256::from(first.result.tx_gas_used() * 10);
    evm.db_mut().commit(first.state);
    // BALANCE(collector), returned as a word.
    let mut raw = vec![0x73];
    raw.extend_from_slice(COLLECTOR.as_slice());
    raw.extend_from_slice(&bytes!("315f5260205ff3"));
    let code = Bytecode::new_raw(raw.into());
    evm.db_mut().insert_account_info(
        TARGET,
        AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        },
    );
    let second = evm
        .transact_raw(TxEnv {
            nonce: 1,
            ..ordinary()
        })
        .unwrap();
    assert_eq!(
        U256::from_be_slice(second.result.output().unwrap()),
        credited
    );
    assert_eq!(
        second.state[&COLLECTOR].info.balance,
        credited + U256::from(second.result.tx_gas_used() * 10)
    );
}

#[test]
fn collector_overflow_rejects_and_l1_simulation_keeps_ethereum_distribution() {
    let mut db = database(bytes!("00"), COLLECTOR);
    db.insert_account_info(
        COLLECTOR,
        AccountInfo {
            balance: U256::MAX,
            ..Default::default()
        },
    );
    let mut evm = EezEvmFactory::for_rollup(COLLECTOR)
        .unwrap()
        .create_evm(db, environment());
    assert!(
        evm.transact_raw(ordinary())
            .unwrap_err()
            .to_string()
            .contains("collector balance overflow")
    );
    evm.db_mut()
        .insert_account_info(COLLECTOR, AccountInfo::default());
    assert!(evm.transact_raw(ordinary()).unwrap().result.is_success());
    let mut l1 = EezEvmFactory::for_l1_simulation()
        .create_evm(database(bytes!("00"), COLLECTOR), environment());
    let output = l1.transact_raw(ordinary()).unwrap();
    assert_eq!(
        output.state[&USER].info.balance,
        U256::from(FUNDS - 13 * 21_000)
    );
    assert_eq!(
        output.state[&BENEFICIARY].info.balance,
        U256::from(3 * 21_000)
    );
    assert!(!output.state.contains_key(&COLLECTOR));
}

#[test]
fn collector_credit_follows_same_transaction_selfdestruct_cleanup() {
    let collector = USER.create(0);
    let mut init = bytes!("600160005573").to_vec();
    init.extend_from_slice(BENEFICIARY.as_slice());
    init.push(0xff);
    let mut db = InMemoryDB::default();
    db.insert_account_info(
        USER,
        AccountInfo {
            balance: U256::from(FUNDS),
            ..Default::default()
        },
    );
    db.insert_account_info(
        collector,
        AccountInfo {
            balance: U256::from(17),
            ..Default::default()
        },
    );
    let mut evm = EezEvmFactory::for_rollup(collector)
        .unwrap()
        .create_evm(db, environment());
    let output = evm
        .transact_raw(TxEnv {
            kind: TxKind::Create,
            data: init.into(),
            gas_limit: 1_000_000,
            ..ordinary()
        })
        .unwrap();
    assert!(output.result.is_success(), "{:?}", output.result);
    let gas = output.result.tx_gas_used();
    evm.db_mut().commit(output.state);
    let recreated = evm.db_mut().basic(collector).unwrap().unwrap();
    assert_eq!(recreated.balance, U256::from(10 * gas));
    assert_eq!(recreated.nonce, 0);
    assert!(recreated.is_empty_code_hash());
    assert_eq!(
        evm.db_mut().storage(collector, U256::ZERO).unwrap(),
        U256::ZERO
    );
    assert_eq!(
        evm.db_mut().basic(BENEFICIARY).unwrap().unwrap().balance,
        U256::from(17 + 3 * gas)
    );
}

#[test]
fn unsigned_zero_price_simulation_does_not_create_collector_funds() {
    let mut env = environment();
    env.cfg_env.disable_base_fee = true;
    let mut evm = EezEvmFactory::for_rollup(COLLECTOR)
        .unwrap()
        .create_evm(database(bytes!("00"), COLLECTOR), env);
    let output = evm
        .transact_raw(TxEnv {
            gas_price: 0,
            ..ordinary()
        })
        .unwrap();
    assert_eq!(output.state[&USER].info.balance, U256::from(FUNDS));
    assert_eq!(
        output
            .state
            .get(&COLLECTOR)
            .map_or(U256::ZERO, |account| account.info.balance),
        U256::ZERO
    );
}

#[test]
fn native_never_checks_or_increments_the_account_nonce_and_ordinary_still_does() {
    for nonce in [0, 37, u64::MAX] {
        for code in [bytes!("00"), bytes!("5f5ffd"), bytes!("fe")] {
            let mut db = database(Bytes::new(), COLLECTOR);
            let code = Bytecode::new_raw(code);
            db.insert_account_info(
                EEZL2_ADDRESS,
                AccountInfo {
                    code_hash: code.hash_slow(),
                    code: Some(code),
                    ..Default::default()
                },
            );
            db.insert_account_info(
                SYSTEM_ADDRESS,
                AccountInfo {
                    nonce,
                    ..Default::default()
                },
            );
            let mut evm = EezEvmFactory::for_rollup(COLLECTOR)
                .unwrap()
                .create_evm(db, environment());
            let native = TxEnv {
                tx_type: SYSTEM_TX_TYPE,
                caller: SYSTEM_ADDRESS,
                kind: TxKind::Call(EEZL2_ADDRESS),
                gas_price: 0,
                value: U256::from(5),
                ..ordinary()
            };
            let output = evm.transact_raw(native).unwrap();
            assert_eq!(output.state[&SYSTEM_ADDRESS].info.nonce, nonce);
            assert_eq!(output.state[&SYSTEM_ADDRESS].info.balance, U256::ZERO);
            assert!(!output.state.contains_key(&COLLECTOR));
            assert!(
                evm.transact_raw(TxEnv {
                    nonce: 1,
                    ..ordinary()
                })
                .is_err()
            );
        }
    }
}

#[test]
fn native_caller_is_empty_during_execution_when_its_nonce_and_balance_are_zero() {
    // Transfer one wei from the newly funded EEZL2 to SYSTEM. The extra 25,000
    // new-account charge proves there was no temporary native nonce increment.
    let mut code = bytes!("5f5f5f5f600173").to_vec();
    code.extend_from_slice(SYSTEM_ADDRESS.as_slice());
    code.extend_from_slice(&bytes!("5af15000"));
    let execute = |nonce| {
        let mut db = InMemoryDB::default();
        let code = Bytecode::new_raw(code.clone().into());
        db.insert_account_info(
            EEZL2_ADDRESS,
            AccountInfo {
                code_hash: code.hash_slow(),
                code: Some(code),
                ..Default::default()
            },
        );
        db.insert_account_info(
            SYSTEM_ADDRESS,
            AccountInfo {
                nonce,
                ..Default::default()
            },
        );
        let mut evm = EezEvmFactory::for_rollup(COLLECTOR)
            .unwrap()
            .create_evm(db, environment());
        let output = evm
            .transact_raw(TxEnv {
                tx_type: SYSTEM_TX_TYPE,
                caller: SYSTEM_ADDRESS,
                kind: TxKind::Call(EEZL2_ADDRESS),
                value: U256::from(13),
                gas_price: 0,
                ..ordinary()
            })
            .unwrap();
        assert!(output.result.is_success());
        assert_eq!(output.state[&SYSTEM_ADDRESS].info.nonce, nonce);
        assert_eq!(output.state[&SYSTEM_ADDRESS].info.balance, U256::ONE);
        output.result.tx_gas_used()
    };
    assert_eq!(execute(0) - execute(1), 25_000);
}
