use alloy_primitives::Bytes;
use eez_blob_codec::{FIELD_ELEMENT_BYTES, Message, encode, pack};

/// Confirms that the protocol's least-significant-byte-first stream assignment
/// is serialized as valid big-endian Ethereum KZG field elements.
#[test]
fn pinned_eez_layout_produces_a_valid_ethereum_blob() {
    let mut operations = vec![0_u8; 32];
    operations[20] = 0xff;
    let encoded = encode(&[
        Message::ChainOperation {
            chain_id: 7,
            operations: Bytes::from(operations),
        },
        Message::CloseBlobStream,
    ])
    .unwrap();
    let packed = pack(&encoded.blob_portion).unwrap();

    // Ethereum's serialized field elements are big-endian. EEZ leaves the
    // most-significant byte at raw offset zero unused and stores its first
    // stream byte in the least-significant byte at raw offset 31.
    assert_eq!(packed[0].as_bytes()[0], 0);
    assert_eq!(packed[0].as_bytes()[FIELD_ELEMENT_BYTES - 1], 0); // version byte

    let ethereum_blob = c_kzg::Blob::from_bytes(packed[0].as_ref()).unwrap();
    c_kzg::ethereum_kzg_settings(0)
        .blob_to_kzg_commitment(&ethereum_blob)
        .expect("EEZ packing must produce canonical Ethereum field elements");
}
