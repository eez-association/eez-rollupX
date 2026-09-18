//! Transport regressions for the native fixture's transparent prover proxy.
use super::*;
use eez_control_rpc::v1::{
    CacheMiss, CapabilitiesRequest, CapabilitiesResponse, PrepareChunk, PrepareResponse,
    RetainOrdinaryRangesRequest, RetainOrdinaryRangesResponse,
};
use tonic::metadata::MetadataMap;

#[derive(Clone)]
struct Upstream {
    received: tokio::sync::mpsc::Sender<MetadataMap>,
    rejection: Status,
}

#[tonic::async_trait]
impl Prover for Upstream {
    async fn capabilities(
        &self,
        _: Request<CapabilitiesRequest>,
    ) -> Result<Response<CapabilitiesResponse>, Status> {
        Err(Status::unimplemented("unused fixture method"))
    }

    async fn retain_ordinary_ranges(
        &self,
        _: Request<RetainOrdinaryRangesRequest>,
    ) -> Result<Response<RetainOrdinaryRangesResponse>, Status> {
        Err(Status::unimplemented("unused fixture method"))
    }

    async fn prepare_ordinary(
        &self,
        request: Request<Streaming<PrepareChunk>>,
    ) -> Result<Response<PrepareResponse>, Status> {
        self.received.try_send(request.metadata().clone()).unwrap();
        let mut stream = request.into_inner();
        while stream.message().await?.is_some() {}
        Ok(Response::new(PrepareResponse::default()))
    }

    async fn prove(
        &self,
        request: Request<Streaming<ProveChunk>>,
    ) -> Result<Response<ProveResponse>, Status> {
        self.received.try_send(request.metadata().clone()).unwrap();
        Err(self.rejection.clone())
    }
}

async fn fixture(
    rejection: Status,
) -> (
    ProverProxyHandle,
    tokio::sync::mpsc::Receiver<MetadataMap>,
    JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (received, messages) = tokio::sync::mpsc::channel(8);
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(ProverServer::new(Upstream {
                received,
                rejection,
            }))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    let proxy = ProverProxyHandle::spawn(format!("http://{address}"), ProverMutation::None, None)
        .await
        .unwrap();
    (proxy, messages, server)
}

#[tokio::test]
async fn preparation_proxy_preserves_bounded_context_and_existing_chunk_limit() {
    let (proxy, mut received, server) = fixture(Status::internal("unused")).await;
    let mut client = ProverClient::connect(proxy.endpoint().to_owned())
        .await
        .unwrap();
    let parent = "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01";
    for duplicate in [false, true] {
        let mut request = Request::new(tokio_stream::iter([PrepareChunk::default()]));
        request
            .metadata_mut()
            .insert("traceparent", parent.parse().unwrap());
        if duplicate {
            // Preserve duplicate context for the real signer's rejection gate.
            request
                .metadata_mut()
                .append("traceparent", parent.parse().unwrap());
            request
                .metadata_mut()
                .append("traceparent", parent.parse().unwrap());
        }
        request
            .metadata_mut()
            .insert("tracestate", "vendor=value".parse().unwrap());
        request
            .metadata_mut()
            .insert("baggage", "excluded=value".parse().unwrap());
        client.prepare_ordinary(request).await.unwrap();
        let forwarded = received.try_recv().unwrap();
        assert_eq!(
            forwarded.get_all("traceparent").iter().count(),
            if duplicate { 2 } else { 1 }
        );
        assert!(
            forwarded
                .get_all("traceparent")
                .iter()
                .all(|value| value == parent)
        );
        assert_eq!(
            forwarded.get("tracestate").unwrap().to_str().unwrap(),
            "vendor=value"
        );
        assert!(!forwarded.contains_key("baggage"));
    }
    let mut request = Request::new(tokio_stream::iter(vec![PrepareChunk::default(); 65]));
    request
        .metadata_mut()
        .insert("tracestate", "x".repeat(513).parse().unwrap());
    client.prepare_ordinary(request).await.unwrap();
    assert!(!received.try_recv().unwrap().contains_key("tracestate"));
    let error = client
        .prepare_ordinary(tokio_stream::iter(vec![PrepareChunk::default(); 66]))
        .await
        .unwrap_err();
    assert_eq!(error.code(), tonic::Code::ResourceExhausted);
    assert!(received.try_recv().is_err());
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn proof_proxy_preserves_cache_details_and_identity_metadata_without_bypassing_faults() {
    for changed in [false, true] {
        let failure = ProveFailure {
            actionable_failure: Some(prove_failure::ActionableFailure::CacheMiss(CacheMiss {
                validation_identity: vec![1; 32],
                block_number: 7,
                block_hash: vec![2; 32],
            })),
        };
        let details = encode_prove_failure(&failure);
        let mut rejection = Status::with_details(
            tonic::Code::FailedPrecondition,
            "reviewed upstream rejection",
            details.clone().into(),
        );
        if changed {
            rejection
                .metadata_mut()
                .insert("eez-capabilities-changed", "1".parse().unwrap());
        }
        let (proxy, mut received, server) = fixture(rejection).await;
        let mut client = ProverClient::connect(proxy.endpoint().to_owned())
            .await
            .unwrap();
        let error = client
            .prove(tokio_stream::iter(Vec::<ProveChunk>::new()))
            .await
            .unwrap_err();
        assert_eq!(error.code(), tonic::Code::FailedPrecondition);
        assert_eq!(error.message(), "reviewed upstream rejection");
        assert_eq!(error.details(), details);
        assert_eq!(
            error.metadata().contains_key("eez-capabilities-changed"),
            changed
        );
        received.try_recv().unwrap();
        assert_eq!(proxy.rejections(), 1);
        proxy.set_mutation(ProverMutation::Unavailable);
        let injected = client
            .prove(tokio_stream::iter(Vec::<ProveChunk>::new()))
            .await
            .unwrap_err();
        assert_eq!(injected.code(), tonic::Code::Unavailable);
        assert_eq!(injected.message(), "fixture temporary upstream outage");
        assert!(received.try_recv().is_err());
        assert_eq!(proxy.attempts(), 2);
        assert_eq!(proxy.rejections(), 1);
        assert_eq!(proxy.successes(), 0);
        server.abort();
        let _ = server.await;
    }
}
