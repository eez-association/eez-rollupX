//! Integration of the real Reth 2.11 factory/block interfaces with the accepted RollupX profile.
use crate::{EezEvmConfig, EezEvmTx, history::WitnessHistory};
use alloy_consensus::{Header, SignableTransaction, TxLegacy, transaction::Recovered};
use alloy_primitives::{Address, B256, Bytes, Signature, TxKind, U256, bytes};
use eez_primitives::{
    EEZL2_ADDRESS, EezTxEnvelope, HISTORY_ADDRESS, SYSTEM_ADDRESS, SystemTransaction,
};
use eez_protocol::{
    l1_history::{HeaderChain, HeaderRules},
    system_tx::LOAD_EXECUTION_TABLE_SELECTOR,
};
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, DynDatabase, InMemoryDB},
};
use reth_chainspec::EthChainSpec;
use reth_evm::{
    BlockExecutor, BlockExecutorFactory, ConfigureEvm, NextBlockEnvAttributes, TransactionEnvMut,
};
use std::sync::Arc;
const USER: Address = Address::repeat_byte(0x11);
const COLLECTOR: Address = Address::repeat_byte(0x44);
const OBSERVER: Address = Address::repeat_byte(0x55);

fn fixture(manager: Bytes) -> (EezEvmConfig, InMemoryDB) {
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
    let mut genesis: alloy_genesis::Genesis =
        serde_json::from_slice(include_bytes!("../../../genesis.json")).unwrap();
    genesis.timestamp = 4;
    genesis.config.chain_id = 901;
    let profile = genesis.config.extra_fields.get_mut("rollupX").unwrap();
    profile["l1Reference"]["chainId"] = serde_json::json!(1337);
    profile["l1Reference"]["timestamp"] = serde_json::json!(0);
    profile["l1Reference"]["hash"] = serde_json::json!(chain.at(0).unwrap().hash());
    profile["l1Reference"]["number"] = serde_json::json!(0);
    profile["l1Reference"]["prevRandao"] = serde_json::json!(B256::ZERO);
    profile["blockIntervalSeconds"] = serde_json::json!(1);
    profile["syncIntervalBlocks"] = serde_json::json!(4);
    profile["feeCollector"] = serde_json::json!(COLLECTOR);
    profile["historyUpdateOffset"] = serde_json::json!(2);
    genesis.alloc.get_mut(&HISTORY_ADDRESS).unwrap().storage =
        Some(eez_protocol::l1_history::genesis_storage());
    let mut db = InMemoryDB::default();
    for (address, account) in &genesis.alloc {
        let info = AccountInfo::empty()
            .with_nonce(account.nonce.unwrap_or_default())
            .with_balance(account.balance)
            .with_code(Bytecode::from_stored(
                account.code.clone().unwrap_or_default(),
            ));
        db.insert_account_info(address, info);
        if let Some(storage) = &account.storage {
            for (key, value) in storage {
                db.insert_account_storage(
                    address,
                    &U256::from_be_bytes(key.0),
                    &U256::from_be_bytes(value.0),
                );
            }
        }
    }
    db.insert_account_info(
        &EEZL2_ADDRESS,
        AccountInfo::empty()
            .with_nonce(1)
            .with_code(Bytecode::from_stored(manager)),
    );
    db.insert_account_info(
        &USER,
        AccountInfo::empty().with_balance(U256::from(1_000_000_000_000_000_000u64)),
    );
    db.insert_account_info(
        &SYSTEM_ADDRESS,
        AccountInfo::empty()
            .with_nonce(37)
            .with_balance(U256::from(9)),
    );
    let config = EezEvmConfig::new(Arc::new(crate::profile::chain_spec(genesis).unwrap()))
        .with_l1_history(Arc::new(WitnessHistory(Arc::new(chain))))
        .unwrap();
    (config, db)
}
fn parent(config: &EezEvmConfig, timestamp: u64) -> Header {
    let mut parent = config.chain_spec().genesis_header().clone();
    parent.number = timestamp - 5;
    parent.timestamp = timestamp - 1;
    parent.mix_hash = B256::ZERO;
    parent
}
fn executor(
    config: &EezEvmConfig,
    db: InMemoryDB,
    timestamp: u64,
    randao: B256,
) -> crate::block::EezBlockExecutor<'_> {
    let parent = parent(config, timestamp);
    let attrs = NextBlockEnvAttributes {
        timestamp,
        suggested_fee_recipient: Address::ZERO,
        prev_randao: randao,
        gas_limit: eez_protocol::l2::GAS_LIMIT,
        parent_beacon_block_root: Some(B256::ZERO),
        withdrawals: Some(Default::default()),
        extra_data: Bytes::new(),
        slot_number: None,
    };
    let mut env = config.next_evm_env(&parent, &attrs).unwrap();
    env.block.basefee = U256::from(10);
    let evm = config.block_executor_factory().evm_with_env(db, env);
    config.block_executor_factory().create_executor(
        evm,
        reth_evm_ethereum::EthBlockExecutionCtx {
            tx_count_hint: None,
            parent_hash: parent.hash_slow(),
            parent_beacon_block_root: Some(B256::ZERO),
            ommers: &[],
            withdrawals: Some(std::borrow::Cow::Borrowed(&[])),
            extra_data: Bytes::new(),
            slot_number: None,
        },
    )
}
fn ordinary(nonce: u64, target: Address, value: u64) -> Recovered<EezTxEnvelope> {
    let tx = TxLegacy {
        chain_id: Some(901),
        nonce,
        gas_limit: 200_000,
        gas_price: 13,
        to: TxKind::Call(target),
        value: U256::from(value),
        ..Default::default()
    };
    Recovered::new_unchecked(
        EezTxEnvelope::from(tx.into_signed(Signature::new(U256::ONE, U256::ONE, false))),
        USER,
    )
}
fn load(value: u64) -> Recovered<EezTxEnvelope> {
    Recovered::new_unchecked(
        SystemTransaction {
            chain_id: 901,
            source_hash: B256::repeat_byte(1),
            gas_limit: eez_primitives::SYSTEM_TX_GAS_CAP,
            to: EEZL2_ADDRESS,
            value: U256::from(value),
            input: Bytes::copy_from_slice(&LOAD_EXECUTION_TABLE_SELECTOR),
        }
        .into(),
        SYSTEM_ADDRESS,
    )
}
fn balance(executor: &mut crate::block::EezBlockExecutor<'_>, address: Address) -> U256 {
    executor
        .evm_mut()
        .overlay_db_mut()
        .get_account(&address)
        .unwrap()
        .map_or(U256::ZERO, |a| a.balance)
}

#[test]
fn group_preview_discard_and_commit_keep_cursor_cleanup_and_storage_together() {
    let (config, db) = fixture(bytes!("4760005500"));
    let mut executor = executor(&config, db, 5, B256::ZERO);
    executor.apply_pre_execution_changes().unwrap();
    let preview = executor
        .execute_transaction_without_commit(load(29))
        .unwrap();
    assert!(executor.group_state().ensure_complete().is_ok());
    drop(preview);
    assert_eq!(balance(&mut executor, EEZL2_ADDRESS), U256::ZERO);
    executor.execute_transaction(load(29)).unwrap();
    assert!(executor.validate_prefix().is_err());
    assert_eq!(balance(&mut executor, EEZL2_ADDRESS), U256::from(29));
    let pending = executor
        .execute_transaction_without_commit(ordinary(0, EEZL2_ADDRESS, 3))
        .unwrap();
    assert!(executor.group_state().ensure_complete().is_err());
    assert!(executor.execution_checkpoint().is_err());
    drop(pending);
    assert_eq!(balance(&mut executor, EEZL2_ADDRESS), U256::from(29));
    executor
        .execute_transaction(ordinary(0, EEZL2_ADDRESS, 3))
        .unwrap();
    executor.validate_prefix().unwrap();
    for address in [SYSTEM_ADDRESS, EEZL2_ADDRESS] {
        assert_eq!(balance(&mut executor, address), U256::ZERO);
    }
    assert_eq!(
        executor
            .evm_mut()
            .overlay_db_mut()
            .get_storage(&EEZL2_ADDRESS, &U256::ZERO)
            .unwrap(),
        U256::from(32)
    );
    let checkpoint = executor.block_state().clone().into_bundle();
    assert_eq!(
        checkpoint
            .state
            .get(&SYSTEM_ADDRESS)
            .unwrap()
            .info
            .as_ref()
            .unwrap()
            .nonce,
        37
    );
    assert_eq!(
        checkpoint
            .state
            .get(&EEZL2_ADDRESS)
            .unwrap()
            .info
            .as_ref()
            .unwrap()
            .balance,
        U256::ZERO
    );
    let output = executor.finish().unwrap();
    assert_eq!(output.result.receipts.len(), 2);
    assert_eq!(
        u8::from(output.result.receipts[0].tx_type),
        eez_primitives::SYSTEM_TX_TYPE
    );
    assert_eq!(output.state.state, checkpoint.state);
}

#[test]
fn standalone_donation_stays_visible_until_terminal_accumulator_cleanup() {
    let (config, db) = fixture(bytes!("4760005500"));
    let mut executor = executor(&config, db, 5, B256::ZERO);
    executor.apply_pre_execution_changes().unwrap();
    executor
        .execute_transaction(ordinary(0, EEZL2_ADDRESS, 29))
        .unwrap();
    assert_eq!(balance(&mut executor, EEZL2_ADDRESS), U256::from(29));
    let before = executor.block_state().clone().into_bundle();
    assert_eq!(
        before
            .state
            .get(&EEZL2_ADDRESS)
            .unwrap()
            .info
            .as_ref()
            .unwrap()
            .balance,
        U256::from(29)
    );
    let output = executor.finish().unwrap();
    assert_eq!(
        output
            .state
            .state
            .get(&EEZL2_ADDRESS)
            .unwrap()
            .info
            .as_ref()
            .unwrap()
            .balance,
        U256::ZERO
    );
    assert_eq!(
        output
            .state
            .state
            .get(&EEZL2_ADDRESS)
            .unwrap()
            .storage
            .get(&U256::ZERO)
            .unwrap()
            .present_value(),
        U256::from(29)
    );
    assert_eq!(output.result.receipts.len(), 1);
}

#[test]
fn real_history_import_is_recorded_metered_and_rejects_duplicates() {
    let (config, mut db) = fixture(bytes!("00"));
    db.insert_account_info(
        &OBSERVER,
        AccountInfo::empty().with_code(Bytecode::from_stored(bytes!("4460005500"))),
    );
    let (plan, tx) = config
        .prepare_history(&mut db, &parent(&config, 7), 7)
        .unwrap();
    let tx = tx.expect("position seven must record the authenticated history update");
    let mut executor = executor(&config, db, 7, plan.prev_randao);
    executor.apply_pre_execution_changes().unwrap();
    let recovered = Recovered::new_unchecked(tx, SYSTEM_ADDRESS);
    executor.execute_transaction(recovered.clone()).unwrap();
    assert_eq!(executor.receipts().len(), 1);
    assert!(executor.receipts()[0].success);
    assert!(executor.receipts()[0].cumulative_gas_used > 21_000);
    assert_eq!(
        u8::from(executor.receipts()[0].tx_type),
        eez_primitives::HISTORY_TX_TYPE
    );
    assert_eq!(
        executor
            .evm_mut()
            .overlay_db_mut()
            .get_account(&SYSTEM_ADDRESS)
            .unwrap()
            .unwrap()
            .nonce,
        37
    );
    let state = config
        .history_state(executor.evm_mut().overlay_db_mut())
        .unwrap();
    assert!(state.imported);
    assert!(
        executor
            .execute_transaction(recovered)
            .unwrap_err()
            .to_string()
            .contains("duplicate/misplaced history transaction")
    );
    executor
        .execute_transaction(ordinary(0, OBSERVER, 0))
        .unwrap();
    assert_eq!(
        executor
            .evm_mut()
            .overlay_db_mut()
            .get_storage(&OBSERVER, &U256::ZERO)
            .unwrap(),
        U256::from_be_bytes(plan.prev_randao.0)
    );
    let output = executor.finish().unwrap();
    assert_eq!(output.result.receipts.len(), 2);
    assert!(output.state.state.contains_key(&HISTORY_ADDRESS));
}

#[test]
fn native_access_list_mutation_is_rejected_instead_of_silently_ignored() {
    let (config, db) = fixture(bytes!("00"));
    let mut executor = executor(&config, db, 5, B256::ZERO);
    executor.apply_pre_execution_changes().unwrap();
    let mut tx = EezEvmTx::from(load(3).into_inner());
    tx.set_access_list(alloy_eips::eip2930::AccessList(vec![
        alloy_eips::eip2930::AccessListItem {
            address: EEZL2_ADDRESS,
            storage_keys: vec![],
        },
    ]));
    let err = executor
        .evm_mut()
        .transact(&Recovered::new_unchecked(tx, SYSTEM_ADDRESS))
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid native system transaction environment"
    );
    assert_eq!(balance(&mut executor, SYSTEM_ADDRESS), U256::from(9));
    assert_eq!(balance(&mut executor, EEZL2_ADDRESS), U256::ZERO);
}

#[test]
fn eligible_history_omission_inherits_randomness_and_wrong_empty_header_is_rejected() {
    let (config, db) = fixture(bytes!("00"));
    let mut omitted = executor(&config, db.clone(), 7, B256::ZERO);
    omitted.apply_pre_execution_changes().unwrap();
    let output = omitted.finish().unwrap();
    assert!(output.result.receipts.is_empty());
    assert_eq!(output.result.gas_used, 0);
    let mut wrong = executor(&config, db, 7, B256::repeat_byte(91));
    wrong.apply_pre_execution_changes().unwrap();
    let error = wrong.finish().unwrap_err();
    assert_eq!(
        error.to_string(),
        "empty block PREVRANDAO differs from inherited L1 origin"
    );
}

#[test]
fn invalid_history_evidence_cannot_write_or_consume_a_receipt() {
    let (config, mut db) = fixture(bytes!("00"));
    let (plan, tx) = config
        .prepare_history(&mut db, &parent(&config, 7), 7)
        .unwrap();
    let EezTxEnvelope::History(history) = tx.unwrap() else {
        unreachable!()
    };
    let mut history = history.into_inner();
    history.blocks[0].block_hash = B256::repeat_byte(71);
    let tx = EezTxEnvelope::from(history);
    let mut executor = executor(&config, db, 7, plan.prev_randao);
    executor.apply_pre_execution_changes().unwrap();
    assert!(
        executor
            .execute_transaction(Recovered::new_unchecked(tx, SYSTEM_ADDRESS))
            .unwrap_err()
            .to_string()
            .contains("history transaction differs from authenticated headers")
    );
    assert!(executor.receipts().is_empty());
    assert!(
        !config
            .history_state(executor.evm_mut().overlay_db_mut())
            .unwrap()
            .imported
    );
    assert_eq!(balance(&mut executor, SYSTEM_ADDRESS), U256::from(9));
}

#[test]
fn native_gas_is_bound_to_remaining_position_and_intrinsic_failure_does_not_mint() {
    let (config, db) = fixture(bytes!("fe"));
    let mut executor = executor(&config, db, 5, B256::ZERO);
    executor.apply_pre_execution_changes().unwrap();
    for nonce in 0..2 {
        let tx = TxLegacy {
            chain_id: Some(901),
            nonce,
            gas_limit: 14_999_999,
            gas_price: 13,
            to: TxKind::Call(EEZL2_ADDRESS),
            ..Default::default()
        };
        executor
            .execute_transaction(Recovered::new_unchecked(
                EezTxEnvelope::from(tx.into_signed(Signature::new(U256::ONE, U256::ONE, false))),
                USER,
            ))
            .unwrap();
    }
    assert_eq!(executor.receipts()[1].cumulative_gas_used, 29_999_998);
    assert!(
        executor
            .execute_transaction(load(3))
            .unwrap_err()
            .to_string()
            .contains("native gas limit differs from its block position")
    );
    let EezTxEnvelope::System(tx) = load(3).into_inner() else {
        unreachable!()
    };
    let mut tx = tx.into_inner();
    tx.gas_limit = 2;
    let correct_budget = EezTxEnvelope::from(tx);
    let error = executor
        .execute_transaction(Recovered::new_unchecked(correct_budget, SYSTEM_ADDRESS))
        .unwrap_err();
    assert!(
        format!("{error:?}").contains("IntrinsicGasTooLow"),
        "{error:?}"
    );
    assert_eq!(executor.receipts().len(), 2);
    assert!(executor.group_state().ensure_complete().is_ok());
    assert_eq!(balance(&mut executor, SYSTEM_ADDRESS), U256::from(9));
    assert_eq!(balance(&mut executor, EEZL2_ADDRESS), U256::ZERO);
}

#[cfg(feature = "stateless")]
mod replay;

#[test]
fn repeated_prefix_restore_keeps_recorded_history_and_complete_group_state() {
    let (config, mut db) = fixture(bytes!("00"));
    let (plan, history) = config
        .prepare_history(&mut db, &parent(&config, 7), 7)
        .unwrap();
    assert_ne!(
        plan.prev_randao,
        B256::ZERO,
        "fixture must require history import instead of inheritance"
    );
    let mut uninterrupted = executor(&config, db.clone(), 7, plan.prev_randao);
    uninterrupted.apply_pre_execution_changes().unwrap();
    assert!(
        uninterrupted.execution_checkpoint().is_err(),
        "unrecorded changed history cannot form a prefix"
    );
    uninterrupted
        .execute_transaction(Recovered::new_unchecked(history.unwrap(), SYSTEM_ADDRESS))
        .unwrap();
    uninterrupted.execute_transaction(load(7)).unwrap();
    assert!(
        uninterrupted.execution_checkpoint().is_err(),
        "incomplete groups cannot be captured"
    );
    uninterrupted
        .execute_transaction(ordinary(0, EEZL2_ADDRESS, 3))
        .unwrap();
    let saved = uninterrupted.execution_checkpoint().unwrap();
    let cache = uninterrupted.evm_mut().overlay_db_mut().cache.clone();
    uninterrupted
        .execute_transaction(ordinary(1, OBSERVER, 5))
        .unwrap();
    let expected = uninterrupted.finish().unwrap();
    for _ in 0..2 {
        let mut restored = executor(&config, db.clone(), 7, plan.prev_randao);
        restored.evm_mut().overlay_db_mut().cache = cache.clone();
        restored
            .restore_execution_checkpoint(saved.clone())
            .unwrap();
        assert_eq!(restored.execution_checkpoint().unwrap(), saved);
        restored
            .execute_transaction(ordinary(1, OBSERVER, 5))
            .unwrap();
        let output = restored.finish().unwrap();
        assert_eq!(output.result, expected.result);
        assert_eq!(output.state, expected.state);
    }
}
