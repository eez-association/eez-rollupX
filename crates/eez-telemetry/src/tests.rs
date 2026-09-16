use std::{collections::HashSet, fs, time::Duration};

use crate::{Liveness, SpoolConfig, Writer, assemble, encode, probe_epoch, schema::*};

fn private_tempdir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o750)).unwrap();
    }
    dir
}

fn identity() -> Identity {
    Identity {
        environment: "devnet".into(),
        deployment: "fixture".into(),
        service: "composer".into(),
        instance: "node-1".into(),
    }
}

fn config(directory: &std::path::Path) -> SpoolConfig {
    serde_json::from_value(serde_json::json!({"directory": directory})).unwrap()
}

fn event() -> Event {
    Event::Reorg {
        ancestor: BlockRef {
            number: 12,
            hash: Hash32([7; 32]),
        },
    }
}

fn data(records: Vec<Vec<u8>>) -> Vec<Data> {
    records
        .iter()
        .map(|bytes| match serde_json::from_slice(bytes).unwrap() {
            Record::Data(record) => record,
            _ => panic!("not data"),
        })
        .collect()
}

fn files(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            matches!(
                path.extension().and_then(|s| s.to_str()),
                Some("active" | "closed")
            )
        })
        .collect()
}

fn records(directory: &std::path::Path) -> Vec<Record> {
    files(directory)
        .iter()
        .flat_map(|path| {
            fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn configuration_defaults_and_invalid_combinations() {
    let dir = private_tempdir();
    let mut cfg = config(dir.path());
    assert_eq!(cfg.queue_records, 4096);
    assert_eq!(cfg.segment_bytes, 64 * 1024 * 1024);
    assert_eq!(cfg.retained_bytes, 2 * 1024 * 1024 * 1024);
    assert_eq!(cfg.retention_seconds, 86400);
    cfg.validate().unwrap();
    cfg.queue_records = 0;
    assert!(cfg.validate().is_err());
    cfg.queue_records = 4097;
    assert!(cfg.validate().is_err());
    cfg.queue_records = 1;
    cfg.retained_bytes = cfg.segment_bytes;
    assert!(cfg.validate().is_err());
    cfg.retained_bytes = 2 * cfg.segment_bytes;
    cfg.retention_seconds = 4;
    assert!(cfg.validate().is_err());
    cfg.retention_seconds = 5;
    cfg.directory = "relative".into();
    assert!(cfg.validate().is_err());
    assert!(
        serde_json::from_value::<SpoolConfig>(
            serde_json::json!({"directory":"/tmp/a", "typo":true})
        )
        .is_err()
    );
    let mut bad = identity();
    bad.instance = "../other-instance".into();
    assert!(bad.validate().is_err());
}

#[test]
fn chunk_replay_is_complete_order_independent_and_conflict_intolerant() {
    let epoch = Epoch([1; 16]);
    let admission = AdmissionId { epoch, sequence: 1 };
    let source = Source {
        chain_id: 100,
        genesis_hash: Hash32([2; 32]),
        rollup_id: 0,
        transaction_hash: Hash32([3; 32]),
        direction: Direction::Inbound,
    };
    let members = (0..300)
        .map(|i| Member {
            source,
            admission: AdmissionId {
                sequence: i + 1,
                ..admission
            },
        })
        .collect();
    let payload = Payload {
        observed_at_unix_ms: 12345,
        trace: None,
        event: Event::AttemptPrepared {
            attempt: AttemptId {
                epoch,
                generation: 2,
                post_batch_hash: Hash32([4; 32]),
            },
            sync_height: 10,
            members,
            encoded_bytes: 123,
            blob_count: 1,
            blob_allowance: 2,
        },
    };
    let encoded = encode(&identity(), epoch, 301, &payload).unwrap();
    assert!(
        encoded
            .iter()
            .all(|line| line.len() <= crate::MAX_RECORD_BYTES && line.last() == Some(&b'\n'))
    );
    let mut chunks = data(encoded);
    assert!(chunks.len() > 1);
    assert!(
        chunks[..chunks.len() - 1]
            .iter()
            .all(|chunk| !chunk.complete && chunk.digest.is_none())
    );
    assert!(chunks.last().unwrap().complete);
    assert_eq!(assemble(&chunks).unwrap(), payload);
    let final_chunk = chunks.pop().unwrap();
    assert_eq!(assemble(&chunks).unwrap_err(), "missing chunks");
    chunks.push(final_chunk);
    chunks.reverse();
    chunks.push(chunks[0].clone());
    assert_eq!(assemble(&chunks).unwrap(), payload);
    chunks
        .last_mut()
        .unwrap()
        .payload_hex
        .replace_range(0..2, "00");
    assert_eq!(
        assemble(&chunks).unwrap_err(),
        "conflicting duplicate chunk"
    );
    chunks.pop();
    chunks[0].payload_hex.replace_range(0..2, "00");
    assert_eq!(
        assemble(&chunks).unwrap_err(),
        "lifecycle payload digest mismatch"
    );
}

#[test]
fn reject_unknown_schema_scope_corruption_and_oversize_claims() {
    let payload = Payload {
        observed_at_unix_ms: 10,
        trace: None,
        event: event(),
    };
    let original = data(encode(&identity(), Epoch([1; 16]), 1, &payload).unwrap());
    let mut chunks = original.clone();
    chunks[0].version = 2;
    assert!(assemble(&chunks).is_err());
    chunks = original.clone();
    chunks[0].chunk_count = u32::MAX;
    assert!(assemble(&chunks).is_err());
    chunks = original.clone();
    chunks[0].payload_hex = "a".repeat(crate::MAX_RECORD_BYTES * 2);
    assert!(assemble(&chunks).is_err());
    chunks = original.clone();
    chunks[0].complete = false;
    assert!(assemble(&chunks).is_err());
    chunks = original;
    let mut duplicate = chunks[0].clone();
    duplicate.identity.deployment = "reset".into();
    chunks.push(duplicate);
    assert!(assemble(&chunks).is_err());
    assert!(serde_json::from_str::<Hash32>("\"not-a-hash\"").is_err());
    assert!(serde_json::from_str::<Event>(r#"{"event":"reorg","ancestor":{"number":12,"hash":"0707070707070707070707070707070707070707070707070707070707070707"},"raw_tx":"secret"}"#).is_err());
}

#[test]
fn writer_fencing_restart_headers_and_permissions() {
    let dir = private_tempdir();
    let cfg = config(dir.path());
    let (publisher, writer) = Writer::start(identity(), cfg.clone()).unwrap();
    let epoch = publisher.epoch();
    assert_eq!(probe_epoch(dir.path(), epoch), Liveness::Live);
    assert_eq!(probe_epoch(dir.path(), Epoch([0; 16])), Liveness::Unknown);
    assert!(Writer::start(identity(), cfg.clone()).is_err());
    // Header and control exist even if no source request arrives.
    assert!(
        records(dir.path())
            .iter()
            .any(|r| matches!(r, Record::Header { epoch: e, .. } if *e == epoch))
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for entry in fs::read_dir(dir.path()).unwrap() {
            assert_eq!(
                entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
    }
    assert!(writer.shutdown(Duration::from_secs(4)));
    assert!(publisher.capture().is_none());
    assert_eq!(probe_epoch(dir.path(), epoch), Liveness::Ended);
    let (second, writer) = Writer::start(identity(), cfg).unwrap();
    assert_ne!(second.epoch(), epoch);
    assert!(writer.shutdown(Duration::from_secs(4)));
    assert!(
        records(dir.path())
            .iter()
            .any(|r| matches!(r, Record::Header { epoch: e, .. } if *e == second.epoch()))
    );
}

#[test]
fn concurrent_publication_has_unique_transition_sequences_and_explicit_drops() {
    let dir = private_tempdir();
    let (publisher, writer) = Writer::start(identity(), config(dir.path())).unwrap();
    let mut threads = Vec::new();
    for _ in 0..8 {
        let producer = publisher.clone();
        threads.push(std::thread::spawn(move || {
            for _ in 0..100 {
                let first = producer.capture().unwrap();
                let second = producer.capture().unwrap();
                // Publication order is deliberately different from transition order.
                producer.publish(second, event(), None);
                producer.publish(first, event(), None);
            }
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }
    assert!(writer.shutdown(Duration::from_secs(8)));
    let stats = publisher.stats();
    assert_eq!(stats.last_assigned_sequence, 1600);
    assert_eq!(stats.attempted_records, 1600);
    assert_eq!(
        stats.attempted_records,
        stats.written_records + stats.dropped_records
    );
    let data: Vec<_> = records(dir.path())
        .into_iter()
        .filter_map(|r| {
            if let Record::Data(d) = r {
                Some(d)
            } else {
                None
            }
        })
        .collect();
    let ids: HashSet<_> = data
        .iter()
        .map(|d| (d.epoch, d.sequence, d.chunk_index))
        .collect();
    assert_eq!(ids.len(), data.len());
    assert_eq!(data.len() as u64, stats.written_records);
    for record in data {
        assert_eq!(assemble(&[record]).unwrap().event, event());
    }
}

#[test]
fn rotation_keeps_immutable_ids_and_bounded_retention() {
    let dir = private_tempdir();
    let mut cfg = config(dir.path());
    cfg.segment_bytes = 64 * 1024;
    cfg.retained_bytes = 128 * 1024;
    let (publisher, writer) = Writer::start(identity(), cfg.clone()).unwrap();
    let initial = files(dir.path()).pop().unwrap();
    let header: Record = serde_json::from_str(
        fs::read_to_string(&initial)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    let Record::Header { segment_id, .. } = header else {
        panic!("header required")
    };
    assert!(
        initial
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(&hex::encode(segment_id.0))
    );
    for _ in 0..1000 {
        publisher.publish(publisher.capture().unwrap(), event(), None);
    }
    assert!(writer.shutdown(Duration::from_secs(8)));
    let paths = files(dir.path());
    assert!(!initial.exists()); // rotated and ultimately expired by byte retention
    assert!(paths.iter().all(|p| p.extension().unwrap() == "closed"));
    let mut bytes = 0;
    for path in paths {
        let size = fs::metadata(&path).unwrap().len();
        bytes += size;
        assert!(size <= cfg.segment_bytes);
        let contents = fs::read_to_string(&path).unwrap();
        let Record::Header { segment_id, .. } =
            serde_json::from_str(contents.lines().next().unwrap()).unwrap()
        else {
            panic!("header required")
        };
        assert!(
            path.file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with(&hex::encode(segment_id.0))
        );
    }
    assert!(bytes <= cfg.retained_bytes);
}

#[test]
fn bounded_reader_replays_real_closed_writer_segments_without_holding_files() {
    use crate::reader::{inventory, read_segment};
    let directory = private_tempdir();
    let (publisher, writer) = Writer::start(identity(), config(directory.path())).unwrap();
    let epoch = publisher.epoch();
    for _ in 0..80 {
        assert!(publisher.publish(publisher.capture().unwrap(), event(), None));
    }
    assert!(writer.shutdown(Duration::from_secs(8)));
    let mut sequences = HashSet::new();
    for segment in inventory(directory.path()).unwrap() {
        assert_eq!(segment.epoch, epoch);
        let mut offset = 0;
        loop {
            let batch = read_segment(directory.path(), segment, offset).unwrap();
            match serde_json::from_slice::<Record>(&batch.header).unwrap() {
                Record::Header {
                    epoch: header_epoch,
                    segment_id,
                    identity: header_identity,
                    ..
                } => {
                    assert_eq!(header_epoch, epoch);
                    assert_eq!(segment_id, segment.segment);
                    assert_eq!(header_identity, identity());
                }
                _ => panic!("writer header expected"),
            }
            for line in batch.lines {
                if let Record::Data(data) = serde_json::from_slice(&line.bytes).unwrap() {
                    assert!(sequences.insert(data.sequence));
                    assert_eq!(assemble(&[data]).unwrap().event, event());
                }
            }
            assert!(batch.tail.is_none());
            assert!(batch.next_offset >= offset);
            offset = batch.next_offset;
            if batch.snapshot_complete {
                break;
            }
        }
        fs::remove_file(directory.path().join(segment.filename(true))).unwrap();
    }
    assert_eq!(sequences.len(), 80);
    assert_eq!(sequences.iter().copied().max(), Some(80));
    assert!(inventory(directory.path()).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn liveness_probe_does_not_follow_a_replaced_epoch_lock() {
    use std::os::unix::fs::symlink;
    let directory = private_tempdir();
    let epoch = Epoch([1; 16]);
    let outside = directory.path().join("other-file");
    fs::write(&outside, b"").unwrap();
    symlink(
        outside,
        directory
            .path()
            .join(format!("{}.lock", hex::encode(epoch.0))),
    )
    .unwrap();
    assert_eq!(probe_epoch(directory.path(), epoch), Liveness::Unknown);
}
