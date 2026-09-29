//! Captured protocol and wire-format regressions.

use alloy_primitives::B256;
use eez_control_rpc::v1::{
    BlockWitness, ExecutionWitness, PostBatch, ProveChunk, ProveHeader, prove_chunk,
};

use super::expected_rollup_id;
use crate::cancel::CancellationToken;
use crate::testkit::TEST_SYSTEM_ADDRESS;
use crate::{settlement, validate};

const PINNED_PROTOCOL_COMMIT: &str = "6fcc90b65063831cb7797e9fa361004064d28f9f";

fn fixture_hex(value: &str) -> Vec<u8> {
    let value = value.trim();
    hex::decode(value.strip_prefix("0x").unwrap_or(value)).unwrap()
}

fn fixture(dir: &str, name: &str) -> String {
    let path = crate::testkit::manifest_dir()
        .join("tests/fixtures")
        .join(dir)
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read fixture {}: {error}", path.display()))
}

fn fixture_json(dir: &str, name: &str) -> serde_json::Value {
    serde_json::from_str(&fixture(dir, name)).unwrap()
}

fn fixture_str<'a>(value: &'a serde_json::Value, field: &str) -> &'a str {
    value[field].as_str().unwrap()
}

fn fixture_u64(value: &serde_json::Value, field: &str) -> u64 {
    value[field].as_u64().unwrap()
}

#[test]
fn captured_pre_int192_protocol_window_is_rejected_by_target_abi() {
    const FIXTURE: &str = "captured-anchor-40155";

    let oracle = fixture_json(FIXTURE, "oracle.json");
    assert_eq!(
        fixture_str(&oracle, "protocol_commit"),
        PINNED_PROTOCOL_COMMIT
    );

    let calldata = fixture_hex(&fixture(FIXTURE, "postbatch.hex"));
    assert_eq!(
        calldata.len(),
        usize::try_from(fixture_u64(&oracle, "postbatch_calldata_bytes")).unwrap()
    );
    assert_eq!(&calldata[..4], &[0xca, 0xfe, 0xf1, 0x25]);
    assert!(matches!(
        settlement::decode_canonical_post_batch(calldata),
        Err(settlement::PostBatchDecodeError::InvalidAbi { .. })
    ));
}

#[test]
fn captured_legacy_inbound_calldata_is_rejected_by_target_abi() {
    let post_batch = fixture_json("fresh-chain-inbound-2175", "postbatch.json");
    let calldata = fixture_hex(fixture_str(&post_batch, "abi_calldata"));

    assert_eq!(&calldata[..4], &[0x8b, 0x1a, 0x09, 0x5a]);
    assert!(matches!(
        settlement::decode_canonical_post_batch(calldata),
        Err(settlement::PostBatchDecodeError::InvalidAbi { .. })
    ));
}

fn recorded_wire_witness(encoded: &str) -> ExecutionWitness {
    let witness: serde_json::Value = serde_json::from_str(encoded).unwrap();
    let decode_items = |field: &str| {
        witness[field]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| fixture_hex(item.as_str().unwrap()))
            .collect()
    };
    ExecutionWitness {
        state: decode_items("state"),
        codes: decode_items("codes"),
        keys: decode_items("keys"),
        headers: decode_items("headers"),
    }
}

#[test]
fn captured_legacy_outbound_window_requires_the_native_genesis_profile() {
    let blocks: Vec<(u64, String, String)> = (626..=630)
        .map(|number| {
            (
                number,
                fixture("nonzero-outbound-630", &format!("block-{number}.rlp.hex")),
                fixture("nonzero-outbound-630", &format!("witness-{number}.json")),
            )
        })
        .collect();
    let oracle = fixture_json("nonzero-outbound-630", "oracle.json");
    let post_batch_json = fixture_json("nonzero-outbound-630", "postbatch.json");
    let from = fixture_u64(&oracle, "from_block");
    let to = fixture_u64(&oracle, "to_block");
    let rollup = fixture_u64(&oracle, "expected_rollup_id");
    assert_eq!(blocks.first().unwrap().0, from);
    assert_eq!(blocks.last().unwrap().0, to);
    assert_eq!(fixture_u64(&post_batch_json, "rollup_id"), rollup);
    assert_eq!(
        oracle["outbound_effects"].as_array().unwrap().len(),
        2,
        "fixture must retain its two legacy outbound effects"
    );

    let calldata = fixture_hex(fixture_str(&post_batch_json, "abi_calldata"));
    let mut chunks = vec![ProveChunk {
        kind: Some(prove_chunk::Kind::Header(ProveHeader {
            parent_provenance: Some(eez_control_rpc::v1::ParentProvenance {
                parent_header_rlp: alloy_rlp::encode(alloy_consensus::Header::default()),
                accepted_l1_number: 0,
                live_slot_endpoint_headers_rlp: Vec::new(),
            }),
            expected_validation_identity: crate::validate::Validator::stub(Vec::new())
                .validation_identity(expected_rollup_id(1))
                .to_vec(),
            rollup_id: rollup,
            from_block: from,
            to_block: to,
            post_batch: Some(PostBatch {
                abi_calldata: calldata,
                blobs: Vec::new(),
                posting_context: Some(crate::testkit::posting_context()),
            }),
        })),
    }];
    for (number, encoded_rlp, encoded_witness) in &blocks {
        let rlp = fixture_hex(encoded_rlp);
        let block = alloy_rlp::decode_exact::<eez_primitives::Block>(&rlp).unwrap();
        assert_eq!(block.header.number, *number);
        chunks.push(ProveChunk {
            kind: Some(prove_chunk::Kind::Block(BlockWitness {
                number: *number,
                hash: block.header.hash_slow().to_vec(),
                parent_hash: block.header.parent_hash.to_vec(),
                rlp,
                witness: Some(recorded_wire_witness(encoded_witness)),
            })),
        });
    }
    let settling_hash = match &chunks.last().unwrap().kind {
        Some(prove_chunk::Kind::Block(block)) => B256::from_slice(&block.hash),
        _ => unreachable!(),
    };
    assert_eq!(
        settling_hash,
        fixture_str(&oracle, "settling_block_hash")
            .parse::<B256>()
            .unwrap()
    );
    assert_eq!(
        fixture_str(&post_batch_json, "block_hash")
            .parse::<B256>()
            .unwrap(),
        settling_hash
    );

    let chain_config =
        serde_json::from_str(&fixture("nonzero-outbound-630", "chain-config.json")).unwrap();
    let validator = validate::Validator::stateless_for_test(
        alloy_genesis::Genesis {
            config: chain_config,
            ..Default::default()
        },
        TEST_SYSTEM_ADDRESS,
    );
    assert_eq!(validator.chain_id(), fixture_u64(&oracle, "l2_chain_id"));

    let mut assembler = crate::window::WindowAssembler::start(
        crate::window::WindowLimits {
            blocks: blocks.len(),
            payload_bytes: 1_000_000,
            witness_items: 10_000,
        },
        chunks[0].clone(),
    )
    .unwrap()
    .verify_rollup_identity(expected_rollup_id(rollup))
    .unwrap();
    for chunk in &chunks[1..] {
        assembler.push(chunk.clone()).unwrap();
    }
    let admitted = assembler.finish().unwrap();
    let (_, admitted_blocks) = admitted.into_parts();
    let error = validator
        .validate_window(admitted_blocks, &CancellationToken::default())
        .unwrap_err();
    assert!(error.to_string().contains("config.rollupX"), "{error}");
}
