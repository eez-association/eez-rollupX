//! Preparation time bounds and ownership through the actual tonic handler.

use super::*;
use eez_control_rpc::v1::{PrepareChunk, PrepareHeader, prepare_chunk};

fn chunks(svc: &ProveSvc, count: u64) -> Vec<PrepareChunk> {
    std::iter::once(PrepareChunk {
        kind: Some(prepare_chunk::Kind::Header(PrepareHeader {
            rollup_id: 1,
            expected_validation_identity: svc.state.validation_identity.to_vec(),
            from_block: 5,
            to_block: 4 + count,
            lease_id: Vec::new(),
        })),
    })
    .chain((5..5 + count).map(|number| {
        let Some(prove_chunk::Kind::Block(block)) =
            block_chunk(number, (number - 1) as u8, number as u8).kind
        else {
            unreachable!()
        };
        PrepareChunk {
            kind: Some(prepare_chunk::Kind::Block(block)),
        }
    }))
    .collect()
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("service did not reach the expected ownership boundary");
}

fn assert_empty(svc: &ProveSvc) {
    let mut cache = svc.prepared_cache.lock().unwrap();
    assert_eq!(cache.usage(), (0, 0));
    assert_eq!(cache.lease_count(), 0);
    assert_eq!(svc.state.attester.signing_calls(), 0);
}

async fn open_stream(initial_chunks: usize, idle: Duration, total: Duration, message: &str) {
    let svc = ProveSvc::new(
        one_accepting_single_block_validator(),
        limits_with(1, 1024 * 1024, idle, total),
    );
    let server = TestServer::with_service(svc.clone()).await;
    let (sender, receiver) = mpsc::channel(2);
    for chunk in chunks(&svc, 1).into_iter().take(initial_chunks) {
        sender.send(chunk).await.unwrap();
    }
    let mut client = server.client().await;
    let mut request =
        tokio::spawn(async move { client.prepare_ordinary(ReceiverStream::new(receiver)).await });
    until(|| svc.preparation_slot.available_permits() == 0).await;
    let response = tokio::time::timeout(Duration::from_secs(2), &mut request).await;
    // Release the test stream even on a baseline failure.
    request.abort();
    drop(sender);
    let status = response
        .expect("open preparation stream exceeded its time bound")
        .unwrap()
        .unwrap_err();
    assert_eq!(status.code(), Code::DeadlineExceeded);
    assert_eq!(status.message(), message);
    assert_eq!(svc.preparation_slot.available_permits(), 1);
    assert_eq!(svc.validation_worker.available_permits(), 1);
    assert_eq!(svc.state.validator.stub_remaining(), 1);
    assert_empty(&svc);

    let response = server
        .client()
        .await
        .prepare_ordinary(tokio_stream::iter(chunks(&svc, 1)))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        response.prepared_count, 1,
        "a fresh preparation must proceed"
    );
    assert_eq!(svc.state.attester.signing_calls(), 0);
}

#[tokio::test]
async fn deadline_bounds_the_first_header() {
    open_stream(
        0,
        Duration::from_secs(10),
        Duration::from_millis(200),
        "preparation deadline exceeded",
    )
    .await;
}

#[tokio::test]
async fn deadline_bounds_an_incomplete_upload() {
    open_stream(
        1,
        Duration::from_secs(10),
        Duration::from_millis(200),
        "preparation deadline exceeded",
    )
    .await;
}

#[tokio::test]
async fn deadline_bounds_eof_after_the_complete_upload() {
    open_stream(
        2,
        Duration::from_secs(10),
        Duration::from_millis(200),
        "preparation deadline exceeded",
    )
    .await;
}

#[tokio::test]
async fn idle_timeout_remains_independent() {
    open_stream(
        2,
        Duration::from_millis(100),
        Duration::from_secs(5),
        "preparation stream idle timeout",
    )
    .await;
}

#[tokio::test]
async fn progressing_upload_does_not_reset_the_deadline() {
    let svc = ProveSvc::new(
        unused_validator(),
        limits_with(
            3,
            1024 * 1024,
            Duration::from_secs(1),
            Duration::from_millis(1500),
        ),
    );
    let server = TestServer::with_service(svc.clone()).await;
    let (sender, receiver) = mpsc::channel(1);
    let mut input = chunks(&svc, 3).into_iter();
    sender.send(input.next().unwrap()).await.unwrap();
    let mut client = server.client().await;
    let mut request =
        tokio::spawn(async move { client.prepare_ordinary(ReceiverStream::new(receiver)).await });
    until(|| svc.preparation_slot.available_permits() == 0).await;
    let producer = tokio::spawn(async move {
        for chunk in input {
            tokio::time::sleep(Duration::from_millis(600)).await;
            if sender.send(chunk).await.is_err() {
                return;
            }
        }
        // Keep EOF pending so the baseline eventually reports its idle timeout.
        std::future::pending::<()>().await;
        drop(sender);
    });
    let response = tokio::time::timeout(Duration::from_secs(4), &mut request).await;
    request.abort();
    producer.abort();
    let status = response.unwrap().unwrap().unwrap_err();
    assert_eq!(status.code(), Code::DeadlineExceeded);
    assert_eq!(status.message(), "preparation deadline exceeded");
    assert_eq!(svc.preparation_slot.available_permits(), 1);
    assert_empty(&svc);
}

#[tokio::test]
async fn running_validation_keeps_its_worker_after_the_deadline() {
    let (validator, started, release) =
        Validator::blocking_stub(Ok(backend_output_for(&[AdmittedBlock::test(
            5, 0x04, 0x05,
        )])));
    let svc = ProveSvc::new(
        inner(validator),
        limits_with(
            1,
            1024 * 1024,
            Duration::from_secs(5),
            Duration::from_millis(500),
        ),
    );
    let server = TestServer::with_service(svc.clone()).await;
    let input = chunks(&svc, 1);
    let mut client = server.client().await;
    let request =
        tokio::spawn(async move { client.prepare_ordinary(tokio_stream::iter(input)).await });
    tokio::time::timeout(Duration::from_secs(2), started)
        .await
        .unwrap()
        .unwrap();
    let status = tokio::time::timeout(Duration::from_secs(2), request)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(status.code(), Code::DeadlineExceeded);
    assert_eq!(svc.validation_worker.available_permits(), 0);
    assert_empty(&svc);
    let observer = svc.clone();
    let shutdown = tokio::spawn(async move { observer.wait_until_idle().await });
    tokio::task::yield_now().await;
    assert!(!shutdown.is_finished());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), shutdown)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(svc.validation_worker.available_permits(), 1);
    assert_empty(&svc);
}

#[tokio::test]
async fn a_later_block_failure_does_not_publish_the_valid_prefix() {
    let validator = Validator::stub(vec![
        Ok(backend_output_for(&[AdmittedBlock::test(5, 0x04, 0x05)])),
        Err("second block rejected".into()),
    ]);
    let svc = ProveSvc::new(inner(validator), limits());
    let server = TestServer::with_service(svc.clone()).await;
    let status = server
        .client()
        .await
        .prepare_ordinary(tokio_stream::iter(chunks(&svc, 2)))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::FailedPrecondition);
    assert!(status.message().contains("second block rejected"));
    assert_eq!(svc.state.validator.stub_remaining(), 0);
    assert_empty(&svc);
}

#[tokio::test]
async fn blocked_cache_publication_expires_without_a_late_lease() {
    let (validator, started, release_validation) = Validator::blocking_stub(Ok(
        backend_output_for(&[AdmittedBlock::test(5, 0x04, 0x05)]),
    ));
    let svc = ProveSvc::new(
        inner(validator),
        limits_with(
            1,
            1024 * 1024,
            Duration::from_secs(5),
            Duration::from_millis(500),
        ),
    );
    let cache = Arc::clone(&svc.prepared_cache);
    let (locked, lock_ready) = oneshot::channel();
    let (unlock, unlock_rx) = std::sync::mpsc::channel();
    let blocker = tokio::task::spawn_blocking(move || {
        let _guard = cache.lock().unwrap();
        locked.send(()).unwrap();
        let _ = unlock_rx.recv();
    });
    lock_ready.await.unwrap();
    let server = TestServer::with_service(svc.clone()).await;
    let input = chunks(&svc, 1);
    let mut client = server.client().await;
    let mut request =
        tokio::spawn(async move { client.prepare_ordinary(tokio_stream::iter(input)).await });
    tokio::time::timeout(Duration::from_secs(2), started)
        .await
        .unwrap()
        .unwrap();
    release_validation.send(()).unwrap();
    until(|| svc.validation_worker.available_permits() == 1).await;
    assert_eq!(svc.preparation_slot.available_permits(), 0);
    let response = tokio::time::timeout(Duration::from_secs(2), &mut request).await;
    // Unblock and join before asserting: a failing baseline must not strand a worker.
    unlock.send(()).unwrap();
    blocker.await.unwrap();
    if response.is_err() {
        let _ = request.await;
    }
    tokio::time::timeout(Duration::from_secs(2), svc.wait_until_idle())
        .await
        .unwrap();
    let status = response
        .expect("publication ignored the request deadline")
        .unwrap()
        .unwrap_err();
    assert_eq!(status.code(), Code::DeadlineExceeded);
    assert_eq!(status.message(), "preparation deadline exceeded");
    assert_empty(&svc);
}
