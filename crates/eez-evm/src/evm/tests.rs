use super::*;
use alloy_primitives::bytes;
use evm2::{
    Inspector,
    bytecode::Bytecode,
    env::BlockEnv,
    evm::{AccountInfo, DynDatabase, InMemoryDB, TxResultWithState},
    interpreter::Interpreter,
};
use reth_evm::BlockExecutorFactory;
use reth_evm_ethereum::{EthBlockExecutorFactory, EthEvmEnv};
use std::{cell::Cell, rc::Rc, sync::Arc};

pub(crate) fn create_evm(
    factory: EezEvmFactory,
    db: InMemoryDB,
    env: EthEvmEnv<EezEvmTypes>,
) -> crate::EezEvm<'static> {
    crate::EezEvm::new(
        EthBlockExecutorFactory::new_with_receipt_builder(
            crate::EezReceiptBuilder,
            Arc::new(
                reth_chainspec::ChainSpecBuilder::mainnet()
                    .osaka_activated()
                    .build(),
            ),
            factory,
        )
        .evm_with_env(db, env),
    )
}

fn execute(
    evm: &mut crate::EezEvm<'_>,
    tx: EezEvmTx,
    caller: Address,
) -> HandlerResult<TxResultWithState<EezEvmTypes>> {
    evm.transact(&Recovered::new_unchecked(tx, caller))
}

fn ordinary_tx(tx: TxLegacy) -> EezEvmTx {
    EezEvmTx::Ethereum(eth::TxEnvelope::Legacy(tx))
}
fn native_tx(tx: TxLegacy) -> EezEvmTx {
    EezEvmTx::Native {
        ty: SYSTEM_TX_TYPE,
        call: tx,
        access_list: Default::default(),
    }
}

struct CountInspector(Rc<Cell<usize>>);
impl Inspector<EezEvmTypes> for CountInspector {
    fn step(&mut self, _: &mut Interpreter<'_, '_, EezEvmTypes>) {
        self.0.set(self.0.get() + 1);
    }
}

const USER: Address = Address::repeat_byte(0x11);
const TARGET: Address = Address::repeat_byte(0x22);
const BENEFICIARY: Address = Address::repeat_byte(0x33);
const COLLECTOR: Address = Address::repeat_byte(0x44);
const FUNDS: u64 = 100_000_000;

fn environment() -> EthEvmEnv<EezEvmTypes> {
    EthEvmEnv::new(
        SpecId::OSAKA,
        BlockEnv::<EezEvmTypes> {
            number: U256::ONE,
            timestamp: U256::ONE,
            basefee: U256::from(10),
            beneficiary: BENEFICIARY,
            gas_limit: U256::from(30_000_000),
            ..Default::default()
        },
        901,
    )
}

fn database(code: Bytes, collector: Address) -> InMemoryDB {
    let mut db = InMemoryDB::default();
    let code = Bytecode::from_stored(code);
    db.insert_account_info(
        &TARGET,
        AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        },
    );
    db.insert_account_info(
        &USER,
        AccountInfo {
            balance: U256::from(FUNDS),
            ..Default::default()
        },
    );
    if collector != USER && collector != BENEFICIARY {
        // A credit must not invoke the collector's code.
        let code = Bytecode::from_stored(bytes!("fe"));
        db.insert_account_info(
            &collector,
            AccountInfo {
                code_hash: code.hash_slow(),
                code: Some(code),
                ..Default::default()
            },
        );
    }
    db
}

fn ordinary() -> TxLegacy {
    TxLegacy {
        to: TxKind::Call(TARGET),
        gas_limit: 100_000,
        gas_price: 13,
        chain_id: Some(901),
        ..Default::default()
    }
}

fn simulation_environment() -> EthEvmEnv<EezEvmTypes> {
    let mut env = environment();
    disable_fee_charging(&mut env.version);
    env.version
        .features
        .remove(EvmFeatures::NONCE_CHECK | EvmFeatures::EIP3607);
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
                &USER,
                AccountInfo {
                    balance: U256::from(balance),
                    ..Default::default()
                },
            );
            let mut evm = create_evm(
                EezEvmFactory::for_rollup(COLLECTOR).unwrap(),
                db,
                simulation_environment(),
            );
            let tx = TxLegacy {
                value: U256::from(7),
                ..ordinary()
            };
            let result = execute(&mut evm, EezEvmTx::Funded(tx.clone()), USER).unwrap();
            assert_eq!(result.result.is_success(), success);
            assert_eq!(
                result.pending_state.account_info(&USER).unwrap().balance,
                U256::from(balance + returned)
            );
            assert_eq!(
                result.pending_state.account_info(&TARGET).unwrap().balance,
                U256::from(if success { 7 - returned } else { 0 })
            );
            assert!(
                result
                    .pending_state
                    .account_changes_for(&COLLECTOR)
                    .is_none()
            );
            assert!(
                result
                    .pending_state
                    .account_info(&BENEFICIARY)
                    .is_none_or(|account| account.balance.is_zero())
            );
            assert_eq!(
                evm.overlay_db_mut()
                    .get_account(&USER)
                    .unwrap()
                    .unwrap()
                    .balance,
                U256::from(balance)
            );
            // A discarded frame cannot leave credit in the next execution.
            assert_eq!(
                execute(&mut evm, EezEvmTx::Funded(tx), USER).unwrap(),
                result
            );
        }
    }
}

#[test]
fn funded_simulation_rejects_invalid_context_and_clears_credit_on_errors() {
    let factory = EezEvmFactory::for_rollup(COLLECTOR).unwrap();
    let db = database(bytes!("00"), COLLECTOR);
    let mut charged = create_evm(factory, db.clone(), environment());
    assert!(
        execute(&mut charged, EezEvmTx::Funded(ordinary()), USER)
            .unwrap_err()
            .to_string()
            .contains("invalid funded simulation call")
    );
    let mut evm = create_evm(factory, db, simulation_environment());
    let tx = TxLegacy {
        value: U256::from(7),
        ..ordinary()
    };
    for (invalid, caller) in [
        (
            TxLegacy {
                to: TxKind::Create,
                ..tx.clone()
            },
            USER,
        ),
        (tx.clone(), SYSTEM_ADDRESS),
    ] {
        assert!(
            execute(&mut evm, EezEvmTx::Funded(invalid), caller)
                .unwrap_err()
                .to_string()
                .contains("invalid funded simulation call")
        );
    }
    // The funded enum has no caller-controlled type byte. A native envelope with
    // the ordinary sender still reaches and fails native environment validation.
    assert!(
        execute(&mut evm, native_tx(tx.clone()), USER)
            .unwrap_err()
            .to_string()
            .contains("invalid native system transaction environment")
    );
    assert!(
        execute(
            &mut evm,
            EezEvmTx::Funded(TxLegacy {
                chain_id: Some(902),
                ..tx.clone()
            }),
            USER
        )
        .is_err()
    );
    let result = execute(&mut evm, EezEvmTx::Funded(tx.clone()), USER).unwrap();
    assert_eq!(
        result.pending_state.account_info(&USER).unwrap().balance,
        U256::from(FUNDS)
    );
    assert_eq!(
        result.pending_state.account_info(&TARGET).unwrap().balance,
        U256::from(7)
    );
    evm.overlay_db_mut().insert_account_info(
        &USER,
        AccountInfo {
            balance: U256::MAX,
            ..Default::default()
        },
    );
    assert!(
        execute(&mut evm, EezEvmTx::Funded(tx), USER)
            .unwrap_err()
            .to_string()
            .contains("credit balance overflow")
    );
    let zero = execute(&mut evm, EezEvmTx::Funded(ordinary()), USER).unwrap();
    assert_eq!(
        zero.pending_state.account_info(&USER).unwrap().balance,
        U256::MAX
    );
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
                db.insert_account_storage(&TARGET, &U256::ZERO, &U256::ONE);
            }
            let factory = EezEvmFactory::for_rollup(collector).unwrap();
            let mut plain = create_evm(factory, db.clone(), environment());
            let mut scoped = create_evm(factory, db.clone(), environment());
            let mut inspected = create_evm(factory, db, environment());
            let steps = Rc::new(Cell::new(0));
            inspected.set_inspector(CountInspector(steps.clone()));
            let tx = TxLegacy {
                input: data,
                ..ordinary()
            };
            let result = execute(&mut plain, ordinary_tx(tx.clone()), USER).unwrap();
            let scoped_steps = Rc::new(Cell::new(0));
            let mut scoped_inspector = CountInspector(scoped_steps.clone());
            let scoped_result = scoped
                .transact_with_inspector(
                    &Recovered::new_unchecked(ordinary_tx(tx.clone()), USER),
                    &mut scoped_inspector,
                )
                .unwrap();
            assert_eq!(result, scoped_result);
            assert!(scoped_steps.get() > 0);
            assert_eq!(
                result,
                execute(&mut inspected, ordinary_tx(tx), USER).unwrap()
            );
            assert_eq!(result.result.is_success(), success);
            assert!(steps.get() > 0);
            let gas = result.result.tx_gas_used();
            if refund {
                assert!(result.result.refunded > 0);
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
                    result.pending_state.account_info(&address).unwrap().balance,
                    balance,
                    "collector={collector} address={address}"
                );
            }
            assert_eq!(result.pending_state.account_info(&USER).unwrap().nonce, 1);
            // Output is provisional until its owner commits. A discarded preview
            // must not credit again when replayed from the same pre-state.
            assert_eq!(
                execute(
                    &mut plain,
                    ordinary_tx(TxLegacy {
                        input: if floor {
                            Bytes::from(vec![1; 100])
                        } else {
                            Bytes::new()
                        },
                        ..ordinary()
                    }),
                    USER
                )
                .unwrap(),
                result
            );
        }
    }
}

#[test]
fn collector_credit_is_visible_to_the_next_transaction_without_running_its_code() {
    let mut evm = create_evm(
        EezEvmFactory::for_rollup(COLLECTOR).unwrap(),
        database(bytes!("00"), COLLECTOR),
        environment(),
    );
    let first = execute(&mut evm, ordinary_tx(ordinary()), USER).unwrap();
    let credited = U256::from(first.result.tx_gas_used() * 10);
    evm.overlay_db_mut().commit_pending(&first.pending_state);
    // BALANCE(collector), returned as a word.
    let mut raw = vec![0x73];
    raw.extend_from_slice(COLLECTOR.as_slice());
    raw.extend_from_slice(&bytes!("315f5260205ff3"));
    let code = Bytecode::from_stored(raw.into());
    evm.overlay_db_mut().insert_account_info(
        &TARGET,
        AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        },
    );
    let second = execute(
        &mut evm,
        ordinary_tx(TxLegacy {
            nonce: 1,
            ..ordinary()
        }),
        USER,
    )
    .unwrap();
    assert_eq!(U256::from_be_slice(&second.result.output), credited);
    assert_eq!(
        second
            .pending_state
            .account_info(&COLLECTOR)
            .unwrap()
            .balance,
        credited + U256::from(second.result.tx_gas_used() * 10)
    );
}

#[test]
fn collector_overflow_rejects_and_l1_simulation_keeps_ethereum_distribution() {
    let mut db = database(bytes!("00"), COLLECTOR);
    db.insert_account_info(
        &COLLECTOR,
        AccountInfo {
            balance: U256::MAX,
            ..Default::default()
        },
    );
    let mut evm = create_evm(
        EezEvmFactory::for_rollup(COLLECTOR).unwrap(),
        db,
        environment(),
    );
    assert!(
        execute(&mut evm, ordinary_tx(ordinary()), USER)
            .unwrap_err()
            .to_string()
            .contains("collector balance overflow")
    );
    evm.overlay_db_mut()
        .insert_account_info(&COLLECTOR, AccountInfo::default());
    assert!(
        execute(&mut evm, ordinary_tx(ordinary()), USER)
            .unwrap()
            .result
            .is_success()
    );
    let mut l1 = create_evm(
        EezEvmFactory::for_l1_simulation(),
        database(bytes!("00"), COLLECTOR),
        environment(),
    );
    let output = execute(&mut l1, ordinary_tx(ordinary()), USER).unwrap();
    assert_eq!(
        output.pending_state.account_info(&USER).unwrap().balance,
        U256::from(FUNDS - 13 * 21_000)
    );
    assert_eq!(
        output
            .pending_state
            .account_info(&BENEFICIARY)
            .unwrap()
            .balance,
        U256::from(3 * 21_000)
    );
    assert!(
        output
            .pending_state
            .account_changes_for(&COLLECTOR)
            .is_none()
    );
}

#[test]
fn collector_credit_follows_same_transaction_selfdestruct_cleanup() {
    let collector = USER.create(0);
    let mut init = bytes!("600160005573").to_vec();
    init.extend_from_slice(BENEFICIARY.as_slice());
    init.push(0xff);
    let mut db = InMemoryDB::default();
    db.insert_account_info(
        &USER,
        AccountInfo {
            balance: U256::from(FUNDS),
            ..Default::default()
        },
    );
    db.insert_account_info(
        &collector,
        AccountInfo {
            balance: U256::from(17),
            ..Default::default()
        },
    );
    let mut evm = create_evm(
        EezEvmFactory::for_rollup(collector).unwrap(),
        db,
        environment(),
    );
    let output = execute(
        &mut evm,
        ordinary_tx(TxLegacy {
            to: TxKind::Create,
            input: init.into(),
            gas_limit: 1_000_000,
            ..ordinary()
        }),
        USER,
    )
    .unwrap();
    assert!(output.result.is_success(), "{:?}", output.result);
    let gas = output.result.tx_gas_used();
    evm.overlay_db_mut().commit_pending(&output.pending_state);
    let recreated = evm
        .overlay_db_mut()
        .get_account(&collector)
        .unwrap()
        .unwrap();
    assert_eq!(recreated.balance, U256::from(10 * gas));
    assert_eq!(recreated.nonce, 0);
    assert!(recreated.is_empty_code_hash());
    assert_eq!(
        evm.overlay_db_mut()
            .get_storage(&collector, &U256::ZERO)
            .unwrap(),
        U256::ZERO
    );
    assert_eq!(
        evm.overlay_db_mut()
            .get_account(&BENEFICIARY)
            .unwrap()
            .unwrap()
            .balance,
        U256::from(17 + 3 * gas)
    );
}

#[test]
fn unsigned_zero_price_simulation_does_not_create_collector_funds() {
    let mut env = environment();
    env.version.features.remove(EvmFeatures::BASE_FEE_CHECK);
    let mut evm = create_evm(
        EezEvmFactory::for_rollup(COLLECTOR).unwrap(),
        database(bytes!("00"), COLLECTOR),
        env,
    );
    let output = execute(
        &mut evm,
        ordinary_tx(TxLegacy {
            gas_price: 0,
            ..ordinary()
        }),
        USER,
    )
    .unwrap();
    assert_eq!(
        output.pending_state.account_info(&USER).unwrap().balance,
        U256::from(FUNDS)
    );
    assert_eq!(
        output
            .pending_state
            .account_info(&COLLECTOR)
            .map_or(U256::ZERO, |account| account.balance),
        U256::ZERO
    );
}

#[test]
fn native_never_checks_or_increments_the_account_nonce_and_ordinary_still_does() {
    for nonce in [0, 37, u64::MAX] {
        for code in [bytes!("00"), bytes!("5f5ffd"), bytes!("fe")] {
            let mut db = database(Bytes::new(), COLLECTOR);
            let code = Bytecode::from_stored(code);
            db.insert_account_info(
                &EEZL2_ADDRESS,
                AccountInfo {
                    code_hash: code.hash_slow(),
                    code: Some(code),
                    ..Default::default()
                },
            );
            db.insert_account_info(
                &SYSTEM_ADDRESS,
                AccountInfo {
                    nonce,
                    ..Default::default()
                },
            );
            let mut evm = create_evm(
                EezEvmFactory::for_rollup(COLLECTOR).unwrap(),
                db,
                environment(),
            );
            let native = TxLegacy {
                to: TxKind::Call(EEZL2_ADDRESS),
                gas_price: 0,
                value: U256::from(5),
                ..ordinary()
            };
            let output = execute(&mut evm, native_tx(native), SYSTEM_ADDRESS).unwrap();
            // Evm2 finalizes EIP-161 deletion before detaching; an explicitly
            // deleted empty caller has the same zero nonce/balance after commit.
            let change = output
                .pending_state
                .account_changes_for(&SYSTEM_ADDRESS)
                .unwrap();
            let system = change.current.cloned().unwrap_or_default();
            assert_eq!(system.nonce, nonce);
            assert_eq!(system.balance, U256::ZERO);
            assert_eq!(change.current.is_none(), nonce == 0);
            assert!(
                output
                    .pending_state
                    .account_changes_for(&COLLECTOR)
                    .is_none()
            );
            assert!(
                execute(
                    &mut evm,
                    ordinary_tx(TxLegacy {
                        nonce: 1,
                        ..ordinary()
                    }),
                    USER
                )
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
        let code = Bytecode::from_stored(code.clone().into());
        db.insert_account_info(
            &EEZL2_ADDRESS,
            AccountInfo {
                code_hash: code.hash_slow(),
                code: Some(code),
                ..Default::default()
            },
        );
        db.insert_account_info(
            &SYSTEM_ADDRESS,
            AccountInfo {
                nonce,
                ..Default::default()
            },
        );
        let mut evm = create_evm(
            EezEvmFactory::for_rollup(COLLECTOR).unwrap(),
            db,
            environment(),
        );
        let output = execute(
            &mut evm,
            native_tx(TxLegacy {
                to: TxKind::Call(EEZL2_ADDRESS),
                value: U256::from(13),
                gas_price: 0,
                ..ordinary()
            }),
            SYSTEM_ADDRESS,
        )
        .unwrap();
        assert!(output.result.is_success());
        assert_eq!(
            output
                .pending_state
                .account_info(&SYSTEM_ADDRESS)
                .unwrap()
                .nonce,
            nonce
        );
        assert_eq!(
            output
                .pending_state
                .account_info(&SYSTEM_ADDRESS)
                .unwrap()
                .balance,
            U256::ONE
        );
        output.result.tx_gas_used()
    };
    assert_eq!(execute(0) - execute(1), 25_000);
}

#[test]
fn native_post_credit_code_rejection_discards_mint_before_the_next_call() {
    let mut db = database(bytes!("00"), COLLECTOR);
    db.insert_account_info(
        &SYSTEM_ADDRESS,
        AccountInfo::empty()
            .with_balance(U256::from(19))
            .with_code(Bytecode::from_stored(bytes!("00"))),
    );
    let mut evm = create_evm(
        EezEvmFactory::for_rollup(COLLECTOR).unwrap(),
        db,
        environment(),
    );
    let error = execute(
        &mut evm,
        native_tx(TxLegacy {
            to: TxKind::Call(EEZL2_ADDRESS),
            gas_price: 0,
            value: U256::from(7),
            ..ordinary()
        }),
        SYSTEM_ADDRESS,
    )
    .unwrap_err();
    assert_eq!(error, HandlerError::RejectCallerWithCode);
    let after = execute(
        &mut evm,
        ordinary_tx(TxLegacy {
            to: TxKind::Call(SYSTEM_ADDRESS),
            value: U256::ZERO,
            ..ordinary()
        }),
        USER,
    )
    .unwrap();
    assert!(after.result.is_success());
    assert_eq!(
        after
            .pending_state
            .account_info(&SYSTEM_ADDRESS)
            .unwrap()
            .balance,
        U256::from(19)
    );
    assert_eq!(
        evm.overlay_db_mut()
            .get_account(&SYSTEM_ADDRESS)
            .unwrap()
            .unwrap()
            .balance,
        U256::from(19)
    );
}

#[test]
fn uncharged_ordinary_call_does_not_synthesize_value_funds() {
    let mut db = database(bytes!("00"), COLLECTOR);
    db.insert_account_info(&USER, AccountInfo::empty().with_balance(U256::from(3)));
    let mut evm = create_evm(
        EezEvmFactory::for_rollup(COLLECTOR).unwrap(),
        db,
        simulation_environment(),
    );
    let result = execute(
        &mut evm,
        ordinary_tx(TxLegacy {
            value: U256::from(7),
            ..ordinary()
        }),
        USER,
    )
    .unwrap();
    assert!(!result.result.is_success());
    assert_eq!(
        result.pending_state.account_info(&USER).unwrap().balance,
        U256::from(3)
    );
    assert_eq!(
        result.pending_state.account_info(&TARGET).unwrap().balance,
        U256::ZERO
    );
}

#[test]
fn charged_simulation_tops_up_effective_cost_without_fee_cap_surplus() {
    for initial in [3u64, 480_001, 1_000_000, 10_000_000] {
        for dynamic_fee in [false, true] {
            for reverts in [false, true] {
                let mut db = database(
                    if reverts {
                        bytes!("5f5ffd")
                    } else {
                        bytes!("00")
                    },
                    COLLECTOR,
                );
                db.insert_account_info(
                    &USER,
                    AccountInfo::empty().with_balance(U256::from(initial)),
                );
                let mut env = environment();
                env.version.features.remove(EvmFeatures::BALANCE_CHECK);
                let mut evm = create_evm(EezEvmFactory::for_rollup(COLLECTOR).unwrap(), db, env);
                let mut expected = initial;
                for nonce in 0..2 {
                    let tx = if dynamic_fee {
                        EezEvmTx::Ethereum(eth::TxEnvelope::Eip1559(alloy_consensus::TxEip1559 {
                            chain_id: 901,
                            nonce,
                            gas_limit: 40_000,
                            max_fee_per_gas: 100,
                            max_priority_fee_per_gas: 2,
                            to: TxKind::Call(TARGET),
                            value: U256::from(7),
                            ..Default::default()
                        }))
                    } else {
                        ordinary_tx(TxLegacy {
                            nonce,
                            gas_limit: 40_000,
                            gas_price: 12,
                            value: U256::from(7),
                            ..ordinary()
                        })
                    };
                    let result = execute(&mut evm, tx, USER).unwrap();
                    assert_eq!(result.result.is_success(), !reverts);
                    let gas_used = if reverts { 21_004 } else { 21_000 };
                    assert_eq!(result.result.tx_gas_used(), gas_used);
                    // Pinned REVM charges effective price 12 and floors at value before execution.
                    // Revert preserves that charge, refunds unused gas, and rolls back the transfer.
                    expected = expected.saturating_sub(480_000).max(7)
                        - if reverts { 0 } else { 7 }
                        + (40_000 - gas_used) * 12;
                    assert_eq!(
                        result.pending_state.account_info(&USER).unwrap().balance,
                        U256::from(expected),
                        "initial={initial}, dynamic_fee={dynamic_fee}, reverts={reverts}, nonce={nonce}"
                    );
                    assert_eq!(
                        result.pending_state.account_info(&TARGET).unwrap().balance,
                        U256::from(if reverts { 0 } else { (nonce + 1) * 7 })
                    );
                    evm.overlay_db_mut().commit_pending(&result.pending_state);
                }
            }
        }
    }
}

#[test]
fn reth_entrypoints_apply_the_same_credit_once_and_discard_stays_provisional() {
    use evm2::evm::inspector::NoopInspector;
    use reth_evm::Evm as RethEvm;
    for collector in [COLLECTOR, USER, BENEFICIARY] {
        for tx in [
            ordinary_tx(ordinary()),
            EezEvmTx::Ethereum(eth::TxEnvelope::Eip2930(alloy_consensus::TxEip2930 {
                chain_id: 901,
                gas_price: 13,
                gas_limit: 100_000,
                to: TxKind::Call(TARGET),
                ..Default::default()
            })),
            EezEvmTx::Ethereum(eth::TxEnvelope::Eip1559(alloy_consensus::TxEip1559 {
                chain_id: 901,
                max_fee_per_gas: 13,
                max_priority_fee_per_gas: 3,
                gas_limit: 100_000,
                to: TxKind::Call(TARGET),
                ..Default::default()
            })),
        ] {
            let tx = Recovered::new_unchecked(tx, USER);
            let create = || {
                create_evm(
                    EezEvmFactory::for_rollup(collector).unwrap(),
                    database(bytes!("00"), collector),
                    environment(),
                )
            };
            let expected = create().transact(&tx).unwrap();
            assert_eq!(RethEvm::transact(&mut create(), &tx).unwrap(), expected);
            let (_, inspected) =
                RethEvm::transact_with_inspector(&mut create(), &tx, NoopInspector::default())
                    .unwrap();
            assert_eq!(inspected, expected);
            let mut committed = create();
            assert_eq!(
                RethEvm::transact_commit(&mut committed, &tx).unwrap(),
                expected.result
            );
            assert_eq!(
                committed
                    .overlay_db_mut()
                    .get_account(&collector)
                    .unwrap()
                    .as_ref(),
                expected.pending_state.account_info(&collector)
            );
            let mut provisional = create();
            let before = provisional
                .overlay_db_mut()
                .get_account(&collector)
                .unwrap();
            assert_eq!(
                RethEvm::transact_result(&mut provisional, &tx).unwrap(),
                expected.result
            );
            assert_eq!(
                provisional
                    .overlay_db_mut()
                    .get_account(&collector)
                    .unwrap(),
                before
            );
            RethEvm::transact_and_discard(&mut provisional, &tx, |executed| {
                assert_eq!(executed.result(), &expected.result);
                assert_eq!(
                    executed.account_changes().count(),
                    expected.pending_state.account_changes().count()
                );
                for a in executed.account_changes() {
                    let b = expected
                        .pending_state
                        .account_changes_for(&a.address)
                        .unwrap();
                    assert_eq!(
                        (
                            a.original,
                            a.current,
                            a.created,
                            a.selfdestructed,
                            a.touched,
                            a.storage
                        ),
                        (
                            b.original,
                            b.current,
                            b.created,
                            b.selfdestructed,
                            b.touched,
                            b.storage
                        )
                    );
                }
            })
            .unwrap();
            assert_eq!(
                provisional
                    .overlay_db_mut()
                    .get_account(&collector)
                    .unwrap(),
                before
            );
        }
    }
}

#[test]
fn accepted_osaka_profile_has_the_finalization_features_used_by_cleanup() {
    let evm = create_evm(
        EezEvmFactory::for_rollup(COLLECTOR).unwrap(),
        database(bytes!("00"), COLLECTOR),
        environment(),
    );
    let raw = evm.into_inner();
    assert_eq!(raw.spec_id(), SpecId::OSAKA);
    assert!(raw.feature(EvmFeatures::EIP161));
    assert!(!raw.feature(EvmFeatures::EIP8246));
}
