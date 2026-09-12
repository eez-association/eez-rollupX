//! Durable ownership for submitted cross-chain transactions.
//!
//! One versioned snapshot is atomically replaced and fsynced before dispatch
//! or ownership transfer. A separate advisory lock excludes a second writer.
//! Never-submitted ingress requests are intentionally outside this journal.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use alloy_primitives::{B256, Bytes, keccak256};
use eez_l1::{BundleTarget, L1BlockRef};
use eez_protocol::settlement_manifest::SettlementManifest;
use serde::{Deserialize, Serialize};

use crate::HeldTx;

/// Identity carried by asynchronous observers. Height reuse cannot match it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptId {
    pub generation: u64,
    pub post_batch_hash: B256,
}

/// Submission observations never prove which users settled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubmissionObservation {
    /// Persisted before the submitter resolves/dispatches the target.
    Prepared,
    /// Persisted at the submitter's last gate, before the network request.
    Dispatched { block: u64, timestamp: u64 },
    /// The signed postBatch has a canonical receipt; derivation still decides
    /// which effects applied.
    Included { block: L1BlockRef },
    /// Conclusively not included in this slot.
    Excluded { block: u64, charge_attempt: bool },
    /// Network error or incomplete observation. Retain ownership.
    Uncertain,
}

/// Explicit terminal user outcomes. Local policy terminations have no L1
/// dependency; canonical source outcomes must be reconsidered after a reorg.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalReason {
    Committed,
    SourceReverted,
    SourceCompletedWithMissingEffects,
    SourceCompleted,
    SourceNonceSpent,
    AttemptsExhausted,
    NonceGap,
    PoolDiscarded,
}

/// Current ownership, independently of historical attempts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserLocation {
    Queued,
    /// Drained for recomposition but not externally dispatched. Restart
    /// returns it to the queue without charging another attempt.
    Composing,
    AwaitingEvidence,
    Terminal {
        reason: TerminalReason,
        canonical: Option<L1BlockRef>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OwnedUser {
    pub tx: HeldTx,
    pub owner: u64,
    pub fifo_position: usize,
    pub location: UserLocation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AttemptRecord {
    pub id: AttemptId,
    pub sync_height: u64,
    pub parent: alloy_consensus::Header,
    pub txs: Vec<HeldTx>,
    pub manifest: SettlementManifest,
    pub post_batch: Bytes,
    pub requested_target: BundleTarget,
    /// Actual resolved target is retained even if observation later fails.
    pub dispatched_target: Option<(u64, u64)>,
    pub observation: SubmissionObservation,
    /// Snapshot against which all currently owned users were classified.
    pub reconciled_at: Option<L1BlockRef>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct JournalState {
    pub next_generation: u64,
    pub attempts: BTreeMap<u64, AttemptRecord>,
    pub users: BTreeMap<B256, OwnedUser>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JournalFile {
    version: u32,
    checksum: B256,
    state: JournalState,
}

#[derive(Debug)]
struct State {
    data: JournalState,
    failed: bool,
}

/// Shared journal for the optimistic ledger and held pool. A failed durable
/// write poisons further mutations so no later dispatch can outrun the disk.
#[derive(Debug)]
pub struct SettlementJournal {
    path: Option<PathBuf>,
    _writer_lock: Option<File>,
    state: Mutex<State>,
}

impl Default for SettlementJournal {
    fn default() -> Self {
        Self {
            path: None,
            _writer_lock: None,
            state: Mutex::new(State {
                data: JournalState::default(),
                failed: false,
            }),
        }
    }
}

impl SettlementJournal {
    /// Open or create an exclusively owned recovery journal.
    ///
    /// # Errors
    /// Rejects lock contention, unsupported versions, corruption and I/O errors.
    pub fn open(path: &Path) -> Result<Self, String> {
        let parent = path.parent().ok_or("journal path has no parent")?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let lock_path = path.with_extension("lock");
        let lock = private_file(&lock_path, false)?;
        lock.try_lock()
            .map_err(|error| format!("settlement journal already in use: {error}"))?;
        let data = match std::fs::read(path) {
            Ok(encoded) => {
                let file: JournalFile = serde_json::from_slice(&encoded)
                    .map_err(|error| format!("invalid settlement journal: {error}"))?;
                if file.version != 1 {
                    return Err(format!(
                        "unsupported settlement journal version {}",
                        file.version
                    ));
                }
                let body = serde_json::to_vec(&file.state).map_err(|error| error.to_string())?;
                if keccak256(body) != file.checksum {
                    return Err("settlement journal checksum mismatch".into());
                }
                for (generation, attempt) in &file.state.attempts {
                    if *generation != attempt.id.generation
                        || *generation >= file.state.next_generation
                    {
                        return Err("invalid journal attempt generation".into());
                    }
                }
                for (hash, user) in &file.state.users {
                    if *hash != user.tx.hash || !file.state.attempts.contains_key(&user.owner) {
                        return Err("invalid journal user ownership".into());
                    }
                }
                file.state
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => JournalState::default(),
            Err(error) => return Err(format!("read settlement journal: {error}")),
        };
        let journal = Self {
            path: Some(path.to_owned()),
            _writer_lock: Some(lock),
            state: Mutex::new(State {
                data,
                failed: false,
            }),
        };
        // Probe durability before accepting users. No submission observer or
        // in-progress composition survives a restart. Keep the dispatch target
        // as evidence, but do not wait for the old observer to finish.
        journal.update(|data| {
            for attempt in data.attempts.values_mut() {
                if matches!(
                    attempt.observation,
                    SubmissionObservation::Prepared | SubmissionObservation::Dispatched { .. }
                ) {
                    attempt.observation = SubmissionObservation::Uncertain;
                }
            }
            for user in data.users.values_mut() {
                if user.location == UserLocation::Composing {
                    user.location = UserLocation::Queued;
                }
            }
            Ok(())
        })?;
        Ok(journal)
    }

    pub(crate) fn read<T>(&self, f: impl FnOnce(&JournalState) -> T) -> T {
        let state = self
            .state
            .lock()
            .expect("settlement journal mutex poisoned");
        f(&state.data)
    }

    /// Commit the complete ownership decision before exposing it to the pool
    /// or submitter. Callers must not hold this mutex across awaits.
    pub(crate) fn update<T>(
        &self,
        f: impl FnOnce(&mut JournalState) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "settlement journal mutex poisoned")?;
        if state.failed {
            return Err("settlement journal stopped after an earlier write failure".into());
        }
        let mut next = state.data.clone();
        let result = f(&mut next)?;
        if let Err(error) = self.persist(&next) {
            state.failed = true;
            return Err(error);
        }
        state.data = next;
        Ok(result)
    }

    fn persist(&self, state: &JournalState) -> Result<(), String> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let body = serde_json::to_vec(state).map_err(|error| error.to_string())?;
        let file = JournalFile {
            version: 1,
            checksum: keccak256(body),
            state: state.clone(),
        };
        let encoded = serde_json::to_vec(&file).map_err(|error| error.to_string())?;
        let temporary = path.with_extension("new");
        let mut output = private_file(&temporary, true)?;
        output
            .write_all(&encoded)
            .and_then(|()| output.sync_all())
            .map_err(|error| format!("write settlement journal: {error}"))?;
        std::fs::rename(&temporary, path)
            .map_err(|error| format!("replace settlement journal: {error}"))?;
        File::open(path.parent().ok_or("journal path has no parent")?)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("sync settlement journal directory: {error}"))?;
        Ok(())
    }
}

fn private_file(path: &Path, truncate: bool) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options
        .create(true)
        .read(true)
        .write(true)
        .truncate(truncate);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| format!("open {}: {error}", path.display()))
}
