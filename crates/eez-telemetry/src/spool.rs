use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

use serde::{Deserialize, Serialize};

pub use crate::schema::StatsSnapshot;
use crate::{
    encode,
    schema::{
        AdmissionId, Epoch, Event, Identity, Payload, PreparationId, ReconciliationId, Record,
        TraceContext, VERSION, WriterStatus,
    },
    wire::{MAX_RECORD_BYTES, line},
};

const CONTROL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpoolConfig {
    pub directory: PathBuf,
    #[serde(default = "queue_records")]
    pub queue_records: usize,
    #[serde(default = "segment_bytes")]
    pub segment_bytes: u64,
    #[serde(default = "retained_bytes")]
    pub retained_bytes: u64,
    #[serde(default = "retention_seconds")]
    pub retention_seconds: u64,
}

fn queue_records() -> usize {
    4096
}
fn segment_bytes() -> u64 {
    64 * 1024 * 1024
}
fn retained_bytes() -> u64 {
    2 * 1024 * 1024 * 1024
}
fn retention_seconds() -> u64 {
    24 * 60 * 60
}

impl SpoolConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.directory.is_absolute() {
            return Err("lifecycle directory must be absolute");
        }
        if self.queue_records == 0 || self.queue_records > queue_records() {
            return Err("lifecycle queue must hold 1..4096 records");
        }
        if self.segment_bytes < 4 * MAX_RECORD_BYTES as u64 || self.segment_bytes > segment_bytes()
        {
            return Err("lifecycle segments must be 64 KiB..64 MiB");
        }
        if self.retained_bytes < 2 * self.segment_bytes || self.retained_bytes > retained_bytes() {
            return Err("lifecycle retention must fit two segments and not exceed 2 GiB");
        }
        if self.retention_seconds < 5 || self.retention_seconds > retention_seconds() {
            return Err("lifecycle retention must be 5 seconds..24 hours");
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
struct Stats {
    attempted: AtomicU64,
    dropped: AtomicU64,
    written: AtomicU64,
    sequence: AtomicU64,
    drop_min: AtomicU64,
    drop_max: AtomicU64,
    io_errors: AtomicU64,
}

impl Stats {
    fn drop_record(&self, sequence: u64, reason: &'static str) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
        // Bounds express uncertainty only. Cumulative values never reset on read.
        let _ = self
            .drop_min
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                Some(if old == 0 {
                    sequence
                } else {
                    old.min(sequence)
                })
            });
        self.drop_max.fetch_max(sequence, Ordering::Relaxed);
        metrics::counter!("eez_telemetry_dropped_events_total", "signal" => "lifecycle", "reason" => reason).increment(1);
    }

    fn io_error(&self) {
        self.io_errors.fetch_add(1, Ordering::Relaxed);
        metrics::counter!("eez_telemetry_writer_errors_total", "reason" => "io").increment(1);
    }

    fn snapshot(&self) -> StatsSnapshot {
        let min = self.drop_min.load(Ordering::Relaxed);
        let max = self.drop_max.load(Ordering::Relaxed);
        StatsSnapshot {
            attempted_records: self.attempted.load(Ordering::Relaxed),
            dropped_records: self.dropped.load(Ordering::Relaxed),
            written_records: self.written.load(Ordering::Relaxed),
            last_assigned_sequence: self.sequence.load(Ordering::Relaxed),
            dropped_sequence_min: (min != 0).then_some(min),
            dropped_sequence_max: (max != 0).then_some(max),
            io_errors: self.io_errors.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug)]
struct Shared {
    identity: Identity,
    epoch: Epoch,
    started_at: u64,
    stats: Stats,
    accepting: AtomicBool,
    active_sends: AtomicU64,
}

/// Move-only reservation allocated under the authoritative state owner. It can
/// be published once; dropping it leaves an observable sequence hole.
#[derive(Debug)]
pub struct Ticket {
    epoch: Epoch,
    sequence: u64,
    observed_at: u64,
}

impl Ticket {
    pub fn reconciliation_id(&self) -> ReconciliationId {
        ReconciliationId {
            epoch: self.epoch,
            sequence: self.sequence,
        }
    }
    pub fn preparation_id(&self) -> PreparationId {
        PreparationId {
            epoch: self.epoch,
            sequence: self.sequence,
        }
    }
    pub fn admission_id(&self) -> AdmissionId {
        AdmissionId {
            epoch: self.epoch,
            sequence: self.sequence,
        }
    }
}

#[derive(Debug)]
struct Queued {
    sequence: u64,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Publisher {
    shared: Arc<Shared>,
    sender: SyncSender<Queued>,
}

/// A transition committed but its complete diagnostic snapshot was unavailable.
/// Do not truncate membership or fabricate an admission to fill the gap.
#[derive(Debug, Clone, Copy)]
pub enum CaptureFailure {
    Limit,
    MissingContext,
}

impl Publisher {
    /// Account for a rejected snapshot after releasing the protocol owner.
    /// This consumes its ticket and leaves an explicit sequence gap.
    pub fn discard(&self, ticket: Ticket, failure: CaptureFailure) {
        if ticket.epoch != self.shared.epoch {
            return;
        }
        self.shared.stats.attempted.fetch_add(1, Ordering::Relaxed);
        self.shared.stats.drop_record(
            ticket.sequence,
            match failure {
                CaptureFailure::Limit => "capture_limit",
                CaptureFailure::MissingContext => "capture_context",
            },
        );
    }
    pub fn epoch(&self) -> Epoch {
        self.shared.epoch
    }
    pub fn stats(&self) -> StatsSnapshot {
        self.shared.stats.snapshot()
    }

    /// No serialization, I/O, or mutex acquisition. Call only once a transition
    /// actually commits. A disabled/ended writer does not allocate identities.
    pub fn capture(&self) -> Option<Ticket> {
        if !self.shared.accepting.load(Ordering::Acquire) {
            return None;
        }
        let sequence = self
            .shared
            .stats
            .sequence
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |old| {
                old.checked_add(1)
            })
            .ok()?
            + 1;
        let Ok(observed_at) = now_ms() else {
            self.shared.stats.attempted.fetch_add(1, Ordering::Relaxed);
            self.shared.stats.drop_record(sequence, "clock");
            return None;
        };
        Some(Ticket {
            epoch: self.shared.epoch,
            sequence,
            observed_at,
        })
    }

    /// Encode after releasing protocol locks. Never waits for a queue slot or
    /// filesystem operation. Returns false if any chunk could not be delivered;
    /// the caller must not retry or change a protocol decision based on this.
    pub fn publish(&self, ticket: Ticket, event: Event, trace: Option<TraceContext>) -> bool {
        let stats = &self.shared.stats;
        if ticket.epoch != self.shared.epoch {
            return false;
        }
        let payload = Payload {
            observed_at_unix_ms: ticket.observed_at,
            trace,
            event,
        };
        let Ok(records) = encode(
            &self.shared.identity,
            ticket.epoch,
            ticket.sequence,
            &payload,
        ) else {
            stats.attempted.fetch_add(1, Ordering::Relaxed);
            stats.drop_record(ticket.sequence, "encoding");
            return false;
        };
        let mut success = true;
        for bytes in records {
            stats.attempted.fetch_add(1, Ordering::Relaxed);
            // Pair the send reservation with shutdown's accepting flag in one
            // total order. The writer cannot finish its final drain while an
            // already admitted send is still about to reach the channel.
            self.shared.active_sends.fetch_add(1, Ordering::SeqCst);
            let reason = if self.shared.accepting.load(Ordering::SeqCst) {
                match self.sender.try_send(Queued {
                    sequence: ticket.sequence,
                    bytes,
                }) {
                    Ok(()) => None,
                    Err(TrySendError::Full(_)) => Some("queue_full"),
                    Err(TrySendError::Disconnected(_)) => Some("writer_closed"),
                }
            } else {
                Some("shutdown")
            };
            self.shared.active_sends.fetch_sub(1, Ordering::SeqCst);
            if let Some(reason) = reason {
                stats.drop_record(ticket.sequence, reason);
                success = false;
            }
        }
        success
    }
}

#[derive(Debug)]
pub struct Writer {
    shared: Arc<Shared>,
    done: Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    /// Configuration errors and lock/storage failures are explicit to the
    /// entrypoint. Runtime integration reports a disabled exporter on collision;
    /// it must not make lifecycle storage a prerequisite for settlement.
    pub fn start(identity: Identity, config: SpoolConfig) -> io::Result<(Publisher, Self)> {
        identity.validate().map_err(io::Error::other)?;
        config.validate().map_err(io::Error::other)?;
        private_directory(&config.directory)?;
        let instance_lock = lock_file(&config.directory.join("instance.lock"), false)?;
        let epoch = Epoch::random()?;
        let epoch_lock = lock_file(
            &config
                .directory
                .join(format!("{}.lock", hex::encode(epoch.0))),
            true,
        )?;
        let shared = Arc::new(Shared {
            identity,
            epoch,
            started_at: now_ms()?,
            stats: Stats::default(),
            accepting: AtomicBool::new(true),
            active_sends: AtomicU64::new(0),
        });
        let (sender, receiver) = mpsc::sync_channel(config.queue_records);
        let (done_sender, done) = mpsc::sync_channel(1);
        let mut spool = Spool {
            shared: Arc::clone(&shared),
            config,
            current: None,
            control_sequence: 0,
            retry_at: Instant::now(),
        };
        spool.open()?;
        spool.control(WriterStatus::Started)?;
        spool.retain()?;
        let worker = thread::Builder::new()
            .name("eez-lifecycle".into())
            .spawn(move || {
                let _locks = (instance_lock, epoch_lock);
                spool.run(receiver);
                let _ = done_sender.send(());
            })?;
        Ok((
            Publisher {
                shared: Arc::clone(&shared),
                sender,
            },
            Self {
                shared,
                done,
                thread: Some(worker),
            },
        ))
    }

    /// Call after protocol draining. Never waits beyond min(2s, remaining/4).
    /// A stuck filesystem thread is detached; process exit may lose trailing
    /// telemetry, which the observer treats as an unknown crash window.
    pub fn shutdown(mut self, remaining: Duration) -> bool {
        self.shared.accepting.store(false, Ordering::SeqCst);
        let complete = self
            .done
            .recv_timeout((remaining / 4).min(Duration::from_secs(2)))
            .is_ok();
        if complete && let Some(thread) = self.thread.take() {
            return thread.join().is_ok();
        }
        complete
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.shared.accepting.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Live,
    Ended,
    Unknown,
}

/// Non-writing shared-lock probe. Age, missing files and probe errors are never
/// evidence that a writer has ended. An acquired lock is released before return.
pub fn probe_epoch(directory: &Path, epoch: Epoch) -> Liveness {
    let Ok(file) =
        crate::reader::open_regular(&directory.join(format!("{}.lock", hex::encode(epoch.0))))
    else {
        return Liveness::Unknown;
    };
    match file.try_lock_shared() {
        Ok(()) => Liveness::Ended,
        Err(TryLockError::WouldBlock) => Liveness::Live,
        Err(TryLockError::Error(_)) => Liveness::Unknown,
    }
}

fn now_ms() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_millis()
        .try_into()
        .map_err(io::Error::other)
}

fn private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o750);
    builder.create(path)?;
    #[cfg(unix)]
    if fs::metadata(path)?.permissions().mode() & 0o027 != 0 {
        return Err(io::Error::other(
            "lifecycle directory must not be writable by group or accessible by others",
        ));
    }
    Ok(())
}

fn open_private(path: &Path, create_new: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(!create_new)
        .create_new(create_new);
    #[cfg(unix)]
    options.mode(0o640);
    options.open(path)
}

fn lock_file(path: &Path, create_new: bool) -> io::Result<File> {
    let file = open_private(path, create_new)?;
    file.try_lock()
        .map_err(|error| io::Error::other(format!("lifecycle writer lock unavailable: {error}")))?;
    Ok(file)
}

struct Segment {
    file: File,
    path: PathBuf,
    bytes: u64,
}
struct Spool {
    shared: Arc<Shared>,
    config: SpoolConfig,
    current: Option<Segment>,
    control_sequence: u64,
    retry_at: Instant,
}

impl Spool {
    fn open(&mut self) -> io::Result<()> {
        let segment_id = Epoch::random()?;
        let path = self.config.directory.join(format!(
            "{}-{}.active",
            hex::encode(self.shared.epoch.0),
            hex::encode(segment_id.0)
        ));
        let mut file = open_private(&path, true)?;
        let header = line(&Record::Header {
            version: VERSION,
            identity: self.shared.identity.clone(),
            epoch: self.shared.epoch,
            segment_id,
            process_id: std::process::id(),
            started_at_unix_ms: self.shared.started_at,
            segment_created_at_unix_ms: now_ms()?,
        })?;
        file.write_all(&header)?;
        self.current = Some(Segment {
            file,
            path,
            bytes: header.len() as u64,
        });
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let segment = self
            .current
            .as_mut()
            .ok_or_else(|| io::Error::other("lifecycle segment unavailable"))?;
        segment.file.write_all(bytes)?;
        segment.bytes += bytes.len() as u64;
        Ok(())
    }

    fn control(&mut self, status: WriterStatus) -> io::Result<()> {
        self.control_sequence = self
            .control_sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("lifecycle control sequence exhausted"))?;
        let record = Record::Control {
            version: VERSION,
            identity: self.shared.identity.clone(),
            epoch: self.shared.epoch,
            control_sequence: self.control_sequence,
            observed_at_unix_ms: now_ms()?,
            status,
            stats: self.shared.stats.snapshot(),
        };
        self.write(&line(&record)?)
    }

    fn close(&mut self, status: WriterStatus) -> io::Result<()> {
        self.control(status)?;
        if let Some(segment) = self.current.take() {
            // Each write already reached the OS. No fsync durability guarantee or
            // blocking flush on the protocol path is implied by segment close.
            drop(segment.file);
            fs::rename(&segment.path, segment.path.with_extension("closed"))?;
        }
        Ok(())
    }

    fn fail(&mut self) {
        self.shared.stats.io_error();
        // Do not append after a partial write. Leave its .active fragment for the
        // reader to classify after this epoch ends and retry in a fresh segment.
        self.current.take();
        self.retry_at = Instant::now() + CONTROL_INTERVAL;
    }

    fn ensure_open(&mut self) -> io::Result<()> {
        if self.current.is_none() {
            if Instant::now() < self.retry_at {
                return Err(io::Error::other("lifecycle I/O backoff"));
            }
            self.open()?;
        }
        Ok(())
    }

    fn maintain(&mut self) -> io::Result<()> {
        self.ensure_open()?;
        // Reserve enough room for a maximal data record and close control.
        if self.current.as_ref().is_some_and(|segment| {
            segment.bytes + 2 * MAX_RECORD_BYTES as u64 > self.config.segment_bytes
        }) {
            self.close(WriterStatus::SegmentClosed)?;
            self.open()?;
            self.retain()?;
        }
        Ok(())
    }

    fn run(&mut self, receiver: Receiver<Queued>) {
        let mut last_control = Instant::now();
        loop {
            let accepting = self.shared.accepting.load(Ordering::SeqCst);
            // Read reservations BEFORE checking the queue, so a send that
            // completed while this value changed cannot be missed at shutdown.
            let draining_complete =
                !accepting && self.shared.active_sends.load(Ordering::SeqCst) == 0;
            let record = if draining_complete {
                receiver.try_recv().ok()
            } else {
                receiver.recv_timeout(Duration::from_millis(100)).ok()
            };
            if let Some(record) = record {
                if self
                    .maintain()
                    .and_then(|()| self.write(&record.bytes))
                    .is_err()
                {
                    self.shared.stats.drop_record(record.sequence, "io");
                    // Backoff is fixed from the actual failure, not extended by
                    // every subsequent producer record during a storage outage.
                    if self.current.is_some() || Instant::now() >= self.retry_at {
                        self.fail();
                    }
                } else {
                    self.shared.stats.written.fetch_add(1, Ordering::Relaxed);
                }
            } else if draining_complete {
                break;
            }
            if last_control.elapsed() >= CONTROL_INTERVAL {
                if self
                    .maintain()
                    .and_then(|()| self.control(WriterStatus::Active))
                    .is_err()
                {
                    self.fail();
                }
                if self.retain().is_err() {
                    self.shared.stats.io_error();
                }
                last_control = Instant::now();
            }
        }
        if self
            .ensure_open()
            .and_then(|()| self.close(WriterStatus::Stopped))
            .is_err()
        {
            self.fail();
        }
        if self.retain().is_err() {
            self.shared.stats.io_error();
        }
    }

    fn retain(&self) -> io::Result<()> {
        let mut files = Vec::new();
        let mut total = 0u64;
        let now = SystemTime::now();
        for entry in fs::read_dir(&self.config.directory)? {
            let entry = entry?;
            let path = entry.path();
            let Some(extension) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !matches!(extension, "active" | "closed") || !entry.file_type()?.is_file() {
                continue;
            }
            let metadata = entry.metadata()?;
            total = total.saturating_add(metadata.len());
            let is_current = self
                .current
                .as_ref()
                .is_some_and(|segment| segment.path == path);
            let ended = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.split_once('-'))
                .and_then(|(epoch, _)| {
                    let mut bytes = [0; 16];
                    hex::decode_to_slice(epoch, &mut bytes)
                        .ok()
                        .map(|()| Epoch(bytes))
                })
                .is_some_and(|epoch| {
                    epoch == self.shared.epoch
                        || probe_epoch(&self.config.directory, epoch) == Liveness::Ended
                });
            if !is_current && (extension == "closed" || ended) {
                files.push((metadata.modified()?, path, metadata.len()));
            }
        }
        files.sort();
        for (modified, path, size) in files {
            let expired = now
                .duration_since(modified)
                .is_ok_and(|age| age >= Duration::from_secs(self.config.retention_seconds));
            // Leave headroom for the active segment to grow to its limit before
            // the next retention pass. Physical open-unlinked bytes still need
            // the deployment's independent filesystem quota.
            if total > self.config.retained_bytes - self.config.segment_bytes || expired {
                fs::remove_file(path)?;
                total = total.saturating_sub(size);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{BlockRef, Hash32};

    fn fixture() -> (tempfile::TempDir, Publisher, Receiver<Queued>, Spool) {
        let directory = tempfile::tempdir().unwrap();
        let config: SpoolConfig = serde_json::from_value(serde_json::json!({
            "directory": directory.path().join("spool"), "queue_records": 1,
            "segment_bytes": 65536, "retained_bytes": 131072, "retention_seconds": 5
        }))
        .unwrap();
        private_directory(&config.directory).unwrap();
        let shared = Arc::new(Shared {
            identity: Identity {
                environment: "test".into(),
                deployment: "fixture".into(),
                service: "composer".into(),
                instance: "one".into(),
            },
            epoch: Epoch::random().unwrap(),
            started_at: now_ms().unwrap(),
            stats: Stats::default(),
            accepting: AtomicBool::new(true),
            active_sends: AtomicU64::new(0),
        });
        let (sender, receiver) = mpsc::sync_channel(1);
        let publisher = Publisher {
            shared: Arc::clone(&shared),
            sender,
        };
        let mut spool = Spool {
            shared,
            config,
            current: None,
            control_sequence: 0,
            retry_at: Instant::now(),
        };
        spool.open().unwrap();
        (directory, publisher, receiver, spool)
    }

    fn event() -> Event {
        Event::Reorg {
            details: None,
            ancestor: BlockRef {
                number: 1,
                hash: Hash32([1; 32]),
            },
        }
    }

    #[test]
    fn full_channel_does_not_block_and_control_records_bypass_it() {
        let (_directory, publisher, receiver, mut spool) = fixture();
        assert!(publisher.publish(publisher.capture().unwrap(), event(), None));
        for _ in 0..99 {
            assert!(!publisher.publish(publisher.capture().unwrap(), event(), None));
        }
        let stats = publisher.stats();
        assert_eq!(stats.attempted_records, 100);
        assert_eq!(stats.dropped_records, 99);
        assert_eq!(stats.dropped_sequence_min, Some(2));
        assert_eq!(stats.dropped_sequence_max, Some(100));
        spool.control(WriterStatus::Active).unwrap();
        let contents = fs::read_to_string(&spool.current.as_ref().unwrap().path).unwrap();
        let Record::Control {
            stats: control_stats,
            control_sequence,
            ..
        } = serde_json::from_str(contents.lines().last().unwrap()).unwrap()
        else {
            panic!("expected control record");
        };
        assert_eq!(control_stats, stats);
        assert_eq!(control_sequence, 1);
        assert_eq!(receiver.try_recv().unwrap().sequence, 1);
        drop(receiver);
        assert!(!publisher.publish(publisher.capture().unwrap(), event(), None));
        assert_eq!(publisher.stats().dropped_records, 100);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn disk_failure_preserves_partial_segment_and_recovers_into_new_identity() {
        let (_directory, publisher, receiver, mut spool) = fixture();
        assert!(publisher.publish(publisher.capture().unwrap(), event(), None));
        let old_path = spool.current.as_ref().unwrap().path.clone();
        spool.write(b"{partial").unwrap();
        // /dev/full provides an actual ENOSPC without filling any shared disk.
        spool.current.as_mut().unwrap().file =
            OpenOptions::new().write(true).open("/dev/full").unwrap();
        let record = receiver.try_recv().unwrap();
        assert!(spool.write(&record.bytes).is_err());
        spool.shared.stats.drop_record(record.sequence, "io");
        spool.fail();
        assert!(spool.ensure_open().is_err());
        // Advance the retry eligibility deterministically, without a timed sleep.
        spool.retry_at = Instant::now();
        spool.ensure_open().unwrap();
        assert_ne!(spool.current.as_ref().unwrap().path, old_path);
        assert!(publisher.publish(publisher.capture().unwrap(), event(), None));
        let record = receiver.try_recv().unwrap();
        spool.write(&record.bytes).unwrap();
        assert!(fs::read(&old_path).unwrap().ends_with(b"{partial"));
        assert_eq!(publisher.stats().dropped_records, 1);
        assert_eq!(publisher.stats().io_errors, 1);
    }

    #[test]
    fn expiration_removes_old_segments_without_copy_truncating_or_touching_other_files() {
        let (_directory, _publisher, _receiver, mut spool) = fixture();
        let old_path = spool
            .current
            .as_ref()
            .unwrap()
            .path
            .with_extension("closed");
        spool.close(WriterStatus::SegmentClosed).unwrap();
        File::options()
            .write(true)
            .open(&old_path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH))
            .unwrap();
        let unrelated = spool.config.directory.join("operator-note.txt");
        fs::write(&unrelated, "keep").unwrap();
        spool.open().unwrap();
        spool.retain().unwrap();
        assert!(!old_path.exists());
        assert!(spool.current.as_ref().unwrap().path.exists());
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "keep");
    }

    #[test]
    fn shutdown_rejects_late_publication_without_reusing_sequence() {
        let (_directory, publisher, _receiver, mut spool) = fixture();
        let ticket = publisher.capture().unwrap();
        publisher.shared.accepting.store(false, Ordering::Release);
        assert!(!publisher.publish(ticket, event(), None));
        assert!(publisher.capture().is_none());
        assert_eq!(publisher.stats().last_assigned_sequence, 1);
        assert_eq!(publisher.stats().dropped_records, 1);
        spool.close(WriterStatus::Stopped).unwrap();
    }

    #[test]
    fn shutdown_drains_a_send_already_admitted_before_the_stop_flag() {
        let (_directory, publisher, receiver, mut spool) = fixture();
        assert!(publisher.publish(publisher.capture().unwrap(), event(), None));
        let second = publisher.capture().unwrap();
        let bytes = encode(
            &publisher.shared.identity,
            publisher.epoch(),
            second.sequence,
            &Payload {
                observed_at_unix_ms: second.observed_at,
                trace: None,
                event: event(),
            },
        )
        .unwrap()
        .pop()
        .unwrap();
        // Model a producer preempted after reserving a send and observing true,
        // but before its try_send. Shutdown must not retire the receiver yet.
        publisher.shared.active_sends.fetch_add(1, Ordering::SeqCst);
        publisher.shared.accepting.store(false, Ordering::SeqCst);
        let (finished, done) = mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            spool.run(receiver);
            finished.send(()).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while publisher.stats().written_records != 1 {
            assert!(
                Instant::now() < deadline,
                "writer did not consume the first record"
            );
            thread::yield_now();
        }
        assert!(!worker.is_finished());
        publisher
            .shared
            .stats
            .attempted
            .fetch_add(1, Ordering::Relaxed);
        publisher
            .sender
            .try_send(Queued {
                sequence: second.sequence,
                bytes,
            })
            .unwrap();
        publisher.shared.active_sends.fetch_sub(1, Ordering::SeqCst);
        done.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        assert_eq!(publisher.stats().attempted_records, 2);
        assert_eq!(publisher.stats().written_records, 2);
        assert_eq!(publisher.stats().dropped_records, 0);
    }
}
