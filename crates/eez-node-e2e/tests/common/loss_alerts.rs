//! Own the real alert backends and preserve bounded native-test diagnostics.
use std::{
    fs::File,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};

pub struct Controller {
    directory: tempfile::TempDir,
    child: Child,
    pub config: PathBuf,
    finished: bool,
}

impl Controller {
    pub async fn start(metrics: &str) -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("eez-native-loss-fixture-")
            .tempdir()?;
        let log = File::create(directory.path().join("controller.log"))?;
        let child = Command::new("python3")
            .arg(eez_testkit::repo_root().join("observability/tests/native-loss-alerts.py"))
            .arg("--directory")
            .arg(directory.path())
            .arg("--native-target")
            .arg(metrics)
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .process_group(0)
            .spawn()?;
        let mut controller = Self {
            directory,
            child,
            config: PathBuf::new(),
            finished: false,
        };
        let ready = controller.wait("ready").await?;
        controller.config =
            PathBuf::from(ready["config"].as_str().context("native config missing")?);
        ensure!(
            controller.config.is_file(),
            "generated native config absent"
        );
        Ok(controller)
    }

    pub fn signal(&self, name: &str) -> Result<()> {
        let temporary = self.directory.path().join(format!("{name}.partial"));
        std::fs::write(&temporary, b"{}")?;
        std::fs::rename(
            temporary,
            self.directory.path().join(format!("{name}.json")),
        )?;
        Ok(())
    }

    pub async fn wait(&mut self, name: &str) -> Result<Value> {
        let deadline = Instant::now() + eez_testkit::SETTLE_TIMEOUT;
        let path = self.directory.path().join(format!("{name}.json"));
        loop {
            if let Some(failure) = record(&self.directory.path().join("failed.json"))? {
                bail!("native alert controller failed: {failure}");
            }
            if let Some(value) = record(&path)? {
                return Ok(value);
            }
            if let Some(status) = self.child.try_wait()? {
                bail!("native alert controller exited before {name}: {status}");
            }
            ensure!(
                Instant::now() < deadline,
                "native alert phase {name} timed out"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn finish(&mut self) -> Result<()> {
        self.signal("done")?;
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = self.child.try_wait()? {
                ensure!(status.success(), "native alert controller failed: {status}");
                let report: Value = serde_json::from_slice(&std::fs::read(
                    self.directory.path().join("report.json"),
                )?)?;
                ensure!(report["status"] == "passed", "native alert report failed");
                self.finished = true;
                return Ok(());
            }
            ensure!(Instant::now() < deadline, "native alert cleanup timed out");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

fn record(path: &Path) -> Result<Option<Value>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            ensure!(bytes.len() <= 4096, "oversized native alert control record");
            Ok(Some(serde_json::from_slice(&bytes)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.signal("abort");
            let deadline = Instant::now() + Duration::from_secs(15);
            while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
            if matches!(self.child.try_wait(), Ok(None)) {
                // The controller has its own process group, including Docker CLI
                // children. Only that owned group is terminated on failed cleanup.
                let _ = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{}", self.child.id())])
                    .status();
                let _ = self.child.wait();
            }
            if let Ok(Some(owned)) = record(&self.directory.path().join("owned.json")) {
                let project = owned["project"].as_str().unwrap_or("");
                if project.starts_with("eez-native-loss-")
                    && let Some(containers) = owned["containers"].as_array()
                {
                    for container in containers.iter().filter_map(Value::as_str) {
                        if container.starts_with(&format!("{project}-")) {
                            let _ = Command::new("timeout")
                                .args(["15", "docker", "rm", "--force", "--volumes", container])
                                .output();
                        }
                    }
                }
            }
        }
        if let Some(logs) = std::env::var_os("EEZ_TEST_LOG_DIR") {
            let destination = PathBuf::from(logs).join(
                self.directory
                    .path()
                    .file_name()
                    .expect("owned fixture basename"),
            );
            if std::fs::create_dir_all(&destination).is_ok() {
                // Never copy the runtime directory containing generated secrets.
                for name in [
                    "report.json",
                    "controller.log",
                    "grafana.log",
                    "prometheus.log",
                ] {
                    let _ = std::fs::copy(self.directory.path().join(name), destination.join(name));
                }
                let _ = std::fs::write(
                    destination.join("scope.json"),
                    json!({"finished":self.finished,"scope":"native loopback alert fixture only"})
                        .to_string(),
                );
            }
        }
    }
}
