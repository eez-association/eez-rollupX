//! Osaka group/terminal cleanup on detached state; no state is accepted here.
use alloy_primitives::Address;
use eez_primitives::{EEZL2_ADDRESS, SYSTEM_ADDRESS};
use evm2::evm::{DbResult, DynDatabase, PendingState};

pub const PROTOCOL_BALANCE_ACCOUNTS: [Address; 2] = [EEZL2_ADDRESS, SYSTEM_ADDRESS];

/// Retain the transaction's original values, lifetime flags and storage changes while clearing
/// protocol balances. Unloaded accounts are read through the same witness database. Missing or
/// already-zero accounts remain untouched; an account made empty is deleted under Osaka rules.
/// Loaded deletion retains lifetime/read metadata and an explicit wipe; an unloaded deletion
/// has no transaction metadata to retain. Both represent storage destruction through `current == None`.
/// Discard the complete pending result on error. No bytecode execution, commit, log or gas charge occurs.
pub fn clear_protocol_balances(
    db: &mut dyn DynDatabase,
    pending: &mut PendingState,
) -> DbResult<()> {
    crate::reconstruction::clear_protocol_balances(db, pending)
}
