//! Real Osaka execution with a scheduled import at Sync and a subsequent value
//! donation to SYSTEM. Expected observation bytes are fixed independently of
//! replay and remain nonzero before finalization even though the closed root burns it.

use super::*;
use alloy_consensus::{Header, SignableTransaction as _, TxLegacy};
use alloy_genesis::{Genesis, GenesisAccount};
use alloy_primitives::{Bytes, Signature, TxKind, U256, keccak256};
use alloy_sol_types::SolCall as _;
use eez_control_rpc::v2::{BlockWitness, ExecutionWitness};
use eez_evm::EezEvmConfig;
use eez_primitives::{Block, EezTxEnvelope, SystemTransaction};
use eez_protocol::l1_history::{HISTORY_ADDRESS, HeaderChain, WitnessHistory};
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, InMemoryDB},
};
use reth_evm::{ConfigureEvm, execute::Executor as _};
use reth_primitives_traits::Block as _;
use std::sync::Arc;

pub(crate) const PROBE: Address = Address::with_last_byte(0x90);

#[derive(Default)]
pub(crate) struct CaptureScenario {
    pub accounts: Vec<(Address, GenesisAccount)>,
    pub transactions: Vec<(TxKind, Bytes)>,
    pub suffix_call: Option<(Address, Bytes)>,
    pub native_prefix: Vec<EezTxEnvelope>,
    pub omit_donation: bool,
    // Characterize the provider projection separately from EVM storage reset.
    pub retain_prior_storage_at: Option<Address>,
    pub followup_call: Option<(Address, Bytes)>,
    pub extra_empty_child: bool,
    pub expected_child_storage: Option<(Address, U256, U256)>,
}

pub(crate) struct Fixture {
    pub genesis: Genesis,
    pub blocks: Vec<BlockWitness>,
    pub origin_hashes: [B256; 2],
    pub(super) capture_states: Vec<Genesis>,
    pub(super) capture_config: EezEvmConfig,
}

/// Full account/storage proofs, including reads used only by observations.
fn witness(alloc: &Genesis) -> (B256, Vec<Bytes>, Vec<Bytes>) {
    #[derive(alloy_rlp::RlpEncodable)]
    struct Account {
        nonce: u64,
        balance: U256,
        storage_root: B256,
        code_hash: B256,
    }
    let mut nodes = Vec::new();
    let mut codes = Vec::new();
    let mut leaves = Vec::new();
    for (address, account) in &alloc.alloc {
        let (storage_root, storage) = history::trie(
            account
                .storage
                .iter()
                .flatten()
                .filter(|(_, value)| !value.is_zero())
                .map(|(key, value)| {
                    (
                        keccak256(key),
                        alloy_rlp::encode(U256::from_be_bytes(value.0)),
                    )
                })
                .collect(),
        );
        nodes.extend(storage);
        let code = account.code.clone().unwrap_or_default();
        let code_hash = keccak256(&code);
        if !code.is_empty() && !codes.contains(&code) {
            codes.push(code);
        }
        leaves.push((
            keccak256(address),
            alloy_rlp::encode(Account {
                nonce: account.nonce.unwrap_or(0),
                balance: account.balance,
                storage_root,
                code_hash,
            }),
        ));
    }
    let (root, accounts) = history::trie(leaves);
    nodes.extend(accounts);
    (root, nodes, codes)
}

fn database(alloc: &Genesis) -> InMemoryDB {
    let mut db = InMemoryDB::default();
    for (address, account) in &alloc.alloc {
        let code = Bytecode::from_stored(account.code.clone().unwrap_or_default());
        db.insert_account_info(
            address,
            AccountInfo {
                balance: account.balance,
                nonce: account.nonce.unwrap_or(0),
                code_hash: code.hash_slow(),
                code: Some(code),
                ..Default::default()
            },
        );
        for (key, value) in account.storage.iter().flatten() {
            db.insert_account_storage(
                address,
                &U256::from_be_bytes(key.0),
                &U256::from_be_bytes(value.0),
            );
        }
    }
    db
}

/// Supplement an old capture's terminal witness by replaying its complete
/// genesis-to-terminal history. Every reconstructed header must remain exact;
/// only trie/code evidence changes, never transactions or expected proofs.
/// This utility closes the gap left by captures made before manager-table
/// observation reads were collected.
pub(crate) fn complete_native_capture_witness(name: &str) {
    use eez_control_rpc::v2::client_frame;
    use prost::Message as _;
    let (genesis, terminal) = super::native_block(name, None);
    let history = super::checked_history(&[terminal]).unwrap();
    let spec = Arc::new(eez_evm::profile::chain_spec(genesis.clone()).unwrap());
    let config = EezEvmConfig::new(Arc::clone(&spec))
        .with_l1_history(Arc::new(WitnessHistory(history)))
        .unwrap();
    let mut frames = super::native_frames(name);
    let block_count = frames
        .iter()
        .filter(|frame| matches!(frame.kind, Some(client_frame::Kind::Block(_))))
        .count();
    let mut alloc = genesis;
    let mut parent = spec.genesis_header().clone();
    let mut ancestors = vec![(parent.number, parent.hash_slow())];
    let mut ordinal = 0;
    for frame in &mut frames {
        let Some(client_frame::Kind::Block(frame)) = &mut frame.kind else {
            continue;
        };
        let wire = frame.data.as_mut().unwrap();
        let block: Block = alloy_rlp::decode_exact(&wire.rlp).unwrap();
        assert_eq!(
            block.header.number,
            parent.number + 1,
            "capture must start at genesis and remain contiguous"
        );
        assert_eq!(block.header.parent_hash, parent.hash_slow());
        let (pre_root, nodes, codes) = witness(&alloc);
        assert_eq!(
            pre_root, parent.state_root,
            "pre-state at block {}",
            block.header.number
        );
        let mut database = database(&alloc);
        for &(number, hash) in &ancestors {
            database.cache.block_hashes.insert(U256::from(number), hash);
        }
        let mut db = database;
        let recovered = block.clone().try_into_recovered().unwrap();
        let output = config.executor(&mut db).execute(&recovered).unwrap();
        let bundle = &output.state;
        for (address, account) in &bundle.state {
            if let Some(info) = &account.info {
                let target = alloc.alloc.entry(*address).or_default();
                target.nonce = Some(info.nonce);
                target.balance = info.balance;
                if let Some(code) = info
                    .code
                    .as_ref()
                    .or_else(|| bundle.contracts.get(&info.code_hash))
                {
                    target.code = Some(code.original_bytes());
                }
                for (key, slot) in &account.storage {
                    target
                        .storage
                        .get_or_insert_default()
                        .insert(B256::from(*key), B256::from(slot.present_value));
                }
            } else {
                alloc.alloc.remove(address);
            }
        }
        let rebuilt = eez_driver::derivation::reassemble_full_from_replay(
            &config,
            &reth_primitives_traits::SealedHeader::seal_slow(parent.clone()),
            &block,
            &output,
            witness(&alloc).0,
        )
        .unwrap();
        assert_eq!(
            alloy_rlp::encode(&rebuilt),
            wire.rlp,
            "reconstructed block {} changed",
            wire.number
        );
        ordinal += 1;
        if ordinal == block_count {
            let witness = wire.witness.as_mut().unwrap();
            witness.state = nodes.into_iter().map(|node| node.to_vec()).collect();
            witness.codes = codes.into_iter().map(|code| code.to_vec()).collect();
        }
        parent = block.header;
        ancestors.push((parent.number, parent.hash_slow()));
    }
    assert!(ordinal > 0);
    let directory = super::manifest_dir().join("tests/fixtures").join(name);
    let mut bytes = Vec::new();
    for frame in frames {
        frame.encode_length_delimited(&mut bytes).unwrap();
    }
    let mut response: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("response.json")).unwrap()).unwrap();
    eprintln!(
        "{name}: reconstructed {ordinal} blocks; request {} -> {} bytes, {} -> {}",
        response["request_bytes"],
        bytes.len(),
        response["request_hash"],
        keccak256(&bytes)
    );
    response["request_bytes"] = bytes.len().into();
    response["request_hash"] = serde_json::json!(keccak256(&bytes));
    std::fs::write(directory.join("request.v2.protobuf"), bytes).unwrap();
    std::fs::write(
        directory.join("response.json"),
        serde_json::to_vec_pretty(&response).unwrap(),
    )
    .unwrap();
}

impl Fixture {
    pub(crate) fn new(native_suffix: bool) -> Self {
        Self::with_probe_code(native_suffix, None)
    }

    pub(crate) fn reverting_probe() -> Self {
        // REVERT with the exact four-byte payload deadbeef.
        Self::with_probe_code(
            false,
            Some(hex::decode("63deadbeef5f526004601cfd").unwrap()),
        )
    }

    pub(crate) fn storage_probe() -> Self {
        // Nonempty calldata writes slot zero; empty calldata reads it. Both
        // return the slot as a 32-byte word, permitting genuine STATICCALL reads.
        Self::with_probe_code(
            false,
            Some(hex::decode("36156009575f355f555b5f545f5260205ff3").unwrap()),
        )
    }

    pub(crate) fn with_probe_code(native_suffix: bool, probe_code: Option<Vec<u8>>) -> Self {
        Self::with_capture_scenario(native_suffix, probe_code, CaptureScenario::default())
    }

    pub(crate) fn with_capture_scenario(
        native_suffix: bool,
        probe_code: Option<Vec<u8>>,
        scenario: CaptureScenario,
    ) -> Self {
        let mut genesis = history::genesis();
        genesis.config.extra_fields.get_mut("rollupX").unwrap()["historyUpdateOffset"] = 5.into();
        let first = history::l1_spec().genesis_header().clone();
        let second = Header {
            number: 1,
            parent_hash: first.hash_slow(),
            timestamp: 80,
            ..first.clone()
        };
        let raw = vec![alloy_rlp::encode(&first), alloy_rlp::encode(&second)];
        let mut chain = HeaderChain::default();
        chain
            .append(
                &raw.iter().cloned().map(Into::into).collect::<Vec<_>>(),
                &super::l1_header_rules(),
                16,
            )
            .unwrap();
        genesis.alloc.get_mut(&HISTORY_ADDRESS).unwrap().storage =
            Some(eez_protocol::l1_history::genesis_storage());
        // Return ABI(SYSTEM.balance, current L1 number, current L1 hash).
        // The transfer changes the first word until the block closes; history
        // import changes the other two. Closed-state RPC reads would see zero.
        let mut code = vec![0x73];
        code.extend_from_slice(eez_primitives::SYSTEM_ADDRESS.as_slice());
        code.extend_from_slice(&hex::decode("315f5263").unwrap());
        code.extend_from_slice(&keccak256("getLatestImportedL1Block()")[..4]);
        code.extend_from_slice(&hex::decode("602052604060206004603c73").unwrap());
        code.extend_from_slice(HISTORY_ADDRESS.as_slice());
        code.extend_from_slice(&hex::decode("5afa5060605ff3").unwrap());
        genesis.alloc.insert(
            PROBE,
            GenesisAccount {
                nonce: Some(1),
                code: Some(probe_code.unwrap_or(code).into()),
                ..Default::default()
            },
        );
        let key =
            alloy_signer_local::PrivateKeySigner::from_bytes(&B256::repeat_byte(0x11)).unwrap();
        genesis.alloc.insert(
            key.address(),
            GenesisAccount {
                balance: U256::from(10u64).pow(U256::from(18)),
                ..Default::default()
            },
        );
        genesis.alloc.extend(scenario.accounts);
        let spec = Arc::new(eez_evm::profile::chain_spec(genesis.clone()).unwrap());
        let config = EezEvmConfig::new(Arc::clone(&spec))
            .with_l1_history(Arc::new(WitnessHistory(Arc::new(chain))))
            .unwrap();
        let mut parent = spec.genesis_header().clone();
        let mut alloc = genesis.clone();
        assert_eq!(witness(&alloc).0, parent.state_root);
        let mut ancestors = vec![alloy_rlp::encode(&parent)];
        let mut blocks = Vec::new();
        let mut capture_states = Vec::new();
        assert!(!scenario.extra_empty_child || scenario.followup_call.is_some());
        let last = if scenario.followup_call.is_some() {
            7
        } else {
            6
        };
        for number in 1..=last + u64::from(scenario.extra_empty_child) {
            capture_states.push(alloc.clone());
            let (pre_root, nodes, codes) = witness(&alloc);
            assert_eq!(pre_root, parent.state_root);
            let mut db = database(&alloc);
            let timestamp = 80 + 2 * number;
            let base_fee = parent
                .next_block_base_fee(eez_protocol::l2::BASE_FEE_PARAMS)
                .unwrap();
            let (_, history_tx) = config.prepare_history(&mut db, &parent, timestamp).unwrap();
            let mut transactions: Vec<EezTxEnvelope> = history_tx.into_iter().collect();
            if number == 6 {
                assert_eq!(transactions.len(), 1);
                transactions.extend(scenario.native_prefix.iter().cloned());
                for (nonce, (to, input)) in scenario.transactions.iter().enumerate() {
                    let tx = TxLegacy {
                        chain_id: Some(1),
                        nonce: nonce as u64,
                        gas_limit: 1_000_000,
                        gas_price: u128::from(base_fee),
                        to: *to,
                        input: input.clone(),
                        ..Default::default()
                    };
                    let signature = Signature::from(
                        key.credential()
                            .sign_prehash_recoverable(tx.signature_hash().as_slice())
                            .unwrap(),
                    );
                    transactions.push(tx.into_signed(signature).into());
                }
                if !scenario.omit_donation {
                    let tx = TxLegacy {
                        chain_id: Some(1),
                        nonce: scenario.transactions.len() as u64,
                        gas_limit: 200_000,
                        gas_price: u128::from(base_fee),
                        to: TxKind::Call(eez_primitives::SYSTEM_ADDRESS),
                        value: U256::from(7),
                        ..Default::default()
                    };
                    let sig = Signature::from(
                        key.credential()
                            .sign_prehash_recoverable(tx.signature_hash().as_slice())
                            .unwrap(),
                    );
                    transactions.push(tx.into_signed(sig).into());
                }
                if native_suffix {
                    transactions.push(
                        SystemTransaction {
                            chain_id: 1,
                            source_hash: B256::with_last_byte(1),
                            gas_limit: eez_primitives::SYSTEM_TX_GAS_CAP,
                            to: eez_primitives::EEZL2_ADDRESS,
                            value: U256::ZERO,
                            input: eez_protocol::abi::loadExecutionTableCall {
                                _entries: vec![],
                                _staticEntries: vec![],
                            }
                            .abi_encode()
                            .into(),
                        }
                        .into(),
                    );
                    // Complete the outbound load/user group; a checkpoint cannot
                    // end on a load. The no-op user does not change the probe.
                    let user = TxLegacy {
                        chain_id: Some(1),
                        nonce: scenario.transactions.len() as u64
                            + u64::from(!scenario.omit_donation),
                        gas_limit: if scenario.suffix_call.is_some() {
                            200_000
                        } else {
                            21_000
                        },
                        gas_price: u128::from(base_fee),
                        to: TxKind::Call(
                            scenario
                                .suffix_call
                                .as_ref()
                                .map_or(Address::with_last_byte(0x91), |(address, _)| *address),
                        ),
                        input: scenario
                            .suffix_call
                            .as_ref()
                            .map_or_else(Bytes::new, |(_, input)| input.clone()),
                        ..Default::default()
                    };
                    let signature = Signature::from(
                        key.credential()
                            .sign_prehash_recoverable(user.signature_hash().as_slice())
                            .unwrap(),
                    );
                    transactions.push(user.into_signed(signature).into());
                }
            }
            if number == 7 {
                let (target, input) = scenario.followup_call.as_ref().unwrap();
                let tx = TxLegacy {
                    chain_id: Some(1),
                    nonce: alloc.alloc[&key.address()].nonce.unwrap_or(0),
                    gas_limit: 1_000_000,
                    gas_price: u128::from(base_fee),
                    to: TxKind::Call(*target),
                    input: input.clone(),
                    ..Default::default()
                };
                let signature = Signature::from(
                    key.credential()
                        .sign_prehash_recoverable(tx.signature_hash().as_slice())
                        .unwrap(),
                );
                transactions.push(tx.into_signed(signature).into());
                assert_eq!(
                    transactions.len(),
                    1,
                    "follow-up must touch the scenario account"
                );
            }
            let header = Header {
                number,
                timestamp,
                parent_hash: parent.hash_slow(),
                gas_limit: eez_protocol::l2::GAS_LIMIT,
                base_fee_per_gas: Some(base_fee),
                withdrawals_root: Some(alloy_consensus::EMPTY_ROOT_HASH),
                parent_beacon_block_root: Some(B256::ZERO),
                blob_gas_used: Some(0),
                excess_blob_gas: Some(0),
                requests_hash: Some(alloy_eips::eip7685::EMPTY_REQUESTS_HASH),
                ..Default::default()
            };
            let block = Block::new(
                header,
                alloy_consensus::BlockBody {
                    transactions,
                    withdrawals: Some(Default::default()),
                    ..Default::default()
                },
            );
            let mut block = block;
            let output = if scenario.native_prefix.is_empty() {
                let recovered = block.clone().try_into_recovered().unwrap();
                config.executor(&mut db).execute(&recovered).unwrap()
            } else {
                use alloy_consensus::transaction::{Recovered, SignerRecoverable as _};
                use reth_evm::BlockExecutor as _;
                let sealed = reth_primitives_traits::SealedBlock::seal_slow(block.clone());
                let mut executor = config.executor_for_block(&mut db, &sealed).unwrap();
                executor.apply_pre_execution_changes().unwrap();
                for tx in &mut block.body.transactions {
                    if let EezTxEnvelope::System(native) = tx {
                        let mut rebound = (**native).clone();
                        rebound.gas_limit = eez_primitives::system_tx_gas_limit(
                            block.header.gas_limit,
                            executor
                                .receipts()
                                .last()
                                .map_or(0, |r| r.cumulative_gas_used),
                        );
                        *native = alloy_primitives::Sealed::new(rebound);
                    }
                    let recovered =
                        Recovered::new_unchecked(tx.clone(), tx.recover_signer().unwrap());
                    let pending = executor
                        .execute_transaction_without_commit(recovered)
                        .unwrap();
                    executor.commit_transaction(pending).unwrap();
                }
                executor.finish().unwrap()
            };
            let bundle = &output.state;
            if number == 7
                && let Some((address, slot, expected)) = scenario.expected_child_storage
            {
                let change = &bundle.state[&address].storage[&slot];
                assert_eq!(change.original_value(), U256::ZERO);
                assert_eq!(
                    change.present_value, expected,
                    "child must execute the observed storage write"
                );
            }
            for (address, account) in &bundle.state {
                if let Some(info) = &account.info {
                    let target = alloc.alloc.entry(*address).or_default();
                    target.nonce = Some(info.nonce);
                    target.balance = info.balance;
                    if let Some(code) = info
                        .code
                        .as_ref()
                        .or_else(|| bundle.contracts.get(&info.code_hash))
                    {
                        target.code = Some(code.original_bytes());
                    }
                    if account.status.is_storage_known()
                        && scenario.retain_prior_storage_at != Some(*address)
                    {
                        target.storage = None;
                    }
                    for (key, slot) in &account.storage {
                        target
                            .storage
                            .get_or_insert_default()
                            .insert(B256::from(*key), B256::from(slot.present_value));
                    }
                } else {
                    alloc.alloc.remove(address);
                }
            }
            assert_eq!(
                alloc
                    .alloc
                    .get(&eez_primitives::SYSTEM_ADDRESS)
                    .map_or(U256::ZERO, |account| account.balance),
                U256::ZERO,
                "closed block must burn the ordinary donation; observation still expects seven"
            );
            let block = eez_driver::derivation::reassemble_full_from_replay(
                &config,
                &reth_primitives_traits::SealedHeader::seal_slow(parent.clone()),
                &block,
                &output,
                witness(&alloc).0,
            )
            .unwrap();
            parent = block.header.clone();
            blocks.push(BlockWitness {
                number,
                hash: parent.hash_slow().to_vec(),
                parent_hash: parent.parent_hash.to_vec(),
                rlp: alloy_rlp::encode(block),
                witness: Some(ExecutionWitness {
                    state: nodes.into_iter().map(|b| b.to_vec()).collect(),
                    codes: codes.into_iter().map(|b| b.to_vec()).collect(),
                    headers: ancestors.clone(),
                    l1_headers_rlp: if number == 1 { raw.clone() } else { vec![] },
                    keys: vec![],
                }),
            });
            ancestors.push(alloy_rlp::encode(&parent));
        }
        Self {
            genesis,
            blocks,
            origin_hashes: [B256::ZERO, second.hash_slow()],
            capture_states,
            capture_config: config,
        }
    }

    /// Capture through the production driver, using exact parent state and real
    /// trie witnesses. The returned feed is consumed unchanged by the signer.
    pub(crate) fn captured(
        &self,
        observations: &[eez_driver::observation::BlockObservation],
    ) -> (
        Vec<BlockWitness>,
        Vec<eez_driver::observation::ObservedOutcome>,
    ) {
        use alloy_rlp::Decodable as _;
        let mut blocks = Vec::new();
        let mut outcomes = Vec::new();
        for (index, original) in self.blocks.iter().enumerate() {
            let provider = super::capture_provider::Provider::new(
                &self.capture_states[index],
                &original.witness.as_ref().unwrap().headers,
            );
            let mut raw = original.rlp.as_slice();
            let block = Block::decode(&mut raw)
                .unwrap()
                .try_into_recovered()
                .unwrap();
            assert!(raw.is_empty());
            let claims = if index + 1 == self.blocks.len() {
                observations
            } else {
                &[]
            };
            let (captured, observed) = eez_driver::witness::block_witness_with_observations(
                &provider,
                &self.capture_config,
                &block,
                eez_driver::witness::ExecutionWitnessMode::Legacy,
                claims,
            )
            .unwrap();
            assert_eq!(captured.rlp.as_ref(), original.rlp.as_slice());
            let witness = captured.witness;
            blocks.push(BlockWitness {
                number: captured.number,
                hash: captured.hash.to_vec(),
                parent_hash: captured.parent_hash.to_vec(),
                rlp: captured.rlp.to_vec(),
                witness: Some(ExecutionWitness {
                    state: witness.state.into_iter().map(|b| b.to_vec()).collect(),
                    codes: witness.codes.into_iter().map(|b| b.to_vec()).collect(),
                    keys: witness.keys.into_iter().map(|b| b.to_vec()).collect(),
                    headers: witness.headers.into_iter().map(|b| b.to_vec()).collect(),
                    l1_headers_rlp: captured
                        .l1_headers_rlp
                        .into_iter()
                        .map(|b| b.to_vec())
                        .collect(),
                }),
            });
            outcomes.extend(observed);
        }
        (blocks, outcomes)
    }

    pub(crate) fn outcome(&self, balance: u64, origin: usize) -> Bytes {
        [
            U256::from(balance).to_be_bytes::<32>(),
            U256::from(origin).to_be_bytes::<32>(),
            self.origin_hashes[origin].0,
        ]
        .concat()
        .into()
    }

    pub(crate) fn admitted(&self) -> Vec<crate::window::AdmittedBlock> {
        self.blocks
            .iter()
            .cloned()
            .map(|block| {
                crate::window::AdmittedBlock::from_wire(
                    block,
                    &mut 0,
                    crate::window::WindowLimits {
                        blocks: 16,
                        payload_bytes: 16 * 1024 * 1024,
                        witness_items: 100_000,
                    },
                )
                .unwrap()
            })
            .collect()
    }
}
