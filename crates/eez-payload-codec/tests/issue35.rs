use eez_payload_codec::v0::Payload;

#[test]
fn issue35_six_hundred_empty_blocks_use_the_exact_direct_v0_body() {
    // Independent published fixture: version, count, empty token, beneficiary
    // run, address, extraData run and zero extraData length. No RLP envelope.
    let mut wire = vec![0, 0xd8, 4, 0xff, 0xd8, 4, 0xd8, 4];
    wire.extend_from_slice(&[0x11; 20]);
    wire.extend_from_slice(&[0xd8, 4, 0]);
    assert_eq!(wire.len(), 31);
    let payload = Payload::decode(&wire).expect("issue #35 direct V0 fixture must decode");
    assert_eq!(payload.block_count(), 600);
    assert_eq!(payload.encode().unwrap(), wire);
}

#[test]
fn shared_empty_vectors_are_exact_and_do_not_expand_large_spans() {
    use alloy_primitives::{Address, Bytes};
    use eez_payload_codec::v0::{BlockRun, encode};
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/v0-empty-vectors.json")).unwrap();
    for vector in vectors["vectors"].as_array().unwrap() {
        let blocks = u32::try_from(vector["blocks"].as_u64().unwrap()).unwrap();
        let beneficiary: Address = vector["beneficiary"].as_str().unwrap().parse().unwrap();
        let extra_data: Bytes = vector["extraData"].as_str().unwrap().parse().unwrap();
        let expected: Bytes = vector["operations"].as_str().unwrap().parse().unwrap();
        assert_eq!(
            expected.len() as u64,
            vector["encodedBytes"].as_u64().unwrap()
        );
        let actual = encode(&[BlockRun {
            blocks,
            beneficiary,
            extra_data: &extra_data,
            transactions: &[],
        }])
        .unwrap();
        assert_eq!(actual, expected);
        let decoded = Payload::decode(&expected).unwrap();
        assert_eq!(decoded.block_count(), blocks);
        assert_eq!(
            decoded.transaction_count() as u64,
            vector["pureL2Transactions"].as_u64().unwrap()
        );
        assert_eq!(decoded.blocks().len(), blocks as usize);
        assert_eq!(decoded.count_runs().count(), 1);
    }
}
