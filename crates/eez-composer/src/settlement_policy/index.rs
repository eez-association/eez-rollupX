//! Canonical pending-block accounting. Only newly observed blocks are loaded
//! and measured in steady state; a hash change rewinds to the common ancestor.

use std::collections::BTreeMap;

use alloy_primitives::B256;
use eez_l1::L2Endpoint;

use super::PolicyError;

/// One measured block. No receipts are needed to count included transactions.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BlockActivity {
    pub endpoint: L2Endpoint,
    pub parent_hash: B256,
    /// The individual block's length as an element of the existing payload's
    /// `Vec<Vec<u8>>`, including the element's RLP framing.
    pub encoded_item_bytes: u64,
    pub pure_transactions: u64,
}

#[derive(Debug, Clone, Copy)]
struct IndexedBlock {
    block: BlockActivity,
    prefix_bytes: u64,
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
    anchor_prefix_bytes: u64,
    anchor_prefix_pure_transactions: u64,
    blocks: BTreeMap<u64, IndexedBlock>,
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
            eez_payload_codec::TAG_SELF_CONTAINED_BLOCKS,
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
            if let Some(block) = self.blocks.get(&anchor.number).copied()
                && block.block.endpoint == anchor
                && self.anchor.is_some_and(|old| old.number <= anchor.number)
            {
                self.anchor_prefix_bytes = block.prefix_bytes;
                self.anchor_prefix_pure_transactions = block.prefix_pure_transactions;
                self.blocks = self.blocks.split_off(&anchor.number);
                self.blocks.remove(&anchor.number);
            } else {
                self.blocks.clear();
                self.anchor_prefix_bytes = 0;
                self.anchor_prefix_pure_transactions = 0;
            }
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
            pending.push(block);
            cursor = L2Endpoint {
                number: cursor.number - 1,
                hash: block.parent_hash,
            };
        }
        if cursor.number == anchor.number && cursor.hash != anchor.hash {
            return Err(PolicyError::Unavailable(
                "pending L2 ancestry does not match the validated anchor".into(),
            ));
        }
        if let Some(after) = cursor.number.checked_add(1) {
            invalidated |= !self.blocks.split_off(&after).is_empty();
        }
        for block in pending.into_iter().rev() {
            let previous = self.blocks.last_key_value().map(|(_, block)| *block);
            let prefix_bytes = previous
                .map_or(self.anchor_prefix_bytes, |block| block.prefix_bytes)
                .checked_add(block.encoded_item_bytes)
                .ok_or(PolicyError::Arithmetic("pending encoded block bytes"))?;
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
            self.blocks.insert(
                block.endpoint.number,
                IndexedBlock {
                    block,
                    prefix_bytes,
                    prefix_pure_transactions,
                    latest_pure,
                },
            );
        }
        Ok(invalidated)
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
        let items = block
            .prefix_bytes
            .checked_sub(self.anchor_prefix_bytes)
            .ok_or(PolicyError::Arithmetic(
                "historical byte prefix subtraction",
            ))?;
        let items = usize::try_from(items)
            .map_err(|_| PolicyError::Arithmetic("historical byte prefix"))?;
        let payload = eez_payload_codec::self_contained_payload_len(items, 0, 0)
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
        let last = self
            .blocks
            .last_key_value()
            .map_or(self.anchor_prefix_bytes, |(_, block)| block.prefix_bytes);
        let bytes = last
            .checked_sub(self.anchor_prefix_bytes)
            .ok_or(PolicyError::Arithmetic("pending byte prefix subtraction"))?;
        let bytes = usize::try_from(bytes)
            .ok()
            .and_then(|bytes| bytes.checked_add(terminal_item_bytes))
            .ok_or(PolicyError::Arithmetic("pending block length"))?;
        let payload_bytes = eez_payload_codec::self_contained_payload_len(
            bytes,
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
                encoded_item_bytes: body.length() as u64,
                pure_transactions: block.body.transactions.len() as u64,
            });
            bodies.push(body);
            parent_hash = hash;
        }
        let measured_stream = |blocks: &[Vec<u8>]| {
            let payload = eez_payload_codec::encode_self_contained(blocks, &[], &[]).unwrap();
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
                    Ok(*activity
                        .iter()
                        .find(|block| block.endpoint.hash == hash)
                        .unwrap())
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
                measured.encoded_item_bytes = bodies[usize::from(n - 1)].length() as u64;
                Ok(measured)
            })
            .unwrap();
        let oracle = |blocks: &[Vec<u8>]| {
            let payload = eez_payload_codec::encode_self_contained(blocks, &[], &[]).unwrap();
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
                measured.encoded_item_bytes = replacement_body.length() as u64;
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
        let terminal = alloy_rlp::encode(reth_ethereum_primitives::Block::default());
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
                entry_items: entries.iter().map(Encodable::length).sum(),
                group_items: groups.iter().map(Encodable::length).sum(),
                semantic_bytes: empty_stream.blob_portion.len()
                    - eez_blob_codec::native_stream_len(0, 0).unwrap(),
            };
            let estimated = index
                .estimated_stream_bytes_with_overhead(terminal.length(), overhead)
                .unwrap();
            let operations = eez_payload_codec::encode_self_contained(
                std::slice::from_ref(&terminal),
                &entries,
                &groups,
            )
            .unwrap();
            let actual =
                eez_blob_codec::encode_native_stream(1, operations.into(), messages).unwrap();
            assert_eq!(estimated, actual.blob_portion.len() as u64);
            let physical = eez_blob_codec::pack(&actual.blob_portion).unwrap();
            assert!(estimated < (physical.len() * eez_blob_codec::BLOB_BYTES) as u64);
            assert!(estimated > index.estimated_stream_bytes(terminal.length()).unwrap());
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
            pure_transactions: u64::from(pure),
        }
    }

    #[test]
    fn changed_codec_identity_rebuilds_byte_accounting_from_canonical_blocks() {
        let genesis = block(0, 0, 0, false).endpoint;
        let pending = block(1, 1, 0, true);
        let mut index = ActivityIndex::default();
        index
            .update(genesis, pending.endpoint, |_| Ok(pending))
            .unwrap();
        let previous_size = index.estimated_stream_bytes(100).unwrap();
        index.codec.0 ^= 1;
        let mut reads = 0;
        index
            .update(genesis, pending.endpoint, |_| {
                reads += 1;
                Ok(BlockActivity {
                    encoded_item_bytes: 200,
                    ..pending
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
                Ok(*chain.iter().find(|b| b.endpoint.hash == hash).unwrap())
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
                    Ok(chain[2])
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
                Ok(*old.iter().find(|b| b.endpoint.hash == hash).unwrap())
            })
            .unwrap();
        let replacement = block(2, 22, 1, false);
        assert!(
            index
                .update(genesis, replacement.endpoint, |hash| {
                    assert_eq!(hash, replacement.endpoint.hash);
                    Ok(replacement)
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
                Ok(*old.iter().find(|b| b.endpoint.hash == hash).unwrap())
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
                    first
                } else {
                    second
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
                .update(genesis, wrong.endpoint, |_| Ok(wrong))
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
