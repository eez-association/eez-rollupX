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

    #[tokio::test]
    async fn canonical_and_embedded_identity_matrix() {
        use alloy_consensus::Header;
        use alloy_primitives::{Address, Bytes, U64};
        use alloy_provider::ProviderBuilder;
        use alloy_transport::mock::Asserter;
        use eez_protocol::l2::{DerivationConfig, GenesisL1Reference};

        use crate::derivation::{DerivationError, DerivationSource};

        let genesis = alloy_genesis::Genesis {
            config: alloy_genesis::ChainConfig {
                chain_id: 31337,
                ..Default::default()
            },
            timestamp: 1000,
            gas_limit: 30_000_000,
            mix_hash: B256::with_last_byte(7),
            ..Default::default()
        };
        let matching = ChainSpec::from_genesis(genesis.clone());
        let mut other_chain = genesis.clone();
        other_chain.config.chain_id = 31338;
        let mut other_genesis = genesis;
        other_genesis.extra_data = Bytes::from_static(b"other genesis");
        let specs = [
            matching,
            ChainSpec::from_genesis(other_chain),
            ChainSpec::from_genesis(other_genesis),
        ];
        let reference = &specs[0].genesis_header;
        let schedule = Schedule::new(
            DerivationConfig {
                fee_collector: Address::repeat_byte(1),
                block_interval_seconds: 1,
                sync_interval_blocks: 4,
                history_update_offset: 0,
                l1_reference: GenesisL1Reference {
                    chain_id: 31337,
                    number: 0,
                    hash: reference.hash(),
                    timestamp: reference.timestamp,
                    prev_randao: reference.mix_hash,
                },
            },
            &Header {
                timestamp: 1004,
                mix_hash: reference.mix_hash,
                ..Default::default()
            },
        )
        .unwrap();
        let expected_profile = offline_l1_rules(&specs[0])
            .unwrap()
            .profile(schedule, 90210)
            .unwrap();

        // These are the production profile/authentication components, not a
        // launched Composer or proof of the embedded database's ancestry.
        for (embedded, embedded_spec) in specs.iter().enumerate() {
            for (canonical, canonical_spec) in specs.iter().enumerate() {
                for pruned in [false, true] {
                    let case =
                        format!("embedded={embedded}, canonical={canonical}, pruned={pruned}");
                    let profile = offline_l1_rules(embedded_spec)
                        .unwrap()
                        .profile(schedule, 90210);
                    if embedded == 1 {
                        assert_eq!(
                            profile.unwrap_err().to_string(),
                            "L1 reference/profile chain mismatch",
                            "{case}"
                        );
                        continue;
                    }
                    let profile = profile.unwrap();
                    assert_eq!(
                        profile.commitment() == expected_profile.commitment(),
                        embedded == 0,
                        "same chain ID must not hide a different configured genesis: {case}"
                    );

                    let replies = Asserter::new();
                    replies.push_success(&U64::from(canonical_spec.chain().id()));
                    if canonical != 1 {
                        if pruned {
                            replies.push_failure(
                                serde_json::from_value(serde_json::json!({
                                    "code": 4444, "message": "pruned history unavailable"
                                }))
                                .unwrap(),
                            );
                            replies.push_success(&Bytes::from(alloy_rlp::encode(
                                canonical_spec.genesis_header.header(),
                            )));
                        } else {
                            let mut block: alloy_rpc_types_eth::Block = Default::default();
                            block.header.inner = canonical_spec.genesis_header.header().clone();
                            block.header.hash = canonical_spec.genesis_header.hash();
                            replies.push_success(&block);
                        }
                    }
                    let result = DerivationSource::authenticate(
                        ProviderBuilder::new().connect_mocked_client(replies.clone()),
                        schedule,
                    )
                    .await;
                    match canonical {
                        0 => assert!(result.is_ok(), "{case}: {result:?}"),
                        1 => assert!(
                            matches!(
                                result,
                                Err(DerivationError::Mismatch(
                                    "genesis reference chain identity"
                                ))
                            ),
                            "{case}: {result:?}"
                        ),
                        2 => assert!(
                            matches!(
                                result,
                                Err(DerivationError::Mismatch("L1 genesis reference header"))
                            ),
                            "{case}: {result:?}"
                        ),
                        _ => unreachable!(),
                    }
                    assert!(replies.read_q().is_empty(), "{case}");
                }
            }
        }
    }

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
