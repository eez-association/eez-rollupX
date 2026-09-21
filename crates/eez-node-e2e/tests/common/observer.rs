//! Live observer fixture shared by native lifecycle, restart and reorg tests.
//! Docker owns only PostgreSQL; the actual observer reads live RPC and spool.
#![allow(dead_code)] // Each integration target uses a different subset of this shared fixture.

use alloy_primitives::{B256, U256};
use alloy_provider::{Provider, ProviderBuilder};
use anyhow::{Context, Result, ensure};
use eez_testkit::{CrossChainWorld, PortLease, repo_root, wait_for};
use serde_json::{Value, json};
use std::{
    fs::OpenOptions,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::io::AsyncWriteExt;

const DEADLINE: Duration = Duration::from_secs(180);

pub struct Telemetry {
    directory: tempfile::TempDir,
    pub spool: PathBuf,
    pub deployment: &'static str,
}
impl Telemetry {
    pub fn new(deployment: &'static str) -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let spool = directory.path().join("lifecycle");
        let config = json!({"identity":{"environment":"devnet","deployment":deployment,
            "service":"eez-composer","instance":"fixture"},"lifecycle":{"directory":spool}});
        std::fs::write(
            directory.path().join("telemetry.json"),
            serde_json::to_vec(&config)?,
        )?;
        Ok(Self {
            directory,
            spool,
            deployment,
        })
    }
    pub fn env(&self) -> (&'static str, String) {
        (
            "EEZ_TELEMETRY_CONFIG",
            self.directory
                .path()
                .join("telemetry.json")
                .to_string_lossy()
                .into_owned(),
        )
    }
}

async fn output(command: &mut tokio::process::Command, input: Option<&str>) -> Result<String> {
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn()?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .context("command stdin")?
            .write_all(input.as_bytes())
            .await?;
    }
    let result = tokio::time::timeout(Duration::from_secs(120), child.wait_with_output()).await??;
    ensure!(
        result.status.success(),
        "fixture command failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    ensure!(
        result.stdout.len() <= 4 * 1024 * 1024,
        "fixture command response exceeded bound"
    );
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}

fn binary() -> Result<PathBuf> {
    if let Some(directory) = std::env::var_os("EEZ_TEST_RUNTIME_BIN_DIR") {
        let path = PathBuf::from(directory).join("eez-observer");
        ensure!(path.is_file(), "verified runtime payload lacks observer");
        return Ok(path);
    }
    ensure!(
        std::env::var_os("EEZ_TEST_PREBUILT").is_none(),
        "archived tests require their verified observer runtime"
    );
    let executable = std::env::current_exe()?;
    let path = executable
        .parent()
        .and_then(Path::parent)
        .context("test target directory")?
        .join("eez-observer");
    ensure!(
        path.is_file(),
        "build current eez-observer before live native observer tests"
    );
    Ok(path)
}

/// Private parent reservations created by the overhead controller before launch.
/// Ordinary functional fixtures do not use this contract.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReservedParents {
    pub database_parent: PathBuf,
    pub socket_parent: PathBuf,
    pub container_name: String,
}

impl ReservedParents {
    pub fn validate(&self, owner: &Path) -> Result<()> {
        let token = self
            .container_name
            .strip_prefix("eez-native-observer-")
            .context("invalid reserved container prefix")?;
        ensure!(
            token.len() == 32
                && token
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "invalid reserved container token"
        );
        ensure!(
            self.database_parent
                == repo_root()
                    .join("target/native-observer-databases")
                    .join(format!("overhead-{token}")),
            "unexpected database reservation"
        );
        ensure!(
            self.socket_parent == Path::new("/tmp").join(format!("eez-opg-{token}")),
            "unexpected socket reservation"
        );
        for path in [&self.database_parent, &self.socket_parent] {
            ensure!(
                path.is_absolute() && path.canonicalize()? == *path,
                "reservation must be canonical and not a symlink"
            );
            let metadata = std::fs::symlink_metadata(path)?;
            ensure!(
                metadata.is_dir()
                    && metadata.uid() == owner.metadata()?.uid()
                    && metadata.permissions().mode() & 0o777 == 0o700,
                "reservation ownership or permissions differ"
            );
            ensure!(
                std::fs::read_dir(path)?.next().is_none(),
                "reservation must be empty before setup"
            );
        }
        Ok(())
    }
}

pub struct Observer {
    container: String,
    controller_owned: bool,
    directory: tempfile::TempDir,
    database_directory: Option<tempfile::TempDir>,
    postgres_settings: Value,
    executable: PathBuf,
    log: PathBuf,
    spool: PathBuf,
    child: Mutex<Option<Child>>,
    failure_captured: AtomicBool,
}

/// Keep the fixture alive while recording Result errors, panic, or cancellation.
/// The caller disarms this only after its last successful assertion/barrier.
#[must_use = "retain this guard until the operation succeeds"]
pub struct FailureCapture<'a> {
    observer: &'a Observer,
    complete: bool,
}

impl FailureCapture<'_> {
    pub fn success(mut self) {
        self.complete = true;
    }
}

impl Drop for FailureCapture<'_> {
    fn drop(&mut self) {
        if !self.complete {
            self.observer.capture_failure();
        }
    }
}

impl Observer {
    pub async fn start(world: &CrossChainWorld, spool: &Path, deployment: &str) -> Result<Self> {
        let fixture = Self::prepare(world, spool, deployment).await?;
        let failure_capture = fixture.capture_on_error();
        fixture.start_prepared().await?;
        failure_capture.success();
        Ok(fixture)
    }

    /// Reserve the same initialized database/configuration while keeping the
    /// observer idle, for a resource-matched disabled overhead arm.
    pub async fn prepare(world: &CrossChainWorld, spool: &Path, deployment: &str) -> Result<Self> {
        Self::prepare_reserved(world, spool, deployment, None).await
    }

    pub async fn prepare_reserved(
        world: &CrossChainWorld,
        spool: &Path,
        deployment: &str,
        reserved: Option<&ReservedParents>,
    ) -> Result<Self> {
        if let Some(parents) = reserved {
            parents.validate(spool)?;
        }
        let mut directory = if let Some(parents) = reserved {
            tempfile::Builder::new()
                .prefix("fixture-")
                .tempdir_in(&parents.socket_parent)?
        } else {
            tempfile::tempdir()?
        };
        // Retain controller-owned children even on setup errors before Self exists.
        directory.disable_cleanup(reserved.is_some());
        // Keep PGDATA with target artifacts instead of allocating a dedicated
        // tmpfs inside PostgreSQL's cgroup. Measurement rejects RAM-backed
        // targets separately; functional fixtures retain workspace flexibility.
        // Keep the short Unix socket/config path in /tmp.
        let database_root = reserved
            .map(|parents| parents.database_parent.clone())
            .unwrap_or_else(|| repo_root().join("target/native-observer-databases"));
        std::fs::create_dir_all(&database_root)?;
        let mut database_directory = tempfile::Builder::new()
            .prefix("eez-native-observer-")
            .tempdir_in(database_root)?;
        database_directory.disable_cleanup(reserved.is_some());
        let filesystem = output(
            tokio::process::Command::new("stat")
                .args(["--file-system", "--format=%T"])
                .arg(database_directory.path()),
            None,
        )
        .await?;
        let container = format!(
            "eez-native-observer-{}-{}",
            std::process::id(),
            directory
                .path()
                .file_name()
                .unwrap()
                .to_string_lossy()
                .trim_start_matches('.')
        );
        let container = reserved
            .map(|parents| parents.container_name.clone())
            .unwrap_or(container);
        let log_root = std::env::var_os("EEZ_TEST_LOG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| directory.path().to_owned());
        std::fs::create_dir_all(&log_root)?;
        let log = log_root.join(format!("{container}.log"));
        let mut fixture = Self {
            container,
            controller_owned: reserved.is_some(),
            directory,
            database_directory: Some(database_directory),
            postgres_settings: Value::Null,
            executable: binary()?,
            log,
            spool: spool.to_owned(),
            child: Mutex::new(None),
            failure_captured: AtomicBool::new(false),
        };
        // Capture setup failures before owned container/data cleanup as well.
        // The guard borrows the fixture, so effective settings are installed
        // only after the guarded setup has completed below.
        let failure_capture = fixture.capture_on_error();
        let socket = fixture.directory.path().join("socket");
        std::fs::create_dir(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o1777))?;
        // PostgreSQL supports arbitrary non-root users via its entrypoint.
        // Match the host fixture owner so forced cleanup cannot strand foreign
        // UID socket files in a sticky bind-mounted directory.
        let owner = fixture.directory.path().metadata()?;
        let user = format!("{}:{}", owner.uid(), owner.gid());
        let images: Value = serde_json::from_slice(&std::fs::read(
            repo_root().join("observability/images.lock.json"),
        )?)?;
        let (name, digest) = images
            .as_object()
            .context("image lock object")?
            .iter()
            .find(|(name, _)| name.starts_with("postgres:"))
            .context("pinned PostgreSQL image")?;
        let image = format!("{name}@{}", digest.as_str().context("image digest")?);
        let mut docker = tokio::process::Command::new("docker");
        docker.arg("run");
        if fixture.controller_owned {
            docker.args([
                "--label",
                &format!("eez.overhead.owner={}", fixture.container),
            ]);
        }
        let container_id = output(
            docker.args([
                "--detach",
                "--rm",
                "--name",
                &fixture.container,
                "--user",
                &user,
                "--network",
                "none",
                "--memory",
                "512m",
                "--cpus",
                "1",
                "--mount",
                &format!(
                    "type=bind,src={},dst=/var/lib/postgresql",
                    fixture
                        .database_directory
                        .as_ref()
                        .unwrap()
                        .path()
                        .display()
                ),
                "--mount",
                &format!("type=bind,src={},dst=/var/run/postgresql", socket.display()),
                "--env",
                "POSTGRES_HOST_AUTH_METHOD=trust",
                "--env",
                "POSTGRES_DB=eez_observer",
                &image,
                "postgres",
                "-c",
                "listen_addresses=",
                "-c",
                "shared_buffers=128MB",
                "-c",
                "max_connections=30",
            ]),
            None,
        )
        .await?;
        if let Some(parents) = reserved {
            let id = container_id.trim();
            ensure!(
                id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()),
                "invalid Docker container ID"
            );
            // Receipt is committed only after daemon-side creation completed.
            // Missing receipt is deliberately not proof that no creation is pending.
            let temporary = parents.database_parent.join("container.pending");
            std::fs::write(
                &temporary,
                serde_json::to_vec(&json!({"name": fixture.container, "id": id}))?,
            )?;
            std::fs::rename(temporary, parents.database_parent.join("container.json"))?;
        }
        wait_for(Duration::from_secs(60), || async {
            let logs = output(
                tokio::process::Command::new("docker").args(["logs", &fixture.container]),
                None,
            )
            .await?;
            if !logs.contains("init process complete") {
                return Ok(None);
            }
            // The entrypoint marker precedes the final server's socket by a
            // short interval. Require a successful query, not elapsed time.
            Ok(fixture
                .sql("SELECT 1")
                .await
                .ok()
                .filter(|value| value == "1")
                .map(|_| ()))
        })
        .await
        .context("private PostgreSQL readiness")?;
        let settings: Value = serde_json::from_str(&fixture.sql(
            "SELECT jsonb_build_object('shared_buffers_bytes',pg_size_bytes(current_setting('shared_buffers')),'max_connections',current_setting('max_connections')::bigint,'data_directory',current_setting('data_directory'))",
        ).await?)?;
        ensure!(
            settings["shared_buffers_bytes"].as_u64() == Some(128 * 1024 * 1024)
                && settings["max_connections"].as_u64() == Some(30),
            "native PostgreSQL settings differ from the packaged profile"
        );
        let inspect: Value = serde_json::from_str(
            &output(
                tokio::process::Command::new("docker").args([
                    "inspect",
                    "--format",
                    "{{json .}}",
                    &fixture.container,
                ]),
                None,
            )
            .await?,
        )?;
        let database_path = fixture.database_directory.as_ref().unwrap().path();
        ensure!(
            inspect["Mounts"]
                .as_array()
                .is_some_and(|mounts| mounts.iter().any(|mount| {
                    mount["Destination"] == "/var/lib/postgresql"
                        && mount["Type"] == "bind"
                        && mount["Source"]
                            .as_str()
                            .is_some_and(|source| Path::new(source) == database_path)
                        && mount["RW"] == true
                }))
                && inspect["HostConfig"]["Memory"].as_u64() == Some(536870912)
                && inspect["HostConfig"]["NanoCpus"].as_u64() == Some(1_000_000_000),
            "native PostgreSQL storage or resource limits differ from the fixture contract"
        );
        let postgres_settings = json!({
            "storage":{"type":"owned_bind","filesystem":filesystem,"data_directory":settings["data_directory"]},
            "shared_buffers_bytes":settings["shared_buffers_bytes"],"max_connections":settings["max_connections"],
            "memory_bytes":inspect["HostConfig"]["Memory"],"nano_cpus":inspect["HostConfig"]["NanoCpus"]
        });
        let admin = fixture.directory.path().join("admin.json");
        let writer = fixture.directory.path().join("writer.json");
        for (path, user) in [(&admin, "postgres"), (&writer, "eez_observer_writer")] {
            std::fs::write(
                path,
                serde_json::to_vec(
                    &json!({"connection_string":format!("host={} user={user} dbname=eez_observer sslmode=disable",socket.display())}),
                )?,
            )?;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        output(
            tokio::process::Command::new(&fixture.executable)
                .arg("migrate")
                .arg("--database-config")
                .arg(&admin),
            None,
        )
        .await?;
        ensure!(
            fixture
                .sql("SELECT max(version) FROM eez.schema_migrations")
                .await?
                == eez_observer::database::SCHEMA_VERSION.to_string(),
            "observer executable schema differs from source test"
        );
        let expected =
            eez_observer::database::DatabaseConfig::load(&admin).map_err(anyhow::Error::msg)?;
        eez_observer::database::Database::connect(&expected)
            .await
            .map_err(anyhow::Error::msg)?
            .verify_schema()
            .await
            .map_err(anyhow::Error::msg)?;
        let digest = output(
            tokio::process::Command::new("sha256sum").arg(&fixture.executable),
            None,
        )
        .await?;
        let digest = digest
            .split_ascii_whitespace()
            .next()
            .context("observer binary digest")?;
        ensure!(
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid observer digest"
        );
        std::fs::write(
            fixture.log.with_extension("provenance.json"),
            serde_json::to_vec_pretty(&json!({
                "observer_sha256":digest,"schema":eez_observer::database::SCHEMA_VERSION,
                "owned_container":fixture.container,
                "schema_checksums_verified":true,"postgres_image":image,"postgres_memory_bytes":536870912u64,
                "postgres":postgres_settings,
                "database_transport":"private_unix_socket","database_network":"none",
                "chain_source":"live_native_rpc","lifecycle_source":"actual_native_spool"
            }))?,
        )?;
        fixture.sql("ALTER ROLE eez_observer_writer LOGIN").await?;
        let l1 = ProviderBuilder::new().connect_http(world.l1_rpc().parse()?);
        let l2 = ProviderBuilder::new().connect_http(world.l2_rpc().parse()?);
        let first1 = l1
            .get_block_by_number(alloy_eips::BlockNumberOrTag::Number(0))
            .await?
            .context("L1 genesis")?;
        let first2 = l2
            .get_block_by_number(alloy_eips::BlockNumberOrTag::Number(0))
            .await?
            .context("L2 genesis")?;
        let port = PortLease::tcp();
        let config = json!({"environment":"devnet","deployment":deployment,"rollup_id":world.cfg.rollup_id,
            "database_config":writer,"batch_blocks":64,
            "exporter":{"l1":{"url":world.l1_rpc(),"chain_id":l1.get_chain_id().await?,"genesis_hash":first1.header.hash},
                "l2":{"url":world.l2_rpc(),"chain_id":l2.get_chain_id().await?,"genesis_hash":first2.header.hash},
                "listen":format!("127.0.0.1:{}",port.port())},
            "l1":{"bootstrap":0,"include_all_transactions":true,"contracts":[world.cfg.eez_address]},
            "l2":{"bootstrap":0,"include_all_transactions":true},
            "lifecycle":[{"service":"eez-composer","instance":"fixture","directory":spool}]});
        std::fs::write(
            fixture.directory.path().join("indexer.json"),
            serde_json::to_vec(&config)?,
        )?;
        eez_observer::indexer::Config::load(&fixture.directory.path().join("indexer.json"))
            .map_err(anyhow::Error::msg)?;
        drop(port);
        failure_capture.success();
        fixture.postgres_settings = postgres_settings;
        Ok(fixture)
    }

    pub async fn start_prepared(&self) -> Result<()> {
        self.launch()?;
        self.wait_sql("SELECT count(*)=2 FROM eez.chain_instances WHERE last_success_at IS NOT NULL AND frozen_reason IS NULL", "t").await
    }

    fn launch(&self) -> Result<()> {
        let mut child_slot = self.child.lock().unwrap();
        ensure!(child_slot.is_none(), "observer already started");
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)?;
        let child = Command::new(&self.executable)
            .arg("indexer")
            .arg("--config")
            .arg(self.directory.path().join("indexer.json"))
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        *child_slot = Some(child);
        Ok(())
    }

    /// External resource sampler identities; an idle reservation has no observer PID.
    pub fn process_id(&self) -> Option<u32> {
        self.child.lock().unwrap().as_ref().map(Child::id)
    }

    pub fn container_name(&self) -> &str {
        &self.container
    }

    pub fn capture_on_error(&self) -> FailureCapture<'_> {
        FailureCapture {
            observer: self,
            complete: false,
        }
    }

    /// Emit the actual selected observer semantics, excluding only owned
    /// database/RPC/listen transports and the separately recorded spool path.
    pub fn measurement_settings(&self) -> Result<Value> {
        // Functional tests may intentionally place their workspace on RAM.
        // Performance arms must match the disk-backed deployment reservation.
        ensure!(
            self.postgres_settings["storage"]["filesystem"]
                .as_str()
                .is_some_and(|filesystem| !matches!(filesystem, "tmpfs" | "ramfs")),
            "native observer measurement requires disk-backed target storage"
        );
        let path = self.directory.path().join("indexer.json");
        let config: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        let parsed = eez_observer::indexer::Config::load(&path).map_err(anyhow::Error::msg)?;
        Ok(
            json!({"environment":parsed.environment,"deployment":parsed.deployment,"rollup_id":parsed.rollup_id,"batch_blocks":parsed.batch_blocks,"l1":config["l1"],"l2":config["l2"],"lifecycle":parsed.lifecycle.iter().map(|source| json!({"service":source.service,"instance":source.instance})).collect::<Vec<_>>(),"optional_collectors":{"blobs":parsed.blobs.is_some(),"follower":parsed.follower.is_some(),"funding":parsed.funding.is_some()},"postgres":self.postgres_settings}),
        )
    }

    pub fn metrics_address(&self) -> Result<String> {
        let config: Value =
            serde_json::from_slice(&std::fs::read(self.directory.path().join("indexer.json"))?)?;
        Ok(config["exporter"]["listen"]
            .as_str()
            .context("observer listen address")?
            .to_owned())
    }

    pub async fn metrics(&self) -> Result<String> {
        let config: Value =
            serde_json::from_slice(&std::fs::read(self.directory.path().join("indexer.json"))?)?;
        let listen = config["exporter"]["listen"]
            .as_str()
            .context("observer listen address")?;
        Ok(reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()?
            .get(format!("http://{listen}/metrics"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }

    pub async fn sql(&self, sql: &str) -> Result<String> {
        output(
            tokio::process::Command::new("docker").args([
                "exec",
                "-i",
                &self.container,
                "psql",
                "-XAtq",
                "-v",
                "ON_ERROR_STOP=1",
                "-U",
                "postgres",
                "-d",
                "eez_observer",
            ]),
            Some(&format!(
                "SET jit=off; SET statement_timeout='10s'; SET lock_timeout='5s'; {sql};"
            )),
        )
        .await
    }

    pub async fn wait_sql(&self, sql: &str, expected: &str) -> Result<()> {
        // Retry an unmet observable predicate. A dead observer or failed query
        // is a test failure immediately, not three minutes of hidden retries.
        let result = wait_for(DEADLINE, || async {
            let current: Result<bool> = async {
                if let Some(child) = self.child.lock().unwrap().as_mut() {
                    ensure!(
                        child.try_wait()?.is_none(),
                        "observer exited; inspect {}",
                        self.log.display()
                    );
                }
                Ok(self.sql(sql).await? == expected)
            }
            .await;
            Ok(match current {
                Ok(true) => Some(Ok(())),
                Ok(false) => None,
                Err(error) => Some(Err(error)),
            })
        })
        .await
        .with_context(|| {
            format!(
                "live observer predicate timed out: {sql}; inspect {}",
                self.log.display()
            )
        })?;
        result.with_context(|| {
            format!(
                "live observer assertion: {sql}; inspect {}",
                self.log.display()
            )
        })
    }

    pub async fn canonical_commits(&self, directions: &[&str], phase: &str) -> Result<()> {
        let config: Value =
            serde_json::from_slice(&std::fs::read(self.directory.path().join("indexer.json"))?)?;
        let listen = config["exporter"]["listen"]
            .as_str()
            .context("fixture metrics address")?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;
        let metrics = wait_for(DEADLINE, || async {
            let text = client.get(format!("http://{listen}/metrics")).send().await?.error_for_status()?.text().await?;
            let valid = text.lines().any(|line| line == "eez_observer_canonical_outcome_evaluation_valid 1");
            let committed = directions.iter().all(|direction| {
                let name = format!("eez_observer_canonical_terminal_admissions{{direction=\"{direction}\",outcome=\"committed\"}} ");
                text.lines().filter_map(|line| line.strip_prefix(&name)).any(|value| value.parse::<u64>().is_ok_and(|count| count>0))
            });
            Ok((valid && committed).then_some(text))
        }).await.context("canonical observer progress metrics did not become valid during continued native appends")?;
        let retained = metrics
            .lines()
            .filter(|line| {
                line.starts_with("eez_observer_canonical_")
                    || line.starts_with("eez_observer_last_canonical_")
            })
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(
            self.log.with_extension(format!("{phase}.metrics.txt")),
            retained,
        )?;
        Ok(())
    }

    // The process fixture assigns a fresh L2 HTTP port on Composer restart.
    // Reload endpoints only: persisted chain identities and observer history
    // must survive, and production identity checks still fence every request.
    pub async fn reconnect_after_native_restart(&self, world: &CrossChainWorld) -> Result<()> {
        let path = self.directory.path().join("indexer.json");
        let mut config: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
        config["exporter"]["l1"]["url"] = json!(world.l1_rpc());
        config["exporter"]["l2"]["url"] = json!(world.l2_rpc());
        std::fs::write(path, serde_json::to_vec(&config)?)?;
        self.restart().await
    }

    pub async fn restart(&self) -> Result<()> {
        let before = self.sql("SELECT extract(epoch FROM now())::text").await?;
        if let Some(mut child) = self.child.lock().unwrap().take() {
            child.kill()?;
            child.wait()?;
        }
        self.launch()?;
        self.wait_sql(&format!("SELECT count(*)=2 FROM eez.chain_instances WHERE last_success_at>to_timestamp({before}) AND collection_error IS NULL AND frozen_reason IS NULL"), "t").await?;
        self.wait_sql(&format!("SELECT count(*)=1 FROM eez.lifecycle_sources WHERE last_success_at>to_timestamp({before}) AND collection_error IS NULL"), "t").await
    }

    pub async fn outcome(&self, hash: B256, outcome: &str) -> Result<()> {
        self.wait_sql(&format!("SELECT count(*)=1 FROM eez.lifecycle_admission_status WHERE source->>'transaction_hash'='{hash:x}' AND status='canonical' AND outcome='{outcome}' AND evidence_class='node_reconciled'"), "t").await
    }

    pub async fn admissions(&self, hash: B256) -> Result<Value> {
        let rows = self.sql(&format!("SELECT COALESCE(jsonb_agg(jsonb_build_object('epoch',encode(admission_epoch,'hex'),'sequence',admission_sequence::text,'status',status,'outcome',outcome) ORDER BY admission_epoch,admission_sequence),'[]') FROM eez.lifecycle_admission_status WHERE source->>'transaction_hash'='{hash:x}'")).await?;
        Ok(serde_json::from_str(&rows)?)
    }

    pub async fn receipt(&self, rpc: &str, role: &str, hash: B256) -> Result<Value> {
        let provider = ProviderBuilder::new().connect_http(rpc.parse()?);
        let receipt: Value = provider
            .client()
            .request("eth_getTransactionReceipt", (hash,))
            .await?;
        ensure!(!receipt.is_null(), "native reference receipt missing");
        let quantity = |field: &str| -> Result<U256> {
            Ok(receipt[field]
                .as_str()
                .with_context(|| format!("missing receipt {field}"))?
                .parse()?)
        };
        let execution = quantity("gasUsed")? * quantity("effectiveGasPrice")?;
        let blob = if receipt["type"] == "0x3" {
            quantity("blobGasUsed")? * quantity("blobGasPrice")?
        } else {
            U256::ZERO
        };
        let block: B256 = receipt["blockHash"]
            .as_str()
            .context("receipt block")?
            .parse()?;
        let index = quantity("transactionIndex")?;
        let success = receipt["status"] == "0x1";
        self.wait_sql(&format!("SELECT count(*)=1 FROM eez.canonical_receipt_facts WHERE role='{role}' AND transaction_hash=decode('{hash:x}','hex') AND block_hash=decode('{block:x}','hex') AND transaction_index={index} AND success={success} AND execution_paid={execution} AND blob_paid={blob} AND accounting_gap IS NULL AND source_valid"), "t").await?;
        Ok(receipt)
    }

    pub async fn allocation(&self, hash: B256) -> Result<()> {
        self.wait_sql(&format!("SELECT count(*)>0 FROM eez.source_posting_allocations WHERE source->>'transaction_hash'='{hash:x}' AND evidence_class='native_membership_receipt_verified'"), "t").await?;
        self.wait_sql("SELECT NOT EXISTS(SELECT FROM eez.source_posting_allocations shares JOIN eez.posting_receipts post ON post.chain_instance_id=shares.l1_chain_id AND post.transaction_hash=shares.post_hash AND post.block_hash=shares.post_block_hash GROUP BY shares.l1_chain_id,shares.l2_chain_id,shares.post_hash,shares.post_block_hash,post.execution_paid,post.blob_paid HAVING sum(shares.allocated_execution_paid)<>post.execution_paid OR sum(shares.allocated_blob_paid)<>post.blob_paid)", "t").await
    }

    pub async fn paid_post(&self, hash: B256, members: usize) -> Result<()> {
        self.wait_sql(&format!("SELECT count(*)=1 FROM eez.posting_allocation_status WHERE transaction_hash=decode('{hash:x}','hex') AND allocation_state='allocated' AND jsonb_array_length(population)={members}"), "t").await?;
        self.wait_sql(&format!("SELECT count(*)={members} FROM eez.source_posting_allocations WHERE post_hash=decode('{hash:x}','hex')"), "t").await
    }
}

impl Observer {
    fn capture_failure(&self) {
        if self.failure_captured.swap(true, Ordering::Relaxed) {
            return;
        }
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if self.controller_owned {
            // The controller owns diagnostics and both success/failure teardown.
            // This also covers external SIGTERM, which cannot run Rust Drops.
            return;
        }
        {
            // Local diagnostics only: these bulk fixtures intentionally live
            // outside EEZ_TEST_LOG_DIR and are not CI log-upload inputs. Store
            // them beside target artifacts rather than copying PGDATA into
            // /tmp again during the failure investigation.
            let root = repo_root().join("target/native-observer-failures");
            if let Err(error) = std::fs::create_dir_all(&root) {
                eprintln!("cannot create local observer failure directory: {error}");
                return;
            }
            if let Ok(snapshot) = tempfile::Builder::new()
                .prefix(&format!("eez-native-observer-failure-{}-", self.container))
                .tempdir_in(root)
            {
                let snapshot = snapshot.keep();
                let mut dump_status = None;
                if let Ok(dump) = std::fs::File::create(snapshot.join("postgres.dump")) {
                    dump_status = Command::new("timeout")
                        .args([
                            "15",
                            "docker",
                            "exec",
                            &self.container,
                            "pg_dump",
                            "-U",
                            "postgres",
                            "-d",
                            "eez_observer",
                            "--format=custom",
                            "--no-owner",
                            "--no-privileges",
                        ])
                        .stdout(dump)
                        .stderr(
                            std::fs::File::create(snapshot.join("pg-dump.stderr"))
                                .map_or_else(|_| Stdio::null(), Stdio::from),
                        )
                        .status()
                        .ok();
                }
                for (name, arguments) in [
                    (
                        "postgres.log",
                        vec!["logs", "--tail", "20000", self.container.as_str()],
                    ),
                    (
                        "memory.txt",
                        vec![
                            "exec",
                            self.container.as_str(),
                            "cat",
                            "/sys/fs/cgroup/memory.events",
                            "/sys/fs/cgroup/memory.peak",
                            "/sys/fs/cgroup/memory.stat",
                        ],
                    ),
                    (
                        "filesystem.txt",
                        vec![
                            "exec",
                            self.container.as_str(),
                            "df",
                            "-P",
                            "/var/lib/postgresql",
                        ],
                    ),
                ] {
                    if let Ok(log) = std::fs::File::create(snapshot.join(name))
                        && let Ok(stderr) = log.try_clone()
                    {
                        let _ = Command::new("timeout")
                            .args(["5", "docker"])
                            .args(arguments)
                            .stdout(log)
                            .stderr(stderr)
                            .status();
                    }
                }
                let _ = std::fs::copy(
                    self.log.with_extension("provenance.json"),
                    snapshot.join("provenance.json"),
                );
                let _ = std::fs::copy(&self.log, snapshot.join("observer.log"));
                let spool_copy = snapshot.join("spool");
                if std::fs::create_dir(&spool_copy).is_ok() {
                    let mut remaining = 32 * 1024 * 1024u64;
                    if let Ok(entries) = std::fs::read_dir(&self.spool) {
                        let mut entries = entries.flatten().collect::<Vec<_>>();
                        entries.sort_by_key(std::fs::DirEntry::file_name);
                        for entry in entries {
                            if remaining == 0 {
                                break;
                            }
                            if entry.file_type().is_ok_and(|kind| kind.is_file())
                                && let (Ok(input), Ok(mut output)) = (
                                    std::fs::File::open(entry.path()),
                                    std::fs::File::create(spool_copy.join(entry.file_name())),
                                )
                            {
                                let mut input = std::io::Read::take(input, remaining);
                                if let Ok(copied) = std::io::copy(&mut input, &mut output) {
                                    remaining = remaining.saturating_sub(copied);
                                }
                            }
                        }
                    }
                    let _ = std::fs::write(
                        snapshot.join("capture.json"),
                        serde_json::to_vec_pretty(&json!({
                            "spool_byte_limit":33554432,"remaining_bytes":remaining,"spool_capture_concurrent":true,
                            "database_dump_complete":dump_status.is_some_and(|status| status.success()),
                            "database_dump_exit_code":dump_status.and_then(|status| status.code()),
                            "purpose":"SQL reproduction and diagnostics, not live reorg evidence"
                        })).unwrap(),
                    );
                }
                eprintln!(
                    "retained local-only observer failure snapshot at {}",
                    snapshot.display()
                );
            }
        }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if self.controller_owned {
            return;
        }
        if std::thread::panicking() {
            self.capture_failure();
        }
        // Named resource is private to this fixture; preserve observer log on
        // failure but never retain a running database or operator-owned state.
        let removed = Command::new("timeout")
            .args([
                "15",
                "docker",
                "rm",
                "--force",
                "--volumes",
                &self.container,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !removed.is_ok_and(|status| status.success())
            && let Some(directory) = self.database_directory.take()
        {
            // A failed/expired removal is not evidence that the container
            // stopped using its mount. Preserve this owned path for cleanup.
            eprintln!(
                "could not confirm removal of {}; retained owned PGDATA at {}",
                self.container,
                directory.keep().display()
            );
        }
        // On successful removal, TempDir fields drop only after this method.
    }
}

/// Process regression used once by cross_chain, rather than duplicated in every
/// integration target that imports this shared fixture module.
pub async fn assert_result_error_capture_and_cleanup(world: &CrossChainWorld) -> Result<()> {
    let spool = tempfile::tempdir()?;
    std::fs::write(
        spool.path().join("diagnostic-marker"),
        b"owned result-error fixture",
    )?;
    let observer = Observer::start(world, spool.path(), "observer-result-error").await?;
    let database = observer
        .database_directory
        .as_ref()
        .unwrap()
        .path()
        .to_owned();
    let socket_directory = observer.directory.path().to_owned();
    let container = observer.container.clone();
    ensure!(
        observer.process_id().is_some(),
        "observer must actually run before failure"
    );

    // Return Err normally: no panic exists for Observer::drop to notice. The
    // scoped guard must capture before this still-live fixture is destroyed.
    let result: Result<()> = (|| {
        let _capture = observer.capture_on_error();
        anyhow::bail!("intentional observer Result failure")
    })();
    ensure!(result.unwrap_err().to_string() == "intentional observer Result failure");
    ensure!(
        observer.process_id().is_none(),
        "failed observer worker was not stopped"
    );
    ensure!(database.is_dir(), "PGDATA removed before evidence capture");
    let prefix = format!("eez-native-observer-failure-{container}-");
    let snapshots = std::fs::read_dir(repo_root().join("target/native-observer-failures"))?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    ensure!(
        snapshots.len() == 1,
        "expected one snapshot for this owned fixture"
    );
    let snapshot = &snapshots[0];
    let capture: Value = serde_json::from_slice(&std::fs::read(snapshot.join("capture.json"))?)?;
    ensure!(capture["database_dump_complete"] == true && capture["database_dump_exit_code"] == 0);
    ensure!(
        std::fs::read(snapshot.join("spool/diagnostic-marker"))?.as_slice()
            == b"owned result-error fixture"
    );
    ensure!(
        std::fs::read_to_string(snapshot.join("postgres.log"))?
            .contains("database system is ready to accept connections")
    );
    ensure!(std::fs::read_to_string(snapshot.join("memory.txt"))?.contains("oom_kill"));
    ensure!(snapshot.join("observer.log").is_file() && snapshot.join("provenance.json").is_file());
    // The pinned pg_restore must parse the captured archive before the owning
    // database container is removed; header bytes alone do not prove a dump.
    let archive = output(
        tokio::process::Command::new("docker")
            .args(["exec", "-i", &container, "pg_restore", "--list"])
            .stdin(Stdio::from(std::fs::File::open(
                snapshot.join("postgres.dump"),
            )?)),
        None,
    )
    .await?;
    ensure!(
        archive.contains("TABLE eez lifecycle_native_checks"),
        "captured archive lacks native evidence schema"
    );
    drop(observer);
    ensure!(
        !database.exists() && !socket_directory.exists(),
        "owned fixture directories survived confirmed cleanup"
    );
    let remaining = output(
        tokio::process::Command::new("docker").args([
            "ps",
            "--all",
            "--filter",
            &format!("name=^/{container}$"),
            "--format",
            "{{.Names}}",
        ]),
        None,
    )
    .await?;
    ensure!(
        remaining.is_empty(),
        "owned database container survived cleanup"
    );
    // Retain snapshots if any assertion fails; successful intentional failure
    // tests remove only their own captured bulk data.
    std::fs::remove_dir_all(snapshot)?;
    Ok(())
}
