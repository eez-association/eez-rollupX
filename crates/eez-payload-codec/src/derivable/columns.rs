//! Count, exception and transaction columns share the existing sparse plan API.

use alloy_primitives::Bytes;
use alloy_rlp::{Decodable, Encodable, Header};

use super::{
    Environment, Error, Record, RecordData, add,
    counts::{self, Token},
    end, list_header, list_len, record_position,
};

fn overrides(records: &[Record]) -> impl Iterator<Item = (u64, &RecordData)> {
    let mut gap = 0;
    records.iter().filter_map(move |record| {
        gap += record.empty_gap;
        if matches!(record.data, RecordData::Transactions(_)) {
            gap += 1;
            None
        } else {
            let current = gap;
            gap = 0;
            Some((current, &record.data))
        }
    })
}

fn transactions(records: &[Record]) -> impl Iterator<Item = &Bytes> {
    records.iter().flat_map(|record| match &record.data {
        RecordData::Transactions(txs)
        | RecordData::EnvironmentChange {
            transactions: txs, ..
        } => txs.as_slice(),
        RecordData::FullBlock(_) => &[],
    })
}

fn override_body_len(gap: u64, data: &RecordData) -> Result<usize, Error> {
    let len = match data {
        RecordData::FullBlock(bytes) => bytes.length(),
        RecordData::EnvironmentChange { environment, .. } => environment.length(),
        RecordData::Transactions(_) => unreachable!("filtered out by overrides"),
    };
    add(add(gap.length(), 1)?, len)
}

pub(super) fn lengths(records: &[Record]) -> Result<(usize, usize), Error> {
    let overrides = overrides(records).try_fold(0, |sum, (gap, data)| {
        add(sum, list_len(override_body_len(gap, data)?)?)
    })?;
    let txs = transactions(records).try_fold(0, |sum, tx| add(sum, tx.length()))?;
    Ok((overrides, txs))
}

pub(super) fn body_len(counts: &Bytes, (overrides, txs): (usize, usize)) -> Result<usize, Error> {
    add(add(counts.length(), list_len(overrides)?)?, list_len(txs)?)
}

pub(super) fn encode(
    counts: &Bytes,
    records: &[Record],
    lengths: (usize, usize),
    out: &mut Vec<u8>,
) -> Result<(), Error> {
    let (override_len, tx_len) = lengths;
    list_header(body_len(counts, lengths)?, out);
    counts.encode(out);
    list_header(override_len, out);
    for (gap, data) in overrides(records) {
        list_header(override_body_len(gap, data)?, out);
        gap.encode(out);
        match data {
            RecordData::FullBlock(bytes) => {
                1u8.encode(out);
                bytes.encode(out);
            }
            RecordData::EnvironmentChange { environment, .. } => {
                2u8.encode(out);
                environment.encode(out);
            }
            RecordData::Transactions(_) => unreachable!("filtered out by overrides"),
        }
    }
    list_header(tx_len, out);
    for tx in transactions(records) {
        tx.encode(out);
    }
    Ok(())
}

pub(super) fn decode<'a>(
    input: &mut &'a [u8],
    count: usize,
) -> Result<(Vec<Record>, &'a [u8]), Error> {
    let mut columns = Header::decode_bytes(input, true)?;
    let encoded_counts = Header::decode_bytes(&mut columns, false)?;
    // Check canonical tokens and coverage before allocating any records/txs.
    let mut tokens = encoded_counts;
    let mut covered = 0usize;
    let mut previous_empty = false;
    while !tokens.is_empty() {
        let span = match counts::read(&mut tokens, &mut previous_empty)? {
            Token::Transactions(_) => 1,
            Token::Empty(n) => usize::try_from(n).map_err(|_| Error::CountEncoding)?,
        };
        covered = covered
            .checked_add(span)
            .filter(|n| *n <= count)
            .ok_or(Error::CountEncoding)?;
    }
    if covered != count {
        return Err(Error::CountEncoding);
    }
    let mut encoded_overrides = Header::decode_bytes(&mut columns, true)?;
    let mut overrides = Vec::new();
    let mut position = 0;
    while !encoded_overrides.is_empty() {
        let mut data = Header::decode_bytes(&mut encoded_overrides, true)?;
        let gap = u64::decode(&mut data)?;
        position = record_position(position, gap, count, overrides.len())?;
        let record = match u8::decode(&mut data)? {
            1 => RecordData::FullBlock(Bytes::decode(&mut data)?),
            2 => RecordData::EnvironmentChange {
                environment: Environment::decode(&mut data)?,
                transactions: vec![],
            },
            kind => return Err(Error::RecordKind(kind)),
        };
        end(data)?;
        overrides.push((position, record));
        position += 1;
    }
    let mut txs = Header::decode_bytes(&mut columns, true)?;
    end(columns)?;
    let mut overrides = overrides.into_iter().peekable();
    let mut records = Vec::new();
    let mut next_record = 0;
    let mut position = 0;
    let mut tokens = encoded_counts;
    let mut previous_empty = false;
    while !tokens.is_empty() {
        let token = counts::read(&mut tokens, &mut previous_empty)?;
        let (span, tx_count) = match token {
            Token::Empty(n) => (n as usize, 0),
            Token::Transactions(n) => (1, n),
        };
        let until = position + span;
        // Empty runs jump directly between overrides, without allocating or
        // iterating through every empty block in the declared history.
        while position < until {
            let data = if overrides.peek().is_some_and(|(at, _)| *at == position) {
                let (_, mut data) = overrides.next().ok_or(Error::CountEncoding)?;
                match &mut data {
                    // Payload::new validates full blocks once and rebuilds the
                    // canonical counts; the caller must compare that column.
                    RecordData::FullBlock(_) => {}
                    RecordData::EnvironmentChange { transactions, .. } => {
                        *transactions = read_transactions(&mut txs, tx_count)?
                    }
                    RecordData::Transactions(_) => unreachable!("only kinds 1 and 2 decoded"),
                }
                Some(data)
            } else if tx_count > 0 {
                Some(RecordData::Transactions(read_transactions(
                    &mut txs, tx_count,
                )?))
            } else {
                None
            };
            if let Some(data) = data {
                records.push(Record {
                    empty_gap: (position - next_record) as u64,
                    data,
                });
                next_record = position + 1;
            }
            position = if tx_count == 0 {
                overrides.peek().map_or(until, |(at, _)| *at).min(until)
            } else {
                until
            };
        }
    }
    end(txs)?;
    Ok((records, encoded_counts))
}

fn read_transactions(input: &mut &[u8], count: usize) -> Result<Vec<Bytes>, Error> {
    let mut txs = Vec::new();
    for _ in 0..count {
        txs.push(Bytes::decode(input)?);
    }
    Ok(txs)
}
