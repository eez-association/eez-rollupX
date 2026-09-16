//! Configure the pinned Reth singleton before CLI startup registers metrics.
use eez_telemetry::native::{Config, LifecycleGuard, describe_process, prometheus_builder};
use reth_ethereum_cli::{Cli, Commands, chainspec::EthereumChainSpecParser};

/// Existing CLI metrics settings have precedence over the new optional file.
/// With no EEZ_TELEMETRY_CONFIG, both initialization and CLI behavior are intact.
pub fn prepare<Ext>(
    cli: &mut Cli<EthereumChainSpecParser, Ext>,
    service: &str,
) -> eyre::Result<LifecycleGuard>
where
    Ext: clap::Args + std::fmt::Debug,
{
    let Some(config) = Config::from_env(service).map_err(|error| eyre::eyre!(error))? else {
        return Ok(LifecycleGuard::default());
    };
    configure_metrics(cli, &config)?;
    // This hook populates the same OnceLock that both embedded clients and the
    // CLI subsequently use. Do not install a separate global metrics recorder.
    reth_node_metrics::recorder::try_install_prometheus_recorder_with_builder(prometheus_builder())?;
    describe_process();
    Ok(LifecycleGuard::start(&config))
}

fn configure_metrics<Ext>(
    cli: &mut Cli<EthereumChainSpecParser, Ext>,
    config: &Config,
) -> eyre::Result<()>
where
    Ext: clap::Args + std::fmt::Debug,
{
    let Commands::Node(node) = &mut cli.command else {
        return Err(eyre::eyre!(
            "EEZ_TELEMETRY_CONFIG is supported only for the node command"
        ));
    };
    if let Some(metrics) = &config.metrics {
        let listen = *node.metrics.prometheus.get_or_insert(metrics.listen);
        metrics
            .validate_address(listen)
            .map_err(|error| eyre::eyre!(error))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn file_listener_reaches_node_config_and_cli_keeps_precedence() {
        let config = Config {
            identity: eez_telemetry::schema::Identity {
                environment: "test".into(),
                deployment: "test".into(),
                service: "eez-composer".into(),
                instance: "one".into(),
            },
            metrics: Some(eez_telemetry::native::MetricsConfig {
                listen: "127.0.0.1:19101".parse().unwrap(),
                allow_non_loopback: false,
            }),
            lifecycle: None,
        };
        let mut cli = Cli::<EthereumChainSpecParser, crate::NoRoleArgs>::try_parse_from([
            "eez-composer",
            "node",
            "--dev",
        ])
        .unwrap();
        configure_metrics(&mut cli, &config).unwrap();
        let Commands::Node(node) = cli.command else {
            panic!("node command");
        };
        assert_eq!(
            node.metrics.prometheus,
            Some("127.0.0.1:19101".parse().unwrap())
        );
        let mut cli = Cli::<EthereumChainSpecParser, crate::NoRoleArgs>::try_parse_from([
            "eez-composer",
            "node",
            "--dev",
            "--metrics",
            "127.0.0.1:19102",
        ])
        .unwrap();
        configure_metrics(&mut cli, &config).unwrap();
        let Commands::Node(node) = cli.command else {
            panic!("node command");
        };
        assert_eq!(
            node.metrics.prometheus,
            Some("127.0.0.1:19102".parse().unwrap())
        );
        let mut cli = Cli::<EthereumChainSpecParser, crate::NoRoleArgs>::try_parse_from([
            "eez-composer",
            "node",
            "--dev",
            "--metrics",
            "0.0.0.0:19102",
        ])
        .unwrap();
        assert!(configure_metrics(&mut cli, &config).is_err());
    }

    #[test]
    fn configured_recorder_is_the_same_singleton_reused_by_node_startup() {
        let configured = reth_node_metrics::recorder::try_install_prometheus_recorder_with_builder(
            eez_telemetry::native::prometheus_builder(),
        )
        .unwrap();
        let reused = reth_node_metrics::recorder::install_prometheus_recorder();
        assert!(std::ptr::eq(configured, reused));
        eez_telemetry::native::describe_process();
        let rendered = reused.handle().render();
        assert!(rendered.contains("reth_eez_build_info"), "{rendered}");
        assert!(rendered.contains("reth_eez_process_start_timestamp_seconds"));
    }
}
