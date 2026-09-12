//! Canonical derivation evidence shared by the deriver and transaction recovery.

use std::ops::Range;

use alloy_primitives::B256;
use serde::{Deserialize, Serialize};

/// A canonical L1 block identity, never just a height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct L1BlockRef {
    pub number: u64,
    pub hash: B256,
}

/// The exact L2 endpoint validated by derivation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct L2Endpoint {
    pub number: u64,
    pub hash: B256,
}

/// Validated replay coordinates. Outbound groups are indivisible signed users;
/// inbound indices count producing deliveries, excluding static/lookup actions.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaySelection {
    pub outbound_groups: Range<usize>,
    pub inbound_effects: Range<usize>,
}

/// One successfully scanned and reconciled occurrence of a posted batch.
/// Empty `applied_effects` is conclusive; an absent record is inconclusive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconciledBatchSettlement {
    pub l1_block: L1BlockRef,
    pub tx_hash: B256,
    pub tx_index: u64,
    /// Hash of the exact postAndVerifyBatch calldata admitted by the scanner.
    pub calldata_hash: B256,
    /// Applied virtual producing slice, excluding the anchor.
    pub applied_effects: Range<usize>,
    pub anchor_applied: bool,
    /// None when this batch committed no L2 state.
    pub endpoint: Option<L2Endpoint>,
    pub selection: ReplaySelection,
}
