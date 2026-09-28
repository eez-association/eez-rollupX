//! RPC limits, ingestion, deadlines, request-slot admission, and worker lifecycle.

use super::*;

#[tokio::test]
async fn final_proof_waiter_wins_the_next_preparation_block_boundary() {
    let svc = ProveSvc::new(unused_validator(), limits());
    let blocker = Arc::clone(&svc.validation_worker)
        .acquire_owned()
        .await
        .unwrap();
    let worker = Arc::clone(&svc.validation_worker);
    let (first_started_tx, first_started_rx) = oneshot::channel();
    let (release_first_tx, release_first_rx) = oneshot::channel();
    let (preparation_reacquired_tx, mut preparation_reacquired_rx) = mpsc::channel(1);
    let preparation = tokio::spawn(async move {
        let first = Arc::clone(&worker).acquire_owned().await.unwrap();
        first_started_tx.send(()).unwrap();
        release_first_rx.await.unwrap();
        drop(first);
        let _second = worker.acquire_owned().await.unwrap();
        preparation_reacquired_tx.send(()).await.unwrap();
    });
    tokio::task::yield_now().await;
    drop(blocker);
    first_started_rx.await.unwrap();

    let worker = Arc::clone(&svc.validation_worker);
    let (final_acquired_tx, final_acquired_rx) = oneshot::channel();
    let (release_final_tx, release_final_rx) = oneshot::channel();
    let final_proof = tokio::spawn(async move {
        let _permit = worker.acquire_owned().await.unwrap();
        final_acquired_tx.send(()).unwrap();
        release_final_rx.await.unwrap();
    });
    tokio::task::yield_now().await;
    release_first_tx.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(1), final_acquired_rx)
        .await
        .expect("queued final proof did not receive the next worker boundary")
        .unwrap();
    assert!(
        preparation_reacquired_rx.try_recv().is_err(),
        "preparation reacquired before the already queued final proof"
    );
    release_final_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), preparation_reacquired_rx.recv())
        .await
        .expect("preparation did not resume after final proof")
        .expect("preparation task ended before reacquiring");
    final_proof.await.unwrap();
    preparation.await.unwrap();
}

#[tokio::test]
async fn missing_or_changed_validation_identity_rejects_before_witnesses_or_signing() {
    let state = unused_validator();
    let server = TestServer::new(Arc::clone(&state)).await;
    for (identity, code) in [
        (Vec::new(), Code::InvalidArgument),
        (vec![0; 31], Code::InvalidArgument),
        (vec![0; 33], Code::InvalidArgument),
        (B256::repeat_byte(0x42).to_vec(), Code::FailedPrecondition),
    ] {
        let mut first = header_chunk(5, 7);
        header_mut(&mut first).expected_validation_identity = identity;
        // Deliberately bypass TestServer's post-identity gate fixtures. No
        // witness is sent: rejection must happen before waiting for a block.
        let status = server
            .client()
            .await
            .prove(tokio_stream::iter([first]))
            .await
            .unwrap_err();
        assert_eq!(status.code(), code);
        assert!(status.message().contains("validation identity"), "{status}");
        assert_eq!(
            status.metadata().contains_key("eez-capabilities-changed"),
            code == Code::FailedPrecondition
        );
        assert!(
            status.details().is_empty(),
            "configuration mismatch is never an actionable source failure"
        );
        assert_eq!(state.attester.signing_calls(), 0);
    }
}

#[tokio::test]
async fn capabilities_report_loaded_identity_and_effective_limits_while_proof_slot_is_busy() {
    use eez_control_rpc::v1::CapabilitiesRequest;
    let manifest = std::env::var("CARGO_MANIFEST_DIR")
        .unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").to_owned());
    let path = std::path::Path::new(&manifest).join("../../genesis.json");
    let loaded = eez_l1::chain_config::load_l2_genesis(&path).unwrap();
    let digest = loaded.digest();
    let genesis = loaded.into_genesis();
    let expected_identity = eez_control_rpc::identity::ValidationContext {
        rollup_id: expected_rollup_id(7),
        chain_id: genesis.config.chain_id,
        system_address: TEST_SYSTEM_ADDRESS,
        eez_l2_address: crate::EEZL2_ADDRESS,
        chain_document_digest: digest,
        canonical_json_version: eez_l1::chain_config::CANONICAL_JSON_VERSION,
    }
    .identity();

    for (blocks, bytes, items, idle, deadline) in [
        (65_536, 536_870_912, 1_000_000, 120, 600),
        (17, 4_096, 100, 2, 3),
        (65_537, 536_870_912, 1_000_000, 120, 600),
    ] {
        let limits = ServiceLimits::new(ServiceLimitsParams {
            max_window_blocks: nz(blocks),
            max_window_bytes: nz(bytes),
            max_window_witness_items: nz(items),
            stream_idle_timeout: Duration::from_secs(idle),
            request_timeout: Duration::from_secs(deadline),
        })
        .unwrap();
        let state = inner_with_rollup(
            Validator::stateless(&path, TEST_SYSTEM_ADDRESS).unwrap(),
            expected_rollup_id(7),
        );
        let svc = ProveSvc::new(Arc::clone(&state), limits);
        let observer = svc.clone();
        let busy_permit = Arc::clone(&svc.active_request_slot)
            .acquire_owned()
            .await
            .unwrap();
        let server = TestServer::with_service(svc).await;
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            server.client().await.capabilities(CapabilitiesRequest {}),
        )
        .await
        .unwrap()
        .unwrap()
        .into_inner();
        assert_eq!(response.operations_tag, 0);
        assert_eq!(response.profile_id, 1);
        assert_eq!(response.validation_identity, expected_identity.to_vec());
        assert_eq!(
            response.max_window_blocks,
            blocks.min(u32::MAX as usize) as u64
        );
        assert_eq!(response.max_request_bytes, bytes as u64);
        assert_eq!(response.max_request_witness_items, items as u64);
        assert_eq!(response.max_message_bytes, bytes.min(268_435_456) as u64);
        assert_eq!(response.stream_idle_timeout_millis, idle * 1_000);
        assert_eq!(response.request_timeout_millis, deadline * 1_000);
        assert_eq!(response.preparation_max_blocks, 64);
        assert_eq!(response.cache_max_entries, 131_072);
        assert_eq!(response.cache_max_bytes, 512 * 1024 * 1024);
        assert_eq!(
            response.maximum_leases,
            eez_control_rpc::MAX_PREPARATION_LEASES
        );
        assert_eq!(
            response.lease_lifetime_millis,
            (deadline * 2).max(120) * 1_000
        );
        assert_eq!(observer.active_request_slot.available_permits(), 0);
        assert_eq!(state.attester.signing_calls(), 0);
        drop(busy_permit);
        if blocks > 65_536 {
            let mut first = header_chunk(1, 1);
            let header = header_mut(&mut first);
            header.rollup_id = 7;
            header.to_block = blocks as u64 + 1;
            header.expected_validation_identity = expected_identity.to_vec();
            let status = server
                .client()
                .await
                .prove(tokio_stream::iter([first]))
                .await
                .unwrap_err();
            assert_eq!(status.code(), Code::ResourceExhausted);
            assert!(status.message().contains(&blocks.to_string()), "{status}");
            assert_eq!(state.attester.signing_calls(), 0);
        }
    }
}

#[test]
fn service_limits_reject_invalid_boundaries() {
    for (name, idle_timeout, request_timeout) in [
        (
            "capability millisecond overflow",
            Duration::from_secs(u64::MAX / 1_000 + 1),
            Duration::from_secs(1),
        ),
        (
            "zero stream idle timeout",
            Duration::ZERO,
            Duration::from_secs(1),
        ),
        (
            "zero request timeout",
            Duration::from_secs(1),
            Duration::ZERO,
        ),
        (
            "out-of-range stream idle timeout",
            Duration::MAX,
            Duration::from_secs(1),
        ),
        (
            "out-of-range request timeout",
            Duration::from_secs(1),
            Duration::MAX,
        ),
    ] {
        assert!(
            ServiceLimits::new(ServiceLimitsParams {
                max_window_blocks: nz(1),
                max_window_bytes: nz(1024),
                max_window_witness_items: nz(1024),
                stream_idle_timeout: idle_timeout,
                request_timeout,
            })
            .is_err(),
            "{name} was accepted"
        );
    }

    let below_ceiling = limits_with(1, 1024, Duration::from_secs(1), Duration::from_secs(1));
    assert_eq!(below_ceiling.max_decoding_message_bytes(), 1024);
    let above_ceiling = limits_with(
        1,
        MAX_DECODING_MESSAGE_BYTES + 1,
        Duration::from_secs(1),
        Duration::from_secs(1),
    );
    assert_eq!(
        above_ceiling.max_decoding_message_bytes(),
        MAX_DECODING_MESSAGE_BYTES
    );
}

#[tokio::test]
async fn a_mismatched_header_rollup_is_rejected_without_waiting_for_eof() {
    let inner = one_accepting_validator();
    let server = TestServer::new(Arc::clone(&inner)).await;
    let (sender, receiver) = mpsc::channel(1);
    let mut chunk = header_chunk(5, 5);
    header_mut(&mut chunk).rollup_id = 2;
    sender.send(chunk).await.unwrap();

    let mut client = server.client().await;
    let status = tokio::time::timeout(
        Duration::from_secs(1),
        client.prove(ReceiverStream::new(receiver)),
    )
    .await
    .expect("a mismatched rollup must be rejected before EOF")
    .expect_err("a mismatched rollup must be rejected");

    assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
    assert_eq!(status.message(), "window rollup identity rejected");
    assert_eq!(inner.validator.stub_remaining(), 1);
    drop(sender);
}

#[tokio::test]
async fn missing_or_invalid_posting_context_never_reaches_execution_or_signing() {
    let inner = one_accepting_validator();
    let server = TestServer::new(Arc::clone(&inner)).await;
    for context in [
        None,
        Some(eez_control_rpc::v1::L1PostingContext {
            chain_id: 10200,
            ..crate::testkit::posting_context()
        }),
        Some(eez_control_rpc::v1::L1PostingContext {
            target_timestamp: 0,
            ..crate::testkit::posting_context()
        }),
    ] {
        let mut chunks = happy_window();
        header_mut(&mut chunks[0])
            .post_batch
            .as_mut()
            .unwrap()
            .posting_context = context;
        let status = server.prove(chunks).await;
        assert_eq!(status.code(), Code::InvalidArgument, "{status:?}");
        assert_eq!(inner.validator.stub_remaining(), 1);
    }
    let _response = server.attest(happy_window()).await;
    assert_eq!(inner.validator.stub_remaining(), 0);
}

#[tokio::test]
async fn malformed_windows_never_reach_the_validator() {
    let inner = one_accepting_validator();
    let server = TestServer::new(Arc::clone(&inner)).await;

    let status = server
        .prove(vec![
            header_chunk(5, 7),
            block_chunk(5, 0x04, 0x05),
            block_chunk(7, 0x06, 0x07),
        ])
        .await;
    assert_eq!(status.code(), Code::InvalidArgument, "{status:?}");
    assert_eq!(inner.validator.stub_remaining(), 1);

    let _response = server.attest(happy_window()).await;
    assert_eq!(inner.validator.stub_remaining(), 0);
}

#[tokio::test]
async fn a_chain_break_is_an_invalid_argument() {
    let server = TestServer::new(unused_validator()).await;
    let status = server
        .prove(vec![
            header_chunk(5, 6),
            block_chunk(5, 0x04, 0x05),
            block_chunk(6, 0xbb, 0x06),
        ])
        .await;
    assert_eq!(status.code(), Code::InvalidArgument, "{status:?}");
    assert!(status.message().contains("hash-chain break"), "{status:?}");
    assert!(
        status
            .message()
            .contains(&format!("{:#x}", alloy_primitives::B256::repeat_byte(0x05))),
        "{status:?}"
    );
    assert!(
        status
            .message()
            .contains(&format!("{:#x}", alloy_primitives::B256::repeat_byte(0xbb))),
        "{status:?}"
    );
}

#[tokio::test]
async fn stream_shape_violations_are_invalid_arguments() {
    let server = TestServer::new(unused_validator()).await;
    let mut missing_witness = block_chunk(5, 0x04, 0x05);
    block_mut(&mut missing_witness).witness = None;
    let mut invalid_l1_parent_hash = header_chunk(5, 5);
    header_mut(&mut invalid_l1_parent_hash)
        .post_batch
        .as_mut()
        .unwrap()
        .posting_context
        .as_mut()
        .unwrap()
        .parent_hash = vec![0; 32];

    for (name, chunks) in [
        ("empty stream", vec![]),
        ("kindless first chunk", vec![ProveChunk { kind: None }]),
        ("block before header", vec![block_chunk(5, 0x04, 0x05)]),
        (
            "duplicate header",
            vec![
                header_chunk(5, 5),
                header_chunk(5, 5),
                block_chunk(5, 0x04, 0x05),
            ],
        ),
        (
            "kindless block chunk",
            vec![header_chunk(5, 5), ProveChunk { kind: None }],
        ),
        ("window without blocks", vec![header_chunk(5, 5)]),
        (
            "header without post_batch",
            vec![ProveChunk {
                kind: Some(prove_chunk::Kind::Header(ProveHeader {
                    parent_provenance: Some(eez_control_rpc::v1::ParentProvenance {
                        parent_header_rlp: alloy_rlp::encode(alloy_consensus::Header::default()),
                        accepted_l1_number: 0,
                        live_slot_endpoint_headers_rlp: Vec::new(),
                    }),
                    expected_validation_identity: Validator::stub(Vec::new())
                        .validation_identity(expected_rollup_id(1))
                        .to_vec(),
                    rollup_id: 1,
                    from_block: 5,
                    to_block: 5,
                    post_batch: None,
                })),
            }],
        ),
        (
            "inverted bounds",
            vec![header_chunk(6, 5), block_chunk(5, 0x04, 0x05)],
        ),
        ("zero l1 parent hash", vec![invalid_l1_parent_hash]),
        ("missing witness", vec![header_chunk(5, 5), missing_witness]),
    ] {
        let status = server.prove(chunks).await;
        assert_eq!(status.code(), Code::InvalidArgument, "{name}: {status:?}");
    }
}

#[tokio::test]
async fn an_over_quota_header_is_rejected_without_waiting_for_eof() {
    let inner = one_accepting_validator();
    let server = TestServer::with_limits(
        Arc::clone(&inner),
        limits_with(
            1,
            1024 * 1024,
            Duration::from_secs(5),
            Duration::from_secs(10),
        ),
    )
    .await;
    let (sender, receiver) = mpsc::channel(1);
    sender.send(header_chunk(5, 6)).await.unwrap();

    let mut client = server.client().await;
    let status = tokio::time::timeout(
        Duration::from_secs(1),
        client.prove(ReceiverStream::new(receiver)),
    )
    .await
    .expect("over-quota header must be rejected before EOF")
    .expect_err("over-quota header must be rejected");
    assert_eq!(status.code(), Code::ResourceExhausted, "{status:?}");
    assert_eq!(inner.validator.stub_remaining(), 1);
    drop(sender);
}

#[tokio::test]
async fn server_accepts_a_message_above_tonics_default() {
    const FIVE_MIB: usize = 5 * 1024 * 1024;
    let server = TestServer::with_limits(
        one_accepting_single_block_validator(),
        limits_with(
            1,
            6 * 1024 * 1024,
            Duration::from_secs(5),
            Duration::from_secs(10),
        ),
    )
    .await;
    let mut large_block = block_chunk(5, 0x04, 0x05);
    block_mut(&mut large_block).witness.as_mut().unwrap().state = vec![vec![0; FIVE_MIB]];

    let _response = server.attest(vec![header_chunk(5, 5), large_block]).await;
}

#[tokio::test]
async fn a_message_above_the_decoding_limit_is_resource_exhausted() {
    let server = TestServer::with_limits(
        unused_validator(),
        limits_with(1, 1024, Duration::from_secs(5), Duration::from_secs(10)),
    )
    .await;
    let mut oversized_block = block_chunk(5, 0x04, 0x05);
    block_mut(&mut oversized_block)
        .witness
        .as_mut()
        .unwrap()
        .state = vec![vec![0; 2 * 1024]];

    let status = server
        .prove(vec![header_chunk(5, 5), oversized_block])
        .await;
    assert_eq!(status.code(), Code::ResourceExhausted, "{status:?}");
    assert_eq!(status.message(), "Prove message exceeds decoding limit");
}

#[tokio::test]
async fn a_complete_prefix_requires_eof_and_hits_the_idle_timeout() {
    let inner = one_accepting_validator();
    let server = TestServer::with_limits(
        Arc::clone(&inner),
        limits_with(
            1,
            1024 * 1024,
            Duration::from_millis(50),
            Duration::from_secs(2),
        ),
    )
    .await;
    let (sender, receiver) = mpsc::channel(2);
    sender.send(header_chunk(5, 5)).await.unwrap();
    sender.send(block_chunk(5, 0x04, 0x05)).await.unwrap();

    let mut client = server.client().await;
    let status = tokio::time::timeout(
        Duration::from_secs(1),
        client.prove(ReceiverStream::new(receiver)),
    )
    .await
    .expect("idle timeout must terminate an open complete prefix")
    .expect_err("a stream without EOF must not validate");
    assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}");
    assert_eq!(inner.validator.stub_remaining(), 1);
    drop(sender);
}

#[tokio::test]
async fn the_request_deadline_is_independent_of_the_idle_timeout() {
    let inner = one_accepting_validator();
    let server = TestServer::with_limits(
        Arc::clone(&inner),
        limits_with(
            1,
            1024 * 1024,
            Duration::from_secs(5),
            Duration::from_millis(50),
        ),
    )
    .await;
    let (sender, receiver) = mpsc::channel(1);
    sender.send(header_chunk(5, 5)).await.unwrap();

    let mut client = server.client().await;
    let status = tokio::time::timeout(
        Duration::from_secs(1),
        client.prove(ReceiverStream::new(receiver)),
    )
    .await
    .expect("request deadline must terminate an open stream")
    .expect_err("a stream beyond the request deadline must not validate");
    assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}");
    assert_eq!(status.message(), "Prove request deadline exceeded");
    assert_eq!(inner.validator.stub_remaining(), 1);
    drop(sender);
}

#[tokio::test]
async fn a_validation_deadline_retains_the_request_slot_until_the_worker_finishes() {
    let (validator, started, release) =
        Validator::blocking_stub(Ok(backend_output_for(&happy_block_inputs())));
    let svc = ProveSvc::new(
        inner(validator),
        limits_with(
            16,
            1024 * 1024,
            Duration::from_secs(5),
            Duration::from_millis(500),
        ),
    );
    let observer = svc.clone();
    let server = TestServer::with_service(svc).await;
    let mut client = server.client().await;
    let request =
        tokio::spawn(async move { client.prove(tokio_stream::iter(happy_window())).await });

    tokio::time::timeout(Duration::from_secs(1), started)
        .await
        .expect("validation did not start")
        .expect("blocking validator dropped its start signal");
    let status = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .expect("request deadline did not return promptly")
        .expect("client task panicked")
        .expect_err("a blocked validator must exceed the request deadline");
    assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}");
    assert_eq!(observer.active_request_slot.available_permits(), 0);

    let saturated = server.prove(happy_window()).await;
    assert_eq!(saturated.code(), Code::Unavailable, "{saturated:?}");

    release.send(()).expect("validation worker stopped early");
    tokio::time::timeout(Duration::from_secs(1), async {
        while observer.active_request_slot.available_permits() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("validation completion did not release its request slot");
    assert_eq!(
        observer.state.attester.signing_calls(),
        0,
        "detached timed-out work signed after completion"
    );
}

#[tokio::test]
async fn a_panicking_validation_worker_returns_a_redacted_internal_error() {
    let svc = ProveSvc::new(inner(Validator::panicking_stub()), limits());
    let observer = svc.clone();
    let server = TestServer::with_service(svc).await;

    let status = server.prove(happy_window()).await;
    assert_eq!(status.code(), Code::Internal, "{status:?}");
    assert_eq!(status.message(), "request pipeline worker failed");
    assert_eq!(observer.active_request_slot.available_permits(), 1);
}

#[test]
fn dropping_the_worker_guard_aborts_queued_work_and_signals_cancellation() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (occupier_started_tx, occupier_started_rx) = oneshot::channel();
        let (release_occupier_tx, release_occupier_rx) = std::sync::mpsc::channel();
        let occupier = tokio::task::spawn_blocking(move || {
            let _ = occupier_started_tx.send(());
            release_occupier_rx.recv().unwrap();
        });
        occupier_started_rx.await.unwrap();

        let queued_ran = Arc::new(AtomicBool::new(false));
        let queued_ran_in_worker = Arc::clone(&queued_ran);
        let queued = tokio::task::spawn_blocking(move || {
            queued_ran_in_worker.store(true, Ordering::SeqCst);
        });
        let cancellation = CancellationToken::default();
        let guard = WorkerGuard::new(queued.abort_handle(), cancellation.clone());
        assert!(!cancellation.is_cancelled());
        drop(guard);
        assert!(cancellation.is_cancelled());

        release_occupier_tx.send(()).unwrap();
        occupier.await.unwrap();
        assert!(queued.await.unwrap_err().is_cancelled());
        assert!(!queued_ran.load(Ordering::SeqCst));
    });
}

#[tokio::test]
async fn the_request_slot_is_shared_across_connections_and_released_after_cancellation() {
    let svc = ProveSvc::new(
        one_accepting_validator(),
        limits_with(
            16,
            1024 * 1024,
            Duration::from_secs(5),
            Duration::from_secs(10),
        ),
    );
    let observer = svc.clone();
    let server = TestServer::with_service(svc).await;
    let (sender, receiver) = mpsc::channel(1);
    sender.send(header_chunk(5, 5)).await.unwrap();

    let mut first_client = server.client().await;
    let first =
        tokio::spawn(async move { first_client.prove(ReceiverStream::new(receiver)).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while observer.active_request_slot.available_permits() > 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first request did not acquire the request slot");

    let status = server.prove(happy_window()).await;
    assert_eq!(status.code(), Code::Unavailable, "{status:?}");

    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    drop(sender);
    tokio::time::timeout(Duration::from_secs(1), async {
        while observer.active_request_slot.available_permits() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelling the first client did not release its request slot");
    assert_eq!(observer.state.attester.signing_calls(), 0);

    let _response = server.attest(happy_window()).await;
}

#[tokio::test]
async fn shutdown_waits_for_detached_request_work() {
    let svc = ProveSvc::new(unused_validator(), limits());
    let request_permit = Arc::clone(&svc.active_request_slot)
        .acquire_owned()
        .await
        .expect("the active-request semaphore is never closed");
    let observer = svc.clone();
    let wait = tokio::spawn(async move { observer.wait_until_idle().await });

    tokio::task::yield_now().await;
    assert!(!wait.is_finished());

    drop(request_permit);
    tokio::time::timeout(Duration::from_secs(1), wait)
        .await
        .expect("shutdown did not observe the released request slot")
        .expect("shutdown waiter panicked");
}
