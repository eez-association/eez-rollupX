//! Direct columnar V0 operations, including the terminal ordinary prefix.
//!
//! Parsing borrows the input and never allocates by the declared block count.
//! A valid u32 empty run is a wire value, not a promise that a node will execute
//! billions of blocks in one request. Callers separately enforce authenticated
//! interval rules and operational replay budgets, executing every admitted block.
//! Actions and authorization belong to the enclosing native EEZ stream.

use alloy_consensus::{Transaction, TxEnvelope};
use alloy_eips::{Decodable2718, Encodable2718};
use alloy_primitives::{Address, Bytes};
use std::iter::FusedIterator;

mod counts;
use counts::Token;

mod sizing;
pub use sizing::PayloadSizer;

pub const VERSION: u8 = 0;
pub const MAX_TRANSACTIONS: usize = counts::MAX_TRANSACTIONS;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("expected direct operations version 0x00")]
    Version,
    #[error("truncated operations")]
    Truncated,
    #[error("noncanonical or overflowing u32 varint")]
    Varint,
    #[error("operations must cover at least one block")]
    EmptyRange,
    #[error("invalid transaction-count coverage or token")]
    Counts,
    #[error("metadata runs must be positive, maximal and cover every block")]
    Metadata,
    #[error("extraData exceeds 32 bytes")]
    ExtraData,
    #[error("pure L2 transactions exceed the count or remaining-byte bound")]
    TransactionCount,
    #[error("transaction length must be positive and fit the remaining bytes")]
    TransactionLength,
    #[error("invalid or noncanonical pure L2 transaction")]
    TransactionEncoding,
    #[error("native and blob transactions cannot appear in the pure L2 column")]
    UnsupportedTransaction,
    #[error("operations have trailing bytes")]
    Trailing,
    #[error("operations size or block count overflow")]
    Overflow,
    #[error("cannot allocate encoded operations")]
    Allocation,
}

/// Producer input: one nonempty ordinary prefix, or a run of empty prefixes
/// sharing metadata. Empty count runs are merged even when metadata changes.
#[derive(Clone, Copy, Debug)]
pub struct BlockRun<'a> {
    pub blocks: u32,
    pub beneficiary: Address,
    pub extra_data: &'a [u8],
    pub transactions: &'a [Bytes],
}

/// Structurally checked, borrowed columns. This type proves canonical syntax;
/// it neither authenticates settlement nor proves transaction execution.
#[derive(Clone, Debug)]
pub struct Payload<'a> {
    wire: &'a [u8],
    blocks: u32,
    transaction_count: usize,
    counts: &'a [u8],
    beneficiaries: &'a [u8],
    extra_data: &'a [u8],
    lengths: &'a [u8],
    transactions: &'a [u8],
}

impl<'a> Payload<'a> {
    /// Check all column coverage, canonical integers, metadata and exact
    /// EIP-2718 bytes before returning a lazy plan. Work is proportional to
    /// supplied bytes, including for a u32::MAX empty span.
    pub fn decode(wire: &'a [u8]) -> Result<Self, Error> {
        let mut input = wire;
        if take(&mut input, 1)? != [VERSION] {
            return Err(Error::Version);
        }
        let blocks = varint(&mut input)?;
        if blocks == 0 {
            return Err(Error::EmptyRange);
        }
        let start = input;
        let mut covered = 0u32;
        let mut previous_empty = false;
        let mut transaction_count = 0usize;
        while covered < blocks {
            let token = counts::read(&mut input, &mut previous_empty).map_err(|_| Error::Counts)?;
            let positions = match token {
                Token::Transactions(n) => {
                    transaction_count = add(transaction_count, n)?;
                    1
                }
                Token::Empty(n) => n,
            };
            covered = covered
                .checked_add(positions)
                .filter(|n| *n <= blocks)
                .ok_or(Error::Counts)?;
        }
        let counts = consumed(start, input);
        let start = input;
        validate_metadata(&mut input, blocks, false)?;
        let beneficiaries = consumed(start, input);
        let start = input;
        validate_metadata(&mut input, blocks, true)?;
        let extra_data = consumed(start, input);
        // Every transaction needs at least a length byte and one data byte.
        // This bound replaces the invalid dense-count block_count <= bytes rule.
        if transaction_count > input.len() / 2 {
            return Err(Error::TransactionCount);
        }
        let start = input;
        let mut total_bytes = 0usize;
        for _ in 0..transaction_count {
            let len = varint(&mut input)? as usize;
            if len == 0 {
                return Err(Error::TransactionLength);
            }
            total_bytes = add(total_bytes, len)?;
            if total_bytes > input.len() {
                return Err(Error::TransactionLength);
            }
        }
        let lengths = consumed(start, input);
        if total_bytes != input.len() {
            return Err(Error::Trailing);
        }
        let payload = Self {
            wire,
            blocks,
            transaction_count,
            counts,
            beneficiaries,
            extra_data,
            lengths,
            transactions: input,
        };
        for tx in payload.transactions() {
            validate_transaction(tx)?;
        }
        Ok(payload)
    }

    pub const fn block_count(&self) -> u32 {
        self.blocks
    }
    pub const fn transaction_count(&self) -> usize {
        self.transaction_count
    }
    pub const fn encoded(&self) -> &'a [u8] {
        self.wire
    }

    /// Canonical decoding requires exact byte equality; no normalization occurs.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        Ok(self.wire.to_vec())
    }

    pub fn transactions(&self) -> Transactions<'a> {
        Transactions {
            lengths: self.lengths,
            data: self.transactions,
            remaining: self.transaction_count,
        }
    }

    /// Run-wise counts permit resource/schedule admission without expanding the
    /// declared span. Metadata boundaries do not split empty count runs.
    pub fn count_runs(&self) -> CountRuns<'a> {
        CountRuns { input: self.counts }
    }

    /// Each item is one execution position. Consumers must apply their replay
    /// cancellation/deadline policy between positions, including empty blocks.
    pub fn blocks(&self) -> Blocks<'a> {
        Blocks {
            remaining: self.blocks,
            counts: self.counts,
            empty_left: 0,
            beneficiaries: MetadataCursor::new(self.beneficiaries, false),
            extra_data: MetadataCursor::new(self.extra_data, true),
            transactions: self.transactions(),
        }
    }

    /// Terminal metadata and ordinary prefix without expanding empty runs.
    /// Partial-consumption replay may need only this position of a large span.
    pub fn terminal(&self) -> BlockPlan<'a> {
        let count = self
            .count_runs()
            .last()
            .expect("nonempty checked range")
            .transactions_per_block;
        let mut transactions = self.transactions();
        for _ in 0..self.transaction_count - count {
            transactions.next().expect("checked transaction coverage");
        }
        let last_metadata = |mut input: &'a [u8], variable| {
            let mut value = &input[..0];
            while !input.is_empty() {
                varint(&mut input).expect("checked metadata run");
                value = metadata_value(&mut input, variable).expect("checked metadata value");
            }
            value
        };
        BlockPlan {
            beneficiary: Address::from_slice(last_metadata(self.beneficiaries, false)),
            extra_data: last_metadata(self.extra_data, true),
            transactions,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Transactions<'a> {
    lengths: &'a [u8],
    data: &'a [u8],
    remaining: usize,
}

impl<'a> Iterator for Transactions<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        // Only Payload::decode and Blocks split these private, validated columns.
        let len = varint(&mut self.lengths).expect("validated transaction length") as usize;
        let bytes = take(&mut self.data, len).expect("validated transaction coverage");
        self.remaining -= 1;
        Some(bytes)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for Transactions<'_> {}
impl FusedIterator for Transactions<'_> {}

/// One maximal empty span, or a single position with a positive pure-L2 count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CountRun {
    pub blocks: u32,
    pub transactions_per_block: usize,
}

#[derive(Debug)]
pub struct CountRuns<'a> {
    input: &'a [u8],
}
impl Iterator for CountRuns<'_> {
    type Item = CountRun;
    fn next(&mut self) -> Option<Self::Item> {
        if self.input.is_empty() {
            return None;
        }
        Some(
            match counts::read(&mut self.input, &mut false).expect("validated count column") {
                Token::Empty(blocks) => CountRun {
                    blocks,
                    transactions_per_block: 0,
                },
                Token::Transactions(transactions_per_block) => CountRun {
                    blocks: 1,
                    transactions_per_block,
                },
            },
        )
    }
}
impl FusedIterator for CountRuns<'_> {}

#[derive(Debug)]
struct MetadataCursor<'a> {
    input: &'a [u8],
    variable: bool,
    left: u32,
    value: &'a [u8],
}
impl<'a> MetadataCursor<'a> {
    fn new(input: &'a [u8], variable: bool) -> Self {
        Self {
            input,
            variable,
            left: 0,
            value: &[],
        }
    }
    fn next(&mut self) -> &'a [u8] {
        if self.left == 0 {
            self.left = varint(&mut self.input).expect("validated metadata run");
            self.value =
                metadata_value(&mut self.input, self.variable).expect("validated metadata value");
        }
        self.left -= 1;
        self.value
    }
}

#[derive(Debug)]
pub struct BlockPlan<'a> {
    pub beneficiary: Address,
    pub extra_data: &'a [u8],
    pub transactions: Transactions<'a>,
}

#[derive(Debug)]
pub struct Blocks<'a> {
    remaining: u32,
    counts: &'a [u8],
    empty_left: u32,
    beneficiaries: MetadataCursor<'a>,
    extra_data: MetadataCursor<'a>,
    transactions: Transactions<'a>,
}
impl<'a> Iterator for Blocks<'a> {
    type Item = BlockPlan<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let count = if self.empty_left > 0 {
            0
        } else {
            match counts::read(&mut self.counts, &mut false).expect("validated count coverage") {
                Token::Empty(n) => {
                    self.empty_left = n;
                    0
                }
                Token::Transactions(n) => n,
            }
        };
        self.empty_left = self.empty_left.saturating_sub(1);
        let lengths = self.transactions.lengths;
        let data = self.transactions.data;
        for _ in 0..count {
            self.transactions
                .next()
                .expect("validated block transaction count");
        }
        self.remaining -= 1;
        Some(BlockPlan {
            beneficiary: Address::from_slice(self.beneficiaries.next()),
            extra_data: self.extra_data.next(),
            transactions: Transactions {
                lengths: consumed(lengths, self.transactions.lengths),
                data: consumed(data, self.transactions.data),
                remaining: count,
            },
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.remaining as usize;
        (n, Some(n))
    }
}
impl ExactSizeIterator for Blocks<'_> {}
impl FusedIterator for Blocks<'_> {}

/// Encode producer runs into the unique columnar body. Runs may be split at any
/// metadata boundary; all three columns independently coalesce equal neighbors.
pub fn encode(runs: &[BlockRun<'_>]) -> Result<Vec<u8>, Error> {
    let summary = runs.iter().try_fold(PayloadSizer::default(), |sum, run| {
        sum.combine(&PayloadSizer::run(run)?)
    })?;
    let mut out = Vec::new();
    out.try_reserve_exact(summary.encoded_len()?)
        .map_err(|_| Error::Allocation)?;
    out.push(VERSION);
    write_varint(summary.block_count(), &mut out);
    let mut empty = 0u32;
    for run in runs {
        if run.transactions.is_empty() {
            empty += run.blocks;
        } else {
            if empty > 0 {
                Token::Empty(empty).encode(&mut out);
                empty = 0;
            }
            Token::Transactions(run.transactions.len()).encode(&mut out);
        }
    }
    if empty > 0 {
        Token::Empty(empty).encode(&mut out);
    }
    encode_metadata(runs, false, &mut out);
    encode_metadata(runs, true, &mut out);
    for tx in runs.iter().flat_map(|run| run.transactions) {
        write_varint(tx.len() as u32, &mut out);
    }
    for tx in runs.iter().flat_map(|run| run.transactions) {
        validate_transaction(tx)?;
        out.extend_from_slice(tx);
    }
    debug_assert_eq!(out.len(), summary.encoded_len()?);
    Ok(out)
}

fn encode_metadata(runs: &[BlockRun<'_>], variable: bool, out: &mut Vec<u8>) {
    let mut runs = runs.iter().peekable();
    while let Some(first) = runs.next() {
        let value = if variable {
            first.extra_data
        } else {
            first.beneficiary.as_slice()
        };
        let mut length = first.blocks;
        while let Some(next) = runs.peek() {
            let next_value = if variable {
                next.extra_data
            } else {
                next.beneficiary.as_slice()
            };
            if next_value != value {
                break;
            }
            length += next.blocks;
            runs.next();
        }
        write_varint(length, out);
        if variable {
            out.push(value.len() as u8);
        }
        out.extend_from_slice(value);
    }
}

fn validate_transaction(bytes: &[u8]) -> Result<(), Error> {
    if bytes
        .first()
        .is_some_and(|ty| *ty == eez_primitives::SYSTEM_TX_TYPE || *ty == 3)
    {
        return Err(Error::UnsupportedTransaction);
    }
    let tx = TxEnvelope::decode_2718_exact(bytes).map_err(|_| Error::TransactionEncoding)?;
    if tx.blob_versioned_hashes().is_some() {
        return Err(Error::UnsupportedTransaction);
    }
    if tx.encoded_2718() != bytes {
        return Err(Error::TransactionEncoding);
    }
    Ok(())
}

fn validate_metadata(input: &mut &[u8], blocks: u32, variable: bool) -> Result<(), Error> {
    let mut covered = 0u32;
    let mut previous = None;
    while covered < blocks {
        let length = varint(input)?;
        if length == 0 {
            return Err(Error::Metadata);
        }
        covered = covered
            .checked_add(length)
            .filter(|n| *n <= blocks)
            .ok_or(Error::Metadata)?;
        let value = metadata_value(input, variable)?;
        if previous == Some(value) {
            return Err(Error::Metadata);
        }
        previous = Some(value);
    }
    Ok(())
}

fn metadata_value<'a>(input: &mut &'a [u8], variable: bool) -> Result<&'a [u8], Error> {
    let length = if variable {
        usize::from(take(input, 1)?[0])
    } else {
        20
    };
    if variable && length > 32 {
        return Err(Error::ExtraData);
    }
    take(input, length)
}

fn take<'a>(input: &mut &'a [u8], count: usize) -> Result<&'a [u8], Error> {
    let (value, rest) = input.split_at_checked(count).ok_or(Error::Truncated)?;
    *input = rest;
    Ok(value)
}

fn consumed<'a>(before: &'a [u8], after: &[u8]) -> &'a [u8] {
    &before[..before.len() - after.len()]
}
fn add(a: usize, b: usize) -> Result<usize, Error> {
    a.checked_add(b).ok_or(Error::Overflow)
}
pub(crate) fn varint_len(n: u32) -> usize {
    ((32 - n.leading_zeros()).max(1)).div_ceil(7) as usize
}
fn write_varint(mut n: u32, out: &mut Vec<u8>) {
    while n >= 128 {
        out.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    out.push(n as u8);
}
fn varint(input: &mut &[u8]) -> Result<u32, Error> {
    let mut n = 0u32;
    for shift in (0..=28).step_by(7) {
        let b = take(input, 1)?[0];
        if shift == 28 && b > 15 {
            return Err(Error::Varint);
        }
        n |= u32::from(b & 127) << shift;
        if b < 128 {
            if shift > 0 && b == 0 {
                return Err(Error::Varint);
            }
            return Ok(n);
        }
    }
    Err(Error::Varint)
}

#[cfg(test)]
mod tests;

/// Conservative whole-interval bound for incremental block admission. `items`
/// already charges separate per-block count/metadata runs and exact transaction
/// lengths; canonical merging may reduce that amount. This is not a wire cap.
pub fn interval_payload_bound(blocks: usize, items: usize) -> Result<usize, Error> {
    let blocks = u32::try_from(blocks).map_err(|_| Error::Overflow)?;
    if blocks == 0 {
        return Err(Error::EmptyRange);
    }
    add(add(1, varint_len(blocks))?, items)
}
