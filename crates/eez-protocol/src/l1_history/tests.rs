use super::*;

const RULES: HeaderRules = HeaderRules {
    london_block: None,
    shanghai_time: None,
    cancun_time: None,
    prague_time: None,
    unsupported_time: None,
};

#[test]
fn recorded_import_limits_delay_regression_and_exact_abi_are_enforced() {
    let evidence = chain(&(0..=70).collect::<Vec<_>>());
    let schedule = HistorySchedule::new(120, 2, 6, 2).unwrap();
    let prior = state(&evidence, 99, 120);
    let valid = schedule
        .execution(126, prior, &evidence, Some(163))
        .unwrap();
    let raw = valid.import.unwrap();
    assert_eq!(decode_import(&raw).unwrap().hashes.len(), 64);
    assert!(
        schedule
            .execution(126, prior, &evidence, Some(164))
            .unwrap_err()
            .to_string()
            .contains("bound")
    );
    assert!(
        schedule
            .execution(126, state(&evidence, 100, 120), &evidence, Some(99))
            .unwrap_err()
            .to_string()
            .contains("regresses")
    );
    let future = chain(&[108, 121]);
    assert!(
        schedule
            .execution(126, state(&future, 99, 120), &future, Some(100))
            .unwrap_err()
            .to_string()
            .contains("newer")
    );
    for mutation in 0..5 {
        let mut bad = raw.to_vec();
        match mutation {
            0 => bad.push(0),
            1 => bad[67] = 96,
            2 => bad[68..100].fill(0xff),
            3 => bad[4] = 1,
            _ => bad[0] ^= 1,
        }
        assert!(decode_import(&bad).is_err(), "mutation {mutation}");
    }
}

fn headers(times: &[u64]) -> Vec<Bytes> {
    let mut parent = B256::ZERO;
    times
        .iter()
        .enumerate()
        .map(|(i, &timestamp)| {
            let header = Header {
                number: 99 + i as u64,
                timestamp,
                parent_hash: parent,
                mix_hash: B256::with_last_byte(i as u8 + 1),
                ..Default::default()
            };
            parent = header.hash_slow();
            alloy_rlp::encode(header).into()
        })
        .collect()
}

fn chain(times: &[u64]) -> HeaderChain {
    let mut chain = HeaderChain::default();
    chain.append(&headers(times), &RULES, 100).unwrap();
    chain
}

fn state(chain: &HeaderChain, endpoint: u64, first_cutoff: u64) -> HistoryState {
    HistoryState {
        first_number: 99,
        endpoint,
        endpoint_hash: chain.at(endpoint).unwrap().hash(),
        first_cutoff,
    }
}

#[test]
fn delay_retains_origin_and_randao_across_sync_then_updates_together() {
    let schedule = HistorySchedule::new(120, 2, 6, 2).unwrap();
    let chain = chain(&[108, 120, 121, 132, 144]);
    let before = state(&chain, 99, 120);
    for time in [122, 124] {
        let e = schedule.execution(time, before, &chain, None).unwrap();
        assert_eq!(e.origin_number, 99);
        assert_eq!(e.prev_randao, B256::with_last_byte(1));
        assert!(e.import.is_none());
    }
    let update = schedule.execution(126, before, &chain, Some(100)).unwrap();
    assert_eq!(update.origin_number, 100);
    assert_eq!(update.prev_randao, B256::with_last_byte(2));
    let call = IL1BlockhashContract::importHistoryCall::abi_decode(update.import.as_ref().unwrap())
        .unwrap();
    assert_eq!(call.cutoff, 120);
    assert_eq!(call.hashes, vec![chain.at(100).unwrap().hash()]);
    let after = state(&chain, 100, 120);
    for time in [128, 130, 132, 134, 136] {
        let e = schedule.execution(time, after, &chain, None).unwrap();
        assert_eq!(e.origin_number, 100);
        assert_eq!(e.prev_randao, B256::with_last_byte(2));
        assert!(e.import.is_none());
    }
    let update = schedule.execution(138, after, &chain, Some(102)).unwrap();
    assert_eq!(update.origin_number, 102);
    assert_eq!(update.prev_randao, B256::with_last_byte(4));
    let call = IL1BlockhashContract::importHistoryCall::abi_decode(update.import.as_ref().unwrap())
        .unwrap();
    assert_eq!(
        call.hashes,
        vec![chain.at(101).unwrap().hash(), chain.at(102).unwrap().hash()]
    );
}

#[test]
fn every_offset_and_exact_cutoff_equality() {
    for q in 0..6 {
        let schedule = HistorySchedule::new(120, 2, 6, q).unwrap();
        let chain = chain(&[108, 120, 132]);
        let update = 120 + 2 * (u64::from(q) + 1);
        assert_eq!(schedule.source_cutoff(update - 2).unwrap(), 108);
        assert_eq!(schedule.source_cutoff(update).unwrap(), 120);
        assert_eq!(
            schedule
                .execution(update, state(&chain, 99, 120), &chain, Some(100))
                .unwrap()
                .origin_number,
            100
        );
        assert_eq!(schedule.source_cutoff(update + 10).unwrap(), 120);
        assert_eq!(schedule.source_cutoff(update + 12).unwrap(), 132);
    }
}

#[test]
fn missed_slots_inherit_without_imports_or_schedule_state_mutation() {
    let schedule = HistorySchedule::new(120, 2, 6, 2).unwrap();
    let chain = chain(&[108, 144]);
    let parent = state(&chain, 99, 120);
    for timestamp in [126, 138, 150, 120 + 4 * 86400 + 6] {
        let e = schedule.execution(timestamp, parent, &chain, None).unwrap();
        assert_eq!(e.origin_number, 99);
        assert_eq!(e.prev_randao, chain.at(99).unwrap().header().mix_hash);
        assert!(e.import.is_none());
        assert!(
            schedule
                .execution(timestamp, parent, &chain, Some(99))
                .is_err()
        );
    }
    let e = schedule.execution(150, parent, &chain, Some(100)).unwrap();
    assert_eq!(e.origin_number, 100);
    assert_eq!(
        decode_import(e.import.as_ref().unwrap())
            .unwrap()
            .hashes
            .len(),
        1
    );
}

#[test]
fn recorded_origins_bind_to_both_exact_anchors_without_latest_source_inference() {
    let mut chain = chain(&[108, 120]);
    let selected = chain.at(99).unwrap().hash();
    let actual = EvidenceAnchor::ActualParent {
        number: 100,
        hash: chain.endpoint().unwrap().hash(),
        target_timestamp: 132,
    };
    chain.verify_origin(99, selected, actual).unwrap();
    assert!(chain.verify_origin(100, selected, actual).is_err());
    let later = headers(&[108, 120, 132]);
    chain.append(&later[2..], &RULES, 3).unwrap();
    let historical = EvidenceAnchor::Historical {
        number: 101,
        hash: chain.endpoint().unwrap().hash(),
    };
    chain.verify_origin(99, selected, historical).unwrap();
    assert!(chain.verify_origin(99, B256::ZERO, historical).is_err());
    assert!(
        chain.verify_anchor(actual).is_err(),
        "extra endpoint evidence must reject"
    );
}

#[test]
fn omitted_unused_bridge_header_and_wrong_branch_reject_atomically() {
    let raw = headers(&[108, 120, 132, 144, 156]);
    let mut chain = HeaderChain::default();
    chain.append(&raw[..2], &RULES, 10).unwrap();
    let before = chain.clone();
    assert!(chain.append(&raw[3..], &RULES, 10).is_err());
    assert_eq!(chain, before);
    let mut bad: Header = alloy_rlp::decode_exact(&raw[3]).unwrap();
    bad.parent_hash = B256::repeat_byte(9);
    assert!(
        chain
            .append(&[raw[2].clone(), alloy_rlp::encode(bad).into()], &RULES, 10)
            .is_err()
    );
    assert_eq!(
        chain, before,
        "even a valid prefix of a rejected segment must not publish"
    );
    chain.append(&raw[2..], &RULES, 10).unwrap();
}

#[test]
fn malformed_duplicate_trailing_and_resource_inputs_reject() {
    let raw = headers(&[108, 120]);
    let mut chain = HeaderChain::default();
    for invalid in [Bytes::new(), vec![0; MAX_HEADER_BYTES + 1].into(), {
        let mut bytes = raw[0].to_vec();
        bytes.push(0);
        bytes.into()
    }] {
        assert!(chain.append(&[invalid], &RULES, 10).is_err());
    }
    assert!(chain.append(&raw, &RULES, 1).is_err());
    assert!(chain.headers().is_empty());
    chain.append(&raw, &RULES, 2).unwrap();
    assert!(chain.append(&raw[1..], &RULES, 3).is_err());
    assert_eq!(chain.headers().len(), 2);
}

#[test]
fn header_layout_changes_are_checked_at_exact_fork_boundaries() {
    let rules = HeaderRules {
        london_block: Some(100),
        shanghai_time: Some(120),
        cancun_time: Some(132),
        prague_time: Some(144),
        unsupported_time: Some(156),
    };
    let mut header = Header {
        number: 99,
        timestamp: 108,
        ..Default::default()
    };
    rules.validate(&header).unwrap();
    header.number = 100;
    assert!(rules.validate(&header).is_err());
    header.base_fee_per_gas = Some(1);
    rules.validate(&header).unwrap();
    header.timestamp = 120;
    assert!(rules.validate(&header).is_err());
    header.withdrawals_root = Some(B256::ZERO);
    rules.validate(&header).unwrap();
    header.timestamp = 132;
    assert!(rules.validate(&header).is_err());
    header.blob_gas_used = Some(0);
    header.excess_blob_gas = Some(0);
    header.parent_beacon_block_root = Some(B256::ZERO);
    rules.validate(&header).unwrap();
    header.timestamp = 144;
    assert!(rules.validate(&header).is_err());
    header.requests_hash = Some(B256::ZERO);
    rules.validate(&header).unwrap();
    header.timestamp = 156;
    assert!(rules.validate(&header).is_err());
}

#[test]
fn repeated_inherited_source_is_valid_but_wrong_parent_state_rejects() {
    let schedule = HistorySchedule::new(120, 2, 6, 2).unwrap();
    let chain = chain(&[108, 120, 132]);
    let mut invalid = state(&chain, 99, 120);
    assert!(schedule.execution(134, invalid, &chain, None).is_ok());
    invalid = state(&chain, 100, 120);
    invalid.endpoint_hash = B256::repeat_byte(9);
    assert!(schedule.execution(134, invalid, &chain, None).is_err());
    assert!(
        schedule
            .execution(138, state(&chain, 100, 120), &chain, Some(100))
            .is_err()
    );
}

#[test]
fn checked_integer_boundaries_and_unscheduled_timestamps() {
    for args in [
        (0, 2, 6, 2),
        (120, 0, 6, 2),
        (120, 2, 1, 0),
        (120, 2, 6, 6),
        (u64::MAX, 2, 6, 2),
        (120, u64::MAX, 6, 2),
    ] {
        assert!(HistorySchedule::new(args.0, args.1, args.2, args.3).is_err());
    }
    let schedule = HistorySchedule::new(120, 2, 6, 2).unwrap();
    assert!(schedule.source_cutoff(119).is_err());
    assert!(schedule.source_cutoff(123).is_err());
    assert!(schedule.source_cutoff(u64::MAX).is_err());
}
