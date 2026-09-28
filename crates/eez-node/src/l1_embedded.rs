//! Embedded L1 reth — config builder for the second `NodeBuilder` that
//! composer mode launches alongside the L2 reth. Runtime modes:
//!   - **Devnet** (vanilla EthereumNode, CL-driven) — private PoS L1.
//!     `EEZ_L1_CHAIN=devnet`.
//!   - **Chiado** (reth_gnosis::GnosisNode) — real chiado state from the
//!     mounted datadir, driven via engine-API by an external lighthouse
//!     CL. `EEZ_L1_CHAIN=chiado`.
//!   - **Testing** (vanilla EthereumNode, 5s auto-mine) — dev mode with the
//!     non-atomic mock bundle RPC; the default. `EEZ_L1_CHAIN=testing`.
//!
//! The `NodeBuilder` launch is inline in [`crate::run_composer`] — the
//! nested-generic `NodeHandle` AddOns types resist a typed helper.

use std::{path::PathBuf, sync::Arc};

use eyre::Result;
use reth_chainspec::ChainSpec;
use reth_gnosis::spec::{chains::CHIADO_GENESIS, gnosis_spec::GnosisChainSpec};
use reth_network_peers::TrustedPeer;
use reth_node_core::{
    args::{DatadirArgs, NetworkArgs, RpcServerArgs},
    node_config::NodeConfig,
};

/// Block time for the testing embedded L1. 5s gives the composer a wide
/// window to land `postBatch + user_tx` in the same L1 block (invariant
/// 4's atomicity requirement); a 1s window lets dev-reth's auto-mine
/// split the two `send_raw_transaction` roundtrips across blocks.
pub const TESTING_L1_BLOCK_TIME: std::time::Duration = std::time::Duration::from_secs(5);

/// Mine on the genesis slot phase. Reth's default interval starts relative to
/// process launch, which can include a timestamp-bound proof in the preceding
/// slot. Skip delayed ticks instead of producing future blocks in a burst.
pub fn testing_mining_interval(genesis: u64) -> Result<tokio::time::Interval> {
    let now = tokio::time::Instant::now();
    let wall = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    let delay = testing_slot_delay(genesis, wall)?;
    let mut interval = tokio::time::interval_at(now + delay, TESTING_L1_BLOCK_TIME);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    Ok(interval)
}

fn testing_slot_delay(genesis: u64, sampled: std::time::Duration) -> Result<std::time::Duration> {
    let elapsed = sampled
        .checked_sub(std::time::Duration::from_secs(genesis))
        .ok_or_else(|| eyre::eyre!("testing L1 clock precedes genesis"))?;
    let remainder = std::time::Duration::new(
        elapsed.as_secs() % TESTING_L1_BLOCK_TIME.as_secs(),
        elapsed.subsec_nanos(),
    );
    // The whole-second period and modulus keep remainder strictly below it.
    Ok(TESTING_L1_BLOCK_TIME
        .checked_sub(remainder)
        .expect("slot remainder is smaller than the period"))
}

/// Preserve the private miner's slot phase without moving its sampled clock
/// into the future. The actual parent supplies the monotonic lower bound after
/// clock jitter; sampling after missed slots or restart skips to the last slot.
/// Real Ethereum/Gnosis consensus never uses this testing clock.
pub fn testing_slot_timestamp(genesis: u64, parent: u64, sampled: u64) -> Result<u64> {
    let elapsed = sampled
        .checked_sub(genesis)
        .ok_or_else(|| eyre::eyre!("testing L1 clock precedes genesis"))?;
    let period = TESTING_L1_BLOCK_TIME.as_secs();
    eyre::ensure!(
        parent >= genesis && (parent - genesis).is_multiple_of(period),
        "testing L1 parent is outside its genesis slot phase"
    );
    let completed_slot = genesis + (elapsed / period) * period;
    let next = parent
        .checked_add(period)
        .ok_or_else(|| eyre::eyre!("testing L1 slot timestamp overflow"))?;
    Ok(completed_slot.max(next))
}

/// Which L1 chain the embedded L1 NodeBuilder should serve. Selected
/// by `EEZ_L1_CHAIN` env at startup; defaults to `Testing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L1ChainKind {
    /// `reth_gnosis::GnosisNode` loading the chiado preset; no
    /// auto-mine — engine API driven by an external lighthouse CL.
    Chiado,
    /// Private PoS Ethereum node driven by an external consensus client.
    Devnet,
    /// Vanilla `EthereumNode` in dev mode (5s auto-mine, --chain dev) with the non-atomic mock bundle RPC.
    Testing,
    /// Private native Gnosis execution fixture, with the same five-second
    /// auto-mining and non-atomic test relay as `Testing`.
    TestingGnosis,
}

impl L1ChainKind {
    pub fn from_env() -> Self {
        match std::env::var("EEZ_L1_CHAIN").as_deref() {
            Ok("chiado") => Self::Chiado,
            Ok("devnet") => Self::Devnet,
            Ok("testing-gnosis") => Self::TestingGnosis,
            _ => Self::Testing,
        }
    }
}

/// Config knobs for the embedded L1 launch. All read from env in
/// `eez-node::main`; defaults match a no-config-needed testing workflow.
#[derive(Debug, Clone)]
pub struct EmbeddedL1Config {
    /// L1 dev chain spec (genesis + hardforks). Used by Testing and Devnet;
    /// ignored by Chiado (which loads its own GnosisChainSpec).
    pub dev_chain_spec: Arc<ChainSpec>,
    /// Which L1 chain the embedded node serves.
    pub kind: L1ChainKind,
    /// Where reth stores L1 chain data + static files. For chiado
    /// this should be the pre-synced chiado-db (symlink or rsync
    /// from the operator's snapshot).
    pub datadir: PathBuf,
    /// HTTP RPC port (so smoke harnesses + `cast` can POST txs to L1).
    pub http_port: u16,
    /// Auth RPC port (engine API). Chiado/Devnet mode: external lighthouse
    /// dials in here.
    pub auth_port: u16,
    /// P2P + discv4 discovery port. Testing mode: disabled. Chiado/Devnet mode:
    /// needed for libp2p peering.
    pub p2p_port: u16,
    /// discv5 UDP port. Separate from `p2p_port` so the embedded L1's
    /// discv5 doesn't bind reth's default port and collide with another
    /// node on the host.
    pub discv5_port: u16,
    /// Path to JWT secret file. Required in chiado/devnet mode (shared with
    /// lighthouse via volume mount).
    pub jwtsecret: Option<PathBuf>,
    /// Static execution peers for private networks.
    pub trusted_peers: Vec<TrustedPeer>,
}

/// Build a reth `NodeConfig<ChainSpec>` for the embedded L1 testing node
/// (vanilla EthereumNode in dev mode).
pub fn build_testing_node_config(cfg: &EmbeddedL1Config) -> Result<NodeConfig<ChainSpec>> {
    let (network_args, rpc_args) = build_network_rpc_args(cfg)?;
    let mut node_cfg = NodeConfig::new(cfg.dev_chain_spec.clone())
        .with_datadir_args(DatadirArgs {
            datadir: cfg.datadir.clone().into(),
            ..DatadirArgs::default()
        })
        .with_network(network_args)
        .with_rpc(rpc_args)
        .dev();
    node_cfg.dev.block_time = Some(TESTING_L1_BLOCK_TIME);
    // The private dev bundle endpoint inserts through this pool. Its default
    // per-sender spam limit (16) is not an L1 bundle validity constraint and
    // must not reject an otherwise fitting nonce chain partway through.
    // Global pool size and actual block gas limits still bound testing nodes.
    node_cfg.txpool.max_account_slots = usize::MAX;
    // This isolated miner has no consensus peer that can restore lost history.
    // Preserve the former short persistence window for crash/restart fixtures;
    // Reth v2.6's 50-block default can otherwise lose their entire L1 chain.
    node_cfg.engine.persistence_threshold = 2;
    node_cfg.engine.num_state_masking_blocks = 0;
    node_cfg.engine.memory_block_buffer_target = Some(0);
    // Synchronous state-root path for dev determinism.
    node_cfg.engine.state_root_fallback = true;
    Ok(node_cfg)
}

/// Build a reth `NodeConfig<GnosisChainSpec>` for chiado. NOT dev
/// mode — block production happens via engine-API from an external
/// lighthouse CL (which we expect the operator to launch separately
/// via `scripts/launch-chiado-lighthouse.sh` / docker-compose).
pub fn build_chiado_node_config(cfg: &EmbeddedL1Config) -> Result<NodeConfig<GnosisChainSpec>> {
    let (network_args, mut rpc_args) = build_network_rpc_args(cfg)?;
    // Keep discovery ENABLED. `newPayload` only lets the EL *execute*
    // blocks whose parent it already holds — it is not a sync path. A
    // snapshot behind the chain tip must backfill the gap over P2P, so
    // the EL needs peers (chiado bootnodes come from the GnosisChainSpec;
    // discovery runs on `cfg.p2p_port`, set in build_network_rpc_args, so
    // it won't collide with another reth on the host).
    // Bind auth RPC to all interfaces so docker-compose lighthouse
    // (host-network mode) can reach it. JWT secret guards access.
    rpc_args.auth_addr = "0.0.0.0".parse().expect("static addr");
    rpc_args.auth_jwtsecret = cfg.jwtsecret.clone();
    let testing = cfg.kind == L1ChainKind::TestingGnosis;
    let chain_spec: Arc<GnosisChainSpec> = Arc::new(GnosisChainSpec::from(if testing {
        cfg.dev_chain_spec.genesis.clone()
    } else {
        CHIADO_GENESIS.clone()
    }));
    eez_l1::chain_config::validate_gnosis_execution_config(&chain_spec.inner.genesis)?;
    let mut node_cfg = NodeConfig::new(chain_spec)
        .with_datadir_args(DatadirArgs {
            datadir: cfg.datadir.clone().into(),
            ..DatadirArgs::default()
        })
        .with_network(network_args)
        .with_rpc(rpc_args);
    // Preserve the synchronous root path used by the Gnosis integration.
    // Reth v2.6 ignores the old legacy-state-root switch.
    node_cfg.engine.state_root_fallback = true;
    if testing {
        node_cfg = node_cfg.dev();
        node_cfg.dev.block_time = Some(TESTING_L1_BLOCK_TIME);
        node_cfg.txpool.max_account_slots = usize::MAX;
        // Match the private Ethereum miner's crash/restart persistence window.
        node_cfg.engine.persistence_threshold = 2;
        node_cfg.engine.num_state_masking_blocks = 0;
        node_cfg.engine.memory_block_buffer_target = Some(0);
    }
    Ok(node_cfg)
}

/// Build a private L1 driven by an external consensus client.
pub fn build_devnet_node_config(cfg: &EmbeddedL1Config) -> Result<NodeConfig<ChainSpec>> {
    let (network_args, mut rpc_args) = build_network_rpc_args(cfg)?;
    rpc_args.auth_addr = "0.0.0.0".parse().expect("static addr");
    rpc_args.auth_jwtsecret = cfg.jwtsecret.clone();
    let mut node_cfg = NodeConfig::new(cfg.dev_chain_spec.clone())
        .with_datadir_args(DatadirArgs {
            datadir: cfg.datadir.clone().into(),
            ..DatadirArgs::default()
        })
        .with_network(network_args)
        .with_rpc(rpc_args);
    node_cfg.engine.state_root_fallback = true;
    Ok(node_cfg)
}

fn build_network_rpc_args(cfg: &EmbeddedL1Config) -> Result<(NetworkArgs, RpcServerArgs)> {
    let ws_port = cfg.http_port.checked_add(1).ok_or_else(|| {
        eyre::eyre!(
            "L1 http_port {} too high for WS = http_port + 1",
            cfg.http_port
        )
    })?;
    let mut network_args = NetworkArgs {
        port: cfg.p2p_port,
        ..NetworkArgs::default()
    };
    network_args.discovery.port = cfg.p2p_port;
    network_args.discovery.discv5_port = Some(cfg.discv5_port);
    network_args.trusted_peers = cfg.trusted_peers.clone();
    let mut rpc_args = RpcServerArgs {
        http: true,
        ws: true,
        http_port: cfg.http_port,
        ws_port,
        auth_port: cfg.auth_port,
        ipcdisable: true,
        ..RpcServerArgs::default()
    };
    // The L1 wallet front classifies signed transactions with a read-only
    // `debug_traceCall`. Reth's default HTTP selection is eth/net/web3, so the
    // embedded L1 must opt into debug explicitly. Keep trace as well to match
    // the public L2 RPC configured by the node CLI.
    rpc_args.http_api = Some(
        "eth,net,web3,debug,trace"
            .parse()
            .expect("static embedded L1 RPC module selection is valid"),
    );
    Ok((network_args, rpc_args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn testing_miner_waits_for_the_next_genesis_aligned_slot() {
        use std::time::Duration;
        let genesis = 1002;
        for (sampled, expected) in [
            (Duration::from_secs(1002), Duration::from_secs(5)),
            (Duration::from_millis(1006250), Duration::from_millis(750)),
            (Duration::from_secs(1007), Duration::from_secs(5)),
            (Duration::from_millis(1801750), Duration::from_millis(250)),
        ] {
            let delay = testing_slot_delay(genesis, sampled).unwrap();
            assert_eq!(delay, expected);
            let slot = sampled + delay;
            assert_eq!(slot.subsec_nanos(), 0);
            assert_eq!((slot.as_secs() - genesis) % 5, 0);
            assert_eq!(
                testing_slot_timestamp(genesis, genesis, slot.as_secs()).unwrap(),
                slot.as_secs()
            );
        }
        assert!(testing_slot_delay(genesis, Duration::from_secs(genesis - 1)).is_err());
    }

    #[test]
    fn testing_l1_clock_preserves_phase_without_future_rounding() {
        let genesis = 1002;
        for (parent, sampled, expected) in [
            (1002, 1007, 1007),
            (1007, 1011, 1012), // Small wall-clock jitter still advances one slot.
            (1002, 1013, 1012), // Startup delay: the last completed slot.
            (1007, 1801, 1797), // Missed slots.
            (1797, 1803, 1802), // Restart from the actual persisted parent.
        ] {
            let timestamp = testing_slot_timestamp(genesis, parent, sampled).unwrap();
            assert_eq!(timestamp, expected);
            assert!(timestamp > parent);
            assert_eq!((timestamp - genesis) % 5, 0);
            if sampled >= parent + 5 {
                assert!(
                    timestamp <= sampled,
                    "a completed slot must not be rounded forward"
                );
            }
        }
        assert!(testing_slot_timestamp(genesis, genesis, genesis - 1).is_err());
        assert!(testing_slot_timestamp(genesis, genesis + 1, 1020).is_err());
        assert!(testing_slot_timestamp(0, 0, u64::MAX).is_ok());
        assert!(testing_slot_timestamp(u64::MAX - 3, u64::MAX - 3, u64::MAX).is_err());
    }

    #[test]
    fn only_private_miners_use_the_short_persistence_window() {
        let mut cfg = EmbeddedL1Config {
            dev_chain_spec: Arc::new(ChainSpec::default()),
            kind: L1ChainKind::Testing,
            datadir: PathBuf::from("/tmp/unused-eez-persistence-test"),
            http_port: 18_545,
            auth_port: 18_551,
            p2p_port: 30_303,
            discv5_port: 30_304,
            jwtsecret: None,
            trusted_peers: Vec::new(),
        };
        let testing = build_testing_node_config(&cfg).unwrap().engine;
        cfg.kind = L1ChainKind::Devnet;
        let devnet = build_devnet_node_config(&cfg).unwrap().engine;
        cfg.dev_chain_spec = Arc::new(ChainSpec::from(CHIADO_GENESIS.clone()));
        cfg.kind = L1ChainKind::TestingGnosis;
        let testing_gnosis = build_chiado_node_config(&cfg).unwrap().engine;
        cfg.kind = L1ChainKind::Chiado;
        let chiado = build_chiado_node_config(&cfg).unwrap().engine;
        for engine in [testing, testing_gnosis] {
            assert_eq!(engine.persistence_threshold, 2);
            assert_eq!(engine.num_state_masking_blocks, 0);
            assert_eq!(engine.memory_block_buffer_target(), 0);
        }
        let defaults = reth_node_core::args::EngineArgs::default();
        for engine in [devnet, chiado] {
            assert_eq!(engine.persistence_threshold, defaults.persistence_threshold);
            assert_eq!(
                engine.num_state_masking_blocks,
                defaults.num_state_masking_blocks
            );
            assert_eq!(
                engine.memory_block_buffer_target(),
                defaults.memory_block_buffer_target()
            );
        }
    }

    #[test]
    fn embedded_l1_http_rpc_exposes_classifier_trace_method() {
        let cfg = EmbeddedL1Config {
            dev_chain_spec: Arc::new(ChainSpec::default()),
            kind: L1ChainKind::Devnet,
            datadir: PathBuf::from("/tmp/unused-eez-l1-test"),
            http_port: 18_545,
            auth_port: 18_551,
            p2p_port: 30_303,
            discv5_port: 30_304,
            jwtsecret: None,
            trusted_peers: Vec::new(),
        };

        let (_, rpc_args) = build_network_rpc_args(&cfg).unwrap();
        let modules = rpc_args.http_api.unwrap().to_string();
        assert!(modules.contains("debug"), "HTTP modules: {modules}");
        assert!(modules.contains("eth"), "HTTP modules: {modules}");
        assert_eq!(
            build_testing_node_config(&cfg)
                .unwrap()
                .txpool
                .max_account_slots,
            usize::MAX
        );
        let mut gnosis = cfg.clone();
        gnosis.kind = L1ChainKind::TestingGnosis;
        gnosis.dev_chain_spec = Arc::new(ChainSpec::from(CHIADO_GENESIS.clone()));
        assert_eq!(
            build_chiado_node_config(&gnosis)
                .unwrap()
                .txpool
                .max_account_slots,
            usize::MAX
        );
        gnosis.kind = L1ChainKind::Chiado;
        assert_eq!(
            build_chiado_node_config(&gnosis)
                .unwrap()
                .txpool
                .max_account_slots,
            reth_node_core::args::TxPoolArgs::default().max_account_slots
        );
    }
}
