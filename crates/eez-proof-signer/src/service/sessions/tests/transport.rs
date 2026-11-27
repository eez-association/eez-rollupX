use super::{replay, vector_service};
use crate::service::ProveSvc;
use eez_control_rpc::{
    transport::{client_endpoint, server_transport},
    v2::{CapabilitiesRequest, ClientFrame, prover_client::ProverClient},
};
use std::{fs, path::Path};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{
    Code,
    transport::{Certificate, Channel, ClientTlsConfig, Endpoint},
};

async fn mint(dir: &Path, mode: &str, authority: &str) {
    let output = tokio::process::Command::new("bash")
        .arg(
            crate::testkit::manifest_dir()
                .join("../../testing/kurtosis/scripts/mint-prover-tls.sh"),
        )
        .arg(dir)
        .arg(mode)
        .arg(authority)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn serve(
    listener: TcpListener,
    dir: &Path,
    service: ProveSvc,
) -> (oneshot::Sender<()>, JoinHandle<()>) {
    let (mut builder, callers) =
        server_transport(listener.local_addr().unwrap(), Some(dir)).unwrap();
    let svc = ProveSvc::with_callers(service.state, service.limits, callers);
    let (stop, shutdown) = oneshot::channel();
    let handle = tokio::spawn(async move {
        builder
            .add_service(svc.clone().into_server())
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = shutdown.await;
                svc.stop_streams();
            })
            .await
            .unwrap();
        svc.wait_until_idle().await;
    });
    (stop, handle)
}

async fn client(endpoint: &str, root: &Path) -> ProverClient<Channel> {
    ProverClient::new(
        client_endpoint(endpoint, Some(root))
            .unwrap()
            .connect()
            .await
            .unwrap(),
    )
}

async fn denied(client: &mut ProverClient<Channel>) {
    let caps = client
        .capabilities(CapabilitiesRequest {})
        .await
        .unwrap_err();
    let stream = client
        .prove_stream(tokio_stream::empty::<ClientFrame>())
        .await
        .unwrap_err();
    for status in [caps, stream] {
        assert_eq!(status.code(), Code::PermissionDenied);
        assert_eq!(status.message(), "client certificate is not authorized");
    }
}

async fn expect_tls_alert(authority: &str, ca: &Path, identity: Option<&Path>, alert: &str) {
    // Read the actual TLS alert. A timeout or a generic gRPC UNAVAILABLE
    // cannot distinguish client admission from a service that never started.
    let mut cmd = tokio::process::Command::new("openssl");
    cmd.args([
        "s_client",
        "-alpn",
        "h2",
        "-tls1_3",
        "-quiet",
        "-verify_return_error",
        "-verify_hostname",
        "localhost",
        "-connect",
        authority,
        "-CAfile",
    ])
    .arg(ca);
    if let Some(dir) = identity {
        cmd.arg("-cert")
            .arg(dir.join("cert.pem"))
            .arg("-key")
            .arg(dir.join("key.pem"));
    }
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        cmd.kill_on_drop(true).output(),
    )
    .await
    .unwrap()
    .unwrap();
    let message = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() && message.contains(alert),
        "{message}"
    );
}

#[tokio::test]
async fn remote_callers_prove_and_withdrawal_applies_to_old_and_fresh_channels() {
    let files = tempfile::tempdir().unwrap();
    let server = files.path().join("signer");
    let alice = files.path().join("composer");
    let bob = files.path().join("composer-b");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let authority = format!("localhost:{}", addr.port());
    let url = format!("https://{authority}");
    mint(files.path(), "shared", &authority).await;
    let grants = fs::read_to_string(server.join("allowed-clients.sha256")).unwrap();
    let b = grants.lines().nth(1).unwrap();
    let (vector, service) = vector_service("sync-range-proof");
    let identity = service.state.validation_identity;
    let (stop, handle) = serve(listener, &server, service);
    let mut existing = client(&url, &alice).await;
    existing.capabilities(CapabilitiesRequest {}).await.unwrap();
    let (first, second) = tokio::join!(
        replay::replay(&vector, &url, identity, Some(&alice)),
        replay::replay(&vector, &url, identity, Some(&bob))
    );
    first.unwrap();
    second.unwrap();

    let foreign = files.path().join("untrusted");
    mint(&foreign, "direct", &authority).await;
    let ca = server.join("ca.pem");
    expect_tls_alert(&authority, &ca, None, "alert certificate required").await;
    expect_tls_alert(
        &authority,
        &ca,
        Some(&foreign.join("composer").join(&authority)),
        "alert unknown ca",
    )
    .await;
    expect_tls_alert(&authority, &ca, Some(&server), "alert number 43").await;
    let tls = ClientTlsConfig::new().ca_certificate(Certificate::from_pem(fs::read(ca).unwrap()));
    let wrong_name =
        tls.domain_name("wrong.example")
            .identity(tonic::transport::Identity::from_pem(
                fs::read(alice.join(&authority).join("cert.pem")).unwrap(),
                fs::read(alice.join(&authority).join("key.pem")).unwrap(),
            ));
    let error = Endpoint::from_shared(url.clone())
        .unwrap()
        .tls_config(wrong_name)
        .unwrap()
        .connect()
        .await
        .unwrap_err();
    assert!(
        format!("{error:?}").contains("NotValidForName"),
        "{error:?}"
    );

    use eez_control_rpc::v2::{Resume, client_frame, server_frame};
    use prost::Message;
    let first = &vector["events"][0]["protobuf_hex"];
    let begin =
        ClientFrame::decode(hex::decode(first.as_str().unwrap()).unwrap().as_slice()).unwrap();
    let (_original_sender, _original_replies, ready) = attach(&mut existing, identity, begin).await;
    assert!(matches!(ready.kind, Some(server_frame::Kind::Ready(_))));
    let resume = ClientFrame {
        session_id: ready.session_id.clone(),
        epoch: 0,
        request_id: 2,
        kind: Some(client_frame::Kind::Resume(Resume {})),
    };
    let mut other = client(&url, &bob).await;
    let (_, _, denied_resume) = attach(&mut other, identity, resume.clone()).await;
    let Some(server_frame::Kind::Rejected(err)) = denied_resume.kind else {
        panic!("expected unbound rejection")
    };
    assert_eq!(err.code, i32::from(Code::NotFound));
    assert!(
        denied_resume.session_id.is_empty()
            && denied_resume.epoch == 0
            && err.validated_hash.is_empty()
            && err.validated_through == 0
    );
    let (_sender, mut active, resumed) = attach(&mut existing, identity, resume).await;
    assert_eq!(resumed.epoch, ready.epoch + 1);
    assert_eq!(resumed.kind, ready.kind);

    // Switch Alice to a newly issued leaf and issuer, retaining Bob's grant.
    let renewed = foreign.join("signer");
    let roots = format!(
        "{}{}",
        fs::read_to_string(server.join("ca.pem")).unwrap(),
        fs::read_to_string(renewed.join("ca.pem")).unwrap()
    );
    let grant = fs::read_to_string(renewed.join("allowed-clients.sha256")).unwrap();
    fs::write(server.join("ca.pem"), roots).unwrap();
    fs::write(
        server.join("allowed-clients.sha256"),
        format!("{b}\n{grant}"),
    )
    .unwrap();
    existing.capabilities(CapabilitiesRequest {}).await.unwrap();
    stop.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), handle)
        .await
        .unwrap()
        .unwrap();
    match active.message().await {
        Ok(None) => {}
        Err(status) => assert_eq!(status.code(), Code::Unavailable),
        Ok(Some(frame)) => panic!("unexpected response after shutdown: {frame:?}"),
    }
    let (_, replacement) = vector_service("sync-range-proof");
    let (stop, handle) = serve(TcpListener::bind(addr).await.unwrap(), &server, replacement);
    client(&url, &bob)
        .await
        .capabilities(CapabilitiesRequest {})
        .await
        .unwrap();
    denied(&mut client(&url, &alice).await).await;
    // Allow the HTTP/2 channel to observe graceful shutdown and reconnect.
    let status = existing.capabilities(CapabilitiesRequest {}).await;
    if let Err(error) = status {
        assert!(matches!(
            error.code(),
            Code::Unavailable | Code::PermissionDenied
        ));
    } else {
        panic!("withdrawn caller admitted on its existing channel");
    }
    denied(&mut existing).await;
    for name in ["cert.pem", "key.pem"] {
        fs::copy(
            foreign.join("composer").join(&authority).join(name),
            alice.join(&authority).join(name),
        )
        .unwrap();
    }
    let (renewed, retained) = tokio::join!(
        replay::replay(&vector, &url, identity, Some(&alice)),
        replay::replay(&vector, &url, identity, Some(&bob))
    );
    renewed.unwrap();
    retained.unwrap();
    denied(&mut existing).await;
    stop.send(()).unwrap();
    handle.await.unwrap();
}

/// Send the first operation on a new authenticated attachment and leave it open.
async fn attach(
    client: &mut ProverClient<Channel>,
    identity: alloy_primitives::B256,
    frame: ClientFrame,
) -> (
    tokio::sync::mpsc::Sender<ClientFrame>,
    tonic::Streaming<eez_control_rpc::v2::ServerFrame>,
    eez_control_rpc::v2::ServerFrame,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    tx.send(frame).await.unwrap();
    let mut req = tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(rx));
    req.metadata_mut().insert_bin(
        "eez-validation-identity-bin",
        tonic::metadata::MetadataValue::from_bytes(identity.as_slice()),
    );
    let mut replies = client.prove_stream(req).await.unwrap().into_inner();
    let response = replies.message().await.unwrap().unwrap();
    (tx, replies, response)
}

#[tokio::test]
async fn incomplete_credentials_and_invalid_grants_fail_before_connect_or_bind() {
    let files = tempfile::tempdir().unwrap();
    let authority = "localhost:50061";
    let url = "https://localhost:50061";
    mint(files.path(), "direct", authority).await;
    let root = files.path().join("composer");
    let dir = files.path().join("signer");
    let addr = "0.0.0.0:50061".parse().unwrap();
    assert!(client_endpoint(url, Some(Path::new("."))).is_err());
    assert!(client_endpoint("https://another.example:50061", Some(&root)).is_err());
    for role in [&dir, &root.join(authority)] {
        for name in ["ca.pem", "cert.pem", "key.pem"] {
            let path = role.join(name);
            let original = fs::read(&path).unwrap();
            for contents in [None, Some(""), Some("invalid PEM")] {
                match contents {
                    None => fs::remove_file(&path).unwrap(),
                    Some(value) => fs::write(&path, value).unwrap(),
                }
                if *role == dir {
                    assert!(server_transport(addr, Some(&dir)).is_err(), "{name}");
                } else {
                    assert!(client_endpoint(url, Some(&root)).is_err(), "{name}");
                }
            }
            fs::write(path, original).unwrap();
        }
    }
    let grants = dir.join("allowed-clients.sha256");
    let allowed = fs::read_to_string(&grants).unwrap();
    fs::remove_file(&grants).unwrap();
    assert!(server_transport(addr, Some(&dir)).is_err());
    for invalid in ["", "wrong", &allowed.repeat(2)] {
        fs::write(&grants, invalid).unwrap();
        assert!(server_transport(addr, Some(&dir)).is_err());
    }
}
