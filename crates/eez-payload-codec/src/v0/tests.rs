use super::*;
use alloy_consensus::{SignableTransaction, TxEip1559};
use alloy_primitives::{Signature, U256};
use proptest::prelude::*;

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

fn empty(blocks: u32) -> BlockRun<'static> {
    BlockRun {
        blocks,
        beneficiary: Address::repeat_byte(0x11),
        extra_data: &[],
        transactions: &[],
    }
}

// Independent encoder for the published u32 integer contract.
fn reference_integer(mut n: u32) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (n % 128) as u8;
        n /= 128;
        out.push(if n > 0 { byte + 128 } else { byte });
        if n == 0 {
            return out;
        }
    }
}

fn reference_empty(n: u32) -> Vec<u8> {
    let integer = reference_integer(n);
    let mut out = vec![0];
    out.extend_from_slice(&integer);
    match n {
        1 => out.push(0),
        2..=64 => out.push(0xc0 + (n - 2) as u8),
        _ => {
            out.push(0xff);
            out.extend_from_slice(&integer);
        }
    }
    out.extend_from_slice(&integer);
    out.extend_from_slice(&[0x11; 20]);
    out.extend_from_slice(&integer);
    out.push(0);
    out
}

#[test]
fn independent_empty_vectors_include_terminal_and_full_u32_runs() {
    for (n, bytes) in [
        (6, 26),
        (600, 31),
        (60_000, 35),
        (43_200, 35),
        (u32::MAX, 43),
    ] {
        let expected = reference_empty(n);
        assert_eq!(expected.len(), bytes);
        assert_eq!(encode(&[empty(n)]).unwrap(), expected);
        let payload = Payload::decode(&expected).unwrap();
        assert_eq!(payload.encoded(), expected);
        assert_eq!(payload.block_count(), n);
        assert_eq!(payload.transaction_count(), 0);
        let terminal = payload.terminal();
        assert_eq!(terminal.beneficiary, Address::repeat_byte(0x11));
        assert!(terminal.extra_data.is_empty());
        assert_eq!(terminal.transactions.len(), 0);
        assert_eq!(
            payload.count_runs().collect::<Vec<_>>(),
            vec![CountRun {
                blocks: n,
                transactions_per_block: 0
            }]
        );
        // No iteration or allocation proportional to n is needed to admit the plan.
        assert_eq!(payload.blocks().len(), n as usize);
        let first = payload.blocks().next().unwrap();
        assert_eq!(first.beneficiary, Address::repeat_byte(0x11));
        assert!(first.extra_data.is_empty());
        assert_eq!(first.transactions.len(), 0);
    }
}

#[test]
fn terminal_metadata_and_transactions_are_columns_without_full_block_escape() {
    let transaction = tx(19);
    let runs = [
        empty(1),
        BlockRun {
            blocks: 1,
            beneficiary: Address::repeat_byte(0x22),
            extra_data: &[7],
            transactions: std::slice::from_ref(&transaction),
        },
    ];
    let mut expected = vec![0, 2, 0, 1, 1];
    expected.extend_from_slice(&[0x11; 20]);
    expected.push(1);
    expected.extend_from_slice(&[0x22; 20]);
    expected.extend_from_slice(&[1, 0, 1, 1, 7]);
    expected.extend_from_slice(&reference_integer(transaction.len() as u32));
    expected.extend_from_slice(&transaction);
    assert_eq!(encode(&runs).unwrap(), expected);
    let payload = Payload::decode(&expected).unwrap();
    let positions = payload.blocks().collect::<Vec<_>>();
    assert_eq!(positions.len(), 2);
    assert_eq!(positions[0].transactions.len(), 0);
    assert_eq!(positions[1].beneficiary, Address::repeat_byte(0x22));
    assert_eq!(positions[1].extra_data, &[7]);
    assert_eq!(
        positions[1].transactions.clone().collect::<Vec<_>>(),
        vec![transaction.as_ref()]
    );
}

#[test]
fn metadata_changes_do_not_split_empty_count_runs() {
    let runs = [
        empty(3),
        BlockRun {
            beneficiary: Address::repeat_byte(0x22),
            extra_data: &[1, 2],
            ..empty(5)
        },
        empty(2),
    ];
    let wire = encode(&runs).unwrap();
    let payload = Payload::decode(&wire).unwrap();
    assert_eq!(payload.counts, &[0xc8]); // Ten empty prefixes, including terminal.
    let blocks = payload.blocks().collect::<Vec<_>>();
    assert_eq!(blocks[2].beneficiary, Address::repeat_byte(0x11));
    assert_eq!(blocks[3].beneficiary, Address::repeat_byte(0x22));
    assert_eq!(blocks[7].extra_data, &[1, 2]);
    assert!(blocks[8].extra_data.is_empty());
}

#[test]
fn all_shortest_u32_boundaries_and_count_tokens_are_checked_without_expansion() {
    for n in [1, 2, 63, 64, 65, 127, 128, 16_383, 16_384, 65_536, u32::MAX] {
        assert!(Payload::decode(&reference_empty(n)).is_ok());
    }
    for malformed in [
        &[0x80, 0][..],
        &[0x80, 0x7f],
        &[0xff, 0],
        &[0xff, 0x40],
        &[0xff, 0xc1, 0],
        &[0x80],
        &[0xff],
        &[0, 0],
        &[0xff, 0xff, 0xff, 0xff, 0xff, 0x10],
    ] {
        let mut wire = vec![0, 2];
        wire.extend_from_slice(malformed);
        wire.extend_from_slice(&reference_empty(2)[3..]);
        assert!(Payload::decode(&wire).is_err(), "{malformed:x?}");
    }
    for invalid in [
        &[0x80, 0][..],
        &[0xff, 0xff, 0xff, 0xff, 16],
        &[0x80, 0x80, 0x80, 0x80, 0x80],
        &[],
    ] {
        assert!(varint(&mut &*invalid).is_err());
    }
    assert_eq!(Payload::decode(&[0, 0]).unwrap_err(), Error::EmptyRange);
    assert_eq!(Payload::decode(&[1]).unwrap_err(), Error::Version);
    let mut suffix = reference_empty(600);
    suffix.push(0);
    assert_eq!(Payload::decode(&suffix).unwrap_err(), Error::Trailing);
}

#[test]
fn metadata_must_cover_exactly_and_be_maximal() {
    let mut wire = vec![0, 2, 0xc0, 1];
    wire.extend_from_slice(&[0x11; 20]);
    wire.push(1);
    wire.extend_from_slice(&[0x11; 20]);
    wire.extend_from_slice(&[2, 0]);
    assert_eq!(Payload::decode(&wire).unwrap_err(), Error::Metadata);
    let mut wire = reference_empty(2);
    wire[3] = 0;
    assert_eq!(Payload::decode(&wire).unwrap_err(), Error::Metadata);
    wire[3] = 3;
    assert_eq!(Payload::decode(&wire).unwrap_err(), Error::Metadata);
    let mut wire = reference_empty(2);
    wire.pop();
    wire.push(33);
    wire.extend_from_slice(&[0; 33]);
    assert_eq!(Payload::decode(&wire).unwrap_err(), Error::ExtraData);
    let mut wire = reference_empty(2);
    wire.truncate(24);
    wire.extend_from_slice(&[1, 0, 1, 0]);
    assert_eq!(Payload::decode(&wire).unwrap_err(), Error::Metadata);
}

#[test]
fn lengths_and_transactions_fail_at_their_boundary() {
    let mut wire = reference_empty(1);
    wire[2] = 1;
    assert_eq!(Payload::decode(&wire).unwrap_err(), Error::TransactionCount);
    wire.extend_from_slice(&[0, 0]);
    assert_eq!(
        Payload::decode(&wire).unwrap_err(),
        Error::TransactionLength
    );
    *wire.last_mut().unwrap() = 0xff;
    wire[26] = 1;
    assert_eq!(
        Payload::decode(&wire).unwrap_err(),
        Error::TransactionEncoding
    );
    for ty in [eez_primitives::SYSTEM_TX_TYPE, 3] {
        *wire.last_mut().unwrap() = ty;
        assert_eq!(
            Payload::decode(&wire).unwrap_err(),
            Error::UnsupportedTransaction
        );
    }
    let mut wire = reference_empty(1);
    wire[2] = 1;
    wire.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x0f, 1]);
    assert_eq!(
        Payload::decode(&wire).unwrap_err(),
        Error::TransactionLength
    );
    let encoded = encode(&[BlockRun {
        transactions: &[tx(1)],
        ..empty(1)
    }])
    .unwrap();
    for end in 0..encoded.len() {
        assert!(
            Payload::decode(&encoded[..end]).is_err(),
            "truncation at {end}"
        );
    }
}

#[test]
fn encoder_and_sizer_reject_nonrepresentable_runs() {
    assert_eq!(encode(&[]).unwrap_err(), Error::EmptyRange);
    assert_eq!(encode(&[empty(0)]).unwrap_err(), Error::EmptyRange);
    assert_eq!(
        encode(&[empty(u32::MAX), empty(1)]).unwrap_err(),
        Error::Overflow
    );
    assert_eq!(
        encode(&[BlockRun {
            transactions: &[tx(0)],
            ..empty(2)
        }])
        .unwrap_err(),
        Error::TransactionCount
    );
    let txs = vec![tx(0); MAX_TRANSACTIONS + 1];
    assert_eq!(
        encode(&[BlockRun {
            transactions: &txs,
            ..empty(1)
        }])
        .unwrap_err(),
        Error::TransactionCount
    );
    assert_eq!(
        encode(&[BlockRun {
            transactions: &[Bytes::new()],
            ..empty(1)
        }])
        .unwrap_err(),
        Error::TransactionLength
    );
    assert_eq!(
        encode(&[BlockRun {
            extra_data: &[0; 33],
            ..empty(1)
        }])
        .unwrap_err(),
        Error::ExtraData
    );
}

proptest! {
    #[test]
    fn sized_concatenation_matches_encoding_and_lazy_dense_reference(
        shapes in prop::collection::vec((0u8..5, 0u8..5, 0usize..4, 1u32..70), 1..80),
        a in 0usize..80, b in 0usize..80,
    ) {
        let transactions: Vec<Vec<Bytes>> = shapes.iter().enumerate().map(|(i, (_, _, n, _))| (0..*n).map(|k| tx((i * 10 + k) as u64)).collect()).collect();
        let metadata: Vec<Vec<u8>> = shapes.iter().map(|(_, id, _, _)| vec![*id; usize::from(*id)]).collect();
        let runs: Vec<_> = shapes.iter().enumerate().map(|(i, (address, _, n, blocks))| BlockRun {
            blocks: if *n == 0 { *blocks } else { 1 }, beneficiary: Address::repeat_byte(*address),
            extra_data: &metadata[i], transactions: &transactions[i],
        }).collect();
        let sum = |runs: &[BlockRun<'_>]| runs.iter().fold(PayloadSizer::default(), |sum, r| sum.combine(&PayloadSizer::run(r).unwrap()).unwrap());
        let a = a.min(runs.len()); let b = b.max(a).min(runs.len());
        let left = sum(&runs[..a]); let mid = sum(&runs[a..b]); let right = sum(&runs[b..]);
        let summary = sum(&runs);
        prop_assert_eq!(left.combine(&mid).unwrap().combine(&right).unwrap(), left.combine(&mid.combine(&right).unwrap()).unwrap());
        let wire = encode(&runs).unwrap();
        prop_assert_eq!(summary.encoded_len().unwrap(), wire.len());
        let payload = Payload::decode(&wire).unwrap();
        prop_assert_eq!(payload.block_count(), runs.iter().map(|r| r.blocks).sum::<u32>());
        let mut plan = payload.blocks();
        for run in &runs { for _ in 0..run.blocks {
            let block = plan.next().unwrap();
            prop_assert_eq!(block.beneficiary, run.beneficiary);
            prop_assert_eq!(block.extra_data, run.extra_data);
            prop_assert_eq!(block.transactions.collect::<Vec<_>>(), run.transactions.iter().map(Bytes::as_ref).collect::<Vec<_>>());
        } }
        prop_assert!(plan.next().is_none());
        let terminal = payload.terminal();
        let last = runs.last().unwrap();
        prop_assert_eq!(terminal.beneficiary, last.beneficiary);
        prop_assert_eq!(terminal.extra_data, last.extra_data);
        prop_assert_eq!(terminal.transactions.collect::<Vec<_>>(), last.transactions.iter().map(Bytes::as_ref).collect::<Vec<_>>());
        prop_assert_eq!(payload.encode().unwrap(), wire);
    }

    #[test]
    fn arbitrary_bytes_do_not_panic_or_expand_declared_empty_spans(bytes in prop::collection::vec(any::<u8>(), 0..1024)) {
        if let Ok(payload) = Payload::decode(&bytes) {
            prop_assert_eq!(payload.encoded(), bytes.as_slice());
            prop_assert!(payload.count_runs().count() <= bytes.len());
            prop_assert!(payload.transactions().count() <= bytes.len() / 2);
        }
    }
}
