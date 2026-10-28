//! Resumable `prove.v2` session runtime.

use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use alloy_primitives::B256;
use eez_control_rpc::v2::prover_server::Prover;
use eez_control_rpc::v2::{
    Cancelled, ClientFrame, Proof, Ready, Rejected, ServerFrame, Validated, client_frame,
    server_frame,
};
use prost::Message as _;
use tokio::sync::{Mutex, Notify, Semaphore, mpsc, watch};
use tokio::time::{timeout, timeout_at};
use tokio_stream::Stream;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{Level, event, warn};

use super::settlement_job::{AttestablePublicInputsHash, PipelineError, settle_validated};
use super::worker::WorkerGuard;
use super::{ProveSvc, ServiceLimits, ServiceState};
use crate::cancel::CancellationToken;
use crate::validate::streaming::{BlockAnchor, ValidatedStreamBlock};
use crate::validate::{ValidatedWindow, ValidationError};
use crate::window::{AdmittedBlock, wire_witness_item_count};

const MAX_RETAINED_SESSIONS: usize = 8;
const MAX_ACTIVE_STREAMS: usize = 8;

type SessionId = B256;
type ResponseStream = Pin<Box<dyn Stream<Item = Result<ServerFrame, Status>> + Send>>;
type FrameResult = Result<(ServerFrame, bool), FrameError>;

#[derive(Debug)]
struct Session {
    anchor: BlockAnchor,
    begin_bytes: usize,
    blocks: Vec<SessionBlock>,
    history: Arc<eez_protocol::l1_history::HeaderChain>,
    history_length: usize,
    anchor_history_length: usize,
    anchor_witness_items: usize,
    epoch: u64,
    connected: bool,
    cancellation: CancellationToken,
}

#[derive(Debug)]
struct SessionBlock {
    validated: Arc<ValidatedStreamBlock>,
    // Charge each session for its own newly admitted submissions.
    payload_bytes: usize,
    witness_items: usize,
}

impl Session {
    fn tip(&self) -> BlockAnchor {
        self.blocks
            .last()
            .map_or(self.anchor, |block| block.validated.as_anchor())
    }

    /// Select a nonempty window from the prefix whose continuity append maintains.
    fn select_window(
        &self,
        finalize: &eez_control_rpc::v2::Finalize,
        terminal_hash: B256,
    ) -> Result<(BlockAnchor, Vec<Arc<ValidatedStreamBlock>>, usize), Status> {
        let first_validated = self.anchor.number.checked_add(1).ok_or_else(|| {
            Status::invalid_argument("session anchor block number cannot be incremented")
        })?;
        if finalize.from_block < first_validated || finalize.to_block < finalize.from_block {
            return Err(Status::failed_precondition(
                "Finalize range is outside the session's validated active prefix",
            ));
        }
        let start_index = usize::try_from(finalize.from_block - first_validated)
            .map_err(|_| Status::resource_exhausted("Finalize range is too large"))?;
        let end_index = usize::try_from(finalize.to_block - first_validated)
            .map_err(|_| Status::resource_exhausted("Finalize range is too large"))?;
        if start_index > end_index || end_index >= self.blocks.len() {
            return Err(Status::failed_precondition(
                "Finalize terminal block has not been fully validated",
            ));
        }
        let artifacts = self.blocks[start_index..=end_index]
            .iter()
            .map(|block| Arc::clone(&block.validated))
            .collect::<Vec<_>>();
        let terminal = artifacts.last().expect("non-empty checked range");
        if terminal.number() != finalize.to_block || terminal.hash() != terminal_hash {
            return Err(Status::failed_precondition(
                "Finalize terminal identity does not match the validated prefix",
            ));
        }
        let anchor = if start_index == 0 {
            self.anchor
        } else {
            self.blocks[start_index - 1].validated.as_anchor()
        };
        Ok((anchor, artifacts, start_index))
    }
}

#[derive(Debug, Default)]
struct Registry {
    // When both locks are needed, acquire the registry before a session.
    sessions: HashMap<SessionId, Arc<Mutex<Session>>>,
    insertion_order: VecDeque<SessionId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BoundSession {
    id: SessionId,
    epoch: u64,
}

type FrameError = (Option<BoundSession>, Status);

/// Shared by every v2 service clone. Session state is private; only immutable,
/// completely checked block artifacts cross session bounds.
#[derive(Debug)]
pub(super) struct SessionRuntime {
    state: Arc<ServiceState>,
    limits: ServiceLimits,
    registry: Mutex<Registry>,
    active_streams: AtomicUsize,
    idle: Notify,
    shutdown: watch::Sender<bool>,
    validation_worker: Arc<Semaphore>,
    active_request_slot: Arc<Semaphore>,
}

impl SessionRuntime {
    pub(super) fn new(
        state: Arc<ServiceState>,
        limits: ServiceLimits,
        validation_worker: Arc<Semaphore>,
        active_request_slot: Arc<Semaphore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            state,
            limits,
            registry: Mutex::new(Registry::default()),
            active_streams: AtomicUsize::new(0),
            idle: Notify::new(),
            shutdown: watch::channel(false).0,
            validation_worker,
            active_request_slot,
        })
    }

    pub(super) async fn wait_until_idle(&self) {
        loop {
            let notified = self.idle.notified();
            if self.active_streams.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    pub(super) fn stop_streams(&self) {
        self.shutdown.send_replace(true);
    }

    /// Serve one RPC stream, sequencing block work while accepting control frames.
    async fn serve_stream(
        self: Arc<Self>,
        mut input: Streaming<ClientFrame>,
        output: mpsc::Sender<Result<ServerFrame, Status>>,
    ) {
        enum Event {
            Input(Result<Option<ClientFrame>, Status>),
            Completed(Result<(u64, FrameResult), tokio::task::JoinError>),
        }

        let mut bound_session = None;
        let mut pending: VecDeque<(ClientFrame, usize, usize)> = VecDeque::new();
        let mut outstanding = HashSet::new();
        let mut pending_bytes = 0usize;
        let mut pending_items = 0usize;
        let mut operation: Option<tokio::task::JoinHandle<(u64, FrameResult)>> = None;
        let mut input_closed = false;
        let mut shutdown = self.shutdown.subscribe();
        loop {
            if *shutdown.borrow() {
                break;
            }
            // Start one queued operation at a time to preserve block and Finalize ordering.
            if operation.is_none()
                && let Some((frame, bytes, items)) = pending.pop_front()
            {
                pending_bytes -= bytes;
                pending_items -= items;
                let runtime = Arc::clone(&self);
                let request_id = frame.request_id;
                let mut local_bound = bound_session;
                operation = Some(tokio::spawn(async move {
                    let result = runtime.handle_frame(&mut local_bound, frame).await;
                    (request_id, result)
                }));
                continue;
            }
            // Once input ends, exit only after all accepted work has drained.
            if operation.is_none() && input_closed {
                break;
            }

            let event = tokio::select! {
                _ = shutdown.changed() => break,
                _ = output.closed() => break,
                event = async { if let Some(active) = operation.as_mut() {
                // Keep reading control frames while active work uses its own request deadline.
                Ok(if input_closed {
                    Event::Completed(active.await)
                } else {
                    tokio::select! {
                        incoming = input.message() => Event::Input(incoming),
                        completed = active => Event::Completed(completed),
                    }
                })
            } else {
                // Apply the idle timeout only while waiting for input without active work.
                timeout(self.limits.stream_idle_timeout(), async {
                    Event::Input(input.message().await)
                })
                .await
            }} => event,
            };
            // Close a stream that stays idle beyond the configured timeout.
            let Ok(event) = event else {
                self.send_response(
                    &output,
                    Err(Status::deadline_exceeded("ProveStream idle timeout")),
                )
                .await;
                break;
            };

            if let Event::Input(Ok(Some(frame))) = &event
                && !outstanding.insert(frame.request_id)
            {
                self.send_response(
                    &output,
                    Err(Status::invalid_argument("duplicate outstanding request ID")),
                )
                .await;
                break;
            }
            match event {
                // Queue dependent work so it cannot overtake earlier blocks or Finalize.
                Event::Input(Ok(Some(message)))
                    if matches!(
                        message.kind,
                        Some(client_frame::Kind::Block(_) | client_frame::Kind::Finalize(_))
                    ) =>
                {
                    // Bound decoded requests before they reach per-session admission.
                    // Allow one window plus its Finalize calldata; keep reading controls
                    // while work runs, but close an overflowing stream explicitly.
                    let bytes = message.encoded_len();
                    let items = match &message.kind {
                        Some(client_frame::Kind::Block(block)) => block
                            .data
                            .as_ref()
                            .and_then(|data| data.witness.as_ref())
                            .map_or(0, wire_witness_item_count),
                        _ => 0,
                    };
                    if pending.len() >= self.limits.window_limits().blocks.saturating_add(1)
                        || items
                            > self
                                .limits
                                .window_limits()
                                .witness_items
                                .saturating_sub(pending_items)
                        || bytes
                            > self
                                .limits
                                .window_limits()
                                .payload_bytes
                                .saturating_mul(2)
                                .saturating_sub(pending_bytes)
                    {
                        self.send_response(
                            &output,
                            Err(Status::resource_exhausted(
                                "ProveStream pending request quota exceeded",
                            )),
                        )
                        .await;
                        break;
                    }
                    pending_bytes += bytes;
                    pending_items += items;
                    pending.push_back((message, bytes, items));
                }
                // Handle control frames directly so Rewind and Cancel can fence active work.
                Event::Input(Ok(Some(message))) => {
                    let request_id = message.request_id;
                    let is_rewind = matches!(message.kind, Some(client_frame::Kind::Rewind(_)));
                    let response = self.handle_frame(&mut bound_session, message).await;
                    outstanding.remove(&request_id);
                    let rewound = is_rewind && response.is_ok();
                    let should_close = matches!(response, Ok((_, true)));
                    let frame = match response {
                        Ok((frame, _)) => frame,
                        Err((authorized, status)) => {
                            self.rejected_frame(authorized, request_id, status).await
                        }
                    };
                    if !self.send_response(&output, Ok(frame)).await {
                        break;
                    }
                    // Discard queued requests targeting the pre-rewind prefix.
                    if rewound {
                        for (frame, _, _) in &pending {
                            outstanding.remove(&frame.request_id);
                        }
                        pending.clear();
                        pending_bytes = 0;
                        pending_items = 0;
                    }
                    if should_close {
                        break;
                    }
                }
                // Stop reading after client half-close, but still finish accepted requests.
                Event::Input(Ok(None)) => {
                    input_closed = true;
                }
                // Input decoding or transport failures terminate the RPC, not just one request.
                Event::Input(Err(error)) => {
                    let status = error;
                    self.send_response(&output, Err(status)).await;
                    break;
                }
                // Release the work slot and deliver the operation's success or rejection.
                Event::Completed(Ok((request_id, response))) => {
                    outstanding.remove(&request_id);
                    operation = None;
                    let should_close = matches!(response, Ok((_, true)));
                    // Suppress replies that do not belong to this stream's current binding.
                    let frame = match response {
                        Ok((frame, _))
                            if bound_session.is_some_and(|bound| {
                                frame.session_id == bound.id.as_slice()
                                    && frame.epoch == bound.epoch
                            }) =>
                        {
                            Some(frame)
                        }
                        Err((Some(authorized), status)) if bound_session == Some(authorized) => {
                            Some(
                                self.rejected_frame(Some(authorized), request_id, status)
                                    .await,
                            )
                        }
                        Err((None, status)) => {
                            Some(self.rejected_frame(None, request_id, status).await)
                        }
                        Ok(_) | Err(_) => None,
                    };
                    let should_close = should_close && frame.is_some();
                    if let Some(frame) = frame
                        && !self.send_response(&output, Ok(frame)).await
                    {
                        break;
                    }
                    if should_close {
                        break;
                    }
                }
                // A task panic or unexpected cancellation makes the stream unusable.
                Event::Completed(Err(error)) => {
                    operation = None;
                    warn!(?error, "prover session operation task failed");
                    self.send_response(
                        &output,
                        Err(Status::internal("prover session operation failed")),
                    )
                    .await;
                    break;
                }
            }
        }
        // End the response stream before waiting for any outstanding work to finish.
        drop(output);
        if let Some(operation) = operation {
            // Dropping the operation cancels its worker guard. Already-running
            // blocking work keeps the shared permit until it actually exits.
            operation.abort();
            let _ = operation.await;
        }
        // An old stream must not mark a newer resumed attachment disconnected.
        if let Some(bound) = bound_session
            && let Ok(session) = self.session(bound.id).await
        {
            let mut session = session.lock().await;
            if session.epoch == bound.epoch {
                session.connected = false;
            }
        }
        // Wake shutdown waiters when the last stream has finished cleaning up.
        if self.active_streams.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.idle.notify_waiters();
        }
    }

    /// Bound response backpressure so a non-reading client cannot retain a stream forever.
    async fn send_response(
        &self,
        output: &mpsc::Sender<Result<ServerFrame, Status>>,
        frame: Result<ServerFrame, Status>,
    ) -> bool {
        let mut shutdown = self.shutdown.subscribe();
        if *shutdown.borrow() {
            return false;
        }
        let reservation = tokio::select! {
            _ = shutdown.changed() => return false,
            result = timeout(self.limits.stream_idle_timeout(), output.reserve()) => result,
        };
        let Ok(Ok(permit)) = reservation else {
            return false;
        };
        if let Ok(response) = &frame
            && matches!(
                response.kind,
                Some(server_frame::Kind::Validated(_) | server_frame::Kind::Proof(_))
            )
        {
            let Ok(id) = parse_session_id(&response.session_id) else {
                return false;
            };
            let Ok(session) = self.session(id).await else {
                return false;
            };
            let session = session.lock().await;
            // Linearize publication with Resume/Rewind, after waiting for output space.
            if session.epoch != response.epoch {
                return false;
            }
            permit.send(frame);
        } else {
            permit.send(frame);
        }
        true
    }

    /// Handle one request, updating its stream binding and returning a response plus a close flag.
    async fn handle_frame(
        &self,
        bound_session: &mut Option<BoundSession>,
        frame: ClientFrame,
    ) -> Result<(ServerFrame, bool), FrameError> {
        let frame_bytes = frame.encoded_len();
        let ClientFrame {
            session_id,
            request_id,
            epoch,
            kind,
        } = frame;
        let Some(kind) = kind else {
            return Err((
                authorized_bound(bound_session.as_ref(), &session_id, epoch),
                Status::unimplemented("frame carries no supported operation"),
            ));
        };

        match kind {
            // Create and bind a new session at the supplied anchor, then report its ready cursor.
            client_frame::Kind::Begin(begin) => {
                if bound_session.is_some() || !session_id.is_empty() || epoch != 0 {
                    return Err((
                        authorized_bound(bound_session.as_ref(), &session_id, epoch),
                        Status::invalid_argument(
                            "Begin must be the first frame with no session ID and epoch zero",
                        ),
                    ));
                }
                let (bound, ready) = self
                    .begin(begin, frame_bytes)
                    .await
                    .map_err(|status| (None, status))?;
                *bound_session = Some(bound);
                Ok((
                    server_frame(bound, request_id, server_frame::Kind::Ready(ready)),
                    false,
                ))
            }
            // Reattach to retained progress with a new epoch that fences the previous attachment.
            client_frame::Kind::Resume(_) => {
                if bound_session.is_some() || epoch != 0 {
                    return Err((
                        authorized_bound(bound_session.as_ref(), &session_id, epoch),
                        Status::invalid_argument("Resume must be the first frame with epoch zero"),
                    ));
                }
                let id = parse_session_id(&session_id).map_err(|status| (None, status))?;
                let (bound, ready) = self.resume(id).await.map_err(|status| (None, status))?;
                *bound_session = Some(bound);
                Ok((
                    server_frame(bound, request_id, server_frame::Kind::Ready(ready)),
                    false,
                ))
            }
            // Truncate to a retained ancestor and advance the epoch to fence old-prefix work.
            client_frame::Kind::Rewind(rewind) => {
                let bound = require_bound(bound_session.as_ref(), &session_id, epoch)?;
                let ancestor =
                    parse_b256("rewind ancestor hash", &rewind.ancestor_hash).map_err(|_| {
                        (
                            Some(bound),
                            Status::failed_precondition("Rewind ancestor is not retained"),
                        )
                    })?;
                let (rewound, ready) = self
                    .rewind(bound, ancestor)
                    .await
                    .map_err(|status| (Some(bound), status))?;
                *bound_session = Some(rewound);
                Ok((
                    server_frame(rewound, request_id, server_frame::Kind::Ready(ready)),
                    false,
                ))
            }
            // Validate or reuse a block and acknowledge checked progress without issuing a proof.
            client_frame::Kind::Block(block) => {
                let bound = require_bound(bound_session.as_ref(), &session_id, epoch)?;
                let started = Instant::now();
                let validated = self
                    .validate_block(bound, block, frame_bytes)
                    .await
                    .map_err(|status| (Some(bound), status))?;
                event!(
                    name: "eez.proof_signer.block_validated",
                    Level::INFO,
                    event_name = "eez.proof_signer.block_validated",
                    session_id = %bound.id,
                    number = validated.number,
                    reused = validated.reused,
                    elapsed_us = started.elapsed().as_micros(),
                    "streamed block validated",
                );
                Ok((
                    server_frame(bound, request_id, server_frame::Kind::Validated(validated)),
                    false,
                ))
            }
            // Check and sign the requested validated range, then close the stream with its proof.
            client_frame::Kind::Finalize(finalize) => {
                let bound = require_bound(bound_session.as_ref(), &session_id, epoch)?;
                let proof = self
                    .finalize(bound, finalize, frame_bytes)
                    .await
                    .map_err(|status| (Some(bound), status))?;
                Ok((
                    server_frame(bound, request_id, server_frame::Kind::Proof(proof)),
                    true,
                ))
            }
            // Retire this session so it cannot resume, then acknowledge cancellation and close.
            client_frame::Kind::Cancel(_) => {
                let bound = require_bound(bound_session.as_ref(), &session_id, epoch)?;
                let mut registry = self.registry.lock().await;
                let session =
                    registry.sessions.get(&bound.id).cloned().ok_or_else(|| {
                        (None, Status::not_found("prover session is not retained"))
                    })?;
                let session = session.lock().await;
                if session.epoch != bound.epoch {
                    return Err((
                        None,
                        Status::permission_denied("prover session epoch is stale"),
                    ));
                }
                session.cancellation.cancel();
                drop(session);
                registry.sessions.remove(&bound.id);
                registry
                    .insertion_order
                    .retain(|retained| *retained != bound.id);
                *bound_session = None;
                Ok((
                    server_frame(
                        bound,
                        request_id,
                        server_frame::Kind::Cancelled(Cancelled {}),
                    ),
                    true,
                ))
            }
        }
    }

    /// Create a retained session at the supplied anchor and return its initial binding and cursor.
    async fn begin(
        &self,
        begin: eez_control_rpc::v2::Begin,
        frame_bytes: usize,
    ) -> Result<(BoundSession, Ready), Status> {
        if begin.rollup_id == 0 {
            return Err(Status::invalid_argument("rollup ID must be nonzero"));
        }
        if frame_bytes > self.limits.window_limits().payload_bytes {
            return Err(Status::resource_exhausted("session byte quota exceeded"));
        }
        if begin.rollup_id != self.state.expected_rollup_id.get() {
            return Err(Status::unimplemented("session rollup identity rejected"));
        }
        begin
            .anchor_number
            .checked_add(1)
            .ok_or_else(|| Status::invalid_argument("anchor block number cannot be incremented"))?;
        let anchor = BlockAnchor {
            number: begin.anchor_number,
            hash: parse_b256("anchor hash", &begin.anchor_hash)?,
            state_root: parse_b256("anchor state root", &begin.anchor_state_root)?,
        };

        let mut registry = self.registry.lock().await;
        // Make room by evicting the oldest-created disconnected sessions, keeping active ones.
        while registry.sessions.len() >= MAX_RETAINED_SESSIONS {
            let mut eviction = None;
            for id in &registry.insertion_order {
                if let Some(session) = registry.sessions.get(id)
                    && !session.lock().await.connected
                {
                    eviction = Some(*id);
                    break;
                }
            }
            let Some(eviction) = eviction else {
                break;
            };
            registry.sessions.remove(&eviction);
            registry
                .insertion_order
                .retain(|retained| *retained != eviction);
        }
        if registry.sessions.len() >= MAX_RETAINED_SESSIONS {
            return Err(Status::resource_exhausted(
                "too many retained prover sessions",
            ));
        }
        let id = loop {
            let mut bytes = [0_u8; 32];
            getrandom::fill(&mut bytes)
                .map_err(|_| Status::internal("unable to allocate a prover session ID"))?;
            let id = B256::from(bytes);
            if id != B256::ZERO && !registry.sessions.contains_key(&id) {
                break id;
            }
        };
        registry.sessions.insert(
            id,
            Arc::new(Mutex::new(Session {
                anchor,
                begin_bytes: frame_bytes,
                blocks: Vec::new(),
                history: Arc::default(),
                history_length: 0,
                anchor_history_length: 0,
                anchor_witness_items: 0,
                epoch: 1,
                connected: true,
                cancellation: CancellationToken::default(),
            })),
        );
        registry.insertion_order.push_back(id);
        Ok((
            BoundSession { id, epoch: 1 },
            Ready {
                validated_through: anchor.number,
                validated_hash: anchor.hash.to_vec(),
            },
        ))
    }

    /// Reattach to retained progress with a new epoch, cancelling work from the previous attachment.
    async fn resume(&self, id: SessionId) -> Result<(BoundSession, Ready), Status> {
        // Keep the registry locked until attachment so eviction cannot remove
        // a disconnected session while it is being resumed.
        let registry = self.registry.lock().await;
        let session = registry
            .sessions
            .get(&id)
            .ok_or_else(|| Status::not_found("prover session is not retained"))?;
        let mut session = session.lock().await;
        let epoch = next_epoch(session.epoch)?;
        session.cancellation.cancel();
        session.epoch = epoch;
        session.connected = true;
        session.cancellation = CancellationToken::default();
        Ok((
            BoundSession {
                id,
                epoch: session.epoch,
            },
            Ready {
                validated_through: session.tip().number,
                validated_hash: session.tip().hash.to_vec(),
            },
        ))
    }

    /// Truncate to a retained ancestor and advance the epoch to invalidate old-prefix work.
    async fn rewind(
        &self,
        bound: BoundSession,
        ancestor_hash: B256,
    ) -> Result<(BoundSession, Ready), Status> {
        let session = self.session(bound.id).await?;
        let mut session = session.lock().await;
        require_epoch(&session, bound.epoch)?;

        let retained = if ancestor_hash == session.anchor.hash {
            0
        } else {
            session
                .blocks
                .iter()
                .position(|block| block.validated.hash() == ancestor_hash)
                .map(|index| index + 1)
                .ok_or_else(|| {
                    Status::failed_precondition(
                        "Rewind ancestor is not in the session's validated active prefix",
                    )
                })?
        };
        let epoch = next_epoch(session.epoch)?;
        session.blocks.truncate(retained);
        session.history_length = session
            .blocks
            .last()
            .map_or(session.anchor_history_length, |block| {
                block.validated.l1_checkpoint_len()
            });
        session.cancellation.cancel();
        session.epoch = epoch;
        session.cancellation = CancellationToken::default();
        let rewound = BoundSession {
            id: bound.id,
            epoch: session.epoch,
        };
        let ready = Ready {
            validated_through: session.tip().number,
            validated_hash: session.tip().hash.to_vec(),
        };
        Ok((rewound, ready))
    }

    async fn session(&self, id: SessionId) -> Result<Arc<Mutex<Session>>, Status> {
        self.registry
            .lock()
            .await
            .sessions
            .get(&id)
            .cloned()
            .ok_or_else(|| Status::not_found("prover session is not retained"))
    }

    /// Validate or reuse a block and retain its checked artifact in the session.
    async fn validate_block(
        &self,
        bound: BoundSession,
        block: eez_control_rpc::v2::Block,
        frame_bytes: usize,
    ) -> Result<Validated, Status> {
        let submitted = block
            .data
            .ok_or_else(|| Status::invalid_argument("Block carries no block data"))?;
        if submitted.witness.is_none() {
            return Err(Status::invalid_argument(
                "Block carries no execution witness",
            ));
        }
        let witness_items = submitted
            .witness
            .as_ref()
            .map_or(0, wire_witness_item_count);
        let admitted = AdmittedBlock::from_wire(submitted, &mut 0, self.limits.window_limits())
            .map_err(|error| Status::invalid_argument(error.to_string()))?;
        let session = self.session(bound.id).await?;
        let session_guard = session.lock().await;
        require_epoch(&session_guard, bound.epoch)?;

        if let Some(existing) = session_guard
            .blocks
            .iter()
            .map(|block| &block.validated)
            .find(|artifact| {
                artifact.number() == admitted.declared_number
                    && artifact.hash() == admitted.claimed_hash
            })
        {
            if existing.parent_hash() != admitted.claimed_parent_hash
                || existing.rlp() != admitted.rlp
            {
                return Err(Status::invalid_argument(
                    "duplicate block number does not match its validated artifact",
                ));
            }
            if !existing.matches_witness(&admitted) {
                return Err(Status::failed_precondition(
                    "duplicate witness differs from checked execution",
                ));
            }
            return Ok(validated_response(existing, true));
        }

        let parent = session_guard.tip();
        // Parse the wire fields; the shared validator checks the block's parent and number.
        let payload_bytes = frame_bytes;
        if parent.number.checked_add(1) != Some(admitted.declared_number)
            || admitted.claimed_parent_hash != parent.hash
        {
            return Err(Status::failed_precondition(
                "block does not extend the session cursor",
            ));
        }

        // Charge aggregate session quotas before replay; admission already bounded this witness.
        let attempted_blocks = session_guard.blocks.len().saturating_add(1);
        if attempted_blocks > self.limits.window_limits().blocks {
            return Err(Status::resource_exhausted("session block quota exceeded"));
        }
        let (attempted_bytes, attempted_items) = session_guard.blocks.iter().try_fold(
            (
                payload_bytes.saturating_add(session_guard.begin_bytes),
                witness_items.saturating_add(session_guard.anchor_witness_items),
            ),
            |(bytes, items), block| {
                let bytes = bytes
                    .checked_add(block.payload_bytes)
                    .ok_or_else(|| Status::resource_exhausted("session payload quota overflow"))?;
                let items = items
                    .checked_add(block.witness_items)
                    .ok_or_else(|| Status::resource_exhausted("session witness quota overflow"))?;
                Ok::<_, Status>((bytes, items))
            },
        )?;
        if attempted_bytes > self.limits.window_limits().payload_bytes {
            return Err(Status::resource_exhausted("session payload quota exceeded"));
        }
        if attempted_items > self.limits.window_limits().witness_items {
            return Err(Status::resource_exhausted("session witness quota exceeded"));
        }

        let session_cancellation = session_guard.cancellation.clone();
        let history = Arc::clone(&session_guard.history);
        let checkpoint_len = session_guard.history_length;
        let previous = session_guard
            .blocks
            .last()
            .map(|block| Arc::clone(&block.validated));
        let max_headers = self.limits.window_limits().witness_items.min(131_072);
        let cancellation = CancellationToken::default();
        drop(session_guard);
        // Structural admission cannot authorize execution. Replay and output checks
        // remain behind Validator; no unvalidated witness enters retained facts.
        let deadline = tokio::time::Instant::now() + self.limits.request_timeout();
        let permit = timeout_at(
            deadline,
            Arc::clone(&self.validation_worker).acquire_owned(),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("block validation deadline exceeded"))?
        .map_err(|_| Status::unavailable("validation worker closed"))?;
        let state = Arc::clone(&self.state);
        let worker_cancellation = cancellation.clone();
        let worker_session_cancellation = session_cancellation.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            check_deadline(deadline)?;
            if worker_session_cancellation.is_cancelled() {
                return Err(Status::cancelled("session was superseded"));
            }
            state
                .validator
                .validate_stream_block(
                    parent,
                    admitted,
                    history,
                    checkpoint_len,
                    previous.as_deref(),
                    &state.offline_rules.headers,
                    max_headers,
                    &worker_cancellation,
                )
                .map(|(block, history)| (Arc::new(block), history))
                .map_err(validation_status)
        });
        let _guard = WorkerGuard::new(worker.abort_handle(), cancellation);
        let (cached, history) = timeout_at(deadline, worker)
            .await
            .map_err(|_| Status::deadline_exceeded("block validation deadline exceeded"))?
            .map_err(|_| Status::internal("block validation worker failed"))??;
        check_deadline(deadline)?;
        if session_cancellation.is_cancelled() {
            return Err(Status::cancelled("session was superseded"));
        }
        let reused = false;

        let mut session_guard = session.lock().await;
        require_epoch(&session_guard, bound.epoch)?;
        // The backend proved an extension of this snapshot. Only its continued
        // ownership needs checking after the await, not the immutable block again.
        if session_guard.tip() != parent {
            return Err(Status::aborted(
                "session cursor changed while block validation was in flight",
            ));
        }
        session_guard.history_length = cached.l1_checkpoint_len();
        session_guard.history = history;
        session_guard.blocks.push(SessionBlock {
            validated: Arc::clone(&cached),
            payload_bytes,
            witness_items,
        });
        Ok(validated_response(&cached, reused))
    }

    /// Authenticate the selected predecessor and bind native settlement to retained execution.
    async fn finalize(
        &self,
        bound: BoundSession,
        mut finalize: eez_control_rpc::v2::Finalize,
        frame_bytes: usize,
    ) -> Result<Proof, Status> {
        let deadline = tokio::time::Instant::now() + self.limits.request_timeout();
        let terminal_hash = parse_b256("terminal hash", &finalize.terminal_hash)
            .map_err(|_| Status::failed_precondition("Finalize terminal is not validated"))?;
        let settlement = finalize
            .settlement
            .take()
            .ok_or_else(|| Status::invalid_argument("Finalize carries no settlement input"))?;
        let posting_context = settlement
            .posting_context
            .as_ref()
            .map(|target| {
                let context = eez_protocol::public_inputs::L1PostingContext {
                    chain_id: target.chain_id,
                    target_timestamp: target.target_timestamp,
                    parent_number: target.parent_number,
                    parent_hash: parse_b256("L1 parent hash", &target.parent_hash)?,
                };
                context
                    .validate()
                    .map_err(|error| Status::invalid_argument(error.to_string()))
            })
            .transpose()?;
        if settlement.blobs.is_empty() || settlement.abi_calldata.is_empty() {
            return Err(Status::invalid_argument(
                "native settlement requires calldata and physical blobs",
            ));
        }
        let session = self.session(bound.id).await?;
        let (anchor, blocks, start_index, history, cancellation) = {
            let mut session = session.lock().await;
            require_epoch(&session, bound.epoch)?;
            let retained_bytes = session
                .blocks
                .iter()
                .fold(session.begin_bytes, |bytes, block| {
                    bytes.saturating_add(block.payload_bytes)
                });
            if frame_bytes
                > self
                    .limits
                    .window_limits()
                    .payload_bytes
                    .saturating_sub(retained_bytes)
            {
                return Err(Status::resource_exhausted(
                    "Finalize exceeds session byte quota",
                ));
            }
            let (anchor, window, start_index) = session.select_window(&finalize, terminal_hash)?;
            session.cancellation.cancel();
            session.cancellation = CancellationToken::default();
            (
                anchor,
                window,
                start_index,
                Arc::clone(&session.history),
                session.cancellation.clone(),
            )
        };
        // Queue complete finalizations fairly;
        // a timed-out blocking worker retains it until that worker has exited.
        let request_permit = timeout_at(
            deadline,
            Arc::clone(&self.active_request_slot).acquire_owned(),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("Finalize admission deadline exceeded"))?
        .map_err(|_| Status::unavailable("proof admission closed"))?;
        let permit = timeout_at(
            deadline,
            Arc::clone(&self.validation_worker).acquire_owned(),
        )
        .await
        .map_err(|_| Status::deadline_exceeded("Finalize worker deadline exceeded"))?
        .map_err(|_| Status::unavailable("validation worker closed"))?;
        let state = Arc::clone(&self.state);
        let worker_cancellation = cancellation.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let result = (|| {
                let canonical_batch =
                    crate::settlement::decode_canonical_post_batch(settlement.abi_calldata)
                        .map_err(super::settlement_job::SettlementPipelineError::from)?;
                let candidate_context = state
                    .deployment
                    .candidate(
                        state.expected_rollup_id.get(),
                        canonical_batch.as_batch(),
                        posting_context,
                        anchor,
                        &blocks,
                        &history,
                    )
                    .map_err(PipelineError::L1Authorization)?;
                let maximum_blobs = state
                    .posting_blob_limit(candidate_context.posting_timestamp())
                    .map_err(|_| {
                        PipelineError::L1Authorization("no supported L1 blob rules at proof anchor")
                    })?;
                let validated_window = ValidatedWindow::from_streamed_prefix(anchor, &blocks)?;
                settle_validated(
                    &state,
                    &validated_window,
                    canonical_batch,
                    settlement.blobs,
                    maximum_blobs,
                    candidate_context,
                    deadline,
                    &worker_cancellation,
                )
            })();
            (request_permit, result)
        });
        let _guard = WorkerGuard::new(worker.abort_handle(), cancellation.clone());
        let (_request_permit, result) = timeout_at(deadline, worker)
            .await
            .map_err(|_| Status::deadline_exceeded("Finalize deadline exceeded"))?
            .map_err(|_| Status::internal("Finalize worker failed"))?;
        let material = result.map_err(|error| {
            warn!(phase = error.phase(), gate = error.gate(), %error, "offline finalization rejected");
            error.status()
        })?;
        let mut session = session.lock().await;
        require_epoch(&session, bound.epoch)?;
        check_deadline(deadline)?;
        if cancellation.is_cancelled() {
            return Err(Status::cancelled("Finalize was superseded"));
        }
        let hash = AttestablePublicInputsHash::authorize(material.recomputed_public_inputs);
        let signature = self
            .state
            .attester
            .sign(hash)
            .map_err(|_| Status::internal("attestation signing failed"))?;
        check_deadline(deadline)?;
        event!(name: "eez.proof_signer.window_signed", Level::INFO,
            event_name = "eez.proof_signer.window_signed", phase = "attestation", rpc_version = 2,
            validated_from_block = finalize.from_block, validated_to_block = finalize.to_block,
            validated_window_pre_block_hash = %material.validated_window_pre_block_hash,
            validated_window_post_block_hash = %material.validated_window_post_block_hash,
            recomputed_public_inputs_hash = %hash.into_inner(), attester = %self.state.attester.address(),
            "window validated and signed");
        session.anchor = anchor;
        if start_index != 0 {
            session.anchor_history_length = session.blocks[start_index - 1]
                .validated
                .l1_checkpoint_len();
            // The shared L1 bridge remains retained. Conservatively keep each
            // dropped frame's quota charge until the session is discarded.
            for index in 0..start_index {
                session.begin_bytes = session
                    .begin_bytes
                    .saturating_add(session.blocks[index].payload_bytes);
                session.anchor_witness_items = session
                    .anchor_witness_items
                    .saturating_add(session.blocks[index].witness_items);
            }
        }
        session.blocks.drain(..start_index);
        Ok(Proof {
            public_inputs_hash: hash.into_inner().to_vec(),
            proof: signature.to_vec(),
        })
    }
}

#[tonic::async_trait]
impl Prover for ProveSvc {
    type ProveStreamStream = ResponseStream;

    async fn capabilities(
        &self,
        _request: Request<eez_control_rpc::v2::CapabilitiesRequest>,
    ) -> Result<Response<eez_control_rpc::v2::CapabilitiesResponse>, Status> {
        let limits = self.limits;
        let window = limits.window_limits();
        // Use the enforced limits, including the aggregate-byte reduction of
        // the per-message ceiling. No permit, witness, or attestable value is
        // acquired/constructed here.
        Ok(Response::new(eez_control_rpc::v2::CapabilitiesResponse {
            operations_tag: u32::from(eez_payload_codec::v0::VERSION),
            profile_id: eez_control_rpc::OFFLINE_PROFILE_ID,
            validation_identity: self.state.validation_identity.to_vec(),
            max_window_blocks: window.blocks as u64,
            max_request_bytes: window.payload_bytes as u64,
            max_request_witness_items: window.witness_items as u64,
            max_message_bytes: limits.max_decoding_message_bytes() as u64,
            stream_idle_timeout_millis: u64::try_from(limits.stream_idle_timeout().as_millis())
                .map_err(|_| Status::internal("stream idle timeout exceeds capability range"))?,
            request_timeout_millis: u64::try_from(limits.request_timeout().as_millis())
                .map_err(|_| Status::internal("request timeout exceeds capability range"))?,
        }))
    }

    /// Start the v2 RPC handler and return its server-frame response stream.
    async fn prove_stream(
        &self,
        request: Request<Streaming<ClientFrame>>,
    ) -> Result<Response<Self::ProveStreamStream>, Status> {
        if *self.sessions.shutdown.borrow() {
            return Err(Status::unavailable("prover is shutting down"));
        }
        let identity = request
            .metadata()
            .get_bin("eez-validation-identity-bin")
            .ok_or_else(|| {
                Status::failed_precondition("offline validation identity metadata is required")
            })?
            .to_bytes()
            .map_err(|_| Status::invalid_argument("invalid validation identity metadata"))?;
        if identity.as_ref() != self.state.validation_identity.as_slice() {
            return Err(Status::failed_precondition("validation identity mismatch"));
        }
        let (sender, receiver) = mpsc::channel(8);
        self.sessions
            .active_streams
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_ACTIVE_STREAMS).then_some(active + 1)
            })
            .map_err(|_| Status::resource_exhausted("too many active prover streams"))?;
        let runtime = Arc::clone(&self.sessions);
        tokio::spawn(runtime.serve_stream(request.into_inner(), sender));
        Ok(Response::new(Box::pin(ReceiverStream::new(receiver))))
    }
}

fn require_bound(
    bound: Option<&BoundSession>,
    submitted: &[u8],
    epoch: u64,
) -> Result<BoundSession, FrameError> {
    let Some(bound) = bound.copied() else {
        return Err((
            None,
            Status::permission_denied("stream is not bound to a session"),
        ));
    };
    if bound.id.as_slice() != submitted || bound.epoch != epoch {
        return Err((
            None,
            Status::permission_denied("frame does not belong to this stream's session"),
        ));
    }
    Ok(bound)
}

fn authorized_bound(
    bound: Option<&BoundSession>,
    submitted: &[u8],
    epoch: u64,
) -> Option<BoundSession> {
    let bound = bound.copied()?;
    (bound.epoch == epoch && submitted == bound.id.as_slice()).then_some(bound)
}

fn require_epoch(session: &Session, epoch: u64) -> Result<(), Status> {
    if session.epoch == epoch {
        Ok(())
    } else {
        Err(Status::permission_denied("prover session epoch is stale"))
    }
}

fn next_epoch(epoch: u64) -> Result<u64, Status> {
    epoch
        .checked_add(1)
        .ok_or_else(|| Status::resource_exhausted("session epoch exhausted; begin a new session"))
}

fn parse_session_id(bytes: &[u8]) -> Result<SessionId, Status> {
    B256::try_from(bytes)
        .map_err(|_| Status::invalid_argument("session ID must contain exactly 32 bytes"))
}

fn parse_b256(name: &'static str, bytes: &[u8]) -> Result<B256, Status> {
    B256::try_from(bytes)
        .map_err(|_| Status::invalid_argument(format!("{name} must contain exactly 32 bytes")))
}

fn validated_response(artifact: &ValidatedStreamBlock, reused: bool) -> Validated {
    Validated {
        number: artifact.number(),
        hash: artifact.hash().to_vec(),
        post_state_root: artifact.post_state_root().to_vec(),
        reused,
    }
}

fn validation_status(error: ValidationError) -> Status {
    warn!(%error, "stream validation failed");
    PipelineError::Validation(error).status()
}

fn server_frame(bound: BoundSession, request_id: u64, kind: server_frame::Kind) -> ServerFrame {
    ServerFrame {
        session_id: bound.id.to_vec(),
        request_id,
        epoch: bound.epoch,
        kind: Some(kind),
    }
}

impl SessionRuntime {
    async fn rejected_frame(
        &self,
        authorized: Option<BoundSession>,
        request_id: u64,
        status: Status,
    ) -> ServerFrame {
        let (session_id, epoch, validated_through, validated_hash) = if let Some(bound) = authorized
        {
            match self.session(bound.id).await {
                Ok(session) => {
                    let session = session.lock().await;
                    if session.epoch == bound.epoch {
                        (
                            bound.id.to_vec(),
                            bound.epoch,
                            session.tip().number,
                            session.tip().hash.to_vec(),
                        )
                    } else {
                        (Vec::new(), 0, 0, Vec::new())
                    }
                }
                Err(_) => (Vec::new(), 0, 0, Vec::new()),
            }
        } else {
            (Vec::new(), 0, 0, Vec::new())
        };
        ServerFrame {
            session_id,
            request_id,
            epoch,
            kind: Some(server_frame::Kind::Rejected(Rejected {
                code: i32::from(status.code()),
                message: status.message().to_owned(),
                details: status.details().to_vec(),
                validated_through,
                validated_hash,
            })),
        }
    }
}

fn check_deadline(deadline: tokio::time::Instant) -> Result<(), Status> {
    if tokio::time::Instant::now() >= deadline {
        Err(Status::deadline_exceeded(
            "ProveStream operation deadline exceeded",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
