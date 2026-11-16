//! Compare request execution against independently captured Reth 2.7 outputs.
use super::*;
use alloy_primitives::{TxKind, bytes};
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, InMemoryDB},
};
use reth_chainspec::ChainSpecBuilder;
use serde_json::json;

#[test]
fn source_request_execution_vectors() {
    let caller = Address::repeat_byte(0x11);
    let target = Address::repeat_byte(0x22);
    let beneficiary = Address::repeat_byte(0x33);
    let config = eez_evm::EezEvmConfig::for_l1_simulation(Arc::new(
        ChainSpecBuilder::mainnet().prague_activated().build(),
    ));
    let header = Header {
        number: 1,
        timestamp: 1,
        gas_limit: 30_000_000,
        beneficiary,
        base_fee_per_gas: Some(10),
        excess_blob_gas: Some(0),
        blob_gas_used: Some(0),
        ..Default::default()
    };
    let env = config.evm_env(&header).unwrap();
    let mut rows = Vec::new();
    for fee_mode in 0..3 {
        for access in [false, true] {
            for explicit in [None, Some(0), Some(1), Some(2)] {
                let mut request = TransactionRequest {
                    from: Some(caller),
                    to: Some(TxKind::Call(target)),
                    nonce: Some(7),
                    chain_id: Some(1),
                    gas: Some(100_000),
                    value: Some(U256::from(3)),
                    transaction_type: explicit,
                    ..Default::default()
                };
                if fee_mode == 0 {
                    request.gas_price = Some(40);
                } else {
                    request.max_fee_per_gas = Some(40);
                    if fee_mode == 1 {
                        request.max_priority_fee_per_gas = Some(2);
                    }
                }
                if access {
                    request.access_list = Some(
                        serde_json::from_value(json!([{
                            "address": target, "storageKeys": [B256::ZERO]
                        }]))
                        .unwrap(),
                    );
                }
                let mut tx = source_transaction(request, &env).unwrap();
                let kind = tx.ty();
                set_source_gas_limit(&mut tx, 150_000).unwrap();
                set_source_gas_limit(&mut tx, 100_000).unwrap();
                let tx = tx.map(eez_evm::EezEvmTx::Ethereum);
                let mut db = InMemoryDB::default();
                db.insert_account_info(
                    &caller,
                    AccountInfo {
                        balance: U256::from(1_000_000_000_000_000_000_u64),
                        nonce: 7,
                        ..Default::default()
                    },
                );
                db.insert_account_info(
                    &target,
                    AccountInfo {
                        nonce: 1,
                        // Return GASPRICE, SLOAD(0), CALLVALUE and ORIGIN.
                        code: Some(Bytecode::new_raw(bytes!(
                            "3a5f525f54602052346040523260605260805ff3"
                        ))),
                        ..Default::default()
                    },
                );
                db.insert_account_storage(&target, &U256::ZERO, &U256::from(7));
                let result = config
                    .evm_with_env(&mut db, env.clone())
                    .transact(&tx)
                    .unwrap();
                assert!(result.result.is_success());
                rows.push(json!({
                    "fee_mode": fee_mode, "access": access, "explicit": explicit,
                    "kind": kind, "gas": result.result.tx_gas_used(),
                    "output": &result.result.output,
                    "caller_balance": result.pending_state.account_info(&caller).unwrap().balance,
                    "caller_nonce": result.pending_state.account_info(&caller).unwrap().nonce,
                    "target_balance": result.pending_state.account_info(&target).unwrap().balance,
                    // The beneficiary is absent in the fixture; a zero tip need not load it.
                    "beneficiary_balance": result.pending_state.account_info(&beneficiary).map_or(U256::ZERO, |a| a.balance),
                }));
            }
        }
    }
    assert_eq!(rows.len(), 24);
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("source-request-reth27-vectors.json")).unwrap();
    let expected = expected.as_array().unwrap();
    assert_eq!(rows.len(), expected.len());
    for (actual, expected) in rows.iter().zip(expected) {
        assert_eq!(actual, expected);
    }
}
