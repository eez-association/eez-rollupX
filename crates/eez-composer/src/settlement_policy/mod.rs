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
        for (reason, name) in Self::ALL {
            if self.contains(reason) {
                reasons.entry(&name);
            }
        }
        reasons.finish()
    }
}

impl DueReasons {
    /// Bounded reason names shared by diagnostics and monitoring.
    pub const ALL: [(Self, &'static str); 7] = [
        (Self::GENERAL, "general"),
        (Self::PURE_ALWAYS, "pure_always"),
        (Self::PURE_INTERVAL, "pure_interval"),
        (Self::CROSS_CHAIN, "cross_chain"),
        (Self::FULLNESS, "blob_fullness"),
        (Self::DRAIN, "drain"),
        (Self::WINDOW_LIMIT, "window_limit"),
    ];
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

/// Facts available before a terminal is built. The target is a height, not a
/// fabricated block commitment. This evaluation cannot create drain ownership.
#[derive(Debug, Clone, Copy)]
pub struct SchedulingSnapshot {
    pub anchor: SettlementAnchor,
    pub target_l2_number: u64,
    pub target_l1_timestamp: u64,
    pub latest_pure_block: Option<L2Endpoint>,
    pub estimated_stream_bytes: Option<u64>,
    pub window_limit_due: bool,
    pub effective_blobs: u64,
    pub usable_bytes_per_blob: u64,
}

impl SchedulingSnapshot {
    fn evaluate(self, config: &PolicyConfig) -> Result<(DueReasons, u64, u64), PolicyError> {
        let unsettled_blocks = self
            .target_l2_number
            .checked_sub(self.anchor.endpoint.number)
            .ok_or_else(|| {
                PolicyError::Unavailable("candidate precedes settled endpoint".into())
            })?;
        let target_ms = self
            .target_l1_timestamp
            .checked_mul(1000)
            .ok_or(PolicyError::Arithmetic("target timestamp in milliseconds"))?;
        let anchor_ms = self
            .anchor
            .l1_timestamp
            .checked_mul(1000)
            .ok_or(PolicyError::Arithmetic("anchor timestamp in milliseconds"))?;
        let settlement_age_ms = target_ms.checked_sub(anchor_ms).ok_or_else(|| {
            PolicyError::Unavailable("target precedes canonical anchor timestamp".into())
        })?;
        if self.effective_blobs == 0 || self.usable_bytes_per_blob == 0 {
            return Err(PolicyError::Unavailable(
                "effective blob capacity is zero or unknown".into(),
            ));
        }
        if let Some(pure) = self.latest_pure_block
            && (pure.number <= self.anchor.endpoint.number || pure.number > self.target_l2_number)
        {
            return Err(PolicyError::Unavailable(
                "pure activity lies outside the pending range".into(),
            ));
        }
        if self.estimated_stream_bytes.is_none() && !self.window_limit_due {
            return Err(PolicyError::Unavailable(
                "missing candidate size without window limit".into(),
            ));
        }
        let mut reasons = DueReasons::default();
        if self.window_limit_due && unsettled_blocks > 0 {
            reasons.insert(DueReasons::WINDOW_LIMIT);
        }
        if unsettled_blocks >= config.max_unsettled_l2_blocks.get() {
            reasons.insert(DueReasons::GENERAL);
        }
        if self.latest_pure_block.is_some() {
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
        if let Some(fullness) = config.blob_fullness_bps
            && unsettled_blocks > 0
            && let Some(bytes) = self.estimated_stream_bytes
        {
            let threshold = u128::from(self.effective_blobs)
                .checked_mul(u128::from(self.usable_bytes_per_blob))
                .and_then(|value| value.checked_mul(u128::from(fullness.get())))
                .ok_or(PolicyError::Arithmetic("aggregate blob fullness capacity"))?;
            if u128::from(bytes) * 10_000 >= threshold {
                reasons.insert(DueReasons::FULLNESS);
            }
        }
        Ok((reasons, unsettled_blocks, settlement_age_ms))
    }
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

    /// Conservative scheduling hint using the same pure triggers as final
    /// evaluation. A retained drain may cause one unnecessary preparation;
    /// only evaluation of a real candidate reconciles or creates that intent.
    pub fn preparation_reasons(
        &self,
        config: &PolicyConfig,
        snapshot: SchedulingSnapshot,
    ) -> Result<DueReasons, PolicyError> {
        let (mut reasons, _, _) = snapshot.evaluate(config)?;
        if self.drain.is_some() {
            reasons.insert(DueReasons::DRAIN);
        }
        Ok(reasons)
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
        let (mut reasons, unsettled_blocks, settlement_age_ms) = SchedulingSnapshot {
            anchor,
            target_l2_number: candidate.number,
            target_l1_timestamp,
            latest_pure_block,
            estimated_stream_bytes,
            window_limit_due,
            effective_blobs,
            usable_bytes_per_blob,
        }
        .evaluate(config)?;
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
