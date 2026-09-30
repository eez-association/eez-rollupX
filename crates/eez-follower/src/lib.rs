//! Launcher for the L1-derived eez follower node.

mod unsafe_head;

use std::{env, str::FromStr, sync::Arc, time::Duration};

use alloy_primitives::Address;
use alloy_provider::RootProvider;
use eez_deriver::Deriver;
use eez_driver::{BlockCommitterHandle, RollupTiming};
use eez_l1::{L1CanonicalHead, L1Reader, L1ReaderConfig, L1Watcher, L1WatcherConfig};
use eez_node_common::EezNode;
use eez_node_common::{
    EezPayloadBuilder, EezPoolBuilder, L2NodeBuilder, correlation_rpc, correlation_store,
    node_cli_with_chain_document, read_checkpoint_dir, validate_system_address, wait_for_l1_ready,
    warn_on_deprecated_env,
};
use reth_chainspec::EthChainSpec as _;
use reth_node_builder::components::BasicPayloadServiceBuilder;
use tracing::{Level, event};

use unsafe_head::UnsafeHeadFollower;

const BOOT_CATCH_UP_INITIAL_RETRY_DELAY: Duration = Duration::from_secs(2);
const BOOT_CATCH_UP_MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
/// ~15 min at the capped backoff: outlasts a restarting L1, but a permanently
/// refused RPC call still surfaces as an exit.
const BOOT_CATCH_UP_MAX_TRANSPORT_FAILURES: u32 = 32;

struct FollowerSystemConfig {
    eezl2_address: Address,
    this_rollup_id: u64,
}

impl FollowerSystemConfig {
    fn into_context(self, l2_chain_id: u64) -> eez_protocol::system_tx::SystemTxContext {
        eez_protocol::system_tx::SystemTxContext {
            eezl2_address: self.eezl2_address,
            l2_chain_id,
            this_rollup_id: self.this_rollup_id,
        }
    }
}

/// Follower-specific CLI arguments layered on top of reth's CLI.
#[derive(clap::Args, Debug, Clone)]
struct FollowerArgs {
    /// Trusted L1 profile: mainnet, chiado, or a genesis/ChainConfig file.
    #[arg(long, env = "EEZ_L1_CHAIN_CONFIG")]
    l1_chain_config: Option<std::path::PathBuf>,
    /// Sequencer JSON-RPC URL. When set, this enables the optional unsafe-head
    /// overlay; safe and finalized remain L1-derived.
    #[arg(long, env = "EEZ_SEQUENCER_RPC")]
    sequencer_rpc: Option<url::Url>,
}

/// Launch an L1-derived follower node.
///
/// # Errors
///
/// Returns an error when configuration is invalid or either reth or the L1
/// bootstrap fails.
pub fn run() -> eyre::Result<()> {
    let (cli, _) = node_cli_with_chain_document::<FollowerArgs>()?;
    cli.run(launch)
}

async fn launch(builder: L2NodeBuilder, ext: FollowerArgs) -> eyre::Result<()> {
    event!(
        name: "eez.node.launching",
        Level::INFO,
        mode = "follower",
        "launching eez follower",
    );
    warn_on_deprecated_env();
    let timing = RollupTiming::from_env()?;
    let derivation_schedule = eez_evm::profile::schedule(builder.config().chain.as_ref())?;
    timing.validate_schedule(&derivation_schedule)?;
    // A production follower must reconstruct the Composer's Sync-block system
    // transactions byte-for-byte. Read every required value before launching
    // reth so a misconfigured follower never appears healthy.
    let system_config = read_system_config()?;
    let l1_chain_spec = ext
        .l1_chain_config
        .as_deref()
        .map(eez_l1::chain_config::load_l1_chain_spec)
        .transpose()?;
    let expected_l1_chain_id = read_l1_chain_id()?;
    if let Some(spec) = &l1_chain_spec {
        eyre::ensure!(
            spec.chain().id() == expected_l1_chain_id,
            "L1 chain configuration ID {} differs from EEZ_L1_CHAIN_ID {expected_l1_chain_id}",
            spec.chain().id()
        );
    }
    let system_address = eez_primitives::SYSTEM_ADDRESS;
    validate_system_address(system_address)?;
    let l2_data_dir = read_checkpoint_dir();
    let correlations = correlation_store(l2_data_dir.as_deref())?;
    let l1_head = Arc::new(L1CanonicalHead::default());
    let rpc_correlations = Arc::clone(&correlations);
    let rpc_l1_head = Arc::clone(&l1_head);

    let handle = builder
        .with_types::<EezNode>()
        .with_components(
            EezNode::components()
                // Reorged-out system transactions must not leak from reth's
                // reinjection path into an ordinary Live block.
                .pool(EezPoolBuilder::new(system_address))
                .payload(BasicPayloadServiceBuilder::new(EezPayloadBuilder::default())),
        )
        .with_add_ons(eez_node_common::rpc::eez_add_ons(
            eez_node_common::rpc::EezEthApiBuilder::new(
                Arc::clone(&l1_head),
                Arc::clone(&correlations),
            ),
        ))
        .extend_rpc_modules(move |ctx| {
            correlation_rpc::install(ctx, Arc::clone(&rpc_correlations), Arc::clone(&rpc_l1_head))
        })
        .launch_with_debug_capabilities()
        .await?;

    let chain_spec: Arc<_> = handle.node.chain_spec();
    let provider = handle.node.provider.clone();
    let task_executor = handle.node.task_executor.clone();
    let block_committer = BlockCommitterHandle::spawn_from_provider(
        &provider,
        handle.node.add_ons_handle.beacon_engine_handle.clone(),
        handle.node.payload_builder_handle.clone(),
        None,
    )?;

    let l1_reader_config = L1ReaderConfig::from_env()?;
    let deploy_block = l1_reader_config.deploy_block;
    let mut l1_reader = L1Reader::new(l1_reader_config);
    let l1_watcher_config = L1WatcherConfig::from_env()?;
    let l1_watcher = if let Some(spec) = l1_chain_spec {
        let rules: eez_l1::BlobRuleSource =
            Arc::new(move |timestamp| spec.blob_params_at_timestamp(timestamp));
        l1_reader = l1_reader.with_blob_rules(Arc::clone(&rules));
        L1Watcher::new_with_blob_rules(l1_watcher_config, rules)
    } else {
        L1Watcher::new(l1_watcher_config)
    };
    let system_tx_cfg = system_config.into_context(chain_spec.chain().id());
    event!(
        name: "eez.node.follower.system_tx_cfg",
        Level::INFO,
        %system_address,
        "cross-chain system tx reconstruction config loaded",
    );

    wait_for_l1_ready(
        &l1_reader,
        deploy_block,
        expected_l1_chain_id,
        derivation_schedule.config().l1_reference.number,
    )
    .await?;

    let deriver = Deriver::new(
        block_committer.clone(),
        Arc::new(provider.clone()),
        l1_reader.clone(),
        chain_spec,
        deploy_block,
        Arc::clone(&l1_head),
        Arc::clone(&correlations),
        Some(system_tx_cfg),
    )
    .await?;

    let mut retry_delay = BOOT_CATCH_UP_INITIAL_RETRY_DELAY;
    let mut attempts = 0_u64;
    let mut transport_failures = 0_u32;
    let (l1_seed_number, l1_seed_hash) = loop {
        match deriver.catch_up_with_seed().await {
            Ok(seed) => break seed,
            Err(err)
                if err.is_l1_transport() && {
                    transport_failures += 1;
                    transport_failures >= BOOT_CATCH_UP_MAX_TRANSPORT_FAILURES
                } =>
            {
                event!(
                    name: "eez.node.deriver.boot_catch_up.transport_exhausted",
                    Level::ERROR,
                    mode = "follower",
                    transport_failures,
                    error = %err,
                    "L1 transport kept failing during boot catch-up; the endpoint is likely refusing a call we need, not merely unreachable",
                );
                return Err(eyre::eyre!(
                    "boot-time deriver catch_up gave up after {transport_failures} L1 transport failures: {err}"
                ));
            }
            Err(err) if err.is_source_incomplete() || err.is_l1_transport() => {
                attempts += 1;
                event!(
                    name: "eez.node.deriver.boot_catch_up.source_incomplete",
                    Level::WARN,
                    mode = "follower",
                    attempts,
                    retry_delay_secs = retry_delay.as_secs(),
                    error = %err,
                    "boot-time catch_up could not read all L1 source data yet; retrying before starting L1-active tasks",
                );
                tokio::time::sleep(retry_delay).await;
                retry_delay = Duration::from_secs(
                    retry_delay
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
                    mode = "follower",
                    error = %err,
                    "boot-time catch_up failed; refusing to start L1-active tasks before reconciliation",
                );
                return Err(eyre::eyre!("boot-time deriver catch_up failed: {err}"));
            }
        }
    };

    correlations.mark_ready();

    event!(
        name: "eez.node.deriver.spawned",
        Level::INFO,
        mode = "follower",
        initial_posted_through = deriver.cursor(),
        "spawning eez deriver",
    );
    let deriver_events = l1_watcher.subscribe();
    task_executor.spawn_critical_task("eez-deriver", async move {
        deriver.run(deriver_events).await;
    });

    if let Some(sequencer_rpc) = ext.sequencer_rpc {
        let follower = UnsafeHeadFollower::new(
            block_committer,
            RootProvider::new_http(sequencer_rpc),
            provider,
            timing.l2_block_time(),
        );
        event!(
            name: "eez.node.follower.sequencer_rpc.spawned",
            Level::INFO,
            "spawning sequencer-RPC unsafe-head follower",
        );
        task_executor.spawn_critical_task("eez-node-follower-unsafe-head", async move {
            follower.run().await;
        });
    } else {
        event!(
            name: "eez.node.follower.l1_derived_only",
            Level::INFO,
            "EEZ_SEQUENCER_RPC not set; running L1-derived-only follower",
        );
    }

    task_executor.spawn_critical_task(
        "eez-l1-watcher",
        l1_watcher.polling(l1_seed_number, l1_seed_hash),
    );

    handle.wait_for_node_exit().await
}

/// Read the mandatory system-transaction identity before follower startup.
fn read_system_config() -> eyre::Result<FollowerSystemConfig> {
    let eezl2_address = required_env("EEZL2_ADDRESS")?;
    let rollup_id = required_env("EEZ_ROLLUP_ID")?;

    let eezl2_address = Address::from_str(&eezl2_address)
        .map_err(|err| eyre::eyre!("EEZL2_ADDRESS malformed: {err}"))?;
    eyre::ensure!(
        eezl2_address == eez_primitives::EEZL2_ADDRESS,
        "EEZL2_ADDRESS must be the native predeploy"
    );
    let this_rollup_id = rollup_id
        .parse()
        .map_err(|e| eyre::eyre!("EEZ_ROLLUP_ID malformed: {e}"))?;

    Ok(FollowerSystemConfig {
        eezl2_address,
        this_rollup_id,
    })
}

fn required_env(name: &str) -> eyre::Result<String> {
    match env::var(name) {
        Ok(value) => Ok(value),
        Err(env::VarError::NotPresent) => Err(eyre::eyre!("{name} is required in follower mode")),
        Err(env::VarError::NotUnicode(_)) => Err(eyre::eyre!("{name} contains non-UTF-8 bytes")),
    }
}

/// Required for followers: guessing would either assert the wrong source
/// chain or silently skip the configured RPC's identity check.
fn read_l1_chain_id() -> eyre::Result<u64> {
    let value = env::var("EEZ_L1_CHAIN_ID").map_err(|err| {
        eyre::eyre!("EEZ_L1_CHAIN_ID is required (the L1 chain id this node derives from): {err}")
    })?;
    value
        .parse::<u64>()
        .map_err(|err| eyre::eyre!("EEZ_L1_CHAIN_ID={value:?} malformed: {err}"))
}
