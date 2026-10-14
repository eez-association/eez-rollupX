//! Startup settings passed to the composer after validation. Environment
//! access belongs at the launcher boundary, before nodes or workers start.

use std::collections::HashMap;

use alloy_primitives::Address;
use eez_driver::RollupTiming;
use eez_l1::posting_fees::{PostingFeeConfig, PostingFeeError};

use crate::settlement_policy::{PolicyConfig, PolicyError};

#[derive(Debug, Clone)]
pub struct ComposerSettings {
    pub(super) timing: RollupTiming,
    pub(super) policy: PolicyConfig,
    pub(super) max_blobs_per_batch: Option<u64>,
    /// `None` selects automatic resource/time admission. Numeric settings keep
    /// the existing operator count limit, including the unset default of 50.
    pub(super) max_user_txs_per_bundle: Option<usize>,
    pub(super) posting_fees: PostingFeeConfig,
    pub(super) enforce_l2_fee_tariff: bool,
    pub(super) shadow_fee_policy: Option<super::fees::pricing::Policy>,
    pub(super) gas_trace: Option<super::gas_trace::Config>,
    pub(super) emission: super::EmissionLimits,
    /// Coinbase of every L2 block this node builds, Live and Sync alike, so
    /// priority fees are not split across recipients.
    pub(super) fee_recipient: Address,
    /// Destination of the postBatch transaction (`EEZ_L1_POSTBATCH_TARGET`).
    /// `None` posts directly to the registry. A wrapper must expose EEZ's
    /// `postAndVerifyBatch` with identical calldata; registry reads never use it.
    pub(super) post_batch_target: Option<Address>,
    max_speculative_depth: u64,
}

impl ComposerSettings {
    /// Intersect local emission policy with the signer's advertised logical
    /// ceiling before runtime construction. Never round upward past its quota.
    pub fn with_prover_block_limit(mut self, maximum: u64) -> Result<Self, PolicyError> {
        let maximum = maximum.min(self.emission.max_blocks);
        if maximum < u64::from(self.timing.k()) {
            return Err(PolicyError::Config(
                "prover block limit cannot fit one complete Sync interval".into(),
            ));
        }
        self.emission.max_blocks = maximum;
        Ok(self)
    }

    pub fn from_env(timing: RollupTiming) -> Result<Self, PolicyError> {
        Self::parse(timing, |name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(_) => Err(PolicyError::Config(format!(
                "{name} contains non-UTF-8 bytes"
            ))),
        })
    }

    /// Parse one immutable startup snapshot. A shared setting is read from its
    /// source once even when both the policy and sequencer validate/use it.
    pub fn parse(
        timing: RollupTiming,
        mut read: impl FnMut(&str) -> Result<Option<String>, PolicyError>,
    ) -> Result<Self, PolicyError> {
        timing
            .validate()
            .map_err(|error| PolicyError::Config(error.to_string()))?;
        if timing.proof_time() + timing.submission_slack() + crate::prover_retry::POST_PROOF_RESERVE
            >= timing
                .l1_block_time()
                .saturating_sub(timing.proof_window_open())
        {
            return Err(PolicyError::Config(
                "EEZ_SETTLEMENT_START_AFTER_BLOCK plus EEZ_SETTLEMENT_START_DELAY_MS leaves no preparation time after the proof seed, submission slack and post-proof reserve".into(),
            ));
        }
        let mut snapshot: HashMap<String, Option<String>> = HashMap::new();
        let mut get = |name: &str| {
            if let Some(value) = snapshot.get(name) {
                return Ok(value.clone());
            }
            let value = read(name)?;
            snapshot.insert(name.to_owned(), value.clone());
            Ok(value)
        };
        let policy = PolicyConfig::parse(&mut get, u64::from(timing.k()))?;
        let posting_fees = PostingFeeConfig::parse(|name| {
            get(name).map_err(|error| PostingFeeError::Config(error.to_string()))
        })
        .map_err(|error| PolicyError::Config(error.to_string()))?;
        let max_gas = match get("EEZ_MAX_POSTBATCH_GAS")? {
            None => u64::MAX,
            Some(value) => value
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| {
                    PolicyError::Config("EEZ_MAX_POSTBATCH_GAS must be a positive integer".into())
                })?,
        };
        let format_max = u64::from(u32::MAX);
        if u64::from(timing.k()) > format_max {
            return Err(PolicyError::Config(
                "Sync interval exceeds operations block ceiling".into(),
            ));
        }
        let max_blocks = format_max;
        let emission = super::EmissionLimits {
            timing,
            max_blocks,
            max_gas,
        };
        let max_blobs_per_batch = match get("EEZ_MAX_BLOBS_PER_BATCH")? {
            None => None,
            Some(value) => Some(
                value
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| {
                        PolicyError::Config(
                            "EEZ_MAX_BLOBS_PER_BATCH must be a positive integer".into(),
                        )
                    })?,
            ),
        };
        let max_user_txs_per_bundle = match get("EEZ_MAX_USER_TXS_PER_BUNDLE") {
            Ok(None) => Some(50),
            Ok(Some(value)) if value == "auto" => None,
            Ok(Some(value)) => match value.parse::<usize>().ok().filter(|value| *value > 0) {
                Some(limit) => Some(limit),
                None => {
                    return Err(PolicyError::Config(
                        "EEZ_MAX_USER_TXS_PER_BUNDLE must be auto or a positive integer".into(),
                    ));
                }
            },
            Err(error) => return Err(error),
        };
        // Unset keeps the historical zero coinbase; a supplied value must be a
        // valid address, so an empty or mistyped setting cannot silently fall back.
        let fee_recipient = match get("EEZ_L2_FEE_RECIPIENT")? {
            None => Address::ZERO,
            Some(value) => value.trim().parse::<Address>().map_err(|_| {
                PolicyError::Config("EEZ_L2_FEE_RECIPIENT must be a 20-byte hex address".into())
            })?,
        };
        // Unset posts to the registry. A supplied value must be a nonzero
        // address; whether it forwards to the registry is checked on L1 at startup.
        let post_batch_target = match get("EEZ_L1_POSTBATCH_TARGET")? {
            None => None,
            Some(value) => Some(
                value
                    .trim()
                    .parse::<Address>()
                    .ok()
                    .filter(|address| !address.is_zero())
                    .ok_or_else(|| {
                        PolicyError::Config(
                            "EEZ_L1_POSTBATCH_TARGET must be a nonzero 20-byte hex address".into(),
                        )
                    })?,
            ),
        };
        let max_speculative_depth = policy.max_speculative_depth;
        let shadow_fee_policy =
            super::fees::pricing::Policy::parse(get(super::fees::pricing::SETTING)?, fee_recipient)
                .map_err(PolicyError::Config)?;
        let enforce_l2_fee_tariff = match get("EEZ_L2_FEE_ENFORCE_TARIFF")?.as_deref() {
            None | Some("false") => false,
            Some("true") if shadow_fee_policy.is_some() => true,
            Some("true") => {
                return Err(PolicyError::Config(
                    "tariff enforcement requires EEZ_L2_FEE_SHADOW_POLICY".into(),
                ));
            }
            Some(_) => {
                return Err(PolicyError::Config(
                    "EEZ_L2_FEE_ENFORCE_TARIFF must be true or false".into(),
                ));
            }
        };
        let gas_trace = super::gas_trace::Config::parse_with_address(
            get("EEZ_COMPOSER_GAS_TRACE_TX_HASH")?,
            get("EEZ_COMPOSER_GAS_TRACE_ADDRESS")?,
            get("EEZ_COMPOSER_GAS_TRACE_DIR")?,
        )
        .map_err(PolicyError::Config)?;
        Ok(Self {
            timing,
            policy,
            max_blobs_per_batch,
            max_user_txs_per_bundle,
            posting_fees,
            enforce_l2_fee_tariff,
            shadow_fee_policy,
            gas_trace,
            emission,
            fee_recipient,
            post_batch_target,
            max_speculative_depth,
        })
    }

    pub const fn max_speculative_depth(&self) -> u64 {
        self.max_speculative_depth
    }

    /// Coinbase for Live and Sync blocks (`EEZ_L2_FEE_RECIPIENT`, default zero).
    pub const fn fee_recipient(&self) -> Address {
        self.fee_recipient
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_requires_positive_preparation_after_the_shared_post_reserve() {
        for (proof, delay) in [
            (749, 0),
            (750, 0),
            (751, 0),
            (449, 300),
            (450, 300),
            (451, 300),
        ] {
            // K=5/P=4: 1000ms left, 100ms slack, 150ms inclusive post reserve.
            let timing = RollupTiming::new(5000, 1000, proof, 100, 4).with_start_delay_ms(delay);
            let mut reached_policy = false;
            let result = ComposerSettings::parse(timing, |_| {
                reached_policy = true;
                Ok(None)
            });
            assert_eq!(reached_policy, proof + delay < 750);
            if proof + delay >= 750 {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("leaves no preparation time")
                );
            }
        }
    }

    fn timing() -> RollupTiming {
        RollupTiming::new(12_000, 1_000, 2_000, 100, 9)
    }

    fn settings(extra: &[(&str, &str)]) -> Result<ComposerSettings, PolicyError> {
        let mut values = HashMap::from([
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "always"),
            ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "150"),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "0"),
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "100000000000"),
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS", "100000000000"),
            ("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "2000000000"),
        ]);
        values.extend(extra.iter().copied());
        let mut reads = std::collections::HashSet::new();
        ComposerSettings::parse(timing(), |name| {
            assert!(
                reads.insert(name.to_owned()),
                "{name} read twice from configuration source"
            );
            Ok(values.get(name).map(|value| (*value).into()))
        })
    }

    #[test]
    fn negotiated_block_limit_reaches_emission_without_rounding_past_signer() {
        let k = u64::from(timing().k());
        let settings = settings(&[]).unwrap();
        assert_eq!(
            settings
                .clone()
                .with_prover_block_limit(65_536)
                .unwrap()
                .emission
                .max_blocks,
            65_536
        );
        let reduced = settings.clone().with_prover_block_limit(k * 2 + 1).unwrap();
        assert_eq!(reduced.emission.max_blocks, k * 2 + 1);
        assert!(settings.with_prover_block_limit(k - 1).is_err());
    }

    #[test]
    fn fee_recipient_defaults_to_zero_and_rejects_empty_or_malformed_values() {
        assert_eq!(settings(&[]).unwrap().fee_recipient(), Address::ZERO);
        let recipient = "0x2eA2A5A1fe1BA4F4134c993E0c9A700706031E52";
        assert_eq!(
            settings(&[("EEZ_L2_FEE_RECIPIENT", recipient)])
                .unwrap()
                .fee_recipient(),
            recipient.parse::<Address>().unwrap()
        );
        // An explicit zero address is a deliberate choice, not an error.
        assert_eq!(
            settings(&[(
                "EEZ_L2_FEE_RECIPIENT",
                "0x0000000000000000000000000000000000000000"
            )])
            .unwrap()
            .fee_recipient(),
            Address::ZERO
        );
        for value in [
            "",
            "0x",
            "0x1234",
            "not-an-address",
            "0xZZA2A5A1fe1BA4F4134c993E0c9A700706031E52",
        ] {
            assert!(
                settings(&[("EEZ_L2_FEE_RECIPIENT", value)]).is_err(),
                "{value:?} must be rejected"
            );
        }
    }

    #[test]
    fn invalid_timing_fails_before_configuration_can_reach_runtime() {
        assert!(
            ComposerSettings::parse(RollupTiming::new(0, 1_000, 2_000, 100, 1), |_| panic!(
                "invalid timing must be rejected before reading settings"
            ))
            .is_err()
        );
    }

    #[test]
    fn operational_defaults_have_no_implicit_blob_or_gas_cap() {
        let config = settings(&[]).unwrap();
        assert!(config.shadow_fee_policy.is_none());
        assert_eq!(config.max_blobs_per_batch, None);
        assert_eq!(config.max_speculative_depth(), 0);
        assert_eq!(config.emission.max_blocks, u64::from(u32::MAX));
        assert_eq!(config.emission.max_gas, u64::MAX);
        let fees = config.posting_fees;
        assert_eq!(fees.priority_override, None);
        assert_eq!(fees.priority_max, 2_000_000_000);
        assert_eq!(fees.gas_margin_bps, 500);
        assert_eq!(fees.declared_gas(100_000, 105_000).unwrap(), 105_000);
    }

    #[test]
    fn tariff_enforcement_is_explicit_and_requires_complete_policy() {
        assert!(!settings(&[]).unwrap().enforce_l2_fee_tariff);
        assert!(
            !settings(&[("EEZ_L2_FEE_ENFORCE_TARIFF", "false")])
                .unwrap()
                .enforce_l2_fee_tariff
        );
        for value in ["", "0", "1", "TRUE", "bad", "true"] {
            assert!(settings(&[("EEZ_L2_FEE_ENFORCE_TARIFF", value)]).is_err());
        }
        let policy = super::super::fees::pricing::tests::policy_json().to_string();
        let beneficiary = Address::repeat_byte(4).to_string();
        assert!(
            settings(&[
                ("EEZ_L2_FEE_ENFORCE_TARIFF", "true"),
                ("EEZ_L2_FEE_SHADOW_POLICY", &policy),
                ("EEZ_L2_FEE_RECIPIENT", &beneficiary)
            ])
            .unwrap()
            .enforce_l2_fee_tariff
        );
    }

    #[test]
    fn shadow_pricing_policy_is_an_optional_validated_startup_snapshot() {
        let raw = super::super::fees::pricing::tests::policy_json().to_string();
        let beneficiary = Address::repeat_byte(4).to_string();
        let config = settings(&[
            (super::super::fees::pricing::SETTING, &raw),
            ("EEZ_L2_FEE_RECIPIENT", &beneficiary),
        ])
        .unwrap();
        assert!(
            config
                .shadow_fee_policy
                .unwrap()
                .validate_network(1, 9, [1])
                .is_ok()
        );
        // A pricing policy cannot silently redirect the sequencer's beneficiary.
        assert!(settings(&[(super::super::fees::pricing::SETTING, &raw)]).is_err());
        for raw in ["", "null", "{}", "bad"] {
            assert!(settings(&[(super::super::fees::pricing::SETTING, raw)]).is_err());
        }
    }

    #[test]
    fn explicit_operator_and_admission_limits_are_validated_once_and_propagated() {
        let config = settings(&[
            ("EEZ_MAX_BLOBS_PER_BATCH", "10"),
            ("EEZ_MAX_USER_TXS_PER_BUNDLE", "3"),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "161"),
        ])
        .unwrap();
        assert_eq!(config.max_blobs_per_batch, Some(10));
        assert_eq!(config.max_user_txs_per_bundle, Some(3));
        assert_eq!(config.max_speculative_depth(), 161);
        for key in ["EEZ_MAX_BLOBS_PER_BATCH", "EEZ_MAX_USER_TXS_PER_BUNDLE"] {
            for value in ["0", "-1", "bad", "1.5", "18446744073709551616", ""] {
                assert!(settings(&[(key, value)]).is_err(), "{key}={value}");
            }
        }
    }

    #[test]
    fn automatic_admission_and_numeric_default_are_supported() {
        assert_eq!(settings(&[]).unwrap().max_user_txs_per_bundle, Some(50));
        assert_eq!(
            settings(&[("EEZ_MAX_USER_TXS_PER_BUNDLE", "auto")])
                .unwrap()
                .max_user_txs_per_bundle,
            None
        );
    }

    #[test]
    fn posting_prices_and_gas_caps_are_checked_before_runtime() {
        for (key, value) in [
            ("EEZ_MAX_POSTBATCH_GAS", "0"),
            ("EEZ_MAX_POSTBATCH_GAS", "invalid"),
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "0"),
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS", "0"),
            ("EEZ_L1_POSTBATCH_PRIORITY_FEE", "invalid"),
            ("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "100000000001"),
        ] {
            assert!(settings(&[(key, value)]).is_err(), "{key}={value}");
        }
        let config = settings(&[
            ("EEZ_MAX_POSTBATCH_GAS", "30000000"),
            ("EEZ_L1_POSTBATCH_PRIORITY_FEE", "3"),
            ("EEZ_L1_POSTBATCH_GAS_MARGIN_BPS", "1500"),
        ])
        .unwrap();
        assert_eq!(config.emission.max_gas, 30_000_000);
        let fees = config.posting_fees;
        assert_eq!(fees.priority_override, Some(3));
        assert_eq!(fees.gas_margin_bps, 1500);
    }

    #[test]
    fn selected_gas_trace_requires_one_selector_and_absolute_location() {
        assert!(settings(&[]).unwrap().gas_trace.is_none());
        assert!(
            settings(&[
                ("EEZ_COMPOSER_GAS_TRACE_TX_HASH", ""),
                ("EEZ_COMPOSER_GAS_TRACE_DIR", ""),
            ])
            .unwrap()
            .gas_trace
            .is_none()
        );
        let hash = alloy_primitives::B256::ZERO.to_string();
        let configured = settings(&[
            ("EEZ_COMPOSER_GAS_TRACE_TX_HASH", &hash),
            ("EEZ_COMPOSER_GAS_TRACE_DIR", "/tmp/gas-traces"),
        ])
        .unwrap();
        assert!(
            configured
                .gas_trace
                .unwrap()
                .selects(alloy_primitives::B256::ZERO)
        );
        let second = alloy_primitives::B256::with_last_byte(1);
        let selected = format!("{},{}", alloy_primitives::B256::ZERO, second);
        let both = settings(&[
            ("EEZ_COMPOSER_GAS_TRACE_TX_HASH", &selected),
            ("EEZ_COMPOSER_GAS_TRACE_DIR", "/tmp/gas-traces"),
        ])
        .unwrap()
        .gas_trace
        .unwrap();
        assert!(both.selects(alloy_primitives::B256::ZERO));
        assert!(both.selects(second));
        let address = alloy_primitives::Address::repeat_byte(0x42).to_string();
        let address_config = settings(&[
            ("EEZ_COMPOSER_GAS_TRACE_ADDRESS", &address),
            ("EEZ_COMPOSER_GAS_TRACE_DIR", "/tmp/gas-traces"),
        ])
        .unwrap()
        .gas_trace
        .unwrap();
        assert!(!address_config.selects(alloy_primitives::B256::ZERO));
        for invalid in [
            vec![("EEZ_COMPOSER_GAS_TRACE_TX_HASH", hash.as_str())],
            vec![("EEZ_COMPOSER_GAS_TRACE_ADDRESS", address.as_str())],
            vec![("EEZ_COMPOSER_GAS_TRACE_DIR", "/tmp/gas-traces")],
            vec![
                ("EEZ_COMPOSER_GAS_TRACE_TX_HASH", "invalid"),
                ("EEZ_COMPOSER_GAS_TRACE_DIR", "/tmp/gas-traces"),
            ],
            vec![
                ("EEZ_COMPOSER_GAS_TRACE_TX_HASH", hash.as_str()),
                ("EEZ_COMPOSER_GAS_TRACE_DIR", "relative"),
            ],
            vec![
                ("EEZ_COMPOSER_GAS_TRACE_ADDRESS", "invalid"),
                ("EEZ_COMPOSER_GAS_TRACE_DIR", "/tmp/gas-traces"),
            ],
            vec![
                ("EEZ_COMPOSER_GAS_TRACE_ADDRESS", address.as_str()),
                ("EEZ_COMPOSER_GAS_TRACE_DIR", "relative"),
            ],
            vec![
                ("EEZ_COMPOSER_GAS_TRACE_TX_HASH", hash.as_str()),
                ("EEZ_COMPOSER_GAS_TRACE_ADDRESS", address.as_str()),
                ("EEZ_COMPOSER_GAS_TRACE_DIR", "/tmp/gas-traces"),
            ],
        ] {
            assert!(settings(&invalid).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn post_batch_target_defaults_to_the_registry_and_rejects_unusable_values() {
        assert_eq!(settings(&[]).unwrap().post_batch_target, None);
        let target = "0x5f531064587c3e4D08145a6f564Ce06Bf33D3E84";
        assert_eq!(
            settings(&[("EEZ_L1_POSTBATCH_TARGET", &format!(" {target} "))])
                .unwrap()
                .post_batch_target,
            Some(target.parse().unwrap())
        );
        for value in [
            "",
            " ",
            "0x0000000000000000000000000000000000000000",
            "0x1234",
            "not-an-address",
        ] {
            let error = settings(&[("EEZ_L1_POSTBATCH_TARGET", value)]).unwrap_err();
            assert!(
                error.to_string().contains("EEZ_L1_POSTBATCH_TARGET"),
                "{value:?}: {error}"
            );
        }
    }
}
