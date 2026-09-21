//! Reproducible synthetic public carrier evidence for the isolated Beacon/RPC
//! fixture. It is not a valid proof or a broadcast transaction.
use alloy_primitives::{U256, hex};
use eez_blob_codec::{Message, blob_kzg_commitments, blob_versioned_hashes, encode, pack};
use eez_protocol::{EvmBatch, entries::encode_postbatch};
use serde_json::json;
fn main() {
    let fixtures = [31, eez_blob_codec::DATA_BYTES_PER_BLOB]
        .into_iter().map(|size| {
            let encoded = encode(&[Message::ChainOperation { chain_id: 1, operations: vec![0; size].into() }, Message::CloseBlobStream]).unwrap();
            let blobs = pack(&encoded.blob_portion).unwrap();
            let commitments = blob_kzg_commitments(&blobs).unwrap();
            let hashes = blob_versioned_hashes(&blobs).unwrap();
            let input = encode_postbatch(&EvmBatch { blobIndices: (0..blobs.len()).map(U256::from).collect(), ..Default::default() });
            json!({"encoded_bytes": encoded.blob_portion.len(), "blob_hashes": hashes,
                "blobs": blobs.iter().map(|blob| format!("0x{}",hex::encode(blob.as_bytes()))).collect::<Vec<_>>(),
                "commitments": commitments.iter().map(|bytes| format!("0x{}",hex::encode(bytes))).collect::<Vec<_>>(),
                "calldata":format!("0x{}",hex::encode(input))})
        }).collect::<Vec<_>>();
    println!("{}", serde_json::to_string(&fixtures).unwrap());
}
