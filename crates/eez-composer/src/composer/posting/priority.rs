//! Canonical posting-price samples that do not feed service-recovery tips back
//! into the next settlement bid. The cache is an optimization, never ownership.

use std::collections::{HashSet, VecDeque};

use alloy_primitives::{Address, B256};
use alloy_provider::{Provider, RootProvider};
use alloy_rpc_types_eth::{Block, TransactionReceipt};
use alloy_sol_types::SolEvent;
use eez_l1::posting_fees::{PRIORITY_FEE_HISTORY_BLOCKS, estimate_next_block_priority_fee};

use super::PostingError;

/// Covers the live 20-block window and recent advisory/reorg branches without
/// retaining receipts, transaction bodies or an unbounded canonical journal.
const MAX_CACHED_BLOCKS: usize = 64;

#[derive(Clone)]
struct Sample {
    hash: B256,
    parent: B256,
    number: u64,
    rewards: Vec<(u128, u64)>,
    gas: u64,
    excluded: usize,
}

impl Sample {
    fn reward(&self, percentile: u8) -> u128 {
        let threshold = u128::from(self.gas) * u128::from(percentile) / 100;
        let mut used = 0u128;
        for (tip, gas) in &self.rewards {
            used += u128::from(*gas);
            if used >= threshold {
                return *tip;
            }
        }
        0
    }
}

#[derive(Default)]
pub(in crate::composer) struct PriorityHistory {
    samples: VecDeque<Sample>,
}

impl PriorityHistory {
    /// Walk exact parent hashes from the caller's pinned head. Receipt reads
    /// are hash-bound, validated before caching, and shared under the runtime's
    /// async mutex. Cancelling the caller drops both its RPC and the guard.
    pub(super) async fn estimate(
        &mut self,
        provider: &RootProvider,
        head: Block,
        poster: Address,
        registry: Address,
        percentile: u8,
    ) -> Result<(u128, usize), PostingError> {
        let last = head.header.number;
        let first = last.saturating_sub(PRIORITY_FEE_HISTORY_BLOCKS - 1);
        let mut hash = head.header.hash;
        let mut supplied_head = Some(head);
        let mut rewards = Vec::with_capacity(PRIORITY_FEE_HISTORY_BLOCKS as usize);
        let mut excluded = 0;
        for number in (first..=last).rev() {
            let sample = if let Some(sample) = self.samples.iter().find(|s| s.hash == hash) {
                sample.clone()
            } else {
                let block = match supplied_head.take().filter(|b| b.header.hash == hash) {
                    Some(block) => block,
                    None => provider
                        .get_block_by_hash(hash)
                        .await
                        .map_err(|e| PostingError::Unavailable(format!("fee sample block: {e}")))?
                        .ok_or_else(|| {
                            PostingError::Unavailable("fee sample block missing".into())
                        })?,
                };
                if block.header.hash != hash || block.header.number != number {
                    return Err(PostingError::Unavailable(
                        "fee sample ancestry mismatch".into(),
                    ));
                }
                let receipts = provider
                    .get_block_receipts(hash.into())
                    .await
                    .map_err(|e| PostingError::Unavailable(format!("fee sample receipts: {e}")))?
                    .ok_or_else(|| {
                        PostingError::Unavailable("fee sample receipts missing".into())
                    })?;
                let mut sample = sample(&block, &receipts, poster, registry)?;
                // Caught reverts erase registry logs, including in a successful
                // source transaction. Inspect canonical call frames when a paid
                // contract execution is not already identified by its receipt.
                // Receipt gas alone cannot prove a plain transfer: execution
                // refunds can reduce a contract call to the intrinsic floor.
                if receipts.iter().any(|r| {
                    r.effective_gas_price
                        > u128::from(block.header.base_fee_per_gas.unwrap_or_default())
                        && r.from != poster
                        && !registry_log(r, registry)
                }) {
                    let traces: serde_json::Value = provider
                        .client()
                        .request(
                            "debug_traceBlockByHash",
                            (
                                hash,
                                serde_json::json!({"tracer":"callTracer","timeout":"4s"}),
                            ),
                        )
                        .await
                        .map_err(|e| {
                            PostingError::Unavailable(format!("fee sample call trace: {e}"))
                        })?;
                    let touched = registry_calls(&block, &traces, registry)?;
                    sample = sample_with_calls(&block, &receipts, poster, registry, &touched)?;
                }
                if self.samples.len() == MAX_CACHED_BLOCKS {
                    self.samples.pop_front();
                }
                self.samples.push_back(sample.clone());
                sample
            };
            if sample.number != number {
                return Err(PostingError::Unavailable(
                    "cached fee sample ancestry mismatch".into(),
                ));
            }
            rewards.push(vec![sample.reward(percentile)]);
            excluded += sample.excluded;
            hash = sample.parent;
        }
        // Selection only sorts the values; restore canonical order for the
        // shared range validator and to keep the input contract explicit.
        rewards.reverse();
        Ok((
            estimate_next_block_priority_fee(first, last, Some(&rewards), percentile)?,
            excluded,
        ))
    }
}

fn sample(
    block: &Block,
    receipts: &[TransactionReceipt],
    poster: Address,
    registry: Address,
) -> Result<Sample, PostingError> {
    sample_with_calls(block, receipts, poster, registry, &HashSet::new())
}

fn registry_log(receipt: &TransactionReceipt, registry: Address) -> bool {
    receipt.inner.logs().iter().any(|log| {
        log.address() == registry
            && matches!(log.topic0(), Some(topic)
            if *topic == eez_protocol::abi::BatchPosted::SIGNATURE_HASH
                || *topic == eez_protocol::abi::ExecutionConsumed::SIGNATURE_HASH
                || *topic == eez_protocol::abi::CrossChainCallExecuted::SIGNATURE_HASH)
    })
}

/// A reverted frame still identifies a service payment. Validate trace coverage
/// against the hash-bound body; successful receipts alone cannot identify it.
fn registry_calls(
    block: &Block,
    traces: &serde_json::Value,
    registry: Address,
) -> Result<HashSet<B256>, PostingError> {
    let invalid =
        || PostingError::Unavailable("incomplete or inconsistent fee sample call traces".into());
    let traces = traces.as_array().ok_or_else(invalid)?;
    if traces.len() != block.transactions.len() {
        return Err(invalid());
    }
    let mut result = HashSet::new();
    // Every nested EVM call costs at least 100 gas under the active warm-access
    // rules. Receipt gas can include the 20% refund; root frames need no call
    // opcode. This bounds traversal without rejecting refunded execution.
    let mut remaining = block.header.gas_used.saturating_mul(5) / 4 / 100 + traces.len() as u64;
    for (hash, trace) in block.transactions.hashes().zip(traces) {
        if trace.get("error").is_some_and(|e| !e.is_null())
            || trace
                .get("txHash")
                .and_then(|h| h.as_str())
                .and_then(|h| h.parse::<B256>().ok())
                != Some(hash)
        {
            return Err(invalid());
        }
        let mut frames = vec![trace.get("result").ok_or_else(invalid)?];
        while let Some(frame) = frames.pop() {
            remaining = remaining.checked_sub(1).ok_or_else(invalid)?;
            let kind = frame
                .get("type")
                .and_then(|t| t.as_str())
                .ok_or_else(invalid)?;
            if let Some(to) = frame.get("to").filter(|v| !v.is_null()) {
                let to = to
                    .as_str()
                    .and_then(|s| s.parse::<Address>().ok())
                    .ok_or_else(invalid)?;
                if to == registry
                    && matches!(kind, "CALL" | "STATICCALL" | "DELEGATECALL" | "CALLCODE")
                {
                    result.insert(hash);
                }
            }
            if let Some(calls) = frame.get("calls") {
                frames.extend(calls.as_array().ok_or_else(invalid)?);
            }
        }
    }
    Ok(result)
}

fn sample_with_calls(
    block: &Block,
    receipts: &[TransactionReceipt],
    poster: Address,
    registry: Address,
    registry_calls: &HashSet<B256>,
) -> Result<Sample, PostingError> {
    let header = &block.header;
    let invalid =
        || PostingError::Unavailable("incomplete or inconsistent fee sample receipts".into());
    if receipts.len() != block.transactions.len() {
        return Err(invalid());
    }
    let base = header.base_fee_per_gas.ok_or_else(invalid)?;
    let mut result = Sample {
        hash: header.hash,
        parent: header.parent_hash,
        number: header.number,
        rewards: Vec::new(),
        gas: 0,
        excluded: 0,
    };
    let mut cumulative = 0;
    for (index, (hash, receipt)) in block.transactions.hashes().zip(receipts).enumerate() {
        if receipt.block_hash != Some(header.hash)
            || receipt.block_number != Some(header.number)
            || receipt.transaction_index != Some(index as u64)
            || receipt.transaction_hash != hash
            || receipt.gas_used == 0
            || receipt.effective_gas_price < u128::from(base)
        {
            return Err(invalid());
        }
        cumulative = u64::checked_add(cumulative, receipt.gas_used).ok_or_else(invalid)?;
        if receipt.inner.cumulative_gas_used() != cumulative {
            return Err(invalid());
        }
        let service = receipt.from == poster
            || registry_log(receipt, registry)
            || registry_calls.contains(&receipt.transaction_hash);
        if service {
            result.excluded += 1;
        } else {
            result.gas = result
                .gas
                .checked_add(receipt.gas_used)
                .ok_or_else(invalid)?;
            result.rewards.push((
                receipt.effective_gas_price - u128::from(base),
                receipt.gas_used,
            ));
        }
    }
    if cumulative != header.gas_used {
        return Err(invalid());
    }
    result.rewards.sort_unstable_by_key(|(tip, _)| *tip);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_network::primitives::BlockTransactions;
    use alloy_provider::{ProviderBuilder, mock::Asserter};
    use serde_json::{Value, json};

    const POSTER: Address = alloy_primitives::address!("2ea2a5a1fe1ba4f4134c993e0c9a700706031e52");
    const REGISTRY: Address =
        alloy_primitives::address!("3c81284dae576d5152c9ca290668537618c44e5e");

    fn block(
        number: u64,
        hash: B256,
        parent: B256,
        rows: &[Value],
    ) -> (Block, Vec<TransactionReceipt>) {
        let mut block: Block = Block::default();
        block.header.hash = hash;
        block.header.inner.parent_hash = parent;
        block.header.inner.number = number;
        block.header.inner.base_fee_per_gas = Some(8);
        let mut cumulative = 0;
        let receipts: Vec<TransactionReceipt> = rows.iter().enumerate().map(|(i, r)| {
            let gas = r["gas"].as_u64().unwrap();
            cumulative += gas;
            let logs: Vec<_> = r["logs"].as_array().unwrap().iter().map(|l| json!({
                "address":l["address"], "topics":l["topics"], "data":"0x",
                "blockHash":hash, "blockNumber":format!("{number:#x}"), "removed":false,
                "transactionHash":r["hash"], "transactionIndex":format!("{i:#x}"), "logIndex":"0x0",
            })).collect();
            serde_json::from_value(json!({
                "type":"0x2", "status":"0x1", "cumulativeGasUsed":format!("{cumulative:#x}"),
                "logs":logs, "logsBloom":format!("0x{}","00".repeat(256)),
                "transactionHash":r["hash"], "transactionIndex":format!("{i:#x}"),
                "blockHash":hash, "blockNumber":format!("{number:#x}"),
                "gasUsed":format!("{gas:#x}"), "effectiveGasPrice":format!("{:#x}",r["price"].as_u64().unwrap()),
                "from":r["from"], "to":Address::repeat_byte(7), "contractAddress":null,
            })).unwrap()
        }).collect();
        block.header.inner.gas_used = cumulative;
        block.transactions =
            BlockTransactions::Hashes(receipts.iter().map(|r| r.transaction_hash).collect());
        (block, receipts)
    }

    fn row(id: u8, gas: u64, tip: u64, registry: Option<Address>) -> Value {
        json!({"hash":B256::repeat_byte(id), "from":Address::repeat_byte(id), "gas":gas,
        "price":tip+8, "logs":registry.map_or_else(Vec::new, |address| vec![json!({
            "address":address,"topics":[eez_protocol::abi::ExecutionConsumed::SIGNATURE_HASH]
        })])})
    }

    #[test]
    fn historical_refund_tips_raise_unfiltered_bid_but_not_external_bid() {
        // Canonical receipts/traces from Chiado 23544818..20, captured during
        // the incident review. Retain only fields used to classify payments;
        // calldata and proof payloads are unnecessary. Expected rewards were
        // independently read from eth_feeHistory, not computed by this code.
        let fixture: Vec<Value> =
            serde_json::from_str(include_str!("priority-fixture.json")).unwrap();
        let expected = [3_316_607_925, 4_460_534_753, 4_276_213_674];
        let mut raw = vec![vec![0]; 17];
        // Earlier canonical rewards from the retained eth_feeHistory response.
        // Leave them unchanged in the comparison; all are at/below the floor.
        raw[9][0] = 1_000_000_000;
        raw[13][0] = 1_000_000_000;
        raw[15][0] = 1;
        raw[16][0] = 1;
        let mut filtered = raw.clone();
        let mut exclusions = 0;
        for (b, reward) in fixture.iter().zip(expected) {
            let (mut block, receipts) = block(
                b["number"].as_u64().unwrap(),
                b["hash"].as_str().unwrap().parse().unwrap(),
                b["parentHash"].as_str().unwrap().parse().unwrap(),
                b["receipts"].as_array().unwrap(),
            );
            block.header.inner.base_fee_per_gas = Some(b["baseFee"].as_u64().unwrap());
            assert_eq!(block.header.gas_used, b["gasUsed"].as_u64().unwrap());
            let unfiltered = sample(&block, &receipts, Address::ZERO, Address::ZERO).unwrap();
            assert_eq!(unfiltered.reward(90), reward);
            let touched = registry_calls(&block, &b["traces"], REGISTRY).unwrap();
            let external =
                sample_with_calls(&block, &receipts, POSTER, REGISTRY, &touched).unwrap();
            assert_eq!(external.reward(90), 0);
            if block.header.number != 23544819 {
                // These two successful source receipts lost their registry
                // logs in a caught revert. Receipt-only filtering fails.
                assert!(
                    sample(&block, &receipts, POSTER, REGISTRY)
                        .unwrap()
                        .reward(90)
                        > 3_000_000_000
                );
            }
            exclusions += external.excluded;
            raw.push(vec![reward]);
            filtered.push(vec![external.reward(90)]);
        }
        assert_eq!(exclusions, 19); // Sixteen sources and three posts.
        let bid = |rewards: &[Vec<u128>]| {
            estimate_next_block_priority_fee(23544801, 23544820, Some(rewards), 90)
                .unwrap()
                .clamp(1_000_000_000, 2_000_000_000)
        };
        assert_eq!(bid(&raw), 2_000_000_000, "recorded baseline price spike");
        assert_eq!(bid(&filtered), 1_000_000_000, "no service-fee feedback");
    }

    #[test]
    fn external_transactions_in_service_blocks_still_price_the_bid() {
        let (block, receipts) = block(
            0,
            B256::repeat_byte(1),
            B256::ZERO,
            &[
                row(1, 900_000, 10_000_000_000, Some(REGISTRY)),
                row(2, 70_000, 2_000_000_000, None),
                row(3, 30_000, 3_000_000_000, None),
            ],
        );
        let s = sample(&block, &receipts, POSTER, REGISTRY).unwrap();
        assert_eq!(s.excluded, 1);
        assert_eq!(s.reward(50), 2_000_000_000);
        assert_eq!(s.reward(90), 3_000_000_000);
        let rewards = vec![vec![s.reward(90)]; 20];
        assert_eq!(
            estimate_next_block_priority_fee(0, 19, Some(&rewards), 90).unwrap(),
            3_000_000_000
        );
    }

    #[test]
    fn unrelated_contract_cannot_hide_its_tip_with_a_forged_event() {
        let (block, receipts) = block(
            0,
            B256::repeat_byte(1),
            B256::ZERO,
            &[row(1, 21_000, 5_000_000_000, Some(Address::repeat_byte(9)))],
        );
        let s = sample(&block, &receipts, POSTER, REGISTRY).unwrap();
        assert_eq!(s.excluded, 0);
        assert_eq!(s.reward(90), 5_000_000_000);
    }

    #[test]
    fn incomplete_mixed_or_mispriced_receipts_never_become_samples() {
        let (block, receipts) = block(
            0,
            B256::repeat_byte(1),
            B256::ZERO,
            &[row(1, 21_000, 10, None)],
        );
        assert!(sample(&block, &[], POSTER, REGISTRY).is_err());
        for change in 0..6 {
            let mut bad = receipts.clone();
            match change {
                0 => bad[0].block_hash = Some(B256::repeat_byte(2)),
                1 => bad[0].block_number = Some(99),
                2 => bad[0].transaction_hash = B256::repeat_byte(3),
                3 => bad[0].transaction_index = Some(1),
                4 => bad[0].gas_used = 20_999,
                _ => bad[0].effective_gas_price = 7,
            }
            assert!(
                sample(&block, &bad, POSTER, REGISTRY).is_err(),
                "mutation {change}"
            );
        }
    }

    fn trace(hash: B256, to: Address) -> Value {
        json!({"txHash":hash, "result":{"type":"CALL", "to":to}})
    }

    #[test]
    fn reverted_registry_calls_count_but_selfdestruct_beneficiaries_do_not() {
        let (block, _) = block(
            0,
            B256::repeat_byte(1),
            B256::ZERO,
            &[row(1, 21_000, 10, None), row(2, 40_000, 20, None)],
        );
        let traces = json!([
            {"txHash":B256::repeat_byte(1), "result":{"type":"CALL", "to":Address::repeat_byte(7),
              "calls":[{"type":"CALL", "to":REGISTRY, "error":"execution reverted"}]}},
            {"txHash":B256::repeat_byte(2), "result":{"type":"SELFDESTRUCT", "to":REGISTRY}}
        ]);
        assert_eq!(
            registry_calls(&block, &traces, REGISTRY).unwrap(),
            HashSet::from([B256::repeat_byte(1)])
        );
        for mutation in 0..6 {
            let mut bad = traces.clone();
            match mutation {
                0 => {
                    bad.as_array_mut().unwrap().pop();
                }
                1 => bad[0]["txHash"] = json!(B256::repeat_byte(3)),
                2 => bad[0]["error"] = json!("timeout"),
                3 => bad[0]["result"]["calls"] = json!({}),
                4 => bad[0]["result"]["to"] = json!("bad address"),
                _ => bad[0]["result"] = Value::Null,
            }
            assert!(
                registry_calls(&block, &bad, REGISTRY).is_err(),
                "mutation {mutation}"
            );
        }
    }

    #[tokio::test]
    async fn unidentified_payments_require_complete_traces_before_caching() {
        let responses = Asserter::new();
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .connect_mocked_client(responses.clone());
        let (head, receipts) = block(
            0,
            B256::repeat_byte(1),
            B256::ZERO,
            &[row(1, 21_000, 10, None), row(2, 40_000, 20, None)],
        );
        let mut cache = PriorityHistory::default();
        responses.push_success(&Some(&receipts));
        responses.push_success(&json!([trace(B256::repeat_byte(1), REGISTRY)]));
        assert!(
            cache
                .estimate(&provider, head.clone(), POSTER, REGISTRY, 90)
                .await
                .is_err()
        );
        assert!(cache.samples.is_empty());
        responses.push_success(&Some(&receipts));
        responses.push_success(&json!([
            trace(B256::repeat_byte(1), REGISTRY),
            trace(B256::repeat_byte(2), Address::repeat_byte(7))
        ]));
        assert_eq!(
            cache
                .estimate(&provider, head.clone(), POSTER, REGISTRY, 90)
                .await
                .unwrap(),
            (20, 1)
        );
        assert_eq!(
            cache
                .estimate(&provider, head, POSTER, REGISTRY, 90)
                .await
                .unwrap(),
            (20, 1)
        );
        assert!(responses.read_q().is_empty());
    }

    #[tokio::test]
    async fn cache_is_hash_bound_bounded_and_reconstructible_after_restart() {
        let responses = Asserter::new();
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .connect_mocked_client(responses.clone());
        let mut cache = PriorityHistory::default();
        let mut parent = B256::ZERO;
        for number in 0..70 {
            let hash = B256::with_last_byte(number as u8 + 1);
            let (head, receipts) = block(number, hash, parent, &[]);
            responses.push_success(&Some(receipts));
            assert_eq!(
                cache
                    .estimate(&provider, head.clone(), POSTER, REGISTRY, 90)
                    .await
                    .unwrap(),
                (0, 0)
            );
            // A warm query (including a different percentile) needs no RPC.
            assert_eq!(
                cache
                    .estimate(&provider, head, POSTER, REGISTRY, 50)
                    .await
                    .unwrap(),
                (0, 0)
            );
            assert!(responses.read_q().is_empty());
            parent = hash;
        }
        assert_eq!(cache.samples.len(), MAX_CACHED_BLOCKS);
        let (replacement, receipts) = block(
            69,
            B256::repeat_byte(99),
            B256::with_last_byte(69),
            &[row(1, 21_000, 30, None)],
        );
        responses.push_success(&Some(receipts));
        responses.push_success(&json!([trace(
            B256::repeat_byte(1),
            Address::repeat_byte(7)
        )]));
        cache
            .estimate(&provider, replacement, POSTER, REGISTRY, 90)
            .await
            .unwrap();
        assert!(
            cache
                .samples
                .iter()
                .any(|s| s.hash == B256::repeat_byte(99))
        );
        assert!(
            cache
                .samples
                .iter()
                .any(|s| s.hash == B256::with_last_byte(70))
        );
        let (genesis, receipts) = block(
            0,
            B256::repeat_byte(88),
            B256::ZERO,
            &[row(1, 21_000, 30, None)],
        );
        let mut cold = PriorityHistory::default();
        responses.push_success(&Some(receipts));
        responses.push_success(&json!([trace(
            B256::repeat_byte(1),
            Address::repeat_byte(7)
        )]));
        assert_eq!(
            cold.estimate(&provider, genesis, POSTER, REGISTRY, 90)
                .await
                .unwrap(),
            (30, 0)
        );
    }

    #[tokio::test]
    async fn missing_receipts_fail_without_caching_or_falling_back() {
        let responses = Asserter::new();
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .connect_mocked_client(responses.clone());
        let (head, _) = block(0, B256::repeat_byte(1), B256::ZERO, &[]);
        responses.push_success(&Option::<Vec<TransactionReceipt>>::None);
        let mut cache = PriorityHistory::default();
        let error = cache
            .estimate(&provider, head, POSTER, REGISTRY, 90)
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("receipts missing"));
        assert!(cache.samples.is_empty());
        assert!(responses.read_q().is_empty());
    }
}
