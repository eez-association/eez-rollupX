//! Bounded ordinary-block preparation RPC.

use std::sync::Arc;

use alloy_primitives::B256;
use eez_control_rpc::v1::{PrepareChunk, PrepareResponse, prepare_chunk};
use eez_control_rpc::v1::{RetainOrdinaryRangesRequest, RetainOrdinaryRangesResponse};
use prost::Message as _;
use tokio::time::{Instant, timeout, timeout_at};
use tonic::{Code, Request, Response, Status, Streaming};
use tracing::info;

use super::ProveSvc;
use crate::cancel::CancellationToken;
use crate::window::{AdmittedBlock, AdmittedBlocks};

pub(crate) const MAX_PREPARATION_BLOCKS: u64 = 64;

pub(super) async fn prepare(
    service: &ProveSvc,
    request: Request<Streaming<PrepareChunk>>,
) -> Result<Response<PrepareResponse>, Status> {
    let _preparation_slot = Arc::clone(&service.preparation_slot)
        .try_acquire_owned()
        .map_err(|_| Status::unavailable("another preparation stream is active"))?;
    let mut stream = request.into_inner();
    let idle = service.limits.stream_idle_timeout();
    let deadline = Instant::now()
        .checked_add(service.limits.request_timeout())
        .ok_or_else(|| Status::internal("preparation timeout is out of range"))?;
    let first = next(&mut stream, idle)
        .await?
        .ok_or_else(|| Status::invalid_argument("empty preparation stream"))?;
    let mut bytes = first.encoded_len();
    let Some(prepare_chunk::Kind::Header(header)) = first.kind else {
        return Err(Status::invalid_argument("preparation header must be first"));
    };
    if header.rollup_id != service.state.expected_rollup_id.get() {
        return Err(Status::failed_precondition(
            "preparation rollup identity rejected",
        ));
    }
    let identity = B256::try_from(header.expected_validation_identity.as_slice())
        .map_err(|_| Status::invalid_argument("preparation identity must contain 32 bytes"))?;
    if identity != service.state.validation_identity {
        return Err(Status::failed_precondition(
            "preparation validation identity rejected",
        ));
    }
    let requested_lease = if header.lease_id.is_empty() {
        None
    } else {
        Some(B256::try_from(header.lease_id.as_slice()).map_err(|_| {
            Status::invalid_argument("preparation lease id must be empty or 32 bytes")
        })?)
    };
    let count = header
        .to_block
        .checked_sub(header.from_block)
        .and_then(|distance| distance.checked_add(1))
        .ok_or_else(|| Status::invalid_argument("invalid preparation bounds"))?;
    if header.from_block == 0 || count == 0 || count > MAX_PREPARATION_BLOCKS {
        return Err(Status::invalid_argument(
            "preparation range must contain 1..=64 blocks",
        ));
    }

    let limits = service.limits.window_limits();
    let mut items = 0usize;
    let mut admitted: Vec<AdmittedBlock> = Vec::with_capacity(count as usize);
    while let Some(chunk) = next(&mut stream, idle).await? {
        bytes = bytes
            .checked_add(chunk.encoded_len())
            .ok_or_else(|| Status::resource_exhausted("preparation byte accounting overflow"))?;
        if bytes > limits.payload_bytes {
            return Err(Status::resource_exhausted(
                "preparation byte quota exceeded",
            ));
        }
        let wire = match chunk.kind {
            Some(prepare_chunk::Kind::Block(block)) => block,
            Some(prepare_chunk::Kind::Header(_)) => {
                return Err(Status::invalid_argument("duplicate preparation header"));
            }
            None => return Err(Status::invalid_argument("preparation chunk has no kind")),
        };
        let expected = header.from_block + admitted.len() as u64;
        if wire.number != expected || wire.number > header.to_block {
            return Err(Status::invalid_argument(
                "preparation block number is out of order",
            ));
        }
        let block = AdmittedBlock::from_wire(wire, &mut items, limits).map_err(|error| {
            Status::new(
                if error.is_resource_exhausted() {
                    Code::ResourceExhausted
                } else {
                    Code::InvalidArgument
                },
                format!("preparation block: {error}"),
            )
        })?;
        if let Some(previous) = admitted.last()
            && block.claimed_parent_hash != previous.claimed_hash
        {
            return Err(Status::invalid_argument(
                "preparation hash chain is discontinuous",
            ));
        }
        admitted.push(block);
    }
    if admitted.len() != count as usize {
        return Err(Status::invalid_argument(
            "preparation stream ended before its declared range",
        ));
    }

    let cancellation = CancellationToken::default();
    let mut prepared = Vec::with_capacity(admitted.len());
    // One block per permit is intentional: an admitted final proof gets the
    // worker at the next block boundary and cannot be starved by a 64-block prep.
    for block in admitted {
        let permit = timeout_at(
            deadline,
            Arc::clone(&service.validation_worker).acquire_owned(),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("preparation deadline exceeded"))?
        .map_err(|_| Status::internal("validation worker closed"))?;
        let state = Arc::clone(&service.state);
        let cancellation = cancellation.clone();
        let span = tracing::Span::current();
        let work = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _entered = span.enter();
            // The timer belongs to the real worker, including when its RPC
            // await expires. Preparation cannot enter authorized-response metrics.
            let mut timer = eez_telemetry::operational::StageTimer::start(
                eez_telemetry::operational::Stage::SignerValidation,
            );
            let result = state
                .validator
                .prepare_ordinary(
                    AdmittedBlocks::from_preparation(block),
                    identity,
                    &cancellation,
                )
                .map_err(super::settlement_job::PipelineError::from);
            timer.finish(super::rpc::stage_outcome(
                result
                    .as_ref()
                    .err()
                    .map(super::settlement_job::PipelineError::status)
                    .as_ref(),
            ));
            result
        });
        let facts = timeout_at(deadline, work)
            .await
            .map_err(|_| Status::deadline_exceeded("preparation deadline exceeded"))?
            .map_err(|_| Status::internal("preparation worker failed"))?
            .map_err(|error| {
                Status::failed_precondition(format!("ordinary preparation rejected: {error}"))
            })?;
        prepared.extend(facts);
        tokio::task::yield_now().await;
    }

    let first = prepared
        .first()
        .ok_or_else(|| Status::internal("prepared range is empty"))?;
    let last = prepared.last().expect("nonempty prepared range");
    let endpoints = (
        first.number(),
        first.block_hash(),
        last.number(),
        last.block_hash(),
    );
    let lease_lifetime = service.limits.lease_lifetime();
    let cache = Arc::clone(&service.prepared_cache);
    let (lease_id, published_count, cache_entries, cache_bytes, active_leases) =
        tokio::task::spawn_blocking(move || {
            let mut cache = cache
                .lock()
                .map_err(|_| Status::internal("prepared cache lock poisoned"))?;
            let lease_id = cache
                .publish_with_lease(prepared, requested_lease, identity, lease_lifetime)
                .map_err(|error| Status::resource_exhausted(format!("prepared cache: {error}")))?;
            let published_count = cache
                .resolve_range(identity, endpoints.0, endpoints.1, endpoints.2, endpoints.3)
                .map_err(|error| Status::internal(format!("prepared cache publication: {error}")))?
                .len();
            let (entries, bytes) = cache.usage();
            let leases = cache.lease_count();
            Ok::<_, Status>((lease_id, published_count, entries, bytes, leases))
        })
        .await
        .map_err(|_| Status::internal("prepared cache worker failed"))??;
    let response = PrepareResponse {
        validation_identity: identity.to_vec(),
        first_number: endpoints.0,
        first_hash: endpoints.1.to_vec(),
        last_number: endpoints.2,
        last_hash: endpoints.3.to_vec(),
        prepared_count: published_count as u64,
        lease_id: lease_id.to_vec(),
    };
    info!(
        event_name = "eez.proof_signer.ordinary_prepared",
        first_number = endpoints.0,
        last_number = endpoints.2,
        prepared_blocks = published_count,
        cache_entries,
        cache_bytes,
        active_leases,
        "ordinary range validated and retained"
    );
    Ok(Response::new(response))
}

pub(super) async fn retain(
    service: &ProveSvc,
    request: RetainOrdinaryRangesRequest,
) -> Result<Response<RetainOrdinaryRangesResponse>, Status> {
    let cache = Arc::clone(&service.prepared_cache);
    let expected_identity = service.state.validation_identity;
    let lifetime = service.limits.lease_lifetime();
    tokio::task::spawn_blocking(move || {
        retain_blocking(cache, expected_identity, lifetime, request)
    })
    .await
    .map_err(|_| Status::internal("retention worker failed"))?
}

fn retain_blocking(
    prepared_cache: Arc<std::sync::Mutex<crate::prepared_cache::PreparedCache>>,
    expected_identity: B256,
    lifetime: std::time::Duration,
    request: RetainOrdinaryRangesRequest,
) -> Result<Response<RetainOrdinaryRangesResponse>, Status> {
    let identity = B256::try_from(request.validation_identity.as_slice())
        .map_err(|_| Status::invalid_argument("retention identity must contain 32 bytes"))?;
    if identity != expected_identity {
        return Err(Status::failed_precondition(
            "retention validation identity rejected",
        ));
    }
    let lease_id = B256::try_from(request.lease_id.as_slice())
        .map_err(|_| Status::invalid_argument("retention lease id must contain 32 bytes"))?;
    let mut cache = prepared_cache
        .lock()
        .map_err(|_| Status::internal("prepared cache lock poisoned"))?;
    if request.release {
        if !request.ranges.is_empty() {
            return Err(Status::invalid_argument(
                "lease release cannot carry ranges",
            ));
        }
        cache
            .release_lease(lease_id, identity)
            .map_err(|error| Status::failed_precondition(format!("lease release: {error}")))?;
        return Ok(Response::new(RetainOrdinaryRangesResponse {
            validation_identity: identity.to_vec(),
            lease_id: lease_id.to_vec(),
            retained_count: 0,
        }));
    }
    if request.ranges.is_empty() {
        return Err(Status::invalid_argument(
            "lease renewal requires at least one range",
        ));
    }
    let mut facts = Vec::new();
    for range in request.ranges {
        let range_identity = B256::try_from(range.validation_identity.as_slice())
            .map_err(|_| Status::invalid_argument("range identity must contain 32 bytes"))?;
        let first_hash = B256::try_from(range.first_hash.as_slice())
            .map_err(|_| Status::invalid_argument("range first hash must contain 32 bytes"))?;
        let last_hash = B256::try_from(range.last_hash.as_slice())
            .map_err(|_| Status::invalid_argument("range last hash must contain 32 bytes"))?;
        if range_identity != identity {
            return Err(Status::failed_precondition(
                "range validation identity rejected",
            ));
        }
        facts.extend(
            cache
                .resolve_range(
                    identity,
                    range.first_number,
                    first_hash,
                    range.last_number,
                    last_hash,
                )
                .map_err(|error| Status::failed_precondition(format!("lease range: {error}")))?,
        );
        if facts.len() > 65_535 {
            return Err(Status::resource_exhausted(
                "lease union exceeds 65535 facts",
            ));
        }
    }
    cache
        .retain_exact(lease_id, identity, &facts, lifetime)
        .map_err(|error| Status::failed_precondition(format!("lease renewal: {error}")))?;
    Ok(Response::new(RetainOrdinaryRangesResponse {
        validation_identity: identity.to_vec(),
        lease_id: lease_id.to_vec(),
        retained_count: facts.len() as u64,
    }))
}

async fn next(
    stream: &mut Streaming<PrepareChunk>,
    idle: std::time::Duration,
) -> Result<Option<PrepareChunk>, Status> {
    timeout(idle, stream.message())
        .await
        .map_err(|_| Status::deadline_exceeded("preparation stream idle timeout"))?
}
