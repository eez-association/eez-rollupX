//! Handshake with the required, isolated physical-ENOSPC runner.
use anyhow::{Context, Result, ensure};
use eez_telemetry::{
    reader,
    schema::{Record, StatsSnapshot},
};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct Control {
    pub directory: PathBuf,
    pub spool: PathBuf,
}

impl Control {
    pub fn required() -> Result<Self> {
        let directory = PathBuf::from(
            std::env::var_os("EEZ_TEST_ENOSPC_CONTROL")
                .context("physical ENOSPC test requires the isolated quota runner")?,
        );
        let spool = PathBuf::from(
            std::env::var_os("EEZ_TEST_ENOSPC_SPOOL")
                .context("isolated runner must supply its physical spool path")?,
        );
        ensure!(directory.is_dir(), "quota control directory is missing");
        ensure!(
            spool.parent().is_some_and(Path::is_dir),
            "quota mount is missing"
        );
        Ok(Self { directory, spool })
    }

    pub fn signal(&self, name: &str, value: &Value) -> Result<()> {
        let temporary = self.directory.join(format!("{name}.partial"));
        std::fs::write(&temporary, serde_json::to_vec(value)?)?;
        std::fs::rename(temporary, self.directory.join(format!("{name}.json")))?;
        Ok(())
    }

    pub async fn wait(&self, name: &str) -> Result<Value> {
        let path = self.directory.join(format!("{name}.json"));
        eez_testkit::wait_for(eez_testkit::SETTLE_TIMEOUT, || async {
            match std::fs::read(&path) {
                Ok(bytes) => {
                    ensure!(bytes.len() <= 4096, "oversized quota control record");
                    Ok(Some(serde_json::from_slice(&bytes)?))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            }
        })
        .await
    }

    fn records(&self) -> Result<Vec<Record>> {
        let mut records = Vec::new();
        // The fixture retains 128KiB. Fault-created partial tails never become
        // complete records; read other segments without retaining descriptors.
        for id in reader::inventory(&self.spool)? {
            let mut offset = 0;
            for _ in 0..16 {
                let batch = match reader::read_segment(&self.spool, id, offset) {
                    Ok(batch) => batch,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                    Err(error) => return Err(error.into()),
                };
                for line in batch.lines {
                    records.push(serde_json::from_slice(&line.bytes)?);
                }
                if batch.snapshot_complete || batch.tail.is_some() {
                    break;
                }
                ensure!(batch.next_offset > offset, "quota reader made no progress");
                offset = batch.next_offset;
            }
        }
        Ok(records)
    }

    pub fn stats(&self) -> Result<Option<StatsSnapshot>> {
        self.stats_after(0)
    }

    pub fn stats_after(&self, observed_at: u64) -> Result<Option<StatsSnapshot>> {
        Ok(self
            .records()?
            .into_iter()
            .filter_map(|record| match record {
                Record::Control {
                    stats,
                    observed_at_unix_ms,
                    ..
                } if observed_at_unix_ms >= observed_at => Some(stats),
                _ => None,
            })
            .max_by_key(|s| (s.last_assigned_sequence, s.io_errors, s.written_records)))
    }

    pub fn persisted_commits(&self, hashes: &[alloy_primitives::B256]) -> Result<bool> {
        use eez_telemetry::schema::{Event, Outcome};
        let mut chunks = std::collections::BTreeMap::new();
        for record in self.records()? {
            if let Record::Data(chunk) = record {
                chunks
                    .entry((chunk.epoch, chunk.sequence))
                    .or_insert_with(Vec::new)
                    .push(chunk);
            }
        }
        let events = chunks
            .values()
            .filter(|parts| parts.len() == parts[0].chunk_count as usize)
            .map(|parts| eez_telemetry::assemble(parts).map_err(anyhow::Error::msg))
            .collect::<Result<Vec<_>>>()?;
        Ok(hashes.iter().all(|hash| {
            events.iter().any(|payload| matches!(payload.event, Event::Admitted {source,..} if source.transaction_hash.0==hash.0))
                && events.iter().any(|payload| matches!(payload.event, Event::Terminal {member,outcome:Outcome::Committed,..} if member.source.transaction_hash.0==hash.0))
        }))
    }
}
