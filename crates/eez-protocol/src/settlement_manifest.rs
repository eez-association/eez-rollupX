//! Local, signed-user accounting for a proposed settlement.
//!
//! Virtual effects are indexed in the scanner's producing sequence (the
//! leading anchor is excluded). Outbound hosts, source roots, source queue
//! indices and L2 transaction positions are deliberately different coordinates.
//! This metadata is not a consensus or DA format.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use alloy_consensus::transaction::SignerRecoverable;
use alloy_consensus::{Transaction, TxEnvelope};
use alloy_eips::Decodable2718;
use alloy_primitives::{Address, B256, Bytes, keccak256};

use crate::RollupId;
use crate::abi::ExecutionEntrySol;
use crate::blob_semantics::SemanticTransaction;
use crate::settlement::{FifoSourceAction, composition_fifo_actions};
use crate::system_tx::{OutboundSyncGroup, outbound_settlement_root_counts};

/// Signed source transaction in original held-pool order.
#[derive(Debug)]
pub struct ProposedUser<'a> {
    /// Exact signed EIP-2718 envelope.
    pub raw: &'a Bytes,
    /// Source rollup (zero for L1).
    pub source: RollupId,
    /// Failed settlement episodes already charged to this transaction.
    pub attempts: u32,
}

/// Inputs already produced and validated by the canonical Sync block builder.
#[derive(Debug)]
pub struct ManifestInput<'a> {
    /// Rollup being settled.
    pub rollup_id: u64,
    /// Proposed terminal L2 height.
    pub sync_number: u64,
    /// Full proposed terminal block hash.
    pub sync_hash: B256,
    /// Users in original held-pool order, independently of execution order.
    pub users: Vec<ProposedUser<'a>>,
    /// Exact native semantic forests from accepted compositions.
    pub semantics: &'a [SemanticTransaction],
    /// Whole signed outbound replay groups.
    pub outbound_groups: &'a [OutboundSyncGroup],
    /// Immediate outbound settlement hosts, without the anchor.
    pub outbound_hosts: &'a [ExecutionEntrySol],
    /// Inclusive L2 transaction positions, including all callback deliveries.
    pub checkpoint_positions: &'a [usize],
    /// Independently built prefix block hashes parallel to the positions.
    pub checkpoint_hashes: &'a [B256],
}

/// A producing inbound call whose canonical source receipt must show consumption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceActionManifest {
    /// Protocol call identity, as materialized in the source table.
    pub call_hash: B256,
    /// Mutable source queue position across the complete batch, including lookups.
    pub source_index: usize,
}

/// Signed identity and all effect/action boundaries for one accepted user.
#[derive(Debug, Clone)]
pub struct UserManifest {
    /// Hash of the exact signed source envelope.
    pub tx_hash: B256,
    /// Zero for L1, otherwise the settling L2.
    pub source_rollup_id: RollupId,
    /// Recovered source sender.
    pub sender: Address,
    /// Source nonce; inbound and outbound namespaces are independent.
    pub nonce: u64,
    /// Failed settlement episodes already charged.
    pub attempts: u32,
    /// Whole outbound replay group, absent for L1-origin users.
    pub outbound_group: Option<usize>,
    /// Producing virtual effects belonging to this user; possibly empty.
    pub effects: Range<usize>,
    /// Producing inbound calls in FIFO order. Lookups are validated at construction.
    pub source_actions: Vec<SourceActionManifest>,
    /// Physical terminal-block positions, including loads, source and callbacks.
    pub l2_transaction_range: Range<usize>,
    /// Observations attached once the final block and signed poster exist.
    /// These references never authorize execution or establish settlement.
    pub transaction_references: Option<TransactionReferences>,
}

/// Chain-qualified identity. Rollup IDs are not Ethereum chain IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainTransaction {
    /// Ethereum transaction chain ID, including for native L2 transactions.
    pub chain_id: u64,
    /// Hash of the final encoded transaction, suitable for receipt lookup.
    pub hash: B256,
}

/// All transaction identities for one source's proposed execution group.
/// Receipts and canonical reconciliation determine which candidates executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionReferences {
    /// Signed L1 source for inbound work, or the poster for outbound work.
    pub l1: ChainTransaction,
    /// Complete physical group in execution order, including native setup.
    pub l2: Vec<ChainTransaction>,
    /// Poster role, which can refer to the same transaction as `l1`.
    pub post_batch: ChainTransaction,
}

/// Local metadata retained until every signed user's ownership is accounted for.
#[derive(Debug, Clone)]
pub struct SettlementManifest {
    /// Proposed terminal L2 height.
    pub sync_number: u64,
    /// Proposed full block hash (not evidence that all users completed).
    pub sync_hash: B256,
    /// Users in held-pool order.
    pub users: Vec<UserManifest>,
    /// Complete endpoint hashes ordered by producing virtual effect (without the anchor).
    pub effects: Vec<B256>,
}

impl SettlementManifest {
    /// Attach identities from the built block, preserving its actual encoded
    /// transactions and group order instead of synthesizing hashes from actions.
    ///
    /// # Errors
    /// Rejects incomplete/overlapping physical coverage and missing outbound
    /// sources. No references are changed on failure.
    pub fn attach_transaction_references(
        &mut self,
        post_batch: ChainTransaction,
        transactions: &[ChainTransaction],
    ) -> Result<(), String> {
        let mut covered = vec![false; transactions.len()];
        let mut references = Vec::with_capacity(self.users.len());
        for user in &self.users {
            let range = user.l2_transaction_range.clone();
            let group = transactions
                .get(range.clone())
                .ok_or("invalid user transaction range")?;
            for index in range {
                if std::mem::replace(&mut covered[index], true) {
                    return Err("overlapping user transaction ranges".into());
                }
            }
            if user.source_rollup_id.0 != 0 && !group.iter().any(|tx| tx.hash == user.tx_hash) {
                return Err("outbound transaction range omits its source".into());
            }
            references.push(TransactionReferences {
                l1: if user.source_rollup_id.0 == 0 {
                    ChainTransaction {
                        chain_id: post_batch.chain_id,
                        hash: user.tx_hash,
                    }
                } else {
                    post_batch
                },
                l2: group.to_vec(),
                post_batch,
            });
        }
        if covered.iter().any(|covered| !covered) {
            return Err("unowned transactions in terminal block".into());
        }
        for (user, references) in self.users.iter_mut().zip(references) {
            user.transaction_references = Some(references);
        }
        Ok(())
    }

    /// Manifest for an anchor-only batch, with no held users or effects.
    #[must_use]
    pub const fn empty(sync_number: u64, sync_hash: B256) -> Self {
        Self {
            sync_number,
            sync_hash,
            users: Vec::new(),
            effects: Vec::new(),
        }
    }
    /// Construct a manifest from the same groups, FIFO projection and physical
    /// checkpoints used by block construction and proof generation.
    ///
    /// # Errors
    ///
    /// Rejects malformed envelopes, missing/duplicate users, inconsistent source
    /// chains, split outbound hosts, and incomplete checkpoint/action coverage.
    pub fn build(input: ManifestInput<'_>) -> Result<Self, String> {
        if input.checkpoint_positions.len() != input.checkpoint_hashes.len()
            || input.checkpoint_positions.windows(2).any(|p| p[0] > p[1])
        {
            return Err("manifest checkpoints have inconsistent positions".into());
        }
        let mut semantics = HashMap::new();
        for semantic in input.semantics {
            if semantics
                .insert(keccak256(&semantic.tx_data), semantic)
                .is_some()
            {
                return Err("duplicate semantic source transaction".into());
            }
        }
        let mut users = Vec::with_capacity(input.users.len());
        let mut inbound_semantics = Vec::new();
        let mut by_hash = HashMap::new();
        for (index, user) in input.users.iter().enumerate() {
            let mut encoded = user.raw.as_ref();
            let signed = TxEnvelope::decode_2718(&mut encoded)
                .map_err(|error| format!("manifest signed user: {error}"))?;
            if !encoded.is_empty() {
                return Err("manifest signed user has trailing bytes".into());
            }
            let hash = keccak256(user.raw);
            let semantic = semantics
                .remove(&hash)
                .ok_or_else(|| format!("missing semantic forest for {hash}"))?;
            if semantic.tx_data != *user.raw
                || semantic.origin_chain != user.source
                || (user.source.0 != 0 && user.source.0 != input.rollup_id)
                || by_hash.insert(hash, index).is_some()
            {
                return Err("manifest source identity mismatch".into());
            }
            // Outbound forests are validated here. Inbound forests are
            // materialized and validated below when mapping their FIFO actions.
            if user.source.0 != 0 {
                semantic.materialize().map_err(|error| error.to_string())?;
            } else {
                inbound_semantics.push((index, semantic));
            }
            users.push(UserManifest {
                tx_hash: *signed.tx_hash(),
                source_rollup_id: user.source,
                sender: signed.recover_signer().map_err(|error| error.to_string())?,
                nonce: signed.nonce(),
                attempts: user.attempts,
                outbound_group: None,
                effects: 0..0,
                source_actions: Vec::new(),
                l2_transaction_range: 0..0,
                transaction_references: None,
            });
        }
        if !semantics.is_empty() {
            return Err("unowned semantic source transaction".into());
        }
        let root_counts = outbound_settlement_root_counts(input.outbound_hosts)?;
        let mut effects = Vec::new();
        let mut root_cursor = 0usize;
        let mut host_cursor = 0usize;
        let mut outbound_users = HashSet::new();
        for (group_index, group) in input.outbound_groups.iter().enumerate() {
            let user_index = *by_hash
                .get(&keccak256(&group.user_tx))
                .ok_or("outbound group has no signed user")?;
            let user = &mut users[user_index];
            if user.source_rollup_id.0 != input.rollup_id
                || !outbound_users.insert(user_index)
                || group.effects.is_empty()
            {
                return Err("invalid outbound user group".into());
            }
            let group_end = root_cursor
                .checked_add(group.effects.len())
                .ok_or("outbound root count overflow")?;
            let start = effects.len();
            let transaction_start = transaction_end(&input, root_cursor)?;
            while root_cursor < group_end {
                let count = *root_counts
                    .get(host_cursor)
                    .ok_or("outbound group has no settlement host")?;
                let end = root_cursor
                    .checked_add(count)
                    .ok_or("outbound host count overflow")?;
                if count == 0 || end > group_end {
                    return Err("outbound settlement host splits signed user groups".into());
                }
                push_checkpoint(&input, &mut effects, end)?;
                root_cursor = end;
                host_cursor += 1;
            }
            user.outbound_group = Some(group_index);
            user.effects = start..effects.len();
            user.l2_transaction_range = transaction_start..transaction_end(&input, root_cursor)?;
        }
        if host_cursor != root_counts.len()
            || users
                .iter()
                .enumerate()
                .any(|(i, user)| user.source_rollup_id.0 != 0 && !outbound_users.contains(&i))
        {
            return Err("incomplete outbound manifest coverage".into());
        }

        let mut mutable_index = 0usize;
        // Inbound composition follows held-pool FIFO, regardless of the
        // outbound-first ordering in the native semantic stream.
        for (user_index, semantic) in inbound_semantics {
            let user = &mut users[user_index];
            let composition =
                crate::materialize_actions(user.source_rollup_id, semantic.actions.clone())
                    .map_err(|error| error.to_string())?;
            let fifo = composition_fifo_actions(&composition)?;
            let root_count = semantic
                .actions
                .iter()
                .filter(|action| action.parent_index.is_none())
                .count();
            if fifo.len() != root_count {
                return Err("source forest and FIFO action counts differ".into());
            }
            let start = effects.len();
            let transaction_start = transaction_end(&input, root_cursor)?;
            let mut local_mutable = 0usize;
            for kind in fifo {
                let FifoSourceAction::Mutable { applies } = kind else {
                    continue;
                };
                let entry = &composition.source.batch.entries[local_mutable];
                local_mutable += 1;
                let source_index = mutable_index;
                mutable_index += 1;
                // Non-producing mutable lookups still occupy queue positions.
                if !applies {
                    continue;
                }
                let end = root_cursor
                    .checked_add(1)
                    .ok_or("inbound root count overflow")?;
                push_checkpoint(&input, &mut effects, end)?;
                root_cursor = end;
                user.source_actions.push(SourceActionManifest {
                    call_hash: entry.proxyEntryHash,
                    source_index,
                });
            }
            user.effects = start..effects.len();
            user.l2_transaction_range = transaction_start..transaction_end(&input, root_cursor)?;
        }
        if root_cursor != input.checkpoint_hashes.len() {
            return Err("unowned physical checkpoints in manifest".into());
        }
        if effects.last().is_some_and(|hash| *hash != input.sync_hash) {
            return Err("manifest endpoint differs from full Sync block".into());
        }
        Ok(Self {
            sync_number: input.sync_number,
            sync_hash: input.sync_hash,
            users,
            effects,
        })
    }
}

fn transaction_end(input: &ManifestInput<'_>, roots: usize) -> Result<usize, String> {
    if roots == 0 {
        return Ok(0);
    }
    input
        .checkpoint_positions
        .get(roots - 1)
        .and_then(|position| position.checked_add(1))
        .ok_or_else(|| "missing or overflowing physical transaction checkpoint".into())
}

fn push_checkpoint(
    input: &ManifestInput<'_>,
    effects: &mut Vec<B256>,
    root_end: usize,
) -> Result<(), String> {
    let last = root_end.checked_sub(1).ok_or("empty effect root range")?;
    let block_hash = *input
        .checkpoint_hashes
        .get(last)
        .ok_or("missing effect block hash")?;
    effects.push(block_hash);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system_tx::{OutboundSourceEntry, OutboundSyncEffect};
    use crate::{CallMode, ExecutedAction, ExecutionOutcome};
    use alloy_consensus::{SignableTransaction, TxEip1559};
    use alloy_eips::Encodable2718;
    use alloy_network::TxSignerSync;
    use alloy_primitives::{TxKind, U256};
    use alloy_signer_local::PrivateKeySigner;

    fn signed(nonce: u64) -> Bytes {
        let signer: PrivateKeySigner = B256::with_last_byte(1).to_string().parse().unwrap();
        let mut tx = TxEip1559 {
            chain_id: 31337,
            nonce,
            gas_limit: 1_000_000,
            max_fee_per_gas: 10,
            max_priority_fee_per_gas: 1,
            to: TxKind::Call(Address::repeat_byte(3)),
            ..Default::default()
        };
        let signature = signer.sign_transaction_sync(&mut tx).unwrap();
        Bytes::from(TxEnvelope::from(tx.into_signed(signature)).encoded_2718())
    }

    fn action(source: u64, mode: CallMode, success: bool) -> ExecutedAction {
        ExecutedAction {
            parent_index: None,
            call_mode: mode,
            target_address: Address::repeat_byte(7),
            target_rollup_id: RollupId(u64::from(source == 0)),
            source_rollup_id: RollupId(source),
            source_address: Address::repeat_byte(8),
            data: Bytes::from_static(b"call"),
            value: U256::ZERO,
            outcome: ExecutionOutcome::Resolved {
                return_data: vec![],
                gas_used: 0,
                success,
            },
            revert_span: None,
        }
    }

    fn input<'a>(users: &'a [SemanticTransaction], order: &[usize]) -> ManifestInput<'a> {
        ManifestInput {
            rollup_id: 1,
            sync_number: 10,
            sync_hash: B256::repeat_byte(5),
            users: order
                .iter()
                .map(|index| ProposedUser {
                    raw: &users[*index].tx_data,
                    source: users[*index].origin_chain,
                    attempts: 2,
                })
                .collect(),
            semantics: users,
            outbound_groups: &[],
            outbound_hosts: &[],
            checkpoint_positions: &[],
            checkpoint_hashes: &[],
        }
    }

    #[test]
    fn manifest_keeps_static_and_reverting_lookup_users_without_effects() {
        let users = [SemanticTransaction {
            origin_chain: RollupId(0),
            tx_data: signed(0),
            actions: vec![
                action(0, CallMode::Static, true),
                action(0, CallMode::Mutable, false),
            ],
        }];
        let manifest = SettlementManifest::build(input(&users, &[0])).unwrap();
        assert!(manifest.effects.is_empty());
        let user = &manifest.users[0];
        assert_eq!(user.nonce, 0);
        assert_eq!(user.attempts, 2);
        assert_eq!(user.effects, 0..0);
        assert!(user.source_actions.is_empty());
    }

    #[test]
    fn manifest_maps_multiple_inbound_actions_at_exact_fifo_positions() {
        let users = [
            SemanticTransaction {
                origin_chain: RollupId(0),
                tx_data: signed(0),
                actions: vec![
                    action(0, CallMode::Mutable, true),
                    action(0, CallMode::Static, true),
                    action(0, CallMode::Mutable, false),
                    action(0, CallMode::Mutable, true),
                ],
            },
            SemanticTransaction {
                origin_chain: RollupId(0),
                tx_data: signed(1),
                actions: vec![action(0, CallMode::Mutable, true)],
            },
        ];
        let hashes = [
            B256::repeat_byte(3),
            B256::repeat_byte(4),
            B256::repeat_byte(5),
        ];
        let mut proposal = input(&users, &[0, 1]);
        proposal.checkpoint_positions = &[0, 1, 2];
        proposal.checkpoint_hashes = &hashes;
        let mut manifest = SettlementManifest::build(proposal).unwrap();
        assert_eq!(manifest.users[0].effects, 0..2);
        assert_eq!(manifest.users[1].effects, 2..3);
        assert_eq!(
            manifest.users[0]
                .source_actions
                .iter()
                .map(|a| a.source_index)
                .collect::<Vec<_>>(),
            [0, 2]
        );
        assert_eq!(manifest.users[1].source_actions[0].source_index, 3);
        let composition = users[0].materialize().unwrap();
        assert_eq!(
            manifest.users[0].source_actions,
            [0, 2].map(|index| SourceActionManifest {
                call_hash: composition.source.batch.entries[index].proxyEntryHash,
                source_index: index,
            })
        );
        assert_eq!(manifest.effects, hashes);
        assert_eq!(manifest.users[0].l2_transaction_range, 0..2);
        assert_eq!(manifest.users[1].l2_transaction_range, 2..3);
        let post = ChainTransaction {
            chain_id: 1,
            hash: B256::repeat_byte(99),
        };
        let transactions = (0..3)
            .map(|i| ChainTransaction {
                chain_id: if i == 1 { 9002 } else { 9001 },
                hash: B256::with_last_byte(i),
            })
            .collect::<Vec<_>>();
        manifest
            .attach_transaction_references(post, &transactions)
            .unwrap();
        for (i, range) in [0..2, 2..3].into_iter().enumerate() {
            let refs = manifest.users[i].transaction_references.as_ref().unwrap();
            assert_eq!(refs.l1.hash, keccak256(&users[i].tx_data));
            assert_eq!(refs.l1.chain_id, 1);
            assert_eq!(refs.post_batch, post);
            assert_eq!(refs.l2, transactions[range]);
        }
        // Reject corrupted boundaries without publishing a partial replacement.
        for range in [1..3, 2..4, 3..3] {
            let mut invalid = manifest.clone();
            invalid.users[1].l2_transaction_range = range;
            assert!(
                invalid
                    .attach_transaction_references(post, &transactions)
                    .is_err()
            );
            assert_eq!(
                invalid.users[0].transaction_references,
                manifest.users[0].transaction_references
            );
        }
    }

    #[test]
    fn lookup_only_user_keeps_fifo_identity_and_advances_mutable_queue_positions() {
        let users = [
            SemanticTransaction {
                origin_chain: RollupId(0),
                tx_data: signed(1),
                actions: vec![action(0, CallMode::Mutable, true)],
            },
            SemanticTransaction {
                origin_chain: RollupId(0),
                tx_data: signed(0),
                actions: vec![
                    action(0, CallMode::Static, true),
                    action(0, CallMode::Mutable, false),
                    action(0, CallMode::Static, true),
                    action(0, CallMode::Mutable, false),
                ],
            },
        ];
        let hashes = [B256::repeat_byte(5)];
        let mut proposal = input(&users, &[1, 0]);
        proposal.checkpoint_positions = &[0];
        proposal.checkpoint_hashes = &hashes;
        let mut manifest = SettlementManifest::build(proposal).unwrap();
        assert_eq!(manifest.users.len(), 2);
        assert_eq!(manifest.users[0].tx_hash, keccak256(&users[1].tx_data));
        assert_eq!(manifest.users[0].effects, 0..0);
        assert!(manifest.users[0].source_actions.is_empty());
        assert_eq!(manifest.users[1].tx_hash, keccak256(&users[0].tx_data));
        assert_eq!(manifest.users[1].effects, 0..1);
        assert_eq!(manifest.users[1].source_actions.len(), 1);
        assert_eq!(manifest.users[1].source_actions[0].source_index, 2);
        assert_eq!(manifest.effects, hashes);
        let post = ChainTransaction {
            chain_id: 1,
            hash: B256::repeat_byte(99),
        };
        manifest
            .attach_transaction_references(
                post,
                &[ChainTransaction {
                    chain_id: 9001,
                    hash: B256::repeat_byte(98),
                }],
            )
            .unwrap();
        assert!(
            manifest.users[0]
                .transaction_references
                .as_ref()
                .unwrap()
                .l2
                .is_empty()
        );
        assert_eq!(
            manifest.users[1]
                .transaction_references
                .as_ref()
                .unwrap()
                .l2
                .len(),
            1
        );
    }

    #[test]
    fn manifest_preserves_fifo_across_outbound_first_and_whole_multiroot_groups() {
        let users = [
            SemanticTransaction {
                origin_chain: RollupId(1),
                tx_data: signed(9),
                actions: vec![
                    action(1, CallMode::Mutable, true),
                    action(1, CallMode::Mutable, true),
                ],
            },
            SemanticTransaction {
                origin_chain: RollupId(0),
                tx_data: signed(0),
                actions: vec![action(0, CallMode::Mutable, true)],
            },
        ];
        let composition = users[0].materialize().unwrap();
        let groups = [OutboundSyncGroup {
            effects: composition
                .source
                .batch
                .entries
                .iter()
                .map(|entry| OutboundSyncEffect {
                    source_entry: OutboundSourceEntry::Mutable(entry.clone()),
                })
                .collect(),
            user_tx: users[0].tx_data.clone(),
        }];
        assert_eq!(groups[0].effects.len(), 2);
        let hosts = composition
            .targets
            .iter()
            .find(|target| target.rollup_id.0 == 0)
            .unwrap()
            .batch
            .entries
            .clone();
        let hashes = [
            B256::repeat_byte(4),
            B256::repeat_byte(4),
            B256::repeat_byte(5),
        ];
        let mut proposal = input(&users, &[1, 0]);
        proposal.outbound_groups = &groups;
        proposal.outbound_hosts = &hosts;
        proposal.checkpoint_positions = &[4, 4, 6];
        proposal.checkpoint_hashes = &hashes;
        let mut manifest = SettlementManifest::build(proposal).unwrap();
        assert_eq!(manifest.users[0].source_rollup_id.0, 0);
        assert_eq!(manifest.users[1].outbound_group, Some(0));
        assert_eq!(manifest.users[1].effects, 0..1);
        assert_eq!(manifest.users[0].effects, 1..2);
        assert_eq!(manifest.effects, [hashes[1], hashes[2]]);
        assert_eq!(manifest.users[1].l2_transaction_range, 0..5);
        assert_eq!(manifest.users[0].l2_transaction_range, 5..7);
        let post = ChainTransaction {
            chain_id: 1,
            hash: B256::repeat_byte(99),
        };
        let mut transactions = (0..7)
            .map(|i| ChainTransaction {
                chain_id: 9001,
                hash: B256::with_last_byte(i),
            })
            .collect::<Vec<_>>();
        assert!(
            manifest
                .attach_transaction_references(post, &transactions)
                .unwrap_err()
                .contains("omits its source")
        );
        assert!(
            manifest
                .users
                .iter()
                .all(|user| user.transaction_references.is_none())
        );
        transactions[1].hash = keccak256(&users[0].tx_data);
        manifest
            .attach_transaction_references(post, &transactions)
            .unwrap();
        let outbound = manifest.users[1].transaction_references.as_ref().unwrap();
        assert_eq!(outbound.l2, transactions[..5]);
        assert_eq!(outbound.l1, post);
        assert_eq!(outbound.post_batch, post);
        assert_eq!(
            manifest.users[0]
                .transaction_references
                .as_ref()
                .unwrap()
                .l2,
            transactions[5..]
        );
    }

    #[test]
    fn manifest_rejects_missing_duplicate_or_malformed_signed_users() {
        let mut users = [SemanticTransaction {
            origin_chain: RollupId(0),
            tx_data: signed(0),
            actions: vec![action(0, CallMode::Static, true)],
        }];
        assert!(SettlementManifest::build(input(&users, &[])).is_err());
        assert!(SettlementManifest::build(input(&users, &[0, 0])).is_err());
        users[0].tx_data = Bytes::from([users[0].tx_data.as_ref(), &[0]].concat());
        assert!(
            SettlementManifest::build(input(&users, &[0]))
                .unwrap_err()
                .contains("trailing")
        );
    }

    #[test]
    fn manifest_rejects_invalid_semantic_forests_in_both_directions() {
        for source in [0, 1] {
            for invalid in ["empty", "pending", "parent", "rollback", "static value"] {
                let mut call = action(source, CallMode::Mutable, true);
                match invalid {
                    "pending" => call.outcome = ExecutionOutcome::Pending,
                    "parent" => call.parent_index = Some(0),
                    "rollback" => call.revert_span = Some(2),
                    "static value" => {
                        call.call_mode = CallMode::Static;
                        call.value = U256::from(1);
                    }
                    _ => {}
                }
                let users = [SemanticTransaction {
                    origin_chain: RollupId(source),
                    tx_data: signed(0),
                    actions: if invalid == "empty" {
                        vec![]
                    } else {
                        vec![call]
                    },
                }];
                let error = SettlementManifest::build(input(&users, &[0])).unwrap_err();
                assert!(
                    !error.contains("checkpoint") && !error.contains("outbound"),
                    "{source}/{invalid} must fail semantic validation: {error}",
                );
                assert!(
                    users[0]
                        .materialize()
                        .unwrap_err()
                        .to_string()
                        .contains(&error)
                );
            }
        }
    }

    #[test]
    fn manifest_rejects_missing_or_unowned_checkpoints() {
        let users = [SemanticTransaction {
            origin_chain: RollupId(0),
            tx_data: signed(0),
            actions: vec![action(0, CallMode::Mutable, true)],
        }];
        assert!(SettlementManifest::build(input(&users, &[0])).is_err());
        let hashes = [B256::repeat_byte(4), B256::repeat_byte(5)];
        for positions in [&[0][..], &[1, 0][..]] {
            let mut proposal = input(&users, &[0]);
            proposal.checkpoint_positions = positions;
            proposal.checkpoint_hashes = &hashes;
            assert!(
                SettlementManifest::build(proposal)
                    .unwrap_err()
                    .contains("inconsistent positions")
            );
        }
        let mut proposal = input(&users, &[0]);
        proposal.checkpoint_positions = &[0, 1];
        proposal.checkpoint_hashes = &hashes;
        assert!(
            SettlementManifest::build(proposal)
                .unwrap_err()
                .contains("unowned")
        );
        let mut proposal = input(&users, &[0]);
        proposal.checkpoint_positions = &[0];
        proposal.checkpoint_hashes = &hashes[..1];
        assert!(
            SettlementManifest::build(proposal)
                .unwrap_err()
                .contains("endpoint")
        );
    }
}
