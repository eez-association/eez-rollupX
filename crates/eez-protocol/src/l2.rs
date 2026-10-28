//! Fixed rules for the fresh-network RollupX L2 execution profile.
//!
//! These are consensus rules, not composer-selectable defaults. The surrounding
//! derivation context additionally binds timestamps, RANDAO, parent and action
//! authorization; validating this profile alone does not authenticate a block.

use alloy_consensus::{EMPTY_OMMER_ROOT_HASH, EMPTY_ROOT_HASH, Header};
use alloy_eips::{eip1559::BaseFeeParams, eip7685::EMPTY_REQUESTS_HASH};
use alloy_primitives::{B64, B256};

mod derivation;
pub use derivation::{
    AnchorKind, DerivationConfig, GenesisL1Reference, Schedule, validate_fee_collector,
};

/// The one portable validation-contract version, also bound by the registered
/// key's network profile. Mirrored by the wire-only RPC crate; independent
/// conformance vectors enforce agreement without runtime-role dependencies.
pub const VALIDATION_VERSION: u64 = 7;

/// Ordinary and native transactions share this metered block budget.
pub const GAS_LIMIT: u64 = 30_000_000;
/// Genesis base fee in wei. Later fees are derived from the actual parent.
pub const GENESIS_BASE_FEE: u64 = 1_000_000_000;
/// EIP-1559 denominator 50 and elasticity 2, independently of L1's parameters.
pub const BASE_FEE_PARAMS: BaseFeeParams = BaseFeeParams::new(50, 2);

/// A supplied header violates a fixed L2 execution rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("invalid RollupX L2 profile: {0}")]
pub struct ProfileError(pub &'static str);

/// Check fixed fields without rewriting invalid input. Execution determines the
/// roots and gas used; the parent and authenticated anchor determine dynamic
/// header fields. Those checks remain required at their respective boundaries.
pub fn validate_header(header: &Header) -> Result<(), ProfileError> {
    if header.gas_limit != GAS_LIMIT {
        return Err(ProfileError("gas limit must be 30,000,000"));
    }
    if header.ommers_hash != EMPTY_OMMER_ROOT_HASH
        || !header.difficulty.is_zero()
        || header.nonce != B64::ZERO
    {
        return Err(ProfileError("ommers, difficulty or nonce"));
    }
    if header.withdrawals_root != Some(EMPTY_ROOT_HASH) {
        return Err(ProfileError("withdrawals root must be present and empty"));
    }
    if header.parent_beacon_block_root != Some(B256::ZERO) {
        return Err(ProfileError(
            "parent beacon block root must be present and zero",
        ));
    }
    if header.requests_hash != Some(EMPTY_REQUESTS_HASH) {
        return Err(ProfileError("requests hash must be present and empty"));
    }
    // Both values start at zero, and no admitted L2 transaction consumes blob
    // gas. Standard parent-derived excess therefore stays zero by induction.
    if header.blob_gas_used != Some(0) || header.excess_blob_gas != Some(0) {
        return Err(ProfileError("blob gas and excess must be present and zero"));
    }
    if header.base_fee_per_gas.is_none() || header.extra_data.len() > 32 {
        return Err(ProfileError("missing base fee or oversized extra data"));
    }
    if header.slot_number.is_some() || header.block_access_list_hash.is_some() {
        return Err(ProfileError("unsupported post-Osaka header fields"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            gas_limit: GAS_LIMIT,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            withdrawals_root: Some(EMPTY_ROOT_HASH),
            parent_beacon_block_root: Some(B256::ZERO),
            requests_hash: Some(EMPTY_REQUESTS_HASH),
            blob_gas_used: Some(0),
            excess_blob_gas: Some(0),
            base_fee_per_gas: Some(GENESIS_BASE_FEE),
            ..Default::default()
        }
    }

    #[test]
    fn fixed_header_fields_are_rejected_without_normalization() {
        let valid = header();
        validate_header(&valid).unwrap();
        let changes: [fn(&mut Header); 14] = [
            |h| h.gas_limit += 1,
            |h| h.ommers_hash = B256::ZERO,
            |h| h.difficulty = alloy_primitives::U256::from(1),
            |h| h.nonce = B64::repeat_byte(1),
            |h| h.withdrawals_root = None,
            |h| h.withdrawals_root = Some(B256::ZERO),
            |h| h.parent_beacon_block_root = None,
            |h| h.parent_beacon_block_root = Some(B256::repeat_byte(1)),
            |h| h.requests_hash = Some(B256::ZERO),
            |h| h.blob_gas_used = Some(1),
            |h| h.excess_blob_gas = Some(1),
            |h| h.base_fee_per_gas = None,
            |h| h.extra_data = vec![0; 33].into(),
            |h| h.slot_number = Some(0),
        ];
        for change in changes {
            let mut invalid = valid.clone();
            change(&mut invalid);
            assert!(validate_header(&invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn independent_fee_vectors_use_two_fifty_and_ethereum_rounding() {
        for (used, parent_fee, next) in [
            (0, GENESIS_BASE_FEE, 980_000_000),
            (GAS_LIMIT / 2, GENESIS_BASE_FEE, GENESIS_BASE_FEE),
            (GAS_LIMIT, GENESIS_BASE_FEE, 1_020_000_000),
            (0, 49, 49),
            (GAS_LIMIT, 1, 2),
        ] {
            assert_eq!(
                BASE_FEE_PARAMS.next_block_base_fee(used, GAS_LIMIT, parent_fee),
                next
            );
        }
        assert_eq!(
            BaseFeeParams::ethereum().next_block_base_fee(0, GAS_LIMIT, GENESIS_BASE_FEE),
            875_000_000
        );
    }
}
