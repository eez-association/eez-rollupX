//! RollupX V0 operations: compact ordinary counts and an unchanged full
//! terminal block. This module checks wire structure, not execution validity.
//!
//! In particular, a decoded payload is NOT an authenticated or signable block
//! window. Callers must bind its anchor, execute every block, verify all ordinary
//! and terminal settlement gates, and authenticate the actual blob commitments.

use alloy_eips::{Decodable2718, Encodable2718};
use alloy_primitives::{Address, B256, Bytes};
use alloy_rlp::{Decodable, Encodable, Header};
use reth_ethereum_primitives::{Block, TransactionSigned};
use std::iter::FusedIterator;

mod columns;
mod counts;
mod sizing;
pub use sizing::{DerivablePayloadSizer, interval_payload_bound};

/// Inner operations tag; does not change the outer blob message version.
pub const TAG: u8 = 0;
/// Maximum transactions in a block, including full exceptions and terminal.
pub const MAX_TRANSACTIONS: usize = counts::MAX_TRANSACTIONS;
/// Execution-input profile described by this version.
pub const PROFILE: u8 = 1;
/// Maximum logical blocks, including the retained terminal.
pub const MAX_BLOCKS: usize = 65_536;

/// Published execution inputs. Fork-dependent optional-field presence and
/// execution validity are enforced by the consumer, not by this wire codec.
#[derive(Clone, Debug, PartialEq, Eq, alloy_rlp::RlpEncodable, alloy_rlp::RlpDecodable)]
pub struct Environment {
    /// Seconds to advance from the actual preceding block timestamp.
    pub timestamp_step_seconds: u64,
    /// EVM fee recipient.
    pub fee_recipient: Address,
    /// EVM randomness input.
    pub prev_randao: B256,
    /// Block gas limit.
    pub gas_limit: u64,
    /// Exact extra data (at most 32 bytes for this profile).
    pub extra_data: Bytes,
    /// Parent beacon root to use when the active fork requires one.
    pub cancun_parent_beacon_root: B256,
}

impl Environment {
    /// Canonical placeholder when a range has no derived ordinary positions.
    /// No block consumes these inputs. It must never replace missing inputs for
    /// an actual derived position.
    pub const fn unused() -> Self {
        Self {
            timestamp_step_seconds: 1,
            fee_recipient: Address::ZERO,
            prev_randao: B256::ZERO,
            gas_limit: 1,
            extra_data: Bytes::new(),
            cancun_parent_beacon_root: B256::ZERO,
        }
    }

    /// Check the profile's context-free input bounds.
    pub fn validate(&self) -> Result<(), Error> {
        if self.timestamp_step_seconds == 0 || self.gas_limit == 0 || self.extra_data.len() > 32 {
            return Err(Error::InvalidEnvironment);
        }
        Ok(())
    }
}

/// One explicitly represented ordinary block. Gaps before it are empty derived
/// blocks under the previously active environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Number of implicit empty blocks before this record.
    pub empty_gap: u64,
    /// The data for the record's single ordinary position.
    pub data: RecordData,
}

/// Context-free ordinary block representation. Signature authentication and the
/// prohibition on intermediate system transactions remain consumer obligations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordData {
    /// At least one exact signed EIP-2718 transaction. On wire these live in
    /// the transaction column, without an override record.
    Transactions(Vec<Bytes>),
    /// Kind 1: exact canonical block RLP. Does not change the active environment.
    FullBlock(Bytes),
    /// Install an environment for THIS block and following derived blocks.
    /// The wire uses a kind-2 override and the shared transaction column.
    /// Transactions may be empty, but the environment must change.
    EnvironmentChange {
        /// New published environment.
        environment: Environment,
        /// Exact signed EIP-2718 transaction bytes.
        transactions: Vec<Bytes>,
    },
}

/// Structurally validated RollupX V0 payload. Fields are private so subsequent mutation
/// cannot invalidate the bounds used by its block-plan iterator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Payload {
    ordinary_block_count: usize,
    initial_environment: Environment,
    records: Vec<Record>,
    counts: Bytes,
    terminal_block: Vec<u8>,
    terminal_number: u64,
    l2_entries: Vec<Vec<u8>>,
    outbound_group_sizes: Vec<u16>,
}

/// Structural rejection. None of these errors may be interpreted as an empty
/// block, another payload format or permission to skip an execution check.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Missing or different version.
    #[error("expected RollupX operations tag 0x00")]
    WrongTag,
    /// Unknown execution-input profile.
    #[error("unsupported derivation profile {0}")]
    Profile(u8),
    /// Reject before decoding variable-length fields or expanding a block plan.
    #[error("ordinary count {0} exceeds 65535")]
    BlockLimit(u64),
    /// Malformed canonical RLP.
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),
    /// Arithmetic bound violation.
    #[error("operations length or position overflow")]
    Overflow,
    /// Noncanonical, truncated, or incorrectly covering count column.
    #[error("invalid transaction-count column")]
    CountEncoding,
    /// A full exception disagrees with its count token.
    #[error("full block transaction count disagrees with count column")]
    CountMismatch,
    /// Record extends beyond the ordinary range.
    #[error("ordinary record {0} exceeds the declared ordinary range")]
    RecordRange(usize),
    /// Zero step/gas or oversized extra data.
    #[error("invalid derivation environment")]
    InvalidEnvironment,
    /// Redundant environment changes are not canonical.
    #[error("ordinary record {0} has a no-op environment change")]
    UnchangedEnvironment(usize),
    /// An empty kind-0 record must have been represented by a gap instead.
    #[error("ordinary record {0} has an empty kind-0 transaction list")]
    EmptyTransactions(usize),
    /// Unsupported record type.
    #[error("unknown ordinary record kind {0}")]
    RecordKind(u8),
    /// Transaction count bound, including full records and terminal.
    #[error("more than 16383 transactions in block at position {0}")]
    TransactionLimit(usize),
    /// Exact transaction envelope decoding failed.
    #[error("invalid transaction at ordinary position {position}, index {transaction}")]
    InvalidTransaction {
        /// Ordinary position.
        position: usize,
        /// Transaction index.
        transaction: usize,
    },
    /// Not exact canonical Ethereum block RLP.
    #[error("invalid full block at position {0}")]
    InvalidBlock(usize),
    /// Number or immediately checkable parent-hash inconsistency.
    #[error("inconsistent full block at position {0}")]
    BlockPlacement(usize),
}

impl Payload {
    /// Construct a payload, checking syntax and bounded positions. This does
    /// not execute transactions, authenticate their senders, or authorize effects.
    pub fn new(
        ordinary_block_count: u64,
        initial_environment: Environment,
        records: Vec<Record>,
        terminal_block: Vec<u8>,
        l2_entries: Vec<Vec<u8>>,
        outbound_group_sizes: Vec<u16>,
    ) -> Result<Self, Error> {
        let count = checked_count(ordinary_block_count)?;
        initial_environment.validate()?;
        let terminal = decode_block(&terminal_block, count)?;
        let first_number = terminal
            .header
            .number
            .checked_sub(ordinary_block_count)
            .filter(|n| *n > 0)
            .ok_or(Error::BlockPlacement(count))?;
        let mut position = 0usize;
        let mut active = &initial_environment;
        let mut previous_full: Option<(usize, B256)> = None;
        let mut encoded_counts = Vec::new();
        let mut empty_run = 0u32;
        for (index, record) in records.iter().enumerate() {
            position = record_position(position, record.empty_gap, count, index)?;
            empty_run += record.empty_gap as u32; // range-checked above
            let transactions = match &record.data {
                RecordData::Transactions(transactions) => {
                    if transactions.is_empty() {
                        return Err(Error::EmptyTransactions(index));
                    }
                    validate_transactions(transactions, position)?;
                    transactions.len()
                }
                RecordData::FullBlock(bytes) => {
                    let block = decode_block(bytes, position)?;
                    if block.header.number != first_number + position as u64 {
                        return Err(Error::BlockPlacement(position));
                    }
                    check_full_parent(previous_full, position, &block)?;
                    previous_full = Some((position, block.header.hash_slow()));
                    block.body.transactions.len()
                }
                RecordData::EnvironmentChange {
                    environment,
                    transactions,
                } => {
                    environment.validate()?;
                    if environment == active {
                        return Err(Error::UnchangedEnvironment(index));
                    }
                    validate_transactions(transactions, position)?;
                    active = environment;
                    transactions.len()
                }
            };
            if transactions == 0 {
                empty_run += 1;
            } else {
                if empty_run > 0 {
                    counts::Token::Empty(empty_run).encode(&mut encoded_counts);
                    empty_run = 0;
                }
                counts::Token::Transactions(transactions).encode(&mut encoded_counts);
            }
            position += 1; // bounded above by count <= 65535
        }
        check_full_parent(previous_full, count, &terminal)?;
        empty_run += (count - position) as u32;
        if empty_run > 0 {
            counts::Token::Empty(empty_run).encode(&mut encoded_counts);
        }
        let payload = Self {
            ordinary_block_count: count,
            initial_environment,
            counts: encoded_counts.into(),
            records,
            terminal_number: terminal.header.number,
            terminal_block,
            l2_entries,
            outbound_group_sizes,
        };
        payload.encoded_len()?;
        Ok(payload)
    }

    /// Decode the bounded sparse shape. The ordinary count is checked before
    /// parsing variable-length fields; record coverage precedes record data.
    pub fn decode(input: &[u8]) -> Result<Self, Error> {
        let Some((&TAG, mut remaining)) = input.split_first() else {
            return Err(Error::WrongTag);
        };
        let mut body = Header::decode_bytes(&mut remaining, true)?;
        end(remaining)?;
        let profile = u8::decode(&mut body)?;
        if profile != PROFILE {
            return Err(Error::Profile(profile));
        }
        let count = u64::decode(&mut body)?;
        let bounded_count = checked_count(count)?;
        let environment = Environment::decode(&mut body)?;
        environment.validate()?;
        let (records, encoded_counts) = columns::decode(&mut body, bounded_count)?;
        let terminal_block = Header::decode_bytes(&mut body, false)?.to_vec();
        let mut entries = Header::decode_bytes(&mut body, true)?;
        let mut l2_entries = Vec::new();
        while !entries.is_empty() {
            l2_entries.push(Header::decode_bytes(&mut entries, false)?.to_vec());
        }
        let groups = Vec::<u16>::decode(&mut body)?;
        end(body)?;
        let payload = Self::new(
            count,
            environment,
            records,
            terminal_block,
            l2_entries,
            groups,
        )?;
        // Derived counts determine exactly how many transactions the column
        // parser consumes. Full-block counts are bound here to their validated
        // bodies, without decoding those bodies a second time.
        if payload.counts.as_ref() != encoded_counts {
            return Err(Error::CountMismatch);
        }
        Ok(payload)
    }

    /// Encode this validated shape without altering the retained fields.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let column_lengths = columns::lengths(&self.records)?;
        let entry_items = self.entry_items_len()?;
        let body_len = self.body_len(column_lengths, entry_items)?;
        let mut out = Vec::with_capacity(add(1, list_len(body_len)?)?);
        out.push(TAG);
        list_header(body_len, &mut out);
        PROFILE.encode(&mut out);
        (self.ordinary_block_count as u64).encode(&mut out);
        self.initial_environment.encode(&mut out);
        columns::encode(&self.counts, &self.records, column_lengths, &mut out)?;
        self.terminal_block.as_slice().encode(&mut out);
        list_header(entry_items, &mut out);
        for entry in &self.l2_entries {
            entry.as_slice().encode(&mut out);
        }
        self.outbound_group_sizes.encode(&mut out);
        Ok(out)
    }

    /// Exact meaningful operations bytes, including tag and RLP framing.
    pub fn encoded_len(&self) -> Result<usize, Error> {
        add(
            1,
            list_len(self.body_len(columns::lengths(&self.records)?, self.entry_items_len()?)?)?,
        )
    }

    fn body_len(&self, column_lengths: (usize, usize), entry_items: usize) -> Result<usize, Error> {
        body_len_from_items(
            self.ordinary_block_count,
            &self.initial_environment,
            columns::body_len(&self.counts, column_lengths)?,
            self.terminal_block.as_slice().length(),
            entry_items,
            sum_lengths(&self.outbound_group_sizes)?,
        )
    }

    fn entry_items_len(&self) -> Result<usize, Error> {
        self.l2_entries
            .iter()
            .try_fold(0, |sum, entry| add(sum, entry.as_slice().length()))
    }

    /// Total logical count, including exactly one full terminal.
    pub const fn block_count(&self) -> usize {
        self.ordinary_block_count + 1
    }

    /// Explicitly declared ordinary block count.
    pub const fn ordinary_block_count(&self) -> usize {
        self.ordinary_block_count
    }

    /// First declared block number, obtained from the full terminal and count.
    /// It must still be bound to the authenticated external anchor by consumers.
    pub const fn first_block_number(&self) -> u64 {
        self.terminal_number - self.ordinary_block_count as u64
    }

    /// Initial published environment (subsequent kind-2 records can replace it).
    pub const fn initial_environment(&self) -> &Environment {
        &self.initial_environment
    }

    /// Borrow sparse records without expanding empty history.
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Original canonical terminal block bytes.
    pub fn terminal_block(&self) -> &[u8] {
        &self.terminal_block
    }

    /// Retained cross-chain entry bytes in their original order.
    pub fn l2_entries(&self) -> &[Vec<u8>] {
        &self.l2_entries
    }

    /// Retained outbound group sizes.
    pub fn outbound_group_sizes(&self) -> &[u16] {
        &self.outbound_group_sizes
    }

    /// Iterate the bounded logical positions without allocating per-empty-block
    /// records. Every derived position must still be executed by the consumer.
    pub fn blocks(&self) -> Blocks<'_> {
        Blocks {
            payload: self,
            position: 0,
            record_index: 0,
            next_record: self.records.first().map(|r| r.empty_gap as usize),
            environment: &self.initial_environment,
        }
    }
}

fn body_len_from_items(
    count: usize,
    environment: &Environment,
    record_items: usize,
    terminal_item: usize,
    entry_items: usize,
    group_items: usize,
) -> Result<usize, Error> {
    let mut length = add(PROFILE.length(), (count as u64).length())?;
    length = add(length, environment.length())?;
    length = add(length, list_len(record_items)?)?;
    length = add(length, terminal_item)?;
    length = add(length, list_len(entry_items)?)?;
    add(length, list_len(group_items)?)
}

/// A structurally decoded position, not an authenticated execution result.
#[derive(Debug, PartialEq, Eq)]
pub enum BlockPlan<'a> {
    /// Execute the given transactions (or an implicit empty list) under inputs.
    Derived {
        /// Zero-based position in the original claimed window.
        position: usize,
        /// Environment active for this position.
        environment: &'a Environment,
        /// Exact EIP-2718 signed transaction bytes.
        transactions: &'a [Bytes],
    },
    /// Execute and verify the retained complete block.
    Full {
        /// Zero-based position in the original claimed window.
        position: usize,
        /// Exact canonical block RLP.
        rlp: &'a [u8],
        /// True only for the last, retained terminal position.
        terminal: bool,
    },
}

/// Allocation-free iterator over at most [`MAX_BLOCKS`] logical positions.
#[derive(Debug)]
pub struct Blocks<'a> {
    payload: &'a Payload,
    position: usize,
    record_index: usize,
    next_record: Option<usize>,
    environment: &'a Environment,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = BlockPlan<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let position = self.position;
        if position >= self.payload.block_count() {
            return None;
        }
        self.position += 1;
        if position == self.payload.ordinary_block_count {
            return Some(BlockPlan::Full {
                position,
                rlp: &self.payload.terminal_block,
                terminal: true,
            });
        }
        let mut txs = [].as_slice();
        if self.next_record == Some(position) {
            let record = &self.payload.records[self.record_index];
            self.record_index += 1;
            self.next_record = self
                .payload
                .records
                .get(self.record_index)
                .map(|r| self.position + r.empty_gap as usize);
            match &record.data {
                RecordData::Transactions(transactions) => txs = transactions,
                RecordData::FullBlock(bytes) => {
                    return Some(BlockPlan::Full {
                        position,
                        rlp: bytes,
                        terminal: false,
                    });
                }
                RecordData::EnvironmentChange {
                    environment,
                    transactions,
                } => {
                    self.environment = environment;
                    txs = transactions;
                }
            }
        }
        Some(BlockPlan::Derived {
            position,
            environment: self.environment,
            transactions: txs,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.payload.block_count() - self.position;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for Blocks<'_> {}
impl FusedIterator for Blocks<'_> {}

fn checked_count(count: u64) -> Result<usize, Error> {
    if count >= MAX_BLOCKS as u64 {
        return Err(Error::BlockLimit(count));
    }
    Ok(count as usize)
}

fn record_position(next: usize, gap: u64, count: usize, index: usize) -> Result<usize, Error> {
    let gap = usize::try_from(gap).map_err(|_| Error::RecordRange(index))?;
    next.checked_add(gap)
        .filter(|p| *p < count)
        .ok_or(Error::RecordRange(index))
}

fn end(bytes: &[u8]) -> Result<(), Error> {
    if bytes.is_empty() {
        Ok(())
    } else {
        Err(alloy_rlp::Error::UnexpectedLength.into())
    }
}

fn validate_transactions(transactions: &[Bytes], position: usize) -> Result<(), Error> {
    if transactions.len() > MAX_TRANSACTIONS {
        return Err(Error::TransactionLimit(position));
    }
    for (index, bytes) in transactions.iter().enumerate() {
        let invalid = || Error::InvalidTransaction {
            position,
            transaction: index,
        };
        let mut input = bytes.as_ref();
        let transaction = TransactionSigned::decode_2718(&mut input).map_err(|_| invalid())?;
        if !input.is_empty() || transaction.encoded_2718().as_slice() != bytes.as_ref() {
            return Err(invalid());
        }
    }
    Ok(())
}

fn decode_block(bytes: &[u8], position: usize) -> Result<Block, Error> {
    let block: Block = alloy_rlp::decode_exact(bytes).map_err(|_| Error::InvalidBlock(position))?;
    if alloy_rlp::encode(&block) != bytes {
        return Err(Error::InvalidBlock(position));
    }
    if block.body.transactions.len() > MAX_TRANSACTIONS {
        return Err(Error::TransactionLimit(position));
    }
    Ok(block)
}

fn check_full_parent(
    previous: Option<(usize, B256)>,
    position: usize,
    block: &Block,
) -> Result<(), Error> {
    if let Some((previous_position, previous_hash)) = previous
        && previous_position + 1 == position
        && block.header.parent_hash != previous_hash
    {
        return Err(Error::BlockPlacement(position));
    }
    Ok(())
}

fn add(a: usize, b: usize) -> Result<usize, Error> {
    a.checked_add(b).ok_or(Error::Overflow)
}

fn sum_lengths<T: Encodable>(items: &[T]) -> Result<usize, Error> {
    items
        .iter()
        .try_fold(0, |sum, item| add(sum, item.length()))
}

fn list_len(payload_length: usize) -> Result<usize, Error> {
    add(
        payload_length,
        Header {
            list: true,
            payload_length,
        }
        .length(),
    )
}

fn list_header(payload_length: usize, out: &mut Vec<u8>) {
    Header {
        list: true,
        payload_length,
    }
    .encode(out);
}

#[cfg(test)]
mod tests;
