//! Read-only JSON-RPC for canonical L1 settlement ↔ exact L2 block lookups.

use std::str::FromStr as _;
use std::sync::Arc;

use alloy_primitives::B256;
use eez_deriver::{BatchCorrelation, CorrelationStore};
use eez_l1::L1CanonicalHead;
use jsonrpsee::core::server::RpcModule;
use jsonrpsee::types::{ErrorObject, ErrorObjectOwned};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockSelector {
    Number(u64),
    Hash(B256),
}

type RpcState = (Arc<CorrelationStore>, Arc<L1CanonicalHead>);

/// Build both correlation directions for local dispatch on Composer fronts.
/// Do not install this module on regular node or follower transports.
///
/// # Errors
/// Returns an error if registering the RPC methods fails.
pub fn module(
    correlations: Arc<CorrelationStore>,
    l1_head: Arc<L1CanonicalHead>,
) -> eyre::Result<RpcModule<RpcState>> {
    let mut module = RpcModule::new((correlations, l1_head));

    module.register_method(
        "eez_getSettlementByL2Block",
        |params, (correlations, l1_head), _ext| {
            ensure_ready(correlations)?;
            let selector = parse_selector(params.one::<Value>().map_err(invalid_params)?)?;
            let found = match selector {
                BlockSelector::Number(number) => correlations.by_l2_number(number),
                BlockSelector::Hash(hash) => correlations.by_l2_hash(hash),
            }
            .map_err(internal_error)?;
            let Some(found) = found else {
                return Ok::<_, ErrorObjectOwned>(None);
            };
            let finalized = found.canonical_l2 && found.l2_block.number <= l1_head.finalized_l2();
            let mut value = batch_json(&found.batch, correlations, l1_head.finalized_l2())
                .map_err(internal_error)?;
            if let Some(object) = value.as_object_mut() {
                object.insert(
                    "matchedL2Block".into(),
                    json!({
                        "number": quantity(found.l2_block.number),
                        "hash": hash(found.l2_block.hash),
                    }),
                );
                object.insert("canonicalL2".into(), found.canonical_l2.into());
                object.insert("l2Finalized".into(), finalized.into());
            }
            Ok::<_, ErrorObjectOwned>(Some(value))
        },
    )?;

    module.register_method(
        "eez_getSettledL2RangesByL1Block",
        |params, (correlations, l1_head), _ext| {
            ensure_ready(correlations)?;
            let selector = parse_selector(params.one::<Value>().map_err(invalid_params)?)?;
            let records = match selector {
                BlockSelector::Number(number) => correlations.by_l1_number(number),
                BlockSelector::Hash(hash) => correlations.by_l1_hash(hash),
            }
            .map_err(internal_error)?;
            records
                .iter()
                .map(|record| batch_json(record, correlations, l1_head.finalized_l2()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(internal_error)
        },
    )?;

    Ok(module)
}

fn batch_json(
    record: &BatchCorrelation,
    correlations: &CorrelationStore,
    finalized_l2: u64,
) -> Result<Value, String> {
    let first = record.first_l2();
    let last = record.last_l2();
    let canonical_l2 = record.l2_blocks.iter().try_fold(true, |canonical, block| {
        let current = correlations.by_l2_number(block.number)?;
        Ok::<_, String>(
            canonical
                && current.is_some_and(|found| {
                    found.batch.l1_transaction_hash == record.l1_transaction_hash
                        && found.l2_block.hash == block.hash
                }),
        )
    })?;
    Ok(json!({
        "l1BlockNumber": quantity(record.l1_block_number),
        "l1BlockHash": hash(record.l1_block_hash),
        "l1TransactionHash": hash(record.l1_transaction_hash),
        "l1TransactionIndex": quantity(record.l1_transaction_index),
        "l2Range": {
            "firstBlockNumber": quantity(first.number),
            "firstBlockHash": hash(first.hash),
            "lastBlockNumber": quantity(last.number),
            "lastBlockHash": hash(last.hash),
            "blockCount": quantity(record.l2_blocks.len() as u64),
        },
        "l2Blocks": record.l2_blocks.iter().map(|block| json!({
            "number": quantity(block.number),
            "hash": hash(block.hash),
        })).collect::<Vec<_>>(),
        "canonicalL2": canonical_l2,
        "l2Finalized": canonical_l2 && last.number <= finalized_l2,
    }))
}

fn parse_selector(value: Value) -> Result<BlockSelector, ErrorObjectOwned> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .map(BlockSelector::Number)
            .ok_or_else(|| invalid_selector("block number must fit u64")),
        Value::String(raw) => parse_selector_string(&raw),
        Value::Object(mut object) if object.len() == 1 => {
            if let Some(value) = object.remove("blockNumber") {
                return match value {
                    Value::Number(number) => number
                        .as_u64()
                        .map(BlockSelector::Number)
                        .ok_or_else(|| invalid_selector("blockNumber must fit u64")),
                    Value::String(raw) => parse_number(&raw).map(BlockSelector::Number),
                    _ => Err(invalid_selector("blockNumber must be a quantity")),
                };
            }
            if let Some(value) = object.remove("blockHash") {
                return match value {
                    Value::String(raw) => B256::from_str(&raw)
                        .map(BlockSelector::Hash)
                        .map_err(|error| invalid_selector(&format!("invalid blockHash: {error}"))),
                    _ => Err(invalid_selector("blockHash must be a 32-byte hex string")),
                };
            }
            Err(invalid_selector(
                "selector object must contain blockNumber or blockHash",
            ))
        }
        _ => Err(invalid_selector(
            "expected a block quantity/hash or {blockNumber|blockHash}",
        )),
    }
}

fn parse_selector_string(raw: &str) -> Result<BlockSelector, ErrorObjectOwned> {
    if raw.len() == 66 && raw.starts_with("0x") {
        B256::from_str(raw)
            .map(BlockSelector::Hash)
            .map_err(|error| invalid_selector(&format!("invalid block hash: {error}")))
    } else {
        parse_number(raw).map(BlockSelector::Number)
    }
}

fn parse_number(raw: &str) -> Result<u64, ErrorObjectOwned> {
    if let Some(hex) = raw.strip_prefix("0x") {
        if hex.is_empty() {
            return Err(invalid_selector("empty hexadecimal block number"));
        }
        u64::from_str_radix(hex, 16)
            .map_err(|error| invalid_selector(&format!("invalid block number: {error}")))
    } else {
        raw.parse::<u64>()
            .map_err(|error| invalid_selector(&format!("invalid block number: {error}")))
    }
}

fn invalid_params(error: impl std::fmt::Display) -> ErrorObjectOwned {
    ErrorObject::owned(
        -32602,
        format!("invalid correlation RPC params: {error}"),
        None::<()>,
    )
}

fn invalid_selector(message: &str) -> ErrorObjectOwned {
    ErrorObject::owned(-32602, message.to_owned(), None::<()>)
}

fn internal_error(error: impl std::fmt::Display) -> ErrorObjectOwned {
    ErrorObject::owned(
        -32000,
        format!("correlation index error: {error}"),
        None::<()>,
    )
}

fn ensure_ready(correlations: &CorrelationStore) -> Result<(), ErrorObjectOwned> {
    if correlations.is_ready() {
        Ok(())
    } else {
        Err(ErrorObject::owned(
            -32001,
            "correlation index is still validating/catching up".to_string(),
            None::<()>,
        ))
    }
}

fn quantity(number: u64) -> String {
    format!("0x{number:x}")
}

fn hash(value: B256) -> String {
    format!("{value:#x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use eez_deriver::L2BlockCorrelation;

    #[test]
    fn selector_accepts_ethereum_quantities_hashes_and_objects() {
        assert_eq!(
            parse_selector(json!("0xcc")).unwrap(),
            BlockSelector::Number(204)
        );
        let block_hash = B256::repeat_byte(0xaa);
        assert_eq!(
            parse_selector(json!(format!("{block_hash:#x}"))).unwrap(),
            BlockSelector::Hash(block_hash)
        );
        assert_eq!(
            parse_selector(json!({ "blockNumber": "204" })).unwrap(),
            BlockSelector::Number(204)
        );
        assert_eq!(
            parse_selector(json!({ "blockHash": format!("{block_hash:#x}") })).unwrap(),
            BlockSelector::Hash(block_hash)
        );
    }

    #[test]
    fn selector_rejects_ambiguous_or_malformed_values() {
        assert!(parse_selector(json!({ "blockNumber": "1", "blockHash": "0x00" })).is_err());
        assert!(parse_selector(json!("0x")).is_err());
        assert!(parse_selector(json!(-1)).is_err());
    }

    fn record() -> BatchCorrelation {
        let endpoint_header = alloy_consensus::Header {
            number: 200,
            ..Default::default()
        };
        BatchCorrelation {
            l1_block_number: 35,
            l1_block_hash: B256::repeat_byte(0x35),
            l1_transaction_hash: B256::repeat_byte(0xaa),
            l1_transaction_index: 2,
            l2_blocks: vec![
                L2BlockCorrelation {
                    number: 199,
                    hash: B256::repeat_byte(0x19),
                },
                L2BlockCorrelation {
                    number: 200,
                    hash: endpoint_header.hash_slow(),
                },
            ],
            endpoint_header,
            endpoint_history: eez_protocol::l1_history::HistoryState {
                imported: true,
                endpoint: 34,
                endpoint_hash: B256::repeat_byte(0x34),
            },
        }
    }

    #[tokio::test]
    async fn rpc_fails_closed_until_boot_validation_finishes() {
        let rpc = module(
            Arc::new(CorrelationStore::in_memory()),
            Arc::new(L1CanonicalHead::default()),
        )
        .unwrap();
        let (response, _) = rpc
            .raw_json_request(
                r#"{"jsonrpc":"2.0","id":1,"method":"eez_getSettlementByL2Block","params":["0x1"]}"#,
                1,
            )
            .await
            .unwrap();
        let response: Value = serde_json::from_str(response.get()).unwrap();
        assert_eq!(response["error"]["code"], -32001);
    }

    #[tokio::test]
    async fn rpc_resolves_both_directions_and_unknowns() {
        let correlations = Arc::new(CorrelationStore::in_memory());
        correlations.put(&record()).unwrap();
        correlations.mark_ready();
        let l1_head = Arc::new(L1CanonicalHead::default());
        l1_head.set_finalized_l2(200);
        let rpc = module(correlations, l1_head).unwrap();

        let (response, _) = rpc
            .raw_json_request(
                r#"{"jsonrpc":"2.0","id":1,"method":"eez_getSettlementByL2Block","params":["0xc8"]}"#,
                1,
            )
            .await
            .unwrap();
        let response: Value = serde_json::from_str(response.get()).unwrap();
        assert_eq!(response["result"]["l1BlockNumber"], "0x23");
        assert_eq!(response["result"]["matchedL2Block"]["number"], "0xc8");
        assert_eq!(response["result"]["canonicalL2"], true);
        assert_eq!(response["result"]["l2Finalized"], true);

        let (response, _) = rpc
            .raw_json_request(
                r#"{"jsonrpc":"2.0","id":2,"method":"eez_getSettledL2RangesByL1Block","params":["0x23"]}"#,
                1,
            )
            .await
            .unwrap();
        let response: Value = serde_json::from_str(response.get()).unwrap();
        assert_eq!(response["result"].as_array().unwrap().len(), 1);
        assert_eq!(response["result"][0]["l2Range"]["firstBlockNumber"], "0xc7");
        assert_eq!(response["result"][0]["l2Range"]["lastBlockNumber"], "0xc8");

        let (response, _) = rpc
            .raw_json_request(
                r#"{"jsonrpc":"2.0","id":3,"method":"eez_getSettlementByL2Block","params":["0xffff"]}"#,
                1,
            )
            .await
            .unwrap();
        let response: Value = serde_json::from_str(response.get()).unwrap();
        assert!(response["result"].is_null());
    }
}
