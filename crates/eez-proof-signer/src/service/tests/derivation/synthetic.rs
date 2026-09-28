//! Independent single-account trie and receipt fixtures. No production block
//! builder, reconstruction helper, or trie-root helper constructs expected roots.

use super::*;
use alloy_consensus::{
    Header, SignableTransaction as _, TxEip1559, TxEip2930, TxEip4844, TxEip7702, TxLegacy,
};
use alloy_eips::Encodable2718 as _;
use alloy_primitives::{B64, Bloom, KECCAK256_EMPTY, TxKind, b256};
use eez_primitives::EezTxEnvelope as TransactionSigned;

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

fn fixture() -> (alloy_genesis::Genesis, Vec<BlockWitness>, Vec<u8>) {
    fixture_with_extra_transaction(None)
}

fn fixture_with_extra_transaction(
    extra_transaction: Option<usize>,
) -> (alloy_genesis::Genesis, Vec<BlockWitness>, Vec<u8>) {
    let (config, blocks) = fixture_blocks(extra_transaction);
    let encoded = payload(&blocks);
    (config, blocks, encoded)
}

fn fixture_blocks(extra_transaction: Option<usize>) -> (alloy_genesis::Genesis, Vec<BlockWitness>) {
    let mut config = genesis();
    config.timestamp = 80;
    let schedule = config.config.extra_fields.get_mut("rollupX").unwrap();
    schedule["blockIntervalSeconds"] = 2u64.into();
    schedule["syncIntervalBlocks"] = 6u64.into();
    schedule["l1Reference"]["timestamp"] = 80u64.into();
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
        blob_gas_used: Some(0),
        excess_blob_gas: Some(0),
        parent_beacon_block_root: Some(B256::ZERO),
        requests_hash: Some(EMPTY_REQUESTS),
        block_access_list_hash: None,
        slot_number: None,
    };
    let initial = eez_driver::derivation::Environment {
        fee_recipient: Address::ZERO,
        prev_randao: B256::ZERO,
        extra_data: Bytes::new(),
    };
    let mut environments = vec![initial.clone(); 8];
    for env in &mut environments[2..] {
        env.fee_recipient = Address::repeat_byte(0x20);
    }
    environments[3].fee_recipient = Address::repeat_byte(0x30); // one-position beneficiary run
    for env in &mut environments[4..] {
        env.extra_data = Bytes::from_static(b"metadata change");
    }
    for env in &mut environments[6..] {
        env.extra_data = Bytes::from_static(b"last run");
    }
    let mut blocks = Vec::new();
    let mut raw_transactions = Vec::new();
    for (position, environment) in environments.iter().enumerate() {
        let pre_state = account_leaf(sender, nonce, balance);
        let kind = match position {
            0 => Some(0),
            2 => Some(1),
            4 | 5 => Some(2),
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
        let timestamp = parent.timestamp + 2;
        let mut header = parent.clone();
        header.parent_hash = parent.hash_slow();
        header.number += 1;
        header.timestamp = timestamp;
        header.beneficiary = environment.fee_recipient;
        header.mix_hash = environment.prev_randao;
        header.extra_data = environment.extra_data.clone();
        header.parent_beacon_block_root = Some(B256::ZERO);
        header.blob_gas_used = Some(if kind == Some(3) { 131_072 } else { 0 });
        header.excess_blob_gas = Some(0);
        header.requests_hash = Some(EMPTY_REQUESTS);
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
    (config, blocks)
}

fn configured_state(config: alloy_genesis::Genesis) -> Arc<ServiceState> {
    Arc::new(
        ServiceState::new(
            crate::service::l1::L1Authority::Synthetic,
            Validator::stateless_for_test(config, TEST_SYSTEM_ADDRESS),
            expected_rollup_id(1),
            test_attester(),
            crate::testkit::l1_chain_spec(),
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn independent_blob_execution_is_rejected_before_attestation() {
    let (config, mut blocks) = fixture_blocks(Some(3));
    // Select the independently constructed Osaka block so this rejection
    // reaches blob execution, not an earlier fork or DA mismatch.
    let block = blocks.remove(6);
    let raw = block.witness.as_ref().unwrap();
    let admitted = AdmittedBlock {
        declared_number: block.number,
        claimed_hash: B256::from_slice(&block.hash),
        claimed_parent_hash: B256::from_slice(&block.parent_hash),
        rlp: block.rlp.clone(),
        witness: alloy_rpc_types_debug::ExecutionWitness {
            state: raw.state.iter().cloned().map(Into::into).collect(),
            codes: raw.codes.iter().cloned().map(Into::into).collect(),
            keys: raw.keys.iter().cloned().map(Into::into).collect(),
            headers: raw.headers.iter().cloned().map(Into::into).collect(),
        },
    };
    let error = Validator::stateless_for_test(config.clone(), TEST_SYSTEM_ADDRESS)
        .validate(&[admitted])
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("EIP-4844 transactions are disabled"),
        "{error}"
    );
    let blocks = vec![block];
    // Valid empty operations keep this request focused on execution rejection.
    let encoded = crate::testkit::encode_da_payload(&[Vec::new()]);
    let state = configured_state(config);
    let server = TestServer::new(state.clone()).await;
    let status = server.prove(request(blocks, encoded)).await;
    assert_eq!(status.code(), Code::FailedPrecondition);
    assert_eq!(status.message(), "window validation rejected");
    assert_eq!(state.attester.signing_calls(), 0);
}

#[tokio::test]
async fn independent_paid_transaction_types_under_osaka_reach_the_attester() {
    let (config, blocks, payload) = fixture();
    let state = configured_state(config);
    let server = TestServer::new(state.clone()).await;
    let chunks = request(blocks, payload.clone());
    let expected = recompute_window_public_inputs_hash(&chunks);
    let response = server.attest(chunks).await;
    assert_attestation(&response, expected, state.attester.address());
    assert_eq!(state.attester.signing_calls(), 1);
}

#[tokio::test]
async fn nonzero_beacon_root_is_rejected_even_without_a_beacon_predeploy() {
    let (config, mut blocks, payload) = fixture();
    let state = configured_state(config);
    let server = TestServer::new(state.clone()).await;
    let response = server
        .attest(request(blocks.clone(), payload.clone()))
        .await;
    assert_eq!(response.signature.len(), 65);
    assert_eq!(state.attester.signing_calls(), 1);
    // Mutate an executed header, not merely its claimed DA environment. Replay
    // must enforce the fixed zero field even when a beacon call would be inert.
    let mut block: Block = alloy_rlp::decode_exact(&blocks[0].rlp).unwrap();
    block.header.parent_beacon_block_root = Some(B256::repeat_byte(0x19));
    blocks[0].rlp = alloy_rlp::encode(&block);
    blocks[0].hash = block.header.hash_slow().to_vec();
    let invalid = vec![blocks.remove(0)];
    let encoded = super::payload(&invalid);
    assert_eq!(
        server.prove(request(invalid, encoded)).await.code(),
        Code::FailedPrecondition
    );
    assert_eq!(state.attester.signing_calls(), 1);
}

#[tokio::test]
async fn transactions_and_input_changes_cannot_move_between_logical_positions() {
    let (config, blocks, _payload) = fixture();
    let published = published_blocks(&blocks);
    let mut swapped = published.clone();
    let (left, right) = swapped.split_at_mut(2);
    std::mem::swap(&mut left[0].transactions, &mut right[0].transactions);
    let mut early = published.clone();
    early.swap(1, 2);
    let mut late = published.clone();
    late[2].beneficiary = published[1].beneficiary;
    let mut changed_terminal = published.clone();
    changed_terminal[7].extra_data = Bytes::from_static(b"forged");
    let state = configured_state(config);
    let server = TestServer::new(state.clone()).await;
    for variant in [swapped, early, late, changed_terminal] {
        assert_eq!(
            server
                .prove(request(blocks.clone(), encode_published(&variant)))
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
    config.config.bpo1_time = Some(112);
    config.config.bpo2_time = Some(115);
    let state = configured_state(config.clone());
    let server = TestServer::new(state.clone()).await;
    let _response = server
        .attest(request(blocks.clone(), payload.clone()))
        .await;
    assert_eq!(state.attester.signing_calls(), 1);
    config.config.amsterdam_time = Some(114);
    let unsupported = configured_state(config);
    let server = TestServer::new(unsupported.clone()).await;
    assert_eq!(
        server.prove(request(blocks, payload.clone())).await.code(),
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
    let chunks = request(blocks.clone(), payload.clone());
    let expected = recompute_window_public_inputs_hash(&chunks);
    let response = server.attest(chunks).await;
    assert_attestation(&response, expected, state.attester.address());
    assert_eq!(state.attester.signing_calls(), 1);
    // Even an execution-reverted transaction cannot be omitted from DA: its
    // nonce and gas/receipt effects remain part of the ordinary block.
    let mut published = published_blocks(&blocks);
    published[6].transactions.clear();
    let missing = encode_published(&published);
    assert_eq!(
        server.prove(request(blocks, missing)).await.code(),
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
        let chunks = request(blocks, payload.clone());
        let expected = recompute_window_public_inputs_hash(&chunks);
        let response = server.attest(chunks).await;
        assert_attestation(&response, expected, state.attester.address());
        assert_eq!(state.attester.signing_calls(), 1);
    }
}

#[test]
fn native_replay_preserves_independent_nonce_receipt_and_state_commitments() {
    use alloy_consensus::Typed2718 as _;
    use reth_primitives_traits::Block as _;

    let mut config = genesis();
    config.timestamp = 80;
    config.config.extra_fields.get_mut("rollupX").unwrap()["blockIntervalSeconds"] = 2u64.into();
    config.config.extra_fields.get_mut("rollupX").unwrap()["syncIntervalBlocks"] = 6u64.into();
    config.config.extra_fields.get_mut("rollupX").unwrap()["l1Reference"]["timestamp"] =
        80u64.into();
    let mut spec = reth_chainspec::ChainSpec::from(config);
    spec.base_fee_params = eez_protocol::l2::BASE_FEE_PARAMS.into();
    let spec = Arc::new(spec);
    let sender = eez_primitives::SYSTEM_ADDRESS;
    let mut parent = Header {
        number: 10,
        timestamp: 100,
        gas_limit: 30_000_000,
        base_fee_per_gas: Some(1_000_000_000),
        state_root: keccak256(account_leaf(sender, 7, U256::ZERO)),
        withdrawals_root: Some(alloy_consensus::EMPTY_ROOT_HASH),
        parent_beacon_block_root: Some(B256::ZERO),
        blob_gas_used: Some(0),
        excess_blob_gas: Some(0),
        requests_hash: Some(EMPTY_REQUESTS),
        ..Default::default()
    };
    for nonce in [7, 8] {
        let transaction: TransactionSigned = eez_primitives::SystemTransaction {
            chain_id: 1,
            nonce,
            to: eez_primitives::EEZL2_ADDRESS,
            value: U256::ZERO,
            input: Bytes::new(),
        }
        .into();
        let raw = transaction.encoded_2718();
        let encoded_receipt = [
            vec![0x76],
            alloy_rlp::encode(Receipt {
                status: 1,
                gas_used: 21_000,
                bloom: Bloom::ZERO,
                logs: vec![],
            }),
        ]
        .concat();
        let mut header = parent.clone();
        header.number += 1;
        header.timestamp += 2;
        header.parent_hash = parent.hash_slow();
        header.gas_used = 21_000;
        header.base_fee_per_gas = parent.next_block_base_fee(eez_protocol::l2::BASE_FEE_PARAMS);
        header.transactions_root = ordered_leaf(raw);
        header.receipts_root = ordered_leaf(encoded_receipt);
        header.state_root = keccak256(account_leaf(sender, nonce + 1, U256::ZERO));
        let block = Block {
            header: header.clone(),
            body: alloy_consensus::BlockBody {
                transactions: vec![transaction],
                withdrawals: Some(Default::default()),
                ..Default::default()
            },
        };
        let recovered = block.try_into_recovered().unwrap();
        let witness = alloy_rpc_types_debug::ExecutionWitness {
            state: vec![account_leaf(sender, nonce, U256::ZERO).into()],
            headers: vec![alloy_rlp::encode(&parent).into()],
            ..Default::default()
        };
        let output = stateless_reth::stateless_validation_recovered_with_state_checkpoints(
            recovered,
            witness,
            Arc::clone(&spec),
            eez_evm::EezEvmConfig::new(Arc::clone(&spec)),
            &[0],
        )
        .unwrap();
        assert_eq!(output.validation.post_state_root, header.state_root);
        assert_eq!(output.validation.block_hash, header.hash_slow());
        assert_eq!(
            output.validation.execution_output.result.receipts[0].ty(),
            0x76
        );
        assert_eq!(
            output.checkpoints.transaction_state_checkpoints[0].state_root,
            header.state_root
        );
        parent = header;
    }
}
