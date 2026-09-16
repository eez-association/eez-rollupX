//! Optional, best-effort lifecycle telemetry. No protocol state, global recorder,
//! subscriber, database, network client, or `RUST_LOG` dependency lives here.
//!
//! Allocate a [`Ticket`] at an authoritative transition under its existing
//! ownership. Move a bounded typed snapshot out of that critical section, then
//! call [`Publisher::publish`] to encode and attempt nonblocking delivery.
//! Delivery may reorder or lose records. It is never a durable admission journal.

#[cfg(feature = "native")]
pub mod native;
pub mod schema;
mod spool;
mod wire;

pub use spool::{Liveness, Publisher, SpoolConfig, StatsSnapshot, Ticket, Writer, probe_epoch};
pub use wire::{MAX_EVENT_BYTES, MAX_RECORD_BYTES, assemble, encode};

#[cfg(test)]
mod tests;
