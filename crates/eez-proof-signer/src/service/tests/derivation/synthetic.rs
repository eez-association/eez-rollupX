//! Independent single-account trie and receipt fixtures. No production block
//! builder, reconstruction helper, or trie-root helper constructs expected roots.

use super::*;
use alloy_consensus::{
    Header, SignableTransaction as _, TxEip1559, TxEip2930, TxEip4844, TxEip7702, TxLegacy,
};
use alloy_eips::Encodable2718 as _;
use alloy_primitives::{B64, Bloom, KECCAK256_EMPTY, TxKind, b256};
use reth_ethereum_primitives::TransactionSigned;

const EMPTY_REQUESTS: B256 =
    b256!("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");

#[derive(alloy_rlp::RlpEncodable)]
struct Account {
    nonce: u64,
    balance: U256,
    storage_root: B256,
    code_hash: B256,
}

#[derive(alloy_rlp::RlpEncodable)]
struct Receipt {
    status: u8,
    gas_used: u64,
    bloom: Bloom,
    logs: Vec<u8>,
}

fn account_leaf(address: Address, nonce: u64, balance: U256) -> Vec<u8> {
    // A full 64-nibble key has even-length hex-prefix leaf encoding 0x20 || key.
    let mut path = vec![0x20];
    path.extend_from_slice(keccak256(address).as_slice());
    alloy_rlp::encode(vec![
        Bytes::from(path),
        Bytes::from(alloy_rlp::encode(Account {
            nonce,
            balance,
            storage_root: alloy_consensus::EMPTY_ROOT_HASH,
            code_hash: KECCAK256_EMPTY,
        })),
    ])
}

fn ordered_leaf(value: Vec<u8>) -> B256 {
    // RLP(0) = 0x80, therefore the only transaction/receipt key is [8,0].
    keccak256(alloy_rlp::encode(vec![
        Bytes::from_static(&[0x20, 0x80]),
        Bytes::from(value),
    ]))
}

fn transaction(kind: usize, nonce: u64) -> TransactionSigned {
    let key = alloy_signer_local::PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).unwrap();
    let sender = key.address();
    let sign = |hash: B256| {
        alloy_primitives::Signature::from(
            key.credential()
                .sign_prehash_recoverable(hash.as_slice())
                .unwrap(),
        )
    };
    match kind {
        0 => {
            let tx = TxLegacy {
                chain_id: Some(1),
                nonce,
                gas_limit: 21_000,
                to: TxKind::Call(sender),
                ..Default::default()
            };
            let signature = sign(tx.signature_hash());
            tx.into_signed(signature).into()
        }
        1 => {
            let tx = TxEip2930 {
                chain_id: 1,
                nonce,
                gas_limit: 21_000,
                to: TxKind::Call(sender),
                ..Default::default()
            };
            let signature = sign(tx.signature_hash());
            tx.into_signed(signature).into()
        }
        2 => {
            let tx = TxEip1559 {
                chain_id: 1,
                nonce,
                gas_limit: 21_000,
                to: TxKind::Call(sender),
                ..Default::default()
            };
            let signature = sign(tx.signature_hash());
            tx.into_signed(signature).into()
        }
        3 => {
            let tx = TxEip4844 {
                chain_id: 1,
                nonce,
                gas_limit: 21_000,
                to: sender,
                max_fee_per_blob_gas: 1,
                blob_versioned_hashes: vec![B256::repeat_byte(1)],
                ..Default::default()
            };
            let signature = sign(tx.signature_hash());
            tx.into_signed(signature).into()
        }
        4 => {
            let tx = TxEip1559 {
                chain_id: 1,
                nonce,
                gas_limit: 100_000,
                to: TxKind::Create,
                // PUSH0 PUSH0 REVERT: no account survives the reverted create.
                input: Bytes::from_static(&[0x5f, 0x5f, 0xfd]),
                ..Default::default()
            };
            let signature = sign(tx.signature_hash());
            tx.into_signed(signature).into()
        }
        5 | 6 => {
            let authorization = alloy_eips::eip7702::Authorization {
                chain_id: U256::from(if kind == 5 { 1 } else { 999 }),
                address: Address::ZERO,
                // Transaction processing increments the sender nonce first.
                nonce: nonce + 1,
            };
            let signature = sign(authorization.signature_hash());
            let tx = TxEip7702 {
                chain_id: 1,
                nonce,
                gas_limit: 100_000,
                to: sender,
                authorization_list: vec![authorization.into_signed(signature)],
                ..Default::default()
            };
            let signature = sign(tx.signature_hash());
            tx.into_signed(signature).into()
        }
        _ => unreachable!(),
    }
}

fn fixture() -> (alloy_genesis::ChainConfig, Vec<BlockWitness>, Payload) {
    fixture_with_extra_transaction(None)
}

fn fixture_with_extra_transaction(
    extra_transaction: Option<usize>,
) -> (alloy_genesis::ChainConfig, Vec<BlockWitness>, Payload) {
    let mut config = chain_config();
    config.cancun_time = Some(106);
    config.prague_time = Some(110);
    config.osaka_time = Some(112);
    let key = alloy_signer_local::PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).unwrap();
    let sender = key.address();
    let mut balance = U256::from(1_000_000_000_000_000_000u64);
    let mut nonce = 0;
    let mut parent = Header {
        parent_hash: B256::ZERO,
        ommers_hash: alloy_consensus::EMPTY_OMMER_ROOT_HASH,
        beneficiary: Address::ZERO,
        state_root: keccak256(account_leaf(sender, nonce, balance)),
        transactions_root: alloy_consensus::EMPTY_ROOT_HASH,
        receipts_root: alloy_consensus::EMPTY_ROOT_HASH,
        logs_bloom: Bloom::ZERO,
        difficulty: U256::ZERO,
        number: 10,
        gas_limit: 30_000_000,
        gas_used: 0,
        timestamp: 100,
        extra_data: Bytes::new(),
        mix_hash: B256::ZERO,
        nonce: B64::ZERO,
        base_fee_per_gas: Some(0),
        withdrawals_root: Some(alloy_consensus::EMPTY_ROOT_HASH),
        blob_gas_used: None,
        excess_blob_gas: None,
        parent_beacon_block_root: None,
        requests_hash: None,
        block_access_list_hash: None,
        slot_number: None,
    };
    let initial = Environment {
        timestamp_step_seconds: 2,
        fee_recipient: Address::ZERO,
        prev_randao: B256::ZERO,
        gas_limit: 30_000_000,
        extra_data: Bytes::new(),
        cancun_parent_beacon_root: B256::repeat_byte(0xb0),
    };
    let mut environments = vec![initial.clone(); 8];
    for env in &mut environments[2..] {
        env.fee_recipient = Address::repeat_byte(0x20);
    }
    environments[3].fee_recipient = Address::repeat_byte(0x30); // full record's isolated input
    for env in &mut environments[4..] {
        env.prev_randao = B256::repeat_byte(0x44);
    }
    for env in &mut environments[6..] {
        env.timestamp_step_seconds = 3;
    }
    let mut blocks = Vec::new();
    let mut raw_transactions = Vec::new();
    for (position, environment) in environments.iter().enumerate() {
        let pre_state = account_leaf(sender, nonce, balance);
        let kind = match position {
            0 => Some(0),
            2 => Some(1),
            4 => Some(2),
            5 => Some(3),
            6 => extra_transaction,
            _ => None,
        };
        let txs = kind
            .map(|kind| transaction(kind, nonce))
            .into_iter()
            .collect::<Vec<_>>();
        let raw = txs
            .iter()
            .map(|tx| Bytes::from(tx.encoded_2718()))
            .collect::<Vec<_>>();
        let timestamp = parent.timestamp + environment.timestamp_step_seconds;
        let mut header = parent.clone();
        header.parent_hash = parent.hash_slow();
        header.number += 1;
        header.timestamp = timestamp;
        header.beneficiary = environment.fee_recipient;
        header.mix_hash = environment.prev_randao;
        header.parent_beacon_block_root =
            (timestamp >= 106).then_some(environment.cancun_parent_beacon_root);
        header.blob_gas_used =
            (timestamp >= 106).then_some(if kind == Some(3) { 131_072 } else { 0 });
        header.excess_blob_gas = (timestamp >= 106).then_some(0);
        header.requests_hash = (timestamp >= 110).then_some(EMPTY_REQUESTS);
        // Reverted CREATE: 53,000 intrinsic + 3 nonzero calldata bytes at 16
        // each + 2 initcode-word gas + two PUSH0 opcodes at 2 each. REVERT
        // consumes no additional gas for empty memory/output.
        header.gas_used = match kind {
            Some(4) => 53_054,
            // 21,000 + 25,000 authorization gas. Existing-authority clearing
            // earns 12,500 refund, capped to 46,000 / 5 = 9,200. A wrong-chain
            // authorization is ignored, earns no refund and changes no nonce.
            Some(5) => 36_800,
            Some(6) => 46_000,
            Some(_) => 21_000,
            None => 0,
        };
        header.transactions_root = raw.first().map_or(alloy_consensus::EMPTY_ROOT_HASH, |tx| {
            ordered_leaf(tx.to_vec())
        });
        header.receipts_root = kind.map_or(alloy_consensus::EMPTY_ROOT_HASH, |kind| {
            let receipt = alloy_rlp::encode(Receipt {
                status: u8::from(kind != 4),
                gas_used: header.gas_used,
                bloom: Bloom::ZERO,
                logs: vec![],
            });
            let encoded = if kind == 0 {
                receipt
            } else {
                let receipt_type = match kind {
                    4 => 2,
                    5 | 6 => 4,
                    _ => kind as u8,
                };
                [&[receipt_type][..], &receipt].concat()
            };
            ordered_leaf(encoded)
        });
        if kind.is_some() {
            nonce += if kind == Some(5) { 2 } else { 1 };
        }
        if kind == Some(3) {
            balance -= U256::from(131_072);
        }
        header.state_root = keccak256(account_leaf(sender, nonce, balance));
        let block = Block {
            header: header.clone(),
            body: alloy_consensus::BlockBody {
                transactions: txs,
                ommers: vec![],
                withdrawals: Some(Default::default()),
            },
        };
        blocks.push(BlockWitness {
            number: header.number,
            hash: header.hash_slow().to_vec(),
            parent_hash: header.parent_hash.to_vec(),
            rlp: alloy_rlp::encode(block),
            witness: Some(ExecutionWitness {
                state: vec![pre_state],
                codes: vec![],
                keys: vec![],
                headers: vec![alloy_rlp::encode(&parent)],
            }),
        });
        raw_transactions.push(raw);
        parent = header;
    }
    let records = vec![
        Record {
            empty_gap: 0,
            data: RecordData::Transactions(raw_transactions[0].clone()),
        },
        Record {
            empty_gap: 1,
            data: RecordData::EnvironmentChange {
                environment: environments[2].clone(),
                transactions: raw_transactions[2].clone(),
            },
        },
        Record {
            empty_gap: 0,
            data: RecordData::FullBlock(blocks[3].rlp.clone().into()),
        },
        Record {
            empty_gap: 0,
            data: RecordData::EnvironmentChange {
                environment: environments[4].clone(),
                transactions: raw_transactions[4].clone(),
            },
        },
        Record {
            empty_gap: 0,
            data: RecordData::Transactions(raw_transactions[5].clone()),
        },
        Record {
            empty_gap: 0,
            data: RecordData::EnvironmentChange {
                environment: environments[6].clone(),
                transactions: raw_transactions[6].clone(),
            },
        },
    ];
    let payload = Payload::new(7, initial, records, blocks[7].rlp.clone(), vec![], vec![]).unwrap();
    (config, blocks, payload)
}

fn configured_state(config: alloy_genesis::ChainConfig) -> Arc<ServiceState> {
    Arc::new(
        ServiceState::new(
            Validator::stateless_for_test(config, TEST_SYSTEM_ADDRESS),
            expected_rollup_id(1),
            test_attester(),
            test_system_transaction_key(),
            crate::testkit::l1_chain_spec(),
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn independent_typed_transactions_blob_gas_and_fork_transitions_reach_the_attester() {
    let (config, blocks, payload) = fixture();
    let state = configured_state(config);
    let server = TestServer::new(state.clone()).await;
    let chunks = request(blocks, payload.encode().unwrap());
    let expected = recompute_window_public_inputs_hash(&chunks);
    let response = server.attest(chunks).await;
    assert_attestation(&response, expected, state.attester.address());
    assert_eq!(state.attester.signing_calls(), 1);
}

#[tokio::test]
async fn unused_pre_cancun_root_is_ignored_but_first_active_root_is_bound() {
    let (config, blocks, payload) = fixture();
    let mut before_cancun = payload.initial_environment().clone();
    before_cancun.cancun_parent_beacon_root = B256::repeat_byte(0x19);
    let altered = Payload::new(
        7,
        before_cancun,
        payload.records().to_vec(),
        payload.terminal_block().to_vec(),
        vec![],
        vec![],
    )
    .unwrap();
    let state = configured_state(config);
    let server = TestServer::new(state.clone()).await;
    let response = server
        .attest(request(blocks.clone(), altered.encode().unwrap()))
        .await;
    assert_eq!(state.attester.signing_calls(), 1);
    assert_eq!(response.signature.len(), 65);
    let mut records = altered.records().to_vec();
    let RecordData::EnvironmentChange { environment, .. } = &mut records[1].data else {
        unreachable!()
    };
    environment.cancun_parent_beacon_root = B256::repeat_byte(0x19);
    let bad = Payload::new(
        7,
        altered.initial_environment().clone(),
        records,
        altered.terminal_block().to_vec(),
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(
        server
            .prove(request(blocks, bad.encode().unwrap()))
            .await
            .code(),
        Code::FailedPrecondition
    );
    assert_eq!(
        state.attester.signing_calls(),
        1,
        "rejected active root must not invoke sign"
    );
}

#[tokio::test]
async fn transactions_and_input_changes_cannot_move_between_logical_positions() {
    let (config, blocks, payload) = fixture();
    let RecordData::Transactions(first) = &payload.records()[0].data else {
        unreachable!()
    };
    let RecordData::EnvironmentChange {
        environment,
        transactions: second,
    } = &payload.records()[1].data
    else {
        unreachable!()
    };
    let mut swapped = payload.records().to_vec();
    swapped[0].data = RecordData::Transactions(second.clone());
    swapped[1].data = RecordData::EnvironmentChange {
        environment: environment.clone(),
        transactions: first.clone(),
    };
    let mut early = payload.records().to_vec();
    early[1].empty_gap = 0;
    early[2].empty_gap = 1; // keep the full block at its original height
    let mut late = payload.records().to_vec();
    late[1].data = RecordData::Transactions(second.clone());
    late[2].data = RecordData::EnvironmentChange {
        environment: environment.clone(),
        transactions: vec![],
    };
    let mut changed_full = payload.records().to_vec();
    let mut full: Block = alloy_rlp::decode_exact(&blocks[3].rlp).unwrap();
    full.header.receipts_root = B256::ZERO;
    changed_full[2].data = RecordData::FullBlock(alloy_rlp::encode(full).into());
    let state = configured_state(config);
    let server = TestServer::new(state.clone()).await;
    for records in [swapped, early, late, changed_full] {
        let bad = Payload::new(
            7,
            payload.initial_environment().clone(),
            records,
            payload.terminal_block().to_vec(),
            vec![],
            vec![],
        )
        .unwrap();
        assert_eq!(
            server
                .prove(request(blocks.clone(), bad.encode().unwrap()))
                .await
                .code(),
            Code::FailedPrecondition
        );
        assert_eq!(state.attester.signing_calls(), 0);
    }
}

#[tokio::test]
async fn bpo_remains_osaka_while_unsupported_execution_forks_fail_before_signing() {
    let (mut config, blocks, payload) = fixture();
    config.bpo1_time = Some(112);
    config.bpo2_time = Some(115);
    let state = configured_state(config.clone());
    let server = TestServer::new(state.clone()).await;
    let _response = server
        .attest(request(blocks.clone(), payload.encode().unwrap()))
        .await;
    assert_eq!(state.attester.signing_calls(), 1);
    config.amsterdam_time = Some(114);
    let unsupported = configured_state(config);
    let server = TestServer::new(unsupported.clone()).await;
    assert_eq!(
        server
            .prove(request(blocks, payload.encode().unwrap()))
            .await
            .code(),
        Code::FailedPrecondition
    );
    assert_eq!(unsupported.attester.signing_calls(), 0);
}

#[tokio::test]
async fn reverted_ordinary_creation_reconstructs_failed_receipt_and_nonce_change() {
    let (config, blocks, payload) = fixture_with_extra_transaction(Some(4));
    let block: Block = alloy_rlp::decode_exact(&blocks[6].rlp).unwrap();
    assert_eq!(block.header.gas_used, 53_054);
    assert_eq!(block.body.transactions.len(), 1);
    assert_eq!(
        block.header.receipts_root,
        ordered_leaf(
            [
                &[2][..],
                &alloy_rlp::encode(Receipt {
                    status: 0,
                    gas_used: 53_054,
                    bloom: Bloom::ZERO,
                    logs: vec![],
                }),
            ]
            .concat()
        )
    );
    let state = configured_state(config);
    let server = TestServer::new(state.clone()).await;
    let chunks = request(blocks.clone(), payload.encode().unwrap());
    let expected = recompute_window_public_inputs_hash(&chunks);
    let response = server.attest(chunks).await;
    assert_attestation(&response, expected, state.attester.address());
    assert_eq!(state.attester.signing_calls(), 1);
    // Even an execution-reverted transaction cannot be omitted from DA: its
    // nonce and gas/receipt effects remain part of the ordinary block.
    let mut records = payload.records().to_vec();
    let RecordData::EnvironmentChange { transactions, .. } = &mut records.last_mut().unwrap().data
    else {
        unreachable!()
    };
    transactions.clear();
    let missing = Payload::new(
        7,
        payload.initial_environment().clone(),
        records,
        payload.terminal_block().to_vec(),
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(
        server
            .prove(request(blocks, missing.encode().unwrap()))
            .await
            .code(),
        Code::FailedPrecondition
    );
    assert_eq!(state.attester.signing_calls(), 1);
}

#[tokio::test]
async fn independent_7702_authorization_nonce_and_refund_are_reconstructed() {
    for kind in [5, 6] {
        let (config, blocks, payload) = fixture_with_extra_transaction(Some(kind));
        let block: Block = alloy_rlp::decode_exact(&blocks[6].rlp).unwrap();
        assert_eq!(
            block.header.gas_used,
            if kind == 5 { 36_800 } else { 46_000 }
        );
        assert_eq!(block.body.transactions[0].encoded_2718()[0], 4);
        let state = configured_state(config);
        let server = TestServer::new(state.clone()).await;
        let chunks = request(blocks, payload.encode().unwrap());
        let expected = recompute_window_public_inputs_hash(&chunks);
        let response = server.attest(chunks).await;
        assert_attestation(&response, expected, state.attester.address());
        assert_eq!(state.attester.signing_calls(), 1);
    }
}
