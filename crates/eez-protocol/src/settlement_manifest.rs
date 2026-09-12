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
use serde::{Deserialize, Serialize};

use crate::abi::ExecutionEntrySol;
use crate::blob_semantics::SemanticTransaction;
use crate::settlement::{FifoSourceAction, composition_fifo_actions};
use crate::system_tx::{OutboundSyncGroup, outbound_settlement_root_counts};
use crate::{ExecutedAction, RollupId};

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

/// One producing virtual effect, with the complete physical replay endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectCheckpoint {
    /// Index into the manifest's FIFO user array.
    pub user: usize,
    /// Indices of logical Sync effects (outbound roots or inbound deliveries).
    pub roots: Range<usize>,
    /// Inclusive L2 transaction index including callbacks.
    pub transaction_end: usize,
    /// Block hash after executing through this endpoint.
    pub block_hash: B256,
}

/// One top-level source action; nested actions remain in the semantic forest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceActionManifest {
    /// Index in `UserManifest::semantic_actions`.
    pub action: usize,
    /// Protocol call identity, as materialized in the source table.
    pub call_hash: B256,
    /// True for a source static-pool entry; false for a mutable-queue entry.
    pub is_static: bool,
    /// Source queue/pool position across the complete batch.
    pub source_index: usize,
    /// Proven source-visible success bit (a revert may be an expected result).
    pub expected_success: bool,
    /// Producing virtual effect, if this source action changes L2 state.
    pub effect: Option<usize>,
}

/// Signed identity and all effect/action boundaries for one accepted user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserManifest {
    /// Hash of the exact signed source envelope.
    pub tx_hash: B256,
    /// Zero for L1, otherwise the settling L2.
    pub source_rollup_id: RollupId,
    /// EIP-155 chain ID bound by the signature, if present.
    pub source_chain_id: Option<u64>,
    /// Recovered source sender.
    pub sender: Address,
    /// Source nonce; inbound and outbound namespaces are independent.
    pub nonce: u64,
    /// Original FIFO position in this attempt.
    pub fifo_position: usize,
    /// Failed settlement episodes already charged.
    pub attempts: u32,
    /// Whole outbound replay group, absent for L1-origin users.
    pub outbound_group: Option<usize>,
    /// Producing virtual effects belonging to this user; possibly empty.
    pub effects: Range<usize>,
    /// FIFO source actions, including static and reverting lookups.
    pub source_actions: Vec<SourceActionManifest>,
    /// Full resolved forest, including nested outcomes and rollback spans.
    pub semantic_actions: Vec<ExecutedAction>,
}

/// Local metadata retained until every signed user's ownership is accounted for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementManifest {
    /// Settling rollup.
    pub rollup_id: u64,
    /// Proposed terminal L2 height.
    pub sync_number: u64,
    /// Proposed full block hash (not evidence that all users completed).
    pub sync_hash: B256,
    /// Users in held-pool order.
    pub users: Vec<UserManifest>,
    /// Ordered producing virtual effects (without the anchor).
    pub effects: Vec<EffectCheckpoint>,
}

impl SettlementManifest {
    /// Manifest for an anchor-only batch, with no held users or effects.
    #[must_use]
    pub const fn empty(rollup_id: u64, sync_number: u64, sync_hash: B256) -> Self {
        Self {
            rollup_id,
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
            }
            users.push(UserManifest {
                tx_hash: *signed.tx_hash(),
                source_rollup_id: user.source,
                source_chain_id: signed.chain_id(),
                sender: signed.recover_signer().map_err(|error| error.to_string())?,
                nonce: signed.nonce(),
                fifo_position: index,
                attempts: user.attempts,
                outbound_group: None,
                effects: 0..0,
                source_actions: Vec::new(),
                semantic_actions: semantic.actions.clone(),
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
                push_checkpoint(&input, &mut effects, user_index, root_cursor..end)?;
                root_cursor = end;
                host_cursor += 1;
            }
            user.outbound_group = Some(group_index);
            user.effects = start..effects.len();
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
        let mut static_index = 0usize;
        // Inbound composition follows held-pool FIFO, regardless of the
        // outbound-first ordering in the native semantic stream.
        for (user_index, user) in users.iter_mut().enumerate() {
            if user.source_rollup_id.0 != 0 {
                continue;
            }
            let composition =
                crate::materialize_actions(user.source_rollup_id, user.semantic_actions.clone())
                    .map_err(|error| error.to_string())?;
            let fifo = composition_fifo_actions(&composition)?;
            let root_indices = user
                .semantic_actions
                .iter()
                .enumerate()
                .filter(|(_, action)| action.parent_index.is_none())
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if fifo.len() != root_indices.len() {
                return Err("source forest and FIFO action counts differ".into());
            }
            let start = effects.len();
            let mut local_mutable = 0usize;
            let mut local_static = 0usize;
            for (kind, action) in fifo.iter().zip(root_indices) {
                let (call_hash, is_static, source_index, expected_success, applies) = match kind {
                    FifoSourceAction::Mutable { applies } => {
                        let entry = &composition.source.batch.entries[local_mutable];
                        local_mutable += 1;
                        let index = mutable_index;
                        mutable_index += 1;
                        (entry.proxyEntryHash, false, index, entry.success, *applies)
                    }
                    FifoSourceAction::Static => {
                        let entry = &composition.source.batch.staticEntries[local_static];
                        local_static += 1;
                        let index = static_index;
                        static_index += 1;
                        (entry.proxyEntryHash, true, index, entry.success, false)
                    }
                };
                let effect = if applies {
                    let index = effects.len();
                    let end = root_cursor
                        .checked_add(1)
                        .ok_or("inbound root count overflow")?;
                    push_checkpoint(&input, &mut effects, user_index, root_cursor..end)?;
                    root_cursor = end;
                    Some(index)
                } else {
                    None
                };
                user.source_actions.push(SourceActionManifest {
                    action,
                    call_hash,
                    is_static,
                    source_index,
                    expected_success,
                    effect,
                });
            }
            user.effects = start..effects.len();
        }
        if root_cursor != input.checkpoint_hashes.len() {
            return Err("unowned physical checkpoints in manifest".into());
        }
        if effects
            .last()
            .is_some_and(|effect| effect.block_hash != input.sync_hash)
        {
            return Err("manifest endpoint differs from full Sync block".into());
        }
        Ok(Self {
            rollup_id: input.rollup_id,
            sync_number: input.sync_number,
            sync_hash: input.sync_hash,
            users,
            effects,
        })
    }
}

fn push_checkpoint(
    input: &ManifestInput<'_>,
    effects: &mut Vec<EffectCheckpoint>,
    user: usize,
    roots: Range<usize>,
) -> Result<(), String> {
    let last = roots.end.checked_sub(1).ok_or("empty effect root range")?;
    let block_hash = *input
        .checkpoint_hashes
        .get(last)
        .ok_or("missing effect block hash")?;
    let transaction_end = *input
        .checkpoint_positions
        .get(last)
        .ok_or("missing effect position")?;
    effects.push(EffectCheckpoint {
        user,
        roots,
        transaction_end,
        block_hash,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system_tx::{OutboundSourceEntry, OutboundSyncEffect};
    use crate::{CallMode, ExecutionOutcome};
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

    /// Pin the complete serialized manifest from before materialization reuse.
    fn assert_manifest_encoding(manifest: &SettlementManifest, expected: B256) {
        assert_eq!(keccak256(serde_json::to_vec(manifest).unwrap()), expected);
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
        assert_manifest_encoding(
            &manifest,
            alloy_primitives::b256!(
                "126b16d5b30970c990ab8ed73a495b10debd0327a56ea17f4417336343596a44"
            ),
        );
        assert!(manifest.effects.is_empty());
        let user = &manifest.users[0];
        assert_eq!(user.source_chain_id, Some(31337));
        assert_eq!(user.nonce, 0);
        assert_eq!(user.attempts, 2);
        assert_eq!(user.effects, 0..0);
        assert_eq!(user.source_actions.len(), 2);
        assert!(user.source_actions[0].is_static);
        assert!(!user.source_actions[1].expected_success);
        assert!(
            user.source_actions
                .iter()
                .all(|action| action.effect.is_none())
        );
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let restored: SettlementManifest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.users[0].tx_hash, user.tx_hash);
        assert_eq!(restored.users[0].source_actions, user.source_actions);
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
        let manifest = SettlementManifest::build(proposal).unwrap();
        assert_manifest_encoding(
            &manifest,
            alloy_primitives::b256!(
                "ca3492c357cc8a04d6ef204e244a10481f95742fe4e234b8ee7b1dbdef4f12b0"
            ),
        );
        assert_eq!(manifest.users[0].effects, 0..2);
        assert_eq!(manifest.users[1].effects, 2..3);
        assert_eq!(
            manifest.users[0]
                .source_actions
                .iter()
                .map(|a| a.effect)
                .collect::<Vec<_>>(),
            [Some(0), None, None, Some(1)]
        );
        assert_eq!(manifest.users[1].source_actions[0].source_index, 3);
        assert_eq!(manifest.effects[1].transaction_end, 1);
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
                    callback_entries: vec![],
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
        proposal.checkpoint_positions = &[1, 1, 2];
        proposal.checkpoint_hashes = &hashes;
        let manifest = SettlementManifest::build(proposal).unwrap();
        assert_manifest_encoding(
            &manifest,
            alloy_primitives::b256!(
                "d2f610174a1452875b7ebdecd0458396fcb507da523158dad7c2cfba12d14f7f"
            ),
        );
        assert_eq!(manifest.users[0].source_rollup_id.0, 0);
        assert_eq!(manifest.users[1].outbound_group, Some(0));
        assert_eq!(manifest.effects[0].user, 1);
        assert_eq!(manifest.effects[0].roots, 0..2);
        assert_eq!(manifest.effects[0].transaction_end, 1);
        assert_eq!(manifest.effects.last().unwrap().user, 0);
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
