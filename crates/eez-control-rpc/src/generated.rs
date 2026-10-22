//! Tonic-generated modules for `prove.v1` and the shared `prove.v2` contract.
//!
//! The v1 module contains:
//! * `ProveChunk`, `ProveHeader`, `PostBatch`, `BlockWitness`,
//!   `ExecutionWitness`, `ProveResponse` message structs.
//! * `prover_server::{Prover, ProverServer}` — the async trait
//!   `eez-proof-signer` implements and its tonic service adapter.
//! * `prover_client::ProverClient` — the composer side (`eez-prover-client`).
pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/prove.v1.rs"));
}

pub mod v2 {
    include!(concat!(env!("OUT_DIR"), "/prove.v2.rs"));
}
