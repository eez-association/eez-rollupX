use super::*;
use alloy_consensus::Header;
use alloy_evm::{
    EvmEnv, EvmFactory,
    block::{BlockExecutor, BlockExecutorFactory},
    eth::EthBlockExecutionCtx,
};
use alloy_primitives::{Address, Bytes, TxKind};
use eez_protocol::l1_history::{HeaderChain, HeaderRules, bootstrap_storage};
use revm::{
    Database, DatabaseCommit,
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

#[test]
fn delayed_import_runs_before_opcode_and_retains_unused_headers_without_transaction_gas() {
    let (config, mut db) = fixture();
    let observer = Address::with_last_byte(42);
    let observer_code = Bytecode::new_raw(alloy_primitives::bytes!("4460005500"));
    db.insert_account_info(
        observer,
        AccountInfo {
            code_hash: observer_code.hash_slow(),
            code: Some(observer_code),
            ..Default::default()
        },
    );
    db.insert_account_info(
        eez_primitives::SYSTEM_ADDRESS,
        AccountInfo {
            nonce: 17,
            balance: U256::from(9),
            ..Default::default()
        },
    );
    let spec = Arc::new(
        reth_chainspec::ChainSpecBuilder::mainnet()
            .osaka_activated()
            .build(),
    );
    let mut factory =
        crate::block::EezBlockExecutorFactory::new(spec, crate::block::ExecutionLayer::Rollup);
    factory.history = Some(config.clone());
    let mut state = State::builder()
        .with_database(db)
        .with_bundle_update()
        .build();
    for timestamp in 5..=14 {
        let number = match timestamp {
            5..=6 => 0,
            7..=10 => 2,
            _ => 4,
        };
        let randao = B256::with_last_byte(number);
        let evm = factory
            .evm_factory()
            .create_evm(&mut state, environment(timestamp, randao));
        let mut executor = factory.create_executor(
            evm,
            EthBlockExecutionCtx {
                parent_hash: B256::repeat_byte(timestamp as u8),
                parent_beacon_block_root: Some(B256::ZERO),
                ommers: &[],
                withdrawals: None,
                extra_data: Bytes::new(),
                tx_count_hint: Some(0),
                slot_number: None,
            },
        );
        executor.apply_pre_execution_changes().unwrap();
        let (mut evm, output) = executor.finish().unwrap();
        assert_eq!(output.gas_used, 0);
        assert!(output.receipts.is_empty());
        // An ordinary application's opcode sees precisely the selected header.
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
            U256::from_be_bytes(randao.0)
        );
        assert_eq!(
            evm.db_mut()
                .storage(HISTORY_ADDRESS, U256::from(ENDPOINT_SLOT))
                .unwrap(),
            U256::from(number)
        );
        let system = evm
            .db_mut()
            .basic(eez_primitives::SYSTEM_ADDRESS)
            .unwrap()
            .unwrap();
        assert_eq!(system.nonce, 17);
        assert_eq!(system.balance, U256::ZERO); // Existing balance cleanup precedes import.
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
fn wrong_randao_code_state_missing_evidence_and_duplicate_import_fail() {
    let (config, db) = fixture();
    let mut wrong_randao = crate::EezEvmFactory.create_evm(db.clone(), environment(7, B256::ZERO));
    assert!(
        apply(&mut wrong_randao, &config)
            .unwrap_err()
            .to_string()
            .contains("PREVRANDAO")
    );
    for slot in [
        FIRST_NUMBER_SLOT,
        NEXT_CUTOFF_SLOT,
        INTERVAL_SLOT,
        UPDATE_DELAY_SLOT,
        MAX_IMPORT_SLOT,
    ] {
        let mut bad = db.clone();
        bad.insert_account_storage(HISTORY_ADDRESS, U256::from(slot), U256::MAX)
            .unwrap();
        let mut evm = crate::EezEvmFactory.create_evm(bad, environment(7, B256::with_last_byte(2)));
        assert!(apply(&mut evm, &config).is_err());
    }
    let mut no_code = db.clone();
    no_code.insert_account_info(HISTORY_ADDRESS, AccountInfo::default());
    let mut evm = crate::EezEvmFactory.create_evm(no_code, environment(7, B256::with_last_byte(2)));
    assert!(
        apply(&mut evm, &config)
            .unwrap_err()
            .to_string()
            .contains("runtime")
    );
    let mut missing = config.clone();
    missing.source = Arc::new(WitnessHistory(Arc::new(HeaderChain::default())));
    let mut evm =
        crate::EezEvmFactory.create_evm(db.clone(), environment(7, B256::with_last_byte(2)));
    assert!(apply(&mut evm, &missing).is_err());
    apply(&mut evm, &config).unwrap();
    assert!(
        apply(&mut evm, &config)
            .unwrap_err()
            .to_string()
            .contains("parent L1 history")
    );
}

#[test]
fn conflicting_import_reverts_without_committing_any_prefix() {
    let (config, mut db) = fixture();
    db.insert_account_storage(HISTORY_ADDRESS, hash_storage_key(2), U256::MAX)
        .unwrap();
    let mut evm = crate::EezEvmFactory.create_evm(db, environment(7, B256::with_last_byte(2)));
    assert!(
        apply(&mut evm, &config)
            .unwrap_err()
            .to_string()
            .contains("reverted")
    );
    assert_eq!(
        evm.db_mut()
            .storage(HISTORY_ADDRESS, hash_storage_key(1))
            .unwrap(),
        U256::ZERO
    );
    assert_eq!(
        evm.db_mut()
            .storage(HISTORY_ADDRESS, U256::from(ENDPOINT_SLOT))
            .unwrap(),
        U256::ZERO
    );
    assert_eq!(
        evm.db_mut()
            .storage(HISTORY_ADDRESS, U256::from(NEXT_CUTOFF_SLOT))
            .unwrap(),
        U256::from(4)
    );
}
