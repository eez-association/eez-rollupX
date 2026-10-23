//! Validation namespace shared by the composer and signer.
//!
//! This is a compatibility check, never authorization to sign or trust a block.
//! Resource settings are deliberately absent. Changes to validation semantics
//! require a schema revision bump even when the wire and dependencies stay fixed.

use std::num::NonZeroU64;

use alloy_primitives::{Address, B256, Bytes, keccak256};
use alloy_rlp::RlpEncodable;

const IDENTITY_SCHEMA_VERSION: u64 = 5;
const VALIDATOR_REVISION: &[u8] = b"eez-offline-v2-validator-v1;operations=columns/count14/u32;history=all-produced/0xee77/64/2000000;context=flat-v1;anchors=actual-parent+historical;seed=delayed-origin;hook=eip2935+cleanup+import";
// A conservative fingerprint: every locked version/source/checksum participates,
// including Reth, Stateless, REVM, Alloy and the JSON implementation. This may
// invalidate reusable facts after an unrelated dependency change; it cannot
// silently reuse them after a validator dependency upgrade.
const DEPENDENCY_LOCK: &[u8] = include_bytes!("../../../Cargo.lock");

/// Trusted local inputs, obtained from the same chain document as execution.
#[derive(Debug, Clone, Copy)]
pub struct ValidationContext {
    pub rollup_id: NonZeroU64,
    pub chain_id: u64,
    pub system_address: Address,
    pub eez_l2_address: Address,
    pub chain_document_digest: B256,
    pub canonical_json_version: u64,
    /// Exact profile committed by the deployed manager, including L1 rules.
    pub offline_profile_hash: B256,
    pub registry: Address,
    pub manager: Address,
}

impl ValidationContext {
    /// Domain-separated Keccak of the canonical RLP identity tuple. No signing
    /// key, quota, transport endpoint or caller-supplied block participates.
    #[must_use]
    pub fn identity(self) -> B256 {
        self.identity_with_revision(VALIDATOR_REVISION, keccak256(DEPENDENCY_LOCK))
    }

    fn identity_with_revision(self, revision: &[u8], dependency_digest: B256) -> B256 {
        #[derive(RlpEncodable)]
        struct Identity {
            domain: Bytes,
            schema: u64,
            canonical_json_version: u64,
            rollup_id: u64,
            chain_id: u64,
            system_address: Address,
            eez_l2_address: Address,
            chain_document_digest: B256,
            offline_profile_hash: B256,
            registry: Address,
            manager: Address,
            validator_revision: Bytes,
            dependency_digest: B256,
        }
        keccak256(alloy_rlp::encode(Identity {
            domain: Bytes::from_static(b"eez.validation-identity"),
            schema: IDENTITY_SCHEMA_VERSION,
            canonical_json_version: self.canonical_json_version,
            rollup_id: self.rollup_id.get(),
            chain_id: self.chain_id,
            system_address: self.system_address,
            eez_l2_address: self.eez_l2_address,
            chain_document_digest: self.chain_document_digest,
            offline_profile_hash: self.offline_profile_hash,
            registry: self.registry,
            manager: self.manager,
            validator_revision: Bytes::copy_from_slice(revision),
            dependency_digest,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ValidationContext {
        ValidationContext {
            rollup_id: NonZeroU64::new(7).unwrap(),
            chain_id: 31337,
            system_address: Address::repeat_byte(0x11),
            eez_l2_address: Address::repeat_byte(0x22),
            chain_document_digest: B256::repeat_byte(0x33),
            canonical_json_version: 1,
            offline_profile_hash: B256::repeat_byte(0x55),
            registry: Address::repeat_byte(0x66),
            manager: Address::repeat_byte(0x77),
        }
    }

    #[test]
    fn every_execution_identity_component_changes_the_namespace() {
        let original = context();
        let identity = original.identity();
        for changed in [
            ValidationContext {
                offline_profile_hash: B256::ZERO,
                ..original
            },
            ValidationContext {
                registry: Address::ZERO,
                ..original
            },
            ValidationContext {
                manager: Address::ZERO,
                ..original
            },
            ValidationContext {
                rollup_id: NonZeroU64::new(8).unwrap(),
                ..original
            },
            ValidationContext {
                chain_id: 31338,
                ..original
            },
            ValidationContext {
                system_address: Address::repeat_byte(0x12),
                ..original
            },
            ValidationContext {
                eez_l2_address: Address::repeat_byte(0x23),
                ..original
            },
            ValidationContext {
                chain_document_digest: B256::repeat_byte(0x34),
                ..original
            },
            ValidationContext {
                canonical_json_version: 2,
                ..original
            },
        ] {
            assert_ne!(identity, changed.identity());
        }
        assert_ne!(
            identity,
            original.identity_with_revision(b"next", keccak256(DEPENDENCY_LOCK))
        );
        assert_ne!(
            identity,
            original.identity_with_revision(VALIDATOR_REVISION, B256::ZERO)
        );
    }

    #[test]
    fn identity_has_a_fixed_external_rlp_keccak_vector() {
        // Fixed revision/dependency inputs isolate the wire vector from future
        // legitimate lockfile changes. Expected hash is computed independently.
        let actual = context().identity_with_revision(b"fixture-v1", B256::repeat_byte(0x44));
        assert_eq!(
            actual,
            alloy_primitives::b256!(
                "fcb5c0a9a49e06e6b95842465d1ca5249362aa8dd690c614811556199d5dbece"
            )
        );
    }
}
