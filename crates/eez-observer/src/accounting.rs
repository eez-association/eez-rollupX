//! Receipt facts, independent of lifecycle attribution and canonical selection.
//! The database selects canonical inclusions; parsing a receipt proves neither
//! finality nor successful cross-chain execution. Reverted receipts still cost.
use alloy_primitives::{B256, U256, U512};
use serde_json::Value;

use crate::{
    config::valid_hash,
    model::{quantity, quantity_hex},
    rpc::Failure,
};

/// EIP-4844 data gas per purchased blob, not the EEZ codec's usable byte capacity.
pub const GAS_PER_BLOB: u64 = 131_072;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    GasUsed,
    EffectiveGasPrice,
    BlobGasUsed,
    BlobGasPrice,
    InvalidBlobGas,
    UnexpectedBlobFields,
    TransactionType,
}

impl Gap {
    pub const fn label(self) -> &'static str {
        match self {
            Self::GasUsed => "gas_used_missing_or_invalid",
            Self::EffectiveGasPrice => "effective_gas_price_missing_or_invalid",
            Self::BlobGasUsed => "blob_gas_used_missing_or_invalid",
            Self::BlobGasPrice => "blob_gas_price_missing_or_invalid",
            Self::InvalidBlobGas => "invalid_physical_blob_gas",
            Self::UnexpectedBlobFields => "non_blob_transaction_has_blob_charge_fields",
            Self::TransactionType => "transaction_type_missing_invalid_or_unsupported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptCost {
    pub gas_used: U256,
    pub effective_gas_price: U256,
    pub blob_gas_used: U256,
    pub blob_gas_price: U256,
    pub execution_paid: U512,
    pub blob_paid: U512,
    pub purchased_blobs: U256,
}

fn amount(value: &Value, gap: Gap) -> Result<U256, Gap> {
    let raw = quantity_hex(value).map_err(|_| gap)?;
    U256::from_str_radix(raw, 16).map_err(|_| gap)
}

impl ReceiptCost {
    pub fn parse(receipt: &Value) -> Result<Self, Gap> {
        let transaction_type = quantity(&receipt["type"]).map_err(|_| Gap::TransactionType)?;
        if transaction_type > 4 {
            return Err(Gap::TransactionType);
        }
        let gas_used = amount(&receipt["gasUsed"], Gap::GasUsed)?;
        let effective_gas_price = amount(&receipt["effectiveGasPrice"], Gap::EffectiveGasPrice)?;
        let (blob_gas_used, blob_gas_price) = if transaction_type == 3 {
            let used = amount(&receipt["blobGasUsed"], Gap::BlobGasUsed)?;
            let price = amount(&receipt["blobGasPrice"], Gap::BlobGasPrice)?;
            if used == U256::ZERO || used % U256::from(GAS_PER_BLOB) != U256::ZERO {
                return Err(Gap::InvalidBlobGas);
            }
            if price == U256::ZERO {
                return Err(Gap::BlobGasPrice);
            }
            (used, price)
        } else {
            for field in ["blobGasUsed", "blobGasPrice"] {
                if !receipt[field].is_null()
                    && amount(&receipt[field], Gap::UnexpectedBlobFields)? != U256::ZERO
                {
                    return Err(Gap::UnexpectedBlobFields);
                }
            }
            // These charges do not apply to known non-blob transaction types.
            (U256::ZERO, U256::ZERO)
        };
        Ok(Self {
            gas_used,
            effective_gas_price,
            blob_gas_used,
            blob_gas_price,
            execution_paid: gas_used.widening_mul(effective_gas_price),
            blob_paid: blob_gas_used.widening_mul(blob_gas_price),
            purchased_blobs: blob_gas_used / U256::from(GAS_PER_BLOB),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub transaction_hash: B256,
    pub block_hash: B256,
    pub block_number: u64,
    pub transaction_index: u64,
    pub success: bool,
    /// Keep an identified inclusion even when required accounting fields are
    /// missing. Persist the gap, never a zero charge or an invented estimate.
    pub cost: Result<ReceiptCost, Gap>,
}

pub(crate) fn hash(value: &Value) -> Result<B256, Failure> {
    let raw = value
        .as_str()
        .filter(|raw| valid_hash(raw))
        .ok_or(Failure::Malformed)?;
    raw.parse().map_err(|_| Failure::Malformed)
}

impl Receipt {
    pub fn parse(value: &Value) -> Result<Self, Failure> {
        let success = match quantity(&value["status"])? {
            0 => false,
            1 => true,
            _ => return Err(Failure::Malformed),
        };
        Ok(Self {
            transaction_hash: hash(&value["transactionHash"])?,
            block_hash: hash(&value["blockHash"])?,
            block_number: quantity(&value["blockNumber"])?,
            transaction_index: quantity(&value["transactionIndex"])?,
            success,
            cost: ReceiptCost::parse(value),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::{HashMap, HashSet};

    fn receipt() -> Value {
        json!({"transactionHash":format!("0x{}","11".repeat(32)), "blockHash":format!("0x{}","22".repeat(32)),
            "blockNumber":"0x1", "transactionIndex":"0x0", "status":"0x1", "type":"0x3",
            "gasUsed":"0x5208", "effectiveGasPrice":"0x7", "blobGasUsed":"0x40000", "blobGasPrice":"0x3"})
    }

    #[test]
    fn reverts_pay_the_same_exact_execution_and_blob_fees() {
        let mut value = receipt();
        let successful = Receipt::parse(&value).unwrap();
        let cost = successful.cost.clone().unwrap();
        assert_eq!(cost.execution_paid.to_string(), "147000");
        assert_eq!(cost.blob_paid.to_string(), "786432");
        assert_eq!(cost.purchased_blobs, U256::from(2));
        value["status"] = "0x0".into();
        let reverted = Receipt::parse(&value).unwrap();
        assert!(!reverted.success);
        assert_eq!(reverted.cost, successful.cost);
        // Limits/caps/estimates cannot replace receipt prices and used gas.
        value["gas"] = "0xffff".into();
        value["maxFeePerGas"] = "0xffffff".into();
        assert_eq!(Receipt::parse(&value).unwrap().cost, reverted.cost);
    }

    #[test]
    fn missing_invalid_and_inapplicable_fields_remain_distinct() {
        for (field, gap) in [
            ("gasUsed", Gap::GasUsed),
            ("effectiveGasPrice", Gap::EffectiveGasPrice),
            ("blobGasUsed", Gap::BlobGasUsed),
            ("blobGasPrice", Gap::BlobGasPrice),
            ("type", Gap::TransactionType),
        ] {
            for invalid in [
                Value::Null,
                json!(1),
                json!("0x00"),
                json!("-1"),
                json!("0x"),
                json!(format!("0x1{}", "0".repeat(64))),
            ] {
                let mut value = receipt();
                value[field] = invalid;
                assert_eq!(Receipt::parse(&value).unwrap().cost, Err(gap));
            }
        }
        for invalid in ["0x0", "0x1", "0x20001"] {
            let mut value = receipt();
            value["blobGasUsed"] = invalid.into();
            assert_eq!(
                Receipt::parse(&value).unwrap().cost,
                Err(Gap::InvalidBlobGas)
            );
        }
        let mut value = receipt();
        value["type"] = "0x2".into();
        assert_eq!(
            Receipt::parse(&value).unwrap().cost,
            Err(Gap::UnexpectedBlobFields)
        );
        value.as_object_mut().unwrap().remove("blobGasUsed");
        value.as_object_mut().unwrap().remove("blobGasPrice");
        assert_eq!(
            Receipt::parse(&value).unwrap().cost.unwrap().blob_paid,
            U512::ZERO
        );
        value["status"] = "0x2".into();
        assert_eq!(Receipt::parse(&value), Err(Failure::Malformed));
    }

    #[test]
    fn full_width_products_are_exact_without_u256_overflow() {
        let mut value = receipt();
        value["type"] = "0x0".into();
        value["gasUsed"] = format!("0x{}", "f".repeat(64)).into();
        value["effectiveGasPrice"] = value["gasUsed"].clone();
        value.as_object_mut().unwrap().remove("blobGasUsed");
        value.as_object_mut().unwrap().remove("blobGasPrice");
        let cost = Receipt::parse(&value).unwrap().cost.unwrap();
        assert_eq!(
            format!("{:x}", cost.execution_paid),
            format!("{}e{}1", "f".repeat(63), "0".repeat(63))
        );
        assert!(cost.execution_paid > U512::from(U256::MAX));
    }

    #[test]
    fn saved_audit_reconciles_exactly_with_explicit_membership_and_separate_l2_fees() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../observability/tests/fixtures/chiado-accounting-v1.json"
        ))
        .unwrap();
        let mut l1 = HashMap::new();
        let mut l2 = HashMap::new();
        for entry in fixture["receipts"].as_array().unwrap() {
            let receipt = Receipt::parse(&entry["receipt"]).unwrap();
            let population = if entry["chain"] == "l1" {
                &mut l1
            } else {
                &mut l2
            };
            assert!(
                population
                    .insert(receipt.transaction_hash, receipt)
                    .is_none()
            );
        }
        assert_eq!(l1.len(), 564);
        assert_eq!(l2.len(), 350);
        let mut selected = HashSet::new();
        let mut post_execution = U512::ZERO;
        let mut companion_execution = U512::ZERO;
        let mut blobs = U512::ZERO;
        let mut purchased = U256::ZERO;
        for post in fixture["posts"].as_array().unwrap() {
            let id = hash(&post["transaction_hash"]).unwrap();
            assert!(selected.insert(id));
            let cost = l1[&id].cost.as_ref().unwrap();
            post_execution = post_execution.checked_add(cost.execution_paid).unwrap();
            blobs = blobs.checked_add(cost.blob_paid).unwrap();
            purchased = purchased.checked_add(cost.purchased_blobs).unwrap();
            let mut batch = cost.execution_paid.checked_add(cost.blob_paid).unwrap();
            for companion in post["companion_hashes"].as_array().unwrap() {
                assert!(!post["membership_provenance"].is_null());
                assert!(
                    post["source_hashes"]
                        .as_array()
                        .unwrap()
                        .contains(companion)
                );
                let id = hash(companion).unwrap();
                assert!(selected.insert(id));
                let cost = l1[&id].cost.as_ref().unwrap();
                companion_execution = companion_execution
                    .checked_add(cost.execution_paid)
                    .unwrap();
                blobs = blobs.checked_add(cost.blob_paid).unwrap();
                batch = batch
                    .checked_add(cost.execution_paid)
                    .unwrap()
                    .checked_add(cost.blob_paid)
                    .unwrap();
            }
            assert_eq!(
                batch.to_string(),
                post["expected_total_wei"].as_str().unwrap()
            );
        }
        assert_eq!(selected.len(), l1.len());
        let expected = &fixture["expected_l1_costs"];
        assert_eq!(
            post_execution.to_string(),
            expected["postExecutionWei"].as_str().unwrap()
        );
        assert_eq!(
            companion_execution.to_string(),
            expected["companionExecutionWei"].as_str().unwrap()
        );
        assert_eq!(
            blobs.to_string(),
            expected["totalBlobWei"].as_str().unwrap()
        );
        assert_eq!(purchased, U256::from(48));
        assert_eq!(
            (post_execution + companion_execution + blobs).to_string(),
            "221751990942390605"
        );
        // L2 source receipts are retained separately and do not enter L1 totals.
        assert!(l2.values().all(|receipt| receipt.cost.is_ok()));
    }
}
