use super::*;
fn limits(items: usize) -> WindowLimits {
    WindowLimits {
        blocks: 1,
        payload_bytes: 4096,
        witness_items: items,
    }
}
fn block() -> BlockWitness {
    BlockWitness {
        number: 1,
        hash: vec![1; 32],
        parent_hash: vec![2; 32],
        rlp: vec![3],
        witness: Some(WireExecutionWitness::default()),
    }
}

#[test]
fn execution_witness_conversion_preserves_every_field() {
    let (witness, l1_headers) = into_execution_witness(WireExecutionWitness {
        state: vec![vec![0x00, 0xff], vec![0x01]],
        codes: vec![vec![0x02]],
        keys: vec![vec![], vec![0x03]],
        headers: vec![vec![0x04, 0x05]],
        l1_headers_rlp: vec![vec![0xc0]],
    });
    let bytes = |items: &[alloy_primitives::Bytes]| {
        items.iter().map(|item| item.to_vec()).collect::<Vec<_>>()
    };
    assert_eq!(bytes(&witness.state), [vec![0x00, 0xff], vec![0x01]]);
    assert_eq!(bytes(&witness.codes), [vec![0x02]]);
    assert_eq!(bytes(&witness.keys), [vec![], vec![0x03]]);
    assert_eq!(bytes(&witness.headers), [vec![0x04, 0x05]]);
    assert_eq!(bytes(&l1_headers), [vec![0xc0]]);
}

#[test]
fn wire_witness_item_count_includes_every_collection() {
    let witness = WireExecutionWitness {
        state: vec![Vec::new(); 2],
        codes: vec![Vec::new()],
        keys: vec![Vec::new(); 2],
        headers: vec![Vec::new()],
        l1_headers_rlp: vec![Vec::new(); 3],
    };

    assert_eq!(wire_witness_item_count(&witness), 9);
}

#[test]
fn malformed_identity_or_missing_witness_never_reaches_replay() {
    for length in [0, 31, 33] {
        let mut input = block();
        input.hash = vec![1; length];
        assert!(
            matches!(AdmittedBlock::from_wire(input, &mut 0, limits(10)), Err(WindowError::BlockHashLength { actual, .. }) if actual == length)
        );
        let mut input = block();
        input.parent_hash = vec![1; length];
        assert!(
            matches!(AdmittedBlock::from_wire(input, &mut 0, limits(10)), Err(WindowError::ParentHashLength { actual, .. }) if actual == length)
        );
    }
    let mut input = block();
    input.witness = None;
    assert!(matches!(
        AdmittedBlock::from_wire(input, &mut 0, limits(10)),
        Err(WindowError::MissingWitness { number: 1 })
    ));
}
#[test]
fn l1_evidence_is_charged_before_conversion_without_overflow() {
    let mut input = block();
    input.witness.as_mut().unwrap().l1_headers_rlp = vec![vec![0xc0]; 3];
    let mut count = 7;
    let admitted = AdmittedBlock::from_wire(input.clone(), &mut count, limits(10)).unwrap();
    assert_eq!(count, 10);
    assert_eq!(admitted.l1_headers_rlp.len(), 3);
    let mut count = 8;
    assert!(matches!(
        AdmittedBlock::from_wire(input.clone(), &mut count, limits(10)),
        Err(WindowError::WitnessItemLimit {
            attempted: 11,
            max: 10
        })
    ));
    assert_eq!(count, 8);
    let mut count = usize::MAX;
    assert!(matches!(
        AdmittedBlock::from_wire(input, &mut count, limits(usize::MAX)),
        Err(WindowError::WitnessItemLimit { .. })
    ));
    assert_eq!(count, usize::MAX);
}
