//! Explicit replay inputs for the new-network v3 format.

use eez_payload_codec::derivable::Environment;
use reth_ethereum_primitives::Block;
use reth_evm::NextBlockEnvAttributes;
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::SealedHeader;

use crate::error::{DeriverError, DeriverResult};

#[derive(Clone, Copy, Debug)]
pub(super) enum ExecutionInputs<'a> {
    Derived(&'a Environment),
    Full(&'a Block),
}

impl<'a> ExecutionInputs<'a> {
    pub(super) const fn full_template(self) -> Option<&'a Block> {
        match self {
            Self::Full(block) => Some(block),
            Self::Derived(_) => None,
        }
    }

    pub(super) fn attributes(
        self,
        config: &EthEvmConfig,
        parent: &SealedHeader,
    ) -> DeriverResult<NextBlockEnvAttributes> {
        match self {
            Self::Derived(environment) => {
                eez_driver::derivation::next_attributes(config, parent, environment)
                    .map_err(DeriverError::l2_provider)
            }
            Self::Full(block) => eez_driver::derivation::full_attributes(config, parent, block)
                .map_err(DeriverError::l2_provider),
        }
    }

    /// Reuse only an already executed local block with the same actual parent
    /// and complete inputs. Transaction equality is checked by the caller.
    pub(super) fn matches_local(
        self,
        config: &EthEvmConfig,
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
        if let Self::Full(template) = self {
            if !template
                .body
                .transactions
                .starts_with(&block.body.transactions)
            {
                return Ok(false);
            }
            if template.body.transactions.len() == block.body.transactions.len() {
                return Ok(template.header.hash_slow() == block.header.hash_slow());
            }
            // Same prefix restrictions as full-template execution. A partial
            // terminal must not silently apply post-block withdrawal effects.
            if template
                .body
                .withdrawals
                .as_ref()
                .is_some_and(|w| !w.is_empty())
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{EMPTY_ROOT_HASH, Header};
    use alloy_primitives::{Address, B256, Bytes};
    use reth_chainspec::ChainSpecBuilder;
    use std::sync::Arc;

    fn fixture() -> (EthEvmConfig, SealedHeader, Environment, Block) {
        let config = EthEvmConfig::new(Arc::new(
            ChainSpecBuilder::mainnet()
                .reset()
                .cancun_activated()
                .build(),
        ));
        let parent = SealedHeader::seal_slow(Header {
            number: 10,
            timestamp: 100,
            gas_limit: 30_000_000,
            base_fee_per_gas: Some(1_000_000_000),
            withdrawals_root: Some(EMPTY_ROOT_HASH),
            parent_beacon_block_root: Some(B256::ZERO),
            blob_gas_used: Some(0),
            excess_blob_gas: Some(0),
            ..Default::default()
        });
        let env = Environment {
            timestamp_step_seconds: 2,
            fee_recipient: Address::repeat_byte(0x11),
            prev_randao: B256::repeat_byte(0x22),
            gas_limit: 30_000_000,
            extra_data: Bytes::from_static(b"published"),
            cancun_parent_beacon_root: B256::repeat_byte(0x33),
        };
        // Independent input/header fixture, without using the shared assembler.
        let block = Block {
            header: Header {
                parent_hash: parent.hash(),
                number: 11,
                timestamp: 102,
                gas_limit: env.gas_limit,
                beneficiary: env.fee_recipient,
                mix_hash: env.prev_randao,
                extra_data: env.extra_data.clone(),
                base_fee_per_gas: Some(875_000_000),
                withdrawals_root: Some(EMPTY_ROOT_HASH),
                parent_beacon_block_root: Some(env.cancun_parent_beacon_root),
                blob_gas_used: Some(0),
                excess_blob_gas: Some(0),
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
            |h| h.parent_beacon_block_root = Some(B256::ZERO),
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

    #[test]
    fn full_terminal_reuse_binds_complete_hash_and_canonical_parent() {
        let (config, parent, _, block) = fixture();
        assert!(
            ExecutionInputs::Full(&block)
                .matches_local(&config, &parent, &block)
                .unwrap()
        );
        let mut other = block.clone();
        other.header.state_root = B256::repeat_byte(0xa1);
        assert!(
            !ExecutionInputs::Full(&block)
                .matches_local(&config, &parent, &other)
                .unwrap()
        );
        let mut wrong_parent = parent.header().clone();
        wrong_parent.extra_data = Bytes::from_static(b"alternate ancestry");
        let wrong_parent = SealedHeader::seal_slow(wrong_parent);
        assert!(
            ExecutionInputs::Full(&block)
                .attributes(&config, &wrong_parent)
                .is_err()
        );
    }
}
