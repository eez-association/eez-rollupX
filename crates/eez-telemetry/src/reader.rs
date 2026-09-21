//! Bounded read-only access to immutable lifecycle segments. These functions do
//! blocking filesystem work; async consumers must call them on a blocking worker.
//! No descriptor survives a read pass, including across database transactions.
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::Path,
};

use sha2::{Digest, Sha256};

use crate::{MAX_RECORD_BYTES, schema::Epoch};

pub const MAX_SEGMENTS: usize = 4096;
pub const MAX_DIRECTORY_ENTRIES: usize = 65536;
pub const RECORDS_PER_PASS: usize = 32;

/// The suffix and inode are deliberately absent: rotation renames the segment
/// without changing its identity. The importer must also validate its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SegmentId {
    pub epoch: Epoch,
    pub segment: Epoch,
}

impl SegmentId {
    pub fn filename(self, closed: bool) -> String {
        format!(
            "{}-{}.{}",
            hex::encode(self.epoch.0),
            hex::encode(self.segment.0),
            if closed { "closed" } else { "active" }
        )
    }

    fn parse(filename: &str) -> Option<Self> {
        let (stem, suffix) = filename.rsplit_once('.')?;
        if !matches!(suffix, "active" | "closed") || stem.len() != 65 {
            return None;
        }
        let (epoch, segment) = stem.split_once('-')?;
        let decode = |text: &str| {
            let mut bytes = [0; 16];
            hex::decode_to_slice(text, &mut bytes).ok()?;
            (hex::encode(bytes) == text).then_some(Epoch(bytes))
        };
        Some(Self {
            epoch: decode(epoch)?,
            segment: decode(segment)?,
        })
    }
}

/// Take a bounded inventory. Sorting is for deterministic enumeration, not event
/// order or writer age. A failed/over-limit inventory must never prove retention
/// loss: only a successful complete inventory can support that claim.
pub fn inventory(directory: &Path) -> io::Result<Vec<SegmentId>> {
    let mut segments = BTreeSet::new();
    for (index, entry) in fs::read_dir(directory)?.enumerate() {
        if index >= MAX_DIRECTORY_ENTRIES {
            return Err(io::Error::other("lifecycle directory entry limit exceeded"));
        }
        let entry = entry?;
        let filename = entry.file_name();
        let Some(filename) = filename.to_str() else {
            continue;
        };
        if let Some(id) = SegmentId::parse(filename) {
            if !entry.file_type()?.is_file() {
                return Err(io::Error::other("lifecycle segment is not a regular file"));
            }
            // A directory enumeration racing a rename can see both names. They
            // are one ID, and read_segment will open its currently existing name.
            segments.insert(id);
            if segments.len() > MAX_SEGMENTS {
                return Err(io::Error::other("lifecycle segment count limit exceeded"));
            }
        }
    }
    Ok(segments.into_iter().collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub offset: u64,
    /// Includes the newline. Invalid/unknown JSON remains available for quarantine.
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tail {
    /// May still be completed by a live writer. Age alone cannot classify death.
    Partial(Line),
    /// Bounded prefix only. Leave the checkpoint here for explicit investigation;
    /// do not allocate an unbounded line or quietly skip bytes to a later newline.
    Oversized(Line),
}

#[derive(Debug)]
pub struct ReadBatch {
    /// Bounded first-line bytes, including newline when fully written. A partial
    /// header must not register an epoch or authorize interpreting later records.
    pub header: Vec<u8>,
    pub header_digest: [u8; 32],
    /// Length when this descriptor was opened; appends wait for the next pass.
    pub snapshot_len: u64,
    pub lines: Vec<Line>,
    pub tail: Option<Tail>,
    /// Immediately after the last complete bounded line, never after a partial.
    pub next_offset: u64,
    pub snapshot_complete: bool,
}

pub(crate) fn open_regular(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A path can change after inventory: refuse symlinks at open and avoid
        // blocking on a FIFO before the descriptor's regular-file check.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("lifecycle segment is not a regular file"));
    }
    Ok(file)
}

fn bounded_line(reader: &mut BufReader<File>, remaining: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(remaining.min(MAX_RECORD_BYTES as u64 + 1))
        .read_until(b'\n', &mut bytes)?;
    Ok(bytes)
}

/// Read at most 32 complete records, one bounded tail and a bounded header.
/// `offset` is a previously committed line boundary (or zero). Header identity,
/// schema and digest comparison with persisted evidence belong to the importer.
/// Complete records and their next offset must commit in one database transaction.
/// NotFound is a possible rotation/retention race; retry a complete inventory
/// before concluding that a previously known source has disappeared.
pub fn read_segment(directory: &Path, id: SegmentId, offset: u64) -> io::Result<ReadBatch> {
    let file = match open_regular(&directory.join(id.filename(false))) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            open_regular(&directory.join(id.filename(true)))?
        }
        Err(error) => return Err(error),
    };
    let snapshot_len = file.metadata()?.len();
    if offset > snapshot_len {
        return Err(io::Error::other(
            "lifecycle segment truncated before checkpoint",
        ));
    }
    let mut reader = BufReader::new(file);
    let header = bounded_line(&mut reader, snapshot_len)?;
    let header_digest = Sha256::digest(&header).into();
    if offset != 0 {
        if header.last() != Some(&b'\n') || offset < header.len() as u64 {
            return Err(io::Error::other(
                "lifecycle checkpoint precedes complete header",
            ));
        }
        reader.seek(SeekFrom::Start(offset - 1))?;
        let mut previous = [0];
        reader.read_exact(&mut previous)?;
        if previous != [b'\n'] {
            return Err(io::Error::other(
                "lifecycle checkpoint is not a line boundary",
            ));
        }
    }
    reader.seek(SeekFrom::Start(offset))?;
    let mut batch = ReadBatch {
        header,
        header_digest,
        snapshot_len,
        lines: Vec::new(),
        tail: None,
        next_offset: offset,
        snapshot_complete: false,
    };
    while batch.lines.len() < RECORDS_PER_PASS && batch.next_offset < snapshot_len {
        let bytes = bounded_line(&mut reader, snapshot_len - batch.next_offset)?;
        if bytes.is_empty() {
            return Err(io::Error::other("lifecycle segment truncated during read"));
        }
        let line = Line {
            offset: batch.next_offset,
            bytes,
        };
        if line.bytes.len() > MAX_RECORD_BYTES {
            batch.tail = Some(Tail::Oversized(line));
            break;
        }
        if line.bytes.last() != Some(&b'\n') {
            batch.tail = Some(Tail::Partial(line));
            break;
        }
        batch.next_offset += line.bytes.len() as u64;
        batch.lines.push(line);
    }
    batch.snapshot_complete = batch.next_offset == snapshot_len;
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn id() -> SegmentId {
        SegmentId {
            epoch: Epoch([1; 16]),
            segment: Epoch([2; 16]),
        }
    }

    #[test]
    fn rotation_reuses_identity_and_partial_tail_never_advances_checkpoint() {
        let directory = tempfile::tempdir().unwrap();
        let active = directory.path().join(id().filename(false));
        let closed = directory.path().join(id().filename(true));
        fs::write(&active, b"header\nvalid\npar").unwrap();
        let first = read_segment(directory.path(), id(), 0).unwrap();
        assert_eq!(first.next_offset, 13);
        assert_eq!(first.lines.len(), 2);
        assert_eq!(first.header, b"header\n");
        assert!(matches!(first.tail, Some(Tail::Partial(_))));
        assert!(!first.snapshot_complete);
        OpenOptions::new()
            .append(true)
            .open(&active)
            .unwrap()
            .write_all(b"tial\n")
            .unwrap();
        fs::rename(&active, &closed).unwrap();
        assert_eq!(inventory(directory.path()).unwrap(), vec![id()]);
        let second = read_segment(directory.path(), id(), first.next_offset).unwrap();
        assert_eq!(second.header_digest, first.header_digest);
        assert_eq!(second.lines[0].bytes, b"partial\n");
        assert!(second.snapshot_complete);
        let replay = read_segment(directory.path(), id(), first.next_offset).unwrap();
        assert_eq!(second.lines, replay.lines);
        assert_eq!(second.next_offset, replay.next_offset);
    }

    #[test]
    fn record_and_line_budgets_preserve_raw_quarantine_positions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(id().filename(false));
        fs::write(&path, b"future schema\n".repeat(RECORDS_PER_PASS + 1)).unwrap();
        let first = read_segment(directory.path(), id(), 0).unwrap();
        assert_eq!(first.lines.len(), RECORDS_PER_PASS);
        assert!(!first.snapshot_complete);
        let second = read_segment(directory.path(), id(), first.next_offset).unwrap();
        assert_eq!(second.lines.len(), 1);
        assert!(second.snapshot_complete);
        let mut bytes = b"header\n".to_vec();
        bytes.extend(vec![b'x'; MAX_RECORD_BYTES + 100]);
        bytes.extend_from_slice(b"\nvalid\n");
        fs::write(&path, bytes).unwrap();
        let batch = read_segment(directory.path(), id(), 0).unwrap();
        assert_eq!(batch.next_offset, 7);
        let Some(Tail::Oversized(line)) = batch.tail else {
            panic!("oversized tail required")
        };
        assert_eq!(line.offset, 7);
        assert_eq!(line.bytes.len(), MAX_RECORD_BYTES + 1);
    }

    #[test]
    fn partial_header_truncation_and_bad_offsets_are_not_valid_checkpoints() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(id().filename(false));
        fs::write(&path, b"partial").unwrap();
        let batch = read_segment(directory.path(), id(), 0).unwrap();
        assert_eq!(batch.next_offset, 0);
        assert!(batch.lines.is_empty());
        assert!(matches!(batch.tail, Some(Tail::Partial(_))));
        assert!(read_segment(directory.path(), id(), 1).is_err());
        assert!(read_segment(directory.path(), id(), 8).is_err());
        fs::write(&path, b"header\ndata\n").unwrap();
        assert!(read_segment(directory.path(), id(), 9).is_err());
        let batch = read_segment(directory.path(), id(), 12).unwrap();
        assert!(batch.snapshot_complete);
        fs::remove_file(path).unwrap();
        assert!(inventory(directory.path()).unwrap().is_empty());
        assert_eq!(
            read_segment(directory.path(), id(), 12).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn filename_validation_is_strict_without_interpreting_directory_order() {
        assert_eq!(SegmentId::parse(&id().filename(true)), Some(id()));
        assert_eq!(SegmentId::parse(&id().filename(false)), Some(id()));
        assert!(SegmentId::parse("../other.active").is_none());
        assert!(SegmentId::parse(&id().filename(false).replace("01", "AA")).is_none());
        assert!(SegmentId::parse(&id().filename(false).replace("active", "jsonl")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn segment_symlinks_and_nonregular_files_are_refused() {
        use std::os::unix::{fs::symlink, net::UnixListener};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(id().filename(false));
        let target = directory.path().join("other");
        fs::write(&target, b"header\n").unwrap();
        symlink(&target, &path).unwrap();
        assert!(inventory(directory.path()).is_err());
        assert!(read_segment(directory.path(), id(), 0).is_err());
        fs::remove_file(&path).unwrap();
        let _listener = UnixListener::bind(&path).unwrap();
        assert!(inventory(directory.path()).is_err());
        assert!(read_segment(directory.path(), id(), 0).is_err());
    }
}
