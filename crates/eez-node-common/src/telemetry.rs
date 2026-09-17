//! Configure the pinned Reth singleton before CLI startup registers metrics.
use eez_telemetry::native::{Config, LifecycleGuard, describe_process, prometheus_builder};
use reth_ethereum_cli::{Cli, Commands, chainspec::EthereumChainSpecParser};
use std::{
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

/// Existing CLI metrics settings have precedence over the new optional file.
/// With no EEZ_TELEMETRY_CONFIG, both initialization and CLI behavior are intact.
pub fn prepare<Ext>(
    cli: &mut Cli<EthereumChainSpecParser, Ext>,
    service: &str,
) -> eyre::Result<Prepared>
where
    Ext: clap::Args + std::fmt::Debug,
{
    let Some(config) = Config::from_env(service).map_err(|error| eyre::eyre!(error))? else {
        return Ok(Prepared::default());
    };
    configure_cli(cli, &config)?;
    // This hook populates the same OnceLock that both embedded clients and the
    // CLI subsequently use. Do not install a separate global metrics recorder.
    reth_node_metrics::recorder::try_install_prometheus_recorder_with_builder(prometheus_builder())?;
    describe_process();
    let (traces, layer) =
        eez_telemetry::traces::Guard::start(&config.identity, config.traces.as_ref())
            .map_err(|error| eyre::eyre!(error))?;
    Ok(Prepared {
        lifecycle: LifecycleGuard::start(&config),
        traces,
        layer,
    })
}

#[derive(Default)]
pub struct Prepared {
    lifecycle: LifecycleGuard,
    traces: eez_telemetry::traces::Guard,
    layer: Option<eez_telemetry::traces::TraceLayer>,
}

impl std::fmt::Debug for Prepared {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Prepared")
            .field("lifecycle", &self.lifecycle)
            .field("traces", &self.traces)
            .field("has_trace_layer", &self.layer.is_some())
            .finish()
    }
}

impl Prepared {
    /// Extend the existing Reth subscriber, preserving its file/stdout layers.
    pub fn run<Ext>(
        mut self,
        cli: Cli<EthereumChainSpecParser, Ext>,
        launcher: impl AsyncFnOnce(crate::L2NodeBuilder, Ext) -> eyre::Result<()>,
    ) -> eyre::Result<()>
    where
        Ext: clap::Args + std::fmt::Debug,
    {
        let mut app = cli.configure();
        if let Some(layer) = self.layer.take() {
            app.access_tracing_layers()?.add_layer(layer);
        }
        // This app never supplies a custom runner: CliApp constructs the pinned
        // CliRunner with this default configuration. Its launcher future is
        // dropped before run_command_until_exit starts the graceful-drain timer.
        // Starting our clock at that earlier boundary is conservative, including
        // time spent in Reth's subsequent runtime teardown.
        let budget =
            DrainBudget::new(reth_cli_runner::CliRunnerConfig::default().graceful_shutdown_timeout);
        let boundary = budget.clone();
        let result = app.run(reth_cli_commands::launcher::FnLauncher::new::<
            EthereumChainSpecParser,
            Ext,
        >(async move |builder, ext| {
            let _boundary = LauncherBoundary(boundary);
            launcher(builder, ext).await
        }));
        self.lifecycle.shutdown(Duration::ZERO);
        self.traces
            .shutdown(budget.remaining(result.is_ok(), Instant::now()));
        result
    }
}

/// Conservative lower bound on the runner's still-available graceful budget.
/// Startup failures and paths that never polled a launcher have no such budget.
#[derive(Clone, Debug)]
struct DrainBudget {
    started: Arc<OnceLock<Instant>>,
    graceful_timeout: Duration,
}
impl DrainBudget {
    fn new(graceful_timeout: Duration) -> Self {
        Self {
            started: Arc::new(OnceLock::new()),
            graceful_timeout,
        }
    }
    fn remaining(&self, succeeded: bool, now: Instant) -> Duration {
        if !succeeded {
            return Duration::ZERO;
        }
        self.started.get().map_or(Duration::ZERO, |started| {
            self.graceful_timeout
                .saturating_sub(now.saturating_duration_since(*started))
        })
    }
}
struct LauncherBoundary(DrainBudget);
impl Drop for LauncherBoundary {
    fn drop(&mut self) {
        let _ = self.0.started.set(Instant::now());
    }
}

fn configure_cli<Ext>(
    cli: &mut Cli<EthereumChainSpecParser, Ext>,
    config: &Config,
) -> eyre::Result<()>
where
    Ext: clap::Args + std::fmt::Debug,
{
    if config.traces.is_some() && cli.traces.otlp.is_some() {
        return Err(eyre::eyre!(
            "native trace configuration conflicts with Reth --tracing-otlp"
        ));
    }
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
            traces: None,
        };
        let mut cli = Cli::<EthereumChainSpecParser, crate::NoRoleArgs>::try_parse_from([
            "eez-composer",
            "node",
            "--dev",
        ])
        .unwrap();
        configure_cli(&mut cli, &config).unwrap();
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
        configure_cli(&mut cli, &config).unwrap();
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
        assert!(configure_cli(&mut cli, &config).is_err());
    }

    #[test]
    fn native_trace_layer_rejects_a_second_reth_otel_span_layer() {
        let mut cli = Cli::<EthereumChainSpecParser, crate::NoRoleArgs>::try_parse_from([
            "eez-composer",
            "node",
            "--dev",
            "--tracing-otlp=http://127.0.0.1:4318/v1/traces",
        ])
        .unwrap();
        let mut config = Config {
            identity: eez_telemetry::schema::Identity {
                environment: "test".into(),
                deployment: "test".into(),
                service: "eez-composer".into(),
                instance: "one".into(),
            },
            metrics: None,
            lifecycle: None,
            traces: Some(eez_telemetry::traces::Config {
                receiver: "127.0.0.1:4318".parse().unwrap(),
                allow_non_loopback: false,
                sample_ratio: 0.1,
                queue_capacity: 2048,
                export_timeout_ms: 1000,
            }),
        };
        assert!(configure_cli(&mut cli, &config).is_err());
        config.traces = None;
        // Existing Reth OTLP CLI usage remains valid without the new signal.
        assert!(configure_cli(&mut cli, &config).is_ok());
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
    #[test]
    fn trace_flush_uses_remaining_runner_budget_after_drain_and_never_invents_one() {
        let now = Instant::now();
        let budget = DrainBudget::new(Duration::from_secs(5));
        assert_eq!(budget.remaining(true, now), Duration::ZERO); // No launcher/startup failure.
        budget.started.set(now).unwrap();
        assert_eq!(budget.remaining(false, now), Duration::ZERO); // Runner error skips its graceful drain.
        let remaining = budget.remaining(true, now + Duration::from_secs(1));
        assert_eq!(remaining, Duration::from_secs(4));
        assert_eq!(
            eez_telemetry::traces::flush_budget(remaining),
            Duration::from_secs(1)
        );
        assert_eq!(
            budget.remaining(true, now + Duration::from_secs(5)),
            Duration::ZERO
        );
        assert_eq!(
            budget.remaining(true, now + Duration::from_secs(6)),
            Duration::ZERO
        );
        let generous = DrainBudget::new(Duration::from_secs(20));
        generous.started.set(now).unwrap();
        assert_eq!(
            eez_telemetry::traces::flush_budget(
                generous.remaining(true, now + Duration::from_secs(1))
            ),
            Duration::from_secs(2)
        );
    }

    #[test]
    fn dropping_the_launcher_records_the_pre_drain_boundary_once() {
        let budget = DrainBudget::new(Duration::from_secs(5));
        let before = Instant::now();
        drop(LauncherBoundary(budget.clone()));
        let after = Instant::now();
        let recorded = *budget.started.get().unwrap();
        assert!(recorded >= before && recorded <= after);
        drop(LauncherBoundary(budget.clone()));
        assert_eq!(*budget.started.get().unwrap(), recorded);
    }
}
