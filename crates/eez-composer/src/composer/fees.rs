//! Native receipt accounting and configured L2 tariff checks. Shadow reports
//! alone never authorize spending or classify a user's funding. L1 cost and L2
//! tips have different denominations until an operator policy qualifies conversion.

use std::collections::HashMap;

use alloy_consensus::{Transaction, Typed2718, transaction::TxHashRef};
use alloy_primitives::{Address, B256, U256};
use eez_l1::{BundleTarget, posting_fees::FeeQuote};
use eez_protocol::settlement_manifest::SettlementManifest;
use serde_json::{Value, json};

use super::posting::BundleExecution;
use crate::local::BuiltSyncBlock;

pub(super) mod config;
pub(super) mod pricing;
pub(crate) mod quote;
pub(super) mod refund;
pub(super) mod settlement;

#[derive(Debug)]
pub(super) struct SourceFees {
    hash: B256,
    sender: Address,
    gas_used: u64,
    gas_limit: u64,
    priority_fee_per_gas: u128,
    burn: U256,
    tip: U256,
}

/// Native cost measurement of the accepted outbound prefix before adding L1
/// sources. This is pricing evidence only; the complete signed bundle must
/// still pass final native execution. Fee-dependent execution makes even a
/// price-only quote change invalidate reuse.
#[derive(Debug)]
pub(super) struct StandaloneCost {
    quote: FeeQuote,
    gas_used: u64,
    blobs: u64,
    sources: Vec<B256>,
}

impl StandaloneCost {
    pub fn new(
        quote: FeeQuote,
        gas_used: u64,
        blobs: u64,
        semantics: &[eez_protocol::blob_semantics::SemanticTransaction],
    ) -> Self {
        Self {
            quote,
            gas_used,
            blobs,
            sources: semantics
                .iter()
                .map(|source| alloy_primitives::keccak256(&source.tx_data))
                .collect(),
        }
    }

    fn validate(&self, quote: FeeQuote, l2: &L2Fees) -> Result<U256, &'static str> {
        if self.quote != quote {
            return Err("l2_standalone_quote_changed");
        }
        if self.gas_used == 0
            || self.blobs == 0
            || self.sources.is_empty()
            || !self
                .sources
                .iter()
                .copied()
                .eq(l2.sources.iter().map(|s| s.hash))
        {
            return Err("l2_standalone_source_mismatch");
        }
        quote
            .estimate(self.gas_used, self.blobs)
            .map(|cost| cost.total)
            .map_err(|_| "l2_standalone_cost_overflow")
    }
}

impl SourceFees {
    pub fn detail(&self, chain_id: u64, block_hash: B256) -> Value {
        json!({
            "source_hash": self.hash,
            "execution_chain_id": chain_id,
            "preflight_basis": "native_l2_terminal_receipt",
            "preflight_used_gas": self.gas_used,
            // Receipts measure charged gas, not the pre-refund execution budget.
            "preflight_required_gas": null,
            "signed_limit_gas": self.gas_limit,
            "fee_observation": {
                "version": 1, "mode": "shadow", "block_hash": block_hash,
                "sender": self.sender, "unit": "l2_native_base_units",
                "effective_priority_fee_per_gas": self.priority_fee_per_gas.to_string(),
                "base_fee_burn": self.burn.to_string(),
                "gross_priority_fee": self.tip.to_string(),
                "qualified_recovery_l1": null,
            },
        })
    }
}

#[derive(Debug)]
pub(super) struct L2Fees {
    pub chain_id: u64,
    rollup_id: u64,
    pub block_hash: B256,
    parent_hash: B256,
    block_number: u64,
    beneficiary: Address,
    base_fee_per_gas: u64,
    ordinary_gas_used: u64,
    native_gas_used: u64,
    burn: U256,
    tip: U256,
    pub sources: Vec<SourceFees>,
    pub standalone_cost: Option<StandaloneCost>,
}

impl L2Fees {
    /// Bind the final native receipts to the manifest's complete outbound
    /// source set. Loads and callback deliveries consume gas but pay no tip.
    /// Historical ordinary blocks are outside this terminal-block observation.
    /// Inconsistent evidence is unavailable; it must not become zero revenue.
    pub fn measure(
        chain_id: u64,
        rollup_id: u64,
        built: &BuiltSyncBlock,
        manifest: &SettlementManifest,
    ) -> Result<Self, &'static str> {
        if built.header.hash() != manifest.sync_hash || built.header.number != manifest.sync_number
        {
            return Err("l2_manifest_endpoint_mismatch");
        }
        if built.block.body().transactions.len() != built.tx_cumulative_gas_used.len() {
            return Err("l2_receipt_count_mismatch");
        }
        let mut users = HashMap::new();
        for user in &manifest.users {
            if user.source_rollup_id.0 == 0 {
                continue;
            }
            if user.source_rollup_id.0 != rollup_id
                || user.outbound_group.is_none()
                || users.insert(user.tx_hash, user).is_some()
            {
                return Err("l2_source_manifest_mismatch");
            }
        }
        let base_fee_per_gas = built.header.base_fee_per_gas.ok_or("l2_base_fee_missing")?;
        let mut fees = Self {
            chain_id,
            rollup_id,
            block_hash: built.header.hash(),
            parent_hash: built.header.parent_hash,
            block_number: built.header.number,
            beneficiary: built.header.beneficiary,
            base_fee_per_gas,
            ordinary_gas_used: 0,
            native_gas_used: 0,
            burn: U256::ZERO,
            tip: U256::ZERO,
            sources: Vec::with_capacity(users.len()),
            standalone_cost: None,
        };
        let mut previous = 0_u64;
        for (tx, &cumulative) in built
            .block
            .transactions_recovered()
            .zip(&built.tx_cumulative_gas_used)
        {
            let gas_used = cumulative
                .checked_sub(previous)
                .ok_or("l2_receipt_gas_not_monotonic")?;
            previous = cumulative;
            if eez_primitives::is_native_type(tx.ty()) {
                fees.native_gas_used = fees
                    .native_gas_used
                    .checked_add(gas_used)
                    .ok_or("l2_gas_overflow")?;
                continue;
            }
            let user = users
                .remove(tx.tx_hash())
                .ok_or("l2_source_not_in_manifest")?;
            if user.sender != tx.signer() || user.nonce != tx.nonce() {
                return Err("l2_source_identity_mismatch");
            }
            if gas_used > tx.gas_limit() {
                return Err("l2_receipt_gas_exceeds_limit");
            }
            let priority_fee_per_gas = tx
                .effective_tip_per_gas(base_fee_per_gas)
                .ok_or("l2_source_fee_below_base")?;
            // u64 gas times a u128 price fits U256; aggregate sums stay checked.
            let burn = U256::from(gas_used) * U256::from(base_fee_per_gas);
            let tip = U256::from(gas_used) * U256::from(priority_fee_per_gas);
            fees.ordinary_gas_used = fees
                .ordinary_gas_used
                .checked_add(gas_used)
                .ok_or("l2_gas_overflow")?;
            fees.burn = fees.burn.checked_add(burn).ok_or("l2_fee_overflow")?;
            fees.tip = fees.tip.checked_add(tip).ok_or("l2_fee_overflow")?;
            fees.sources.push(SourceFees {
                hash: *tx.tx_hash(),
                sender: tx.signer(),
                gas_used,
                gas_limit: tx.gas_limit(),
                priority_fee_per_gas,
                burn,
                tip,
            });
        }
        if !users.is_empty() {
            return Err("l2_source_receipt_missing");
        }
        if previous != built.header.gas_used {
            return Err("l2_receipts_block_gas_mismatch");
        }
        Ok(fees)
    }

    fn summary(&self) -> Value {
        json!({
            "status": "measured", "scope": "terminal_sync_block",
            "chain_id": self.chain_id, "rollup_id": self.rollup_id, "unit": "l2_native_base_units",
            "block_hash": self.block_hash, "parent_hash": self.parent_hash,
            "block_number": self.block_number, "beneficiary": self.beneficiary,
            "base_fee_per_gas": self.base_fee_per_gas.to_string(),
            "source_count": self.sources.len(),
            "ordinary_gas_used": self.ordinary_gas_used,
            "native_gas_used": self.native_gas_used,
            "base_fee_burn": self.burn.to_string(),
            "gross_priority_fees": self.tip.to_string(),
        })
    }
}

#[derive(Debug)]
struct L1Fees {
    chain_id: u64,
    companion_count: usize,
    post_cost: U256,
    detail: Value,
}

fn l1_summary(
    quote: FeeQuote,
    execution: &BundleExecution,
    post_hash: B256,
    native_currency: &str,
) -> Result<L1Fees, &'static str> {
    let BundleTarget::Exact { block, timestamp } = quote.target else {
        return Err("l1_target_unresolved");
    };
    if quote.sample.number.checked_add(1) != Some(block) {
        return Err("l1_target_parent_mismatch");
    }
    if execution.transaction_hashes.first() != Some(&post_hash)
        || execution.gas_used.len() != execution.transaction_hashes.len()
        || execution.gas_limits.len() != execution.transaction_hashes.len()
        || execution.blob_counts.len() != execution.transaction_hashes.len()
    {
        return Err("l1_preflight_identity_mismatch");
    }
    let used = execution.gas_used[0];
    let limit = execution.gas_limits[0];
    let blobs = execution.blob_counts[0];
    if used > limit || blobs == 0 {
        return Err("l1_preflight_gas_or_blobs_invalid");
    }
    let estimate = quote
        .estimate(used, blobs)
        .map_err(|_| "l1_cost_overflow")?;
    let reserve = quote
        .reserve(limit, blobs)
        .map_err(|_| "l1_cost_overflow")?;
    let detail = json!({
        "status": "measured", "scope": "complete_post_batch",
        "chain_id": quote.chain_id, "native_currency": native_currency,
        "unit": "l1_native_base_units", "transaction_hash": post_hash,
        "sample_block": quote.sample.number, "sample_hash": quote.sample.hash,
        "target_block": block, "target_timestamp": timestamp,
        "gas_used": used, "signed_gas_limit": limit, "physical_blobs": blobs,
        "effective_gas_price": quote.estimated_effective_gas_price.to_string(),
        "blob_gas_price": quote.estimated_blob_gas_price.to_string(),
        "estimated_execution_cost": estimate.execution.to_string(),
        "estimated_blob_cost": estimate.blobs.to_string(),
        "estimated_post_cost": estimate.total.to_string(),
        "fee_cap_balance_reservation": reserve.total.to_string(),
        "companion_count": execution.transaction_hashes.len() - 1,
    });
    Ok(L1Fees {
        chain_id: quote.chain_id,
        companion_count: execution.transaction_hashes.len() - 1,
        post_cost: estimate.total,
        detail,
    })
}

/// Check a complete candidate before its signed L1 transaction can be submitted.
/// Missing native receipts or invalid policy evidence cannot authorize dispatch.
pub(super) fn check_tariff(
    policy: &pricing::Policy,
    quote: FeeQuote,
    execution: &BundleExecution,
    post_hash: B256,
    l2: Option<&Result<L2Fees, &'static str>>,
    poster: Address,
) -> Result<Value, &'static str> {
    let mut l1 = l1_summary(quote, execution, post_hash, "")?;
    let l2 = l2
        .ok_or("terminal_receipts_not_captured")?
        .as_ref()
        .map_err(|reason| *reason)?;
    let complete_post_cost = l1.post_cost;
    let (scope, gas_used, blobs) = if l1.companion_count == 0 {
        (
            "complete_post_batch",
            execution.gas_used[0],
            execution.blob_counts[0],
        )
    } else {
        let standalone = l2
            .standalone_cost
            .as_ref()
            .ok_or("l2_standalone_cost_missing")?;
        l1.post_cost = standalone.validate(quote, l2)?;
        ("standalone_outbound", standalone.gas_used, standalone.blobs)
    };
    policy.check_tariff(&l1, l2, poster)?;
    Ok(json!({
        "scope": scope,
        "gas_used": gas_used,
        "physical_blobs": blobs,
        "l1_post_cost": l1.post_cost.to_string(),
        "complete_post_cost": complete_post_cost.to_string(),
        "additional_l1_post_cost": complete_post_cost.saturating_sub(l1.post_cost).to_string(),
        "minimum_l2_payment": policy.required_payment(l1.post_cost)?.minimum_l2.to_string(),
        "sample_hash": quote.sample.hash,
        "target_block": l1.detail["target_block"],
        "target_timestamp": l1.detail["target_timestamp"],
    }))
}

/// Additive detail on the existing final-preflight event. A failed observation
/// is diagnostic only; it cannot reject, evict, fund or release accepted work.
pub(super) fn observation(
    quote: FeeQuote,
    execution: &BundleExecution,
    post_hash: B256,
    native_currency: &str,
    l2: Option<&Result<L2Fees, &'static str>>,
    policy: Option<&pricing::Policy>,
    poster: Address,
) -> Value {
    let unavailable = |reason| json!({"status": "unavailable", "reason": reason});
    let l1 = l1_summary(quote, execution, post_hash, native_currency);
    let pricing = policy.map_or_else(
        || json!({"status":"not_configured"}),
        |policy| policy.observe(&l1, l2, poster),
    );
    let calculated = pricing["status"] == "calculated";
    let l1 = l1.map(|fees| fees.detail).unwrap_or_else(unavailable);
    let l2 = match l2 {
        Some(Ok(fees)) => fees.summary(),
        Some(Err(reason)) => unavailable(reason),
        None => unavailable("terminal_receipts_not_captured"),
    };
    json!({
        "version": 2, "mode": "shadow", "l1": l1, "l2": l2,
        "pricing_status": if calculated { "shadow_priced" } else { "unpriced" },
        "qualified_recovery_l1": null,
        "required_tip_per_gas": pricing["required_tip_per_gas"],
        "pricing": pricing,
        "unpriced_components": if calculated { vec![] } else { vec!["asset_realization", "proving", "operations", "risk", "profit"] },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Header, SignableTransaction, TxEip1559, TxLegacy};
    use alloy_eips::Encodable2718;
    use alloy_primitives::Signature;
    use eez_primitives::{EezTxEnvelope, engine::EezEngineTypes};
    use eez_protocol::{RollupId, settlement_manifest::UserManifest};
    use reth_payload_primitives::PayloadTypes;
    use reth_primitives_traits::RecoveredBlock;

    fn fixture() -> (BuiltSyncBlock, SettlementManifest) {
        let ordinary = |tx| EezTxEnvelope::Ethereum(tx);
        let native = |source_id: u64| {
            eez_primitives::SystemTransaction {
                chain_id: 9,
                source_hash: alloy_primitives::B256::from(
                    alloy_primitives::U256::from(source_id).to_be_bytes::<32>(),
                ),
                gas_limit: eez_primitives::SYSTEM_TX_GAS_CAP,
                to: eez_primitives::EEZL2_ADDRESS,
                value: U256::ZERO,
                input: Default::default(),
            }
            .into()
        };
        let body = eez_primitives::BlockBody {
            transactions: vec![
                native(0),
                ordinary(
                    TxEip1559 {
                        chain_id: 9,
                        gas_limit: 200_000,
                        max_fee_per_gas: 12,
                        max_priority_fee_per_gas: 5,
                        ..Default::default()
                    }
                    .into_signed(Signature::test_signature())
                    .into(),
                ),
                native(1), // A callback after its source must never earn another tip.
                ordinary(
                    TxLegacy {
                        chain_id: Some(9),
                        nonce: 1,
                        gas_limit: 100_000,
                        gas_price: 17,
                        ..Default::default()
                    }
                    .into_signed(Signature::test_signature())
                    .into(),
                ),
            ],
            ..Default::default()
        };
        let block = RecoveredBlock::new_unhashed(
            eez_primitives::Block::new(
                Header {
                    number: 5,
                    parent_hash: B256::repeat_byte(3),
                    beneficiary: Address::repeat_byte(4),
                    base_fee_per_gas: Some(10),
                    gas_used: 121_000,
                    ..Default::default()
                },
                body,
            ),
            vec![
                eez_primitives::SYSTEM_ADDRESS,
                Address::ZERO,
                eez_primitives::SYSTEM_ADDRESS,
                Address::ZERO,
            ],
        );
        let sealed = block.sealed_block().clone();
        let header = sealed.sealed_header().clone();
        let manifest = SettlementManifest {
            sync_number: header.number,
            sync_hash: header.hash(),
            effects: vec![],
            users: block
                .transactions_recovered()
                .filter(|tx| !eez_primitives::is_native_type(tx.ty()))
                .enumerate()
                .map(|(i, tx)| UserManifest {
                    tx_hash: *tx.tx_hash(),
                    source_rollup_id: RollupId(1),
                    sender: tx.signer(),
                    nonce: tx.nonce(),
                    attempts: 0,
                    outbound_group: Some(i),
                    effects: i..i + 1,
                    source_actions: vec![],
                    l2_transaction_range: 0..0,
                    transaction_references: None,
                })
                .collect(),
        };
        (
            BuiltSyncBlock {
                payload: EezEngineTypes::block_to_payload(sealed, None),
                l1_origin_hash: B256::ZERO,
                header,
                block,
                tx_successes: vec![true, true, true, false],
                tx_cumulative_gas_used: vec![21_000, 71_000, 101_000, 121_000],
            },
            manifest,
        )
    }

    #[test]
    fn l2_fees_use_receipt_gas_clipped_tips_and_keep_native_callbacks_free() {
        let (built, manifest) = fixture();
        let fees = L2Fees::measure(9, 1, &built, &manifest).unwrap();
        assert_eq!(fees.ordinary_gas_used, 70_000);
        assert_eq!(fees.native_gas_used, 51_000);
        assert_eq!(fees.burn, U256::from(700_000));
        assert_eq!(fees.tip, U256::from(50_000 * 2 + 20_000 * 7));
        assert_eq!(fees.sources.len(), 2);
        assert_eq!(fees.sources[0].priority_fee_per_gas, 2);
        assert_eq!(fees.sources[1].gas_used, 20_000, "reverts still pay fees");
        assert_eq!(fees.sources[1].hash, manifest.users[1].tx_hash);
        assert_eq!(
            fees.summary()["beneficiary"],
            json!(Address::repeat_byte(4))
        );
        assert_eq!(
            fees.sources[0].detail(9, fees.block_hash)["preflight_required_gas"],
            Value::Null
        );
    }

    #[test]
    fn incomplete_or_mismatched_l2_evidence_is_unavailable() {
        let check = |built: &BuiltSyncBlock, manifest: &SettlementManifest, error| {
            assert_eq!(L2Fees::measure(9, 1, built, manifest).unwrap_err(), error);
        };
        let (mut built, manifest) = fixture();
        built.tx_cumulative_gas_used.pop();
        check(&built, &manifest, "l2_receipt_count_mismatch");
        let (mut built, manifest) = fixture();
        built.tx_cumulative_gas_used[2] = 0;
        check(&built, &manifest, "l2_receipt_gas_not_monotonic");
        let (mut built, manifest) = fixture();
        built.tx_cumulative_gas_used[3] += 1;
        check(&built, &manifest, "l2_receipts_block_gas_mismatch");
        let (built, mut manifest) = fixture();
        manifest.sync_hash = B256::ZERO;
        check(&built, &manifest, "l2_manifest_endpoint_mismatch");
        let (built, mut manifest) = fixture();
        manifest.users.pop();
        check(&built, &manifest, "l2_source_not_in_manifest");
        let (built, mut manifest) = fixture();
        manifest.users.push(manifest.users[0].clone());
        check(&built, &manifest, "l2_source_manifest_mismatch");
        let (built, mut manifest) = fixture();
        manifest.users[0].sender = Address::repeat_byte(1);
        check(&built, &manifest, "l2_source_identity_mismatch");
        let (built, mut manifest) = fixture();
        let mut absent = manifest.users[0].clone();
        absent.tx_hash = B256::repeat_byte(8);
        manifest.users.push(absent);
        check(&built, &manifest, "l2_source_receipt_missing");
    }

    pub(super) fn preflight() -> (FeeQuote, BundleExecution, B256) {
        let post = B256::repeat_byte(1);
        (
            FeeQuote {
                chain_id: 1,
                sample: eez_l1::L1BlockRef {
                    number: 42,
                    hash: B256::repeat_byte(2),
                },
                target: BundleTarget::Exact {
                    block: 43,
                    timestamp: 1_000,
                },
                max_fee_per_gas: 31,
                max_priority_fee_per_gas: 3,
                max_fee_per_blob_gas: 11,
                estimated_effective_gas_price: 13,
                estimated_blob_gas_price: 7,
            },
            BundleExecution {
                gas_used: vec![100, 900_000],
                gas_required: vec![150, 950_000],
                gas_limits: vec![200, 1_000_000],
                transaction_hashes: vec![post, B256::repeat_byte(9)],
                blob_counts: vec![2, 3],
            },
            post,
        )
    }

    #[test]
    fn poster_cost_includes_its_physical_blobs_without_companion_cost_or_limit_gas() {
        let (quote, execution, hash) = preflight();
        let report = l1_summary(quote, &execution, hash, "ETH").unwrap().detail;
        assert_eq!(report["estimated_execution_cost"], "1300");
        let blobs = U256::from(2 * alloy_eips::eip4844::DATA_GAS_PER_BLOB * 7);
        assert_eq!(report["estimated_blob_cost"], blobs.to_string());
        assert_eq!(
            report["estimated_post_cost"],
            (blobs + U256::from(1300)).to_string()
        );
        assert_eq!(
            report["fee_cap_balance_reservation"],
            quote.reserve(200, 2).unwrap().total.to_string()
        );
        assert_eq!(report["physical_blobs"], 2);
        assert_eq!(report["companion_count"], 1);
        assert_eq!(report["sample_hash"], json!(quote.sample.hash));
    }

    fn mixed_tariff_fixture() -> (FeeQuote, BundleExecution, B256, L2Fees) {
        let (mut quote, mut execution, hash) = preflight();
        quote.estimated_blob_gas_price = 1;
        execution.gas_used[0] = 10_000;
        execution.gas_limits[0] = 20_000;
        let (built, manifest) = fixture();
        let semantics = built
            .block
            .body()
            .transactions
            .iter()
            .filter(|tx| !eez_primitives::is_native_type(tx.ty()))
            .map(|tx| eez_protocol::blob_semantics::SemanticTransaction {
                origin_chain: RollupId(1),
                tx_data: tx.encoded_2718().into(),
                actions: vec![],
            })
            .collect::<Vec<_>>();
        let mut l2 = L2Fees::measure(9, 1, &built, &manifest).unwrap();
        l2.standalone_cost = Some(StandaloneCost::new(quote, 100, 1, &semantics));
        (quote, execution, hash, l2)
    }

    #[test]
    fn mixed_tariff_requires_outbound_coverage_without_charging_inbound_cost() {
        let policy = pricing::tests::parse(pricing::tests::policy_json()).unwrap();
        let (quote, execution, hash, mut l2) = mixed_tariff_fixture();
        let complete = l1_summary(quote, &execution, hash, "").unwrap();
        assert_eq!(
            policy.check_tariff(&complete, &l2, Address::repeat_byte(7)),
            Err("l2_tip_underfunded")
        );
        let report = check_tariff(
            &policy,
            quote,
            &execution,
            hash,
            Some(&Ok(l2)),
            Address::repeat_byte(7),
        )
        .unwrap();
        assert_eq!(report["scope"], "standalone_outbound");
        assert_eq!(
            report["l1_post_cost"],
            quote.estimate(100, 1).unwrap().total.to_string()
        );
        assert_eq!(
            report["additional_l1_post_cost"],
            (complete.post_cost - quote.estimate(100, 1).unwrap().total).to_string()
        );

        (_, _, _, l2) = mixed_tariff_fixture();
        for source in &mut l2.sources {
            source.tip = U256::ZERO;
        }
        l2.tip = U256::ZERO;
        assert_eq!(
            check_tariff(
                &policy,
                quote,
                &execution,
                hash,
                Some(&Ok(l2)),
                Address::repeat_byte(7)
            ),
            Err("l2_tip_underfunded")
        );
    }

    #[test]
    fn mixed_tariff_rejects_missing_stale_and_mismatched_reference() {
        let policy = pricing::tests::parse(pricing::tests::policy_json()).unwrap();
        for case in 0..12 {
            let (quote, execution, hash, mut l2) = mixed_tariff_fixture();
            let reference = l2.standalone_cost.as_mut().unwrap();
            let expected = match case {
                0 => {
                    l2.standalone_cost = None;
                    "l2_standalone_cost_missing"
                }
                1 => {
                    reference.quote.sample.hash = B256::ZERO;
                    "l2_standalone_quote_changed"
                }
                2 => {
                    reference.quote.target = BundleTarget::Exact {
                        block: 12,
                        timestamp: 1_001,
                    };
                    "l2_standalone_quote_changed"
                }
                3 => {
                    reference.quote.max_priority_fee_per_gas += 1;
                    "l2_standalone_quote_changed"
                }
                4 => {
                    reference.quote.max_fee_per_gas += 1;
                    "l2_standalone_quote_changed"
                }
                5 => {
                    reference.quote.estimated_effective_gas_price += 1;
                    "l2_standalone_quote_changed"
                }
                6 => {
                    reference.quote.estimated_blob_gas_price += 1;
                    "l2_standalone_quote_changed"
                }
                7 => {
                    reference.sources.swap(0, 1);
                    "l2_standalone_source_mismatch"
                }
                8 => {
                    reference.sources.pop();
                    "l2_standalone_source_mismatch"
                }
                9 => {
                    reference.sources[0] = B256::ZERO;
                    "l2_standalone_source_mismatch"
                }
                10 => {
                    reference.blobs = 0;
                    "l2_standalone_source_mismatch"
                }
                _ => {
                    reference.gas_used = 0;
                    "l2_standalone_source_mismatch"
                }
            };
            assert_eq!(
                check_tariff(
                    &policy,
                    quote,
                    &execution,
                    hash,
                    Some(&Ok(l2)),
                    Address::repeat_byte(7)
                ),
                Err(expected),
                "case {case}"
            );
        }
    }

    #[test]
    fn outbound_only_tariff_retains_complete_final_receipt_cost() {
        let policy = pricing::tests::parse(pricing::tests::policy_json()).unwrap();
        let (quote, mut execution, hash, l2) = mixed_tariff_fixture();
        execution.gas_used.truncate(1);
        execution.gas_required.truncate(1);
        execution.gas_limits.truncate(1);
        execution.transaction_hashes.truncate(1);
        execution.blob_counts.truncate(1);
        assert_eq!(
            check_tariff(
                &policy,
                quote,
                &execution,
                hash,
                Some(&Ok(l2)),
                Address::repeat_byte(7)
            ),
            Err("l2_tip_underfunded"),
            "a smaller reference cannot discount an outbound-only post"
        );
    }

    #[test]
    fn l1_observation_rejects_stale_identity_missing_gas_and_blob_overflow() {
        let (mut quote, mut execution, hash) = preflight();
        assert_eq!(
            l1_summary(quote, &execution, B256::ZERO, "ETH").unwrap_err(),
            "l1_preflight_identity_mismatch"
        );
        quote.sample.number -= 1;
        assert_eq!(
            l1_summary(quote, &execution, hash, "ETH").unwrap_err(),
            "l1_target_parent_mismatch"
        );
        quote.sample.number += 1;
        execution.blob_counts[0] = u64::MAX;
        assert_eq!(
            l1_summary(quote, &execution, hash, "ETH").unwrap_err(),
            "l1_cost_overflow"
        );
        execution.gas_used.clear();
        assert_eq!(
            l1_summary(quote, &execution, hash, "ETH").unwrap_err(),
            "l1_preflight_identity_mismatch"
        );
    }

    #[test]
    fn shadow_reports_never_invent_conversion_income_or_missing_receipts() {
        let (quote, execution, hash) = preflight();
        let (built, manifest) = fixture();
        let l2 = L2Fees::measure(9, 1, &built, &manifest);
        let report = observation(
            quote,
            &execution,
            hash,
            "xDAI",
            Some(&l2),
            None,
            Address::ZERO,
        );
        assert_eq!(report["l1"]["native_currency"], "xDAI");
        assert_eq!(report["l2"]["unit"], "l2_native_base_units");
        assert_eq!(report["l2"]["gross_priority_fees"], "240000");
        assert_eq!(report["pricing_status"], "unpriced");
        assert!(report["qualified_recovery_l1"].is_null());
        assert!(report["required_tip_per_gas"].is_null());
        for input in [None, Some(Err("l2_source_receipt_missing"))] {
            let unavailable = observation(
                quote,
                &execution,
                hash,
                "ETH",
                input.as_ref(),
                None,
                Address::ZERO,
            );
            assert_eq!(unavailable["l2"]["status"], "unavailable");
            assert!(unavailable["l2"].get("gross_priority_fees").is_none());
            assert_eq!(unavailable["l1"]["status"], "measured");
        }
    }

    #[test]
    fn configured_shadow_comparison_uses_final_clipped_tips_and_keeps_income_unqualified() {
        let policy = pricing::tests::parse(pricing::tests::policy_json()).unwrap();
        let (quote, execution, hash) = preflight();
        let (built, manifest) = fixture();
        let l2 = L2Fees::measure(9, 1, &built, &manifest);
        let report = observation(
            quote,
            &execution,
            hash,
            "ETH",
            Some(&l2),
            Some(&policy),
            Address::repeat_byte(77),
        );
        assert_eq!(report["version"], 2);
        assert_eq!(report["pricing_status"], "shadow_priced");
        assert_eq!(report["pricing"]["eligible_priority_fees_l2"], "240000");
        assert_eq!(report["pricing"]["projected_recovery_l1"], "160000");
        assert_eq!(report["pricing"]["required_service_payment_l1"], "2111773");
        assert_eq!(report["pricing"]["shortfall_l1"], "1951773");
        assert!(report["qualified_recovery_l1"].is_null());
        assert!(
            report["required_tip_per_gas"].is_null(),
            "shared/mixed candidate cannot quote a standalone source"
        );
        let unavailable = observation(
            quote,
            &execution,
            B256::ZERO,
            "ETH",
            Some(&l2),
            Some(&policy),
            Address::ZERO,
        );
        assert_eq!(unavailable["pricing_status"], "unpriced");
        assert_eq!(
            unavailable["pricing"]["reason"],
            "l1_preflight_identity_mismatch"
        );
        assert!(unavailable["required_tip_per_gas"].is_null());
    }
}
