//! Canonical direct V0 ChainOperation payloads for the fresh RollupX network.
//! Syntax validation does not authorize settlement or establish execution.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod block_size;
pub use block_size::BlockDaSizer;

/// Borrowed columnar operations, count tokens and exact producer sizing.
pub mod v0;
