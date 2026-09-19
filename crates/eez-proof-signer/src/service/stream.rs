//! Conversion of one client-streaming RPC body into an admitted window.

use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eez_control_rpc::v1::{CacheMiss, ProveChunk, ProveFailure, prove_failure};
use tokio::time::timeout;
use tonic::{Status, Streaming};
use tracing::{Span, debug};

use crate::window;

/// Drain one stream into a complete, structurally admitted [`window::AdmittedWindow`].
///
/// Chunks are checked incrementally for ordering, quotas, adjacency between
/// streamed hash claims, and expected rollup identity. This phase does not execute
/// blocks or validate settlement calldata. `idle_timeout` bounds every wait for
/// a message or stream EOF; the caller enforces the end-to-end deadline. EOF is
/// accepted only after the complete declared span.
pub(super) async fn read_window(
    mut stream: Streaming<ProveChunk>,
    expected_rollup_id: NonZeroU64,
    expected_validation_identity: alloy_primitives::B256,
    prepared_cache: Arc<Mutex<crate::prepared_cache::PreparedCache>>,
    limits: window::WindowLimits,
    idle_timeout: Duration,
) -> Result<window::AdmittedWindow, Status> {
    let first = next_message(&mut stream, idle_timeout)
        .await?
        .ok_or_else(|| window_status(window::WindowError::EmptyStream))?;
    let supplied_identity = match &first.kind {
        Some(eez_control_rpc::v1::prove_chunk::Kind::Header(header)) => {
            alloy_primitives::B256::try_from(header.expected_validation_identity.as_slice()).ok()
        }
        _ => None,
    };
    let assembler = window::WindowAssembler::start(limits, first).map_err(window_status)?;
    let claimed_rollup_id = assembler.claimed_rollup_id();
    let header = assembler.header();
    // The request span exists before the streamed header is available. Record
    // the Composer-declared fields now, before comparing its rollup claim with
    // operator configuration, so a mismatch remains diagnosable.
    let request_span = Span::current();
    request_span.record("wire_rollup_id", claimed_rollup_id);
    request_span.record("declared_from_block", header.declared_from_block);
    request_span.record("declared_to_block", header.declared_to_block);
    request_span.record(
        "declared_blocks",
        header.declared_to_block - header.declared_from_block + 1,
    );
    let declared_to_block = header.declared_to_block;
    let mut assembler = assembler
        .verify_rollup_identity(expected_rollup_id)
        .map_err(|_| Status::failed_precondition("window rollup identity rejected"))?;
    let supplied_identity = supplied_identity.ok_or_else(|| {
        Status::invalid_argument("expected validation identity must contain 32 bytes")
    })?;
    if supplied_identity != expected_validation_identity {
        let mut status =
            Status::failed_precondition("validation identity changed or differs from local rules");
        status.metadata_mut().insert(
            "eez-capabilities-changed",
            "1".parse().expect("static ASCII metadata"),
        );
        return Err(status);
    }
    debug!(phase = "ingestion", "window header accepted");

    while let Some(chunk) = next_message(&mut stream, idle_timeout).await? {
        let encoded_len = prost::Message::encoded_len(&chunk);
        match chunk.kind {
            Some(eez_control_rpc::v1::prove_chunk::Kind::CachedRange(range)) => {
                let identity =
                    alloy_primitives::B256::try_from(range.validation_identity.as_slice())
                        .map_err(|_| {
                            Status::invalid_argument("cached range identity must contain 32 bytes")
                        })?;
                if identity != expected_validation_identity {
                    return Err(Status::failed_precondition(
                        "cached range validation identity rejected",
                    ));
                }
                let first_hash = alloy_primitives::B256::try_from(range.first_hash.as_slice())
                    .map_err(|_| {
                        Status::invalid_argument("cached range first hash must contain 32 bytes")
                    })?;
                let last_hash = alloy_primitives::B256::try_from(range.last_hash.as_slice())
                    .map_err(|_| {
                        Status::invalid_argument("cached range last hash must contain 32 bytes")
                    })?;
                if range.last_number >= declared_to_block {
                    return Err(Status::invalid_argument(
                        "terminal block must carry a fresh witness",
                    ));
                }
                let cache = Arc::clone(&prepared_cache);
                let first_number = range.first_number;
                let last_number = range.last_number;
                let facts = tokio::task::spawn_blocking(move || {
                    cache
                        .lock()
                        .map_err(|_| Status::internal("prepared cache lock poisoned"))?
                        .resolve_range(identity, first_number, first_hash, last_number, last_hash)
                        .map_err(|error| cache_status(identity, error))
                })
                .await
                .map_err(|_| Status::internal("prepared cache resolver failed"))??;
                debug!(
                    phase = "ingestion",
                    cache_hit_blocks = facts.len(),
                    first_number,
                    last_number,
                    "resolved cached ordinary range"
                );
                assembler
                    .push_cached(encoded_len, facts)
                    .map_err(window_status)?;
            }
            kind => assembler.push(ProveChunk { kind }).map_err(window_status)?,
        }
    }
    assembler.finish().map_err(window_status)
}

fn cache_status(
    identity: alloy_primitives::B256,
    error: crate::prepared_cache::CacheError,
) -> Status {
    if let crate::prepared_cache::CacheError::Miss { number, hash } = error {
        let failure = ProveFailure {
            actionable_failure: Some(prove_failure::ActionableFailure::CacheMiss(CacheMiss {
                validation_identity: identity.to_vec(),
                block_number: number,
                block_hash: hash.to_vec(),
            })),
        };
        Status::with_details(
            tonic::Code::FailedPrecondition,
            "cached ordinary block is unavailable",
            eez_control_rpc::encode_prove_failure(&failure).into(),
        )
    } else {
        Status::invalid_argument(format!("cached ordinary range: {error}"))
    }
}

/// Read one frame with an idle timeout and normalize decode-limit failures.
async fn next_message(
    stream: &mut Streaming<ProveChunk>,
    idle_timeout: Duration,
) -> Result<Option<ProveChunk>, Status> {
    timeout(idle_timeout, stream.message())
        .await
        .map_err(|_| Status::deadline_exceeded("Prove stream idle timeout"))?
        .map_err(|error| {
            if error.code() == tonic::Code::OutOfRange {
                Status::resource_exhausted("Prove message exceeds decoding limit")
            } else {
                error
            }
        })
}

/// Distinguish local capacity refusals from malformed stream structure.
fn window_status(error: window::WindowError) -> Status {
    if error.is_resource_exhausted() {
        Status::resource_exhausted(format!("window quota: {error}"))
    } else {
        Status::invalid_argument(format!("window: {error}"))
    }
}
