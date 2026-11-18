//! Reth-specific infrastructure for cross-chain composition.
//!
//! Per-transaction composition building lives in `eez-protocol`; the revm
//! inspector lives in `eez-evm-inspector`.
//! This module provides the reth-backed implementation of the
//! protocol traits the orchestrator drives:
//!
//! - [`LocalChainClient`] — unified chain client impl (entry or follower)
//! - `LocalExecutionSession` — stateful per-source-tx target session
//! - `slot` — slot-scoped execution contexts driving the real contract paths
//! - `ChainProvider` / `HeaderReader` — reth
//!   state access abstractions

pub(crate) mod build;
pub(crate) mod client;
pub mod gnosis_adapter;
pub(crate) mod provider;
pub(crate) mod session;
pub(crate) mod slot;
pub(crate) mod state;

#[doc(inline)]
pub use build::{
    BuildError, BuiltSyncBlock, build_sync_block, sync_block_hashes_at_positions,
    sync_block_pair_hashes,
};
#[doc(inline)]
pub use client::LocalChainClient;
#[doc(inline)]
pub use gnosis_adapter::GnosisL1Adapter;
#[doc(inline)]
pub use slot::LocalComposeClients;
#[doc(inline)]
pub use state::SimState;
// Slot execution contexts are driven only by `composer.rs`.
pub(crate) use slot::{InboundL2TargetSession, L1SlotState, L1TargetSession};
// `Role` + `LocalExecutionSession` are implementation details of
// `local.rs` and `session.rs`; not re-exported beyond the crate.
#[allow(unused_imports, reason = "future test/public consumers")]
pub(crate) use client::Role;
#[allow(unused_imports, reason = "future test/public consumers")]
pub(crate) use session::LocalExecutionSession;
