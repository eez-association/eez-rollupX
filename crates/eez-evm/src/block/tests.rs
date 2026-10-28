use super::*;
use crate::balances::PROTOCOL_BALANCE_ACCOUNTS;
use alloy_eips::{
    eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE},
    eip4788::BEACON_ROOTS_ADDRESS,
    eip7002::WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
    eip7251::CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
    eip7685::EMPTY_REQUESTS_HASH,
};
use alloy_evm::EvmEnv;
use alloy_primitives::{Address, U256};
use alloy_primitives::{B256, bytes};
use eez_primitives::{EEZL2_ADDRESS, SYSTEM_ADDRESS};
use reth_chainspec::ChainSpecBuilder;
use revm::{
    Database,
    context::{BlockEnv, CfgEnv},
    database::{InMemoryDB, State, states::bundle_state::BundleRetention},
    state::{AccountInfo, Bytecode},
};

fn execute_empty_blocks(
    layer: ExecutionLayer,
    accounts: &[(Address, AccountInfo)],
) -> State<InMemoryDB> {
    let spec = Arc::new(ChainSpecBuilder::mainnet().osaka_activated().build());
    let mut factory = EezBlockExecutorFactory::new(spec, layer);
    let (history, mut db) = crate::history::tests::fixture();
    factory.history = Some(history);
    if layer == ExecutionLayer::Rollup {
        factory.evm = crate::tests::factory();
    }
    for (address, raw) in [
        (HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE.clone()),
        // These adversarial predeploys make unwanted calls observable even
        // though no real request or beacon predeploy is required on L2.
        (BEACON_ROOTS_ADDRESS, bytes!("600160005500")),
        (WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS, bytes!("600160005500")),
        (
            CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
            bytes!("600160005500"),
        ),
    ] {
        let code = Bytecode::new_raw(raw);
        db.insert_account_info(
            address,
            AccountInfo {
                code_hash: code.hash_slow(),
                code: Some(code),
                ..Default::default()
            },
        );
        db.insert_account_storage(address, U256::ZERO, U256::from(37))
            .unwrap();
    }
    for (address, info) in accounts {
        db.insert_account_info(*address, info.clone());
    }
    let mut state = State::builder()
        .with_database(db)
        .with_bundle_update()
        .build();
    for number in [1u64, 2] {
        let evm = factory.evm_factory().create_evm(
            &mut state,
            EvmEnv {
                cfg_env: CfgEnv::new_with_spec(SpecId::OSAKA).with_chain_id(1),
                block_env: BlockEnv {
                    number: U256::from(number),
                    timestamp: U256::from(number + 4),
                    gas_limit: 30_000_000,
                    basefee: 1,
                    ..Default::default()
                },
            },
        );
        let mut executor = factory.create_executor(
            evm,
            EthBlockExecutionCtx {
                parent_hash: B256::repeat_byte(number as u8),
                parent_beacon_block_root: Some(B256::repeat_byte(42)),
                ommers: &[],
                withdrawals: None,
                extra_data: Default::default(),
                tx_count_hint: Some(0),
                slot_number: None,
            },
        );
        executor.apply_pre_execution_changes().unwrap();
        if number == 1 {
            for (address, original) in accounts {
                assert_eq!(
                    executor
                        .evm_mut()
                        .db_mut()
                        .basic(*address)
                        .unwrap()
                        .unwrap()
                        .balance,
                    original.balance,
                    "pre-execution must not burn {address}"
                );
            }
        }
        let (_, output) = executor.finish().unwrap();
        assert!(output.receipts.is_empty());
        assert_eq!(output.gas_used, 0);
        assert_eq!(output.blob_gas_used, 0);
        assert_eq!(output.requests.requests_hash(), EMPTY_REQUESTS_HASH);
    }
    state
}

#[test]
fn issue35_empty_l2_blocks_write_history_without_beacon_or_request_side_effects() {
    for layer in [ExecutionLayer::Rollup, ExecutionLayer::L1Simulation] {
        let mut state = execute_empty_blocks(layer, &[]);
        for (slot, expected) in [(0, 1), (1, 2)] {
            assert_eq!(
                state
                    .storage(HISTORY_STORAGE_ADDRESS, U256::from(slot))
                    .unwrap(),
                U256::from_be_bytes(B256::repeat_byte(expected).0)
            );
        }
        for address in [
            BEACON_ROOTS_ADDRESS,
            WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
            CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
        ] {
            let expected = if layer == ExecutionLayer::Rollup {
                37
            } else {
                1
            };
            assert_eq!(
                state.storage(address, U256::ZERO).unwrap(),
                U256::from(expected),
                "{layer:?} {address}"
            );
        }
    }
}

#[test]
fn rollup_blocks_burn_protocol_balances_at_finalization_and_l1_simulation_keeps_them() {
    let code = Bytecode::new_raw(bytes!("00"));
    let accounts = [
        (
            SYSTEM_ADDRESS,
            AccountInfo {
                balance: U256::from(17),
                nonce: 3,
                ..Default::default()
            },
        ),
        (
            EEZL2_ADDRESS,
            AccountInfo {
                balance: U256::from(29),
                code_hash: code.hash_slow(),
                code: Some(code),
                ..Default::default()
            },
        ),
    ];
    for layer in [ExecutionLayer::Rollup, ExecutionLayer::L1Simulation] {
        let mut state = execute_empty_blocks(layer, &accounts);
        for (address, original) in &accounts {
            let info = state.basic(*address).unwrap().unwrap();
            let expected = if layer == ExecutionLayer::Rollup {
                U256::ZERO
            } else {
                original.balance
            };
            assert_eq!(info.balance, expected, "{layer:?} {address}");
            // Only the balance is burned; nonce and code remain.
            assert_eq!(info.nonce, original.nonce, "{layer:?} {address}");
            assert_eq!(info.code_hash, original.code_hash, "{layer:?} {address}");
        }
    }
}

#[test]
fn rollup_blocks_do_not_touch_zero_or_absent_protocol_accounts() {
    let code = Bytecode::new_raw(bytes!("00"));
    // EEZL2 exists with a zero balance; SYSTEM_ADDRESS is absent from genesis.
    let accounts = [(
        EEZL2_ADDRESS,
        AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        },
    )];
    let mut state = execute_empty_blocks(ExecutionLayer::Rollup, &accounts);
    state.merge_transitions(BundleRetention::Reverts);
    for address in PROTOCOL_BALANCE_ACCOUNTS {
        assert!(
            !state.bundle_state.state.contains_key(&address),
            "{address} must not receive a transition"
        );
    }
    assert!(state.basic(SYSTEM_ADDRESS).unwrap().is_none());
}

mod groups;
