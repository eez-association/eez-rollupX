//! Startup settings passed to the composer after validation. Environment
//! access belongs at the launcher boundary, before nodes or workers start.

use std::collections::HashMap;

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
    pub(super) gas_trace: Option<super::gas_trace::Config>,
    pub(super) emission: super::EmissionLimits,
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
        let format_max = eez_payload_codec::derivable::MAX_BLOCKS as u64;
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
        let max_speculative_depth = policy.max_speculative_depth;
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
            gas_trace,
            emission,
            max_speculative_depth,
        })
    }

    pub const fn max_speculative_depth(&self) -> u64 {
        self.max_speculative_depth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing() -> RollupTiming {
        RollupTiming::new(12_000, 1_000, 2_000, 100)
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
    fn invalid_timing_fails_before_configuration_can_reach_runtime() {
        assert!(
            ComposerSettings::parse(RollupTiming::new(0, 1_000, 2_000, 100), |_| panic!(
                "invalid timing must be rejected before reading settings"
            ))
            .is_err()
        );
    }

    #[test]
    fn operational_defaults_have_no_implicit_blob_or_gas_cap() {
        let config = settings(&[]).unwrap();
        assert_eq!(config.max_blobs_per_batch, None);
        assert_eq!(config.max_speculative_depth(), 0);
        assert_eq!(config.emission.max_blocks, 65_536);
        assert_eq!(config.emission.max_gas, u64::MAX);
        let fees = config.posting_fees;
        assert_eq!(fees.priority_override, None);
        assert_eq!(fees.priority_max, 2_000_000_000);
        assert_eq!(fees.gas_margin_bps, 500);
        assert_eq!(fees.declared_gas(100_000, 105_000).unwrap(), 105_000);
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
}
