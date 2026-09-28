//! Apply the fixed L2 rules to the exact operator-supplied genesis document.

use alloy_eips::eip2935::{HISTORY_STORAGE_ADDRESS, HISTORY_STORAGE_CODE};
use alloy_genesis::Genesis;
use eez_protocol::l2::{
    BASE_FEE_PARAMS, GENESIS_BASE_FEE, ProfileError, Schedule, validate_header,
};
use reth_chainspec::{ChainSpec, EthChainSpec};

/// Decode the one required genesis derivation profile. There is no legacy or
/// environment-derived fallback; reference authenticity is checked through L1.
pub fn schedule(
    spec: &impl EthChainSpec<Header = alloy_consensus::Header>,
) -> Result<Schedule, ProfileError> {
    let supplied = spec
        .genesis()
        .config
        .extra_fields
        .get("rollupX")
        .ok_or(ProfileError("missing config.rollupX derivation parameters"))?;
    let config = serde_json::from_value(supplied.clone())
        .map_err(|_| ProfileError("invalid config.rollupX derivation parameters"))?;
    Schedule::new(config, spec.genesis_header())
}

pub(crate) fn validate_next(
    parent: &alloy_consensus::Header,
    attributes: &reth_evm::NextBlockEnvAttributes,
) -> Result<(), ProfileError> {
    validate_header(parent)?;
    if parent.number.checked_add(1).is_none() || attributes.timestamp <= parent.timestamp {
        return Err(ProfileError(
            "block number overflow or non-increasing timestamp",
        ));
    }
    if attributes.gas_limit != eez_protocol::l2::GAS_LIMIT
        || attributes.parent_beacon_block_root != Some(alloy_primitives::B256::ZERO)
        || !attributes
            .withdrawals
            .as_ref()
            .is_some_and(|w| w.is_empty())
        || attributes.extra_data.len() > 32
        || attributes.slot_number.is_some()
    {
        return Err(ProfileError("noncanonical next-block attributes"));
    }
    Ok(())
}

pub(crate) fn validate_payload(
    data: &alloy_rpc_types_engine::ExecutionData,
) -> Result<(), ProfileError> {
    let payload = &data.payload;
    if payload.gas_limit() != eez_protocol::l2::GAS_LIMIT
        || data.parent_beacon_block_root() != Some(alloy_primitives::B256::ZERO)
        || data.sidecar.requests_hash() != Some(alloy_eips::eip7685::EMPTY_REQUESTS_HASH)
        || !data.withdrawals().is_some_and(Vec::is_empty)
        || payload.blob_gas_used() != Some(0)
        || payload.excess_blob_gas() != Some(0)
        || payload.slot_number().is_some()
        || payload.block_access_list().is_some()
        || payload.as_v1().extra_data.len() > 32
        || u64::try_from(payload.as_v1().base_fee_per_gas).is_err()
    {
        return Err(ProfileError("noncanonical Engine payload fields"));
    }
    Ok(())
}

/// Construct the L2 specification used by consensus, execution and replay.
/// Reth's generic genesis conversion always installs Ethereum's `2/8` fee
/// parameters; RollupX instead has the fixed `2/50` rule. No supplied configurable
/// fee rule is overwritten. Invalid genesis fields are rejected, not normalized.
///
/// This constructor is never used for L1 Ethereum or Gnosis configuration.
pub fn chain_spec(genesis: Genesis) -> Result<ChainSpec, ProfileError> {
    let config = &genesis.config;
    if config.osaka_time != Some(0)
        || config.amsterdam_time.is_some()
        || config.bogota_time.is_some()
    {
        return Err(ProfileError(
            "Osaka must be active from genesis; later execution forks are unsupported",
        ));
    }
    // Osaka's EVM selection alone does not activate the historical consensus
    // checks. In particular, sender recovery still consults Homestead for the
    // low-s rule, and transaction validation consults the earlier typed forks.
    if [
        config.homestead_block,
        config.eip150_block,
        config.eip155_block,
        config.eip158_block,
        config.byzantium_block,
        config.constantinople_block,
        config.petersburg_block,
        config.istanbul_block,
        config.berlin_block,
        config.london_block,
        config.merge_netsplit_block,
        config.shanghai_time,
        config.cancun_time,
        config.prague_time,
    ]
    .iter()
    .any(|activation| *activation != Some(0))
        || config.terminal_total_difficulty != Some(alloy_primitives::U256::ZERO)
        || !config.terminal_total_difficulty_passed
    {
        return Err(ProfileError(
            "all pre-Osaka execution and consensus rules must be active at genesis",
        ));
    }
    if genesis.config.dao_fork_block.is_some() {
        return Err(ProfileError("DAO transition is not part of L2 execution"));
    }
    if genesis
        .base_fee_per_gas
        .is_some_and(|fee| fee != u128::from(GENESIS_BASE_FEE))
    {
        return Err(ProfileError("genesis base fee must be 1 gwei"));
    }
    let history = genesis
        .alloc
        .get(&HISTORY_STORAGE_ADDRESS)
        .ok_or(ProfileError("missing EIP-2935 history predeploy"))?;
    if history.code.as_ref() != Some(&HISTORY_STORAGE_CODE)
        || history.storage.as_ref().is_some_and(|s| !s.is_empty())
    {
        return Err(ProfileError("history predeploy code or initial storage"));
    }
    let mut spec = ChainSpec::from_genesis(genesis);
    validate_header(spec.genesis_header())?;
    schedule(&spec)?;
    spec.base_fee_params = BASE_FEE_PARAMS.into();
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip1559::BaseFeeParams;
    fn genesis() -> Genesis {
        serde_json::from_slice(include_bytes!("../../../genesis.json")).unwrap()
    }

    #[test]
    fn same_genesis_has_explicit_l2_fee_rules_without_changing_l1() {
        let genesis = genesis();
        let ethereum = ChainSpec::from_genesis(genesis.clone());
        let rollup = chain_spec(genesis.clone()).unwrap();
        assert_eq!(rollup.genesis, genesis);
        assert_eq!(rollup.base_fee_params_at_timestamp(0), BASE_FEE_PARAMS);
        assert_eq!(
            rollup.base_fee_params_at_timestamp(u64::MAX),
            BASE_FEE_PARAMS
        );
        assert_eq!(
            ethereum.base_fee_params_at_timestamp(0),
            BaseFeeParams::ethereum()
        );
        assert_eq!(
            rollup.genesis_header().base_fee_per_gas,
            Some(GENESIS_BASE_FEE)
        );
    }

    #[test]
    fn incompatible_supplied_genesis_is_rejected_not_repaired() {
        let mutations: &[fn(&mut Genesis)] = &[
            |g| g.gas_limit += 1,
            |g| g.base_fee_per_gas = Some(2),
            |g| g.config.osaka_time = Some(1),
            |g| g.config.amsterdam_time = Some(u64::MAX),
            |g| g.config.bogota_time = Some(u64::MAX),
            |g| g.config.homestead_block = Some(1),
            |g| g.config.homestead_block = None,
            |g| g.config.berlin_block = Some(1),
            |g| g.config.london_block = None,
            |g| g.config.shanghai_time = Some(1),
            |g| g.config.cancun_time = None,
            |g| g.config.prague_time = Some(1),
            |g| g.config.merge_netsplit_block = None,
            |g| g.config.terminal_total_difficulty = None,
            |g| g.config.terminal_total_difficulty_passed = false,
            |g| {
                g.config.extra_fields.remove("rollupX");
            },
            |g| {
                g.config
                    .extra_fields
                    .insert("rollupX".into(), serde_json::Value::Null);
            },
            |g| g.config.extra_fields.get_mut("rollupX").unwrap()["unknown"] = true.into(),
            |g| {
                g.config.extra_fields.get_mut("rollupX").unwrap()["blockIntervalSeconds"] = 0.into()
            },
            |g| g.mix_hash = alloy_primitives::B256::with_last_byte(1),
            |g| g.alloc.get_mut(&HISTORY_STORAGE_ADDRESS).unwrap().code = Some(Default::default()),
            |g| {
                g.alloc.remove(&HISTORY_STORAGE_ADDRESS);
            },
        ];
        for mutation in mutations {
            let mut invalid = genesis();
            mutation(&mut invalid);
            assert!(chain_spec(invalid).is_err());
        }
    }
}
