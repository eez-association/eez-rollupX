use super::*;
use alloy_consensus::{BlockBody, SignableTransaction, TxEip1559};
use alloy_primitives::{Signature, U256, hex};
use proptest::prelude::*;

fn environment() -> Environment {
    Environment {
        timestamp_step_seconds: 2,
        fee_recipient: Address::repeat_byte(0x11),
        prev_randao: B256::repeat_byte(0x22),
        gas_limit: 1,
        extra_data: Bytes::new(),
        cancun_parent_beacon_root: B256::repeat_byte(0x33),
    }
}

fn block(number: u64, parent_hash: B256) -> Vec<u8> {
    alloy_rlp::encode(Block {
        header: alloy_consensus::Header {
            number,
            parent_hash,
            ..Default::default()
        },
        body: BlockBody::default(),
    })
}

fn tx(nonce: u64) -> Bytes {
    TxEip1559 {
        chain_id: 1,
        nonce,
        gas_limit: 21_000,
        ..Default::default()
    }
    .into_signed(Signature::new(U256::from(1), U256::from(2), false))
    .encoded_2718()
    .into()
}

fn payload(count: u64, records: Vec<Record>) -> Payload {
    Payload::new(
        count,
        environment(),
        records,
        block(100_000, B256::ZERO),
        vec![],
        vec![],
    )
    .unwrap()
}

// Independent wire builder: concatenate already-encoded fields. This lets tests
// construct malformed cases which the validated production constructor forbids.
fn list(fields: &[Vec<u8>]) -> Vec<u8> {
    let payload_length = fields.iter().map(Vec::len).sum();
    let mut out = Vec::new();
    Header {
        list: true,
        payload_length,
    }
    .encode(&mut out);
    for field in fields {
        out.extend_from_slice(field);
    }
    out
}

fn wire(count: u64, env: &Environment, records: &[Vec<u8>]) -> Vec<u8> {
    // Independent empty-run writer (not the production token encoder).
    let mut counts = match count {
        0 => vec![],
        1 => vec![0],
        2..=64 => vec![0xc0 + (count - 2) as u8],
        _ => vec![0xff],
    };
    if count >= 65 {
        let mut n = count;
        while n >= 128 {
            counts.push((n as u8 & 127) | 128);
            n >>= 7;
        }
        counts.push(n as u8);
    }
    wire_columns(count, env, &counts, records, &[])
}

fn wire_columns(
    count: u64,
    env: &Environment,
    counts: &[u8],
    overrides: &[Vec<u8>],
    txs: &[Bytes],
) -> Vec<u8> {
    let body = list(&[
        vec![1],
        alloy_rlp::encode(count),
        alloy_rlp::encode(env),
        list(&[
            alloy_rlp::encode(counts),
            list(overrides),
            list(&txs.iter().map(alloy_rlp::encode).collect::<Vec<_>>()),
        ]),
        alloy_rlp::encode(block(100_000, B256::ZERO).as_slice()),
        vec![0xc0],
        vec![0xc0],
    ]);
    [vec![TAG], body].concat()
}

fn item<'a>(input: &mut &'a [u8]) -> &'a [u8] {
    let before = *input;
    let header = Header::decode(input).unwrap();
    *input = &input[header.payload_length..];
    &before[..before.len() - input.len()]
}

#[test]
fn environment_wire_has_fixed_field_order_and_byte_strings() {
    let expected = hex::decode(concat!(
        "f85a0294",
        "1111111111111111111111111111111111111111",
        "a0",
        "2222222222222222222222222222222222222222222222222222222222222222",
        "0180a0",
        "3333333333333333333333333333333333333333333333333333333333333333"
    ))
    .unwrap();
    assert_eq!(alloy_rlp::encode(environment()), expected);
    assert_eq!(
        alloy_rlp::decode_exact::<Environment>(expected).unwrap(),
        environment()
    );
}

#[test]
fn maximum_empty_span_is_sparse_and_iterator_is_exact_and_fused() {
    let payload = payload(65_535, vec![]);
    let bytes = payload.encode().unwrap();
    assert_eq!(bytes, wire(65_535, &environment(), &[]));
    assert_eq!(Payload::decode(&bytes).unwrap(), payload);
    assert_eq!(payload.block_count(), 65_536);
    assert!(
        bytes.len() < 2_000,
        "empty history must not carry per-block headers/counts"
    );
    assert!(payload.records().is_empty());
    let mut blocks = payload.blocks();
    for position in 0..65_535 {
        assert_eq!(blocks.len(), 65_536 - position);
        assert_eq!(
            blocks.next(),
            Some(BlockPlan::Derived {
                position,
                environment: &environment(),
                transactions: &[],
            })
        );
    }
    assert_eq!(
        blocks.next(),
        Some(BlockPlan::Full {
            position: 65_535,
            rlp: payload.terminal_block(),
            terminal: true,
        })
    );
    assert_eq!(blocks.len(), 0);
    assert_eq!(blocks.next(), None);
    assert_eq!(blocks.next(), None);
}

#[test]
fn terminal_only_payload_does_not_derive_a_block() {
    let payload = payload(0, vec![]);
    assert_eq!(payload.first_block_number(), 100_000);
    assert_eq!(payload.blocks().count(), 1);
    assert!(matches!(
        payload.blocks().next(),
        Some(BlockPlan::Full { terminal: true, .. })
    ));
}

#[test]
fn decoder_preserves_sparse_plan_without_activation_configuration() {
    let p = payload(
        3,
        vec![Record {
            empty_gap: 1,
            data: RecordData::Transactions(vec![tx(7)]),
        }],
    );
    let decoded = Payload::decode(&p.encode().unwrap()).unwrap();
    assert_eq!(decoded, p);
    assert!(matches!(
        decoded.blocks().nth(1),
        Some(BlockPlan::Derived { transactions, .. }) if transactions == [tx(7)]
    ));
}

#[test]
fn runtime_decoder_rejects_earlier_tags_without_an_activation_or_fallback_mode() {
    for payload in [vec![1], vec![2], vec![3]] {
        assert!(matches!(Payload::decode(&payload), Err(Error::WrongTag)));
    }
    assert!(matches!(Payload::decode(&[]), Err(Error::WrongTag)));
}

#[test]
fn terminal_and_entries_are_byte_strings_with_exact_contents_and_unchanged_groups() {
    let terminal = block(99, B256::ZERO);
    let entries = vec![vec![0, 1, 127, 128, 255], vec![], vec![255; 56]];
    let groups = vec![0, 1, 127, 128, 255, 256, u16::MAX];
    let new = Payload::new(
        0,
        environment(),
        vec![],
        terminal.clone(),
        entries.clone(),
        groups.clone(),
    )
    .unwrap();
    let encoded = new.encode().unwrap();
    let decoded = Payload::decode(&encoded).unwrap();
    assert_eq!(decoded.terminal_block(), terminal);
    assert_eq!(decoded.l2_entries(), entries);
    assert_eq!(decoded.outbound_group_sizes(), groups);
    let mut new_input = &encoded[1..];
    let mut new_body = Header::decode_bytes(&mut new_input, true).unwrap();
    for _ in 0..4 {
        item(&mut new_body);
    }
    assert_eq!(
        Header::decode_bytes(&mut new_body, false).unwrap(),
        terminal
    );
    let mut encoded_entries = Header::decode_bytes(&mut new_body, true).unwrap();
    // Independent literal vector: zero and high bytes are raw data, not integers.
    assert_eq!(item(&mut encoded_entries), &[0x85, 0, 1, 127, 128, 255]);
    assert_eq!(item(&mut encoded_entries), &[0x80]);
    let mut expected = vec![0xb8, 56];
    expected.extend([255; 56]);
    assert_eq!(item(&mut encoded_entries), expected);
    assert!(encoded_entries.is_empty());
    assert_eq!(new_body, alloy_rlp::encode(&groups));

    // Reject the superseded integer-list wrappers, even though the contents
    // would otherwise describe the same block and entries.
    let fields = [
        vec![1],
        alloy_rlp::encode(0u64),
        alloy_rlp::encode(environment()),
        list(&[vec![0x80], vec![0xc0], vec![0xc0]]),
        alloy_rlp::encode(terminal.as_slice()),
        list(
            &entries
                .iter()
                .map(|entry| alloy_rlp::encode(entry.as_slice()))
                .collect::<Vec<_>>(),
        ),
        alloy_rlp::encode(&groups),
    ];
    for (index, legacy) in [
        (4, alloy_rlp::encode(&terminal)),
        (5, alloy_rlp::encode(&entries)),
    ] {
        let mut invalid = fields.clone();
        invalid[index] = legacy;
        assert!(matches!(
            Payload::decode(&[vec![TAG], list(&invalid)].concat()),
            Err(Error::Rlp(_))
        ));
    }
}

#[test]
fn environment_change_applies_to_its_own_block_and_persists_across_full_blocks() {
    let mut changed = environment();
    changed.fee_recipient = Address::repeat_byte(7);
    let full = block(99_998, B256::ZERO);
    let records = vec![
        Record {
            empty_gap: 1,
            data: RecordData::Transactions(vec![tx(1)]),
        },
        Record {
            empty_gap: 1,
            data: RecordData::EnvironmentChange {
                environment: changed.clone(),
                transactions: vec![],
            },
        },
        Record {
            empty_gap: 1,
            data: RecordData::FullBlock(full.clone().into()),
        },
    ];
    let payload = payload(7, records);
    let encoded = payload.encode().unwrap();
    assert_eq!(Payload::decode(&encoded).unwrap(), payload);
    for (position, plan) in payload.blocks().enumerate() {
        match plan {
            BlockPlan::Derived {
                environment: actual,
                transactions,
                ..
            } => {
                assert_eq!(
                    *actual,
                    if position < 3 {
                        environment()
                    } else {
                        changed.clone()
                    }
                );
                assert_eq!(
                    transactions,
                    if position == 1 { vec![tx(1)] } else { vec![] }
                );
            }
            BlockPlan::Full {
                rlp,
                terminal: false,
                ..
            } => {
                assert_eq!(position, 5);
                assert_eq!(rlp, full);
            }
            BlockPlan::Full { terminal: true, .. } => assert_eq!(position, 7),
        }
    }
}

#[test]
fn gap_and_environment_wire_match_independent_encoder() {
    let mut changed = environment();
    changed.timestamp_step_seconds = 3;
    let txs = vec![tx(10)];
    let record = list(&[vec![0x80], vec![2], alloy_rlp::encode(&changed)]);
    let expected = wire_columns(2, &environment(), &[1, 0], &[record], &txs);
    let payload = payload(
        2,
        vec![Record {
            empty_gap: 0,
            data: RecordData::EnvironmentChange {
                environment: changed,
                transactions: txs,
            },
        }],
    );
    assert_eq!(payload.encode().unwrap(), expected);
}

#[test]
fn oversized_count_is_rejected_before_missing_environment() {
    for count in [65_536u64, u64::MAX] {
        let input = [vec![TAG], list(&[vec![PROFILE], alloy_rlp::encode(count)])].concat();
        assert!(matches!(Payload::decode(&input), Err(Error::BlockLimit(n)) if n==count));
    }
}

#[test]
fn out_of_range_gap_is_rejected_before_missing_record_data() {
    for (count, gap) in [(0, 0), (1, 1), (65_535, 65_535), (65_535, u64::MAX)] {
        let input = wire(count, &environment(), &[list(&[alloy_rlp::encode(gap)])]);
        assert!(matches!(
            Payload::decode(&input),
            Err(Error::RecordRange(0))
        ));
    }
}

#[test]
fn cumulative_record_coverage_cannot_consume_terminal() {
    let one = list(&[vec![0x80], vec![2], alloy_rlp::encode(environment())]);
    let input = wire(1, &environment(), &[one.clone(), one]);
    assert!(matches!(
        Payload::decode(&input),
        Err(Error::RecordRange(1))
    ));
}

#[test]
fn empty_transaction_record_unknown_kind_and_noop_change_are_rejected() {
    let empty = list(&[vec![0x80], vec![0x80], vec![0xc0]]);
    assert!(matches!(
        Payload::decode(&wire(1, &environment(), &[empty])),
        Err(Error::RecordKind(0))
    ));
    let unknown = list(&[vec![0x80], vec![3]]);
    assert!(matches!(
        Payload::decode(&wire(1, &environment(), &[unknown])),
        Err(Error::RecordKind(3))
    ));
    let same = list(&[vec![0x80], vec![2], alloy_rlp::encode(environment())]);
    assert!(matches!(
        Payload::decode(&wire(1, &environment(), &[same])),
        Err(Error::UnchangedEnvironment(0))
    ));
}

#[test]
fn noop_change_is_relative_to_the_active_not_initial_environment() {
    let mut changed = environment();
    changed.gas_limit = 2;
    let change = Record {
        empty_gap: 0,
        data: RecordData::EnvironmentChange {
            environment: changed,
            transactions: vec![],
        },
    };
    assert!(matches!(
        Payload::new(
            2,
            environment(),
            vec![change.clone(), change],
            block(100, B256::ZERO),
            vec![],
            vec![]
        ),
        Err(Error::UnchangedEnvironment(1))
    ));
}

#[test]
fn invalid_environment_fields_and_wrong_field_lengths_fail() {
    let mut environments = vec![environment(); 3];
    environments[0].timestamp_step_seconds = 0;
    environments[1].gas_limit = 0;
    environments[2].extra_data = vec![0; 33].into();
    for env in environments {
        assert!(matches!(
            Payload::decode(&wire(0, &env, &[])),
            Err(Error::InvalidEnvironment)
        ));
    }
    let good = alloy_rlp::encode(environment());
    let mut input = good.as_slice();
    let mut fields = Header::decode_bytes(&mut input, true).unwrap();
    let items: Vec<Vec<u8>> = (0..6).map(|_| item(&mut fields).to_vec()).collect();
    for index in [1, 2, 5] {
        let mut bad = items.clone();
        bad[index] = alloy_rlp::encode(Bytes::from(vec![0; 19]));
        assert!(alloy_rlp::decode_exact::<Environment>(list(&bad)).is_err());
    }
}

#[test]
fn rejects_noncanonical_integers_profile_and_trailing_fields() {
    let canonical = payload(0, vec![]).encode().unwrap();
    for cut in 0..canonical.len() {
        assert!(
            Payload::decode(&canonical[..cut]).is_err(),
            "accepted truncation {cut}"
        );
    }
    assert!(Payload::decode(&[canonical.clone(), vec![0]].concat()).is_err());
    let mut input = &canonical[1..];
    let mut body = Header::decode_bytes(&mut input, true).unwrap();
    let parts: Vec<Vec<u8>> = (0..7).map(|_| item(&mut body).to_vec()).collect();
    for invalid in [vec![0], vec![0x81, 0], vec![0x82, 0, 1], vec![0x81, 1]] {
        let mut bad = parts.clone();
        bad[1] = invalid;
        assert!(Payload::decode(&[vec![TAG], list(&bad)].concat()).is_err());
    }
    let mut unknown = parts.clone();
    unknown[0] = vec![2];
    assert!(matches!(
        Payload::decode(&[vec![TAG], list(&unknown)].concat()),
        Err(Error::Profile(2))
    ));
    let mut extra = parts;
    extra.push(vec![0x80]);
    assert!(Payload::decode(&[vec![TAG], list(&extra)].concat()).is_err());
}

#[test]
fn malformed_and_nonexact_transaction_envelopes_are_rejected() {
    let good = tx(0);
    for bad in [
        Bytes::new(),
        Bytes::from_static(&[0x7e]),
        Bytes::from([good.as_ref(), &[0]].concat()),
        Bytes::from_static(&[0x02, 0xc0]),
    ] {
        assert!(matches!(
            Payload::decode(&wire_columns(1, &environment(), &[1], &[], &[bad])),
            Err(Error::InvalidTransaction { .. })
        ));
    }
}

#[test]
fn constructor_cannot_emit_a_transaction_the_decoder_rejects() {
    let result = Payload::new(
        1,
        environment(),
        vec![Record {
            empty_gap: 0,
            data: RecordData::Transactions(vec![Bytes::new()]),
        }],
        block(100_000, B256::ZERO),
        vec![],
        vec![],
    );
    assert!(matches!(result, Err(Error::InvalidTransaction { .. })));
}

#[test]
fn transaction_count_is_bounded_before_validating_transaction_data() {
    assert!(matches!(
        Payload::new(
            1,
            environment(),
            vec![Record {
                empty_gap: 0,
                data: RecordData::Transactions(vec![Bytes::new(); 16_384]),
            }],
            block(100, B256::ZERO),
            vec![],
            vec![]
        ),
        Err(Error::TransactionLimit(0))
    ));
}

#[test]
fn count_coverage_and_flat_transaction_column_are_exact() {
    for counts in [
        &[][..],
        &[0xc0],
        &[0, 0],
        &[0x80, 1],
        &[0xff, 1],
        &[0xff, 0xff, 0xff, 0xff, 0xff, 15],
    ] {
        assert!(matches!(
            Payload::decode(&wire_columns(1, &environment(), counts, &[], &[])),
            Err(Error::CountEncoding)
        ));
    }
    assert!(Payload::decode(&wire_columns(1, &environment(), &[1], &[], &[])).is_err());
    assert!(Payload::decode(&wire_columns(1, &environment(), &[0], &[], &[tx(0)])).is_err());
    for actual in [0, 1, 128] {
        let full_block = Block {
            header: alloy_consensus::Header {
                number: 99_998,
                ..Default::default()
            },
            body: BlockBody {
                transactions: (0..actual)
                    .map(|n| {
                        reth_ethereum_primitives::TransactionSigned::decode_2718(
                            &mut tx(n).as_ref(),
                        )
                        .unwrap()
                    })
                    .collect(),
                ..Default::default()
            },
        };
        let full = list(&[
            vec![0x80],
            vec![1],
            alloy_rlp::encode(Bytes::from(alloy_rlp::encode(full_block))),
        ]);
        // Leave a derived block between the exception and terminal so this
        // fixture tests count binding, not adjacent full-block parent links.
        let wrong = if actual == 0 { vec![1, 0] } else { vec![0xc0] };
        assert!(matches!(
            Payload::decode(&wire_columns(
                2,
                &environment(),
                &wrong,
                std::slice::from_ref(&full),
                &[]
            )),
            Err(Error::CountMismatch)
        ));
        let correct = match actual {
            0 => vec![0xc0],
            1 => vec![1, 0],
            _ => vec![0x80, 0x80, 0],
        };
        let wire = wire_columns(2, &environment(), &correct, &[full], &[]);
        assert_eq!(Payload::decode(&wire).unwrap().encode().unwrap(), wire);
    }
}

#[test]
fn mixed_twelve_block_count_vector_is_independent_of_record_framing() {
    let p = payload(
        12,
        vec![
            Record {
                empty_gap: 3,
                data: RecordData::Transactions(vec![tx(1)]),
            },
            Record {
                empty_gap: 5,
                data: RecordData::Transactions(vec![tx(2)]),
            },
        ],
    );
    let expected = wire_columns(
        12,
        &environment(),
        &[0xc1, 1, 0xc3, 1, 0xc0],
        &[],
        &[tx(1), tx(2)],
    );
    assert_eq!(p.encode().unwrap(), expected);
    assert_eq!(Payload::decode(&expected).unwrap(), p);
}

#[test]
fn full_terminal_and_full_ordinary_share_the_count_ceiling() {
    let transaction = TransactionSigned::decode_2718(&mut tx(0).as_ref()).unwrap();
    let mut full = Block {
        header: alloy_consensus::Header {
            number: 99_999,
            ..Default::default()
        },
        body: BlockBody {
            transactions: vec![transaction.clone(); MAX_TRANSACTIONS],
            ..Default::default()
        },
    };
    let terminal = Payload::new(
        0,
        environment(),
        vec![],
        alloy_rlp::encode(&full),
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(
        Payload::decode(&terminal.encode().unwrap()).unwrap(),
        terminal
    );
    full.body.transactions.push(transaction);
    assert!(matches!(
        Payload::new(
            0,
            environment(),
            vec![],
            alloy_rlp::encode(&full),
            vec![],
            vec![]
        ),
        Err(Error::TransactionLimit(0))
    ));
    assert!(matches!(
        Payload::new(
            1,
            environment(),
            vec![Record {
                empty_gap: 0,
                data: RecordData::FullBlock(alloy_rlp::encode(&full).into())
            }],
            block(100_000, full.header.hash_slow()),
            vec![],
            vec![]
        ),
        Err(Error::TransactionLimit(0))
    ));
}

#[test]
fn full_records_have_exact_number_and_immediately_checkable_parent_links() {
    let first = block(99, B256::ZERO);
    let hash = alloy_rlp::decode_exact::<Block>(&first)
        .unwrap()
        .header
        .hash_slow();
    let record = Record {
        empty_gap: 0,
        data: RecordData::FullBlock(first.clone().into()),
    };
    let good = Payload::new(
        1,
        environment(),
        vec![record.clone()],
        block(100, hash),
        vec![],
        vec![],
    )
    .unwrap();
    assert_eq!(Payload::decode(&good.encode().unwrap()).unwrap(), good);
    assert!(matches!(
        Payload::new(
            1,
            environment(),
            vec![record],
            block(100, B256::ZERO),
            vec![],
            vec![]
        ),
        Err(Error::BlockPlacement(1))
    ));
    let wrong = Record {
        empty_gap: 0,
        data: RecordData::FullBlock(block(98, B256::ZERO).into()),
    };
    assert!(matches!(
        Payload::new(
            1,
            environment(),
            vec![wrong],
            block(100, hash),
            vec![],
            vec![]
        ),
        Err(Error::BlockPlacement(0))
    ));
    for count in [100, 101] {
        assert!(matches!(
            Payload::new(
                count,
                environment(),
                vec![],
                block(100, hash),
                vec![],
                vec![]
            ),
            Err(Error::BlockPlacement(_))
        ));
    }
}

#[test]
fn complete_payload_size_comparison_with_pr34_record_framing() {
    for (name, selected) in [
        ("terminal-only", vec![]),
        ("600 empty ordinary", vec![false; 600]),
        ("600 nonempty ordinary", vec![true; 600]),
        (
            "600 alternating ordinary",
            (0..600).map(|i| i % 2 == 0).collect(),
        ),
    ] {
        let mut gap = 0;
        let mut records = Vec::new();
        for active in &selected {
            if *active {
                records.push(Record {
                    empty_gap: gap,
                    data: RecordData::Transactions(vec![tx(0)]),
                });
                gap = 0;
            } else {
                gap += 1;
            }
        }
        let p = payload(selected.len() as u64, records);
        // PR #34's exact seven-field layout, independent of the new encoder.
        let old_records = p
            .records()
            .iter()
            .map(|record| {
                let RecordData::Transactions(txs) = &record.data else {
                    unreachable!()
                };
                list(&[
                    alloy_rlp::encode(record.empty_gap),
                    alloy_rlp::encode(0u8),
                    alloy_rlp::encode(txs),
                ])
            })
            .collect::<Vec<_>>();
        let baseline = 1 + list(&[
            vec![1],
            alloy_rlp::encode(selected.len() as u64),
            alloy_rlp::encode(environment()),
            list(&old_records),
            alloy_rlp::encode(p.terminal_block.clone()),
            vec![0xc0],
            vec![0xc0],
        ])
        .len();
        let current = p.encode().unwrap().len();
        eprintln!("{name}: PR34={baseline}, V0={current}");
        assert!(current < baseline);
    }
}

#[test]
fn exact_sizing_includes_all_rlp_width_boundaries_and_raw_byte_values() {
    for count in [0, 1, 55, 56, 127, 128, 255, 256, 10_000, 65_535] {
        for length in [0, 1, 54, 55, 56, 127, 128, 255, 256, 1024] {
            let p = Payload::new(
                count,
                environment(),
                vec![],
                block(100_000, B256::ZERO),
                vec![vec![0; length], vec![255; length]],
                vec![0, 127, 128, 256, 65535],
            )
            .unwrap();
            let encoded = p.encode().unwrap();
            assert_eq!(p.encoded_len().unwrap(), encoded.len());
            assert_eq!(Payload::decode(&encoded).unwrap(), p);
        }
    }
    assert!(matches!(list_len(usize::MAX), Err(Error::Overflow)));
    assert!(matches!(add(usize::MAX, 1), Err(Error::Overflow)));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_bytes_never_panic_and_any_accepted_form_is_canonical(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        if let Ok(decoded) = Payload::decode(&bytes) {
            prop_assert_eq!(decoded.encode().unwrap(), bytes);
            prop_assert!(decoded.block_count() <= MAX_BLOCKS);
            prop_assert_eq!(decoded.blocks().count(), decoded.block_count());
        }
    }

    #[test]
    fn sparse_model_preserves_transaction_positions(
        selections in prop::collection::vec(any::<bool>(), 0..300),
        entry in prop::collection::vec(any::<u8>(), 0..260),
    ) {
        let mut records = Vec::new();
        let mut gap = 0;
        for (position, selected) in selections.iter().enumerate() {
            if *selected {
                records.push(Record { empty_gap: gap, data: RecordData::Transactions(vec![tx(position as u64)]) });
                gap = 0;
            } else { gap += 1; }
        }
        let p = Payload::new(selections.len() as u64, environment(), records, block(1000, B256::ZERO), vec![entry], vec![]).unwrap();
        let bytes = p.encode().unwrap();
        prop_assert_eq!(bytes.len(), p.encoded_len().unwrap());
        let decoded = Payload::decode(&bytes).unwrap();
        prop_assert_eq!(&decoded, &p);
        for (position, plan) in decoded.blocks().take(selections.len()).enumerate() {
            let BlockPlan::Derived { transactions, .. } = plan else { panic!("unexpected full block") };
            prop_assert_eq!(transactions.len(), usize::from(selections[position]));
            if selections[position] { prop_assert_eq!(&transactions[0], &tx(position as u64)); }
        }
    }

    #[test]
    fn mixed_records_match_an_independent_dense_environment_model(
        kinds in prop::collection::vec(0u8..4, 0..80),
    ) {
        let mut active = environment();
        let mut expected = Vec::new();
        let mut records = Vec::new();
        let mut gap = 0;
        let mut previous_hash = B256::ZERO;
        for (position, kind) in kinds.iter().copied().enumerate() {
            let number = 100_000 - kinds.len() as u64 + position as u64;
            let mut full = None;
            let mut txs = vec![];
            let data = match kind {
                0 => None,
                1 => {
                    txs.push(tx(position as u64));
                    Some(RecordData::Transactions(txs.clone()))
                }
                2 => {
                    let bytes = block(number, previous_hash);
                    previous_hash = alloy_rlp::decode_exact::<Block>(&bytes).unwrap().header.hash_slow();
                    full = Some(bytes.clone());
                    Some(RecordData::FullBlock(bytes.into()))
                }
                3 => {
                    // Every change is distinct, including after intervening full
                    // records; the environment remains in force across those.
                    active.extra_data = vec![position as u8].into();
                    if position % 2 == 0 {
                        txs.push(tx(position as u64));
                    }
                    Some(RecordData::EnvironmentChange {
                        environment: active.clone(), transactions: txs.clone(),
                    })
                }
                _ => unreachable!(),
            };
            expected.push((active.clone(), txs, full));
            if let Some(data) = data {
                records.push(Record { empty_gap: gap, data });
                gap = 0;
            } else {
                gap += 1;
            }
        }
        let payload = Payload::new(
            kinds.len() as u64, environment(), records,
            block(100_000, previous_hash), vec![], vec![],
        ).unwrap();
        let encoded = payload.encode().unwrap();
        prop_assert_eq!(payload.encoded_len().unwrap(), encoded.len());
        let decoded = Payload::decode(&encoded).unwrap();
        let mut plans = decoded.blocks();
        for (position, (environment, transactions, full)) in expected.iter().enumerate() {
            let expected_plan = match full {
                Some(rlp) => BlockPlan::Full { position, rlp, terminal: false },
                None => BlockPlan::Derived { position, environment, transactions },
            };
            prop_assert_eq!(plans.next(), Some(expected_plan));
        }
        prop_assert!(matches!(plans.next(), Some(BlockPlan::Full { terminal: true, .. })), "expected terminal");
        prop_assert_eq!(plans.next(), None);
    }

    #[test]
    fn mutation_cannot_create_an_accepted_noncanonical_or_unbounded_payload(index in 0usize..4096, byte in any::<u8>()) {
        let mut encoded = payload(127, vec![Record { empty_gap: 126, data: RecordData::Transactions(vec![tx(1)]) }]).encode().unwrap();
        let offset = index % encoded.len(); encoded[offset] = byte;
        if let Ok(decoded) = Payload::decode(&encoded) {
            prop_assert_eq!(decoded.encode().unwrap(), encoded);
            prop_assert!(decoded.block_count() <= MAX_BLOCKS);
            prop_assert_eq!(decoded.blocks().count(), decoded.block_count());
        }
    }
}
