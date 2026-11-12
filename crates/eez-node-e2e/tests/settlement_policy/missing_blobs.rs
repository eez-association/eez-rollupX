use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_follower_waits_for_missing_and_malformed_blobs_then_replays() {
    use http_body_util::Full;
    use hyper::{Response, body::Bytes, service::service_fn};
    use hyper_util::rt::TokioIo;
    use std::convert::Infallible;

    let world = setup_cross_chain_with_env(&policy_env(Vec::new(), "always"))
        .await
        .unwrap();
    let transfer = send_l2_value_transfer_confirmed(
        &world.l2_rpc(),
        ANVIL_KEY_1,
        ANVIL_ADDR_3,
        U256::from(2),
        TIMEOUT,
    )
    .await
    .unwrap();
    let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse().unwrap());
    let receipt = l2.get_transaction_receipt(transfer).await.unwrap().unwrap();
    wait_for_safe_prefix_convergence(&[&world.node], receipt.block_number.unwrap(), TIMEOUT)
        .await
        .unwrap();
    let mut env = world.partial_follower_env();
    let upstream = env
        .iter()
        .find(|(key, _)| *key == "EEZ_L1_BEACON_URL")
        .unwrap()
        .1
        .clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let mode = Arc::new(AtomicU8::new(1));
    let fault = Arc::clone(&mode);
    // This test's Tokio runtime owns every proxy task, including on panic.
    let proxy = tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let (upstream, client, fault) = (upstream.clone(), client.clone(), Arc::clone(&fault));
            tokio::spawn(async move {
                let service = service_fn(move |request: hyper::Request<hyper::body::Incoming>| {
                    let (upstream, client, fault) =
                        (upstream.clone(), client.clone(), Arc::clone(&fault));
                    async move {
                        let path = request.uri().path();
                        let mode = fault.load(Ordering::Relaxed);
                        let body = if mode != 0
                            && (path.contains("/blobs/") || path.contains("/blob_sidecars/"))
                        {
                            Bytes::from_static(if mode == 1 {
                                br#"{"data":[]}"#
                            } else {
                                br#"{"data":[{}]}"#
                            })
                        } else {
                            let response = client
                                .get(format!("{upstream}{path}"))
                                .send()
                                .await
                                .unwrap();
                            assert!(response.status().is_success());
                            response.bytes().await.unwrap()
                        };
                        Ok::<_, Infallible>(Response::new(Full::new(body)))
                    }
                });
                hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await
                    .unwrap();
            });
        }
    });
    env.retain(|(key, _)| *key != "EEZ_L1_BEACON_URL");
    env.push(("EEZ_L1_BEACON_URL", endpoint));
    for fault in [1, 2] {
        mode.store(fault, Ordering::Relaxed);
        let follower = NodeHandle::start(
            &format!("missing-blob-{fault}"),
            &NodeConfig {
                binary: NodeBinary::Follower,
                genesis_path: Some(&world.cfg.l2_genesis.0),
            },
            &env,
        )
        .await
        .unwrap();
        eez_testkit::wait_for(TIMEOUT, || async {
            assert!(
                follower.exit_status().is_none(),
                "{}",
                follower.log_tail(60)
            );
            let failures = follower.log_count_matching(&[
                "boot-time catch_up could not read all L1 source data yet",
            ])?;
            Ok((failures >= 2).then_some(()))
        })
        .await
        .unwrap();
        let logs = std::fs::read_to_string(&follower.log_path).unwrap();
        assert!(
            logs.contains(if fault == 1 {
                "beacon blob response does not contain transaction blob"
            } else {
                "invalid beacon blobs response"
            }),
            "{logs}"
        );
        assert_eq!(
            follower
                .log_count_matching(&["spawning eez deriver"])
                .unwrap(),
            0
        );
        let replay = ProviderBuilder::new().connect_http(follower.l2_rpc_url().parse().unwrap());
        assert!(
            replay
                .get_transaction_receipt(transfer)
                .await
                .unwrap()
                .is_none()
        );
        mode.store(0, Ordering::Relaxed);
        wait_for_safe_prefix_convergence(
            &[&world.node, &follower],
            receipt.block_number.unwrap(),
            TIMEOUT,
        )
        .await
        .unwrap();
        let replayed = replay
            .get_transaction_receipt(transfer)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replayed.block_hash, receipt.block_hash);
        assert_eq!(replayed.inner, receipt.inner);
        assert_eq!(
            follower
                .log_count_matching(&["spawning eez deriver"])
                .unwrap(),
            1
        );
        follower.assert_no_divergence_failure_logs();
    }
    world.node.assert_no_divergence_failure_logs();
    proxy.abort();
}
