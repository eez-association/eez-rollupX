//! Regenerate the empty posting-call RPC fixture using the protocol's ABI encoder.
fn main() {
    let encoded = eez_protocol::entries::encode_postbatch(&eez_protocol::EvmBatch::default());
    println!("0x{}", alloy_primitives::hex::encode(encoded));
}
