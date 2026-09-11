//! Offline verifier for a blob-backed `postAndVerifyBatch` evidence bundle.
//!
//! The surrounding Kurtosis smoke test captures unmodified JSON-RPC and
//! beacon REST responses. This program independently authenticates the blob
//! bodies, decodes the strict native EEZ profile, and reports both the L2
//! synchronization payload and standardized cross-chain semantics.

use std::{error::Error, fs, str::FromStr as _};

use alloy_primitives::{B256, Bytes, U256, hex, keccak256};
use alloy_sol_types::SolCall as _;
use eez_blob_codec::{
    BLOB_BYTES, Blob, MAX_BLOBS_PER_TRANSACTION, blob_kzg_commitments, blob_versioned_hashes,
    decode_native_payload,
};
use eez_protocol::abi::postAndVerifyBatchCall;
use eez_protocol::{CallMode, ExecutionOutcome};
use serde_json::{Value, json};

const BLOB_GAS_PER_BLOB: u64 = 1 << 17;

#[path = "verify_blob_evidence/prefixes.rs"]
mod prefixes;

fn main() {
    if let Err(error) = run() {
        eprintln!("blob evidence verification failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let first = args
        .next()
        .ok_or("usage: verify_blob_evidence [--prefixes] <evidence-input.json>")?;
    let prefix_mode = first == "--prefixes";
    let input_path = if prefix_mode {
        args.next().ok_or("missing prefix evidence input")?
    } else {
        first
    };
    if args.next().is_some() {
        return Err("too many arguments".into());
    }

    let evidence: Value = serde_json::from_slice(&fs::read(&input_path)?)?;
    if prefix_mode {
        println!(
            "{}",
            serde_json::to_string_pretty(&prefixes::verify(&evidence)?)?
        );
        return Ok(());
    }
    let rollup_id = required_u64(&evidence, "/rollup_id")?;
    let expected_slot = required_u64(&evidence, "/beacon_slot")?;
    let transaction = required(&evidence, "/l1_transaction")?;
    let receipt = required(&evidence, "/l1_receipt")?;
    let sidecar_response = required(&evidence, "/beacon_sidecars")?;

    let transaction_hash = required_b256(transaction, "/hash")?;
    if required_u64(transaction, "/type")? != 3 {
        return Err(format!("L1 transaction {transaction_hash} is not type 3").into());
    }
    let transaction_block = required_u64(transaction, "/blockNumber")?;
    let transaction_block_hash = required_b256(transaction, "/blockHash")?;
    let chain_id = required_u64(transaction, "/chainId")?;
    let transaction_blob_hashes = required(transaction, "/blobVersionedHashes")?
        .as_array()
        .ok_or("l1_transaction.blobVersionedHashes is not an array")?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_str()
                .ok_or_else(|| format!("blobVersionedHashes[{index}] is not a string"))
                .and_then(|raw| B256::from_str(raw).map_err(|error| error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if transaction_blob_hashes.is_empty()
        || transaction_blob_hashes.len() > MAX_BLOBS_PER_TRANSACTION
    {
        return Err(format!(
            "transaction has {} blobs; expected 1..={MAX_BLOBS_PER_TRANSACTION}",
            transaction_blob_hashes.len()
        )
        .into());
    }

    if required_b256(receipt, "/transactionHash")? != transaction_hash {
        return Err("receipt transactionHash does not match the L1 transaction".into());
    }
    if required_u64(receipt, "/status")? != 1 {
        return Err(format!("L1 transaction {transaction_hash} reverted").into());
    }
    if required_u64(receipt, "/blockNumber")? != transaction_block
        || required_b256(receipt, "/blockHash")? != transaction_block_hash
    {
        return Err("receipt block does not match the L1 transaction".into());
    }
    let expected_blob_gas = u64::try_from(transaction_blob_hashes.len())?
        .checked_mul(BLOB_GAS_PER_BLOB)
        .ok_or("blob gas overflow")?;
    if required_u64(receipt, "/blobGasUsed")? != expected_blob_gas {
        return Err("receipt blobGasUsed does not match the transaction blob count".into());
    }

    let input = required_hex_bytes(transaction, "/input")?;
    let posted = postAndVerifyBatchCall::abi_decode(&input)?;
    if posted.abi_encode() != input {
        return Err("postAndVerifyBatch calldata is not canonical ABI".into());
    }
    if !posted.batch.callData.is_empty() {
        return Err("blob-backed batch carries non-empty ABI callData".into());
    }
    let exact_indices = posted.batch.blobIndices.len() == transaction_blob_hashes.len()
        && posted
            .batch
            .blobIndices
            .iter()
            .enumerate()
            .all(|(position, index)| *index == U256::from(position));
    if !exact_indices {
        return Err("blobIndices do not select every transaction blob once and in order".into());
    }
    if !posted
        .batch
        .rollupIdsWithProofSystems
        .iter()
        .any(|rollup| rollup.rollupId == rollup_id)
    {
        return Err(format!("batch does not prove expected rollup {rollup_id}").into());
    }

    let sidecars = required(sidecar_response, "/data")?
        .as_array()
        .ok_or("beacon blob response data is not an array")?;
    let mut authenticated = Vec::with_capacity(sidecars.len());
    for (position, sidecar) in sidecars.iter().enumerate() {
        // Beacon API v4 getBlobs returns bare blob strings. Pre-Fulu clients
        // may still return legacy sidecar objects, which carry an advertised
        // commitment and signed-header slot that we additionally verify.
        let (physical, advertised_commitment) = if let Some(raw) = sidecar.as_str() {
            (
                decode_hex_string(raw, &format!("beacon blob {position}"))?,
                None,
            )
        } else {
            let slot = required_u64(sidecar, "/signed_block_header/message/slot")?;
            if slot != expected_slot {
                return Err(format!(
                    "sidecar {position} belongs to slot {slot}, expected {expected_slot}"
                )
                .into());
            }
            (
                required_hex_bytes(sidecar, "/blob")?,
                Some(required_hex_bytes(sidecar, "/kzg_commitment")?),
            )
        };
        if physical.len() != BLOB_BYTES {
            return Err(format!(
                "beacon blob {position} has {} bytes, expected {BLOB_BYTES}",
                physical.len()
            )
            .into());
        }
        let blob = Blob::try_from_slice(position, &physical)?;
        let computed_commitment = blob_kzg_commitments(std::slice::from_ref(&blob))?
            .into_iter()
            .next()
            .ok_or("one blob did not produce one KZG commitment")?;
        if advertised_commitment
            .as_ref()
            .is_some_and(|commitment| commitment.as_ref() != computed_commitment)
        {
            return Err(format!("sidecar {position} KZG commitment mismatch").into());
        }
        let versioned_hash = blob_versioned_hashes(std::slice::from_ref(&blob))?
            .into_iter()
            .next()
            .ok_or("one blob did not produce one versioned hash")?;
        authenticated.push((versioned_hash, blob, computed_commitment));
    }

    let mut selected_blobs = Vec::with_capacity(transaction_blob_hashes.len());
    let mut selected_commitments = Vec::with_capacity(transaction_blob_hashes.len());
    for expected_hash in &transaction_blob_hashes {
        let mut matches = authenticated
            .iter()
            .filter(|(computed_hash, _, _)| computed_hash == expected_hash);
        let (_, blob, commitment) = matches.next().ok_or_else(|| {
            format!("beacon response does not contain transaction blob {expected_hash}")
        })?;
        if matches.next().is_some() {
            return Err(
                format!("beacon response duplicates transaction blob {expected_hash}").into(),
            );
        }
        selected_blobs.push(blob.clone());
        selected_commitments.push(format!("0x{}", hex::encode(commitment)));
    }

    let native = decode_native_payload(&selected_blobs, rollup_id)?;
    let semantic_transactions =
        eez_protocol::blob_semantics::decode_semantic_transactions(&native.semantic_messages)?;
    let decoded = eez_payload_codec::decode(&native.operations)?;
    if decoded.block_rlps.is_empty() {
        return Err("blob payload is not the self-contained block profile".into());
    }
    let payload_blocks = decoded
        .block_rlps
        .iter()
        .enumerate()
        .map(|(index, encoded)| {
            let block = alloy_rlp::decode_exact::<reth_ethereum_primitives::Block>(encoded)
                .map_err(|error| format!("decode self-contained block {index}: {error}"))?;
            Ok(json!({
                "number": block.header.number,
                "hash": block.header.hash_slow(),
                "parent_hash": block.header.parent_hash,
                "timestamp": block.header.timestamp,
                "state_root": block.header.state_root,
                "transaction_count": block.body.transactions.len(),
                "rlp_bytes": encoded.len(),
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let payload_transaction_hashes = decoded
        .transactions
        .iter()
        .map(|raw| keccak256(raw).to_string())
        .collect::<Vec<_>>();
    let blob_indices = posted
        .batch
        .blobIndices
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let semantic_details = semantic_transactions
        .iter()
        .map(|transaction| {
            json!({
                "origin_chain": transaction.origin_chain.0,
                "tx_data": format!("0x{}", hex::encode(&transaction.tx_data)),
                "tx_data_bytes": transaction.tx_data.len(),
                "actions": transaction.actions.iter().enumerate().map(|(index, action)| {
                    let (success, return_data) = match &action.outcome {
                        ExecutionOutcome::Resolved { success, return_data, .. } => {
                            (*success, return_data.as_slice())
                        }
                        ExecutionOutcome::Pending => (false, &[][..]),
                    };
                    json!({
                        "index": index,
                        "parent_index": action.parent_index,
                        "kind": match action.call_mode {
                            CallMode::Mutable => "Call",
                            CallMode::Static => "StaticCall",
                        },
                        "source_chain": action.source_rollup_id.0,
                        "target_chain": action.target_rollup_id.0,
                        "from_address": action.source_address,
                        "to_address": action.target_address,
                        "value": action.value.to_string(),
                        "data": format!("0x{}", hex::encode(&action.data)),
                        "success": success,
                        "result_kind": if success { "ReturnSuccess" } else { "ReturnFail" },
                        "return_data": format!("0x{}", hex::encode(return_data)),
                        "revert_span": action.revert_span,
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "verified": true,
            "l1_transaction_hash": transaction_hash,
            "l1_block_number": transaction_block,
            "l1_block_hash": transaction_block_hash,
            "l1_chain_id": chain_id,
            "beacon_slot": expected_slot,
            "rollup_id": rollup_id,
            "blob_count": selected_blobs.len(),
            "blob_gas_used": expected_blob_gas,
            "blob_indices": blob_indices,
            "blob_versioned_hashes": transaction_blob_hashes,
            "kzg_commitments": selected_commitments,
            "abi_call_data_bytes": posted.batch.callData.len(),
            "payload": {
                "bytes": native.operations.len(),
                "semantic_transaction_count": semantic_transactions.len(),
                "semantic_action_count": semantic_transactions
                    .iter()
                    .map(|transaction| transaction.actions.len())
                    .sum::<usize>(),
                "semantic_origins": semantic_transactions
                    .iter()
                    .map(|transaction| transaction.origin_chain.0)
                    .collect::<Vec<_>>(),
                "semantic_transactions": semantic_details,
                "block_count": decoded.block_count(),
                "block_tx_counts": decoded.block_tx_counts,
                "blocks": payload_blocks,
                "transaction_count": decoded.transactions.len(),
                "transaction_hashes": payload_transaction_hashes,
                "l2_entry_count": decoded.l2_entries.len(),
                "outbound_group_sizes": decoded.outbound_group_sizes,
            }
        }))?
    );
    Ok(())
}

fn required<'a>(value: &'a Value, pointer: &str) -> Result<&'a Value, Box<dyn Error>> {
    value
        .pointer(pointer)
        .ok_or_else(|| format!("missing JSON field {pointer}").into())
}

fn required_u64(value: &Value, pointer: &str) -> Result<u64, Box<dyn Error>> {
    let value = required(value, pointer)?;
    if let Some(number) = value.as_u64() {
        return Ok(number);
    }
    let raw = value
        .as_str()
        .ok_or_else(|| format!("JSON field {pointer} is not a u64 or string"))?;
    if let Some(hexadecimal) = raw.strip_prefix("0x") {
        Ok(u64::from_str_radix(hexadecimal, 16)?)
    } else {
        Ok(raw.parse()?)
    }
}

fn required_b256(value: &Value, pointer: &str) -> Result<B256, Box<dyn Error>> {
    let raw = required(value, pointer)?
        .as_str()
        .ok_or_else(|| format!("JSON field {pointer} is not a string"))?;
    Ok(B256::from_str(raw)?)
}

fn required_hex_bytes(value: &Value, pointer: &str) -> Result<Bytes, Box<dyn Error>> {
    let raw = required(value, pointer)?
        .as_str()
        .ok_or_else(|| format!("JSON field {pointer} is not a string"))?;
    decode_hex_string(raw, &format!("JSON field {pointer}"))
}

fn decode_hex_string(raw: &str, field: &str) -> Result<Bytes, Box<dyn Error>> {
    let hexadecimal = raw
        .strip_prefix("0x")
        .ok_or_else(|| format!("{field} is missing its 0x prefix"))?;
    Ok(hex::decode(hexadecimal)?.into())
}
