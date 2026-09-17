//! Entrypoint-owned optional configuration. The node installs this builder via
//! Reth's singleton; standalone processes can use `start_metrics` instead.
use std::{
    fs::File,
    io::Read,
    net::SocketAddr,
    path::Path,
    sync::OnceLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
use serde::{Deserialize, Serialize};

use crate::{Publisher, SpoolConfig, Writer, schema::Identity};

const MAX_CONFIG_BYTES: u64 = 64 * 1024;
pub const STAGE_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1., 2.5, 5., 10., 30., 60., 120., 300., 600.,
    1800.,
];
pub const QUEUE_BUCKETS: &[f64] = &[
    0.1, 1., 5., 10., 30., 60., 120., 300., 600., 1800., 3600., 21600., 86400.,
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsConfig {
    pub listen: SocketAddr,
    #[serde(default)]
    pub allow_non_loopback: bool,
}

impl MetricsConfig {
    pub fn validate_address(&self, address: SocketAddr) -> Result<(), &'static str> {
        if !address.ip().is_loopback() && !self.allow_non_loopback {
            return Err("non-loopback native metrics require allow_non_loopback=true");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub identity: Identity,
    pub metrics: Option<MetricsConfig>,
    pub lifecycle: Option<SpoolConfig>,
    pub traces: Option<crate::traces::Config>,
}

impl Config {
    /// Absent env var preserves existing startup; no default file search and no
    /// key-bearing deployment files are loaded here. Parse errors are redacted.
    pub fn from_env(expected_service: &str) -> Result<Option<Self>, &'static str> {
        std::env::var_os("EEZ_TELEMETRY_CONFIG")
            .map(|path| Self::load(Path::new(&path), expected_service))
            .transpose()
    }

    pub fn load(path: &Path, expected_service: &str) -> Result<Self, &'static str> {
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(|_| "cannot open EEZ telemetry config")?
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "cannot read EEZ telemetry config")?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err("EEZ telemetry config exceeds 64 KiB");
        }
        let config: Self =
            serde_json::from_slice(&bytes).map_err(|_| "invalid EEZ telemetry config")?;
        config.validate(expected_service)?;
        Ok(config)
    }

    pub fn validate(&self, expected_service: &str) -> Result<(), &'static str> {
        self.identity.validate()?;
        if self.identity.service != expected_service {
            return Err("telemetry service identity differs from executable role");
        }
        if let Some(metrics) = &self.metrics {
            metrics.validate_address(metrics.listen)?;
        }
        if let Some(lifecycle) = &self.lifecycle {
            lifecycle.validate()?;
        }
        if let Some(traces) = &self.traces {
            traces.validate()?;
        }
        if self.metrics.is_none() && self.lifecycle.is_none() && self.traces.is_none() {
            return Err("telemetry config must enable at least one signal");
        }
        Ok(())
    }
}

/// Suffix matching covers both the standalone and Reth-prefixed names, without
/// changing bucket/summary behavior for unrelated upstream metrics.
pub fn prometheus_builder() -> PrometheusBuilder {
    PrometheusBuilder::new()
        .set_buckets_for_metric(
            Matcher::Suffix("eez_stage_duration_seconds".into()),
            STAGE_BUCKETS,
        )
        .expect("static stage buckets are valid")
        .set_buckets_for_metric(
            Matcher::Suffix("eez_request_duration_seconds".into()),
            QUEUE_BUCKETS,
        )
        .expect("static request buckets are valid")
}

#[derive(Debug)]
pub struct MetricsGuard {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MetricsGuard {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Standalone process only. Embedded nodes MUST use Reth's singleton hook.
pub fn start_metrics(config: &MetricsConfig) -> Result<MetricsGuard, &'static str> {
    config.validate_address(config.listen)?;
    let (recorder, exporter) = prometheus_builder()
        .with_http_listener(config.listen)
        .build()
        .map_err(|_| "cannot start private metrics listener")?;
    metrics::set_global_recorder(recorder).map_err(|_| "metrics recorder already installed")?;
    let task = tokio::spawn(async move {
        if exporter.await.is_err() {
            tracing::error!("native metrics listener failed");
        }
    });
    Ok(MetricsGuard { task })
}

pub fn describe_process() {
    metrics::describe_gauge!("eez_build_info", "EEZ executable build version");
    metrics::gauge!("eez_build_info", "version" => env!("CARGO_PKG_VERSION")).set(1.);
    if let Ok(elapsed) = SystemTime::now().duration_since(UNIX_EPOCH) {
        metrics::gauge!("eez_process_start_timestamp_seconds").set(elapsed.as_secs_f64());
    }
}

static PUBLISHER: OnceLock<Publisher> = OnceLock::new();

/// A shared, immutable process handle. Runtime code still captures under its
/// own state owner; the telemetry handle never owns or mutates protocol state.
pub fn publisher() -> Option<&'static Publisher> {
    PUBLISHER.get()
}

#[derive(Debug, Default)]
pub struct LifecycleGuard {
    writer: Option<Writer>,
}

impl LifecycleGuard {
    pub fn start(config: &Config) -> Self {
        if config.metrics.is_some() {
            crate::operational::enable();
            if config.identity.service == "eez-proof-signer" {
                crate::operational::initialize_signer_metrics();
            }
        }
        let Some(spool) = &config.lifecycle else {
            return Self::default();
        };
        if config.metrics.is_some() {
            for reason in [
                "capture_limit",
                "capture_context",
                "clock",
                "encoding",
                "queue_full",
                "writer_closed",
                "shutdown",
                "io",
            ] {
                metrics::counter!("eez_telemetry_dropped_events_total", "signal" => "lifecycle", "reason" => reason).increment(0);
            }
        }
        match Writer::start(config.identity.clone(), spool.clone()) {
            Ok((publisher, writer)) => {
                if PUBLISHER.set(publisher).is_err() {
                    eprintln!(
                        "EEZ lifecycle exporter disabled: process exporter already initialized"
                    );
                    return Self::default();
                }
                metrics::gauge!("eez_lifecycle_writer_enabled").set(1.);
                Self {
                    writer: Some(writer),
                }
            }
            Err(_) => {
                // Node tracing has not necessarily initialized yet. Never echo
                // configuration bytes and never stop settlement on spool loss.
                eprintln!("EEZ lifecycle exporter disabled: storage or writer lock unavailable");
                metrics::gauge!("eez_lifecycle_writer_enabled").set(0.);
                Self::default()
            }
        }
    }

    pub fn shutdown(mut self, remaining: Duration) -> bool {
        self.writer
            .take()
            .is_none_or(|writer| writer.shutdown(remaining))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_util::layers::{Layer, PrefixLayer};

    #[test]
    fn metrics_activation_is_independent_of_the_lifecycle_writer() {
        // This is the only telemetry-unit test that enables the process flag.
        // Pool tests use an instance-local helper in their separate test binary.
        assert!(!crate::operational::enabled());
        assert!(LifecycleGuard::default().shutdown(Duration::ZERO));
        assert!(!crate::operational::enabled());
        let directory = tempfile::tempdir().unwrap();
        let mut config: Config = serde_json::from_value(serde_json::json!({
            "identity": {"environment":"devnet", "deployment":"test", "service":"eez-composer", "instance":"metrics-test"},
            "lifecycle": {"directory": directory.path().join("spool")}
        })).unwrap();
        config.validate("eez-composer").unwrap();
        let lifecycle = LifecycleGuard::start(&config);
        assert!(lifecycle.writer.is_some());
        assert!(!crate::operational::enabled());
        assert!(lifecycle.shutdown(Duration::from_secs(8)));
        config.lifecycle = None;
        config.metrics = Some(MetricsConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            allow_non_loopback: false,
        });
        config.validate("eez-composer").unwrap();
        let metrics_only = LifecycleGuard::start(&config);
        assert!(metrics_only.writer.is_none());
        assert!(crate::operational::enabled());
        assert!(metrics_only.shutdown(Duration::ZERO));
    }

    #[tokio::test]
    async fn standalone_listener_exposes_metrics_and_rejects_duplicate_global_install() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = reservation.local_addr().unwrap();
        drop(reservation);
        let guard = start_metrics(&MetricsConfig {
            listen: address,
            allow_non_loopback: false,
        })
        .unwrap();
        describe_process();
        metrics::histogram!("eez_stage_duration_seconds", "stage" => "signer", "outcome" => "ok")
            .record(0.1);
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        let response = String::from_utf8(bytes).unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.contains("eez_build_info"));
        assert!(response.contains("eez_stage_duration_seconds_bucket"));
        assert!(
            start_metrics(&MetricsConfig {
                listen: "127.0.0.1:0".parse().unwrap(),
                allow_non_loopback: false
            })
            .is_err()
        );
        drop(guard);
    }

    #[test]
    fn histogram_buckets_and_names_work_with_the_reth_prefix() {
        for prefix in ["", "reth."] {
            let recorder = prometheus_builder().build_recorder();
            let handle = recorder.handle();
            let record = || {
                metrics::histogram!("eez_stage_duration_seconds", "stage" => "signer", "outcome" => "ok").record(0.25);
                metrics::histogram!("eez_request_duration_seconds", "outcome" => "committed")
                    .record(600.);
                describe_process();
            };
            if prefix.is_empty() {
                metrics::with_local_recorder(&recorder, record);
            } else {
                metrics::with_local_recorder(&PrefixLayer::new("reth").layer(recorder), record);
            }
            let rendered = handle.render();
            let metric = format!("{}eez_stage_duration_seconds", prefix.replace('.', "_"));
            assert!(
                rendered.contains(&format!("# TYPE {metric} histogram")),
                "{rendered}"
            );
            assert!(
                rendered.contains(&format!(
                    "{metric}_bucket{{stage=\"signer\",outcome=\"ok\",le=\"0.25\"}} 1"
                )),
                "{rendered}"
            );
            assert!(!rendered.contains("quantile="));
            assert!(!rendered.contains("deployment="));
            assert!(!rendered.contains("epoch="));
        }
    }

    #[test]
    fn config_requires_matching_role_private_binding_and_no_unknown_fields() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        let mut config = serde_json::json!({
            "identity": {"environment":"devnet","deployment":"test","service":"eez-composer","instance":"one"},
            "metrics": {"listen":"127.0.0.1:19101"}
        });
        std::fs::write(&path, config.to_string()).unwrap();
        assert!(Config::load(&path, "eez-composer").is_ok());
        assert!(Config::load(&path, "eez-proof-signer").is_err());
        config["metrics"]["listen"] = "0.0.0.0:19101".into();
        std::fs::write(&path, config.to_string()).unwrap();
        assert!(Config::load(&path, "eez-composer").is_err());
        config["metrics"]["allow_non_loopback"] = true.into();
        std::fs::write(&path, config.to_string()).unwrap();
        assert!(Config::load(&path, "eez-composer").is_ok());
        config["metrics"]["secret-typo"] = "never echo this".into();
        std::fs::write(&path, config.to_string()).unwrap();
        assert_eq!(
            Config::load(&path, "eez-composer").unwrap_err(),
            "invalid EEZ telemetry config"
        );
    }
    #[test]
    fn trace_only_file_reaches_validated_native_config_with_explicit_sampling() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("trace.json");
        let mut config = serde_json::json!({
            "identity": {"environment":"devnet","deployment":"test","service":"eez-proof-signer","instance":"one"},
            "traces": {"receiver":"127.0.0.1:4318", "sample_ratio":0.25}
        });
        std::fs::write(&path, config.to_string()).unwrap();
        let parsed = Config::load(&path, "eez-proof-signer").unwrap();
        assert!(parsed.metrics.is_none());
        assert!(parsed.lifecycle.is_none());
        let traces = parsed.traces.unwrap();
        assert_eq!(traces.sample_ratio.to_bits(), 0.25_f64.to_bits());
        assert_eq!(traces.queue_capacity, 2048);
        assert_eq!(traces.export_timeout_ms, 1000);
        for fields in [
            serde_json::json!({"receiver":"127.0.0.1:4318"}),
            serde_json::json!({"receiver":"127.0.0.1:4318","sample_ratio":2}),
            serde_json::json!({"receiver":"127.0.0.1:4318","sample_ratio":1,"headers":{"secret":"redacted"}}),
        ] {
            config["traces"] = fields;
            std::fs::write(&path, config.to_string()).unwrap();
            assert!(Config::load(&path, "eez-proof-signer").is_err());
        }
    }
}
