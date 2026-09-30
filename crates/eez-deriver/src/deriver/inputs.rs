//! Explicit replay inputs for the direct V0 format.

use eez_driver::derivation::Environment;
use eez_evm::EezEvmConfig;
use eez_primitives::Block;
use reth_evm::NextBlockEnvAttributes;
use reth_primitives_traits::SealedHeader;

use crate::error::{DeriverError, DeriverResult};

#[derive(Clone, Copy, Debug)]
pub(super) enum ExecutionInputs<'a> {
    Derived(&'a Environment),
}

impl ExecutionInputs<'_> {
    pub(super) fn attributes(
        self,
        config: &EezEvmConfig,
        parent: &SealedHeader,
    ) -> DeriverResult<NextBlockEnvAttributes> {
        match self {
            Self::Derived(environment) => {
                eez_driver::derivation::next_attributes(config, parent, environment)
                    .map_err(DeriverError::l2_provider)
            }
        }
    }

    /// Reuse only an already executed local block with the same actual parent
    /// and complete inputs. Transaction equality is checked by the caller.
    pub(super) fn matches_local(
        self,
        config: &EezEvmConfig,
        parent: &SealedHeader,
        block: &Block,
    ) -> DeriverResult<bool> {
        let expected = self.attributes(config, parent)?;
        let Ok(actual) = eez_driver::derivation::full_attributes(config, parent, block) else {
            return Ok(false);
        };
        if actual != expected {
            return Ok(false);
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{EMPTY_ROOT_HASH, Header};
    use alloy_primitives::{Address, B256, Bytes};
    use std::sync::Arc;

    fn fixture() -> (EezEvmConfig, SealedHeader, Environment, Block) {
        let mut genesis =
            eez_l1::chain_config::parse_l2_genesis(include_bytes!("../../../../genesis.json"))
                .unwrap()
                .into_genesis();
        genesis.timestamp = 80;
        let derivation = genesis.config.extra_fields.get_mut("rollupX").unwrap();
        derivation["blockIntervalSeconds"] = 2u64.into();
        derivation["syncIntervalBlocks"] = 6u64.into();
        derivation["l1Reference"]["timestamp"] = 80u64.into();
        let config = EezEvmConfig::new(Arc::new(eez_evm::profile::chain_spec(genesis).unwrap()));
        let parent = SealedHeader::seal_slow(Header {
            number: 10,
            timestamp: 100,
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1_000_000_000),
            withdrawals_root: Some(EMPTY_ROOT_HASH),
            parent_beacon_block_root: Some(B256::ZERO),
            blob_gas_used: Some(0),
            excess_blob_gas: Some(0),
            requests_hash: Some(alloy_eips::eip7685::EMPTY_REQUESTS_HASH),
            ..Default::default()
        });
        let env = Environment {
            fee_recipient: Address::repeat_byte(0x11),
            prev_randao: B256::repeat_byte(0x22),
            extra_data: Bytes::from_static(b"published"),
        };
        // Independent input/header fixture, without using the shared assembler.
        let block = Block {
            header: Header {
                parent_hash: parent.hash(),
                number: 11,
                timestamp: 102,
                gas_limit: eez_protocol::l2::GAS_LIMIT,
                beneficiary: env.fee_recipient,
                mix_hash: env.prev_randao,
                extra_data: env.extra_data.clone(),
                base_fee_per_gas: Some(980_000_000),
                withdrawals_root: Some(EMPTY_ROOT_HASH),
                parent_beacon_block_root: Some(B256::ZERO),
                blob_gas_used: Some(0),
                excess_blob_gas: Some(0),
                requests_hash: Some(alloy_eips::eip7685::EMPTY_REQUESTS_HASH),
                ..Default::default()
            },
            body: alloy_consensus::BlockBody {
                withdrawals: Some(Default::default()),
                ..Default::default()
            },
        };
        (config, parent, env, block)
    }

    #[test]
    fn published_inputs_supply_every_variable_execution_attribute() {
        let (config, parent, env, block) = fixture();
        let derived = ExecutionInputs::Derived(&env);
        let attrs = derived.attributes(&config, &parent).unwrap();
        assert_eq!(attrs.timestamp, block.header.timestamp);
        assert_eq!(attrs.suggested_fee_recipient, block.header.beneficiary);
        assert_eq!(attrs.prev_randao, block.header.mix_hash);
        assert_eq!(attrs.extra_data, block.header.extra_data);
        assert!(derived.matches_local(&config, &parent, &block).unwrap());
    }

    #[test]
    fn same_transactions_with_changed_environment_or_parent_are_not_reusable() {
        let (config, parent, env, block) = fixture();
        let changes: &[fn(&mut Header)] = &[
            |h| h.timestamp += 1,
            |h| h.beneficiary = Address::ZERO,
            |h| h.mix_hash = B256::ZERO,
            |h| h.gas_limit += 1,
            |h| h.extra_data = Bytes::new(),
            |h| h.parent_beacon_block_root = Some(B256::repeat_byte(1)),
            |h| h.parent_hash = B256::ZERO,
            |h| h.number += 1,
            |h| h.slot_number = Some(1),
            |h| h.block_access_list_hash = Some(B256::ZERO),
        ];
        for change in changes {
            let mut local = block.clone();
            change(&mut local.header);
            assert!(
                !ExecutionInputs::Derived(&env)
                    .matches_local(&config, &parent, &local)
                    .unwrap()
            );
        }
        let mut local = block;
        local.body.ommers.push(Header::default());
        assert!(
            !ExecutionInputs::Derived(&env)
                .matches_local(&config, &parent, &local)
                .unwrap()
        );
    }
}
