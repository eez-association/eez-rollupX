//! Fee collection switches and explicit cost assumptions, read once at startup.

use alloy_primitives::Address;

use super::{pricing, refund};

#[derive(Debug, Clone)]
pub(crate) struct FeeConfig {
    l1_to_l2_enabled: bool,
    l2_to_l1_enabled: bool,
    pricing: Option<pricing::Config>,
    refund: Option<refund::Policy>,
}

/// Bound to the actual node before any quote or settlement workers start.
/// Inactive refund terms are omitted so consumers cannot accidentally activate
/// collection merely because an operator has prepared the configuration.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeFees {
    pub l2_to_l1_enabled: bool,
    pub pricing: Option<pricing::Policy>,
    pub l1_refund: Option<refund::Policy>,
}

impl FeeConfig {
    pub fn parse(
        mut get: impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        for name in [
            "EEZ_L2_FEE_SHADOW_POLICY",
            "EEZ_L2_FEE_ENFORCE_TARIFF",
            "EEZ_L1_FEE_REFUND_POLICY",
        ] {
            if get(name)?.is_some() {
                return Err(format!(
                    "{name} has been removed; migrate to EEZ_FEES_* and remove obsolete settings (see docs/configurable-settlement-policy.md#cross-chain-fee-configuration)"
                ));
            }
        }
        let mut enabled = |name| match get(name)?.as_deref() {
            None | Some("false") => Ok(false),
            Some("true") => Ok(true),
            Some(_) => Err(format!("{name} must be true or false")),
        };
        let l1_to_l2_enabled = enabled("EEZ_FEES_L1_TO_L2_ENABLED")?;
        let l2_to_l1_enabled = enabled("EEZ_FEES_L2_TO_L1_ENABLED")?;
        let pricing = pricing::Config::parse(&mut get)?;
        let refund = refund::Policy::parse(
            get("EEZ_FEES_L1_REFUND_RECIPIENT")?,
            get("EEZ_FEES_L1_REFUND_PERCENT")?,
        )?;
        if (l1_to_l2_enabled || l2_to_l1_enabled) && pricing.is_none() {
            return Err(
                "enabling cross-chain fees requires complete EEZ_FEES_* cost assumptions".into(),
            );
        }
        if l1_to_l2_enabled && refund.is_none() {
            return Err("EEZ_FEES_L1_TO_L2_ENABLED requires EEZ_FEES_L1_REFUND_RECIPIENT and EEZ_FEES_L1_REFUND_PERCENT".into());
        }
        Ok(Self {
            l1_to_l2_enabled,
            l2_to_l1_enabled,
            pricing,
            refund,
        })
    }

    pub fn bind(
        self,
        l1_chain_id: u64,
        l2_chain_id: u64,
        owned_rollups: impl IntoIterator<Item = u64>,
        beneficiary: Address,
    ) -> Result<RuntimeFees, String> {
        let pricing = self
            .pricing
            .map(|pricing| pricing.bind(l1_chain_id, l2_chain_id, owned_rollups, beneficiary))
            .transpose()?;
        Ok(RuntimeFees {
            l2_to_l1_enabled: self.l2_to_l1_enabled,
            pricing,
            l1_refund: self.refund.filter(|_| self.l1_to_l2_enabled),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn model() -> HashMap<&'static str, String> {
        pricing::tests::settings(&pricing::tests::policy_json())
            .into_iter()
            .collect()
    }

    fn parse(values: &HashMap<&str, String>) -> Result<FeeConfig, String> {
        FeeConfig::parse(|name| Ok(values.get(name).cloned()))
    }

    fn bind(config: FeeConfig) -> Result<RuntimeFees, String> {
        config.bind(1, 9, [1], Address::repeat_byte(4))
    }

    #[test]
    fn absent_disabled_and_prepared_settings_never_enable_collection() {
        let empty = parse(&HashMap::new())
            .unwrap()
            .bind(0, 0, [], Address::ZERO)
            .unwrap();
        assert!(!empty.l2_to_l1_enabled);
        assert!(empty.l1_refund.is_none());
        assert!(empty.pricing.is_none());
        let mut values = model();
        values.insert("EEZ_FEES_L1_REFUND_PERCENT", "90".into());
        values.insert(
            "EEZ_FEES_L1_REFUND_RECIPIENT",
            Address::repeat_byte(9).to_string(),
        );
        for explicit_false in [false, true] {
            if explicit_false {
                values.insert("EEZ_FEES_L1_TO_L2_ENABLED", "false".into());
                values.insert("EEZ_FEES_L2_TO_L1_ENABLED", "false".into());
            }
            let runtime = bind(parse(&values).unwrap()).unwrap();
            assert!(
                runtime.pricing.is_some(),
                "advisory L2 quotes retain the cost model"
            );
            assert!(!runtime.l2_to_l1_enabled);
            assert!(
                runtime.l1_refund.is_none(),
                "prepared terms cannot enable L1 quotes or refunds"
            );
        }
        for name in pricing::Config::SETTINGS {
            values.remove(name);
        }
        assert!(bind(parse(&values).unwrap()).unwrap().l1_refund.is_none());
    }

    #[test]
    fn enabling_a_direction_requires_its_complete_dependencies() {
        for flag in ["EEZ_FEES_L1_TO_L2_ENABLED", "EEZ_FEES_L2_TO_L1_ENABLED"] {
            let mut values = HashMap::from([(flag, "true".into())]);
            assert!(parse(&values).unwrap_err().contains("cost assumptions"));
            values.extend(model());
            if flag == "EEZ_FEES_L1_TO_L2_ENABLED" {
                assert!(parse(&values).unwrap_err().contains("REFUND_RECIPIENT"));
                values.insert(
                    "EEZ_FEES_L1_REFUND_RECIPIENT",
                    Address::repeat_byte(9).to_string(),
                );
                assert!(parse(&values).unwrap_err().contains("REFUND_PERCENT"));
                values.insert("EEZ_FEES_L1_REFUND_PERCENT", "90".into());
            }
            assert!(bind(parse(&values).unwrap()).is_ok());
            for invalid in ["", "0", "1", "TRUE", " true", "invalid"] {
                values.insert(flag, invalid.into());
                assert!(parse(&values).unwrap_err().contains(flag));
            }
        }
    }

    #[test]
    fn omitted_policy_expiry_binds_both_fee_directions_without_a_deadline() {
        for l1_enabled in [false, true] {
            for l2_enabled in [false, true] {
                let mut values = model();
                values.remove("EEZ_FEES_VALID_UNTIL_L1_TIMESTAMP");
                values.insert("EEZ_FEES_L1_TO_L2_ENABLED", l1_enabled.to_string());
                values.insert("EEZ_FEES_L2_TO_L1_ENABLED", l2_enabled.to_string());
                values.insert("EEZ_FEES_L1_REFUND_PERCENT", "90".into());
                values.insert(
                    "EEZ_FEES_L1_REFUND_RECIPIENT",
                    Address::repeat_byte(9).to_string(),
                );
                let runtime = bind(parse(&values).unwrap()).unwrap();
                assert_eq!(runtime.l2_to_l1_enabled, l2_enabled);
                assert_eq!(runtime.l1_refund.is_some(), l1_enabled);
                let policy = runtime.pricing.unwrap();
                assert!(policy.quote_eligible(Address::repeat_byte(5), Address::ZERO, u64::MAX));
                assert_eq!(
                    policy.details()["assumptions"].get("valid_until_l1_timestamp"),
                    Some(&serde_json::Value::Null)
                );
            }
        }
    }

    #[test]
    fn obsolete_settings_are_rejected_even_empty_disabled_or_mixed() {
        for name in [
            "EEZ_L2_FEE_SHADOW_POLICY",
            "EEZ_L2_FEE_ENFORCE_TARIFF",
            "EEZ_L1_FEE_REFUND_POLICY",
        ] {
            for value in ["", "false", "{}", "true"] {
                for mut values in [HashMap::new(), model()] {
                    values.insert(name, value.into());
                    let error = parse(&values).unwrap_err();
                    assert!(error.contains(name) && error.contains("has been removed"));
                }
            }
        }
    }

    #[test]
    fn monetary_assumptions_are_complete_bounded_and_never_defaulted() {
        for name in pricing::Config::SETTINGS {
            let mut values = model();
            values.remove(name);
            if name == "EEZ_FEES_VALID_UNTIL_L1_TIMESTAMP" {
                assert!(parse(&values).is_ok());
            } else {
                assert!(parse(&values).unwrap_err().contains(name), "missing {name}");
            }
            if name != "EEZ_FEES_EXCLUDED_SENDERS" {
                for invalid in [
                    "",
                    "-1",
                    "+1",
                    "1.5",
                    "0x1",
                    " 1",
                    "invalid",
                    &"9".repeat(79),
                ] {
                    values.insert(name, invalid.into());
                    assert!(
                        parse(&values).unwrap_err().contains(name),
                        "{name}={invalid}"
                    );
                }
            }
        }
        for name in [
            "EEZ_FEES_L1_PER_L2_NUMERATOR",
            "EEZ_FEES_L1_PER_L2_DENOMINATOR",
            "EEZ_FEES_VALID_UNTIL_L1_TIMESTAMP",
        ] {
            let mut values = model();
            values.insert(name, "0".into());
            assert!(parse(&values).unwrap_err().contains(name));
        }
        for name in ["EEZ_FEES_RISK_BPS", "EEZ_FEES_PROFIT_BPS"] {
            let mut values = model();
            values.insert(name, "10000".into());
            assert!(parse(&values).is_ok());
            values.insert(name, "10001".into());
            assert!(parse(&values).unwrap_err().contains(name));
        }
        let mut values = model();
        for name in [
            "EEZ_FEES_PROVING_COST_L1",
            "EEZ_FEES_OPERATIONS_COST_L1",
            "EEZ_FEES_TREASURY_COST_L1",
            "EEZ_FEES_RISK_BPS",
            "EEZ_FEES_PROFIT_BPS",
        ] {
            values.insert(name, "0".into());
        }
        let policy = bind(parse(&values).unwrap()).unwrap().pricing.unwrap();
        assert_eq!(
            policy
                .required_l1_payment(alloy_primitives::U256::from(100))
                .unwrap()
                .l1,
            alloy_primitives::U256::from(100)
        );
    }

    #[test]
    fn explicit_expiry_requires_complete_costs_and_fits_positive_u64() {
        let name = "EEZ_FEES_VALID_UNTIL_L1_TIMESTAMP";
        let values = HashMap::from([(name, "1000".into())]);
        assert!(
            parse(&values)
                .unwrap_err()
                .contains("EEZ_FEES_L1_PER_L2_NUMERATOR")
        );

        let mut values = model();
        values.insert(name, u64::MAX.to_string());
        assert!(
            bind(parse(&values).unwrap())
                .unwrap()
                .pricing
                .unwrap()
                .quote_eligible(Address::repeat_byte(5), Address::ZERO, u64::MAX)
        );
        values.insert(name, (u128::from(u64::MAX) + 1).to_string());
        assert!(parse(&values).unwrap_err().contains("exceeds u64"));
    }

    #[test]
    fn exclusions_are_explicit_distinct_and_bounded() {
        let mut values = model();
        let name = "EEZ_FEES_EXCLUDED_SENDERS";
        let address = Address::repeat_byte(6).to_string();
        for raw in [
            String::new(),
            address.clone(),
            (1..=64)
                .map(|i| Address::repeat_byte(i).to_string())
                .collect::<Vec<_>>()
                .join(","),
        ] {
            values.insert(name, raw);
            assert!(parse(&values).is_ok());
        }
        for raw in [
            "bad".into(),
            " ".into(),
            format!("{address},"),
            format!("{address},{address}"),
            " ".repeat(4097),
            (1..=65)
                .map(|i| Address::repeat_byte(i).to_string())
                .collect::<Vec<_>>()
                .join(","),
        ] {
            values.insert(name, raw);
            assert!(parse(&values).unwrap_err().contains(name));
        }
    }

    #[test]
    fn binding_requires_unambiguous_network_and_nonzero_beneficiary() {
        for (l1, l2, rollups, beneficiary) in [
            (0, 9, vec![1], Address::repeat_byte(4)),
            (1, 0, vec![1], Address::repeat_byte(4)),
            (1, 9, vec![], Address::repeat_byte(4)),
            (1, 9, vec![0], Address::repeat_byte(4)),
            (1, 9, vec![1, 2], Address::repeat_byte(4)),
            (1, 9, vec![1], Address::ZERO),
        ] {
            assert!(
                parse(&model())
                    .unwrap()
                    .bind(l1, l2, rollups, beneficiary)
                    .is_err()
            );
        }
        let policy = bind(parse(&model()).unwrap()).unwrap().pricing.unwrap();
        assert_eq!(
            policy.details()["assumptions"],
            pricing::tests::policy_json()
        );
        assert_eq!(
            policy.details()["id"],
            serde_json::json!(alloy_primitives::keccak256(
                pricing::tests::policy_json().to_string()
            ))
        );
    }
}
