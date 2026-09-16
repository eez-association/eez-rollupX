//! Strictly bounded source data. Failed samples never become zero-valued data.
use serde::Deserialize;
use serde_json::Value;

use crate::{config::valid_hash, rpc::Failure};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    L1,
    L2,
    ReferenceL1,
    Queue,
}

impl Source {
    pub const fn label(self) -> &'static str {
        match self {
            Self::L1 => "l1",
            Self::L2 => "l2",
            Self::ReferenceL1 => "reference_l1",
            Self::Queue => "queue",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Queue {
    pub queued: u64,
    pub in_flight: u64,
    pub ready: u64,
    pub blocked: u64,
    pub unknown: u64,
    pub oldest_pending_age_ms: Option<u64>,
    pub oldest_service_age_ms: Option<u64>,
    pub evaluated_at_ms: Option<u64>,
    pub source_state: Option<Value>,
}

impl Queue {
    pub fn parse(value: Value) -> Result<Self, Failure> {
        let queue: Self = serde_json::from_value(value).map_err(|_| Failure::Malformed)?;
        let sum = queue
            .ready
            .checked_add(queue.blocked)
            .and_then(|v| v.checked_add(queue.unknown));
        if sum != Some(queue.queued) || queue.queued.checked_add(queue.in_flight).is_none() {
            return Err(Failure::Malformed);
        }
        if let Some(state) = &queue.source_state {
            for chain in ["l1", "l2"] {
                if state[chain]["number"].as_u64().is_none()
                    || !state[chain]["hash"].as_str().is_some_and(valid_hash)
                {
                    return Err(Failure::Malformed);
                }
            }
        }
        Ok(queue)
    }

    pub fn evaluation_valid(&self, now_ms: u64) -> bool {
        self.source_state.as_ref().is_some_and(Value::is_object)
            && self.evaluated_at_ms.is_some_and(|time| {
                time <= now_ms.saturating_add(5_000) && now_ms.saturating_sub(time) <= 60_000
            })
    }
}

#[derive(Clone, Debug)]
pub struct Head {
    pub number: u64,
    pub timestamp: u64,
    pub hash: String,
}

pub(crate) fn quantity(value: &Value) -> Result<u64, Failure> {
    let s = value.as_str().ok_or(Failure::Malformed)?;
    let raw = s.strip_prefix("0x").ok_or(Failure::Malformed)?;
    if raw.is_empty()
        || !raw.bytes().all(|b| b.is_ascii_hexdigit())
        || (raw.len() > 1 && raw.starts_with('0'))
    {
        return Err(Failure::Malformed);
    }
    u64::from_str_radix(raw, 16).map_err(|_| Failure::Malformed)
}

impl Head {
    pub fn parse(value: &Value) -> Result<Self, Failure> {
        let hash = value["hash"]
            .as_str()
            .filter(|s| valid_hash(s))
            .ok_or(Failure::Malformed)?;
        Ok(Self {
            number: quantity(&value["number"])?,
            timestamp: quantity(&value["timestamp"])?,
            hash: hash.to_owned(),
        })
    }
}

#[derive(Clone, Debug)]
pub enum Data {
    Head(Head),
    Queue(Queue),
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub source: Source,
    pub last_success: Option<u64>,
    pub last_attempt: Option<u64>,
    pub data: Option<Data>,
    pub failure: Option<Failure>,
}

impl Sample {
    pub const fn new(source: Source) -> Self {
        Self {
            source,
            last_success: None,
            last_attempt: None,
            data: None,
            failure: None,
        }
    }

    pub fn is_current(&self, now_ms: u64) -> bool {
        self.data.is_some()
            && self.last_success.is_some_and(|time| {
                time <= now_ms.saturating_add(5_000) && now_ms.saturating_sub(time) <= 90_000
            })
    }

    pub fn update(&mut self, result: Result<Data, Failure>, now_ms: u64) {
        self.last_attempt = Some(now_ms);
        match result {
            Ok(data) => {
                self.last_success = Some(now_ms);
                self.data = Some(data);
                self.failure = None;
            }
            Err(error) => {
                self.data = None;
                self.failure = Some(error);
            }
        }
    }
}
