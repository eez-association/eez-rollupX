//! CI-only prefix reconstruction. State roots come from the test node's
//! debug_intermediateRoots replay, not from composer-provided checkpoint hints.
//! Receipts/body are checked against the blob-authenticated terminal header.
//! This is RPC-backed integration evidence, not a stateless execution proof.

use std::error::Error;

use alloy_consensus::{
    ReceiptEnvelope, Transaction as _, TxReceipt as _,
    proofs::{calculate_receipt_root, calculate_transaction_root},
};
use alloy_eips::{
    eip2718::{Decodable2718 as _, Encodable2718 as _},
    eip7685::EMPTY_REQUESTS_HASH,
};
use alloy_primitives::{B256, Bloom};
use reth_ethereum_primitives::Block;
use serde_json::{Value, json};

use super::{required, required_b256, required_hex_bytes};

pub(super) fn verify(input: &Value) -> Result<Value, Box<dyn Error>> {
    let raw = required_hex_bytes(input, "/block_rlp")?;
    let block = alloy_rlp::decode_exact::<Block>(&raw)?;
    let expected_hash = required_b256(input, "/block_hash")?;
    if block.header.hash_slow() != expected_hash || alloy_rlp::encode(&block) != raw.as_ref() {
        return Err("prefix block does not match the authenticated block hash".into());
    }
    let roots = required(input, "/state_roots")?
        .as_array()
        .ok_or("state_roots is not an array")?
        .iter()
        .map(|value| serde_json::from_value::<B256>(value.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    let raw_receipts = required(input, "/receipts_rlp")?
        .as_array()
        .ok_or("receipts_rlp is not an array")?;
    let mut receipts = Vec::with_capacity(raw_receipts.len());
    for raw in raw_receipts {
        let encoded = super::decode_hex_string(
            raw.as_str().ok_or("receipt is not a hex string")?,
            "receipt",
        )?;
        let receipt = ReceiptEnvelope::decode_2718(&mut encoded.as_ref())?;
        if receipt.encoded_2718() != encoded.as_ref() {
            return Err("receipt encoding is not canonical".into());
        }
        if receipt.logs().iter().collect::<Bloom>() != *receipt.logs_bloom() {
            return Err("receipt bloom does not match its logs".into());
        }
        receipts.push(receipt);
    }
    reconstruct(&block, &receipts, &roots)
}

fn reconstruct(
    block: &Block,
    receipts: &[ReceiptEnvelope],
    roots: &[B256],
) -> Result<Value, Box<dyn Error>> {
    let txs = &block.body.transactions;
    if txs.is_empty() || roots.len() != txs.len() || receipts.len() != txs.len() {
        return Err(
            "prefix evidence must contain one state root and receipt per transaction".into(),
        );
    }
    if !block.body.ommers.is_empty()
        || block
            .body
            .withdrawals
            .as_ref()
            .is_some_and(|items| !items.is_empty())
        || block
            .header
            .requests_hash
            .is_some_and(|hash| hash != EMPTY_REQUESTS_HASH)
    {
        return Err(
            "prefix reconstruction does not support ommers, withdrawals or execution requests"
                .into(),
        );
    }
    if calculate_transaction_root(txs) != block.header.transactions_root
        || calculate_receipt_root(receipts) != block.header.receipts_root
        || roots.last() != Some(&block.header.state_root)
    {
        return Err(
            "prefix evidence disagrees with terminal transaction/receipt/state roots".into(),
        );
    }
    let mut prefixes = Vec::with_capacity(txs.len());
    let mut bloom = Bloom::ZERO;
    let mut gas = 0;
    let mut blob_gas = 0_u64;
    for (index, ((tx, receipt), root)) in txs.iter().zip(receipts).zip(roots).enumerate() {
        if receipt.cumulative_gas_used() < gas || receipt.tx_type() as u8 != tx.tx_type() as u8 {
            return Err("prefix receipts have invalid cumulative gas or transaction types".into());
        }
        gas = receipt.cumulative_gas_used();
        bloom |= receipt.bloom();
        blob_gas = blob_gas
            .checked_add(tx.blob_gas_used().unwrap_or(0))
            .ok_or("blob gas overflow")?;
        let mut header = block.header.clone();
        header.state_root = *root;
        header.transactions_root = calculate_transaction_root(&txs[..=index]);
        header.receipts_root = calculate_receipt_root(&receipts[..=index]);
        header.logs_bloom = bloom;
        header.gas_used = gas;
        if header.blob_gas_used.is_some() {
            header.blob_gas_used = Some(blob_gas);
        }
        let hash = header.hash_slow();
        if index + 1 == txs.len() && hash != block.header.hash_slow() {
            return Err(
                "final reconstructed prefix is not the authenticated terminal block".into(),
            );
        }
        prefixes.push(json!({"transaction_index": index, "hash": hash, "state_root": root}));
    }
    Ok(
        json!({"verified": true, "block_hash": block.header.hash_slow(),
        "block_number": block.header.number, "state_root_source": "debug_intermediateRoots",
        "prefixes": prefixes}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Eip658Value, Receipt, TxLegacy};
    use alloy_primitives::{Signature, U256};
    use reth_ethereum_primitives::{Transaction, TransactionSigned};

    fn fixture() -> (Block, Vec<ReceiptEnvelope>, Vec<B256>) {
        let mut block = Block::default();
        block.header.number = 42;
        block.body.transactions = (0..2)
            .map(|nonce| {
                TransactionSigned::new_unhashed(
                    Transaction::Legacy(TxLegacy {
                        nonce,
                        ..Default::default()
                    }),
                    Signature::new(U256::ONE, U256::ONE, false),
                )
            })
            .collect();
        let receipts = (1..=2)
            .map(|index| {
                ReceiptEnvelope::Legacy(
                    Receipt {
                        status: Eip658Value::Eip658(true),
                        cumulative_gas_used: index * 21_000,
                        logs: vec![],
                    }
                    .with_bloom(),
                )
            })
            .collect::<Vec<_>>();
        let roots = vec![B256::repeat_byte(0x11), B256::repeat_byte(0x22)];
        block.header.state_root = roots[1];
        block.header.transactions_root = calculate_transaction_root(&block.body.transactions);
        block.header.receipts_root = calculate_receipt_root(&receipts);
        block.header.gas_used = 42_000;
        (block, receipts, roots)
    }

    fn input(block: &Block, receipts: &[ReceiptEnvelope], roots: &[B256]) -> Value {
        json!({"block_hash":block.header.hash_slow(),
            "block_rlp":format!("0x{}", alloy_primitives::hex::encode(alloy_rlp::encode(block))),
            "receipts_rlp":receipts.iter().map(|r| format!("0x{}", alloy_primitives::hex::encode(r.encoded_2718()))).collect::<Vec<_>>(),
            "state_roots":roots})
    }

    #[test]
    fn reconstructs_intermediate_and_terminal_headers() {
        let (block, receipts, roots) = fixture();
        let output = verify(&input(&block, &receipts, &roots)).unwrap();
        assert_eq!(output["prefixes"].as_array().unwrap().len(), 2);
        assert_ne!(output["prefixes"][0]["hash"], output["block_hash"]);
        assert_eq!(output["prefixes"][1]["hash"], output["block_hash"]);
        let mut prefix = block.header.clone();
        prefix.state_root = roots[0];
        prefix.transactions_root = calculate_transaction_root(&block.body.transactions[..1]);
        prefix.receipts_root = calculate_receipt_root(&receipts[..1]);
        prefix.gas_used = 21_000;
        assert_eq!(output["prefixes"][0]["hash"], json!(prefix.hash_slow()));
    }

    #[test]
    fn rejects_incomplete_or_inconsistent_replay_evidence() {
        let (block, receipts, roots) = fixture();
        assert!(reconstruct(&block, &receipts[..1], &roots).is_err());
        assert!(reconstruct(&block, &receipts, &roots[..1]).is_err());
        assert!(reconstruct(&block, &receipts, &[roots[0], B256::ZERO]).is_err());
        let mut reordered = receipts.clone();
        reordered.reverse();
        assert!(reconstruct(&block, &reordered, &roots).is_err());
        for field in [
            "transactions_root",
            "receipts_root",
            "gas_used",
            "logs_bloom",
            "requests_hash",
        ] {
            let mut mutated = block.clone();
            match field {
                "transactions_root" => mutated.header.transactions_root = B256::ZERO,
                "receipts_root" => mutated.header.receipts_root = B256::ZERO,
                "gas_used" => mutated.header.gas_used += 1,
                "logs_bloom" => mutated.header.logs_bloom = Bloom::repeat_byte(1),
                "requests_hash" => mutated.header.requests_hash = Some(B256::ZERO),
                _ => unreachable!(),
            }
            assert!(
                verify(&input(&mutated, &receipts, &roots)).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn rejects_wrong_block_and_noncanonical_receipt_bytes() {
        let (block, receipts, roots) = fixture();
        let mut evidence = input(&block, &receipts, &roots);
        evidence["block_hash"] = json!(B256::ZERO);
        assert!(verify(&evidence).is_err());
        let mut evidence = input(&block, &receipts, &roots);
        evidence["receipts_rlp"][0] = json!(format!(
            "{}00",
            evidence["receipts_rlp"][0].as_str().unwrap()
        ));
        assert!(verify(&evidence).is_err());
    }

    #[test]
    fn rejects_invalid_receipts_even_when_their_root_matches() {
        let (mut block, mut receipts, roots) = fixture();
        let ReceiptEnvelope::Legacy(receipt) = receipts[0].clone() else {
            unreachable!()
        };
        receipts[0] = ReceiptEnvelope::Eip1559(receipt);
        block.header.receipts_root = calculate_receipt_root(&receipts);
        assert!(verify(&input(&block, &receipts, &roots)).is_err());

        let (mut block, mut receipts, roots) = fixture();
        let ReceiptEnvelope::Legacy(receipt) = &mut receipts[0] else {
            unreachable!()
        };
        receipt.receipt.cumulative_gas_used = 50_000; // next receipt only has 42,000
        block.header.receipts_root = calculate_receipt_root(&receipts);
        assert!(verify(&input(&block, &receipts, &roots)).is_err());

        let (mut block, mut receipts, roots) = fixture();
        let ReceiptEnvelope::Legacy(receipt) = &mut receipts[0] else {
            unreachable!()
        };
        receipt.logs_bloom = Bloom::repeat_byte(1); // no logs
        block.header.receipts_root = calculate_receipt_root(&receipts);
        assert!(verify(&input(&block, &receipts, &roots)).is_err());
    }

    #[test]
    fn rejects_unsupported_post_transaction_state_changes() {
        let (mut block, receipts, roots) = fixture();
        block.body.ommers.push(block.header.clone());
        assert!(verify(&input(&block, &receipts, &roots)).is_err());
        let (mut block, receipts, roots) = fixture();
        block.body.withdrawals = Some(vec![alloy_eips::eip4895::Withdrawal::default()].into());
        assert!(verify(&input(&block, &receipts, &roots)).is_err());
    }
}
