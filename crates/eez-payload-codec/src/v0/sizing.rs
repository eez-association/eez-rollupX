use super::{BlockRun, Error, MAX_TRANSACTIONS, add, counts, varint_len};
use alloy_primitives::Bytes;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Edge {
    blocks: u32,
    value: Bytes,
}

/// The first/last runs suffice to merge a column exactly. Interior runs never
/// change at a concatenation boundary. The whole-range case extends both edges.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MetadataSummary {
    blocks: u32,
    bytes: usize,
    first: Edge,
    last: Edge,
}

impl MetadataSummary {
    fn run(blocks: u32, value: &[u8], variable: bool) -> Self {
        let edge = Edge {
            blocks,
            value: Bytes::copy_from_slice(value),
        };
        Self {
            blocks,
            bytes: varint_len(blocks) + value.len() + usize::from(variable),
            first: edge.clone(),
            last: edge,
        }
    }
    fn combine(&self, right: &Self, variable: bool) -> Result<Self, Error> {
        let blocks = self
            .blocks
            .checked_add(right.blocks)
            .ok_or(Error::Overflow)?;
        let mut bytes = add(self.bytes, right.bytes)?;
        let mut first = self.first.clone();
        let mut last = right.last.clone();
        if self.last.value == right.first.value {
            let joined = self
                .last
                .blocks
                .checked_add(right.first.blocks)
                .ok_or(Error::Overflow)?;
            bytes -= varint_len(self.last.blocks)
                + varint_len(right.first.blocks)
                + self.last.value.len()
                + usize::from(variable);
            bytes = add(bytes, varint_len(joined))?;
            if self.first.blocks == self.blocks {
                first.blocks = joined;
            }
            if right.last.blocks == right.blocks {
                last.blocks = joined;
            }
        }
        Ok(Self {
            blocks,
            bytes,
            first,
            last,
        })
    }
}

/// Constant-size exact operations summary for concatenating producer ranges.
/// Count and metadata runs merge independently across the join, including when
/// a metadata transition lies inside one maximal empty-count run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PayloadSizer {
    blocks: u32,
    transactions: usize,
    lengths_and_data: usize,
    counts: counts::Summary,
    beneficiary: Option<MetadataSummary>,
    extra_data: Option<MetadataSummary>,
}

impl PayloadSizer {
    /// Size exact transaction bytes without treating syntax as authorization.
    /// Nonempty runs describe a single block; the per-block count is bounded.
    pub fn run(run: &BlockRun<'_>) -> Result<Self, Error> {
        if run.blocks == 0 {
            return Err(Error::EmptyRange);
        }
        if run.extra_data.len() > 32 {
            return Err(Error::ExtraData);
        }
        if run.transactions.len() > MAX_TRANSACTIONS
            || (!run.transactions.is_empty() && run.blocks != 1)
        {
            return Err(Error::TransactionCount);
        }
        let counts = if run.transactions.is_empty() {
            counts::Summary::empty(run.blocks)
        } else {
            counts::Summary::block(run.transactions.len()).map_err(|_| Error::TransactionCount)?
        };
        let lengths_and_data = run.transactions.iter().try_fold(0, |sum, tx| {
            let length = u32::try_from(tx.len()).map_err(|_| Error::TransactionLength)?;
            if length == 0 {
                return Err(Error::TransactionLength);
            }
            add(sum, add(tx.len(), varint_len(length))?)
        })?;
        Ok(Self {
            blocks: run.blocks,
            transactions: run.transactions.len(),
            lengths_and_data,
            counts,
            beneficiary: Some(MetadataSummary::run(
                run.blocks,
                run.beneficiary.as_slice(),
                false,
            )),
            extra_data: Some(MetadataSummary::run(run.blocks, run.extra_data, true)),
        })
    }

    pub fn combine(&self, right: &Self) -> Result<Self, Error> {
        if self.blocks == 0 {
            return Ok(right.clone());
        }
        if right.blocks == 0 {
            return Ok(self.clone());
        }
        // Nonempty summaries are only constructed with both metadata columns.
        let metadata =
            |left: &Option<MetadataSummary>, right: &Option<MetadataSummary>, variable| {
                left.as_ref()
                    .expect("nonempty summary metadata")
                    .combine(right.as_ref().expect("nonempty summary metadata"), variable)
                    .map(Some)
            };
        Ok(Self {
            blocks: self
                .blocks
                .checked_add(right.blocks)
                .ok_or(Error::Overflow)?,
            transactions: add(self.transactions, right.transactions)?,
            lengths_and_data: add(self.lengths_and_data, right.lengths_and_data)?,
            counts: self
                .counts
                .combine(&right.counts)
                .map_err(|_| Error::Overflow)?,
            beneficiary: metadata(&self.beneficiary, &right.beneficiary, false)?,
            extra_data: metadata(&self.extra_data, &right.extra_data, true)?,
        })
    }

    pub const fn block_count(&self) -> u32 {
        self.blocks
    }
    pub const fn transaction_count(&self) -> usize {
        self.transactions
    }
    pub fn encoded_len(&self) -> Result<usize, Error> {
        if self.blocks == 0 {
            return Err(Error::EmptyRange);
        }
        [
            varint_len(self.blocks),
            self.counts.encoded_len(),
            self.lengths_and_data,
            self.beneficiary
                .as_ref()
                .expect("nonempty summary metadata")
                .bytes,
            self.extra_data
                .as_ref()
                .expect("nonempty summary metadata")
                .bytes,
        ]
        .into_iter()
        .try_fold(1, add)
    }
}
