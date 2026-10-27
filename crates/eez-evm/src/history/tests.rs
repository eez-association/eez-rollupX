use super::*;
use alloy_consensus::{Header, Transaction as _};
use alloy_evm::{
    EvmEnv, EvmFactory,
    block::{BlockExecutor, BlockExecutorFactory},
    eth::EthBlockExecutionCtx,
};
use alloy_primitives::{Address, Bytes, TxKind};
use eez_primitives::SYSTEM_ADDRESS;
use eez_protocol::l1_history::{HeaderChain, HeaderRules, bootstrap_storage};
use revm::{
    DatabaseCommit,
    context::{BlockEnv, CfgEnv, TxEnv},
    database::{InMemoryDB, State},
    primitives::hardfork::SpecId,
    state::{AccountInfo, Bytecode},
};

pub(crate) fn fixture() -> (HistoryConfig, InMemoryDB) {
    let schedule = HistorySchedule::new(4, 1, 4, 2).unwrap();
    let mut parent = B256::ZERO;
    let headers: Vec<Bytes> = [0, 1, 4, 5, 8, 9, 12]
        .into_iter()
        .enumerate()
        .map(|(number, timestamp)| {
            let header = Header {
                number: number as u64,
                timestamp,
                parent_hash: parent,
                mix_hash: B256::with_last_byte(number as u8),
                ..Default::default()
            };
            parent = header.hash_slow();
            alloy_rlp::encode(header).into()
        })
        .collect();
    let mut chain = HeaderChain::default();
    chain
        .append(
            &headers,
            &HeaderRules {
                london_block: None,
                shanghai_time: None,
                cancun_time: None,
                prague_time: None,
                unsupported_time: None,
            },
            64,
        )
        .unwrap();
    let bootstrap = chain.at(0).unwrap().hash();
    let config = HistoryConfig {
        schedule,
        first_number: 0,
        l2_chain_id: 1337,
        source: Arc::new(WitnessHistory(Arc::new(chain))),
    };
    let mut db = InMemoryDB::default();
    let code = Bytecode::new_raw(Bytes::copy_from_slice(HISTORY_RUNTIME));
    db.insert_account_info(
        HISTORY_ADDRESS,
        AccountInfo {
            nonce: 1,
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        },
    );
    for (key, value) in bootstrap_storage(0, bootstrap, schedule).unwrap() {
        db.insert_account_storage(HISTORY_ADDRESS, key.into(), value.into())
            .unwrap();
    }
    (config, db)
}

fn prepare_at<DB: Database>(
    db: &mut DB,
    config: &HistoryConfig,
    timestamp: u64,
) -> Result<(HistoryExecution, Option<EezTxEnvelope>), BlockExecutionError> {
    prepare(db, config, timestamp)
}

fn environment(timestamp: u64, randao: B256) -> EvmEnv {
    EvmEnv {
        cfg_env: CfgEnv::new_with_spec(SpecId::OSAKA).with_chain_id(1337),
        block_env: BlockEnv {
            number: U256::from(timestamp - 4),
            timestamp: U256::from(timestamp),
            prevrandao: Some(randao),
            gas_limit: 30_000_000,
            ..Default::default()
        },
    }
}

fn execute(
    config: &HistoryConfig,
    state: &mut State<InMemoryDB>,
    timestamp: u64,
    randao: B256,
    transactions: &[EezTxEnvelope],
) -> Result<alloy_evm::block::BlockExecutionResult<eez_primitives::Receipt>, BlockExecutionError> {
    let spec = Arc::new(
        reth_chainspec::ChainSpecBuilder::mainnet()
            .osaka_activated()
            .build(),
    );
    let mut factory =
        crate::block::EezBlockExecutorFactory::new(spec, crate::block::ExecutionLayer::Rollup);
    factory.history = Some(config.clone());
    let evm = crate::tests::factory().create_evm(state, environment(timestamp, randao));
    let mut executor = factory.create_executor(
        evm,
        EthBlockExecutionCtx {
            parent_hash: B256::repeat_byte(timestamp as u8),
            parent_beacon_block_root: Some(B256::ZERO),
            ommers: &[],
            withdrawals: None,
            extra_data: Bytes::new(),
            tx_count_hint: Some(transactions.len()),
            slot_number: None,
        },
    );
    executor.apply_pre_execution_changes()?;
    for tx in transactions {
        executor.execute_transaction(reth_primitives_traits::Recovered::new_unchecked(
            tx.clone(),
            SYSTEM_ADDRESS,
        ))?;
    }
    executor.finish().map(|(_, output)| output)
}

#[test]
fn recorded_import_precedes_applications_and_meters_gas_receipts_without_a_nonce() {
    let (config, mut db) = fixture();
    let observer = Address::with_last_byte(42);
    let code = Bytecode::new_raw(alloy_primitives::bytes!("4460005500"));
    db.insert_account_info(
        observer,
        AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        },
    );
    db.insert_account_info(
        SYSTEM_ADDRESS,
        AccountInfo {
            nonce: 17,
            balance: U256::from(9),
            ..Default::default()
        },
    );
    let mut state = State::builder()
        .with_database(db)
        .with_bundle_update()
        .build();
    for timestamp in 5..=14 {
        let (plan, tx) = prepare_at(&mut state, &config, timestamp).unwrap();
        let count = usize::from(tx.is_some());
        let output = execute(
            &config,
            &mut state,
            timestamp,
            plan.prev_randao,
            &tx.into_iter().collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(output.receipts.len(), count);
        if count == 1 {
            assert!(
                output.gas_used > 21_000
                    && output.gas_used <= eez_protocol::l1_history::IMPORT_GAS_LIMIT
            );
            assert!(output.receipts[0].success);
            assert_eq!(output.receipts[0].cumulative_gas_used, output.gas_used);
        } else {
            assert_eq!(output.gas_used, 0);
        }
        let number = match timestamp {
            5..=6 => 0,
            7..=10 => 2,
            _ => 4,
        };
        assert_eq!(plan.origin_number, number);
        let mut evm = crate::tests::factory()
            .create_evm(&mut state, environment(timestamp, plan.prev_randao));
        let result = evm
            .transact_raw(TxEnv {
                caller: Address::with_last_byte(100),
                kind: TxKind::Call(observer),
                gas_limit: 100_000,
                nonce: timestamp - 5,
                chain_id: Some(1337),
                ..Default::default()
            })
            .unwrap();
        assert!(result.result.is_success());
        evm.db_mut().commit(result.state);
        assert_eq!(
            evm.db_mut().storage(observer, U256::ZERO).unwrap(),
            U256::from_be_bytes(plan.prev_randao.0)
        );
        assert_eq!(
            evm.db_mut()
                .storage(HISTORY_ADDRESS, U256::from(ENDPOINT_SLOT))
                .unwrap(),
            U256::from(number)
        );
        let system = evm.db_mut().basic(SYSTEM_ADDRESS).unwrap().unwrap();
        assert_eq!(system.nonce, 17);
        assert_eq!(system.balance, U256::ZERO);
    }
    let chain = config.source.headers(8).unwrap();
    for number in 0..=4 {
        assert_eq!(
            state
                .storage(HISTORY_ADDRESS, hash_storage_key(number))
                .unwrap(),
            U256::from_be_bytes(chain.at(number).unwrap().hash().0)
        );
    }
    assert_eq!(
        state.storage(HISTORY_ADDRESS, hash_storage_key(5)).unwrap(),
        U256::ZERO
    );
}

#[test]
fn repeat_is_valid_after_late_headers_arrive_and_advancement_resumes_at_next_update() {
    let (mut config, db) = fixture();
    let full = config.source.headers(4).unwrap();
    config.source = Arc::new(WitnessHistory(Arc::new(full.range(0, 0).unwrap())));
    let mut state = State::builder()
        .with_database(db)
        .with_bundle_update()
        .build();
    let (repeat, tx) = prepare_at(&mut state, &config, 7).unwrap();
    assert_eq!(repeat.origin_number, 0);
    assert!(tx.is_none());
    // Replay gets all headers; the recorded choice remains valid and unchanged.
    config.source = Arc::new(WitnessHistory(full));
    execute(&config, &mut state, 7, B256::ZERO, &[]).unwrap();
    assert_eq!(
        state
            .storage(HISTORY_ADDRESS, U256::from(ENDPOINT_SLOT))
            .unwrap(),
        U256::ZERO
    );
    for timestamp in 8..=10 {
        execute(&config, &mut state, timestamp, B256::ZERO, &[]).unwrap();
    }
    let (advance, tx) = prepare_at(&mut state, &config, 11).unwrap();
    assert_eq!(advance.origin_number, 4);
    execute(&config, &mut state, 11, advance.prev_randao, &[tx.unwrap()]).unwrap();
}

#[test]
fn omitted_updates_inherit_but_duplicate_misplaced_malformed_and_wrong_randao_reject() {
    let (config, mut db) = fixture();
    let (plan, tx) = prepare_at(&mut db, &config, 7).unwrap();
    let tx = tx.unwrap();
    for (timestamp, randao, transactions, reason) in [
        (7, plan.prev_randao, vec![], "PREVRANDAO"),
        (7, B256::ZERO, vec![tx.clone()], "PREVRANDAO"),
        (
            7,
            plan.prev_randao,
            vec![tx.clone(), tx.clone()],
            "misplaced",
        ),
        (5, B256::ZERO, vec![tx.clone()], "off-schedule"),
    ] {
        let mut state = State::builder()
            .with_database(db.clone())
            .with_bundle_update()
            .build();
        assert!(
            execute(&config, &mut state, timestamp, randao, &transactions)
                .unwrap_err()
                .to_string()
                .contains(reason)
        );
    }
    for mutation in 0..6 {
        let EezTxEnvelope::History(native) = tx.clone() else {
            unreachable!()
        };
        let mut native = (*native).clone();
        match mutation {
            0 => native.hashes.clear(),
            1 => native.hashes[0] = B256::repeat_byte(99),
            2 => native.chain_id += 1,
            3 => native.version += 1,
            4 => native.hashes = vec![B256::repeat_byte(1); 65],
            _ => native.hashes[0] = B256::ZERO,
        }
        let mut state = State::builder()
            .with_database(db.clone())
            .with_bundle_update()
            .build();
        assert!(
            execute(&config, &mut state, 7, plan.prev_randao, &[native.into()]).is_err(),
            "mutation {mutation}"
        );
    }
    // Omission is valid even with newer evidence available, at every eligible
    // and ineligible position. Empty blocks still authenticate inherited RANDAO.
    for timestamp in 5..=30 {
        let mut state = State::builder()
            .with_database(db.clone())
            .with_bundle_update()
            .build();
        execute(&config, &mut state, timestamp, B256::ZERO, &[]).unwrap();
        let mut bad = State::builder()
            .with_database(db.clone())
            .with_bundle_update()
            .build();
        assert!(
            execute(&config, &mut bad, timestamp, B256::repeat_byte(99), &[])
                .unwrap_err()
                .to_string()
                .contains("PREVRANDAO")
        );
    }
    assert!(
        tx.input().is_empty(),
        "history has no producer-supplied calldata"
    );
}

#[test]
fn wrong_runtime_state_and_missing_evidence_reject_before_import() {
    let (config, mut db) = fixture();
    let (plan, tx) = prepare_at(&mut db, &config, 7).unwrap();
    for slot in [
        FIRST_NUMBER_SLOT,
        FIRST_CUTOFF_SLOT,
        INTERVAL_SLOT,
        UPDATE_DELAY_SLOT,
        MAX_IMPORT_SLOT,
    ] {
        let mut bad = db.clone();
        bad.insert_account_storage(HISTORY_ADDRESS, U256::from(slot), U256::MAX)
            .unwrap();
        let mut state = State::builder()
            .with_database(bad)
            .with_bundle_update()
            .build();
        assert!(
            execute(
                &config,
                &mut state,
                7,
                plan.prev_randao,
                &[tx.clone().unwrap()]
            )
            .is_err()
        );
    }
    let mut no_code = db.clone();
    no_code.insert_account_info(HISTORY_ADDRESS, AccountInfo::default());
    let mut state = State::builder()
        .with_database(no_code)
        .with_bundle_update()
        .build();
    assert!(
        execute(
            &config,
            &mut state,
            7,
            plan.prev_randao,
            &[tx.clone().unwrap()]
        )
        .unwrap_err()
        .to_string()
        .contains("runtime")
    );
    let mut missing = config.clone();
    missing.source = Arc::new(WitnessHistory(Arc::new(HeaderChain::default())));
    let mut state = State::builder()
        .with_database(db)
        .with_bundle_update()
        .build();
    assert!(execute(&missing, &mut state, 7, plan.prev_randao, &[tx.unwrap()]).is_err());
}

#[test]
fn conflicting_import_reverts_without_committing_any_history_prefix() {
    let (config, mut db) = fixture();
    let (plan, tx) = prepare_at(&mut db, &config, 7).unwrap();
    db.insert_account_storage(HISTORY_ADDRESS, hash_storage_key(2), U256::MAX)
        .unwrap();
    let mut state = State::builder()
        .with_database(db)
        .with_bundle_update()
        .build();
    assert!(
        execute(&config, &mut state, 7, plan.prev_randao, &[tx.unwrap()])
            .unwrap_err()
            .to_string()
            .contains("history transaction failed")
    );
    assert_eq!(
        state.storage(HISTORY_ADDRESS, hash_storage_key(1)).unwrap(),
        U256::ZERO
    );
    assert_eq!(
        state
            .storage(HISTORY_ADDRESS, U256::from(ENDPOINT_SLOT))
            .unwrap(),
        U256::ZERO
    );
    assert_eq!(
        state
            .storage(HISTORY_ADDRESS, U256::from(FIRST_CUTOFF_SLOT))
            .unwrap(),
        U256::from(4)
    );
}

#[test]
fn maximum_import_fits_native_budget_and_invisible_imports_are_rejected() {
    use alloy_sol_types::SolCall as _;
    let (_, mut db) = fixture();
    db.insert_account_info(
        SYSTEM_ADDRESS,
        AccountInfo {
            nonce: 91,
            ..Default::default()
        },
    );
    let data: Bytes = eez_protocol::l1_history::IL1BlockhashContract::importHistoryCall {
        cutoff: 4,
        hashes: (1..=64).map(B256::with_last_byte).collect(),
    }
    .abi_encode()
    .into();
    let mut evm = crate::tests::factory().create_evm(db, environment(7, B256::ZERO));
    let native = TxEnv {
        tx_type: eez_primitives::HISTORY_TX_TYPE,
        caller: SYSTEM_ADDRESS,
        kind: TxKind::Call(HISTORY_ADDRESS),
        gas_limit: eez_protocol::l1_history::IMPORT_GAS_LIMIT,
        nonce: 0,
        chain_id: Some(1337),
        data,
        ..Default::default()
    };
    let result = evm.transact_raw(native.clone()).unwrap();
    assert!(result.result.is_success(), "{:?}", result.result);
    assert!(result.result.tx_gas_used() <= eez_protocol::l1_history::IMPORT_GAS_LIMIT);
    assert_eq!(result.state[&SYSTEM_ADDRESS].info.nonce, 91);
    assert_eq!(
        result.state[&HISTORY_ADDRESS].storage[&U256::from(ENDPOINT_SLOT)].present_value(),
        U256::from(64)
    );
    let result = evm
        .transact_raw(TxEnv {
            gas_limit: 100_000,
            ..native
        })
        .unwrap();
    assert!(result.result.is_halt());
    assert_eq!(
        evm.db_mut()
            .storage(HISTORY_ADDRESS, U256::from(ENDPOINT_SLOT))
            .unwrap(),
        U256::ZERO
    );
    for caller in [SYSTEM_ADDRESS, Address::ZERO] {
        assert!(
            evm.transact_system_call(caller, HISTORY_ADDRESS, Bytes::new())
                .unwrap_err()
                .to_string()
                .contains("recorded native transaction")
        );
    }
}
