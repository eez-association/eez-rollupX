//! Deterministic settlement decisions. No RPC, clock, proof, or environment
//! access belongs here. The composer supplies one canonical snapshot and owns
//! transaction admission, reconciliation, and submission.

mod config;
pub(crate) mod index;

pub use config::{PolicyConfig, PureL2Mode};

use alloy_primitives::B256;
use eez_l1::{L1BlockRef, L2Endpoint};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PolicyError {
    #[error("settlement policy configuration: {0}")]
    Config(String),
    #[error("settlement policy arithmetic overflow: {0}")]
    Arithmetic(&'static str),
    #[error("settlement policy snapshot unavailable: {0}")]
    Unavailable(String),
}

/// A validated L2 commitment, attributed to its canonical containing L1 block
/// and transaction position. Registration uses the registration transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementAnchor {
    pub l1: L1BlockRef,
    pub l1_timestamp: u64,
    pub tx_hash: B256,
    pub tx_index: u64,
    pub endpoint: L2Endpoint,
}

/// All independent causes of one settlement, rather than separate submissions.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct DueReasons(u8);

impl std::fmt::Debug for DueReasons {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut reasons = formatter.debug_set();
        for (reason, name) in [
            (Self::GENERAL, "general"),
            (Self::PURE_ALWAYS, "pure_always"),
            (Self::PURE_INTERVAL, "pure_interval"),
            (Self::CROSS_CHAIN, "cross_chain"),
            (Self::FULLNESS, "blob_fullness"),
            (Self::DRAIN, "drain"),
            (Self::WINDOW_LIMIT, "window_limit"),
        ] {
            if self.contains(reason) {
                reasons.entry(&name);
            }
        }
        reasons.finish()
    }
}

impl DueReasons {
    pub const GENERAL: Self = Self(1);
    pub const PURE_ALWAYS: Self = Self(2);
    pub const PURE_INTERVAL: Self = Self(4);
    pub const CROSS_CHAIN: Self = Self(8);
    pub const FULLNESS: Self = Self(16);
    pub const DRAIN: Self = Self(32);
    pub const WINDOW_LIMIT: Self = Self(64);

    #[must_use]
    pub const fn contains(self, reason: Self) -> bool {
        self.0 & reason.0 != 0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn insert(&mut self, reason: Self) {
        self.0 |= reason.0;
    }
}

/// Only admitted executable work or a validated finite dependency is urgent.
/// Temporarily unavailable, oversized and invalid pool entries are not signals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CrossChainWork {
    #[default]
    None,
    Executable,
    Predecessor(L2Endpoint),
}

/// Relationship of the retained endpoint to canonical, validated history.
/// The caller resolves this by hash after existing ownership reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainCoverage {
    Pending,
    Covered,
    Replaced,
}

#[derive(Debug, Clone, Copy)]
pub struct PolicySnapshot {
    pub anchor: SettlementAnchor,
    pub candidate: L2Endpoint,
    /// Seconds from the actual selected network target, not the local clock.
    pub target_l1_timestamp: u64,
    /// Latest canonical block containing an included ordinary transaction.
    pub latest_pure_block: Option<L2Endpoint>,
    pub cross_chain: CrossChainWork,
    /// None only when the whole candidate exceeds the supported window; a
    /// bounded historical prefix must then be selected instead.
    pub estimated_stream_bytes: Option<u64>,
    /// Another whole Sync interval would exceed the effective window ceiling.
    pub window_limit_due: bool,
    pub effective_blobs: u64,
    pub usable_bytes_per_blob: u64,
    pub drain_coverage: DrainCoverage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrainIntent {
    pub endpoint: L2Endpoint,
    pub reasons: DueReasons,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyDecision {
    pub reasons: DueReasons,
    pub required_endpoint: Option<L2Endpoint>,
    pub unsettled_blocks: u64,
    pub settlement_age_ms: u64,
}

impl PolicyDecision {
    #[must_use]
    pub const fn is_due(self) -> bool {
        !self.reasons.is_empty()
    }
}

/// Volatile periodic/pure-activity obligation. Cross-chain ownership and its
/// finite, group-linked predecessor are supplied on each snapshot by existing
/// admission/recovery. They must not leave an orphan drain after invalidation.
/// Restart deliberately starts with `default()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyState {
    drain: Option<DrainIntent>,
}

impl PolicyState {
    #[must_use]
    pub const fn drain(&self) -> Option<DrainIntent> {
        self.drain
    }

    /// Evaluate one consistent canonical snapshot. No state changes on error.
    ///
    /// # Errors
    /// Missing/contradictory target inputs or overflow. These are deferrals,
    /// never evidence that the batch is over capacity or already settled.
    pub fn evaluate(
        &mut self,
        config: &PolicyConfig,
        snapshot: PolicySnapshot,
    ) -> Result<PolicyDecision, PolicyError> {
        let PolicySnapshot {
            anchor,
            candidate,
            target_l1_timestamp,
            latest_pure_block,
            cross_chain,
            estimated_stream_bytes,
            window_limit_due,
            effective_blobs,
            usable_bytes_per_blob,
            drain_coverage,
        } = snapshot;
        let unsettled_blocks = candidate
            .number
            .checked_sub(anchor.endpoint.number)
            .ok_or_else(|| {
                PolicyError::Unavailable("candidate precedes settled endpoint".into())
            })?;
        let target_ms = target_l1_timestamp
            .checked_mul(1000)
            .ok_or(PolicyError::Arithmetic("target timestamp in milliseconds"))?;
        let anchor_ms = anchor
            .l1_timestamp
            .checked_mul(1000)
            .ok_or(PolicyError::Arithmetic("anchor timestamp in milliseconds"))?;
        let settlement_age_ms = target_ms.checked_sub(anchor_ms).ok_or_else(|| {
            PolicyError::Unavailable("target precedes canonical anchor timestamp".into())
        })?;
        if effective_blobs == 0 || usable_bytes_per_blob == 0 {
            return Err(PolicyError::Unavailable(
                "effective blob capacity is zero or unknown".into(),
            ));
        }
        if let Some(pure) = latest_pure_block
            && (pure.number <= anchor.endpoint.number || pure.number > candidate.number)
        {
            return Err(PolicyError::Unavailable(
                "pure activity lies outside the pending range".into(),
            ));
        }
        if estimated_stream_bytes.is_none() && !window_limit_due {
            return Err(PolicyError::Unavailable(
                "missing candidate size without window limit".into(),
            ));
        }
        let mut reasons = DueReasons::default();
        if window_limit_due && unsettled_blocks > 0 {
            reasons.insert(DueReasons::WINDOW_LIMIT);
        }
        if unsettled_blocks >= config.max_unsettled_l2_blocks.get() {
            reasons.insert(DueReasons::GENERAL);
        }
        if latest_pure_block.is_some() {
            match config.pure_l2 {
                PureL2Mode::Always => reasons.insert(DueReasons::PURE_ALWAYS),
                PureL2Mode::Interval { milliseconds } => {
                    let deadline = anchor_ms
                        .checked_add(milliseconds.get())
                        .ok_or(PolicyError::Arithmetic("settlement interval deadline"))?;
                    if target_ms >= deadline {
                        reasons.insert(DueReasons::PURE_INTERVAL);
                    }
                }
            }
        }
        let cross_chain_endpoint = match cross_chain {
            CrossChainWork::None => None,
            CrossChainWork::Executable => Some(candidate),
            CrossChainWork::Predecessor(endpoint) => {
                if endpoint.number <= anchor.endpoint.number || endpoint.number > candidate.number {
                    return Err(PolicyError::Unavailable(
                        "cross-chain predecessor is not finite pending history".into(),
                    ));
                }
                Some(endpoint)
            }
        };
        if cross_chain_endpoint.is_some() {
            reasons.insert(DueReasons::CROSS_CHAIN);
        }
        if let Some(fullness) = config.blob_fullness_bps
            && unsettled_blocks > 0
            && let Some(estimated_stream_bytes) = estimated_stream_bytes
        {
            let occupied = u128::from(estimated_stream_bytes) * 10_000;
            let threshold = u128::from(effective_blobs)
                .checked_mul(u128::from(usable_bytes_per_blob))
                .and_then(|value| value.checked_mul(u128::from(fullness.get())))
                .ok_or(PolicyError::Arithmetic("aggregate blob fullness capacity"))?;
            if occupied >= threshold {
                reasons.insert(DueReasons::FULLNESS);
            }
        }
        let mut drain = match drain_coverage {
            DrainCoverage::Pending => self.drain,
            DrainCoverage::Covered | DrainCoverage::Replaced => None,
        };
        let retained_reasons = DueReasons(reasons.0 & !DueReasons::CROSS_CHAIN.0);
        if let Some(ref mut intent) = drain {
            // Time/general/fullness never extend an unfinished finite drain,
            // including after an intermediate prefix advances its anchor.
            let new_pure = reasons.contains(DueReasons::PURE_ALWAYS)
                && latest_pure_block.is_some_and(|block| block.number > intent.endpoint.number);
            if new_pure {
                intent.endpoint = candidate;
            }
            intent.reasons.0 |= retained_reasons.0;
            reasons.insert(DueReasons::DRAIN);
        } else if !retained_reasons.is_empty() {
            drain = Some(DrainIntent {
                endpoint: candidate,
                reasons: retained_reasons,
            });
        }
        self.drain = drain;
        let required_endpoint = match (drain.map(|intent| intent.endpoint), cross_chain_endpoint) {
            (Some(periodic), Some(cross)) if periodic.number > cross.number => Some(periodic),
            (_, Some(cross)) => Some(cross),
            (periodic, None) => periodic,
        };
        Ok(PolicyDecision {
            reasons,
            required_endpoint,
            unsettled_blocks,
            settlement_age_ms,
        })
    }
}

#[cfg(test)]
mod tests;
