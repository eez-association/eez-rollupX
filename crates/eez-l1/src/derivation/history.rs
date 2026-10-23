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
        parent_timestamp: u64,
        through: L1BlockRef,
        rules: HeaderRules,
        previous: Option<Arc<HeaderChain>>,
    ) -> Result<PreparedHistory, DerivationError> {
        if through.number == u64::MAX {
            return Err(DerivationError::Mismatch("reserved L1 endpoint number"));
        }
        let endpoint = self.canonical_header(through).await?;
        let cutoff = self.schedule.history()?.source_cutoff(parent_timestamp)?;
        if endpoint.header().timestamp < cutoff {
            return Err(DerivationError::Unavailable(
                "canonical L1 has not resolved the source cutoff",
            ));
        }
        let reference = self.schedule.config().l1_reference;
        let (mut low, mut high) = (reference.number, through.number);
        let mut selected = None;
        while low <= high {
            let middle = low + (high - low) / 2;
            let header = fetch(&self.provider, reference.chain_id, middle.into()).await?;
            if header.header().timestamp <= cutoff {
                selected = Some(header);
                let Some(next) = middle.checked_add(1) else {
                    break;
                };
                low = next;
            } else {
                let Some(previous) = middle.checked_sub(1) else {
                    break;
                };
                high = previous;
            }
        }
        let first = selected.ok_or(DerivationError::Mismatch(
            "missing source at predecessor cutoff",
        ))?;
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
                || chain.select(cutoff)?.header().number != start
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
/// Settlement never changes selection; only canonical L1 headers and fixed
/// timestamps do. Old snapshots can remain borrowed by in-flight execution.
#[derive(Debug, Clone)]
pub struct CanonicalHistory {
    schedule: Schedule,
    prepared: Option<PreparedHistory>,
    /// Conditional execution for an owned, bounded replacement on this exact
    /// unchanged parent. Settlement still has to establish that it was the
    /// actual inclusion parent; this is never canonical-progress evidence.
    replacement: Option<(L1BlockRef, u64)>,
}
impl CanonicalHistory {
    /// Empty evidence fails closed until the deriver publishes a checked range.
    pub fn new(schedule: Schedule) -> Self {
        Self {
            schedule,
            prepared: None,
            replacement: None,
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
        if self
            .replacement
            .is_some_and(|(parent, _)| parent != prepared.through)
        {
            self.replacement = None;
        }
        self.prepared = Some(prepared);
        Ok(())
    }

    pub(crate) fn authorize_replacement(
        &mut self,
        parent: L1BlockRef,
        target_timestamp: u64,
    ) -> Result<(), DerivationError> {
        let prepared = self
            .prepared
            .as_ref()
            .ok_or(DerivationError::Unavailable("L1 history is not prepared"))?;
        let endpoint = prepared.chain.endpoint()?;
        let offset = crate::submitter::policy_target_slot_offset(
            endpoint.header().timestamp,
            target_timestamp,
            self.schedule.history()?.interval(),
        );
        if parent != prepared.through
            || offset.is_none_or(|offset| offset < 2)
            || self
                .replacement
                .is_some_and(|(_, previous)| target_timestamp < previous)
        {
            return Err(DerivationError::Mismatch(
                "invalid replacement history target",
            ));
        }
        self.replacement = Some((parent, target_timestamp));
        Ok(())
    }
    /// Current snapshot for bounded incremental acquisition.
    pub fn prepared(&self) -> Option<PreparedHistory> {
        self.prepared.clone()
    }
    /// Ready evidence for execution. Unresolved cutoffs wait, except while an
    /// owned replacement is conditionally bound to this exact inclusion parent
    /// and a later target timestamp. New canonical evidence revokes that bound.
    pub fn headers(&self, cutoff: u64) -> Result<Arc<HeaderChain>, ProfileError> {
        let prepared = self
            .prepared
            .as_ref()
            .ok_or(ProfileError("L1 history is not prepared"))?;
        if prepared.chain.endpoint()?.header().timestamp < cutoff
            && !self
                .replacement
                .is_some_and(|(parent, target)| parent == prepared.through && cutoff < target)
        {
            return Err(ProfileError("canonical L1 has not resolved source cutoff"));
        }
        prepared.chain.select(cutoff)?;
        Ok(prepared.chain())
    }
    /// Select one block's randomness; no accepted-live-anchor seed exists.
    pub fn prev_randao(&self, number: u64) -> Result<B256, DerivationError> {
        let timestamp = self.schedule.timestamp(number)?;
        let cutoff = self.schedule.history()?.source_cutoff(timestamp)?;
        Ok(self.headers(cutoff)?.select(cutoff)?.header().mix_hash)
    }
    /// Derive exact next-block time and randomness together.
    pub fn next_block_context(
        &self,
        parent: &alloy_consensus::Header,
    ) -> Result<(u64, B256), DerivationError> {
        let (number, timestamp) = self.schedule.next(parent)?;
        Ok((timestamp, self.prev_randao(number)?))
    }
    /// Drop stale evidence immediately on reorg. A subsequent prepared range
    /// must be published before any execution can continue.
    pub(crate) fn invalidate(&mut self) {
        self.prepared = None;
        self.replacement = None;
    }

    pub(crate) fn invalidate_replacement(&mut self) {
        self.replacement = None;
    }
}

#[cfg(test)]
mod tests;
