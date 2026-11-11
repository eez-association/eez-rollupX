//! Virtual Composer discovery ABI. Only Composer HTTP fronts implement it;
//! this is neither deployed bytecode nor an EVM precompile.

use alloy_primitives::{Address, B256, U256, address, b256};
use alloy_sol_types::{SolCall, sol};

/// Last 20 bytes of `keccak256("eez.composer.discovery.v1")`.
pub const ADDRESS: Address = address!("7Ae2c80116976915a0Ee9b7994e7Bb12026087f8");

/// `keccak256("EEZ_COMPOSER_DISCOVERY")`, identifying a discovery response.
pub const MARKER: B256 = b256!("f331ada39582afc2fae4082b2baeb82639a90399d9d6688eda56cc24dd03e734");

/// Wire schema version, independent of the Composer package version.
pub const SCHEMA_VERSION: u64 = 1;

sol! {
    /// Current-runtime discovery, served via `eth_call` with the latest block tag.
    function composerInfo(bytes32 nonce) external view returns (
        bytes32 marker,
        uint256 schemaVersion,
        bytes32 echoedNonce,
        uint256 sourceChainId,
        bytes info
    );
}

/// The four-byte `composerInfo(bytes32)` selector (`0x98da5085`).
pub const SELECTOR: [u8; 4] = composerInfoCall::SELECTOR;

/// Encode the discovery return tuple, with `info` supplied as serialized JSON.
pub fn encode_response(nonce: B256, source_chain_id: u64, info: &[u8]) -> Vec<u8> {
    composerInfoCall::abi_encode_returns(&composerInfoReturn {
        marker: MARKER,
        schemaVersion: U256::from(SCHEMA_VERSION),
        echoedNonce: nonce,
        sourceChainId: U256::from(source_chain_id),
        info: info.to_vec().into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::keccak256;

    #[test]
    fn discovery_constants_and_abi() {
        assert_eq!(
            ADDRESS.as_slice(),
            &keccak256("eez.composer.discovery.v1")[12..]
        );
        assert_eq!(MARKER, keccak256("EEZ_COMPOSER_DISCOVERY"));
        assert_eq!(SELECTOR, [0x98, 0xda, 0x50, 0x85]);
        assert_ne!(ADDRESS, crate::EEZL2_ADDRESS);
        let nonce = B256::repeat_byte(0xa5);
        let info = br#"{"version":"test"}"#;
        let encoded = encode_response(nonce, 10201, info);
        // Five head words, a length word, and padded JSON, with the dynamic
        // bytes offset measured from the start of the return tuple.
        assert_eq!(&encoded[..32], MARKER.as_slice());
        assert_eq!(&encoded[64..96], nonce.as_slice());
        assert_eq!(U256::from_be_slice(&encoded[128..160]), U256::from(160));
        let decoded = composerInfoCall::abi_decode_returns_validate(&encoded).unwrap();
        assert_eq!(decoded.schemaVersion, U256::from(1));
        assert_eq!(decoded.sourceChainId, U256::from(10201));
        assert_eq!(decoded.info.as_ref(), info);
    }
}
