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

#[cfg(test)]
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

#[cfg(test)]
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

/// Network format ceiling; page responses retain the ordinary 1 MiB RPC limit.
pub(crate) const MAX_RANGE_BLOCKS: usize = 65_536;
pub(crate) const PAGE_BLOCKS: usize = 1024;

/// Assemble one exact receipt's range. Nothing is persisted before the last page
/// and independently recomputed digest agree; a digest is continuity, not trust.
pub(crate) struct RangePages {
    claim: Settlement,
    binding: [Value; 5],
    first: BlockRef,
    last: BlockRef,
    count: usize,
    digest: B256,
    hashes: HashSet<B256>,
}

impl RangePages {
    pub(crate) fn new(value: &Value, anchor: BlockRef, transaction: B256) -> Result<Self, Failure> {
        let l1_block = BlockRef {
            number: quantity(&value["l1BlockNumber"])?,
            hash: hash(&value["l1BlockHash"])?,
        };
        if l1_block != anchor || hash(&value["l1TransactionHash"])? != transaction {
            return Err(Failure::Malformed);
        }
        let range = &value["l2Range"];
        let count =
            usize::try_from(quantity(&range["blockCount"])?).map_err(|_| Failure::Oversized)?;
        if !(1..=MAX_RANGE_BLOCKS).contains(&count) {
            return Err(Failure::Oversized);
        }
        let first = BlockRef {
            number: quantity(&range["firstBlockNumber"])?,
            hash: hash(&range["firstBlockHash"])?,
        };
        let last = BlockRef {
            number: quantity(&range["lastBlockNumber"])?,
            hash: hash(&range["lastBlockHash"])?,
        };
        if first.number.checked_add(count as u64 - 1) != Some(last.number) {
            return Err(Failure::Malformed);
        }
        let mut pages = Self {
            claim: Settlement {
                l1_block,
                transaction_hash: transaction,
                transaction_index: quantity(&value["l1TransactionIndex"])?,
                l2_blocks: Vec::with_capacity(count),
                reported_canonical_l2: true,
            },
            binding: Self::binding(value),
            first,
            last,
            count,
            digest: hash(&value["evidenceDigest"])?,
            hashes: HashSet::with_capacity(count),
        };
        pages.append(value)?;
        Ok(pages)
    }

    fn binding(value: &Value) -> [Value; 5] {
        [
            "l1BlockNumber",
            "l1BlockHash",
            "l1TransactionHash",
            "l1TransactionIndex",
            "l2Range",
        ]
        .map(|key| value[key].clone())
    }

    pub(crate) fn next_offset(&self) -> Option<usize> {
        (self.claim.l2_blocks.len() < self.count).then_some(self.claim.l2_blocks.len())
    }

    pub(crate) fn append(&mut self, value: &Value) -> Result<(), Failure> {
        let offset = self.claim.l2_blocks.len();
        if Self::binding(value) != self.binding
            || hash(&value["evidenceDigest"])? != self.digest
            || value["offset"].as_u64() != Some(offset as u64)
            || offset >= self.count
        {
            return Err(Failure::Malformed);
        }
        let blocks = value["l2Blocks"].as_array().ok_or(Failure::Malformed)?;
        let size = PAGE_BLOCKS.min(self.count - offset);
        if blocks.len() != size {
            return Err(Failure::Malformed);
        }
        let end = offset + size;
        let expected_next = (end < self.count).then_some(end as u64);
        if !value
            .get("nextOffset")
            .is_some_and(|next| match expected_next {
                Some(expected) => next.as_u64() == Some(expected),
                None => next.is_null(),
            })
        {
            return Err(Failure::Malformed);
        }
        self.claim.reported_canonical_l2 &=
            value["canonicalL2"].as_bool().ok_or(Failure::Malformed)?;
        for (index, block) in blocks.iter().enumerate() {
            let block = BlockRef {
                number: quantity(&block["number"])?,
                hash: hash(&block["hash"])?,
            };
            if self.first.number.checked_add((offset + index) as u64) != Some(block.number)
                || !self.hashes.insert(block.hash)
            {
                return Err(Failure::Malformed);
            }
            self.claim.l2_blocks.push(block);
        }
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<Settlement, Failure> {
        if self.claim.l2_blocks.len() != self.count
            || self.claim.l2_blocks.first() != Some(&self.first)
            || self.claim.l2_blocks.last() != Some(&self.last)
            || range_digest(&self.claim) != self.digest
        {
            return Err(Failure::Malformed);
        }
        Ok(self.claim)
    }
}

fn range_digest(claim: &Settlement) -> B256 {
    let mut digest = alloy_primitives::Keccak256::new();
    digest.update(b"EEZ_CORRELATION_PAGE_V1");
    digest.update(claim.l1_block.number.to_be_bytes());
    digest.update(claim.l1_block.hash);
    digest.update(claim.transaction_hash);
    digest.update(claim.transaction_index.to_be_bytes());
    digest.update((claim.l2_blocks.len() as u64).to_be_bytes());
    for block in &claim.l2_blocks {
        digest.update(block.number.to_be_bytes());
        digest.update(block.hash);
    }
    digest.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn page_digest_matches_independent_foundry_fixture_vector() {
        let claim = Settlement {
            l1_block: BlockRef {
                number: 2,
                hash: B256::from(alloy_primitives::U256::from(102).to_be_bytes::<32>()),
            },
            transaction_hash: B256::from(alloy_primitives::U256::from(1002).to_be_bytes::<32>()),
            transaction_index: 0,
            l2_blocks: vec![
                BlockRef {
                    number: 1,
                    hash: B256::from(alloy_primitives::U256::from(201).to_be_bytes::<32>()),
                },
                BlockRef {
                    number: 2,
                    hash: B256::from(alloy_primitives::U256::from(202).to_be_bytes::<32>()),
                },
            ],
            reported_canonical_l2: true,
        };
        assert_eq!(
            range_digest(&claim),
            "0x82dc7dff05f410b005c1b2af8d7316f331673dd2897377a4d327a26a36c560eb"
                .parse::<B256>()
                .unwrap()
        );
    }

    fn long_claim(count: usize) -> Settlement {
        Settlement {
            l1_block: BlockRef {
                number: 7,
                hash: B256::repeat_byte(1),
            },
            transaction_hash: B256::repeat_byte(4),
            transaction_index: 1,
            l2_blocks: (0..count)
                .map(|index| BlockRef {
                    number: index as u64 + 10,
                    hash: B256::from(alloy_primitives::U256::from(index + 100).to_be_bytes::<32>()),
                })
                .collect(),
            reported_canonical_l2: true,
        }
    }
    fn page(claim: &Settlement, offset: usize) -> Value {
        let end = (offset + PAGE_BLOCKS).min(claim.l2_blocks.len());
        let first = claim.l2_blocks.first().unwrap();
        let last = claim.l2_blocks.last().unwrap();
        json!({"l1BlockNumber":format!("0x{:x}",claim.l1_block.number),"l1BlockHash":format!("{:#x}",claim.l1_block.hash),
            "l1TransactionHash":format!("{:#x}",claim.transaction_hash),"l1TransactionIndex":format!("0x{:x}",claim.transaction_index),
            "l2Range":{"blockCount":format!("0x{:x}",claim.l2_blocks.len()),"firstBlockNumber":format!("0x{:x}",first.number),
                "firstBlockHash":format!("{:#x}",first.hash),"lastBlockNumber":format!("0x{:x}",last.number),"lastBlockHash":format!("{:#x}",last.hash)},
            "l2Blocks":claim.l2_blocks[offset..end].iter().map(|b|json!({"number":format!("0x{:x}",b.number),"hash":format!("{:#x}",b.hash)})).collect::<Vec<_>>(),
            "canonicalL2":true,"l2Finalized":true,"offset":offset,"nextOffset":(end<claim.l2_blocks.len()).then_some(end),
            "evidenceDigest":format!("{:#x}",range_digest(claim))})
    }

    #[test]
    fn paged_range_supports_exact_network_ceiling_and_rejects_larger_claims() {
        let claim = long_claim(MAX_RANGE_BLOCKS);
        let first = page(&claim, 0);
        let mut assembled =
            RangePages::new(&first, claim.l1_block, claim.transaction_hash).unwrap();
        while let Some(offset) = assembled.next_offset() {
            assembled.append(&page(&claim, offset)).unwrap();
        }
        assert_eq!(assembled.finish().unwrap(), claim);
        for count in [0, MAX_RANGE_BLOCKS + 1] {
            let mut invalid = first.clone();
            invalid["l2Range"]["blockCount"] = json!(format!("0x{count:x}"));
            assert!(matches!(
                RangePages::new(&invalid, claim.l1_block, claim.transaction_hash),
                Err(Failure::Oversized)
            ));
        }
    }

    #[test]
    fn partial_changed_duplicate_or_reordered_pages_never_form_a_settlement() {
        let claim = long_claim(PAGE_BLOCKS + 1);
        let first = page(&claim, 0);
        assert!(
            RangePages::new(&first, claim.l1_block, claim.transaction_hash)
                .unwrap()
                .finish()
                .is_err()
        );
        for (pointer, replacement) in [
            ("/offset", json!(0)),
            ("/nextOffset", json!(PAGE_BLOCKS + 1)),
            ("/l1TransactionIndex", json!("0x2")),
            ("/l1BlockHash", json!(format!("{:#x}", B256::ZERO))),
            ("/evidenceDigest", json!(format!("{:#x}", B256::ZERO))),
            ("/l2Range/blockCount", json!("0x402")),
            ("/l2Blocks/0/number", json!("0x999")),
            ("/l2Blocks/0/hash", first["l2Blocks"][0]["hash"].clone()),
        ] {
            let mut assembled =
                RangePages::new(&first, claim.l1_block, claim.transaction_hash).unwrap();
            let mut last = page(&claim, PAGE_BLOCKS);
            *last.pointer_mut(pointer).unwrap() = replacement;
            assert!(assembled.append(&last).is_err(), "{pointer}");
        }
        let mut shortened = first.clone();
        shortened["l2Blocks"].as_array_mut().unwrap().pop();
        assert!(RangePages::new(&shortened, claim.l1_block, claim.transaction_hash).is_err());
        let mut overflow = first.clone();
        overflow["l2Range"]["firstBlockNumber"] = json!("0xffffffffffffffff");
        assert!(RangePages::new(&overflow, claim.l1_block, claim.transaction_hash).is_err());
    }

    #[test]
    fn page_digest_is_recomputed_and_mutable_flags_remain_only_diagnostics() {
        let claim = long_claim(PAGE_BLOCKS + 1);
        let mut first = page(&claim, 0);
        let mut last = page(&claim, PAGE_BLOCKS);
        last["canonicalL2"] = json!(false);
        last["l2Finalized"] = json!(false);
        let mut assembled =
            RangePages::new(&first, claim.l1_block, claim.transaction_hash).unwrap();
        assembled.append(&last).unwrap();
        let mut expected = claim.clone();
        expected.reported_canonical_l2 = false;
        assert_eq!(assembled.finish().unwrap(), expected);
        first["evidenceDigest"] = json!(format!("{:#x}", B256::ZERO));
        last["evidenceDigest"] = first["evidenceDigest"].clone();
        let mut assembled =
            RangePages::new(&first, claim.l1_block, claim.transaction_hash).unwrap();
        assembled.append(&last).unwrap();
        assert!(assembled.finish().is_err());
    }

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
