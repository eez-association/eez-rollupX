//! Independent causal verifier for real-network E2E evidence.
//!
//! Input is a JSON document assembled from canonical transaction calldata and
//! receipts. The verifier selects the scenario's actual L2 entries by their
//! runtime entry hashes, pairs them with the actual posted L1 entries, invokes
//! the protocol's independent two-table verifier, and checks the retained
//! source/destination events as exact multisets.

use std::collections::HashMap;
#[cfg(not(test))]
use std::env;
use std::error::Error;
#[cfg(not(test))]
use std::fs;
use std::str::FromStr;

use alloy_primitives::{Address, B256, Bytes, Log, LogData, keccak256};
use alloy_sol_types::SolCall;
use eez_protocol::RollupId;
use eez_protocol::abi::{
    ExecutionEntrySol, L2StaticExecutionEntrySol, executeIncomingCrossChainCallCall,
    loadExecutionTableCall, postAndVerifyBatchCall,
};
use eez_protocol::entries::{NestedPairDirection, verify_nested_group};
use eez_protocol::outbound_gate::{observations_from_logs, verify_expected_outbound_hashes};
use eez_protocol::system_tx::{OutboundSourceEntry, lift_entry_from_l2};
use serde_json::{Value, json};

type AnyResult<T> = Result<T, Box<dyn Error>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Inbound,
    Outbound,
}

fn fail<T>(message: impl Into<String>) -> AnyResult<T> {
    Err(message.into().into())
}

fn required_str<'a>(input: &'a Value, key: &str) -> AnyResult<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string field {key}").into())
}

#[cfg(test)]
fn parse_bytes(input: &Value, key: &str) -> AnyResult<Bytes> {
    Bytes::from_str(required_str(input, key)?).map_err(Into::into)
}

fn parse_bytes_list(input: &Value, key: &str) -> AnyResult<Vec<Bytes>> {
    let value = input
        .get(key)
        .ok_or_else(|| format!("missing field {key}"))?;
    if let Some(value) = value.as_str() {
        return Ok(vec![Bytes::from_str(value)?]);
    }
    value
        .as_array()
        .ok_or_else(|| format!("{key} is neither a hex string nor an array"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| format!("{key} contains a non-string value"))
                .and_then(|value| Bytes::from_str(value).map_err(|error| error.to_string()))
                .map_err(Into::into)
        })
        .collect()
}

fn runtime_entry_hash(entry: &ExecutionEntrySol) -> B256 {
    let mut preimage = [0_u8; 64];
    preimage[..32].copy_from_slice(entry.proxyEntryHash.as_slice());
    preimage[32..].copy_from_slice(entry.rollingHash.as_slice());
    keccak256(preimage)
}

fn expected_hashes(input: &Value) -> AnyResult<Vec<B256>> {
    input
        .get("expected_l2_entry_hashes")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing expected_l2_entry_hashes".to_owned())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| "expected L2 entry hash is not a string".to_owned())
                .and_then(|value| B256::from_str(value).map_err(|error| error.to_string()))
                .map_err(Into::into)
        })
        .collect()
}

fn select_static_entry(
    entries: &[L2StaticExecutionEntrySol],
    key: B256,
) -> AnyResult<(usize, &L2StaticExecutionEntrySol)> {
    let mut matches = entries
        .iter()
        .enumerate()
        // This diagnostic selects a pure static root immediately after its load,
        // so no mutable entry has advanced the cursor.
        .filter(|(_, entry)| entry.expectedEntryIndex.is_zero() && entry.proxyEntryHash == key);
    let selected = matches.next().ok_or("requested static entry is missing")?;
    if matches.next().is_some() {
        return fail("requested static entry is duplicated");
    }
    Ok(selected)
}

fn exact_multiset(label: &str, expected: &[B256], actual: &[B256]) -> AnyResult<()> {
    let mut remaining = HashMap::<B256, usize>::new();
    for hash in expected {
        *remaining.entry(*hash).or_default() += 1;
    }
    for (index, hash) in actual.iter().enumerate() {
        let Some(count) = remaining.get_mut(hash) else {
            return fail(format!("{label}: unexpected element {index} ({hash})"));
        };
        if *count == 0 {
            return fail(format!("{label}: duplicate element {index} ({hash})"));
        }
        *count -= 1;
    }
    let missing = remaining.values().copied().sum::<usize>();
    if missing != 0 || expected.len() != actual.len() {
        return fail(format!(
            "{label}: {missing} expected element(s) missing (expected {}, actual {})",
            expected.len(),
            actual.len()
        ));
    }
    Ok(())
}

fn select_entries(
    actual: &[ExecutionEntrySol],
    expected: &[B256],
) -> AnyResult<Vec<(usize, ExecutionEntrySol)>> {
    let mut remaining = HashMap::<B256, usize>::new();
    for hash in expected {
        *remaining.entry(*hash).or_default() += 1;
    }
    let mut selected = Vec::with_capacity(expected.len());
    for (index, entry) in actual.iter().enumerate() {
        let hash = runtime_entry_hash(entry);
        if let Some(count) = remaining.get_mut(&hash)
            && *count != 0
        {
            *count -= 1;
            selected.push((index, entry.clone()));
        }
    }
    let missing = remaining.values().copied().sum::<usize>();
    if missing != 0 || selected.len() != expected.len() {
        return fail(format!(
            "loaded L2 table is missing {missing} expected entry occurrence(s)"
        ));
    }
    Ok(selected)
}

fn decode_l2_tables(
    calldata: &[u8],
    destination: RollupId,
) -> AnyResult<(Vec<ExecutionEntrySol>, Vec<L2StaticExecutionEntrySol>)> {
    if calldata.starts_with(&loadExecutionTableCall::SELECTOR) {
        let call = loadExecutionTableCall::abi_decode(calldata)?;
        if call.abi_encode() != calldata {
            return fail("loadExecutionTable calldata is not canonical ABI");
        }
        return Ok((
            call._entries
                .iter()
                .map(|entry| lift_entry_from_l2(entry, destination))
                .collect(),
            call._staticEntries,
        ));
    }
    if calldata.starts_with(&executeIncomingCrossChainCallCall::SELECTOR) {
        let call = executeIncomingCrossChainCallCall::abi_decode(calldata)?;
        if call.abi_encode() != calldata {
            return fail("executeIncomingCrossChainCall calldata is not canonical ABI");
        }
        return Ok((
            call._entries
                .iter()
                .map(|entry| lift_entry_from_l2(entry, destination))
                .collect(),
            call._staticEntries,
        ));
    }
    fail("L2 system transaction is neither loadExecutionTable nor executeIncomingCrossChainCall")
}

fn receipt_logs(receipts: &Value) -> AnyResult<Vec<Log>> {
    let receipts = receipts
        .as_array()
        .ok_or_else(|| "receipt collection is not an array".to_owned())?;
    let mut decoded = Vec::new();
    for receipt in receipts {
        let logs = receipt
            .get("logs")
            .and_then(Value::as_array)
            .ok_or_else(|| "receipt has no logs array".to_owned())?;
        for log in logs {
            let address = Address::from_str(
                log.get("address")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "log has no address".to_owned())?,
            )?;
            let topics = log
                .get("topics")
                .and_then(Value::as_array)
                .ok_or_else(|| "log has no topics".to_owned())?
                .iter()
                .map(|topic| {
                    topic
                        .as_str()
                        .ok_or_else(|| "log topic is not a string".to_owned())
                        .and_then(|topic| B256::from_str(topic).map_err(|error| error.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let data = Bytes::from_str(
                log.get("data")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "log has no data".to_owned())?,
            )?;
            decoded.push(Log {
                address,
                data: LogData::new_unchecked(topics, data),
            });
        }
    }
    Ok(decoded)
}

fn indexed_hashes(logs: &[Log], address: Address, signature: B256) -> Vec<B256> {
    logs.iter()
        .filter(|log| log.address == address)
        .filter(|log| log.data.topics().first() == Some(&signature))
        .filter_map(|log| log.data.topics().get(1).copied())
        .collect()
}

fn parse_direction(input: &Value) -> AnyResult<Direction> {
    match required_str(input, "direction")? {
        "L1_to_L2" => Ok(Direction::Inbound),
        "L2_to_L1" => Ok(Direction::Outbound),
        other => fail(format!("unsupported direction {other}")),
    }
}

#[cfg(not(test))]
fn main() -> AnyResult<()> {
    let mut args = env::args().skip(1);
    let input_path = args
        .next()
        .ok_or("usage: e2e-causal-verify <input.json> <output.json>")?;
    let output_path = args.next().ok_or("missing output path")?;
    if args.next().is_some() {
        return fail("too many arguments");
    }

    let input: Value = serde_json::from_slice(&fs::read(input_path)?)?;
    let output = verify_input(&input)?;
    fs::write(output_path, serde_json::to_vec_pretty(&output)?)?;
    Ok(())
}

fn verify_input(input: &Value) -> AnyResult<Value> {
    let direction = parse_direction(input)?;
    let rollup_id = input.get("rollup_id").and_then(Value::as_u64).unwrap_or(1);
    let l2_rollup = RollupId(rollup_id);
    if l2_rollup.is_mainnet() {
        return fail("rollup_id must be non-zero");
    }
    let registry = Address::from_str(required_str(input, "registry")?)?;
    let manager_l2 = Address::from_str(required_str(input, "manager_l2")?)?;
    let l1_calldata = parse_bytes_list(input, "l1_calldata")?;
    let l2_calldata = parse_bytes_list(input, "l2_calldata")?;
    let select_static = input
        .get("select_static")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if l1_calldata.is_empty() {
        return fail("no L1 settlement calldata");
    }
    if l1_calldata.len() > 1 && (direction != Direction::Inbound || select_static) {
        return fail("multiple settlements require mutable inbound evidence");
    }
    let mut posted_batches = Vec::with_capacity(l1_calldata.len());
    for calldata in &l1_calldata {
        let call = postAndVerifyBatchCall::abi_decode(calldata)?;
        if call.abi_encode() != *calldata {
            return fail("postAndVerifyBatch calldata is not canonical ABI");
        }
        posted_batches.push(call.batch);
    }
    let posted = &posted_batches[0];
    let posted_entry_count = posted_batches
        .iter()
        .map(|batch| batch.entries.len())
        .sum::<usize>();
    let l2_destination = match direction {
        Direction::Inbound => l2_rollup,
        Direction::Outbound => RollupId::MAINNET,
    };
    let mut loaded_entries = Vec::new();
    let mut loaded_static = Vec::new();
    for calldata in &l2_calldata {
        let (entries, static_entries) = decode_l2_tables(calldata, l2_destination)?;
        loaded_entries.extend(entries);
        loaded_static.extend(static_entries);
    }
    let expected_l2 = expected_hashes(input)?;
    let selected = select_entries(&loaded_entries, &expected_l2)?;

    let l1_logs = receipt_logs(input.get("l1_receipts").ok_or("missing l1_receipts")?)?;
    let l2_logs = receipt_logs(input.get("l2_receipts").ok_or("missing l2_receipts")?)?;
    let mut pairs = Vec::new();
    let mut expected_outbound_hashes = Vec::new();

    match (direction, select_static) {
        (Direction::Inbound, false) => {
            if selected.is_empty() {
                return fail("inbound causal verification selected no L2 entries");
            }
            // A multi-transaction scenario can settle across blocks. The
            // collector supplies whole batches in canonical transaction order;
            // preserve that order and report indices within each original batch.
            let ordered_l1 = posted_batches
                .iter()
                .enumerate()
                .flat_map(|(batch_index, batch)| {
                    batch
                        .entries
                        .iter()
                        .enumerate()
                        .map(move |(entry_index, entry)| (batch_index, entry_index, entry))
                })
                .collect::<Vec<_>>();
            let mut next_l1 = 0_usize;
            let mut paired_l1 = Vec::new();
            let mut used_batches = vec![false; posted_batches.len()];
            for (l2_index, l2_entry) in &selected {
                let mut match_index = None;
                for (l1_index, (_, _, l1_entry)) in ordered_l1.iter().enumerate().skip(next_l1) {
                    if l1_entry.proxyEntryHash != l2_entry.proxyEntryHash {
                        continue;
                    }
                    if let Ok(facts) = verify_nested_group(
                        l1_entry,
                        std::slice::from_ref(l2_entry),
                        NestedPairDirection::Inbound,
                        l2_rollup,
                    ) {
                        match_index = Some((l1_index, facts));
                        break;
                    }
                }
                let Some((l1_index, facts)) = match_index else {
                    return fail(format!(
                        "no ordered L1 entry causally verifies L2 entry {l2_index}"
                    ));
                };
                next_l1 = l1_index + 1;
                let (batch_index, entry_index, l1_entry) = ordered_l1[l1_index];
                used_batches[batch_index] = true;
                if l1_entry.success {
                    paired_l1.push(l1_entry.proxyEntryHash);
                }
                expected_outbound_hashes.extend(facts.l2_outbound_hashes.iter().copied());
                pairs.push(json!({
                    "kind": "mutable",
                    "direction": "L1_to_L2",
                    "l1_settlement_transaction_index": batch_index,
                    "l1_entry_indices": [entry_index],
                    "l2_entry_indices": [l2_index],
                    "proxy_entry_hash": format!("{}", l1_entry.proxyEntryHash),
                    "ether_delta": facts.ether_delta.to_string(),
                    "l2_outbound_hashes": facts.l2_outbound_hashes.iter()
                        .map(ToString::to_string).collect::<Vec<_>>()
                }));
            }
            if used_batches.iter().any(|used| !used) {
                return fail("L1 settlement contains no selected causal entry");
            }

            let l1_consumed_signature =
                keccak256("ExecutionConsumed(bytes32,uint64,uint256)".as_bytes());
            let observed_l1_consumed = indexed_hashes(&l1_logs, registry, l1_consumed_signature);
            exact_multiset("L1 trigger consumptions", &paired_l1, &observed_l1_consumed)?;

            let incoming_signature = keccak256(
                "IncomingCrossChainCallExecuted(bytes32,bool,address,uint64,address,uint256,uint64,bytes)"
                    .as_bytes(),
            );
            let expected_incoming = selected
                .iter()
                .map(|(_, entry)| entry.proxyEntryHash)
                .collect::<Vec<_>>();
            let observed_incoming = indexed_hashes(&l2_logs, manager_l2, incoming_signature);
            exact_multiset(
                "L2 destination deliveries",
                &expected_incoming,
                &observed_incoming,
            )?;
        }
        (Direction::Outbound, false) => {
            if selected.is_empty() && loaded_static.is_empty() {
                return fail(
                    "outbound causal verification selected no L2 entries or static sources",
                );
            }
            // Each outbound user has its own complete load. Static positions
            // are local to that load, so combining pools from multiple loads or
            // filtering away mutable rows would change their meaning.
            if !loaded_static.is_empty()
                && (l2_calldata.len() != 1
                    || !l2_calldata[0].starts_with(&loadExecutionTableCall::SELECTOR)
                    || selected.len() != loaded_entries.len())
            {
                return fail("mixed outbound evidence requires one complete source load");
            }
            let sources = selected
                .iter()
                .map(|(_, entry)| OutboundSourceEntry::Mutable(entry.clone()))
                .chain(
                    loaded_static
                        .iter()
                        .cloned()
                        .map(OutboundSourceEntry::Static),
                )
                .collect::<Vec<_>>();
            let mut candidates = Vec::new();
            for (l1_index, l1_entry) in posted.entries.iter().enumerate() {
                if let Ok(facts) = eez_protocol::entries::verify_outbound_source_group(
                    l1_entry, &sources, l2_rollup,
                ) {
                    candidates.push((l1_index, facts));
                }
            }
            if candidates.len() != 1 {
                return fail(format!(
                    "expected one L1 host for the L2 user transaction, found {}",
                    candidates.len()
                ));
            }
            let (l1_index, facts) = candidates.remove(0);
            expected_outbound_hashes.extend(facts.l2_outbound_hashes.iter().copied());
            pairs.push(json!({
                "kind": if loaded_static.is_empty() { "mutable" }
                    else if selected.is_empty() { "static" } else { "mixed" },
                "direction": "L2_to_L1",
                "l2_static_entry_indices": (0..loaded_static.len()).collect::<Vec<_>>(),
                "l1_entry_indices": [l1_index],
                "l2_entry_indices": selected.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
                "proxy_entry_hash": format!("{}", posted.entries[l1_index].proxyEntryHash),
                "ether_delta": facts.ether_delta.to_string(),
                "l2_outbound_hashes": facts.l2_outbound_hashes.iter()
                    .map(ToString::to_string).collect::<Vec<_>>()
            }));
        }
        (Direction::Outbound, true) => {
            if !selected.is_empty() || !expected_l2.is_empty() {
                return fail("static outbound evidence cannot select mutable entries");
            }
            // Select from canonical, unmodified shared table calldata. Never
            // pre-filter the calldata or silently accept a duplicate key.
            let key = B256::from_str(required_str(input, "expected_l2_static_key")?)?;
            let (static_index, selected_static) = select_static_entry(&loaded_static, key)?;
            let source = selected_static;
            let mut candidates = Vec::new();
            for (l1_index, host) in posted.entries.iter().enumerate() {
                if eez_protocol::entries::verify_outbound_source_group(
                    host,
                    &[OutboundSourceEntry::Static(source.clone())],
                    l2_rollup,
                )
                .is_ok()
                {
                    candidates.push(l1_index);
                }
            }
            if candidates.len() != 1 {
                return fail(format!(
                    "expected one L1 host for the static L2 source, found {}",
                    candidates.len()
                ));
            }
            pairs.push(json!({
                "kind": "static",
                "direction": "L2_to_L1",
                "l1_entry_indices": candidates,
                "l2_static_entry_indices": [static_index],
                "proxy_entry_hash": format!("{}", source.proxyEntryHash),
                "ether_delta": "0",
                "l2_outbound_hashes": []
            }));
        }
        (Direction::Inbound, true) => {
            return fail("top-level L1-to-L2 static reads have no two-table causal pair");
        }
    }

    let observations = observations_from_logs(&l2_logs, manager_l2);
    verify_expected_outbound_hashes(&expected_outbound_hashes, &observations)
        .map_err(|error| format!("L2 source-event bijection failed: {error}"))?;

    let output = json!({
        "verified": true,
        "direction": match direction { Direction::Inbound => "L1_to_L2", Direction::Outbound => "L2_to_L1" },
        "posted_l1_entries": posted_entry_count,
        "l1_settlement_transactions": posted_batches.len(),
        "l2_system_transactions": l2_calldata.len(),
        "loaded_l2_entries": loaded_entries.len(),
        "loaded_l2_static_entries": loaded_static.len(),
        "selected_l2_entries": selected.len(),
        "pairs": pairs,
        "event_bijection": {
            "expected_l2_outbound": expected_outbound_hashes.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "observed_l2_outbound": observations.iter().map(|event| event.call_hash().to_string()).collect::<Vec<_>>()
        },
        "claims": {
            "canonical_l1_calldata": true,
            "canonical_l2_calldata": true,
            "ordered_two_table_pairing": true,
            "full_call_and_result_verification": true,
            "rolling_hashes_recomputed": true,
            "ether_delta_recomputed": true,
            "event_multiplicity_bijection": true
        }
    });
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inbound_fixture() -> Value {
        use alloy_primitives::U256;
        use eez_protocol::abi::{CrossChainCallSol, L2ExecutionEntrySol, RootUpdateSol};
        use eez_protocol::action::{CallHashInput, CallMode, common_cross_chain_call_hash};
        use eez_protocol::rolling_hash::EntryRollingHash;

        let source = Address::repeat_byte(0x11);
        let target = Address::repeat_byte(0x22);
        let registry = Address::repeat_byte(0x33);
        let manager = Address::repeat_byte(0x44);
        let key = common_cross_chain_call_hash(CallHashInput {
            call_mode: CallMode::Mutable,
            source_address: source,
            source_rollup_id: RollupId::MAINNET,
            target_address: target,
            target_rollup_id: RollupId(1),
            value: U256::ZERO,
            data: &[],
        });
        let mut post = postAndVerifyBatchCall {
            batch: Default::default(),
        };
        let mut deliveries = Vec::new();
        let mut expected = Vec::new();
        for n in 1..=3_u8 {
            let current_root = B256::with_last_byte(n - 1);
            let result = Bytes::from(U256::from(n).to_be_bytes::<32>().to_vec());
            post.batch.entries.push(ExecutionEntrySol {
                rootUpdates: vec![RootUpdateSol {
                    rollupId: 1,
                    currentRoot: current_root,
                    newRoot: B256::with_last_byte(n),
                    etherDelta: Default::default(),
                }],
                proxyEntryHash: key,
                destinationRollupId: 1,
                rollingHash: EntryRollingHash::seed_for_l1([(1, current_root)], key).current(),
                success: true,
                returnData: result.clone(),
                ..Default::default()
            });
            let mut rolling = EntryRollingHash::seed_for_l2(key);
            rolling.call_begin(key);
            rolling.call_end(true, &result);
            let entry = L2ExecutionEntrySol {
                proxyEntryHash: key,
                incomingCalls: vec![CrossChainCallSol {
                    sourceAddress: source,
                    sourceRollupId: 0,
                    targetAddress: target,
                    gas: 0,
                    revertNextNCalls: 0,
                    isStatic: false,
                    value: U256::ZERO,
                    data: Bytes::new(),
                }],
                rollingHash: rolling.current(),
                success: true,
                returnData: result,
                ..Default::default()
            };
            expected.push(runtime_entry_hash(&lift_entry_from_l2(&entry, RollupId(1))));
            deliveries.push(Bytes::from(
                executeIncomingCrossChainCallCall {
                    _entries: vec![entry],
                    _staticEntries: vec![],
                }
                .abi_encode(),
            ));
        }
        let log = |address: Address, signature: &str| {
            json!({
                "address": address, "topics": [keccak256(signature), key], "data": "0x"
            })
        };
        json!({
            "direction": "L1_to_L2", "rollup_id": 1,
            "registry": registry, "manager_l2": manager,
            "l1_calldata": Bytes::from(post.abi_encode()),
            "l2_calldata": deliveries, "expected_l2_entry_hashes": expected,
            "l1_receipts": [{"logs": vec![log(registry, "ExecutionConsumed(bytes32,uint64,uint256)"); 3]}],
            "l2_receipts": [{"logs": vec![log(manager,
                "IncomingCrossChainCallExecuted(bytes32,bool,address,uint64,address,uint256,uint64,bytes)"); 3]}]
        })
    }

    #[test]
    fn inbound_multi_transaction_evidence_spans_settlement_batches() {
        let mut input = inbound_fixture();
        assert_eq!(
            verify_input(&input).unwrap()["pairs"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
        let mut first =
            postAndVerifyBatchCall::abi_decode(&parse_bytes(&input, "l1_calldata").unwrap())
                .unwrap();
        let mut second = first.clone();
        second.batch.entries = first.batch.entries.split_off(2);
        input["l1_calldata"] = json!(Bytes::from(first.abi_encode()));
        assert!(
            verify_input(&input)
                .unwrap_err()
                .to_string()
                .contains("L2 entry 2")
        );
        input["l1_calldata"] = json!([
            Bytes::from(first.abi_encode()),
            Bytes::from(second.abi_encode())
        ]);
        let result = verify_input(&input).unwrap();
        assert_eq!(result["posted_l1_entries"], 3);
        assert_eq!(result["l1_settlement_transactions"], 2);
        assert_eq!(result["pairs"][2]["l1_settlement_transaction_index"], 1);
        assert_eq!(result["pairs"][2]["l1_entry_indices"], json!([0]));
    }

    #[test]
    fn split_inbound_evidence_rejects_incomplete_reordered_and_corrupt_inputs() {
        for mutation in [
            "missing_batch",
            "reordered_batches",
            "changed_result",
            "noncanonical",
            "empty",
            "unused_batch",
            "missing_delivery",
            "duplicate_delivery",
            "missing_trigger",
        ] {
            let mut input = inbound_fixture();
            let mut first =
                postAndVerifyBatchCall::abi_decode(&parse_bytes(&input, "l1_calldata").unwrap())
                    .unwrap();
            let mut second = first.clone();
            second.batch.entries = first.batch.entries.split_off(2);
            if mutation == "changed_result" {
                second.batch.entries[0].returnData = Bytes::from(vec![0xff]);
            }
            let mut batches = vec![
                Bytes::from(first.abi_encode()),
                Bytes::from(second.abi_encode()),
            ];
            let expected_error = match mutation {
                "missing_batch" => {
                    batches.pop();
                    "L2 entry 2"
                }
                "reordered_batches" => {
                    batches.reverse();
                    "no ordered L1 entry"
                }
                "changed_result" => "L2 entry 2",
                "noncanonical" => {
                    let mut bytes = batches[1].to_vec();
                    bytes.extend_from_slice(&[0; 32]);
                    batches[1] = Bytes::from(bytes);
                    "not canonical ABI"
                }
                "empty" => {
                    batches.clear();
                    "no L1 settlement"
                }
                "unused_batch" => {
                    batches.push(Bytes::from(second.abi_encode()));
                    "no selected causal entry"
                }
                "missing_delivery" => {
                    input["l2_receipts"][0]["logs"]
                        .as_array_mut()
                        .unwrap()
                        .pop();
                    "L2 destination deliveries"
                }
                "duplicate_delivery" => {
                    let log = input["l2_receipts"][0]["logs"][0].clone();
                    input["l2_receipts"][0]["logs"]
                        .as_array_mut()
                        .unwrap()
                        .push(log);
                    "L2 destination deliveries"
                }
                "missing_trigger" => {
                    input["l1_receipts"][0]["logs"]
                        .as_array_mut()
                        .unwrap()
                        .pop();
                    "L1 trigger consumptions"
                }
                _ => unreachable!(),
            };
            input["l1_calldata"] = json!(batches);
            let error = verify_input(&input).unwrap_err().to_string();
            assert!(error.contains(expected_error), "{mutation}: {error}");
        }
        let mut outbound = mixed_fixture(false);
        outbound["l1_calldata"] = json!([outbound["l1_calldata"], outbound["l1_calldata"]]);
        assert!(
            verify_input(&outbound)
                .unwrap_err()
                .to_string()
                .contains("mutable inbound evidence")
        );
    }

    fn static_only_fixture() -> Value {
        use alloy_primitives::U256;
        use eez_protocol::abi::{L2ToL1CallSol, RootUpdateSol};
        use eez_protocol::{
            CallHashInput, CallMode, EntryRollingHash, common_cross_chain_call_hash,
        };
        let mut host = ExecutionEntrySol {
            destinationRollupId: 1,
            success: true,
            rootUpdates: vec![RootUpdateSol {
                rollupId: 1,
                currentRoot: B256::ZERO,
                newRoot: B256::with_last_byte(1),
                etherDelta: Default::default(),
            }],
            ..Default::default()
        };
        let mut rolling = EntryRollingHash::seed_for_l1([(1, B256::ZERO)], B256::ZERO);
        let mut statics = vec![];
        for tag in [1_u8, 2] {
            let call = L2ToL1CallSol {
                isStatic: true,
                sourceRollupId: 1,
                sourceAddress: Address::with_last_byte(1),
                targetAddress: Address::with_last_byte(2),
                value: U256::ZERO,
                gas: 0,
                revertNextNCalls: 0,
                data: Bytes::from(vec![tag]),
            };
            let key = common_cross_chain_call_hash(CallHashInput {
                call_mode: CallMode::Static,
                source_rollup_id: RollupId(1),
                target_rollup_id: RollupId::MAINNET,
                source_address: call.sourceAddress,
                target_address: call.targetAddress,
                value: U256::ZERO,
                data: &call.data,
            });
            rolling.call_begin(key);
            rolling.call_end(true, &[tag]);
            host.l2ToL1Calls.push(call);
            statics.push(L2StaticExecutionEntrySol {
                expectedEntryIndex: U256::ZERO,
                proxyEntryHash: key,
                incomingCalls: vec![],
                rollingHash: B256::ZERO,
                success: true,
                returnData: Bytes::from(vec![tag]),
            });
        }
        host.rollingHash = rolling.current();
        let mut post = postAndVerifyBatchCall {
            batch: Default::default(),
        };
        post.batch.entries.push(host);
        let load = loadExecutionTableCall {
            _entries: vec![],
            _staticEntries: statics,
        };
        json!({
            "direction":"L2_to_L1", "rollup_id":1,
            "registry":Address::with_last_byte(3), "manager_l2":Address::with_last_byte(4),
            "l1_calldata":Bytes::from(post.abi_encode()),
            "l2_calldata":[Bytes::from(load.abi_encode())],
            "expected_l2_entry_hashes":[], "l1_receipts":[{"logs":[]}], "l2_receipts":[{"logs":[]}]
        })
    }

    #[test]
    fn static_only_evidence_requires_the_complete_position_bound_source_pool() {
        let input = static_only_fixture();
        let verified = verify_input(&input).expect("complete static-only load");
        assert_eq!(verified["pairs"][0]["kind"], "static");
        assert_eq!(
            verified["pairs"][0]["l2_static_entry_indices"],
            json!([0, 1])
        );
        for mutation in ["missing", "extra", "position", "result", "split"] {
            let mut forged = input.clone();
            let raw = parse_bytes_list(&forged, "l2_calldata").unwrap();
            let mut load = loadExecutionTableCall::abi_decode(&raw[0]).unwrap();
            match mutation {
                "missing" => {
                    load._staticEntries.pop();
                }
                "extra" => load._staticEntries.push(load._staticEntries[0].clone()),
                "position" => {
                    load._staticEntries[0].expectedEntryIndex = alloy_primitives::U256::ONE
                }
                "result" => load._staticEntries[0].returnData = Bytes::from_static(&[0xff]),
                "split" => {}
                _ => unreachable!(),
            }
            forged["l2_calldata"] = json!([Bytes::from(load.abi_encode())]);
            let expected = if mutation == "split" {
                forged["l2_calldata"] = json!([
                    Bytes::from(load.abi_encode()),
                    Bytes::from(load.abi_encode())
                ]);
                "one complete source load"
            } else {
                "expected one L1 host"
            };
            let error = verify_input(&forged).unwrap_err().to_string();
            assert!(error.contains(expected), "{mutation}: {error}");
        }
    }

    fn mixed_fixture(repeated: bool) -> Value {
        serde_json::from_str(if repeated {
            include_str!("../tests/fixtures/events-causal/staticReadX2WriteL2.json")
        } else {
            include_str!("../tests/fixtures/events-causal/staticReadWriteL2.json")
        })
        .unwrap()
    }

    #[test]
    fn canonical_read_write_read_evidence_includes_position_bound_static_rows() {
        for repeated in [false, true] {
            let result = verify_input(&mixed_fixture(repeated)).unwrap();
            assert_eq!(result["verified"], true);
            assert_eq!(result["pairs"][0]["kind"], "mixed");
            assert_eq!(
                result["pairs"][0]["l2_static_entry_indices"]
                    .as_array()
                    .unwrap()
                    .len(),
                if repeated { 4 } else { 2 }
            );
        }
    }

    #[test]
    fn mixed_evidence_rejects_missing_extra_changed_and_mispositioned_static_rows() {
        for mutation in ["missing", "extra", "result", "position", "key", "success"] {
            let mut input = mixed_fixture(true);
            let raw = parse_bytes_list(&input, "l2_calldata").unwrap();
            let mut load = loadExecutionTableCall::abi_decode(&raw[0]).unwrap();
            match mutation {
                "missing" => {
                    load._staticEntries.remove(0);
                }
                "extra" => load._staticEntries.push(load._staticEntries[0].clone()),
                "result" => load._staticEntries[0].returnData = Bytes::from(vec![0xff]),
                "position" => {
                    load._staticEntries[0].expectedEntryIndex = alloy_primitives::U256::from(1)
                }
                "key" => load._staticEntries[0].proxyEntryHash = B256::ZERO,
                "success" => load._staticEntries[0].success = false,
                _ => unreachable!(),
            }
            input["l2_calldata"] = json!([format!("0x{}", hex::encode(load.abi_encode()))]);
            let error = verify_input(&input).unwrap_err().to_string();
            assert!(
                error.contains("expected one L1 host"),
                "{mutation}: {error}"
            );
        }
    }

    #[test]
    fn mixed_evidence_keeps_load_boundaries_and_exact_event_multiplicity() {
        for mutation in [
            "second_load",
            "unselected_mutable",
            "incoming_instead_of_load",
            "missing_events",
            "duplicate_events",
            "duplicate_host",
        ] {
            let mut input = mixed_fixture(false);
            let raw = parse_bytes_list(&input, "l2_calldata").unwrap();
            let mut load = loadExecutionTableCall::abi_decode(&raw[0]).unwrap();
            let expected_error = match mutation {
                "second_load" => {
                    input["l2_calldata"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!(format!(
                            "0x{}",
                            hex::encode(
                                loadExecutionTableCall {
                                    _entries: vec![],
                                    _staticEntries: vec![]
                                }
                                .abi_encode()
                            )
                        )));
                    "one complete source load"
                }
                "unselected_mutable" => {
                    load._entries.push(Default::default());
                    input["l2_calldata"] = json!([format!("0x{}", hex::encode(load.abi_encode()))]);
                    "one complete source load"
                }
                "incoming_instead_of_load" => {
                    let incoming = executeIncomingCrossChainCallCall {
                        _entries: load._entries,
                        _staticEntries: load._staticEntries,
                    };
                    input["l2_calldata"] =
                        json!([format!("0x{}", hex::encode(incoming.abi_encode()))]);
                    "one complete source load"
                }
                "missing_events" => {
                    input["l2_receipts"] = json!([]);
                    "bijection"
                }
                "duplicate_events" => {
                    let receipt = input["l2_receipts"][0].clone();
                    input["l2_receipts"].as_array_mut().unwrap().push(receipt);
                    "bijection"
                }
                "duplicate_host" => {
                    let result = verify_input(&input).unwrap();
                    let index =
                        result["pairs"][0]["l1_entry_indices"][0].as_u64().unwrap() as usize;
                    let mut posted = postAndVerifyBatchCall::abi_decode(
                        &parse_bytes(&input, "l1_calldata").unwrap(),
                    )
                    .unwrap();
                    posted
                        .batch
                        .entries
                        .push(posted.batch.entries[index].clone());
                    input["l1_calldata"] = json!(format!("0x{}", hex::encode(posted.abi_encode())));
                    "found 2"
                }
                _ => unreachable!(),
            };
            let error = verify_input(&input).unwrap_err().to_string();
            assert!(error.contains(expected_error), "{mutation}: {error}");
        }
    }

    #[test]
    fn static_selection_accepts_unrelated_entries_but_rejects_missing_and_duplicate_keys() {
        let selected = L2StaticExecutionEntrySol {
            proxyEntryHash: B256::repeat_byte(0x11),
            ..Default::default()
        };
        let unrelated = L2StaticExecutionEntrySol {
            proxyEntryHash: B256::repeat_byte(0x22),
            ..Default::default()
        };
        let entries = [unrelated, selected.clone()];
        let calldata = loadExecutionTableCall {
            _entries: vec![Default::default()],
            _staticEntries: entries.to_vec(),
        }
        .abi_encode();
        let (mutable, decoded_static) = decode_l2_tables(&calldata, RollupId::MAINNET).unwrap();
        assert_eq!(
            mutable.len(),
            1,
            "unrelated rows must not be stripped from calldata"
        );
        assert!(select_entries(&mutable, &[]).unwrap().is_empty());
        assert_eq!(decoded_static.len(), 2);
        assert_eq!(
            select_static_entry(&decoded_static, selected.proxyEntryHash)
                .unwrap()
                .0,
            1
        );
        assert_eq!(
            select_static_entry(&entries, selected.proxyEntryHash)
                .unwrap()
                .0,
            1
        );
        assert!(select_static_entry(&entries, B256::ZERO).is_err());
        assert!(
            select_static_entry(
                &[selected.clone(), selected.clone()],
                selected.proxyEntryHash
            )
            .is_err()
        );
    }

    #[test]
    fn multiset_preserves_duplicate_multiplicity() {
        let a = B256::repeat_byte(0xaa);
        assert!(exact_multiset("duplicate", &[a, a], &[a, a]).is_ok());
        assert!(exact_multiset("missing", &[a, a], &[a]).is_err());
        assert!(exact_multiset("extra", &[a], &[a, a]).is_err());
    }

    #[test]
    fn multiset_rejects_replacement_and_reordering_is_irrelevant() {
        let a = B256::repeat_byte(0xaa);
        let b = B256::repeat_byte(0xbb);
        let c = B256::repeat_byte(0xcc);
        assert!(exact_multiset("order", &[a, b], &[b, a]).is_ok());
        assert!(exact_multiset("replacement", &[a, b], &[a, c]).is_err());
    }

    #[test]
    fn entry_selection_preserves_duplicate_occurrences_across_deliveries() {
        let entry = ExecutionEntrySol {
            proxyEntryHash: B256::repeat_byte(0x11),
            rollingHash: B256::repeat_byte(0x22),
            ..Default::default()
        };
        let expected = runtime_entry_hash(&entry);

        assert!(select_entries(std::slice::from_ref(&entry), &[expected, expected]).is_err());
        let selected = select_entries(&[entry.clone(), entry], &[expected, expected]).unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].0, 0);
        assert_eq!(selected[1].0, 1);
    }

    #[test]
    fn l2_calldata_accepts_one_or_many_system_transactions() {
        let one = json!({"l2_calldata": "0x0102"});
        assert_eq!(parse_bytes_list(&one, "l2_calldata").unwrap().len(), 1);

        let many = json!({"l2_calldata": ["0x0102", "0x0304"]});
        assert_eq!(parse_bytes_list(&many, "l2_calldata").unwrap().len(), 2);

        let malformed = json!({"l2_calldata": ["0x0102", 7]});
        assert!(parse_bytes_list(&malformed, "l2_calldata").is_err());
    }
}
