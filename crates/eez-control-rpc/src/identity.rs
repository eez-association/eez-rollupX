//! Validation namespace shared by the composer and signer.
//!
//! This is a compatibility check, never authorization to sign or trust a block.
//! Resource settings are deliberately absent. Changes to validation semantics
//! require a validation-contract version bump even when the wire stays fixed.

use std::num::NonZeroU64;

use alloy_primitives::{Address, B256, Bytes, keccak256};
use alloy_rlp::RlpEncodable;

/// One contract version for identity encoding, JSON canonicalization and accepted
/// execution/settlement semantics. These rules cannot be selected independently.
/// See VALIDATION_IDENTITY.md; implementation/build metadata never participates.
pub const VALIDATION_VERSION: u64 = 7;

/// Trusted local inputs, obtained from the same chain document as execution.
#[derive(Debug, Clone, Copy)]
pub struct ValidationContext {
    pub rollup_id: NonZeroU64,
    pub chain_id: u64,
    pub system_address: Address,
    pub eez_l2_address: Address,
    pub chain_document_digest: B256,
    /// Exact network profile committed by the registered vkey, including L1 rules.
    pub network_profile_hash: B256,
    pub registry: Address,
    pub manager: Address,
}

impl ValidationContext {
    /// Domain-separated Keccak of the canonical RLP identity tuple. No signing
    /// key, quota, transport endpoint or caller-supplied block participates.
    #[must_use]
    pub fn identity(self) -> B256 {
        keccak256(self.preimage(VALIDATION_VERSION))
    }

    fn preimage(self, version: u64) -> Vec<u8> {
        #[derive(RlpEncodable)]
        struct Identity {
            domain: Bytes,
            version: u64,
            rollup_id: u64,
            chain_id: u64,
            system_address: Address,
            eez_l2_address: Address,
            chain_document_digest: B256,
            network_profile_hash: B256,
            registry: Address,
            manager: Address,
        }
        alloy_rlp::encode(Identity {
            domain: Bytes::from_static(b"eez.validation-identity"),
            version,
            rollup_id: self.rollup_id.get(),
            chain_id: self.chain_id,
            system_address: self.system_address,
            eez_l2_address: self.eez_l2_address,
            chain_document_digest: self.chain_document_digest,
            network_profile_hash: self.network_profile_hash,
            registry: self.registry,
            manager: self.manager,
        })
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
            network_profile_hash: B256::repeat_byte(0x55),
            registry: Address::repeat_byte(0x66),
            manager: Address::repeat_byte(0x77),
        }
    }

    #[test]
    fn portable_identity_vectors() {
        let vectors: serde_json::Value =
            serde_json::from_str(include_str!("../conformance/validation-identity.json")).unwrap();
        assert_eq!(vectors["validation_version"], VALIDATION_VERSION);
        assert_eq!(vectors["identities"].as_array().unwrap().len(), 3);
        for vector in vectors["identities"].as_array().unwrap() {
            let context = ValidationContext {
                rollup_id: vector["rollup_id"].as_str().unwrap().parse().unwrap(),
                chain_id: vector["chain_id"].as_str().unwrap().parse().unwrap(),
                system_address: vector["system_address"].as_str().unwrap().parse().unwrap(),
                eez_l2_address: vector["eez_l2_address"].as_str().unwrap().parse().unwrap(),
                chain_document_digest: vector["chain_document_digest"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap(),
                network_profile_hash: vector["network_profile_hash"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap(),
                registry: vector["registry"].as_str().unwrap().parse().unwrap(),
                manager: vector["manager"].as_str().unwrap().parse().unwrap(),
            };
            assert_eq!(
                context.preimage(VALIDATION_VERSION),
                hex::decode(vector["rlp"].as_str().unwrap().strip_prefix("0x").unwrap()).unwrap()
            );
            assert_eq!(
                context.identity(),
                vector["identity"]
                    .as_str()
                    .unwrap()
                    .parse::<B256>()
                    .unwrap(),
                "{}",
                vector["name"]
            );
        }
    }

    #[test]
    fn every_execution_identity_component_changes_the_namespace() {
        let original = context();
        let identity = original.identity();
        for changed in [
            ValidationContext {
                network_profile_hash: B256::ZERO,
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
        ] {
            assert_ne!(identity, changed.identity());
        }
        assert_ne!(
            identity,
            keccak256(original.preimage(VALIDATION_VERSION + 1))
        );
    }

    #[test]
    fn rust_dependency_changes_require_a_semantics_review() {
        // This conservative guard is local review evidence, never a wire input.
        // For a lockfile change, review execution/decoding impacts; bump the
        // protocol version if semantics changed, otherwise document equivalence
        // and update only this digest. Never copy build metadata into identity().
        assert_eq!(
            keccak256(include_bytes!("../../../Cargo.lock")),
            alloy_primitives::b256!(
                "ef9c10b8e56657b4aabc920048ef94f09093fb6db20da8f775dfb221d090fa8e"
            ),
            "review dependency semantics and VALIDATION_VERSION before updating this guard"
        );
    }
}
