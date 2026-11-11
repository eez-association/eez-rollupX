//! Bounded diagnostics for a cross-chain source evicted because composing it
//! failed deterministically. They are observations for the status RPC only:
//! never consulted for admission, ownership or recovery, and discarded on
//! restart with the rest of the queue telemetry.

use alloy_primitives::{Address, Bytes};
use eez_protocol::{CallMode, ExecutedAction, ExecutionOutcome};
use serde_json::{Value, json};

/// Cross-chain calls kept per rejection; later calls are only counted.
pub const MAX_REJECTION_CALLS: usize = 16;
/// Bytes kept of the source output and of each call's return data.
pub const MAX_REJECTION_BYTES: usize = 512;

/// Why the Composer rejected a source, with the simulated call tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRejection {
    /// Display form of the composition error that triggered the eviction.
    pub error: String,
    /// The source simulation outcome, when it ran to completion.
    pub source: Option<SourceFailure>,
    /// Recorded cross-chain calls in preorder, at most [`MAX_REJECTION_CALLS`].
    pub calls: Vec<CallDiagnostic>,
    /// Recorded calls beyond the bound.
    pub omitted_calls: usize,
}

/// Outcome of the failed source simulation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFailure {
    pub gas_used: u64,
    /// Revert data (or other output), truncated to [`MAX_REJECTION_BYTES`].
    pub output: Bytes,
    /// Length of the untruncated output.
    pub output_len: usize,
}

/// One recorded cross-chain call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallDiagnostic {
    /// 1 for a call made directly by the source transaction.
    pub depth: usize,
    pub source_rollup_id: u64,
    pub target_rollup_id: u64,
    pub caller: Address,
    /// The original target address the proxy represents.
    pub target: Address,
    /// First four calldata bytes, when present.
    pub selector: Option<Bytes>,
    pub static_call: bool,
    /// `None` while the call had not returned when composition stopped.
    pub success: Option<bool>,
    pub gas_used: Option<u64>,
    /// Return or revert data, truncated to [`MAX_REJECTION_BYTES`].
    pub return_data: Bytes,
    pub return_len: usize,
}

fn truncated(data: &[u8]) -> Bytes {
    Bytes::copy_from_slice(&data[..data.len().min(MAX_REJECTION_BYTES)])
}

impl SourceRejection {
    /// Diagnostic for an error that carries no simulation detail.
    pub fn from_error(error: impl std::fmt::Display) -> Self {
        Self {
            error: error.to_string(),
            source: None,
            calls: Vec::new(),
            omitted_calls: 0,
        }
    }

    /// Diagnostic for a source whose simulation completed unsuccessfully.
    pub fn from_simulation(
        error: impl std::fmt::Display,
        gas_used: u64,
        output: &[u8],
        recorded: &[ExecutedAction],
    ) -> Self {
        let mut depths = Vec::with_capacity(recorded.len());
        let calls = recorded
            .iter()
            .enumerate()
            .map(|(index, action)| {
                // Preorder: a parent always precedes its children, so its
                // depth is already known. A malformed edge counts as a root.
                let depth = action
                    .parent_index
                    .filter(|parent| *parent < index)
                    .map_or(1, |parent| depths[parent] + 1);
                depths.push(depth);
                (depth, action)
            })
            .take(MAX_REJECTION_CALLS)
            .map(|(depth, action)| {
                let (success, gas_used, data) = match &action.outcome {
                    ExecutionOutcome::Pending => (None, None, &[][..]),
                    ExecutionOutcome::Resolved {
                        return_data,
                        gas_used,
                        success,
                    } => (Some(*success), Some(*gas_used), return_data.as_slice()),
                };
                CallDiagnostic {
                    depth,
                    source_rollup_id: action.source_rollup_id.0,
                    target_rollup_id: action.target_rollup_id.0,
                    caller: action.source_address,
                    target: action.target_address,
                    selector: (action.data.len() >= 4)
                        .then(|| Bytes::copy_from_slice(&action.data[..4])),
                    static_call: action.call_mode == CallMode::Static,
                    success,
                    gas_used,
                    return_data: truncated(data),
                    return_len: data.len(),
                }
            })
            .collect();
        Self {
            error: error.to_string(),
            source: Some(SourceFailure {
                gas_used,
                output: truncated(output),
                output_len: output.len(),
            }),
            calls,
            omitted_calls: recorded.len().saturating_sub(MAX_REJECTION_CALLS),
        }
    }

    /// Replace the error text with the error that actually caused eviction.
    #[must_use]
    pub fn with_error(mut self, error: impl std::fmt::Display) -> Self {
        self.error = error.to_string();
        self
    }

    pub fn json(&self) -> Value {
        json!({
            "error": self.error,
            "source": self.source.as_ref().map(|source| json!({
                "gasUsed": source.gas_used,
                "output": source.output,
                "outputLength": source.output_len,
            })),
            "calls": self.calls.iter().map(|call| json!({
                "depth": call.depth,
                "sourceRollupId": call.source_rollup_id,
                "targetRollupId": call.target_rollup_id,
                "caller": call.caller,
                "target": call.target,
                "selector": call.selector,
                "static": call.static_call,
                "success": call.success,
                "gasUsed": call.gas_used,
                "returnData": call.return_data,
                "returnLength": call.return_len,
            })).collect::<Vec<_>>(),
            "omittedCalls": self.omitted_calls,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use eez_protocol::RollupId;

    fn action(
        parent: Option<usize>,
        source: u64,
        target: u64,
        outcome: ExecutionOutcome,
    ) -> ExecutedAction {
        ExecutedAction {
            parent_index: parent,
            call_mode: CallMode::Mutable,
            target_address: Address::repeat_byte(target as u8 + 0x10),
            target_rollup_id: RollupId(target),
            source_rollup_id: RollupId(source),
            source_address: Address::repeat_byte(source as u8 + 0x20),
            data: Bytes::from_static(&[0x61, 0x46, 0x19, 0x54, 0xff]),
            value: U256::ZERO,
            outcome,
            revert_spans: vec![],
        }
    }

    fn resolved(success: bool, data: Vec<u8>) -> ExecutionOutcome {
        ExecutionOutcome::Resolved {
            return_data: data,
            gas_used: 1_000,
            success,
        }
    }

    #[test]
    fn nested_round_trip_keeps_depths_outcomes_and_the_source_revert() {
        // L2 source -> L1 root that fails after two nested legs succeeded.
        let recorded = vec![
            action(None, 1, 0, resolved(false, vec![0xc5, 0xca, 0x42, 0x2b])),
            action(Some(0), 0, 1, resolved(true, vec![])),
            action(Some(1), 1, 0, resolved(true, vec![0x01])),
        ];
        let rejection = SourceRejection::from_simulation(
            "source transaction does not execute successfully",
            24_228,
            &[0xc5, 0xca, 0x42, 0x2b],
            &recorded,
        );
        assert_eq!(
            rejection.calls.iter().map(|c| c.depth).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(rejection.calls[0].success, Some(false));
        assert_eq!(
            rejection.calls[0].return_data.as_ref(),
            [0xc5, 0xca, 0x42, 0x2b]
        );
        assert_eq!(
            rejection.calls[0].selector.as_ref().map(|s| s.to_vec()),
            Some(vec![0x61, 0x46, 0x19, 0x54])
        );
        assert_eq!(rejection.calls[2].source_rollup_id, 1);
        let source = rejection.source.as_ref().unwrap();
        assert_eq!((source.gas_used, source.output_len), (24_228, 4));
        let json = rejection.json();
        assert_eq!(json["source"]["output"], "0xc5ca422b");
        assert_eq!(json["calls"][0]["returnData"], "0xc5ca422b");
        assert_eq!(json["calls"][1]["depth"], 2);
        assert_eq!(json["omittedCalls"], 0);
    }

    #[test]
    fn data_and_call_lists_are_bounded() {
        let long = vec![0xab; MAX_REJECTION_BYTES * 3];
        let recorded: Vec<_> = (0..MAX_REJECTION_CALLS + 5)
            .map(|_| action(None, 1, 0, resolved(false, long.clone())))
            .collect();
        let rejection = SourceRejection::from_simulation("failed", 1, &long, &recorded);
        assert_eq!(rejection.calls.len(), MAX_REJECTION_CALLS);
        assert_eq!(rejection.omitted_calls, 5);
        assert_eq!(rejection.calls[0].return_data.len(), MAX_REJECTION_BYTES);
        assert_eq!(rejection.calls[0].return_len, long.len());
        let source = rejection.source.unwrap();
        assert_eq!(
            (source.output.len(), source.output_len),
            (MAX_REJECTION_BYTES, long.len())
        );
    }

    #[test]
    fn pending_calls_short_calldata_and_bad_edges_are_reported_conservatively() {
        let mut short = action(Some(7), 1, 0, ExecutionOutcome::Pending);
        short.data = Bytes::from_static(&[0x01]);
        let rejection = SourceRejection::from_simulation("failed", 0, &[], &[short]);
        let call = &rejection.calls[0];
        assert_eq!(
            (
                call.depth,
                call.success,
                call.gas_used,
                call.selector.clone()
            ),
            (1, None, None, None)
        );
        let error_only = SourceRejection::from_error("protocol: no calls").json();
        assert_eq!(error_only["source"], Value::Null);
        assert_eq!(error_only["calls"], json!([]));
    }
}
