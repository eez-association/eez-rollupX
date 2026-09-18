use super::*;
use alloy_consensus::{BlockBody, EMPTY_OMMER_ROOT_HASH, EMPTY_ROOT_HASH};
use alloy_primitives::{Address, B64, Bloom, Bytes, U256, b256};
use reth_chainspec::{ChainSpecBuilder, EthereumHardfork, ForkCondition};
use reth_storage_api::noop::NoopProvider;
use std::sync::Arc;

const POST_ROOT: B256 = B256::repeat_byte(0x91);
const EMPTY_REQUESTS: B256 =
    b256!("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");

fn environment() -> Environment {
    Environment {
        timestamp_step_seconds: 2,
        fee_recipient: Address::repeat_byte(0x11),
        prev_randao: B256::repeat_byte(0x22),
        gas_limit: 30_000_000,
        extra_data: Bytes::from_static(b"published inputs"),
        cancun_parent_beacon_root: B256::repeat_byte(0x33),
    }
}

fn config(fork: EthereumHardfork) -> EthEvmConfig {
    let builder = ChainSpecBuilder::mainnet().reset();
    let builder = match fork {
        EthereumHardfork::London => builder.london_activated(),
        EthereumHardfork::Paris => builder.paris_activated(),
        EthereumHardfork::Shanghai => builder.shanghai_activated(),
        EthereumHardfork::Cancun => builder.cancun_activated(),
        EthereumHardfork::Prague => builder.prague_activated(),
        EthereumHardfork::Osaka => builder.osaka_activated(),
        EthereumHardfork::Amsterdam => builder.amsterdam_activated(),
        _ => panic!("unsupported test fork"),
    };
    EthEvmConfig::new(Arc::new(builder.build()))
}

fn parent() -> SealedHeader {
    SealedHeader::seal_slow(Header {
        number: 10,
        timestamp: 100,
        gas_limit: 30_000_000,
        base_fee_per_gas: Some(1_000_000_000),
        // A valid Cancun-or-later empty parent. Pre-Cancun tests strip these.
        withdrawals_root: Some(EMPTY_ROOT_HASH),
        parent_beacon_block_root: Some(B256::ZERO),
        blob_gas_used: Some(0),
        excess_blob_gas: Some(0),
        ..Default::default()
    })
}

fn parent_for(fork: EthereumHardfork) -> SealedHeader {
    let mut header = parent().header().clone();
    if fork < EthereumHardfork::Cancun {
        header.parent_beacon_block_root = None;
        header.blob_gas_used = None;
        header.excess_blob_gas = None;
    }
    if fork < EthereumHardfork::Shanghai {
        header.withdrawals_root = None;
    }
    if fork >= EthereumHardfork::Prague {
        header.requests_hash = Some(EMPTY_REQUESTS);
    }
    SealedHeader::seal_slow(header)
}

#[test]
fn writer_uses_retained_inputs_and_reanchors_after_an_environment_change() {
    use eez_payload_codec::derivable::{BlockPlan, RecordData};
    let fork = EthereumHardfork::Prague;
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
        parent = SealedHeader::seal_slow(block.header.clone());
        blocks.push(block);
    }
    let rlps = blocks.iter().map(alloy_rlp::encode).collect::<Vec<_>>();
    let payload = encode_operations(&config, &anchor, &rlps, &[vec![0, 128, 255]], &[2]).unwrap();
    assert_eq!(payload.ordinary_block_count(), 5);
    assert_eq!(payload.records().len(), 1);
    assert_eq!(payload.records()[0].empty_gap, 2);
    assert!(
        matches!(&payload.records()[0].data, RecordData::EnvironmentChange { transactions, .. } if transactions.is_empty())
    );
    for (position, plan) in payload.blocks().enumerate() {
        if let BlockPlan::Derived {
            environment,
            transactions,
            ..
        } = plan
        {
            assert_eq!(
                environment.fee_recipient,
                blocks[position].header.beneficiary
            );
            assert!(transactions.is_empty());
        }
    }
    assert_eq!(payload.l2_entries(), [vec![0, 128, 255]]);
    assert_eq!(payload.outbound_group_sizes(), [2]);
    assert_eq!(payload.terminal_block(), rlps.last().unwrap());
    let new_anchor = SealedHeader::seal_slow(blocks[1].header.clone());
    let suffix = encode_operations(&config, &new_anchor, &rlps[2..], &[], &[]).unwrap();
    assert!(suffix.records().is_empty());
    assert_eq!(
        suffix.initial_environment().fee_recipient,
        Address::repeat_byte(0xa5)
    );
    assert!(encode_operations(&config, &anchor, &rlps[2..], &[], &[]).is_err());
    assert!(encode_operations(&config, &anchor, &[], &[], &[]).is_err());
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
            base_fee_per_gas: Some(875_000_000), // Empty parent: 1 gwei minus 1/8.
            withdrawals_root: shanghai.then_some(EMPTY_ROOT_HASH),
            blob_gas_used: cancun.then_some(0),
            excess_blob_gas: cancun.then_some(0),
            parent_beacon_block_root: cancun.then_some(B256::repeat_byte(0x33)),
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

fn assemble_empty(config: &EthEvmConfig, parent: &SealedHeader, env: &Environment) -> Block {
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
fn whole_block_matches_independent_fixture_at_each_supported_fork() {
    for fork in [
        EthereumHardfork::Paris,
        EthereumHardfork::Shanghai,
        EthereumHardfork::Cancun,
        EthereumHardfork::Prague,
        EthereumHardfork::Osaka,
    ] {
        let config = config(fork);
        let parent = parent_for(fork);
        let actual = assemble_empty(&config, &parent, &environment());
        let expected = expected_empty(&parent, fork);
        assert_eq!(actual, expected, "{fork:?}");
        require_exact_block(&actual, &alloy_rlp::encode(&expected)).unwrap();
    }
}

#[test]
fn fork_crossings_use_child_timestamp_and_parent_history() {
    for (old, new) in [
        (EthereumHardfork::Paris, EthereumHardfork::Shanghai),
        (EthereumHardfork::Shanghai, EthereumHardfork::Cancun),
        (EthereumHardfork::Cancun, EthereumHardfork::Prague),
        (EthereumHardfork::Prague, EthereumHardfork::Osaka),
    ] {
        let old_config = config(old);
        let mut spec = old_config.chain_spec().as_ref().clone();
        spec.hardforks.insert(new, ForkCondition::Timestamp(102));
        let config = EthEvmConfig::new(Arc::new(spec));
        let parent = parent_for(old);
        let actual = assemble_empty(&config, &parent, &environment());
        assert_eq!(actual, expected_empty(&parent, new), "{new:?}");
        // Immediately before the transition the old optional-field shape holds.
        let mut earlier = environment();
        earlier.timestamp_step_seconds = 1;
        let before = assemble_empty(&config, &parent, &earlier);
        let mut expected_before = expected_empty(&parent, old);
        expected_before.header.timestamp = 101;
        assert_eq!(before, expected_before, "before {new:?}");
    }
}

#[test]
fn all_environment_inputs_affect_the_block_where_applicable() {
    let config = config(EthereumHardfork::Osaka);
    let parent = parent();
    let original = assemble_empty(&config, &parent, &environment());
    let original_rlp = alloy_rlp::encode(&original);
    let mut changes = vec![environment(); 6];
    changes[0].timestamp_step_seconds += 1;
    changes[1].fee_recipient = Address::repeat_byte(0x44);
    changes[2].prev_randao = B256::repeat_byte(0x55);
    changes[3].gas_limit += 1;
    changes[4].extra_data = Bytes::from_static(b"other");
    changes[5].cancun_parent_beacon_root = B256::repeat_byte(0x66);
    for env in changes {
        let changed = assemble_empty(&config, &parent, &env);
        assert!(require_exact_block(&changed, &original_rlp).is_err());
    }
    let config = self::config(EthereumHardfork::Paris);
    let before = assemble_empty(&config, &parent, &environment());
    let mut other_root = environment();
    other_root.cancun_parent_beacon_root = B256::repeat_byte(0xff);
    assert_eq!(before, assemble_empty(&config, &parent, &other_root));
    assert!(before.header.parent_beacon_block_root.is_none());
}

#[test]
fn rejects_pre_merge_and_future_forks_including_boundary() {
    for fork in [EthereumHardfork::London, EthereumHardfork::Amsterdam] {
        let error = next_attributes(&config(fork), &parent(), &environment()).unwrap_err();
        assert!(error.to_string().contains("unsupported profile-1"));
    }
    let mut spec = config(EthereumHardfork::Osaka)
        .chain_spec()
        .as_ref()
        .clone();
    spec.hardforks
        .insert(EthereumHardfork::Amsterdam, ForkCondition::Timestamp(102));
    let config = EthEvmConfig::new(Arc::new(spec));
    let mut env = environment();
    env.timestamp_step_seconds = 1;
    next_attributes(&config, &parent(), &env).unwrap();
    env.timestamp_step_seconds = 2;
    assert!(next_attributes(&config, &parent(), &env).is_err());
}

#[test]
fn arithmetic_and_invalid_environment_fail_before_reth() {
    let config = config(EthereumHardfork::Osaka);
    let mut header = parent().header().clone();
    header.number = u64::MAX;
    assert!(
        next_attributes(&config, &header, &environment())
            .unwrap_err()
            .to_string()
            .contains("number overflow")
    );
    header.number = 10;
    header.timestamp = u64::MAX - 1;
    assert!(
        next_attributes(&config, &header, &environment())
            .unwrap_err()
            .to_string()
            .contains("timestamp overflow")
    );
    for env in [
        Environment {
            timestamp_step_seconds: 0,
            ..environment()
        },
        Environment {
            gas_limit: 0,
            ..environment()
        },
        Environment {
            extra_data: vec![0; 33].into(),
            ..environment()
        },
    ] {
        assert!(next_attributes(&config, &parent(), &env).is_err());
    }
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
        |h| h.parent_beacon_block_root = Some(B256::ZERO),
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
    assert_eq!(actual.header.base_fee_per_gas, Some(1_125_000_000));
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
fn empty_derived_blocks_execute_beacon_root_storage_updates() {
    use alloy_primitives::{address, hex};
    use reth_evm::{Evm as _, block::BlockExecutor as _, execute::BlockBuilder as _};
    use revm::{
        bytecode::Bytecode,
        database::{InMemoryDB, State, states::bundle_state::BundleRetention},
        state::AccountInfo,
    };

    // EIP-4788's deployed bytecode, as pinned in alloy-eips 2.0.5. Exercising the
    // actual system call demonstrates why even a gap block must be executed.
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
    let mut state = State::builder()
        .with_database(database)
        .with_bundle_update()
        .build();
    let config = config(EthereumHardfork::Cancun);
    let mut parent = parent_for(EthereumHardfork::Cancun);

    // Reuse actual in-memory state for two consecutive empty derived blocks.
    // Merely advancing numbers/hashes would leave both of these slots unset.
    for timestamp in [102, 104] {
        let env = environment();
        let attrs = next_attributes(&config, &parent, &env).unwrap();
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
            output.storage(&address, U256::from(timestamp)),
            Some(U256::from(timestamp))
        );
        assert_eq!(
            output.storage(&address, U256::from(timestamp + 8191)),
            Some(U256::from_be_bytes(env.cancun_parent_beacon_root.0)),
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
    let mut state = State::builder()
        .with_database(database)
        .with_bundle_update()
        .build();
    let config = config(EthereumHardfork::Cancun);
    let parent = parent_for(EthereumHardfork::Cancun);
    let attrs = next_attributes(&config, &parent, &environment()).unwrap();
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
