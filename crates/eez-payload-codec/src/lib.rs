//! Chain-operation payload codecs for the new network.
//!
//! Shipping code accepts only the RollupX V0 tag 0/profile 1 format via
//! [`derivable::Payload`]. Historical formats (including earlier uses of tag 0)
//! are intentionally unsupported.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod block_size;
pub use block_size::BlockDaSizer;

/// Derivable ordinary history for the new network.
pub mod derivable;
pub use derivable::DerivablePayloadSizer;
