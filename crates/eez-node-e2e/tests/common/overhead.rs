//! Explicit, owned, prebuilt performance arm. Never part of the ordinary suite.
#[path = "observer.rs"]
mod observer;

use alloy_provider::{Provider, ProviderBuilder};
use anyhow::{Context, Result, ensure};
use eez_testkit::{NativeL1Profile, NodeBinary, NodeConfig, NodeHandle, PortLease};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::Read,
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    arm: String,
    probe: bool,
    waves: usize,
    #[serde(default = "default_arm_seconds")]
    arm_seconds: u64,
    #[serde(default = "default_experiment_seconds")]
    experiment_seconds: u64,
    receiver: SocketAddr,
    directory: PathBuf,
}
fn default_arm_seconds() -> u64 {
    600
}
fn default_experiment_seconds() -> u64 {
    9000
}
impl Config {
    fn validate_limits(&self) -> Result<()> {
        ensure!((1..=64).contains(&self.waves), "waves outside fixed bound");
        ensure!(
            (1..=1800).contains(&self.arm_seconds),
            "arm budget outside fixed bound"
        );
        ensure!(
            (1..=21600).contains(&self.experiment_seconds),
            "experiment budget outside fixed bound"
        );
        ensure!(
            self.experiment_seconds >= self.arm_seconds,
            "experiment budget cannot be shorter than one arm allowance"
        );
        Ok(())
    }
    fn validate(&self) -> Result<()> {
        ensure!(matches!(self.arm.as_str(), "A" | "B"), "arm must be A or B");
        self.validate_limits()?;
        ensure!(
            self.receiver.ip().is_loopback() && self.receiver.port() != 0,
            "owned loopback receiver required"
        );
        ensure!(
            self.directory.is_absolute() && self.directory.is_dir(),
            "existing absolute evidence directory required"
        );
        ensure!(
            !self.probe || cfg!(feature = "benchmark-lock-timing"),
            "probe needs explicitly built feature"
        );
        ensure!(
            std::env::var_os("EEZ_TEST_PREBUILT").is_some(),
            "measurement requires frozen prebuilt binaries"
        );
        Ok(())
    }
}
fn write(path: &Path, value: &Value) -> Result<()> {
    let temporary = path.with_extension("pending");
    std::fs::write(&temporary, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}
async fn barrier(path: &Path, deadline: Instant) -> Result<()> {
    let remaining = deadline
        .saturating_duration_since(Instant::now())
        .min(Duration::from_secs(30));
    ensure!(
        !remaining.is_zero(),
        "arm budget exhausted at controller barrier"
    );
    eez_testkit::wait_for(remaining, || async { Ok(path.exists().then_some(())) }).await
}
async fn queue_bracket(world: &eez_testkit::CrossChainWorld) -> Result<()> {
    let value: Value = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()?
        .post(world.l2_rpc())
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"eez_getCrossChainQueueStatus","params":[]}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    ensure!(
        value.get("result").is_some() && value.get("error").is_none(),
        "queue bracket failed"
    );
    Ok(())
}
async fn scrape(address: &str) -> Result<String> {
    let text = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()?
        .get(format!("http://{address}/metrics"))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    ensure!(text.len() <= 4 * 1024 * 1024, "metrics exceeded bound");
    Ok(text)
}
async fn canonical_sources(
    observer: &observer::Observer,
    sources: &[alloy_primitives::B256],
) -> Result<()> {
    // HTTP renders cached worker results. Do not repeatedly execute the costly
    // exact-source SQL view while measuring backend catch-up.
    eez_testkit::wait_for(Duration::from_secs(180), || async {
        let metrics = observer.metrics().await?;
        let valid = metrics
            .lines()
            .any(|line| line == "eez_observer_canonical_outcome_evaluation_valid 1");
        let count = metrics
            .lines()
            .filter(|line| {
                line.starts_with("eez_observer_canonical_terminal_admissions{")
                    && line.contains("outcome=\"committed\"")
            })
            .filter_map(|line| line.split_whitespace().last()?.parse::<u64>().ok())
            .sum::<u64>();
        Ok((valid && count >= sources.len() as u64).then_some(()))
    })
    .await?;
    let identities = sources
        .iter()
        .map(|hash| format!("'{hash:x}'"))
        .collect::<Vec<_>>()
        .join(",");
    let actual = observer.sql(&format!("SELECT count(*)={} AND count(DISTINCT source->>'transaction_hash')={} FROM eez.lifecycle_admission_status WHERE source->>'transaction_hash' IN ({identities}) AND status='canonical' AND outcome='committed' AND evidence_class='node_reconciled'",sources.len(),sources.len())).await?;
    ensure!(
        actual == "t",
        "cached population did not cover exact measured source identities"
    );
    Ok(())
}

async fn native_configuration(
    config: &Config,
    world: &eez_testkit::CrossChainWorld,
    observer: &observer::Observer,
) -> Result<()> {
    let mut genesis_evidence = serde_json::Map::new();
    for (role, path, rpc) in [
        ("l1", &world.cfg.l1_genesis.0, world.l1_rpc()),
        ("l2", &world.cfg.l2_genesis.0, world.l2_rpc()),
    ] {
        let bytes = std::fs::read(path)?;
        let genesis: alloy_genesis::Genesis = serde_json::from_slice(&bytes)?;
        ensure!(
            genesis.timestamp == world.cfg.genesis_time,
            "fixture timestamps diverged"
        );
        std::fs::write(config.directory.join(format!("{role}-genesis.json")), bytes)?;
        let header_hash = eez_testkit::genesis_block_hash(path)?;
        let provider = ProviderBuilder::new().connect_http(rpc.parse()?);
        let actual = provider
            .get_block_by_number(alloy_eips::BlockNumberOrTag::Number(0))
            .await?
            .context("actual genesis block")?
            .header
            .hash;
        ensure!(
            header_hash == actual,
            "helper header differs from actual {role} genesis"
        );
        genesis_evidence.insert(role.to_owned(), json!({"timestamp_seconds":genesis.timestamp,"header_hash":header_hash,"rpc_genesis_hash":actual,"header_hash_source":"eez_testkit::genesis_block_hash"}));
    }
    write(
        &config.directory.join("genesis_evidence.json"),
        &Value::Object(genesis_evidence),
    )?;
    write(
        &config.directory.join("native-effective.json"),
        &json!({
            "schema_version":1,
            "native_l1_profile":"gnosis-prague",
            "workers":{"measurement_tokio_workers":4,"native_policy":"pinned entrypoint defaults; no worker override"},
            "protocol":{"effective_reviewed_environment":world.measurement_protocol_settings(),"absent_setting":"pinned runtime default","prover_transport":"direct owned gRPC; no mutation proxy"},
            "logging":{"stdout_format":"json","stdout_filter":std::env::var("EEZ_TEST_LOG").context("measurement requires explicit shared log filter")?,"file_max_files":0,"stdout_writer":"pinned Reth synchronous fmt; owned regular file","signer_filter":"info"},
            "observer_indexer":observer.measurement_settings()?,
            "workload":{"measured_waves":config.waves,"arm_seconds":config.arm_seconds,"experiment_seconds":config.experiment_seconds,"stages":[{"name":"small_queue","successful_sources":8*config.waves},{"name":"busy_waves","successful_sources":128*config.waves}]}
        }),
    )?;
    Ok(())
}

async fn run(config: Config) -> Result<()> {
    config.validate()?;
    let deadline = Instant::now() + Duration::from_secs(config.arm_seconds);
    let enabled = config.arm == "B";
    let spool = config.directory.join("lifecycle");
    std::fs::create_dir(&spool)?;
    std::fs::set_permissions(&spool, std::fs::Permissions::from_mode(0o700))?;
    let mut leases = Vec::new();
    let mut addresses = BTreeMap::new();
    for role in ["composer", "signer", "follower"] {
        let lease = PortLease::tcp();
        addresses.insert(role, format!("127.0.0.1:{}", lease.port()));
        leases.push(lease);
        let service = match role {
            "composer" => "eez-composer",
            "signer" => "eez-proof-signer",
            _ => "eez-follower",
        };
        let mut native = json!({"identity":{"environment":"devnet","deployment":"overhead","service":service,"instance":"fixture"}, "metrics":{"listen":addresses[role]}});
        if role != "follower" {
            native["traces"] = json!({"receiver":config.receiver,"sample_ratio":1.0,"queue_capacity":2048,"export_timeout_ms":1000});
        }
        if role == "composer" {
            native["lifecycle"] = json!({"directory":spool});
        }
        write(&config.directory.join(format!("{role}.json")), &native)?;
    }
    let mut environment = super::env();
    environment.push(("EEZ_BENCH_HELD_POOL_TIMING", config.probe.to_string()));
    if enabled {
        environment.push((
            "EEZ_TELEMETRY_CONFIG",
            config
                .directory
                .join("composer.json")
                .to_string_lossy()
                .into_owned(),
        ));
    }
    // The fixture itself leases the ordinary runtime ports. These three leases
    // cover optional exporter addresses until immediately before role startup.
    drop(leases);
    let signer_config = config.directory.join("signer.json");
    let world = eez_testkit::setup_cross_chain_with_profile_and_signer_telemetry(
        NativeL1Profile::GnosisPrague,
        &environment,
        enabled.then_some(signer_config.as_path()),
    )
    .await?;
    let mut follower_env = world.partial_follower_env();
    if enabled {
        follower_env.push((
            "EEZ_TELEMETRY_CONFIG",
            config
                .directory
                .join("follower.json")
                .to_string_lossy()
                .into_owned(),
        ));
    }
    let follower = NodeHandle::start(
        "overhead-follower",
        &NodeConfig {
            binary: NodeBinary::Follower,
            genesis_path: Some(&world.cfg.l2_genesis.0),
        },
        &follower_env,
    )
    .await?;
    let observer = observer::Observer::prepare(&world, &spool, "overhead").await?;
    if enabled {
        observer.start_prepared().await?;
        addresses.insert("observer", observer.metrics_address()?);
    } else {
        addresses.clear();
    }
    native_configuration(&config, &world, &observer).await?;
    write(
        &config.directory.join("ready.json"),
        &json!({"processes":{"composer":world.node.process_id(),"signer":world.proof_signer.process_id(),"follower":follower.process_id(),"observer":observer.process_id()},"postgres_container":observer.container_name(),"metrics":addresses,"telemetry_enabled":enabled}),
    )?;
    if enabled {
        for role in ["composer", "signer"] {
            let metrics = scrape(&addresses[role]).await?;
            ensure!(
                metrics
                    .lines()
                    .any(|line| line.ends_with("eez_trace_export_enabled 1")),
                "trace profile disabled in {role}"
            );
            if role == "composer" {
                ensure!(
                    metrics
                        .lines()
                        .any(|line| line.ends_with("eez_lifecycle_writer_enabled 1")),
                    "lifecycle profile disabled before measurement"
                );
            }
        }
    }
    barrier(&config.directory.join("start"), deadline).await?;
    let warmup = super::measured_mixed_wave(&world, &follower, 1, 1).await;
    let mut all_sources = warmup.source_hashes;
    if enabled {
        canonical_sources(&observer, &all_sources).await?;
    }
    let mut stages = Vec::new();
    let mut next_value = 10;
    for (name, count) in [("small_queue", 4), ("busy_waves", 64)] {
        queue_bracket(&world).await?;
        tokio::time::sleep(Duration::from_millis(10)).await;
        let start_unix_ns = super::measurement_unix_ns();
        let started = Instant::now();
        let mut waves = Vec::new();
        let mut elapsed_seconds = 0.;
        let mut preparation_ms = Vec::new();
        let mut preparation_ns = Vec::new();
        let mut preparation_group_counts = Vec::new();
        for _ in 0..config.waves {
            let wave = super::measured_mixed_wave(&world, &follower, count, next_value).await;
            next_value += count;
            elapsed_seconds += wave.elapsed_seconds;
            all_sources.extend_from_slice(&wave.source_hashes);
            for event in &wave.events {
                if event.name == "eez.composer.admission.prepared" {
                    let nanos = event.fields["preparation_ns"]
                        .as_u64()
                        .context("nanosecond preparation clock missing or overflowed")?;
                    ensure!(
                        event.fields["preparation_ms"].as_u64() == Some(nanos / 1_000_000),
                        "native preparation clocks disagree"
                    );
                    preparation_ns.push(nanos);
                    preparation_ms.push(
                        event.fields["preparation_ms"]
                            .as_u64()
                            .context("preparation duration missing")?,
                    );
                    preparation_group_counts.push(
                        event.fields["accepted"]
                            .as_u64()
                            .context("preparation accepted groups missing")?,
                    );
                }
            }
            waves.push(json!({"start_unix_ns":wave.started_unix_ns,"end_unix_ns":wave.finished_unix_ns,"elapsed_seconds":wave.elapsed_seconds,"successful_sources":wave.source_count,"source_hashes":wave.source_hashes}));
        }
        let resource_bracket_elapsed_seconds = started.elapsed().as_secs_f64();
        let end_unix_ns = super::measurement_unix_ns();
        tokio::time::sleep(Duration::from_millis(10)).await;
        queue_bracket(&world).await?;
        stages.push(json!({"name":name,"start_unix_ns":start_unix_ns,"end_unix_ns":end_unix_ns,"elapsed_seconds":elapsed_seconds,"resource_bracket_elapsed_seconds":resource_bracket_elapsed_seconds,"throughput_clock":"sum of verified submission-to-canonical wave intervals; setup and inter-wave signature verification excluded","successful_sources":count*2*config.waves as u64,"preparation_ms":preparation_ms,"preparation_ns":preparation_ns,"preparation_group_counts":preparation_group_counts,"waves":waves}));
    }
    write(
        &config.directory.join("progress.json"),
        &json!({"semantic_ok":true,"telemetry_healthy":null,"stages":stages,"all_source_hashes":all_sources}),
    )?;
    let catchup_start = super::measurement_unix_ns();
    let mut health = BTreeMap::new();
    if enabled {
        canonical_sources(&observer, &all_sources).await?;
        observer
            .canonical_commits(&["inbound", "outbound"], "overhead-complete")
            .await?;
        for (role, address) in &addresses {
            let metrics = scrape(address).await?;
            if matches!(*role, "composer" | "signer") {
                let exported = metrics
                    .lines()
                    .filter(|line| !line.starts_with('#'))
                    .find_map(|line| {
                        let (name, value) = line.split_once(' ')?;
                        name.ends_with("eez_trace_exported_spans_total")
                            .then(|| value.parse::<f64>().ok())
                            .flatten()
                    });
                ensure!(
                    exported.is_some_and(|count| count > 0.),
                    "no actual exported spans in {role}"
                );
            }
            if *role == "composer" {
                ensure!(
                    metrics
                        .lines()
                        .any(|line| line.ends_with("eez_lifecycle_writer_enabled 1")),
                    "lifecycle writer disabled"
                );
            }
            for line in metrics.lines().filter(|line| {
                (line.contains("eez_telemetry_dropped_events_total{")
                    || line.contains("eez_telemetry_writer_errors_total{"))
                    && !line.starts_with('#')
            }) {
                ensure!(
                    line.split_whitespace().last() == Some("0"),
                    "native telemetry loss in {role}"
                );
            }
            std::fs::write(
                config.directory.join(format!("{role}-final.prom")),
                &metrics,
            )?;
            health.insert(*role, true);
        }
    }
    let catchup_end = super::measurement_unix_ns();
    let mut resource_phases = stages.clone();
    resource_phases.push(json!({"name":"observer_catchup_and_health","start_unix_ns":catchup_start,"end_unix_ns":catchup_end}));
    // Keep every sampled PID and the owned PG reservation alive until the
    // controller has taken its trailing resource sample and stopped sampling.
    write(
        &config.directory.join("evidence.json"),
        &json!({"schema_version":1,"preparation_clock":{"source":"admission.prepared.preparation_ns","unit":"ns","resolution_ns":1},"arm":config.arm,"probe":config.probe,"semantic_ok":true,"telemetry_healthy":true,"health":health,"stages":stages,"resource_phases":resource_phases,"verified_observer_source_count":if enabled {all_sources.len()} else {0},"measured_waves":config.waves,"extra_queue_brackets_per_arm":4}),
    )?;
    barrier(&config.directory.join("finish"), deadline).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit owned, fixed-binary overhead experiment"]
async fn observability_overhead_arm() {
    let path = std::env::var_os("EEZ_OVERHEAD_ARM_CONFIG").expect("explicit arm configuration");
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .unwrap()
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes.len() <= 16 * 1024, "arm configuration exceeds bound");
    let config: Config = serde_json::from_slice(&bytes).unwrap();
    config
        .validate()
        .expect("invalid explicit measurement configuration");
    let budget = Duration::from_secs(config.arm_seconds);
    tokio::time::timeout(budget, run(config))
        .await
        .expect("arm exceeded its declared bounded budget")
        .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Value {
        json!({"arm":"A","probe":false,"waves":1,"receiver":"127.0.0.1:4318","directory":"/tmp"})
    }
    #[test]
    fn absent_budgets_preserve_defaults_and_explicit_maxima_validate() {
        let default: Config = serde_json::from_value(config()).unwrap();
        assert_eq!(
            (
                default.waves,
                default.arm_seconds,
                default.experiment_seconds
            ),
            (1, 600, 9000)
        );
        default.validate_limits().unwrap();
        let mut maximum = config();
        maximum["waves"] = json!(64);
        maximum["arm_seconds"] = json!(1800);
        maximum["experiment_seconds"] = json!(21600);
        serde_json::from_value::<Config>(maximum)
            .unwrap()
            .validate_limits()
            .unwrap();
    }
    #[test]
    fn invalid_limits_and_combinations_fail_before_runtime() {
        for (field, value) in [
            ("waves", 0),
            ("waves", 65),
            ("arm_seconds", 0),
            ("arm_seconds", 1801),
            ("experiment_seconds", 0),
            ("experiment_seconds", 21601),
            ("experiment_seconds", 599),
        ] {
            let mut invalid = config();
            invalid[field] = json!(value);
            assert!(
                serde_json::from_value::<Config>(invalid)
                    .unwrap()
                    .validate_limits()
                    .is_err(),
                "{field}={value}"
            );
        }
        for value in [json!(-1), json!(true), json!("600")] {
            let mut invalid = config();
            invalid["arm_seconds"] = value;
            assert!(serde_json::from_value::<Config>(invalid).is_err());
        }
    }
}
