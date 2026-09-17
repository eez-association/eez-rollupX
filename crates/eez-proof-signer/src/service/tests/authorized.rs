//! Metric authorization follows the real service boundary, never admission alone.
use super::*;
use eez_telemetry::operational;

fn assert_authorized_count(rendered: &str, outcome: &str, count: u64) {
    let expected = format!("eez_signer_authorized_requests_total{{outcome=\"{outcome}\"}} {count}");
    assert!(rendered.lines().any(|line| line == expected), "{rendered}");
}

#[tokio::test(flavor = "current_thread")]
async fn only_fully_validated_rpc_response_enters_authorized_metrics() {
    let recorder = eez_telemetry::native::prometheus_builder().build_recorder();
    let handle = recorder.handle();
    // This runtime and its tonic tasks stay on this thread. Blocking validation
    // does not own authorized-response observations and need not share recorder.
    let _local = metrics::set_default_local_recorder(&recorder);
    operational::enable();
    operational::initialize_signer_metrics();
    let malformed = TestServer::new(unused_validator()).await;
    assert_eq!(
        malformed.prove(Vec::new()).await.code(),
        Code::InvalidArgument
    );
    let rejected = TestServer::new(one_accepting_validator()).await;
    let mut window = happy_window();
    replace_post_batch(
        &mut window,
        public_input_post_batch_for(eez_protocol::EvmBatch::default()),
    );
    assert_eq!(
        rejected.prove(window).await.code(),
        Code::FailedPrecondition
    );
    for outcome in ["success", "internal", "deadline", "panicked", "cancelled"] {
        assert_authorized_count(&handle.render(), outcome, 0);
    }
    let accepted = TestServer::new(one_accepting_validator()).await;
    let window = happy_window();
    let expected_hash = recompute_window_public_inputs_hash(&window);
    let response = accepted.attest(window).await;
    assert_attestation(&response, expected_hash, test_attester().address());
    assert_authorized_count(&handle.render(), "success", 1);
    for outcome in ["internal", "deadline", "panicked", "cancelled"] {
        assert_authorized_count(&handle.render(), outcome, 0);
    }
}

#[test]
fn authorized_material_that_reaches_response_after_deadline_records_deadline_once() {
    let recorder = eez_telemetry::native::prometheus_builder().build_recorder();
    let handle = recorder.handle();
    metrics::with_local_recorder(&recorder, || {
        operational::enable();
        operational::initialize_signer_metrics();
        let state = one_accepting_validator();
        let mut chunks = happy_window().into_iter();
        let mut assembler =
            window::WindowAssembler::start(limits().window_limits(), chunks.next().unwrap())
                .unwrap()
                .verify_rollup_identity(expected_rollup_id(1))
                .unwrap();
        for chunk in chunks {
            assembler.push(chunk).unwrap();
        }
        let (header, blocks) = assembler.finish().unwrap().into_parts();
        let maximum_blobs = state.posting_blob_limit(&header.posting_context).unwrap();
        // No test-only constructor of the authorized hash: every settlement gate
        // still executes, with the same existing stub execution fixture as RPC tests.
        let material = validate_and_settle(
            &state,
            blocks,
            header.submitted_post_batch_calldata,
            header.submitted_blobs,
            maximum_blobs,
            tokio::time::Instant::now() + Duration::from_secs(60),
            &CancellationToken::default(),
        )
        .unwrap();
        let service = ProveSvc::new(state, limits());
        let started = tokio::time::Instant::now();
        let status = service
            .finish_authorized_response(
                material,
                5,
                7,
                super::super::rpc::RequestTiming {
                    started,
                    deadline: started,
                },
                started,
            )
            .unwrap_err();
        assert_eq!(status.code(), Code::DeadlineExceeded);
        assert_eq!(status.message(), "Prove request deadline exceeded");
    });
    let rendered = handle.render();
    assert_authorized_count(&rendered, "deadline", 1);
    assert_authorized_count(&rendered, "success", 0);
    assert_authorized_count(&rendered, "internal", 0);
    assert!(
        !rendered.contains("stage=\"signer_signing\""),
        "expired authorized work must not sign"
    );
}
