//! Shared fixtures for the crate's test suites.
//!
//! Everything here is fixed test material, or a tiny constructor for it,
//! consumed from multiple test modules. Test support has three homes:
//! byte-identical fixtures live here, the stub validation backend lives in
//! `validate::testing`, and behavior-specific builders stay beside the test
//! suite that owns them.

use alloy_primitives::{Address, B256, address, b256};

/// Nextest supplies the relocated checkout when running archived test binaries.
pub(crate) fn manifest_dir() -> std::path::PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into())
        .into()
}

pub(crate) fn posting_context() -> eez_control_rpc::v1::L1PostingContext {
    eez_control_rpc::v1::L1PostingContext {
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
    wire: &eez_control_rpc::v1::L1PostingContext,
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
pub(crate) const LEGACY_USER_TX: &str = "f85f8001825208944200000000000000000000000000000000000007808026a0ed95c78ea14cbb6af669c61f27c5fb7fb0192101d4d706d055ab9ff9895c9f66a027c2e67303de8fa1cad36d0e59298a98df684e54295eb5f61ab99609c1738f73";

/// Canonical native wire vector: chain 1, nonce 0, EEZL2, zero value and input.
pub(crate) const SYSTEM_TX: &str = "76d901809442000000000000000000000000000000000000078080";

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

/// Exact terminal witness and semantic actions from an exported successful
/// native Prove request. The full pipeline fixture tests separately bind the
/// request digest, signature, complete window and DA carrier.
pub(crate) fn native_capture(
    name: &str,
) -> (
    alloy_genesis::Genesis,
    crate::window::AdmittedBlock,
    Vec<eez_protocol::blob_semantics::SemanticTransaction>,
) {
    use eez_control_rpc::v1::{ProveChunk, prove_chunk};
    use prost::Message as _;
    let directory = manifest_dir().join("tests/fixtures").join(name);
    let bytes = std::fs::read(directory.join("cold-request.protobuf")).unwrap();
    let mut remaining = bytes.as_slice();
    let mut chunks = Vec::new();
    while !remaining.is_empty() {
        chunks.push(ProveChunk::decode_length_delimited(&mut remaining).unwrap());
    }
    let Some(prove_chunk::Kind::Header(header)) = chunks.remove(0).kind else {
        panic!("capture header")
    };
    let post = header.post_batch.unwrap();
    let batch = crate::settlement::decode_canonical_post_batch(post.abi_calldata).unwrap();
    let da =
        crate::settlement::verify_da_carrier_with_limit(&batch, &post.blobs, header.rollup_id, 16)
            .unwrap();
    let Some(prove_chunk::Kind::Block(block)) = chunks.pop().unwrap().kind else {
        panic!("capture terminal")
    };
    let witness = block.witness.unwrap();
    (
        serde_json::from_slice(&std::fs::read(directory.join("l2-genesis.json")).unwrap()).unwrap(),
        crate::window::AdmittedBlock {
            declared_number: block.number,
            claimed_hash: B256::from_slice(&block.hash),
            claimed_parent_hash: B256::from_slice(&block.parent_hash),
            rlp: block.rlp,
            witness: alloy_rpc_types_debug::ExecutionWitness {
                state: witness.state.into_iter().map(Into::into).collect(),
                codes: witness.codes.into_iter().map(Into::into).collect(),
                keys: witness.keys.into_iter().map(Into::into).collect(),
                headers: witness.headers.into_iter().map(Into::into).collect(),
            },
        },
        da.semantic_transactions().to_vec(),
    )
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
