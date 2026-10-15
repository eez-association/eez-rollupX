//! Receipt observability consumes existing canonical attempt evidence. It
//! neither releases transaction ownership nor creates settlement urgency.

use std::collections::{BTreeMap, HashSet};

use alloy_consensus::{Signed, Transaction as _, TxEip4844WithSidecar};
use alloy_eips::{Decodable2718 as _, eip7594::BlobTransactionSidecarVariant};
use alloy_sol_types::SolCall as _;
use eez_l1::{
    L1BlockRef, L2Endpoint,
    posting_fees::{PostingCost, ReceiptCost},
};

use super::*;

const REFUND_STATUSES: [&str; 4] = ["qualified", "underfunded", "missing", "ambiguous"];
const REFUND_AMOUNTS: [&str; 3] = ["received", "posting_cost", "shortfall"];

pub(super) fn register_refund_metrics(rollup: u64, chain: u64, currency: &str) {
    metrics::describe_gauge!(
        "eez_composer_l1_refund_native",
        "L1 refund observations in native currency since process start; reorg-corrected, not durable accounting"
    );
    metrics::describe_gauge!(
        "eez_composer_l1_refund_reports",
        "L1 refund report counts since process start, corrected on replacement and reorg"
    );
    metrics::describe_counter!(
        "eez_composer_l1_refund_observation_errors_total",
        "Failed receipt-observation attempts while L1 refunds are enabled; retries count again"
    );
    let rollup = rollup.to_string();
    let chain = chain.to_string();
    for kind in REFUND_AMOUNTS {
        metrics::gauge!("eez_composer_l1_refund_native", "rollup_id" => rollup.clone(),
            "chain_id" => chain.clone(), "currency" => currency.to_owned(), "kind" => kind)
        .increment(0.0);
    }
    for status in REFUND_STATUSES {
        metrics::gauge!("eez_composer_l1_refund_reports", "rollup_id" => rollup.clone(),
            "chain_id" => chain.clone(), "currency" => currency.to_owned(), "status" => status)
        .increment(0.0);
    }
    metrics::counter!("eez_composer_l1_refund_observation_errors_total", "rollup_id" => rollup)
        .increment(0);
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Coverage {
    pub blocks: u64,
    pub transactions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PaidTransaction {
    hash: B256,
    block: L1BlockRef,
    transaction_index: u64,
    success: bool,
    cost: ReceiptCost,
}

impl PaidTransaction {
    fn from_source(
        hash: B256,
        source: eez_l1::submitter::CanonicalSourceState,
    ) -> Result<Option<Self>, String> {
        source
            .receipt
            .map(|receipt| {
                Ok(Self {
                    hash,
                    block: receipt.block,
                    transaction_index: receipt.transaction_index,
                    success: receipt.success,
                    cost: receipt.cost.map_err(|error| error.to_string())?,
                })
            })
            .transpose()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefundReport {
    plan: super::fees::refund::Plan,
    payment: eez_l1::refund::ObservedPayment,
    minimum: U256,
    qualified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CostReport {
    at: L1BlockRef,
    post: Option<PaidTransaction>,
    companions: Vec<PaidTransaction>,
    endpoint: Option<L2Endpoint>,
    coverage: Coverage,
    refund: Option<RefundReport>,
}

impl CostReport {
    /// Receipt observation may be delayed past a successful retry of the same
    /// signed source transaction. Only companions in this attempt's L1 block
    /// belong to its bundle. Otherwise a later retry would be charged twice.
    fn retain_bundle_companions(
        &mut self,
        dispatched_target: Option<(u64, u64)>,
        snapshot: &eez_l1::l1_canonical_head::SettlementSnapshot,
    ) {
        self.companions.retain(|companion| {
            if let Some(post) = &self.post {
                source_follows_postbatch(
                    companion.block,
                    companion.transaction_index,
                    post.block,
                    post.transaction_index,
                    snapshot,
                )
            } else {
                dispatched_target.is_some_and(|(block, _)| companion.block.number == block)
            }
        });
    }

    fn amounts(&self) -> Result<(PostingCost, PostingCost, PostingCost), String> {
        let post = self
            .post
            .as_ref()
            .map_or(PostingCost::default(), |tx| tx.cost.paid);
        let mut seen = HashSet::new();
        if let Some(tx) = &self.post {
            seen.insert(tx.hash);
        }
        let mut companions = PostingCost::default();
        for tx in &self.companions {
            if !seen.insert(tx.hash) {
                return Err("duplicate paid transaction in cost report".into());
            }
            companions = companions
                .checked_add(tx.cost.paid)
                .map_err(|error| error.to_string())?;
        }
        Ok((
            post,
            companions,
            post.checked_add(companions)
                .map_err(|error| error.to_string())?,
        ))
    }

    fn emit(
        &self,
        rollup_id: u64,
        hash: B256,
        chain_id: u64,
        currency: &str,
        action: &str,
    ) -> Result<(), String> {
        let (post, companions, bundle) = self.amounts()?;
        let (refund_status, refund_hash, refund_value) = match &self.refund {
            None => ("not_requested", None, U256::ZERO),
            Some(refund) => match refund.payment {
                eez_l1::refund::ObservedPayment::Missing => ("missing", None, U256::ZERO),
                eez_l1::refund::ObservedPayment::Ambiguous => ("ambiguous", None, U256::ZERO),
                eez_l1::refund::ObservedPayment::Confirmed { hash, value } => (
                    if refund.qualified {
                        "qualified"
                    } else {
                        "underfunded"
                    },
                    Some(hash),
                    value,
                ),
            },
        };
        let shortfall = self.refund.as_ref().map_or(U256::ZERO, |refund| {
            refund.minimum.saturating_sub(refund_value)
        });
        if self.refund.is_some() {
            // Only the idempotent ledger emits apply/retract pairs. Finalization
            // drops tracking without subtracting the observed contribution.
            // U256 fits in finite f64; approximation is for monitoring only.
            let sign = if action == "retract" { -1.0 } else { 1.0 };
            let rollup = rollup_id.to_string();
            let chain = chain_id.to_string();
            for (kind, amount) in
                REFUND_AMOUNTS
                    .into_iter()
                    .zip([refund_value, post.total, shortfall])
            {
                metrics::gauge!("eez_composer_l1_refund_native", "rollup_id" => rollup.clone(),
                    "chain_id" => chain.clone(), "currency" => currency.to_owned(), "kind" => kind)
                .increment(sign * f64::from(amount) / 1e18);
            }
            metrics::gauge!("eez_composer_l1_refund_reports", "rollup_id" => rollup,
                "chain_id" => chain, "currency" => currency.to_owned(), "status" => refund_status)
            .increment(sign);
        }
        let per_block =
            (self.coverage.blocks > 0).then(|| post.total / U256::from(self.coverage.blocks));
        let per_transaction = (self.coverage.transactions > 0)
            .then(|| post.total / U256::from(self.coverage.transactions));
        event!(name: "eez.composer.postbatch.actual_cost", Level::INFO,
            event_name = "eez.composer.postbatch.actual_cost", rollup_id, chain_id,
            native_currency = currency, unit = "native_base_units", action,
            post_batch_hash = %hash, canonical_sample_number = self.at.number,
            canonical_sample_hash = %self.at.hash,
            post_included = self.post.is_some(), post_success = ?self.post.as_ref().map(|tx| tx.success),
            post_execution_cost = %post.execution, post_blob_cost = %post.blobs, post_total_cost = %post.total,
            companion_execution_cost = %companions.execution, companion_blob_cost = %companions.blobs,
            companion_total_cost = %companions.total, bundle_total_cost = %bundle.total,
            post_gas_used = self.post.as_ref().map_or(0, |tx| tx.cost.gas_used),
            post_blob_gas_used = self.post.as_ref().map_or(0, |tx| tx.cost.blob_gas_used),
            settled_l2_blocks = self.coverage.blocks, included_l2_transactions = self.coverage.transactions,
            post_cost_per_l2_block = ?per_block, post_cost_per_l2_transaction = ?per_transaction,
            applied_endpoint = ?self.endpoint, companions = ?self.companions,
            refund_status, refund_hash = ?refund_hash, refund_value = %refund_value,
            refund_source = ?self.refund.as_ref().map(|r|r.plan.request.source()),
            refund_minimum = ?self.refund.as_ref().map(|r|r.minimum),
            refund_projected = ?self.refund.as_ref().map(|r|r.plan.projected),
            refund_shortfall = %shortfall,
            "canonical receipt charges; normalized integer costs round down and exclude zero denominators");
        Ok(())
    }
}

/// Only unfinalized observability records are retained. Reorg events retract
/// the exact previous report; repeated recovery reads cannot charge twice.
#[derive(Default)]
pub(super) struct CostLedger {
    reports: BTreeMap<(u64, B256), CostReport>,
    refund_plans: BTreeMap<(u64, B256), super::fees::refund::Plan>,
}

impl CostLedger {
    pub fn plan_refund(
        &mut self,
        rollup: u64,
        hash: B256,
        plan: super::fees::refund::Plan,
    ) -> Result<(), &'static str> {
        let key = (rollup, hash);
        if self
            .refund_plans
            .get(&key)
            .is_some_and(|previous| *previous != plan)
        {
            return Err("one post hash cannot acquire different refund terms");
        }
        self.refund_plans.insert(key, plan);
        Ok(())
    }

    fn validate(&self, key: (u64, B256), report: &CostReport) -> Result<(), String> {
        report.amounts()?;
        if let Some(RefundReport { payment: eez_l1::refund::ObservedPayment::Confirmed { hash: payment_hash, .. }, .. }) = &report.refund
            && self.reports.iter().any(|(other, report)| *other != key && report.refund.as_ref().is_some_and(|r|
                matches!(r.payment, eez_l1::refund::ObservedPayment::Confirmed { hash, .. } if hash == *payment_hash))) {
            return Err("refund payment is already attributed to another post".into());
        }
        Ok(())
    }

    pub fn record(
        &mut self,
        rollup: u64,
        hash: B256,
        chain: u64,
        currency: &str,
        report: CostReport,
    ) -> Result<(), String> {
        let key = (rollup, hash);
        self.validate(key, &report)?;
        if self.reports.get(&key) == Some(&report) {
            return Ok(());
        }
        if let Some(previous) = self.reports.get(&key) {
            previous.emit(rollup, hash, chain, currency, "retract")?;
        }
        report.emit(rollup, hash, chain, currency, "apply")?;
        self.reports.insert(key, report);
        Ok(())
    }

    pub fn invalidate_after(&mut self, ancestor: u64, chain: u64, currency: &str) {
        self.reports.retain(|&(rollup, hash), report| {
            if report.at.number <= ancestor {
                return true;
            }
            // A stored report already passed checked arithmetic on insertion.
            let _ = report.emit(rollup, hash, chain, currency, "retract");
            false
        });
    }

    pub fn forget_finalized(&mut self, rollup: u64, hash: B256) {
        self.reports.remove(&(rollup, hash));
        self.refund_plans.remove(&(rollup, hash));
    }
}

impl<L2> Composer<L2>
where
    L2: BlockReader<Header = alloy_consensus::Header>
        + StateProviderFactory
        + Send
        + Sync
        + 'static,
    <L2 as TransactionsProvider>::Transaction: Encodable2718,
{
    /// One outstanding cost observation per existing recovery tick. Failures
    /// retain the attempt for retry without blocking user recovery or posting.
    pub(super) async fn observe_settlement_cost(
        &self,
        rollup_id: u64,
        rollup: &RollupState<L2>,
    ) -> Result<(), String> {
        let posting = &self.inner.posting;
        let Some(attempt) = rollup.optimistic.pending_cost() else {
            return Ok(());
        };
        let snapshot = rollup.l1_head.settlement_snapshot();
        let at = snapshot
            .through
            .ok_or("canonical scan is incomplete for receipt accounting")?;
        let submitter = &self.inner.submitter;
        let post = PaidTransaction::from_source(
            attempt.id.post_batch_hash,
            submitter
                .source_state_at(attempt.id.post_batch_hash, submitter.poster_address(), at)
                .await
                .map_err(|error| error.to_string())?,
        )?;
        let mut companions = Vec::new();
        for tx in attempt
            .txs
            .iter()
            .filter(|tx| tx.direction == Direction::Inbound)
        {
            if let Some(paid) = PaidTransaction::from_source(
                tx.hash,
                submitter
                    .source_state_at(tx.hash, tx.sender, at)
                    .await
                    .map_err(|error| error.to_string())?,
            )? {
                companions.push(paid);
            }
        }
        let evidence = snapshot.settlements.get(&attempt.id.post_batch_hash);
        if let Some(evidence) = evidence {
            validate_attempt_evidence(&attempt, evidence)?;
            if post
                .as_ref()
                .is_none_or(|post| post.block != evidence.l1_block || !post.success)
            {
                return Err("cost receipt and applied settlement occurrence disagree".into());
            }
        }
        let endpoint = evidence.and_then(|evidence| evidence.endpoint);
        let coverage = if let Some(endpoint) = endpoint {
            let mut raw = attempt.post_batch.as_ref();
            let signed =
                Signed::<TxEip4844WithSidecar<BlobTransactionSidecarVariant>>::decode_2718(
                    &mut raw,
                )
                .map_err(|error| error.to_string())?;
            let call = eez_protocol::abi::postAndVerifyBatchCall::abi_decode(signed.tx().input())
                .map_err(|error| error.to_string())?;
            let start = call
                .batch
                .entries
                .first()
                .and_then(|entry| {
                    entry
                        .rootUpdates
                        .iter()
                        .find(|update| update.rollupId == rollup_id)
                })
                .ok_or("postBatch lacks an initial commitment for cost coverage")?
                .currentRoot;
            self.cost_coverage(rollup, start, endpoint)?
        } else {
            Coverage::default()
        };
        let plan = posting
            .costs
            .lock()
            .unwrap()
            .refund_plans
            .get(&(rollup_id, attempt.id.post_batch_hash))
            .copied();
        let refund = if let (Some(plan), Some(post)) = (plan, &post) {
            let source = companions.iter().find(|tx| {
                tx.hash == plan.request.source()
                    && tx.success
                    && source_follows_postbatch(
                        tx.block,
                        tx.transaction_index,
                        post.block,
                        post.transaction_index,
                        &snapshot,
                    )
            });
            let payment = if let Some(source) = source {
                submitter
                    .refund_payment_at(post.block, source.transaction_index, plan.request)
                    .await
                    .map_err(|e| e.to_string())?
            } else {
                eez_l1::refund::ObservedPayment::Missing
            };
            let minimum = posting
                .shadow_fee_policy
                .as_ref()
                .ok_or("refund cost policy missing")?
                .required_l1_payment(post.cost.paid.total)?
                .minimum_l1;
            let qualified = post.success
                && matches!(payment, eez_l1::refund::ObservedPayment::Confirmed { value, .. } if value >= minimum);
            Some(RefundReport {
                plan,
                payment,
                minimum,
                qualified,
            })
        } else {
            None
        };
        let _guard = self
            .inner
            .committer
            .begin_reconcile()
            .await
            .map_err(|error| error.to_string())?;
        if !rollup.l1_head.generation_is(snapshot.generation)
            || submitter
                .canonical_block_hash(at.number)
                .await
                .map_err(|error| error.to_string())?
                != Some(at.hash)
        {
            return Err("canonical state changed during receipt accounting".into());
        }
        let mut ledger = posting.costs.lock().unwrap();
        if !rollup.l1_head.generation_is(snapshot.generation) {
            return Err("canonical scan changed before receipt accounting".into());
        }
        let mut report = CostReport {
            at,
            post,
            companions,
            endpoint,
            coverage,
            refund,
        };
        report.retain_bundle_companions(attempt.dispatched_target, &snapshot);
        // Invalid attribution remains unresolved rather than double-crediting
        // revenue. Refund observations never gate quoting or source execution.
        ledger.validate((rollup_id, attempt.id.post_batch_hash), &report)?;
        rollup.optimistic.record_cost(attempt.id, at)?;
        ledger.record(
            rollup_id,
            attempt.id.post_batch_hash,
            posting.ctx.l1_chain_id,
            posting.native_currency(),
            report,
        )
    }

    fn cost_coverage(
        &self,
        rollup: &RollupState<L2>,
        start_hash: B256,
        endpoint: L2Endpoint,
    ) -> Result<Coverage, String> {
        let header = |hash: B256| {
            rollup
                .l2_provider
                .sealed_header_by_hash(hash)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("L2 cost-coverage header {hash} unavailable"))
        };
        let transactions = |hash: B256| -> Result<HashSet<B256>, String> {
            rollup
                .l2_provider
                .transactions_by_block(hash.into())
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("L2 cost-coverage transactions {hash} unavailable"))
                .map(|txs| {
                    txs.iter()
                        .map(|tx| alloy_primitives::keccak256(tx.encoded_2718()))
                        .collect()
                })
        };
        coverage_from_hashes(start_hash, endpoint, header, transactions)
    }
}

fn coverage_from_hashes(
    start_hash: B256,
    endpoint: L2Endpoint,
    mut header: impl FnMut(
        B256,
    ) -> Result<
        reth_primitives_traits::SealedHeader<alloy_consensus::Header>,
        String,
    >,
    mut transactions: impl FnMut(B256) -> Result<HashSet<B256>, String>,
) -> Result<Coverage, String> {
    let start = header(start_hash)?;
    let mut end = header(endpoint.hash)?;
    if end.number() != endpoint.number || end.number() < start.number() {
        return Err("cost coverage has inconsistent endpoint heights".into());
    }
    let mut coverage = Coverage::default();
    while end.number() > start.number() {
        coverage.blocks = coverage
            .blocks
            .checked_add(1)
            .ok_or("settled block count overflow")?;
        coverage.transactions = coverage
            .transactions
            .checked_add(
                u64::try_from(transactions(end.hash())?.len())
                    .map_err(|error| error.to_string())?,
            )
            .ok_or("settled transaction count overflow")?;
        let previous_number = end.number();
        end = header(end.parent_hash)?;
        if end.number().checked_add(1) != Some(previous_number) {
            return Err("L2 cost coverage has a broken parent chain".into());
        }
    }
    if end.hash() != start.hash() {
        // Partial application can extend a block at the same height. Count
        // only newly included transactions, using exact endpoint hashes.
        let previous = transactions(start.hash())?;
        let applied = transactions(end.hash())?;
        coverage.blocks = coverage
            .blocks
            .checked_add(1)
            .ok_or("settled block count overflow")?;
        coverage.transactions = coverage
            .transactions
            .checked_add(
                u64::try_from(applied.difference(&previous).count())
                    .map_err(|error| error.to_string())?,
            )
            .ok_or("settled transaction count overflow")?;
    }
    Ok(coverage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(number: u64, tag: u8) -> L1BlockRef {
        L1BlockRef {
            number,
            hash: B256::with_last_byte(tag),
        }
    }

    fn paid(tag: u8, gas: u64, blobs: u64, success: bool) -> PaidTransaction {
        let blob_gas_used = blobs * alloy_eips::eip4844::DATA_GAS_PER_BLOB;
        PaidTransaction {
            hash: B256::with_last_byte(tag),
            block: block(10, 1),
            transaction_index: u64::from(tag),
            success,
            cost: ReceiptCost {
                gas_used: gas,
                effective_gas_price: 2,
                blob_gas_used,
                blob_gas_price: 3,
                paid: PostingCost::calculate(gas, 2, blob_gas_used, 3).unwrap(),
            },
        }
    }

    #[test]
    fn receipt_reports_keep_post_companions_and_unincluded_costs_separate() {
        let mut report = CostReport {
            at: block(10, 1),
            post: Some(paid(1, 100, 2, false)),
            companions: vec![paid(2, 50, 1, false), paid(3, 60, 0, true)],
            endpoint: None,
            coverage: Coverage::default(),
            refund: None,
        };
        let (post, companions, total) = report.amounts().unwrap();
        assert_eq!(post.execution, U256::from(200));
        assert_eq!(companions.execution, U256::from(220));
        assert_eq!(
            total.blobs,
            U256::from(9 * alloy_eips::eip4844::DATA_GAS_PER_BLOB)
        );
        assert_eq!(total.total, post.total + companions.total);
        report.post = None;
        assert_eq!(report.amounts().unwrap().0, PostingCost::default());
        report.companions.clear();
        assert_eq!(report.amounts().unwrap().2, PostingCost::default());
        report.companions = vec![paid(1, 100, 0, true); 2];
        assert!(report.amounts().is_err());
    }

    #[test]
    fn delayed_receipt_observation_does_not_charge_a_later_retry_twice() {
        let snapshot = Default::default();
        let mut later_source = paid(2, 60, 0, true);
        later_source.block = block(11, 2);
        let mut earlier = CostReport {
            at: block(12, 3), // observation happens after both attempts
            post: Some(paid(1, 100, 1, true)),
            companions: vec![paid(3, 50, 0, false), later_source.clone()],
            endpoint: None,
            coverage: Coverage::default(),
            refund: None,
        };
        earlier.retain_bundle_companions(Some((10, 1000)), &snapshot);
        assert_eq!(earlier.companions.len(), 1);
        assert_eq!(earlier.amounts().unwrap().1.execution, U256::from(100));

        let mut retry_post = paid(4, 100, 1, true);
        retry_post.block = block(11, 2);
        retry_post.transaction_index = 1;
        let mut retry = CostReport {
            at: earlier.at,
            post: Some(retry_post),
            companions: vec![later_source.clone()],
            endpoint: None,
            coverage: Coverage::default(),
            refund: None,
        };
        retry.retain_bundle_companions(Some((11, 1005)), &snapshot);
        assert_eq!(retry.amounts().unwrap().1.execution, U256::from(120));
        assert_eq!(
            earlier.amounts().unwrap().1.execution + retry.amounts().unwrap().1.execution,
            U256::from(220),
            "each canonical source receipt is charged to its actual bundle once"
        );

        // A missing poster still permits charging actually included companions
        // at its recorded target; an unsent preparation has no bundle costs.
        earlier.post = None;
        earlier.companions.push(later_source);
        earlier.retain_bundle_companions(Some((10, 1000)), &snapshot);
        assert_eq!(earlier.amounts().unwrap().1.execution, U256::from(100));
        earlier.retain_bundle_companions(None, &snapshot);
        assert_eq!(earlier.amounts().unwrap().2, PostingCost::default());
    }

    #[test]
    fn companion_costs_follow_each_post_position_when_two_posts_share_one_l1_block() {
        let later = eez_l1::ReconciledBatchSettlement {
            l1_block: block(10, 1),
            tx_hash: B256::with_last_byte(3),
            tx_index: 3,
            calldata_hash: B256::ZERO,
            applied_effects: 0..1,
            anchor_applied: true,
            endpoint: None,
            selection: Default::default(),
        };
        let mut snapshot = eez_l1::l1_canonical_head::SettlementSnapshot::default();
        snapshot.settlements.insert(later.tx_hash, later);
        let sources = vec![paid(2, 50, 0, false), paid(4, 60, 0, true)];
        let mut first = CostReport {
            at: block(12, 3),
            post: Some(paid(1, 100, 1, true)),
            companions: sources.clone(),
            endpoint: None,
            coverage: Coverage::default(),
            refund: None,
        };
        let mut second = first.clone();
        second.post = Some(paid(3, 100, 1, true));
        first.retain_bundle_companions(Some((10, 1000)), &snapshot);
        second.retain_bundle_companions(Some((10, 1000)), &snapshot);
        assert_eq!(first.companions, sources[..1]);
        assert_eq!(second.companions, sources[1..]);
        assert_eq!(first.amounts().unwrap().1.execution, U256::from(100));
        assert_eq!(second.amounts().unwrap().1.execution, U256::from(120));
    }

    #[test]
    fn reorg_retracts_costs_and_same_height_replacement_is_accounted_once() {
        let hash = B256::with_last_byte(7);
        let original = CostReport {
            at: block(10, 1),
            post: Some(paid(1, 100, 1, true)),
            companions: vec![],
            endpoint: Some(L2Endpoint { number: 5, hash }),
            coverage: Coverage {
                blocks: 5,
                transactions: 3,
            },
            refund: None,
        };
        let mut ledger = CostLedger::default();
        for _ in 0..2 {
            ledger
                .record(1, hash, 10200, "xDAI", original.clone())
                .unwrap();
        }
        assert_eq!(ledger.reports.len(), 1);
        ledger.invalidate_after(10, 10200, "xDAI");
        assert_eq!(ledger.reports.len(), 1);
        ledger.invalidate_after(9, 10200, "xDAI");
        assert!(ledger.reports.is_empty());
        let mut replacement = original;
        replacement.at = block(10, 2);
        replacement.post.as_mut().unwrap().block = replacement.at;
        replacement.endpoint.as_mut().unwrap().hash = B256::with_last_byte(8);
        replacement.coverage.transactions = 1;
        ledger
            .record(1, hash, 10200, "xDAI", replacement.clone())
            .unwrap();
        assert_eq!(ledger.reports[&(1, hash)], replacement);
        ledger.forget_finalized(1, hash);
        assert!(ledger.reports.is_empty());
    }

    #[test]
    fn refund_reports_bind_terms_prevent_double_credit_and_retract_on_reorg() {
        let post = B256::repeat_byte(7);
        let payment = B256::repeat_byte(8);
        let plan = super::super::fees::refund::Plan {
            request: eez_l1::refund::BundleRefund::new(
                Address::repeat_byte(9),
                90,
                B256::repeat_byte(6),
            )
            .unwrap(),
            minimum: U256::from(700),
            projected: U256::from(900),
        };
        let report = CostReport {
            at: block(10, 1),
            post: Some(paid(1, 100, 0, true)),
            companions: vec![],
            endpoint: None,
            coverage: Coverage::default(),
            refund: Some(RefundReport {
                plan,
                payment: eez_l1::refund::ObservedPayment::Confirmed {
                    hash: payment,
                    value: U256::from(900),
                },
                minimum: plan.minimum,
                qualified: true,
            }),
        };
        let mut ledger = CostLedger::default();
        ledger.plan_refund(1, post, plan).unwrap();
        ledger.plan_refund(1, post, plan).unwrap();
        let mut different = plan;
        different.projected += U256::from(1);
        assert!(ledger.plan_refund(1, post, different).is_err());
        ledger.record(1, post, 1, "ETH", report.clone()).unwrap();
        ledger.record(1, post, 1, "ETH", report.clone()).unwrap();
        assert_eq!(ledger.reports.len(), 1);
        assert!(
            ledger
                .record(1, B256::repeat_byte(5), 1, "ETH", report.clone())
                .is_err()
        );
        ledger.invalidate_after(9, 1, "ETH");
        assert!(ledger.reports.is_empty());
        assert_eq!(
            ledger.refund_plans[&(1, post)],
            plan,
            "reorg does not invent new refund terms"
        );
        ledger.record(1, post, 1, "ETH", report).unwrap();
        ledger.forget_finalized(1, post);
        assert!(ledger.refund_plans.is_empty());
    }

    #[test]
    fn refund_metrics_correct_replacements_duplicates_reorgs_and_finalization() {
        use eez_l1::refund::ObservedPayment;
        use metrics_exporter_prometheus::PrometheusBuilder;
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let value = |family: &str, label: &str, rollup: u64| -> f64 {
            handle
                .render()
                .lines()
                .find(|line| {
                    line.starts_with(&format!("eez_composer_l1_refund_{family}{{"))
                        && line.contains(label)
                        && line.contains(&format!("rollup_id=\"{rollup}\""))
                })
                .unwrap()
                .rsplit_once(' ')
                .unwrap()
                .1
                .parse()
                .unwrap()
        };
        let close = |actual: f64, expected: f64| {
            assert!((actual - expected).abs() < 1e-28, "{actual} != {expected}");
        };
        metrics::with_local_recorder(&recorder, || {
            register_refund_metrics(1, 10200, "xDAI");
            register_refund_metrics(2, 10200, "xDAI");
            let hash = B256::repeat_byte(7);
            let plan = super::super::fees::refund::Plan {
                request: eez_l1::refund::BundleRefund::new(
                    Address::repeat_byte(9),
                    90,
                    B256::repeat_byte(6),
                )
                .unwrap(),
                minimum: U256::from(700),
                projected: U256::from(900),
            };
            let mut report = CostReport {
                at: block(10, 1),
                post: Some(paid(1, 100, 0, true)),
                companions: vec![],
                endpoint: None,
                coverage: Coverage::default(),
                refund: Some(RefundReport {
                    plan,
                    payment: ObservedPayment::Missing,
                    minimum: plan.minimum,
                    qualified: false,
                }),
            };
            let mut ledger = CostLedger::default();
            for _ in 0..2 {
                ledger
                    .record(1, hash, 10200, "xDAI", report.clone())
                    .unwrap();
            }
            close(value("reports", "status=\"missing\"", 1), 1.0);
            close(value("native", "kind=\"received\"", 1), 0.0);
            close(value("native", "kind=\"posting_cost\"", 1), 200e-18);
            close(value("native", "kind=\"shortfall\"", 1), 700e-18);
            for (payment, status, received) in [
                (ObservedPayment::Ambiguous, "ambiguous", 0),
                (
                    ObservedPayment::Confirmed {
                        hash: B256::repeat_byte(8),
                        value: U256::from(150),
                    },
                    "underfunded",
                    150,
                ),
                (
                    ObservedPayment::Confirmed {
                        hash: B256::repeat_byte(8),
                        value: U256::from(900),
                    },
                    "qualified",
                    900,
                ),
            ] {
                let refund = report.refund.as_mut().unwrap();
                refund.payment = payment;
                refund.qualified = received >= 700;
                ledger
                    .record(1, hash, 10200, "xDAI", report.clone())
                    .unwrap();
                close(
                    value("native", "kind=\"received\"", 1),
                    f64::from(received) / 1e18,
                );
                close(
                    value("native", "kind=\"shortfall\"", 1),
                    f64::from((700 - received).max(0)) / 1e18,
                );
                close(value("native", "kind=\"posting_cost\"", 1), 200e-18);
                for candidate in REFUND_STATUSES {
                    close(
                        value("reports", &format!("status=\"{candidate}\""), 1),
                        if candidate == status { 1.0 } else { 0.0 },
                    );
                }
            }
            assert!(
                ledger
                    .record(1, B256::repeat_byte(5), 10200, "xDAI", report.clone())
                    .is_err()
            );
            close(value("native", "kind=\"received\"", 1), 900e-18);
            ledger.invalidate_after(9, 10200, "xDAI");
            for kind in REFUND_AMOUNTS {
                close(value("native", &format!("kind=\"{kind}\""), 1), 0.0);
                close(value("native", &format!("kind=\"{kind}\""), 2), 0.0);
            }
            ledger.record(2, hash, 10200, "xDAI", report).unwrap();
            ledger.forget_finalized(2, hash);
            close(value("native", "kind=\"received\"", 2), 900e-18);
            close(value("reports", "status=\"qualified\"", 2), 1.0);
            register_refund_metrics(2, 10200, "xDAI");
            close(value("native", "kind=\"received\"", 2), 900e-18);
        });
        let text = handle.render();
        assert!(
            !text.contains("0x"),
            "transaction hashes must not be metric labels"
        );
    }

    #[test]
    fn normalization_uses_applied_hashes_and_only_new_transactions_at_partial_endpoints() {
        use alloy_consensus::Header;
        use reth_primitives_traits::SealedHeader;
        let old = SealedHeader::seal_slow(Header {
            number: 5,
            ..Default::default()
        });
        let replaced = SealedHeader::seal_slow(Header {
            number: 5,
            timestamp: 1,
            ..Default::default()
        });
        let next = SealedHeader::seal_slow(Header {
            number: 6,
            parent_hash: replaced.hash(),
            ..Default::default()
        });
        let tx = |tag| B256::with_last_byte(tag);
        let headers = [old.clone(), replaced.clone(), next.clone()]
            .into_iter()
            .map(|header| (header.hash(), header))
            .collect::<HashMap<_, _>>();
        let txs = HashMap::from([
            (old.hash(), HashSet::from([tx(1)])),
            (replaced.hash(), HashSet::from([tx(1), tx(2)])),
            (next.hash(), HashSet::from([tx(3), tx(4)])),
        ]);
        let coverage = |endpoint| {
            coverage_from_hashes(
                old.hash(),
                endpoint,
                |hash| headers.get(&hash).cloned().ok_or("missing header".into()),
                |hash| txs.get(&hash).cloned().ok_or("missing body".into()),
            )
        };
        assert_eq!(
            coverage(L2Endpoint {
                number: 5,
                hash: old.hash()
            })
            .unwrap(),
            Coverage::default()
        );
        assert_eq!(
            coverage(L2Endpoint {
                number: 5,
                hash: replaced.hash()
            })
            .unwrap(),
            Coverage {
                blocks: 1,
                transactions: 1
            }
        );
        assert_eq!(
            coverage(L2Endpoint {
                number: 6,
                hash: next.hash()
            })
            .unwrap(),
            Coverage {
                blocks: 2,
                transactions: 3
            }
        );
        assert!(
            coverage(L2Endpoint {
                number: 7,
                hash: next.hash()
            })
            .is_err()
        );
        assert!(
            coverage(L2Endpoint {
                number: 6,
                hash: B256::ZERO
            })
            .is_err()
        );
    }
}
