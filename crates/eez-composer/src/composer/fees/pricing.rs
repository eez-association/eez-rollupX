//! Local, hypothetical pricing of a fully executed candidate. A configured
//! conversion is an operator assumption, not evidence of treasury realization.

use alloy_primitives::{Address, B256, U256, U512, keccak256};
use serde_json::{Value, json};

use super::{L1Fees, L2Fees};

/// Immutable startup policy. Explicit costs may be zero, but missing costs or
/// conversion never acquire a default. Amounts use each chain's base units.
#[derive(Debug, Clone)]
pub(crate) struct Policy {
    id: B256,
    assumptions: Value,
    l1_chain_id: u64,
    l2_chain_id: u64,
    rollup_id: u64,
    beneficiary: Address,
    costs: Config,
}

/// Operator assumptions; network identity is supplied by runtime wiring.
#[derive(Debug, Clone)]
pub(crate) struct Config {
    valid_until_l1_timestamp: u64,
    numerator: U256,
    denominator: U256,
    proving: U256,
    operations: U256,
    treasury: U256,
    risk_bps: u64,
    profit_bps: u64,
    excluded_senders: Vec<Address>,
}

impl Config {
    pub const SETTINGS: [&'static str; 9] = [
        "EEZ_FEES_VALID_UNTIL_L1_TIMESTAMP",
        "EEZ_FEES_L1_PER_L2_NUMERATOR",
        "EEZ_FEES_L1_PER_L2_DENOMINATOR",
        "EEZ_FEES_PROVING_COST_L1",
        "EEZ_FEES_OPERATIONS_COST_L1",
        "EEZ_FEES_TREASURY_COST_L1",
        "EEZ_FEES_RISK_BPS",
        "EEZ_FEES_PROFIT_BPS",
        "EEZ_FEES_EXCLUDED_SENDERS",
    ];

    pub fn parse(
        mut get: impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        let values = Self::SETTINGS
            .into_iter()
            .map(|name| get(name).map(|value| (name, value)))
            .collect::<Result<std::collections::HashMap<_, _>, _>>()?;
        if values.values().all(Option::is_none) {
            return Ok(None);
        }
        let required = |name: &str| {
            values
                .get(name)
                .and_then(|value| value.as_deref())
                .ok_or_else(|| {
                    format!("{name} is required when any fee cost assumption is supplied")
                })
        };
        let integer = |name| -> Result<u64, String> {
            let raw = required(name)?;
            if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("{name} must be an unsigned decimal integer"));
            }
            raw.parse().map_err(|_| format!("{name} exceeds u64"))
        };
        let amount = |name| -> Result<U256, String> {
            let raw = required(name)?;
            if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!("{name} must be an unsigned decimal integer"));
            }
            U256::from_str_radix(raw, 10).map_err(|_| format!("{name} exceeds U256"))
        };
        let valid_until_l1_timestamp = integer("EEZ_FEES_VALID_UNTIL_L1_TIMESTAMP")?;
        if valid_until_l1_timestamp == 0 {
            return Err("EEZ_FEES_VALID_UNTIL_L1_TIMESTAMP must be positive".into());
        }
        let numerator = amount("EEZ_FEES_L1_PER_L2_NUMERATOR")?;
        let denominator = amount("EEZ_FEES_L1_PER_L2_DENOMINATOR")?;
        if numerator.is_zero() || denominator.is_zero() {
            return Err(
                "EEZ_FEES_L1_PER_L2_NUMERATOR and EEZ_FEES_L1_PER_L2_DENOMINATOR must be positive"
                    .into(),
            );
        }
        let risk_bps = integer("EEZ_FEES_RISK_BPS")?;
        let profit_bps = integer("EEZ_FEES_PROFIT_BPS")?;
        if risk_bps > 10_000 || profit_bps > 10_000 {
            return Err(
                "EEZ_FEES_RISK_BPS and EEZ_FEES_PROFIT_BPS must be between 0 and 10000".into(),
            );
        }
        let raw = required("EEZ_FEES_EXCLUDED_SENDERS")?;
        // At most 64 comma-separated addresses, allowing surrounding whitespace.
        if raw.len() > 4096 {
            return Err("EEZ_FEES_EXCLUDED_SENDERS exceeds 4096 bytes".into());
        }
        let mut excluded_senders = Vec::new();
        if !raw.is_empty() {
            for raw in raw.split(',') {
                let sender = raw.trim().parse::<Address>().map_err(
                    |_| "EEZ_FEES_EXCLUDED_SENDERS must be a comma-separated address list",
                )?;
                if excluded_senders.contains(&sender) || excluded_senders.len() == 64 {
                    return Err(
                        "EEZ_FEES_EXCLUDED_SENDERS accepts at most 64 distinct addresses".into(),
                    );
                }
                excluded_senders.push(sender);
            }
        }
        Ok(Some(Self {
            valid_until_l1_timestamp,
            numerator,
            denominator,
            proving: amount("EEZ_FEES_PROVING_COST_L1")?,
            operations: amount("EEZ_FEES_OPERATIONS_COST_L1")?,
            treasury: amount("EEZ_FEES_TREASURY_COST_L1")?,
            risk_bps,
            profit_bps,
            excluded_senders,
        }))
    }

    /// The shipping node wires one L2 rollup. Do not assign one policy to an
    /// arbitrary rollup if that wiring changes in the future.
    pub fn bind(
        self,
        l1_chain_id: u64,
        l2_chain_id: u64,
        owned_rollups: impl IntoIterator<Item = u64>,
        beneficiary: Address,
    ) -> Result<Policy, String> {
        let mut rollups = owned_rollups.into_iter();
        let rollup_id = rollups
            .next()
            .filter(|id| *id > 0)
            .ok_or("fee pricing requires an owned L2 rollup")?;
        if rollups.next().is_some() || l1_chain_id == 0 || l2_chain_id == 0 {
            return Err(
                "fee pricing requires positive runtime chain IDs and exactly one owned L2 rollup"
                    .into(),
            );
        }
        if beneficiary.is_zero() {
            return Err("fee pricing requires a nonzero EEZ_L2_FEE_RECIPIENT".into());
        }
        let assumptions = json!({
            "version": 1, "l1_chain_id": l1_chain_id, "l2_chain_id": l2_chain_id,
            "rollup_id": rollup_id, "beneficiary": beneficiary,
            "valid_until_l1_timestamp": self.valid_until_l1_timestamp,
            "l1_per_l2_numerator": self.numerator.to_string(),
            "l1_per_l2_denominator": self.denominator.to_string(),
            "proving_cost_l1": self.proving.to_string(),
            "operations_cost_l1": self.operations.to_string(),
            "treasury_cost_l1": self.treasury.to_string(),
            "risk_bps": self.risk_bps, "profit_bps": self.profit_bps,
            "excluded_senders": self.excluded_senders,
        });
        Ok(Policy {
            id: keccak256(assumptions.to_string()),
            assumptions,
            l1_chain_id,
            l2_chain_id,
            rollup_id,
            beneficiary,
            costs: self,
        })
    }
}

pub(crate) struct RequiredL1Payment {
    pub subtotal: U256,
    pub risk: U256,
    pub profit: U256,
    pub l1: U256,
    pub minimum_l1: U256,
}

pub(crate) struct RequiredPayment {
    pub subtotal: U256,
    pub risk: U256,
    pub profit: U256,
    pub l1: U256,
    pub l2: U256,
    pub minimum_l1: U256,
    pub minimum_l2: U256,
}

impl Policy {
    pub(crate) fn details(&self) -> Value {
        json!({"id": self.id, "assumptions": self.assumptions})
    }

    pub(crate) fn quote_eligible(&self, sender: Address, poster: Address, target: u64) -> bool {
        target <= self.costs.valid_until_l1_timestamp
            && sender != poster
            && sender != self.beneficiary
            && !self.costs.excluded_senders.contains(&sender)
    }

    pub(crate) fn required_l1_payment(
        &self,
        post_cost: U256,
    ) -> Result<RequiredL1Payment, &'static str> {
        let subtotal = [
            post_cost,
            self.costs.proving,
            self.costs.operations,
            self.costs.treasury,
        ]
        .into_iter()
        .try_fold(U256::ZERO, |sum, cost| {
            sum.checked_add(cost).ok_or("pricing_cost_overflow")
        })?;
        let risk = mul_div(
            subtotal,
            U256::from(self.costs.risk_bps),
            U256::from(10_000),
            true,
        )?;
        let profit = mul_div(
            subtotal,
            U256::from(self.costs.profit_bps),
            U256::from(10_000),
            true,
        )?;
        let minimum_l1 = subtotal.checked_add(risk).ok_or("pricing_cost_overflow")?;
        let l1 = minimum_l1
            .checked_add(profit)
            .ok_or("pricing_cost_overflow")?;
        Ok(RequiredL1Payment {
            subtotal,
            risk,
            profit,
            l1,
            minimum_l1,
        })
    }

    pub(crate) fn required_payment(
        &self,
        post_cost: U256,
    ) -> Result<RequiredPayment, &'static str> {
        let RequiredL1Payment {
            subtotal,
            risk,
            profit,
            l1,
            minimum_l1,
        } = self.required_l1_payment(post_cost)?;
        let minimum_l2 = mul_div(
            minimum_l1,
            self.costs.denominator,
            self.costs.numerator,
            true,
        )?;
        let l2 = mul_div(l1, self.costs.denominator, self.costs.numerator, true)?;
        Ok(RequiredPayment {
            subtotal,
            risk,
            profit,
            l1,
            l2,
            minimum_l1,
            minimum_l2,
        })
    }

    /// Preserve policy identity even when evidence is unavailable. These
    /// diagnostics must never become admission or request-ownership decisions.
    pub(super) fn observe(
        &self,
        l1: &Result<L1Fees, &'static str>,
        l2: Option<&Result<L2Fees, &'static str>>,
        poster: Address,
    ) -> Value {
        let calculate = || {
            let l1 = l1.as_ref().map_err(|error| *error)?;
            let l2 = l2
                .ok_or("terminal_receipts_not_captured")?
                .as_ref()
                .map_err(|error| *error)?;
            self.calculate(l1, l2, poster)
        };
        let mut report =
            calculate().unwrap_or_else(|reason| json!({"status":"unavailable", "reason":reason}));
        report["policy_id"] = json!(self.id);
        report["assumptions"] = self.assumptions.clone();
        report["poster_excluded_from_revenue"] = json!(poster);
        report
    }

    fn calculate(&self, l1: &L1Fees, l2: &L2Fees, poster: Address) -> Result<Value, &'static str> {
        self.validate_evidence(l1, l2)?;
        let payment = self.required_payment(l1.post_cost)?;
        let eligible = self.eligible_tips(l2, poster)?;
        self.report(l1, l2, payment, eligible)
    }

    /// A dispatch tariff check uses typed native receipt amounts. It does not
    /// turn the configured conversion into realized or spendable L1 revenue.
    pub(super) fn check_tariff(
        &self,
        l1: &L1Fees,
        l2: &L2Fees,
        poster: Address,
    ) -> Result<(), &'static str> {
        self.validate_evidence(l1, l2)?;
        let payment = self.required_payment(l1.post_cost)?;
        let (tips, _) = self.eligible_tips(l2, poster)?;
        // Profit is a quote target. Only refreshed cost and the explicit risk
        // reserve are mandatory; normal price drift may consume the markup.
        if tips < payment.minimum_l2 {
            return Err("l2_tip_underfunded");
        }
        Ok(())
    }

    fn validate_evidence(&self, l1: &L1Fees, l2: &L2Fees) -> Result<(), &'static str> {
        if l1.chain_id != self.l1_chain_id
            || l2.chain_id != self.l2_chain_id
            || l2.rollup_id != self.rollup_id
        {
            return Err("pricing_network_mismatch");
        }
        if l1.target_timestamp > self.costs.valid_until_l1_timestamp {
            return Err("pricing_policy_expired");
        }
        if l2.beneficiary != self.beneficiary {
            return Err("pricing_beneficiary_mismatch");
        }
        Ok(())
    }

    fn eligible_tips(&self, l2: &L2Fees, poster: Address) -> Result<(U256, usize), &'static str> {
        let mut eligible_tips = U256::ZERO;
        let mut eligible_sources = 0;
        for source in &l2.sources {
            if source.sender == poster
                || source.sender == self.beneficiary
                || self.costs.excluded_senders.contains(&source.sender)
            {
                continue;
            }
            eligible_sources += 1;
            eligible_tips = eligible_tips
                .checked_add(source.tip)
                .ok_or("pricing_revenue_overflow")?;
        }
        Ok((eligible_tips, eligible_sources))
    }

    fn report(
        &self,
        l1: &L1Fees,
        l2: &L2Fees,
        payment: RequiredPayment,
        (eligible_tips, eligible_sources): (U256, usize),
    ) -> Result<Value, &'static str> {
        let RequiredPayment {
            subtotal,
            risk,
            profit,
            l1: required_l1,
            l2: required_l2,
            minimum_l1,
            minimum_l2,
        } = payment;
        let excluded_tips = l2
            .tip
            .checked_sub(eligible_tips)
            .ok_or("pricing_revenue_mismatch")?;
        let recovery = mul_div(
            eligible_tips,
            self.costs.numerator,
            self.costs.denominator,
            false,
        )?;
        let (comparison, shortfall, surplus) = if recovery < required_l1 {
            ("shortfall", required_l1 - recovery, U256::ZERO)
        } else {
            ("covered", U256::ZERO, recovery - required_l1)
        };
        // Aggregate receipt gas cannot quote an independently submitted source.
        // Even the single-source result is bound to the current signed fees:
        // GASPRICE-dependent execution requires a fresh simulation after edits.
        let (tip_status, required_tip, min_fee_cap) =
            if l2.sources.len() != 1 || l1.companion_count != 0 {
                ("standalone_estimate_required", None, None)
            } else if eligible_sources != 1 {
                ("no_external_source", None, None)
            } else if l2.sources[0].gas_used == 0 {
                ("zero_source_gas", None, None)
            } else {
                let tip = mul_div(
                    required_l2,
                    U256::from(1),
                    U256::from(l2.sources[0].gas_used),
                    true,
                )?;
                let cap = tip
                    .checked_add(U256::from(l2.base_fee_per_gas))
                    .ok_or("pricing_fee_cap_overflow")?;
                if cap > U256::from(u128::MAX) {
                    ("fee_fields_out_of_range", None, None)
                } else {
                    (
                        "single_source_candidate",
                        Some(tip.to_string()),
                        Some(cap.to_string()),
                    )
                }
            };
        Ok(json!({
            "status":"calculated", "scope":"complete_prepared_candidate",
            "subtotal_l1":subtotal.to_string(), "risk_allowance_l1":risk.to_string(),
            "profit_allowance_l1":profit.to_string(),
            "required_service_payment_l1":required_l1.to_string(),
            "required_service_payment_l2":required_l2.to_string(),
            "minimum_service_payment_l1":minimum_l1.to_string(),
            "minimum_service_payment_l2":minimum_l2.to_string(),
            "cost_coverage":if recovery >= minimum_l1 {"covered"} else {"shortfall"},
            "eligible_source_count":eligible_sources,
            "eligible_priority_fees_l2":eligible_tips.to_string(),
            "excluded_priority_fees_l2":excluded_tips.to_string(),
            "projected_recovery_l1":recovery.to_string(), "comparison":comparison,
            "shortfall_l1":shortfall.to_string(), "surplus_l1":surplus.to_string(),
            "tip_status":tip_status, "required_tip_per_gas":required_tip,
            "minimum_max_fee_per_gas_at_current_base":min_fee_cap,
            "source_hash": if l2.sources.len() == 1 { Some(l2.sources[0].hash) } else { None },
        }))
    }
}

/// Widen the product so a representable quotient does not fail merely because
/// intermediate multiplication exceeded U256. Required payments round up;
/// speculative receipts round down. No saturating monetary arithmetic.
fn mul_div(a: U256, b: U256, divisor: U256, round_up: bool) -> Result<U256, &'static str> {
    if divisor.is_zero() {
        return Err("pricing_zero_divisor");
    }
    let product = U512::from(a) * U512::from(b);
    let divisor = U512::from(divisor);
    let quotient = product / divisor;
    let result = if round_up && product % divisor != U512::ZERO {
        quotient
            .checked_add(U512::from(1))
            .ok_or("pricing_conversion_overflow")?
    } else {
        quotient
    };
    U256::checked_from_limbs_slice(result.as_limbs()).ok_or("pricing_conversion_overflow")
}

#[cfg(test)]
pub(in crate::composer) mod tests {
    use super::*;
    use crate::composer::fees::SourceFees;

    pub(in crate::composer) fn policy_json() -> Value {
        json!({
            "version":1, "l1_chain_id":1, "l2_chain_id":9, "rollup_id":1,
            "beneficiary":Address::repeat_byte(4), "valid_until_l1_timestamp":1000,
            "l1_per_l2_numerator":"2", "l1_per_l2_denominator":"3",
            "proving_cost_l1":"7", "operations_cost_l1":"5", "treasury_cost_l1":"3",
            "risk_bps":1000, "profit_bps":500, "excluded_senders":[Address::repeat_byte(6)],
        })
    }

    pub(in crate::composer) fn settings(value: &Value) -> Vec<(&'static str, String)> {
        Config::SETTINGS
            .into_iter()
            .map(|name| {
                let key = name.strip_prefix("EEZ_FEES_").unwrap().to_ascii_lowercase();
                let field = &value[&key];
                let raw = if key == "excluded_senders" {
                    field
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap())
                        .collect::<Vec<_>>()
                        .join(",")
                } else {
                    field
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| field.to_string())
                };
                (name, raw)
            })
            .collect()
    }

    pub(in crate::composer) fn parse(value: Value) -> Result<Policy, String> {
        let values = settings(&value);
        Config::parse(|name| {
            Ok(values
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.clone()))
        })?
        .unwrap()
        .bind(1, 9, [1], Address::repeat_byte(4))
    }

    fn candidate() -> (L1Fees, L2Fees) {
        (
            L1Fees {
                chain_id: 1,
                target_timestamp: 1000,
                companion_count: 0,
                post_cost: U256::from(101),
                detail: Value::Null,
            },
            L2Fees {
                chain_id: 9,
                rollup_id: 1,
                block_hash: B256::repeat_byte(1),
                parent_hash: B256::ZERO,
                block_number: 5,
                beneficiary: Address::repeat_byte(4),
                base_fee_per_gas: 10,
                ordinary_gas_used: 100,
                native_gas_used: 1000,
                burn: U256::from(1000),
                tip: U256::from(100),
                sources: vec![SourceFees {
                    hash: B256::repeat_byte(2),
                    sender: Address::repeat_byte(5),
                    gas_used: 100,
                    gas_limit: 1_000_000,
                    priority_fee_per_gas: 1,
                    burn: U256::from(1000),
                    tip: U256::from(100),
                }],
            },
        )
    }

    #[test]
    fn target_profit_is_optional_but_costs_and_risk_are_mandatory() {
        let mut config = policy_json();
        config["risk_bps"] = json!(0);
        config["profit_bps"] = json!(2000);
        config["l1_per_l2_numerator"] = json!(1.to_string());
        config["l1_per_l2_denominator"] = json!(1.to_string());
        let policy = parse(config.clone()).unwrap();
        let (l1, mut l2) = candidate();
        // All costs: 101 + 7 + 5 + 3 = 116. Quote 140; 122 pays ~5%
        // surplus, and 117 pays less than 1%. Both remain above cost.
        assert_eq!(
            policy.required_payment(l1.post_cost).unwrap().l2,
            U256::from(140)
        );
        for payment in [140, 122, 117, 116] {
            l2.sources[0].tip = U256::from(payment);
            l2.tip = U256::from(payment);
            assert_eq!(policy.check_tariff(&l1, &l2, Address::ZERO), Ok(()));
        }
        l2.sources[0].tip = U256::from(115);
        l2.tip = U256::from(115);
        assert_eq!(
            policy.check_tariff(&l1, &l2, Address::ZERO),
            Err("l2_tip_underfunded")
        );
        config["risk_bps"] = json!(500);
        let policy = parse(config).unwrap();
        for (payment, expected) in [(121, Err("l2_tip_underfunded")), (122, Ok(()))] {
            l2.sources[0].tip = U256::from(payment);
            l2.tip = U256::from(payment);
            assert_eq!(policy.check_tariff(&l1, &l2, Address::ZERO), expected);
        }
    }

    #[test]
    fn dispatch_tariff_uses_actual_external_receipts_and_rechecks_context() {
        let policy = parse(policy_json()).unwrap();
        let (mut l1, mut l2) = candidate();
        assert_eq!(
            policy.check_tariff(&l1, &l2, Address::ZERO),
            Err("l2_tip_underfunded")
        );
        l2.sources[0].tip = U256::from(192);
        l2.tip = U256::from(192);
        assert!(policy.check_tariff(&l1, &l2, Address::ZERO).is_ok());
        l1.post_cost += U256::from(1);
        assert_eq!(
            policy.check_tariff(&l1, &l2, Address::ZERO),
            Err("l2_tip_underfunded")
        );
        l1.post_cost -= U256::from(1);
        assert_eq!(
            policy.check_tariff(&l1, &l2, l2.sources[0].sender),
            Err("l2_tip_underfunded")
        );
        l2.beneficiary = Address::ZERO;
        assert_eq!(
            policy.check_tariff(&l1, &l2, Address::ZERO),
            Err("pricing_beneficiary_mismatch")
        );
        l2.beneficiary = Address::repeat_byte(4);
        l1.target_timestamp += 1;
        assert_eq!(
            policy.check_tariff(&l1, &l2, Address::ZERO),
            Err("pricing_policy_expired")
        );
    }

    #[test]
    fn standalone_pricing_includes_all_costs_and_rounds_in_the_safe_direction() {
        let policy = parse(policy_json()).unwrap();
        let (l1, mut l2) = candidate();
        let price = policy.calculate(&l1, &l2, Address::ZERO).unwrap();
        // Independent tiny-unit example: 101+7+5+3=116; ceil(10%)=12,
        // ceil(5%)=6. Required 134 L1 = 201 L2. Only 100 source gas pays.
        for (field, value) in [
            ("subtotal_l1", "116"),
            ("risk_allowance_l1", "12"),
            ("profit_allowance_l1", "6"),
            ("required_service_payment_l1", "134"),
            ("required_service_payment_l2", "201"),
            ("projected_recovery_l1", "66"),
            ("shortfall_l1", "68"),
            ("surplus_l1", "0"),
            ("required_tip_per_gas", "3"),
            ("minimum_max_fee_per_gas_at_current_base", "13"),
        ] {
            assert_eq!(price[field], value, "{field}");
        }
        assert_eq!(price["comparison"], "shortfall");
        assert_eq!(price["tip_status"], "single_source_candidate");
        // Halving charged source gas raises the required rate despite a
        // generous unchanged signed limit and native gas usage.
        l2.sources[0].gas_used = 50;
        l2.sources[0].tip = U256::from(50);
        l2.tip = U256::from(50);
        let price = policy.calculate(&l1, &l2, Address::ZERO).unwrap();
        assert_eq!(price["required_tip_per_gas"], "5");
        assert_eq!(price["shortfall_l1"], "101");
        l2.sources[0].tip = U256::from(250);
        l2.tip = U256::from(250);
        let covered = policy.calculate(&l1, &l2, Address::ZERO).unwrap();
        assert_eq!(covered["comparison"], "covered");
        assert_eq!(covered["surplus_l1"], "32");
    }

    #[test]
    fn operator_tips_and_wrong_beneficiary_cannot_supply_projected_revenue() {
        let policy = parse(policy_json()).unwrap();
        let (l1, mut l2) = candidate();
        for sender in [
            Address::repeat_byte(4),
            Address::repeat_byte(6),
            Address::repeat_byte(7),
        ] {
            l2.sources[0].sender = sender;
            let price = policy.calculate(&l1, &l2, Address::repeat_byte(7)).unwrap();
            assert_eq!(price["eligible_priority_fees_l2"], "0");
            assert_eq!(price["excluded_priority_fees_l2"], "100");
            assert_eq!(price["projected_recovery_l1"], "0");
            assert_eq!(price["tip_status"], "no_external_source");
            assert!(price["required_tip_per_gas"].is_null());
        }
        l2.beneficiary = Address::repeat_byte(8);
        assert_eq!(
            policy.calculate(&l1, &l2, Address::ZERO).unwrap_err(),
            "pricing_beneficiary_mismatch"
        );
    }

    #[test]
    fn shared_and_mixed_candidates_have_comparisons_but_no_standalone_tip() {
        let policy = parse(policy_json()).unwrap();
        let (mut l1, mut l2) = candidate();
        l1.companion_count = 1;
        let mixed = policy.calculate(&l1, &l2, Address::ZERO).unwrap();
        assert_eq!(mixed["tip_status"], "standalone_estimate_required");
        assert_eq!(mixed["comparison"], "shortfall");
        assert!(mixed["required_tip_per_gas"].is_null());
        l1.companion_count = 0;
        let (_, mut other) = candidate();
        let mut source = other.sources.remove(0);
        source.hash = B256::repeat_byte(3);
        source.tip = U256::from(200);
        l2.sources.push(source);
        l2.tip += U256::from(200);
        let shared = policy.calculate(&l1, &l2, Address::ZERO).unwrap();
        assert_eq!(shared["eligible_source_count"], 2);
        assert_eq!(shared["projected_recovery_l1"], "200");
        assert_eq!(shared["comparison"], "covered");
        assert!(shared["required_tip_per_gas"].is_null());
    }

    #[test]
    fn expired_mismatched_or_missing_evidence_never_yields_a_price() {
        let policy = parse(policy_json()).unwrap();
        let (mut l1, l2) = candidate();
        l1.target_timestamp = 1001;
        assert_eq!(
            policy.calculate(&l1, &l2, Address::ZERO).unwrap_err(),
            "pricing_policy_expired"
        );
        l1.target_timestamp = 1000;
        l1.chain_id = 2;
        assert_eq!(
            policy.calculate(&l1, &l2, Address::ZERO).unwrap_err(),
            "pricing_network_mismatch"
        );
        for report in [
            policy.observe(
                &Err("l1_target_parent_mismatch"),
                Some(&Ok(l2)),
                Address::ZERO,
            ),
            policy.observe(&Ok(l1), None, Address::ZERO),
        ] {
            assert_eq!(report["status"], "unavailable");
            assert_eq!(report["policy_id"], json!(policy.id));
            assert!(report.get("required_service_payment_l1").is_none());
        }
    }

    #[test]
    fn zero_gas_unrepresentable_fees_and_cost_overflow_are_explicit() {
        let policy = parse(policy_json()).unwrap();
        let (mut l1, mut l2) = candidate();
        l2.sources[0].gas_used = 0;
        assert_eq!(
            policy.calculate(&l1, &l2, Address::ZERO).unwrap()["tip_status"],
            "zero_source_gas"
        );
        l2.sources[0].gas_used = 1;
        l1.post_cost = U256::from(u128::MAX);
        let report = policy.calculate(&l1, &l2, Address::ZERO).unwrap();
        assert_eq!(report["tip_status"], "fee_fields_out_of_range");
        assert!(report["required_tip_per_gas"].is_null());
        l1.post_cost = U256::MAX;
        assert_eq!(
            policy.calculate(&l1, &l2, Address::ZERO).unwrap_err(),
            "pricing_cost_overflow"
        );
    }

    #[test]
    fn integer_conversion_neither_underquotes_nor_overstates_receipts() {
        for required in 1_u64..80 {
            for (n, d) in [(1, 1), (2, 3), (3, 2), (7, 11)] {
                let up = mul_div(U256::from(required), U256::from(d), U256::from(n), true).unwrap();
                assert_eq!(up, U256::from((required * d).div_ceil(n)));
                let realized = mul_div(up, U256::from(n), U256::from(d), false).unwrap();
                assert!(realized >= U256::from(required));
                let less =
                    mul_div(up - U256::from(1), U256::from(n), U256::from(d), false).unwrap();
                assert!(less < U256::from(required));
            }
        }
        assert_eq!(
            mul_div(U256::MAX, U256::MAX, U256::MAX, true).unwrap(),
            U256::MAX
        );
        assert_eq!(
            mul_div(U256::MAX, U256::from(2), U256::from(1), false).unwrap_err(),
            "pricing_conversion_overflow"
        );
        assert_eq!(
            mul_div(U256::from(1), U256::from(1), U256::ZERO, true).unwrap_err(),
            "pricing_zero_divisor"
        );
    }
}
