//! Independent causal verifier for real-network E2E evidence.
//!
//! Input is a JSON document assembled from canonical transaction calldata and
//! receipts. The verifier selects the scenario's actual L2 entries by their
//! runtime entry hashes, pairs them with the actual posted L1 entries, invokes
//! the protocol's independent two-table verifier, and checks the retained
//! source/destination events as exact multisets.

use std::collections::HashMap;
use std::env;
use std::error::Error;
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
    let direction = parse_direction(&input)?;
    let rollup_id = input.get("rollup_id").and_then(Value::as_u64).unwrap_or(1);
    let l2_rollup = RollupId(rollup_id);
    if l2_rollup.is_mainnet() {
        return fail("rollup_id must be non-zero");
    }
    let registry = Address::from_str(required_str(&input, "registry")?)?;
    let manager_l2 = Address::from_str(required_str(&input, "manager_l2")?)?;
    let l1_calldata = parse_bytes(&input, "l1_calldata")?;
    let l2_calldata = parse_bytes_list(&input, "l2_calldata")?;
    let select_static = input
        .get("select_static")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let posted_call = postAndVerifyBatchCall::abi_decode(&l1_calldata)?;
    if posted_call.abi_encode() != l1_calldata {
        return fail("postAndVerifyBatch calldata is not canonical ABI");
    }
    let posted = posted_call.batch;
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
    let expected_l2 = expected_hashes(&input)?;
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
            let mut next_l1 = 0_usize;
            let mut paired_l1 = Vec::new();
            for (l2_index, l2_entry) in &selected {
                let mut match_index = None;
                for l1_index in next_l1..posted.entries.len() {
                    let l1_entry = &posted.entries[l1_index];
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
                let l1_entry = &posted.entries[l1_index];
                if l1_entry.success {
                    paired_l1.push(l1_entry.proxyEntryHash);
                }
                expected_outbound_hashes.extend(facts.l2_outbound_hashes.iter().copied());
                pairs.push(json!({
                    "kind": "mutable",
                    "direction": "L1_to_L2",
                    "l1_entry_indices": [l1_index],
                    "l2_entry_indices": [l2_index],
                    "proxy_entry_hash": format!("{}", l1_entry.proxyEntryHash),
                    "ether_delta": facts.ether_delta.to_string(),
                    "l2_outbound_hashes": facts.l2_outbound_hashes.iter()
                        .map(ToString::to_string).collect::<Vec<_>>()
                }));
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
            if selected.is_empty() {
                return fail("outbound causal verification selected no L2 entries");
            }
            let selected_entries = selected
                .iter()
                .map(|(_, entry)| entry.clone())
                .collect::<Vec<_>>();
            let mut candidates = Vec::new();
            for (l1_index, l1_entry) in posted.entries.iter().enumerate() {
                if let Ok(facts) = verify_nested_group(
                    l1_entry,
                    &selected_entries,
                    NestedPairDirection::Outbound,
                    l2_rollup,
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
                "kind": "mutable",
                "direction": "L2_to_L1",
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
            let key = B256::from_str(required_str(&input, "expected_l2_static_key")?)?;
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
        "posted_l1_entries": posted.entries.len(),
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
    fs::write(output_path, serde_json::to_vec_pretty(&output)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
