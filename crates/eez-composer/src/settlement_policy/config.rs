//! Startup parsing. Runtime decisions never read the process environment.

use std::num::{NonZeroU16, NonZeroU64};

use super::PolicyError;

const ENABLED: &str = "EEZ_SETTLEMENT_POLICY_ENABLED";
const MODE: &str = "EEZ_SETTLEMENT_PURE_L2_MODE";
const INTERVAL: &str = "EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS";
const GENERAL: &str = "EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS";
const FULLNESS: &str = "EEZ_SETTLEMENT_BLOB_FULLNESS_BPS";

/// Treatment of included ordinary L2 transactions, including reverted ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PureL2Mode {
    Always,
    Interval { milliseconds: NonZeroU64 },
}

/// Validated enabled policy. Absence of this configuration means legacy mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyConfig {
    pub pure_l2: PureL2Mode,
    pub max_unsettled_l2_blocks: NonZeroU64,
    pub blob_fullness_bps: Option<NonZeroU16>,
    /// Retained for the startup diagnostic, never used as an `always` timer.
    pub inactive_interval_ms: Option<NonZeroU64>,
}

impl PolicyConfig {
    /// Parse an environment snapshot without mutating the process environment.
    /// `k` is the validated number of L2 blocks per L1 slot.
    ///
    /// # Errors
    /// Invalid, contradictory or missing enabled settings, including a
    /// speculative-depth guard that cannot reach the general rule.
    pub fn parse(
        mut get: impl FnMut(&str) -> Result<Option<String>, PolicyError>,
        k: u64,
    ) -> Result<Option<Self>, PolicyError> {
        let enabled = match get(ENABLED)?.as_deref().map(str::trim) {
            None | Some("0" | "false" | "no" | "off" | "") => false,
            Some("1" | "true" | "yes" | "on") => true,
            Some(value) => {
                return Err(PolicyError::Config(format!(
                    "{ENABLED}: expected boolean, got {value:?}"
                )));
            }
        };
        let mode = get(MODE)?;
        let interval = get(INTERVAL)?;
        let general = get(GENERAL)?;
        let fullness = get(FULLNESS)?;
        if !enabled {
            if mode.is_some() || interval.is_some() || general.is_some() || fullness.is_some() {
                return Err(PolicyError::Config(format!(
                    "settlement policy settings require {ENABLED}=true"
                )));
            }
            return Ok(None);
        }
        if k < 2 {
            return Err(PolicyError::Config(
                "settlement policy requires validated K >= 2".into(),
            ));
        }
        if get("EEZ_MAX_BLOCKS_PER_BATCH")?.is_some() {
            return Err(PolicyError::Config(format!(
                "EEZ_MAX_BLOCKS_PER_BATCH is obsolete with the settlement policy; \
                 remove it and configure {GENERAL} (a trigger, not a packing cap)"
            )));
        }
        let interval = interval
            .as_deref()
            .map(|value| positive(INTERVAL, value))
            .transpose()?;
        let pure_l2 = match mode.as_deref() {
            Some("always") => PureL2Mode::Always,
            Some("interval") => PureL2Mode::Interval {
                milliseconds: interval.ok_or_else(|| required(INTERVAL))?,
            },
            _ => {
                return Err(PolicyError::Config(format!(
                    "{MODE}: required; expected always or interval"
                )));
            }
        };
        let max_unsettled_l2_blocks = positive(
            GENERAL,
            general.as_deref().ok_or_else(|| required(GENERAL))?,
        )?;
        let minimum_depth = max_unsettled_l2_blocks
            .get()
            .checked_add(k - 1)
            .ok_or(PolicyError::Arithmetic("N + K - 1"))?;
        let depth = get("EEZ_MAX_SPECULATIVE_DEPTH")?
            .map(|value| {
                value.parse::<u64>().map_err(|_| {
                    PolicyError::Config(
                        "EEZ_MAX_SPECULATIVE_DEPTH: expected a nonnegative integer".into(),
                    )
                })
            })
            .transpose()?
            .unwrap_or(eez_driver::DEFAULT_MAX_SPECULATIVE_DEPTH);
        if depth != 0 && depth < minimum_depth {
            return Err(PolicyError::Config(format!(
                "EEZ_MAX_SPECULATIVE_DEPTH must be 0 or at least {minimum_depth} \
                 (N + K - 1) for this settlement policy; configured {depth}"
            )));
        }
        let blob_fullness_bps = match fullness.as_deref() {
            None | Some("off") => None,
            Some(value) => {
                let value = value
                    .parse::<u16>()
                    .ok()
                    .and_then(NonZeroU16::new)
                    .filter(|value| value.get() <= 10_000)
                    .ok_or_else(|| {
                        PolicyError::Config(format!("{FULLNESS}: expected off or 1..=10000"))
                    })?;
                Some(value)
            }
        };
        Ok(Some(Self {
            pure_l2,
            max_unsettled_l2_blocks,
            blob_fullness_bps,
            inactive_interval_ms: if pure_l2 == PureL2Mode::Always {
                interval
            } else {
                None
            },
        }))
    }
}

fn required(name: &str) -> PolicyError {
    PolicyError::Config(format!("{name} is required when {ENABLED}=true"))
}

fn positive(name: &str, value: &str) -> Result<NonZeroU64, PolicyError> {
    value
        .parse::<u64>()
        .ok()
        .and_then(NonZeroU64::new)
        .ok_or_else(|| PolicyError::Config(format!("{name}: expected a positive integer")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(extra: &[(&str, &str)], k: u64) -> Result<Option<PolicyConfig>, PolicyError> {
        let mut vars = std::collections::HashMap::from([
            (ENABLED, "true"),
            (MODE, "always"),
            (GENERAL, "150"),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "0"),
        ]);
        vars.extend(extra.iter().copied());
        PolicyConfig::parse(
            |name| Ok(vars.get(name).map(|value| (*value).to_owned())),
            k,
        )
    }

    #[test]
    fn absent_settings_preserve_legacy_defaults_and_old_cap() {
        assert_eq!(PolicyConfig::parse(|_| Ok(None), 12).unwrap(), None);
        assert_eq!(
            PolicyConfig::parse(
                |name| Ok((name == "EEZ_MAX_BLOCKS_PER_BATCH").then(|| "300".into())),
                12
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn enabled_defaults_are_explicit_and_fullness_is_off() {
        let cfg = parse(&[], 12).unwrap().unwrap();
        assert_eq!(cfg.pure_l2, PureL2Mode::Always);
        assert_eq!(cfg.max_unsettled_l2_blocks.get(), 150);
        assert_eq!(cfg.blob_fullness_bps, None);
        assert_eq!(cfg.inactive_interval_ms, None);
        for required in [MODE, GENERAL] {
            assert!(
                PolicyConfig::parse(
                    |name| Ok(match name {
                        ENABLED => Some("true".into()),
                        "EEZ_MAX_SPECULATIVE_DEPTH" => Some("0".into()),
                        name if name == required => None,
                        MODE => Some("always".into()),
                        GENERAL => Some("150".into()),
                        _ => None,
                    }),
                    12
                )
                .is_err()
            );
        }
    }

    #[test]
    fn modes_and_inactive_interval_are_validated() {
        let cfg = parse(
            &[(MODE, "interval"), (INTERVAL, "900000"), (FULLNESS, "1000")],
            5,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            cfg.pure_l2,
            PureL2Mode::Interval {
                milliseconds: NonZeroU64::new(900_000).unwrap()
            }
        );
        assert_eq!(cfg.blob_fullness_bps.unwrap().get(), 1000);
        let cfg = parse(&[(INTERVAL, "15")], 5).unwrap().unwrap();
        assert_eq!(cfg.inactive_interval_ms.unwrap().get(), 15);
        for extra in [
            vec![(MODE, "interval")],
            vec![(INTERVAL, "0")],
            vec![(INTERVAL, "invalid")],
            vec![(MODE, "ALWAYS")],
        ] {
            assert!(parse(&extra, 5).is_err(), "{extra:?}");
        }
    }

    #[test]
    fn rejects_invalid_numbers_and_disabled_or_obsolete_settings() {
        for value in ["0", "-1", "1.5", "18446744073709551616", ""] {
            assert!(parse(&[(GENERAL, value)], 12).is_err());
        }
        for value in ["0", "10001", "65536", "-1", "false", ""] {
            assert!(parse(&[(FULLNESS, value)], 12).is_err());
        }
        for value in ["1", "10000", "off"] {
            assert!(parse(&[(FULLNESS, value)], 12).is_ok());
        }
        assert!(parse(&[(ENABLED, "false")], 12).is_err());
        assert!(parse(&[(ENABLED, "invalid")], 12).is_err());
        assert!(
            parse(&[("EEZ_MAX_BLOCKS_PER_BATCH", "300")], 12)
                .unwrap_err()
                .to_string()
                .contains("obsolete")
        );
    }

    #[test]
    fn depth_bound_preserves_grid_and_uses_checked_arithmetic() {
        for k in [5, 6, 12] {
            let minimum = 150 + k - 1;
            assert!(
                parse(
                    &[("EEZ_MAX_SPECULATIVE_DEPTH", &(minimum - 1).to_string())],
                    k
                )
                .is_err()
            );
            assert!(parse(&[("EEZ_MAX_SPECULATIVE_DEPTH", &minimum.to_string())], k).is_ok());
            assert!(parse(&[], k).is_ok());
        }
        assert!(parse(&[(GENERAL, &u64::MAX.to_string())], 5).is_err());
        assert!(parse(&[("EEZ_MAX_SPECULATIVE_DEPTH", "bad")], 5).is_err());
        assert!(parse(&[], 0).is_err());
    }
}
