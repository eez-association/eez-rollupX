//! Bounded v2 exchanges. Retained progress is a transport optimization; every
//! Finalize binds the exact native settlement and checks the returned signature.
use super::{RemoteProver, map_rpc_status, wire_block};
use alloy_primitives::B256;
use eez_control_rpc::v2::{
    Begin as StreamBegin, Block as StreamBlock, ClientFrame, Finalize as StreamFinalize,
    Resume as StreamResume, Rewind as StreamRewind, ServerFrame, client_frame,
    prover_client::ProverClient as StreamingProverClient, server_frame,
};
use eez_prover::{BlockWitness, ProverError, ProverResult, RetryableProverError};
use tonic::{Code, Status};

const MAX_IN_FLIGHT: usize = 8;

#[derive(Debug, Clone, Copy)]
pub(super) struct StreamAnchor {
    pub(super) number: u64,
    pub(super) hash: B256,
    pub(super) state_root: B256,
}

fn block_header(block: &BlockWitness) -> ProverResult<alloy_consensus::Header> {
    let decoded: eez_primitives::Block = alloy_rlp::decode_exact(&block.rlp)
        .map_err(|_| ProverError::Backend("invalid local block RLP".into()))?;
    if decoded.header.number != block.number
        || decoded.header.hash_slow() != block.hash
        || decoded.header.parent_hash != block.parent_hash
    {
        return Err(ProverError::Backend(
            "local block identity differs from RLP".into(),
        ));
    }
    Ok(decoded.header)
}

fn wire_segment(
    block: &BlockWitness,
    tip: &mut Option<alloy_primitives::Bytes>,
) -> ProverResult<eez_control_rpc::v1::BlockWitness> {
    let first = block
        .l1_headers_rlp
        .first()
        .ok_or_else(|| ProverError::Backend("captured witness has no L1 endpoint".into()))?;
    let last = block.l1_headers_rlp.last().expect("nonempty checked above");
    let mut wire = wire_block(block.clone());
    if let Some(previous) = tip.as_ref() {
        if previous != first {
            return Err(ProverError::Backend(
                "captured L1 witness segments disagree at shared endpoint".into(),
            ));
        }
        wire.witness
            .as_mut()
            .expect("wire_block always supplies a witness")
            .l1_headers_rlp
            .remove(0);
    }
    *tip = Some(last.clone());
    Ok(wire)
}

#[derive(Debug, Default)]
pub(super) struct StreamingState {
    session: Option<StreamingSession>,
    next_request_id: u64,
}

#[derive(Debug)]
struct StreamingSession {
    // Transport binding only; the server owns the anchor and validated history.
    rollup_id: u64,
    id: Vec<u8>,
    epoch: u64,
    // Oldest retained predecessor, from Begin or a successful subrange Proof.
    // Background prevalidation can start beyond an unsettled candidate's parent.
    anchor: StreamAnchor,
    // Only this original Begin anchor has an empty header checkpoint. A
    // promoted subrange anchor retains the headers of its executed predecessor.
    empty_history_anchor: Option<B256>,
    // Finalize extends the terminal evidence through its proof anchor. A later
    // target must rewind that frame, even when the L2 block hash is unchanged.
    extended_at: Option<u64>,
}

impl StreamingState {
    async fn send(
        &mut self,
        requests: &tokio::sync::mpsc::Sender<ClientFrame>,
        kind: client_frame::Kind,
    ) -> ProverResult<u64> {
        let message = match &kind {
            client_frame::Kind::Rewind(_) => "ProveStream closed while sending Rewind",
            client_frame::Kind::Block(_) => "ProveStream closed while sending a block",
            client_frame::Kind::Finalize(_) => "ProveStream closed while sending Finalize",
            _ => unreachable!("only bound block, rewind and finalize requests use this path"),
        };
        let request_id = self.next_id();
        let session = self.session.as_ref().expect("ready session exists");
        requests
            .send(ClientFrame {
                session_id: session.id.clone(),
                request_id,
                epoch: session.epoch,
                kind: Some(kind),
            })
            .await
            .map_err(|_| ProverError::Retryable {
                kind: RetryableProverError::Unavailable,
                message: message.into(),
            })?;
        Ok(request_id)
    }
    async fn response(
        &mut self,
        responses: &mut tonic::Streaming<ServerFrame>,
        request_id: u64,
        range: (u64, u64),
    ) -> ProverResult<ServerFrame> {
        let session = self.session.as_ref().expect("ready session exists");
        let response = next_stream_response(
            responses,
            request_id,
            Some(&session.id),
            Some(session.epoch),
            range.0,
            range.1,
        )
        .await?;
        Ok(response)
    }
    fn next_id(&mut self) -> u64 {
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.next_request_id
    }
}

async fn next_stream_response(
    responses: &mut tonic::Streaming<ServerFrame>,
    request_id: u64,
    expected_session_id: Option<&[u8]>,
    expected_epoch: Option<u64>,
    from_block: u64,
    to_block: u64,
) -> ProverResult<ServerFrame> {
    let response = responses
        .message()
        .await
        .map_err(|status| map_rpc_status(from_block, to_block, status))?
        .ok_or_else(|| ProverError::Retryable {
            kind: RetryableProverError::Unavailable,
            message: "ProveStream ended before its response".into(),
        })?;
    if response.request_id != request_id {
        return Err(ProverError::Backend(format!(
            "ProveStream response {} crossed request {}",
            response.request_id, request_id,
        )));
    }
    // An unknown or stale session has no authorized binding to echo.
    let unbound_rejection = matches!(response.kind, Some(server_frame::Kind::Rejected(_)))
        && response.session_id.is_empty()
        && response.epoch == 0;
    if !unbound_rejection
        && expected_session_id.is_some_and(|expected| response.session_id != expected)
    {
        return Err(ProverError::Backend(
            "ProveStream response crossed Composer sessions".into(),
        ));
    }
    if !unbound_rejection && expected_epoch.is_some_and(|expected| response.epoch != expected) {
        return Err(ProverError::Backend(format!(
            "ProveStream response epoch {} crossed epoch {}",
            response.epoch,
            expected_epoch.expect("checked above"),
        )));
    }
    Ok(response)
}

fn rejection_error(response: &ServerFrame, from_block: u64, to_block: u64) -> Option<ProverError> {
    let server_frame::Kind::Rejected(rejected) = response.kind.as_ref()? else {
        return None;
    };
    let status = Status::with_details(
        Code::from_i32(rejected.code),
        rejected.message.clone(),
        rejected.details.clone().into(),
    );
    Some(map_rpc_status(from_block, to_block, status))
}

impl RemoteProver {
    /// Best-effort v2 prevalidation using the caller's complete durable range.
    /// A gap retains prior progress; final proving always backfills what is missing.
    /// Unsupported peers fail explicitly; there is no v1 proving fallback.
    pub async fn prevalidate_range(
        &self,
        rollup_id: u64,
        blocks: &[BlockWitness],
    ) -> ProverResult<()> {
        let connection = self.connection().await?;
        let chunk_size = (connection.capabilities.max_window_blocks as usize).min(64);
        for chunk in blocks.chunks(chunk_size) {
            self.prevalidate_chunk(rollup_id, chunk).await?;
        }
        Ok(())
    }

    async fn prevalidate_chunk(&self, rollup_id: u64, blocks: &[BlockWitness]) -> ProverResult<()> {
        let Some(first) = blocks.first() else {
            return Ok(());
        };
        let mut parent = None;
        for bytes in &first.witness.headers {
            let header: alloy_consensus::Header = alloy_rlp::decode_exact(bytes)
                .map_err(|_| ProverError::Backend("invalid local witness header".into()))?;
            if header.hash_slow() == first.parent_hash {
                parent = Some(header);
            }
        }
        let parent = parent
            .ok_or_else(|| ProverError::Backend("prevalidation parent header is missing".into()))?;
        let anchor = StreamAnchor {
            number: parent.number,
            hash: first.parent_hash,
            state_root: parent.state_root,
        };
        match self.stream_exchange(rollup_id, anchor, blocks, None).await {
            Ok(None) => Ok(()),
            Ok(Some(_)) => Err(ProverError::Backend(
                "unsolicited prevalidation proof".into(),
            )),
            Err(error) => Err(error),
        }
    }

    async fn open_stream(
        &self,
        state: &mut StreamingState,
        begin: Option<StreamBegin>,
        from_block: u64,
        to_block: u64,
    ) -> ProverResult<(
        tokio::sync::mpsc::Sender<ClientFrame>,
        tonic::Streaming<ServerFrame>,
        ServerFrame,
    )> {
        let connection = self.connection().await?;
        let mut client = StreamingProverClient::new(connection.channel.clone())
            .max_encoding_message_size(connection.capabilities.max_message_bytes as usize)
            .max_decoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES);
        let request_id = state.next_id();
        let expected_session = state.session.as_ref().map(|session| session.id.clone());
        let initial = ClientFrame {
            session_id: expected_session.clone().unwrap_or_default(),
            request_id,
            epoch: 0,
            kind: Some(begin.map_or(
                client_frame::Kind::Resume(StreamResume {}),
                client_frame::Kind::Begin,
            )),
        };
        let (requests, request_stream) = tokio::sync::mpsc::channel(1);
        requests
            .send(initial)
            .await
            .map_err(|_| ProverError::Backend("unable to open ProveStream".into()))?;
        let mut request =
            tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(request_stream));
        request.metadata_mut().insert_bin(
            "eez-validation-identity-bin",
            tonic::metadata::MetadataValue::from_bytes(self.inner.expected_identity.as_slice()),
        );
        let response = client.prove_stream(request).await;
        if response
            .as_ref()
            .is_err_and(|status| status.code() == Code::Unavailable)
        {
            self.invalidate_connection(&connection).await;
        }
        let mut responses = response
            .map_err(|status| {
                if status.code() == Code::Unimplemented {
                    state.session = None;
                    ProverError::Backend("offline v2 proving is required".into())
                } else {
                    map_rpc_status(from_block, to_block, status)
                }
            })?
            .into_inner();
        let ready = next_stream_response(
            &mut responses,
            request_id,
            expected_session.as_deref(),
            None,
            from_block,
            to_block,
        )
        .await?;
        Ok((requests, responses, ready))
    }

    pub(super) async fn stream_exchange(
        &self,
        rollup_id: u64,
        anchor: StreamAnchor,
        blocks: &[BlockWitness],
        finalize: Option<(u64, u64, eez_control_rpc::v2::SettlementInput)>,
    ) -> ProverResult<Option<(Vec<u8>, Vec<u8>)>> {
        // One deadline covers lock acquisition, connect, both stream directions and EOF.
        // Dropping the exchange releases its lock; a later Resume recovers accepted work.
        tokio::time::timeout(
            self.inner.exchange_timeout,
            self.stream_exchange_inner(rollup_id, anchor, blocks, finalize),
        )
        .await
        .map_err(|_| ProverError::Retryable {
            kind: RetryableProverError::DeadlineExceeded,
            message: "ProveStream exchange deadline exceeded".into(),
        })?
    }

    async fn stream_exchange_inner(
        &self,
        rollup_id: u64,
        anchor: StreamAnchor,
        blocks: &[BlockWitness],
        finalize: Option<(u64, u64, eez_control_rpc::v2::SettlementInput)>,
    ) -> ProverResult<Option<(Vec<u8>, Vec<u8>)>> {
        let mut state = self.inner.streaming.lock().await;
        if state.session.as_ref().is_some_and(|session| {
            session.rollup_id != rollup_id
                || anchor.number < session.anchor.number
                || (anchor.number == session.anchor.number && anchor.hash != session.anchor.hash)
        }) {
            state.session = None;
        }
        if state.session.as_ref().is_some_and(|session| {
            session.extended_at.is_some_and(|number| {
                finalize.is_none() || blocks.first().is_some_and(|block| block.number > number)
            })
        }) {
            // A previous proof's anchor bridge is not the next part's execution
            // history. Begin from the supplied authenticated predecessor.
            state.session = None;
        }
        let range = finalize.as_ref().map_or_else(
            || {
                (
                    blocks.first().map_or(anchor.number, |block| block.number),
                    blocks.last().map_or(anchor.number, |block| block.number),
                )
            },
            |(from, to, _)| (*from, *to),
        );
        let finalize = finalize
            .map(|(from_block, to_block, post_batch)| {
                let terminal = blocks
                    .iter()
                    .find(|block| block.number == to_block)
                    .ok_or_else(|| {
                        ProverError::Backend("Finalize terminal block was not supplied".into())
                    })?;
                Ok::<_, ProverError>(StreamFinalize {
                    from_block,
                    to_block,
                    terminal_hash: terminal.hash.to_vec(),
                    settlement: Some(post_batch),
                })
            })
            .transpose()?;

        // Retained history can fill the quota or prune a prefix needed after an
        // L1 rollback; baseline servers can also reject subrange selection.
        // Rebuild once at the exact predecessor, preserving inputs and deadline.
        let mut retry_at_anchor = state.session.is_some()
            && finalize
                .as_ref()
                .is_some_and(|f| anchor.number.checked_add(1) == Some(f.from_block));
        'attempt: loop {
            // Only the server retains the validated prefix. A fresh Ready determines
            // what to send, including work completed after a previous response was lost.
            let (requests, mut responses, blocks, mut l1_tip) = loop {
                let creating = state.session.is_none();
                if creating {
                    retry_at_anchor = false; // Already starting at the requested anchor.
                }
                let begin = creating.then(|| StreamBegin {
                    rollup_id,
                    anchor_number: anchor.number,
                    anchor_hash: anchor.hash.to_vec(),
                    anchor_state_root: anchor.state_root.to_vec(),
                });
                let (requests, mut responses, response) = self
                    .open_stream(&mut state, begin, range.0, range.1)
                    .await?;
                if !creating
                    && matches!(&response.kind,
                        Some(server_frame::Kind::Rejected(rejected)) if matches!(Code::from_i32(rejected.code), Code::NotFound | Code::Unimplemented)
                    )
                {
                    state.session = None;
                    continue; // The server evicted the session or restarted; begin at the supplied anchor.
                }
                if let Some(error) = rejection_error(&response, range.0, range.1) {
                    return Err(error);
                }
                let Some(server_frame::Kind::Ready(ready)) = response.kind else {
                    return Err(ProverError::Backend("ProveStream expected Ready".into()));
                };
                if response.session_id.len() != 32
                    || response.epoch == 0
                    || ready.validated_hash.len() != 32
                {
                    return Err(ProverError::Backend(
                        "ProveStream returned an invalid Ready binding".into(),
                    ));
                }
                if !creating
                    && state
                        .session
                        .as_ref()
                        .is_some_and(|session| response.epoch <= session.epoch)
                {
                    return Err(ProverError::Backend(
                        "Resume did not advance the session epoch".into(),
                    ));
                }
                if creating
                    && (ready.validated_through != anchor.number
                        || ready.validated_hash != anchor.hash.as_slice())
                {
                    return Err(ProverError::Backend(
                        "ProveStream Begin acknowledged another anchor".into(),
                    ));
                }
                let previous = state.session.take();
                let session_anchor = previous.as_ref().map_or(anchor, |session| session.anchor);
                let extended_at = previous.as_ref().and_then(|session| session.extended_at);
                let empty_history_anchor =
                    previous.map_or(Some(anchor.hash), |session| session.empty_history_anchor);
                state.session = Some(StreamingSession {
                    rollup_id,
                    id: response.session_id,
                    epoch: response.epoch,
                    anchor: session_anchor,
                    empty_history_anchor,
                    extended_at,
                });

                let mut skip = blocks
                    .iter()
                    .position(|block| {
                        block.number == ready.validated_through
                            && block.hash.as_slice() == ready.validated_hash
                    })
                    .map_or(0, |index| index + 1);
                if let Some(finalize) = &finalize {
                    let terminal = blocks
                        .iter()
                        .position(|block| block.number == finalize.to_block)
                        .ok_or_else(|| {
                            ProverError::Backend("missing finalization terminal".into())
                        })?;
                    skip = skip.min(terminal);
                    if let Some(index) = extended_at
                        .and_then(|number| blocks.iter().position(|block| block.number == number))
                    {
                        skip = skip.min(index);
                    }
                }
                let remaining = &blocks[skip..];
                if let Some(first) = remaining.first()
                    && (ready.validated_through.checked_add(1) != Some(first.number)
                        || ready.validated_hash != first.parent_hash.as_slice())
                {
                    if finalize.is_none()
                        && first.number > ready.validated_through.saturating_add(1)
                    {
                        // Best-effort prevalidation can skip blocks while capture is busy.
                        // Keep the checked prefix; a complete prove() will backfill the gap.
                        drop(requests);
                        drop(responses);
                        return Ok(None);
                    }
                    // Without a matching cursor, try the retained predecessor. A
                    // missing ancestor requires a fresh session and complete backfill.
                    let request_id = state
                        .send(
                            &requests,
                            client_frame::Kind::Rewind(StreamRewind {
                                ancestor_hash: first.parent_hash.to_vec(),
                            }),
                        )
                        .await?;
                    let session = state.session.as_ref().expect("ready session exists");
                    let response = next_stream_response(
                        &mut responses,
                        request_id,
                        Some(&session.id),
                        None,
                        range.0,
                        range.1,
                    )
                    .await?;
                    if !creating
                        && matches!(&response.kind,
                            Some(server_frame::Kind::Rejected(rejected)) if matches!(Code::from_i32(rejected.code), Code::FailedPrecondition | Code::Unimplemented)
                        )
                    {
                        state.session = None;
                        continue; // The requested parent is no longer retained; backfill a new session.
                    }
                    if let Some(error) = rejection_error(&response, range.0, range.1) {
                        return Err(error);
                    }
                    let Some(server_frame::Kind::Ready(ready)) = response.kind else {
                        return Err(ProverError::Backend(
                            "ProveStream expected Ready after Rewind".into(),
                        ));
                    };
                    if response.epoch <= session.epoch
                        || ready.validated_through.checked_add(1) != Some(first.number)
                        || ready.validated_hash != first.parent_hash.as_slice()
                    {
                        state.session = None;
                        return Err(ProverError::Backend(
                            "ProveStream Rewind acknowledged another parent".into(),
                        ));
                    }
                    state.session.as_mut().expect("ready session exists").epoch = response.epoch;
                }
                let empty = remaining
                    .first()
                    .is_none_or(|first| empty_history_anchor == Some(first.parent_hash));
                let l1_tip = if empty {
                    None
                } else {
                    Some(
                        remaining[0]
                            .l1_headers_rlp
                            .first()
                            .ok_or_else(|| {
                                ProverError::Backend(
                                    "missing predecessor L1 header in captured witness".into(),
                                )
                            })?
                            .clone(),
                    )
                };
                break (requests, responses, remaining, l1_tip);
            };

            // Poll both directions: a slow reader must not deadlock our request sender.
            // Keep outstanding frames bounded while draining acknowledgements.
            let total = blocks.len() + usize::from(finalize.is_some());
            let mut pending_finalize = finalize.clone();
            let mut next = 0;
            let mut pending = std::collections::VecDeque::new();
            let mut proof = None;
            while next < total || !pending.is_empty() {
                let request_id = pending.front().map_or(0, |&(id, _)| id);
                tokio::select! {
                    permit = requests.reserve(), if next < total && pending.len() < MAX_IN_FLIGHT => {
                        let permit = permit.map_err(|_| ProverError::Retryable {
                            kind: RetryableProverError::Unavailable,
                            message: "ProveStream request stream closed".into(),
                        })?;
                        let kind = if next < blocks.len() {
                            let block = &blocks[next];
                            let wire = wire_segment(block, &mut l1_tip)?;
                            if finalize.as_ref().is_some_and(|finalize| finalize.to_block == block.number) {
                                state.session.as_mut().expect("ready session exists").extended_at = Some(block.number);
                            }
                            client_frame::Kind::Block(StreamBlock { data: Some(wire) })
                        } else {
                            client_frame::Kind::Finalize(pending_finalize.take().expect("last request is Finalize"))
                        };
                        let id = state.next_id();
                        let session = state.session.as_ref().expect("Ready established a session");
                        permit.send(ClientFrame { session_id: session.id.clone(), epoch: session.epoch, request_id: id, kind: Some(kind) });
                        pending.push_back((id, next));
                        next += 1;
                    }
                    response = state.response(&mut responses, request_id, range), if !pending.is_empty() => {
                        let response = response?;
                        let (_, index) = pending.pop_front().expect("response has an outstanding request");
                        if retry_at_anchor && !response.session_id.is_empty()
                            && matches!(&response.kind, Some(server_frame::Kind::Rejected(rejected))
                                if Code::from_i32(rejected.code) == Code::ResourceExhausted
                                    || (index == blocks.len() && matches!(Code::from_i32(rejected.code),
                                        Code::InvalidArgument | Code::Unimplemented | Code::FailedPrecondition) && rejected.details.is_empty())) {
                            state.session = None;
                            continue 'attempt;
                        }
                        if let Some(error) = rejection_error(&response, range.0, range.1) {
                            if response.session_id.is_empty()
                                || error.retryable_kind() == Some(RetryableProverError::Aborted) {
                                state.session = None;
                            }
                            return Err(error);
                        }
                        if let Some(block) = blocks.get(index) {
                            if !matches!(&response.kind, Some(server_frame::Kind::Validated(validated))
                                if validated.number == block.number && validated.hash == block.hash.as_slice()
                                    && validated.post_state_root == block_header(block)?.state_root.as_slice()) {
                                return Err(ProverError::Backend("ProveStream Validated response identity mismatch".into()));
                            }

                        } else {
                            let Some(server_frame::Kind::Proof(result)) = response.kind else {
                                return Err(ProverError::Backend("ProveStream did not answer Finalize with Proof".into()));
                            };
                            let session = state.session.as_mut().expect("ready session exists");
                            session.anchor = anchor;
                            if session.empty_history_anchor != Some(anchor.hash) {
                                session.empty_history_anchor = None;
                            }
                            proof = Some((result.public_inputs_hash, result.proof));
                        }
                    }
                }
            }
            if proof.is_none() {
                drop(requests);
                drop(responses);
                return Ok(None);
            }
            drop(requests);
            return match responses.message().await {
                Ok(None) => Ok(proof),
                Ok(Some(_)) => Err(ProverError::Backend(
                    "ProveStream returned an unsolicited response".into(),
                )),
                Err(status) => Err(map_rpc_status(range.0, range.1, status)),
            };
        }
    }
}
