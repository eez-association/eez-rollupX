//! L1 authentication and accepted-anchor seed history for L2 derivation.
//!
//! The provider is an operator-selected canonical L1 source. Composer-supplied
//! headers, receipts or claimed finality are never inputs to authentication.
//! Acceptance still requires reconciled EEZ effects; reading a header alone
//! cannot advance this history.

pub mod proof;

use alloy_consensus::Header;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use alloy_provider::Provider;
use eez_protocol::l2::{AnchorKind, ProfileError, Schedule};

use crate::{L1BlockRef, L2Endpoint, ReconciledBatchSettlement};

/// Expected source failures remain separate from invalid supplied evidence.
/// Provider errors are deliberately sanitized: RPC URLs may contain secrets.
#[derive(Debug, thiserror::Error)]
pub enum DerivationError {
    /// The source did not conclusively establish the requested canonical fact.
    #[error("L1 derivation source unavailable: {0}")]
    Unavailable(&'static str),
    /// Exact identity, finality, or ordered acceptance did not match.
    #[error("invalid L1 derivation context: {0}")]
    Mismatch(&'static str),
    /// The endpoint does not satisfy the shared L2 schedule.
    #[error(transparent)]
    Profile(#[from] ProfileError),
}

/// An exact header fetched by canonical height from the configured L1 source.
/// Its authenticity is relative to that source at the time of the read; later
/// uses must revalidate the target/canonical generation to handle reorgs.
#[derive(Debug, Clone)]
pub struct AuthenticatedL1Header {
    chain_id: u64,
    header: Header,
    hash: B256,
}

impl AuthenticatedL1Header {
    /// Exact execution header, including the authenticated seed and timestamp.
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// Canonical identity checked against a recomputed header hash.
    pub const fn reference(&self) -> L1BlockRef {
        L1BlockRef {
            number: self.header.number,
            hash: self.hash,
        }
    }
}

/// Read-only derivation source, initialized only after checking the genesis
/// reference's network identity, canonical header and finalized height.
/// Requests inherit the provider's transport timeout; callers retain their
/// existing request deadline/cancellation ownership around these async reads.
#[derive(Debug, Clone)]
pub struct DerivationSource<P = alloy_provider::DynProvider> {
    provider: P,
    schedule: Schedule,
}

impl<P: Provider> DerivationSource<P> {
    /// Authenticate the immutable genesis reference. A source that cannot serve
    /// `finalized` is unavailable, never implicitly treated as final.
    pub async fn authenticate(provider: P, schedule: Schedule) -> Result<Self, DerivationError> {
        let reference = schedule.config().l1_reference;
        let chain = provider
            .get_chain_id()
            .await
            .map_err(|_| DerivationError::Unavailable("chain identity"))?;
        if chain != reference.chain_id {
            return Err(DerivationError::Mismatch(
                "genesis reference chain identity",
            ));
        }
        let finalized = fetch(&provider, chain, BlockNumberOrTag::Finalized).await?;
        if finalized.header.number < reference.number {
            return Err(DerivationError::Mismatch(
                "genesis reference is not finalized",
            ));
        }
        // Read by number *after* finality. A by-hash read alone could serve an
        // orphan at the reference height and would not establish canonicality.
        let canonical = fetch(&provider, chain, reference.number.into()).await?;
        if canonical.hash != reference.hash
            || canonical.header.timestamp != reference.timestamp
            || canonical.header.mix_hash != reference.prev_randao
            || finalized.header.timestamp < canonical.header.timestamp
            || (finalized.header.number == reference.number && finalized.hash != canonical.hash)
        {
            return Err(DerivationError::Mismatch(
                "finalized genesis reference header",
            ));
        }
        Ok(Self { provider, schedule })
    }

    /// The same immutable schedule whose initial seed was authenticated.
    pub const fn schedule(&self) -> &Schedule {
        &self.schedule
    }

    /// Fetch by canonical height and require the expected hash. This detects a
    /// stale claimed target even if the provider can still serve it by hash.
    pub async fn canonical_header(
        &self,
        expected: L1BlockRef,
    ) -> Result<AuthenticatedL1Header, DerivationError> {
        let header = fetch(
            &self.provider,
            self.schedule.config().l1_reference.chain_id,
            expected.number.into(),
        )
        .await?;
        if header.hash != expected.hash {
            return Err(DerivationError::Mismatch("L1 block is no longer canonical"));
        }
        Ok(header)
    }

    /// Empty acceptance history, starting from the authenticated genesis seed.
    /// Restart must rebuild all canonical acceptances before exposing this to
    /// production; a persisted L2 header alone does not recover seed history.
    pub fn seed_history(&self) -> SeedHistory {
        SeedHistory::new(self.schedule)
    }
}

async fn fetch(
    provider: &impl Provider,
    chain_id: u64,
    tag: BlockNumberOrTag,
) -> Result<AuthenticatedL1Header, DerivationError> {
    let block = provider
        .get_block_by_number(tag)
        .await
        .map_err(|_| DerivationError::Unavailable("canonical header RPC"))?
        .ok_or(DerivationError::Unavailable("canonical header absent"))?;
    let hash = block.header.inner.hash_slow();
    if block.header.hash != hash
        || matches!(tag, BlockNumberOrTag::Number(number) if number != block.header.number)
    {
        return Err(DerivationError::Mismatch("canonical RPC header identity"));
    }
    Ok(AuthenticatedL1Header {
        chain_id,
        header: block.header.inner,
        hash,
    })
}

/// One accepted endpoint with an exact canonical L1 occurrence. This is not
/// deserializable: loading a checkpoint cannot confer settlement authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptedSeedAnchor {
    l1: L1BlockRef,
    tx_index: u64,
    tx_hash: B256,
    endpoint: L2Endpoint,
    kind: AnchorKind,
    seed: B256,
}

impl AcceptedSeedAnchor {
    /// Exact containing canonical L1 block.
    pub const fn l1(&self) -> L1BlockRef {
        self.l1
    }
    /// Accepted L2 endpoint, including same-height partial variants.
    pub const fn endpoint(&self) -> L2Endpoint {
        self.endpoint
    }
    /// Catch-up acceptance never refreshes a seed.
    pub const fn kind(&self) -> AnchorKind {
        self.kind
    }
}

/// Ordered history owned by canonical reconciliation, not by speculative block
/// building. Space is proportional to accepted settlements, never to a supplied
/// compressed block count. Publication/reorg must share the owner's canonical
/// lock and scan-completion boundary.
#[derive(Debug, Clone)]
pub struct SeedHistory {
    schedule: Schedule,
    anchors: Vec<AcceptedSeedAnchor>,
}

impl SeedHistory {
    /// Pure history initialized from a checked schedule. This does not establish
    /// L1 authenticity: runtimes obtain it through `DerivationSource::seed_history`
    /// only after authenticating the finalized genesis reference.
    pub fn new(schedule: Schedule) -> Self {
        Self {
            schedule,
            anchors: Vec::new(),
        }
    }

    /// Immutable schedule shared by all accepted seed transitions.
    pub const fn schedule(&self) -> Schedule {
        self.schedule
    }

    /// Record an independently reconciled EEZ acceptance and its reconstructed
    /// endpoint header. The caller must establish ordered applied effects, not
    /// merely a successful transaction receipt or a `BatchPosted` log.
    /// Failed/no-effect batches cannot produce an anchor. Mutation is atomic:
    /// any mismatch leaves the previous history intact.
    pub fn record(
        &mut self,
        settlement: &ReconciledBatchSettlement,
        l1: &AuthenticatedL1Header,
        endpoint_header: &Header,
    ) -> Result<(), DerivationError> {
        let anchor = self.validate_record(settlement, l1, endpoint_header)?;
        self.insert(anchor);
        Ok(())
    }

    pub(crate) fn validate_record(
        &self,
        settlement: &ReconciledBatchSettlement,
        l1: &AuthenticatedL1Header,
        endpoint_header: &Header,
    ) -> Result<AcceptedSeedAnchor, DerivationError> {
        let endpoint = settlement
            .endpoint
            .ok_or(DerivationError::Mismatch("batch has no accepted endpoint"))?;
        if settlement.l1_block != l1.reference()
            || l1.chain_id != self.schedule.config().l1_reference.chain_id
            || endpoint.number == 0
            || endpoint.number != endpoint_header.number
            || endpoint.hash != endpoint_header.hash_slow()
            || (!settlement.anchor_applied && settlement.applied_effects.is_empty())
            || l1.header.number < self.schedule.config().l1_reference.number
        {
            return Err(DerivationError::Mismatch(
                "accepted endpoint/header binding",
            ));
        }
        self.validate_endpoint(l1, settlement.tx_hash, settlement.tx_index, endpoint_header)
    }

    /// Restore an endpoint from the node's locally validated correlation index,
    /// or record one whose accepted hash was independently authenticated. The
    /// caller must establish acceptance separately; this method checks header,
    /// schedule and seed continuity, and cannot confer settlement authority.
    pub fn record_accepted_endpoint(
        &mut self,
        l1: &AuthenticatedL1Header,
        tx_hash: B256,
        tx_index: u64,
        endpoint_header: &Header,
    ) -> Result<(), DerivationError> {
        let anchor = self.validate_endpoint(l1, tx_hash, tx_index, endpoint_header)?;
        self.insert(anchor);
        Ok(())
    }

    fn validate_endpoint(
        &self,
        l1: &AuthenticatedL1Header,
        tx_hash: B256,
        tx_index: u64,
        endpoint_header: &Header,
    ) -> Result<AcceptedSeedAnchor, DerivationError> {
        if l1.chain_id != self.schedule.config().l1_reference.chain_id
            || l1.header.number < self.schedule.config().l1_reference.number
            || endpoint_header.number == 0
        {
            return Err(DerivationError::Mismatch(
                "accepted endpoint network/height",
            ));
        }
        let endpoint = L2Endpoint {
            number: endpoint_header.number,
            hash: endpoint_header.hash_slow(),
        };
        let kind = self.schedule.anchor_kind(
            endpoint.number,
            endpoint_header.timestamp,
            l1.header.timestamp,
        )?;
        // B[0] accepted live refreshes only descendants. B[1] at the same
        // height still executes under the seed that originally built B[0].
        if endpoint_header.mix_hash != self.seed_for(endpoint.number) {
            return Err(DerivationError::Mismatch(
                "accepted endpoint used the wrong seed",
            ));
        }
        let anchor = AcceptedSeedAnchor {
            l1: l1.reference(),
            tx_index,
            tx_hash,
            endpoint,
            kind,
            seed: match kind {
                AnchorKind::Live => l1.header.mix_hash,
                AnchorKind::CatchUp => self.seed_before_partition(
                    self.anchors
                        .partition_point(|anchor| anchor.endpoint.number <= endpoint.number),
                ),
            },
        };
        if let Ok(index) = self
            .anchors
            .binary_search_by_key(&(anchor.l1.number, anchor.tx_index), |previous| {
                (previous.l1.number, previous.tx_index)
            })
        {
            return if self.anchors[index] == anchor {
                Ok(anchor)
            } else {
                Err(DerivationError::Mismatch(
                    "conflicting accepted anchor occurrence",
                ))
            };
        }
        if let Some(previous) = self.anchors.last()
            && ((anchor.l1.number, anchor.tx_index) <= (previous.l1.number, previous.tx_index)
                || anchor.endpoint.number < previous.endpoint.number
                || (anchor.l1.number == previous.l1.number && anchor.l1.hash != previous.l1.hash))
        {
            return Err(DerivationError::Mismatch(
                "unordered or conflicting accepted anchor",
            ));
        }
        Ok(anchor)
    }

    fn insert(&mut self, anchor: AcceptedSeedAnchor) {
        if self
            .anchors
            .binary_search_by_key(&(anchor.l1.number, anchor.tx_index), |previous| {
                (previous.l1.number, previous.tx_index)
            })
            .is_err()
        {
            // validate_endpoint permits only a new ordered tail or an exact duplicate.
            self.anchors.push(anchor);
        }
    }

    /// Exact next position and captured seed for an authenticated parent.
    pub fn next_block_context(&self, parent: &Header) -> Result<(u64, B256), DerivationError> {
        let (number, timestamp) = self.schedule.next(parent)?;
        Ok((timestamp, self.seed_for(number)))
    }

    /// Select by the height being executed. Selecting just the latest accepted
    /// seed would corrupt replay of a partially consumed terminal block.
    pub fn seed_for(&self, number: u64) -> B256 {
        self.seed_before_partition(
            self.anchors
                .partition_point(|anchor| anchor.endpoint.number < number),
        )
    }

    fn seed_before_partition(&self, end: usize) -> B256 {
        end.checked_sub(1)
            .map_or(self.schedule.config().l1_reference.prev_randao, |index| {
                self.anchors[index].seed
            })
    }

    /// Remove non-surviving occurrences. Removing catch-up anchors preserves the
    /// seed; removing a live anchor restores the last surviving live seed.
    /// The caller also rewinds canonical L2 state and invalidates unsafe children
    /// before publishing scan completion. Finalized genesis cannot be removed.
    pub fn reorg_to(&mut self, ancestor: &AuthenticatedL1Header) -> Result<(), DerivationError> {
        let reference = self.schedule.config().l1_reference;
        if ancestor.chain_id != reference.chain_id
            || ancestor.header.number < reference.number
            || (ancestor.header.number == reference.number && ancestor.hash != reference.hash)
        {
            return Err(DerivationError::Mismatch(
                "reorg crosses finalized genesis reference",
            ));
        }
        self.anchors.retain(|anchor| {
            anchor.l1.number < ancestor.header.number
                || (anchor.l1.number == ancestor.header.number && anchor.l1.hash == ancestor.hash)
        });
        Ok(())
    }

    /// Reconciliation has already authenticated the surviving L1 ancestor.
    /// Keep this mutation with the cursor/evidence retreat under its owner lock.
    pub(crate) fn retain_through(&mut self, ancestor: u64) {
        self.anchors.retain(|anchor| anchor.l1.number <= ancestor);
    }

    /// Canonical occurrences retained for provenance/restart verification.
    pub fn anchors(&self) -> &[AcceptedSeedAnchor] {
        &self.anchors
    }
}

#[cfg(test)]
mod tests;
