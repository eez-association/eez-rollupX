//! Launcher for the production eez Composer node.
//!
//! Wraps reth with our composer stack: reth provides the EVM, storage,
//! networking, RPC, and engine; we provide block production
//! (Sequencer + Scheduler), L1 anchoring (`L1Watcher` + Deriver), and
//! batch submission (Composer umbrella).
//!
mod bundle_rpc;
mod ingress;
mod l1_embedded;
mod queue_metrics;
mod queue_rpc;
mod witness_source;

// Offline/native recovery tools use the same validated persistent reader as the producer.
pub use witness_source::WitnessDb;

use std::{
    collections::{HashMap, VecDeque},
    env,
    str::FromStr,
    sync::Arc,
    time::Duration,
};

use alloy_primitives::{Address, B256};
use alloy_provider::Provider as _;
use alloy_signer_local::PrivateKeySigner;
use eez_composer::composer::CrossChainWiring;
use eez_composer::{Composer, ComposerSettings, HeldPool, RollupConfig, RollupState};
use eez_deriver::Deriver;
use eez_driver::{
    BlockCommitterHandle, EthAttributesBuilder, RollupTiming, Sequencer, SyncSlotComposerHandle,
    spawn_l1_anchored,
};
use eez_l1::{
    L1CanonicalHead, L1HeadStream, L1Watcher, L1WatcherConfig, Submitter, SubmitterConfig,
};
use eez_node_common::{
    EezPayloadBuilder, EezPoolBuilder, L2NodeBuilder, NoRoleArgs, correlation_rpc,
    correlation_store, node_cli_with_chain_document, read_checkpoint_dir, validate_system_address,
    wait_for_l1_ready, warn_on_deprecated_env,
};
use reth_chainspec::EthChainSpec as _;
use reth_ethereum_cli::chainspec::EthereumChainSpecParser;
use reth_node_builder::components::BasicPayloadServiceBuilder;
use reth_node_ethereum::EthereumNode;
use reth_provider::CanonStateSubscriptions as _;
use tokio::sync::mpsc;
use tracing::{Level, event};

const BOOT_CATCH_UP_INITIAL_RETRY_DELAY: Duration = Duration::from_secs(2);
const BOOT_CATCH_UP_MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
/// ~15 min at the capped backoff: outlasts a restarting L1, but a permanently
/// refused RPC call still surfaces as an exit.
const BOOT_CATCH_UP_MAX_TRANSPORT_FAILURES: u32 = 32;
const L1_CHAIN_ID_READ_TIMEOUT: Duration = Duration::from_secs(30);
const L2_SYSTEM_TX_GAS_PRICE: u128 = 1_000_000_000;
const L2_SYSTEM_TX_GAS_LIMIT: u64 = 2_000_000;

/// Witness-capture resources selected by the mandatory composer prover.
enum WitnessCapture {
    Remote {
        sender: mpsc::UnboundedSender<eez_l1::L2Endpoint>,
        receiver: mpsc::UnboundedReceiver<eez_l1::L2Endpoint>,
        store: witness_source::WitnessStore,
        prover: eez_prover_client::RemoteProver,
    },
}

struct ComposerProving {
    maximum_blocks: u64,
    prover: Arc<dyn eez_prover::Prover>,
    witness_capture: WitnessCapture,
}

impl WitnessCapture {
    fn sender(&self) -> mpsc::UnboundedSender<eez_l1::L2Endpoint> {
        match self {
            Self::Remote { sender, .. } => sender.clone(),
        }
    }
}

/// Embedded L1 reth handle — owned by `main` for the node lifetime so
/// the L1 stays alive (drop tears it down). Generic over both variants'
/// `NodeHandle` params because the AddOns type differs between
/// EthereumNode (Devnet/Testing) and GnosisNode (Chiado).
///
/// Downstream matches `.as_ref()` for the chain_spec / provider /
/// evm_config to build the cross-chain composer; the Chiado provider
/// goes through `eez_composer::GnosisL1Adapter` to translate
/// `GnosisHeader → alloy_consensus::Header` on each read.
enum EmbeddedL1<Ethereum, Chiado> {
    Ethereum(Ethereum),
    Chiado(Chiado),
}

/// Launch a production composer node.
pub fn run_composer() -> eyre::Result<()> {
    let (cli, document_digest) = node_cli_with_chain_document::<NoRoleArgs>()?;
    cli.run(move |builder, ext| async move {
        let document_digest = document_digest.ok_or_else(|| {
            eyre::eyre!("composer startup is missing its loaded chain document identity")
        })?;
        Box::pin(launch_composer(builder, ext, document_digest)).await
    })
}

async fn composer_proving_from_env(expected_identity: B256) -> eyre::Result<ComposerProving> {
    let url = env::var("EEZ_PROVER_URL")
        .map_err(|_| eyre::eyre!("EEZ_PROVER_URL is required for composer proving"))?;
    if url.trim().is_empty() {
        return Err(eyre::eyre!("EEZ_PROVER_URL must not be empty"));
    }
    let attester = env::var("EEZ_ATTESTER_ADDRESS")
        .map_err(|_| eyre::eyre!("EEZ_ATTESTER_ADDRESS is required for composer proving"))?;
    let attester =
        Address::from_str(attester.trim()).map_err(|e| eyre::eyre!("EEZ_ATTESTER_ADDRESS: {e}"))?;
    let remote = eez_prover_client::RemoteProver::new(url, attester, expected_identity);
    let capabilities = remote.capabilities().await?;
    event!(Level::INFO, validation_identity = %expected_identity,
        maximum_blocks = capabilities.max_window_blocks,
        maximum_request_bytes = capabilities.max_request_bytes,
        maximum_witness_items = capabilities.max_request_witness_items,
        preparation_max_blocks = capabilities.preparation_max_blocks,
        cache_max_entries = capabilities.cache_max_entries,
        cache_max_bytes = capabilities.cache_max_bytes,
        maximum_leases = capabilities.maximum_leases,
        lease_lifetime_millis = capabilities.lease_lifetime_millis,
        "prover configuration agreement established");
    let (sender, receiver) = mpsc::unbounded_channel::<eez_l1::L2Endpoint>();
    let witness_db_path =
        env::var("EEZ_WITNESS_DB_PATH").unwrap_or_else(|_| "eez-witnesses".to_owned());
    let store = witness_source::new_store(std::path::Path::new(&witness_db_path))?;
    event!(
        name: "eez.node.witness_store.opened",
        Level::INFO,
        path = %witness_db_path,
        "persistent witness store opened",
    );
    Ok(ComposerProving {
        maximum_blocks: capabilities.max_window_blocks,
        prover: Arc::new(remote.clone()),
        witness_capture: WitnessCapture::Remote {
            sender,
            receiver,
            store,
            prover: remote,
        },
    })
}

async fn run_preparation_worker(
    mut receiver: mpsc::Receiver<eez_l1::L2Endpoint>,
    notifier: witness_source::PreparationNotifier,
    store: witness_source::WitnessStore,
    prover: eez_prover_client::RemoteProver,
    rollup_id: u64,
    system_address: Address,
    l1_head: Arc<L1CanonicalHead>,
) {
    let mut last_prepared: Option<eez_l1::L2Endpoint> = None;
    let mut retained_settled = l1_head.policy_anchor_snapshot().endpoint;
    let mut lease_window_full = false;
    let renewal_interval = Duration::from_secs(30);
    let mut renewal_due = tokio::time::Instant::now() + renewal_interval;
    loop {
        let next = if last_prepared.is_some() {
            tokio::select! {
                next = receiver.recv() => next,
                () = tokio::time::sleep_until(renewal_due) => {
                    if l1_head.policy_anchor_snapshot().endpoint == retained_settled {
                        match prover.renew_preparation_lease().await {
                            Ok(true) => {
                                store.preparation_status(None);
                                renewal_due = tokio::time::Instant::now() + renewal_interval;
                                continue;
                            }
                            Ok(false) => {
                                // Expiry or signer restart cleared the client's stale
                                // hints. Rebuild the exact last durable endpoint even
                                // when no new block has committed.
                                lease_window_full = false;
                                renewal_due = tokio::time::Instant::now() + renewal_interval;
                                last_prepared.take()
                            }
                            Err(error) => {
                                store.preparation_status(Some(error.to_string()));
                                event!(name: "eez.node.witness_preparation.retry", Level::WARN,
                                    %error, "preparation lease renewal failed");
                                renewal_due = tokio::time::Instant::now() + Duration::from_secs(1);
                                continue;
                            }
                        }
                    } else {
                        // Re-enter the main loop with the last known durable
                        // endpoint so canonical advancement releases the old
                        // lease even when no new L2 block is produced.
                        last_prepared
                    }
                }
            }
        } else {
            receiver.recv().await
        };
        let Some(mut target) = next else {
            break;
        };
        // Collapse queued identities to the newest endpoint. The exact durable
        // ancestry walk below reconstructs every skipped block without keeping
        // a second witness copy in this channel.
        while let Ok(queued) = receiver.try_recv() {
            target = queued;
        }
        if let Some(watermark) = notifier.take_watermark() {
            target = watermark;
        }
        loop {
            if last_prepared.is_some() && tokio::time::Instant::now() >= renewal_due {
                match prover.renew_preparation_lease().await {
                    Ok(true) => {
                        store.preparation_status(None);
                        renewal_due = tokio::time::Instant::now() + renewal_interval;
                    }
                    Ok(false) => {
                        last_prepared = None;
                        lease_window_full = false;
                        renewal_due = tokio::time::Instant::now() + renewal_interval;
                    }
                    Err(error) => {
                        store.preparation_status(Some(error.to_string()));
                        event!(name: "eez.node.witness_preparation.retry", Level::WARN,
                            %error, "preparation lease renewal failed");
                        renewal_due = tokio::time::Instant::now() + Duration::from_secs(1);
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                }
            }
            let settled = l1_head.policy_anchor_snapshot().endpoint;
            if settled != retained_settled {
                if let Err(error) = prover.release_preparation_lease().await {
                    store.preparation_status(Some(error.to_string()));
                    event!(name: "eez.node.witness_preparation.retry", Level::WARN,
                        canonical_settled = ?settled, %error,
                        "canonical advancement lease release failed");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                retained_settled = settled;
                last_prepared = None;
                lease_window_full = false;
                renewal_due = tokio::time::Instant::now() + renewal_interval;
            }
            let floor = settled;
            if last_prepared == Some(target) {
                store.preparation_status(None);
                break;
            }
            let source = Arc::clone(&store);
            let stop = last_prepared;
            let result =
                tokio::task::spawn_blocking(move || preparation_path(&source, target, stop, floor))
                    .await;
            let path = match result {
                Ok(Ok(path)) => path,
                Ok(Err(error @ PreparationPathError::Missing { .. })) => {
                    // Reconciliation can replace a terminal block with an
                    // applied prefix whose exact parent has no standalone
                    // witness. Do not deadlock that reconciliation by making
                    // a speculative prewarm miss globally unhealthy. If a
                    // final proof actually needs this block, the authoritative
                    // witness source still fails that request explicitly.
                    store.preparation_status(None);
                    event!(name: "eez.node.witness_preparation.retry", Level::WARN,
                        number = target.number, hash = %target.hash, %error,
                        "durable preparation predecessor is not yet available; retrying after reconciliation");
                    while let Ok(queued) = receiver.try_recv() {
                        target = queued;
                    }
                    if let Some(watermark) = notifier.take_watermark() {
                        target = watermark;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                Ok(Err(error)) => {
                    let error = error.to_string();
                    store.preparation_status(Some(error.clone()));
                    event!(name: "eez.node.witness_preparation.retry", Level::ERROR,
                        number = target.number, hash = %target.hash, %error,
                        "durable preparation path failed; admission paused");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                Err(error) => {
                    store.preparation_status(Some(error.to_string()));
                    event!(name: "eez.node.witness_preparation.retry", Level::ERROR,
                        number = target.number, hash = %target.hash, %error,
                        "preparation path worker failed; admission paused");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            if last_prepared.is_some() && !path.connected_to_stop {
                if let Err(error) = prover.release_preparation_lease().await {
                    store.preparation_status(Some(error.to_string()));
                    event!(name: "eez.node.witness_preparation.retry", Level::WARN,
                        number = target.number, hash = %target.hash, %error,
                        "canonical preparation branch changed; stale lease release failed");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                last_prepared = None;
                lease_window_full = false;
                renewal_due = tokio::time::Instant::now() + renewal_interval;
            }
            let prepared_endpoint = path.blocks.last().map(|block| eez_l1::L2Endpoint {
                number: block.number,
                hash: block.hash,
            });
            if lease_window_full && path.connected_to_stop {
                // The current lease already reserves the oldest maximum-sized
                // prefix. Newer descendants belong to the next settlement
                // window and must wait for the authenticated floor to advance.
                store.preparation_status(None);
                while let Ok(queued) = receiver.try_recv() {
                    target = queued;
                }
                if let Some(watermark) = notifier.take_watermark() {
                    target = watermark;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            let mut failure = None;
            for segment in path
                .blocks
                .split(|block| !ordinary_preparable(block, system_address))
            {
                if let Err(error) = prover.prepare_ordinary_range(rollup_id, segment).await {
                    failure = Some(error.to_string());
                }
                if failure.is_some() {
                    break;
                }
            }
            if let Some(error) = failure {
                store.preparation_status(Some(error.clone()));
                event!(name: "eez.node.witness_preparation.retry", Level::WARN,
                    number = target.number, hash = %target.hash, %error,
                    "signer preparation failed; durable witnesses retained for retry");
                // Probe the lease immediately on the next iteration. Cache
                // publication reports both capacity and a restart-lost lease as
                // resource exhaustion; exact renewal distinguishes them without
                // weakening either failure.
                renewal_due = tokio::time::Instant::now();
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            store.preparation_status(None);
            if let Some(prepared_endpoint) = prepared_endpoint {
                last_prepared = Some(prepared_endpoint);
                renewal_due = tokio::time::Instant::now() + renewal_interval;
            }
            if last_prepared == Some(target) || target.number <= floor.map_or(0, |f| f.number) {
                lease_window_full = false;
                break;
            }
            lease_window_full = true;
            // One reservation can cover exactly one maximum ordinary window.
            // Keep the oldest prefix ready for the next historical settlement,
            // then observe floor advancement before replacing it with the next
            // prefix. Continue tracking the newest durable target while waiting.
            while let Ok(queued) = receiver.try_recv() {
                target = queued;
            }
            if let Some(watermark) = notifier.take_watermark() {
                target = watermark;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

fn ordinary_preparable(block: &eez_prover::BlockWitness, system_address: Address) -> bool {
    use reth_primitives_traits::SignerRecoverable as _;
    let Ok(block) = alloy_rlp::decode_exact::<reth_ethereum_primitives::Block>(&block.rlp) else {
        return true;
    };
    block.body.transactions.iter().all(|transaction| {
        transaction
            .recover_signer()
            .is_ok_and(|signer| signer != system_address)
    })
}

#[derive(Debug)]
enum PreparationPathError {
    Missing { number: u64, hash: B256 },
    Invalid(String),
}

impl std::fmt::Display for PreparationPathError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { number, hash } => {
                write!(formatter, "durable witness {number}/{hash} is missing")
            }
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

fn preparation_path(
    store: &witness_source::WitnessDb,
    target: eez_l1::L2Endpoint,
    stop: Option<eez_l1::L2Endpoint>,
    canonical_floor: Option<eez_l1::L2Endpoint>,
) -> Result<PreparationPath, PreparationPathError> {
    preparation_path_with_limit(
        store,
        target,
        stop,
        canonical_floor,
        eez_payload_codec::derivable::MAX_BLOCKS - 1,
    )
}

#[derive(Debug)]
struct PreparationPath {
    blocks: Vec<eez_prover::BlockWitness>,
    connected_to_stop: bool,
}

fn preparation_path_with_limit(
    store: &witness_source::WitnessDb,
    target: eez_l1::L2Endpoint,
    stop: Option<eez_l1::L2Endpoint>,
    canonical_floor: Option<eez_l1::L2Endpoint>,
    maximum_blocks: usize,
) -> Result<PreparationPath, PreparationPathError> {
    if maximum_blocks == 0 {
        return Err(PreparationPathError::Invalid(
            "preparation block limit must be nonzero".into(),
        ));
    }
    let floor_number = canonical_floor.map_or(0, |floor| floor.number);
    if target.number <= floor_number {
        return Ok(PreparationPath {
            blocks: Vec::new(),
            connected_to_stop: stop.is_none(),
        });
    }
    let mut number = target.number;
    let mut hash = target.hash;
    let mut reversed = VecDeque::with_capacity(maximum_blocks);
    let mut connected_to_stop = stop.is_none();
    loop {
        let witness = store
            .get(number, hash)
            .map_err(|error| PreparationPathError::Invalid(error.to_string()))?
            .ok_or(PreparationPathError::Missing { number, hash })?;
        let parent = witness.parent_hash;
        if reversed.len() == maximum_blocks {
            reversed.pop_front();
        }
        reversed.push_back(witness);
        let reached_floor = canonical_floor.is_some_and(|floor| {
            floor.number.checked_add(1) == Some(number) && floor.hash == parent
        }) || (canonical_floor.is_none() && number == 1);
        let reached_stop = stop
            .is_some_and(|stop| stop.number.checked_add(1) == Some(number) && stop.hash == parent);
        if reached_stop {
            connected_to_stop = true;
        }
        if reached_floor || reached_stop {
            break;
        }
        number = number.checked_sub(1).ok_or_else(|| {
            PreparationPathError::Invalid("preparation ancestry underflow".into())
        })?;
        hash = parent;
    }
    Ok(PreparationPath {
        blocks: reversed.into_iter().rev().collect(),
        connected_to_stop,
    })
}

#[allow(clippy::too_many_lines)]
async fn launch_composer(
    builder: L2NodeBuilder,
    _ext: NoRoleArgs,
    chain_document_digest: B256,
) -> eyre::Result<()> {
    let metrics_enabled = builder.config().metrics.prometheus.is_some();
    event!(
        name: "eez.node.launching",
        Level::INFO,
        mode = "composer",
        "launching eez composer",
    );

    warn_on_deprecated_env();
    let timing = RollupTiming::from_env()?;
    let composer_settings = ComposerSettings::from_env(timing)?;
    let rollup_config = RollupConfig::from_env()?;
    let depth = composer_settings.max_speculative_depth();
    let system_key = env::var("EEZ_L2_SYSTEM_KEY")
        .map_err(|_| eyre::eyre!("EEZ_L2_SYSTEM_KEY required in composer mode"))?;
    let system_signer =
        PrivateKeySigner::from_bytes(&B256::from_str(system_key.trim_start_matches("0x"))?)?;
    let system_address = system_signer.address();
    validate_system_address(system_address)?;
    let expected_identity = eez_control_rpc::identity::ValidationContext {
        rollup_id: std::num::NonZeroU64::new(rollup_config.rollup_id)
            .ok_or_else(|| eyre::eyre!("rollup ID must be nonzero"))?,
        chain_id: builder.config().chain.chain().id(),
        system_address,
        eez_l2_address: eez_protocol::EEZL2_ADDRESS,
        chain_document_digest,
        canonical_json_version: eez_l1::chain_config::CANONICAL_JSON_VERSION,
    }
    .identity();
    // Agreement is established before either node starts sequencing work.
    let ComposerProving {
        maximum_blocks,
        prover,
        witness_capture,
    } = composer_proving_from_env(expected_identity).await?;
    let composer_settings = composer_settings.with_prover_block_limit(maximum_blocks)?;
    // Launch the embedded L1 reth first in composer mode — its
    // `StateProviderFactory` backs `LocalChainClient::new_entry` for
    // L1 source-tx simulation. Inline (not in `l1_embedded.rs`)
    // because the `NodeHandle` AddOns type resists a typed return.
    // Shared L1-reth tokio runtime — built once, used by whichever
    // L1 path runs.
    let build_l1_runtime = || {
        reth_tasks::RuntimeBuilder::new(reth_tasks::RuntimeConfig::default().with_tokio(
            reth_tasks::TokioConfig::existing_handle(tokio::runtime::Handle::current()),
        ))
        .build()
        .map_err(|e| eyre::eyre!("L1 embedded RuntimeBuilder: {e}"))
    };
    // Testing = vanilla EthereumNode (5s auto-mine); Devnet = vanilla
    // EthereumNode + external CL; Chiado = reth_gnosis::GnosisNode +
    // external CL, its provider wrapped by `GnosisL1Adapter` for the
    // alloy-Header bound. Returns an `EmbeddedL1` owning the NodeHandle so
    // the L1 outlives the node.
    let embedded_l1: EmbeddedL1<_, _> = {
        let l1_cfg = build_embedded_l1_config()?;
        match l1_cfg.kind {
            l1_embedded::L1ChainKind::Devnet => {
                let node_cfg = l1_embedded::build_devnet_node_config(&l1_cfg)?;
                let db = reth_db::init_db(
                    node_cfg.datadir().db(),
                    reth_db::mdbx::DatabaseArguments::default(),
                )
                .map_err(|e| eyre::eyre!("L1 embedded init_db: {e}"))?;
                event!(
                    name: "eez.node.l1_embedded.launching",
                    Level::INFO,
                    kind = "devnet",
                    http_port = l1_cfg.http_port,
                    auth_port = l1_cfg.auth_port,
                    "launching embedded L1 reth (devnet); external consensus client must connect to authrpc",
                );
                let l1_handle = reth_node_builder::NodeBuilder::new(node_cfg)
                    .with_database(db)
                    .with_launch_context(build_l1_runtime()?)
                    .node(EthereumNode::default())
                    .launch_with_debug_capabilities()
                    .await?;
                event!(
                    name: "eez.node.l1_embedded.ready",
                    Level::INFO,
                    kind = "devnet",
                    l1_chain_id = %l1_handle.node.chain_spec().chain(),
                    "embedded L1 reth (devnet) ready",
                );
                EmbeddedL1::Ethereum(l1_handle)
            }
            l1_embedded::L1ChainKind::Testing => {
                let node_cfg = l1_embedded::build_testing_node_config(&l1_cfg)?;
                let db = reth_db::init_db(
                    node_cfg.datadir().db(),
                    reth_db::mdbx::DatabaseArguments::default(),
                )
                .map_err(|e| eyre::eyre!("L1 embedded init_db: {e}"))?;
                event!(
                    name: "eez.node.l1_embedded.launching",
                    Level::INFO,
                    kind = "testing",
                    http_port = l1_cfg.http_port,
                    "launching embedded L1 reth (testing)",
                );
                let l1_handle = reth_node_builder::NodeBuilder::new(node_cfg)
                    .with_database(db)
                    .with_launch_context(build_l1_runtime()?)
                    .node(EthereumNode::default())
                    .extend_rpc_modules(bundle_rpc::install_dev_bundle_rpc)
                    .launch_with_debug_capabilities()
                    .await?;
                event!(
                    name: "eez.node.l1_embedded.ready",
                    Level::INFO,
                    kind = "testing",
                    l1_chain_id = %l1_handle.node.chain_spec().chain(),
                    "embedded L1 reth (testing) ready",
                );
                EmbeddedL1::Ethereum(l1_handle)
            }
            l1_embedded::L1ChainKind::Chiado | l1_embedded::L1ChainKind::TestingGnosis => {
                let node_cfg = l1_embedded::build_chiado_node_config(&l1_cfg)?;
                let db = reth_db::init_db(
                    node_cfg.datadir().db(),
                    reth_db::mdbx::DatabaseArguments::default(),
                )
                .map_err(|e| eyre::eyre!("L1 chiado init_db: {e}"))?;
                event!(
                    name: "eez.node.l1_embedded.launching",
                    Level::INFO,
                    kind = "chiado",
                    http_port = l1_cfg.http_port,
                    auth_port = l1_cfg.auth_port,
                    datadir = ?l1_cfg.datadir,
                    "launching embedded L1 reth (chiado via reth_gnosis::GnosisNode); \
                     external lighthouse CL must connect to authrpc",
                );
                let testing = l1_cfg.kind == l1_embedded::L1ChainKind::TestingGnosis;
                let chiado_handle = reth_node_builder::NodeBuilder::new(node_cfg)
                    .with_database(db)
                    .with_launch_context(build_l1_runtime()?)
                    .node(reth_gnosis::GnosisNode::default())
                    .extend_rpc_modules(move |ctx| {
                        if testing {
                            bundle_rpc::install_dev_bundle_rpc(ctx)?;
                        }
                        Ok(())
                    })
                    .launch_with_debug_capabilities()
                    .await?;
                event!(
                    name: "eez.node.l1_embedded.ready",
                    Level::INFO,
                    kind = "chiado",
                    l1_chain_id = %chiado_handle.node.chain_spec().inner.chain(),
                    "embedded L1 reth (chiado) ready — cross-chain composer \
                     wraps the provider via eez_composer::GnosisL1Adapter",
                );
                EmbeddedL1::Chiado(chiado_handle)
            }
        }
    };

    // L2 reth. `EezPayloadBuilder` writes `gas_limit`/`extra_data` from
    // shared `eez-driver` constants so deriver replay and sequencer builds
    // yield identical headers.
    let l2_data_dir = read_checkpoint_dir();
    let correlations = correlation_store(l2_data_dir.as_deref())?;
    // Shared L1-confirmed L2 head. The Deriver is the sole writer; the
    // Sequencer and correlation RPC read it.
    let l1_head = Arc::new(L1CanonicalHead::default());
    let rpc_correlations = Arc::clone(&correlations);
    let rpc_l1_head = Arc::clone(&l1_head);
    // The L2 pool and ingress/composer consult the same source ownership.
    // Reorged cross-chain users require a Sync block, not ordinary sequencing.
    let optimistic = Arc::new(eez_composer::OptimisticallyIncluded::new());
    let held_pool = Arc::new(HeldPool::new(Arc::clone(&optimistic)));
    let reserved_sources = Arc::clone(&held_pool);
    let rpc_held_pool = Arc::clone(&held_pool);
    // Every composer needs durable-witness backpressure.
    let data_guard = Some(Arc::new(eez_node_common::SequencingDataGuard::new(
        rollup_config.rollup_id,
    )));
    let handle = builder
        .with_types::<EthereumNode>()
        .with_components(
            EthereumNode::components()
                // Reorged-out system transactions must not leak from reth's
                // reinjection path into an ordinary Live block.
                .pool(
                    EezPoolBuilder::new(system_address)
                        .with_data_guard(data_guard.clone())
                        .with_cross_chain_filter(move |hash| {
                            reserved_sources.is_cross_chain_source(hash)
                        }),
                )
                .payload(BasicPayloadServiceBuilder::new(
                    EezPayloadBuilder::default().with_data_guard(data_guard.clone()),
                )),
        )
        .with_add_ons(reth_node_ethereum::node::EthereumAddOns::default())
        .extend_rpc_modules(move |ctx| {
            ctx.modules
                .merge_configured(queue_rpc::module(Arc::clone(&rpc_held_pool))?)?;
            correlation_rpc::install(ctx, Arc::clone(&rpc_correlations), Arc::clone(&rpc_l1_head))
        })
        .launch_with_debug_capabilities()
        .await?;

    let chain_spec: Arc<_> = handle.node.chain_spec();
    let l2_genesis_timestamp = chain_spec.genesis().timestamp;
    let provider = handle.node.provider.clone();
    let payload_builder_handle = handle.node.payload_builder_handle.clone();
    let beacon_engine_handle = handle.node.add_ons_handle.beacon_engine_handle.clone();
    let task_executor = handle.node.task_executor.clone();
    if metrics_enabled {
        queue_metrics::spawn(
            Arc::clone(&held_pool),
            rollup_config.rollup_id,
            &task_executor,
        );
    }

    let attributes = EthAttributesBuilder::new(chain_spec.clone());

    let block_committer = BlockCommitterHandle::spawn_from_provider(
        &provider,
        beacon_engine_handle,
        payload_builder_handle,
        Some(witness_capture.sender()),
    )?;
    let restart_head = block_committer.last_header();
    if restart_head.number > 0 {
        witness_capture
            .sender()
            .send(eez_l1::L2Endpoint {
                number: restart_head.number,
                hash: restart_head.hash(),
            })
            .map_err(|_| eyre::eyre!("durable witness capture channel closed at startup"))?;
    }
    let submitter_config = SubmitterConfig::from_env()?;
    let deploy_block = submitter_config.reader.deploy_block;
    let l1_watcher_config = L1WatcherConfig::from_env()?;

    let posting_l1_spec = match &embedded_l1 {
        EmbeddedL1::Ethereum(handle) => handle.node.chain_spec(),
        EmbeddedL1::Chiado(handle) => Arc::new(handle.node.chain_spec().inner.clone()),
    };
    let blob_rules: eez_l1::BlobRuleSource =
        Arc::new(move |timestamp| posting_l1_spec.blob_params_at_timestamp(timestamp));
    let submitter = Submitter::new_with_blob_rules(
        submitter_config,
        timing.l1_block_time(),
        timing.submission_slack(),
        Arc::clone(&blob_rules),
    )?;
    // Handle only — polling starts after boot catch-up fixes the
    // seed and every consumer has subscribed.
    let l1_watcher = L1Watcher::new_with_blob_rules(l1_watcher_config, blob_rules);
    // Policy catch-up needs canonical settlement evidence before the next
    // proof cutoff. Wake the existing scanner as soon as the embedded L1
    // changes, retaining periodic HTTP scans for retries and recovery.
    match &embedded_l1 {
        EmbeddedL1::Ethereum(handle) => task_executor.spawn_task(
            l1_watcher
                .clone()
                .wake_on_canonical_changes(handle.node.provider.subscribe_to_canonical_state()),
        ),
        EmbeddedL1::Chiado(handle) => task_executor.spawn_task(
            l1_watcher
                .clone()
                .wake_on_canonical_changes(handle.node.provider.subscribe_to_canonical_state()),
        ),
    };

    // Build the Composer and Sequencer around the same binary-owned
    // BlockCommitter handle. The Deriver receives another clone below, so all
    // engine traffic shares one actor, reconcile lock, and head mirror.
    let (sequencer, composer, held_pool, system_tx_cfg, l1_source_chain_id, preparation_worker) = {
        let rollup_id = rollup_config.rollup_id;
        let l1_variant = &embedded_l1;
        let l1_source_chain_id = match l1_variant {
            EmbeddedL1::Ethereum(l1_handle) => l1_handle.node.chain_spec().chain().id(),
            EmbeddedL1::Chiado(chiado_handle) => chiado_handle.node.chain_spec().inner.chain().id(),
        };
        // Share the SAME HeldPool the ingress middleware pushes into.
        let rollup_state = RollupState {
            config: rollup_config.clone(),
            l2_provider: Arc::new(provider.clone()),
            l1_head: Arc::clone(&l1_head),
            held_pool: Arc::clone(&held_pool),
            optimistic,
        };
        let mut rollups = HashMap::with_capacity(1);
        rollups.insert(rollup_id, rollup_state);
        // EVM config feeds the manual Sync-block construction path
        // (build_sync_block → reth-evm BlockBuilder). Same chain spec
        // the engine uses, so blocks produced here pass validation
        // when reth ingests them via newPayload.
        let evm_config = reth_evm_ethereum::EthEvmConfig::new(chain_spec.clone());

        // Build the cross-chain composer over the mandatory embedded L1.
        // Inlined because the `FullNode` AddOns type resists a typed helper
        // return.
        use eez_composer::{GnosisL1Adapter, LocalChainClient};
        use eez_protocol::rollup_id::RollupId;
        use eez_protocol::{ProxyLookupConfig, TargetConfig};

        let eez_registry: Address =
            Address::from_str(&env::var("EEZ_REGISTRY_ADDRESS").map_err(|_| {
                eyre::eyre!(
                    "EEZ_REGISTRY_ADDRESS required for the cross-chain composer (set by deploy.sh)"
                )
            })?)?;

        let eezl2_address: Address =
            Address::from_str(&env::var("EEZL2_ADDRESS").map_err(|_| {
                eyre::eyre!(
                    "EEZL2_ADDRESS required for the cross-chain composer (set by deploy.sh)"
                )
            })?)?;
        let l1_rollup_id_u64 = read_l1_rollup_id()?;
        let l1_rollup_id = RollupId(l1_rollup_id_u64);
        let l2_rollup_id_typed = RollupId(rollup_id);

        // L1 entry differs per kind: Devnet/Testing use the native
        // provider + EvmConfig; Chiado wraps it in
        // `GnosisL1Adapter` and builds a fresh `EthEvmConfig`
        // over the chiado ChainSpec (source-sim needs only
        // revm, not GnosisNode's AuRa paths). Both yield the
        // same concrete client type, so composition is identical.
        let l1_entry_client: Arc<LocalChainClient> = match l1_variant {
            EmbeddedL1::Ethereum(l1_handle) => {
                let l1_provider = l1_handle.node.provider.clone();
                let l1_evm_config = l1_handle.node.evm_config.clone();
                LocalChainClient::new_entry(
                    l1_provider,
                    l1_evm_config,
                    l1_rollup_id,
                    eez_registry,
                    eez_protocol::ChainDialect::EvmL1Style,
                )
            }
            EmbeddedL1::Chiado(chiado_handle) => {
                let l1_provider = GnosisL1Adapter::new(chiado_handle.node.provider.clone());
                LocalChainClient::new_gnosis_entry(
                    l1_provider,
                    chiado_handle.node.evm_config.clone(),
                    l1_rollup_id,
                    eez_registry,
                    eez_protocol::ChainDialect::EvmL1Style,
                )
            }
        };
        let entry_client_view: Arc<dyn eez_protocol::executor::ChainClient + Send + Sync> =
            l1_entry_client.clone();

        // L2 follower — EvmL2Style. Its dispatch contract is the
        // `EEZL2` predeploy, whose inherited `authorizedProxies`
        // mapping occupies slot 0.
        let l2_follower = LocalChainClient::new_follower(
            provider.clone(),
            evm_config.clone(),
            l2_rollup_id_typed,
            eezl2_address,
            eez_protocol::ChainDialect::EvmL2Style,
        );
        let l2_follower_view: std::sync::Arc<
            dyn eez_protocol::executor::ChainClient + Send + Sync,
        > = l2_follower.clone();

        // L2 ENTRY client (follower's provider/dialect, but
        // Role::Entry) — the follower client errors `Unavailable` for
        // the outbound source-sim `simulate_and_resolve_recorded_for`.
        let l2_entry_client = LocalChainClient::new_entry_sharing_overlay(
            provider.clone(),
            evm_config.clone(),
            l2_rollup_id_typed,
            eezl2_address,
            eez_protocol::ChainDialect::EvmL2Style,
            l2_follower.as_ref(),
        );

        let entry_cfg = TargetConfig {
            proxy_lookup: ProxyLookupConfig {
                contract_address: eez_registry,
                authorized_proxies_slot: eez_protocol::ChainDialect::EvmL1Style.proxy_lookup_slot(),
            },
            dialect: eez_protocol::ChainDialect::EvmL1Style,
        };
        let l2_follower_cfg = TargetConfig {
            proxy_lookup: ProxyLookupConfig {
                contract_address: eezl2_address,
                authorized_proxies_slot: eez_protocol::ChainDialect::EvmL2Style.proxy_lookup_slot(),
            },
            dialect: eez_protocol::ChainDialect::EvmL2Style,
        };

        let mut wired_rollups = std::collections::HashMap::new();
        wired_rollups.insert(l1_rollup_id, (entry_client_view, entry_cfg));
        // A colliding id would silently overwrite the L1 entry
        // registration (EEZ_L1_ROLLUP_ID defaults to 0).
        if wired_rollups
            .insert(l2_rollup_id_typed, (l2_follower_view, l2_follower_cfg))
            .is_some()
        {
            return Err(eyre::eyre!(
                "duplicate rollup id {l2_rollup_id_typed}: EEZ_ROLLUP_ID collides \
                     with EEZ_L1_ROLLUP_ID; the L2 follower registration would \
                     overwrite the L1 entry"
            ));
        }
        // CrossChainExecCtx: signer + L2 addresses needed to wrap
        // EvmComposer's `(load_table, execute)` calldata pairs
        // into signed legacy L2 system txs at Sync-slot time.
        // Constructed only when EvmComposer is constructed —
        // both are tied to embedded L1 mode.
        // Submission RPC for postBatch and inbound source-chain reads.
        // This can differ from the embedded L1 used for local source
        // simulation (the E2E harness deliberately uses Anvil here), so
        // derive the signing chain ID from this provider rather than the
        // embedded chain spec.
        let l1_rpc_url: reqwest::Url = env::var("EEZ_L1_RPC_URL")
            .map_err(|_| eyre::eyre!("EEZ_L1_RPC_URL required for L1 forwarding"))?
            .parse()
            .map_err(|e| eyre::eyre!("EEZ_L1_RPC_URL malformed: {e}"))?;
        let l1_provider = alloy_provider::RootProvider::new_http(l1_rpc_url.clone());
        let l1_submission_chain_id =
            tokio::time::timeout(L1_CHAIN_ID_READ_TIMEOUT, l1_provider.get_chain_id())
                .await
                .map_err(|_| eyre::eyre!("timed out reading chain id from EEZ_L1_RPC_URL"))?
                .map_err(|e| eyre::eyre!("read chain id from EEZ_L1_RPC_URL: {e}"))?;
        let l1_poster_key = env::var("EEZ_L1_POSTER_KEY")
            .map_err(|_| eyre::eyre!("EEZ_L1_POSTER_KEY required for L1 postBatch signing"))?;
        let l1_poster_signer =
            PrivateKeySigner::from_bytes(&B256::from_str(l1_poster_key.trim_start_matches("0x"))?)?;
        let ecdsa_proof_system_address: Address =
            Address::from_str(&env::var("EEZ_ECDSA_PROOF_SYSTEM_ADDRESS").map_err(|_| {
                eyre::eyre!(
                    "EEZ_ECDSA_PROOF_SYSTEM_ADDRESS required for L1 postBatch \
                         proofSystems[0]"
                )
            })?)?;
        let blob_sidecar_encoding =
            match env::var("EEZ_L1_BLOB_SIDECAR_ENCODING")
                .map_err(|_| {
                    eyre::eyre!(
                        "EEZ_L1_BLOB_SIDECAR_ENCODING is required (eip4844 or eip7594); native blob DA is mandatory"
                    )
                })?
                .as_str()
            {
                "eip4844" => eez_blob_codec::BlobSidecarEncoding::Eip4844,
                "eip7594" => eez_blob_codec::BlobSidecarEncoding::Eip7594,
            value => {
                return Err(eyre::eyre!(
                    "EEZ_L1_BLOB_SIDECAR_ENCODING must be eip4844 or eip7594, got {value}"
                ));
            }
        };
        // One submission path everywhere: the composer always
        // routes `[postBatch, user_tx_1, …]` through the shared
        // Submitter. The Submitter owns the transport decision —
        // `eth_sendBundle` on relays that support it (rbuilder),
        // ordered mempool submission on plain execution RPCs
        // (dev reth, anvil) detected via JSON-RPC -32601.
        let exec_ctx = Arc::new(eez_composer::CrossChainExecCtx {
            system_signer,
            eezl2_address,
            l2_chain_id: chain_spec.chain().id(),
            l2_gas_price: L2_SYSTEM_TX_GAS_PRICE,
            l2_gas_limit: L2_SYSTEM_TX_GAS_LIMIT,
            l1_provider,
            submitter: submitter.clone(),
            l1_poster_signer,
            l1_chain_id: l1_submission_chain_id,
            blob_sidecar_encoding,
            ecdsa_proof_system_address,
        });
        event!(
            name: "eez.node.evm_composer.ready",
            Level::INFO,
            l1_rollup_id = l1_rollup_id_u64,
            l2_rollup_id = rollup_id,
            eez_registry = %eez_registry,
            %eezl2_address,
            "cross-chain composer constructed (L1 entry + L2 follower)",
        );
        let cross_chain = CrossChainWiring {
            entry_rollup_id: l1_rollup_id,
            rollups: wired_rollups,
            exec_ctx,
            local: eez_composer::LocalComposeClients {
                l1_entry: l1_entry_client,
                l2_entry: l2_entry_client,
            },
        };

        // Project the Arc<CrossChainExecCtx> into a SystemTxContext
        // BEFORE moving it into the Composer. The Deriver picks
        // this up further down to STF-reconstruct the same L2
        // system txs the composer produced.
        let deriver_system_tx_cfg = {
            let ctx = &cross_chain.exec_ctx;
            eez_protocol::system_tx::SystemTxContext {
                system_signer: ctx.system_signer.clone(),
                eezl2_address: ctx.eezl2_address,
                l2_chain_id: ctx.l2_chain_id,
                l2_gas_price: ctx.l2_gas_price,
                l2_gas_limit: ctx.l2_gas_limit,
                this_rollup_id: rollup_id,
            }
        };
        // Remote-prover mode: spawn the commit-time witness capture and back the
        // composer's witness source with its store. Capturing at commit (parent
        // state still fresh) is why this works on a non-archival node. Spawned
        // before `evm_config` is moved into `Composer::new` below.
        let (witness_source, preparation_worker) = match witness_capture {
            WitnessCapture::Remote {
                receiver: rx,
                store,
                prover: preparation_prover,
                ..
            } => {
                let cap_provider = provider.clone();
                let cap_evm = evm_config.clone();
                let ws_provider = provider.clone();
                let ws_evm = evm_config.clone();
                let cap_store = Arc::clone(&store);
                // Purge floor = L1-FINALIZED height (a reorg could un-settle a posted batch).
                let cap_l1_head = Arc::clone(&l1_head);
                let (preparation_notifier, preparation_rx) =
                    witness_source::PreparationNotifier::channel(128);
                let capture_notifier = preparation_notifier.clone();
                task_executor.spawn_critical_task("eez-witness-capture", async move {
                    witness_source::run_capture(
                        rx,
                        cap_store,
                        cap_provider,
                        cap_evm,
                        move || cap_l1_head.finalized_l2(),
                        Some(capture_notifier),
                    )
                    .await;
                });
                let preparation_store = Arc::clone(&store);
                let preparation_l1_head = Arc::clone(&l1_head);
                let preparation_worker = async move {
                    run_preparation_worker(
                        preparation_rx,
                        preparation_notifier,
                        preparation_store,
                        preparation_prover,
                        rollup_id,
                        system_address,
                        preparation_l1_head,
                    )
                    .await;
                };
                // Hybrid: read the store, else re-exec on demand the newest block
                // the async capture hasn't drained yet (state still retained; fast
                // for near-empty blocks).
                (
                    Some(Arc::new(witness_source::NodeWitnessSource::new(
                        store,
                        ws_provider,
                        ws_evm,
                    ))
                        as Arc<dyn eez_prover::ProvingWitnessSource>),
                    preparation_worker,
                )
            }
        };
        let composer = Composer::new(
            rollups,
            prover,
            evm_config,
            cross_chain,
            block_committer.clone(),
            witness_source,
            composer_settings,
        )?;
        let sync_slot_handle: SyncSlotComposerHandle = Arc::new(composer.clone());
        if let Some(guard) = &data_guard {
            guard.bind(&sync_slot_handle)?;
        }

        let schedule_rx = spawn_l1_anchored(
            L1HeadStream::from_watcher(&l1_watcher),
            timing,
            l2_genesis_timestamp,
        );
        let sequencer = Sequencer::composer(
            attributes,
            block_committer.clone(),
            schedule_rx,
            timing,
            rollup_id,
            sync_slot_handle,
            depth,
            Arc::clone(&l1_head),
        );
        (
            sequencer,
            composer,
            held_pool,
            deriver_system_tx_cfg,
            l1_source_chain_id,
            preparation_worker,
        )
    };

    let l2_source_chain_id = chain_spec.chain().id();
    // Resolve both required fronts now. Their upstream chain-id checks run
    // after the L1 readiness gate so the ports can bind while L1 catches up.
    let mut xchain_fronts = Vec::new();
    for spec in xchain_front_specs(l1_source_chain_id, l2_source_chain_id) {
        let (port, url, parsed) = read_xchain_front_config(spec.port_env, spec.url_env)?;
        let validation_provider = alloy_provider::RootProvider::new_http(parsed);
        let dispatch_address =
            Address::from_str(&env::var(spec.dispatch_address_env).map_err(|_| {
                eyre::eyre!(
                    "{} required for transaction classification on the cross-chain front",
                    spec.dispatch_address_env
                )
            })?)
            .map_err(|e| eyre::eyre!("{} malformed: {e}", spec.dispatch_address_env))?;
        xchain_fronts.push((spec, port, url, validation_provider, dispatch_address));
    }

    // Deriver: drives BlockCommitter from L1Events (follower +
    // composer). A wired `SystemTxContext` makes it reconstruct the
    // same L2 system txs the composer produced (single-source STF).
    let deriver = Deriver::new(
        block_committer.clone(),
        Arc::new(provider.clone()),
        submitter.reader(),
        chain_spec,
        deploy_block,
        Arc::clone(&l1_head),
        Arc::clone(&correlations),
        Some(system_tx_cfg),
        l2_data_dir,
    );

    // Bind before the L1 wait so port checks see a live front. Submissions are
    // refused until catch-up completes and the watcher owns subsequent blocks.
    let xchain_ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let xchain_checks: Vec<_> = xchain_fronts
        .iter()
        .map(|(spec, _, _, provider, _)| (spec.expected_source_chain_id, provider.clone()))
        .collect();
    for (spec, port, url, validation_provider, dispatch_address) in xchain_fronts {
        let pool = Arc::clone(&held_pool);
        let ready = Arc::clone(&xchain_ready);
        task_executor.spawn_critical_task(spec.task, async move {
            ingress::run_cross_chain_front(
                port,
                url,
                spec.direction,
                pool,
                validation_provider,
                spec.expected_source_chain_id,
                dispatch_address,
                ready,
            )
            .await
            .unwrap_or_else(|e| panic!("configured cross-chain front exited: {e:#}"));
        });
    }

    let l1_reader = submitter.reader();
    wait_for_l1_ready(&l1_reader, deploy_block, l1_source_chain_id).await?;
    for (expected_chain_id, provider) in &xchain_checks {
        ingress::validate_cross_chain_front(provider, *expected_chain_id).await?;
    }

    let mut catch_up_retry_delay = BOOT_CATCH_UP_INITIAL_RETRY_DELAY;
    let mut catch_up_attempts = 0_u64;
    let mut transport_failures = 0_u32;
    let (l1_seed_number, l1_seed_hash) = loop {
        let catch_up = deriver.catch_up_with_seed_retaining_pure_l2().await;
        match catch_up {
            Ok(seed) => {
                match composer.validate_settlement_policy_bootstrap().await {
                    Ok(()) => break seed,
                    Err(error) => {
                        event!(
                            name: "eez.node.settlement.bootstrap_not_ready",
                            Level::WARN,
                            event_name = "eez.node.settlement.bootstrap_not_ready",
                            %error,
                            "canonical settlement anchor unavailable; readiness remains closed before sequencing or posting"
                        );
                        tokio::time::sleep(catch_up_retry_delay).await;
                        // Repeat canonical catch-up as well: registration and
                        // external settlements can arrive while readiness waits.
                    }
                }
            }
            Err(err)
                if err.is_l1_transport() && {
                    transport_failures += 1;
                    transport_failures >= BOOT_CATCH_UP_MAX_TRANSPORT_FAILURES
                } =>
            {
                event!(
                    name: "eez.node.deriver.boot_catch_up.transport_exhausted",
                    Level::ERROR,
                    mode = "composer",
                    transport_failures,
                    error = %err,
                    "L1 transport kept failing during boot catch-up; the endpoint is likely refusing a call we need, not merely unreachable",
                );
                return Err(eyre::eyre!(
                    "boot-time deriver catch_up gave up after {transport_failures} L1 transport failures: {err}"
                ));
            }
            Err(err) if err.is_source_incomplete() || err.is_l1_transport() => {
                catch_up_attempts += 1;
                event!(
                    name: "eez.node.deriver.boot_catch_up.source_incomplete",
                    Level::WARN,
                    mode = "composer",
                    attempts = catch_up_attempts,
                    retry_delay_secs = catch_up_retry_delay.as_secs(),
                    error = %err,
                    "boot-time catch_up could not read all L1 source data yet; retrying before starting L1-active tasks",
                );
                tokio::time::sleep(catch_up_retry_delay).await;
                catch_up_retry_delay = Duration::from_secs(
                    catch_up_retry_delay
                        .as_secs()
                        .saturating_mul(2)
                        .min(BOOT_CATCH_UP_MAX_RETRY_DELAY.as_secs()),
                );
            }
            Err(err) => {
                event!(
                    name: "eez.node.deriver.boot_catch_up.failed",
                    Level::ERROR,
                    event_name = "eez.node.deriver.boot_catch_up.failed",
                    mode = "composer",
                    error = %err,
                    "boot-time catch_up failed; refusing to start L1-active tasks before reconciliation",
                );
                return Err(eyre::eyre!("boot-time deriver catch_up failed: {err}"));
            }
        }
    };
    composer.validate_settlement_policy_startup().await?;
    // Retained-history validation must succeed before this process acquires
    // proof-signer cache ownership. A rejected startup cannot release its
    // process-local lease ID and must not consume restart overlap capacity.
    task_executor.spawn_critical_task("eez-witness-preparation", preparation_worker);
    correlations.mark_ready();
    event!(
        name: "eez.node.deriver.spawned",
        Level::INFO,
        mode = "composer",
        initial_posted_through = deriver.cursor(),
        "spawning eez deriver",
    );
    let deriver_run = deriver.clone();
    let deriver_events = l1_watcher.subscribe();
    task_executor.spawn_critical_task("eez-deriver", async move {
        deriver_run.run(deriver_events).await;
    });

    event!(name: "eez.node.sequencer.spawned", Level::INFO, mode = "composer", "spawning eez sequencer (L1-anchored)");
    task_executor.spawn_critical_task("eez-sequencer", async move {
        sequencer.run().await;
    });

    event!(name: "eez.node.composer.spawned", Level::INFO, "spawning eez composer umbrella");
    let composer_events = l1_watcher.subscribe();
    task_executor.spawn_critical_task("eez-composer", async move {
        composer.run(composer_events).await;
    });

    // Start polling last: every consumer is subscribed and catch-up
    // covered [deploy_block, seed], so the watcher owns all blocks
    // strictly after its seed — no startup scan gap.
    task_executor.spawn_critical_task(
        "eez-l1-watcher",
        l1_watcher.polling(l1_seed_number, l1_seed_hash),
    );
    xchain_ready.store(true, std::sync::atomic::Ordering::Relaxed);

    handle.wait_for_node_exit().await
}

/// Read the L1 rollup id from env. Defaults to `0` to match the bridge
/// E2E fixture's `MAINNET_ROLLUP_ID` only when the variable is absent.
fn read_l1_rollup_id() -> eyre::Result<u64> {
    match env::var("EEZ_L1_ROLLUP_ID") {
        Ok(raw) => parse_l1_rollup_id(Some(&raw)),
        Err(env::VarError::NotPresent) => parse_l1_rollup_id(None),
        Err(env::VarError::NotUnicode(_)) => {
            Err(eyre::eyre!("EEZ_L1_ROLLUP_ID contains non-UTF-8 bytes"))
        }
    }
}

fn parse_l1_rollup_id(raw: Option<&str>) -> eyre::Result<u64> {
    let Some(raw) = raw else {
        return Ok(0);
    };
    raw.trim()
        .parse::<u64>()
        .map_err(|e| eyre::eyre!("EEZ_L1_ROLLUP_ID={raw:?} malformed: {e}"))
}

#[derive(Debug, Clone, Copy)]
struct XchainFrontSpec {
    port_env: &'static str,
    url_env: &'static str,
    direction: eez_composer::Direction,
    task: &'static str,
    expected_source_chain_id: u64,
    dispatch_address_env: &'static str,
}

fn xchain_front_specs(l1_chain_id: u64, l2_chain_id: u64) -> [XchainFrontSpec; 2] {
    [
        XchainFrontSpec {
            port_env: "EEZ_L1_XCHAIN_PORT",
            url_env: "EEZ_L1_RPC_URL",
            direction: eez_composer::Direction::Inbound,
            task: "eez-l1-xchain-front",
            expected_source_chain_id: l1_chain_id,
            dispatch_address_env: "EEZ_REGISTRY_ADDRESS",
        },
        XchainFrontSpec {
            port_env: "EEZ_L2_XCHAIN_PORT",
            url_env: "EEZ_L2_RPC_URL",
            direction: eez_composer::Direction::Outbound,
            task: "eez-l2-xchain-front",
            expected_source_chain_id: l2_chain_id,
            dispatch_address_env: "EEZL2_ADDRESS",
        },
    ]
}

fn read_xchain_front_config(
    port_env: &str,
    url_env: &str,
) -> eyre::Result<(u16, String, reqwest::Url)> {
    let port = match env::var(port_env) {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(err) => return Err(eyre::eyre!("{port_env} is not valid unicode: {err}")),
    };
    let url = match env::var(url_env) {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => None,
        Err(err) => return Err(eyre::eyre!("{url_env} is not valid unicode: {err}")),
    };
    parse_xchain_front_config(port_env, url_env, port.as_deref(), url.as_deref())
}

fn parse_xchain_front_config(
    port_env: &str,
    url_env: &str,
    port: Option<&str>,
    url: Option<&str>,
) -> eyre::Result<(u16, String, reqwest::Url)> {
    let port_raw = port.ok_or_else(|| eyre::eyre!("{port_env} is required in composer mode"))?;
    let port = port_raw
        .parse::<u16>()
        .map_err(|err| eyre::eyre!("{port_env}={port_raw:?} malformed: {err}"))?;
    let Some(url_raw) = url else {
        return Err(eyre::eyre!("{port_env} is set but {url_env} is missing"));
    };
    let parsed = url_raw
        .parse::<reqwest::Url>()
        .map_err(|err| eyre::eyre!("{url_env}={url_raw:?} malformed: {err}"))?;
    Ok((port, url_raw.to_string(), parsed))
}

/// Build the `EmbeddedL1Config` from env; all vars optional, with testing
/// defaults so the smoke harness only overrides what it needs.
///
///   - `EEZ_L1_HTTP_PORT` — default `18545` (WS = http_port + 1)
///   - `EEZ_L1_AUTH_PORT` — default `http_port + 6`
///   - `EEZ_L1_P2P_PORT`  — default `30444` (P2P + discv4)
///   - `EEZ_L1_DISCV5_PORT` — default `p2p_port + 10` (discv5 UDP)
///   - `EEZ_L1_DATADIR`   — default `$TMPDIR/eez-l1-embedded` (ephemeral)
///   - `EEZ_L1_CHAIN_PATH` — L1 genesis JSON; unset → reth's `dev`
///     chainspec (all forks at genesis, no funded accounts)
fn build_embedded_l1_config() -> eyre::Result<l1_embedded::EmbeddedL1Config> {
    use reth_cli::chainspec::ChainSpecParser;

    let http_port = env::var("EEZ_L1_HTTP_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(18545);
    // Auth RPC port — kept clear of the WS port (which
    // build_network_rpc_args derives as http_port + 1) and configurable
    // so it can dodge a default-port collision with other nodes on the
    // host. Defaults to http_port + 6.
    let auth_port = env::var("EEZ_L1_AUTH_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(http_port.wrapping_add(6));
    let p2p_port = env::var("EEZ_L1_P2P_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(30444);
    // discv5 UDP port — kept separate from p2p_port (discv4) and
    // configurable so it can dodge a default-port collision with other
    // nodes on the host. Defaults to p2p_port + 10.
    let discv5_port = env::var("EEZ_L1_DISCV5_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(p2p_port.wrapping_add(10));
    let datadir = env::var("EEZ_L1_DATADIR").map_or_else(
        |_| std::env::temp_dir().join("eez-l1-embedded"),
        std::path::PathBuf::from,
    );

    // Chain spec: explicit path wins. Otherwise default to reth's
    // built-in `dev` chainspec — same shape `reth --chain dev` uses,
    // which has all forks (including Cancun/Prague where supported)
    // active at genesis with a couple of pre-funded dev EOAs.
    let chain_arg = env::var("EEZ_L1_CHAIN_PATH").unwrap_or_else(|_| "dev".to_string());
    let dev_chain_spec = EthereumChainSpecParser::parse(&chain_arg)
        .map_err(|e| eyre::eyre!("EEZ_L1_CHAIN_PATH={chain_arg}: {e}"))?;

    // L1 chain selector: `testing` (vanilla EthereumNode, auto-mine 5s),
    // `devnet` (EthereumNode + external CL), or `chiado`
    // (reth_gnosis::GnosisNode, real chiado state). The
    // `dev_chain_spec` is consumed by testing/devnet paths.
    let kind = l1_embedded::L1ChainKind::from_env();

    // JWT secret path — required for chiado/devnet (lighthouse engine API
    // auth); optional for testing mode (no external CL).
    let jwtsecret = env::var("EEZ_L1_JWT_SECRET")
        .ok()
        .map(std::path::PathBuf::from);

    let trusted_peers = env::var("EEZ_L1_TRUSTED_PEERS")
        .ok()
        .map(|peers| {
            peers
                .split([',', ' '])
                .map(str::trim)
                .filter(|peer| !peer.is_empty())
                .map(str::parse)
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();

    Ok(l1_embedded::EmbeddedL1Config {
        dev_chain_spec,
        kind,
        datadir,
        http_port,
        auth_port,
        p2p_port,
        discv5_port,
        jwtsecret,
        trusted_peers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_catch_up_walks_exact_durable_ancestry_and_respects_floor() {
        let directory = tempfile::tempdir().unwrap();
        let store = witness_source::WitnessDb::open(directory.path()).unwrap();
        let mut parent = B256::ZERO;
        let mut endpoints = Vec::new();
        for number in 1..=5 {
            let block = reth_ethereum_primitives::Block {
                header: alloy_consensus::Header {
                    number,
                    parent_hash: parent,
                    extra_data: alloy_primitives::Bytes::from(vec![number as u8]),
                    ..Default::default()
                },
                ..Default::default()
            };
            let hash = block.header.hash_slow();
            store
                .put(&eez_prover::BlockWitness {
                    number,
                    hash,
                    parent_hash: parent,
                    rlp: alloy_rlp::encode(block).into(),
                    witness: Default::default(),
                })
                .unwrap();
            endpoints.push(eez_l1::L2Endpoint { number, hash });
            parent = hash;
        }
        let path = preparation_path(&store, endpoints[4], Some(endpoints[1]), None).unwrap();
        assert!(path.connected_to_stop);
        assert_eq!(
            path.blocks
                .iter()
                .map(|block| block.number)
                .collect::<Vec<_>>(),
            [3, 4, 5]
        );
        let floor_path = preparation_path(&store, endpoints[4], None, Some(endpoints[3])).unwrap();
        assert!(floor_path.connected_to_stop);
        assert_eq!(floor_path.blocks.len(), 1);
        assert_eq!(floor_path.blocks[0].hash, endpoints[4].hash);

        let mut branch_parent = endpoints[1].hash;
        let mut branch_tip = endpoints[1];
        for number in 3..=5 {
            let block = reth_ethereum_primitives::Block {
                header: alloy_consensus::Header {
                    number,
                    parent_hash: branch_parent,
                    extra_data: alloy_primitives::Bytes::from(vec![0xb0, number as u8]),
                    ..Default::default()
                },
                ..Default::default()
            };
            let hash = block.header.hash_slow();
            store
                .put(&eez_prover::BlockWitness {
                    number,
                    hash,
                    parent_hash: branch_parent,
                    rlp: alloy_rlp::encode(block).into(),
                    witness: Default::default(),
                })
                .unwrap();
            branch_tip = eez_l1::L2Endpoint { number, hash };
            branch_parent = hash;
        }
        let branch =
            preparation_path(&store, branch_tip, Some(endpoints[4]), Some(endpoints[1])).unwrap();
        assert!(!branch.connected_to_stop);
        assert_eq!(
            branch
                .blocks
                .iter()
                .map(|block| block.number)
                .collect::<Vec<_>>(),
            [3, 4, 5]
        );
        assert_eq!(branch.blocks.last().unwrap().hash, branch_tip.hash);
        assert!(matches!(
            preparation_path(
                &store,
                eez_l1::L2Endpoint {
                    number: 6,
                    hash: B256::repeat_byte(6)
                },
                Some(endpoints[4]),
                None,
            ),
            Err(PreparationPathError::Missing { number: 6, .. })
        ));

        // A partial settlement may install a canonical prefix hash that is not
        // itself a complete stored block. The first child links to that exact
        // floor; preparation must stop before trying to load a floor witness.
        let partial_floor = eez_l1::L2Endpoint {
            number: 5,
            hash: B256::repeat_byte(0xa5),
        };
        let child = reth_ethereum_primitives::Block {
            header: alloy_consensus::Header {
                number: 6,
                parent_hash: partial_floor.hash,
                extra_data: alloy_primitives::Bytes::from_static(&[6]),
                ..Default::default()
            },
            ..Default::default()
        };
        let child_hash = child.header.hash_slow();
        store
            .put(&eez_prover::BlockWitness {
                number: 6,
                hash: child_hash,
                parent_hash: partial_floor.hash,
                rlp: alloy_rlp::encode(child).into(),
                witness: Default::default(),
            })
            .unwrap();
        let partial = preparation_path(
            &store,
            eez_l1::L2Endpoint {
                number: 6,
                hash: child_hash,
            },
            None,
            Some(partial_floor),
        )
        .unwrap();
        assert_eq!(partial.blocks.len(), 1);
        assert_eq!(partial.blocks[0].parent_hash, partial_floor.hash);

        // Overflow recovery prepares the oldest bounded prefix, then resumes
        // from that exact endpoint instead of retrying the newest suffix.
        let mut parent = child_hash;
        let mut extended = Vec::new();
        for number in 7..=12 {
            let block = reth_ethereum_primitives::Block {
                header: alloy_consensus::Header {
                    number,
                    parent_hash: parent,
                    extra_data: alloy_primitives::Bytes::from(vec![0xc0, number as u8]),
                    ..Default::default()
                },
                ..Default::default()
            };
            let hash = block.header.hash_slow();
            store
                .put(&eez_prover::BlockWitness {
                    number,
                    hash,
                    parent_hash: parent,
                    rlp: alloy_rlp::encode(block).into(),
                    witness: Default::default(),
                })
                .unwrap();
            extended.push(eez_l1::L2Endpoint { number, hash });
            parent = hash;
        }
        let tip = *extended.last().unwrap();
        let first = preparation_path_with_limit(&store, tip, None, Some(partial_floor), 3).unwrap();
        assert_eq!(
            first
                .blocks
                .iter()
                .map(|block| block.number)
                .collect::<Vec<_>>(),
            [6, 7, 8]
        );
        let first_end = eez_l1::L2Endpoint {
            number: first.blocks.last().unwrap().number,
            hash: first.blocks.last().unwrap().hash,
        };
        let second =
            preparation_path_with_limit(&store, tip, Some(first_end), Some(partial_floor), 3)
                .unwrap();
        assert!(second.connected_to_stop);
        assert_eq!(
            second
                .blocks
                .iter()
                .map(|block| block.number)
                .collect::<Vec<_>>(),
            [9, 10, 11]
        );
    }

    #[test]
    fn l1_rollup_id_defaults_only_when_absent() {
        assert_eq!(parse_l1_rollup_id(None).unwrap(), 0);
        assert_eq!(parse_l1_rollup_id(Some(" 7 ")).unwrap(), 7);
        let err = parse_l1_rollup_id(Some("1o")).unwrap_err().to_string();
        assert!(err.contains("EEZ_L1_ROLLUP_ID=\"1o\" malformed"));
    }

    #[test]
    fn xchain_front_missing_port_fails_fast() {
        let err = parse_xchain_front_config("PORT", "URL", None, Some("http://127.0.0.1:8545"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("PORT is required in composer mode"));
    }

    #[test]
    fn xchain_front_malformed_port_fails_fast() {
        let err =
            parse_xchain_front_config("PORT", "URL", Some("not-a-port"), Some("http://127.0.0.1"))
                .unwrap_err()
                .to_string();
        assert!(err.contains("PORT=\"not-a-port\" malformed"));
    }

    #[test]
    fn xchain_front_missing_upstream_fails_fast() {
        let err = parse_xchain_front_config("PORT", "URL", Some("8546"), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("PORT is set but URL is missing"));
    }

    #[test]
    fn xchain_front_malformed_upstream_fails_fast() {
        let err = parse_xchain_front_config("PORT", "URL", Some("8546"), Some("not a url"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("URL=\"not a url\" malformed"));
    }

    #[test]
    fn xchain_front_valid_config_is_returned() {
        let (port, url, parsed) =
            parse_xchain_front_config("PORT", "URL", Some("8546"), Some("http://127.0.0.1:8545"))
                .expect("valid config");

        assert_eq!(port, 8546);
        assert_eq!(url, "http://127.0.0.1:8545");
        assert_eq!(parsed.as_str(), "http://127.0.0.1:8545/");
    }

    #[test]
    fn xchain_front_specs_assign_source_chain_ids() {
        let specs = xchain_front_specs(31_337, 90_210);

        assert_eq!(specs[0].port_env, "EEZ_L1_XCHAIN_PORT");
        assert_eq!(specs[0].direction, eez_composer::Direction::Inbound);
        assert_eq!(specs[0].expected_source_chain_id, 31_337);
        assert_eq!(specs[0].dispatch_address_env, "EEZ_REGISTRY_ADDRESS");

        assert_eq!(specs[1].port_env, "EEZ_L2_XCHAIN_PORT");
        assert_eq!(specs[1].direction, eez_composer::Direction::Outbound);
        assert_eq!(specs[1].expected_source_chain_id, 90_210);
        assert_eq!(specs[1].dispatch_address_env, "EEZL2_ADDRESS");
    }
}
