//! Canonical producer/follower evidence. Only the configured L1 source reads
//! the network; EVM consumers receive an immutable, bounded checked snapshot.
use std::sync::Arc;

use super::{DerivationError, DerivationSource, fetch};
use crate::L1BlockRef;
use alloy_primitives::{B256, Bytes};
use alloy_provider::Provider;
use eez_protocol::{
    l1_history::{HeaderChain, HeaderRules},
    l2::{ProfileError, Schedule},
};

/// Four days at five-second produced-block spacing fit with substantial margin.
/// This bounds retained producer evidence, not the permanent L2 mapping.
pub const MAX_HISTORY_HEADERS: usize = 131_072;
const FETCH_CONCURRENCY: u64 = 8;

/// A canonical source snapshot. Construction includes a final canonical check
/// after all reads and validation; callers still fence publication against reorgs.
#[derive(Debug, Clone)]
pub struct PreparedHistory {
    schedule: Schedule,
    chain: Arc<HeaderChain>,
    through: L1BlockRef,
}
impl PreparedHistory {
    /// Complete checked evidence, with no provider handle or lazy reads.
    pub fn chain(&self) -> Arc<HeaderChain> {
        Arc::clone(&self.chain)
    }
    /// Exact source boundary whose canonical hash was rechecked after preparation.
    pub const fn through(&self) -> L1BlockRef {
        self.through
    }
    /// Fixed genesis and schedule used to choose the oldest required source.
    pub const fn schedule(&self) -> Schedule {
        self.schedule
    }
}

impl<P: Provider + Clone + 'static> DerivationSource<P> {
    /// Fetch every produced header from the predecessor's selected source to
    /// `through`. Reuse only a linked, still-canonical prior snapshot. RPC work
    /// is bounded to eight owned tasks, cancelled when this future is dropped;
    /// decoding the complete bridge runs in one awaited blocking worker.
    pub async fn prepare_history(
        &self,
        first_number: u64,
        through: L1BlockRef,
        rules: HeaderRules,
        previous: Option<Arc<HeaderChain>>,
    ) -> Result<PreparedHistory, DerivationError> {
        if through.number == u64::MAX {
            return Err(DerivationError::Mismatch("reserved L1 endpoint number"));
        }
        let reference = self.schedule.config().l1_reference;
        if first_number < reference.number || first_number > through.number {
            return Err(DerivationError::Mismatch(
                "history predecessor outside anchor ancestry",
            ));
        }
        let first = fetch(&self.provider, reference.chain_id, first_number.into()).await?;
        let count = through
            .number
            .checked_sub(first.header().number)
            .and_then(|n| n.checked_add(1))
            .ok_or(DerivationError::Mismatch("invalid canonical history range"))?;
        if count > MAX_HISTORY_HEADERS as u64 {
            return Err(DerivationError::Unavailable(
                "canonical ancestry bridge exceeds supported history bound",
            ));
        }
        let mut retained = None;
        if let Some(previous) = previous {
            let last = previous.endpoint()?;
            let boundary = L1BlockRef {
                number: last.header().number.min(through.number),
                hash: previous
                    .at(last.header().number.min(through.number))?
                    .hash(),
            };
            if previous
                .at(first.header().number)
                .is_ok_and(|header| header.hash() == first.reference().hash)
                && boundary.number >= first.header().number
            {
                match self.canonical_header(boundary).await {
                    Ok(_) => {
                        let oldest = previous
                            .headers()
                            .first()
                            .expect("endpoint checked nonempty")
                            .header()
                            .number;
                        let start = if through
                            .number
                            .checked_sub(oldest)
                            .and_then(|n| n.checked_add(1))
                            .is_some_and(|count| count <= MAX_HISTORY_HEADERS as u64)
                        {
                            oldest
                        } else {
                            first.header().number
                        };
                        retained = Some((previous, start, boundary.number));
                    }
                    Err(DerivationError::Mismatch(_)) => {} // Reorg: construct a fresh linked branch.
                    Err(error) => return Err(error),
                }
            }
        }
        let mut next = retained
            .as_ref()
            .map_or(first.header().number, |(_, _, last)| last.saturating_add(1));
        let mut segment = Vec::<Bytes>::new();
        while next <= through.number {
            let end = next
                .saturating_add(FETCH_CONCURRENCY - 1)
                .min(through.number);
            let mut reads = tokio::task::JoinSet::new();
            for number in next..=end {
                let provider = self.provider.clone();
                reads.spawn(async move {
                    let header = fetch(&provider, reference.chain_id, number.into()).await?;
                    Ok::<_, DerivationError>((
                        number,
                        Bytes::from(alloy_rlp::encode(header.header())),
                    ))
                });
            }
            let mut chunk = Vec::with_capacity(FETCH_CONCURRENCY as usize);
            while let Some(result) = reads.join_next().await {
                chunk.push(result.map_err(|_| {
                    DerivationError::Unavailable("canonical header read worker failed")
                })??);
            }
            chunk.sort_unstable_by_key(|(number, _)| *number);
            segment.extend(chunk.into_iter().map(|(_, raw)| raw));
            let Some(after) = end.checked_add(1) else {
                break;
            };
            next = after;
        }
        let start = first.header().number;
        let expected = first.reference().hash;
        let chain = tokio::task::spawn_blocking(move || {
            let mut chain = match retained {
                Some((chain, retained_start, last)) => chain.range(retained_start, last)?,
                None => HeaderChain::default(),
            };
            chain.append(&segment, &rules, MAX_HISTORY_HEADERS)?;
            if chain.at(start)?.hash() != expected
                || chain.endpoint()?.hash() != through.hash
                || chain.endpoint()?.header().number != through.number
            {
                return Err(ProfileError(
                    "canonical history boundary or source selection mismatch",
                ));
            }
            Ok::<_, ProfileError>(Arc::new(chain))
        })
        .await
        .map_err(|_| DerivationError::Unavailable("canonical header validation worker failed"))??;
        self.canonical_header(through).await?;
        Ok(PreparedHistory {
            schedule: self.schedule,
            chain,
            through,
        })
    }
}

/// In-memory evidence owner, initialized before the engine exposes readiness.
/// Settlement never changes recorded origin selections. Producers may select
/// eligible advances from this snapshot; executors validate the recorded input. Old snapshots can remain borrowed by in-flight execution.
#[derive(Debug, Clone)]
pub struct CanonicalHistory {
    schedule: Schedule,
    prepared: Option<PreparedHistory>,
}
impl CanonicalHistory {
    /// Empty evidence fails closed until the deriver publishes a checked range.
    pub fn new(schedule: Schedule) -> Self {
        Self {
            schedule,
            prepared: None,
        }
    }
    /// Fixed network schedule, independent of settlement occurrences.
    pub const fn schedule(&self) -> Schedule {
        self.schedule
    }
    /// Replace only evidence prepared for this exact network.
    pub fn publish(&mut self, prepared: PreparedHistory) -> Result<(), DerivationError> {
        if prepared.schedule != self.schedule {
            return Err(DerivationError::Mismatch(
                "history belongs to another genesis/profile",
            ));
        }
        // Replaying an older accepted batch can request an earlier anchor.
        // Keep already checked descendants when this range is their exact
        // prefix; discarding them would make local production depend on a new
        // RPC read after an otherwise harmless historical replay.
        if let Some(ready) = &self.prepared
            && prepared.chain.headers().first().is_some_and(|first| {
                ready
                    .chain
                    .at(first.header().number)
                    .is_ok_and(|known| known == first)
            })
            && ready
                .chain
                .at(prepared.through.number)
                .is_ok_and(|known| known.hash() == prepared.through.hash)
        {
            return Ok(());
        }
        self.prepared = Some(prepared);
        Ok(())
    }

    /// Current snapshot for bounded incremental acquisition.
    pub fn prepared(&self) -> Option<PreparedHistory> {
        self.prepared.clone()
    }
    /// Ready evidence only. An unresolved cutoff permits a recorded repeat;
    /// execution never waits for proof that no newer L1 block was produced.
    pub fn headers(&self, cutoff: u64) -> Result<Arc<HeaderChain>, ProfileError> {
        let prepared = self
            .prepared
            .as_ref()
            .ok_or(ProfileError("L1 history is not prepared"))?;
        prepared.chain.select(cutoff)?;
        Ok(prepared.chain())
    }
    /// Next grid timestamp with inherited randomness. An explicit history
    /// transaction may replace that randomness during block preparation.
    pub fn next_block_context(
        &self,
        parent: &alloy_consensus::Header,
    ) -> Result<(u64, B256), DerivationError> {
        let (_, timestamp) = self.schedule.next(parent)?;
        Ok((timestamp, parent.mix_hash))
    }
    /// A known reorg invalidates the old branch. Recover authenticated evidence
    /// before executing on a possibly orphaned origin; uncertainty is not authority.
    pub(crate) fn invalidate(&mut self) {
        self.prepared = None;
    }
}

#[cfg(test)]
mod tests;
