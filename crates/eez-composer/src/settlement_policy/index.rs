//! Canonical pending-block accounting. Only newly observed blocks are loaded
//! and measured in steady state; a hash change rewinds to the common ancestor.

use std::collections::BTreeMap;

use alloy_primitives::B256;
use eez_l1::L2Endpoint;
use eez_payload_codec::{DerivablePayloadSizer, derivable::Environment};

use super::PolicyError;

/// One measured block. No receipts are needed to count included transactions.
#[derive(Debug, Clone)]
pub(crate) struct BlockActivity {
    pub endpoint: L2Endpoint,
    pub parent_hash: B256,
    /// The full block's RLP byte-string length when promoted to terminal.
    pub encoded_item_bytes: u64,
    /// This position represented as ordinary, before terminal promotion.
    pub ordinary: DerivablePayloadSizer,
    pub pure_transactions: u64,
}

#[derive(Debug, Clone)]
struct IndexedBlock {
    block: BlockActivity,
    prefix_pure_transactions: u64,
    latest_pure: Option<L2Endpoint>,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DaOverhead {
    pub entry_items: usize,
    pub group_items: usize,
    pub semantic_bytes: usize,
}

/// Not persisted. The cache's format identity includes both existing codecs.
#[derive(Debug, Default)]
pub(crate) struct ActivityIndex {
    codec: (u8, u8),
    anchor: Option<L2Endpoint>,
    anchor_prefix_pure_transactions: u64,
    blocks: BTreeMap<u64, IndexedBlock>,
    // Aligned power-of-two ordinary summaries, through level 15. Larger ranges
    // compose several entries so no cached summary exceeds the format ceiling.
    summaries: BTreeMap<(u32, u64), DerivablePayloadSizer>,
}

impl ActivityIndex {
    /// Update through the supplied parent, including reth's just-built pending
    /// parent. Hash lookups avoid a race with provider height-index promotion.
    /// Returns whether previously cached blocks may have been invalidated.
    /// A steady append does not require walking the separate immutable RLP cache.
    pub fn update(
        &mut self,
        anchor: L2Endpoint,
        parent: L2Endpoint,
        mut load: impl FnMut(B256) -> Result<BlockActivity, PolicyError>,
    ) -> Result<bool, PolicyError> {
        let codec = (
            eez_payload_codec::derivable::TAG,
            eez_blob_codec::PROTOCOL_VERSION,
        );
        let mut invalidated = self.codec != codec || self.anchor != Some(anchor);
        if self.codec != codec {
            *self = Self {
                codec,
                ..Self::default()
            };
        }
        if parent.number < anchor.number {
            return Err(PolicyError::Unavailable(
                "L2 parent precedes the settlement anchor".into(),
            ));
        }
        if self.anchor != Some(anchor) {
            if let Some(block) = self.blocks.get(&anchor.number).cloned()
                && block.block.endpoint == anchor
                && self.anchor.is_some_and(|old| old.number <= anchor.number)
            {
                self.anchor_prefix_pure_transactions = block.prefix_pure_transactions;
                self.blocks = self.blocks.split_off(&anchor.number);
                self.blocks.remove(&anchor.number);
            } else {
                self.blocks.clear();
                self.summaries.clear();
                self.anchor_prefix_pure_transactions = 0;
            }
            self.summaries
                .retain(|(_, start), _| *start > anchor.number);
            self.anchor = Some(anchor);
        }
        let mut cursor = parent;
        let mut pending = Vec::new();
        while cursor.number > anchor.number {
            if self
                .blocks
                .get(&cursor.number)
                .is_some_and(|cached| cached.block.endpoint == cursor)
            {
                break;
            }
            let block = load(cursor.hash)?;
            if block.endpoint != cursor {
                return Err(PolicyError::Unavailable(
                    "block lookup returned a different identity".into(),
                ));
            }
            cursor = L2Endpoint {
                number: cursor.number - 1,
                hash: block.parent_hash,
            };
            pending.push(block);
        }
        if cursor.number == anchor.number && cursor.hash != anchor.hash {
            return Err(PolicyError::Unavailable(
                "pending L2 ancestry does not match the validated anchor".into(),
            ));
        }
        let removed = cursor
            .number
            .checked_add(1)
            .is_some_and(|after| !self.blocks.split_off(&after).is_empty());
        invalidated |= removed;
        if removed {
            self.summaries.retain(|(level, start), _| {
                start
                    .checked_add((1u64 << level) - 1)
                    .is_some_and(|end| end <= cursor.number)
            });
        }
        for block in pending.into_iter().rev() {
            let previous = self.blocks.last_key_value().map(|(_, block)| block);
            let prefix_pure_transactions = previous
                .map_or(self.anchor_prefix_pure_transactions, |block| {
                    block.prefix_pure_transactions
                })
                .checked_add(block.pure_transactions)
                .ok_or(PolicyError::Arithmetic("pending pure transaction count"))?;
            let latest_pure = if block.pure_transactions > 0 {
                Some(block.endpoint)
            } else {
                previous.and_then(|block| block.latest_pure)
            };
            self.insert_summary(block.endpoint.number, block.ordinary.clone())?;
            self.blocks.insert(
                block.endpoint.number,
                IndexedBlock {
                    block,
                    prefix_pure_transactions,
                    latest_pure,
                },
            );
        }
        Ok(invalidated)
    }

    fn insert_summary(
        &mut self,
        number: u64,
        summary: DerivablePayloadSizer,
    ) -> Result<(), PolicyError> {
        self.summaries.insert((0, number), summary);
        for level in 1..=15 {
            let width = 1u64 << level;
            let start = number & !(width - 1);
            let middle = start + width / 2;
            let (Some(left), Some(right)) = (
                self.summaries.get(&(level - 1, start)),
                self.summaries.get(&(level - 1, middle)),
            ) else {
                break;
            };
            let combined = left
                .combine(right)
                .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
            self.summaries.insert((level, start), combined);
        }
        Ok(())
    }

    /// Compose [anchor + 1, end), selecting the initial environment anew. A byte
    /// prefix subtraction cannot account for gaps or environment transitions.
    fn ordinary_range(&self, end: u64) -> Result<DerivablePayloadSizer, PolicyError> {
        let Some((&first, _)) = self.blocks.first_key_value() else {
            return Ok(DerivablePayloadSizer::default());
        };
        let mut cursor = first;
        let mut summary = DerivablePayloadSizer::default();
        while cursor < end {
            let remaining = end - cursor;
            let level = cursor.trailing_zeros().min(remaining.ilog2()).min(15);
            let part = self.summaries.get(&(level, cursor)).ok_or_else(|| {
                PolicyError::Unavailable("missing canonical ordinary summary".into())
            })?;
            summary = summary
                .combine(part)
                .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
            cursor = cursor
                .checked_add(1u64 << level)
                .ok_or(PolicyError::Arithmetic("ordinary range cursor"))?;
        }
        Ok(summary)
    }

    pub fn latest_pure(&self) -> Option<L2Endpoint> {
        self.blocks
            .last_key_value()
            .and_then(|(_, block)| block.latest_pure)
            .filter(|block| {
                self.anchor
                    .is_some_and(|anchor| block.number > anchor.number)
            })
    }

    pub fn pending_pure_transactions(&self) -> Result<u64, PolicyError> {
        self.blocks
            .last_key_value()
            .map_or(self.anchor_prefix_pure_transactions, |(_, block)| {
                block.prefix_pure_transactions
            })
            .checked_sub(self.anchor_prefix_pure_transactions)
            .ok_or(PolicyError::Arithmetic(
                "pending pure transaction prefix subtraction",
            ))
    }

    pub fn endpoint_at(&self, number: u64) -> Option<L2Endpoint> {
        self.blocks
            .get(&number)
            .map(|block| block.block.endpoint)
            .or_else(|| self.anchor.filter(|anchor| anchor.number == number))
    }

    /// Exact size for an anchor-only historical prefix. Its terminal is already
    /// indexed and it has no endpoint-dependent derivation or semantic entries.
    pub fn historical_stream_bytes(&self, endpoint: L2Endpoint) -> Result<u64, PolicyError> {
        let block = self
            .blocks
            .get(&endpoint.number)
            .filter(|block| block.block.endpoint == endpoint)
            .ok_or_else(|| {
                PolicyError::Unavailable(
                    "historical prefix is not in the canonical byte index".into(),
                )
            })?;
        let ordinary = self.ordinary_range(endpoint.number)?;
        let terminal = usize::try_from(block.block.encoded_item_bytes)
            .map_err(|_| PolicyError::Arithmetic("historical terminal size"))?;
        let payload = ordinary
            .encoded_len_from_items(&Environment::unused(), terminal, 0, 0)
            .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
        let stream = eez_blob_codec::native_stream_len(payload, 0)
            .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
        u64::try_from(stream).map_err(|_| PolicyError::Arithmetic("historical stream size"))
    }

    /// Estimate the complete pending range plus the current terminal block.
    /// No blob allocations, padding, hashing, or semantic re-encoding occurs.
    #[cfg(test)]
    pub fn estimated_stream_bytes(&self, terminal_item_bytes: usize) -> Result<u64, PolicyError> {
        self.estimated_stream_bytes_with_overhead(terminal_item_bytes, DaOverhead::default())
    }

    pub fn estimated_stream_bytes_with_overhead(
        &self,
        terminal_item_bytes: usize,
        overhead: DaOverhead,
    ) -> Result<u64, PolicyError> {
        let end = self.blocks.last_key_value().map_or(Ok(0), |(number, _)| {
            number
                .checked_add(1)
                .ok_or(PolicyError::Arithmetic("pending terminal number"))
        })?;
        let ordinary = self.ordinary_range(end)?;
        let payload_bytes = ordinary
            .encoded_len_from_items(
                &Environment::unused(),
                terminal_item_bytes,
                overhead.entry_items,
                overhead.group_items,
            )
            .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
        let stream_bytes =
            eez_blob_codec::native_stream_len(payload_bytes, overhead.semantic_bytes)
                .map_err(|error| PolicyError::Unavailable(error.to_string()))?;
        u64::try_from(stream_bytes).map_err(|_| PolicyError::Arithmetic("native stream byte count"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_range_index_matches_dense_wire_after_anchor_changes_and_terminal_promotion() {
        use alloy_rlp::Encodable as _;
        use eez_payload_codec::derivable::{Payload, Record, RecordData};
        let anchor = L2Endpoint {
            number: 0,
            hash: B256::ZERO,
        };
        let mut previous = anchor.hash;
        let mut chain = Vec::new();
        let mut bodies = Vec::new();
        let mut environments = Vec::new();
        for number in 1..=513 {
            let mut environment = Environment::unused();
            environment.timestamp_step_seconds = (number / 128) + 1;
            let block = reth_ethereum_primitives::Block {
                header: alloy_consensus::Header {
                    number,
                    parent_hash: previous,
                    ..Default::default()
                },
                ..Default::default()
            };
            let encoded = alloy_rlp::encode(&block);
            let ordinary = if number % 17 == 0 {
                DerivablePayloadSizer::full_block(&encoded, 0).unwrap()
            } else {
                DerivablePayloadSizer::derived_block(environment.clone(), &[]).unwrap()
            };
            chain.push(BlockActivity {
                endpoint: L2Endpoint {
                    number,
                    hash: block.header.hash_slow(),
                },
                parent_hash: previous,
                encoded_item_bytes: encoded.as_slice().length() as u64,
                ordinary,
                pure_transactions: 0,
            });
            previous = block.header.hash_slow();
            bodies.push(encoded);
            environments.push(environment);
        }
        let mut index = ActivityIndex::default();
        let end = chain.last().unwrap().endpoint;
        index
            .update(anchor, end, |hash| {
                Ok(chain
                    .iter()
                    .find(|b| b.endpoint.hash == hash)
                    .unwrap()
                    .clone())
            })
            .unwrap();
        assert!(index.summaries.len() < chain.len() * 2);
        for start in [0, 1, 16, 17, 126, 127, 128, 255, 256, 510, 512] {
            let anchor = if start == 0 {
                anchor
            } else {
                chain[start - 1].endpoint
            };
            index
                .update(anchor, end, |_| {
                    panic!("anchor advance must reuse measurements")
                })
                .unwrap();
            for terminal in [start, (start + 128).min(512), 512] {
                let initial = (start..terminal)
                    .find(|p| (p + 1) % 17 != 0)
                    .map_or_else(Environment::unused, |p| environments[p].clone());
                let mut active = initial.clone();
                let mut records = Vec::new();
                let mut gap = 0;
                for p in start..terminal {
                    let data = if (p + 1) % 17 == 0 {
                        Some(RecordData::FullBlock(bodies[p].clone().into()))
                    } else if active != environments[p] {
                        active = environments[p].clone();
                        Some(RecordData::EnvironmentChange {
                            environment: active.clone(),
                            transactions: vec![],
                        })
                    } else {
                        None
                    };
                    if let Some(data) = data {
                        records.push(Record {
                            empty_gap: gap,
                            data,
                        });
                        gap = 0;
                    } else {
                        gap += 1;
                    }
                }
                let payload = Payload::new(
                    (terminal - start) as u64,
                    initial,
                    records,
                    bodies[terminal].clone(),
                    vec![],
                    vec![],
                )
                .unwrap()
                .encode()
                .unwrap();
                let actual =
                    eez_blob_codec::encode_native_stream(1, payload.into(), vec![]).unwrap();
                assert_eq!(
                    index
                        .historical_stream_bytes(chain[terminal].endpoint)
                        .unwrap(),
                    actual.blob_portion.len() as u64,
                    "range {}..={}",
                    start + 1,
                    terminal + 1
                );
            }
        }
    }

    fn full_payload(blocks: &[Vec<u8>], entries: &[Vec<u8>], groups: &[u16]) -> Vec<u8> {
        use eez_payload_codec::derivable::{Payload, Record, RecordData};
        Payload::new(
            blocks.len() as u64 - 1,
            Environment::unused(),
            blocks[..blocks.len() - 1]
                .iter()
                .map(|rlp| Record {
                    empty_gap: 0,
                    data: RecordData::FullBlock(rlp.clone().into()),
                })
                .collect(),
            blocks.last().unwrap().clone(),
            entries.to_vec(),
            groups.to_vec(),
        )
        .unwrap()
        .encode()
        .unwrap()
    }

    #[test]
    fn individually_fitting_blocks_can_exceed_capacity_before_the_first_sync_boundary() {
        use alloy_rlp::Encodable as _;

        // Chiado's five-block Sync interval: three ordinary data blocks, then
        // two empty blocks. Only the last block is an eligible terminal.
        let budget = 2 * eez_blob_codec::DATA_BYTES_PER_BLOB;
        let mut parent_hash = B256::ZERO;
        let mut bodies = Vec::new();
        let mut activity = Vec::new();
        for number in 1..=5 {
            let mut block = reth_ethereum_primitives::Block::default();
            block.header.number = number;
            block.header.parent_hash = parent_hash;
            if number <= 3 {
                block.body.transactions.push(
                    reth_ethereum_primitives::TransactionSigned::new_unhashed(
                        alloy_consensus::TxLegacy {
                            nonce: number - 1,
                            input: vec![0u8; 100_164].into(),
                            ..Default::default()
                        }
                        .into(),
                        alloy_primitives::Signature::test_signature(),
                    ),
                );
            }
            block.header.transactions_root =
                alloy_consensus::proofs::calculate_transaction_root(&block.body.transactions);
            let hash = block.header.hash_slow();
            let body = alloy_rlp::encode(&block);
            activity.push(BlockActivity {
                endpoint: L2Endpoint { number, hash },
                parent_hash,
                encoded_item_bytes: body.as_slice().length() as u64,
                ordinary: DerivablePayloadSizer::full_block(&body, block.body.transactions.len())
                    .unwrap(),
                pure_transactions: block.body.transactions.len() as u64,
            });
            bodies.push(body);
            parent_hash = hash;
        }
        let measured_stream = |blocks: &[Vec<u8>]| {
            let payload = full_payload(blocks, &[], &[]);
            eez_blob_codec::encode_native_stream(1, payload.into(), vec![])
                .unwrap()
                .blob_portion
                .len()
        };
        for body in &bodies {
            assert!(measured_stream(std::slice::from_ref(body)) <= budget);
        }
        assert!(measured_stream(&bodies[..2]) <= budget);
        assert!(measured_stream(&bodies[..3]) > budget);
        let mut index = ActivityIndex::default();
        let endpoint = activity.last().unwrap().endpoint;
        index
            .update(
                L2Endpoint {
                    number: 0,
                    hash: B256::ZERO,
                },
                endpoint,
                |hash| {
                    Ok(activity
                        .iter()
                        .find(|block| block.endpoint.hash == hash)
                        .unwrap()
                        .clone())
                },
            )
            .unwrap();
        let first_terminal_bytes = index.historical_stream_bytes(endpoint).unwrap();
        assert_eq!(first_terminal_bytes, measured_stream(&bodies) as u64);
        assert!(first_terminal_bytes > budget as u64);
        assert_eq!(index.pending_pure_transactions().unwrap(), 3);
    }

    #[test]
    fn historical_prefix_sizes_match_encoding_after_anchor_advance_and_reorg() {
        use alloy_rlp::Encodable as _;
        let mut parent_hash = B256::ZERO;
        let bodies: Vec<Vec<u8>> = [0, 55, 128, 126_976]
            .into_iter()
            .enumerate()
            .map(|(index, size)| {
                let mut block = reth_ethereum_primitives::Block::default();
                block.header.number = index as u64 + 1;
                block.header.parent_hash = parent_hash;
                block.body.transactions.push(
                    reth_ethereum_primitives::TransactionSigned::new_unhashed(
                        alloy_consensus::TxLegacy {
                            input: vec![0u8; size].into(),
                            ..Default::default()
                        }
                        .into(),
                        alloy_primitives::Signature::test_signature(),
                    ),
                );
                parent_hash = block.header.hash_slow();
                alloy_rlp::encode(block)
            })
            .collect();
        let anchor = L2Endpoint {
            number: 0,
            hash: B256::ZERO,
        };
        let end = L2Endpoint {
            number: 4,
            hash: B256::with_last_byte(4),
        };
        let mut index = ActivityIndex::default();
        index
            .update(anchor, end, |hash| {
                let n = hash[31];
                let mut measured = block(n, n, n - 1, true);
                measured.encoded_item_bytes = bodies[usize::from(n - 1)].as_slice().length() as u64;
                measured.ordinary =
                    DerivablePayloadSizer::full_block(&bodies[usize::from(n - 1)], 1).unwrap();
                Ok(measured)
            })
            .unwrap();
        let oracle = |blocks: &[Vec<u8>]| {
            let payload = full_payload(blocks, &[], &[]);
            eez_blob_codec::encode_native_stream(1, payload.into(), vec![])
                .unwrap()
                .blob_portion
                .len() as u64
        };
        for n in 1..=4 {
            let endpoint = index.endpoint_at(n).unwrap();
            assert_eq!(
                index.historical_stream_bytes(endpoint).unwrap(),
                oracle(&bodies[..n as usize])
            );
        }
        let next_anchor = index.endpoint_at(2).unwrap();
        index
            .update(next_anchor, end, |_| {
                panic!("unchanged history must reuse its measurements")
            })
            .unwrap();
        assert_eq!(
            index.historical_stream_bytes(end).unwrap(),
            oracle(&bodies[2..])
        );
        let replacement = L2Endpoint {
            number: 4,
            hash: B256::with_last_byte(9),
        };
        let mut replacement_block: reth_ethereum_primitives::Block =
            alloy_rlp::decode_exact(&bodies[3]).unwrap();
        replacement_block.body.transactions.clear();
        let replacement_body = alloy_rlp::encode(replacement_block);
        index
            .update(next_anchor, replacement, |_| {
                let mut measured = block(4, 9, 3, false);
                measured.encoded_item_bytes = replacement_body.as_slice().length() as u64;
                measured.ordinary =
                    DerivablePayloadSizer::full_block(&replacement_body, 0).unwrap();
                Ok(measured)
            })
            .unwrap();
        assert!(index.historical_stream_bytes(end).is_err());
        assert_eq!(
            index.historical_stream_bytes(replacement).unwrap(),
            oracle(&[bodies[2].clone(), replacement_body])
        );
    }

    #[test]
    fn measured_fullness_includes_derivation_metadata_and_zero_valued_semantic_data() {
        use alloy_primitives::Bytes;
        use alloy_rlp::Encodable;
        use eez_blob_codec::Message;
        let mut block = reth_ethereum_primitives::Block::default();
        block.header.number = 1;
        let terminal = alloy_rlp::encode(block);
        let index = ActivityIndex::default();
        for size in [0, 55, 128, 16_384, 126_976] {
            let entries = vec![vec![0u8; size], vec![0u8; 128]];
            let groups = vec![1u16, 128];
            let messages = vec![
                Message::InitiateCrossChainTransaction {
                    chain_id: 1,
                    tx_data: Bytes::from(vec![0; size]),
                },
                Message::FinishCrossChainTransaction,
            ];
            let empty_stream =
                eez_blob_codec::encode_native_stream(1, Bytes::new(), messages.clone()).unwrap();
            let overhead = DaOverhead {
                entry_items: entries.iter().map(|entry| entry.as_slice().length()).sum(),
                group_items: groups.iter().map(Encodable::length).sum(),
                semantic_bytes: empty_stream.blob_portion.len()
                    - eez_blob_codec::native_stream_len(0, 0).unwrap(),
            };
            let estimated = index
                .estimated_stream_bytes_with_overhead(terminal.as_slice().length(), overhead)
                .unwrap();
            let operations = full_payload(std::slice::from_ref(&terminal), &entries, &groups);
            let actual =
                eez_blob_codec::encode_native_stream(1, operations.into(), messages).unwrap();
            assert_eq!(estimated, actual.blob_portion.len() as u64);
            let physical = eez_blob_codec::pack(&actual.blob_portion).unwrap();
            assert!(estimated < (physical.len() * eez_blob_codec::BLOB_BYTES) as u64);
            assert!(
                estimated
                    > index
                        .estimated_stream_bytes(terminal.as_slice().length())
                        .unwrap()
            );
        }
    }

    fn block(number: u8, hash: u8, parent: u8, pure: bool) -> BlockActivity {
        BlockActivity {
            endpoint: L2Endpoint {
                number: u64::from(number),
                hash: B256::with_last_byte(hash),
            },
            parent_hash: B256::with_last_byte(parent),
            encoded_item_bytes: 100,
            ordinary: DerivablePayloadSizer::full_block(&[0; 100], 0).unwrap(),
            pure_transactions: u64::from(pure),
        }
    }

    #[test]
    fn changed_codec_identity_rebuilds_byte_accounting_from_canonical_blocks() {
        let genesis = block(0, 0, 0, false).endpoint;
        let pending = block(1, 1, 0, true);
        let mut index = ActivityIndex::default();
        index
            .update(genesis, pending.endpoint, |_| Ok(pending.clone()))
            .unwrap();
        let previous_size = index.estimated_stream_bytes(100).unwrap();
        index.codec.0 ^= 1;
        let mut reads = 0;
        index
            .update(genesis, pending.endpoint, |_| {
                reads += 1;
                Ok(BlockActivity {
                    encoded_item_bytes: 200,
                    ordinary: DerivablePayloadSizer::full_block(&[0; 200], 0).unwrap(),
                    ..pending.clone()
                })
            })
            .unwrap();
        assert_eq!(reads, 1);
        assert_eq!(index.latest_pure(), Some(pending.endpoint));
        assert!(index.estimated_stream_bytes(100).unwrap() > previous_size);
    }

    #[test]
    fn index_loads_only_new_blocks_and_prunes_settled_activity() {
        let genesis = block(0, 0, 0, false).endpoint;
        let chain = [
            block(1, 1, 0, true),
            block(2, 2, 1, false),
            block(3, 3, 2, false),
        ];
        let mut index = ActivityIndex::default();
        let mut reads = 0;
        index
            .update(genesis, chain[1].endpoint, |hash| {
                reads += 1;
                Ok(chain
                    .iter()
                    .find(|b| b.endpoint.hash == hash)
                    .unwrap()
                    .clone())
            })
            .unwrap();
        assert_eq!(reads, 2);
        assert_eq!(index.latest_pure(), Some(chain[0].endpoint));
        assert_eq!(index.pending_pure_transactions().unwrap(), 1);
        let full_bytes = index.estimated_stream_bytes(100).unwrap();
        assert!(
            !index
                .update(genesis, chain[1].endpoint, |_| {
                    panic!("idle index must not reload history")
                })
                .unwrap(),
            "an unchanged range must not rescan the immutable encoding cache"
        );
        assert!(
            !index
                .update(genesis, chain[2].endpoint, |hash| {
                    assert_eq!(hash, chain[2].endpoint.hash);
                    Ok(chain[2].clone())
                })
                .unwrap(),
            "an append must not rescan the immutable encoding cache"
        );
        assert!(index.estimated_stream_bytes(100).unwrap() > full_bytes);
        assert!(
            index
                .update(chain[0].endpoint, chain[2].endpoint, |_| {
                    panic!("advancing anchor must reuse measured blocks")
                })
                .unwrap(),
            "an anchor change must prune settled encodings"
        );
        assert_eq!(index.latest_pure(), None);
        assert_eq!(index.pending_pure_transactions().unwrap(), 0);
        assert_eq!(index.estimated_stream_bytes(100).unwrap(), full_bytes);
    }

    #[test]
    fn same_height_reorg_replaces_activity_and_size_and_rewind_rebuilds() {
        let genesis = block(0, 0, 0, false).endpoint;
        let old = [block(1, 1, 0, false), block(2, 2, 1, true)];
        let mut index = ActivityIndex::default();
        index
            .update(genesis, old[1].endpoint, |hash| {
                Ok(old
                    .iter()
                    .find(|b| b.endpoint.hash == hash)
                    .unwrap()
                    .clone())
            })
            .unwrap();
        let replacement = block(2, 22, 1, false);
        assert!(
            index
                .update(genesis, replacement.endpoint, |hash| {
                    assert_eq!(hash, replacement.endpoint.hash);
                    Ok(replacement.clone())
                })
                .unwrap(),
            "a reorg must remove encodings for the replaced hashes"
        );
        assert_eq!(index.latest_pure(), None);
        assert_eq!(index.pending_pure_transactions().unwrap(), 0);
        assert_eq!(index.endpoint_at(2), Some(replacement.endpoint));
        index
            .update(old[0].endpoint, replacement.endpoint, |_| {
                panic!("reuse canonical prefix")
            })
            .unwrap();
        let mut reads = 0;
        index
            .update(genesis, old[1].endpoint, |hash| {
                reads += 1;
                Ok(old
                    .iter()
                    .find(|b| b.endpoint.hash == hash)
                    .unwrap()
                    .clone())
            })
            .unwrap();
        assert_eq!(reads, 2);
        assert_eq!(index.latest_pure(), Some(old[1].endpoint));
        assert_eq!(index.pending_pure_transactions().unwrap(), 1);
    }

    #[test]
    fn pending_transaction_count_is_incremental_and_counts_all_included_users() {
        let anchor = block(0, 0, 0, false).endpoint;
        let mut first = block(1, 1, 0, true);
        first.pure_transactions = 23;
        let mut second = block(2, 2, 1, true);
        second.pure_transactions = 7;
        let mut index = ActivityIndex::default();
        index
            .update(anchor, second.endpoint, |hash| {
                Ok(if hash == first.endpoint.hash {
                    first.clone()
                } else {
                    second.clone()
                })
            })
            .unwrap();
        assert_eq!(index.pending_pure_transactions().unwrap(), 30);
        index
            .update(first.endpoint, second.endpoint, |_| {
                panic!("reuse canonical counts")
            })
            .unwrap();
        assert_eq!(index.pending_pure_transactions().unwrap(), 7);
        index
            .update(second.endpoint, second.endpoint, |_| {
                panic!("no new block reads")
            })
            .unwrap();
        assert_eq!(index.pending_pure_transactions().unwrap(), 0);
    }

    #[test]
    fn missing_or_inconsistent_history_is_not_an_empty_backlog() {
        let mut index = ActivityIndex::default();
        let genesis = block(0, 0, 0, false).endpoint;
        let wrong = block(1, 1, 99, true);
        assert!(
            index
                .update(genesis, wrong.endpoint, |_| Ok(wrong.clone()))
                .is_err()
        );
        assert!(
            index
                .update(genesis, wrong.endpoint, |_| Err(PolicyError::Unavailable(
                    "missing".into()
                )))
                .is_err()
        );
    }
}
