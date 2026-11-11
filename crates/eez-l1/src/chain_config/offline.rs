//! One canonical semantic L1 profile for deployment, Composer and offline proving.

use alloy_primitives::{B256, U256, keccak256};
use eez_protocol::{
    l1_history::{HeaderRules, NetworkProfile},
    l2::Schedule,
};
use reth_chainspec::{ChainSpec, EthereumHardfork, ForkCondition, Hardforks as _};

/// Effective rules, independently derived from the operator's loaded L1 spec.
/// The canonical bytes are publishable configuration, not network evidence.
#[derive(Debug, Clone)]
pub struct OfflineL1Rules {
    /// Strict historical execution-header layouts.
    pub headers: HeaderRules,
    /// Canonical semantic document committing all effective forks and blob rules.
    pub canonical_json: Vec<u8>,
    /// Keccak of `canonical_json`.
    pub config_hash: B256,
    chain_id: u64,
    last_block_fork: u64,
}

impl OfflineL1Rules {
    /// Build the profile committed by every proof producer and its registered key.
    /// Fresh L2 bootstrap must follow all block/TTD-based L1 transitions: future
    /// header transitions are timestamp-based, as on supported PoS L1s.
    pub fn profile(&self, schedule: Schedule, l2_chain_id: u64) -> eyre::Result<NetworkProfile> {
        let config = schedule.config();
        eyre::ensure!(
            config.l1_reference.chain_id == self.chain_id,
            "L1 reference/profile chain mismatch"
        );
        eyre::ensure!(
            config.l1_reference.number >= self.last_block_fork,
            "fresh L2 bootstrap precedes a block-based L1 rule transition"
        );
        let profile = NetworkProfile {
            l1ChainId: U256::from(self.chain_id),
            l2ChainId: U256::from(l2_chain_id),
            genesisHash: schedule.genesis_hash(),
            genesisTimestamp: schedule.timestamp(0)?,
            blockInterval: config.block_interval_seconds,
            syncInterval: config.sync_interval_blocks,
            updateOffset: config.history_update_offset,
            l1ConfigHash: self.config_hash,
            feeCollector: config.fee_collector,
        };
        profile.validate(U256::from(self.chain_id))?;
        schedule.history()?;
        Ok(profile)
    }
}

/// Commit the effective pinned fork and blob schedule, including preset-specific
/// Gnosis parameters and unnamed timestamp blob updates. No RPC is performed.
pub fn offline_l1_rules(spec: &ChainSpec) -> eyre::Result<OfflineL1Rules> {
    let timestamp = |fork| -> eyre::Result<Option<u64>> {
        match spec.hardforks.fork(fork) {
            ForkCondition::Timestamp(at) => Ok(Some(at)),
            ForkCondition::Block(0) => Ok(Some(0)),
            ForkCondition::Never => Ok(None),
            _ => eyre::bail!("unsupported coordinate for timestamp-based L1 fork"),
        }
    };
    let london_block = match spec.hardforks.fork(EthereumHardfork::London) {
        ForkCondition::Block(at) => Some(at),
        ForkCondition::Never => None,
        _ => eyre::bail!("London must have a block-based activation"),
    };
    let headers = HeaderRules {
        london_block,
        shanghai_time: timestamp(EthereumHardfork::Shanghai)?,
        cancun_time: timestamp(EthereumHardfork::Cancun)?,
        prague_time: timestamp(EthereumHardfork::Prague)?,
        unsupported_time: [
            timestamp(EthereumHardfork::Amsterdam)?,
            timestamp(EthereumHardfork::Bogota)?,
        ]
        .into_iter()
        .flatten()
        .min(),
    };
    let mut forks = std::collections::BTreeMap::new();
    let mut last_block_fork = 0;
    for (fork, condition) in spec.forks_iter() {
        let value = match condition {
            ForkCondition::Block(number) => {
                last_block_fork = last_block_fork.max(number);
                serde_json::json!({"block": number})
            }
            ForkCondition::Timestamp(at) => {
                serde_json::json!({"timestamp": at})
            }
            ForkCondition::TTD {
                activation_block_number,
                fork_block,
                total_difficulty,
            } => {
                last_block_fork = last_block_fork.max(activation_block_number);
                serde_json::json!({"activationBlock": activation_block_number, "advertisedForkBlock": fork_block, "totalDifficulty": total_difficulty})
            }
            ForkCondition::Never => serde_json::Value::Null,
        };
        eyre::ensure!(
            forks.insert(fork.name(), value).is_none(),
            "duplicate L1 fork name"
        );
    }
    eyre::ensure!(
        spec.blob_params
            .scheduled
            .windows(2)
            .all(|pair| pair[0].0 < pair[1].0),
        "unordered L1 blob schedule"
    );
    let document = serde_json::json!({
        "domain": "EEZ_ROLLUPX_EFFECTIVE_L1_RULES_V1",
        "chainId": spec.chain().id(), "genesisHash": spec.genesis_header.hash(),
        "forks": forks, "headerRules": headers,
        "blobRules": {"cancun": spec.blob_params.cancun, "prague": spec.blob_params.prague,
            "osaka": spec.blob_params.osaka, "scheduled": spec.blob_params.scheduled},
    });
    let canonical_json = super::canonicalize_chain_json(&serde_json::to_vec(&document)?)?;
    Ok(OfflineL1Rules {
        headers,
        config_hash: keccak256(&canonical_json),
        canonical_json,
        chain_id: spec.chain().id(),
        last_block_fork,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn effective_rule_identity_binds_unnamed_blob_updates_and_fork_boundaries() {
        let spec = reth_chainspec::MAINNET.clone();
        let original = offline_l1_rules(&spec).unwrap();
        let mut changed = (*spec).clone();
        let next = changed
            .blob_params
            .scheduled
            .last()
            .map_or(2_000_000_000, |(at, _)| at + 100);
        changed
            .blob_params
            .scheduled
            .push((next, changed.blob_params.osaka));
        let next_rules = offline_l1_rules(&changed).unwrap();
        assert_ne!(original.config_hash, next_rules.config_hash);
        changed
            .blob_params
            .scheduled
            .push((next, changed.blob_params.osaka));
        assert!(offline_l1_rules(&changed).is_err());
    }
}
