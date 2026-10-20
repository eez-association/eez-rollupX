//! Reusable Anvil-driven EEZ integration framework.
//!
//! The crate intentionally lives as a dev-dependency of protocol components:
//! it owns process lifecycle, deterministic chain fixtures, transaction helpers,
//! structured node signals, and the declarative scenario runner.

#![allow(missing_debug_implementations)]

mod artifacts;
mod l1_reorg;
pub mod native;
mod ports;
pub mod signals;
pub use l1_reorg::{L1ReorgControl, setup_cross_chain_with_l1_reorg};

use std::{
    collections::HashSet,
    convert::Infallible,
    fmt::Write as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use alloy_consensus::{EthereumTxEnvelope, Transaction as _, TxEip4844WithSidecar};
use alloy_eips::Decodable2718;
use alloy_primitives::{Address, B256, Bytes, Signature, U256, address, hex};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::{BlockNumHash, BlockNumberOrTag, TransactionReceipt, TransactionRequest};
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{SolCall, SolError, SolEvent, SolValue, sol};
use anyhow::{Context, Result, anyhow, bail, ensure};
use eez_control_rpc::{
    MAX_MESSAGE_BYTES, encode_prove_failure,
    v1::{
        InboundFailure, ProveChunk, ProveFailure, ProveResponse, prove_chunk, prove_failure,
        prover_client::ProverClient,
        prover_server::{Prover, ProverServer},
    },
};
use eez_protocol::EEZL2_ADDRESS;
use http_body_util::Full;
use hyper::{Request as HttpRequest, Response as HttpResponse, StatusCode, body::Incoming};
use hyper_util::rt::TokioIo;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status, Streaming};

pub use crate::signals::NodeSignal;
use crate::{artifacts::FailureDatadir, ports::PortLease};

/// Anvil's first default account (mnemonic `test test test test test test test test test test test junk`).
pub const ANVIL_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";
pub const ANVIL_ADDR: Address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
pub const ANVIL_KEY_1: &str = "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";
pub const ANVIL_KEY_2: &str = "0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a";
pub const ANVIL_KEY_3: &str = "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6";
pub const ANVIL_KEY_4: &str = "0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a";
/// Second cross-chain sender. Eviction cascades along one sender's nonce chain,
/// so a poison tx needs its own sender to leave co-bundled survivors alone.
pub const ANVIL_KEY_6: &str = "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e";
/// Unfunded proof-signer identity, separate from every transaction sender.
pub const ANVIL_ATTESTER_KEY: &str =
    "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba";
pub const ANVIL_ADDR_3: Address = address!("0x90F79bf6EB2c4f870365E785982E1f101E93b906");
/// Keyless protocol sender, shared by every composer, follower and signer.
pub const L2_SYSTEM_ADDRESS: Address = eez_primitives::SYSTEM_ADDRESS;

// K = L1/L2 = 2 leaves one L2 slot for proving.
const L1_BLOCK_TIME_SECS: u64 = 4;
// `anvil_reorg` can assign replacement-block timestamps between regular
// four-second mining ticks. Model one-second Beacon slots (with skipped slots)
// so every canonical Anvil block still has an unambiguous consensus slot.
const BEACON_FIXTURE_SECONDS_PER_SLOT: u64 = 1;

// Consumed by the launcher as `--chain`; never forwarded to `eez-node`.
const TEST_L2_GENESIS_ENV: &str = "EEZ_TEST_L2_GENESIS_PATH";

const TX_SPAM_INTERVAL: Duration = Duration::from_secs(1);

static LOG_COUNTER: AtomicUsize = AtomicUsize::new(0);
static WORKSPACE_BUILD_LOCK: Mutex<()> = Mutex::new(());

pub const FATAL_SIGNALS: &[&str] = signals::FATAL;

pub fn repo_root() -> PathBuf {
    manifest_dir()
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn manifest_dir() -> PathBuf {
    // nextest remaps this at runtime when executing an archive on another runner.
    std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into())
}

/// Stop dotenv's parent-directory search, including when TMPDIR or retained
/// test datadirs live below a checkout containing real deployment settings.
fn isolate_dotenv(directory: &std::path::Path) -> Result<()> {
    for name in [".env", "deployments.env"] {
        std::fs::write(directory.join(name), "")
            .with_context(|| format!("isolate test {name} in {}", directory.display()))?;
    }
    Ok(())
}

pub fn anvil_bin() -> String {
    for path in [
        "/root/.foundry/bin/anvil",
        &format!(
            "{}/.foundry/bin/anvil",
            std::env::var("HOME").unwrap_or_default()
        ),
    ] {
        if std::path::Path::new(path).exists() {
            return path.to_string();
        }
    }
    "anvil".to_string()
}

pub struct Anvil {
    child: Child,
    pub rpc_url: String,
    log_path: PathBuf,
    _log_dir: Option<tempfile::TempDir>,
}

struct AnvilConfig {
    block_time_secs: u64,
    gas_limit: u64,
    genesis_timestamp: u64,
}

impl AnvilConfig {
    fn standard(genesis_timestamp: u64) -> Self {
        Self {
            block_time_secs: L1_BLOCK_TIME_SECS,
            gas_limit: 30_000_000,
            genesis_timestamp,
        }
    }
}

impl Anvil {
    async fn spawn_with(port_lease: PortLease, cfg: AnvilConfig) -> Result<Self> {
        let port = port_lease.port();
        let (log_path, log_dir) = test_log_destination("anvil")?;
        let log = std::fs::File::create(&log_path).context("create anvil log")?;
        let err_log = log.try_clone().context("clone anvil log")?;
        let mut cmd = Command::new(anvil_bin());
        cmd.args([
            "--port",
            &port.to_string(),
            "--chain-id",
            &DEV_CHAIN_ID.to_string(),
            "--block-time",
            &cfg.block_time_secs.to_string(),
            "--hardfork",
            "osaka",
            "--silent",
        ]);
        cmd.args(["--gas-limit", &cfg.gas_limit.to_string()]);
        cmd.args(["--timestamp", &cfg.genesis_timestamp.to_string()]);
        cmd.stdout(Stdio::from(log)).stderr(Stdio::from(err_log));
        drop(port_lease);
        let mut child = cmd.spawn().context("spawn anvil")?;
        let rpc_url = format!("http://127.0.0.1:{port}");
        let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
        let startup_timeout = Duration::from_secs(10);
        let start = Instant::now();
        while start.elapsed() < startup_timeout {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let probe = tokio::time::timeout(Duration::from_secs(1), provider.get_block_number());
            if matches!(probe.await, Ok(Ok(_))) {
                return Ok(Self {
                    child,
                    rpc_url,
                    log_path,
                    _log_dir: log_dir,
                });
            }
            if let Some(status) = child.try_wait().context("poll anvil process")? {
                bail!(
                    "anvil exited before RPC became ready with {status}; log:\n{}",
                    std::fs::read_to_string(&log_path).unwrap_or_default(),
                );
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        bail!(
            "anvil did not start within {startup_timeout:?} on port {port}; log:\n{}",
            std::fs::read_to_string(&log_path).unwrap_or_default(),
        );
    }

    pub async fn set_balance(&self, addr: Address, wei: U256) -> Result<()> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let _: serde_json::Value = provider
            .client()
            .request("anvil_setBalance", (addr, wei))
            .await
            .context("anvil_setBalance")?;
        Ok(())
    }

    pub async fn reorg(&self, depth: u64) -> Result<()> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let _: serde_json::Value = provider
            .client()
            .request("anvil_reorg", (depth, Vec::<serde_json::Value>::new()))
            .await
            .context("anvil_reorg")?;
        // Anvil rewinds its wall-clock offset to the common ancestor when it
        // reorgs. Without restoring it, all future blocks stay depth * cadence
        // seconds behind real time and the composer's submission windows expire
        // before their parent exists. A real L1 reorg does not rewind the clock.
        // Preserve monotonicity even if a test explicitly mined into the future.
        let head = provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| anyhow!("missing Anvil head after reorg"))?;
        let wall_time_ms = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis(),
        )?;
        let timestamp_ms = wall_time_ms.max(head.header.timestamp * 1000);
        let _: serde_json::Value = provider
            .client()
            .request("evm_setTime", (timestamp_ms,))
            .await
            .context("restore Anvil wall clock after reorg")?;
        Ok(())
    }
}

impl Drop for Anvil {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "\n== anvil log tail ==\n{}",
                last_lines(&self.log_path, 40, None)
            );
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Forwards single-transaction `eth_sendBundle` payloads to Anvil.
///
/// Anvil has no builder API, so the stub explicitly rejects bundles containing
/// zero or multiple transactions instead of pretending to preserve atomicity.
struct BundleStub {
    child: Child,
    url: String,
    log_path: PathBuf,
    _log_dir: Option<tempfile::TempDir>,
    capture_dir: tempfile::TempDir,
}

impl BundleStub {
    async fn spawn(port_lease: PortLease, upstream: &str) -> Result<Self> {
        Self::spawn_with_mode(port_lease, upstream, false).await
    }

    async fn spawn_passthrough(port_lease: PortLease, upstream: &str) -> Result<Self> {
        Self::spawn_with_mode(port_lease, upstream, true).await
    }

    async fn spawn_with_mode(
        port_lease: PortLease,
        upstream: &str,
        passthrough: bool,
    ) -> Result<Self> {
        let port = port_lease.port();
        let script = repo_root().join("scripts/builder-stub.py");
        let listen = format!("127.0.0.1:{port}");
        let (log_path, log_dir) = test_log_destination("builder-stub")?;
        let capture_dir = tempfile::tempdir().context("builder blob capture tempdir")?;
        let control_dir = capture_dir.path().join("control");
        std::fs::create_dir(&control_dir)?;
        let log = std::fs::File::create(&log_path).context("create builder stub log")?;
        let err_log = log.try_clone().context("clone builder stub log")?;
        let mut command = Command::new("python3");
        command
            .arg(script)
            .args(["--listen", &listen, "--upstream", upstream])
            .arg("--capture-dir")
            .arg(capture_dir.path())
            .arg("--test-control-dir")
            .arg(&control_dir);
        if passthrough {
            command.arg("--passthrough-bundles");
        }
        command
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err_log));
        drop(port_lease);
        let mut child = command.spawn().context("spawn builder-stub.py")?;
        let url = format!("http://{listen}");
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if std::net::TcpStream::connect(&listen).is_ok() {
                return Ok(Self {
                    child,
                    url,
                    log_path,
                    _log_dir: log_dir,
                    capture_dir,
                });
            }
            if let Some(status) = child.try_wait().context("poll builder stub process")? {
                bail!(
                    "builder stub exited before binding {listen} with {status}; log:\n{}",
                    std::fs::read_to_string(&log_path).unwrap_or_default(),
                );
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        bail!(
            "builder-stub did not bind within 3s on {listen}; log:\n{}",
            std::fs::read_to_string(&log_path).unwrap_or_default(),
        );
    }

    fn capture_dir(&self) -> &std::path::Path {
        self.capture_dir.path()
    }
}

impl Drop for BundleStub {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "\n== builder stub log tail ==\n{}",
                last_lines(&self.log_path, 40, None),
            );
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Clone)]
struct BeaconFixtureState {
    anvil_rpc: String,
    capture_dir: PathBuf,
    genesis_time: u64,
}

/// Test-only Beacon REST fixture backed by canonical Anvil receipts.
///
/// The builder stub captures the exact pooled EIP-4844 envelope before it is
/// forwarded to Anvil. Sidecar requests only expose captures whose receipt is
/// currently canonical and whose execution timestamp maps to the requested
/// slot, so `anvil_reorg` also removes orphaned sidecars from this view.
struct BeaconStub {
    endpoint: String,
    task: JoinHandle<()>,
}

impl BeaconStub {
    async fn spawn(anvil_rpc: String, capture_dir: PathBuf, genesis_time: u64) -> Result<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let state = BeaconFixtureState {
            anvil_rpc,
            capture_dir,
            genesis_time,
        };
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let state = state.clone();
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |request| {
                        beacon_fixture_response(request, state.clone())
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Ok(Self {
            endpoint: format!("http://{address}"),
            task,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
}

impl Drop for BeaconStub {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Test-only marker for [`setup_cross_chain_with_env`]: when set to `1`, the
/// Composer's relay list is a [`RejectingBuilder`] followed by the normal
/// forwarding stub. Consumed by the fixture, never passed to the node.
pub const REJECTING_BUILDER_ENV: &str = "EEZ_TEST_REJECTING_BUILDER";

/// Test-only marker for [`setup_cross_chain_with_env`]: when set to `1`, the
/// fixture deploys `contracts/src/EEZPostBatcher.sol` for the registry from
/// [`POST_BATCHER_DEPLOYER`] and sets `EEZ_L1_POSTBATCH_TARGET` to it. Consumed
/// by the fixture, never passed to the node.
pub const POST_BATCHER_ENV: &str = "EEZ_TEST_POST_BATCHER";

/// Deploys the [`POST_BATCHER_ENV`] wrapper at its first L1 nonce, so the node
/// can be configured with the address before the wrapper exists.
pub const POST_BATCHER_DEPLOYER: &str = ANVIL_KEY_6;

/// Test-only marker for [`setup_cross_chain_with_env`]: the proof signer's
/// `EEZ_PROOF_SIGNER_MAX_REQUEST_BLOCKS`, which the Composer adopts as its
/// batch block cap. Consumed by the fixture, never passed to the node.
pub const SIGNER_MAX_REQUEST_BLOCKS_ENV: &str = "EEZ_TEST_SIGNER_MAX_REQUEST_BLOCKS";

/// `eth_sendBundle` relay that answers every request with a JSON-RPC
/// rejection (`-32000`), so multi-relay tests can show that one failing relay
/// does not prevent settlement through another.
pub struct RejectingBuilder {
    endpoint: String,
    bundle_requests: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl RejectingBuilder {
    async fn spawn() -> Result<Self> {
        use http_body_util::BodyExt as _;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let bundle_requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&bundle_requests);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let counter = Arc::clone(&counter);
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(
                        move |request: HttpRequest<Incoming>| {
                            let counter = Arc::clone(&counter);
                            async move {
                                let body = request
                                    .into_body()
                                    .collect()
                                    .await
                                    .map(http_body_util::Collected::to_bytes);
                                let request: serde_json::Value = body
                                    .ok()
                                    .and_then(|body| serde_json::from_slice(&body).ok())
                                    .unwrap_or_default();
                                if request["method"] == "eth_sendBundle" {
                                    counter.fetch_add(1, Ordering::SeqCst);
                                }
                                Ok::<_, Infallible>(beacon_json_response(
                                    StatusCode::OK,
                                    serde_json::json!({
                                        "jsonrpc": "2.0",
                                        "id": request["id"],
                                        "error": {"code": -32000, "message": "fixture builder rejects every bundle"},
                                    }),
                                ))
                            }
                        },
                    );
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Ok(Self {
            endpoint: format!("http://{address}"),
            bundle_requests,
            task,
        })
    }

    /// `eth_sendBundle` requests received (and rejected) so far.
    pub fn bundle_requests(&self) -> usize {
        self.bundle_requests.load(Ordering::SeqCst)
    }
}

impl Drop for RejectingBuilder {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn beacon_fixture_response(
    request: HttpRequest<Incoming>,
    state: BeaconFixtureState,
) -> Result<HttpResponse<Full<hyper::body::Bytes>>, Infallible> {
    let path = request.uri().path();
    let response = if path == "/eth/v1/beacon/genesis" {
        serde_json::json!({"data": {"genesis_time": state.genesis_time.to_string()}}).to_string()
    } else if path == "/eth/v1/config/spec" {
        serde_json::json!({"data": {"SECONDS_PER_SLOT": BEACON_FIXTURE_SECONDS_PER_SLOT.to_string()}})
            .to_string()
    } else if let Some(raw_slot) = path.strip_prefix("/eth/v1/beacon/blobs/") {
        match raw_slot.parse::<u64>() {
            Ok(slot) => match canonical_blob_sidecars(&state, slot).await {
                Ok(sidecars) => serde_json::json!({
                    "data": sidecars
                        .iter()
                        .filter_map(|sidecar| sidecar.get("blob"))
                        .collect::<Vec<_>>()
                })
                .to_string(),
                Err(error) => {
                    return Ok(beacon_json_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        serde_json::json!({"code": 500, "message": error.to_string()}),
                    ));
                }
            },
            Err(error) => {
                return Ok(beacon_json_response(
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"code": 400, "message": error.to_string()}),
                ));
            }
        }
    } else if let Some(raw_slot) = path.strip_prefix("/eth/v1/beacon/blob_sidecars/") {
        match raw_slot.parse::<u64>() {
            Ok(slot) => match canonical_blob_sidecars(&state, slot).await {
                Ok(sidecars) => serde_json::json!({"data": sidecars}).to_string(),
                Err(error) => {
                    return Ok(beacon_json_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        serde_json::json!({"code": 500, "message": error.to_string()}),
                    ));
                }
            },
            Err(error) => {
                return Ok(beacon_json_response(
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"code": 400, "message": error.to_string()}),
                ));
            }
        }
    } else {
        return Ok(beacon_json_response(
            StatusCode::NOT_FOUND,
            serde_json::json!({"code": 404, "message": "not found"}),
        ));
    };
    Ok(beacon_json_response(
        StatusCode::OK,
        serde_json::from_str(&response).expect("fixture generated valid JSON"),
    ))
}

fn beacon_json_response(
    status: StatusCode,
    value: serde_json::Value,
) -> HttpResponse<Full<hyper::body::Bytes>> {
    HttpResponse::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(Full::new(hyper::body::Bytes::from(value.to_string())))
        .expect("static Beacon fixture response")
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapturedBlobTransaction {
    raw: String,
    transaction_hash: Option<B256>,
}

type PooledBlobEnvelope = EthereumTxEnvelope<TxEip4844WithSidecar>;

fn read_captured_blob_transaction(path: &std::path::Path) -> Result<PooledBlobEnvelope> {
    let capture: CapturedBlobTransaction = serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("decode {}", path.display()))?;
    let raw = hex::decode(capture.raw.strip_prefix("0x").unwrap_or(&capture.raw))
        .with_context(|| format!("decode pooled blob tx in {}", path.display()))?;
    let envelope = PooledBlobEnvelope::decode_2718(&mut raw.as_slice())
        .with_context(|| format!("decode pooled blob envelope in {}", path.display()))?;
    if capture
        .transaction_hash
        .is_some_and(|captured| captured != *envelope.tx_hash())
    {
        bail!(
            "captured blob transaction hash does not match envelope in {}",
            path.display()
        );
    }
    Ok(envelope)
}

async fn canonical_blob_sidecars(
    state: &BeaconFixtureState,
    requested_slot: u64,
) -> Result<Vec<serde_json::Value>> {
    let provider = ProviderBuilder::new().connect_http(state.anvil_rpc.parse()?);
    let mut captures = std::fs::read_dir(&state.capture_dir)
        .with_context(|| format!("read blob captures at {}", state.capture_dir.display()))?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    captures.sort();

    let mut sidecars = Vec::new();
    for path in captures {
        let envelope = read_captured_blob_transaction(&path)?;
        let transaction_hash = *envelope.tx_hash();
        let Some(receipt) = provider.get_transaction_receipt(transaction_hash).await? else {
            continue;
        };
        let Some(block_hash) = receipt.block_hash else {
            continue;
        };
        let Some(block) = provider.get_block_by_hash(block_hash).await? else {
            continue;
        };
        let Some(since_genesis) = block.header.timestamp.checked_sub(state.genesis_time) else {
            continue;
        };
        if !since_genesis.is_multiple_of(BEACON_FIXTURE_SECONDS_PER_SLOT)
            || since_genesis / BEACON_FIXTURE_SECONDS_PER_SLOT != requested_slot
        {
            continue;
        }

        let signed = envelope
            .as_eip4844()
            .ok_or_else(|| anyhow!("captured transaction is not EIP-4844"))?;
        let captured = signed.tx().sidecar();
        // The legacy Beacon endpoint requires whole-blob proofs, while Osaka
        // propagation carries cell proofs. Derive the former from the exact
        // captured blobs and retain their authenticated commitment identity.
        let sidecar = match captured.as_eip4844() {
            Some(sidecar) => std::borrow::Cow::Borrowed(sidecar),
            None => std::borrow::Cow::Owned(
                alloy_eips::eip4844::BlobTransactionSidecar::try_from_blobs(
                    captured.blobs().to_vec(),
                )
                .context("derive legacy Beacon proofs for Osaka blobs")?,
            ),
        };
        ensure!(
            sidecar.commitments.as_slice() == captured.commitments(),
            "captured blob commitments do not match physical blobs"
        );
        for ((blob, commitment), proof) in sidecar
            .blobs
            .iter()
            .zip(&sidecar.commitments)
            .zip(&sidecar.proofs)
        {
            sidecars.push(serde_json::json!({
                "index": sidecars.len().to_string(),
                "blob": format!("0x{}", hex::encode(blob.as_slice())),
                "kzg_commitment": format!("0x{}", hex::encode(commitment.as_slice())),
                "kzg_proof": format!("0x{}", hex::encode(proof.as_slice())),
            }));
        }
    }
    Ok(sidecars)
}

/// Persists logs under `EEZ_TEST_LOG_DIR`, or owns them in a temporary directory.
fn test_log_destination(name: &str) -> Result<(PathBuf, Option<tempfile::TempDir>)> {
    if let Ok(dir) = std::env::var("EEZ_TEST_LOG_DIR") {
        std::fs::create_dir_all(&dir).with_context(|| format!("create EEZ_TEST_LOG_DIR {dir}"))?;
        let suffix = LOG_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(dir).join(format!("{name}-{}-{suffix}.log", std::process::id()));
        return Ok((path, None));
    }
    let dir = tempfile::tempdir().context("log tempdir")?;
    let path = dir.path().join(format!("{name}.log"));
    Ok((path, Some(dir)))
}

fn proof_signer_binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("EEZ_TEST_PROOF_SIGNER_BIN") {
        let path = PathBuf::from(path);
        anyhow::ensure!(
            path.is_file(),
            "configured proof signer is missing: {}",
            path.display()
        );
        return Ok(path);
    }
    if std::env::var_os("EEZ_TEST_PREBUILT").is_some() {
        bail!("prebuilt tests require EEZ_TEST_PROOF_SIGNER_BIN; refusing to compile");
    }
    let current = std::env::current_exe().context("current test executable")?;
    let target_profile = current
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or_else(|| anyhow!("test executable has no target profile directory"))?;
    let path = target_profile.join(format!("eez-proof-signer{}", std::env::consts::EXE_SUFFIX));

    // Keep concurrent test fns from racing the build. Do not treat an existing
    // binary as current: `cargo test` builds the signer's test executable, not the
    // un-hashed executable launched below, so an ABI change can otherwise
    // leave these integration tests running a stale signer indefinitely.
    let _build_guard = WORKSPACE_BUILD_LOCK
        .lock()
        .map_err(|_| anyhow!("workspace build lock poisoned"))?;
    let profile = target_profile
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("target profile directory is not valid UTF-8"))?;
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.current_dir(repo_root()).args([
        "build",
        "--locked",
        "-p",
        "eez-proof-signer",
        "--bin",
        "eez-proof-signer",
    ]);
    if profile != "debug" {
        command.args(["--profile", profile]);
    }
    let status = command.status().context("build eez-proof-signer")?;
    if !status.success() {
        bail!("building eez-proof-signer failed with {status}");
    }
    path.is_file()
        .then_some(path)
        .ok_or_else(|| anyhow!("eez-proof-signer build completed without producing the binary"))
}

/// Configuration for one real `eez-proof-signer` process.
#[derive(Debug)]
/// Launch configuration for real-signer native and retained-history tests.
pub struct ProofSignerConfig<'a> {
    pub l1_rpc_url: String,
    pub registry_address: Address,
    pub chain_config: &'a std::path::Path,
    pub l1_chain_config: Option<String>,
    pub rollup_id: u64,
    pub signer_key: &'a str,
    /// Expected verifier key under the signer's validation policy. Negative
    /// tests can deliberately configure a key that differs from L1's registry.
    pub vkey: B256,
    pub proof_system: Address,
    /// Signer window block limit (`EEZ_PROOF_SIGNER_MAX_REQUEST_BLOCKS`). The
    /// Composer adopts it as its batch block cap; `None` keeps the default.
    pub max_request_blocks: Option<u64>,
}

/// The native Anvil harness and embedded test L1 use this same fork profile.
/// Its chain ID differs from the L2 fixture's execution chain ID.
fn write_l1_chain_config(directory: &std::path::Path) -> Result<PathBuf> {
    let mut config = fixture_genesis()?.config;
    config.chain_id = DEV_CHAIN_ID;
    config.extra_fields.remove("rollupX");
    let path = directory.join("l1-chain-config.json");
    std::fs::write(&path, serde_json::to_vec(&config)?).context("write native test L1 profile")?;
    Ok(path)
}

/// Owns a proof-signer process and its diagnostic log.
#[derive(Debug)]
pub struct ProofSignerHandle {
    child: std::sync::Mutex<Child>,
    endpoint: String,
    log_path: PathBuf,
    _log_dir: Option<tempfile::TempDir>,
    _working_dir: tempfile::TempDir,
}

impl ProofSignerHandle {
    /// Start the configured signer on a leased loopback port and wait for readiness.
    pub async fn spawn(cfg: &ProofSignerConfig<'_>) -> Result<Self> {
        let port_lease = PortLease::tcp();
        let listen = format!("127.0.0.1:{}", port_lease.port());
        let attester = signer_address(cfg.signer_key)?;
        let l2_system_address = L2_SYSTEM_ADDRESS;
        let (log_path, log_dir) = test_log_destination("eez-proof-signer")?;
        let working_dir = tempfile::tempdir().context("proof signer working directory")?;
        isolate_dotenv(working_dir.path())?;
        // The fixture's L1 uses the same configured fork schedule with the
        // separate dev chain ID. Every proof uses this operator-owned profile.
        let l1_config_path = write_l1_chain_config(working_dir.path())?;
        let l1_profile = cfg.l1_chain_config.as_deref().unwrap_or(
            l1_config_path
                .to_str()
                .context("non-UTF-8 L1 profile path")?,
        );
        let log = std::fs::File::create(&log_path).context("create proof signer log")?;
        let err_log = log.try_clone().context("clone proof signer log")?;
        let mut command = Command::new(proof_signer_binary()?);
        command
            // The signer also loads dotenv at startup. Keep its explicitly
            // constructed test environment isolated from repository config.
            .current_dir(working_dir.path())
            .args([
                "--listen-addr",
                &listen,
                "--chain-config",
                cfg.chain_config
                    .to_str()
                    .context("non-UTF-8 L2 genesis path")?,
                "--l1-rpc-url",
                &cfg.l1_rpc_url,
                "--registry-address",
                &format!("{:#x}", cfg.registry_address),
                "--l1-chain-config",
                l1_profile,
                "--rollup-id",
                &cfg.rollup_id.to_string(),
                "--vkey",
                &format!("{:#x}", cfg.vkey),
                "--attester-address",
                &format!("{attester:#x}"),
                "--proof-system",
                &format!("{:#x}", cfg.proof_system),
                "--l2-system-address",
                &format!("{l2_system_address:#x}"),
            ])
            .env_clear()
            .env("EEZ_PROOF_SIGNER_KEY", cfg.signer_key)
            .env("NO_COLOR", "1")
            .env("RUST_LOG", "info")
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(err_log));
        if let Some(blocks) = cfg.max_request_blocks {
            command.env("EEZ_PROOF_SIGNER_MAX_REQUEST_BLOCKS", blocks.to_string());
        }
        drop(port_lease);
        let mut child = command.spawn().context("spawn eez-proof-signer")?;

        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if tokio::net::TcpStream::connect(&listen).await.is_ok() {
                return Ok(Self {
                    child: std::sync::Mutex::new(child),
                    endpoint: format!("http://{listen}"),
                    log_path,
                    _log_dir: log_dir,
                    _working_dir: working_dir,
                });
            }
            if let Some(status) = child.try_wait().context("poll proof signer process")? {
                bail!(
                    "eez-proof-signer exited before binding with {status}; log:\n{}",
                    std::fs::read_to_string(&log_path).unwrap_or_default()
                );
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        bail!(
            "eez-proof-signer did not bind within 10s; log:\n{}",
            std::fs::read_to_string(&log_path).unwrap_or_default()
        )
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn log_tail(&self, max: usize) -> String {
        last_lines(&self.log_path, max, None)
    }

    fn successful_attestations(&self) -> Result<usize> {
        let contents = std::fs::read_to_string(&self.log_path)
            .with_context(|| format!("read signer log {}", self.log_path.display()))?;
        Ok(contents
            .lines()
            .filter_map(signals::parse)
            .filter(|signal| signal.name == "eez.proof_signer.window_signed")
            .count())
    }

    pub fn exit_status(&self) -> Option<std::process::ExitStatus> {
        self.child
            .lock()
            .expect("proof signer child mutex poisoned")
            .try_wait()
            .ok()
            .flatten()
    }

    pub fn assert_alive(&self) {
        if let Ok(Some(status)) = self
            .child
            .lock()
            .expect("proof signer child mutex poisoned")
            .try_wait()
        {
            panic!(
                "eez-proof-signer exited with {status}; log:\n{}",
                std::fs::read_to_string(&self.log_path).unwrap_or_default()
            );
        }
    }
}

impl Drop for ProofSignerHandle {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "\n== proof signer log tail ==\n{}",
                last_lines(&self.log_path, 60, None),
            );
        }
        let child = self
            .child
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ProverMutation {
    None,
    PostBatch,
    Witness,
    /// Deterministic opaque rejection used to exercise Composer recovery. This
    /// is the status returned by the real signer when its resource capacity is
    /// exceeded.
    ResourceExhausted,
    /// Reject even historical/anchor-only requests until an operator recovers it.
    AllResourceExhausted,
    /// Temporary transport/provider outage, without changing request contents.
    Unavailable,
    /// A typed rejection whose candidate identity cannot belong to the current
    /// request. The Composer must treat it as opaque rather than acting on it.
    MismatchedActionable,
}

impl ProverMutation {
    fn apply(self, chunks: &mut [ProveChunk]) -> Result<(), Status> {
        match self {
            Self::None => {}
            Self::AllResourceExhausted => {
                return Err(Status::resource_exhausted(
                    "fixture manual prover request limit",
                ));
            }
            Self::Unavailable => {
                return Err(Status::unavailable("fixture temporary upstream outage"));
            }
            Self::PostBatch => {
                let calldata = chunks
                    .iter_mut()
                    .find_map(|chunk| match chunk.kind.as_mut() {
                        Some(prove_chunk::Kind::Header(header)) => header
                            .post_batch
                            .as_mut()
                            .map(|post_batch| &mut post_batch.abi_calldata),
                        _ => None,
                    })
                    .ok_or_else(|| Status::internal("missing Prove header"))?;
                let byte = calldata
                    .last_mut()
                    .ok_or_else(|| Status::internal("empty PostBatch calldata"))?;
                *byte ^= 1;
            }
            Self::Witness => {
                let witness = chunks
                    .iter_mut()
                    .find_map(|chunk| match chunk.kind.as_mut() {
                        Some(prove_chunk::Kind::Block(block)) => block.witness.as_mut(),
                        _ => None,
                    })
                    .ok_or_else(|| Status::internal("missing block witness"))?;
                // Witnesses may carry surplus nodes that validation never needs,
                // and node order varies between attempts. Corrupting only one
                // node can hit a surplus one, which the signer correctly
                // ignores. Corrupt every node so a needed one is always hit.
                let mut corrupted = false;
                for byte in witness.state.iter_mut().filter_map(|node| node.first_mut()) {
                    *byte ^= 1;
                    corrupted = true;
                }
                if !corrupted {
                    witness.state.push(vec![0xff]);
                }
            }
            Self::ResourceExhausted | Self::MismatchedActionable => {
                let calldata = chunks
                    .iter()
                    .find_map(|chunk| match chunk.kind.as_ref() {
                        Some(prove_chunk::Kind::Header(header)) => header
                            .post_batch
                            .as_ref()
                            .map(|post_batch| post_batch.abi_calldata.as_slice()),
                        _ => None,
                    })
                    .ok_or_else(|| Status::internal("missing Prove header"))?;
                let batch = eez_protocol::entries::decode_postbatch(calldata)
                    .map_err(|error| Status::internal(format!("decode PostBatch: {error}")))?;
                // Entry zero is the state-chain anchor. The real checkpoint
                // quota is exercised only by effect candidates, so leave
                // anchor-only historical/minimal proofs healthy.
                if batch.entries.len() > 1 {
                    return match self {
                        Self::ResourceExhausted => Err(Status::resource_exhausted(
                            "prover resource capacity exceeded",
                        )),
                        Self::MismatchedActionable => {
                            let failure = ProveFailure {
                                actionable_failure: Some(
                                    prove_failure::ActionableFailure::Inbound(InboundFailure {
                                        entry_index: u32::MAX,
                                        entry_hash: vec![0xff; 32],
                                    }),
                                ),
                            };
                            Err(Status::with_details(
                                tonic::Code::FailedPrecondition,
                                "candidate identity does not match the request",
                                encode_prove_failure(&failure).into(),
                            ))
                        }
                        Self::None
                        | Self::PostBatch
                        | Self::Witness
                        | Self::AllResourceExhausted
                        | Self::Unavailable => unreachable!(),
                    };
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct ProxyCounters {
    attempts: AtomicUsize,
    successes: AtomicUsize,
    rejections: AtomicUsize,
}

/// Bound complete wire captures, including framing, without changing the RPC input.
fn encode_fixture_stream<M: prost::Message>(chunks: &[M]) -> Result<Vec<u8>, Status> {
    let bytes = chunks.iter().try_fold(0usize, |sum, chunk| {
        sum.checked_add(chunk.encoded_len())
            .and_then(|n| n.checked_add(prost::length_delimiter_len(chunk.encoded_len())))
            .ok_or_else(|| Status::resource_exhausted("fixture size overflow"))
    })?;
    if bytes > 16 * 1024 * 1024 {
        return Err(Status::resource_exhausted("fixture capture exceeds 16 MiB"));
    }
    let mut encoded = Vec::with_capacity(bytes);
    for chunk in chunks {
        chunk
            .encode_length_delimited(&mut encoded)
            .expect("Vec encoding");
    }
    Ok(encoded)
}

/// Expand only captured references, following parent hashes through the exact
/// preparation witnesses. The RPC still forwards the untouched warm request;
/// signer regressions independently validate this cold stream and compare its
/// attestation with the original response.
fn cold_fixture_stream(chunks: &[ProveChunk], directory: &Path) -> Result<Vec<u8>, Status> {
    use prost::Message as _;
    let mut encoded = Vec::new();
    let append = |output: &mut Vec<u8>, bytes: &[u8]| -> Result<(), Status> {
        if output.len().saturating_add(bytes.len()) > 16 * 1024 * 1024 {
            return Err(Status::resource_exhausted("cold fixture exceeds 16 MiB"));
        }
        output.extend_from_slice(bytes);
        Ok(())
    };
    for chunk in chunks {
        let Some(prove_chunk::Kind::CachedRange(range)) = &chunk.kind else {
            append(
                &mut encoded,
                &encode_fixture_stream(std::slice::from_ref(chunk))?,
            )?;
            continue;
        };
        if range
            .last_number
            .checked_sub(range.first_number)
            .is_none_or(|n| n >= 2048)
        {
            return Err(Status::resource_exhausted("fixture cached range limit"));
        }
        let mut expected = B256::try_from(range.last_hash.as_slice())
            .map_err(|_| Status::invalid_argument("fixture range hash length"))?;
        let mut blocks = Vec::new();
        let mut bytes = 0usize;
        for number in (range.first_number..=range.last_number).rev() {
            let path = directory
                .join("preparation")
                .join(format!("{expected:x}.protobuf"));
            let size = std::fs::metadata(&path)
                .map_err(|error| Status::internal(format!("missing prepared fixture: {error}")))?
                .len();
            if size > (16 * 1024 * 1024 - bytes) as u64 {
                return Err(Status::resource_exhausted(
                    "prepared fixture exceeds 16 MiB",
                ));
            }
            let wire = std::fs::read(path)
                .map_err(|error| Status::internal(format!("read prepared fixture: {error}")))?;
            bytes = bytes.saturating_add(wire.len());
            if bytes > 16 * 1024 * 1024 {
                return Err(Status::resource_exhausted(
                    "prepared fixture exceeds 16 MiB",
                ));
            }
            let mut remaining = wire.as_slice();
            let prepared = ProveChunk::decode_length_delimited(&mut remaining)
                .map_err(|error| Status::internal(format!("decode prepared fixture: {error}")))?;
            let Some(prove_chunk::Kind::Block(block)) = prepared.kind else {
                return Err(Status::internal("prepared fixture is not a block"));
            };
            if !remaining.is_empty()
                || block.number != number
                || block.hash != expected.as_slice()
                || (number == range.first_number && block.hash != range.first_hash)
            {
                return Err(Status::internal(
                    "prepared fixture does not match cached range",
                ));
            }
            expected = B256::try_from(block.parent_hash.as_slice())
                .map_err(|_| Status::invalid_argument("fixture parent hash length"))?;
            blocks.push(wire);
        }
        for wire in blocks.into_iter().rev() {
            append(&mut encoded, &wire)?;
        }
    }
    Ok(encoded)
}

#[derive(Clone, Debug)]
struct ProverProxyService {
    upstream: String,
    fixture_directory: Option<PathBuf>,
    mutation: Arc<Mutex<ProverMutation>>,
    counters: Arc<ProxyCounters>,
}

#[tonic::async_trait]
impl Prover for ProverProxyService {
    async fn retain_ordinary_ranges(
        &self,
        request: Request<eez_control_rpc::v1::RetainOrdinaryRangesRequest>,
    ) -> Result<Response<eez_control_rpc::v1::RetainOrdinaryRangesResponse>, Status> {
        ProverClient::connect(self.upstream.clone())
            .await
            .map_err(|error| Status::unavailable(error.to_string()))?
            .retain_ordinary_ranges(request)
            .await
    }

    async fn prepare_ordinary(
        &self,
        request: Request<Streaming<eez_control_rpc::v1::PrepareChunk>>,
    ) -> Result<Response<eez_control_rpc::v1::PrepareResponse>, Status> {
        let mut input = request.into_inner();
        let mut chunks = Vec::new();
        while let Some(chunk) = input.message().await? {
            if chunks.len() >= 65 {
                return Err(Status::resource_exhausted("preparation proxy chunk limit"));
            }
            chunks.push(chunk);
        }
        // Cached proving requests omit these witnesses. Explicit fixture export
        // must retain the original preparation input for independent cold replay.
        let fixture = self
            .fixture_directory
            .as_ref()
            .map(|_| {
                let mut total = 0usize;
                chunks
                    .iter()
                    .filter_map(|chunk| match &chunk.kind {
                        Some(eez_control_rpc::v1::prepare_chunk::Kind::Block(block)) => Some(block),
                        _ => None,
                    })
                    .map(|block| {
                        let hash = B256::try_from(block.hash.as_slice())
                            .map_err(|_| Status::invalid_argument("fixture block hash length"))?;
                        let encoded = encode_fixture_stream(&[ProveChunk {
                            kind: Some(prove_chunk::Kind::Block(block.clone())),
                        }])?;
                        total = total.saturating_add(encoded.len());
                        if total > 16 * 1024 * 1024 {
                            return Err(Status::resource_exhausted(
                                "preparation fixture exceeds 16 MiB",
                            ));
                        }
                        Ok((hash, encoded))
                    })
                    .collect::<Result<Vec<_>, Status>>()
            })
            .transpose()?;
        let response = ProverClient::connect(self.upstream.clone())
            .await
            .map_err(|error| Status::unavailable(error.to_string()))?
            .prepare_ordinary(tokio_stream::iter(chunks))
            .await?;
        if let (Some(directory), Some(encoded)) = (&self.fixture_directory, fixture) {
            let directory = directory.join("preparation");
            std::fs::create_dir_all(&directory).map_err(|error| {
                Status::internal(format!("create preparation fixture dir: {error}"))
            })?;
            for (hash, block) in encoded {
                let mut file = tempfile::NamedTempFile::new_in(&directory).map_err(|error| {
                    Status::internal(format!("create prepared fixture: {error}"))
                })?;
                std::io::Write::write_all(file.as_file_mut(), &block).map_err(|error| {
                    Status::internal(format!("write preparation fixture: {error}"))
                })?;
                file.persist(directory.join(format!("{hash:x}.protobuf")))
                    .map_err(|error| {
                        Status::internal(format!("publish preparation fixture: {error}"))
                    })?;
            }
        }
        Ok(response)
    }

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

    async fn prove(
        &self,
        request: Request<Streaming<ProveChunk>>,
    ) -> Result<Response<ProveResponse>, Status> {
        self.counters.attempts.fetch_add(1, Ordering::Relaxed);
        let mut input = request.into_inner();
        let mut chunks = Vec::new();
        while let Some(chunk) = input.message().await? {
            chunks.push(chunk);
        }
        let mutation = *self.mutation.lock().unwrap();
        if let Err(status) = mutation.apply(&mut chunks) {
            if matches!(
                status.code(),
                tonic::Code::ResourceExhausted | tonic::Code::FailedPrecondition
            ) {
                self.counters.rejections.fetch_add(1, Ordering::Relaxed);
            }
            return Err(status);
        }

        // Explicit fixture export only. Preserve complete augmented witnesses
        // from the real composer; ordinary debug RPC misses observation-only
        // reads and removal-closure nodes. Never capture in normal test runs.
        let fixture = self
            .fixture_directory
            .as_ref()
            .map(|directory| {
                Ok::<_, Status>((
                    encode_fixture_stream(&chunks)?,
                    cold_fixture_stream(&chunks, directory)?,
                ))
            })
            .transpose()?;
        let mut client = ProverClient::connect(self.upstream.clone())
            .await
            .map_err(|error| Status::unavailable(format!("connect upstream signer: {error}")))?
            .max_encoding_message_size(MAX_MESSAGE_BYTES)
            .max_decoding_message_size(MAX_MESSAGE_BYTES);
        match client.prove(tokio_stream::iter(chunks)).await {
            Ok(response) => {
                if let (Some(directory), Some((encoded, cold))) = (&self.fixture_directory, fixture)
                {
                    let hash = alloy_primitives::keccak256(&encoded);
                    let path = directory.join(format!("{hash:x}"));
                    std::fs::write(path.with_extension("protobuf"), &encoded).map_err(|error| {
                        Status::internal(format!("write proving fixture: {error}"))
                    })?;
                    std::fs::write(path.with_extension("cold.protobuf"), &cold).map_err(
                        |error| Status::internal(format!("write cold proving fixture: {error}")),
                    )?;
                    std::fs::write(path.with_extension("json"), serde_json::to_vec_pretty(&serde_json::json!({
                        "request_hash": hash,
                        "request_bytes": encoded.len(),
                        "cold_request_hash": alloy_primitives::keccak256(&cold),
                        "cold_request_bytes": cold.len(),
                        "public_inputs_hash": hex::encode(&response.get_ref().public_inputs_hash),
                        "signature": hex::encode(&response.get_ref().signature),
                    })).expect("fixture metadata serialization"))
                        .map_err(|error| Status::internal(format!("write proving fixture metadata: {error}")))?;
                }
                self.counters.successes.fetch_add(1, Ordering::Relaxed);
                Ok(response)
            }
            Err(status) => {
                // Transport failures do not prove that the signer rejected the input.
                if matches!(
                    status.code(),
                    tonic::Code::InvalidArgument
                        | tonic::Code::FailedPrecondition
                        | tonic::Code::ResourceExhausted
                ) {
                    self.counters.rejections.fetch_add(1, Ordering::Relaxed);
                }
                Err(Status::new(status.code(), status.message().to_string()))
            }
        }
    }
}

/// Optional mutation proxy in front of a real proof signer.
#[derive(Debug)]
pub struct ProverProxyHandle {
    endpoint: String,
    counters: Arc<ProxyCounters>,
    mutation: Arc<Mutex<ProverMutation>>,
    task: JoinHandle<()>,
}

impl ProverProxyHandle {
    async fn spawn(
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
            mutation: Arc::clone(&mutation),
            counters: Arc::clone(&counters),
        };
        let task = tokio::spawn(async move {
            let server = ProverServer::new(service)
                .max_decoding_message_size(MAX_MESSAGE_BYTES)
                .max_encoding_message_size(MAX_MESSAGE_BYTES);
            let _ = tonic::transport::Server::builder()
                .add_service(server)
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

/// Owns the L1, builder stub, deployment, genesis, and proof signers for a test.
pub struct Harness {
    pub anvil: Anvil,
    stub: BundleStub,
    beacon: BeaconStub,
    pub dep: Deployment,
    // The owning tempdir must outlive every node and signer using this genesis.
    l2_genesis: (PathBuf, tempfile::TempDir),
    // Each composer gets isolated state because the signer admits one request at a time.
    provers: std::sync::Mutex<Vec<(ProofSignerHandle, tempfile::TempDir)>>,
}

impl Harness {
    /// Starts a fixture with L1 and L2 genesis timestamps aligned to wall time.
    pub async fn fresh() -> Result<Self> {
        let ts = now_unix_secs();
        let cfg = AnvilConfig::standard(ts);
        Self::with_anvil_config(cfg).await
    }

    /// Starts the reorg fixture with one timestamp shared by Anvil and the L2.
    pub async fn for_reorg() -> Result<Self> {
        Self::fresh().await
    }

    async fn with_anvil_config(cfg: AnvilConfig) -> Result<Self> {
        let genesis_time = cfg.genesis_timestamp;
        let anvil = Anvil::spawn_with(PortLease::tcp(), cfg).await?;
        let reference = ProviderBuilder::new()
            .connect_http(anvil.rpc_url.parse()?)
            .get_block_by_number(0.into())
            .await?
            .context("Anvil genesis reference absent")?;
        ensure!(
            reference.header.hash == reference.header.inner.hash_slow(),
            "Anvil genesis header identity"
        );
        let l2_genesis = write_l2_genesis_at(genesis_time)?;
        bind_l2_genesis_reference(
            &l2_genesis.0,
            &reference.header.inner,
            2,
            u32::try_from(L1_BLOCK_TIME_SECS / 2)?,
        )?;
        let initial_block_hash = genesis_block_hash(&l2_genesis.0)?;
        write_l1_chain_config(l2_genesis.1.path())?;
        let stub = BundleStub::spawn(PortLease::tcp(), &anvil.rpc_url).await?;
        let beacon = BeaconStub::spawn(
            anvil.rpc_url.clone(),
            stub.capture_dir().to_path_buf(),
            genesis_time,
        )
        .await?;
        let dep =
            deploy_contracts_with_initial(&anvil.rpc_url, ANVIL_KEY, initial_block_hash).await?;
        Ok(Self {
            anvil,
            stub,
            beacon,
            dep,
            l2_genesis,
            provers: std::sync::Mutex::new(Vec::new()),
        })
    }

    pub fn chain(&self) -> Chain<'_> {
        Chain::new(&self.anvil, &self.dep)
    }

    /// Read the actual type-3 payload captured by the builder/Beacon fixture.
    pub fn captured_blob_transaction(&self, hash: B256) -> Result<PooledBlobEnvelope> {
        for entry in std::fs::read_dir(self.stub.capture_dir())? {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let envelope = read_captured_blob_transaction(&path)?;
                if *envelope.tx_hash() == hash {
                    return Ok(envelope);
                }
            }
        }
        bail!("no captured blob transaction for {hash}")
    }

    pub fn builder_rpc_url(&self) -> &str {
        &self.stub.url
    }

    /// Counts signer successes so negative tests cannot pass because proving stalled.
    pub fn successful_attestations(&self) -> Result<usize> {
        self.provers
            .lock()
            .map_err(|_| anyhow!("prover registry poisoned"))?
            .iter()
            .try_fold(0usize, |total, (signer, _)| {
                Ok(total + signer.successful_attestations()?)
            })
    }

    pub fn l2_genesis_path(&self) -> &std::path::Path {
        &self.l2_genesis.0
    }

    pub fn l2_genesis_block_hash(&self) -> Result<B256> {
        genesis_block_hash(&self.l2_genesis.0)
    }

    pub async fn env(&self) -> Result<Vec<(&'static str, String)>> {
        self.env_for(ANVIL_KEY, false).await
    }

    /// Composer environment that posts the native operations and semantic
    /// payload as an EIP-4844 transaction and retrieves it through the Beacon
    /// REST fixture.
    pub async fn blob_env(&self) -> Result<Vec<(&'static str, String)>> {
        self.env().await
    }

    pub async fn env_for(
        &self,
        poster_key: &str,
        expect_external_batches: bool,
    ) -> Result<Vec<(&'static str, String)>> {
        self.env_for_options(NodeEnvOptions {
            poster_key: Some(poster_key),
            proof_signer_key: Some(ANVIL_ATTESTER_KEY),
            rollup_id: self.dep.rollup_id,
            expect_external_batches,
            sequencer_rpc: None,
        })
        .await
    }

    pub async fn follower_env(
        &self,
        sequencer_rpc: Option<&str>,
    ) -> Result<Vec<(&'static str, String)>> {
        self.env_for_options(NodeEnvOptions {
            poster_key: None,
            proof_signer_key: None,
            rollup_id: self.dep.rollup_id,
            expect_external_batches: true,
            sequencer_rpc,
        })
        .await
    }

    pub async fn env_with_rollup_id(&self, rollup_id: u64) -> Result<Vec<(&'static str, String)>> {
        self.env_for_options(NodeEnvOptions {
            poster_key: Some(ANVIL_KEY),
            proof_signer_key: Some(ANVIL_ATTESTER_KEY),
            rollup_id,
            expect_external_batches: false,
            sequencer_rpc: None,
        })
        .await
    }

    pub async fn env_with_proof_signer(
        &self,
        proof_signer_key: &str,
    ) -> Result<Vec<(&'static str, String)>> {
        self.env_for_options(NodeEnvOptions {
            poster_key: Some(ANVIL_KEY),
            proof_signer_key: Some(proof_signer_key),
            rollup_id: self.dep.rollup_id,
            expect_external_batches: false,
            sequencer_rpc: None,
        })
        .await
    }

    async fn env_for_options(
        &self,
        opts: NodeEnvOptions<'_>,
    ) -> Result<Vec<(&'static str, String)>> {
        let mut env = vec![
            (
                "EEZ_L1_CHAIN_CONFIG",
                self.l2_genesis
                    .1
                    .path()
                    .join("l1-chain-config.json")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("EEZ_L1_CHAIN_PATH", "dev".to_string()),
            ("EEZ_L1_RPC_URL", self.anvil.rpc_url.clone()),
            ("EEZ_L1_BEACON_URL", self.beacon.endpoint().to_string()),
            ("EEZ_L1_BLOB_SIDECAR_ENCODING", "eip7594".to_string()),
            ("EEZ_L1_CHAIN_ID", DEV_CHAIN_ID.to_string()),
            ("EEZ_L1_CHAIN", "testing".to_string()),
            ("EEZL2_ADDRESS", format!("{EEZL2_ADDRESS:#x}")),
            (
                "EEZ_L1_BLOCK_TIME_MS",
                (L1_BLOCK_TIME_SECS * 1000).to_string(),
            ),
            ("EEZ_L2_BLOCK_TIME_MS", "2000".to_string()),
            ("EEZ_SETTLEMENT_START_AFTER_BLOCK", "1".to_string()),
            ("EEZ_PROOF_TIME_MS", "1000".to_string()),
            ("EEZ_SUBMISSION_SLACK_MS", "100".to_string()),
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".to_string()),
            ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "86400000".to_string()),
            (
                "EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS",
                "1000000".to_string(),
            ),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "0".to_string()),
            (
                "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS",
                "500000000000".to_string(),
            ),
            (
                "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS",
                "500000000000".to_string(),
            ),
            (
                "EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE",
                "100000000000".to_string(),
            ),
            (
                "EEZ_REGISTRY_ADDRESS",
                format!("{:#x}", self.dep.eez_address),
            ),
            (
                "EEZ_REGISTRY_DEPLOY_BLOCK",
                self.dep.deploy_block.to_string(),
            ),
            (
                "EEZ_ECDSA_PROOF_SYSTEM_ADDRESS",
                format!("{:#x}", self.dep.proof_system_address),
            ),
            (
                "EEZ_ROLLUP_MANAGER_ADDRESS",
                format!("{:#x}", self.dep.rollup_manager_address),
            ),
            ("EEZ_ROLLUP_ID", opts.rollup_id.to_string()),
            (
                "EEZ_COMPOSER_EXPECT_EXTERNAL_BATCHES",
                opts.expect_external_batches.to_string(),
            ),
            (
                "EEZ_L2_DATADIR",
                "/tmp/unused-overridden-by-flag".to_string(),
            ),
            (
                "RUST_LOG",
                std::env::var("EEZ_TEST_LOG").unwrap_or_else(|_| "warn".to_string()),
            ),
            (
                TEST_L2_GENESIS_ENV,
                self.l2_genesis.0.to_string_lossy().into_owned(),
            ),
        ];

        if let Some(poster_key) = opts.poster_key {
            env.extend([
                ("EEZ_L1_TARGET_RPC_URL", self.anvil.rpc_url.clone()),
                ("EEZ_L1_BUILDER_RPC_URL", self.stub.url.clone()),
                ("EEZ_L1_POSTER_KEY", poster_key.to_string()),
            ]);
        }

        // Composer tests use the real remote-prover path. Followers omit it.
        if let Some(signer_key) = opts.proof_signer_key {
            let genesis = self.l2_genesis_path();
            let attester = signer_address(signer_key)?;
            let signer = ProofSignerHandle::spawn(&ProofSignerConfig {
                l1_rpc_url: self.anvil.rpc_url.clone(),
                registry_address: self.dep.eez_address,
                l1_chain_config: None,
                chain_config: genesis,
                rollup_id: opts.rollup_id,
                signer_key,
                // Match the composer's locally derived vkey. In the
                // unauthorized-signer case both roles deliberately agree on
                // the wrong authority, so L1's real registry rejects the proof.
                vkey: attester.into_word(),
                proof_system: self.dep.proof_system_address,
                max_request_blocks: None,
            })
            .await?;
            let witness_dir = tempfile::tempdir().context("witness DB tempdir")?;
            env.extend([
                ("EEZ_PROVER_URL", signer.endpoint().to_owned()),
                ("EEZ_ATTESTER_ADDRESS", format!("{attester:#x}")),
                (
                    "EEZ_WITNESS_DB_PATH",
                    witness_dir.path().to_string_lossy().into_owned(),
                ),
            ]);
            self.provers
                .lock()
                .map_err(|_| anyhow!("prover registry poisoned"))?
                .push((signer, witness_dir));
        }
        if let Some(sequencer_rpc) = opts.sequencer_rpc {
            env.push(("EEZ_SEQUENCER_RPC", sequencer_rpc.to_string()));
        }
        Ok(env)
    }
}

struct NodeEnvOptions<'a> {
    poster_key: Option<&'a str>,
    proof_signer_key: Option<&'a str>,
    rollup_id: u64,
    expect_external_batches: bool,
    sequencer_rpc: Option<&'a str>,
}

pub struct Deployment {
    pub eez_address: Address,
    pub deploy_block: u64,
    pub proof_system_address: Address,
    pub rollup_manager_address: Address,
    pub rollup_id: u64,
}

sol! {
    #[sol(rpc)]
    interface IEEZ {
        error InvalidProof();
        error InvalidProofSystemConfig();
        event BatchPosted(bytes32 sharedPublicInput, uint64[] rollupIds);
        event L2ExecutionPerformed(uint64 indexed rollupId, bytes32 newRoot, uint256 etherBalance);
        event L2TxSkipped(uint256 indexed transientIdx, bytes revertData);
        function rollups(uint64 rollupId) external view returns (address rollupContract, bytes32 root, uint256 etherBalance);
        function rollupCounter() external view returns (uint256);
        function registerRollup(address rollupContract, bytes32 initialRoot) external returns (uint64 rollupId);
        function executeCrossChainCall(address sourceAddress, bytes calldata callData) external payable returns (bytes memory result);
    }

    interface IRollupInit {
        function initialize(address initialOwner, uint256 initialThreshold, address[] proofSystems, bytes32[] vkeys) external;
    }
}

pub const INVALID_PROOF_SELECTOR: [u8; 4] = IEEZ::InvalidProof::SELECTOR;
pub const INVALID_PROOF_SYSTEM_CONFIG_SELECTOR: [u8; 4] = IEEZ::InvalidProofSystemConfig::SELECTOR;

/// Wall-clock seconds for stamping test genesis + anvil, so the sequencer's
/// defer-on-lateness gate doesn't read every trigger as late.
fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_secs()
}

/// The shared L2 genesis fixture used by every harness mode.
pub fn l2_genesis_fixture_path() -> PathBuf {
    repo_root().join("crates/eez-node-e2e/tests/fixtures/genesis.json")
}

fn fixture_genesis() -> Result<alloy_genesis::Genesis> {
    let raw = std::fs::read_to_string(l2_genesis_fixture_path()).context("read genesis.json")?;
    serde_json::from_str(&raw).context("parse genesis.json")
}

/// Canonical genesis header hash for the exact JSON passed to the L2 node.
pub fn genesis_block_hash(path: &std::path::Path) -> Result<B256> {
    let raw = std::fs::read_to_string(path).context("read L2 genesis")?;
    let genesis: alloy_genesis::Genesis = serde_json::from_str(&raw).context("parse L2 genesis")?;
    let spec: reth_chainspec::ChainSpec = genesis.into();
    Ok(spec.genesis_header().hash_slow())
}

/// Sign and submit one EIP-1559 L2 value transfer using the pool nonce.
pub async fn send_l2_value_transfer(
    rpc_url: &str,
    signing_key: &str,
    to: Address,
    value: U256,
) -> Result<alloy_primitives::TxHash> {
    let signer: PrivateKeySigner = signing_key
        .strip_prefix("0x")
        .unwrap_or(signing_key)
        .parse()
        .context("parse signing key")?;
    let from = signer.address();

    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let chain_id = provider.get_chain_id().await?;
    let nonce = provider.get_transaction_count(from).pending().await?;
    let fees = provider.estimate_eip1559_fees().await?;
    let mut tx = TxEip1559 {
        chain_id,
        nonce,
        gas_limit: 21_000,
        max_fee_per_gas: fees.max_fee_per_gas,
        max_priority_fee_per_gas: fees.max_priority_fee_per_gas,
        to: alloy_primitives::TxKind::Call(to),
        value,
        access_list: alloy_rpc_types_eth::AccessList::default(),
        input: alloy_primitives::Bytes::default(),
    };
    let sig = signer.sign_transaction_sync(&mut tx)?;
    let envelope = TxEnvelope::from(tx.into_signed(sig));
    let raw = envelope.encoded_2718();

    let hash = provider
        .send_raw_transaction(&raw)
        .await?
        .tx_hash()
        .to_owned();
    Ok(hash)
}

/// Send one L2 value transfer and wait for its receipt. Use this when a test
/// needs to prove a later L1 attestation includes a fresh L2 state transition.
pub async fn send_l2_value_transfer_confirmed(
    rpc_url: &str,
    signing_key: &str,
    to: Address,
    value: U256,
    timeout: Duration,
) -> Result<alloy_primitives::TxHash> {
    let hash = send_l2_value_transfer(rpc_url, signing_key, to, value).await?;
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    wait_for(timeout, || async {
        let Some(receipt) = provider.get_transaction_receipt(hash).await? else {
            return Ok(None);
        };
        if !receipt.status() {
            bail!("L2 value transfer reverted: {hash}");
        }
        Ok(Some(()))
    })
    .await?;
    Ok(hash)
}

/// Read the hash of the latest `safe` block on the L2 at `rpc_url`.
/// Returns `Ok(None)` while the L2 hasn't yet adopted any safe block
/// (genesis L1 derivation pending). Used by the multi-composer reorg
/// test to verify both composers settle on the same canonical L2 head.
pub async fn safe_block_hash(rpc_url: &str) -> Result<Option<B256>> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let block = provider
        .get_block_by_number(alloy_rpc_types_eth::BlockNumberOrTag::Safe)
        .await?;
    Ok(block.map(|b| b.header.hash))
}

/// Block number and hash at a named tag (`latest`, `safe`, `finalized`, …).
/// `None` when no block exists at that tag yet.
pub async fn block_number_and_hash_at(
    rpc_url: &str,
    tag: alloy_rpc_types_eth::BlockNumberOrTag,
) -> Result<Option<(u64, B256)>> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let block = provider.get_block_by_number(tag).await?;
    Ok(block.map(|b| (b.header.number, b.header.hash)))
}

/// Deploy and register the protocol with an explicit initial L2 state.
async fn deploy_contracts_with_initial(
    rpc_url: &str,
    key: &str,
    initial_state: B256,
) -> Result<Deployment> {
    // Anvil signs transactions from its default accounts.
    let signer: PrivateKeySigner = key
        .strip_prefix("0x")
        .unwrap_or(key)
        .parse()
        .context("parse signer key")?;
    let signer_addr = signer.address();
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);

    let out = repo_root().join("contracts/out");

    // EEZ sits behind an EEZProxy, as in DeployEEZ.s.sol. EEZ has no
    // initializer; the proxy address is the registry.
    let eez_implementation = deploy(
        &provider,
        signer_addr,
        &out.join("EEZ.sol/EEZ.json"),
        signer_addr.abi_encode(),
    )
    .await?;
    let eez_address = deploy(
        &provider,
        signer_addr,
        &out.join("EEZProxy.sol/EEZProxy.json"),
        (eez_implementation, signer_addr).abi_encode_params(),
    )
    .await?;
    let deploy_block = provider.get_block_number().await?;

    // Keep the authorized attester separate from the poster/deployer key.
    let attester = signer_address(ANVIL_ATTESTER_KEY)?;
    let proof_system_address = deploy(
        &provider,
        signer_addr,
        &out.join("ECDSAProofSystem.sol/ECDSAProofSystem.json"),
        attester.abi_encode(),
    )
    .await?;

    // Rollup is upgradeable, as in DeployRollup.s.sol: the implementation
    // (constructor `Rollup(address eez)`) locks itself, and a transparent proxy
    // runs `initialize(owner, threshold, proofSystems, vkeys)` at construction.
    // The proxy is the rollup manager; the deployer also owns upgrades.
    let proof_systems: Vec<Address> = vec![proof_system_address];
    // vkey embeds the authorized signer address; the registry treats vkey as
    // opaque but checks non-zero + membership (see DeployRollup.s.sol).
    let vkeys: Vec<B256> = vec![attester.into_word()];
    let rollup_implementation = deploy(
        &provider,
        signer_addr,
        &out.join("Rollup.sol/Rollup.json"),
        eez_address.abi_encode(),
    )
    .await?;
    let initialize = IRollupInit::initializeCall {
        initialOwner: signer_addr,
        initialThreshold: U256::from(1u64),
        proofSystems: proof_systems,
        vkeys,
    }
    .abi_encode();
    let rollup_manager_address = deploy(
        &provider,
        signer_addr,
        &out.join("TransparentUpgradeableProxy.sol/TransparentUpgradeableProxy.json"),
        (
            rollup_implementation,
            signer_addr,
            alloy_primitives::Bytes::from(initialize),
        )
            .abi_encode_params(),
    )
    .await?;

    // registerRollup via a plain eth_sendTransaction (anvil signs for default
    // accounts); avoids the alloy wallet-filler requirement on `to`.
    let calldata = IEEZ::registerRollupCall {
        rollupContract: rollup_manager_address,
        initialRoot: initial_state,
    }
    .abi_encode();
    let receipt = provider
        .send_transaction(
            TransactionRequest::default()
                .from(signer_addr)
                .to(eez_address)
                .input(calldata.into()),
        )
        .await?
        .get_receipt()
        .await?;
    if !receipt.status() {
        bail!("registerRollup tx reverted");
    }
    let registry = IEEZ::new(eez_address, &provider);
    let rollup_id = registry.rollupCounter().call().await?.try_into()?;

    Ok(Deployment {
        eez_address,
        deploy_block,
        proof_system_address,
        rollup_manager_address,
        rollup_id,
    })
}

async fn deploy<P: Provider>(
    provider: &P,
    from: Address,
    artifact_path: &std::path::Path,
    constructor_args: Vec<u8>,
) -> Result<Address> {
    let artifact: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(artifact_path)
            .with_context(|| format!("read {}", artifact_path.display()))?,
    )?;
    let bytecode_hex = artifact["bytecode"]["object"]
        .as_str()
        .ok_or_else(|| anyhow!("bytecode.object not found in {}", artifact_path.display()))?
        .strip_prefix("0x")
        .unwrap_or_default();
    let mut deploy_data = hex::decode(bytecode_hex).context("decode bytecode hex")?;
    deploy_data.extend_from_slice(&constructor_args);
    let receipt = provider
        .send_transaction(
            TransactionRequest::default()
                .from(from)
                .input(deploy_data.into()),
        )
        .await?
        .get_receipt()
        .await?;
    if !receipt.status() {
        bail!("deploy of {} reverted", artifact_path.display());
    }
    receipt.contract_address.ok_or_else(|| {
        anyhow!(
            "no contract_address in receipt for {}",
            artifact_path.display()
        )
    })
}

pub struct NodeHandle {
    child: Mutex<Child>,
    background_tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Human label for assertion messages ("c1", "c2", or the default "node").
    pub name: String,
    /// Where the node's merged stdout+stderr is written. Goes to
    /// `EEZ_TEST_LOG_DIR/eez-node-<pid>.log` if that env var is set,
    /// otherwise inside a tempdir held by this handle.
    pub log_path: PathBuf,
    /// Tempdirs whose lifetime is tied to this handle (currently the log dir).
    _keep_alive: Vec<tempfile::TempDir>,
    /// Owned node database. CI retains it only if this test unwinds in failure.
    owned_datadir: Option<FailureDatadir>,
    pub http_port: u16,
    l2_composer_url: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum NodeBinary {
    #[default]
    Composer,
    Follower,
}

impl NodeBinary {
    const fn name(self) -> &'static str {
        match self {
            Self::Composer => "eez-composer",
            Self::Follower => "eez-follower",
        }
    }

    fn path(self) -> Result<PathBuf> {
        let name = self.name();
        // The CI archive also contains Cargo's test-feature-unified binaries.
        // Composer/follower must instead use the frozen shipping candidate.
        {
            if let Some(directory) = std::env::var_os("EEZ_TEST_RUNTIME_BIN_DIR") {
                let path = PathBuf::from(directory).join(name);
                ensure!(
                    path.is_file(),
                    "missing release candidate {}",
                    path.display()
                );
                return Ok(path);
            }
            ensure!(
                std::env::var_os("EEZ_TEST_PREBUILT").is_none(),
                "prebuilt tests require EEZ_TEST_RUNTIME_BIN_DIR"
            );
        }
        let nextest_key = format!("NEXTEST_BIN_EXE_{}", name.replace('-', "_"));
        if let Some(path) = std::env::var_os(&nextest_key) {
            return Ok(path.into());
        }
        ensure!(
            std::env::var_os("EEZ_TEST_PREBUILT").is_none(),
            "missing {nextest_key} in prebuilt test"
        );
        if let Some(path) = std::env::var_os(format!("CARGO_BIN_EXE_{name}")) {
            return Ok(path.into());
        }

        // `eez-testkit` is compiled as a library, where Cargo does not expose
        // `CARGO_BIN_EXE_*` to `env!`. At test runtime the executable sits in
        // `<target>/<profile>/deps`, alongside the role binaries' parent dir.
        let current = std::env::current_exe().context("current test executable")?;
        let target_profile = current
            .parent()
            .and_then(std::path::Path::parent)
            .ok_or_else(|| anyhow!("test executable has no target profile directory"))?;
        let path = target_profile.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        if path.is_file() {
            return Ok(path);
        }
        bail!(
            "{name} binary not found next to the test profile at {}; build the role binaries before running the harness",
            target_profile.display(),
        )
    }
}

#[derive(Default)]
pub struct NodeConfig<'a> {
    /// Explicit node-role executable to launch.
    pub binary: NodeBinary,
    pub genesis_path: Option<&'a std::path::Path>,
}

impl NodeHandle {
    /// Stall this fixture's process across a scheduler deadline while leaving
    /// its separate signer alive. Dropping the handle still kills the child.
    pub async fn pause_for(&self, duration: Duration) -> Result<()> {
        let pid = self.child.lock().expect("node child mutex poisoned").id();
        let signal = |name: &str| -> Result<()> {
            ensure!(
                Command::new("kill")
                    .args([name, &pid.to_string()])
                    .status()
                    .context("signal fixture process")?
                    .success(),
                "failed to signal fixture process {pid} with {name}"
            );
            Ok(())
        };
        signal("-STOP")?;
        tokio::time::sleep(duration).await;
        signal("-CONT")
    }

    fn spawn_with(
        name: &str,
        datadir: &std::path::Path,
        cfg: &NodeConfig<'_>,
        env: &[(&'static str, String)],
    ) -> Result<Self> {
        Self::spawn_with_reservations(name, datadir, cfg, env, Vec::new())
    }

    fn spawn_with_reservations(
        name: &str,
        datadir: &std::path::Path,
        cfg: &NodeConfig<'_>,
        env: &[(&'static str, String)],
        mut port_leases: Vec<PortLease>,
    ) -> Result<Self> {
        let (log_path, log_tempdir) = test_log_destination(&format!("eez-node-{name}"))?;
        let f = std::fs::File::create(&log_path).context("create log file")?;
        let f2 = f.try_clone().context("clone log file")?;
        let (stdout, stderr) = (Stdio::from(f), Stdio::from(f2));
        let authrpc_lease = PortLease::tcp();
        let http_lease = PortLease::tcp();
        let ws_lease = PortLease::tcp();
        let p2p_lease = PortLease::tcp();
        let l1_http_lease = PortLease::http_pair();
        let l1_auth_lease = PortLease::tcp();
        // Embedded L1 uses this numeric port for RLPx TCP and discovery UDP.
        let l1_p2p_lease = PortLease::tcp_udp();
        let l1_discv5_lease = PortLease::udp();
        let l1_xchain_lease = PortLease::tcp();
        let l2_xchain_lease = PortLease::tcp();
        let authrpc_port = authrpc_lease.port();
        let http_port = http_lease.port();
        let ws_port = ws_lease.port();
        let p2p_port = p2p_lease.port();
        let l1_http_port = l1_http_lease.port();
        let l1_auth_port = l1_auth_lease.port();
        let l1_p2p_port = l1_p2p_lease.port();
        let l1_discv5_port = l1_discv5_lease.port();
        let l1_xchain_port = l1_xchain_lease.port();
        let l2_xchain_port = l2_xchain_lease.port();
        port_leases.extend([
            authrpc_lease,
            http_lease,
            ws_lease,
            p2p_lease,
            l1_http_lease,
            l1_auth_lease,
            l1_p2p_lease,
            l1_discv5_lease,
            l1_xchain_lease,
            l2_xchain_lease,
        ]);
        let l1_datadir = datadir.join("embedded-l1");
        let env_genesis = env
            .iter()
            .find(|(k, _)| *k == TEST_L2_GENESIS_ENV)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
            .map(std::ffi::OsString::from);
        let chain_arg: std::ffi::OsString = cfg
            .genesis_path
            .map(|p| p.as_os_str().to_owned())
            .or(env_genesis)
            .unwrap_or_else(|| std::ffi::OsString::from("dev"));
        let mut cmd = Command::new(cfg.binary.path()?);
        // env_clear alone does not stop dotenv searching parent directories.
        isolate_dotenv(datadir)?;
        let signal_filter = std::env::var("EEZ_TEST_LOG").unwrap_or_else(|_| {
            "warn,eez_node=info,eez_l1=info,eez_composer=info,eez_deriver=info,eez_posting=info,eez_driver=info,eez_driver::slot=debug"
                .to_string()
        });
        // Each role binary loads dotenv files from its working directory. Running from
        // the datadir prevents repository settings from changing the explicit
        // test configuration or redirecting L1 traffic to a developer endpoint.
        cmd.arg("node");
        // Pool inspection supports rejected-transaction diagnostics on the
        // loopback fixture. Execution-witness capture remains opt-in; shipping
        // node defaults are untouched.
        match env
            .iter()
            .rev()
            .find(|(name, _)| *name == "EEZ_TEST_L2_DEBUG_RPC")
        {
            Some((_, value)) if value == "true" => {
                cmd.args(["--http.api", "eth,net,web3,txpool,debug"]);
            }
            Some((_, value)) if value == "false" => {
                cmd.args(["--http.api", "eth,net,web3,txpool"]);
            }
            Some(_) => bail!("EEZ_TEST_L2_DEBUG_RPC must be true or false"),
            None => {
                cmd.args(["--http.api", "eth,net,web3,txpool"]);
            }
        }

        cmd.current_dir(datadir)
            .arg("--chain")
            .arg(&chain_arg)
            .arg("--datadir")
            .arg(datadir)
            .stdout(stdout)
            .args([
                "--log.stdout.format",
                "json",
                "--log.stdout.filter",
                &signal_filter,
                "--log.file.max-files",
                "0",
                "--http",
                "--http.addr",
                "127.0.0.1",
                "--http.port",
                &http_port.to_string(),
                "--ws.port",
                &ws_port.to_string(),
                "--authrpc.port",
                &authrpc_port.to_string(),
                "--port",
                &p2p_port.to_string(),
                "--disable-discovery",
                "--ipcdisable",
            ])
            .stderr(stderr)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", std::env::var("HOME").unwrap_or_default())
            .env("NO_COLOR", "1")
            .env(
                "RUSTUP_HOME",
                std::env::var("RUSTUP_HOME").unwrap_or_default(),
            )
            .env(
                "CARGO_HOME",
                std::env::var("CARGO_HOME").unwrap_or_default(),
            );
        cmd.env("EEZ_L1_HTTP_PORT", l1_http_port.to_string())
            .env("EEZ_L1_AUTH_PORT", l1_auth_port.to_string())
            .env("EEZ_L1_P2P_PORT", l1_p2p_port.to_string())
            .env("EEZ_L1_DISCV5_PORT", l1_discv5_port.to_string())
            .env("EEZ_L1_XCHAIN_PORT", l1_xchain_port.to_string())
            .env("EEZ_L2_XCHAIN_PORT", l2_xchain_port.to_string())
            .env("EEZ_L1_DATADIR", &l1_datadir)
            // May be overridden below when a test uses another L2 upstream.
            .env("EEZ_L2_RPC_URL", format!("http://127.0.0.1:{http_port}"));
        for (k, v) in env {
            if *k == TEST_L2_GENESIS_ENV
                || *k == "EEZ_TEST_L2_GENESIS_AGE_SECONDS"
                || *k == REJECTING_BUILDER_ENV
                || *k == POST_BATCHER_ENV
                || *k == SIGNER_MAX_REQUEST_BLOCKS_ENV
            {
                continue; // test-only marker (consumed as --chain above)
            }
            cmd.env(*k, v);
        }
        // Keep the checkpoint path aligned with the explicit reth datadir.
        // Test environments historically carried a placeholder value because
        // only the CLI flag mattered before persistent reconciliation existed.
        cmd.env("EEZ_L2_DATADIR", datadir);
        // Keep every selected port bound until the child command is complete.
        // The subprocess bind itself cannot be atomic, but this leaves only the
        // unavoidable drop/spawn boundary exposed to another process.
        drop(port_leases);
        let child = cmd.spawn().context("spawn eez node role")?;
        Ok(Self {
            child: Mutex::new(child),
            background_tasks: Mutex::new(Vec::new()),
            name: name.to_string(),
            log_path,
            _keep_alive: log_tempdir.into_iter().collect(),
            owned_datadir: None,
            http_port,
            l2_composer_url: format!(
                "http://127.0.0.1:{}",
                env.iter()
                    .rev()
                    .find(|(name, _)| *name == "EEZ_L2_XCHAIN_PORT")
                    .map(|(_, port)| port.clone())
                    .unwrap_or_else(|| l2_xchain_port.to_string())
            ),
        })
    }

    pub async fn start(
        name: &str,
        cfg: &NodeConfig<'_>,
        env: &[(&'static str, String)],
    ) -> Result<Self> {
        let mut datadir = FailureDatadir::new(&format!("eez-node-{name}"))?;
        let mut handle = Self::start_with_datadir(name, datadir.path(), cfg, env).await?;
        datadir.fixture_ready();
        handle.owned_datadir = Some(datadir);
        Ok(handle)
    }

    pub async fn start_with_datadir(
        name: &str,
        datadir: &std::path::Path,
        cfg: &NodeConfig<'_>,
        env: &[(&'static str, String)],
    ) -> Result<Self> {
        let handle = Self::spawn_with(name, datadir, cfg, env)?;
        handle
            .wait_for_rpc(&handle.l2_rpc_url(), Duration::from_mins(3), "L2 RPC")
            .await?;
        Ok(handle)
    }

    pub fn l2_rpc_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.http_port)
    }

    /// Composer-only observations belong on the front, not the node RPC.
    pub fn composer_rpc_url(&self) -> &str {
        &self.l2_composer_url
    }

    /// Waits for RPC readiness while surfacing early process exits and logs.
    pub async fn wait_for_rpc(&self, rpc_url: &str, timeout: Duration, label: &str) -> Result<()> {
        let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
        let start = Instant::now();
        while start.elapsed() < timeout {
            let probe = tokio::time::timeout(Duration::from_secs(1), provider.get_block_number());
            if matches!(probe.await, Ok(Ok(_))) {
                return Ok(());
            }
            if let Some(status) = self.exit_status() {
                bail!(
                    "{} exited with {status} before {label} became ready; log:\n{}",
                    self.name,
                    self.log_tail(80),
                );
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        bail!(
            "{} {label} at {rpc_url} did not become ready within {timeout:?}; log:\n{}",
            self.name,
            self.log_tail(80),
        )
    }

    /// Starts managed background traffic; the task is aborted with the node.
    pub fn run_tx_spammer(&self, from_key: &'static str) {
        let url = self.l2_rpc_url();
        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(TX_SPAM_INTERVAL).await;
                let _ =
                    send_l2_value_transfer(&url, from_key, ANVIL_ADDR_3, U256::from(1u64)).await;
            }
        });
        self.background_tasks
            .lock()
            .expect("node background task mutex poisoned")
            .push(task);
    }

    // Convergence alone can pass without exercising the reorg path.
    pub async fn wait_for_reorg_seen(&self, timeout: Duration) -> Result<()> {
        wait_for(timeout, || async {
            Ok((self.count_signals(&[
                signals::L1_REORG_DETECTED,
                signals::DERIVER_REORG_RETREATED,
                signals::DERIVER_REORG_NOOP,
            ])? > 0)
                .then_some(()))
        })
        .await
        .with_context(|| format!("{} deriver missed the reorg", self.name))
    }

    pub fn assert_no_process_death(&self) {
        if let Some(status) = self.exit_status() {
            panic!(
                "{} exited unexpectedly with {status}; log:\n{}",
                self.name,
                self.log_tail(80),
            );
        }
        assert_eq!(
            self.count_signals(FATAL_SIGNALS).unwrap(),
            0,
            "{} emitted a panic, divergence, or fatal-lifecycle signal",
            self.name,
        );
        assert_eq!(
            self.log_count_matching(&["Fatal", "UnexpectedStaticFile"])
                .unwrap(),
            0,
            "{} logged a fatal-class failure",
            self.name,
        );
    }

    pub fn assert_no_divergence_failure_logs(&self) {
        self.assert_no_process_death();
        assert_eq!(
            self.log_count_matching(&[
                "engine rejected safe/finalized FCU",
                "payload builder returned no payload",
            ])
            .unwrap(),
            0,
            "{} logged a fatal-class failure",
            self.name,
        );
    }

    /// Report a terminal node failure that makes further test progress
    /// impossible. Long-running waits use this to surface the actual failure
    /// instead of eventually reporting an unrelated timeout.
    pub fn progress_failure(&self) -> Result<Option<String>> {
        if let Some(status) = self.exit_status() {
            return Ok(Some(format!(
                "{} exited unexpectedly with {status}; log:\n{}",
                self.name,
                self.log_tail(80),
            )));
        }

        if self.count_signals(FATAL_SIGNALS)? > 0 {
            return Ok(Some(format!(
                "{} emitted a panic, divergence, or fatal-lifecycle signal:\n{}",
                self.name,
                self.log_lines_matching(FATAL_SIGNALS, 20),
            )));
        }

        let fatal_log_markers = [
            "engine rejected safe/finalized FCU",
            "payload builder returned no payload",
            "Fatal",
            "UnexpectedStaticFile",
        ];
        if self.log_count_matching(&fatal_log_markers)? > 0 {
            return Ok(Some(format!(
                "{} logged a fatal-class failure:\n{}",
                self.name,
                self.log_lines_matching(&fatal_log_markers, 20),
            )));
        }

        Ok(None)
    }

    pub fn log_count_matching(&self, patterns: &[&str]) -> Result<usize> {
        let contents = std::fs::read_to_string(&self.log_path)
            .with_context(|| format!("read node log {}", self.log_path.display()))?;
        Ok(contents
            .lines()
            .filter(|line| json_line_matches(line, patterns))
            .count())
    }

    /// Decode the node's JSON tracing stream and count a stable signal field.
    /// Human messages may change without invalidating assertions.
    pub fn count_signal(&self, signal: &str) -> Result<usize> {
        self.count_signals(&[signal])
    }

    pub fn count_signals(&self, signals: &[&str]) -> Result<usize> {
        let contents = std::fs::read_to_string(&self.log_path)
            .with_context(|| format!("read signal stream {}", self.log_path.display()))?;
        Ok(contents
            .lines()
            .filter_map(signals::parse)
            .filter(|signal| signals.contains(&signal.name.as_str()))
            .count())
    }

    pub fn log_lines_matching(&self, patterns: &[&str], max: usize) -> String {
        last_lines(&self.log_path, max, Some(patterns))
    }

    pub fn log_tail(&self, max: usize) -> String {
        last_lines(&self.log_path, max, None)
    }

    pub fn exit_status(&self) -> Option<std::process::ExitStatus> {
        self.child
            .lock()
            .expect("node child mutex poisoned")
            .try_wait()
            .ok()
            .flatten()
    }

    /// Line cursor for a subsequent [`Self::signals_since`] query.
    pub fn signal_cursor(&self) -> Result<usize> {
        let contents = std::fs::read_to_string(&self.log_path)
            .with_context(|| format!("read signal stream {}", self.log_path.display()))?;
        // Do not advance past a record whose write is still in progress.
        Ok(contents.bytes().filter(|byte| *byte == b'\n').count())
    }

    /// Structured records emitted after `cursor`. Signal values are exact
    /// tracing event names, while human-readable messages remain irrelevant.
    pub fn signals_since(&self, cursor: usize) -> Result<Vec<NodeSignal>> {
        let contents = std::fs::read_to_string(&self.log_path)
            .with_context(|| format!("read signal stream {}", self.log_path.display()))?;
        Ok(contents
            .lines()
            .skip(cursor)
            .filter_map(signals::parse)
            .collect())
    }
}

fn last_lines(path: &std::path::Path, max: usize, patterns: Option<&[&str]>) -> String {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return format!("<unreadable log {}>", path.display());
    };
    let selected: Vec<&str> = contents
        .lines()
        .filter(|line| patterns.is_none_or(|patterns| json_line_matches(line, patterns)))
        .collect();
    let skip = selected.len().saturating_sub(max);
    selected[skip..]
        .iter()
        .map(|line| {
            // A single record can contain a whole blob or transaction array.
            // Keep failure output usable even when artifact uploads fail.
            const MAX_CHARS: usize = 4096;
            let mut chars = line.chars();
            let mut bounded: String = chars.by_ref().take(MAX_CHARS).collect();
            if chars.next().is_some() {
                bounded.push_str(" … [record truncated]");
            }
            bounded
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn json_line_matches(line: &str, patterns: &[&str]) -> bool {
    serde_json::from_str::<serde_json::Value>(line).is_ok()
        && patterns.iter().any(|pattern| line.contains(pattern))
}

// These diagnostic filters inspect message fields inside JSON records. Test
// assertions use stable event names wherever the producer exposes one.
const SETTLEMENT_LOG_MARKERS: &[&str] = &[
    "remote prover attested the window",
    "dispatching bundle to builder",
    "eth_sendBundle response received",
    "bundle dropped",
    "compose",
    "postBatch",
    "postAndVerifyBatch",
    "InvalidProof",
    "prover",
    "attest",
    "revert",
    "ERROR",
    "WARN",
];

impl Drop for NodeHandle {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "\n== {} node log tail ==\n{}",
                self.name,
                self.log_tail(100)
            );
        }
        let tasks = self
            .background_tasks
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for task in tasks.drain(..) {
            task.abort();
        }
        let child = self
            .child
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = child.kill();
        let _ = child.wait();
    }
}

async fn l2_block_by_tag(rpc_url: &str, tag: BlockNumberOrTag) -> Result<Option<BlockNumHash>> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let block = provider.get_block_by_number(tag).await?;
    Ok(block.map(|b| BlockNumHash::new(b.header.number, b.header.hash)))
}

async fn l2_block_by_number(rpc_url: &str, number: u64) -> Result<Option<BlockNumHash>> {
    l2_block_by_tag(rpc_url, BlockNumberOrTag::Number(number)).await
}

async fn latest_block_snapshot(node: &NodeHandle) -> Result<Option<BlockNumHash>> {
    l2_block_by_tag(&node.l2_rpc_url(), BlockNumberOrTag::Latest).await
}

pub async fn wait_for_latest_height(
    node: &NodeHandle,
    min_height: u64,
    timeout: Duration,
) -> Result<BlockNumHash> {
    wait_for(timeout, || async {
        let latest = latest_block_snapshot(node).await?;
        Ok(latest.filter(|b| b.number >= min_height))
    })
    .await
}

/// Waits for nodes to agree at a safe height at or above `min_height`.
/// The lower bound prevents stale agreement from satisfying convergence.
pub async fn wait_for_safe_prefix_convergence(
    nodes: &[&NodeHandle],
    min_height: u64,
    timeout: Duration,
) -> Result<BlockNumHash> {
    wait_for_tag_prefix_convergence(nodes, BlockNumberOrTag::Safe, min_height, timeout).await
}

async fn wait_for_tag_prefix_convergence(
    nodes: &[&NodeHandle],
    tag: BlockNumberOrTag,
    min_height: u64,
    timeout: Duration,
) -> Result<BlockNumHash> {
    let result = wait_for(timeout, || async {
        let mut tag_blocks = Vec::with_capacity(nodes.len());
        for node in nodes {
            let Some(block) = l2_block_by_tag(&node.l2_rpc_url(), tag).await? else {
                return Ok(None);
            };
            tag_blocks.push(block);
        }

        let target = tag_blocks
            .iter()
            .map(|b| b.number)
            .min()
            .unwrap_or_default();
        if target < min_height {
            return Ok(None);
        }

        let mut blocks = Vec::with_capacity(nodes.len());
        for node in nodes {
            let Some(block) = l2_block_by_number(&node.l2_rpc_url(), target).await? else {
                return Ok(None);
            };
            blocks.push(block);
        }
        let first = blocks[0];
        Ok(blocks.iter().all(|b| b.hash == first.hash).then_some(first))
    })
    .await;
    let err = match result {
        Ok(block) => return Ok(block),
        Err(err) => err,
    };

    let mut diagnostics = String::new();
    let mut diagnostic_height = None;
    for node in nodes {
        if let Ok(Some(block)) = l2_block_by_tag(&node.l2_rpc_url(), tag).await {
            diagnostic_height = Some(
                diagnostic_height.map_or(block.number, |height: u64| height.min(block.number)),
            );
        }
    }
    for node in nodes {
        let latest = l2_block_by_tag(&node.l2_rpc_url(), BlockNumberOrTag::Latest)
            .await
            .ok()
            .flatten();
        let tagged = l2_block_by_tag(&node.l2_rpc_url(), tag)
            .await
            .ok()
            .flatten();
        let header = if let Some(height) = diagnostic_height {
            let provider = ProviderBuilder::new().connect_http(node.l2_rpc_url().parse()?);
            provider
                .get_block_by_number(BlockNumberOrTag::Number(height))
                .await
                .ok()
                .flatten()
                .map(|block| block.header)
        } else {
            None
        };
        let _ = write!(
            diagnostics,
            "\n== {} convergence state ==\nexit={:?} latest={latest:?} target_tag={tagged:?} common_header={header:#?}\n{}\n",
            node.name,
            node.exit_status(),
            node.log_lines_matching(
                &[
                    "advanced L2 safe head",
                    "reconcile",
                    "diverged",
                    "reorg",
                    "ERROR",
                    "WARN",
                ],
                40,
            ),
        );
    }
    Err(err.context(format!("nodes did not converge:{diagnostics}")))
}

pub async fn wait_for<F, Fut, T>(timeout: Duration, f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Option<T>>>,
{
    wait_for_with_failure(timeout, || Ok(None), f).await
}

/// Poll through transient errors while checking terminal failures every 500 ms,
/// including while a poll is pending. Keep the same poll alive between checks:
/// a slow healthy RPC must not be cancelled and restarted at every check.
/// Returning an error from the poll alone is retryable, as in [`wait_for`].
pub async fn wait_for_with_failure<F, Fut, T>(
    timeout: Duration,
    mut failure: impl FnMut() -> Result<Option<String>>,
    mut f: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Option<T>>>,
{
    let deadline = Instant::now() + timeout;
    let mut last_error: Option<anyhow::Error> = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        if let Some(reason) = failure()? {
            bail!(reason);
        }
        let poll = tokio::time::timeout(remaining, f());
        tokio::pin!(poll);
        let result = loop {
            tokio::select! {
                result = &mut poll => break result,
                () = tokio::time::sleep(Duration::from_millis(500)) => {
                    if let Some(reason) = failure()? {
                        bail!(reason);
                    }
                }
            }
        };
        match result {
            Ok(Ok(Some(value))) => return Ok(value),
            Ok(Ok(None)) => {}
            Ok(Err(error)) => last_error = Some(error),
            Err(_) => break,
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        tokio::time::sleep(remaining.min(Duration::from_millis(500))).await;
    }
    match last_error {
        Some(error) => Err(error.context(format!("timed out after {timeout:?}; last poll failed"))),
        None => bail!("timed out after {timeout:?}"),
    }
}

/// Registry counters and roots read from one L1 block.
#[derive(Clone, Copy, Debug)]
pub struct ChainSnapshot {
    pub batches_posted: usize,
    pub executions_performed: usize,
    pub entries_skipped: usize,
    pub state_root: B256,
    pub latest_execution_state: Option<B256>,
}

/// Only the root call's error and return data are evidence of the mined revert.
/// Do not match text in error messages, nested calls or ABI arguments.
#[derive(Debug, serde::Deserialize)]
struct MinedRevertTrace {
    error: Option<String>,
    #[serde(default)]
    output: alloy_primitives::Bytes,
}

impl MinedRevertTrace {
    fn assert_selector(&self, expected: [u8; 4]) -> Result<()> {
        if self.error.as_deref() != Some("execution reverted")
            || !self.output.starts_with(&expected)
        {
            bail!(
                "mined postAndVerifyBatch trace returned error {:?}, output {}; expected revert 0x{}",
                self.error,
                self.output,
                hex::encode(expected),
            );
        }
        Ok(())
    }
}

pub struct Chain<'a> {
    rpc_url: &'a str,
    eez_address: Address,
    deploy_block: u64,
    rollup_id: u64,
}

impl<'a> Chain<'a> {
    fn new(anvil: &'a Anvil, dep: &Deployment) -> Self {
        Self {
            rpc_url: &anvil.rpc_url,
            eez_address: dep.eez_address,
            deploy_block: dep.deploy_block,
            rollup_id: dep.rollup_id,
        }
    }

    /// Deployment coordinates for tests that construct their own L1 calls.
    pub const fn rpc_url(&self) -> &str {
        self.rpc_url
    }

    pub const fn eez_address(&self) -> Address {
        self.eez_address
    }

    pub const fn deploy_block(&self) -> u64 {
        self.deploy_block
    }

    pub const fn rollup_id(&self) -> u64 {
        self.rollup_id
    }

    /// Anvil uses `--block-time`, so `evm_setAutomine` does not stop it.
    /// An interval of zero pauses block production; restore it with
    /// [`Self::block_time_secs`].
    pub async fn set_interval_mining(&self, secs: u64) -> Result<()> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let _: serde_json::Value = provider
            .client()
            .request("anvil_setIntervalMining", (secs,))
            .await
            .context("anvil_setIntervalMining")?;
        Ok(())
    }

    /// Mine exactly one L1 block.
    pub async fn mine(&self) -> Result<()> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let _: serde_json::Value = provider
            .client()
            .request("evm_mine", ())
            .await
            .context("evm_mine")?;
        Ok(())
    }

    pub const fn block_time_secs() -> u64 {
        L1_BLOCK_TIME_SECS
    }

    pub async fn batches_posted(&self) -> Result<usize> {
        count_events(
            self.rpc_url,
            self.eez_address,
            IEEZ::BatchPosted::SIGNATURE_HASH,
            self.deploy_block,
        )
        .await
    }

    pub async fn executions_performed(&self) -> Result<usize> {
        count_events(
            self.rpc_url,
            self.eez_address,
            IEEZ::L2ExecutionPerformed::SIGNATURE_HASH,
            self.deploy_block,
        )
        .await
    }

    pub async fn entries_skipped(&self) -> Result<usize> {
        count_events(
            self.rpc_url,
            self.eez_address,
            IEEZ::L2TxSkipped::SIGNATURE_HASH,
            self.deploy_block,
        )
        .await
    }

    pub async fn state_root(&self) -> Result<B256> {
        state_root(self.rpc_url, self.eez_address, self.rollup_id).await
    }

    /// Pin related reads to one L1 block so a batch landing between RPC calls
    /// cannot manufacture a lockstep failure.
    pub async fn snapshot(&self) -> Result<ChainSnapshot> {
        let block = self.block_number().await?;
        Ok(ChainSnapshot {
            batches_posted: count_events_at(
                self.rpc_url,
                self.eez_address,
                IEEZ::BatchPosted::SIGNATURE_HASH,
                self.deploy_block,
                Some(block),
            )
            .await?,
            executions_performed: count_events_at(
                self.rpc_url,
                self.eez_address,
                IEEZ::L2ExecutionPerformed::SIGNATURE_HASH,
                self.deploy_block,
                Some(block),
            )
            .await?,
            entries_skipped: count_events_at(
                self.rpc_url,
                self.eez_address,
                IEEZ::L2TxSkipped::SIGNATURE_HASH,
                self.deploy_block,
                Some(block),
            )
            .await?,
            state_root: state_root_at(self.rpc_url, self.eez_address, self.rollup_id, Some(block))
                .await?,
            latest_execution_state: latest_l2_execution_state_at(
                self.rpc_url,
                self.eez_address,
                self.rollup_id,
                self.deploy_block,
                Some(block),
            )
            .await?,
        })
    }

    pub async fn executed_states(&self) -> Result<Vec<B256>> {
        all_l2_execution_states(
            self.rpc_url,
            self.eez_address,
            self.rollup_id,
            self.deploy_block,
        )
        .await
    }

    pub async fn block_number(&self) -> Result<u64> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        Ok(provider.get_block_number().await?)
    }

    /// Wait for L1 to advance `n` more blocks from now.
    pub async fn wait_for_l1_blocks(&self, n: u64, timeout: Duration) -> Result<u64> {
        let from = self.block_number().await?;
        wait_for_l1_blocks(self.rpc_url, from + n, timeout).await
    }

    /// Wait until ≥ `n` `BatchPosted` events visible.
    pub async fn wait_for_batches(&self, n: usize, timeout: Duration) -> Result<usize> {
        wait_for(timeout, || async {
            let count = self.batches_posted().await?;
            Ok((count >= n).then_some(count))
        })
        .await
    }

    /// Wait for L1 batch progress, failing immediately if any watched node
    /// reports a terminal condition that makes the target unreachable.
    pub async fn wait_for_batches_or_node_failure(
        &self,
        n: usize,
        nodes: &[&NodeHandle],
        timeout: Duration,
    ) -> Result<usize> {
        wait_for_with_failure(
            timeout,
            || {
                for node in nodes {
                    if let Some(failure) = node.progress_failure()? {
                        return Ok(Some(failure));
                    }
                }
                Ok(None)
            },
            || async {
                let count = self.batches_posted().await?;
                Ok((count >= n).then_some(count))
            },
        )
        .await
    }

    /// Assert that a submitted `postAndVerifyBatch` for `expected_rollup_id`
    /// was mined and reverted with `expected_revert_selector`.
    /// Trace the original transaction, including its blob hashes and exact
    /// pre-transaction state; a reconstructed eth_call can fail differently.
    pub async fn assert_failed_post_and_verify_batch(
        &self,
        expected_rollup_id: u64,
        expected_revert_selector: [u8; 4],
    ) -> Result<()> {
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let latest = provider.get_block_number().await?;

        for block_number in self.deploy_block..=latest {
            let Some(block) = provider
                .get_block_by_number(BlockNumberOrTag::Number(block_number))
                .full()
                .await?
            else {
                continue;
            };
            for transaction in block.transactions.txns() {
                if transaction.inner.to() != Some(self.eez_address)
                    || !transaction
                        .inner
                        .input()
                        .starts_with(&eez_protocol::abi::postAndVerifyBatchCall::SELECTOR)
                {
                    continue;
                }
                let call = eez_protocol::abi::postAndVerifyBatchCall::abi_decode(
                    transaction.inner.input(),
                )?;
                if !call
                    .batch
                    .rollupIdsWithProofSystems
                    .iter()
                    .any(|rollup| rollup.rollupId == expected_rollup_id)
                {
                    continue;
                }

                let tx_hash = *transaction.inner.tx_hash();
                let receipt = provider
                    .get_transaction_receipt(tx_hash)
                    .await?
                    .ok_or_else(|| anyhow!("postAndVerifyBatch receipt {tx_hash} is missing"))?;
                if receipt.status() {
                    bail!(
                        "postAndVerifyBatch transaction {tx_hash} for rollup {expected_rollup_id} succeeded"
                    );
                }

                let trace: MinedRevertTrace = provider
                    .client()
                    .request(
                        "debug_traceTransaction",
                        (
                            tx_hash,
                            serde_json::json!({
                                "tracer": "callTracer",
                                "tracerConfig": { "onlyTopCall": true },
                            }),
                        ),
                    )
                    .await
                    .with_context(|| format!("trace mined postAndVerifyBatch {tx_hash}"))?;
                trace.assert_selector(expected_revert_selector)?;
                return Ok(());
            }
        }

        bail!("no mined postAndVerifyBatch transaction found for rollup {expected_rollup_id}")
    }
}

/// Wait until L1's `block_number >= target`. Lets tests assert "the
/// composer had at least N opportunities to act" without arbitrary
/// sleeps — target = current + N is tied to L1 progress instead of wall-clock
/// assumptions.
pub async fn wait_for_l1_blocks(rpc_url: &str, target: u64, timeout: Duration) -> Result<u64> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    wait_for(timeout, || async {
        let n = provider.get_block_number().await?;
        Ok((n >= target).then_some(n))
    })
    .await
}

pub async fn state_root(rpc_url: &str, eez: Address, rollup_id: u64) -> Result<B256> {
    state_root_at(rpc_url, eez, rollup_id, None).await
}

async fn state_root_at(
    rpc_url: &str,
    eez: Address,
    rollup_id: u64,
    block: Option<u64>,
) -> Result<B256> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let registry = IEEZ::new(eez, &provider);
    let mut call = registry.rollups(rollup_id);
    if let Some(block) = block {
        call = call.block(BlockNumberOrTag::Number(block).into());
    }
    Ok(call.call().await?.root)
}

pub async fn rollup_ether_balance(rpc_url: &str, eez: Address, rollup_id: u64) -> Result<U256> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let registry = IEEZ::new(eez, &provider);
    Ok(registry.rollups(rollup_id).call().await?.etherBalance)
}

async fn account_balance(rpc_url: &str, address: Address) -> Result<U256> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    Ok(provider.get_balance(address).await?)
}

/// Derive the address seen as `msg.sender` on the destination chain.
pub async fn cross_chain_source_proxy(
    rpc_url: &str,
    manager: Address,
    original: Address,
    original_rollup_id: u64,
) -> Result<Address> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    Ok(IProxyDerivation::new(manager, &provider)
        .computeCrossChainProxyAddress(original, original_rollup_id)
        .call()
        .await?)
}

/// Return the revert data from an `eth_call` that is expected to fail.
pub async fn call_revert_data(
    rpc_url: &str,
    from: Address,
    to: Address,
    data: Vec<u8>,
) -> Result<Bytes> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let request = TransactionRequest::default()
        .from(from)
        .to(to)
        .input(Bytes::from(data).into());
    match provider.call(request).await {
        Ok(returned) => bail!("eth_call unexpectedly succeeded, returning {returned}"),
        Err(err) => {
            let payload = err
                .as_error_resp()
                .ok_or_else(|| anyhow!("eth_call failed without an RPC error payload: {err}"))?;
            payload
                .as_revert_data()
                .ok_or_else(|| anyhow!("eth_call failed without revert data: {err}"))
        }
    }
}

/// Return matching logs in chain order.
pub async fn events_since(
    rpc_url: &str,
    contract: Address,
    event_sig_hash: B256,
    from_block: u64,
) -> Result<Vec<alloy_rpc_types_eth::Log>> {
    use alloy_rpc_types_eth::Filter;
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let filter = Filter::new()
        .address(contract)
        .event_signature(event_sig_hash)
        .from_block(from_block);
    Ok(provider.get_logs(&filter).await?)
}

/// Count matching events emitted at or after `from_block`.
pub async fn count_events(
    rpc_url: &str,
    contract: Address,
    event_sig_hash: B256,
    from_block: u64,
) -> Result<usize> {
    count_events_at(rpc_url, contract, event_sig_hash, from_block, None).await
}

async fn count_events_at(
    rpc_url: &str,
    contract: Address,
    event_sig_hash: B256,
    from_block: u64,
    to_block: Option<u64>,
) -> Result<usize> {
    use alloy_rpc_types_eth::Filter;
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let mut filter = Filter::new()
        .address(contract)
        .event_signature(event_sig_hash)
        .from_block(from_block);
    if let Some(to_block) = to_block {
        filter = filter.to_block(to_block);
    }
    let logs = provider.get_logs(&filter).await?;
    Ok(logs.len())
}

/// Count `BatchPosted` events on the EEZ contract since `from_block`.
pub async fn batches_posted(l1_rpc: &str, eez: Address, from_block: u64) -> Result<usize> {
    count_events(l1_rpc, eez, IEEZ::BatchPosted::SIGNATURE_HASH, from_block).await
}

/// Recomputes the latest batch's public-input hash and verifies its signature.
pub async fn assert_latest_batch_signature(
    l1_rpc: &str,
    dep: &Deployment,
    expected_attester: Address,
) -> Result<()> {
    use alloy_rpc_types_eth::Filter;

    let provider = ProviderBuilder::new().connect_http(l1_rpc.parse()?);
    let filter = Filter::new()
        .address(dep.eez_address)
        .event_signature(IEEZ::BatchPosted::SIGNATURE_HASH)
        .from_block(dep.deploy_block);
    let log = provider
        .get_logs(&filter)
        .await?
        .pop()
        .ok_or_else(|| anyhow!("no BatchPosted event"))?;
    let tx_hash = log
        .transaction_hash
        .ok_or_else(|| anyhow!("BatchPosted log has no transaction hash"))?;
    let receipt = provider
        .get_transaction_receipt(tx_hash)
        .await?
        .ok_or_else(|| anyhow!("postAndVerifyBatch receipt is missing"))?;
    if !receipt.status() {
        bail!("postAndVerifyBatch transaction reverted");
    }
    let transaction = provider
        .get_transaction_by_hash(tx_hash)
        .await?
        .ok_or_else(|| anyhow!("postAndVerifyBatch transaction is missing"))?;
    let call = eez_protocol::abi::postAndVerifyBatchCall::abi_decode(transaction.inner.input())?;
    let proof = call
        .batch
        .proofs
        .first()
        .ok_or_else(|| anyhow!("posted batch has no proof"))?;
    let transaction_blob_hashes = transaction
        .inner
        .blob_versioned_hashes()
        .map_or_else(Vec::new, <[B256]>::to_vec);
    let containing = provider
        .get_block_by_hash(
            receipt
                .block_hash
                .ok_or_else(|| anyhow!("posted receipt has no block hash"))?,
        )
        .await?
        .ok_or_else(|| anyhow!("posted block is unavailable"))?;
    let posting_context = eez_protocol::public_inputs::L1PostingContext {
        chain_id: provider.get_chain_id().await?,
        parent_number: containing
            .header
            .number
            .checked_sub(1)
            .ok_or_else(|| anyhow!("a posted transaction cannot be in genesis"))?,
        parent_hash: containing.header.parent_hash,
        target_timestamp: containing.header.timestamp,
    };
    let public_inputs_hash = eez_protocol::public_inputs::public_inputs_hashes_with_blob_hashes(
        &call.batch,
        expected_attester.into_word(),
        &transaction_blob_hashes,
        posting_context,
    )?
    .into_iter()
    .next()
    .ok_or_else(|| anyhow!("posted batch has no public-input hash"))?;
    let signature = Signature::try_from(proof.as_ref()).context("decode posted signature")?;
    let recovered = signature
        .recover_address_from_prehash(&public_inputs_hash)
        .context("recover posted signature")?;
    if recovered != expected_attester {
        bail!("posted proof recovered {recovered}, expected {expected_attester}");
    }
    Ok(())
}

async fn latest_l2_execution_state_at(
    rpc_url: &str,
    contract: Address,
    rollup_id: u64,
    from_block: u64,
    to_block: Option<u64>,
) -> Result<Option<B256>> {
    use alloy_rpc_types_eth::Filter;
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let mut filter = Filter::new()
        .address(contract)
        .event_signature(IEEZ::L2ExecutionPerformed::SIGNATURE_HASH)
        .topic1(B256::from(U256::from(rollup_id)))
        .from_block(from_block);
    if let Some(to_block) = to_block {
        filter = filter.to_block(to_block);
    }
    let mut logs = provider.get_logs(&filter).await?;
    let Some(last) = logs.pop() else {
        return Ok(None);
    };
    let decoded = IEEZ::L2ExecutionPerformed::decode_log(&last.inner)?;
    Ok(Some(decoded.newRoot))
}

pub async fn all_l2_execution_states(
    rpc_url: &str,
    contract: Address,
    rollup_id: u64,
    from_block: u64,
) -> Result<Vec<B256>> {
    Ok(
        all_l2_execution_events(rpc_url, contract, rollup_id, from_block)
            .await?
            .into_iter()
            .map(|event| event.state)
            .collect(),
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct L2ExecutionEvent {
    state: B256,
    block_number: u64,
    block_hash: B256,
    transaction_hash: B256,
    transaction_index: u64,
    log_index: u64,
}

async fn all_l2_execution_events(
    rpc_url: &str,
    contract: Address,
    rollup_id: u64,
    from_block: u64,
) -> Result<Vec<L2ExecutionEvent>> {
    use alloy_rpc_types_eth::Filter;
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let filter = Filter::new()
        .address(contract)
        .event_signature(IEEZ::L2ExecutionPerformed::SIGNATURE_HASH)
        .topic1(B256::from(U256::from(rollup_id)))
        .from_block(from_block);
    let logs = provider.get_logs(&filter).await?;
    logs.into_iter()
        .map(|log| {
            let block_number = log
                .block_number
                .ok_or_else(|| anyhow!("L2ExecutionPerformed log is missing block_number"))?;
            let transaction_hash = log
                .transaction_hash
                .ok_or_else(|| anyhow!("L2ExecutionPerformed log is missing transaction_hash"))?;
            let log_index = log
                .log_index
                .ok_or_else(|| anyhow!("L2ExecutionPerformed log is missing log_index"))?;
            let decoded = IEEZ::L2ExecutionPerformed::decode_log(&log.inner)?;
            Ok(L2ExecutionEvent {
                state: decoded.newRoot,
                block_number,
                block_hash: log
                    .block_hash
                    .ok_or_else(|| anyhow!("L2ExecutionPerformed log is missing block_hash"))?,
                transaction_hash,
                transaction_index: log.transaction_index.ok_or_else(|| {
                    anyhow!("L2ExecutionPerformed log is missing transaction_index")
                })?,
                log_index,
            })
        })
        .collect()
}

/// Waits for a non-genesis safe state that appears anywhere in L1 execution history.
/// Using the full history avoids racing an advancing on-chain head.
pub async fn wait_for_safe_state(
    node: &NodeHandle,
    chain: &Chain<'_>,
    genesis_root: B256,
    timeout: Duration,
) -> Result<()> {
    wait_for(timeout, || async {
        let node_root = async {
            let provider = ProviderBuilder::new().connect_http(node.l2_rpc_url().parse()?);
            let block = provider
                .get_block_by_number(alloy_rpc_types_eth::BlockNumberOrTag::Safe)
                .await?;
            Ok::<Option<B256>, anyhow::Error>(block.map(|b| b.header.hash))
        }
        .await
        .ok()
        .flatten();
        let attested = chain.executed_states().await.unwrap_or_default();
        Ok(match node_root {
            Some(n) if n != B256::ZERO && n != genesis_root && attested.contains(&n) => Some(()),
            _ => None,
        })
    })
    .await
}

/// Waits for a safe block whose state was newly attested after `previous_states`.
/// The returned number and hash identify the exact newly attested block.
pub async fn wait_for_new_attested_safe_block(
    node: &NodeHandle,
    chain: &Chain<'_>,
    previous_states: &[B256],
    timeout: Duration,
) -> Result<(u64, B256)> {
    wait_for(timeout, || {
        let rpc = node.l2_rpc_url();
        async move {
            let provider = ProviderBuilder::new().connect_http(rpc.parse()?);
            let Some(block) = provider
                .get_block_by_number(alloy_rpc_types_eth::BlockNumberOrTag::Safe)
                .await?
            else {
                return Ok(None);
            };
            let number = block.header.number;
            let hash = block.header.hash;
            let commitment = block.header.hash;
            if number == 0 || commitment == B256::ZERO || previous_states.contains(&commitment) {
                return Ok(None);
            }
            let attested = chain.executed_states().await?;
            Ok(attested.contains(&commitment).then_some((number, hash)))
        }
    })
    .await
}

/// Waits until the safe chain contains the exact block hash at `number`.
pub async fn wait_for_safe_chain_contains(
    node: &NodeHandle,
    number: u64,
    hash: B256,
    timeout: Duration,
) -> Result<()> {
    wait_for(timeout, || {
        let rpc = node.l2_rpc_url();
        async move {
            let Some((safe_number, _)) =
                block_number_and_hash_at(&rpc, alloy_rpc_types_eth::BlockNumberOrTag::Safe).await?
            else {
                return Ok(None);
            };
            if safe_number < number {
                return Ok(None);
            }
            let Some((_, actual_hash)) = block_number_and_hash_at(
                &rpc,
                alloy_rpc_types_eth::BlockNumberOrTag::Number(number),
            )
            .await?
            else {
                return Ok(None);
            };
            Ok((actual_hash == hash).then_some(()))
        }
    })
    .await
}

/// Return `env` with `key`'s value replaced by `value`. No-op if `key`
/// isn't present.
pub fn override_env(
    mut env: Vec<(&'static str, String)>,
    key: &str,
    value: &str,
) -> Vec<(&'static str, String)> {
    for (k, v) in &mut env {
        if *k == key {
            *v = value.to_string();
        }
    }
    env
}

// Cross-chain test fixture.

use alloy_consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy_network::TxSignerSync;
use alloy_network::eip2718::Encodable2718;

/// Chain ID used by the embedded dev L1.
pub const DEV_CHAIN_ID: u64 = 1337;

pub const FIRST_ROLLUP_ID: u64 = 1;

sol! {
    #[sol(rpc)]
    interface IEEZProxy {
        event CrossChainProxyCreated(address indexed proxy, address indexed originalAddress, uint64 indexed originalRollupId);
        function createCrossChainProxy(address originalAddress, uint64 originalRollupId) external returns (address proxy);
    }
    #[sol(rpc)]
    interface IEEZL2Proxy {
        function createCrossChainProxy(address originalAddress, uint64 originalRollupId) external returns (address proxy);
        function computeCrossChainProxyAddress(address originalAddress, uint64 originalRollupId) external view returns (address proxy);
        function authorizedProxies(address proxy) external view returns (bool isProxy, address originalAddress, uint64 originalRollupId);
    }
    #[sol(rpc)]
    interface IValue {
        event ValueSet(address indexed by, uint256 newValue);
        function value() external view returns (uint256);
        function setValue(uint256 v) external returns (bool changed, uint256 newValue);
    }
    #[sol(rpc)]
    interface IValueNoRet {
        function value() external view returns (uint256);
        function setValue(uint256 v) external;
    }
    #[sol(rpc)]
    interface IEmptyCall {
        function calls() external view returns (uint256);
        function received() external view returns (uint256);
        function lastValue() external view returns (uint256);
        function setValue(uint256 next) external payable returns (uint256);
    }
    #[sol(rpc)]
    interface ISetterWrapper {
        function readWriteRead(uint256 v) external;
        function readChangingValueAroundWrite(uint256 v) external;
        function mixedBefore() external view returns (uint256);
        function mixedAfter() external view returns (uint256);
        function mixedReadSum() external view returns (uint256);
        event Wrapped(uint256 input, bool ok, bool changed, uint256 newValue);
        function proxy() external view returns (address);
        function setViaProxy(uint256 v) external;
        function setSameValueTwice(uint256 v) external;
        function completedProxyCalls() external view returns (uint256);
        function lastChanged() external view returns (bool);
        function lastNewValue() external view returns (uint256);
    }
    #[sol(rpc)]
    interface IRevertingTarget {
        error Rejected(uint256 seen);
        function calls() external view returns (uint256);
        function lastValue() external view returns (uint256);
        function revertCustom(uint256 v) external payable;
        function revertString(uint256 v) external payable;
        function writeThenRevert(uint256 v) external payable;
        function succeed(uint256 v) external payable returns (uint256);
    }
    #[sol(rpc)]
    interface IRevertBubbleWrapper {
        function callAndRecord(bytes calldata data) external;
        function failures() external view returns (uint256);
        function lastRevertLength() external view returns (uint256);
        function lastRevertHash() external view returns (bytes32);
        function successes() external view returns (uint256);
    }
    #[sol(rpc)]
    interface IReturnData {
        function echo(bytes calldata value) external returns (bytes memory);
        function emptyBytes() external returns (bytes memory);
    }
    #[sol(rpc)]
    interface IReturnDataWrapper {
        function callAndRecord(bytes calldata data) external;
        function lastReturnDataLength() external view returns (uint256);
        function lastReturnDataHash() external view returns (bytes32);
    }
    #[sol(rpc)]
    interface INestedSetterInner {
        function completedProxyCalls() external view returns (uint256);
    }
    #[sol(rpc)]
    interface INestedSetterOuter {
        function setViaInner(uint256 v) external;
    }
    #[sol(rpc)]
    interface IOverlayProbeL2 {
        error ValueNotCleared(uint256 value);
        function value() external view returns (uint256);
        function completed() external view returns (uint256);
        function startDeliverThenBurnAndPing(address driver, address probe, uint256 amount) external;
        function startDeliverBurnThenPing(address driver, address probe, uint256 amount) external;
        function startDeliverThenRevert(address driver, address probe, uint256 amount) external;
    }
    #[sol(rpc)]
    interface IOverlayDriverL1 {
        function pings() external view returns (uint256);
        function deliverThenBurnAndPing(address probe, address driver, uint256 amount) external;
        function deliverThenRevert(address probe, uint256 amount) external;
    }
    #[sol(rpc)]
    interface IStaticRoundTripReader {
        function read() external returns (uint256);
        function lastRead() external view returns (uint256);
        function reads() external view returns (uint256);
    }
    /// Derives the source proxy used on the destination chain.
    #[sol(rpc)]
    interface IProxyDerivation {
        function computeCrossChainProxyAddress(address originalAddress, uint64 originalRollupId) external view returns (address);
    }
    #[sol(rpc)]
    interface IEEZL2Direct {
        error UnauthorizedProxy();
        function executeCrossChainCall(address sourceAddress, bytes calldata callData) external payable returns (bytes memory);
    }
    /// Order-dependent target: each call returns state left by the previous one.
    #[sol(rpc)]
    interface ICounter {
        function count() external view returns (uint256);
        function increment() external returns (uint256 newCount);
        function add(uint256 x) external returns (uint256 newCount);
    }
}

/// Write the L2 fixture genesis with a current timestamp.
fn write_l2_genesis_at(ts: u64) -> Result<(PathBuf, tempfile::TempDir)> {
    write_fixture_genesis(ts, None, "l2-genesis.json")
}

/// Write the L1 fixture genesis with the dev chain ID.
fn write_l1_dev_genesis_at(ts: u64) -> Result<(PathBuf, tempfile::TempDir)> {
    write_fixture_genesis(ts, Some(DEV_CHAIN_ID), "l1-genesis.json")
}

fn write_fixture_genesis(
    ts: u64,
    chain_id: Option<u64>,
    filename: &str,
) -> Result<(PathBuf, tempfile::TempDir)> {
    let mut genesis = fixture_genesis()?;
    genesis.timestamp = ts;
    if let Some(id) = chain_id {
        genesis.config.chain_id = id;
        genesis.config.extra_fields.remove("rollupX");
    }
    let dir = tempfile::tempdir().context("genesis tempdir")?;
    let path = dir.path().join(filename);
    std::fs::write(&path, serde_json::to_vec(&genesis)?).context("write genesis")?;
    Ok((path, dir))
}

/// Bind a newly created fixture before calculating its registration commitment.
/// This never rewrites a running node's genesis. Private fixtures select their
/// actual cadence explicitly, and runtime startup authenticates this L1 header.
fn bind_l2_genesis_reference(
    path: &std::path::Path,
    reference: &alloy_consensus::Header,
    block_interval_seconds: u64,
    sync_interval_blocks: u32,
) -> Result<()> {
    use eez_protocol::l2::{DerivationConfig, GenesisL1Reference};
    let mut genesis: alloy_genesis::Genesis = serde_json::from_slice(&std::fs::read(path)?)?;
    genesis.mix_hash = reference.mix_hash;
    let config = DerivationConfig {
        block_interval_seconds,
        sync_interval_blocks,
        l1_reference: GenesisL1Reference {
            chain_id: DEV_CHAIN_ID,
            number: reference.number,
            hash: reference.hash_slow(),
            timestamp: reference.timestamp,
            prev_randao: reference.mix_hash,
        },
    };
    genesis
        .config
        .extra_fields
        .insert("rollupX".into(), serde_json::to_value(config)?);
    let spec = reth_chainspec::ChainSpec::from_genesis(genesis.clone());
    eez_protocol::l2::Schedule::new(config, spec.genesis_header())?;
    std::fs::write(path, serde_json::to_vec(&genesis)?)?;
    Ok(())
}

fn signer_of(key: &str) -> Result<PrivateKeySigner> {
    key.strip_prefix("0x")
        .unwrap_or(key)
        .parse()
        .context("parse signer key")
}

pub fn signer_address(key: &str) -> Result<Address> {
    Ok(signer_of(key)?.address())
}

/// Sign and submit an EIP-1559 transaction. `to == None` is a CREATE.
#[allow(clippy::too_many_arguments)]
pub async fn sign_and_send(
    rpc_url: &str,
    key: &str,
    chain_id: u64,
    nonce: u64,
    to: Option<Address>,
    value: U256,
    input: Vec<u8>,
    gas_limit: u64,
) -> Result<alloy_primitives::TxHash> {
    let signer = signer_of(key)?;
    let mut tx = TxEip1559 {
        chain_id,
        nonce,
        gas_limit,
        // Keep transactions includable while postBatch retains priority.
        max_fee_per_gas: 100_000_000_000,
        max_priority_fee_per_gas: 1_000_000_000,
        to: to.map_or(
            alloy_primitives::TxKind::Create,
            alloy_primitives::TxKind::Call,
        ),
        value,
        access_list: alloy_rpc_types_eth::AccessList::default(),
        input: input.into(),
    };
    let sig = signer.sign_transaction_sync(&mut tx)?;
    let env = TxEnvelope::from(tx.into_signed(sig));
    let hash = *env.tx_hash();
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    // Cross-chain fronts temporarily refuse submissions while starting.
    let deadline = std::time::Instant::now() + Duration::from_mins(2);
    loop {
        match provider.send_raw_transaction(&env.encoded_2718()).await {
            Ok(_) => return Ok(hash),
            Err(err)
                if err.to_string().contains("starting up")
                    && std::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(err) => {
                // Capture the race after rejection without changing the nonce,
                // retrying the transaction, or delaying failure indefinitely.
                // These separate RPC reads are observations, not one snapshot.
                let diagnostics = tokio::time::timeout(Duration::from_secs(2), async {
                    let sender = signer.address();
                    let (latest, pending, pool) = tokio::join!(
                        async { provider.get_transaction_count(sender).await.ok() },
                        async { provider.get_transaction_count(sender).pending().await.ok() },
                        async {
                            provider
                                .client()
                                .request::<_, serde_json::Value>("txpool_contentFrom", (sender,))
                                .await
                                .ok()
                        },
                    );
                    // Log only identities at the rejected nonce, never the
                    // pool's transaction input or an unbounded response.
                    let conflict = |category: &str| {
                        let entries = pool.as_ref()?.get(category)?;
                        entries
                            .get(nonce.to_string())
                            .or_else(|| entries.get(format!("{nonce:#x}")))?
                            .get("hash")?
                            .as_str()?
                            .parse::<B256>()
                            .ok()
                    };
                    format!(
                        "latest_nonce={latest:?}, pending_nonce={pending:?}, \
                         pool_available={}, pending_conflict={:?}, queued_conflict={:?}",
                        pool.is_some(),
                        conflict("pending"),
                        conflict("queued"),
                    )
                })
                .await
                .unwrap_or_else(|_| "nonce/pool diagnostics timed out".into());
                return Err(err).with_context(|| {
                    format!(
                        "submit fixture transaction {hash}: chain={chain_id}, sender={}, \
                         nonce={nonce}, to={to:?}; observed after rejection: {diagnostics}",
                        signer.address(),
                    )
                });
            }
        }
    }
}

/// Return the confirmed nonce. Cross-chain ingress validates against the
/// confirmed nonce plus held transactions, so using the pool's pending nonce
/// here would count held transactions twice.
pub async fn onchain_nonce(rpc_url: &str, key: &str) -> Result<u64> {
    let addr = signer_of(key)?.address();
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    Ok(provider.get_transaction_count(addr).await?)
}

async fn wait_for_successful_receipt(
    rpc_url: &str,
    hash: alloy_primitives::TxHash,
    action: &str,
) -> Result<TransactionReceipt> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let receipt = wait_for(Duration::from_mins(1), || {
        let provider = provider.clone();
        async move { Ok(provider.get_transaction_receipt(hash).await?) }
    })
    .await
    .with_context(|| format!("{action}: waiting for receipt {hash}"))?;
    if !receipt.status() {
        bail!("{action} reverted");
    }
    eprintln!(
        "fixture receipt: {action}, transaction={hash}, block={:?}, block_hash={:?}",
        receipt.block_number, receipt.block_hash,
    );
    Ok(receipt)
}

fn deployment_input(artifact_path: &std::path::Path, constructor_args: Vec<u8>) -> Result<Vec<u8>> {
    let artifact: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(artifact_path)
            .with_context(|| format!("read {}", artifact_path.display()))?,
    )?;
    let bytecode_hex = artifact["bytecode"]["object"]
        .as_str()
        .ok_or_else(|| anyhow!("bytecode.object missing in {}", artifact_path.display()))?
        .strip_prefix("0x")
        .unwrap_or_default();
    let mut data = hex::decode(bytecode_hex).context("decode bytecode")?;
    data.extend_from_slice(&constructor_args);
    Ok(data)
}

async fn deploy_raw(
    rpc_url: &str,
    key: &str,
    chain_id: u64,
    artifact_path: &std::path::Path,
    constructor_args: Vec<u8>,
) -> Result<Address> {
    let data = deployment_input(artifact_path, constructor_args)?;
    let nonce = onchain_nonce(rpc_url, key).await?;
    let hash = sign_and_send(
        rpc_url,
        key,
        chain_id,
        nonce,
        None,
        U256::ZERO,
        data,
        6_000_000,
    )
    .await?;
    let action = format!("deploy of {}", artifact_path.display());
    let receipt = wait_for_successful_receipt(rpc_url, hash, &action).await?;
    receipt
        .contract_address
        .ok_or_else(|| anyhow!("no contract_address for {}", artifact_path.display()))
}

/// Reject a stale Foundry artifact before a fixture deploys bytecode that does
/// not match the Rust ABI used to construct its transactions.
fn require_artifact_method(
    artifact_path: &Path,
    signature: &str,
    expected_selector: [u8; 4],
) -> Result<()> {
    let artifact: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(artifact_path)
            .with_context(|| format!("read {}", artifact_path.display()))?,
    )?;
    let expected = hex::encode(expected_selector);
    let actual = artifact["methodIdentifiers"]
        .get(signature)
        .and_then(serde_json::Value::as_str);
    if actual != Some(expected.as_str()) {
        bail!(
            "stale Solidity artifact {}: expected {signature} selector {expected}, found {}; rebuild with `cd contracts && forge build`",
            artifact_path.display(),
            actual.unwrap_or("missing"),
        );
    }

    let deployed_bytecode = artifact["deployedBytecode"]["object"]
        .as_str()
        .ok_or_else(|| {
            anyhow!(
                "deployedBytecode.object missing in {}",
                artifact_path.display()
            )
        })?;
    if !deployed_bytecode.contains(&format!("63{expected}")) {
        bail!(
            "stale Solidity artifact {}: deployed bytecode does not dispatch {signature} ({expected}); rebuild with `cd contracts && forge build`",
            artifact_path.display(),
        );
    }
    Ok(())
}

/// Deploy EEZ and the Rollup behind transparent proxies, plus the
/// ECDSAProofSystem, on the embedded dev L1, then register the Rollup proxy.
/// Contract addresses are deterministic.
pub async fn deploy_protocol_dev(
    l1_rpc: &str,
    deployer_key: &str,
    attester_key: &str,
    initial_state: B256,
) -> Result<Deployment> {
    let signer = signer_of(deployer_key)?;
    let signer_addr = signer.address();
    let attester = signer_address(attester_key)?;
    let out = repo_root().join("contracts/out");
    let provider = ProviderBuilder::new().connect_http(l1_rpc.parse()?);
    let nonce = onchain_nonce(l1_rpc, deployer_key).await?;
    let register_nonce = nonce.checked_add(5).context("fixture nonce overflow")?;
    // EEZ and the Rollup sit behind transparent proxies, as in DeployEEZ.s.sol
    // and DeployRollup.s.sol; the proxies are the registry and rollup manager.
    // Each proxy creates its ProxyAdmin itself, using no deployer nonce.
    let eez_implementation = signer_addr.create(nonce);
    let eez_address = signer_addr.create(nonce + 1);
    let proof_system_address = signer_addr.create(nonce + 2);
    let rollup_implementation = signer_addr.create(nonce + 3);
    let rollup_manager_address = signer_addr.create(nonce + 4);

    let vkeys: Vec<B256> = vec![B256::from_slice(&{
        let mut padded = [0u8; 32];
        padded[12..].copy_from_slice(attester.as_slice());
        padded
    })];
    // CREATE addresses are known before mining. Queue dependent transactions
    // from one signer in nonce order, then verify every receipt. Waiting after
    // each send would spend six L1 slots on identical setup in every test.
    let initialize = IRollupInit::initializeCall {
        initialOwner: signer_addr,
        initialThreshold: U256::from(1u64),
        proofSystems: vec![proof_system_address],
        vkeys,
    }
    .abi_encode();
    let deployments = [
        (
            "EEZ.sol/EEZ.json",
            signer_addr.abi_encode(),
            eez_implementation,
        ),
        (
            "EEZProxy.sol/EEZProxy.json",
            (eez_implementation, signer_addr).abi_encode_params(),
            eez_address,
        ),
        (
            "ECDSAProofSystem.sol/ECDSAProofSystem.json",
            attester.abi_encode(),
            proof_system_address,
        ),
        (
            "Rollup.sol/Rollup.json",
            eez_address.abi_encode(),
            rollup_implementation,
        ),
        (
            "TransparentUpgradeableProxy.sol/TransparentUpgradeableProxy.json",
            (
                rollup_implementation,
                signer_addr,
                alloy_primitives::Bytes::from(initialize),
            )
                .abi_encode_params(),
            rollup_manager_address,
        ),
    ];
    let mut pending = Vec::new();
    for (offset, (artifact, args, address)) in deployments.into_iter().enumerate() {
        let hash = sign_and_send(
            l1_rpc,
            deployer_key,
            DEV_CHAIN_ID,
            nonce + u64::try_from(offset)?,
            None,
            U256::ZERO,
            deployment_input(&out.join(artifact), args)?,
            6_000_000,
        )
        .await?;
        pending.push((hash, address, artifact));
    }

    let calldata = IEEZ::registerRollupCall {
        rollupContract: rollup_manager_address,
        initialRoot: initial_state,
    }
    .abi_encode();
    let hash = sign_and_send(
        l1_rpc,
        deployer_key,
        DEV_CHAIN_ID,
        register_nonce,
        Some(eez_address),
        U256::ZERO,
        calldata,
        1_000_000,
    )
    .await?;
    let mut deploy_block = None;
    for (deployment, expected, artifact) in pending {
        let receipt = wait_for_successful_receipt(l1_rpc, deployment, artifact).await?;
        ensure!(
            receipt.contract_address == Some(expected),
            "unexpected CREATE address for {artifact}"
        );
        if deploy_block.is_none() {
            deploy_block = Some(
                receipt
                    .block_number
                    .context("protocol deployment receipt has no block number")?,
            );
        }
    }
    wait_for_successful_receipt(l1_rpc, hash, "registerRollup").await?;
    let registry = IEEZ::new(eez_address, &provider);
    let rollup_id: u64 = registry.rollupCounter().call().await?.try_into()?;

    Ok(Deployment {
        eez_address,
        deploy_block: deploy_block.context("protocol deployment receipt has no block number")?,
        proof_system_address,
        rollup_manager_address,
        rollup_id,
    })
}

pub async fn deploy_value(
    rpc_url: &str,
    key: &str,
    chain_id: u64,
    initial: U256,
) -> Result<Address> {
    let out = repo_root().join("contracts/out");
    deploy_raw(
        rpc_url,
        key,
        chain_id,
        &out.join("Value.sol/Value.json"),
        initial.abi_encode(),
    )
    .await
}

/// Deploy the `EEZPostBatcher` fixture forwarding to `registry`.
pub async fn deploy_post_batcher(
    rpc_url: &str,
    key: &str,
    chain_id: u64,
    registry: Address,
) -> Result<Address> {
    let artifact = repo_root().join("contracts/out/EEZPostBatcher.sol/EEZPostBatcher.json");
    // Derivation decodes the poster's input with the registry ABI.
    require_artifact_method(
        &artifact,
        eez_protocol::abi::postAndVerifyBatchCall::SIGNATURE,
        eez_protocol::abi::postAndVerifyBatchCall::SELECTOR,
    )?;
    deploy_raw(rpc_url, key, chain_id, &artifact, registry.abi_encode()).await
}

async fn deploy_empty_call(rpc_url: &str, key: &str, chain_id: u64) -> Result<Address> {
    let out = repo_root().join("contracts/out");
    deploy_raw(
        rpc_url,
        key,
        chain_id,
        &out.join("EmptyCall.sol/EmptyCall.json"),
        Vec::new(),
    )
    .await
}

pub async fn deploy_value_no_ret(
    rpc_url: &str,
    key: &str,
    chain_id: u64,
    initial: U256,
) -> Result<Address> {
    let out = repo_root().join("contracts/out");
    deploy_raw(
        rpc_url,
        key,
        chain_id,
        &out.join("ValueNoRet.sol/ValueNoRet.json"),
        initial.abi_encode(),
    )
    .await
}

async fn deploy_reverting_target(rpc_url: &str, key: &str, chain_id: u64) -> Result<Address> {
    let out = repo_root().join("contracts/out");
    deploy_raw(
        rpc_url,
        key,
        chain_id,
        &out.join("RevertingTarget.sol/RevertingTarget.json"),
        Vec::new(),
    )
    .await
}

async fn deploy_revert_bubble_wrapper(
    rpc_url: &str,
    key: &str,
    chain_id: u64,
    proxy: Address,
) -> Result<Address> {
    let out = repo_root().join("contracts/out");
    deploy_raw(
        rpc_url,
        key,
        chain_id,
        &out.join("RevertBubbleWrapper.sol/RevertBubbleWrapper.json"),
        proxy.abi_encode(),
    )
    .await
}

async fn deploy_return_data(rpc_url: &str, key: &str, chain_id: u64) -> Result<Address> {
    let out = repo_root().join("contracts/out");
    deploy_raw(
        rpc_url,
        key,
        chain_id,
        &out.join("ReturnData.sol/ReturnData.json"),
        Vec::new(),
    )
    .await
}

async fn deploy_return_data_wrapper(
    rpc_url: &str,
    key: &str,
    chain_id: u64,
    proxy: Address,
) -> Result<Address> {
    let out = repo_root().join("contracts/out");
    deploy_raw(
        rpc_url,
        key,
        chain_id,
        &out.join("ReturnDataWrapper.sol/ReturnDataWrapper.json"),
        proxy.abi_encode(),
    )
    .await
}

pub async fn deploy_nested_setter_inner(
    rpc_url: &str,
    key: &str,
    chain_id: u64,
    proxy: Address,
) -> Result<Address> {
    let out = repo_root().join("contracts/out");
    deploy_raw(
        rpc_url,
        key,
        chain_id,
        &out.join("NestedSetterWrapper.sol/NestedSetterInner.json"),
        proxy.abi_encode(),
    )
    .await
}

pub async fn deploy_nested_setter_outer(
    rpc_url: &str,
    key: &str,
    chain_id: u64,
    inner: Address,
) -> Result<Address> {
    let out = repo_root().join("contracts/out");
    deploy_raw(
        rpc_url,
        key,
        chain_id,
        &out.join("NestedSetterWrapper.sol/NestedSetterOuter.json"),
        inner.abi_encode(),
    )
    .await
}

pub async fn deploy_setter_wrapper(
    rpc_url: &str,
    key: &str,
    chain_id: u64,
    proxy: Address,
) -> Result<Address> {
    let out = repo_root().join("contracts/out");
    let artifact = out.join("SetterWrapper.sol/SetterWrapper.json");
    require_artifact_method(
        &artifact,
        "setSameValueTwice(uint256)",
        ISetterWrapper::setSameValueTwiceCall::SELECTOR,
    )?;
    deploy_raw(rpc_url, key, chain_id, &artifact, proxy.abi_encode()).await
}

/// Wait until the outbound fixture is readable and Composer observes an L2
/// parent after that state is deployed. Deployment receipts alone are not a
/// sufficient hand-off boundary: ingress reads latest state, while composition
/// executes against the parent selected for its Sync block. Observe invocation
/// rather than drain because catch-up and in-flight slots return before draining.
async fn wait_for_outbound_fixture_parent(
    node: &NodeHandle,
    l2_rpc: &str,
    wrapper: Address,
    proxy: Address,
) -> Result<()> {
    let provider = ProviderBuilder::new().connect_http(l2_rpc.parse()?);

    let wrapper_code = provider.get_code_at(wrapper).await?;
    if wrapper_code.is_empty() {
        bail!("outbound fixture wrapper {wrapper} has no code");
    }
    let configured_proxy = ISetterWrapper::new(wrapper, &provider)
        .proxy()
        .call()
        .await?;
    if configured_proxy != proxy {
        bail!("outbound fixture wrapper {wrapper} points to {configured_proxy}, expected {proxy}");
    }
    let proxy_code = provider.get_code_at(proxy).await?;
    if proxy_code.is_empty() {
        bail!("outbound fixture proxy {proxy} has no code");
    }
    let proxy_info = IEEZL2Proxy::new(EEZL2_ADDRESS, &provider)
        .authorizedProxies(proxy)
        .call()
        .await?;
    if !proxy_info.isProxy {
        bail!("outbound fixture proxy {proxy} is not authorized by EEZL2");
    }

    let fixture_height = provider.get_block_number().await?;
    let cursor = node.signal_cursor()?;
    wait_for(SETUP_TIMEOUT, || async {
        let covered = node
            .signals_since(cursor)?
            .into_iter()
            .any(|signal| signal.name == signals::COMPOSER_SYNC_SLOT_INVOKED);
        Ok(covered.then_some(()))
    })
    .await
    .with_context(|| {
        format!("Composer did not start a Sync slot after outbound fixture height {fixture_height}")
    })
}

/// Deploy `Counter` (no constructor args) on either chain.
pub async fn deploy_counter(rpc_url: &str, key: &str, chain_id: u64) -> Result<Address> {
    let out = repo_root().join("contracts/out");
    deploy_raw(
        rpc_url,
        key,
        chain_id,
        &out.join("Counter.sol/Counter.json"),
        Vec::new(),
    )
    .await
}

pub async fn counter_count(rpc_url: &str, counter: Address) -> Result<U256> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    Ok(ICounter::new(counter, &provider).count().call().await?)
}

pub async fn value_no_ret(rpc_url: &str, value_addr: Address) -> Result<U256> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    Ok(IValueNoRet::new(value_addr, &provider)
        .value()
        .call()
        .await?)
}

pub async fn completed_proxy_calls(rpc_url: &str, wrapper: Address) -> Result<U256> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    Ok(ISetterWrapper::new(wrapper, &provider)
        .completedProxyCalls()
        .call()
        .await?)
}

pub async fn last_proxy_result(rpc_url: &str, wrapper: Address) -> Result<(bool, U256)> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let wrapper = ISetterWrapper::new(wrapper, &provider);
    Ok((
        wrapper.lastChanged().call().await?,
        wrapper.lastNewValue().call().await?,
    ))
}

/// Create an outbound proxy through the `EEZL2` predeploy.
pub async fn create_l2_cross_chain_proxy(
    l2_rpc: &str,
    key: &str,
    target: Address,
    original_rollup_id: u64,
) -> Result<Address> {
    let provider = ProviderBuilder::new().connect_http(l2_rpc.parse()?);
    let eezl2 = IEEZL2Proxy::new(EEZL2_ADDRESS, &provider);
    let proxy = eezl2
        .computeCrossChainProxyAddress(target, original_rollup_id)
        .call()
        .await?;
    let chain_id = provider.get_chain_id().await?;
    let nonce = onchain_nonce(l2_rpc, key).await?;
    let hash = sign_and_send(
        l2_rpc,
        key,
        chain_id,
        nonce,
        Some(EEZL2_ADDRESS),
        U256::ZERO,
        IEEZL2Proxy::createCrossChainProxyCall {
            originalAddress: target,
            originalRollupId: original_rollup_id,
        }
        .abi_encode(),
        1_500_000,
    )
    .await?;
    wait_for_successful_receipt(l2_rpc, hash, "create L2 cross-chain proxy").await?;
    Ok(proxy)
}

/// Create an inbound proxy and read its address from the emitted event.
pub async fn create_cross_chain_proxy(
    l1_rpc: &str,
    key: &str,
    eez: Address,
    target: Address,
    rollup_id: u64,
) -> Result<Address> {
    create_cross_chain_proxies(l1_rpc, key, eez, &[target], rollup_id)
        .await?
        .pop()
        .context("missing created proxy")
}

/// Submit independent fixture proxies in nonce order before waiting for mining.
async fn create_cross_chain_proxies(
    l1_rpc: &str,
    key: &str,
    eez: Address,
    targets: &[Address],
    rollup_id: u64,
) -> Result<Vec<Address>> {
    let nonce = onchain_nonce(l1_rpc, key).await?;
    let mut pending = Vec::new();
    for (offset, &target) in targets.iter().enumerate() {
        let calldata = IEEZProxy::createCrossChainProxyCall {
            originalAddress: target,
            originalRollupId: rollup_id,
        }
        .abi_encode();
        pending.push(
            sign_and_send(
                l1_rpc,
                key,
                DEV_CHAIN_ID,
                nonce
                    .checked_add(u64::try_from(offset)?)
                    .context("fixture nonce overflow")?,
                Some(eez),
                U256::ZERO,
                calldata,
                2_000_000,
            )
            .await?,
        );
    }
    let mut proxies = Vec::new();
    for hash in pending {
        let receipt = wait_for_successful_receipt(l1_rpc, hash, "createCrossChainProxy").await?;
        proxies.push(
            receipt
                .inner
                .logs()
                .iter()
                .filter(|log| log.address() == eez)
                .find_map(|log| {
                    IEEZProxy::CrossChainProxyCreated::decode_log(&log.inner)
                        .ok()
                        .map(|e| e.proxy)
                })
                .ok_or_else(|| anyhow!("CrossChainProxyCreated event not found in receipt"))?,
        );
    }
    Ok(proxies)
}

pub async fn l2_value(l2_rpc: &str, value_addr: Address) -> Result<U256> {
    let provider = ProviderBuilder::new().connect_http(l2_rpc.parse()?);
    Ok(IValue::new(value_addr, &provider).value().call().await?)
}

pub async fn l2_balance(l2_rpc: &str, addr: Address) -> Result<U256> {
    let provider = ProviderBuilder::new().connect_http(l2_rpc.parse()?);
    Ok(provider.get_balance(addr).await?)
}

pub async fn account_code(rpc_url: &str, addr: Address) -> Result<Bytes> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    Ok(provider.get_code_at(addr).await?)
}

pub async fn receipt_ok(rpc_url: &str, hash: alloy_primitives::TxHash) -> Result<Option<bool>> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    // Native receipts use type 0x76, outside Ethereum's closed receipt enum.
    let receipt: Option<
        TransactionReceipt<alloy_consensus_any::AnyReceiptEnvelope<alloy_rpc_types_eth::Log>>,
    > = provider
        .client()
        .request("eth_getTransactionReceipt", (hash,))
        .await?;
    Ok(receipt.map(|receipt| alloy_network::ReceiptResponse::status(&receipt)))
}

/// Wait for a source receipt, surfacing composer death and retained terminal
/// queue dispositions before the overall deadline. Bound each RPC poll so an
/// unresponsive source endpoint cannot hide a known queue removal. RPC failures
/// remain retryable when no terminal outcome is known. A receipt
/// does not establish canonical cross-chain completion; callers must check
/// settlement and follower convergence separately.
pub async fn wait_for_receipt_or_failure(
    node: &NodeHandle,
    rpc_url: &str,
    hash: B256,
    timeout: Duration,
) -> Result<bool> {
    const RPC_POLL_TIMEOUT: Duration = Duration::from_secs(1);
    let source = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let composer = ProviderBuilder::new().connect_http(node.composer_rpc_url().parse()?);
    wait_for_with_failure(
        timeout,
        || node.progress_failure(),
        || async {
            let receipt =
                tokio::time::timeout(RPC_POLL_TIMEOUT, source.get_transaction_receipt(hash)).await;
            if let Ok(Ok(Some(receipt))) = &receipt {
                return Ok(Some(Ok(receipt.status())));
            }
            let status: serde_json::Value = tokio::time::timeout(
                RPC_POLL_TIMEOUT,
                composer
                    .client()
                    .request("eez_getCrossChainTransaction", (hash,)),
            )
            .await
            .context("queue status RPC poll timed out")??;
            if let Some(reason) = terminal_queue_failure(&status) {
                // A terminal error is a completed outcome, not a retryable poll
                // error. Flatten it only after leaving the wait loop.
                return Ok(Some(Err(anyhow!(
                    "transaction {hash} ended before its receipt: {reason}"
                ))));
            }
            receipt
                .context("source receipt RPC poll timed out")?
                .map(|_| None)
                .map_err(Into::into)
        },
    )
    .await?
}

fn terminal_queue_failure(status: &serde_json::Value) -> Option<&str> {
    if status["lifecycle"] != "terminal" {
        return None;
    }
    match status["ownership"]["reason"].as_str()? {
        // Canonical success can precede the source RPC's receipt visibility.
        "Committed" | "SourceCompleted" => None,
        reason => Some(reason),
    }
}

/// Ports, addresses, keys, and genesis files for the cross-chain fixture.
pub struct CrossChainConfig {
    pub l1_http_port: u16,
    pub l1_auth_port: u16,
    pub l1_p2p_port: u16,
    pub l1_xchain_port: u16,
    pub l2_xchain_port: u16,
    pub eez_address: Address,
    pub proof_system_address: Address,
    pub rollup_manager_address: Address,
    pub rollup_id: u64,
    pub initial_state: B256,
    pub genesis_time: u64,
    /// Kept separate from the poster key for deterministic CREATE addresses.
    pub deployer_key: &'static str,
    /// Unfunded identity used only by the standalone proof signer.
    pub attester_key: &'static str,
    pub poster_key: &'static str,
    pub l1_genesis: (PathBuf, tempfile::TempDir),
    pub l2_genesis: (PathBuf, tempfile::TempDir),
    pub l1_profile: NativeL1Profile,
    port_leases: Vec<PortLease>,
}

/// Explicit private-network fixtures; Gnosis runs its native node/executor.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NativeL1Profile {
    #[default]
    EthereumOsaka,
    EthereumPrague,
    GnosisPrague,
}

impl CrossChainConfig {
    fn new(l1_profile: NativeL1Profile) -> Result<Self> {
        let deployer_key = ANVIL_KEY_1;
        let attester_key = ANVIL_ATTESTER_KEY;
        let poster_key = ANVIL_KEY;
        let deployer = signer_of(deployer_key)?.address();
        // These CREATE nonces must match `deploy_protocol_dev`: the EEZ and
        // Rollup addresses are their proxies.
        let eez_address = deployer.create(1);
        let proof_system_address = deployer.create(2);
        let rollup_manager_address = deployer.create(4);
        let ts = now_unix_secs();
        let l2_genesis = write_l2_genesis_at(ts)?;
        let l1_http_lease = PortLease::http_pair();
        let l1_auth_lease = PortLease::tcp();
        let l1_p2p_lease = PortLease::tcp_udp();
        let l1_xchain_lease = PortLease::tcp();
        let l2_xchain_lease = PortLease::tcp();
        let l1_http_port = l1_http_lease.port();
        let l1_auth_port = l1_auth_lease.port();
        let l1_p2p_port = l1_p2p_lease.port();
        let l1_xchain_port = l1_xchain_lease.port();
        let l2_xchain_port = l2_xchain_lease.port();
        let l1_genesis = write_l1_dev_genesis_at(ts)?;
        if l1_profile != NativeL1Profile::EthereumOsaka {
            let mut genesis: alloy_genesis::Genesis =
                serde_json::from_slice(&std::fs::read(&l1_genesis.0)?)?;
            genesis.config.osaka_time = None;
            genesis.config.prague_time = Some(0);
            if l1_profile == NativeL1Profile::GnosisPrague {
                let chiado = &*reth_gnosis::spec::chains::CHIADO_GENESIS;
                genesis.config.deposit_contract_address = chiado.config.deposit_contract_address;
                for name in ["eip1559collector", "blockRewardsContract"] {
                    genesis
                        .config
                        .extra_fields
                        .insert(name.to_owned(), chiado.config.extra_fields[name].clone());
                }
                // The native executor calls the configured reward contract;
                // carry the reviewed Chiado predeploy code and storage too.
                genesis.alloc.extend(chiado.alloc.clone());
            }
            std::fs::write(&l1_genesis.0, serde_json::to_vec(&genesis)?)?;
        }
        let reference: alloy_genesis::Genesis =
            serde_json::from_slice(&std::fs::read(&l1_genesis.0)?)?;
        let spec = reth_chainspec::ChainSpec::from_genesis(reference);
        bind_l2_genesis_reference(
            &l2_genesis.0,
            spec.genesis_header(),
            CROSS_CHAIN_L2_BLOCK_TIME_MS / 1000,
            u32::try_from(CROSS_CHAIN_SYNC_INTERVAL)?,
        )?;
        let initial_state = genesis_block_hash(&l2_genesis.0)?;
        Ok(Self {
            l1_http_port,
            l1_auth_port,
            l1_p2p_port,
            l1_xchain_port,
            l2_xchain_port,
            eez_address,
            proof_system_address,
            rollup_manager_address,
            rollup_id: FIRST_ROLLUP_ID,
            initial_state,
            genesis_time: ts,
            deployer_key,
            attester_key,
            poster_key,
            l1_genesis,
            l1_profile,
            l2_genesis,
            port_leases: vec![
                l1_http_lease,
                l1_auth_lease,
                l1_p2p_lease,
                l1_xchain_lease,
                l2_xchain_lease,
            ],
        })
    }

    fn take_port_leases(&mut self) -> Vec<PortLease> {
        std::mem::take(&mut self.port_leases)
    }

    fn l1_rpc_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.l1_http_port)
    }

    fn l1_xchain_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.l1_xchain_port)
    }

    fn l2_xchain_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.l2_xchain_port)
    }

    fn env(&self, builder_rpc_url: &str, beacon_url: &str) -> Vec<(&'static str, String)> {
        vec![
            (
                "EEZ_L1_CHAIN",
                if self.l1_profile == NativeL1Profile::GnosisPrague {
                    "testing-gnosis"
                } else {
                    "testing"
                }
                .to_string(),
            ),
            ("EEZ_L1_CHAIN_CONFIG", self.l1_chain_config()),
            ("EEZ_L1_CHAIN_ID", DEV_CHAIN_ID.to_string()),
            ("EEZ_L1_RPC_URL", self.l1_rpc_url()),
            ("EEZ_L1_BEACON_URL", beacon_url.to_string()),
            (
                "EEZ_L1_BLOB_SIDECAR_ENCODING",
                if self.l1_profile == NativeL1Profile::EthereumOsaka {
                    "eip7594"
                } else {
                    "eip4844"
                }
                .to_string(),
            ),
            ("EEZ_L1_TARGET_RPC_URL", self.l1_rpc_url()),
            ("EEZ_L1_BUILDER_RPC_URL", builder_rpc_url.to_string()),
            ("EEZ_L1_XCHAIN_PORT", self.l1_xchain_port.to_string()),
            ("EEZ_L2_XCHAIN_PORT", self.l2_xchain_port.to_string()),
            ("EEZ_L1_HTTP_PORT", self.l1_http_port.to_string()),
            ("EEZ_L1_AUTH_PORT", self.l1_auth_port.to_string()),
            ("EEZ_L1_P2P_PORT", self.l1_p2p_port.to_string()),
            (
                "EEZ_L1_CHAIN_PATH",
                self.l1_genesis.0.to_string_lossy().into_owned(),
            ),
            ("EEZ_L1_POSTER_KEY", self.poster_key.to_string()),
            ("EEZL2_ADDRESS", format!("{EEZL2_ADDRESS:#x}")),
            (
                "EEZ_L1_BLOCK_TIME_MS",
                CROSS_CHAIN_L1_BLOCK_TIME_MS.to_string(),
            ),
            (
                "EEZ_L2_BLOCK_TIME_MS",
                CROSS_CHAIN_L2_BLOCK_TIME_MS.to_string(),
            ),
            ("EEZ_SETTLEMENT_START_AFTER_BLOCK", "3".to_string()),
            ("EEZ_PROOF_TIME_MS", "1000".to_string()),
            ("EEZ_SUBMISSION_SLACK_MS", "100".to_string()),
            ("EEZ_SETTLEMENT_PURE_L2_MODE", "interval".to_string()),
            ("EEZ_SETTLEMENT_PURE_L2_INTERVAL_MS", "86400000".to_string()),
            (
                "EEZ_SETTLEMENT_MAX_UNSETTLED_L2_BLOCKS",
                "1000000".to_string(),
            ),
            ("EEZ_MAX_SPECULATIVE_DEPTH", "0".to_string()),
            (
                "EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS",
                "500000000000".to_string(),
            ),
            (
                "EEZ_L1_POSTBATCH_MAX_FEE_PER_BLOB_GAS",
                "500000000000".to_string(),
            ),
            (
                "EEZ_L1_POSTBATCH_MAX_PRIORITY_FEE",
                "100000000000".to_string(),
            ),
            ("EEZ_REGISTRY_ADDRESS", format!("{:#x}", self.eez_address)),
            ("EEZ_REGISTRY_DEPLOY_BLOCK", "0".to_string()),
            (
                "EEZ_ECDSA_PROOF_SYSTEM_ADDRESS",
                format!("{:#x}", self.proof_system_address),
            ),
            (
                "EEZ_ROLLUP_MANAGER_ADDRESS",
                format!("{:#x}", self.rollup_manager_address),
            ),
            ("EEZ_ROLLUP_ID", self.rollup_id.to_string()),
            ("EEZ_COMPOSER_EXPECT_EXTERNAL_BATCHES", "false".to_string()),
            (
                "EEZ_L2_DATADIR",
                "/tmp/unused-overridden-by-flag".to_string(),
            ),
            (
                "RUST_LOG",
                std::env::var("EEZ_TEST_LOG").unwrap_or_else(|_| {
                    "warn,eez_node=info,eez_l1=info,eez_composer=info,eez_prover_client=info"
                        .to_string()
                }),
            ),
            (
                TEST_L2_GENESIS_ENV,
                self.l2_genesis.0.to_string_lossy().into_owned(),
            ),
        ]
    }

    fn l1_chain_config(&self) -> String {
        let path = self.l1_genesis.0.to_string_lossy();
        if self.l1_profile == NativeL1Profile::GnosisPrague {
            format!("gnosis:{path}")
        } else {
            path.into_owned()
        }
    }
}

pub const SETUP_TIMEOUT: Duration = Duration::from_secs(90);
pub const SETTLE_TIMEOUT: Duration = Duration::from_secs(90);
const CROSS_CHAIN_L1_BLOCK_TIME_MS: u64 = 5_000;
const CROSS_CHAIN_L2_BLOCK_TIME_MS: u64 = 1_000;
const CROSS_CHAIN_SYNC_INTERVAL: u64 = CROSS_CHAIN_L1_BLOCK_TIME_MS / CROSS_CHAIN_L2_BLOCK_TIME_MS;

/// L1's rollup ID in cross-chain identities.
pub const L1_ROLLUP_ID: u64 = 0;

/// Separate deployer keeps CREATE addresses deterministic.
pub const TARGET_DEPLOYER: &str = ANVIL_KEY_3;
pub const INBOUND_USER: &str = ANVIL_KEY_2;
pub const OUTBOUND_USER: &str = ANVIL_KEY_4;

/// Shared cross-chain integration fixture and its spawned processes.
pub struct CrossChainWorld {
    pub node: NodeHandle,
    pub cfg: CrossChainConfig,
    pub dep: Deployment,
    pub l2_chain_id: u64,
    pub value_l2: Address,
    pub inbound_no_ret: Address,
    pub outbound_value: Address,
    pub outbound_no_ret: Address,
    pub recipient: Address,
    pub withdrawal_recipient: Address,
    pub setter_proxy: Address,
    pub deposit_proxy: Address,
    pub inbound_no_ret_proxy: Address,
    pub inbound_wrapper: Address,
    pub outbound_proxy: Address,
    pub outbound_no_ret_proxy: Address,
    pub withdrawal_proxy: Address,
    pub outbound_wrapper: Address,
    /// `to` of every postBatch: the registry, or the [`POST_BATCHER_ENV`] wrapper.
    pub post_batch_target: Address,
    /// Present only in signer-mutation tests.
    pub prover_proxy: Option<ProverProxyHandle>,
    pub proof_signer: ProofSignerHandle,
    /// Present only when the existing fixture uses controlled L1 consensus.
    pub l1_reorg_control: Option<L1ReorgControl>,
    builder_stub: BundleStub,
    /// Present only when [`REJECTING_BUILDER_ENV`] selected a second relay.
    rejecting_builder: Option<RejectingBuilder>,
    beacon: BeaconStub,
    witness_dir: tempfile::TempDir,
    datadir: FailureDatadir,
    env: Vec<(&'static str, String)>,
}

pub struct CrossChainEmptyCallWorld {
    pub world: CrossChainWorld,
    pub empty_call_l2: Address,
    pub empty_call_proxy: Address,
}

impl std::ops::Deref for CrossChainEmptyCallWorld {
    type Target = CrossChainWorld;

    fn deref(&self) -> &Self::Target {
        &self.world
    }
}

pub struct CrossChainReturnDataWorld {
    pub world: CrossChainWorld,
    pub return_data_wrapper: Address,
}

impl std::ops::Deref for CrossChainReturnDataWorld {
    type Target = CrossChainWorld;

    fn deref(&self) -> &Self::Target {
        &self.world
    }
}

pub struct CrossChainNestedSetterWorld {
    pub world: CrossChainWorld,
    pub nested_setter_inner: Address,
    pub nested_setter_outer: Address,
}

impl std::ops::Deref for CrossChainNestedSetterWorld {
    type Target = CrossChainWorld;

    fn deref(&self) -> &Self::Target {
        &self.world
    }
}

pub struct CrossChainOutboundReturnDataWorld {
    pub world: CrossChainWorld,
    pub return_data_wrapper: Address,
}

impl std::ops::Deref for CrossChainOutboundReturnDataWorld {
    type Target = CrossChainWorld;

    fn deref(&self) -> &Self::Target {
        &self.world
    }
}

/// A destination that reverts, reached through a source-side wrapper. Both
/// directions are wired so one fixture covers inbound and outbound.
pub struct CrossChainRevertWorld {
    pub world: CrossChainWorld,
    /// Reverting target on L2, called by `inbound_wrapper` on L1.
    pub reverting_target_l2: Address,
    pub inbound_wrapper: Address,
    /// Reverting target on L1, called by `outbound_wrapper` on L2.
    pub reverting_target_l1: Address,
    pub outbound_wrapper: Address,
}

impl std::ops::Deref for CrossChainRevertWorld {
    type Target = CrossChainWorld;

    fn deref(&self) -> &Self::Target {
        &self.world
    }
}

pub struct CrossChainCodelessWorld {
    pub world: CrossChainWorld,
    pub inbound_wrapper: Address,
    pub outbound_wrapper: Address,
}

impl std::ops::Deref for CrossChainCodelessWorld {
    type Target = CrossChainWorld;

    fn deref(&self) -> &Self::Target {
        &self.world
    }
}

/// Round-trip fixtures in which L1 calls back into a suspended L2 execution.
pub struct CrossChainOverlayRoundTripWorld {
    pub world: CrossChainWorld,
    /// `OverlayProbeL2` on L2.
    pub probe: Address,
    /// L1's proxy for `probe`.
    pub probe_on_l1: Address,
    /// `OverlayDriverL1` on L1.
    pub driver: Address,
    /// L2's proxy for `driver`.
    pub driver_on_l2: Address,
}

impl std::ops::Deref for CrossChainOverlayRoundTripWorld {
    type Target = CrossChainWorld;

    fn deref(&self) -> &Self::Target {
        &self.world
    }
}

/// A top-level static read whose remote target makes one static callback to
/// the reader's chain (the protocol's `topLevelStaticReentrantCounter` shape).
///
/// The reader and `Counter` live on the source chain; the forwarder and the
/// callback helper live on the remote chain. `Counter` has been incremented
/// once, so a correct round trip reads 1.
pub struct CrossChainStaticRoundTripWorld {
    pub world: CrossChainWorld,
    /// `true` for an L1 reader (L1 → L2 → L1), `false` for an L2 reader.
    pub inbound: bool,
    /// `StaticRoundTripReader` on the source chain.
    pub reader: Address,
    /// `Counter` on the source chain.
    pub counter: Address,
    /// `StaticCountForwarder` on the remote chain.
    pub forwarder: Address,
    /// `StaticCountCallback` on the remote chain.
    pub callback: Address,
    /// The source chain's proxy for `callback`, which dispatches the callback.
    pub callback_source_proxy: Address,
}

impl std::ops::Deref for CrossChainStaticRoundTripWorld {
    type Target = CrossChainWorld;

    fn deref(&self) -> &Self::Target {
        &self.world
    }
}

impl CrossChainWorld {
    /// Durable witnesses for this fixture. Readers must use the same database
    /// path as the writer so MDBX's process locks remain shared.
    /// Bundles rejected by the extra relay selected with
    /// [`REJECTING_BUILDER_ENV`], or `None` when the fixture has one relay.
    pub fn rejected_bundle_requests(&self) -> Option<usize> {
        self.rejecting_builder
            .as_ref()
            .map(RejectingBuilder::bundle_requests)
    }

    pub fn witness_db_path(&self) -> &std::path::Path {
        self.witness_dir.path()
    }

    /// Kill the composer without flushing and reopen its databases with an empty pool.
    /// Callers must establish persistence of any local history their assertions require.
    /// Returns the L2 block/state disk frontiers observed after SIGKILL, before reopening.
    pub async fn crash_and_restart_composer(&mut self) -> Result<(u64, u64)> {
        {
            let mut child = self.node.child.lock().expect("node child mutex poisoned");
            ensure!(
                child.try_wait().context("read composer status")?.is_none(),
                "composer exited before the intended crash"
            );
            child.kill().context("crash composer for restart")?;
            let status = child.wait().context("wait for composer exit")?;
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                ensure!(
                    status.signal() == Some(9),
                    "composer did not exit via SIGKILL: {status}"
                );
            }
        }
        let checkpoint = self
            .persisted_l2_frontiers()
            .await?
            .context("crashed L2 has no Finish checkpoint")?;
        eprintln!(
            "crashed L2 disk frontiers: blocks={}, state={}",
            checkpoint.0, checkpoint.1
        );
        self.reopen_composer().await?;
        Ok(checkpoint)
    }

    /// Read the persisted L2 block and state/trie frontiers in a fresh MDBX
    /// transaction. Returns `None` before the first Finish checkpoint exists.
    /// The blocking reader closes before returning; RPC height is not a disk barrier.
    pub async fn persisted_l2_frontiers(&self) -> Result<Option<(u64, u64)>> {
        let path = self.datadir.path().join("db");
        tokio::task::spawn_blocking(move || -> Result<Option<(u64, u64)>> {
            use reth_db::{Database, tables, transaction::DbTx};
            let db = reth_db::open_db_read_only(path, Default::default())
                .map_err(|error| anyhow!("open persisted L2: {error:#}"))?;
            let tx = db.tx()?;
            let Some(checkpoint) = tx.get::<tables::StageCheckpoints>("Finish".into())? else {
                return Ok(None);
            };
            let state_tip = checkpoint
                .finish_stage_checkpoint()
                .and_then(|finish| finish.partial_state_trie())
                .unwrap_or(checkpoint.block_number);
            Ok(Some((checkpoint.block_number, state_tip)))
        })
        .await
        .context("join persisted L2 checkpoint reader")?
    }

    /// Flush via SIGTERM before reopening with a new startup policy snapshot.
    /// A failed or timed-out graceful shutdown fails the test; it is never
    /// silently replaced with SIGKILL. Already-exited startup failures may be retried.
    pub async fn restart_composer_with_env(
        &mut self,
        extra_env: &[(&'static str, String)],
    ) -> Result<()> {
        let pid = {
            let mut child = self.node.child.lock().expect("node child mutex poisoned");
            child
                .try_wait()
                .context("read composer status")?
                .is_none()
                .then(|| child.id())
        };
        if let Some(pid) = pid {
            let signal = tokio::process::Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .await
                .context("send composer SIGTERM")?;
            ensure!(
                signal.success(),
                "sending composer SIGTERM failed: {signal}"
            );
            let status = wait_for(Duration::from_mins(1), || async {
                self.node
                    .child
                    .lock()
                    .expect("node child mutex poisoned")
                    .try_wait()
                    .context("wait for graceful composer shutdown")
            })
            .await
            .with_context(|| {
                format!(
                    "composer did not stop gracefully:\n{}",
                    self.node.log_tail(60)
                )
            })?;
            ensure!(
                status.success(),
                "composer graceful shutdown failed: {status}\n{}",
                self.node.log_tail(60)
            );
        }
        self.env.extend_from_slice(extra_env);
        self.reopen_composer().await
    }

    /// Wait for this isolated miner's block and state to be committed to MDBX.
    /// Latest RPC height includes the engine's in-memory suffix and is not a
    /// persistence barrier. Each poll opens a fresh read-only transaction in a
    /// blocking worker and closes it before sleeping or restarting the process.
    pub async fn wait_for_persisted_l1_block(
        &self,
        block: BlockNumHash,
        timeout: Duration,
    ) -> Result<()> {
        ensure!(
            self.l1_reorg_control.is_none(),
            "persistence barrier requires the isolated L1 miner"
        );
        let path = self.datadir.path().join("embedded-l1/db");
        wait_for(timeout, || {
            let path = path.clone();
            async move {
                let persisted = tokio::task::spawn_blocking(move || -> Result<bool> {
                    use reth_db::{Database, tables, transaction::DbTx};
                    let db = reth_db::open_db_read_only(path, Default::default())
                        .map_err(|error| anyhow!("open persisted L1: {error:#}"))?;
                    let tx = db.tx()?;
                    let Some(checkpoint) = tx.get::<tables::StageCheckpoints>("Finish".into())?
                    else {
                        return Ok(false);
                    };
                    let state_tip = checkpoint
                        .finish_stage_checkpoint()
                        .and_then(|finish| finish.partial_state_trie())
                        .unwrap_or(checkpoint.block_number);
                    Ok(checkpoint.block_number >= block.number
                        && state_tip >= block.number
                        && tx.get::<tables::HeaderNumbers>(block.hash)? == Some(block.number))
                })
                .await
                .context("join persisted L1 reader")??;
                Ok(persisted.then_some(()))
            }
        })
        .await
        .context("wait for the required L1 block and state to persist")
    }

    async fn reopen_composer(&mut self) -> Result<()> {
        self.node = NodeHandle::start_with_datadir(
            "restarted-composer",
            self.datadir.path(),
            &NodeConfig::default(),
            &self.env,
        )
        .await?;
        self.node
            .wait_for_rpc(&self.l1_rpc(), SETUP_TIMEOUT, "restarted embedded L1")
            .await
    }

    /// Hold the next proved request until `release_bundle` supplies its L1
    /// execution sequence. The fixture relay times out rather than hanging.
    pub fn hold_next_bundle(&self) -> Result<()> {
        self.hold_bundle_after(0)
    }

    /// Forward the next `skip` bundles, then hold the following one. Arm it
    /// before the targeted bundle can be composed: arming after observing an
    /// earlier settlement races the composer's next dispatch.
    pub fn hold_bundle_after(&self, skip: u32) -> Result<()> {
        let directory = self.builder_stub.capture_dir().join("control");
        let temporary = directory.join("hold-next.new");
        std::fs::write(
            &temporary,
            serde_json::to_vec(&serde_json::json!({"skip": skip}))?,
        )?;
        std::fs::rename(temporary, directory.join("hold-next.json"))?;
        Ok(())
    }

    /// Forward `skip` bundles, then reject every subsequent bundle until
    /// `resume_bundles`. Unlike a one-shot hold, this also blocks recompositions
    /// while a test persists a canonical prefix and restarts the Composer.
    pub fn pause_bundles_after(&self, skip: u32) -> Result<()> {
        let directory = self.builder_stub.capture_dir().join("control");
        let temporary = directory.join("pause-after.new");
        std::fs::write(
            &temporary,
            serde_json::to_vec(&serde_json::json!({"skip": skip}))?,
        )?;
        std::fs::rename(temporary, directory.join("pause-after.json"))?;
        Ok(())
    }

    /// The first proved bundle rejected by the persistent relay barrier.
    pub fn paused_bundle(&self) -> Result<Option<serde_json::Value>> {
        match std::fs::read(
            self.builder_stub
                .capture_dir()
                .join("control/paused-bundle.json"),
        ) {
            Ok(raw) => Ok(Some(serde_json::from_slice(&raw)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn resume_bundles(&self) -> Result<()> {
        std::fs::remove_file(
            self.builder_stub
                .capture_dir()
                .join("control/pause-after.json"),
        )?;
        Ok(())
    }

    /// The existing relay also forwards read RPC unchanged; tests may point
    /// the canonical reader here to inject a narrow, reversible RPC outage.
    pub fn fault_rpc_url(&self) -> &str {
        &self.builder_stub.url
    }

    pub fn fail_rpc_methods(&self, methods: &[&str]) -> Result<()> {
        let directory = self.builder_stub.capture_dir().join("control");
        let temporary = directory.join("rpc-errors.new");
        std::fs::write(&temporary, serde_json::to_vec(methods)?)?;
        std::fs::rename(temporary, directory.join("rpc-errors.json"))?;
        Ok(())
    }

    /// Reject log ranges and block-by-number reads before `block`, retaining
    /// raw headers and newer history. Zero restores reads; no data is deleted.
    pub fn prune_rpc_history_before(&self, block: u64) -> Result<()> {
        let directory = self.builder_stub.capture_dir().join("control");
        let temporary = directory.join("pruned-before.new");
        std::fs::write(&temporary, serde_json::to_vec(&block)?)?;
        std::fs::rename(temporary, directory.join("pruned-before.json"))?;
        Ok(())
    }

    /// Accept the next bundle at the relay without including it on L1.
    pub fn drop_next_bundle(&self) -> Result<()> {
        self.drop_next_bundles(1)
    }

    /// Acknowledge a bounded series of bundles without including them, allowing
    /// process tests to observe consecutive canonical exclusions and recovery.
    /// The bound covers the Composer's excluded-slot budget (12).
    pub fn drop_next_bundles(&self, count: u32) -> Result<()> {
        anyhow::ensure!((1..=12).contains(&count), "drop count must be in 1..=12");
        std::fs::write(
            self.builder_stub
                .capture_dir()
                .join("control/drop-next.json"),
            serde_json::to_vec(&serde_json::json!({"remaining": count}))?,
        )?;
        Ok(())
    }

    pub fn dropped_bundle(&self) -> Result<Option<serde_json::Value>> {
        match std::fs::read(
            self.builder_stub
                .capture_dir()
                .join("control/dropped-bundle.json"),
        ) {
            Ok(raw) => Ok(Some(serde_json::from_slice(&raw)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn held_bundle(&self) -> Result<Option<serde_json::Value>> {
        match std::fs::read(
            self.builder_stub
                .capture_dir()
                .join("control/held-bundle.json"),
        ) {
            Ok(raw) => Ok(Some(serde_json::from_slice(&raw)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn release_bundle(&self, request: &serde_json::Value) -> Result<()> {
        let directory = self.builder_stub.capture_dir().join("control");
        let temporary = directory.join("released-bundle.new");
        std::fs::write(&temporary, serde_json::to_vec(request)?)?;
        std::fs::rename(temporary, directory.join("released-bundle.json"))?;
        Ok(())
    }

    /// Omit one of three inbound users from the next fully proved bundle.
    /// This controls valid L1 execution; it does not emulate builder policy.
    /// Optionally hold that exact signed user's retry before it reaches L1.
    pub fn omit_next_bundle_user(&self, user_index: usize, hold_retry: bool) -> Result<()> {
        ensure!(user_index < 3, "three-user fixture index out of bounds");
        let directory = self.builder_stub.capture_dir().join("control");
        let temporary = directory.join("omit-user.new");
        std::fs::write(
            &temporary,
            serde_json::to_vec(&serde_json::json!({
                "userIndex": user_index,
                "holdRetry": hold_retry,
            }))?,
        )?;
        std::fs::rename(temporary, directory.join("omit-user.json"))?;
        Ok(())
    }

    /// Original signed request and the exact one-time inclusion fault.
    pub fn bundle_omission(&self) -> Result<Option<serde_json::Value>> {
        match std::fs::read(
            self.builder_stub
                .capture_dir()
                .join("control/omission.json"),
        ) {
            Ok(raw) => Ok(Some(serde_json::from_slice(&raw)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// A fresh follower receives only canonical L1 and blob availability.
    pub fn partial_follower_env(&self) -> Vec<(&'static str, String)> {
        let mut env = self.cfg.env(&self.builder_stub.url, self.beacon.endpoint());
        env.retain(|(name, _)| {
            !matches!(
                *name,
                "EEZ_L1_POSTER_KEY"
                    | "EEZ_COMPOSER_EXPECT_EXTERNAL_BATCHES"
                    | "EEZ_L1_HTTP_PORT"
                    | "EEZ_L1_AUTH_PORT"
                    | "EEZ_L1_P2P_PORT"
                    | "EEZ_L1_XCHAIN_PORT"
                    | "EEZ_L2_XCHAIN_PORT"
            )
        });
        env.push(("EEZ_COMPOSER_EXPECT_EXTERNAL_BATCHES", "true".to_owned()));
        env
    }

    /// Collects node and signer state for settlement timeout failures.
    pub fn settlement_diagnostics(&self) -> String {
        let node_exit = self.node.exit_status();
        let signer_exit = self.proof_signer.exit_status();
        let proxy = self.prover_proxy.as_ref().map_or_else(
            || "none (composer dials the signer directly)".to_owned(),
            |proxy| {
                format!(
                    "{} attempts, {} attested, {} rejected",
                    proxy.attempts(),
                    proxy.successes(),
                    proxy.rejections(),
                )
            },
        );
        format!(
            "\n== settlement diagnostics ==\n\
             node exit: {node_exit:?} (None = still running)\n\
             signer exit: {signer_exit:?} (None = still running)\n\
             prover proxy: {proxy}\n\
             \n== node log, slot and replacement timeline ==\n{}\n\
             \n== node log, proof deadlines and posting failures ==\n{}\n\
             \n== node log, settlement lines ==\n{}\n\
             \n== node log, tail ==\n{}\n\
             \n== proof-signer log, tail ==\n{}\n",
            self.node.log_lines_matching(
                &[
                    "eez.composer.slot.plan",
                    "eez.sequencer.slot.decision",
                    "eez.sequencer.slot.plan_timeout",
                    "sync-slot trigger firing",
                    "schedule channel full; skipping live tick",
                    "built Sync block after held-pool admission",
                    "eez.composer.settlement.replacement",
                    "eez.submitter.bundle.dispatch",
                    "eez.submitter.bundle.fanout",
                    "eez.composer.bundle.observed",
                    "eez.composer.admission.skipped_slot",
                    "canonical settlement evidence changed during proving",
                    "next-slot replacement deferred",
                ],
                80,
            ),
            self.node.log_lines_matching(
                &[
                    "eez.composer.posting.v1",
                    "proof_requested",
                    "cross-chain compose failed",
                ],
                40,
            ),
            self.node.log_lines_matching(SETTLEMENT_LOG_MARKERS, 60),
            self.node.log_tail(40),
            self.proof_signer.log_tail(60),
        )
    }

    pub fn l1_rpc(&self) -> String {
        self.cfg.l1_rpc_url()
    }
    pub fn l1_xchain(&self) -> String {
        self.cfg.l1_xchain_url()
    }
    pub fn l2_xchain(&self) -> String {
        self.cfg.l2_xchain_url()
    }
    pub fn l2_rpc(&self) -> String {
        self.node.l2_rpc_url()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScenarioDirection {
    Inbound,
    Outbound,
}

#[derive(Clone, Debug)]
pub struct ScenarioCall {
    pub to: Address,
    pub value: U256,
    pub data: Vec<u8>,
    pub gas_limit: u64,
}

impl ScenarioCall {
    pub fn new(to: Address, data: Vec<u8>) -> Self {
        Self {
            to,
            value: U256::ZERO,
            data,
            gas_limit: 900_000,
        }
    }

    /// Set the value bridged by this call.
    #[must_use]
    pub fn with_value(mut self, value: impl IntoTestU256) -> Self {
        self.value = value.into_test_u256();
        self
    }

    #[must_use]
    pub const fn with_gas_limit(mut self, gas_limit: u64) -> Self {
        self.gas_limit = gas_limit;
        self
    }
}

pub trait IntoTestU256 {
    fn into_test_u256(self) -> U256;
}

impl IntoTestU256 for U256 {
    fn into_test_u256(self) -> U256 {
        self
    }
}

impl IntoTestU256 for u64 {
    fn into_test_u256(self) -> U256 {
        U256::from(self)
    }
}

// Represent a `bytes32` getter result as a single word.
impl IntoTestU256 for B256 {
    fn into_test_u256(self) -> U256 {
        U256::from_be_bytes(self.0)
    }
}

pub fn setter_call(proxy: Address, value: impl IntoTestU256) -> ScenarioCall {
    ScenarioCall::new(
        proxy,
        IValue::setValueCall {
            v: value.into_test_u256(),
        }
        .abi_encode(),
    )
}

/// A labeled single-word state read.
#[derive(Clone, Debug)]
pub struct StateRead {
    target: Address,
    calldata: Vec<u8>,
    label: String,
}

/// Read `Value.value()`.
pub fn value_read(contract: Address) -> StateRead {
    call_read(contract, "value()", IValue::valueCall {}.abi_encode())
}

/// Define a state read for a getter that returns one word.
pub fn call_read(target: Address, label: impl Into<String>, calldata: Vec<u8>) -> StateRead {
    StateRead {
        target,
        calldata,
        label: label.into(),
    }
}

#[derive(Clone, Copy, Debug)]
enum StateSide {
    L1,
    L2,
}

#[derive(Clone, Debug)]
struct StateExpectation {
    side: StateSide,
    read: StateRead,
    expected: U256,
}

// Execute a state read and decode its first 32-byte word.
pub async fn read_state_word(rpc_url: &str, read: &StateRead) -> Result<U256> {
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
    let returned = provider
        .call(
            TransactionRequest::default()
                .to(read.target)
                .input(Bytes::from(read.calldata.clone()).into()),
        )
        .await
        .with_context(|| format!("eth_call {} on {}", read.label, read.target))?;
    let word: [u8; 32] = returned
        .get(..32)
        .and_then(|w| w.try_into().ok())
        .ok_or_else(|| {
            anyhow!(
                "{} on {} returned {} bytes; expected at least one word",
                read.label,
                read.target,
                returned.len()
            )
        })?;
    Ok(U256::from_be_bytes(word))
}

#[derive(Clone, Debug)]
struct ExecutedScenarioAction {
    direction: ScenarioDirection,
    value: U256,
    nonce: u64,
    hash: alloy_primitives::TxHash,
}

#[derive(Debug)]
struct StandardOracleSnapshot {
    signal_cursor: usize,
    execution_events: Vec<L2ExecutionEvent>,
    safe: Option<(u64, B256)>,
    finalized: Option<(u64, B256)>,
    rollup_ether_balance: U256,
    eez_contract_balance: U256,
    inbound_nonce: u64,
    outbound_nonce: u64,
    grid_residue: Option<u64>,
}

impl StandardOracleSnapshot {
    async fn capture(world: &CrossChainWorld) -> Result<Self> {
        let l1_rpc = world.l1_rpc();
        let l2_rpc = world.l2_rpc();
        let prior_signals = world.node.signals_since(0)?;
        let grid_residue = prior_signals.iter().rev().find_map(|signal| {
            signal
                .fields
                .get("sync_height")
                .and_then(serde_json::Value::as_u64)
                .map(|height| height % CROSS_CHAIN_SYNC_INTERVAL)
        });
        // Bracket the log cursor with identical L1 histories. If a settlement
        // lands across the cursor read, the history changes and we retry.
        let mut aligned_boundary = None;
        for _ in 0..10 {
            let before = all_l2_execution_events(
                &l1_rpc,
                world.cfg.eez_address,
                world.cfg.rollup_id,
                world.dep.deploy_block,
            )
            .await?;
            let cursor = world.node.signal_cursor()?;
            let after = all_l2_execution_events(
                &l1_rpc,
                world.cfg.eez_address,
                world.cfg.rollup_id,
                world.dep.deploy_block,
            )
            .await?;
            if before == after {
                aligned_boundary = Some((after, cursor));
                break;
            }
        }
        let (execution_events, signal_cursor) = aligned_boundary.ok_or_else(|| {
            anyhow!("could not align L1 execution history with the node signal stream")
        })?;
        Ok(Self {
            execution_events,
            signal_cursor,
            safe: block_number_and_hash_at(&l2_rpc, BlockNumberOrTag::Safe).await?,
            finalized: block_number_and_hash_at(&l2_rpc, BlockNumberOrTag::Finalized).await?,
            rollup_ether_balance: rollup_ether_balance(
                &l1_rpc,
                world.cfg.eez_address,
                world.cfg.rollup_id,
            )
            .await?,
            eez_contract_balance: account_balance(&l1_rpc, world.cfg.eez_address).await?,
            inbound_nonce: onchain_nonce(&l1_rpc, INBOUND_USER).await?,
            outbound_nonce: onchain_nonce(&l2_rpc, OUTBOUND_USER).await?,
            grid_residue,
        })
    }

    async fn assert_after(
        &self,
        world: &CrossChainWorld,
        actions: &[ExecutedScenarioAction],
        expect_settled: bool,
        scenario_name: &str,
    ) -> Result<()> {
        world.node.assert_no_divergence_failure_logs();

        let l1_rpc = world.l1_rpc();
        let l2_rpc = world.l2_rpc();
        let records = world.node.signals_since(self.signal_cursor)?;
        let has_reorg = records.iter().any(|record| {
            matches!(
                record.name.as_str(),
                signals::L1_REORG_DETECTED
                    | signals::DERIVER_REORG_RETREATED
                    | signals::DERIVER_REORG_NOOP
            )
        });
        if records
            .iter()
            .any(|record| FATAL_SIGNALS.contains(&record.name.as_str()))
        {
            bail!("{scenario_name}: node emitted a divergence, fatal, or panic signal");
        }
        // The scenario runner only submits valid calls, so any eviction is a bug.
        if records.iter().any(|record| {
            matches!(
                record.name.as_str(),
                signals::TX_POISON_EVICTED | signals::TX_NONCE_CHAIN_EVICTED
            )
        }) {
            bail!("{scenario_name}: a valid scenario transaction was evicted");
        }

        let safe = block_number_and_hash_at(&l2_rpc, BlockNumberOrTag::Safe).await?;
        let finalized = block_number_and_hash_at(&l2_rpc, BlockNumberOrTag::Finalized).await?;
        if let (Some(before), Some(after)) = (self.safe, safe)
            && after.0 < before.0
            && !has_reorg
        {
            bail!(
                "{scenario_name}: safe head retreated {} -> {} without an L1 reorg signal",
                before.0,
                after.0
            );
        }
        if let (Some(before), Some(after)) = (self.safe, safe)
            && after.0 >= before.0
        {
            let canonical_before =
                block_number_and_hash_at(&l2_rpc, BlockNumberOrTag::Number(before.0)).await?;
            if canonical_before.map(|(_, hash)| hash) != Some(before.1) && !has_reorg {
                bail!(
                    "{scenario_name}: captured safe block {} at height {} left the canonical ancestry without an L1 reorg signal; canonical block is {:?}",
                    before.1,
                    before.0,
                    canonical_before,
                );
            }
        }
        if let (Some(before), Some(after)) = (self.finalized, finalized)
            && after.0 < before.0
        {
            bail!(
                "{scenario_name}: finalized head retreated {} -> {}",
                before.0,
                after.0
            );
        }
        if let (Some(before), Some(after)) = (self.finalized, finalized)
            && after.0 >= before.0
        {
            let canonical_before =
                block_number_and_hash_at(&l2_rpc, BlockNumberOrTag::Number(before.0)).await?;
            if canonical_before.map(|(_, hash)| hash) != Some(before.1) {
                bail!(
                    "{scenario_name}: captured finalized block {} at height {} left the canonical ancestry; canonical block is {:?}",
                    before.1,
                    before.0,
                    canonical_before,
                );
            }
        }

        let mut observed_safe = self.safe.map(|head| head.0).unwrap_or(0);
        let mut reorg_authorized = false;
        let mut observed_finalized = self.finalized.map(|head| head.0).unwrap_or(0);
        let mut settlement_points = Vec::new();
        let mut grid_residue = self.grid_residue;
        for record in &records {
            match record.name.as_str() {
                signals::L1_REORG_DETECTED
                | signals::DERIVER_REORG_RETREATED
                | signals::DERIVER_REORG_NOOP => reorg_authorized = true,
                signals::DERIVER_SAFE_ADVANCED => {
                    let next = record.u64("to_block")?;
                    if next < observed_safe && !reorg_authorized {
                        bail!(
                            "{scenario_name}: safe signal retreated {observed_safe} -> {next} without an L1 reorg signal"
                        );
                    }
                    observed_safe = next;
                    reorg_authorized = false;
                    settlement_points.push((
                        record.u64("applied_entries")? as usize,
                        record.b256("l1_settled_root")?,
                        record.b256("l2_safe_block_hash")?,
                        record.u64("l1_block_number")?,
                        record.b256("tx_hash")?,
                    ));
                }
                signals::DERIVER_FINALIZED_ADVANCED => {
                    let next = record.u64("l2_finalized")?;
                    if next < observed_finalized {
                        bail!(
                            "{scenario_name}: finalized signal retreated {observed_finalized} -> {next}"
                        );
                    }
                    observed_finalized = next;
                }
                signals::COMPOSER_BUNDLE_DISPATCHED
                | signals::COMPOSER_PHASE1_BUNDLE_DISPATCHED
                | signals::DERIVER_SYNC_BLOCK_BUILT => {
                    assert_sync_grid(record, &mut grid_residue, scenario_name)?;
                }
                _ => {}
            }
        }

        let execution_events = all_l2_execution_events(
            &l1_rpc,
            world.cfg.eez_address,
            world.cfg.rollup_id,
            world.dep.deploy_block,
        )
        .await?;
        if execution_events.len() < self.execution_events.len() {
            bail!("{scenario_name}: L2ExecutionPerformed history retreated");
        }
        for (applied, l1_settled_root, l2_safe_root, l1_block_number, batch_tx_hash) in
            settlement_points
        {
            if l1_settled_root != l2_safe_root {
                bail!(
                    "{scenario_name}: L1 settled root {l1_settled_root} != L2 safe block root {l2_safe_root}"
                );
            }
            // A signal can be written after the snapshot cursor even though its
            // L1 event was already captured. Bind the signal to its immutable L1
            // block instead of assuming the independently sampled streams share
            // an index boundary. The event may be in a different transaction
            // from `batch_tx_hash`: deferred inbound execution emits it from the
            // bundled user transaction rather than postBatch.
            let endpoint = execution_events
                .iter()
                .rfind(|event| {
                    event.block_number == l1_block_number && event.state == l1_settled_root
                })
                .ok_or_else(|| {
                    anyhow!(
                        "{scenario_name}: batch {batch_tx_hash} reported L1 settled root {l1_settled_root}, but L1 block {l1_block_number} has no matching L2ExecutionPerformed event"
                    )
                })?;

            // A delayed signal for an event already present at capture belongs
            // to the pre-scenario boundary and must not be charged to this run.
            if self.execution_events.contains(endpoint) {
                continue;
            }

            assert_batch_settlement_events(
                &l1_rpc,
                world.cfg.eez_address,
                world.cfg.rollup_id,
                batch_tx_hash,
                applied,
                endpoint,
                &execution_events,
            )
            .await
            .with_context(|| {
                format!("{scenario_name}: batch {batch_tx_hash} settlement evidence")
            })?;
        }
        if safe.is_some() && expect_settled {
            let committed = state_root(&l1_rpc, world.cfg.eez_address, world.cfg.rollup_id).await?;
            let safe_root = safe_block_hash(&l2_rpc)
                .await?
                .ok_or_else(|| anyhow!("{scenario_name}: safe L2 block is absent"))?;
            if committed != safe_root {
                bail!("{scenario_name}: L1 committed root != L2 safe root");
            }
        }

        assert_action_and_nonce_invariants(self, world, actions, scenario_name).await?;
        if expect_settled {
            assert_value_conservation(self, world, actions, scenario_name).await?;
        }
        Ok(())
    }
}

/// Bind the signal to the batch's immutable transaction window. Failed entry
/// frames roll back their events, although their authenticated outcomes still
/// contribute to the deriver's virtual applied-step count.
async fn assert_batch_settlement_events(
    l1_rpc: &str,
    eez: Address,
    rollup_id: u64,
    batch_tx_hash: B256,
    applied: usize,
    endpoint: &L2ExecutionEvent,
    events: &[L2ExecutionEvent],
) -> Result<()> {
    use alloy_rpc_types_eth::Filter;
    use eez_protocol::abi::{BatchPosted, L2TxSkipped, postAndVerifyBatchCall};

    let provider = ProviderBuilder::new().connect_http(l1_rpc.parse()?);
    let transaction = provider
        .get_transaction_by_hash(batch_tx_hash)
        .await?
        .ok_or_else(|| anyhow!("postBatch transaction is missing"))?;
    ensure!(
        transaction.block_hash == Some(endpoint.block_hash),
        "batch and endpoint belong to different L1 blocks"
    );
    let batch_index = transaction
        .transaction_index
        .context("batch has no transaction index")?;
    let call = postAndVerifyBatchCall::abi_decode(transaction.inner.input())?;
    let receipt = provider
        .get_transaction_receipt(batch_tx_hash)
        .await?
        .ok_or_else(|| anyhow!("postBatch receipt is missing"))?;
    ensure!(
        receipt.status() && receipt.block_hash == Some(endpoint.block_hash),
        "batch receipt is unsuccessful or belongs to another block"
    );
    let mut skipped = HashSet::new();
    for log in receipt.inner.logs() {
        if log.address() == eez && log.topic0() == Some(&L2TxSkipped::SIGNATURE_HASH) {
            let event = L2TxSkipped::decode_log(&log.inner)?;
            skipped.insert(usize::try_from(event.transientIdx)?);
        }
    }

    // A later postBatch for this rollup replaces its deferred queue. Its events
    // must never supply evidence for the earlier batch, even if roots repeat.
    let posted = provider
        .get_logs(
            &Filter::new()
                .address(eez)
                .event_signature(BatchPosted::SIGNATURE_HASH)
                .at_block_hash(endpoint.block_hash),
        )
        .await?;
    let mut window_end = u64::MAX;
    for log in posted {
        let index = log
            .transaction_index
            .context("BatchPosted has no transaction index")?;
        if index <= batch_index || index >= window_end {
            continue;
        }
        let hash = log
            .transaction_hash
            .context("BatchPosted has no transaction hash")?;
        let next = provider
            .get_transaction_by_hash(hash)
            .await?
            .context("later postBatch transaction is missing")?;
        ensure!(
            next.block_hash == Some(endpoint.block_hash),
            "later batch belongs to another L1 block"
        );
        let next = postAndVerifyBatchCall::abi_decode(next.inner.input())?;
        if next
            .batch
            .rollupIdsWithProofSystems
            .iter()
            .any(|rollup| rollup.rollupId == rollup_id)
        {
            window_end = index;
        }
    }
    let observed: Vec<_> = events
        .iter()
        .filter(|event| {
            event.block_hash == endpoint.block_hash
                && event.transaction_index >= batch_index
                && event.transaction_index < window_end
        })
        .map(|event| event.state)
        .collect();
    ensure!(
        observed.last() == Some(&endpoint.state),
        "settlement endpoint is not the last event in this batch's window"
    );
    let claimed: Vec<_> = call
        .batch
        .entries
        .iter()
        .enumerate()
        .flat_map(|(index, entry)| {
            let emits = entry.success && !skipped.contains(&index);
            let lookup = eez_protocol::settlement::is_lookup_only_inbound_entry(entry, rollup_id);
            entry
                .rootUpdates
                .iter()
                .filter(move |update| update.rollupId == rollup_id)
                .map(move |update| ExpectedSettlementStep {
                    root: update.newRoot,
                    emits,
                    lookup,
                })
        })
        .collect();
    assert_applied_step_events(&claimed, applied, &observed)
}

#[derive(Clone, Copy)]
struct ExpectedSettlementStep {
    root: B256,
    emits: bool,
    lookup: bool,
}

fn assert_applied_step_events(
    claimed: &[ExpectedSettlementStep],
    applied: usize,
    observed: &[B256],
) -> Result<()> {
    ensure!(
        applied > 0 && applied <= claimed.len(),
        "invalid applied-step count {applied} for {} claims",
        claimed.len()
    );
    // Independently enumerate contiguous fixture ranges. The anchor counts as
    // one applied step; authenticated lookups count as zero. A range cannot
    // cut away a failed physical checkpoint already covered by its carrier.
    let matches = (0..claimed.len()).any(|start| {
        (start + 1..=claimed.len()).any(|end| {
            let steps = &claimed[start..end];
            steps.iter().filter(|step| !step.lookup).count() == applied
                && steps
                    .iter()
                    .find(|step| !step.lookup)
                    .is_some_and(|step| step.emits)
                && steps
                    .windows(2)
                    .all(|pair| pair[1].emits || pair[1].lookup || pair[1].root == pair[0].root)
                && claimed[end..]
                    .iter()
                    .find(|step| !step.lookup)
                    .is_none_or(|next| next.emits || next.root != steps.last().unwrap().root)
                && expected_settlement_logs_match(steps, observed)
        })
    });
    ensure!(
        matches,
        "{applied} applied steps have no matching ordered L1 event sequence: {observed:?}"
    );
    Ok(())
}

/// Fixture-only oracle: emitting lookups may disappear with a source-frame
/// revert. Every emitting physical carrier is mandatory and ordered. Keep
/// this independent of the production scanner's weighted matching algorithm.
fn expected_settlement_logs_match(steps: &[ExpectedSettlementStep], observed: &[B256]) -> bool {
    let Some((step, rest)) = steps.split_first() else {
        return observed.is_empty();
    };
    if !step.emits {
        return expected_settlement_logs_match(rest, observed);
    }
    (step.lookup && expected_settlement_logs_match(rest, observed))
        || observed.split_first().is_some_and(|(root, remaining)| {
            *root == step.root && expected_settlement_logs_match(rest, remaining)
        })
}

fn assert_sync_grid(
    record: &NodeSignal,
    residue: &mut Option<u64>,
    scenario_name: &str,
) -> Result<()> {
    let height = record.u64("sync_height")?;
    let actual = height % CROSS_CHAIN_SYNC_INTERVAL;
    match *residue {
        Some(expected) if expected != actual => bail!(
            "{scenario_name}: sync block {height} is off the deterministic K={CROSS_CHAIN_SYNC_INTERVAL} grid (residue {actual}, expected {expected})"
        ),
        None => *residue = Some(actual),
        _ => {}
    }
    Ok(())
}

async fn assert_action_and_nonce_invariants(
    before: &StandardOracleSnapshot,
    world: &CrossChainWorld,
    actions: &[ExecutedScenarioAction],
    scenario_name: &str,
) -> Result<()> {
    let mut hashes = HashSet::with_capacity(actions.len());
    let mut inbound = before.inbound_nonce;
    let mut outbound = before.outbound_nonce;
    for action in actions {
        if !hashes.insert(action.hash) {
            bail!("{scenario_name}: action hash {} was reused", action.hash);
        }
        let expected = match action.direction {
            ScenarioDirection::Inbound => &mut inbound,
            ScenarioDirection::Outbound => &mut outbound,
        };
        if action.nonce != *expected {
            bail!(
                "{scenario_name}: {:?} sender nonce {} was reused or skipped; expected {}",
                action.direction,
                action.nonce,
                *expected
            );
        }
        *expected += 1;
    }
    // Exact, not `>=`: these actors are scenario-private, so any nonce the
    // scenario did not submit is unaccounted-for activity.
    let actual_inbound = onchain_nonce(&world.l1_rpc(), INBOUND_USER).await?;
    let actual_outbound = onchain_nonce(&world.l2_rpc(), OUTBOUND_USER).await?;
    if actual_inbound != inbound || actual_outbound != outbound {
        bail!(
            "{scenario_name}: sender nonces are {actual_inbound}/{actual_outbound}, expected {inbound}/{outbound}"
        );
    }
    Ok(())
}

async fn assert_value_conservation(
    before: &StandardOracleSnapshot,
    world: &CrossChainWorld,
    actions: &[ExecutedScenarioAction],
    scenario_name: &str,
) -> Result<()> {
    let mut inbound = U256::ZERO;
    let mut outbound = U256::ZERO;
    for action in actions {
        match action.direction {
            ScenarioDirection::Inbound => inbound += action.value,
            ScenarioDirection::Outbound => outbound += action.value,
        }
    }
    let after =
        rollup_ether_balance(&world.l1_rpc(), world.cfg.eez_address, world.cfg.rollup_id).await?;
    if before.rollup_ether_balance + inbound != after + outbound {
        bail!(
            "{scenario_name}: bridged value was not conserved (before={}, inbound={}, after={}, outbound={})",
            before.rollup_ether_balance,
            inbound,
            after,
            outbound
        );
    }
    let contract_balance = account_balance(&world.l1_rpc(), world.cfg.eez_address).await?;
    if contract_balance < after
        || contract_balance + outbound < before.eez_contract_balance + inbound
    {
        bail!("{scenario_name}: L1 EEZ balance no longer backs bridged rollup funds");
    }
    Ok(())
}

/// Declarative cross-chain case. Calls and state oracles are data, so a matrix
/// can be expressed as a `Vec<Scenario>` and run by [`run_scenarios`].
#[derive(Debug)]
pub struct Scenario {
    name: String,
    calls: Vec<(ScenarioDirection, ScenarioCall)>,
    states: Vec<StateExpectation>,
    expect_settled: bool,
}

impl Scenario {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            calls: Vec::new(),
            states: Vec::new(),
            expect_settled: false,
        }
    }

    #[must_use]
    pub fn inbound(mut self, call: ScenarioCall) -> Self {
        self.calls.push((ScenarioDirection::Inbound, call));
        self
    }

    #[must_use]
    pub fn outbound(mut self, call: ScenarioCall) -> Self {
        self.calls.push((ScenarioDirection::Outbound, call));
        self
    }

    #[must_use]
    pub fn expect_l2_state(mut self, read: StateRead, expected: impl IntoTestU256) -> Self {
        self.states.push(StateExpectation {
            side: StateSide::L2,
            read,
            expected: expected.into_test_u256(),
        });
        self
    }

    #[must_use]
    pub fn expect_l1_state(mut self, read: StateRead, expected: impl IntoTestU256) -> Self {
        self.states.push(StateExpectation {
            side: StateSide::L1,
            read,
            expected: expected.into_test_u256(),
        });
        self
    }

    #[must_use]
    pub const fn expect_settled_fully(mut self) -> Self {
        self.expect_settled = true;
        self
    }

    pub async fn run(self, world: &CrossChainWorld) -> Result<()> {
        let l1_rpc = world.l1_rpc();
        let l2_rpc = world.l2_rpc();
        let oracle = StandardOracleSnapshot::capture(world).await?;
        let mut actions = Vec::with_capacity(self.calls.len());
        for (direction, call) in self.calls {
            let (ingress, receipt_rpc, key, chain_id) = match direction {
                ScenarioDirection::Inbound => (
                    world.l1_xchain(),
                    l1_rpc.as_str(),
                    INBOUND_USER,
                    DEV_CHAIN_ID,
                ),
                ScenarioDirection::Outbound => (
                    world.l2_xchain(),
                    l2_rpc.as_str(),
                    OUTBOUND_USER,
                    world.l2_chain_id,
                ),
            };
            let nonce = onchain_nonce(receipt_rpc, key).await?;
            let hash = sign_and_send(
                &ingress,
                key,
                chain_id,
                nonce,
                Some(call.to),
                call.value,
                call.data,
                call.gas_limit,
            )
            .await
            .with_context(|| format!("{}: submit {direction:?} call", self.name))?;
            // A terminal Composer rejection fails here with its reason instead
            // of waiting out the receipt timeout.
            let succeeded =
                wait_for_receipt_or_failure(&world.node, receipt_rpc, hash, SETTLE_TIMEOUT)
                    .await
                    .with_context(|| format!("{}: {direction:?} call was not mined", self.name))?;
            if !succeeded {
                bail!("{}: {direction:?} call reverted", self.name);
            }
            actions.push(ExecutedScenarioAction {
                direction,
                value: call.value,
                nonce,
                hash,
            });
        }

        for expectation in &self.states {
            let rpc = match expectation.side {
                StateSide::L1 => l1_rpc.as_str(),
                StateSide::L2 => l2_rpc.as_str(),
            };
            let converged = wait_for(SETTLE_TIMEOUT, || async {
                Ok(
                    (read_state_word(rpc, &expectation.read).await? == expectation.expected)
                        .then_some(()),
                )
            })
            .await;
            // Include the last observed value when a state expectation times out.
            if converged.is_err() {
                let actual = read_state_word(rpc, &expectation.read).await;
                bail!(
                    "{}: {:?} {} on {} is {:?}, expected {}",
                    self.name,
                    expectation.side,
                    expectation.read.label,
                    expectation.read.target,
                    actual,
                    expectation.expected,
                );
            }
        }

        if self.expect_settled {
            let l2_provider = ProviderBuilder::new().connect_http(l2_rpc.parse()?);
            wait_for(SETTLE_TIMEOUT, || async {
                let committed =
                    state_root(&l1_rpc, world.cfg.eez_address, world.cfg.rollup_id).await?;
                let Some((safe_height, safe_hash)) =
                    block_number_and_hash_at(&l2_rpc, BlockNumberOrTag::Safe).await?
                else {
                    return Ok(None);
                };
                if safe_hash != committed {
                    return Ok(None);
                }
                // Equal heads may still be the pre-action heads. An outbound
                // receipt first appears on the optimistic chain; wait until
                // this exact transaction is included in canonical safe history.
                for action in &actions {
                    if !matches!(action.direction, ScenarioDirection::Outbound) {
                        continue;
                    }
                    let Some(receipt) = l2_provider.get_transaction_receipt(action.hash).await?
                    else {
                        return Ok(None);
                    };
                    let Some(height) = receipt.block_number.filter(|height| *height <= safe_height)
                    else {
                        return Ok(None);
                    };
                    let canonical =
                        block_number_and_hash_at(&l2_rpc, BlockNumberOrTag::Number(height)).await?;
                    if !receipt.status() || canonical.map(|(_, hash)| hash) != receipt.block_hash {
                        return Ok(None);
                    }
                }
                Ok(Some(()))
            })
            .await
            .with_context(|| {
                format!("{}: committed and safe roots did not reconcile", self.name)
            })?;
        }

        oracle
            .assert_after(world, &actions, self.expect_settled, &self.name)
            .await
    }
}

pub async fn run_scenarios(
    world: &CrossChainWorld,
    scenarios: impl IntoIterator<Item = Scenario>,
) -> Result<()> {
    for scenario in scenarios {
        scenario.run(world).await?;
    }
    Ok(())
}

/// Start the shared cross-chain fixture.
pub async fn setup_cross_chain() -> Result<CrossChainWorld> {
    setup_cross_chain_inner(
        None,
        None,
        &[],
        NativeL1Profile::default(),
        false,
        false,
        false,
    )
    .await
}

/// Start the fixture with additional node environment variables.
pub async fn setup_cross_chain_with_env(
    extra_env: &[(&'static str, String)],
) -> Result<CrossChainWorld> {
    setup_cross_chain_inner(
        None,
        None,
        extra_env,
        NativeL1Profile::default(),
        false,
        false,
        false,
    )
    .await
}

pub async fn setup_cross_chain_with_profile(
    profile: NativeL1Profile,
    extra_env: &[(&'static str, String)],
) -> Result<CrossChainWorld> {
    setup_cross_chain_inner(None, None, extra_env, profile, false, false, false).await
}

/// Route the canonical reader through the existing fault-capable relay from
/// startup, so tests can delay L1 observations without stopping the L1 miner.
pub async fn setup_cross_chain_with_profile_and_rpc_faults(
    profile: NativeL1Profile,
    extra_env: &[(&'static str, String)],
) -> Result<CrossChainWorld> {
    setup_cross_chain_inner(None, None, extra_env, profile, false, true, false).await
}

/// The existing streaming proxy with runtime fault/recovery control.
pub async fn setup_cross_chain_with_prover_faults(
    extra_env: &[(&'static str, String)],
) -> Result<CrossChainWorld> {
    setup_cross_chain_inner(
        Some(ProverMutation::None),
        None,
        extra_env,
        NativeL1Profile::default(),
        false,
        false,
        false,
    )
    .await
}

/// Starts the fixture with a mutation proxy and expected attester override.
pub async fn setup_cross_chain_proxied(
    mutation: ProverMutation,
    attester: Address,
) -> Result<CrossChainWorld> {
    setup_cross_chain_inner(
        Some(mutation),
        Some(attester),
        &[],
        NativeL1Profile::default(),
        false,
        false,
        false,
    )
    .await
}

/// Native fee comparison fixture. Zero execution base fee remains zero while
/// these controlled blocks stay below their target; each chain's real blob fee
/// floor remains active. Poster tips are measured from canonical receipts.
pub async fn setup_cross_chain_with_fixed_fee_fixture(
    profile: NativeL1Profile,
    extra_env: &[(&'static str, String)],
) -> Result<CrossChainWorld> {
    setup_cross_chain_inner(None, None, extra_env, profile, true, false, false).await
}

pub async fn setup_cross_chain_empty_call() -> Result<CrossChainEmptyCallWorld> {
    let world = setup_cross_chain().await?;
    let l1_rpc = world.l1_rpc();
    let l2_rpc = world.l2_rpc();
    let empty_call_l2 = deploy_empty_call(&l2_rpc, TARGET_DEPLOYER, world.l2_chain_id).await?;
    let empty_call_proxy = create_cross_chain_proxy(
        &l1_rpc,
        world.cfg.deployer_key,
        world.cfg.eez_address,
        empty_call_l2,
        world.cfg.rollup_id,
    )
    .await?;

    Ok(CrossChainEmptyCallWorld {
        world,
        empty_call_l2,
        empty_call_proxy,
    })
}

pub async fn setup_cross_chain_return_data() -> Result<CrossChainReturnDataWorld> {
    let world = setup_cross_chain().await?;
    let l1_rpc = world.l1_rpc();
    let l2_rpc = world.l2_rpc();
    let return_data_l2 = deploy_return_data(&l2_rpc, TARGET_DEPLOYER, world.l2_chain_id).await?;
    let return_data_proxy = create_cross_chain_proxy(
        &l1_rpc,
        world.cfg.deployer_key,
        world.cfg.eez_address,
        return_data_l2,
        world.cfg.rollup_id,
    )
    .await?;
    let return_data_wrapper =
        deploy_return_data_wrapper(&l1_rpc, TARGET_DEPLOYER, DEV_CHAIN_ID, return_data_proxy)
            .await?;

    Ok(CrossChainReturnDataWorld {
        world,
        return_data_wrapper,
    })
}

pub async fn setup_cross_chain_nested_setter() -> Result<CrossChainNestedSetterWorld> {
    let world = setup_cross_chain().await?;
    let l1_rpc = world.l1_rpc();
    let nested_setter_inner =
        deploy_nested_setter_inner(&l1_rpc, TARGET_DEPLOYER, DEV_CHAIN_ID, world.setter_proxy)
            .await?;
    let nested_setter_outer =
        deploy_nested_setter_outer(&l1_rpc, TARGET_DEPLOYER, DEV_CHAIN_ID, nested_setter_inner)
            .await?;

    Ok(CrossChainNestedSetterWorld {
        world,
        nested_setter_inner,
        nested_setter_outer,
    })
}

pub async fn setup_cross_chain_outbound_return_data() -> Result<CrossChainOutboundReturnDataWorld> {
    let world = setup_cross_chain().await?;
    let l1_rpc = world.l1_rpc();
    let l2_rpc = world.l2_rpc();
    let return_data_l1 = deploy_return_data(&l1_rpc, TARGET_DEPLOYER, DEV_CHAIN_ID).await?;
    let return_data_proxy =
        create_l2_cross_chain_proxy(&l2_rpc, TARGET_DEPLOYER, return_data_l1, 0).await?;
    let return_data_wrapper = deploy_return_data_wrapper(
        &l2_rpc,
        TARGET_DEPLOYER,
        world.l2_chain_id,
        return_data_proxy,
    )
    .await?;

    Ok(CrossChainOutboundReturnDataWorld {
        world,
        return_data_wrapper,
    })
}

pub async fn setup_cross_chain_reverting() -> Result<CrossChainRevertWorld> {
    let world = setup_cross_chain().await?;
    let l1_rpc = world.l1_rpc();
    let l2_rpc = world.l2_rpc();

    let reverting_target_l2 =
        deploy_reverting_target(&l2_rpc, TARGET_DEPLOYER, world.l2_chain_id).await?;
    let inbound_proxy = create_cross_chain_proxy(
        &l1_rpc,
        world.cfg.deployer_key,
        world.cfg.eez_address,
        reverting_target_l2,
        world.cfg.rollup_id,
    )
    .await?;
    let inbound_wrapper =
        deploy_revert_bubble_wrapper(&l1_rpc, TARGET_DEPLOYER, DEV_CHAIN_ID, inbound_proxy).await?;

    let reverting_target_l1 =
        deploy_reverting_target(&l1_rpc, TARGET_DEPLOYER, DEV_CHAIN_ID).await?;
    let outbound_proxy =
        create_l2_cross_chain_proxy(&l2_rpc, TARGET_DEPLOYER, reverting_target_l1, L1_ROLLUP_ID)
            .await?;
    let outbound_wrapper =
        deploy_revert_bubble_wrapper(&l2_rpc, TARGET_DEPLOYER, world.l2_chain_id, outbound_proxy)
            .await?;

    Ok(CrossChainRevertWorld {
        world,
        reverting_target_l2,
        inbound_wrapper,
        reverting_target_l1,
        outbound_wrapper,
    })
}

pub async fn setup_cross_chain_codeless() -> Result<CrossChainCodelessWorld> {
    let world = setup_cross_chain().await?;
    let l1_rpc = world.l1_rpc();
    let l2_rpc = world.l2_rpc();
    let inbound_wrapper =
        deploy_return_data_wrapper(&l1_rpc, TARGET_DEPLOYER, DEV_CHAIN_ID, world.deposit_proxy)
            .await?;
    let outbound_wrapper = deploy_return_data_wrapper(
        &l2_rpc,
        TARGET_DEPLOYER,
        world.l2_chain_id,
        world.withdrawal_proxy,
    )
    .await?;

    Ok(CrossChainCodelessWorld {
        world,
        inbound_wrapper,
        outbound_wrapper,
    })
}

pub async fn setup_cross_chain_overlay_round_trip() -> Result<CrossChainOverlayRoundTripWorld> {
    setup_cross_chain_overlay_round_trip_with_env(&[]).await
}

/// The round-trip fixture with explicit Composer settings, for fee-enabled
/// simulations of the same callback topology used by settlement tests.
pub async fn setup_cross_chain_overlay_round_trip_with_env(
    settings: &[(&'static str, String)],
) -> Result<CrossChainOverlayRoundTripWorld> {
    let world = setup_cross_chain_with_env(settings).await?;
    let l1_rpc = world.l1_rpc();
    let l2_rpc = world.l2_rpc();
    let out = repo_root().join("contracts/out/OverlayRoundTrip.sol");
    let probe = deploy_raw(
        &l2_rpc,
        TARGET_DEPLOYER,
        world.l2_chain_id,
        &out.join("OverlayProbeL2.json"),
        Vec::new(),
    )
    .await?;
    let driver = deploy_raw(
        &l1_rpc,
        TARGET_DEPLOYER,
        DEV_CHAIN_ID,
        &out.join("OverlayDriverL1.json"),
        Vec::new(),
    )
    .await?;
    let probe_on_l1 = create_cross_chain_proxy(
        &l1_rpc,
        TARGET_DEPLOYER,
        world.cfg.eez_address,
        probe,
        world.cfg.rollup_id,
    )
    .await?;
    let driver_on_l2 =
        create_l2_cross_chain_proxy(&l2_rpc, TARGET_DEPLOYER, driver, L1_ROLLUP_ID).await?;

    Ok(CrossChainOverlayRoundTripWorld {
        world,
        probe,
        probe_on_l1,
        driver,
        driver_on_l2,
    })
}

/// Deploy the static round-trip fixture for an L1 (`inbound`) or L2 reader.
///
/// With `deploy_callback_source_proxy == false`, the source chain's proxy for
/// the callback helper is left undeployed. The protocol cannot create it
/// inside a static frame, so the read must be rejected at admission.
pub async fn setup_cross_chain_static_round_trip(
    inbound: bool,
    deploy_callback_source_proxy: bool,
) -> Result<CrossChainStaticRoundTripWorld> {
    let world = setup_cross_chain().await?;
    let l1_rpc = world.l1_rpc();
    let l2_rpc = world.l2_rpc();
    let eez = world.cfg.eez_address;
    let l2_id = world.cfg.rollup_id;
    let out = repo_root().join("contracts/out/StaticRoundTrip.sol");
    let (source_rpc, source_chain_id, remote_rpc, remote_chain_id) = if inbound {
        (
            l1_rpc.clone(),
            DEV_CHAIN_ID,
            l2_rpc.clone(),
            world.l2_chain_id,
        )
    } else {
        (
            l2_rpc.clone(),
            world.l2_chain_id,
            l1_rpc.clone(),
            DEV_CHAIN_ID,
        )
    };
    // A proxy on the source chain for an address on the remote chain, and
    // the reverse.
    let source_proxy_for = |remote: Address| {
        let (l1_rpc, l2_rpc) = (l1_rpc.clone(), l2_rpc.clone());
        async move {
            if inbound {
                create_cross_chain_proxy(&l1_rpc, TARGET_DEPLOYER, eez, remote, l2_id).await
            } else {
                create_l2_cross_chain_proxy(&l2_rpc, TARGET_DEPLOYER, remote, L1_ROLLUP_ID).await
            }
        }
    };
    let remote_proxy_for = |source: Address| {
        let (l1_rpc, l2_rpc) = (l1_rpc.clone(), l2_rpc.clone());
        async move {
            if inbound {
                create_l2_cross_chain_proxy(&l2_rpc, TARGET_DEPLOYER, source, L1_ROLLUP_ID).await
            } else {
                create_cross_chain_proxy(&l1_rpc, TARGET_DEPLOYER, eez, source, l2_id).await
            }
        }
    };

    let counter = deploy_counter(&source_rpc, TARGET_DEPLOYER, source_chain_id).await?;
    let nonce = onchain_nonce(&source_rpc, TARGET_DEPLOYER).await?;
    let hash = sign_and_send(
        &source_rpc,
        TARGET_DEPLOYER,
        source_chain_id,
        nonce,
        Some(counter),
        U256::ZERO,
        ICounter::incrementCall {}.abi_encode(),
        200_000,
    )
    .await?;
    wait_for_successful_receipt(&source_rpc, hash, "static round-trip counter increment").await?;

    let counter_proxy = remote_proxy_for(counter).await?;
    let callback = deploy_raw(
        &remote_rpc,
        TARGET_DEPLOYER,
        remote_chain_id,
        &out.join("StaticCountCallback.json"),
        counter_proxy.abi_encode(),
    )
    .await?;
    let forwarder = deploy_raw(
        &remote_rpc,
        TARGET_DEPLOYER,
        remote_chain_id,
        &out.join("StaticCountForwarder.json"),
        callback.abi_encode(),
    )
    .await?;
    let forwarder_proxy = source_proxy_for(forwarder).await?;
    let callback_source_proxy = if deploy_callback_source_proxy {
        source_proxy_for(callback).await?
    } else {
        let manager = if inbound { eez } else { EEZL2_ADDRESS };
        let provider = ProviderBuilder::new().connect_http(source_rpc.parse()?);
        IProxyDerivation::new(manager, &provider)
            .computeCrossChainProxyAddress(callback, if inbound { l2_id } else { L1_ROLLUP_ID })
            .call()
            .await?
    };
    let reader = deploy_raw(
        &source_rpc,
        TARGET_DEPLOYER,
        source_chain_id,
        &out.join("StaticRoundTripReader.json"),
        forwarder_proxy.abi_encode(),
    )
    .await?;

    Ok(CrossChainStaticRoundTripWorld {
        world,
        inbound,
        reader,
        counter,
        forwarder,
        callback,
        callback_source_proxy,
    })
}

/// Setup can return an error before the caller panics. Emit diagnostics while
/// the process-owned log directories still exist, including without artifacts.
struct FixtureSetupDiagnostics<'a> {
    node: &'a NodeHandle,
    signer: &'a ProofSignerHandle,
    complete: bool,
}

impl Drop for FixtureSetupDiagnostics<'_> {
    fn drop(&mut self) {
        if !self.complete {
            eprintln!(
                "\n== incomplete fixture setup: reconciliation ==\n{}\n\
                 \n== incomplete fixture setup: node ==\n{}\n\
                 \n== incomplete fixture setup: signer ==\n{}",
                self.node.log_lines_matching(
                    &[
                        "eez.deriver",
                        "eez.composer.settlement.replacement",
                        "eez.node.pool.recovery_rejected"
                    ],
                    40,
                ),
                self.node.log_tail(50),
                self.signer.log_tail(20),
            );
        }
    }
}

async fn setup_cross_chain_inner(
    mutation: Option<ProverMutation>,
    attester_override: Option<Address>,
    extra_env: &[(&'static str, String)],
    profile: NativeL1Profile,
    fixed_fee_fixture: bool,
    reader_faults: bool,
    controlled_consensus: bool,
) -> Result<CrossChainWorld> {
    let setup_started = std::time::Instant::now();
    let mut cfg = CrossChainConfig::new(profile)?;
    // Both genesis timestamps move together: L2 cannot precede its finalized
    // L1 reference. The development miner catches up to the current slot clock.
    if let Some((_, age)) = extra_env
        .iter()
        .rev()
        .find(|(name, _)| *name == "EEZ_TEST_L2_GENESIS_AGE_SECONDS")
    {
        let age = age
            .parse::<u64>()
            .context("EEZ_TEST_L2_GENESIS_AGE_SECONDS must be an unsigned integer")?;
        ensure!(
            age <= 131_072 && age.is_multiple_of(CROSS_CHAIN_L1_BLOCK_TIME_MS / 1000),
            "native history fixture age must be a bounded whole settlement interval"
        );
        cfg.genesis_time = cfg
            .genesis_time
            .checked_sub(age)
            .context("native history genesis timestamp underflow")?;
        for path in [&cfg.l1_genesis.0, &cfg.l2_genesis.0] {
            let mut genesis: alloy_genesis::Genesis =
                serde_json::from_slice(&std::fs::read(path)?)?;
            genesis.timestamp = cfg.genesis_time;
            std::fs::write(path, serde_json::to_vec(&genesis)?)?;
        }
    }
    if fixed_fee_fixture {
        let mut genesis: alloy_genesis::Genesis =
            serde_json::from_slice(&std::fs::read(&cfg.l1_genesis.0)?)?;
        genesis.base_fee_per_gas = Some(0);
        std::fs::write(&cfg.l1_genesis.0, serde_json::to_vec(&genesis)?)?;
    }
    // Fee/history variants change the reference hash. Finalize the reference
    // before exporting fixtures, starting the signer or registering L2.
    let reference: alloy_genesis::Genesis =
        serde_json::from_slice(&std::fs::read(&cfg.l1_genesis.0)?)?;
    let reference = reth_chainspec::ChainSpec::from_genesis(reference);
    bind_l2_genesis_reference(
        &cfg.l2_genesis.0,
        reference.genesis_header(),
        CROSS_CHAIN_L2_BLOCK_TIME_MS / 1000,
        u32::try_from(CROSS_CHAIN_SYNC_INTERVAL)?,
    )?;
    cfg.initial_state = genesis_block_hash(&cfg.l2_genesis.0)?;
    let fixture_directory = std::env::var_os("EEZ_TEST_PROVE_FIXTURE_DIR").map(PathBuf::from);
    if let Some(directory) = &fixture_directory {
        std::fs::create_dir_all(directory)?;
        let l2: serde_json::Value = serde_json::from_slice(&std::fs::read(&cfg.l2_genesis.0)?)?;
        std::fs::write(
            directory.join("chain-config.json"),
            serde_json::to_vec_pretty(&l2["config"])?,
        )?;
        std::fs::copy(&cfg.l1_genesis.0, directory.join("l1-genesis.json"))?;
        std::fs::copy(&cfg.l2_genesis.0, directory.join("l2-genesis.json"))?;
        std::fs::write(
            directory.join("identity.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "rollup_id": cfg.rollup_id,
                "proof_system": cfg.proof_system_address,
                "vkey": signer_address(cfg.attester_key)?.into_word(),
                "l2_system_address": L2_SYSTEM_ADDRESS,
                "gnosis_l1": profile == NativeL1Profile::GnosisPrague,
            }))?,
        )?;
    }
    let signer_attester = signer_address(cfg.attester_key)?;
    // Test-only: a small signer window drives the Composer's batch block cap,
    // reaching the historical-prefix path without a multi-thousand-block chain.
    let max_request_blocks = extra_env
        .iter()
        .rev()
        .find(|(name, _)| *name == SIGNER_MAX_REQUEST_BLOCKS_ENV)
        .map(|(_, blocks)| {
            blocks.parse::<u64>().with_context(|| {
                format!("{SIGNER_MAX_REQUEST_BLOCKS_ENV} must be an unsigned integer")
            })
        })
        .transpose()?;
    let proof_signer = ProofSignerHandle::spawn(&ProofSignerConfig {
        l1_rpc_url: cfg.l1_rpc_url(),
        registry_address: cfg.eez_address,
        chain_config: &cfg.l2_genesis.0,
        l1_chain_config: Some(cfg.l1_chain_config()),
        rollup_id: cfg.rollup_id,
        signer_key: cfg.attester_key,
        vkey: signer_address(cfg.attester_key)?.into_word(),
        proof_system: cfg.proof_system_address,
        max_request_blocks,
    })
    .await?;
    let mutation = mutation.or_else(|| fixture_directory.as_ref().map(|_| ProverMutation::None));
    let prover_proxy = match mutation {
        Some(mutation) => Some(
            ProverProxyHandle::spawn(
                proof_signer.endpoint().to_owned(),
                mutation,
                fixture_directory,
            )
            .await?,
        ),
        None => None,
    };
    let prover_url = prover_proxy
        .as_ref()
        .map_or(proof_signer.endpoint(), ProverProxyHandle::endpoint);
    let attester = attester_override.unwrap_or(signer_attester);
    let witness_dir = tempfile::tempdir().context("witness DB tempdir")?;
    let mut datadir = FailureDatadir::new("eez-cross-chain")?;
    let builder_stub = BundleStub::spawn_passthrough(PortLease::tcp(), &cfg.l1_rpc_url()).await?;
    let beacon = BeaconStub::spawn(
        cfg.l1_rpc_url(),
        builder_stub.capture_dir().to_path_buf(),
        cfg.genesis_time,
    )
    .await?;
    let mut l1_reorg_control = controlled_consensus
        .then(|| L1ReorgControl::prepare(&cfg, &builder_stub))
        .transpose()?;
    let mut env = cfg.env(&builder_stub.url, beacon.endpoint());
    if reader_faults {
        env.push(("EEZ_L1_RPC_URL", builder_stub.url.clone()));
    }
    env.extend([
        ("EEZ_PROVER_URL", prover_url.to_string()),
        ("EEZ_ATTESTER_ADDRESS", format!("{attester:#x}")),
        (
            "EEZ_WITNESS_DB_PATH",
            witness_dir.path().to_string_lossy().into_owned(),
        ),
    ]);
    // Before the assertion below, so caller-supplied vars are checked too.
    env.extend_from_slice(extra_env);
    // The node validates the wrapper during boot and does not post until it
    // exists, so its precomputed address can be configured before deployment.
    let post_batcher = if extra_env
        .iter()
        .any(|(name, value)| *name == POST_BATCHER_ENV && value == "1")
    {
        let address = signer_address(POST_BATCHER_DEPLOYER)?.create(0);
        env.push(("EEZ_L1_POSTBATCH_TARGET", format!("{address:#x}")));
        Some(address)
    } else {
        None
    };
    if let Some(control) = &l1_reorg_control {
        env.extend(control.env());
    }
    // The rejecting relay is listed first so success cannot depend on only
    // the first configured relay being consulted.
    let rejecting_builder = if extra_env
        .iter()
        .any(|(name, value)| *name == REJECTING_BUILDER_ENV && value == "1")
    {
        let relay = RejectingBuilder::spawn().await?;
        env.push((
            "EEZ_L1_BUILDER_RPC_URL",
            format!("{},{}", relay.endpoint, builder_stub.url),
        ));
        Some(relay)
    } else {
        None
    };
    assert!(
        !env.iter().any(|(name, _)| *name == "EEZ_PROOF_SIGNER_KEY"),
        "remote composer environment must not contain the proof-signer key",
    );
    let node = NodeHandle::spawn_with_reservations(
        "node",
        datadir.path(),
        &NodeConfig::default(),
        &env,
        cfg.take_port_leases(),
    )?;
    let mut setup_diagnostics = FixtureSetupDiagnostics {
        node: &node,
        signer: &proof_signer,
        complete: false,
    };
    let l1_rpc = cfg.l1_rpc_url();
    let l2_rpc = node.l2_rpc_url();

    let recipient: Address = address!("0x2222222222222222222222222222222222222222");
    let withdrawal_recipient: Address = address!("0x3333333333333333333333333333333333333333");

    node.wait_for_rpc(&l1_rpc, SETUP_TIMEOUT, "embedded L1 RPC")
        .await?;
    if let Some(control) = &mut l1_reorg_control {
        control.start(&cfg).await?;
    }
    let dep = deploy_protocol_dev(
        &l1_rpc,
        cfg.deployer_key,
        cfg.attester_key,
        cfg.initial_state,
    )
    .await?;
    if dep.eez_address != cfg.eez_address
        || dep.proof_system_address != cfg.proof_system_address
        || dep.rollup_manager_address != cfg.rollup_manager_address
    {
        bail!("protocol deployment addresses are not deterministic");
    }
    if dep.rollup_id != cfg.rollup_id {
        bail!(
            "unexpected rollup id: deployed {}, expected {}",
            dep.rollup_id,
            cfg.rollup_id
        );
    }
    if let Some(expected) = post_batcher {
        let deployed = deploy_post_batcher(
            &l1_rpc,
            POST_BATCHER_DEPLOYER,
            DEV_CHAIN_ID,
            cfg.eez_address,
        )
        .await?;
        ensure!(
            deployed == expected,
            "post batcher deployed at {deployed}, configured {expected}"
        );
    }
    let post_batch_target = post_batcher.unwrap_or(cfg.eez_address);

    node.wait_for_rpc(&l2_rpc, SETUP_TIMEOUT, "L2 RPC").await?;
    let l2_chain_id = ProviderBuilder::new()
        .connect_http(l2_rpc.parse()?)
        .get_chain_id()
        .await?;

    // Nonces are chain-local. Overlap independent L1/L2 setup, but keep each
    // account's transactions ordered on its own chain.
    let ((value_l2, inbound_no_ret), (outbound_value, outbound_no_ret)) = tokio::try_join!(
        async {
            let value =
                deploy_value(&l2_rpc, TARGET_DEPLOYER, l2_chain_id, U256::from(5u64)).await?;
            let no_ret =
                deploy_value_no_ret(&l2_rpc, TARGET_DEPLOYER, l2_chain_id, U256::from(5u64))
                    .await?;
            Ok::<_, anyhow::Error>((value, no_ret))
        },
        async {
            let value =
                deploy_value(&l1_rpc, TARGET_DEPLOYER, DEV_CHAIN_ID, U256::from(5u64)).await?;
            let no_ret =
                deploy_value_no_ret(&l1_rpc, TARGET_DEPLOYER, DEV_CHAIN_ID, U256::from(5u64))
                    .await?;
            Ok::<_, anyhow::Error>((value, no_ret))
        },
    )?;
    let expected_value = signer_address(TARGET_DEPLOYER)?.create(0);
    if value_l2 != expected_value {
        bail!("Value address not deterministic");
    }
    let (
        (setter_proxy, deposit_proxy, inbound_no_ret_proxy, inbound_wrapper),
        (outbound_proxy, outbound_no_ret_proxy, withdrawal_proxy, outbound_wrapper),
    ) = tokio::try_join!(
        async {
            let proxies: [Address; 3] = create_cross_chain_proxies(
                &l1_rpc,
                cfg.deployer_key,
                cfg.eez_address,
                &[value_l2, recipient, inbound_no_ret],
                cfg.rollup_id,
            )
            .await?
            .try_into()
            .map_err(|_| anyhow!("expected three fixture proxies"))?;
            let [setter, deposit, no_ret] = proxies;
            let wrapper =
                deploy_setter_wrapper(&l1_rpc, TARGET_DEPLOYER, DEV_CHAIN_ID, setter).await?;
            Ok::<_, anyhow::Error>((setter, deposit, no_ret, wrapper))
        },
        async {
            let outbound =
                create_l2_cross_chain_proxy(&l2_rpc, TARGET_DEPLOYER, outbound_value, 0).await?;
            let no_ret =
                create_l2_cross_chain_proxy(&l2_rpc, TARGET_DEPLOYER, outbound_no_ret, 0).await?;
            let withdrawal =
                create_l2_cross_chain_proxy(&l2_rpc, TARGET_DEPLOYER, withdrawal_recipient, 0)
                    .await?;
            let wrapper =
                deploy_setter_wrapper(&l2_rpc, TARGET_DEPLOYER, l2_chain_id, outbound).await?;
            Ok::<_, anyhow::Error>((outbound, no_ret, withdrawal, wrapper))
        },
    )?;

    wait_for_outbound_fixture_parent(&node, &l2_rpc, outbound_wrapper, outbound_proxy).await?;

    eprintln!(
        "Native {profile:?} fixture ready in {:.3}s",
        setup_started.elapsed().as_secs_f64()
    );
    datadir.fixture_ready();
    setup_diagnostics.complete = true;
    drop(setup_diagnostics);
    Ok(CrossChainWorld {
        node,
        cfg,
        dep,
        l2_chain_id,
        value_l2,
        inbound_no_ret,
        outbound_value,
        outbound_no_ret,
        recipient,
        withdrawal_recipient,
        setter_proxy,
        deposit_proxy,
        inbound_no_ret_proxy,
        inbound_wrapper,
        outbound_proxy,
        outbound_no_ret_proxy,
        withdrawal_proxy,
        outbound_wrapper,
        post_batch_target,
        prover_proxy,
        proof_signer,
        l1_reorg_control,
        builder_stub,
        rejecting_builder,
        beacon,
        witness_dir,
        datadir,
        env,
    })
}

#[cfg(test)]
mod mined_revert_tests {
    use super::{INVALID_PROOF_SELECTOR, MinedRevertTrace};
    use serde_json::json;

    #[test]
    fn accepts_only_the_top_level_revert_selector() {
        let trace: MinedRevertTrace = serde_json::from_value(json!({
            "error": "execution reverted", "output": "0x09bde33900000000",
        }))
        .unwrap();
        trace.assert_selector(INVALID_PROOF_SELECTOR).unwrap();
    }

    #[test]
    fn rejects_missing_wrong_or_nested_revert_evidence() {
        for value in [
            json!({"output": "0x09bde339"}),
            json!({"error": "out of gas", "output": "0x09bde339"}),
            json!({"error": "execution reverted"}),
            json!({"error": "execution reverted", "output": "0x09bd"}),
            json!({"error": "execution reverted", "output": "0x91d5f57109bde339"}),
            json!({"error": "execution reverted: 0x09bde339", "output": "0x91d5f571"}),
            json!({"error": "execution reverted", "output": "0x91d5f571",
                   "calls": [{"error": "execution reverted", "output": "0x09bde339"}]}),
        ] {
            let trace: MinedRevertTrace = serde_json::from_value(value.clone()).unwrap();
            assert!(
                trace.assert_selector(INVALID_PROOF_SELECTOR).is_err(),
                "{value}"
            );
        }
    }
}

#[cfg(test)]
mod framework_tests {
    use super::*;

    #[test]
    fn failure_log_tail_bounds_large_unicode_records_without_losing_selection() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("node.log");
        let large = serde_json::json!({"fields": {
            "event_name": "eez.test.failure",
            "message": "é".repeat(10_000),
        }});
        std::fs::write(
            &path,
            format!("{large}\n{{\"fields\":{{\"event_name\":\"eez.test.success\"}}}}\n"),
        )
        .unwrap();
        let selected = last_lines(&path, 1, Some(&["eez.test.failure"]));
        assert!(selected.contains("eez.test.failure"));
        assert!(selected.ends_with(" … [record truncated]"));
        assert!(selected.chars().count() < 4200);
        assert!(last_lines(&path, 1, None).contains("eez.test.success"));
        assert!(last_lines(&path, 0, None).is_empty());
    }

    #[tokio::test]
    async fn rejected_fixture_submission_reports_identity_and_original_rpc_error() {
        use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
        let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
        let address = server.local_addr().unwrap();
        let mut module = RpcModule::new(());
        module
            .register_method("eth_sendRawTransaction", |_, _, _| {
                Err::<String, _>(ErrorObjectOwned::owned(
                    -32000,
                    "replacement transaction underpriced",
                    None::<()>,
                ))
            })
            .unwrap();
        module
            .register_method("eth_getTransactionCount", |params, _, _| {
                let (_, block): (Address, String) = params.parse()?;
                Ok::<_, ErrorObjectOwned>(if block == "pending" { "0x12" } else { "0x11" })
            })
            .unwrap();
        module
            .register_method("txpool_contentFrom", |_, _, _| {
                serde_json::json!({"pending": {"17": {
                    "hash": B256::repeat_byte(0xab),
                    "input": "transaction input must not appear in diagnostics",
                }}, "queued": {}})
            })
            .unwrap();
        let handle = server.start(module);
        let error = sign_and_send(
            &format!("http://{address}"),
            ANVIL_KEY_1,
            DEV_CHAIN_ID,
            17,
            Some(ANVIL_ADDR_3),
            U256::from(1),
            Vec::new(),
            21_000,
        )
        .await
        .unwrap_err();
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains("chain=1337"), "{diagnostic}");
        assert!(diagnostic.contains("nonce=17"), "{diagnostic}");
        assert!(diagnostic.contains(&signer_address(ANVIL_KEY_1).unwrap().to_string()));
        assert!(diagnostic.contains("-32000"), "{diagnostic}");
        assert!(diagnostic.contains("replacement transaction underpriced"));
        assert!(diagnostic.contains("latest_nonce=Some(17)"), "{diagnostic}");
        assert!(
            diagnostic.contains("pending_nonce=Some(18)"),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains(&format!(
                "pending_conflict=Some({})",
                B256::repeat_byte(0xab)
            )),
            "{diagnostic}"
        );
        assert!(!diagnostic.contains("transaction input must not appear"));
        assert!(!diagnostic.contains(ANVIL_KEY_1));
        handle.stop().unwrap();
        handle.stopped().await;
    }

    #[test]
    fn cold_fixture_expands_exact_prepared_ancestry_and_rejects_wrong_endpoints() {
        use eez_control_rpc::v1::{BlockWitness, CachedOrdinaryRange};
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("preparation")).unwrap();
        let block = |number: u64, hash: u8, parent: u8| ProveChunk {
            kind: Some(prove_chunk::Kind::Block(BlockWitness {
                number,
                hash: B256::repeat_byte(hash).to_vec(),
                parent_hash: B256::repeat_byte(parent).to_vec(),
                ..Default::default()
            })),
        };
        let first = block(1, 1, 0);
        let second = block(2, 2, 1);
        for (hash, chunk) in [(1, &first), (2, &second)] {
            std::fs::write(
                directory
                    .path()
                    .join("preparation")
                    .join(format!("{:x}.protobuf", B256::repeat_byte(hash))),
                encode_fixture_stream(std::slice::from_ref(chunk)).unwrap(),
            )
            .unwrap();
        }
        let mut range = CachedOrdinaryRange {
            first_number: 1,
            last_number: 2,
            first_hash: B256::repeat_byte(1).to_vec(),
            last_hash: B256::repeat_byte(2).to_vec(),
            ..Default::default()
        };
        let expand = |range: CachedOrdinaryRange| {
            cold_fixture_stream(
                &[
                    ProveChunk {
                        kind: Some(prove_chunk::Kind::CachedRange(range)),
                    },
                    block(3, 3, 2),
                ],
                directory.path(),
            )
        };
        assert_eq!(
            expand(range.clone()).unwrap(),
            encode_fixture_stream(&[first, second, block(3, 3, 2)]).unwrap()
        );
        range.first_hash = B256::repeat_byte(9).to_vec();
        assert_eq!(
            expand(range.clone()).unwrap_err().message(),
            "prepared fixture does not match cached range"
        );
        range.last_number = 2049;
        assert_eq!(
            expand(range.clone()).unwrap_err().code(),
            tonic::Code::ResourceExhausted
        );
        range.last_number = 2;
        range.last_hash = B256::repeat_byte(9).to_vec();
        assert!(
            expand(range)
                .unwrap_err()
                .message()
                .starts_with("missing prepared fixture")
        );
    }

    fn controlled_child() -> NodeHandle {
        let directory = tempfile::tempdir().unwrap();
        let log_path = directory.path().join("child.log");
        std::fs::File::create(&log_path).unwrap();
        let child = Command::new("sh")
            .args(["-c", "read -r line"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        NodeHandle {
            child: Mutex::new(child),
            background_tasks: Mutex::new(Vec::new()),
            name: "controlled-exit".into(),
            log_path,
            _keep_alive: vec![directory],
            owned_datadir: None,
            http_port: 0,
            l2_composer_url: "http://127.0.0.1:0".into(),
        }
    }

    #[tokio::test]
    async fn pre_pr_process_exit_interrupts_a_hanging_poll() {
        let node = controlled_child();
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            wait_for_with_failure(
                Duration::from_secs(30),
                || node.progress_failure(),
                || {
                    // EOF terminates the real child only after the first
                    // failure check and once the hanging poll starts.
                    drop(node.child.lock().unwrap().stdin.take());
                    std::future::pending::<Result<Option<()>>>()
                },
            ),
        )
        .await
        .expect("process exit must interrupt a poll before its overall timeout")
        .unwrap_err();
        assert!(
            result
                .to_string()
                .contains("controlled-exit exited unexpectedly")
        );
    }

    #[tokio::test]
    async fn pre_pr_terminal_status_is_observed_when_receipt_rpc_hangs() {
        use http_body_util::BodyExt as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let receipts = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut node = controlled_child();
        node.http_port = receipts.local_addr().unwrap().port();
        node.l2_composer_url = format!("http://{}", listener.local_addr().unwrap());
        let receipt_server = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                let (stream, _) = receipts.accept().await.unwrap();
                connections.spawn(async move {
                    let service = hyper::service::service_fn(|request: HttpRequest<Incoming>| async {
                        let bytes = request.into_body().collect().await.unwrap().to_bytes();
                        let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                        if request["method"] == "eth_getTransactionReceipt" {
                            return std::future::pending::<Result<_, Infallible>>().await;
                        }
                        Ok::<_, Infallible>(beacon_json_response(StatusCode::OK, serde_json::json!({
                            "jsonrpc":"2.0", "id":request["id"],
                            "error":{"code":-32601, "message":"no Composer methods on regular RPC"}
                        })))
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service).await;
                });
            }
        });
        node.background_tasks.lock().unwrap().push(receipt_server);
        let server = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                connections.spawn(async move {
                    let service = hyper::service::service_fn(|request: HttpRequest<Incoming>| async {
                        let bytes = request.into_body().collect().await.unwrap().to_bytes();
                        let request: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                        assert_eq!(request["method"], "eez_getCrossChainTransaction");
                        Ok::<_, Infallible>(beacon_json_response(StatusCode::OK, serde_json::json!({
                            "jsonrpc": "2.0", "id": request["id"],
                            "result": {"lifecycle": "terminal", "ownership": {"reason": "PoolDiscarded"}},
                        })))
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service).await;
                });
            }
        });
        node.background_tasks.lock().unwrap().push(server);
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            wait_for_receipt_or_failure(
                &node,
                &node.l2_rpc_url(),
                B256::repeat_byte(1),
                Duration::from_secs(30),
            ),
        )
        .await
        .expect("a stalled receipt endpoint must not hide a terminal queue outcome")
        .unwrap_err();
        assert!(result.to_string().contains("PoolDiscarded"));
        assert!(
            node.exit_status().is_none(),
            "this case must keep the process alive"
        );
    }

    #[tokio::test]
    async fn slow_healthy_poll_survives_failure_checks_without_restarting() {
        let mut polls = 0;
        let result = wait_for_with_failure(
            Duration::from_secs(5),
            || Ok(None),
            || {
                polls += 1;
                async {
                    tokio::time::sleep(Duration::from_millis(750)).await;
                    Ok(Some(7))
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(result, 7);
        assert_eq!(polls, 1);
    }

    #[tokio::test]
    async fn terminal_failure_stops_wait_before_polling() {
        let polls = std::cell::Cell::new(0);
        let error = wait_for_with_failure(
            Duration::from_secs(30),
            || Ok(Some("composer exited".into())),
            || {
                polls.set(polls.get() + 1);
                std::future::ready(Ok(Some(())))
            },
        )
        .await
        .unwrap_err();
        assert_eq!(polls.get(), 0);
        assert_eq!(error.to_string(), "composer exited");
    }

    #[tokio::test]
    async fn transient_poll_error_does_not_hide_a_later_terminal_failure() {
        let polls = std::cell::Cell::new(0);
        let error = wait_for_with_failure(
            Duration::from_secs(30),
            || Ok((polls.get() > 0).then(|| "composer exited".into())),
            || {
                polls.set(polls.get() + 1);
                async { Err::<Option<()>, _>(anyhow!("temporary RPC failure")) }
            },
        )
        .await
        .unwrap_err();
        assert_eq!(polls.get(), 1);
        assert_eq!(error.to_string(), "composer exited");
    }

    #[tokio::test]
    async fn receipt_wait_retries_transport_errors_and_preserves_reverted_receipts() {
        let mut polls = 0;
        let status = wait_for_with_failure(
            Duration::from_secs(30),
            || Ok(None),
            || {
                polls += 1;
                std::future::ready(if polls == 1 {
                    Err(anyhow!("temporary RPC failure"))
                } else {
                    Ok(Some(false))
                })
            },
        )
        .await
        .unwrap();
        assert!(!status);
        assert_eq!(polls, 2);
    }

    #[tokio::test]
    async fn terminal_queue_outcome_leaves_polling_without_retry() {
        let status = serde_json::json!({
            "lifecycle": "terminal", "ownership": {"reason": "AttemptsExhausted"}
        });
        let mut polls = 0;
        let result: Result<()> = wait_for(Duration::from_secs(30), || {
            polls += 1;
            let reason = terminal_queue_failure(&status).unwrap();
            async move { Ok(Some(Err(anyhow!(reason.to_owned())))) }
        })
        .await
        .unwrap();
        assert_eq!(polls, 1);
        assert_eq!(result.unwrap_err().to_string(), "AttemptsExhausted");
        for lifecycle in ["queued", "in_flight", "unknown"] {
            let current = serde_json::json!({
                "lifecycle": lifecycle, "ownership": {"reason": "AttemptsExhausted"}
            });
            assert_eq!(terminal_queue_failure(&current), None);
        }
        for reason in ["Committed", "SourceCompleted"] {
            let current = serde_json::json!({
                "lifecycle": "terminal", "ownership": {"reason": reason}
            });
            assert_eq!(terminal_queue_failure(&current), None);
        }
        assert_eq!(terminal_queue_failure(&serde_json::Value::Null), None);
    }

    #[test]
    fn settlement_oracle_requires_ordered_events_for_successful_carriers() {
        let a = B256::repeat_byte(1);
        let b = B256::repeat_byte(2);
        let c = B256::repeat_byte(3);
        let physical = |(root, emits)| ExpectedSettlementStep {
            root,
            emits,
            lookup: false,
        };
        let claimed = [(a, true), (a, false), (b, true), (b, false), (c, true)].map(physical);
        assert_applied_step_events(&claimed, 2, &[a]).unwrap();
        assert_applied_step_events(&claimed, 4, &[a, b]).unwrap();
        assert_applied_step_events(&claimed, 3, &[b, c]).unwrap();
        assert_applied_step_events(&claimed, 5, &[a, b, c]).unwrap();
        for (applied, observed) in [
            (0, vec![]),
            (6, vec![a, b, c]),
            (1, vec![a]),          // drops the failed outcome carried by this anchor
            (3, vec![a, b]),       // cuts the next carrier away from its failed step
            (5, vec![a, c]),       // missing a successful event
            (5, vec![b, a, c]),    // wrong order
            (5, vec![a, a, b, c]), // extra event
            (5, vec![]),
        ] {
            assert!(assert_applied_step_events(&claimed, applied, &observed).is_err());
        }
        // Successful no-op updates still emit: equal roots must not collapse.
        let duplicate_roots = [(a, true), (a, true)].map(physical);
        assert_applied_step_events(&duplicate_roots, 2, &[a, a]).unwrap();
        assert!(assert_applied_step_events(&duplicate_roots, 2, &[a]).is_err());
    }

    #[test]
    fn settlement_oracle_excludes_lookups_but_never_skips_physical_carriers() {
        let a = B256::with_last_byte(1);
        let b = B256::with_last_byte(2);
        for emits in [false, true] {
            let claimed = [
                ExpectedSettlementStep {
                    root: a,
                    emits: true,
                    lookup: false,
                },
                ExpectedSettlementStep {
                    root: a,
                    emits,
                    lookup: true,
                },
                ExpectedSettlementStep {
                    root: b,
                    emits: true,
                    lookup: false,
                },
                ExpectedSettlementStep {
                    root: b,
                    emits,
                    lookup: true,
                },
            ];
            assert_applied_step_events(&claimed, 1, &[a]).unwrap();
            assert_applied_step_events(&claimed, 1, &[b]).unwrap();
            assert_applied_step_events(&claimed, 2, &[a, b]).unwrap();
            assert!(assert_applied_step_events(&claimed, 3, &[a, b]).is_err());
            assert!(assert_applied_step_events(&claimed, 2, &[a]).is_err());
            if emits {
                assert_applied_step_events(&claimed, 2, &[a, a, b, b]).unwrap();
                assert_applied_step_events(&claimed, 2, &[a, b, b]).unwrap();
                assert_applied_step_events(&claimed, 1, &[a, a]).unwrap();
            }
        }
        let skipped = [
            ExpectedSettlementStep {
                root: a,
                emits: true,
                lookup: false,
            },
            ExpectedSettlementStep {
                root: b,
                emits: false,
                lookup: false,
            },
        ];
        assert_applied_step_events(&skipped, 1, &[a]).unwrap();
        assert!(assert_applied_step_events(&skipped, 2, &[a]).is_err());
    }

    #[test]
    fn l2_genesis_fixture_is_resolvable_after_testkit_extraction() {
        assert!(l2_genesis_fixture_path().is_file());
        let _ = genesis_block_hash(&l2_genesis_fixture_path()).unwrap();
    }
}
