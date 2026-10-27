//! Shared worker and capability invariants. Wire admission, identity, quotas,
//! EOF/half-close, deadlines and cancellation are exercised by v2 session tests.

use super::*;

#[tokio::test]
async fn capabilities_report_offline_identity_and_effective_limits_while_worker_is_busy() {
    let path = crate::testkit::manifest_dir()
        .join("tests/fixtures/native-offline-bidirectional/l2-genesis.json");
    let loaded = eez_l1::chain_config::load_l2_genesis(&path).unwrap();
    let digest = loaded.digest();
    let genesis = loaded.into_genesis();
    for (blocks, bytes, items, idle, deadline) in [
        (65_536, 536_870_912, 1_000_000, 120, 600),
        (17, 4_096, 100, 2, 3),
        (65_537, 536_870_912, 1_000_000, 120, 600),
    ] {
        let state = native_validator_state(expected_rollup_id(7));
        let expected = eez_control_rpc::identity::ValidationContext {
            rollup_id: expected_rollup_id(7),
            chain_id: genesis.config.chain_id,
            system_address: TEST_SYSTEM_ADDRESS,
            eez_l2_address: crate::EEZL2_ADDRESS,
            chain_document_digest: digest,
            offline_profile_hash: state.deployment.profile.commitment(),
            registry: Address::repeat_byte(0x33),
            manager: Address::repeat_byte(0x44),
        }
        .identity();
        let svc = ProveSvc::new(
            Arc::clone(&state),
            ServiceLimits::new(ServiceLimitsParams {
                max_window_blocks: nz(blocks),
                max_window_bytes: nz(bytes),
                max_window_witness_items: nz(items),
                stream_idle_timeout: Duration::from_secs(idle),
                request_timeout: Duration::from_secs(deadline),
            })
            .unwrap(),
        );
        let observer = svc.clone();
        let busy = Arc::clone(&svc.active_request_slot)
            .acquire_owned()
            .await
            .unwrap();
        let server = TestServer::with_service(svc).await;
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            server
                .client()
                .await
                .capabilities(eez_control_rpc::v2::CapabilitiesRequest {}),
        )
        .await
        .unwrap()
        .unwrap()
        .into_inner();
        assert_eq!(response.operations_tag, 0);
        assert_eq!(response.profile_id, eez_control_rpc::OFFLINE_PROFILE_ID);
        assert_eq!(response.validation_identity, expected.to_vec());
        assert_eq!(
            response.max_window_blocks,
            blocks.min(u32::MAX as usize) as u64
        );
        assert_eq!(response.max_request_bytes, bytes as u64);
        assert_eq!(response.max_request_witness_items, items as u64);
        assert_eq!(
            response.max_message_bytes,
            bytes.min(MAX_DECODING_MESSAGE_BYTES) as u64
        );
        assert_eq!(response.stream_idle_timeout_millis, idle * 1_000);
        assert_eq!(response.request_timeout_millis, deadline * 1_000);
        assert_eq!(observer.active_request_slot.available_permits(), 0);
        assert_eq!(state.attester.signing_calls(), 0);
        drop(busy);
    }
}

#[tokio::test]
async fn final_proof_waiter_wins_the_next_block_validation_block_boundary() {
    let svc = ProveSvc::new(unused_validator(), limits());
    let blocker = Arc::clone(&svc.validation_worker)
        .acquire_owned()
        .await
        .unwrap();
    let worker = Arc::clone(&svc.validation_worker);
    let (first_started_tx, first_started_rx) = oneshot::channel();
    let (release_first_tx, release_first_rx) = oneshot::channel();
    let (block_validation_reacquired_tx, mut block_validation_reacquired_rx) = mpsc::channel(1);
    let block_validation = tokio::spawn(async move {
        let first = Arc::clone(&worker).acquire_owned().await.unwrap();
        first_started_tx.send(()).unwrap();
        release_first_rx.await.unwrap();
        drop(first);
        let _second = worker.acquire_owned().await.unwrap();
        block_validation_reacquired_tx.send(()).await.unwrap();
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
        block_validation_reacquired_rx.try_recv().is_err(),
        "block_validation reacquired before the already queued final proof"
    );
    release_final_tx.send(()).unwrap();
    tokio::time::timeout(
        Duration::from_secs(1),
        block_validation_reacquired_rx.recv(),
    )
    .await
    .expect("block_validation did not resume after final proof")
    .expect("block_validation task ended before reacquiring");
    final_proof.await.unwrap();
    block_validation.await.unwrap();
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

#[test]
fn portable_validation_version_matches_manager_execution_contract() {
    assert_eq!(
        eez_control_rpc::identity::VALIDATION_VERSION,
        eez_protocol::l2::VALIDATION_VERSION
    );
}
