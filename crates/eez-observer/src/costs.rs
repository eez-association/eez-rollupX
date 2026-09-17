//! Native membership supplies an allocation population, never a paid amount.
//! Canonical receipt and settlement-window checks are performed by SQL consumers.
use crate::outcomes::{AttemptMembership, Gap, Record, attempt_membership};
use eez_telemetry::schema::{
    AttemptId, BlockRef, ChainScope, Event, ReceiptRef, ReconciliationDetails,
};

#[derive(Debug)]
pub struct PaidAttempt<'a> {
    pub owner: AttemptId,
    pub snapshot: BlockRef,
    pub post: ReceiptRef,
    pub membership: AttemptMembership<'a>,
    pub reconciliation: &'a ReconciliationDetails,
}

/// Accept paid native attempts independently of source terminal outcomes. A
/// reverted post may requeue every member and still incur a real receipt charge.
/// Missing posting receipts cannot be replaced with dispatch target heights.
pub fn paid_attempt<'a>(
    scopes: &[ChainScope; 2],
    parent: Record<'a>,
    lineage: &[Record<'a>],
) -> Result<PaidAttempt<'a>, Gap> {
    let Event::Reconciled {
        attempt,
        anchor,
        details: Some(details),
    } = parent.event
    else {
        return Err(Gap::MissingParent);
    };
    if details.l1 != scopes[0] || details.l2 != scopes[1] {
        return Err(Gap::Identity);
    }
    let membership = attempt_membership(scopes, parent, *attempt, lineage)?;
    let post = details.post.ok_or(Gap::References)?;
    if post.block.number > anchor.number
        || details.sources.len() != membership.members.len()
        || membership
            .members
            .iter()
            .zip(&details.sources)
            .any(|(member, source)| {
                member.source.transaction_hash == attempt.post_batch_hash
                    || member.source.transaction_hash != source.transaction_hash
                    || member.source.direction != source.direction
                    || source.receipt.is_some_and(|receipt| {
                        source.source_block != Some(receipt.block)
                            || (source.direction == eez_telemetry::schema::Direction::Inbound
                                && receipt.block.number > anchor.number)
                    })
            })
    {
        return Err(Gap::References);
    }
    Ok(PaidAttempt {
        owner: *attempt,
        snapshot: *anchor,
        post,
        membership,
        reconciliation: details,
    })
}
