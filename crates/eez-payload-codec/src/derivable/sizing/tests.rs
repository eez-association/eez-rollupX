use super::*;
use crate::derivable::{Payload, Record, RecordData};
use alloy_consensus::{BlockBody, SignableTransaction as _, TxEip1559};
use alloy_eips::{Decodable2718 as _, Encodable2718 as _};
use alloy_primitives::{Address, B256, Signature, U256};
use proptest::prelude::*;
use reth_ethereum_primitives::{Block, TransactionSigned};

#[derive(Clone)]
struct Item {
    environment: Option<Environment>,
    transactions: Vec<Bytes>,
    full: Vec<u8>,
}

fn env(id: u8) -> Environment {
    Environment {
        timestamp_step_seconds: 2,
        fee_recipient: Address::repeat_byte(id),
        prev_randao: B256::ZERO,
        gas_limit: 30_000_000,
        extra_data: vec![id].into(),
        cancun_parent_beacon_root: B256::ZERO,
    }
}

fn items(shapes: &[(u8, u8, usize)]) -> Vec<Item> {
    let mut parent_hash = B256::ZERO;
    shapes
        .iter()
        .chain(std::iter::once(&(0, 0, 0)))
        .enumerate()
        .map(|(index, &(kind, id, data_len))| {
            let transactions = if kind == 1 || kind == 3 {
                vec![
                    TxEip1559 {
                        chain_id: 1,
                        nonce: index as u64,
                        gas_limit: 100_000,
                        input: vec![0xa1; data_len].into(),
                        ..Default::default()
                    }
                    .into_signed(Signature::new(U256::from(1), U256::from(2), false))
                    .encoded_2718()
                    .into(),
                ]
            } else {
                vec![]
            };
            let block = Block {
                header: alloy_consensus::Header {
                    number: index as u64 + 1,
                    parent_hash,
                    ..Default::default()
                },
                body: BlockBody {
                    transactions: transactions
                        .iter()
                        .map(|tx: &Bytes| TransactionSigned::decode_2718(&mut tx.as_ref()).unwrap())
                        .collect(),
                    ..Default::default()
                },
            };
            parent_hash = block.header.hash_slow();
            Item {
                environment: (kind != 2).then(|| env(id)),
                transactions,
                full: alloy_rlp::encode(block),
            }
        })
        .collect()
}

fn summary(items: &[Item]) -> DerivablePayloadSizer {
    items
        .iter()
        .fold(DerivablePayloadSizer::default(), |summary, item| {
            let leaf = match &item.environment {
                Some(environment) => {
                    DerivablePayloadSizer::derived_block(environment.clone(), &item.transactions)
                }
                None => DerivablePayloadSizer::full_block(&item.full, item.transactions.len()),
            }
            .unwrap();
            summary.combine(&leaf).unwrap()
        })
}

// Dense independent writer: walk each logical position, select the initial
// environment, then emit a record only for a full block, input change or txs.
fn encoded(items: &[Item], terminal: &Item, entries: Vec<Vec<u8>>, groups: Vec<u16>) -> Payload {
    let initial = items
        .iter()
        .find_map(|item| item.environment.clone())
        .unwrap_or_else(|| env(0));
    let mut active = initial.clone();
    let mut records = Vec::new();
    let mut gap = 0;
    for item in items {
        let data = match &item.environment {
            None => Some(RecordData::FullBlock(item.full.clone().into())),
            Some(environment) if *environment != active => {
                active = environment.clone();
                Some(RecordData::EnvironmentChange {
                    environment: active.clone(),
                    transactions: item.transactions.clone(),
                })
            }
            Some(_) if !item.transactions.is_empty() => {
                Some(RecordData::Transactions(item.transactions.clone()))
            }
            Some(_) => None,
        };
        if let Some(data) = data {
            records.push(Record {
                empty_gap: gap,
                data,
            });
            gap = 0;
        } else {
            gap += 1;
        }
    }
    Payload::new(
        items.len() as u64,
        initial,
        records,
        terminal.full.clone(),
        entries,
        groups,
    )
    .unwrap()
}

fn assert_exact(items: &[Item], terminal: &Item) {
    let summary = summary(items);
    let entries = vec![vec![0xff; 56], vec![0; 128]];
    let groups = vec![0, 127, 128, 65_535];
    let payload = encoded(items, terminal, entries.clone(), groups.clone());
    assert_eq!(summary.ordinary_block_count(), items.len());
    assert_eq!(
        summary.transaction_count(),
        items
            .iter()
            .map(|item| item.transactions.len())
            .sum::<usize>()
    );
    assert_eq!(summary.record_count(), payload.records().len());
    let full_items = items
        .iter()
        .chain(std::iter::once(terminal))
        .map(|item| crate::BlockDaSizer::retained_item_bytes(&item.full).unwrap())
        .sum();
    let reserved = interval_payload_bound(items.len() + 1, full_items).unwrap();
    assert!(
        reserved
            >= encoded(items, terminal, vec![], vec![])
                .encode()
                .unwrap()
                .len()
    );
    assert_eq!(
        summary
            .encoded_len(&env(0), &terminal.full, &entries, &groups)
            .unwrap(),
        payload.encode().unwrap().len()
    );
}

#[test]
fn future_interval_reserve_rejects_zero_oversized_and_overflowing_inputs() {
    assert!(interval_payload_bound(0, 0).is_err());
    assert!(interval_payload_bound(65_537, 0).is_err());
    assert!(interval_payload_bound(1, usize::MAX).is_err());
    assert!(interval_payload_bound(65_536, 0).is_ok());
}

#[test]
fn joining_changed_empty_environment_inserts_a_record_and_reanchors_next_gap() {
    for gap in [0, 1, 54, 55, 56, 126, 127, 128, 254, 255, 256] {
        let mut shapes = vec![(0, 0, 0); gap];
        shapes.extend([(0, 1, 0), (0, 2, 0)]);
        shapes.extend(vec![(0, 2, 0); gap]);
        shapes.push((1, 2, 56));
        let data = items(&shapes);
        assert_exact(&data[..shapes.len()], &data[shapes.len()]);
        for at in [0, gap, gap + 1, gap + 2] {
            assert_exact(&data[at..shapes.len()], &data[shapes.len()]);
        }
    }
}

#[test]
fn full_prefixes_and_terminal_promotion_do_not_delay_environment_changes() {
    let data = items(&[
        (2, 0, 0),
        (2, 0, 0),
        (0, 1, 0),
        (2, 0, 0),
        (3, 2, 55),
        (0, 2, 0),
    ]);
    for terminal in 0..data.len() {
        for start in 0..=terminal {
            assert_exact(&data[start..terminal], &data[terminal]);
        }
    }
}

#[test]
fn transaction_and_nested_record_rlp_widths_match_actual_encoding() {
    for length in [0, 1, 55, 56, 255, 256, 65_535, 65_536] {
        let data = items(&[(1, 128, length), (0, 129, 0), (1, 129, length)]);
        assert_exact(&data[..3], &data[3]);
    }
}

#[test]
fn maximum_summary_count_and_checked_size_failures_are_bounded() {
    let leaf = DerivablePayloadSizer::derived_block(env(0), &[]).unwrap();
    let mut summary = DerivablePayloadSizer::default();
    for _ in 0..65_535 {
        summary = summary.combine(&leaf).unwrap();
    }
    assert_eq!(summary.record_count(), 0);
    assert!(matches!(
        summary.combine(&leaf),
        Err(Error::BlockLimit(65_536))
    ));
    assert!(matches!(replace_len(0, 1, 0), Err(Error::Overflow)));
    assert!(matches!(
        replace_len(usize::MAX, 0, 1),
        Err(Error::Overflow)
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn composition_is_associative_and_matches_dense_encoding(
        shapes in prop::collection::vec((0u8..4, any::<u8>(), 0usize..260), 0..100),
        left in 0usize..100, right in 0usize..100,
    ) {
        let data = items(&shapes);
        let end = shapes.len();
        let first = left.min(end);
        let second = right.min(end).max(first);
        let a = summary(&data[..first]);
        let b = summary(&data[first..second]);
        let c = summary(&data[second..end]);
        prop_assert_eq!(a.combine(&b).unwrap().combine(&c).unwrap(), a.combine(&b.combine(&c).unwrap()).unwrap());
        assert_exact(&data[..end], &data[end]);
        assert_exact(&data[first..second], &data[second]);
    }
}
