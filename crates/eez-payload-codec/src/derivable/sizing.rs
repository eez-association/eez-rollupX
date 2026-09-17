//! Exact composable ordinary-range accounting. Each summary selects its own
//! first derived environment, so range queries naturally re-anchor encoding.

use alloy_primitives::Bytes;
use alloy_rlp::Encodable;

use super::{
    Environment, Error, add, body_len_from_items, checked_count, list_len, retained_bytes_len,
    sum_lengths,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct SizedRecord {
    position: usize,
    data_len: usize,
}

impl SizedRecord {
    fn shifted(&self, offset: usize) -> Result<Self, Error> {
        Ok(Self {
            position: add(self.position, offset)?,
            data_len: self.data_len,
        })
    }

    fn encoded_len(&self, gap: usize) -> Result<usize, Error> {
        // All assigned kinds have a one-byte canonical integer representation.
        list_len(add(add((gap as u64).length(), 1)?, self.data_len)?)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FirstDerived {
    position: usize,
    environment: Environment,
    transactions_list_len: usize,
    empty: bool,
}

/// Constant-size summary of a contiguous ordinary range, suitable for a range
/// index. Combining summaries adjusts the first gap AND the joining environment
/// transition, including inserting/removing the representation of an empty block.
/// It does not validate execution or certify that a full block is derivable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DerivablePayloadSizer {
    count: usize,
    record_items_len: usize,
    records: usize,
    transactions: usize,
    first_record: Option<SizedRecord>,
    last_record: Option<SizedRecord>,
    first_derived: Option<FirstDerived>,
    next_record_after_first_derived: Option<SizedRecord>,
    last_environment: Option<Environment>,
}

impl DerivablePayloadSizer {
    /// One profile-compatible ordinary block. Transaction bytes are sized
    /// exactly; their validity remains the encoder/executor's responsibility.
    pub fn derived_block(environment: Environment, transactions: &[Bytes]) -> Result<Self, Error> {
        environment.validate()?;
        check_transaction_count(transactions.len())?;
        let transactions_list_len = list_len(sum_lengths(transactions)?)?;
        let record = (!transactions.is_empty()).then_some(SizedRecord {
            position: 0,
            data_len: transactions_list_len,
        });
        Ok(Self {
            count: 1,
            record_items_len: record
                .as_ref()
                .map_or(Ok(0), |record| record.encoded_len(0))?,
            records: usize::from(record.is_some()),
            transactions: transactions.len(),
            first_record: record.clone(),
            last_record: record,
            first_derived: Some(FirstDerived {
                position: 0,
                environment: environment.clone(),
                transactions_list_len,
                empty: transactions.is_empty(),
            }),
            next_record_after_first_derived: None,
            last_environment: Some(environment),
        })
    }

    /// One full ordinary exception, with its actual transaction count. Full
    /// records do not install an environment for subsequent derived blocks.
    pub fn full_block(rlp: &[u8], transaction_count: usize) -> Result<Self, Error> {
        check_transaction_count(transaction_count)?;
        let record = SizedRecord {
            position: 0,
            data_len: rlp.length(),
        };
        Ok(Self {
            count: 1,
            record_items_len: record.encoded_len(0)?,
            records: 1,
            transactions: transaction_count,
            first_record: Some(record.clone()),
            last_record: Some(record),
            ..Self::default()
        })
    }

    /// Concatenate consecutive ranges, preserving their execution environments.
    /// The empty summary is the identity. All arithmetic and logical counts are
    /// checked before a caller can use the summary to admit more history.
    pub fn combine(&self, other: &Self) -> Result<Self, Error> {
        if self.count == 0 {
            return Ok(other.clone());
        }
        if other.count == 0 {
            return Ok(self.clone());
        }
        let count = add(self.count, other.count)?;
        checked_count(count as u64)?;
        let mut right = other.clone();
        if let (Some(left_environment), Some(first)) =
            (&self.last_environment, &right.first_derived)
            && *left_environment != first.environment
        {
            right.install_initial_change()?;
        }
        let mut record_items_len = add(self.record_items_len, right.record_items_len)?;
        if let Some(first) = &right.first_record {
            let trailing_empty = self
                .last_record
                .as_ref()
                .map_or(self.count, |last| self.count - last.position - 1);
            record_items_len = replace_len(
                record_items_len,
                first.encoded_len(first.position)?,
                first.encoded_len(add(trailing_empty, first.position)?)?,
            )?;
        }
        let shift = |record: &SizedRecord| record.shifted(self.count);
        let first_derived = if let Some(first) = &self.first_derived {
            Some(first.clone())
        } else {
            right.first_derived.clone().map(|mut first| {
                first.position += self.count;
                first
            })
        };
        let next_record_after_first_derived = if self.first_derived.is_some() {
            self.next_record_after_first_derived.clone().or(right
                .first_record
                .as_ref()
                .map(shift)
                .transpose()?)
        } else {
            right
                .next_record_after_first_derived
                .as_ref()
                .map(shift)
                .transpose()?
        };
        Ok(Self {
            count,
            record_items_len,
            records: add(self.records, right.records)?,
            transactions: add(self.transactions, right.transactions)?,
            first_record: self.first_record.clone().or(right
                .first_record
                .as_ref()
                .map(shift)
                .transpose()?),
            last_record: right
                .last_record
                .as_ref()
                .map(shift)
                .transpose()?
                .or(self.last_record.clone()),
            first_derived,
            next_record_after_first_derived,
            last_environment: right.last_environment.or(self.last_environment.clone()),
        })
    }

    /// Convert the right range's first derived block into kind 2 for a join.
    /// Before that first derived position every block is necessarily full, so
    /// the new record's internal gap is zero. Joining the ranges adjusts the
    /// outer first gap afterward. This transient form is never returned alone.
    fn install_initial_change(&mut self) -> Result<(), Error> {
        let first = self.first_derived.as_ref().ok_or(Error::Overflow)?;
        let change = SizedRecord {
            position: first.position,
            data_len: list_len(add(
                first.environment.length(),
                first.transactions_list_len,
            )?)?,
        };
        if first.empty {
            self.record_items_len = add(self.record_items_len, change.encoded_len(0)?)?;
            self.records = add(self.records, 1)?;
            if let Some(next) = &self.next_record_after_first_derived {
                let old_gap = next
                    .position
                    .checked_sub(first.position)
                    .ok_or(Error::Overflow)?;
                self.record_items_len = replace_len(
                    self.record_items_len,
                    next.encoded_len(old_gap)?,
                    next.encoded_len(old_gap.checked_sub(1).ok_or(Error::Overflow)?)?,
                )?;
            }
        } else {
            let old = SizedRecord {
                position: first.position,
                data_len: first.transactions_list_len,
            };
            self.record_items_len = replace_len(
                self.record_items_len,
                old.encoded_len(0)?,
                change.encoded_len(0)?,
            )?;
        }
        if self
            .first_record
            .as_ref()
            .is_none_or(|record| record.position >= first.position)
        {
            self.first_record = Some(change.clone());
        }
        if self
            .last_record
            .as_ref()
            .is_none_or(|record| record.position <= first.position)
        {
            self.last_record = Some(change);
        }
        Ok(())
    }

    /// Exact total operations bytes when this ordinary range precedes a retained
    /// full terminal. To promote a historical position to terminal, query the
    /// range ending BEFORE it and pass its full RLP here; do not subtract a prefix
    /// byte sum. The unused environment applies only to an all-full/empty range.
    pub fn encoded_len(
        &self,
        unused_environment: &Environment,
        terminal: &[u8],
        entries: &[Vec<u8>],
        groups: &[u16],
    ) -> Result<usize, Error> {
        let entry_items = entries
            .iter()
            .try_fold(0, |sum, entry| add(sum, retained_bytes_len(entry)?))?;
        self.encoded_len_from_items(
            unused_environment,
            retained_bytes_len(terminal)?,
            entry_items,
            sum_lengths(groups)?,
        )
    }

    /// Exact size using already measured unchanged terminal/metadata RLP item
    /// lengths. Each argument excludes the enclosing list header.
    pub fn encoded_len_from_items(
        &self,
        unused_environment: &Environment,
        terminal_item: usize,
        entry_items: usize,
        group_items: usize,
    ) -> Result<usize, Error> {
        let environment = self
            .first_derived
            .as_ref()
            .map_or(unused_environment, |first| &first.environment);
        environment.validate()?;
        let body = body_len_from_items(
            self.count,
            environment,
            self.record_items_len,
            terminal_item,
            entry_items,
            group_items,
        )?;
        add(1, list_len(body)?)
    }

    /// Ordinary positions, excluding the terminal passed to `encoded_len`.
    pub const fn ordinary_block_count(&self) -> usize {
        self.count
    }
    /// Explicit records after environment and gap composition.
    pub const fn record_count(&self) -> usize {
        self.records
    }
    /// Actual transaction activity represented by this ordinary range.
    pub const fn transaction_count(&self) -> usize {
        self.transactions
    }
    /// The natural initial environment selected from the first derived block.
    pub fn initial_environment(&self) -> Option<&Environment> {
        self.first_derived.as_ref().map(|first| &first.environment)
    }
}

fn replace_len(total: usize, old: usize, new: usize) -> Result<usize, Error> {
    add(total.checked_sub(old).ok_or(Error::Overflow)?, new)
}

fn check_transaction_count(count: usize) -> Result<(), Error> {
    if count > usize::from(u16::MAX) {
        return Err(Error::TransactionLimit(0));
    }
    Ok(())
}

/// Conservative admission reserve for a not-yet-built interval. `block_items`
/// bounds the sum of each position's full block item OR opaque transaction
/// items, including the future terminal. Use `BlockDaSizer::item_bytes` for future
/// blocks and `BlockDaSizer::retained_item_bytes` for known blocks. Each ordinary
/// position may carry transactions and an environment change, or a full exception.
/// Charge environment and framing at EVERY
/// position, plus maximum record-list framing, independent of actual gaps.
/// Use exact summaries for already measured candidate ranges.
pub fn interval_payload_bound(total_blocks: usize, block_items: usize) -> Result<usize, Error> {
    let ordinary = total_blocks.checked_sub(1).ok_or(Error::Overflow)?;
    checked_count(ordinary as u64)?;
    let environment = Environment {
        timestamp_step_seconds: u64::MAX,
        gas_limit: u64::MAX,
        extra_data: Bytes::from(vec![255; 32]),
        ..Environment::unused()
    };
    // u16 ordinary gaps, one-byte kind, and three possible nested list headers
    // (record, environment-change body, transactions). Nine is the maximum
    // canonical RLP prefix width for a u64-addressable payload.
    let record_overhead = add(environment.length(), 3 + 1 + 3 * 9)?;
    let records_overhead = ordinary
        .checked_mul(record_overhead)
        .ok_or(Error::Overflow)?;
    let body = add(
        add(
            add(
                add(add(1, (ordinary as u64).length())?, environment.length())?,
                block_items,
            )?,
            records_overhead,
        )?,
        9 + 1 + 1,
    )?;
    add(1, list_len(body)?)
}

#[cfg(test)]
mod tests;
