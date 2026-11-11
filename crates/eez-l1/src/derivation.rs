//! Canonical L1 authentication for producer and follower execution.
//!
//! The provider is an operator-selected canonical L1 source. The offline
//! signer uses request evidence and on-chain context binding instead.

pub mod history;

use alloy_consensus::Header;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use alloy_provider::Provider;
use eez_protocol::l2::{ProfileError, Schedule};

use crate::L1BlockRef;

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
    ///
    /// The L1 genesis block (number zero) is final by definition: no fork
    /// choice can replace it without changing the chain itself. It only has to
    /// match the canonical block zero, so a fresh devnet need not wait for its
    /// first finalized checkpoint (execution clients report no `finalized`
    /// block until then). Any later reference still requires finality.
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
        if reference.number == 0 {
            let genesis = fetch(&provider, chain, 0.into()).await?;
            if genesis.hash != reference.hash
                || genesis.header.timestamp != reference.timestamp
                || genesis.header.mix_hash != reference.prev_randao
            {
                return Err(DerivationError::Mismatch("L1 genesis reference header"));
            }
            return Ok(Self { provider, schedule });
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
}

async fn fetch(
    provider: &impl Provider,
    _chain_id: u64,
    tag: BlockNumberOrTag,
) -> Result<AuthenticatedL1Header, DerivationError> {
    let (header, claimed_hash) = match provider.get_block_by_number(tag).await {
        Ok(Some(block)) => (block.header.inner, block.header.hash),
        Ok(None) => return Err(DerivationError::Unavailable("canonical header absent")),
        // Reth's block RPC (including eth_getHeaderByNumber) requires retained
        // bodies. Error 4444 reports expired history even when the canonical
        // header remains available. The raw-header method reads headers alone;
        // keep the same canonical number/tag, never fall back to a by-hash read.
        Err(error)
            if error
                .as_error_resp()
                .is_some_and(|error| error.code == 4444) =>
        {
            let raw: alloy_primitives::Bytes = provider
                .raw_request("debug_getRawHeader".into(), (tag,))
                .await
                .map_err(|_| DerivationError::Unavailable("canonical header RPC"))?;
            let header = alloy_rlp::decode_exact::<Header>(&raw)
                .map_err(|_| DerivationError::Mismatch("canonical raw header encoding"))?;
            // Bind the decoded representation to the bytes the source served,
            // as well as checking the caller's expected canonical identity.
            (header, alloy_primitives::keccak256(&raw))
        }
        Err(_) => return Err(DerivationError::Unavailable("canonical header RPC")),
    };
    let hash = header.hash_slow();
    if claimed_hash != hash
        || matches!(tag, BlockNumberOrTag::Number(number) if number != header.number)
    {
        return Err(DerivationError::Mismatch("canonical RPC header identity"));
    }
    Ok(AuthenticatedL1Header { header, hash })
}

#[cfg(test)]
mod tests;
