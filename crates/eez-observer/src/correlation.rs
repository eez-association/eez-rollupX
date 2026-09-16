//! Exact node-reported settlement ranges. Parsing is not canonical verification:
//! persistence must additionally check receipt, L1 anchor and every applied L2 hash.
use alloy_primitives::B256;
use serde_json::Value;
use std::collections::HashSet;

use crate::{accounting::hash, model::quantity, rpc::Failure};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlockRef {
    pub number: u64,
    pub hash: B256,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Settlement {
    pub l1_block: BlockRef,
    pub transaction_hash: B256,
    pub transaction_index: u64,
    pub l2_blocks: Vec<BlockRef>,
    /// Diagnostic source claim only; the observer checks exact canonical hashes.
    pub reported_canonical_l2: bool,
}

impl Settlement {
    fn parse(value: &Value, expected: BlockRef) -> Result<Self, Failure> {
        let l1_block = BlockRef {
            number: quantity(&value["l1BlockNumber"])?,
            hash: hash(&value["l1BlockHash"])?,
        };
        if l1_block != expected {
            return Err(Failure::Malformed);
        }
        let blocks = value["l2Blocks"].as_array().ok_or(Failure::Malformed)?;
        if blocks.is_empty() || blocks.len() > 4096 {
            return Err(Failure::Malformed);
        }
        let mut l2_blocks: Vec<BlockRef> = Vec::with_capacity(blocks.len());
        let mut hashes = HashSet::new();
        for block in blocks {
            let block = BlockRef {
                number: quantity(&block["number"])?,
                hash: hash(&block["hash"])?,
            };
            if !hashes.insert(block.hash)
                || l2_blocks
                    .last()
                    .is_some_and(|previous| previous.number.checked_add(1) != Some(block.number))
            {
                return Err(Failure::Malformed);
            }
            l2_blocks.push(block);
        }
        let first = l2_blocks.first().ok_or(Failure::Malformed)?;
        let last = l2_blocks.last().ok_or(Failure::Malformed)?;
        let range = &value["l2Range"];
        if quantity(&range["blockCount"])? != l2_blocks.len() as u64
            || quantity(&range["firstBlockNumber"])? != first.number
            || hash(&range["firstBlockHash"])? != first.hash
            || quantity(&range["lastBlockNumber"])? != last.number
            || hash(&range["lastBlockHash"])? != last.hash
        {
            return Err(Failure::Malformed);
        }
        // l2Finalized is intentionally ignored: a height-derived node flag is
        // not the required finalized L1 receipt anchor plus exact L2 membership.
        Ok(Self {
            l1_block,
            transaction_hash: hash(&value["l1TransactionHash"])?,
            transaction_index: quantity(&value["l1TransactionIndex"])?,
            l2_blocks,
            reported_canonical_l2: value["canonicalL2"].as_bool().ok_or(Failure::Malformed)?,
        })
    }
}

pub(crate) fn parse(value: &Value, expected: BlockRef) -> Result<Vec<Settlement>, Failure> {
    let values = value.as_array().ok_or(Failure::Malformed)?;
    if values.len() > 1024 {
        return Err(Failure::Oversized);
    }
    let mut hashes = HashSet::new();
    let mut indices = HashSet::new();
    let mut result = Vec::with_capacity(values.len());
    let mut total_blocks = 0usize;
    for value in values {
        let settlement = Settlement::parse(value, expected)?;
        total_blocks = total_blocks
            .checked_add(settlement.l2_blocks.len())
            .ok_or(Failure::Oversized)?;
        if total_blocks > 4096 {
            return Err(Failure::Oversized);
        }
        if !hashes.insert(settlement.transaction_hash)
            || !indices.insert(settlement.transaction_index)
        {
            return Err(Failure::Malformed);
        }
        result.push(settlement);
    }
    result.sort_by_key(|settlement| settlement.transaction_index);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (Value, BlockRef) {
        let l1 = BlockRef {
            number: 7,
            hash: B256::repeat_byte(1),
        };
        let blocks = vec![
            json!({"number":"0xa","hash":format!("{:#x}",B256::repeat_byte(2))}),
            json!({"number":"0xb","hash":format!("{:#x}",B256::repeat_byte(3))}),
        ];
        let value = json!({"l1BlockNumber":"0x7","l1BlockHash":format!("{:#x}",l1.hash),"l1TransactionHash":format!("{:#x}",B256::repeat_byte(4)),"l1TransactionIndex":"0x1",
            "l2Blocks":blocks,"l2Range":{"firstBlockNumber":"0xa","lastBlockNumber":"0xb","firstBlockHash":blocks[0]["hash"],"lastBlockHash":blocks[1]["hash"],"blockCount":"0x2"},
            "canonicalL2":true,"l2Finalized":true});
        (value, l1)
    }

    #[test]
    fn exact_range_membership_and_l1_identity_are_required() {
        let (value, l1) = fixture();
        let parsed = parse(&json!([value]), l1).unwrap();
        assert_eq!(parsed[0].l2_blocks.len(), 2);
        for (pointer, replacement) in [
            ("/l1BlockNumber", json!("0x8")),
            ("/l1BlockHash", json!(format!("{:#x}", B256::ZERO))),
            ("/l2Range/blockCount", json!("0x3")),
            (
                "/l2Range/lastBlockHash",
                json!(format!("{:#x}", B256::ZERO)),
            ),
            ("/l2Blocks/1/number", json!("0xc")),
            ("/l2Blocks/1/hash", value["l2Blocks"][0]["hash"].clone()),
            ("/canonicalL2", Value::Null),
        ] {
            let mut changed = value.clone();
            *changed.pointer_mut(pointer).unwrap() = replacement;
            assert!(parse(&json!([changed]), l1).is_err());
        }
        assert!(parse(&json!([value, value]), l1).is_err());
    }

    #[test]
    fn order_is_deterministic_and_node_finality_is_not_accepted_as_evidence() {
        let (mut first, l1) = fixture();
        let mut second = first.clone();
        second["l1TransactionIndex"] = json!("0x2");
        second["l1TransactionHash"] = json!(format!("{:#x}", B256::repeat_byte(5)));
        let original = parse(&json!([second, first]), l1).unwrap();
        assert_eq!(original[0].transaction_index, 1);
        first["l2Finalized"] = json!(false);
        assert_eq!(Settlement::parse(&first, l1).unwrap(), original[0]);
        assert!(parse(&json!([]), l1).unwrap().is_empty());
        assert!(parse(&Value::Null, l1).is_err());
    }
}
