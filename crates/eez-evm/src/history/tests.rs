use super::*;
use alloy_consensus::{Header, Transaction as _};
use alloy_evm::{
    EvmEnv, EvmFactory,
    block::{BlockExecutor, BlockExecutorFactory},
    eth::EthBlockExecutionCtx,
};
use alloy_primitives::{Address, Bytes, TxKind};
use eez_primitives::SYSTEM_ADDRESS;
use eez_protocol::l1_history::{HeaderChain, HeaderRules, genesis_storage};
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
        bootstrap: eez_protocol::l2::GenesisL1Reference {
            chain_id: 1,
            number: 0,
            hash: bootstrap,
            timestamp: 0,
            prev_randao: B256::ZERO,
        },
        l2_chain_id: 1337,
        source: Arc::new(WitnessHistory(Arc::new(chain))),
    };
    let mut db = InMemoryDB::default();
    let code = Bytecode::new_raw(Bytes::copy_from_slice(HISTORY_RUNTIME));
    db.insert_account_info(
        HISTORY_ADDRESS,
        AccountInfo {
            nonce: 2,
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        },
    );
    let implementation = Bytecode::new_raw(Bytes::copy_from_slice(
        eez_protocol::l1_history::HISTORY_IMPLEMENTATION_RUNTIME,
    ));
    db.insert_account_info(
        eez_protocol::l1_history::HISTORY_IMPLEMENTATION_ADDRESS,
        AccountInfo {
            nonce: 1,
            code_hash: implementation.hash_slow(),
            code: Some(implementation),
            ..Default::default()
        },
    );
    for (key, value) in genesis_storage() {
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
    assert_eq!(
        state.storage(HISTORY_ADDRESS, hash_storage_key(0)).unwrap(),
        U256::ZERO
    );
    for number in 1..=4 {
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
            0 => native.blocks.clear(),
            1 => native.blocks[0].block_hash = B256::repeat_byte(99),
            2 => native.chain_id += 1,
            3 => native.version += 1,
            4 => {
                native.blocks = vec![
                    HistoryBlock {
                        number: 1,
                        block_hash: B256::repeat_byte(1)
                    };
                    65
                ]
            }
            _ => native.blocks[0].block_hash = B256::ZERO,
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
    for (slot, value) in [
        (U256::from(ENDPOINT_SLOT), U256::MAX),
        (U256::from(ENDPOINT_SLOT), U256::from(1)), // no hash at the claimed endpoint
        (hash_storage_key(0), U256::from(1)),       // block zero must stay absent
    ] {
        let mut bad = db.clone();
        bad.insert_account_storage(HISTORY_ADDRESS, slot, value)
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
fn regressing_import_is_rejected_without_committing_any_history_prefix() {
    let (config, mut db) = fixture();
    let (plan, tx) = prepare_at(&mut db, &config, 7).unwrap();
    db.insert_account_storage(HISTORY_ADDRESS, U256::from(ENDPOINT_SLOT), U256::from(2))
        .unwrap();
    let recorded = config.source.headers(4).unwrap().at(2).unwrap().hash();
    db.insert_account_storage(
        HISTORY_ADDRESS,
        hash_storage_key(2),
        U256::from_be_bytes(recorded.0),
    )
    .unwrap();
    let mut state = State::builder()
        .with_database(db)
        .with_bundle_update()
        .build();
    assert!(
        execute(&config, &mut state, 7, plan.prev_randao, &[tx.unwrap()])
            .unwrap_err()
            .to_string()
            .contains("numbers do not increase")
    );
    assert_eq!(
        state.storage(HISTORY_ADDRESS, hash_storage_key(1)).unwrap(),
        U256::ZERO
    );
    assert_eq!(
        state
            .storage(HISTORY_ADDRESS, U256::from(ENDPOINT_SLOT))
            .unwrap(),
        U256::from(2)
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
    let data: Bytes = eez_protocol::l1_history::IL1BlockhashContract::importL1BlocksCall {
        blocks: (1..=64)
            .map(
                |number| eez_protocol::l1_history::IL1BlockhashContract::L1Block {
                    number: u64::from(number),
                    blockHash: B256::with_last_byte(number),
                },
            )
            .collect(),
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
    // The implementation exhausts its delegated gas; the transparent proxy
    // bubbles that failure as an empty revert. No partial writes may survive.
    assert!(matches!(
        result.result,
        revm::context::result::ExecutionResult::Revert { .. }
    ));
    assert!(result.result.output().unwrap().is_empty());
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

#[test]
fn positive_bootstrap_stays_publicly_empty_then_can_be_published_once() {
    let (mut config, db) = fixture();
    let chain = config.source.headers(4).unwrap();
    let reference = chain.at(2).unwrap();
    config.bootstrap.number = 2;
    config.bootstrap.hash = reference.hash();
    config.bootstrap.timestamp = reference.header().timestamp;
    config.bootstrap.prev_randao = reference.header().mix_hash;
    config.schedule = HistorySchedule::new(8, 1, 4, 2).unwrap();
    config.source = Arc::new(WitnessHistory(Arc::new(chain.range(2, 2).unwrap())));
    let mut state = State::builder()
        .with_database(db)
        .with_bundle_update()
        .build();
    // No record is installed by genesis or an omitted opportunity, even though
    // execution authenticates and inherits the positive bootstrap origin.
    execute(&config, &mut state, 9, config.bootstrap.prev_randao, &[]).unwrap();
    assert_eq!(
        state
            .storage(HISTORY_ADDRESS, U256::from(ENDPOINT_SLOT))
            .unwrap(),
        U256::ZERO
    );
    assert_eq!(
        state.storage(HISTORY_ADDRESS, hash_storage_key(2)).unwrap(),
        U256::ZERO
    );
    let (plan, tx) = prepare_at(&mut state, &config, 11).unwrap();
    let tx = tx.unwrap();
    execute(
        &config,
        &mut state,
        11,
        plan.prev_randao,
        std::slice::from_ref(&tx),
    )
    .unwrap();
    assert_eq!(
        state
            .storage(HISTORY_ADDRESS, U256::from(ENDPOINT_SLOT))
            .unwrap(),
        U256::from(2)
    );
    assert_eq!(
        state.storage(HISTORY_ADDRESS, hash_storage_key(2)).unwrap(),
        U256::from_be_bytes(config.bootstrap.hash.0)
    );
    assert!(prepare_at(&mut state, &config, 15).unwrap().1.is_none());
    assert!(
        execute(&config, &mut state, 15, plan.prev_randao, &[tx])
            .unwrap_err()
            .to_string()
            .contains("numbers do not increase")
    );
}

#[test]
fn sparse_recorded_import_authenticates_bridge_but_never_stores_or_backfills_gaps() {
    let (config, db) = fixture();
    let chain = config.source.headers(8).unwrap();
    let tx: EezTxEnvelope = HistoryTransaction {
        version: HISTORY_TX_VERSION,
        chain_id: 1337,
        blocks: [1, 4]
            .map(|number| HistoryBlock {
                number,
                block_hash: chain.at(number).unwrap().hash(),
            })
            .to_vec(),
    }
    .into();
    let mut state = State::builder()
        .with_database(db.clone())
        .with_bundle_update()
        .build();
    let randao = chain.at(4).unwrap().header().mix_hash;
    execute(&config, &mut state, 11, randao, std::slice::from_ref(&tx)).unwrap();
    assert_eq!(
        state
            .storage(HISTORY_ADDRESS, U256::from(ENDPOINT_SLOT))
            .unwrap(),
        U256::from(4)
    );
    for number in [0, 2, 3, 5] {
        assert_eq!(
            state
                .storage(HISTORY_ADDRESS, hash_storage_key(number))
                .unwrap(),
            U256::ZERO
        );
    }
    for number in [1, 4] {
        assert_eq!(
            state
                .storage(HISTORY_ADDRESS, hash_storage_key(number))
                .unwrap(),
            U256::from_be_bytes(chain.at(number).unwrap().hash().0)
        );
    }
    let backfill: EezTxEnvelope = HistoryTransaction {
        version: HISTORY_TX_VERSION,
        chain_id: 1337,
        blocks: vec![HistoryBlock {
            number: 3,
            block_hash: chain.at(3).unwrap().hash(),
        }],
    }
    .into();
    assert!(
        execute(&config, &mut state, 15, randao, &[backfill])
            .unwrap_err()
            .to_string()
            .contains("numbers do not increase")
    );
    let mut incomplete = config.clone();
    incomplete.source = Arc::new(WitnessHistory(Arc::new(chain.range(0, 1).unwrap())));
    let mut missing = State::builder()
        .with_database(db)
        .with_bundle_update()
        .build();
    assert!(
        execute(&incomplete, &mut missing, 11, randao, &[tx])
            .unwrap_err()
            .to_string()
            .contains("outside supplied evidence")
    );
}
