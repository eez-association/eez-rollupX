//! Transport regressions adapted from #206. The stub models retained progress,
//! not execution validity; native execution is covered by signer transcripts.
use super::*;
use eez_control_rpc::v2::{self, client_frame as cf, server_frame as sf};
use std::{collections::HashMap, pin::Pin, time::Duration};
use tokio::{sync::Mutex, task::JoinHandle};
use tokio_stream::{
    Stream,
    wrappers::{ReceiverStream, TcpListenerStream},
};

#[derive(Default)]
struct Retained {
    anchor: (u64, B256),
    blocks: Vec<(u64, B256, B256)>,
    epoch: u64,
}
impl Retained {
    fn tip(&self) -> (u64, B256) {
        self.blocks.last().map_or(self.anchor, |b| (b.0, b.1))
    }
}
#[derive(Default)]
struct Model {
    sessions: HashMap<Vec<u8>, Retained>,
    begins: usize,
    blocks: usize,
    resumes: usize,
    settlements: Vec<v2::SettlementInput>,
}
#[derive(Clone)]
struct StreamStub {
    model: Arc<Mutex<Model>>,
    hash: B256,
    capacity: usize,
    unsupported: Option<&'static str>,
    corrupt: Option<&'static str>,
    stall: Option<&'static str>,
    pipeline: bool,
}
impl StreamStub {
    fn new(ctx: &ProvingContext) -> Self {
        Self {
            model: Arc::default(),
            hash: request_hash(ctx, test_key().address().into_word()).unwrap(),
            capacity: 512,
            unsupported: None,
            corrupt: None,
            stall: None,
            pipeline: false,
        }
    }
}
#[tonic::async_trait]
impl v2::prover_server::Prover for StreamStub {
    type ProveStreamStream = Pin<Box<dyn Stream<Item = Result<v2::ServerFrame, Status>> + Send>>;
    async fn prove_stream(
        &self,
        request: Request<Streaming<v2::ClientFrame>>,
    ) -> Result<Response<Self::ProveStreamStream>, Status> {
        assert_eq!(
            request
                .metadata()
                .get_bin("eez-validation-identity-bin")
                .unwrap()
                .to_bytes()
                .unwrap()
                .as_ref(),
            TEST_IDENTITY.as_slice()
        );
        let mut input = request.into_inner();
        let this = self.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            let mut staged = Vec::new();
            while let Ok(Some(frame)) = input.message().await {
                let mut model = this.model.lock().await;
                let mut id = frame.session_id.clone();
                let mut epoch = frame.epoch;
                let rejected = |code: Code, tip: (u64, B256)| {
                    sf::Kind::Rejected(v2::Rejected {
                        code: code.into(),
                        message: "test rejection".into(),
                        details: vec![],
                        validated_through: tip.0,
                        validated_hash: tip.1.to_vec(),
                    })
                };
                let mut done = false;
                let mut phase = "block";
                let kind = match frame.kind.unwrap() {
                    cf::Kind::Begin(begin) => {
                        phase = "ready";
                        model.begins += 1;
                        id = B256::from(alloy_primitives::U256::from(model.begins)).to_vec();
                        epoch = 1;
                        model.sessions.insert(
                            id.clone(),
                            Retained {
                                anchor: (begin.anchor_number, B256::from_slice(&begin.anchor_hash)),
                                epoch,
                                ..Default::default()
                            },
                        );
                        sf::Kind::Ready(v2::Ready {
                            validated_through: begin.anchor_number,
                            validated_hash: begin.anchor_hash,
                        })
                    }
                    cf::Kind::Resume(_) => {
                        phase = "ready";
                        model.resumes += 1;
                        if this.unsupported == Some("resume") {
                            id.clear();
                            rejected(Code::Unimplemented, (0, B256::ZERO))
                        } else if let Some(session) = model.sessions.get_mut(&id) {
                            session.epoch += 1;
                            epoch = session.epoch;
                            let tip = session.tip();
                            sf::Kind::Ready(v2::Ready {
                                validated_through: tip.0,
                                validated_hash: tip.1.to_vec(),
                            })
                        } else {
                            id.clear();
                            rejected(Code::NotFound, (0, B256::ZERO))
                        }
                    }
                    cf::Kind::Rewind(rewind) => {
                        let session = model.sessions.get_mut(&id).unwrap();
                        let keep = if rewind.ancestor_hash == session.anchor.1.as_slice() {
                            Some(0)
                        } else {
                            session
                                .blocks
                                .iter()
                                .position(|b| rewind.ancestor_hash == b.1.as_slice())
                                .map(|i| i + 1)
                        };
                        if this.unsupported == Some("rewind") {
                            rejected(Code::Unimplemented, session.tip())
                        } else if let Some(keep) = keep {
                            session.blocks.truncate(keep);
                            session.epoch += 1;
                            epoch = session.epoch;
                            let tip = session.tip();
                            sf::Kind::Ready(v2::Ready {
                                validated_through: tip.0,
                                validated_hash: tip.1.to_vec(),
                            })
                        } else {
                            rejected(Code::FailedPrecondition, session.tip())
                        }
                    }
                    cf::Kind::Block(block) => {
                        let block = block.data.unwrap();
                        let decoded: eez_primitives::Block =
                            alloy_rlp::decode_exact(&block.rlp).unwrap();
                        let session = model.sessions.get_mut(&id).unwrap();
                        if session.blocks.len() == this.capacity {
                            rejected(Code::ResourceExhausted, session.tip())
                        } else {
                            assert_eq!(
                                session.tip(),
                                (block.number - 1, B256::from_slice(&block.parent_hash))
                            );
                            session.blocks.push((
                                block.number,
                                B256::from_slice(&block.hash),
                                decoded.header.state_root,
                            ));
                            model.blocks += 1;
                            if this.corrupt == Some("session") {
                                id = vec![0xff; 32];
                            }
                            sf::Kind::Validated(v2::Validated {
                                number: block.number,
                                hash: block.hash,
                                post_state_root: if this.corrupt == Some("state") {
                                    vec![0xff; 32]
                                } else {
                                    decoded.header.state_root.to_vec()
                                },
                                reused: false,
                            })
                        }
                    }
                    cf::Kind::Finalize(finalize) => {
                        let session = model.sessions.get_mut(&id).unwrap();
                        if finalize.from_block <= session.anchor.0 {
                            rejected(Code::FailedPrecondition, session.tip())
                        } else if this.unsupported == Some("subrange")
                            && finalize.from_block != session.anchor.0 + 1
                        {
                            rejected(Code::Unimplemented, session.tip())
                        } else {
                            let start = (finalize.from_block - session.anchor.0 - 1) as usize;
                            if start > 0 {
                                session.anchor =
                                    (session.blocks[start - 1].0, session.blocks[start - 1].1);
                                session.blocks.drain(..start);
                            }
                            model.settlements.push(finalize.settlement.unwrap());
                            done = true;
                            sf::Kind::Proof(v2::Proof {
                                public_inputs_hash: this.hash.to_vec(),
                                proof: sign_65(&test_key(), this.hash),
                            })
                        }
                    }
                    cf::Kind::Cancel(_) => unreachable!(),
                };
                drop(model);
                if this.stall == Some(phase) {
                    tx.closed().await;
                    return;
                }
                let response = Ok(v2::ServerFrame {
                    session_id: id,
                    epoch,
                    request_id: frame.request_id,
                    kind: Some(kind),
                });
                if this.pipeline && phase != "ready" {
                    staged.push(response);
                    if !done {
                        continue;
                    }
                    assert_eq!(
                        staged.len(),
                        3,
                        "two blocks and Finalize must arrive before their ACKs"
                    );
                    for reply in staged.drain(..) {
                        if tx.send(reply).await.is_err() {
                            return;
                        }
                    }
                } else if tx.send(response).await.is_err() {
                    return;
                }
                if done {
                    if this.stall == Some("eof") {
                        tx.closed().await;
                    }
                    return;
                }
            }
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}

struct ServerTask(JoinHandle<()>);
impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn serve(stub: StreamStub) -> (RemoteProver, ServerTask) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (headers, _) = tokio::sync::mpsc::unbounded_channel();
    let v1 = StubProver {
        signer: test_key(),
        hash: stub.hash,
        headers,
        retentions: None,
        capabilities: test_capabilities(),
        capability_calls: Arc::default(),
    };
    let task = tokio::spawn(async move {
        Server::builder()
            .add_service(ProverServer::new(v1))
            .add_service(
                v2::prover_server::ProverServer::new(stub)
                    .max_decoding_message_size(eez_control_rpc::MAX_MESSAGE_BYTES),
            )
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    (
        RemoteProver::new(endpoint, test_key().address(), TEST_IDENTITY),
        ServerTask(task),
    )
}

fn context(count: usize) -> ProvingContext {
    let mut ctx = test_context();
    let mut parent = alloy_consensus::Header {
        state_root: B256::repeat_byte(0x41),
        ..Default::default()
    };
    ctx.parent_provenance.parent = parent.clone();
    let mut blocks = Vec::new();
    for number in 1..=count as u64 {
        let header = alloy_consensus::Header {
            number,
            parent_hash: parent.hash_slow(),
            state_root: B256::from(alloy_primitives::U256::from(number)),
            ..Default::default()
        };
        let mut block = eez_prover::BlockWitness {
            number,
            hash: header.hash_slow(),
            parent_hash: header.parent_hash,
            rlp: alloy_rlp::encode(eez_primitives::Block::new(
                header.clone(),
                Default::default(),
            ))
            .into(),
            witness: Default::default(),
        };
        block.witness.headers = vec![alloy_rlp::encode(&parent).into()];
        blocks.push(block);
        parent = header;
    }
    ctx.from_block = 1;
    ctx.to_block = count as u64;
    ctx.blocks = blocks.into();
    ctx
}
fn selected(ctx: &ProvingContext, from: u64, to: u64) -> ProvingContext {
    let mut next = ctx.clone();
    next.from_block = from;
    next.to_block = to;
    next.blocks = ctx.blocks[(from - 1) as usize..to as usize].to_vec().into();
    if from > 1 {
        let parent: eez_primitives::Block =
            alloy_rlp::decode_exact(&ctx.blocks[(from - 2) as usize].rlp).unwrap();
        next.parent_provenance.parent = parent.header;
    }
    next
}

#[tokio::test]
async fn retained_capacity_recovers_once_at_requested_anchor() {
    let ctx = context(600);
    let stub = StreamStub::new(&ctx);
    let model = Arc::clone(&stub.model);
    let (prover, _server) = serve(stub).await;
    prover.prove(selected(&ctx, 1, 300)).await.unwrap();
    prover.prove(selected(&ctx, 301, 600)).await.unwrap();
    let model = model.lock().await;
    assert_eq!(model.begins, 2);
    assert_eq!(model.blocks, 812);
    assert_eq!(model.settlements.len(), 2);
}

#[tokio::test]
async fn rollback_below_promoted_anchor_backfills_identical_blocks() {
    let ctx = context(100);
    let stub = StreamStub::new(&ctx);
    let model = Arc::clone(&stub.model);
    let (prover, _server) = serve(stub).await;
    prover.prove(ctx.clone()).await.unwrap();
    prover.prove(selected(&ctx, 51, 100)).await.unwrap();
    assert_eq!(model.lock().await.blocks, 100);
    prover.prove(ctx).await.unwrap();
    let model = model.lock().await;
    assert_eq!(model.begins, 2);
    assert_eq!(model.blocks, 200);
    assert_eq!(model.settlements.len(), 3);
}

#[tokio::test]
async fn skipped_prevalidation_keeps_prefix_and_final_proof_backfills_gap() {
    let ctx = context(3);
    let stub = StreamStub::new(&ctx);
    let model = Arc::clone(&stub.model);
    let (prover, _server) = serve(stub).await;
    assert_eq!(
        prover.prevalidate_range(1, &ctx.blocks[..1]).await.unwrap(),
        Prevalidation::Available
    );
    prover.prevalidate_range(1, &ctx.blocks[2..]).await.unwrap();
    assert_eq!(model.lock().await.blocks, 1);
    prover.prove(ctx).await.unwrap();
    let model = model.lock().await;
    assert_eq!(model.begins, 1);
    assert_eq!(model.blocks, 3);
    assert_eq!(model.resumes, 2);
}

#[tokio::test]
async fn optional_controls_fall_back_without_omitting_native_settlement() {
    for operation in ["resume", "rewind", "subrange"] {
        let ctx = context(2);
        let mut stub = StreamStub::new(&ctx);
        stub.unsupported = Some(operation);
        let model = Arc::clone(&stub.model);
        let (prover, _server) = serve(stub).await;
        prover.prevalidate_range(1, &ctx.blocks).await.unwrap();
        let request = match operation {
            "rewind" => selected(&ctx, 1, 1),
            "subrange" => selected(&ctx, 2, 2),
            _ => ctx.clone(),
        };
        prover.prove(request.clone()).await.unwrap();
        let model = model.lock().await;
        assert_eq!(model.begins, 2, "{operation}");
        let input = &model.settlements[0];
        assert_eq!(
            input.blobs,
            request
                .blobs
                .iter()
                .map(|b| b.as_bytes().to_vec())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            input.abi_calldata,
            eez_protocol::abi::postAndVerifyBatchCall {
                batch: request.batch
            }
            .abi_encode()
        );
        let target = input.posting_context.as_ref().unwrap();
        assert_eq!(target.parent_hash, request.l1_posting.parent_hash.to_vec());
        assert_eq!(target.target_timestamp, request.l1_posting.target_timestamp);
    }
}

#[tokio::test]
async fn server_restart_loses_only_volatile_progress() {
    let ctx = context(2);
    let stub = StreamStub::new(&ctx);
    let model = Arc::clone(&stub.model);
    let (prover, _server) = serve(stub).await;
    prover.prevalidate_range(1, &ctx.blocks[..1]).await.unwrap();
    model.lock().await.sessions.clear();
    prover.prove(ctx).await.unwrap();
    let model = model.lock().await;
    assert_eq!(model.begins, 2);
    assert_eq!(model.blocks, 3);
}

#[tokio::test]
async fn pipelining_drains_both_directions_before_first_ack() {
    let ctx = context(2);
    let mut stub = StreamStub::new(&ctx);
    stub.pipeline = true;
    let (prover, _server) = serve(stub).await;
    tokio::time::timeout(Duration::from_secs(5), prover.prove(ctx))
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn cross_session_wrong_state_and_wrong_digest_fail_closed() {
    for corrupt in ["session", "state", "digest"] {
        let ctx = context(1);
        let mut stub = StreamStub::new(&ctx);
        let expected = match corrupt {
            "session" => "ProveStream response crossed Composer sessions".to_owned(),
            "state" => "ProveStream Validated response identity mismatch".to_owned(),
            "digest" => format!(
                "attestation publicInputsHash {} != request hash {}",
                B256::repeat_byte(0xf1),
                stub.hash,
            ),
            _ => unreachable!(),
        };
        if corrupt == "digest" {
            stub.hash = B256::repeat_byte(0xf1);
        } else {
            stub.corrupt = Some(corrupt);
        }
        let (prover, _server) = serve(stub).await;
        let error = prover.prove(ctx).await.unwrap_err();
        assert!(
            matches!(&error, ProverError::Backend(message) if message == &expected),
            "{corrupt}: {error:?}"
        );
        assert!(error.actionable_failure().is_none());
    }
}

#[tokio::test]
async fn exchange_deadline_covers_ready_block_eof_and_mutex_waits() {
    for phase in ["ready", "block", "eof", "lock"] {
        let ctx = context(1);
        let mut stub = StreamStub::new(&ctx);
        stub.stall = Some(phase);
        let (mut prover, _server) = serve(stub).await;
        Arc::get_mut(&mut prover.inner).unwrap().exchange_timeout = Duration::from_millis(50);
        prover.capabilities().await.unwrap();
        let guard = if phase == "lock" {
            Some(prover.inner.streaming.lock().await)
        } else {
            None
        };
        let error = prover.prove(ctx).await.unwrap_err();
        assert_eq!(
            error.retryable_kind(),
            Some(RetryableProverError::DeadlineExceeded),
            "{phase}: {error:?}"
        );
        drop(guard);
    }
}
