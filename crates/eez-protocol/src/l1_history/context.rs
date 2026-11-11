//! Network-bound verification keys and the original Core Rollup anchor context.

use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use alloy_sol_types::{SolValue as _, sol};

use super::{
    EvidenceAnchor, HISTORY_ADDRESS, HISTORY_RUNTIME, IMPORT_GAS_LIMIT, MAX_IMPORT_HEADERS,
};
use crate::l2::ProfileError;

sol! {
    /// Execution configuration committed by the registered verification key.
    /// Genesis binds bootstrap storage/runtime; the remaining fields bind the
    /// independently loaded execution and L1 header rules.
    #[derive(Debug, PartialEq, Eq)]
    struct NetworkProfile {
        uint256 l1ChainId;
        uint256 l2ChainId;
        bytes32 genesisHash;
        uint64 genesisTimestamp;
        uint64 blockInterval;
        uint32 syncInterval;
        uint32 updateOffset;
        bytes32 l1ConfigHash;
        address feeCollector;
    }

    /// Original Core Rollup initializer.
    function initialize(address initialOwner, uint256 initialThreshold,
        address[] proofSystems, bytes32[] vkeys) external;
}

impl NetworkProfile {
    /// Validate the profile against the independently loaded L1 chain identity.
    pub fn validate(&self, l1_chain_id: U256) -> Result<(), ProfileError> {
        crate::l2::validate_fee_collector(self.feeCollector)?;
        if self.l1ChainId.is_zero()
            || self.l1ChainId != l1_chain_id
            || self.l2ChainId.is_zero()
            || self.genesisHash.is_zero()
            || self.l1ConfigHash.is_zero()
        {
            return Err(ProfileError("invalid network profile"));
        }
        super::HistorySchedule::new(
            self.genesisTimestamp,
            self.blockInterval,
            self.syncInterval,
            self.updateOffset,
        )?;
        Ok(())
    }

    /// Static Solidity ABI tuple used by deployment and independent provers.
    pub fn canonical_bytes(&self) -> Bytes {
        (
            keccak256("EEZ_ROLLUPX_NETWORK_PROFILE_V1"),
            // Preserve the deployed signing domain for compatible extensions.
            // RPC validation capabilities have their own version; changing them
            // must not revoke otherwise valid signatures. Change this domain only
            // with an explicit incompatible-rule/key migration.
            7u64,
            HISTORY_ADDRESS,
            MAX_IMPORT_HEADERS as u64,
            IMPORT_GAS_LIMIT,
            keccak256(HISTORY_RUNTIME),
            self.clone(),
        )
            .abi_encode_params()
            .into()
    }

    /// Commitment to the local validation program and its network configuration.
    pub fn commitment(&self) -> B256 {
        keccak256(self.canonical_bytes())
    }

    /// Key registered with Core Rollup and included in Core's signed public inputs.
    /// Core additionally binds the rollup ID. The manager address is omitted to
    /// avoid a circular dependency with its CREATE2 initialization payload.
    pub fn verification_key(
        &self,
        registry: Address,
        proof_system: Address,
        attester: Address,
    ) -> B256 {
        keccak256(
            (
                keccak256("EEZ_ROLLUPX_VERIFICATION_KEY_V1"),
                self.commitment(),
                registry,
                proof_system,
                attester,
            )
                .abi_encode(),
        )
    }
}

impl EvidenceAnchor {
    /// Verify the Core Rollup anchor against actual inclusion. Header layouts are
    /// checked under their own L1 rules, not by requiring equal rule periods.
    pub fn validate_inclusion(
        self,
        endpoint: &alloy_consensus::Header,
        inclusion: &alloy_consensus::Header,
    ) -> Result<(), ProfileError> {
        let (number, hash) = match self {
            Self::ActualParent {
                number,
                hash,
                target_timestamp,
            } => {
                if number.checked_add(1) != Some(inclusion.number)
                    || hash != inclusion.parent_hash
                    || target_timestamp != inclusion.timestamp
                    || target_timestamp <= endpoint.timestamp
                {
                    return Err(ProfileError("actual-parent inclusion mismatch"));
                }
                (number, hash)
            }
            Self::Historical { number, hash } => {
                if number == 0
                    || number == u64::MAX
                    || inclusion
                        .number
                        .checked_sub(number)
                        .is_none_or(|age| !(1..=256).contains(&age))
                {
                    return Err(ProfileError("historical inclusion anchor unavailable"));
                }
                (number, hash)
            }
        };
        if hash.is_zero() || number != endpoint.number || hash != endpoint.hash_slow() {
            return Err(ProfileError("inclusion endpoint mismatch"));
        }
        Ok(())
    }
}

/// Conditional Core Rollup context. The checked header chain authenticates the
/// anchor when Core independently obtains the same hash at inclusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProofContext {
    rollup_id: u64,
    l2_chain_id: U256,
    anchor: EvidenceAnchor,
}

impl ProofContext {
    /// `anchor_timestamp` comes from the independently checked endpoint header.
    pub fn new(
        l2_chain_id: U256,
        rollup_id: u64,
        anchor: EvidenceAnchor,
        anchor_timestamp: u64,
    ) -> Result<Self, ProfileError> {
        if l2_chain_id.is_zero() || rollup_id == 0 {
            return Err(ProfileError("invalid proof network identity"));
        }
        match anchor {
            EvidenceAnchor::ActualParent {
                number,
                hash,
                target_timestamp,
            } => {
                if number == u64::MAX || hash.is_zero() || target_timestamp <= anchor_timestamp {
                    return Err(ProfileError("invalid actual-parent context"));
                }
            }
            EvidenceAnchor::Historical { number, hash } => {
                if number == 0 || number == u64::MAX || hash.is_zero() {
                    return Err(ProfileError("invalid historical context"));
                }
            }
        }
        Ok(Self {
            rollup_id,
            l2_chain_id,
            anchor,
        })
    }

    /// Exact settlement selector; zero is not a supported proving mode.
    pub const fn selector(&self) -> u64 {
        match self.anchor {
            EvidenceAnchor::ActualParent { .. } => u64::MAX,
            EvidenceAnchor::Historical { number, .. } => number,
        }
    }

    /// Native occurrence context for a live target. Historical proofs cannot
    /// contain cross-chain actions.
    pub fn live_source_context(
        &self,
    ) -> Result<crate::native_source::LiveSourceContext, ProfileError> {
        let EvidenceAnchor::ActualParent { hash, .. } = self.anchor else {
            return Err(ProfileError(
                "historical context cannot authorize native cross-chain actions",
            ));
        };
        let chain_id = u64::try_from(self.l2_chain_id)
            .map_err(|_| ProfileError("native EVM chain ID exceeds uint64"))?;
        crate::native_source::LiveSourceContext::new(chain_id, hash)
    }

    /// Original Rollup.getCustomData: abi.encode(uint256 timestamp, bytes32 hash).
    pub fn custom_data(&self) -> Bytes {
        let (timestamp, hash) = match self.anchor {
            EvidenceAnchor::ActualParent {
                hash,
                target_timestamp,
                ..
            } => (target_timestamp, hash),
            EvidenceAnchor::Historical { hash, .. } => (0, hash),
        };
        (U256::from(timestamp), hash).abi_encode().into()
    }

    /// Rollup selected by this proof and Core's proof-system accumulator.
    pub const fn rollup_id(&self) -> u64 {
        self.rollup_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> NetworkProfile {
        NetworkProfile {
            l1ChainId: U256::from(1),
            l2ChainId: U256::from(900),
            genesisHash: B256::repeat_byte(1),
            genesisTimestamp: 120,
            blockInterval: 2,
            syncInterval: 6,
            updateOffset: 2,
            l1ConfigHash: B256::repeat_byte(2),
            feeCollector: alloy_primitives::address!("f39fd6e51aad88f6f4ce6ab8827279cfffb92266"),
        }
    }

    #[test]
    fn profile_and_key_match_independent_portable_vectors() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../eez-control-rpc/conformance/native-profile-v1.json"
        ))
        .unwrap();
        let vectors = corpus["vectors"].as_array().unwrap();
        let mut profiles = Vec::new();
        for vector in vectors
            .iter()
            .filter(|v| v["id"].as_str().unwrap().starts_with("profile-"))
        {
            let bytes =
                alloy_primitives::hex::decode(vector["expectedBytes"].as_str().unwrap()).unwrap();
            let profile = NetworkProfile::abi_decode(&bytes[6 * 32..]).unwrap();
            profile.validate(profile.l1ChainId).unwrap();
            assert_eq!(profile.canonical_bytes().as_ref(), bytes);
            assert_eq!(
                profile.commitment(),
                vector["expectedKeccak256"]
                    .as_str()
                    .unwrap()
                    .parse::<B256>()
                    .unwrap()
            );
            profiles.push(profile);
        }
        assert_eq!(profiles.len(), 2);
        let mut count = 0;
        for vector in vectors
            .iter()
            .filter(|v| v["id"].as_str().unwrap().starts_with("verification-key-"))
        {
            let args = &vector["arguments"];
            let hash = args[1].as_str().unwrap().parse::<B256>().unwrap();
            let profile = profiles.iter().find(|p| p.commitment() == hash).unwrap();
            let address = |index| args[index].as_str().unwrap().parse::<Address>().unwrap();
            assert_eq!(
                profile.verification_key(address(2), address(3), address(4)),
                vector["expectedKeccak256"]
                    .as_str()
                    .unwrap()
                    .parse::<B256>()
                    .unwrap()
            );
            count += 1;
        }
        assert_eq!(count, 5);
    }

    #[test]
    fn verification_key_binds_network_program_and_deployment() {
        let p = profile();
        p.validate(U256::from(1)).unwrap();
        let registry = Address::repeat_byte(3);
        let system = Address::repeat_byte(4);
        let signer = Address::repeat_byte(5);
        let key = p.verification_key(registry, system, signer);
        for changed in [
            NetworkProfile {
                l1ChainId: U256::from(2),
                ..p.clone()
            },
            NetworkProfile {
                l2ChainId: U256::from(901),
                ..p.clone()
            },
            NetworkProfile {
                genesisHash: B256::repeat_byte(6),
                ..p.clone()
            },
            NetworkProfile {
                l1ConfigHash: B256::repeat_byte(7),
                ..p.clone()
            },
            NetworkProfile {
                feeCollector: Address::repeat_byte(8),
                ..p.clone()
            },
        ] {
            assert_ne!(changed.verification_key(registry, system, signer), key);
        }
        assert_ne!(p.verification_key(Address::ZERO, system, signer), key);
        assert_ne!(p.verification_key(registry, Address::ZERO, signer), key);
        assert_ne!(p.verification_key(registry, system, Address::ZERO), key);
        assert_eq!(p.canonical_bytes().len(), 15 * 32);
        assert!(p.validate(U256::from(2)).is_err());
    }

    #[test]
    fn custom_data_is_exact_core_pair_and_modes_remain_distinct() {
        let hash = B256::repeat_byte(9);
        let actual = ProofContext::new(
            U256::from(900),
            7,
            EvidenceAnchor::ActualParent {
                number: 42,
                hash,
                target_timestamp: 144,
            },
            132,
        )
        .unwrap();
        let historical = ProofContext::new(
            U256::from(900),
            7,
            EvidenceAnchor::Historical { number: 42, hash },
            132,
        )
        .unwrap();
        assert_eq!(
            actual.custom_data().as_ref(),
            (U256::from(144), hash).abi_encode()
        );
        assert_eq!(
            historical.custom_data().as_ref(),
            (U256::ZERO, hash).abi_encode()
        );
        assert_eq!(actual.selector(), u64::MAX);
        assert_eq!(historical.selector(), 42);
        assert!(historical.live_source_context().is_err());
        for number in [0, u64::MAX] {
            assert!(
                ProofContext::new(
                    U256::from(900),
                    7,
                    EvidenceAnchor::Historical { number, hash },
                    132
                )
                .is_err()
            );
        }
    }

    #[test]
    fn inclusion_authenticates_endpoint_and_core_blockhash_window() {
        let endpoint = alloy_consensus::Header {
            number: 42,
            timestamp: 132,
            ..Default::default()
        };
        let hash = endpoint.hash_slow();
        let anchor = EvidenceAnchor::Historical { number: 42, hash };
        // A known fork transition between these timestamps does not expire a
        // historical proof. Each header is checked under its own layout rules.
        for age in [1, 256] {
            anchor
                .validate_inclusion(
                    &endpoint,
                    &alloy_consensus::Header {
                        number: 42 + age,
                        timestamp: 10_000,
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        for number in [41, 42, 299] {
            assert!(
                anchor
                    .validate_inclusion(
                        &endpoint,
                        &alloy_consensus::Header {
                            number,
                            timestamp: 10_000,
                            ..Default::default()
                        }
                    )
                    .is_err()
            );
        }
        let actual = EvidenceAnchor::ActualParent {
            number: 42,
            hash,
            target_timestamp: 144,
        };
        let mut inclusion = alloy_consensus::Header {
            number: 43,
            parent_hash: hash,
            timestamp: 144,
            ..Default::default()
        };
        actual.validate_inclusion(&endpoint, &inclusion).unwrap();
        inclusion.timestamp += 1;
        assert!(actual.validate_inclusion(&endpoint, &inclusion).is_err());
        let wrong = EvidenceAnchor::Historical {
            number: 42,
            hash: B256::repeat_byte(1),
        };
        assert!(wrong.validate_inclusion(&endpoint, &inclusion).is_err());
    }
}
