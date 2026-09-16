//! Version 1 diagnostic schema. Fixed-size identifiers and closed enums prevent
//! callers from accidentally exporting raw transactions or arbitrary error text.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const VERSION: u16 = 1;

macro_rules! hex_id {
    ($name:ident, $len:expr) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub [u8; $len]);

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&hex::encode(self.0))
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                let mut bytes = [0; $len];
                hex::decode_to_slice(&value, &mut bytes).map_err(serde::de::Error::custom)?;
                if value != hex::encode(bytes) {
                    return Err(serde::de::Error::custom("identifier must be lowercase hex"));
                }
                Ok(Self(bytes))
            }
        }
    };
}

hex_id!(Epoch, 16);
hex_id!(Hash32, 32);
hex_id!(TraceId, 16);
hex_id!(SpanId, 8);

impl Epoch {
    pub fn random() -> std::io::Result<Self> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
        Ok(Self(bytes))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub environment: String,
    pub deployment: String,
    pub service: String,
    pub instance: String,
}

impl Identity {
    pub fn validate(&self) -> Result<(), &'static str> {
        for value in [
            &self.environment,
            &self.deployment,
            &self.service,
            &self.instance,
        ] {
            if value.is_empty()
                || value.len() > 64
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
            {
                return Err(
                    "identity fields require 1..64 ASCII letters, digits, dots, underscores or hyphens",
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BlockRef {
    pub number: u64,
    pub hash: Hash32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceState {
    pub l1: BlockRef,
    pub l2: BlockRef,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdmissionId {
    pub epoch: Epoch,
    pub sequence: u64,
}

/// Existing native attempt identity, scoped by process epoch. Never an alternate
/// ownership generation or a claim that a dispatch incurred a receipt charge.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AttemptId {
    pub epoch: Epoch,
    pub generation: u64,
    pub post_batch_hash: Hash32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub chain_id: u64,
    pub genesis_hash: Hash32,
    pub rollup_id: u64,
    pub transaction_hash: Hash32,
    pub direction: Direction,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Member {
    pub source: Source,
    pub admission: AdmissionId,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Committed,
    SourceReverted,
    SourceCompletedWithMissingEffects,
    SourceCompleted,
    SourceNonceSpent,
    AttemptsExhausted,
    NonceGap,
    PoolDiscarded,
    Replaced,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Assurance {
    ReceiptVerified,
    NodeReconciled,
    LocalPolicy,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    Nonce,
    Balance,
    Fee,
    Escrow,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    Admitted {
        source: Source,
        previous: Option<AdmissionId>,
        replaced: Option<AdmissionId>,
    },
    Readiness {
        member: Member,
        source_state: SourceState,
        blocked: Option<BlockReason>,
    },
    AttemptPrepared {
        attempt: AttemptId,
        sync_height: u64,
        members: Vec<Member>,
        encoded_bytes: u64,
        blob_count: u32,
        blob_allowance: u32,
    },
    Dispatched {
        attempt: AttemptId,
        target: BlockRef,
        transaction_hash: Hash32,
        nonce: u64,
    },
    Reconciled {
        attempt: AttemptId,
        anchor: BlockRef,
    },
    Terminal {
        member: Member,
        attempt: Option<AttemptId>,
        outcome: Outcome,
        assurance: Assurance,
        anchor: Option<BlockRef>,
        elapsed_ns: u64,
    },
    Reorg {
        ancestor: BlockRef,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub observed_at_unix_ms: u64,
    pub trace: Option<TraceContext>,
    pub event: Event,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TraceContext {
    pub trace_id: TraceId,
    pub span_id: SpanId,
}

/// JSONL transport record. Chunk bytes are hex-encoded UTF-8 payload fragments;
/// digest is SHA-256 of the concatenated original bytes, not of hex or JSONL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Data {
    pub version: u16,
    pub identity: Identity,
    pub epoch: Epoch,
    pub sequence: u64,
    pub chunk_index: u32,
    pub chunk_count: u32,
    pub payload_hex: String,
    pub complete: bool,
    pub digest: Option<Hash32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WriterStatus {
    Started,
    Active,
    SegmentClosed,
    Stopped,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StatsSnapshot {
    pub attempted_records: u64,
    pub dropped_records: u64,
    pub written_records: u64,
    pub last_assigned_sequence: u64,
    pub dropped_sequence_min: Option<u64>,
    pub dropped_sequence_max: Option<u64>,
    pub io_errors: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "record", rename_all = "snake_case", deny_unknown_fields)]
pub enum Record {
    Header {
        version: u16,
        identity: Identity,
        epoch: Epoch,
        segment_id: Epoch,
        process_id: u32,
        started_at_unix_ms: u64,
        segment_created_at_unix_ms: u64,
    },
    Data(Data),
    Control {
        version: u16,
        identity: Identity,
        epoch: Epoch,
        control_sequence: u64,
        observed_at_unix_ms: u64,
        status: WriterStatus,
        stats: StatsSnapshot,
    },
}
