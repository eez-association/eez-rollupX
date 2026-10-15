//! Local, hypothetical pricing of a fully executed candidate. A configured
//! conversion is an operator assumption, not evidence of treasury realization.

use alloy_primitives::{Address, B256, U256, U512, keccak256};
use serde_json::{Value, json};

use super::{L1Fees, L2Fees};

pub(crate) const SETTING: &str = "EEZ_L2_FEE_SHADOW_POLICY";

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
        target <= self.valid_until_l1_timestamp
            && sender != poster
            && sender != self.beneficiary
            && !self.excluded_senders.contains(&sender)
    }

    pub(crate) fn required_l1_payment(
        &self,
        post_cost: U256,
    ) -> Result<RequiredL1Payment, &'static str> {
        let subtotal = [post_cost, self.proving, self.operations, self.treasury]
            .into_iter()
            .try_fold(U256::ZERO, |sum, cost| {
                sum.checked_add(cost).ok_or("pricing_cost_overflow")
            })?;
        let risk = mul_div(
            subtotal,
            U256::from(self.risk_bps),
            U256::from(10_000),
            true,
        )?;
        let profit = mul_div(
            subtotal,
            U256::from(self.profit_bps),
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
        let minimum_l2 = mul_div(minimum_l1, self.denominator, self.numerator, true)?;
        let l2 = mul_div(l1, self.denominator, self.numerator, true)?;
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

    pub fn parse(raw: Option<String>, fee_recipient: Address) -> Result<Option<Self>, String> {
        let Some(raw) = raw else { return Ok(None) };
        let parse = || -> Result<Self, String> {
            if raw.len() > 8_192 {
                return Err("JSON exceeds 8192 bytes".into());
            }
            let value: Value = serde_json::from_str(&raw).map_err(|_| "invalid JSON object")?;
            let fields = value.as_object().ok_or("expected a JSON object")?;
            const FIELDS: &[&str] = &[
                "version",
                "l1_chain_id",
                "l2_chain_id",
                "rollup_id",
                "beneficiary",
                "valid_until_l1_timestamp",
                "l1_per_l2_numerator",
                "l1_per_l2_denominator",
                "proving_cost_l1",
                "operations_cost_l1",
                "treasury_cost_l1",
                "risk_bps",
                "profit_bps",
                "excluded_senders",
            ];
            if fields.len() != FIELDS.len()
                || fields.keys().any(|key| !FIELDS.contains(&key.as_str()))
            {
                return Err(
                    "all documented fields are required; unknown fields are rejected".into(),
                );
            }
            let integer = |key: &str| {
                value[key]
                    .as_u64()
                    .ok_or_else(|| format!("{key} must be a u64 integer"))
            };
            let positive = |key: &str| {
                integer(key).and_then(|n| {
                    if n > 0 {
                        Ok(n)
                    } else {
                        Err(format!("{key} must be positive"))
                    }
                })
            };
            let amount = |key: &str| -> Result<U256, String> {
                let s = value[key]
                    .as_str()
                    .ok_or_else(|| format!("{key} must be a decimal string"))?;
                if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                    return Err(format!("{key} must be an unsigned decimal string"));
                }
                U256::from_str_radix(s, 10).map_err(|_| format!("{key} exceeds U256"))
            };
            if integer("version")? != 1 {
                return Err("unsupported policy version".into());
            }
            let beneficiary: Address = value["beneficiary"]
                .as_str()
                .ok_or("invalid beneficiary")?
                .parse()
                .map_err(|_| "invalid beneficiary")?;
            if beneficiary.is_zero() || beneficiary != fee_recipient {
                return Err("beneficiary must equal the nonzero EEZ_L2_FEE_RECIPIENT".into());
            }
            let numerator = amount("l1_per_l2_numerator")?;
            let denominator = amount("l1_per_l2_denominator")?;
            if numerator.is_zero() || denominator.is_zero() {
                return Err("conversion numerator and denominator must be positive".into());
            }
            let risk_bps = integer("risk_bps")?;
            let profit_bps = integer("profit_bps")?;
            if risk_bps > 10_000 || profit_bps > 10_000 {
                return Err("risk_bps and profit_bps must be between 0 and 10000".into());
            }
            let excluded = value["excluded_senders"]
                .as_array()
                .ok_or("excluded_senders must be an array")?;
            if excluded.len() > 64 {
                return Err("at most 64 excluded_senders are supported".into());
            }
            let mut excluded_senders = Vec::with_capacity(excluded.len());
            for sender in excluded {
                let sender = sender
                    .as_str()
                    .ok_or("invalid excluded sender")?
                    .parse::<Address>()
                    .map_err(|_| "invalid excluded sender")?;
                if excluded_senders.contains(&sender) {
                    return Err("duplicate excluded sender".into());
                }
                excluded_senders.push(sender);
            }
            Ok(Self {
                id: keccak256(value.to_string()),
                l1_chain_id: positive("l1_chain_id")?,
                l2_chain_id: positive("l2_chain_id")?,
                rollup_id: positive("rollup_id")?,
                beneficiary,
                valid_until_l1_timestamp: positive("valid_until_l1_timestamp")?,
                numerator,
                denominator,
                proving: amount("proving_cost_l1")?,
                operations: amount("operations_cost_l1")?,
                treasury: amount("treasury_cost_l1")?,
                risk_bps,
                profit_bps,
                excluded_senders,
                assumptions: value,
            })
        };
        parse()
            .map(Some)
            .map_err(|error| format!("{SETTING}: {error}"))
    }

    /// Check the actual runtime wiring, not only environment-provided labels.
    pub fn validate_network(
        &self,
        l1: u64,
        l2: u64,
        owned_rollups: impl IntoIterator<Item = u64>,
    ) -> Result<(), String> {
        if self.l1_chain_id != l1
            || self.l2_chain_id != l2
            || !owned_rollups.into_iter().any(|id| id == self.rollup_id)
        {
            return Err(format!(
                "{SETTING}: chain/rollup identity differs from runtime"
            ));
        }
        Ok(())
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
        if l1.target_timestamp > self.valid_until_l1_timestamp {
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
                || self.excluded_senders.contains(&source.sender)
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
        let recovery = mul_div(eligible_tips, self.numerator, self.denominator, false)?;
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

    fn parse(value: Value) -> Result<Policy, String> {
        Policy::parse(Some(value.to_string()), Address::repeat_byte(4)).map(|p| p.unwrap())
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
        for ids in [(2, 9, 1), (1, 8, 1), (1, 9, 2)] {
            assert!(policy.validate_network(ids.0, ids.1, [ids.2]).is_err());
        }
        assert!(policy.validate_network(1, 9, [1]).is_ok());
        assert!(policy.validate_network(1, 9, []).is_err());
        assert!(
            policy.validate_network(1, 9, [0]).is_err(),
            "L1 composition entry is not the owned L2 rollup"
        );
        assert!(policy.validate_network(1, 9, [2, 1]).is_ok());
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

    #[test]
    fn configuration_requires_explicit_bounded_assumptions() {
        assert!(Policy::parse(None, Address::ZERO).unwrap().is_none());
        for text in ["", "null", "[]", "{}", "{", &" ".repeat(8193)] {
            assert!(Policy::parse(Some(text.into()), Address::repeat_byte(4)).is_err());
        }
        let valid = policy_json();
        for key in valid.as_object().unwrap().keys() {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(parse(missing).is_err(), "missing {key}");
        }
        for (key, bad) in [
            ("version", json!(2)),
            ("l1_chain_id", json!(0)),
            ("l2_chain_id", json!("9")),
            ("rollup_id", json!(0)),
            ("valid_until_l1_timestamp", json!(0)),
            ("risk_bps", json!(10001)),
            ("profit_bps", json!(-1)),
            ("l1_per_l2_numerator", json!("0")),
            ("l1_per_l2_denominator", json!("0")),
            ("proving_cost_l1", json!(7)),
            ("operations_cost_l1", json!("-1")),
            ("treasury_cost_l1", json!("0x01")),
            ("proving_cost_l1", json!("1.2")),
            ("beneficiary", json!(Address::ZERO)),
            ("beneficiary", json!(Address::repeat_byte(3))),
            ("excluded_senders", json!(null)),
            ("excluded_senders", json!(["bad"])),
            ("excluded_senders", json!([Address::ZERO, Address::ZERO])),
            ("excluded_senders", json!(vec![Address::ZERO; 65])),
            ("typo", json!(1)),
            ("treasury_cost_l1", json!(format!("{}0", U256::MAX))),
        ] {
            let mut invalid = valid.clone();
            invalid[key] = bad;
            assert!(parse(invalid).is_err(), "{key}");
        }
        let parsed = parse(valid.clone()).unwrap();
        assert_eq!(parsed.id, keccak256(valid.to_string()));
        assert!(Policy::parse(Some(valid.to_string()), Address::ZERO).is_err());
        let mut zeros = valid;
        for key in ["proving_cost_l1", "operations_cost_l1", "treasury_cost_l1"] {
            zeros[key] = json!("0");
        }
        zeros["risk_bps"] = json!(0);
        zeros["profit_bps"] = json!(0);
        assert!(parse(zeros).is_ok());
    }
}
