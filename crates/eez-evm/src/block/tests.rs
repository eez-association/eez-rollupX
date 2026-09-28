use super::*;
use alloy_eips::{
    eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE},
    eip4788::BEACON_ROOTS_ADDRESS,
    eip7002::WITHDRAWAL_REQUEST_PREDEPLOY_ADDRESS,
    eip7251::CONSOLIDATION_REQUEST_PREDEPLOY_ADDRESS,
    eip7685::EMPTY_REQUESTS_HASH,
};
use alloy_evm::EvmEnv;
use alloy_primitives::{B256, U256, bytes};
use reth_chainspec::ChainSpecBuilder;
use revm::{
    Database,
    context::{BlockEnv, CfgEnv},
    database::{InMemoryDB, State},
    state::{AccountInfo, Bytecode},
};

fn execute_empty_blocks(layer: ExecutionLayer) -> State<InMemoryDB> {
    let spec = Arc::new(ChainSpecBuilder::mainnet().osaka_activated().build());
    let factory = EezBlockExecutorFactory::new(spec, layer);
    let mut db = InMemoryDB::default();
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
                    timestamp: U256::from(number),
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
        let mut state = execute_empty_blocks(layer);
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
