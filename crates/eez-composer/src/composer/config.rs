//! Startup settings passed to the composer after validation. Environment
//! access belongs at the launcher boundary, before nodes or workers start.

use std::collections::HashMap;

use eez_driver::{DEFAULT_MAX_SPECULATIVE_DEPTH, RollupTiming};
use eez_l1::posting_fees::{PostingFeeConfig, PostingFeeError};

use crate::settlement_policy::{PolicyConfig, PolicyError};

#[derive(Debug, Clone)]
pub struct ComposerSettings {
    pub(super) timing: RollupTiming,
    pub(super) policy: Option<PolicyConfig>,
    pub(super) max_blobs_per_batch: Option<u64>,
    /// `None` selects automatic resource/time admission. Numeric settings keep
    /// the existing operator count limit, including the unset default of 50.
    pub(super) max_user_txs_per_bundle: Option<usize>,
    pub(super) posting_fees: Option<PostingFeeConfig>,
    pub(super) emission: super::EmissionLimits,
    max_speculative_depth: u64,
}

impl ComposerSettings {
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
        let enabled = policy.is_some();
        let posting_fees = if enabled {
            Some(
                PostingFeeConfig::parse(|name| {
                    get(name).map_err(|error| PostingFeeError::Config(error.to_string()))
                })
                .map_err(|error| PolicyError::Config(error.to_string()))?,
            )
        } else {
            for name in [
                "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS",
                "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS",
                "EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE",
                "EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE",
                "EEZ_L1_POSTBATCH_PRIORITY_MARGIN_BPS",
                "EEZ_L1_POSTBATCH_EXECUTION_HEADROOM_BPS",
                "EEZ_L1_POSTBATCH_BLOB_HEADROOM_BPS",
                "EEZ_L1_POSTBATCH_GAS_MARGIN_BPS",
            ] {
                if get(name)?.is_some() {
                    return Err(PolicyError::Config(format!(
                        "{name} requires EEZ_SETTLEMENT_POLICY_ENABLED=true"
                    )));
                }
            }
            None
        };
        let requested_gas = get("EEZ_MAX_POSTBATCH_GAS");
        let max_gas = if enabled {
            match requested_gas? {
                None => u64::MAX,
                Some(value) => value
                    .parse::<u64>()
                    .ok()
                    .filter(|value| *value > 0)
                    .ok_or_else(|| {
                        PolicyError::Config(
                            "EEZ_MAX_POSTBATCH_GAS must be a positive integer".into(),
                        )
                    })?,
            }
        } else {
            super::clamp_max_postbatch_gas(
                requested_gas
                    .ok()
                    .flatten()
                    .and_then(|value| value.parse().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(super::DEFAULT_MAX_POSTBATCH_GAS),
            )
        };
        let max_blocks = if enabled {
            u64::MAX
        } else {
            super::grid_aligned_cap(
                get("EEZ_MAX_BLOCKS_PER_BATCH")
                    .ok()
                    .flatten()
                    .and_then(|value| value.parse().ok())
                    .filter(|value| *value > 0)
                    .unwrap_or(eez_driver::MAX_BLOCKS_PER_BATCH),
                u64::from(timing.k()),
            )
        };
        let emission = super::EmissionLimits {
            timing,
            max_blocks,
            max_gas,
        };
        let max_blobs_per_batch = match get("EEZ_MAX_BLOBS_PER_BATCH")? {
            None => None,
            Some(_) if !enabled => {
                return Err(PolicyError::Config(
                    "EEZ_MAX_BLOBS_PER_BATCH requires EEZ_SETTLEMENT_POLICY_ENABLED=true".into(),
                ));
            }
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
            Ok(Some(value)) if value == "auto" => {
                if !enabled {
                    return Err(PolicyError::Config(
                        "EEZ_MAX_USER_TXS_PER_BUNDLE=auto requires EEZ_SETTLEMENT_POLICY_ENABLED=true".into(),
                    ));
                }
                None
            }
            Ok(Some(value)) => match value.parse::<usize>().ok().filter(|value| *value > 0) {
                Some(limit) => Some(limit),
                None if !enabled => Some(50),
                None => {
                    return Err(PolicyError::Config(
                        "EEZ_MAX_USER_TXS_PER_BUNDLE must be auto or a positive integer".into(),
                    ));
                }
            },
            Err(_) if !enabled => Some(50),
            Err(error) => return Err(error),
        };
        // Enabled policy already validates this value and its N+K-1 bound.
        // Legacy mode retains its existing lenient default.
        let max_speculative_depth = get("EEZ_MAX_SPECULATIVE_DEPTH")
            .ok()
            .flatten()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_MAX_SPECULATIVE_DEPTH);
        Ok(Self {
            timing,
            policy,
            max_blobs_per_batch,
            max_user_txs_per_bundle,
            posting_fees,
            emission,
            max_speculative_depth,
        })
    }

    pub const fn max_speculative_depth(&self) -> u64 {
        self.max_speculative_depth
    }

    pub const fn settlement_policy_enabled(&self) -> bool {
        self.policy.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing() -> RollupTiming {
        RollupTiming::new(12_000, 1_000, 2_000, 100)
    }

    fn enabled(extra: &[(&str, &str)]) -> Result<ComposerSettings, PolicyError> {
        let mut values = HashMap::from([
            ("EEZ_SETTLEMENT_POLICY_ENABLED", "true"),
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "always"),
            ("EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS", "150"),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "0"),
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "100000000000"),
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS", "100000000000"),
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
    fn invalid_timing_fails_before_configuration_can_reach_runtime() {
        assert!(
            ComposerSettings::parse(RollupTiming::new(0, 1_000, 2_000, 100), |_| panic!(
                "invalid timing must be rejected before reading settings"
            ))
            .is_err()
        );
    }

    #[test]
    fn operational_defaults_preserve_legacy_and_have_no_implicit_blob_cap() {
        let config = ComposerSettings::parse(timing(), |_| Ok(None)).unwrap();
        assert!(config.policy.is_none());
        assert_eq!(config.max_user_txs_per_bundle, Some(50));
        assert_eq!(config.max_blobs_per_batch, None);
        assert_eq!(
            config.max_speculative_depth(),
            DEFAULT_MAX_SPECULATIVE_DEPTH
        );
        let config = enabled(&[]).unwrap();
        assert_eq!(config.max_blobs_per_batch, None);
        assert_eq!(config.max_speculative_depth(), 0);
        assert_eq!(config.emission.max_blocks, u64::MAX);
        assert_eq!(config.emission.max_gas, u64::MAX);
        let fees = config.posting_fees.unwrap();
        assert_eq!(fees.priority_override, None);
        assert_eq!(fees.priority_margin_bps, 0);
    }

    #[test]
    fn explicit_operator_and_admission_limits_are_validated_once_and_propagated() {
        let config = enabled(&[
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
                assert!(enabled(&[(key, value)]).is_err(), "{key}={value}");
            }
        }
    }

    #[test]
    fn disabled_blob_settings_fail_and_old_invalid_admission_retains_legacy_default() {
        assert!(
            ComposerSettings::parse(timing(), |name| Ok(
                (name == "EEZ_MAX_BLOBS_PER_BATCH").then(|| "2".into())
            ))
            .is_err()
        );
        let config = ComposerSettings::parse(timing(), |name| {
            Ok((name == "EEZ_MAX_USER_TXS_PER_BUNDLE").then(|| "0".into()))
        })
        .unwrap();
        assert_eq!(config.max_user_txs_per_bundle, Some(50));
    }

    #[test]
    fn automatic_admission_requires_network_aware_policy_and_preserves_numeric_defaults() {
        assert_eq!(enabled(&[]).unwrap().max_user_txs_per_bundle, Some(50));
        assert_eq!(
            enabled(&[("EEZ_MAX_USER_TXS_PER_BUNDLE", "auto")])
                .unwrap()
                .max_user_txs_per_bundle,
            None
        );
        assert!(
            ComposerSettings::parse(timing(), |name| {
                Ok((name == "EEZ_MAX_USER_TXS_PER_BUNDLE").then(|| "auto".into()))
            })
            .is_err()
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
            ("EEZ_L1_POSTBATCH_PRIORITY_MARGIN_BPS", "-1"),
            ("EEZ_L1_POSTBATCH_PRIORITY_MARGIN_BPS", "1.5"),
            (
                "EEZ_L1_POSTBATCH_PRIORITY_MARGIN_BPS",
                "18446744073709551616",
            ),
            ("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "100000000001"),
        ] {
            assert!(enabled(&[(key, value)]).is_err(), "{key}={value}");
        }
        let config = enabled(&[
            ("EEZ_MAX_POSTBATCH_GAS", "30000000"),
            ("EEZ_L1_POSTBATCH_PRIORITY_FEE", "3"),
            ("EEZ_L1_POSTBATCH_GAS_MARGIN_BPS", "1500"),
        ])
        .unwrap();
        assert_eq!(config.emission.max_gas, 30_000_000);
        let fees = config.posting_fees.unwrap();
        assert_eq!(fees.priority_override, Some(3));
        assert_eq!(fees.gas_margin_bps, 1500);
    }

    #[test]
    fn network_priority_margin_is_propagated_and_requires_enabled_policy() {
        let config = enabled(&[("EEZ_L1_POSTBATCH_PRIORITY_MARGIN_BPS", "2000")]).unwrap();
        let fees = config.posting_fees.unwrap();
        assert_eq!(fees.priority_override, None);
        assert_eq!(fees.priority_margin_bps, 2000);
        for value in ["0", "2000"] {
            assert!(
                ComposerSettings::parse(timing(), |name| Ok((name
                    == "EEZ_L1_POSTBATCH_PRIORITY_MARGIN_BPS")
                    .then(|| value.into())))
                .is_err()
            );
        }
    }
}
