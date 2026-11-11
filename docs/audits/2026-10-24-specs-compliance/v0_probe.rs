use eez_payload_codec::v0::{BlockRun, Error, Payload, encode};
fn main() {
    let text = "00060000000000000600000000000000000000000000000000000000000600";
    let draft: Vec<u8> = (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i+2], 16).unwrap()).collect();
    assert_eq!(draft.len(), 31);
    assert!(matches!(Payload::decode(&draft), Err(Error::Counts)));
    let local = encode(&[BlockRun { blocks: 6, beneficiary: Default::default(), extra_data: &[], transactions: &[] }]).unwrap();
    assert_eq!(local.len(), 26);
    assert_eq!(Payload::decode(&local).unwrap().block_count(), 6);
    println!("draft_vector_1: 31 bytes -> Counts (rejected)");
    println!("rollupx_equivalent: 26 bytes -> 6 blocks (accepted)");
    println!("local_hex: {}", local.iter().map(|b| format!("{b:02x}")).collect::<String>());
}
