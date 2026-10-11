//! Shared, role-neutral bootstrap substrate for eez node binaries.

#[cfg(feature = "l1-startup")]
use eez_deriver::CorrelationStore;
#[cfg(feature = "l1-startup")]
use std::sync::Arc;
use std::{env, path::PathBuf};

use mimalloc::MiMalloc;
use reth_node_builder::{NodeBuilder, WithLaunchContext};
use tracing::{Level, event};

#[cfg(feature = "l1-startup")]
pub mod correlation_rpc;

pub mod node;
pub use node::EezNode;
mod payload;
mod payload_builder;
mod pool_transaction;
pub mod rpc;
pub use payload::{EezPayloadBuilder, SequencingDataGuard};
mod pool;
pub use pool::EezPoolBuilder;

/// Per M-MIMALLOC-APPS — meaningful win on allocation-heavy workloads.
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Role-neutral reth builder passed to each typed launcher.
pub type L2NodeBuilder =
    WithLaunchContext<NodeBuilder<reth_db::DatabaseEnv, reth_chainspec::ChainSpec>>;

/// Composer nodes have no role-specific CLI arguments.
#[derive(clap::Args, Debug, Clone)]
pub struct NoRoleArgs {}

#[cfg(feature = "l1-startup")]
mod chain_cli;
#[cfg(feature = "l1-startup")]
pub use chain_cli::node_cli_with_chain_document;

#[cfg(feature = "l1-startup")]
fn node_cli_arguments() -> Vec<String> {
    let _ = dotenvy::dotenv();
    let _ = dotenvy::from_filename("deployments.env");

    if std::env::var_os("RUST_BACKTRACE").is_none() {
        // SAFETY: set during single-threaded startup before any other thread is spawned.
        unsafe {
            std::env::set_var("RUST_BACKTRACE", "1");
        }
    }

    // Engine API FCUs may intentionally unwind the L2 head during recovery.
    let mut argv: Vec<String> = std::env::args().collect();
    for flag in [
        "--engine.always-process-payload-attributes-on-canonical-head",
        "--engine.allow-unwind-canonical-header",
    ] {
        if !argv.iter().any(|a| a == flag) {
            argv.push(flag.to_owned());
        }
    }

    argv
}

/// Warn about obsolete runtime role selectors now replaced by explicit binaries.
pub fn warn_on_deprecated_env() {
    for name in [
        "EEZ_COMPOSER_INTERVAL_SECS",
        "EEZ_SEQUENCER_DISABLED",
        "EEZ_COMPOSER_DISABLED",
    ] {
        if std::env::var_os(name).is_some() {
            event!(
                name: "eez.node.env.deprecated",
                Level::WARN,
                env = name,
                "env var is ignored; select the node role with the eez-composer or eez-follower executable."
            );
        }
    }
}

/// Resolve the L2 datadir once at startup. A missing or unusable directory
/// disables the boot-checkpoint cache and preserves the safe full L1 rescan.
#[must_use]
pub fn read_checkpoint_dir() -> Option<PathBuf> {
    let configured = env::var("EEZ_L2_DATADIR").ok();
    let checkpoint_dir = configured.as_ref().and_then(|raw| {
        let dir = PathBuf::from(raw);
        match std::fs::create_dir_all(&dir) {
            Ok(()) if dir.is_dir() => Some(dir),
            Ok(()) => None,
            Err(error) => {
                event!(
                    name: "eez.node.datadir.unusable",
                    Level::WARN,
                    path = %dir.display(),
                    error = %error,
                    "EEZ_L2_DATADIR could not be prepared for derived metadata",
                );
                None
            }
        }
    });
    if checkpoint_dir.is_none() {
        event!(
            name: "eez.node.checkpoint.disabled",
            Level::INFO,
            configured = ?configured,
            "no usable EEZ_L2_DATADIR; boot will rescan L1 from the deploy block",
        );
    }
    checkpoint_dir
}

/// Open the shared persistent settlement index, or an in-memory index without a datadir.
#[cfg(feature = "l1-startup")]
pub fn correlation_store(datadir: Option<&std::path::Path>) -> eyre::Result<Arc<CorrelationStore>> {
    let store = match datadir {
        Some(datadir) => {
            let path = datadir.join("eez-correlations");
            let store = CorrelationStore::open(&path).map_err(|error| eyre::eyre!(error))?;
            event!(
                name: "eez.node.correlation_store.opened",
                Level::INFO,
                path = %path.display(),
                "persistent L1/L2 settlement correlation index opened",
            );
            store
        }
        None => {
            event!(
                name: "eez.node.correlation_store.memory_only",
                Level::WARN,
                "no usable EEZ_L2_DATADIR; correlation RPC history will not survive restart",
            );
            CorrelationStore::in_memory()
        }
    };
    Ok(Arc::new(store))
}

/// Wait for the expected L1 chain, deployment height, and genesis-reference finality.
///
/// This is only a preflight. The deriver authenticates the reference and selects
/// a validated persistent checkpoint (or deployment for a fresh node), then
/// reads the required history before either role starts L1-active tasks. Probing
/// deployment receipts here would prevent a valid restart on a pruned source.
///
/// # Errors
///
/// Returns an error for a mismatched chain or when the L1 source fails to make
/// progress within the bounded startup window.
#[cfg(feature = "l1-startup")]
pub async fn wait_for_l1_ready(
    l1_reader: &eez_l1::L1Reader,
    deploy_block: u64,
    expected_l1_chain_id: u64,
    genesis_reference_number: u64,
) -> eyre::Result<()> {
    use std::time::Duration;

    const POLL: Duration = Duration::from_secs(2);
    const STALLED_POLLS: u32 = 450; // 15 min with no progress
    const MAX_POLLS: u32 = 3_600; // 2 h overall, even while it claims to sync

    let mut best_remaining = u64::MAX;
    let mut stalled = 0_u32;
    let mut waited = 0_u32;
    let mut last_err: Option<String> = None;
    let mut chain_verified = false;

    loop {
        if !chain_verified {
            match l1_reader.chain_id().await {
                Ok(actual) if actual != expected_l1_chain_id => {
                    return Err(eyre::eyre!(
                        "EEZ_L1_RPC_URL serves chain {actual}, expected {expected_l1_chain_id}"
                    ));
                }
                Ok(_) => chain_verified = true,
                Err(err) => last_err = Some(format!("eth_chainId: {err}")),
            }
        }

        let (progressing, status) = match l1_reader.readiness().await {
            Err(err) => {
                last_err = Some(err.to_string());
                (false, format!("unreachable: {err}"))
            }
            Ok(state) if state.head_block_number < deploy_block => {
                let remaining = deploy_block - state.head_block_number;
                let closer = remaining < best_remaining;
                best_remaining = best_remaining.min(remaining);
                (closer, format!("{remaining} blocks below the deploy block"))
            }
            Ok(state) if chain_verified => {
                // Block zero needs no finality probe (see `reference_is_final`);
                // a fresh devnet reports no `finalized` block for 20+ minutes.
                let finalized = if genesis_reference_number == 0 {
                    Ok(None)
                } else {
                    l1_reader.finalized_block().await
                };
                match finalized {
                    Ok(finalized)
                        if reference_is_final(
                            genesis_reference_number,
                            finalized.map(|(number, _)| number),
                        ) =>
                    {
                        event!(
                            name: "eez.node.l1_ready",
                            Level::INFO,
                            head = state.head_block_number,
                            deploy_block,
                            genesis_reference_number,
                            "L1 chain and finality ready; history validation deferred to deriver recovery anchor",
                        );
                        // Readiness is not authentication: the deriver separately
                        // checks the exact canonical reference hash and seed.
                        return Ok(());
                    }
                    Ok(_) => (false, "genesis reference is not finalized yet".to_string()),
                    Err(err) => {
                        last_err = Some(err.to_string());
                        (false, "finality probe failed".to_string())
                    }
                }
            }
            Ok(_) => (false, "chain id not read yet".to_string()),
        };

        stalled = if progressing { 0 } else { stalled + 1 };
        waited += 1;
        if waited <= 1 || waited.is_multiple_of(30) {
            event!(
                name: "eez.node.l1_not_ready",
                Level::WARN,
                deploy_block,
                stalled_polls = stalled,
                status,
                "waiting for L1 chain, deployment height and finality",
            );
        }
        if stalled >= STALLED_POLLS || waited >= MAX_POLLS {
            let cause = last_err.unwrap_or(status);
            return Err(eyre::eyre!(
                "L1 preflight never became ready at deployment block {deploy_block}: {cause}"
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Whether the L1 genesis reference is final enough to start from.
///
/// Block zero is final by definition and is authenticated against the
/// canonical block zero, not `finalized` (matching
/// `eez_l1::derivation::DerivationSource::authenticate`). Any later reference
/// requires a reported finalized block at or above it; no report means not yet.
#[cfg(feature = "l1-startup")]
const fn reference_is_final(reference_number: u64, finalized: Option<u64>) -> bool {
    match finalized {
        _ if reference_number == 0 => true,
        Some(number) => number >= reference_number,
        None => false,
    }
}

/// Reject a configured address that differs from the native system identity.
pub fn validate_system_address(system_address: alloy_primitives::Address) -> eyre::Result<()> {
    eyre::ensure!(
        system_address == eez_primitives::SYSTEM_ADDRESS,
        "native system address is fixed"
    );
    match env::var("EEZ_L2_SYSTEM_ADDRESS") {
        Ok(raw) => {
            let configured: alloy_primitives::Address = raw
                .trim()
                .parse()
                .map_err(|error| eyre::eyre!("EEZ_L2_SYSTEM_ADDRESS malformed: {error}"))?;
            eyre::ensure!(
                configured == system_address,
                "EEZ_L2_SYSTEM_ADDRESS {configured} does not match the fixed native system address"
            );
            Ok(())
        }
        Err(env::VarError::NotPresent) => Ok(()),
        Err(env::VarError::NotUnicode(_)) => Err(eyre::eyre!(
            "EEZ_L2_SYSTEM_ADDRESS contains non-UTF-8 bytes"
        )),
    }
}

#[cfg(all(test, feature = "l1-startup"))]
mod tests {
    use super::reference_is_final;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    /// Serve headers but reject receipts, as a synced, receipt-pruned Reth does.
    async fn pruned_source(
        chain_id: u64,
        finalized: u64,
    ) -> (
        eez_l1::L1Reader,
        jsonrpsee::server::ServerHandle,
        Arc<AtomicUsize>,
    ) {
        use jsonrpsee::{RpcModule, server::ServerBuilder, types::ErrorObjectOwned};
        let server = ServerBuilder::default()
            .build((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let rpc_url = format!("http://{}", server.local_addr().unwrap())
            .parse()
            .unwrap();
        let probes = Arc::new(AtomicUsize::new(0));
        let mut module = RpcModule::new(probes.clone());
        module
            .register_method("eth_chainId", move |_, _, _| format!("{chain_id:#x}"))
            .unwrap();
        module
            .register_method("eth_blockNumber", |_, _, _| "0x64")
            .unwrap();
        module
            .register_method("eth_syncing", |_, _, _| false)
            .unwrap();
        module
            .register_method("eth_getBlockByNumber", move |params, _, _| {
                let (tag, _): (String, bool) = params.parse()?;
                let mut block: alloy_rpc_types_eth::Block = Default::default();
                block.header.inner.number = if tag == "finalized" { finalized } else { 5 };
                Ok::<_, ErrorObjectOwned>(block)
            })
            .unwrap();
        module
            .register_method("eth_getLogs", |_, probes, _| {
                probes.fetch_add(1, Ordering::SeqCst);
                Err::<serde_json::Value, _>(ErrorObjectOwned::owned(
                    4444,
                    "pruned history unavailable",
                    None::<()>,
                ))
            })
            .unwrap();
        let handle = server.start(module);
        let reader = eez_l1::L1Reader::new(eez_l1::L1ReaderConfig {
            rpc_url,
            beacon_url: None,
            eez: alloy_primitives::Address::repeat_byte(1),
            rollup_id: 1,
            deploy_block: 5,
        });
        (reader, handle, probes)
    }

    #[tokio::test]
    async fn readiness_defers_pruned_history_to_checkpoint_aware_derivation() {
        let (reader, server, probes) = pruned_source(1, 80).await;
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            super::wait_for_l1_ready(&reader, 5, 1, 40),
        )
        .await;
        server.stop().unwrap();
        result
            .expect("deployment receipts must not block checkpoint selection")
            .unwrap();
        assert_eq!(probes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn pruned_source_still_requires_correct_chain_and_final_reference() {
        let (reader, server, _) = pruned_source(2, 80).await;
        let error = super::wait_for_l1_ready(&reader, 5, 1, 40)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("serves chain 2, expected 1"));
        server.stop().unwrap();

        let (reader, server, probes) = pruned_source(1, 39).await;
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                super::wait_for_l1_ready(&reader, 5, 1, 40),
            )
            .await
            .is_err(),
            "an unfinalized reference must not pass readiness"
        );
        assert_eq!(probes.load(Ordering::SeqCst), 0);
        server.stop().unwrap();
    }

    #[test]
    fn block_zero_reference_needs_no_finalized_report() {
        // A fresh devnet reports no finalized block until its first checkpoint.
        assert!(reference_is_final(0, None));
        assert!(reference_is_final(0, Some(0)));
        assert!(reference_is_final(0, Some(7)));
    }

    #[test]
    fn later_reference_requires_finality_at_or_above_it() {
        assert!(!reference_is_final(40, None));
        assert!(!reference_is_final(40, Some(39)));
        assert!(reference_is_final(40, Some(40)));
        assert!(reference_is_final(40, Some(41)));
    }
}
