//! Shared fixtures for the crate's test suites.
//!
//! Everything here is fixed test material, or a tiny constructor for it,
//! consumed from multiple test modules. Test support has three homes:
//! byte-identical fixtures live here, the stub validation backend lives in
//! `validate::testing`, and behavior-specific builders stay beside the test
//! suite that owns them.

use alloy_primitives::{Address, B256, address, b256};

pub(crate) mod history;

/// Nextest supplies the relocated checkout when running archived test binaries.
pub(crate) fn manifest_dir() -> std::path::PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into())
        .into()
}

pub(crate) fn posting_context() -> eez_control_rpc::v2::L1PostingContext {
    eez_control_rpc::v2::L1PostingContext {
        chain_id: 1,
        parent_number: 99,
        parent_hash: B256::with_last_byte(0xab).to_vec(),
        target_timestamp: 1_710_338_135, // Mainnet Cancun activation.
    }
}

pub(crate) fn l1_target() -> eez_protocol::public_inputs::L1PostingContext {
    target_from_wire(&posting_context())
}

pub(crate) fn target_from_wire(
    wire: &eez_control_rpc::v2::L1PostingContext,
) -> eez_protocol::public_inputs::L1PostingContext {
    eez_protocol::public_inputs::L1PostingContext {
        chain_id: wire.chain_id,
        parent_number: wire.parent_number,
        parent_hash: B256::from_slice(&wire.parent_hash),
        target_timestamp: wire.target_timestamp,
    }
}

pub(crate) fn l1_chain_spec() -> std::sync::Arc<reth_chainspec::ChainSpec> {
    reth_chainspec::MAINNET.clone()
}

/// Local timing/header fixture for controlled worker tests. Its stub backend
/// supplies execution output; it never establishes a real-network proof oracle.
pub(crate) fn controlled_l1_header() -> alloy_consensus::Header {
    alloy_consensus::Header {
        number: 20_000_000,
        timestamp: 1_710_338_400,
        base_fee_per_gas: Some(0),
        withdrawals_root: Some(alloy_consensus::EMPTY_ROOT_HASH),
        blob_gas_used: Some(0),
        excess_blob_gas: Some(0),
        parent_beacon_block_root: Some(B256::ZERO),
        ..Default::default()
    }
}

pub(crate) fn controlled_genesis_header() -> alloy_consensus::Header {
    alloy_consensus::Header {
        timestamp: controlled_l1_header().timestamp + 12,
        ..Default::default()
    }
}

pub(crate) fn controlled_schedule() -> eez_protocol::l2::Schedule {
    let reference = controlled_l1_header();
    eez_protocol::l2::Schedule::new(
        eez_protocol::l2::DerivationConfig {
            block_interval_seconds: 2,
            sync_interval_blocks: 6,
            history_update_offset: 2,
            l1_reference: eez_protocol::l2::GenesisL1Reference {
                chain_id: 1,
                number: reference.number,
                hash: reference.hash_slow(),
                timestamp: reference.timestamp,
                prev_randao: reference.mix_hash,
            },
        },
        &controlled_genesis_header(),
    )
    .unwrap()
}

/// Fixed deployment metadata for settlement/hash unit tests. This is not
/// authenticated ancestry; full service tests load captured deployment profiles.
pub(crate) fn offline_context(
    batch: &eez_protocol::EvmBatch,
    posting: eez_protocol::public_inputs::L1PostingContext,
) -> eez_protocol::l1_history::OfflineContext {
    use eez_protocol::l1_history::{
        EvidenceAnchor, HISTORY_RUNTIME, OfflineContext, OfflineProfile,
    };
    let profile = OfflineProfile {
        l1ChainId: alloy_primitives::U256::from(posting.chain_id),
        l2ChainId: alloy_primitives::U256::from(1),
        genesisHash: B256::repeat_byte(0x11),
        genesisTimestamp: 12,
        blockInterval: 2,
        syncInterval: 6,
        updateOffset: 2,
        l1ConfigHash: B256::repeat_byte(0x22),
        historyCodeHash: alloy_primitives::keccak256(HISTORY_RUNTIME),
        ruleTimes: vec![0],
    };
    let pin = batch.expectedRootPerRollup.first();
    OfflineContext::new(
        &profile,
        Address::repeat_byte(0x33),
        Address::repeat_byte(0x44),
        pin.map_or(1, |pin| pin.rollupId),
        pin.map_or(B256::repeat_byte(0x04), |pin| pin.root),
        if batch.blockNumber == u64::MAX {
            EvidenceAnchor::ActualParent {
                number: posting.parent_number,
                hash: posting.parent_hash,
                target_timestamp: posting.target_timestamp,
            }
        } else {
            EvidenceAnchor::Historical {
                number: batch.blockNumber,
                hash: posting.parent_hash,
            }
        },
        posting.target_timestamp.checked_sub(1).unwrap(),
    )
    .unwrap()
}

/// Native private fixtures activate all supported execution-header fields at genesis.
pub(crate) fn l1_header_rules() -> eez_protocol::l1_history::HeaderRules {
    eez_protocol::l1_history::HeaderRules {
        london_block: Some(0),
        shanghai_time: Some(0),
        cancun_time: Some(0),
        prague_time: Some(0),
        unsupported_time: None,
    }
}

pub(crate) fn checked_history(
    blocks: &[crate::window::AdmittedBlock],
) -> Result<std::sync::Arc<eez_protocol::l1_history::HeaderChain>, crate::validate::ValidationError>
{
    let mut history = eez_protocol::l1_history::HeaderChain::default();
    for block in blocks {
        let mut segment = block.l1_headers_rlp.as_slice();
        if let (Ok(endpoint), Some(first)) = (history.endpoint(), segment.first())
            && alloy_primitives::keccak256(first) == endpoint.hash()
        {
            segment = &segment[1..];
        }
        history
            .append(segment, &l1_header_rules(), 131_072)
            .map_err(|error| crate::validate::ValidationError::Rejected(error.to_string()))?;
    }
    Ok(std::sync::Arc::new(history))
}

/// Deterministic system-transaction identity used only by tests.
pub(crate) const TEST_SYSTEM_ADDRESS_ARG: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0076";

/// Parsed form of [`TEST_SYSTEM_ADDRESS_ARG`] for transaction and policy tests.
pub(crate) const TEST_SYSTEM_ADDRESS: Address =
    address!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0076");

/// Public Anvil account #0 key, used only for ordinary signature tests.
pub(crate) const LEGACY_USER_PRIVATE_KEY: B256 =
    b256!("ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80");

/// Legacy Ethereum transaction from the public Anvil account #0, used to
/// exercise ordinary signature recovery and historical block decoding.
/// Chain 1, nonce 0, gas price 1, gas 21000, to EEZL2, no value or input;
/// reproduce with `cast mktx --legacy --private-key <LEGACY_USER_PRIVATE_KEY>
/// --chain 1 --nonce 0 --gas-price 1 --gas-limit 21000 <EEZL2_ADDRESS>`.
pub(crate) const LEGACY_USER_TX: &str = "f85f8001825208944b2760f84b83d6b3829e30a7573b0172b6d6402c808025a0e89cbbd24d25863a59a811d77b98a9c2ad4c9ea8712c5f04eeae1b12e1cf53e6a03477c662e64a29190132f5c647ee0e0f40e1589959cdac9951f1afab86652d04";

/// Canonical native wire vector: chain 1, nonce 0, EEZL2, zero value and input.
pub(crate) const SYSTEM_TX: &str = "76d90180944b2760f84b83d6b3829e30a7573b0172b6d6402c8080";

/// Canonical context for reconstructing system transactions in tests; tests
/// that need a noncanonical variant mutate one field of a fresh copy.
pub(crate) fn system_transaction_context() -> eez_protocol::system_tx::SystemTxContext {
    eez_protocol::system_tx::SystemTxContext {
        eezl2_address: crate::EEZL2_ADDRESS,
        l2_chain_id: 1,
        this_rollup_id: 1,
    }
}

pub(crate) fn test_proof_system_vkey() -> crate::attest::NonZeroProofSystemVkey {
    crate::attest::NonZeroProofSystemVkey::new(B256::repeat_byte(0x42)).unwrap()
}

pub(crate) fn checkpoint(
    transaction_index: usize,
    block_hash: B256,
) -> crate::validate::TransactionStateCheckpoint {
    crate::validate::TransactionStateCheckpoint {
        transaction_index,
        block_hash,
    }
}

/// Complete captured v2 exchange. Its digest and byte count are fixed by the
/// producing native process, independently of this replay helper.
pub(crate) fn native_frames(name: &str) -> Vec<eez_control_rpc::v2::ClientFrame> {
    use prost::Message as _;
    let directory = manifest_dir().join("tests/fixtures").join(name);
    let bytes = std::fs::read(directory.join("request.v2.protobuf")).unwrap();
    let response: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("response.json")).unwrap()).unwrap();
    assert_eq!(
        bytes.len() as u64,
        response["request_bytes"].as_u64().unwrap()
    );
    assert_eq!(
        alloy_primitives::keccak256(&bytes),
        response["request_hash"]
            .as_str()
            .unwrap()
            .parse::<B256>()
            .unwrap()
    );
    let mut remaining = bytes.as_slice();
    let mut frames = Vec::new();
    while !remaining.is_empty() {
        frames.push(
            eez_control_rpc::v2::ClientFrame::decode_length_delimited(&mut remaining).unwrap(),
        );
    }
    frames
}

/// One independently executable block, with the complete L1 prefix supplied by
/// its original session. The index is zero-based; `None` selects the terminal.
pub(crate) fn native_block(
    name: &str,
    index: Option<usize>,
) -> (alloy_genesis::Genesis, crate::window::AdmittedBlock) {
    use eez_control_rpc::v2::client_frame;
    let mut blocks = native_frames(name)
        .into_iter()
        .filter_map(|frame| match frame.kind {
            Some(client_frame::Kind::Block(block)) => block.data,
            _ => None,
        })
        .collect::<Vec<_>>();
    let index = index.unwrap_or(blocks.len() - 1);
    let mut l1 = Vec::new();
    for block in &blocks[..=index] {
        l1.extend(
            block
                .witness
                .as_ref()
                .unwrap()
                .l1_headers_rlp
                .iter()
                .cloned(),
        );
    }
    let mut block = blocks.swap_remove(index);
    block.witness.as_mut().unwrap().l1_headers_rlp = l1;
    let admitted = crate::window::AdmittedBlock::from_wire(
        block,
        &mut 0,
        crate::window::WindowLimits {
            blocks: 65_536,
            payload_bytes: 64 * 1024 * 1024,
            witness_items: 1_000_000,
        },
    )
    .unwrap();
    let genesis = serde_json::from_slice(
        &std::fs::read(
            manifest_dir()
                .join("tests/fixtures")
                .join(name)
                .join("l2-genesis.json"),
        )
        .unwrap(),
    )
    .unwrap();
    (genesis, admitted)
}

/// Exact terminal witness and semantic actions from a successful native v2
/// request. Complete-service tests separately validate its signature and range.
pub(crate) fn native_capture(
    name: &str,
) -> (
    alloy_genesis::Genesis,
    crate::window::AdmittedBlock,
    Vec<eez_protocol::blob_semantics::SemanticTransaction>,
) {
    use eez_control_rpc::v2::client_frame;
    let mut frames = native_frames(name);
    let Some(client_frame::Kind::Begin(begin)) = frames.remove(0).kind else {
        panic!("capture Begin")
    };
    let Some(client_frame::Kind::Finalize(finalize)) = frames.pop().unwrap().kind else {
        panic!("capture Finalize")
    };
    let post = finalize.settlement.unwrap();
    let batch = crate::settlement::decode_canonical_post_batch(post.abi_calldata).unwrap();
    let da =
        crate::settlement::verify_da_carrier_with_limit(&batch, &post.blobs, begin.rollup_id, 16)
            .unwrap();
    let (genesis, block) = native_block(name, None);
    (genesis, block, da.semantic_transactions().to_vec())
}

/// Encode ordinary prefixes with default metadata for synthetic settlement tests.
/// Action transactions live in the semantic stream and must not be supplied here.
pub(crate) fn encode_da_payload(blocks: &[Vec<Vec<u8>>]) -> Vec<u8> {
    use eez_payload_codec::v0::{BlockRun, encode};
    let transactions = blocks
        .iter()
        .map(|block| {
            block
                .iter()
                .cloned()
                .map(alloy_primitives::Bytes::from)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    encode(
        &transactions
            .iter()
            .map(|transactions| BlockRun {
                blocks: 1,
                beneficiary: Address::ZERO,
                extra_data: &[],
                transactions,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap()
}
