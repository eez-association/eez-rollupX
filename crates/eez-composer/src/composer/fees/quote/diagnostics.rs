//! Bounded, debug-only execution details. Provider errors never enter this projection.
use super::{EstimateError, Value, json};
use alloy_sol_types::{Panic, Revert, SolError};

const MAX_REVERT_BYTES: usize = 4096;
const MAX_MESSAGE_BYTES: usize = 512;

pub(super) fn execution(stage: &str, cause: &EstimateError) -> Option<Value> {
    let mut details = match cause {
        EstimateError::Revert(data) => {
            // Decode only complete, bounded payloads. Unknown/custom errors retain
            // their bytes; a selector alone does not establish an ABI or origin.
            let message = if data.len() <= MAX_REVERT_BYTES {
                Revert::abi_decode_validate(data)
                    .map(|error| error.reason)
                    .or_else(|_| Panic::abi_decode_validate(data).map(|error| error.to_string()))
                    .unwrap_or_else(|_| "execution reverted".into())
            } else {
                "execution reverted".into()
            };
            let mut details = message_details("execution_reverted", &message);
            details["revertData"] = json!(alloy_primitives::hex::encode_prefixed(
                &data[..data.len().min(MAX_REVERT_BYTES)]
            ));
            details["revertDataLength"] = json!(data.len());
            details["revertDataTruncated"] = json!(data.len() > MAX_REVERT_BYTES);
            details
        }
        EstimateError::Effects(message) => message_details("effects_mismatch", message),
        EstimateError::Unsupported(message) => message_details("unsupported_execution", message),
        _ => return None,
    };
    details["stage"] = json!(stage);
    Some(details)
}

fn message_details(reason: &str, message: &str) -> Value {
    let mut end = message.len().min(MAX_MESSAGE_BYTES);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    json!({
        "reason":reason,
        "message":&message[..end],
        "messageTruncated":end < message.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;

    #[test]
    fn decodes_standard_errors_without_guessing_custom_or_malformed_abis() {
        let cases = [
            (
                Revert::from("order expired").abi_encode(),
                "order expired".to_owned(),
            ),
            (
                Panic::from(0x11_u64).abi_encode(),
                Panic::from(0x11_u64).to_string(),
            ),
            (vec![], "execution reverted".to_owned()),
            (
                vec![0xf9, 0xd3, 0x30, 0xad],
                "execution reverted".to_owned(),
            ),
            (Revert::SELECTOR.to_vec(), "execution reverted".to_owned()),
            (
                [Revert::SELECTOR.as_slice(), &[0xff; 64]].concat(),
                "execution reverted".to_owned(),
            ),
        ];
        for (data, message) in cases {
            let details = execution(
                "gas_estimation",
                &EstimateError::Revert(data.clone().into()),
            )
            .unwrap();
            assert_eq!(details["message"], message);
            assert_eq!(
                details["revertData"],
                alloy_primitives::hex::encode_prefixed(&data)
            );
            assert_eq!(details["revertDataLength"], data.len());
            assert_eq!(details["revertDataTruncated"], false);
            assert_eq!(details["messageTruncated"], false);
        }
    }

    #[test]
    fn bounds_revert_data_and_utf8_messages_without_losing_length_or_stage() {
        for len in [MAX_REVERT_BYTES, MAX_REVERT_BYTES + 1, MAX_REVERT_BYTES * 2] {
            let details = execution(
                "native_replay",
                &EstimateError::Revert(Bytes::from(vec![0xff; len])),
            )
            .unwrap();
            assert_eq!(
                details["revertData"].as_str().unwrap().len(),
                2 + 2 * len.min(MAX_REVERT_BYTES)
            );
            assert_eq!(details["revertDataLength"], len);
            assert_eq!(details["revertDataTruncated"], len > MAX_REVERT_BYTES);
            assert_eq!(details["stage"], "native_replay");
        }
        let message = format!("{}€", "x".repeat(MAX_MESSAGE_BYTES - 1));
        for cause in [
            EstimateError::Effects(message.clone()),
            EstimateError::Unsupported(message.clone()),
            EstimateError::Revert(Revert::from(message.clone()).abi_encode().into()),
        ] {
            let details = execution("final_validation", &cause).unwrap();
            assert_eq!(details["message"], "x".repeat(MAX_MESSAGE_BYTES - 1));
            assert_eq!(details["messageTruncated"], true);
        }
        let details = execution(
            "final_validation",
            &EstimateError::Effects("x".repeat(MAX_MESSAGE_BYTES)),
        )
        .unwrap();
        assert_eq!(
            details["message"].as_str().unwrap().len(),
            MAX_MESSAGE_BYTES
        );
        assert_eq!(details["messageTruncated"], false);
    }

    #[test]
    fn operational_errors_are_not_exposed_as_execution_details() {
        for error in [
            EstimateError::Unavailable("private provider detail".into()),
            EstimateError::Invalid("private input detail".into()),
            EstimateError::Timeout,
            EstimateError::SettlementPending,
            EstimateError::ContextChanged,
        ] {
            assert!(execution("gas_estimation", &error).is_none());
        }
    }
}
