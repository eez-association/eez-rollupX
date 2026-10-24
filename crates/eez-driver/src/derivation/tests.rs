use super::*;
use alloy_consensus::{BlockBody, EMPTY_OMMER_ROOT_HASH, EMPTY_ROOT_HASH};
use alloy_primitives::{B64, Bloom, U256, b256};
use reth_chainspec::{ChainSpecBuilder, EthereumHardfork, ForkCondition};
use reth_storage_api::noop::NoopProvider;
use std::sync::Arc;

const POST_ROOT: B256 = B256::repeat_byte(0x91);
const EMPTY_REQUESTS: B256 =
    b256!("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");

fn environment() -> Environment {
    Environment {
        fee_recipient: Address::repeat_byte(0x11),
        extra_data: Bytes::from_static(b"published inputs"),
    }
}

fn config(fork: EthereumHardfork) -> EezEvmConfig {
    use eez_protocol::l1_history::{
        HISTORY_ADDRESS, HeaderChain, HeaderRules, HistorySchedule, WitnessHistory,
        bootstrap_storage,
    };
    let reference = Header {
        timestamp: 68,
        mix_hash: B256::repeat_byte(0x22),
        ..Default::default()
    };
    let endpoint = Header {
        number: 1,
        timestamp: 1_000_000,
        parent_hash: reference.hash_slow(),
        ..Default::default()
    };
    let mut genesis: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../../genesis.json")).unwrap();
    genesis["timestamp"] = serde_json::json!("0x50");
    genesis["mixHash"] = serde_json::json!(reference.mix_hash);
    genesis["config"]["chainId"] = serde_json::json!(1);
    genesis["config"]["rollupX"]["blockIntervalSeconds"] = serde_json::json!(2);
    genesis["config"]["rollupX"]["syncIntervalBlocks"] = serde_json::json!(6);
    genesis["config"]["rollupX"]["historyUpdateOffset"] = serde_json::json!(2);
    genesis["config"]["rollupX"]["l1Reference"]["timestamp"] = serde_json::json!(68);
    genesis["config"]["rollupX"]["l1Reference"]["hash"] = serde_json::json!(reference.hash_slow());
    genesis["config"]["rollupX"]["l1Reference"]["prevRandao"] =
        serde_json::json!(reference.mix_hash);
    genesis["alloc"][format!("{HISTORY_ADDRESS:#x}")]["storage"] = serde_json::to_value(
        bootstrap_storage(
            0,
            reference.hash_slow(),
            HistorySchedule::new(80, 2, 6, 2).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let mut spec = eez_evm::profile::chain_spec(serde_json::from_value(genesis).unwrap()).unwrap();
    if fork != EthereumHardfork::Osaka {
        let builder = ChainSpecBuilder::mainnet().reset();
        spec.hardforks = match fork {
            EthereumHardfork::London => builder.london_activated(),
            EthereumHardfork::Amsterdam => builder.amsterdam_activated(),
            _ => panic!("unexpected non-profile test fork"),
        }
        .build()
        .hardforks;
    }
    let mut chain = HeaderChain::default();
    chain
        .append(
            &[
                alloy_rlp::encode(reference).into(),
                alloy_rlp::encode(endpoint).into(),
            ],
            &HeaderRules {
                london_block: None,
                shanghai_time: None,
                cancun_time: None,
                prague_time: None,
                unsupported_time: None,
            },
            2,
        )
        .unwrap();
    EezEvmConfig::new(Arc::new(spec))
        .with_l1_history(Arc::new(WitnessHistory(Arc::new(chain))))
        .unwrap()
}

fn parent() -> SealedHeader {
    SealedHeader::seal_slow(Header {
        number: 10,
        timestamp: 100,
        mix_hash: B256::repeat_byte(0x22),
        gas_limit: 30_000_000,
        base_fee_per_gas: Some(1_000_000_000),
        // Fixed-profile empty parent for the independent assembly oracle.
        withdrawals_root: Some(EMPTY_ROOT_HASH),
        parent_beacon_block_root: Some(B256::ZERO),
        blob_gas_used: Some(0),
        excess_blob_gas: Some(0),
        requests_hash: Some(EMPTY_REQUESTS),
        ..Default::default()
    })
}

fn parent_for(fork: EthereumHardfork) -> SealedHeader {
    assert_eq!(fork, EthereumHardfork::Osaka);
    parent()
}

fn install_parent_history(database: &mut revm::database::InMemoryDB, config: &EezEvmConfig) {
    use eez_protocol::l1_history::{HISTORY_ADDRESS, NEXT_CUTOFF_SLOT};
    use revm::{bytecode::Bytecode, state::AccountInfo};
    let history = &config.chain_spec().genesis.alloc[&HISTORY_ADDRESS];
    let code = Bytecode::new_raw(history.code.clone().unwrap());
    database.insert_account_info(
        HISTORY_ADDRESS,
        AccountInfo {
            nonce: 1,
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        },
    );
    for (slot, value) in history.storage.as_ref().unwrap() {
        database
            .insert_account_storage(
                HISTORY_ADDRESS,
                U256::from_be_bytes(slot.0),
                U256::from_be_bytes(value.0),
            )
            .unwrap();
    }
    // Parent 10 at timestamp 100 has executed the repeated-source updates at
    // 86 and 98. Its next cutoff is 104 (installed at 110), not genesis's 80.
    database
        .insert_account_storage(
            HISTORY_ADDRESS,
            U256::from(NEXT_CUTOFF_SLOT),
            U256::from(104),
        )
        .unwrap();
}

#[test]
fn writer_publishes_exact_metadata_for_every_position_and_reanchors() {
    use eez_payload_codec::v0::Payload;
    let fork = EthereumHardfork::Osaka;
    let config = config(fork);
    let anchor = parent_for(fork);
    let mut parent = anchor.clone();
    let mut blocks = Vec::new();
    for position in 0..6 {
        let mut block = expected_empty(&parent, fork);
        block.header.number = parent.number + 1;
        block.header.timestamp = parent.timestamp + 2;
        if position >= 2 {
            block.header.beneficiary = Address::repeat_byte(0xa5);
        }
        if position == 5 {
            block.header.extra_data = Bytes::from_static(b"terminal metadata");
        }
        parent = SealedHeader::seal_slow(block.header.clone());
        blocks.push(block);
    }
    let rlps = blocks.iter().map(alloy_rlp::encode).collect::<Vec<_>>();
    let wire = encode_operations(&config, &anchor, &rlps).unwrap();
    let payload = Payload::decode(&wire).unwrap();
    assert_eq!(payload.block_count(), 6);
    assert_eq!(
        payload.count_runs().count(),
        1,
        "metadata does not split empty runs"
    );
    for (position, plan) in payload.blocks().enumerate() {
        assert_eq!(plan.beneficiary, blocks[position].header.beneficiary);
        assert_eq!(plan.extra_data, blocks[position].header.extra_data.as_ref());
        assert_eq!(plan.transactions.len(), 0);
    }
    let summary = blocks.iter().fold(
        eez_payload_codec::v0::PayloadSizer::default(),
        |sum, block| sum.combine(&operations_size(block).unwrap()).unwrap(),
    );
    assert_eq!(summary.encoded_len().unwrap(), wire.len());
    let new_anchor = SealedHeader::seal_slow(blocks[1].header.clone());
    let suffix = encode_operations(&config, &new_anchor, &rlps[2..]).unwrap();
    assert!(
        Payload::decode(&suffix)
            .unwrap()
            .blocks()
            .all(|plan| plan.beneficiary == Address::repeat_byte(0xa5))
    );
    assert!(encode_operations(&config, &anchor, &rlps[2..]).is_err());
    assert!(encode_operations(&config, &anchor, &[]).is_err());
}

#[test]
fn issue_35_six_hundred_empty_blocks_use_the_exact_direct_31_byte_body() {
    let config = config(EthereumHardfork::Osaka);
    let anchor = parent();
    let mut parent = anchor.clone();
    let mut rlps = Vec::new();
    for _ in 0..600 {
        let mut block = expected_empty(&parent, EthereumHardfork::Osaka);
        block.header.number = parent.number + 1;
        block.header.timestamp = parent.timestamp + 2;
        block.header.extra_data = Bytes::new();
        parent = SealedHeader::seal_slow(block.header.clone());
        rlps.push(alloy_rlp::encode(block));
    }
    let wire = encode_operations(&config, &anchor, &rlps).unwrap();
    let mut expected = vec![0, 0xd8, 4, 0xff, 0xd8, 4, 0xd8, 4];
    expected.extend_from_slice(Address::repeat_byte(0x11).as_slice());
    expected.extend_from_slice(&[0xd8, 4, 0]);
    assert_eq!(wire, expected);
    assert_eq!(wire.len(), 31);
}

// Independently specified empty-block header. This does not call the production
// adapter, Reth's assembler, next_evm_env or chain-spec fee calculations. No `..`
// is intentional: upgrading Header requires reviewing every field here.
// The sentinel state root represents an externally authenticated replay result;
// these unit fixtures are assembly checks, not execution-valid chain fixtures.
fn expected_empty(parent: &SealedHeader, fork: EthereumHardfork) -> Block {
    let shanghai = fork >= EthereumHardfork::Shanghai;
    let cancun = fork >= EthereumHardfork::Cancun;
    let prague = fork >= EthereumHardfork::Prague;
    Block {
        header: Header {
            parent_hash: parent.hash(),
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: Address::repeat_byte(0x11),
            state_root: POST_ROOT,
            transactions_root: EMPTY_ROOT_HASH,
            receipts_root: EMPTY_ROOT_HASH,
            logs_bloom: Bloom::ZERO,
            difficulty: U256::ZERO,
            number: 11,
            gas_limit: 30_000_000,
            gas_used: 0,
            timestamp: 102,
            extra_data: Bytes::from_static(b"published inputs"),
            mix_hash: B256::repeat_byte(0x22),
            nonce: B64::ZERO,
            base_fee_per_gas: Some(980_000_000), // Empty parent: 1 gwei minus 1/50.
            withdrawals_root: shanghai.then_some(EMPTY_ROOT_HASH),
            blob_gas_used: cancun.then_some(0),
            excess_blob_gas: cancun.then_some(0),
            parent_beacon_block_root: cancun.then_some(B256::ZERO),
            requests_hash: prague.then_some(EMPTY_REQUESTS),
            block_access_list_hash: None,
            slot_number: None,
        },
        body: BlockBody {
            transactions: vec![],
            ommers: vec![],
            withdrawals: shanghai.then(Default::default),
        },
    }
}

fn assemble_empty(config: &EezEvmConfig, parent: &SealedHeader, env: &Environment) -> Block {
    reassemble(
        config,
        parent,
        env,
        vec![],
        &BlockExecutionOutput::default(),
        POST_ROOT,
        &NoopProvider::eth(config.chain_spec().clone()),
    )
    .unwrap()
}

#[test]
fn whole_block_matches_independent_fixed_osaka_fixture() {
    let config = config(EthereumHardfork::Osaka);
    let parent = parent();
    let actual = assemble_empty(&config, &parent, &environment());
    let expected = expected_empty(&parent, EthereumHardfork::Osaka);
    assert_eq!(actual, expected);
    require_exact_block(&actual, &alloy_rlp::encode(&expected)).unwrap();
}

#[test]
fn metadata_binds_assembly_and_wrong_history_randao_or_fixed_fields_are_rejected() {
    let config = config(EthereumHardfork::Osaka);
    let parent = parent();
    let original = assemble_empty(&config, &parent, &environment());
    let original_rlp = alloy_rlp::encode(&original);
    for change in [
        |env: &mut Environment| env.fee_recipient = Address::repeat_byte(0x44),
        |env: &mut Environment| env.extra_data = Bytes::from_static(b"other"),
    ] {
        let mut env = environment();
        change(&mut env);
        let changed = assemble_empty(&config, &parent, &env);
        assert!(require_exact_block(&changed, &original_rlp).is_err());
    }
    for change in [
        |h: &mut Header| h.mix_hash = B256::repeat_byte(0x55),
        |h: &mut Header| h.timestamp += 1,
        |h: &mut Header| h.gas_limit += 1,
        |h: &mut Header| h.parent_beacon_block_root = Some(B256::repeat_byte(0x66)),
    ] {
        let mut changed = original.clone();
        change(&mut changed.header);
        assert!(full_attributes(&config, &parent, &changed).is_err());
    }
}

#[test]
fn rejects_pre_merge_and_future_forks_including_boundary() {
    for fork in [EthereumHardfork::London, EthereumHardfork::Amsterdam] {
        let error = next_attributes(&config(fork), &parent(), &environment(), None).unwrap_err();
        assert!(error.to_string().contains("unsupported L2 execution"));
    }
    let mut spec = config(EthereumHardfork::Osaka)
        .chain_spec()
        .as_ref()
        .clone();
    spec.hardforks
        .insert(EthereumHardfork::Amsterdam, ForkCondition::Timestamp(102));
    let config = EezEvmConfig::new(Arc::new(spec));
    let env = environment();
    // The first child is exactly at the unsupported fork activation.
    assert!(next_attributes(&config, &parent(), &env, None).is_err());
}

#[test]
fn arithmetic_and_invalid_environment_fail_before_reth() {
    let config = config(EthereumHardfork::Osaka);
    let mut header = parent().header().clone();
    header.number = u64::MAX;
    assert!(next_attributes(&config, &header, &environment(), None).is_err());
    header.number = 10;
    header.timestamp = u64::MAX - 1;
    assert!(next_attributes(&config, &header, &environment(), None).is_err());
    let env = Environment {
        extra_data: vec![0; 33].into(),
        ..environment()
    };
    assert!(next_attributes(&config, &parent(), &env, None).is_err());
}

#[test]
fn rejects_false_parent_seal_and_receipt_count_mismatch() {
    let config = config(EthereumHardfork::Osaka);
    let provider = NoopProvider::eth(config.chain_spec().clone());
    let fake_parent = SealedHeader::new(parent().header().clone(), B256::ZERO);
    let error = reassemble(
        &config,
        &fake_parent,
        &environment(),
        vec![],
        &BlockExecutionOutput::default(),
        POST_ROOT,
        &provider,
    )
    .unwrap_err();
    assert!(error.to_string().contains("parent seal"));
    let mut output = BlockExecutionOutput::default();
    output.result.receipts.push(Receipt::default());
    let error = reassemble(
        &config,
        &parent(),
        &environment(),
        vec![],
        &output,
        POST_ROOT,
        &provider,
    )
    .unwrap_err();
    assert!(error.to_string().contains("receipt count"));
}

#[test]
fn no_header_or_body_component_is_ignored_by_equality() {
    let config = config(EthereumHardfork::Osaka);
    let actual = assemble_empty(&config, &parent(), &environment());
    // Each supplied-header field is mutated separately. This includes outputs
    // which an input-only comparison would incorrectly trust from the producer.
    let mutations: &[fn(&mut Header)] = &[
        |h| h.parent_hash = B256::ZERO,
        |h| h.ommers_hash = B256::ZERO,
        |h| h.beneficiary = Address::ZERO,
        |h| h.state_root = B256::ZERO,
        |h| h.transactions_root = B256::ZERO,
        |h| h.receipts_root = B256::ZERO,
        |h| h.logs_bloom = Bloom::repeat_byte(1),
        |h| h.difficulty = U256::from(1),
        |h| h.number += 1,
        |h| h.gas_limit += 1,
        |h| h.gas_used += 1,
        |h| h.timestamp += 1,
        |h| h.extra_data = Bytes::new(),
        |h| h.mix_hash = B256::ZERO,
        |h| h.nonce = B64::repeat_byte(1),
        |h| h.base_fee_per_gas = Some(1),
        |h| h.withdrawals_root = None,
        |h| h.blob_gas_used = Some(1),
        |h| h.excess_blob_gas = Some(1),
        |h| h.parent_beacon_block_root = Some(B256::repeat_byte(1)),
        |h| h.requests_hash = Some(B256::ZERO),
        |h| h.block_access_list_hash = Some(B256::ZERO),
        |h| h.slot_number = Some(1),
    ];
    for (index, mutate) in mutations.iter().enumerate() {
        let mut supplied = actual.clone();
        mutate(&mut supplied.header);
        assert!(
            require_exact_block(&actual, &alloy_rlp::encode(supplied)).is_err(),
            "field {index}"
        );
    }
    let mut supplied = actual.clone();
    supplied.body.ommers.push(Header::default());
    assert!(require_exact_block(&actual, &alloy_rlp::encode(supplied)).is_err());
    let mut supplied = actual.clone();
    supplied.body.withdrawals = None;
    assert!(require_exact_block(&actual, &alloy_rlp::encode(supplied)).is_err());
    let mut bytes = alloy_rlp::encode(&actual);
    bytes.push(0);
    assert!(require_exact_block(&actual, &bytes).is_err());
}

#[test]
fn state_commitment_and_parent_fee_are_taken_from_replay_and_parent() {
    let config = config(EthereumHardfork::Osaka);
    let mut full_parent = parent().header().clone();
    full_parent.gas_used = full_parent.gas_limit;
    let full_parent = SealedHeader::seal_slow(full_parent);
    let actual = reassemble(
        &config,
        &full_parent,
        &environment(),
        vec![],
        &BlockExecutionOutput::default(),
        B256::repeat_byte(0xbb),
        &NoopProvider::eth(config.chain_spec().clone()),
    )
    .unwrap();
    assert_eq!(actual.header.parent_hash, full_parent.hash());
    assert_eq!(actual.header.base_fee_per_gas, Some(1_020_000_000));
    assert_eq!(actual.header.state_root, B256::repeat_byte(0xbb));
    assert!(
        require_exact_block(
            &actual,
            &alloy_rlp::encode(expected_empty(&full_parent, EthereumHardfork::Osaka))
        )
        .is_err()
    );
}

#[test]
fn empty_derived_blocks_execute_history_but_no_beacon_updates() {
    use alloy_primitives::{address, hex};
    use reth_evm::{Evm as _, block::BlockExecutor as _, execute::BlockBuilder as _};
    use revm::{
        bytecode::Bytecode,
        database::{InMemoryDB, State, states::bundle_state::BundleRetention},
        state::AccountInfo,
    };

    // Real EIP-4788 code must remain untouched even though Osaka is active.
    let address = address!("000F3df6D732807Ef1319fB7B8bB8522d0Beac02");
    let code = Bytecode::new_raw(Bytes::from_static(&hex!(
        "3373fffffffffffffffffffffffffffffffffffffffe14604d57602036146024575f5ffd5b5f35801560495762001fff810690815414603c575f5ffd5b62001fff01545f5260205ff35b5f5ffd5b62001fff42064281555f359062001fff015500"
    )));
    let mut database = InMemoryDB::default();
    database.insert_account_info(
        address,
        AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        },
    );
    let history = alloy_eips::eip2935::HISTORY_STORAGE_ADDRESS;
    let history_code = Bytecode::new_raw(alloy_eips::eip2935::HISTORY_STORAGE_CODE.clone());
    database.insert_account_info(
        history,
        AccountInfo {
            code_hash: history_code.hash_slow(),
            code: Some(history_code),
            ..Default::default()
        },
    );
    let config = config(EthereumHardfork::Osaka);
    install_parent_history(&mut database, &config);
    let mut state = State::builder()
        .with_database(database)
        .with_bundle_update()
        .build();
    let mut parent = parent_for(EthereumHardfork::Osaka);

    // Reuse actual in-memory state for two consecutive empty derived blocks.
    // Merely advancing numbers/hashes would omit the parent-history writes.
    for timestamp in [102, 104] {
        let env = environment();
        let attrs = next_attributes(&config, &parent, &env, None).unwrap();
        assert_eq!(attrs.timestamp, timestamp);
        let mut builder = config
            .builder_for_next_block(&mut state, &parent, attrs)
            .unwrap();
        builder.apply_pre_execution_changes().unwrap();
        let (evm, result) = builder.into_executor().finish().unwrap();
        let (db, _) = evm.finish();
        db.merge_transitions(BundleRetention::PlainState);
        let output = BlockExecutionOutput {
            result,
            state: db.take_bundle(),
        };
        assert!(output.result.receipts.is_empty());
        assert_eq!(output.result.gas_used, 0);
        assert_eq!(
            output.storage(&history, U256::from(parent.number % 8191)),
            Some(U256::from_be_slice(parent.hash().as_slice()))
        );
        assert!(output.storage(&address, U256::from(timestamp)).is_none());
        assert!(
            output
                .storage(&address, U256::from(timestamp + 8191))
                .is_none()
        );
        // This unit test checks execution and assembly, not trie-root proof
        // verification. Full signer/follower integration must compute this root.
        let block = reassemble(
            &config,
            &parent,
            &env,
            vec![],
            &output,
            POST_ROOT,
            &NoopProvider::eth(config.chain_spec().clone()),
        )
        .unwrap();
        parent = SealedHeader::seal_slow(block.header);
    }
}

#[test]
fn nonempty_derived_block_uses_executed_transactions_receipts_and_gas() {
    use alloy_consensus::{SignableTransaction, TxLegacy, TxReceipt as _};
    use alloy_primitives::{Signature, TxKind, keccak256};
    use reth_evm::{Evm as _, block::BlockExecutor as _, execute::BlockBuilder as _};
    use reth_primitives_traits::SignedTransaction as _;
    use revm::{
        database::{InMemoryDB, State, states::bundle_state::BundleRetention},
        state::AccountInfo,
    };

    let recipient = Address::repeat_byte(0xa1);
    let transaction = TransactionSigned::from(
        TxLegacy {
            chain_id: Some(1),
            gas_price: 1_000_000_000,
            gas_limit: 21_000,
            to: TxKind::Call(recipient),
            value: U256::from(100),
            ..Default::default()
        }
        .into_signed(Signature::new(U256::from(1), U256::from(2), false)),
    );
    let recovered = transaction.clone().try_into_recovered().unwrap();
    let mut database = InMemoryDB::default();
    database.insert_account_info(
        recovered.signer(),
        AccountInfo {
            balance: U256::from(1_000_000_000_000_000u64),
            ..Default::default()
        },
    );
    let config = config(EthereumHardfork::Osaka);
    install_parent_history(&mut database, &config);
    let mut state = State::builder()
        .with_database(database)
        .with_bundle_update()
        .build();
    let parent = parent_for(EthereumHardfork::Osaka);
    let attrs = next_attributes(&config, &parent, &environment(), None).unwrap();
    let mut builder = config
        .builder_for_next_block(&mut state, &parent, attrs)
        .unwrap();
    builder.apply_pre_execution_changes().unwrap();
    builder.execute_transaction(recovered).unwrap();
    let (evm, result) = builder.into_executor().finish().unwrap();
    let (db, _) = evm.finish();
    db.merge_transitions(BundleRetention::PlainState);
    let output = BlockExecutionOutput {
        result,
        state: db.take_bundle(),
    };
    assert_eq!(
        output.account(&recipient).unwrap().unwrap().balance,
        U256::from(100)
    );
    assert_eq!(output.result.gas_used, 21_000);
    assert_eq!(output.result.receipts.len(), 1);
    assert!(output.result.receipts[0].success);

    let actual = reassemble(
        &config,
        &parent,
        &environment(),
        vec![transaction.clone()],
        &output,
        POST_ROOT,
        &NoopProvider::eth(config.chain_spec().clone()),
    )
    .unwrap();
    assert_eq!(actual.body.transactions, vec![transaction.clone()]);
    assert_eq!(actual.header.gas_used, 21_000);
    // Independently construct each single-leaf MPT: RLP(0) has key nibbles
    // [8, 0], so the hex-prefix encoded leaf path is [0x20, 0x80]. Legacy
    // envelopes are already their exact EIP-2718 bytes when RLP-encoded.
    let leaf_root = |value: Vec<u8>| {
        keccak256(alloy_rlp::encode(vec![
            Bytes::from_static(&[0x20, 0x80]),
            Bytes::from(value),
        ]))
    };
    assert_eq!(
        actual.header.transactions_root,
        leaf_root(alloy_rlp::encode(transaction))
    );
    assert_eq!(
        actual.header.receipts_root,
        leaf_root(alloy_rlp::encode(
            output.result.receipts[0].with_bloom_ref()
        )),
    );
    // A zero-work output cannot replace the successful replay of a nonempty
    // block, even if a caller hands the assembler the same claimed state root.
    assert!(
        reassemble(
            &config,
            &parent,
            &environment(),
            actual.body.transactions.clone(),
            &BlockExecutionOutput::default(),
            POST_ROOT,
            &NoopProvider::eth(config.chain_spec().clone()),
        )
        .is_err()
    );
}
