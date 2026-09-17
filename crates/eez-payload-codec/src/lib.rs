//! Chain-operation payload codecs for the new network.
//!
//! Shipping code accepts only the sparse derivable tag 3/profile 1 format via
//! [`derivable::Payload`]. Historical tags 0 through 2 are intentionally not
//! implemented.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod block_size;
pub use block_size::BlockDaSizer;

/// Derivable ordinary history for the new network.
pub mod derivable;
pub use derivable::DerivablePayloadSizer;
