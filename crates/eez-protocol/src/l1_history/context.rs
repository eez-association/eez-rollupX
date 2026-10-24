//! Exact manager profile and context encoding. Encoded context is conditional on
//! the manager returning these same values at inclusion; it is not authorization.

use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use alloy_sol_types::{SolType as _, SolValue as _, sol, sol_data};

use super::{
    EvidenceAnchor, HISTORY_ADDRESS, IMPORT_GAS_LIMIT, MAX_IMPORT_HEADERS, MAX_RULE_EPOCHS,
};
use crate::l2::ProfileError;

sol! {
    /// Canonical profile preimage mirrored by OfflineRollup.Profile. Genesis
    /// binds bootstrap storage/runtime; the remaining fields bind execution
    /// configuration that is not itself part of the genesis block header.
    #[derive(Debug, PartialEq, Eq)]
    struct OfflineProfile {
        uint256 l1ChainId;
        uint256 l2ChainId;
        bytes32 genesisHash;
        uint64 genesisTimestamp;
        uint64 blockInterval;
        uint32 syncInterval;
        uint32 updateOffset;
        bytes32 l1ConfigHash;
        bytes32 historyCodeHash;
        address feeCollector;
        uint64[] ruleTimes;
    }

    /// Atomic fresh-network manager initialization, shared with deployment tools.
    function initializeOffline(address initialOwner, uint256 initialThreshold,
        address[] proofSystems, bytes32[] vkeys, OfflineProfile profile) external;
}

fn valid_rule_epochs(times: &[u64]) -> bool {
    times.first() == Some(&0)
        && times.len() <= MAX_RULE_EPOCHS
        && times.windows(2).all(|pair| pair[0] < pair[1])
}

impl OfflineProfile {
    /// Check the complete shape against independently loaded chain/code identity.
    /// The caller additionally checks bootstrap and the canonical L1 schedule.
    pub fn validate(&self, l1_chain_id: U256, history_code_hash: B256) -> Result<(), ProfileError> {
        crate::l2::validate_fee_collector(self.feeCollector)?;
        if self.l1ChainId.is_zero()
            || self.l1ChainId != l1_chain_id
            || self.l2ChainId.is_zero()
            || self.genesisHash.is_zero()
            || self.l1ConfigHash.is_zero()
            || self.historyCodeHash != history_code_hash
            || history_code_hash.is_zero()
            || !valid_rule_epochs(&self.ruleTimes)
        {
            return Err(ProfileError("invalid offline network profile"));
        }
        super::HistorySchedule::new(
            self.genesisTimestamp,
            self.blockInterval,
            self.syncInterval,
            self.updateOffset,
        )?;
        Ok(())
    }

    /// Bytes published in OfflineProfileInitialized; no JSON formatting or
    /// arbitrary operator label participates in this commitment.
    pub fn canonical_bytes(&self) -> Bytes {
        (
            keccak256("EEZ_ROLLUPX_OFFLINE_PROFILE"),
            HISTORY_ADDRESS,
            MAX_IMPORT_HEADERS as u64,
            IMPORT_GAS_LIMIT,
            self.clone(),
        )
            .abi_encode_params()
            .into()
    }

    /// On-chain protocolConfigHash, after validating the profile and its inputs.
    pub fn commitment(&self) -> B256 {
        keccak256(self.canonical_bytes())
    }

    /// Epoch-specific identity, including transitions that change only blob rules.
    pub fn rules_id(&self, timestamp: u64) -> Result<B256, ProfileError> {
        if !valid_rule_epochs(&self.ruleTimes) || self.l1ConfigHash.is_zero() {
            return Err(ProfileError("invalid L1 rule epochs"));
        }
        let index = self.ruleTimes.partition_point(|&at| at <= timestamp) - 1;
        Ok(keccak256(
            (
                keccak256("EEZ_ROLLUPX_L1_RULES_V1"),
                self.l1ConfigHash,
                self.ruleTimes[index],
            )
                .abi_encode(),
        ))
    }

    /// Public follower check after inclusion. The offline prover cannot know a
    /// future inclusion header: the manager enforces these same constraints there.
    pub fn validate_inclusion(
        &self,
        anchor: EvidenceAnchor,
        endpoint: &alloy_consensus::Header,
        inclusion: &alloy_consensus::Header,
    ) -> Result<(), ProfileError> {
        let (number, hash, rules_timestamp) = match anchor {
            EvidenceAnchor::ActualParent {
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
                (number, hash, target_timestamp)
            }
            EvidenceAnchor::Historical { number, hash } => {
                if number == 0
                    || number == u64::MAX
                    || inclusion
                        .number
                        .checked_sub(number)
                        .is_none_or(|age| !(1..=256).contains(&age))
                {
                    return Err(ProfileError("historical inclusion anchor unavailable"));
                }
                (number, hash, endpoint.timestamp)
            }
        };
        if hash.is_zero()
            || number != endpoint.number
            || hash != endpoint.hash_slow()
            || self.rules_id(rules_timestamp)? != self.rules_id(inclusion.timestamp)?
        {
            return Err(ProfileError(
                "inclusion endpoint or L1 rule period mismatch",
            ));
        }
        Ok(())
    }
}

/// Complete conditional context used by all public-input producers/consumers.
/// Construct after checking the L1 endpoint and exact accepted L2 commitment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfflineContext {
    l1_chain_id: U256,
    registry: Address,
    manager: Address,
    rollup_id: u64,
    l2_chain_id: U256,
    profile_hash: B256,
    rules_id: B256,
    current_l2_root: B256,
    anchor: EvidenceAnchor,
}

impl OfflineContext {
    /// Assemble checked-shape context. `anchor_timestamp` must be decoded from
    /// the checked L1 endpoint; accepted-root and deployment authorization are
    /// established by the manager comparison, not by this constructor.
    pub fn new(
        profile: &OfflineProfile,
        registry: Address,
        manager: Address,
        rollup_id: u64,
        current_l2_root: B256,
        anchor: EvidenceAnchor,
        anchor_timestamp: u64,
    ) -> Result<Self, ProfileError> {
        profile.validate(profile.l1ChainId, profile.historyCodeHash)?;
        if registry.is_zero() || manager.is_zero() || rollup_id == 0 || current_l2_root.is_zero() {
            return Err(ProfileError("invalid offline deployment or accepted root"));
        }
        let rules_time = match anchor {
            EvidenceAnchor::ActualParent {
                number,
                hash,
                target_timestamp,
            } => {
                if number == u64::MAX || hash.is_zero() || target_timestamp <= anchor_timestamp {
                    return Err(ProfileError("invalid actual-parent context"));
                }
                target_timestamp
            }
            EvidenceAnchor::Historical { number, hash } => {
                if number == 0 || number == u64::MAX || hash.is_zero() {
                    return Err(ProfileError("invalid historical context"));
                }
                anchor_timestamp
            }
        };
        Ok(Self {
            l1_chain_id: profile.l1ChainId,
            registry,
            manager,
            rollup_id,
            l2_chain_id: profile.l2ChainId,
            profile_hash: profile.commitment(),
            rules_id: profile.rules_id(rules_time)?,
            current_l2_root,
            anchor,
        })
    }

    /// Selector required in this exact settlement ABI; mode substitution rejects.
    pub const fn selector(&self) -> u64 {
        match self.anchor {
            EvidenceAnchor::ActualParent { .. } => u64::MAX,
            EvidenceAnchor::Historical { number, .. } => number,
        }
    }

    /// Native occurrence context for this exact live target. Historical proofs
    /// cannot contain cross-chain actions and therefore have no such context.
    pub fn live_source_context(
        &self,
    ) -> Result<crate::native_source::LiveSourceContext, ProfileError> {
        let EvidenceAnchor::ActualParent {
            hash,
            target_timestamp,
            ..
        } = self.anchor
        else {
            return Err(ProfileError(
                "historical context cannot authorize native cross-chain actions",
            ));
        };
        crate::native_source::LiveSourceContext::new(
            self.l1_chain_id
                .try_into()
                .map_err(|_| ProfileError("unsupported L1 chain id"))?,
            self.registry,
            crate::RollupId(self.rollup_id),
            self.l2_chain_id
                .try_into()
                .map_err(|_| ProfileError("unsupported L2 chain id"))?,
            target_timestamp,
            hash,
        )
    }

    /// Exact flat Solidity ABI tuple returned by OfflineRollup.getCustomData.
    pub fn custom_data(&self) -> Bytes {
        let mut encoded = (
            keccak256("EEZ_ROLLUPX_L1_HISTORY_V1"),
            self.l1_chain_id,
            self.registry,
            self.manager,
            self.rollup_id,
            self.l2_chain_id,
            self.profile_hash,
            self.rules_id,
            self.current_l2_root,
        )
            .abi_encode();
        // All fields are static ABI words. Concatenating these two static
        // tuples is the specified flat tuple, with no bytes wrapper or offset.
        match self.anchor {
            EvidenceAnchor::ActualParent {
                number,
                hash,
                target_timestamp,
            } => {
                type Tail = (
                    sol_data::Uint<8>,
                    sol_data::Uint<64>,
                    sol_data::FixedBytes<32>,
                    sol_data::Uint<64>,
                );
                encoded.extend(Tail::abi_encode(&(1, number, hash, target_timestamp)));
            }
            EvidenceAnchor::Historical { number, hash } => {
                type Tail = (
                    sol_data::Uint<8>,
                    sol_data::Uint<64>,
                    sol_data::FixedBytes<32>,
                );
                encoded.extend(Tail::abi_encode(&(2, number, hash)));
            }
        }
        encoded.into()
    }

    /// Registry rollup whose actual accepted commitment is bound by the context.
    pub const fn rollup_id(&self) -> u64 {
        self.rollup_id
    }
    /// Accepted block-hash commitment; a state-root or height match cannot substitute.
    pub const fn current_l2_root(&self) -> B256 {
        self.current_l2_root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> OfflineProfile {
        OfflineProfile {
            feeCollector: alloy_primitives::address!("f39fd6e51aad88f6f4ce6ab8827279cfffb92266"),
            l1ChainId: U256::from(1),
            l2ChainId: U256::from(900),
            genesisHash: B256::repeat_byte(1),
            genesisTimestamp: 120,
            blockInterval: 2,
            syncInterval: 6,
            updateOffset: 2,
            l1ConfigHash: B256::repeat_byte(2),
            historyCodeHash: B256::repeat_byte(3),
            ruleTimes: vec![0, 144],
        }
    }

    #[test]
    fn profile_encoding_matches_solidity_argument_list() {
        let p = profile();
        let word = |value: u64| U256::from(value).to_be_bytes::<32>();
        let mut address = [0; 32];
        address[12..].copy_from_slice(HISTORY_ADDRESS.as_slice());
        // abi.encode(domain, address, limit, gas, Profile): five outer
        // arguments, followed by the dynamic struct and its rule-time array.
        let expected = [
            keccak256("EEZ_ROLLUPX_OFFLINE_PROFILE").0,
            address,
            word(64),
            word(2_000_000),
            word(5 * 32),
            word(1),
            word(900),
            p.genesisHash.0,
            word(120),
            word(2),
            word(6),
            word(2),
            p.l1ConfigHash.0,
            p.historyCodeHash.0,
            p.feeCollector.into_word().0,
            word(11 * 32),
            word(2),
            word(0),
            word(144),
        ]
        .concat();
        assert_eq!(p.canonical_bytes().as_ref(), expected);
    }

    #[test]
    fn profile_matches_independent_abi_and_keccak_vectors() {
        let corpus: serde_json::Value = serde_json::from_str(include_str!(
            "../../../eez-control-rpc/conformance/native-profile-v1.json"
        ))
        .unwrap();
        for (id, collector) in [("profile-collector-a", 0x11), ("profile-collector-b", 0x12)] {
            let profile = OfflineProfile {
                l1ChainId: U256::from(1),
                l2ChainId: U256::from(901),
                genesisHash: B256::repeat_byte(0x33),
                genesisTimestamp: 120,
                blockInterval: 2,
                syncInterval: 6,
                updateOffset: 2,
                l1ConfigHash: B256::repeat_byte(0x55),
                historyCodeHash: B256::repeat_byte(0x44),
                feeCollector: Address::repeat_byte(collector),
                ruleTimes: vec![0, 1000],
            };
            profile
                .validate(U256::from(1), profile.historyCodeHash)
                .unwrap();
            let vector = corpus["vectors"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["id"] == id)
                .unwrap();
            assert_eq!(
                profile.canonical_bytes(),
                vector["expectedBytes"]
                    .as_str()
                    .unwrap()
                    .parse::<Bytes>()
                    .unwrap()
            );
            assert_eq!(
                profile.commitment(),
                vector["expectedKeccak256"]
                    .as_str()
                    .unwrap()
                    .parse::<B256>()
                    .unwrap()
            );
        }
    }

    #[test]
    fn actual_and_historical_have_flat_distinct_encodings() {
        let profile = profile();
        let registry = Address::repeat_byte(0x11);
        let manager = Address::repeat_byte(0x22);
        let root = B256::repeat_byte(0x33);
        let hash = B256::repeat_byte(0x44);
        for anchor in [
            EvidenceAnchor::ActualParent {
                number: 100,
                hash,
                target_timestamp: 132,
            },
            EvidenceAnchor::Historical { number: 100, hash },
        ] {
            let context =
                OfflineContext::new(&profile, registry, manager, 7, root, anchor, 120).unwrap();
            let actual = context.custom_data();
            let word = |value: u64| U256::from(value).to_be_bytes::<32>();
            let address_word = |value: Address| {
                let mut w = [0; 32];
                w[12..].copy_from_slice(value.as_slice());
                w
            };
            let mut independent = Vec::new();
            for w in [
                keccak256("EEZ_ROLLUPX_L1_HISTORY_V1").0,
                word(1),
                address_word(registry),
                address_word(manager),
                word(7),
                word(900),
                profile.commitment().0,
                profile.rules_id(120).unwrap().0,
                root.0,
            ] {
                independent.extend(w);
            }
            match anchor {
                EvidenceAnchor::ActualParent { .. } => {
                    independent.extend(word(1));
                    independent.extend(word(100));
                    independent.extend(hash.0);
                    independent.extend(word(132));
                }
                EvidenceAnchor::Historical { .. } => {
                    independent.extend(word(2));
                    independent.extend(word(100));
                    independent.extend(hash.0);
                }
            }
            assert_eq!(actual.as_ref(), independent);
            assert_eq!(
                context.selector(),
                if actual.len() == 13 * 32 {
                    u64::MAX
                } else {
                    100
                }
            );
        }
    }

    #[test]
    fn historical_reuse_is_bound_to_rule_epoch_and_root() {
        let profile = profile();
        assert_eq!(
            profile.rules_id(120).unwrap(),
            profile.rules_id(143).unwrap()
        );
        assert_ne!(
            profile.rules_id(143).unwrap(),
            profile.rules_id(144).unwrap()
        );
        let make = |root, timestamp| {
            OfflineContext::new(
                &profile,
                Address::repeat_byte(1),
                Address::repeat_byte(2),
                1,
                root,
                EvidenceAnchor::Historical {
                    number: 100,
                    hash: B256::repeat_byte(4),
                },
                timestamp,
            )
            .unwrap()
            .custom_data()
        };
        assert_eq!(
            make(B256::repeat_byte(3), 120),
            make(B256::repeat_byte(3), 143)
        );
        assert_ne!(
            make(B256::repeat_byte(3), 120),
            make(B256::repeat_byte(3), 144)
        );
        assert_ne!(
            make(B256::repeat_byte(3), 120),
            make(B256::repeat_byte(5), 120)
        );
    }

    #[test]
    fn profile_and_rules_id_enforce_the_same_epoch_boundaries() {
        let mut p = profile();
        for times in [
            vec![],
            vec![1],
            vec![0, 1, 1],
            vec![0, 2, 1],
            (0..=MAX_RULE_EPOCHS as u64).collect(),
        ] {
            p.ruleTimes = times;
            assert!(p.validate(U256::from(1), p.historyCodeHash).is_err());
            assert!(p.rules_id(u64::MAX).is_err());
        }
        for times in [vec![0], (0..MAX_RULE_EPOCHS as u64).collect()] {
            p.ruleTimes = times;
            p.validate(U256::from(1), p.historyCodeHash).unwrap();
            assert!(p.rules_id(0).is_ok());
            assert!(p.rules_id(u64::MAX).is_ok());
        }
    }

    #[test]
    fn invalid_profile_and_reserved_modes_reject() {
        let mut p = profile();
        p.validate(U256::from(1), p.historyCodeHash).unwrap();
        p.ruleTimes = vec![0, 120, 120];
        assert!(p.validate(U256::from(1), p.historyCodeHash).is_err());
        for number in [0, u64::MAX] {
            assert!(
                OfflineContext::new(
                    &profile(),
                    Address::repeat_byte(1),
                    Address::repeat_byte(2),
                    1,
                    B256::repeat_byte(3),
                    EvidenceAnchor::Historical {
                        number,
                        hash: B256::repeat_byte(4)
                    },
                    120
                )
                .is_err()
            );
        }
    }

    #[test]
    fn public_replay_enforces_inclusion_age_target_and_rule_epoch() {
        let p = profile();
        let endpoint = alloy_consensus::Header {
            number: 100,
            timestamp: 120,
            ..Default::default()
        };
        let hash = endpoint.hash_slow();
        let historical = EvidenceAnchor::Historical { number: 100, hash };
        for (age, valid) in [(0, false), (1, true), (256, true), (257, false)] {
            let inclusion = alloy_consensus::Header {
                number: 100 + age,
                timestamp: 132,
                parent_hash: hash,
                ..Default::default()
            };
            assert_eq!(
                p.validate_inclusion(historical, &endpoint, &inclusion)
                    .is_ok(),
                valid
            );
        }
        let mut inclusion = alloy_consensus::Header {
            number: 101,
            timestamp: 132,
            parent_hash: hash,
            ..Default::default()
        };
        let actual = EvidenceAnchor::ActualParent {
            number: 100,
            hash,
            target_timestamp: 132,
        };
        p.validate_inclusion(actual, &endpoint, &inclusion).unwrap();
        inclusion.timestamp = 133;
        assert!(p.validate_inclusion(actual, &endpoint, &inclusion).is_err());
        p.validate_inclusion(historical, &endpoint, &inclusion)
            .unwrap();
        inclusion.timestamp = 144;
        assert!(
            p.validate_inclusion(historical, &endpoint, &inclusion)
                .is_err()
        );
        inclusion.timestamp = 132;
        inclusion.parent_hash = B256::repeat_byte(9);
        assert!(p.validate_inclusion(actual, &endpoint, &inclusion).is_err());
        for number in [0, 101, 102, u64::MAX] {
            assert!(
                p.validate_inclusion(
                    EvidenceAnchor::Historical { number, hash },
                    &endpoint,
                    &inclusion
                )
                .is_err()
            );
        }
        assert!(
            p.validate_inclusion(
                EvidenceAnchor::Historical {
                    number: 100,
                    hash: B256::ZERO
                },
                &endpoint,
                &inclusion
            )
            .is_err()
        );
    }
}
