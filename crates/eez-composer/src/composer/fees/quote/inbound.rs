//! L1-origin quotes use the same private composition as gas estimation, then
//! execute the native blob post and source at the actual proposed fee fields.
use super::{
    Arc, BlockReader, Budget, Composer, FeeQuoteError, Header, MAX_ITERATIONS, PreparedL2FeeQuote,
    StateProviderFactory, Transaction, U256, Value, compact, history_size, json, keccak256,
    required_tip, unavailable,
};
use crate::composer::estimate::InboundPricing;
use eez_l1::L1BlockRef;

impl<L2> Composer<L2>
where
    L2: BlockReader<Block = eez_primitives::Block, Header = Header>
        + StateProviderFactory
        + Send
        + Sync
        + 'static,
{
    pub(super) fn quote_l1_fee_in_context(
        &self,
        prepared: &PreparedL2FeeQuote,
        snapshot: &super::super::super::estimate::Snapshot,
        budget: &Arc<Budget>,
    ) -> Result<Value, FeeQuoteError> {
        let runtime = &self.inner.posting;
        let refund = runtime
            .collection
            .l1_refund
            .as_ref()
            .ok_or(FeeQuoteError::Disabled)?;
        let policy = runtime
            .collection
            .pricing
            .as_ref()
            .ok_or(FeeQuoteError::Disabled)?;
        let posting = &prepared.posting;
        let request = &prepared.request;
        let provider = runtime
            .chain
            .provider
            .state_by_block_hash(posting.quote.sample.hash)
            .map_err(|e| unavailable("L1 quote state", e))?;
        if provider
            .account_code(&refund.recipient)
            .map_err(|e| unavailable("refund recipient", e))?
            .is_some_and(|code| !code.is_empty())
        {
            return Err(FeeQuoteError::Unsupported(
                "refund recipient must be a plain EOA",
            ));
        }
        let rollup = &self.inner.rollups[&prepared.rollup_id];
        self.inner
            .emission
            .checked_window_span(prepared.endpoint.number, snapshot.l2_parent.number + 1)
            .map_err(|e| unavailable("settlement history capacity", e))?;
        let operations = history_size(
            rollup.l2_provider.as_ref(),
            &snapshot.l2_parent,
            prepared.endpoint,
            self.inner.fee_recipient,
            self.inner.emission.max_blocks.saturating_sub(1),
            budget,
        )?;
        let maximum_blobs = self
            .remaining_posting_blob_allowance(posting.quote.target, 0)
            .map_err(|e| unavailable("blob allowance", e))?;
        let estimator = self
            .gas_estimator(prepared.rollup_id)
            .map_err(compact::estimate_error)?;
        let base = u128::from(posting.env.block_env.basefee);
        let mut tip = if request.signed_fees {
            request
                .tx
                .max_priority_fee_per_gas()
                .unwrap_or_else(|| request.tx.max_fee_per_gas())
        } else {
            0
        };
        for iteration in 1..=MAX_ITERATIONS {
            budget.check()?;
            let cap = if request.signed_fees {
                request.tx.max_fee_per_gas()
            } else {
                request.quoted_fee_cap(base, tip)?
            };
            let effective_tip = request
                .env(tip, cap)
                .gas_priority_fee
                .map_or(cap.saturating_sub(base), |tip| {
                    tip.min(cap.saturating_sub(base))
                });
            if cap < base {
                return Err(FeeQuoteError::SourceFeeCap {
                    cap,
                    base,
                    chain_id: runtime.ctx.l1_chain_id,
                });
            }
            let replay = estimator
                .priced_inbound(
                    snapshot,
                    request.env(tip, cap),
                    budget.deadline,
                    InboundPricing {
                        runtime,
                        posting,
                        operations,
                        maximum_blobs,
                        budget: budget.clone(),
                    },
                )
                .map_err(|error| compact::estimate_error_at("fee_simulation", error))?;
            budget.check()?;
            let (post_gas, blobs, da_bytes) = replay
                .settlement
                .ok_or(FeeQuoteError::Unavailable("L1 settlement receipt"))?;
            let cost = posting
                .quote
                .estimate(post_gas, blobs as u64)
                .map_err(|e| unavailable("L1 settlement cost", e))?;
            let payment = policy
                .required_l1_payment(cost.total)
                .map_err(FeeQuoteError::Limit)?;
            let required = if request.signed_fees {
                payment.minimum_l1
            } else {
                payment.l1
            };
            let gross_required = refund
                .gross(required, posting.quote)
                .map_err(FeeQuoteError::Limit)?;
            let next_tip = required_tip(gross_required, replay.source_gas)?;
            if next_tip > effective_tip {
                if request.signed_fees {
                    return Err(FeeQuoteError::L1Underfunded(Box::new(
                        crate::FundingDiagnostic::measured(
                            posting.quote,
                            L1BlockRef {
                                number: snapshot.l2_parent.number,
                                hash: snapshot.l2_parent.hash(),
                            },
                            runtime.ctx.l1_chain_id,
                            gross_required,
                            U256::from(effective_tip) * U256::from(replay.source_gas),
                            replay.source_gas,
                            base,
                        ),
                    )));
                }
                tip = next_tip;
                continue;
            }
            let transaction = request.fields(tip, cap);
            let mut fields = transaction.clone();
            // `fields` is constructed by the shared strict transaction encoder.
            let object = fields
                .as_object_mut()
                .expect("constructed transaction object");
            for key in ["gasPrice", "maxFeePerGas", "maxPriorityFeePerGas"] {
                object.remove(key);
            }
            let gross = U256::from(effective_tip) * U256::from(replay.source_gas);
            return Ok(json!({
                "version":1,"mode":"advisory","transaction":transaction,"request":fields,
                "requestHash":keccak256(fields.to_string()),"expiresAt":prepared.expires_at,
                "policy":policy.details(),
                "context":{"l1ParentHash":posting.quote.sample.hash,"l2ParentHash":snapshot.l2_parent.hash(),
                    "settledL2Hash":prepared.endpoint.hash,"rollupId":prepared.rollup_id},
                "estimate":{"basis":"isolated_native_l1_post_and_source","proof":"metered_assumed_valid",
                    "iterations":iteration,"sourceGasUsed":replay.source_gas,"l1Gas":post_gas,
                    "blobs":blobs,"daBytes":da_bytes,"l1BaseFeePerGas":base.to_string(),
                    "l1PriorityFeePerGas":posting.quote.max_priority_fee_per_gas.to_string(),
                    "l1ExecutionPrice":posting.quote.estimated_effective_gas_price.to_string(),
                    "l1BlobPrice":posting.quote.estimated_blob_gas_price.to_string(),
                    "l1ExecutionCost":cost.execution.to_string(),"l1BlobCost":cost.blobs.to_string(),
                    "l1PostingCost":cost.total.to_string(),"l1Subtotal":payment.subtotal.to_string(),
                    "l1Risk":payment.risk.to_string(),"l1Profit":payment.profit.to_string(),
                    "requiredL1":payment.l1.to_string(),"minimumL1":payment.minimum_l1.to_string(),
                    "grossL1Tip":gross.to_string(),"refundPercent":refund.percent,"refundRecipient":refund.recipient,
                    "payoutCostL1":refund.payout_cost(posting.quote).to_string(),
                    "projectedRefundL1":refund.net(gross,posting.quote).map_err(FeeQuoteError::Limit)?.to_string(),
                    "qualifiedRecoveryL1":null}
            }));
        }
        Err(FeeQuoteError::Limit(
            "fee-dependent execution did not converge in four simulations",
        ))
    }
}
