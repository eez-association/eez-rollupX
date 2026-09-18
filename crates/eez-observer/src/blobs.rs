//! Authenticated version-00 byte measurements. Receipt status and settlement
//! outcomes are deliberately outside this pure transport/codec boundary.
use alloy_primitives::{B256, U256, keccak256};
use eez_blob_codec::{
    Blob, Error as CodecError, physical_blob_versioned_hash, unpack, validated_blob_len,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

// Work/memory budgets, not consensus or active-fork allowances.
pub(crate) const MAX_BLOBS: usize = 64;
pub(crate) const RESPONSE_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct Metadata {
    pub hashes: Vec<B256>,
    pub indices: Vec<U256>,
    pub tail_hash: B256,
    pub tail: Vec<u8>,
}
impl Metadata {
    pub fn parse(raw: &Value, calldata_hash: B256) -> Result<Self, &'static str> {
        let values = raw["blobVersionedHashes"]
            .as_array()
            .ok_or("metadata_unavailable")?;
        if values.is_empty() || values.len() > MAX_BLOBS {
            return Err("limit");
        }
        let hashes = values
            .iter()
            .map(crate::accounting::hash)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "invalid_metadata")?;
        if hashes.iter().any(|hash| hash[0] != 1) {
            return Err("unsupported_carrier");
        }
        let input = raw["input"]
            .as_str()
            .and_then(|raw| raw.strip_prefix("0x"))
            .ok_or("invalid_metadata")?;
        let input = alloy_primitives::hex::decode(input).map_err(|_| "invalid_metadata")?;
        if keccak256(&input) != calldata_hash
            || crate::posting::Analysis::parse(&input).state != "decoded"
        {
            return Err("invalid_metadata");
        }
        let batch =
            eez_protocol::entries::decode_postbatch(&input).map_err(|_| "invalid_metadata")?;
        if batch.blobIndices.len() > MAX_BLOBS {
            return Err("limit");
        }
        Ok(Self {
            hashes,
            indices: batch.blobIndices,
            tail_hash: keccak256(&batch.callData),
            tail: batch.callData.to_vec(),
        })
    }
    /// The deployed native carrier selects every purchased blob once in order.
    /// Other profiles retain their metadata and explicit unknown measurement.
    pub fn supported(&self) -> bool {
        self.indices.len() == self.hashes.len()
            && self
                .indices
                .iter()
                .enumerate()
                .all(|(i, n)| *n == U256::from(i))
    }
}

pub(crate) fn measure(
    value: Value,
    legacy: bool,
    metadata: &Metadata,
) -> Result<u64, &'static str> {
    if !metadata.supported() {
        return Err("unsupported_carrier");
    }
    let values = value["data"].as_array().ok_or("invalid_response")?;
    if values.is_empty() {
        return Err("unavailable");
    }
    if values.len() > MAX_BLOBS {
        return Err("limit");
    }
    let mut bodies = Vec::with_capacity(values.len());
    for value in values {
        let raw = if legacy { &value["blob"] } else { value };
        let raw = raw
            .as_str()
            .and_then(|raw| raw.strip_prefix("0x"))
            .ok_or("invalid_response")?;
        if raw.len() != eez_blob_codec::BLOB_BYTES * 2 {
            return Err("invalid_response");
        }
        let bytes = alloy_primitives::hex::decode(raw).map_err(|_| "invalid_response")?;
        bodies.push(bytes);
    }
    let computed = bodies
        .iter()
        .map(|bytes| physical_blob_versioned_hash(bytes))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "authentication_failed")?;
    if legacy {
        for (value, hash) in values.iter().zip(&computed) {
            let raw = value["kzg_commitment"]
                .as_str()
                .and_then(|raw| raw.strip_prefix("0x"))
                .ok_or("invalid_response")?;
            let commitment = alloy_primitives::hex::decode(raw).map_err(|_| "invalid_response")?;
            if commitment.len() != 48 {
                return Err("invalid_response");
            }
            let mut advertised: [u8; 32] = Sha256::digest(&commitment).into();
            advertised[0] = 1;
            if hash.as_slice() != advertised {
                return Err("authentication_failed");
            }
        }
    }
    let mut ordered = Vec::with_capacity(metadata.hashes.len());
    for expected in &metadata.hashes {
        let index = computed
            .iter()
            .position(|hash| hash == expected)
            .ok_or("authentication_failed")?;
        ordered.push(Blob::try_from_slice(index, &bodies[index]).map_err(|_| "invalid_stream")?);
    }
    let stream = unpack(&ordered).map_err(|_| "invalid_stream")?;
    let length = validated_blob_len(&stream, &metadata.tail).map_err(|error| match error {
        CodecError::UnknownProtocolVersion(_) => "unsupported_codec",
        _ => "invalid_stream",
    })?;
    u64::try_from(length).map_err(|_| "limit")
}

#[cfg(test)]
mod tests {
    use super::*;
    use eez_blob_codec::{Message, blob_kzg_commitments, blob_versioned_hashes, encode, pack};
    use serde_json::json;
    fn fixture(version: u8) -> (Vec<Blob>, Metadata, Value) {
        fixture_operations(version, vec![0; 31])
    }
    fn fixture_operations(version: u8, operations: Vec<u8>) -> (Vec<Blob>, Metadata, Value) {
        let encoded = encode(&[
            Message::ChainOperation {
                chain_id: 7,
                operations: operations.into(),
            },
            Message::CloseBlobStream,
        ])
        .unwrap();
        let mut stream = encoded.blob_portion.to_vec();
        stream[0] = version;
        let bodies = pack(&stream).unwrap();
        let hashes = blob_versioned_hashes(&bodies).unwrap();
        let metadata = Metadata {
            hashes,
            indices: vec![U256::ZERO],
            tail_hash: keccak256([]),
            tail: vec![],
        };
        let response = json!({"data":bodies.iter().map(|blob|format!("0x{}",alloy_primitives::hex::encode(blob.as_bytes()))).collect::<Vec<_>>()});
        (bodies, metadata, response)
    }
    #[test]
    fn inner_derivable_tag_is_opaque_to_outer_version_zero_measurement() {
        // Transport accounting deliberately does not authorize the inner recipe.
        // Tag 3/profile bytes (including zeros) remain part of the exact length;
        // the protocol's inner decoder independently validates their semantics.
        let mut operations = vec![0; 31];
        operations[0] = 3;
        operations[1] = 1;
        let (_, metadata, response) = fixture_operations(0, operations);
        assert_eq!(measure(response, false, &metadata), Ok(43));
    }

    #[test]
    fn authentication_is_required_before_exact_measurement_and_version_interpretation() {
        let (_, mut metadata, response) = fixture(0);
        assert_eq!(measure(response.clone(), false, &metadata), Ok(43));
        metadata.hashes[0] = B256::ZERO;
        assert_eq!(
            measure(response, false, &metadata),
            Err("authentication_failed")
        );
        let (_, metadata, response) = fixture(1);
        assert_eq!(
            measure(response, false, &metadata),
            Err("unsupported_codec")
        );
    }
    #[test]
    fn legacy_sidecars_validate_advertised_commitments_and_carrier_bounds() {
        let (bodies, mut metadata, _) = fixture(0);
        let commitment = blob_kzg_commitments(&bodies).unwrap();
        let mut response = json!({"data":[{"blob":format!("0x{}",alloy_primitives::hex::encode(bodies[0].as_bytes())),"kzg_commitment":format!("0x{}",alloy_primitives::hex::encode(commitment[0]))}]});
        assert_eq!(measure(response.clone(), true, &metadata), Ok(43));
        response["data"][0]["kzg_commitment"] = json!(format!("0x{}", "00".repeat(48)));
        assert_eq!(
            measure(response.clone(), true, &metadata),
            Err("authentication_failed")
        );
        metadata.indices[0] = U256::from(1);
        assert_eq!(
            measure(response, true, &metadata),
            Err("unsupported_carrier")
        );
        metadata.indices[0] = U256::ZERO;
        assert_eq!(
            measure(json!({"data":[]}), false, &metadata),
            Err("unavailable")
        );
        assert_eq!(
            measure(json!({"data":vec!["0x";MAX_BLOBS+1]}), false, &metadata),
            Err("limit")
        );
    }
    #[test]
    fn metadata_is_bound_to_exact_calldata_and_preserves_ordered_hashes_and_indices() {
        let (_, expected, _) = fixture(0);
        let batch = eez_protocol::EvmBatch {
            blobIndices: vec![U256::ZERO],
            ..Default::default()
        };
        let input = eez_protocol::entries::encode_postbatch(&batch);
        let raw = json!({"blobVersionedHashes":expected.hashes,"input":format!("0x{}",alloy_primitives::hex::encode(&input))});
        let parsed = Metadata::parse(&raw, keccak256(&input)).unwrap();
        assert_eq!(parsed.hashes, expected.hashes);
        assert_eq!(parsed.indices, vec![U256::ZERO]);
        assert!(parsed.supported());
        assert_eq!(
            Metadata::parse(&raw, B256::ZERO).unwrap_err(),
            "invalid_metadata"
        );
        let mut absent = raw.clone();
        absent
            .as_object_mut()
            .unwrap()
            .remove("blobVersionedHashes");
        assert_eq!(
            Metadata::parse(&absent, keccak256(&input)).unwrap_err(),
            "metadata_unavailable"
        );
    }
    #[test]
    fn unrelated_non_eez_blobs_cannot_disable_measurement_of_selected_content() {
        let (_, metadata, mut response) = fixture(0);
        let mut other = vec![0; eez_blob_codec::BLOB_BYTES];
        other[0] = 1; // Valid BLS scalar (2^248), but outside EEZ's 31-byte packing.
        assert!(Blob::try_from_slice(0, &other).is_err());
        assert!(physical_blob_versioned_hash(&other).is_ok());
        assert!(physical_blob_versioned_hash(&other[..other.len() - 1]).is_err());
        response["data"].as_array_mut().unwrap().insert(
            0,
            json!(format!("0x{}", alloy_primitives::hex::encode(&other))),
        );
        assert_eq!(measure(response, false, &metadata), Ok(43));
    }
}
