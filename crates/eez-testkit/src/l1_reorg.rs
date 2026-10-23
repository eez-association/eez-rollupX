//! Controlled consensus for the existing native Ethereum cross-chain fixture.
//! The composer remains alive during the reorg, including its held-pool ledger.

use super::{
    BundleStub, CrossChainConfig, CrossChainWorld, NativeL1Profile, repo_root,
    test_log_destination, wait_for,
};
use alloy_primitives::B256;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};

/// Owns a test-only Engine API driver. Its payloads come from the embedded L1.
pub struct L1ReorgControl {
    child: Option<Child>,
    control: tempfile::TempDir,
    log_path: PathBuf,
    _log_dir: Option<tempfile::TempDir>,
}

/// Start the existing native fixture with controlled consensus from genesis.
/// Avoids a setup restart that would discard unpersisted local-miner history.
pub async fn setup_cross_chain_with_l1_reorg(
    extra_env: &[(&'static str, String)],
) -> Result<CrossChainWorld> {
    super::setup_cross_chain_inner(
        None,
        None,
        extra_env,
        NativeL1Profile::EthereumOsaka,
        false,
        false,
        true,
    )
    .await
}

impl L1ReorgControl {
    pub(super) fn prepare(cfg: &CrossChainConfig, builder: &BundleStub) -> Result<Self> {
        ensure!(
            cfg.l1_profile == NativeL1Profile::EthereumOsaka,
            "controlled native reorg fixture requires Ethereum Osaka"
        );
        let control = tempfile::tempdir().context("L1 reorg control directory")?;
        std::fs::write(control.path().join("jwt.hex"), "11".repeat(32))?;
        // Only outbound, single-post bundles are supported by this controller.
        std::fs::write(
            builder
                .capture_dir()
                .join("control/single-transaction.json"),
            b"{}",
        )?;
        let (log_path, log_dir) = test_log_destination("l1-consensus")?;
        Ok(Self {
            child: None,
            control,
            log_path,
            _log_dir: log_dir,
        })
    }

    pub(super) fn env(&self) -> [(&'static str, String); 2] {
        [
            ("EEZ_L1_CHAIN", "devnet".into()),
            (
                "EEZ_L1_JWT_SECRET",
                self.control
                    .path()
                    .join("jwt.hex")
                    .to_string_lossy()
                    .into_owned(),
            ),
        ]
    }

    pub(super) async fn start(
        &mut self,
        cfg: &CrossChainConfig,
        history_end: Option<u64>,
    ) -> Result<()> {
        let log = std::fs::File::create(&self.log_path)?;
        let history_args = history_end
            .map(|timestamp| ["--replay-history-until".to_owned(), timestamp.to_string()]);
        self.child = Some(
            Command::new("python3")
                .arg(repo_root().join("scripts/native-l1-consensus.py"))
                .args([
                    "--rpc",
                    &cfg.l1_rpc_url(),
                    "--engine",
                    &format!("http://127.0.0.1:{}", cfg.l1_auth_port),
                ])
                .arg("--jwt")
                .arg(self.control.path().join("jwt.hex"))
                .arg("--control")
                .arg(self.control.path())
                .args([
                    "--genesis-time",
                    &cfg.genesis_time.to_string(),
                    "--block-seconds",
                    &(super::CROSS_CHAIN_L1_BLOCK_TIME_MS / 1000).to_string(),
                ])
                .args(history_args.into_iter().flatten())
                .stderr(Stdio::from(log.try_clone()?))
                .stdout(Stdio::from(log))
                .spawn()
                .context("start native L1 consensus driver")?,
        );
        let ready = self.control.path().join("ready.json");
        wait_for(Duration::from_secs(30), || async {
            Ok(ready.exists().then_some(()))
        })
        .await
        .with_context(|| format!("L1 consensus startup: {}", self.diagnostics()))?;
        self.check_running()
    }

    fn diagnostics(&self) -> String {
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }

    fn check_running(&mut self) -> Result<()> {
        ensure!(
            self.child
                .as_mut()
                .context("consensus driver not started")?
                .try_wait()?
                .is_none(),
            "L1 consensus driver exited: {}",
            self.diagnostics()
        );
        Ok(())
    }

    /// Omit one future consensus slot while preserving the current branch.
    /// The next block still comes from the real Engine API payload builder.
    pub async fn skip_slot(&mut self, timestamp: u64) -> Result<()> {
        self.arm_slot("skip", timestamp).await
    }

    /// Build the requested slot normally but hold its payload before import.
    /// Release only after the caller has observed the replacement dispatch.
    pub async fn delay_slot(&mut self, timestamp: u64) -> Result<()> {
        self.arm_slot("delay", timestamp).await
    }

    async fn arm_slot(&mut self, kind: &str, timestamp: u64) -> Result<()> {
        self.check_running()?;
        let result = self.control.path().join(format!("{kind}-slot-armed.json"));
        if result.exists() {
            std::fs::remove_file(&result)?;
        }
        let request = self.control.path().join(format!("{kind}-slot.new.json"));
        std::fs::write(
            &request,
            serde_json::to_vec(&json!({"timestamp": timestamp}))?,
        )?;
        std::fs::rename(
            request,
            self.control.path().join(format!("{kind}-slot.json")),
        )?;
        let record = wait_for(Duration::from_secs(10), || async {
            match std::fs::read(&result) {
                Ok(bytes) => Ok(Some(serde_json::from_slice::<Value>(&bytes)?)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            }
        })
        .await
        .with_context(|| format!("L1 {kind}-slot barrier failed: {}", self.diagnostics()))?;
        self.check_running()?;
        ensure!(
            record["timestamp"] == timestamp,
            "controlled slot identity mismatch"
        );
        Ok(())
    }

    /// Import the already-built original payload and wait for canonical adoption.
    /// Fails if no delayed payload is ready or the controller cannot import it.
    pub async fn release_delayed_slot(&mut self) -> Result<()> {
        self.check_running()?;
        ensure!(
            self.control.path().join("delay-slot-built.json").exists(),
            "delayed payload has not been built"
        );
        std::fs::write(self.control.path().join("release-slot.json"), b"{}")?;
        wait_for(Duration::from_secs(10), || async {
            Ok(self
                .control
                .path()
                .join("delay-slot-released.json")
                .exists()
                .then_some(()))
        })
        .await
        .with_context(|| format!("delayed slot release failed: {}", self.diagnostics()))?;
        self.check_running()
    }

    /// Replace the current unfinalized branch with a new block on this exact
    /// ancestor. The returned record includes the actual replacement identity.
    pub async fn reorg_to(&mut self, number: u64, hash: B256) -> Result<Value> {
        self.check_running()?;
        let result = self.control.path().join("reorg-result.json");
        if result.exists() {
            std::fs::remove_file(&result)?;
        }
        let request = self.control.path().join("reorg.new.json");
        std::fs::write(
            &request,
            serde_json::to_vec(&json!({"number": number, "hash": hash}))?,
        )?;
        std::fs::rename(request, self.control.path().join("reorg.json"))?;
        let record = wait_for(Duration::from_secs(30), || async {
            match std::fs::read(&result) {
                Ok(bytes) => Ok(Some(serde_json::from_slice::<Value>(&bytes)?)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
        .await
        .with_context(|| format!("L1 reorg failed: {}", self.diagnostics()))?;
        self.check_running()?;
        ensure!(
            record["number"] == number && record["hash"] == hash.to_string(),
            "L1 reorg acknowledgment identity mismatch"
        );
        Ok(record)
    }
}

impl Drop for L1ReorgControl {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        if std::thread::panicking() {
            eprintln!("L1 consensus driver:\n{}", self.diagnostics());
        }
    }
}
