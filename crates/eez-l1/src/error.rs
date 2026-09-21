//! Error type for L1 interactions. `thiserror`-derived; small surface.

use alloy_primitives::B256;
use thiserror::Error;

/// Convenience [`Result`] alias.
pub type L1Result<T> = Result<T, L1Error>;

/// Error returned by L1 readers, watchers, and the [`Submitter`](crate::Submitter).
#[derive(Debug, Error)]
pub enum L1Error {
    /// Bad / missing env var, malformed URL, etc.
    #[error("configuration error: {0}")]
    Config(String),
    /// Anything that went wrong talking to L1 (RPC transport, JSON-RPC error).
    #[error("L1 provider error: {0}")]
    Provider(String),
    /// On-chain BYTES we cannot decode. Deterministic, so callers fail loudly
    /// rather than loop. A malformed RPC *response* is `Provider`, not this.
    #[error("L1 decode error: {0}")]
    Decode(String),
    /// L1 exposed canonical metadata but not all data needed to decode
    /// or replay it yet. This is expected while a local L1 source is
    /// warming up and should be retried by boot reconciliation.
    #[error("L1 source incomplete at block {block}: tx {tx_hash} unavailable ({detail})")]
    SourceIncomplete {
        block: u64,
        tx_hash: B256,
        detail: String,
    },
    /// An applied batch's actual native retrieval cannot obtain its blobs.
    /// The retry class and displayed message match SourceIncomplete.
    #[error("L1 source incomplete at block {block}: tx {tx_hash} unavailable ({detail})")]
    RequiredBlobUnavailable {
        block: u64,
        tx_hash: B256,
        detail: String,
        reference: Box<crate::blob_diagnostics::RequiredBlob>,
        reason: crate::blob_diagnostics::BlobUnavailableReason,
    },
    /// postBatch tx didn't land or reverted on-chain.
    #[error("postBatch submission failed: {0}")]
    Submission(String),
    /// The configured builder relay answered `eth_sendBundle` with
    /// JSON-RPC `-32601` (method not found) — it is a plain execution
    /// RPC, not the timestamp-pinned bundle relay required for settlement.
    #[error("builder relay does not support eth_sendBundle")]
    BundleRpcUnsupported,
    /// L1 reorg deeper than the configured tolerance — beyond Ethereum
    /// finality. Operator must intervene (most likely a transient L1
    /// node anomaly; check L1 health and restart).
    #[error(
        "L1 reorg too deep: walked {walked} blocks without finding a common ancestor (max {max})"
    )]
    ReorgTooDeep { walked: usize, max: usize },
}

impl L1Error {
    pub(crate) fn with_required_blob(
        self,
        reference: crate::blob_diagnostics::RequiredBlob,
        reason: crate::blob_diagnostics::BlobUnavailableReason,
    ) -> Self {
        match self {
            Self::SourceIncomplete {
                block,
                tx_hash,
                detail,
            } => Self::RequiredBlobUnavailable {
                block,
                tx_hash,
                detail,
                reference: Box::new(reference),
                reason,
            },
            other => other,
        }
    }
    pub fn required_blob(
        &self,
    ) -> Option<(
        crate::blob_diagnostics::RequiredBlob,
        crate::blob_diagnostics::BlobUnavailableReason,
    )> {
        match self {
            Self::RequiredBlobUnavailable {
                reference, reason, ..
            } => Some((**reference, *reason)),
            _ => None,
        }
    }
    /// Returns true when the L1 source may simply need more time to
    /// expose already-observed canonical data.
    #[must_use]
    pub const fn is_source_incomplete(&self) -> bool {
        matches!(
            self,
            Self::SourceIncomplete { .. } | Self::RequiredBlobUnavailable { .. }
        )
    }

    /// True for a transport failure (DNS, TCP, HTTP) — the peer never answered,
    /// so it says nothing about the payload we sent. Decode failures are NOT
    /// transport: they are deterministic and never clear on retry.
    #[must_use]
    pub const fn is_transport(&self) -> bool {
        matches!(self, Self::Provider(_))
    }

    /// True when retrying re-reads the same bytes or re-walks the same chain,
    /// so a polling caller would spin forever instead of failing. These cases
    /// require operator intervention; this is what makes that real.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Config(_) | Self::Decode(_) | Self::ReorgTooDeep { .. }
        )
    }
}
