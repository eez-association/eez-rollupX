//! Optional, best-effort lifecycle telemetry. No protocol state, global recorder,
//! subscriber or database is installed by the library. Native signal exporters
//! are explicit optional features; lifecycle delivery never depends on `RUST_LOG`.
//!
//! Allocate a [`Ticket`] at an authoritative transition under its existing
//! ownership. Move a bounded typed snapshot out of that critical section, then
//! call [`Publisher::publish`] to encode and attempt nonblocking delivery.
//! Delivery may reorder or lose records. It is never a durable admission journal.

#[cfg(feature = "native")]
pub mod native;
pub mod operational;
pub mod reader;
pub mod schema;
mod spool;
#[cfg(feature = "traces")]
pub mod traces;
mod wire;

pub use spool::{
    CaptureFailure, Liveness, Publisher, SpoolConfig, StatsSnapshot, Ticket, Writer, probe_epoch,
};
pub use wire::{MAX_EVENT_BYTES, MAX_RECORD_BYTES, assemble, encode, validate_chunk};

#[cfg(test)]
mod tests;
