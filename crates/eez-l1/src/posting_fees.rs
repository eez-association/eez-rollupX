//! Posting fee configuration, checked quotes, and native-unit cost accounting.
//! This is independent of settlement urgency: a fee error preserves due work.

use alloy_primitives::U256;

use crate::{BundleTarget, L1BlockRef};

/// Recent canonical blocks used to price an exact-next-block submission.
pub const PRIORITY_FEE_HISTORY_BLOCKS: u64 = 20;
/// Gas-weighted reward percentile requested for each historical block.
pub const PRIORITY_FEE_REWARD_PERCENTILE: f64 = 90.0;
/// Percentile of the per-block rewards selected as the next-block bid.
const PRIORITY_FEE_CONFIDENCE_PERCENTILE: usize = 90;

/// All prices are native base units per gas unit. Ceilings are mandatory for
/// enabled production posting; profile examples choose their own values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostingFeeConfig {
    pub priority_override: Option<u128>,
    pub priority_min: u128,
    pub priority_max: u128,
    pub execution_price_ceiling: u128,
    pub blob_price_ceiling: u128,
    pub execution_headroom_bps: u64,
    pub blob_headroom_bps: u64,
    pub gas_margin_bps: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PostingFeeError {
    #[error("posting fee configuration: {0}")]
    Config(String),
    #[error("posting fee input unavailable: {0}")]
    Unavailable(&'static str),
    #[error("fee_cap_blocked: {0}")]
    CapBlocked(&'static str),
    #[error("posting cost arithmetic overflow: {0}")]
    Arithmetic(&'static str),
    #[error(
        "estimated gas {estimated} plus margin {margin_bps} bps exceeds transaction budget {maximum}"
    )]
    GasBudget {
        estimated: u64,
        margin_bps: u64,
        maximum: u64,
    },
}

impl PostingFeeConfig {
    /// Parse an immutable startup snapshot. The fee-history estimator is used
    /// unless the existing explicit priority override is supplied.
    pub fn parse(
        mut get: impl FnMut(&str) -> Result<Option<String>, PostingFeeError>,
    ) -> Result<Self, PostingFeeError> {
        let mut integer = |name: &str| -> Result<Option<u128>, PostingFeeError> {
            get(name)?
                .map(|value| {
                    value.parse::<u128>().map_err(|_| {
                        PostingFeeError::Config(format!("{name} must be a nonnegative integer"))
                    })
                })
                .transpose()
        };
        let required_positive = |name: &str, value: Option<u128>| {
            value.filter(|value| *value > 0).ok_or_else(|| {
                PostingFeeError::Config(format!("{name} is required and must be positive"))
            })
        };
        let execution_price_ceiling = required_positive(
            "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS",
            integer("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS")?,
        )?;
        let blob_price_ceiling = required_positive(
            "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS",
            integer("EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS")?,
        )?;
        let priority_override = integer("EEZ_L1_POSTBATCH_PRIORITY_FEE")?;
        let priority_min = integer("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE")?.unwrap_or(0);
        let configured_priority_max = integer("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE")?;
        let mut bps = |name: &str, default: u64| -> Result<u64, PostingFeeError> {
            u64::try_from(integer(name)?.unwrap_or(u128::from(default)))
                .map_err(|_| PostingFeeError::Config(format!("{name} exceeds u64")))
        };
        let priority_max = match (configured_priority_max, priority_override) {
            (Some(maximum), _) => maximum,
            (None, Some(fixed)) => fixed,
            (None, None) => {
                return Err(PostingFeeError::Config(
                    "EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE is required when using the fee-history priority estimator"
                        .into(),
                ));
            }
        };
        let config = Self {
            priority_override,
            priority_min,
            priority_max,
            execution_price_ceiling,
            blob_price_ceiling,
            // These retain the existing 2x base-fee headroom, now configurable.
            execution_headroom_bps: bps("EEZ_L1_POSTBATCH_EXECUTION_HEADROOM_BPS", 10_000)?,
            blob_headroom_bps: bps("EEZ_L1_POSTBATCH_BLOB_HEADROOM_BPS", 10_000)?,
            // Gas selection is validated for complete effects after this margin.
            gas_margin_bps: bps("EEZ_L1_POSTBATCH_GAS_MARGIN_BPS", 500)?,
        };
        if config.priority_min > config.priority_max
            || config.priority_max > config.execution_price_ceiling
        {
            return Err(PostingFeeError::Config(
                "priority minimum <= priority maximum <= execution price ceiling is required"
                    .into(),
            ));
        }
        if config
            .priority_override
            .is_some_and(|value| value < config.priority_min || value > config.priority_max)
        {
            return Err(PostingFeeError::Config(
                "EEZ_L1_POSTBATCH_PRIORITY_FEE is outside the configured priority bounds".into(),
            ));
        }
        Ok(config)
    }

    /// Select fees for one actual attempt. Base fees and the estimated tip
    /// come from the selected network, not a currency conversion or forecast.
    pub fn quote(&self, sample: FeeSample) -> Result<FeeQuote, PostingFeeError> {
        let BundleTarget::Exact { block, .. } = sample.target else {
            return Err(PostingFeeError::Unavailable("unresolved submission target"));
        };
        if block <= sample.block.number {
            return Err(PostingFeeError::Unavailable(
                "submission target is not in the future",
            ));
        }
        let execution_base = sample
            .execution_base_fee
            .ok_or(PostingFeeError::Unavailable("execution base fee"))?;
        let blob_base = sample
            .blob_base_fee
            .ok_or(PostingFeeError::Unavailable("blob base fee"))?;
        let priority = match self.priority_override {
            Some(value) => value,
            None => sample
                .estimated_priority_fee
                .ok_or(PostingFeeError::Unavailable("priority fee history"))?
                .clamp(self.priority_min, self.priority_max),
        };
        let required_execution = execution_base
            .checked_add(priority)
            .ok_or(PostingFeeError::Arithmetic("base fee + priority"))?;
        if required_execution > self.execution_price_ceiling {
            return Err(PostingFeeError::CapBlocked(
                "execution price ceiling is below base fee plus priority",
            ));
        }
        if blob_base == 0 {
            return Err(PostingFeeError::Unavailable(
                "blob base fee must be positive",
            ));
        }
        if blob_base > self.blob_price_ceiling {
            return Err(PostingFeeError::CapBlocked(
                "blob price ceiling is below the active blob fee",
            ));
        }
        let max_fee_per_gas = with_margin(execution_base, self.execution_headroom_bps)?
            .checked_add(priority)
            .ok_or(PostingFeeError::Arithmetic(
                "execution fee headroom + priority",
            ))?
            .min(self.execution_price_ceiling);
        let max_fee_per_blob_gas =
            with_margin(blob_base, self.blob_headroom_bps)?.min(self.blob_price_ceiling);
        Ok(FeeQuote {
            chain_id: sample.chain_id,
            sample: sample.block,
            target: sample.target,
            max_priority_fee_per_gas: priority,
            max_fee_per_gas,
            max_fee_per_blob_gas,
            estimated_effective_gas_price: required_execution,
            estimated_blob_gas_price: blob_base,
        })
    }

    pub fn declared_gas(&self, estimated: u64, maximum: u64) -> Result<u64, PostingFeeError> {
        let gas = with_margin(u128::from(estimated), self.gas_margin_bps)?;
        let gas = u64::try_from(gas)
            .map_err(|_| PostingFeeError::Arithmetic("declared execution gas"))?;
        if estimated == 0 || gas > maximum {
            return Err(PostingFeeError::GasBudget {
                estimated,
                margin_bps: self.gas_margin_bps,
                maximum,
            });
        }
        Ok(gas)
    }

    /// Largest conservative estimate whose configured margin fits the actual
    /// budget. Admission uses this before proving, so a valid group is not
    /// repeatedly admitted and then rejected solely by its known gas margin.
    pub fn estimate_budget(&self, maximum: u64) -> u64 {
        let factor = 10_000_u128 + u128::from(self.gas_margin_bps);
        // The product fits u128 for every u64 input; factor is at least 10k,
        // so the quotient is no greater than maximum and fits u64.
        (u128::from(maximum) * 10_000 / factor) as u64
    }
}

/// Select a deadline-oriented priority fee from `eth_feeHistory` rewards.
///
/// The RPC request asks for one gas-weighted p90 reward per block. Taking the
/// p90 of those recent block samples avoids reacting to one isolated spike but
/// prices above ordinary transactions in most recent blocks. Empty blocks
/// contribute the zero reward returned by the RPC; configured bounds are
/// applied later by [`PostingFeeConfig::quote`].
pub fn estimate_next_block_priority_fee(
    oldest_block: u64,
    head_block: u64,
    rewards: Option<&[Vec<u128>]>,
) -> Result<u128, PostingFeeError> {
    let rewards = rewards
        .filter(|rewards| !rewards.is_empty())
        .ok_or(PostingFeeError::Unavailable("priority fee history"))?;
    let reward_count = u64::try_from(rewards.len())
        .map_err(|_| PostingFeeError::Unavailable("priority fee history is too large"))?;
    if oldest_block.checked_add(reward_count - 1) != Some(head_block) {
        return Err(PostingFeeError::Unavailable(
            "priority fee history canonical range",
        ));
    }
    let mut samples = Vec::with_capacity(rewards.len());
    for reward in rewards {
        let [sample] = reward.as_slice() else {
            return Err(PostingFeeError::Unavailable("priority fee history shape"));
        };
        samples.push(*sample);
    }
    samples.sort_unstable();
    let index = (samples.len() - 1) * PRIORITY_FEE_CONFIDENCE_PERCENTILE / 100;
    Ok(samples[index])
}

/// All network observations for a quote refer to the same canonical sample.
#[derive(Debug, Clone, Copy)]
pub struct FeeSample {
    pub chain_id: u64,
    pub block: L1BlockRef,
    pub target: BundleTarget,
    pub execution_base_fee: Option<u128>,
    pub blob_base_fee: Option<u128>,
    pub estimated_priority_fee: Option<u128>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeQuote {
    pub chain_id: u64,
    pub sample: L1BlockRef,
    pub target: BundleTarget,
    pub max_priority_fee_per_gas: u128,
    pub max_fee_per_gas: u128,
    pub max_fee_per_blob_gas: u128,
    pub estimated_effective_gas_price: u128,
    pub estimated_blob_gas_price: u128,
}

impl FeeQuote {
    pub fn is_current(self, chain_id: u64, block: L1BlockRef, target: BundleTarget) -> bool {
        self.chain_id == chain_id && self.sample == block && self.target == target
    }

    pub fn estimate(self, gas_used: u64, blobs: u64) -> Result<PostingCost, PostingFeeError> {
        PostingCost::calculate(
            gas_used,
            self.estimated_effective_gas_price,
            blobs
                .checked_mul(alloy_eips::eip4844::DATA_GAS_PER_BLOB)
                .ok_or(PostingFeeError::Arithmetic("physical blob gas"))?,
            self.estimated_blob_gas_price,
        )
    }

    /// Balance reservation is deliberately separate from expected gas usage.
    pub fn reserve(self, declared_gas: u64, blobs: u64) -> Result<PostingCost, PostingFeeError> {
        PostingCost::calculate(
            declared_gas,
            self.max_fee_per_gas,
            blobs
                .checked_mul(alloy_eips::eip4844::DATA_GAS_PER_BLOB)
                .ok_or(PostingFeeError::Arithmetic("reserved blob gas"))?,
            self.max_fee_per_blob_gas,
        )
    }
}

/// Separate native-unit execution and blob charges. Receipt callers use
/// actual gas and effective prices even if execution reverted or was partial.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PostingCost {
    pub execution: U256,
    pub blobs: U256,
    pub total: U256,
}

impl PostingCost {
    pub fn checked_add(self, other: Self) -> Result<Self, PostingFeeError> {
        Ok(Self {
            execution: self
                .execution
                .checked_add(other.execution)
                .ok_or(PostingFeeError::Arithmetic("aggregate execution cost"))?,
            blobs: self
                .blobs
                .checked_add(other.blobs)
                .ok_or(PostingFeeError::Arithmetic("aggregate blob cost"))?,
            total: self
                .total
                .checked_add(other.total)
                .ok_or(PostingFeeError::Arithmetic("aggregate total cost"))?,
        })
    }

    pub fn calculate(
        gas_used: u64,
        effective_gas_price: u128,
        blob_gas_used: u64,
        blob_gas_price: u128,
    ) -> Result<Self, PostingFeeError> {
        let execution = U256::from(gas_used)
            .checked_mul(U256::from(effective_gas_price))
            .ok_or(PostingFeeError::Arithmetic("execution cost"))?;
        let blobs = U256::from(blob_gas_used)
            .checked_mul(U256::from(blob_gas_price))
            .ok_or(PostingFeeError::Arithmetic("blob cost"))?;
        let total = execution
            .checked_add(blobs)
            .ok_or(PostingFeeError::Arithmetic("total cost"))?;
        Ok(Self {
            execution,
            blobs,
            total,
        })
    }
}

/// Measured transaction charges. Status does not affect payment: a reverted
/// transaction pays for its execution and all of its physical blobs too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReceiptCost {
    pub gas_used: u64,
    pub effective_gas_price: u128,
    pub blob_gas_used: u64,
    pub blob_gas_price: u128,
    pub paid: PostingCost,
}

impl ReceiptCost {
    pub fn from_receipt(
        receipt: &alloy_rpc_types_eth::TransactionReceipt,
    ) -> Result<Self, PostingFeeError> {
        let blob = receipt.inner.tx_type() == alloy_consensus::TxType::Eip4844;
        let (blob_gas_used, blob_gas_price) = if blob {
            let used = receipt
                .blob_gas_used
                .filter(|used| {
                    *used > 0 && used.is_multiple_of(alloy_eips::eip4844::DATA_GAS_PER_BLOB)
                })
                .ok_or(PostingFeeError::Unavailable(
                    "valid physical blob gas in canonical receipt",
                ))?;
            let price = receipt.blob_gas_price.filter(|price| *price > 0).ok_or(
                PostingFeeError::Unavailable("blob gas price in canonical receipt"),
            )?;
            (used, price)
        } else {
            if receipt.blob_gas_used.is_some_and(|gas| gas != 0) {
                return Err(PostingFeeError::Unavailable(
                    "non-blob receipt reports blob gas",
                ));
            }
            (0, 0)
        };
        Ok(Self {
            gas_used: receipt.gas_used,
            effective_gas_price: receipt.effective_gas_price,
            blob_gas_used,
            blob_gas_price,
            paid: PostingCost::calculate(
                receipt.gas_used,
                receipt.effective_gas_price,
                blob_gas_used,
                blob_gas_price,
            )?,
        })
    }
}

/// `ceil(value * (10000 + bps) / 10000)`, without wrapping or saturating.
pub fn with_margin(value: u128, bps: u64) -> Result<u128, PostingFeeError> {
    if bps == 0 {
        return Ok(value);
    }
    let scaled = value
        .checked_mul(10_000u128 + u128::from(bps))
        .ok_or(PostingFeeError::Arithmetic("basis point margin"))?;
    Ok(scaled.div_ceil(10_000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    fn config(extra: &[(&str, &str)]) -> Result<PostingFeeConfig, PostingFeeError> {
        let mut values = std::collections::HashMap::from([
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "1000"),
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS", "100"),
            ("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "500"),
        ]);
        values.extend(extra.iter().copied());
        PostingFeeConfig::parse(|key| Ok(values.get(key).map(|value| (*value).into())))
    }

    fn sample() -> FeeSample {
        FeeSample {
            chain_id: 1,
            block: L1BlockRef {
                number: 10,
                hash: B256::with_last_byte(1),
            },
            target: BundleTarget::Exact {
                block: 11,
                timestamp: 1000,
            },
            execution_base_fee: Some(100),
            blob_base_fee: Some(3),
            estimated_priority_fee: Some(2),
        }
    }

    #[test]
    fn gas_margin_defaults_to_five_percent_and_preserves_explicit_values() {
        assert_eq!(
            config(&[]).unwrap().declared_gas(100_000, 120_000).unwrap(),
            105_000
        );
        for (value, expected) in [("0", 100_000), ("500", 105_000), ("2000", 120_000)] {
            let fees = config(&[("EEZ_L1_POSTBATCH_GAS_MARGIN_BPS", value)]).unwrap();
            assert_eq!(fees.declared_gas(100_000, 120_000).unwrap(), expected);
        }
        for value in ["", "bad", "-1", "5%", "18446744073709551616"] {
            assert!(
                config(&[("EEZ_L1_POSTBATCH_GAS_MARGIN_BPS", value)]).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn dynamic_fee_history_requires_an_independent_priority_ceiling() {
        let config = config(&[]).unwrap();
        assert_eq!(config.priority_override, None);
        assert_eq!(config.priority_min, 0);
        assert_eq!(config.priority_max, 500);
        assert_eq!(config.gas_margin_bps, 500);
        let quote = config.quote(sample()).unwrap();
        assert_eq!(quote.max_fee_per_gas, 202);
        assert_eq!(quote.max_fee_per_blob_gas, 6);
        assert!(PostingFeeConfig::parse(|_| Ok(None)).is_err());

        let values = std::collections::HashMap::from([
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS", "1000"),
            ("EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS", "100"),
        ]);
        let error = PostingFeeConfig::parse(|key| Ok(values.get(key).map(|value| (*value).into())))
            .unwrap_err();
        assert!(error.to_string().contains("MAX_PRIORITY_FEE is required"));

        // An exact override is itself a safe independent ceiling when no
        // separate maximum is supplied.
        let mut fixed = values;
        fixed.insert("EEZ_L1_POSTBATCH_PRIORITY_FEE", "7");
        let config =
            PostingFeeConfig::parse(|key| Ok(fixed.get(key).map(|value| (*value).into()))).unwrap();
        assert_eq!(config.priority_override, Some(7));
        assert_eq!(config.priority_max, 7);
    }

    #[test]
    fn observed_percentage_uplift_reproduces_the_self_feedback_loop() {
        let mut estimate = 114_475_460_012;
        for expected in [137_370_552_015, 164_844_662_418, 197_813_594_902] {
            estimate = with_margin(estimate, 2_000).unwrap();
            assert_eq!(estimate, expected);
        }
    }

    #[test]
    fn fee_history_selects_recent_p90_of_p90_without_chasing_one_spike() {
        let ascending: Vec<_> = (1..=20).map(|reward| vec![reward]).collect();
        assert_eq!(
            estimate_next_block_priority_fee(81, 100, Some(&ascending)).unwrap(),
            18
        );

        let mut one_spike = vec![vec![7]; 20];
        one_spike[19][0] = u128::MAX;
        assert_eq!(
            estimate_next_block_priority_fee(81, 100, Some(&one_spike)).unwrap(),
            7
        );

        for malformed in [
            None,
            Some([].as_slice()),
            Some([vec![]].as_slice()),
            Some([vec![1, 2]].as_slice()),
        ] {
            assert!(estimate_next_block_priority_fee(81, 100, malformed).is_err());
        }
        assert!(estimate_next_block_priority_fee(80, 100, Some(&ascending)).is_err());
        assert!(estimate_next_block_priority_fee(u64::MAX, u64::MAX, Some(&ascending)).is_err());
    }

    #[test]
    fn sustained_self_posting_does_not_amplify_estimator_samples() {
        let config = config(&[
            ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "5"),
            ("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "50"),
        ])
        .unwrap();
        // One poster transaction among nineteen empty blocks cannot move the
        // market percentile. The optional relay floor is stable and cannot
        // become an input to a recursive uplift.
        let mut estimate = 10;
        for _ in 0..20 {
            let mut history = vec![vec![0]; 20];
            history[19][0] = estimate;
            let market = estimate_next_block_priority_fee(81, 100, Some(&history)).unwrap();
            let quote = config
                .quote(FeeSample {
                    estimated_priority_fee: Some(market),
                    ..sample()
                })
                .unwrap();
            assert_eq!(quote.max_priority_fee_per_gas, 5);
            estimate = quote.max_priority_fee_per_gas;
        }
    }

    #[test]
    fn sparse_samples_use_the_floor_and_spikes_use_the_priority_ceiling() {
        let config = config(&[
            ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "5"),
            ("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "50"),
        ])
        .unwrap();
        for (estimated, selected) in [(0, 5), (1, 5), (5, 5), (49, 49), (50, 50), (u128::MAX, 50)] {
            let quote = config
                .quote(FeeSample {
                    estimated_priority_fee: Some(estimated),
                    ..sample()
                })
                .unwrap();
            assert_eq!(quote.max_priority_fee_per_gas, selected);
        }
    }

    #[test]
    fn fixed_priority_override_is_exact_and_does_not_query_the_estimator() {
        let config = config(&[("EEZ_L1_POSTBATCH_PRIORITY_FEE", "7")]).unwrap();
        for estimate in [None, Some(500)] {
            let quote = config
                .quote(FeeSample {
                    estimated_priority_fee: estimate,
                    ..sample()
                })
                .unwrap();
            assert_eq!(quote.max_priority_fee_per_gas, 7);
        }
    }

    #[test]
    fn external_fee_history_changes_are_followed_within_independent_bounds() {
        let config = config(&[
            ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "5"),
            ("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "50"),
        ])
        .unwrap();
        let selected: Vec<_> = [5, 8, 13, 21, 34, 55]
            .into_iter()
            .map(|estimate| {
                let history = vec![vec![estimate]; 20];
                let market = estimate_next_block_priority_fee(81, 100, Some(&history)).unwrap();
                config
                    .quote(FeeSample {
                        estimated_priority_fee: Some(market),
                        ..sample()
                    })
                    .unwrap()
                    .max_priority_fee_per_gas
            })
            .collect();
        assert_eq!(selected, [5, 8, 13, 21, 34, 50]);
    }

    #[test]
    fn unavailable_estimates_and_execution_caps_fail_closed() {
        let config = config(&[]).unwrap();
        assert!(matches!(
            config.quote(FeeSample {
                estimated_priority_fee: None,
                ..sample()
            }),
            Err(PostingFeeError::Unavailable("priority fee history"))
        ));
        let expensive = FeeSample {
            execution_base_fee: Some(989),
            estimated_priority_fee: Some(12),
            ..sample()
        };
        assert!(matches!(
            config.quote(expensive),
            Err(PostingFeeError::CapBlocked(_))
        ));
        let quote = config
            .quote(FeeSample {
                execution_base_fee: Some(988),
                ..expensive
            })
            .unwrap();
        assert_eq!(quote.estimated_effective_gas_price, 1000);
        assert_eq!(quote.max_fee_per_gas, 1000);
    }

    #[test]
    fn quotes_require_a_concrete_future_target() {
        let config = config(&[]).unwrap();
        for target in [
            BundleTarget::NextBlock,
            BundleTarget::Exact {
                block: 10,
                timestamp: 1000,
            },
            BundleTarget::Exact {
                block: 9,
                timestamp: 1000,
            },
        ] {
            assert!(matches!(
                config.quote(FeeSample { target, ..sample() }),
                Err(PostingFeeError::Unavailable(_))
            ));
        }
    }

    #[test]
    fn validates_override_minimum_maximum_and_bad_numbers() {
        for extra in [
            vec![("EEZ_L1_POSTBATCH_PRIORITY_FEE", "bad")],
            vec![("EEZ_L1_POSTBATCH_PRIORITY_FEE", "1001")],
            vec![("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "")],
            vec![("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "bad")],
            vec![("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "1001")],
            vec![
                ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "3"),
                ("EEZ_L1_POSTBATCH_PRIORITY_FEE", "2"),
            ],
            vec![
                ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "8"),
                ("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "7"),
            ],
            vec![("EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS", "0")],
            vec![("EEZ_L1_POSTBATCH_GAS_MARGIN_BPS", "-1")],
        ] {
            assert!(config(&extra).is_err(), "{extra:?}");
        }
        let fixed_zero = config(&[("EEZ_L1_POSTBATCH_PRIORITY_FEE", "0")]).unwrap();
        let quote = fixed_zero
            .quote(FeeSample {
                estimated_priority_fee: None,
                ..sample()
            })
            .unwrap();
        assert_eq!(quote.max_priority_fee_per_gas, 0);
        let zero_dynamic = config(&[("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "0")])
            .unwrap()
            .quote(sample())
            .unwrap();
        assert_eq!(zero_dynamic.max_priority_fee_per_gas, 0);
    }

    #[test]
    fn network_prices_margins_and_caps_are_independent() {
        let config = config(&[
            ("EEZ_L1_POSTBATCH_MIN_PRIORITY_FEE", "5"),
            ("EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE", "10"),
            ("EEZ_L1_POSTBATCH_EXECUTION_HEADROOM_BPS", "1000"),
            ("EEZ_L1_POSTBATCH_BLOB_HEADROOM_BPS", "0"),
        ])
        .unwrap();
        let ethereum = config.quote(sample()).unwrap();
        let chiado = config
            .quote(FeeSample {
                chain_id: 10200,
                execution_base_fee: Some(20),
                blob_base_fee: Some(1),
                ..sample()
            })
            .unwrap();
        assert_eq!(ethereum.max_fee_per_gas, 115);
        assert_eq!(chiado.max_fee_per_gas, 27);
        assert_eq!(chiado.max_fee_per_blob_gas, 1);
        let clipped = config
            .quote(FeeSample {
                execution_base_fee: Some(990),
                ..sample()
            })
            .unwrap();
        assert_eq!(clipped.max_fee_per_gas, 1000);
        assert!(matches!(
            config.quote(FeeSample {
                execution_base_fee: Some(1000),
                ..sample()
            }),
            Err(PostingFeeError::CapBlocked(_))
        ));
        assert!(matches!(
            config.quote(FeeSample {
                blob_base_fee: Some(101),
                ..sample()
            }),
            Err(PostingFeeError::CapBlocked(_))
        ));
    }

    #[test]
    fn missing_prices_never_become_zero_and_new_heads_invalidate_quotes() {
        let config = config(&[]).unwrap();
        for input in [
            FeeSample {
                execution_base_fee: None,
                ..sample()
            },
            FeeSample {
                blob_base_fee: None,
                ..sample()
            },
            FeeSample {
                blob_base_fee: Some(0),
                ..sample()
            },
            FeeSample {
                estimated_priority_fee: None,
                ..sample()
            },
        ] {
            assert!(matches!(
                config.quote(input),
                Err(PostingFeeError::Unavailable(_))
            ));
        }
        let input = sample();
        let quote = config.quote(input).unwrap();
        assert!(quote.is_current(input.chain_id, input.block, input.target));
        assert!(!quote.is_current(10200, input.block, input.target));
        assert!(!quote.is_current(
            input.chain_id,
            L1BlockRef {
                hash: B256::ZERO,
                ..input.block
            },
            input.target
        ));
        assert!(!quote.is_current(
            input.chain_id,
            input.block,
            BundleTarget::Exact {
                block: 12,
                timestamp: 1012
            }
        ));
    }

    #[test]
    fn canonical_receipts_charge_reverts_and_full_physical_blobs() {
        let mut receipt = alloy_rpc_types_eth::TransactionReceipt {
            inner: alloy_consensus::ReceiptEnvelope::Eip4844(Default::default()),
            gas_used: 90_000,
            effective_gas_price: 7,
            blob_gas_used: Some(2 * alloy_eips::eip4844::DATA_GAS_PER_BLOB),
            blob_gas_price: Some(3),
            transaction_hash: B256::ZERO,
            transaction_index: Some(0),
            block_hash: Some(B256::ZERO),
            block_number: Some(1),
            from: alloy_primitives::Address::ZERO,
            to: None,
            contract_address: None,
        };
        receipt
            .inner
            .as_receipt_with_bloom_mut()
            .unwrap()
            .receipt
            .status = false.into();
        assert!(!receipt.status());
        let cost = ReceiptCost::from_receipt(&receipt).unwrap();
        assert_eq!(cost.paid.execution, U256::from(630_000));
        assert_eq!(
            cost.paid.blobs,
            U256::from(6 * alloy_eips::eip4844::DATA_GAS_PER_BLOB)
        );
        for gas in [None, Some(0), Some(1)] {
            receipt.blob_gas_used = gas;
            assert!(ReceiptCost::from_receipt(&receipt).is_err());
        }
        receipt.blob_gas_used = Some(alloy_eips::eip4844::DATA_GAS_PER_BLOB);
        for price in [None, Some(0)] {
            receipt.blob_gas_price = price;
            assert!(ReceiptCost::from_receipt(&receipt).is_err());
        }
        receipt.inner = alloy_consensus::ReceiptEnvelope::Eip1559(Default::default());
        assert!(ReceiptCost::from_receipt(&receipt).is_err());
        receipt.blob_gas_used = None;
        assert_eq!(
            ReceiptCost::from_receipt(&receipt).unwrap().paid.blobs,
            U256::ZERO
        );
    }

    #[test]
    fn admission_budget_is_the_largest_estimate_whose_margin_fits() {
        for margin in [0, 1, 500, 2_000, 10_000, u64::MAX] {
            let mut config = config(&[]).unwrap();
            config.gas_margin_bps = margin;
            for maximum in [0, 1, 21_000, 120_001, 16_777_216, u64::MAX] {
                let estimate = config.estimate_budget(maximum);
                assert!(estimate <= maximum);
                if estimate > 0 {
                    assert!(config.declared_gas(estimate, maximum).unwrap() <= maximum);
                }
                if let Some(next) = estimate.checked_add(1) {
                    assert!(config.declared_gas(next, maximum).is_err());
                }
            }
        }
    }

    #[test]
    fn margin_rounding_gas_reservation_and_actual_cost_are_distinct() {
        assert_eq!(with_margin(1, 1).unwrap(), 2);
        assert_eq!(with_margin(100, 0).unwrap(), 100);
        assert_eq!(with_margin(u128::MAX, 0).unwrap(), u128::MAX);
        assert!(with_margin(u128::MAX, 1).is_err());
        let config = config(&[]).unwrap();
        assert_eq!(config.declared_gas(100_001, 1_000_000).unwrap(), 105_002);
        assert!(config.declared_gas(100_001, 105_001).is_err());
        let quote = config.quote(sample()).unwrap();
        let estimate = quote.estimate(100_001, 1).unwrap();
        let reserve = quote.reserve(105_002, 1).unwrap();
        assert!(reserve.total > estimate.total);
        assert_eq!(
            estimate.blobs,
            U256::from(alloy_eips::eip4844::DATA_GAS_PER_BLOB * 3)
        );
        let actual =
            PostingCost::calculate(90_000, 101, alloy_eips::eip4844::DATA_GAS_PER_BLOB, 2).unwrap();
        assert_eq!(actual.execution, U256::from(9_090_000));
        assert_ne!(actual, estimate);
        assert_eq!(
            PostingCost::calculate(0, 0, 0, 0).unwrap(),
            PostingCost::default()
        );
    }
}
