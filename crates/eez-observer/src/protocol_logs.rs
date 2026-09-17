//! Fixed-width protocol log evidence. Missing/malformed evidence never changes
//! the separately validated inclusion or its receipt charges.
use crate::{
    accounting::{Receipt, address, hash},
    model::quantity,
};
use alloy_primitives::{Address, B256, U256};
use alloy_sol_types::SolEvent;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const MAX_RECEIPT_EVENTS: usize = 4096;
const MAX_LOGS: usize = 32768;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    Missing,
    Malformed,
    Limit,
}
impl Gap {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Missing => "logs_missing",
            Self::Malformed => "logs_malformed",
            Self::Limit => "logs_capture_limit",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Root { root: B256 },
    Consumed { call_hash: B256, queue_index: U256 },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Log {
    pub address: Address,
    pub index: u64,
    pub rollup_id: u64,
    pub event: Event,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub events: Vec<Log>,
}

impl Evidence {
    pub fn parse(value: &Value, receipt: &Receipt) -> Result<Self, Gap> {
        let logs = value
            .get("logs")
            .filter(|v| !v.is_null())
            .ok_or(Gap::Missing)?
            .as_array()
            .ok_or(Gap::Malformed)?;
        if logs.len() > MAX_LOGS {
            return Err(Gap::Limit);
        }
        if !receipt.success && !logs.is_empty() {
            return Err(Gap::Malformed);
        }
        let mut events = Vec::new();
        let mut previous = None;
        for log in logs {
            let index = quantity(&log["logIndex"]).map_err(|_| Gap::Malformed)?;
            if previous.is_some_and(|old| index <= old)
                || hash(&log["blockHash"]).map_err(|_| Gap::Malformed)? != receipt.block_hash
                || hash(&log["transactionHash"]).map_err(|_| Gap::Malformed)?
                    != receipt.transaction_hash
                || quantity(&log["blockNumber"]).map_err(|_| Gap::Malformed)?
                    != receipt.block_number
                || quantity(&log["transactionIndex"]).map_err(|_| Gap::Malformed)?
                    != receipt.transaction_index
                || log.get("removed").is_some_and(|v| v != &Value::Bool(false))
            {
                return Err(Gap::Malformed);
            }
            previous = Some(index);
            let emitter = address(log["address"].as_str().ok_or(Gap::Malformed)?)
                .map_err(|_| Gap::Malformed)?;
            let topics = log["topics"].as_array().ok_or(Gap::Malformed)?;
            if topics.len() > 4 {
                return Err(Gap::Malformed);
            }
            let topics = topics
                .iter()
                .map(|v| hash(v).map_err(|_| Gap::Malformed))
                .collect::<Result<Vec<_>, _>>()?;
            let data = log["data"]
                .as_str()
                .and_then(|v| v.strip_prefix("0x"))
                .ok_or(Gap::Malformed)?;
            if !data.len().is_multiple_of(2) || !data.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(Gap::Malformed);
            }
            let (rollup, event) = match topics.first() {
                Some(signature)
                    if *signature == eez_protocol::abi::L2ExecutionPerformed::SIGNATURE_HASH =>
                {
                    if topics.len() != 2 || data.len() != 64 {
                        return Err(Gap::Malformed);
                    }
                    let bytes = alloy_primitives::hex::decode(data).map_err(|_| Gap::Malformed)?;
                    (
                        topics[1],
                        Event::Root {
                            root: B256::from_slice(&bytes),
                        },
                    )
                }
                Some(signature)
                    if *signature == eez_protocol::abi::ExecutionConsumed::SIGNATURE_HASH =>
                {
                    if topics.len() != 4 || !data.is_empty() {
                        return Err(Gap::Malformed);
                    }
                    (
                        topics[2],
                        Event::Consumed {
                            call_hash: topics[1],
                            queue_index: U256::from_be_bytes(topics[3].0),
                        },
                    )
                }
                _ => continue,
            };
            let rollup_id =
                u64::try_from(U256::from_be_bytes(rollup.0)).map_err(|_| Gap::Malformed)?;
            if events.len() == MAX_RECEIPT_EVENTS {
                return Err(Gap::Limit);
            }
            events.push(Log {
                address: emitter,
                index,
                rollup_id,
                event,
            });
        }
        Ok(Self { events })
    }

    pub fn digest(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        for log in &self.events {
            digest.update(log.address.as_slice());
            digest.update(log.index.to_be_bytes());
            digest.update(log.rollup_id.to_be_bytes());
            match log.event {
                Event::Root { root } => {
                    digest.update([0]);
                    digest.update(root.as_slice());
                }
                Event::Consumed {
                    call_hash,
                    queue_index,
                } => {
                    digest.update([1]);
                    digest.update(call_hash.as_slice());
                    digest.update(queue_index.to_be_bytes::<32>());
                }
            }
        }
        digest.finalize().into()
    }

    pub fn records(&self) -> Value {
        json!(self.events.iter().map(|log| {
            let (kind,root,call,index)=match log.event {
                Event::Root{root}=>("root",Some(alloy_primitives::hex::encode(root)),None,None),
                Event::Consumed{call_hash,queue_index}=>("consumed",None,Some(alloy_primitives::hex::encode(call_hash)),Some(queue_index.to_string())),
            };
            json!({"address":alloy_primitives::hex::encode(log.address),"index":log.index.to_string(),"rollup_id":log.rollup_id.to_string(),"kind":kind,"root":root,"call_hash":call,"queue_index":index})
        }).collect::<Vec<_>>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn receipt() -> Value {
        json!({"transactionHash":format!("{:#x}",B256::repeat_byte(1)),"blockHash":format!("{:#x}",B256::repeat_byte(2)),"blockNumber":"0x3","transactionIndex":"0x4","status":"0x1","type":"0x2","gasUsed":"0x5208","effectiveGasPrice":"0x7","logs":[]})
    }
    fn log(index: u64, consumed: bool) -> Value {
        let r = receipt();
        let mut log = json!({"transactionHash":r["transactionHash"],"blockHash":r["blockHash"],"blockNumber":r["blockNumber"],"transactionIndex":r["transactionIndex"],"logIndex":format!("{index:#x}"),"removed":false,"address":format!("{:#x}",Address::repeat_byte(7))});
        if consumed {
            log["topics"] = json!([
                format!(
                    "{:#x}",
                    eez_protocol::abi::ExecutionConsumed::SIGNATURE_HASH
                ),
                format!("{:#x}", B256::repeat_byte(8)),
                format!("0x{:064x}", 9),
                format!("0x{:064x}", U256::MAX)
            ]);
            log["data"] = json!("0x");
        } else {
            log["topics"] = json!([
                format!(
                    "{:#x}",
                    eez_protocol::abi::L2ExecutionPerformed::SIGNATURE_HASH
                ),
                format!("0x{:064x}", 9)
            ]);
            log["data"] = json!(format!("{:#x}", B256::repeat_byte(10)));
        }
        log
    }
    fn parse(value: &Value) -> Result<Evidence, Gap> {
        Evidence::parse(value, &Receipt::parse(value).unwrap())
    }

    #[test]
    fn empty_missing_malformed_and_reverted_sets_do_not_change_fees() {
        let mut value = receipt();
        let cost = Receipt::parse(&value).unwrap().cost;
        assert!(parse(&value).unwrap().events.is_empty());
        for invalid in [Value::Null, json!(false), json!({})] {
            value["logs"] = invalid;
            assert!(parse(&value).is_err());
            assert_eq!(Receipt::parse(&value).unwrap().cost, cost);
        }
        value.as_object_mut().unwrap().remove("logs");
        assert_eq!(parse(&value), Err(Gap::Missing));
        value["logs"] = json!([log(1, true)]);
        value["status"] = json!("0x0");
        assert_eq!(parse(&value), Err(Gap::Malformed));
        assert_eq!(Receipt::parse(&value).unwrap().cost, cost);
        value["logs"] = json!([]);
        assert!(parse(&value).unwrap().events.is_empty());
    }

    #[test]
    fn exact_positions_emitters_rollups_and_full_width_queue_indices_are_retained() {
        let mut value = receipt();
        let first = log(20, true);
        let mut second = log(21, true);
        second["topics"][3] = json!(format!("0x{:064x}", 2));
        let mut root = log(22, false);
        root["address"] = json!(format!("{:#x}", Address::repeat_byte(11)));
        root["topics"][1] = json!(format!("0x{:064x}", 10));
        value["logs"] = json!([first, second, root]);
        let evidence = parse(&value).unwrap();
        assert_eq!(evidence.events.len(), 3);
        assert_eq!(
            evidence.events[0].event,
            Event::Consumed {
                call_hash: B256::repeat_byte(8),
                queue_index: U256::MAX
            }
        );
        assert_eq!(
            evidence.events[1].event,
            Event::Consumed {
                call_hash: B256::repeat_byte(8),
                queue_index: U256::from(2)
            }
        );
        assert_eq!(evidence.events[2].address, Address::repeat_byte(11));
        assert_eq!(evidence.events[2].rollup_id, 10);
        assert_eq!(evidence.records()[0]["queue_index"], U256::MAX.to_string());
        assert_eq!(evidence.digest(), parse(&value).unwrap().digest());
        value["logs"][2]["topics"][1] = json!(format!("0x{:064x}", 11));
        assert_ne!(evidence.digest(), parse(&value).unwrap().digest());
    }

    #[test]
    fn foreign_removed_duplicate_or_out_of_order_logs_reject_the_whole_set() {
        for (field, wrong) in [
            ("transactionHash", json!(format!("{:#x}", B256::ZERO))),
            ("blockHash", json!(format!("{:#x}", B256::ZERO))),
            ("blockNumber", json!("0x4")),
            ("transactionIndex", json!("0x5")),
            ("removed", json!(true)),
            ("removed", json!("false")),
            ("logIndex", json!("0x01")),
        ] {
            let mut value = receipt();
            let mut bad = log(2, true);
            bad[field] = wrong;
            value["logs"] = json!([log(1, false), bad]);
            assert_eq!(parse(&value), Err(Gap::Malformed), "{field}");
        }
        for index in [0, 1] {
            let mut value = receipt();
            value["logs"] = json!([log(1, false), log(index, true)]);
            assert_eq!(parse(&value), Err(Gap::Malformed));
        }
    }

    #[test]
    fn strict_fixed_abi_and_capture_limits_never_produce_partial_evidence() {
        let mut value = receipt();
        for consumed in [true, false] {
            let mut bad = log(1, consumed);
            bad["data"] = json!("0x01");
            value["logs"] = json!([bad]);
            assert_eq!(parse(&value), Err(Gap::Malformed));
        }
        let mut bad = log(1, false);
        bad["topics"][1] = json!(format!("0x{:064x}", U256::MAX));
        value["logs"] = json!([bad]);
        assert_eq!(parse(&value), Err(Gap::Malformed));
        value["logs"] = json!(
            (0..MAX_RECEIPT_EVENTS as u64)
                .map(|index| log(index, true))
                .collect::<Vec<_>>()
        );
        assert_eq!(parse(&value).unwrap().events.len(), MAX_RECEIPT_EVENTS);
        value["logs"]
            .as_array_mut()
            .unwrap()
            .push(log(MAX_RECEIPT_EVENTS as u64, true));
        assert_eq!(parse(&value), Err(Gap::Limit));
        value["logs"] = json!(vec![Value::Null; MAX_LOGS + 1]);
        assert_eq!(parse(&value), Err(Gap::Limit));
    }
}
