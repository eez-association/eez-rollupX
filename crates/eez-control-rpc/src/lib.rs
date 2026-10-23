//! Shared capabilities, execution witnesses and v2 streaming messages.
//!
//! The Composer checks the offline validation identity, then sends
//! `Begin -> Block* -> Finalize` over `prove.v2.Prover.ProveStream`.
//! Only a Proof attests independently recomputed settlement public inputs.

mod generated;
pub mod identity;

/// Tonic-generated protobuf module for the `prove.v1` package.
pub use generated::v1;
/// Shared streaming contract; execution and settlement profiles remain deployment-specific.
pub use generated::v2;

use prost::Message;

/// Max size of a single `Prove` gRPC message, applied to BOTH the client
/// (encode) and server (decode).
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024 * 1024;

/// Fresh-network L1 history, synchronized randomness and offline v2 proving.
pub const OFFLINE_PROFILE_ID: u32 = 2;

/// Encode one typed [`v1::ProveFailure`] for the gRPC status-details field.
#[must_use]
pub fn encode_prove_failure(failure: &v1::ProveFailure) -> Vec<u8> {
    failure.encode_to_vec()
}

/// Decode a typed [`v1::ProveFailure`] from the gRPC status-details field.
///
/// # Errors
///
/// Returns [`prost::DecodeError`] when the details are not a valid
/// `prove.v1.ProveFailure` protobuf payload.
pub fn decode_prove_failure(details: &[u8]) -> Result<v1::ProveFailure, prost::DecodeError> {
    v1::ProveFailure::decode(details)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use prost::Message;

    use super::v1::{OutboundFailure, ProveFailure, prove_failure};
    use super::v2::{Begin, ClientFrame, Proof, client_frame};
    use super::{MAX_MESSAGE_BYTES, decode_prove_failure, encode_prove_failure};

    #[test]
    fn begin_round_trips_through_protobuf() {
        let frame = ClientFrame {
            request_id: 1,
            kind: Some(client_frame::Kind::Begin(Begin {
                rollup_id: 7,
                anchor_number: 10,
                anchor_hash: vec![1; 32],
                anchor_state_root: vec![2; 32],
            })),
            ..Default::default()
        };
        let encoded = frame.encode_to_vec();
        assert!(encoded.len() < MAX_MESSAGE_BYTES);
        assert_eq!(ClientFrame::decode(encoded.as_slice()).unwrap(), frame);
    }

    #[test]
    fn response_round_trips_and_truncated_payload_is_rejected() {
        let response = Proof {
            public_inputs_hash: vec![0x11; 32],
            proof: vec![0x22; 65],
        };
        let encoded = response.encode_to_vec();

        assert_eq!(Proof::decode(encoded.as_slice()).unwrap(), response);
        assert!(Proof::decode(&encoded[..encoded.len() - 1]).is_err());
    }

    #[test]
    fn prove_failure_details_round_trip() {
        let failure = ProveFailure {
            actionable_failure: Some(prove_failure::ActionableFailure::Outbound(
                OutboundFailure {
                    transaction_index: 3,
                    transaction_hash: vec![0x44; 32],
                },
            )),
        };

        assert_eq!(
            decode_prove_failure(&encode_prove_failure(&failure)).unwrap(),
            failure
        );
    }

    proptest! {
        // Proto decode of arbitrary RPC bytes must never panic.
        #[test]
        fn arbitrary_wire_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let _ = ClientFrame::decode(bytes.as_slice());
            let _ = Proof::decode(bytes.as_slice());
        }
    }
}
