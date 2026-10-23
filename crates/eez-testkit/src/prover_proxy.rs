//! Bounded v2 mutation/capture adapter for the real signer process tests.
use super::*;
use eez_control_rpc::v2::{
    Begin, ClientFrame, Finalize, ServerFrame, client_frame,
    prover_client::ProverClient as StreamClient,
    prover_server::{Prover as StreamProver, ProverServer as StreamServer},
    server_frame,
};
use prost::Message as _;
use std::collections::{BTreeMap, VecDeque};
use tokio_stream::wrappers::ReceiverStream;

const CAPTURE_BYTES: usize = 256 * 1024 * 1024;
const CAPTURE_BLOCKS: usize = 4096;
const CAPTURE_SESSIONS: usize = 8;

#[derive(Clone, Copy, Debug)]
pub enum ProverMutation {
    None,
    PostBatch,
    Witness,
    ResourceExhausted,
    AllResourceExhausted,
    Unavailable,
    MismatchedActionable,
}

impl ProverMutation {
    fn apply(self, frame: &mut ClientFrame) -> Result<(), Status> {
        if let (Self::Witness, Some(client_frame::Kind::Block(block))) = (self, &mut frame.kind) {
            let witness = block
                .data
                .as_mut()
                .and_then(|block| block.witness.as_mut())
                .ok_or_else(|| Status::internal("missing fixture block witness"))?;
            // Corrupt every node: a single surplus node need not be used by execution.
            for byte in witness.state.iter_mut().filter_map(|node| node.first_mut()) {
                *byte ^= 1;
            }
            if witness.state.is_empty() {
                witness.state.push(vec![0xff]);
            }
        }
        let Some(client_frame::Kind::Finalize(finalize)) = &mut frame.kind else {
            return Ok(());
        };
        let settlement = finalize
            .settlement
            .as_mut()
            .ok_or_else(|| Status::internal("missing fixture settlement input"))?;
        match self {
            Self::None | Self::Witness => Ok(()),
            Self::PostBatch => {
                let byte = settlement
                    .abi_calldata
                    .last_mut()
                    .ok_or_else(|| Status::internal("empty fixture calldata"))?;
                *byte ^= 1;
                Ok(())
            }
            Self::AllResourceExhausted => Err(Status::resource_exhausted(
                "fixture manual prover request limit",
            )),
            Self::Unavailable => Err(Status::unavailable("fixture temporary upstream outage")),
            Self::ResourceExhausted | Self::MismatchedActionable => {
                let batch = eez_protocol::entries::decode_postbatch(&settlement.abi_calldata)
                    .map_err(|error| {
                        Status::internal(format!("decode fixture PostBatch: {error}"))
                    })?;
                if batch.entries.len() <= 1 {
                    return Ok(());
                }
                if matches!(self, Self::ResourceExhausted) {
                    return Err(Status::resource_exhausted(
                        "prover resource capacity exceeded",
                    ));
                }
                let failure = ProveFailure {
                    actionable_failure: Some(prove_failure::ActionableFailure::Inbound(
                        InboundFailure {
                            entry_index: u32::MAX,
                            entry_hash: vec![0xff; 32],
                        },
                    )),
                };
                Err(Status::with_details(
                    tonic::Code::FailedPrecondition,
                    "candidate identity does not match the request",
                    encode_prove_failure(&failure).into(),
                ))
            }
        }
    }
}

#[derive(Debug, Default)]
struct ProxyCounters {
    attempts: AtomicUsize,
    successes: AtomicUsize,
    rejections: AtomicUsize,
}

/// Retain only ACKed frames. The original Begin and executed prefix are kept
/// through subrange proofs so exported input can be replayed by a cold signer.
#[derive(Debug)]
struct Capture {
    begin: Begin,
    blocks: Vec<eez_control_rpc::v1::BlockWitness>,
    bytes: usize,
    attached_epoch: Option<u64>,
}

/// Mirror the signer's oldest-disconnected eviction rule. Connection teardown
/// is epoch-fenced so an old stream cannot detach a resumed capture.
#[derive(Debug, Default)]
struct CaptureBook {
    sessions: BTreeMap<Vec<u8>, Capture>,
    insertion_order: VecDeque<Vec<u8>>,
}

impl CaptureBook {
    fn begin(&mut self, id: Vec<u8>, begin: Begin, epoch: u64) -> Result<(), Status> {
        while self.sessions.len() >= CAPTURE_SESSIONS {
            let index = self
                .insertion_order
                .iter()
                .position(|id| {
                    self.sessions
                        .get(id)
                        .is_some_and(|capture| capture.attached_epoch.is_none())
                })
                .ok_or_else(|| {
                    Status::resource_exhausted("fixture active session capture bound")
                })?;
            let old = self
                .insertion_order
                .remove(index)
                .expect("index found above");
            self.sessions.remove(&old);
        }
        self.insertion_order.push_back(id.clone());
        self.sessions.insert(
            id,
            Capture {
                begin,
                blocks: vec![],
                bytes: 0,
                attached_epoch: Some(epoch),
            },
        );
        Ok(())
    }

    fn detach(&mut self, id: &[u8], epoch: u64) {
        if let Some(capture) = self.sessions.get_mut(id)
            && capture.attached_epoch == Some(epoch)
        {
            capture.attached_epoch = None;
        }
    }

    fn remove(&mut self, id: &[u8]) {
        self.sessions.remove(id);
        self.insertion_order.retain(|entry| entry != id);
    }
}

impl Capture {
    fn append(&mut self, block: eez_control_rpc::v1::BlockWitness) -> Result<(), Status> {
        if let Some(prior) = self
            .blocks
            .iter()
            .find(|prior| prior.number == block.number)
        {
            return if prior == &block {
                Ok(())
            } else {
                Err(Status::internal("conflicting ACKed fixture block"))
            };
        }
        let (number, hash) = self.blocks.last().map_or(
            (self.begin.anchor_number, &self.begin.anchor_hash),
            |last| (last.number, &last.hash),
        );
        if number.checked_add(1) != Some(block.number) || hash != &block.parent_hash {
            return Err(Status::internal("ACKed fixture ancestry gap"));
        }
        let bytes = self.bytes.saturating_add(block.encoded_len());
        if self.blocks.len() >= CAPTURE_BLOCKS || bytes > CAPTURE_BYTES {
            return Err(Status::resource_exhausted("fixture prefix capture bound"));
        }
        self.bytes = bytes;
        self.blocks.push(block);
        Ok(())
    }

    fn rewind(&mut self, hash: &[u8]) -> Result<(), Status> {
        let keep = if self.begin.anchor_hash == hash {
            0
        } else {
            self.blocks
                .iter()
                .position(|block| block.hash == hash)
                .ok_or_else(|| Status::internal("fixture rewind outside captured prefix"))?
                + 1
        };
        self.blocks.truncate(keep);
        self.bytes = self.blocks.iter().map(prost::Message::encoded_len).sum();
        Ok(())
    }

    fn encode(&self, finalize: &Finalize) -> Result<Vec<u8>, Status> {
        let end = self
            .blocks
            .iter()
            .position(|block| {
                block.number == finalize.to_block && block.hash == finalize.terminal_hash
            })
            .ok_or_else(|| Status::internal("fixture terminal outside captured prefix"))?;
        let mut encoded = Vec::new();
        let mut append = |request_id: u64, kind: client_frame::Kind| -> Result<(), Status> {
            let frame = ClientFrame {
                session_id: vec![],
                request_id,
                epoch: 0,
                kind: Some(kind),
            };
            if encoded
                .len()
                .saturating_add(frame.encoded_len())
                .saturating_add(10)
                > CAPTURE_BYTES
            {
                return Err(Status::resource_exhausted("fixture wire capture bound"));
            }
            frame
                .encode_length_delimited(&mut encoded)
                .expect("Vec encoding");
            Ok(())
        };
        append(1, client_frame::Kind::Begin(self.begin.clone()))?;
        for (index, block) in self.blocks[..=end].iter().enumerate() {
            append(
                index as u64 + 2,
                client_frame::Kind::Block(eez_control_rpc::v2::Block {
                    data: Some(block.clone()),
                }),
            )?;
        }
        append(
            end as u64 + 3,
            client_frame::Kind::Finalize(finalize.clone()),
        )?;
        Ok(encoded)
    }
}

#[derive(Clone, Debug)]
struct ProverProxyService {
    upstream: String,
    fixture_directory: Option<PathBuf>,
    captures: Arc<Mutex<CaptureBook>>,
    mutation: Arc<Mutex<ProverMutation>>,
    counters: Arc<ProxyCounters>,
}

#[tonic::async_trait]
impl Prover for ProverProxyService {
    async fn capabilities(
        &self,
        request: Request<eez_control_rpc::v1::CapabilitiesRequest>,
    ) -> Result<Response<eez_control_rpc::v1::CapabilitiesResponse>, Status> {
        ProverClient::connect(self.upstream.clone())
            .await
            .map_err(|error| Status::unavailable(error.to_string()))?
            .capabilities(request)
            .await
    }
}

impl ProverProxyService {
    fn rejection(&self, code: tonic::Code) {
        if matches!(
            code,
            tonic::Code::InvalidArgument
                | tonic::Code::FailedPrecondition
                | tonic::Code::ResourceExhausted
        ) {
            self.counters.rejections.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn capture(
        &self,
        request: &ClientFrame,
        response: &ServerFrame,
    ) -> Result<Option<(Vec<u8>, eez_control_rpc::v2::Proof)>, Status> {
        if self.fixture_directory.is_none() {
            return Ok(None);
        }
        let mut captures = self
            .captures
            .lock()
            .map_err(|_| Status::internal("fixture captures poisoned"))?;
        match (&request.kind, &response.kind) {
            (Some(client_frame::Kind::Begin(begin)), Some(server_frame::Kind::Ready(_))) => {
                captures.begin(response.session_id.clone(), begin.clone(), response.epoch)?;
            }
            (Some(client_frame::Kind::Block(block)), Some(server_frame::Kind::Validated(_))) => {
                let capture = captures
                    .sessions
                    .get_mut(&response.session_id)
                    .ok_or_else(|| Status::internal("fixture session absent"))?;
                capture.append(
                    block
                        .data
                        .clone()
                        .ok_or_else(|| Status::internal("fixture block absent"))?,
                )?;
                let total: usize = captures
                    .sessions
                    .values()
                    .map(|capture| capture.bytes)
                    .sum();
                if total > CAPTURE_BYTES {
                    return Err(Status::resource_exhausted(
                        "fixture aggregate capture bound",
                    ));
                }
            }
            (Some(client_frame::Kind::Rewind(rewind)), Some(server_frame::Kind::Ready(_))) => {
                let capture = captures
                    .sessions
                    .get_mut(&response.session_id)
                    .ok_or_else(|| Status::internal("fixture rewind session absent"))?;
                capture.rewind(&rewind.ancestor_hash)?;
                capture.attached_epoch = Some(response.epoch);
            }
            (Some(client_frame::Kind::Resume(_)), Some(server_frame::Kind::Ready(_))) => {
                captures
                    .sessions
                    .get_mut(&response.session_id)
                    .ok_or_else(|| Status::internal("fixture resume session absent"))?
                    .attached_epoch = Some(response.epoch);
            }
            (
                Some(client_frame::Kind::Finalize(finalize)),
                Some(server_frame::Kind::Proof(proof)),
            ) => {
                let capture = captures
                    .sessions
                    .get(&response.session_id)
                    .ok_or_else(|| Status::internal("fixture proof session absent"))?;
                return Ok(Some((capture.encode(finalize)?, proof.clone())));
            }
            (Some(client_frame::Kind::Cancel(_)), Some(server_frame::Kind::Cancelled(_))) => {
                captures.remove(&response.session_id);
            }
            _ => {}
        }
        Ok(None)
    }
}

#[tonic::async_trait]
impl StreamProver for ProverProxyService {
    type ProveStreamStream = ReceiverStream<Result<ServerFrame, Status>>;
    async fn prove_stream(
        &self,
        request: Request<Streaming<ClientFrame>>,
    ) -> Result<Response<Self::ProveStreamStream>, Status> {
        let (metadata, extensions, mut input) = request.into_parts();
        let (up_tx, up_rx) = tokio::sync::mpsc::channel(8);
        let mut client = StreamClient::connect(self.upstream.clone())
            .await
            .map_err(|error| Status::unavailable(error.to_string()))?
            .max_encoding_message_size(MAX_MESSAGE_BYTES)
            .max_decoding_message_size(MAX_MESSAGE_BYTES);
        let mut upstream = client
            .prove_stream(Request::from_parts(
                metadata,
                extensions,
                ReceiverStream::new(up_rx),
            ))
            .await?
            .into_inner();
        let (out_tx, out_rx) = tokio::sync::mpsc::channel(8);
        let service = self.clone();
        tokio::spawn(async move {
            let mut pending = BTreeMap::new();
            let mut input_open = true;
            let mut sender = Some(up_tx);
            let mut bound = None;
            let result: Result<(), Status> = async {
                loop {
                    tokio::select! {
                        _ = out_tx.closed() => return Ok(()),
                        next = input.message(), if input_open && pending.len() < 16 => {
                            let Some(mut frame) = next? else { input_open = false; sender.take(); continue };
                            if matches!(frame.kind, Some(client_frame::Kind::Finalize(_))) {
                                service.counters.attempts.fetch_add(1, Ordering::Relaxed);
                            }
                            let mutation = *service.mutation.lock().map_err(|_| Status::internal("fixture mutation poisoned"))?;
                            if matches!(mutation, ProverMutation::Witness) && matches!(frame.kind, Some(client_frame::Kind::Block(_))) {
                                service.counters.attempts.fetch_add(1, Ordering::Relaxed);
                            }
                            mutation.apply(&mut frame)?;
                            if service.fixture_directory.is_some() { pending.insert(frame.request_id, frame.clone()); }
                            sender.as_ref().expect("input is still open").send(frame).await.map_err(|_| Status::unavailable("fixture upstream closed"))?;
                        }
                        next = upstream.message() => {
                            let Some(frame) = next? else { return Ok(()) };
                            if matches!(frame.kind, Some(server_frame::Kind::Ready(_))) {
                                bound = Some((frame.session_id.clone(), frame.epoch));
                            }
                            if let Some(server_frame::Kind::Rejected(rejected)) = &frame.kind { service.rejection(tonic::Code::from_i32(rejected.code)); }
                            if let Some(request) = pending.remove(&frame.request_id)
                                && let Some((encoded, proof)) = service.capture(&request, &frame)? {
                                    let directory = service.fixture_directory.clone().expect("capture requires directory");
                                    tokio::task::spawn_blocking(move || -> Result<(), Status> {
                                        let hash = alloy_primitives::keccak256(&encoded);
                                        let path = directory.join(format!("{hash:x}"));
                                        std::fs::write(path.with_extension("v2.protobuf"), &encoded)
                                            .map_err(|error| Status::internal(format!("write v2 fixture: {error}")))?;
                                        std::fs::write(path.with_extension("json"), serde_json::to_vec_pretty(&serde_json::json!({
                                            "request_hash": hash, "request_bytes": encoded.len(),
                                            "public_inputs_hash": hex::encode(proof.public_inputs_hash), "signature": hex::encode(proof.proof),
                                        })).expect("fixture metadata serialization"))
                                            .map_err(|error| Status::internal(format!("write fixture metadata: {error}")))?;
                                        Ok(())
                                    }).await.map_err(|_| Status::internal("fixture writer failed"))??;
                            }
                            let complete = matches!(frame.kind, Some(server_frame::Kind::Proof(_)));
                            if complete { service.counters.successes.fetch_add(1, Ordering::Relaxed); }
                            out_tx.send(Ok(frame)).await.map_err(|_| Status::cancelled("fixture client closed"))?;
                        }
                    }
                }
            }.await;
            if let Some((id, epoch)) = bound
                && let Ok(mut captures) = service.captures.lock()
            {
                captures.detach(&id, epoch);
            }
            if let Err(status) = result {
                service.rejection(status.code());
                let _ = out_tx.send(Err(status)).await;
            }
        });
        Ok(Response::new(ReceiverStream::new(out_rx)))
    }
}

/// Optional proxy; every successful proof still comes from the real signer.
#[derive(Debug)]
pub struct ProverProxyHandle {
    endpoint: String,
    counters: Arc<ProxyCounters>,
    mutation: Arc<Mutex<ProverMutation>>,
    task: JoinHandle<()>,
}
impl ProverProxyHandle {
    pub(super) async fn spawn(
        upstream: String,
        mutation: ProverMutation,
        fixture_directory: Option<PathBuf>,
    ) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let counters = Arc::new(ProxyCounters::default());
        let mutation = Arc::new(Mutex::new(mutation));
        let service = ProverProxyService {
            upstream,
            fixture_directory,
            captures: Arc::default(),
            mutation: mutation.clone(),
            counters: counters.clone(),
        };
        let task = tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(
                    ProverServer::new(service.clone())
                        .max_decoding_message_size(MAX_MESSAGE_BYTES)
                        .max_encoding_message_size(MAX_MESSAGE_BYTES),
                )
                .add_service(
                    StreamServer::new(service)
                        .max_decoding_message_size(MAX_MESSAGE_BYTES)
                        .max_encoding_message_size(MAX_MESSAGE_BYTES),
                )
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await;
        });
        Ok(Self {
            endpoint: format!("http://{addr}"),
            counters,
            mutation,
            task,
        })
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub fn attempts(&self) -> usize {
        self.counters.attempts.load(Ordering::Relaxed)
    }
    pub fn successes(&self) -> usize {
        self.counters.successes.load(Ordering::Relaxed)
    }
    pub fn rejections(&self) -> usize {
        self.counters.rejections.load(Ordering::Relaxed)
    }
    pub fn set_mutation(&self, mutation: ProverMutation) {
        *self.mutation.lock().unwrap() = mutation;
    }
}
impl Drop for ProverProxyHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cold_capture_retains_exact_prefix_and_replaces_rewound_terminal() {
        let block = |number, hash, parent| eez_control_rpc::v1::BlockWitness {
            number,
            hash: vec![hash; 32],
            parent_hash: vec![parent; 32],
            ..Default::default()
        };
        let mut capture = Capture {
            begin: Begin {
                rollup_id: 1,
                anchor_number: 0,
                anchor_hash: vec![0; 32],
                anchor_state_root: vec![9; 32],
            },
            blocks: vec![],
            bytes: 0,
            attached_epoch: None,
        };
        capture.append(block(1, 1, 0)).unwrap();
        capture.append(block(2, 2, 1)).unwrap();
        capture.append(block(2, 2, 1)).unwrap();
        assert!(capture.append(block(2, 9, 1)).is_err());
        assert!(capture.append(block(4, 4, 2)).is_err());
        capture.rewind(&[1; 32]).unwrap();
        capture.append(block(2, 3, 1)).unwrap();
        let finalize = Finalize {
            from_block: 2,
            to_block: 2,
            terminal_hash: vec![3; 32],
            settlement: None,
        };
        let encoded = capture.encode(&finalize).unwrap();
        let mut input = encoded.as_slice();
        let mut frames = Vec::new();
        while !input.is_empty() {
            frames.push(ClientFrame::decode_length_delimited(&mut input).unwrap());
        }
        assert_eq!(frames.len(), 4);
        assert!(
            matches!(&frames[2].kind, Some(client_frame::Kind::Block(b)) if b.data.as_ref().unwrap().hash == vec![3;32])
        );
        assert!(
            capture
                .encode(&Finalize {
                    terminal_hash: vec![2; 32],
                    ..finalize
                })
                .is_err()
        );
    }

    #[test]
    fn capture_eviction_preserves_active_and_resumed_sessions() {
        let mut book = CaptureBook::default();
        for id in 1..=8 {
            book.begin(vec![id; 32], Begin::default(), 1).unwrap();
        }
        assert!(book.begin(vec![9; 32], Begin::default(), 1).is_err());
        book.detach(&[2; 32], 1);
        book.detach(&[4; 32], 1);
        book.begin(vec![9; 32], Begin::default(), 1).unwrap();
        assert!(!book.sessions.contains_key(&[2; 32][..]));
        assert!(book.sessions.contains_key(&[1; 32][..]));
        book.sessions.get_mut(&[4; 32][..]).unwrap().attached_epoch = Some(2);
        book.detach(&[4; 32], 1);
        assert_eq!(book.sessions[&[4; 32][..]].attached_epoch, Some(2));
        book.detach(&[4; 32], 2);
        book.begin(vec![10; 32], Begin::default(), 1).unwrap();
        assert!(!book.sessions.contains_key(&[4; 32][..]));
        assert_eq!(book.sessions.len(), CAPTURE_SESSIONS);
    }
}
