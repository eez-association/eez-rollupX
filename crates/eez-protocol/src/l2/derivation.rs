//! Parent-derived timing and live/catch-up classification. These pure checks
//! require independently authenticated L1 context; they cannot establish it.

use alloy_consensus::Header;
use alloy_primitives::B256;
use serde::{Deserialize, Serialize};

use super::ProfileError;

/// Finalized L1 reference selected before calculating the L2 genesis hash.
/// Startup must check the exact canonical finalized header and chain identity;
/// deserialization alone does not authenticate these supplied values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenesisL1Reference {
    /// Settlement network's EIP-155 identity.
    pub chain_id: u64,
    /// Reference execution block number (zero is valid on private fixtures).
    pub number: u64,
    /// Exact reference header hash, including its timestamp and RANDAO.
    pub hash: B256,
    /// Reference timestamp, also fixing the settlement slot phase.
    pub timestamp: u64,
    /// Bootstrap randomness, retained until the first scheduled history update.
    pub prev_randao: B256,
}

/// Explicit genesis parameters, separate from local preparation/proof deadlines.
/// Mainnet uses 2-second blocks and six positions; Chiado uses 1 and five.
/// Private settlement networks may select their own whole-second schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DerivationConfig {
    /// Whole seconds between every pair of consecutive L2 blocks.
    pub block_interval_seconds: u64,
    /// Number of positions in a complete Sync-to-Sync interval, at least two.
    pub sync_interval_blocks: u32,
    /// Fixed history/PREVRANDAO update offset; must be less than the Sync interval.
    pub history_update_offset: u32,
    /// Independently authenticated startup reference for timing and bootstrap history.
    pub l1_reference: GenesisL1Reference,
}

/// A structurally checked schedule pinned to one exact L2 genesis. Its L1
/// reference still needs canonical/finalized authentication by the runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    config: DerivationConfig,
    genesis_hash: B256,
    genesis_timestamp: u64,
    l1_slot_seconds: u64,
}

/// Endpoint eligibility relative to the settlement context. History and randomness
/// follow their fixed delayed schedule independently of this classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorKind {
    /// Endpoint timestamp equals its containing L1 block's timestamp.
    Live,
    /// Endpoint predates the settlement context; cross-chain actions are forbidden.
    CatchUp,
}

impl Schedule {
    /// Reject invalid configuration or genesis values without supplying defaults
    /// or changing phase. This does not check L1 canonicality or finality.
    pub fn new(config: DerivationConfig, genesis: &Header) -> Result<Self, ProfileError> {
        let step = config.block_interval_seconds;
        let positions = config.sync_interval_blocks;
        if step == 0
            || positions < 2
            || config.history_update_offset >= positions
            || config.l1_reference.chain_id == 0
            || config.l1_reference.hash == B256::ZERO
        {
            return Err(ProfileError("invalid genesis derivation parameters"));
        }
        match config.l1_reference.chain_id {
            1 if (step, positions) != (2, 6) => {
                return Err(ProfileError(
                    "mainnet requires a 2-second, six-position schedule",
                ));
            }
            10200 if (step, positions) != (1, 5) => {
                return Err(ProfileError(
                    "Chiado requires a 1-second, five-position schedule",
                ));
            }
            _ => {}
        }
        let l1_slot_seconds = step
            .checked_mul(u64::from(positions))
            .ok_or(ProfileError("settlement interval overflow"))?;
        let phase = genesis
            .timestamp
            .checked_sub(config.l1_reference.timestamp)
            .ok_or(ProfileError("genesis precedes its L1 reference"))?;
        if genesis.number != 0
            || !phase.is_multiple_of(l1_slot_seconds)
            || genesis.mix_hash != config.l1_reference.prev_randao
        {
            return Err(ProfileError(
                "genesis number, slot phase or reference RANDAO",
            ));
        }
        Ok(Self {
            config,
            genesis_hash: genesis.hash_slow(),
            genesis_timestamp: genesis.timestamp,
            l1_slot_seconds,
        })
    }

    /// Shared delayed-update schedule. Reject a genesis before its bootstrap
    /// window; no legacy seed path or activation bootstrap is inferred.
    pub fn history(&self) -> Result<crate::l1_history::HistorySchedule, ProfileError> {
        let history = crate::l1_history::HistorySchedule::new(
            self.genesis_timestamp,
            self.config.block_interval_seconds,
            self.config.sync_interval_blocks,
            self.config.history_update_offset,
        )?;
        if self.config.l1_reference.timestamp > history.bootstrap_cutoff() {
            return Err(ProfileError("genesis reference is after bootstrap cutoff"));
        }
        Ok(history)
    }

    /// Exact genesis identity whose timestamp defines block-number alignment.
    pub const fn genesis_hash(&self) -> B256 {
        self.genesis_hash
    }

    /// Immutable parameters shared by production, reconstruction and replay.
    pub const fn config(&self) -> &DerivationConfig {
        &self.config
    }

    /// Nominal L1 interval in whole seconds, independent of missed slots.
    pub const fn l1_slot_seconds(&self) -> u64 {
        self.l1_slot_seconds
    }

    /// Derive the timestamp with checked multiplication/addition. No wall clock,
    /// local Live/Future label, or supplied child timestamp participates.
    pub fn timestamp(&self, number: u64) -> Result<u64, ProfileError> {
        number
            .checked_mul(self.config.block_interval_seconds)
            .and_then(|offset| self.genesis_timestamp.checked_add(offset))
            .ok_or(ProfileError("block timestamp overflow"))
    }

    /// Check the actual parent's position before deriving its next position.
    /// Parent hash/state authentication remains the caller's responsibility.
    pub fn next(&self, parent: &Header) -> Result<(u64, u64), ProfileError> {
        self.validate_position(parent)?;
        let number = parent
            .number
            .checked_add(1)
            .ok_or(ProfileError("block number overflow"))?;
        Ok((number, self.timestamp(number)?))
    }

    /// A complete candidate starts from a Sync parent and ends at a Sync position.
    /// Wire count validity and operator replay budgets are separate checks.
    /// `has_actions` includes failed/static actions that emit no L2 transaction.
    pub fn candidate(
        &self,
        parent: &Header,
        blocks: u32,
        target_timestamp: u64,
        has_actions: bool,
    ) -> Result<(u64, AnchorKind), ProfileError> {
        self.validate_position(parent)?;
        let interval = u64::from(self.config.sync_interval_blocks);
        if blocks == 0
            || !parent.number.is_multiple_of(interval)
            || !u64::from(blocks).is_multiple_of(interval)
        {
            return Err(ProfileError(
                "candidate must cover complete Sync-to-Sync intervals",
            ));
        }
        let number = parent
            .number
            .checked_add(u64::from(blocks))
            .ok_or(ProfileError("candidate endpoint overflow"))?;
        let endpoint_timestamp = self.timestamp(number)?;
        let kind = self.anchor_kind(number, endpoint_timestamp, target_timestamp)?;
        if kind == AnchorKind::CatchUp && has_actions {
            return Err(ProfileError(
                "catch-up candidates cannot carry cross-chain actions",
            ));
        }
        Ok((number, kind))
    }

    /// Classify an endpoint against the bound settlement timestamp. Neither
    /// classification changes the delayed history/PREVRANDAO update schedule.
    pub fn anchor_kind(
        &self,
        number: u64,
        timestamp: u64,
        l1_timestamp: u64,
    ) -> Result<AnchorKind, ProfileError> {
        if !number.is_multiple_of(u64::from(self.config.sync_interval_blocks))
            || timestamp != self.timestamp(number)?
            || l1_timestamp
                .checked_sub(self.config.l1_reference.timestamp)
                .is_none_or(|offset| !offset.is_multiple_of(self.l1_slot_seconds))
            || timestamp > l1_timestamp
        {
            return Err(ProfileError(
                "anchor position or containing settlement slot",
            ));
        }
        Ok(if timestamp == l1_timestamp {
            AnchorKind::Live
        } else {
            AnchorKind::CatchUp
        })
    }

    /// Check Engine payload coordinates before a header or execution state exists.
    pub fn validate_coordinates(&self, number: u64, timestamp: u64) -> Result<(), ProfileError> {
        if timestamp != self.timestamp(number)? {
            return Err(ProfileError(
                "header does not occupy its genesis-derived position",
            ));
        }
        Ok(())
    }

    /// Enforce the same number/timestamp relationship on supplied replay headers.
    pub fn validate_position(&self, header: &Header) -> Result<(), ProfileError> {
        self.validate_coordinates(header.number, header.timestamp)?;
        if header.number == 0 && header.hash_slow() != self.genesis_hash {
            return Err(ProfileError(
                "header does not occupy its genesis-derived position",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(chain_id: u64, step: u64, positions: u32) -> (DerivationConfig, Header) {
        let seed = B256::with_last_byte(7);
        (
            DerivationConfig {
                history_update_offset: 0,
                block_interval_seconds: step,
                sync_interval_blocks: positions,
                l1_reference: GenesisL1Reference {
                    chain_id,
                    number: 10,
                    hash: B256::with_last_byte(10),
                    timestamp: 1000,
                    prev_randao: seed,
                },
            },
            Header {
                timestamp: 1000,
                mix_hash: seed,
                ..Default::default()
            },
        )
    }

    #[test]
    fn exact_mainnet_and_chiado_positions_and_complete_intervals() {
        for (chain, step, positions) in [(1, 2, 6), (10200, 1, 5)] {
            let (config, genesis) = fixture(chain, step, positions);
            let schedule = Schedule::new(config, &genesis).unwrap();
            let slot = step * u64::from(positions);
            for n in 1..=u64::from(positions) * 3 {
                assert_eq!(schedule.timestamp(n).unwrap(), 1000 + n * step);
            }
            assert_eq!(schedule.next(&genesis).unwrap(), (1, 1000 + step));
            assert_eq!(
                schedule
                    .candidate(&genesis, positions, 1000 + slot, true)
                    .unwrap(),
                (u64::from(positions), AnchorKind::Live)
            );
            assert_eq!(
                schedule
                    .candidate(&genesis, positions * 2, 1000 + slot * 3, false)
                    .unwrap()
                    .1,
                AnchorKind::CatchUp
            );
            assert!(
                schedule
                    .candidate(&genesis, positions * 2, 1000 + slot * 3, true)
                    .is_err()
            );
            for count in [0, 1, positions - 1, positions + 1] {
                assert!(
                    schedule
                        .candidate(&genesis, count, 1000 + slot * 3, false)
                        .is_err()
                );
            }
            assert!(
                schedule
                    .candidate(&genesis, positions * 2, 1000 + slot, false)
                    .is_err()
            );
            assert!(
                schedule
                    .candidate(&genesis, positions, 1001 + slot, false)
                    .is_err()
            );
            let mut parent = genesis.clone();
            parent.number = 1;
            parent.timestamp += step;
            assert!(
                schedule
                    .candidate(&parent, positions, 1000 + slot * 2, false)
                    .is_err()
            );
            parent.timestamp += 1;
            assert!(schedule.next(&parent).is_err());
        }
    }

    #[test]
    fn full_u32_range_is_not_a_wire_ceiling_or_an_allocation() {
        let (config, genesis) = fixture(10200, 1, 5);
        let schedule = Schedule::new(config, &genesis).unwrap();
        let endpoint = u64::from(u32::MAX);
        assert_eq!(
            schedule
                .candidate(&genesis, u32::MAX, 1000 + endpoint, false)
                .unwrap(),
            (endpoint, AnchorKind::Live)
        );
        assert!(schedule.timestamp(u64::MAX).is_err());
        let overflow = Header {
            number: u64::MAX,
            timestamp: 1000,
            ..Default::default()
        };
        assert!(schedule.next(&overflow).is_err());
    }

    #[test]
    fn supplied_config_genesis_and_unknown_fields_are_rejected_without_defaults() {
        let (config, genesis) = fixture(1, 2, 6);
        for (step, positions) in [(0, 6), (2, 0), (1, 6), (2, 5), (u64::MAX, u32::MAX)] {
            let mut wrong = config;
            wrong.block_interval_seconds = step;
            wrong.sync_interval_blocks = positions;
            assert!(Schedule::new(wrong, &genesis).is_err());
        }
        let changes: &[fn(&mut Header)] = &[
            |h| h.timestamp += 1,
            |h| h.timestamp -= 1,
            |h| h.number = 1,
            |h| h.mix_hash = B256::ZERO,
        ];
        for change in changes {
            let mut wrong = genesis.clone();
            change(&mut wrong);
            assert!(Schedule::new(config, &wrong).is_err());
        }
        let mut document = serde_json::to_value(config).unwrap();
        document["blockIntervalMillis"] = 2000.into();
        assert!(serde_json::from_value::<DerivationConfig>(document).is_err());
    }
}
