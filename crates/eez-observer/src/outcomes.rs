//! Validate the coherence of native decision evidence before checking any chain
//! anchors. A successful result is still a node claim, not canonical assurance.
use eez_telemetry::schema::{
    Assurance, AttemptDetails, AttemptId, BlockRef, ChainScope, Direction, Epoch, Event, Member,
    Outcome, ReconciledSource, ReconciliationDetails, ResolutionDisposition, Source, SourceBasis,
    SourcePlan,
};
use std::collections::HashSet;

const MAX_MEMBERS: usize = 4096;
const MAX_EFFECTS: usize = 32768;
const MAX_ACTIONS: usize = 32768;
pub const MAX_LINEAGE: usize = 128;

#[derive(Debug, Clone, Copy)]
pub struct Record<'a> {
    pub source_id: i64,
    pub epoch: Epoch,
    pub sequence: u64,
    pub event: &'a Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    UnsupportedClaim,
    MissingParent,
    Identity,
    Resolution,
    MissingMembership,
    Lineage,
    Source,
    References,
    Limit,
}

impl Gap {
    pub const fn label(self) -> &'static str {
        match self {
            Self::UnsupportedClaim => "unsupported_claim",
            Self::MissingParent => "missing_parent",
            Self::Identity => "identity",
            Self::Resolution => "resolution",
            Self::MissingMembership => "missing_membership",
            Self::Lineage => "lineage",
            Self::Source => "source",
            Self::References => "references",
            Self::Limit => "limit",
        }
    }
}

#[derive(Debug)]
pub struct NativeDecision<'a> {
    pub member: Member,
    pub owner: AttemptId,
    pub outcome: Outcome,
    pub elapsed_ns: u64,
    /// The scan watermark is distinct from every receipt inclusion reference.
    pub l1_snapshot: BlockRef,
    pub details: &'a ReconciliationDetails,
    pub source: &'a ReconciledSource,
    pub manifest: &'a AttemptDetails,
    pub plan: &'a SourcePlan,
}

fn source_matches(source: &Source, scopes: &[ChainScope; 2]) -> bool {
    let scope = &scopes[usize::from(source.direction == Direction::Outbound)];
    (source.chain_id, source.genesis_hash, source.rollup_id)
        == (scope.chain_id, scope.genesis_hash, scope.rollup_id)
}

fn before(record: Record<'_>, next: Record<'_>) -> bool {
    record.source_id == next.source_id
        && record.epoch == next.epoch
        && record.sequence > 0
        && record.sequence < next.sequence
}

/// `lineage` starts with the prepared manifest and contains every subsequent
/// repricing needed to reach the parent's exact signed attempt identity. The
/// caller must load all records from complete, digest-verified transport events.
/// Missing records, duplicate membership and contradictory fields stay unknown.
/// This does not independently re-run the native outcome classification.
pub fn native_decision<'a>(
    scopes: &[ChainScope; 2],
    terminal: Record<'a>,
    parent: Record<'a>,
    lineage: &[Record<'a>],
) -> Result<NativeDecision<'a>, Gap> {
    let Event::Terminal {
        member,
        attempt: Some(owner),
        outcome,
        assurance: Assurance::NodeReconciled,
        anchor: None,
        elapsed_ns,
        reconciliation: Some(reference),
    } = terminal.event
    else {
        return Err(Gap::UnsupportedClaim);
    };
    if !matches!(
        outcome,
        Outcome::Committed
            | Outcome::SourceReverted
            | Outcome::SourceCompleted
            | Outcome::SourceCompletedWithMissingEffects
            | Outcome::SourceNonceSpent
    ) {
        return Err(Gap::UnsupportedClaim);
    }
    if !before(parent, terminal)
        || reference.epoch != parent.epoch
        || reference.sequence != parent.sequence
        || member.admission.epoch != terminal.epoch
        || member.admission.sequence == 0
        || member.admission.sequence >= parent.sequence
        || owner.epoch != terminal.epoch
    {
        return Err(Gap::Identity);
    }
    let Event::Reconciled {
        attempt,
        anchor,
        details: Some(details),
    } = parent.event
    else {
        return Err(Gap::MissingParent);
    };
    // Only local nonce-gap cascades can retain another generation's owner in
    // this native ledger. Those are handled by the local-policy views instead.
    if attempt != owner
        || details.l1 != scopes[0]
        || details.l2 != scopes[1]
        || scopes[0].rollup_id != 0
        || !source_matches(&member.source, scopes)
    {
        return Err(Gap::Identity);
    }
    if details.resolutions.len() > MAX_MEMBERS
        || details.sources.len() > MAX_MEMBERS
        || lineage.len() > MAX_LINEAGE
    {
        return Err(Gap::Limit);
    }
    let mut resolutions = details
        .resolutions
        .iter()
        .filter(|r| r.member.admission == member.admission);
    let resolution = resolutions.next().ok_or(Gap::Resolution)?;
    if resolutions.next().is_some()
        || resolution.member != *member
        || resolution.owner != *owner
        || resolution.elapsed_ns != *elapsed_ns
        || resolution.disposition
            != (ResolutionDisposition::Terminal {
                outcome: *outcome,
                assurance: Assurance::NodeReconciled,
            })
    {
        return Err(Gap::Resolution);
    }
    let prepared = *lineage.first().ok_or(Gap::MissingMembership)?;
    let Event::AttemptPrepared {
        attempt: initial,
        members,
        details: Some(manifest),
        sync_height,
        ..
    } = prepared.event
    else {
        return Err(Gap::MissingMembership);
    };
    if !before(prepared, parent)
        || initial.epoch != prepared.epoch
        || members.len() != manifest.plans.len()
        || members.len() > MAX_MEMBERS
        || manifest.l2_chain_id != scopes[1].chain_id
        || manifest.l2_genesis_hash != scopes[1].genesis_hash
        || manifest.rollup_id != scopes[1].rollup_id
    {
        return Err(Gap::MissingMembership);
    }
    let actions = manifest.plans.iter().try_fold(0usize, |count, plan| {
        count.checked_add(plan.source_actions.len())
    });
    let consumed = details.sources.iter().try_fold(0usize, |count, source| {
        count.checked_add(source.consumed.len())
    });
    if manifest.effects.len() > MAX_EFFECTS
        || actions.is_none_or(|count| count > MAX_ACTIONS)
        || consumed.is_none_or(|count| count > MAX_ACTIONS)
    {
        return Err(Gap::Limit);
    }
    // Sequence order bounds traversal; signed hashes may recur after a fee
    // quote changes again. Such recurrence is not another paid receipt.
    let mut current = *initial;
    let mut previous = prepared;
    for record in &lineage[1..] {
        let Event::AttemptRepriced {
            previous: old,
            replacement,
        } = record.event
        else {
            return Err(Gap::Lineage);
        };
        if !before(previous, *record)
            || !before(*record, parent)
            || *old != current
            || replacement.epoch != current.epoch
            || replacement.generation != current.generation
            || replacement.post_batch_hash == current.post_batch_hash
        {
            return Err(Gap::Lineage);
        }
        current = *replacement;
        previous = *record;
    }
    if current != *owner {
        return Err(Gap::Lineage);
    }
    let outbound_count = members
        .iter()
        .filter(|member| member.source.direction == Direction::Outbound)
        .count() as u64;
    let mut groups = HashSet::new();
    let mut admissions = HashSet::new();
    let mut hashes = HashSet::new();
    for (candidate, plan) in members.iter().zip(&manifest.plans) {
        let group_valid = match (candidate.source.direction, plan.outbound_group) {
            (Direction::Inbound, None) => true,
            (Direction::Outbound, Some(group)) => {
                group < outbound_count
                    && groups.insert(group)
                    && plan.effects_start < plan.effects_end
            }
            _ => false,
        };
        if !group_valid
            || candidate.admission.epoch != prepared.epoch
            || candidate.admission.sequence == 0
            || candidate.admission.sequence >= prepared.sequence
            || !source_matches(&candidate.source, scopes)
            || !admissions.insert((candidate.admission.epoch, candidate.admission.sequence))
            || !hashes.insert(candidate.source.transaction_hash)
            || plan.effects_start > plan.effects_end
            || plan.effects_end > manifest.effects.len() as u64
        {
            return Err(Gap::MissingMembership);
        }
    }
    let position = members
        .iter()
        .position(|candidate| candidate == member)
        .ok_or(Gap::MissingMembership)?;
    let plan = &manifest.plans[position];
    if details.sources.len() != members.len()
        || details.sources.iter().zip(members).any(|(source, member)| {
            source.transaction_hash != member.source.transaction_hash
                || source.direction != member.source.direction
        })
    {
        return Err(Gap::Source);
    }
    let source = details.sources.get(position).ok_or(Gap::Source)?;
    let l2_snapshot = details.l2_snapshot.ok_or(Gap::Source)?;
    if source.observed_nonce <= plan.nonce {
        return Err(Gap::Source);
    }
    if source.direction == Direction::Outbound
        && source
            .source_block
            .is_some_and(|block| block.number > l2_snapshot.number)
    {
        return Err(Gap::References);
    }
    if let Some(post) = details.post
        && post.block.number > anchor.number
    {
        return Err(Gap::References);
    }
    if let Some(applied) = &details.applied
        && (applied.transaction_hash != owner.post_batch_hash
            || applied.block.number > anchor.number
            || applied.effects_start > applied.effects_end
            || applied.effects_end > manifest.effects.len() as u64
            || applied.outbound_groups_start > applied.outbound_groups_end
            || applied.inbound_effects_start > applied.inbound_effects_end
            || applied
                .endpoint
                .is_some_and(|endpoint| endpoint.number > l2_snapshot.number)
            || details.post.is_some_and(|post| {
                post.block != applied.block
                    || post.transaction_index != applied.transaction_index
                    || !post.success
            }))
    {
        return Err(Gap::References);
    }
    if let Some(applied) = &details.applied {
        // Native callers receive validated replay boundaries. Telemetry is an
        // external input here, so also reject a partial intersection with any
        // indivisible outbound group before sharing the coordinate checks.
        if manifest.plans.iter().any(|plan| {
            plan.outbound_group.is_some()
                && applied.effects_start < plan.effects_end
                && plan.effects_start < applied.effects_end
                && !(applied.effects_start <= plan.effects_start
                    && plan.effects_end <= applied.effects_end)
        }) {
            return Err(Gap::References);
        }
        let index = |value| usize::try_from(value).map_err(|_| Gap::References);
        let range = |start, end| Ok::<_, Gap>(index(start)?..index(end)?);
        let users = manifest
            .plans
            .iter()
            .map(|plan| {
                Ok((
                    plan.outbound_group.map(index).transpose()?,
                    range(plan.effects_start, plan.effects_end)?,
                ))
            })
            .collect::<Result<Vec<_>, Gap>>()?;
        eez_protocol::settlement_manifest::validate_replay_coordinates(
            *sync_height,
            &manifest.effects,
            &range(applied.effects_start, applied.effects_end)?,
            applied
                .endpoint
                .as_ref()
                .map(|endpoint| (endpoint.number, &endpoint.hash)),
            &range(applied.outbound_groups_start, applied.outbound_groups_end)?,
            users,
        )
        .map_err(|_| Gap::References)?;
    }
    match source.basis {
        SourceBasis::Receipt => {
            let receipt = source.receipt.ok_or(Gap::Source)?;
            if source.source_block != Some(receipt.block)
                || (source.direction == Direction::Inbound && receipt.block.number > anchor.number)
                || (*outcome == Outcome::SourceReverted) == receipt.success
                || (!receipt.success && !source.consumed.is_empty())
                || *outcome == Outcome::SourceNonceSpent
            {
                return Err(Gap::Source);
            }
        }
        SourceBasis::Nonce => {
            if *outcome != Outcome::SourceNonceSpent
                || source.receipt.is_some()
                || source.source_block.is_some()
                || !source.consumed.is_empty()
            {
                return Err(Gap::Source);
            }
        }
        SourceBasis::AppliedOutboundGroup | SourceBasis::CanonicalInclusion => {
            if source.direction != Direction::Outbound
                || *outcome != Outcome::Committed
                || source.source_block.is_none()
                || source.receipt.is_some()
                || !source.consumed.is_empty()
            {
                return Err(Gap::Source);
            }
            if source.basis == SourceBasis::AppliedOutboundGroup {
                let applied = details.applied.as_ref().ok_or(Gap::References)?;
                if source.source_block != applied.endpoint {
                    return Err(Gap::References);
                }
            }
        }
    }
    if *outcome == Outcome::Committed
        && (member.source.direction == Direction::Inbound
            || source.basis == SourceBasis::AppliedOutboundGroup)
    {
        let applied = details.applied.as_ref().ok_or(Gap::References)?;
        if applied.endpoint.is_none()
            || plan.effects_start == plan.effects_end
            || applied.effects_start > plan.effects_start
            || plan.effects_end > applied.effects_end
        {
            return Err(Gap::References);
        }
        if member.source.direction == Direction::Inbound {
            let receipt = source.receipt.ok_or(Gap::References)?;
            let consumed = source
                .consumed
                .iter()
                .map(|action| (action.call_hash, action.source_index))
                .collect::<HashSet<_>>();
            if receipt.block != applied.block
                || receipt.transaction_index <= applied.transaction_index
                || !plan
                    .source_actions
                    .iter()
                    .all(|action| consumed.contains(&(action.call_hash, action.source_index)))
            {
                return Err(Gap::References);
            }
        }
    }
    Ok(NativeDecision {
        member: *member,
        owner: *owner,
        outcome: *outcome,
        elapsed_ns: *elapsed_ns,
        l1_snapshot: *anchor,
        details,
        source,
        manifest,
        plan,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use eez_telemetry::schema::{
        Address20, AdmissionId, AppliedSettlement, Hash32, ReceiptRef, ReconciliationId,
        SourceAction, SourceResolution,
    };
    fn hash(byte: u8) -> Hash32 {
        Hash32([byte; 32])
    }
    fn block(number: u64, byte: u8) -> BlockRef {
        BlockRef {
            number,
            hash: hash(byte),
        }
    }
    fn epoch() -> Epoch {
        Epoch([1; 16])
    }
    struct Fixture {
        scopes: [ChainScope; 2],
        prepared: Event,
        parent: Event,
        terminal: Event,
        repriced: Vec<Event>,
    }
    impl Fixture {
        fn new() -> Self {
            let scopes = [
                ChainScope {
                    chain_id: 1,
                    genesis_hash: hash(1),
                    rollup_id: 0,
                },
                ChainScope {
                    chain_id: 2,
                    genesis_hash: hash(2),
                    rollup_id: 7,
                },
            ];
            let source = Source {
                chain_id: 1,
                genesis_hash: hash(1),
                rollup_id: 0,
                transaction_hash: hash(4),
                direction: Direction::Inbound,
            };
            let member = Member {
                source,
                admission: AdmissionId {
                    epoch: epoch(),
                    sequence: 1,
                },
            };
            let owner = AttemptId {
                epoch: epoch(),
                generation: 1,
                post_batch_hash: hash(5),
            };
            let action = SourceAction {
                call_hash: hash(8),
                source_index: 0,
            };
            let prepared = Event::AttemptPrepared {
                attempt: owner,
                sync_height: 3,
                members: vec![member],
                encoded_bytes: 42,
                blob_count: 1,
                blob_allowance: 1,
                details: Some(AttemptDetails {
                    preparation: None,
                    l2_chain_id: 2,
                    l2_genesis_hash: hash(2),
                    rollup_id: 7,
                    sync_hash: hash(32),
                    nonce: 0,
                    plans: vec![SourcePlan {
                        sender: Address20([3; 20]),
                        nonce: 3,
                        charged_attempts: 0,
                        outbound_group: None,
                        effects_start: 0,
                        effects_end: 1,
                        source_actions: vec![action],
                    }],
                    effects: vec![hash(32)],
                }),
            };
            let parent = Event::Reconciled {
                attempt: owner,
                anchor: block(12, 12),
                details: Some(Box::new(ReconciliationDetails {
                    l1: scopes[0],
                    l2: scopes[1],
                    l2_snapshot: Some(block(3, 33)),
                    post: Some(ReceiptRef {
                        block: block(11, 11),
                        transaction_index: 0,
                        success: true,
                    }),
                    applied: Some(AppliedSettlement {
                        block: block(11, 11),
                        transaction_hash: owner.post_batch_hash,
                        transaction_index: 0,
                        calldata_hash: hash(10),
                        effects_start: 0,
                        effects_end: 1,
                        anchor_applied: true,
                        endpoint: Some(block(3, 32)),
                        outbound_groups_start: 0,
                        outbound_groups_end: 0,
                        inbound_effects_start: 0,
                        inbound_effects_end: 1,
                    }),
                    sources: vec![ReconciledSource {
                        transaction_hash: source.transaction_hash,
                        direction: source.direction,
                        observed_nonce: 4,
                        source_block: Some(block(11, 11)),
                        basis: SourceBasis::Receipt,
                        receipt: Some(ReceiptRef {
                            block: block(11, 11),
                            transaction_index: 1,
                            success: true,
                        }),
                        consumed: vec![action],
                    }],
                    resolutions: vec![SourceResolution {
                        member,
                        owner,
                        disposition: ResolutionDisposition::Terminal {
                            outcome: Outcome::Committed,
                            assurance: Assurance::NodeReconciled,
                        },
                        elapsed_ns: 100,
                    }],
                })),
            };
            let terminal = Event::Terminal {
                member,
                attempt: Some(owner),
                outcome: Outcome::Committed,
                assurance: Assurance::NodeReconciled,
                anchor: None,
                elapsed_ns: 100,
                reconciliation: Some(ReconciliationId {
                    epoch: epoch(),
                    sequence: 10,
                }),
            };
            Self {
                scopes,
                prepared,
                parent,
                terminal,
                repriced: vec![],
            }
        }
        fn facts(&mut self) -> &mut ReconciliationDetails {
            let Event::Reconciled {
                details: Some(details),
                ..
            } = &mut self.parent
            else {
                panic!()
            };
            details
        }
        fn manifest(&mut self) -> &mut AttemptDetails {
            let Event::AttemptPrepared {
                details: Some(details),
                ..
            } = &mut self.prepared
            else {
                panic!()
            };
            details
        }
        fn outcome(&mut self, value: Outcome) {
            let Event::Terminal { outcome, .. } = &mut self.terminal else {
                panic!()
            };
            *outcome = value;
            self.facts().resolutions[0].disposition = ResolutionDisposition::Terminal {
                outcome: value,
                assurance: Assurance::NodeReconciled,
            };
        }
        fn outbound(&mut self, basis: SourceBasis) {
            let Event::Terminal { member, .. } = &mut self.terminal else {
                panic!()
            };
            member.source = Source {
                chain_id: 2,
                genesis_hash: hash(2),
                rollup_id: 7,
                transaction_hash: hash(4),
                direction: Direction::Outbound,
            };
            let member = *member;
            let Event::AttemptPrepared { members, .. } = &mut self.prepared else {
                panic!()
            };
            members[0] = member;
            self.manifest().plans[0].outbound_group = Some(0);
            let facts = self.facts();
            facts.resolutions[0].member = member;
            facts.applied.as_mut().unwrap().outbound_groups_end = 1;
            facts.applied.as_mut().unwrap().inbound_effects_end = 0;
            facts.sources[0].direction = Direction::Outbound;
            facts.sources[0].basis = basis;
            facts.sources[0].receipt = None;
            facts.sources[0].consumed.clear();
            facts.sources[0].source_block = Some(block(
                3,
                if basis == SourceBasis::AppliedOutboundGroup {
                    32
                } else {
                    33
                },
            ));
        }
        fn record<'a>(&self, event: &'a Event, sequence: u64) -> Record<'a> {
            Record {
                source_id: 1,
                epoch: epoch(),
                sequence,
                event,
            }
        }
        fn check(&self) -> Result<NativeDecision<'_>, Gap> {
            let mut lineage = vec![self.record(&self.prepared, 3)];
            lineage.extend(
                self.repriced
                    .iter()
                    .enumerate()
                    .map(|(i, event)| self.record(event, i as u64 + 4)),
            );
            native_decision(
                &self.scopes,
                self.record(&self.terminal, 10_001),
                self.record(&self.parent, 10),
                &lineage,
            )
        }
    }

    #[test]
    fn inclusion_and_applied_prefix_never_become_scan_or_proposal_hashes() {
        let mut fixture = Fixture::new();
        let decision = fixture.check().unwrap();
        assert_eq!(decision.l1_snapshot, block(12, 12));
        assert_eq!(decision.details.post.unwrap().block, block(11, 11));
        assert_eq!(decision.source.receipt.unwrap().transaction_index, 1);
        fixture.outbound(SourceBasis::AppliedOutboundGroup);
        // A second held source extends the proposed full block beyond the
        // actually applied prefix. The current snapshot can extend it again.
        let member = Member {
            source: Source {
                chain_id: 1,
                genesis_hash: hash(1),
                rollup_id: 0,
                transaction_hash: hash(14),
                direction: Direction::Inbound,
            },
            admission: AdmissionId {
                epoch: epoch(),
                sequence: 2,
            },
        };
        let Event::AttemptPrepared {
            members,
            details: Some(manifest),
            ..
        } = &mut fixture.prepared
        else {
            panic!()
        };
        members.push(member);
        manifest.sync_hash = hash(9);
        manifest.effects.push(hash(9));
        manifest.plans.push(SourcePlan {
            sender: Address20([6; 20]),
            nonce: 7,
            charged_attempts: 0,
            outbound_group: None,
            effects_start: 1,
            effects_end: 2,
            source_actions: vec![SourceAction {
                call_hash: hash(18),
                source_index: 1,
            }],
        });
        let facts = fixture.facts();
        facts.sources.push(ReconciledSource {
            transaction_hash: hash(14),
            direction: Direction::Inbound,
            observed_nonce: 7,
            source_block: None,
            basis: SourceBasis::Nonce,
            receipt: None,
            consumed: vec![],
        });
        facts.resolutions.push(SourceResolution {
            member,
            owner: facts.resolutions[0].owner,
            disposition: ResolutionDisposition::Requeued {
                charged_attempts: 0,
            },
            elapsed_ns: 100,
        });
        let decision = fixture.check().unwrap();
        assert_eq!(decision.source.source_block, Some(block(3, 32)));
        assert_eq!(decision.details.l2_snapshot, Some(block(3, 33)));
        assert_eq!(decision.manifest.sync_hash, hash(9));
        fixture.facts().sources[0].source_block = Some(block(3, 33));
        assert_eq!(fixture.check().unwrap_err(), Gap::References);
    }

    #[test]
    fn competing_canonical_inclusion_does_not_require_the_original_post_to_apply() {
        let mut fixture = Fixture::new();
        fixture.outbound(SourceBasis::CanonicalInclusion);
        fixture.facts().applied = None;
        fixture.facts().post = None;
        let decision = fixture.check().unwrap();
        assert_eq!(decision.outcome, Outcome::Committed);
        assert_eq!(decision.source.basis, SourceBasis::CanonicalInclusion);
        assert_eq!(decision.source.source_block, Some(block(3, 33)));
        // Canonical settlement/receipt checks are still required by the caller.
    }

    #[test]
    fn all_native_canonical_outcomes_keep_their_distinct_source_facts() {
        for outcome in [
            Outcome::SourceReverted,
            Outcome::SourceCompleted,
            Outcome::SourceCompletedWithMissingEffects,
            Outcome::SourceNonceSpent,
        ] {
            let mut fixture = Fixture::new();
            fixture.outcome(outcome);
            fixture.facts().applied = None;
            fixture.facts().post = None;
            let source = &mut fixture.facts().sources[0];
            if outcome == Outcome::SourceNonceSpent {
                source.basis = SourceBasis::Nonce;
                source.receipt = None;
                source.source_block = None;
                source.consumed.clear();
            }
            if outcome == Outcome::SourceReverted {
                source.receipt.as_mut().unwrap().success = false;
                source.consumed.clear();
            }
            assert_eq!(fixture.check().unwrap().outcome, outcome);
            fixture.facts().sources[0].observed_nonce = 3;
            assert_eq!(fixture.check().unwrap_err(), Gap::Source);
        }
    }

    #[test]
    fn untrusted_replay_coordinates_cannot_split_an_outbound_group() {
        let mut fixture = Fixture::new();
        fixture.outbound(SourceBasis::AppliedOutboundGroup);
        fixture.manifest().effects.push(hash(9));
        fixture.manifest().sync_hash = hash(9);
        fixture.manifest().plans[0].effects_end = 2;
        // The native shared check assumes a scanner-validated boundary. The
        // telemetry consumer must reject this illegal partial intersection even
        // when the supplied selection claims that no outbound group was chosen.
        fixture
            .facts()
            .applied
            .as_mut()
            .unwrap()
            .outbound_groups_end = 0;
        assert_eq!(fixture.check().unwrap_err(), Gap::References);
        fixture.facts().applied.as_mut().unwrap().effects_end = 2;
        fixture
            .facts()
            .applied
            .as_mut()
            .unwrap()
            .outbound_groups_end = 1;
        fixture.facts().applied.as_mut().unwrap().endpoint = Some(block(3, 9));
        fixture.facts().sources[0].source_block = Some(block(3, 9));
        assert!(fixture.check().is_ok());
        fixture.facts().applied.as_mut().unwrap().endpoint = Some(block(3, 32));
        fixture.facts().sources[0].source_block = Some(block(3, 32));
        assert_eq!(fixture.check().unwrap_err(), Gap::References);
    }

    #[test]
    fn own_batch_commit_requires_the_whole_source_and_its_receipt_consumption() {
        let mut fixture = Fixture::new();
        fixture.facts().sources[0].consumed.clear();
        assert_eq!(fixture.check().unwrap_err(), Gap::References);
        let mut fixture = Fixture::new();
        fixture.facts().sources[0]
            .receipt
            .as_mut()
            .unwrap()
            .transaction_index = 0;
        assert_eq!(fixture.check().unwrap_err(), Gap::References);
        let mut fixture = Fixture::new();
        fixture.outbound(SourceBasis::AppliedOutboundGroup);
        fixture.facts().applied.as_mut().unwrap().effects_end = 0;
        fixture
            .facts()
            .applied
            .as_mut()
            .unwrap()
            .outbound_groups_end = 0;
        assert_eq!(fixture.check().unwrap_err(), Gap::References);
    }

    #[test]
    fn repricing_requires_a_complete_ordered_same_generation_identity_chain() {
        let mut fixture = Fixture::new();
        let Event::AttemptPrepared { attempt: old, .. } = fixture.prepared else {
            panic!()
        };
        let replacement = AttemptId {
            post_batch_hash: hash(6),
            ..old
        };
        fixture.repriced.push(Event::AttemptRepriced {
            previous: old,
            replacement,
        });
        let Event::Terminal { attempt, .. } = &mut fixture.terminal else {
            panic!()
        };
        *attempt = Some(replacement);
        let Event::Reconciled { attempt, .. } = &mut fixture.parent else {
            panic!()
        };
        *attempt = replacement;
        fixture.facts().resolutions[0].owner = replacement;
        fixture.facts().applied.as_mut().unwrap().transaction_hash = replacement.post_batch_hash;
        assert_eq!(fixture.check().unwrap().owner, replacement);
        fixture.repriced.clear();
        assert_eq!(fixture.check().unwrap_err(), Gap::Lineage);
        fixture.repriced.push(Event::AttemptRepriced {
            previous: old,
            replacement: AttemptId {
                generation: 2,
                ..replacement
            },
        });
        assert_eq!(fixture.check().unwrap_err(), Gap::Lineage);
        fixture.repriced[0] = Event::AttemptRepriced {
            previous: old,
            replacement: old,
        };
        assert_eq!(fixture.check().unwrap_err(), Gap::Lineage);
    }

    #[test]
    fn an_earlier_signed_candidate_can_reappear_in_a_later_repricing_observation() {
        let mut fixture = Fixture::new();
        let Event::AttemptPrepared {
            attempt: original, ..
        } = fixture.prepared
        else {
            panic!()
        };
        let other = AttemptId {
            post_batch_hash: hash(6),
            ..original
        };
        fixture.repriced = vec![
            Event::AttemptRepriced {
                previous: original,
                replacement: other,
            },
            Event::AttemptRepriced {
                previous: other,
                replacement: original,
            },
        ];
        assert_eq!(fixture.check().unwrap().owner, original);
        fixture.repriced.swap(0, 1);
        assert_eq!(fixture.check().unwrap_err(), Gap::Lineage);
    }

    #[test]
    fn missing_and_ambiguous_membership_resolution_or_sources_never_choose_a_winner() {
        let mut fixture = Fixture::new();
        let duplicate = fixture.facts().resolutions[0];
        fixture.facts().resolutions.push(duplicate);
        assert_eq!(fixture.check().unwrap_err(), Gap::Resolution);
        fixture.facts().resolutions.pop();
        fixture.facts().resolutions[0].elapsed_ns += 1;
        assert_eq!(fixture.check().unwrap_err(), Gap::Resolution);
        let mut fixture = Fixture::new();
        let duplicate = fixture.facts().sources[0].clone();
        fixture.facts().sources.push(duplicate);
        assert_eq!(fixture.check().unwrap_err(), Gap::Source);
        let mut fixture = Fixture::new();
        let Event::AttemptPrepared {
            members,
            details: Some(details),
            ..
        } = &mut fixture.prepared
        else {
            panic!()
        };
        members.push(members[0]);
        details.plans.push(details.plans[0].clone());
        assert_eq!(fixture.check().unwrap_err(), Gap::MissingMembership);
        let mut fixture = Fixture::new();
        let Event::Reconciled { details, .. } = &mut fixture.parent else {
            panic!()
        };
        *details = None;
        assert_eq!(fixture.check().unwrap_err(), Gap::MissingParent);
    }

    #[test]
    fn scope_limits_and_explicit_assurance_fail_closed() {
        let mut fixture = Fixture::new();
        fixture.scopes[1].genesis_hash = hash(99);
        assert_eq!(fixture.check().unwrap_err(), Gap::Identity);
        let mut fixture = Fixture::new();
        let source = fixture.facts().sources[0].clone();
        fixture.facts().sources = vec![source; MAX_MEMBERS + 1];
        assert_eq!(fixture.check().unwrap_err(), Gap::Limit);
        let fixture = Fixture::new();
        let mut terminal = fixture.record(&fixture.terminal, 11);
        terminal.source_id = 2;
        assert_eq!(
            native_decision(
                &fixture.scopes,
                terminal,
                fixture.record(&fixture.parent, 10),
                &[fixture.record(&fixture.prepared, 2)]
            )
            .unwrap_err(),
            Gap::Identity
        );
        let mut fixture = Fixture::new();
        let Event::Terminal { assurance, .. } = &mut fixture.terminal else {
            panic!()
        };
        *assurance = Assurance::Unknown;
        assert_eq!(fixture.check().unwrap_err(), Gap::UnsupportedClaim);
    }

    #[test]
    fn source_and_applied_endpoint_cannot_be_after_the_pinned_l2_snapshot() {
        let mut fixture = Fixture::new();
        fixture.facts().l2_snapshot = Some(block(2, 31));
        assert_eq!(fixture.check().unwrap_err(), Gap::References);
        let mut fixture = Fixture::new();
        fixture.outbound(SourceBasis::CanonicalInclusion);
        fixture.facts().applied = None;
        fixture.facts().post = None;
        fixture.facts().sources[0].source_block = Some(block(4, 34));
        assert_eq!(fixture.check().unwrap_err(), Gap::References);
    }
}
