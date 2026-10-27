//! Independent trie fixtures for nonce, receipt and checkpoint regressions.
//! The expected state is assembled from account values and the fixed schedule,
//! without executing the EVM, history contract or checkpoint implementation.

use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use eez_protocol::l1_history::{
    HISTORY_ADDRESS, HISTORY_RUNTIME, HistorySchedule, NEXT_CUTOFF_SLOT, bootstrap_storage,
};
use reth_trie_common::{HashBuilder, Nibbles, proof::ProofRetainer};
use std::sync::Arc;

pub(crate) fn l1_spec() -> Arc<reth_chainspec::ChainSpec> {
    let mut genesis: alloy_genesis::Genesis = serde_json::from_slice(
        &std::fs::read(
            super::manifest_dir()
                .join("tests/fixtures/native-offline-bidirectional/l1-genesis.json"),
        )
        .unwrap(),
    )
    .unwrap();
    genesis.timestamp = 68;
    Arc::new(reth_chainspec::ChainSpec::from(genesis))
}

pub(crate) fn l1_headers() -> Vec<Vec<u8>> {
    let reference = l1_spec().genesis_header().clone();
    let endpoint = alloy_consensus::Header {
        number: 1,
        parent_hash: reference.hash_slow(),
        timestamp: reference.timestamp + 9 * 12,
        ..reference.clone()
    };
    vec![alloy_rlp::encode(reference), alloy_rlp::encode(endpoint)]
}

pub(crate) fn genesis() -> alloy_genesis::Genesis {
    let mut genesis: alloy_genesis::Genesis = serde_json::from_slice(
        &std::fs::read(
            super::manifest_dir()
                .join("tests/fixtures/native-offline-bidirectional/l2-genesis.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let l1 = l1_spec();
    let reference = l1.genesis_header();
    genesis.timestamp = 80;
    genesis.config.extra_fields.insert("rollupX".into(), serde_json::json!({
        "feeCollector": "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266",
        "blockIntervalSeconds": 2, "syncIntervalBlocks": 6, "historyUpdateOffset": 2,
        "l1Reference": { "chainId": l1.chain().id(), "number": 0,
            "hash": reference.hash_slow(), "timestamp": reference.timestamp, "prevRandao": reference.mix_hash }
    }));
    genesis.alloc.get_mut(&HISTORY_ADDRESS).unwrap().code =
        Some(Bytes::copy_from_slice(HISTORY_RUNTIME));
    genesis.alloc.get_mut(&HISTORY_ADDRESS).unwrap().storage = Some(
        bootstrap_storage(
            0,
            reference.hash_slow(),
            HistorySchedule::new(80, 2, 6, 2).unwrap(),
        )
        .unwrap()
        .into_iter()
        .collect(),
    );
    genesis
}

pub(super) fn trie(mut leaves: Vec<(B256, Vec<u8>)>) -> (B256, Vec<Bytes>) {
    leaves.sort_unstable_by_key(|(key, _)| *key);
    assert!(leaves.windows(2).all(|pair| pair[0].0 != pair[1].0));
    let retainer = ProofRetainer::new(leaves.iter().map(|(key, _)| Nibbles::unpack(key)).collect());
    let mut builder = HashBuilder::default().with_proof_retainer(retainer);
    for (key, value) in leaves {
        builder.add_leaf(Nibbles::unpack(key), &value);
    }
    let root = builder.root();
    (
        root,
        builder
            .take_proof_nodes()
            .into_nodes_sorted()
            .into_iter()
            .map(|(_, node)| node)
            .collect(),
    )
}

/// Complete account/storage witness and independent root at the given L2 time.
/// The only produced source before every tested cutoff is the bootstrap header;
/// scheduled updates advance `nextCutoff` without adding a duplicate hash.
pub(crate) fn state(accounts: &[(Address, u64, U256)], timestamp: u64) -> (B256, Vec<Bytes>) {
    #[derive(alloy_rlp::RlpEncodable)]
    struct Account {
        nonce: u64,
        balance: U256,
        storage_root: B256,
        code_hash: B256,
    }
    let reference = l1_spec().genesis_header().hash_slow();
    let schedule = HistorySchedule::new(80, 2, 6, 2).unwrap();
    let mut storage = bootstrap_storage(0, reference, schedule).unwrap();
    // T0=80, I=12, delay=6. This arithmetic is independent of execution().
    let next = if timestamp < 86 {
        80
    } else {
        80 + ((timestamp - 86) / 12 + 1) * 12
    };
    storage.insert(U256::from(NEXT_CUTOFF_SLOT).into(), U256::from(next).into());
    let (storage_root, mut nodes) = trie(
        storage
            .into_iter()
            .filter(|(_, value)| !value.is_zero())
            .map(|(slot, value)| {
                (
                    keccak256(slot),
                    alloy_rlp::encode(U256::from_be_bytes(value.0)),
                )
            })
            .collect(),
    );
    let mut leaves = accounts
        .iter()
        .map(|(address, nonce, balance)| {
            assert_ne!(*address, HISTORY_ADDRESS);
            (
                keccak256(address),
                alloy_rlp::encode(Account {
                    nonce: *nonce,
                    balance: *balance,
                    storage_root: alloy_consensus::EMPTY_ROOT_HASH,
                    code_hash: alloy_primitives::KECCAK256_EMPTY,
                }),
            )
        })
        .collect::<Vec<_>>();
    leaves.push((
        keccak256(HISTORY_ADDRESS),
        alloy_rlp::encode(Account {
            nonce: 1,
            balance: U256::ZERO,
            storage_root,
            code_hash: keccak256(HISTORY_RUNTIME),
        }),
    ));
    let (root, accounts) = trie(leaves);
    nodes.extend(accounts);
    (root, nodes)
}
