use std::{
    collections::BTreeMap,
    io::{self, Write},
};

use sha2::{Digest, Sha256};

use crate::schema::{Data, Epoch, Hash32, Identity, Payload, Record, VERSION};

pub const MAX_RECORD_BYTES: usize = 16 * 1024;
pub const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;
const CHUNK_BYTES: usize = 6 * 1024;

struct BoundedBytes(Vec<u8>);

impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_EVENT_BYTES {
            return Err(io::Error::other("lifecycle event exceeds byte limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Encode outside protocol locks. One sequence identifies the entire event.
/// Records include their newline and never exceed the wire record limit.
pub fn encode(
    identity: &Identity,
    epoch: Epoch,
    sequence: u64,
    payload: &Payload,
) -> Result<Vec<Vec<u8>>, &'static str> {
    identity.validate()?;
    if sequence == 0 {
        return Err("zero lifecycle sequence");
    }
    let mut bytes = BoundedBytes(Vec::new());
    serde_json::to_writer(&mut bytes, payload)
        .map_err(|_| "invalid or oversized lifecycle payload")?;
    let digest = Hash32(Sha256::digest(&bytes.0).into());
    let count = bytes.0.len().div_ceil(CHUNK_BYTES) as u32;
    bytes
        .0
        .chunks(CHUNK_BYTES)
        .enumerate()
        .map(|(index, chunk)| {
            let complete = index + 1 == count as usize;
            let record = Record::Data(Data {
                version: VERSION,
                identity: identity.clone(),
                epoch,
                sequence,
                chunk_index: index as u32,
                chunk_count: count,
                payload_hex: hex::encode(chunk),
                complete,
                digest: complete.then_some(digest),
            });
            line(&record).map_err(|_| "oversized lifecycle record")
        })
        .collect()
}

pub(crate) fn line(record: &Record) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(io::Error::other("oversized lifecycle record"));
    }
    Ok(bytes)
}

/// Validate one bounded wire chunk before durable staging. This does not prove
/// payload completeness or authenticity; [`assemble`] checks the complete set.
pub fn validate_chunk(record: &Data) -> Result<(), &'static str> {
    record.identity.validate()?;
    if record.version != VERSION
        || record.sequence == 0
        || record.chunk_count == 0
        || record.chunk_count as usize > MAX_EVENT_BYTES.div_ceil(CHUNK_BYTES)
        || record.chunk_index >= record.chunk_count
        || record.payload_hex.is_empty()
        || record.payload_hex.len() > CHUNK_BYTES * 2
        || !record.payload_hex.len().is_multiple_of(2)
        || !record
            .payload_hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || record.complete != (record.chunk_index + 1 == record.chunk_count)
        || record.digest.is_some() != record.complete
    {
        return Err("invalid lifecycle chunk");
    }
    Ok(())
}

/// Assemble only a complete, consistent chunk set. Exact duplicates are safe;
/// conflicting duplicates, mismatched scope/version/digest and incomplete input
/// fail without yielding a partly trusted event. Callers retain/quarantine raw
/// records and their original replay position on error.
pub fn assemble(records: &[Data]) -> Result<Payload, &'static str> {
    let first = records.first().ok_or("missing chunks")?;
    validate_chunk(first)?;
    let mut chunks = BTreeMap::new();
    for record in records {
        validate_chunk(record)?;
        if record.identity != first.identity
            || record.epoch != first.epoch
            || record.sequence != first.sequence
            || record.chunk_count != first.chunk_count
        {
            return Err("inconsistent lifecycle chunks");
        }
        if let Some(previous) = chunks.insert(record.chunk_index, record)
            && previous != record
        {
            return Err("conflicting duplicate chunk");
        }
    }
    if chunks.len() != first.chunk_count as usize {
        return Err("missing chunks");
    }
    let mut bytes = Vec::new();
    for record in chunks.values() {
        let decoded = hex::decode(&record.payload_hex).map_err(|_| "invalid chunk hex")?;
        if record.payload_hex != hex::encode(&decoded) {
            return Err("noncanonical chunk hex");
        }
        if bytes.len().saturating_add(decoded.len()) > MAX_EVENT_BYTES {
            return Err("oversized lifecycle payload");
        }
        bytes.extend(decoded);
    }
    let actual = Hash32(Sha256::digest(&bytes).into());
    if chunks
        .last_key_value()
        .and_then(|(_, record)| record.digest)
        != Some(actual)
    {
        return Err("lifecycle payload digest mismatch");
    }
    serde_json::from_slice(&bytes).map_err(|_| "invalid lifecycle payload")
}
