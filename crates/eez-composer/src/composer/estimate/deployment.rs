//! The assumed proof result is supported only for this fork's pinned code.
//! Fingerprints exclude Solidity metadata and zero constructor immutables.
//! Native execution always uses the original deployed bytes, including metadata.
//! Sources: the pinned core protocol and contracts/src/RollupX.sol, built
//! with contracts/foundry.toml (0.8.34, via IR, runs=1). Only the supported
//! fixed-profile managers can authorize an assumed proof; unknown code cannot.
use super::{
    Address, B256, EstimateError, GasEstimator, Snapshot, StateProvider, unavailable, unsupported,
};
use alloy_primitives::{b256, keccak256};

const IMPLEMENTATION_SLOT: B256 =
    b256!("360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc");

struct Code {
    length: usize,
    immutables: &'static [usize],
    metadata_hashes: &'static [usize],
    hash: B256,
}
const PROXY: Code = Code {
    length: 699,
    immutables: &[7],
    metadata_hashes: &[],
    hash: b256!("2711c709cbaba3ef6ca5c08eb27c4f009b3479c1489e1098e3928ec7b924c6a4"),
};
const REGISTRY: Code = Code {
    length: 23889,
    immutables: &[2282, 3071, 10421],
    // CrossChainProxy initcode embeds an IPFS metadata hash whose source paths
    // vary by build directory. Mask only that 32-byte hash, not its CBOR framing
    // or executable instructions. Real setup still uses the deployed initcode
    // and its actual CREATE2 addresses; recognition never rewrites execution.
    metadata_hashes: &[23750],
    hash: b256!("4002bf893eb71b85237faebb323218a4c9a1e33b77535639241e00feace3b650"),
};
const ROLLUPX: Code = Code {
    length: 6544,
    immutables: &[486, 1351, 2000, 5432],
    metadata_hashes: &[],
    hash: b256!("36f68c4114b00ffa785fe384b273c37d3a02c31a3a80bc0df29c8bde3b69612f"),
};
// The deployed pre-simplification manager has identical context and policy semantics.
// Retain its exact fingerprint so a Composer update does not require a proxy upgrade.
const ROLLUPX_BASELINE: Code = Code {
    length: 6610,
    immutables: &[486, 3547, 4181, 5260],
    metadata_hashes: &[],
    hash: b256!("3b4d3cd772339a817a2641172b70510d71040646dd5620bbdaf5011708f30cab"),
};
// contracts/src/ECDSAProofSystem.sol, used by the Chiado and Kurtosis networks.
const VERIFIER: Code = Code {
    length: 392,
    immutables: &[176, 319],
    metadata_hashes: &[],
    hash: b256!("440eebc16fc5e247953d790c578d9f99155c85263cd7ada2d1cbd66d94291a42"),
};
// The protocol's src/proofSystems/ECDSAProofSystem.sol, deployed on Ethereum
// mainnet by the upstream procedure. Estimation never runs either verifier: it
// answers the registry's single `verify` call (see `l1::AssumedProof`).
const PROTOCOL_VERIFIER: Code = Code {
    length: 1056,
    immutables: &[],
    metadata_hashes: &[],
    hash: b256!("1d1d49bdc9b5871658c0c181abdded35d730b14fb9fdd4b802e32ce792eb02e4"),
};
// contracts/src/EEZPostBatcher.sol at fork ab6e44c, same compiler profile.
const POST_BATCHER: Code = Code {
    length: 4828,
    immutables: &[157, 222, 1031, 3141],
    metadata_hashes: &[],
    hash: b256!("b9ede5d6a14473b896fe72b193f500250998ab99c9d3e65ad2c7153df35b61d1"),
};

fn matches(code: &[u8], expected: &Code) -> bool {
    let Some(suffix) = code.get(code.len().saturating_sub(2)..) else {
        return false;
    };
    let Ok(suffix): Result<[u8; 2], _> = suffix.try_into() else {
        return false;
    };
    let Some(length) = code
        .len()
        .checked_sub(usize::from(u16::from_be_bytes(suffix)) + 2)
    else {
        return false;
    };
    if length != expected.length {
        return false;
    }
    let mut normalized = code[..length].to_vec();
    for start in expected.immutables.iter().chain(expected.metadata_hashes) {
        normalized[*start..*start + 32].fill(0);
    }
    keccak256(normalized) == expected.hash
}

fn check(
    state: &dyn StateProvider,
    address: Address,
    expected: &Code,
) -> Result<(), EstimateError> {
    check_any(state, address, &[expected])
}

fn check_any(
    state: &dyn StateProvider,
    address: Address,
    accepted: &[&Code],
) -> Result<(), EstimateError> {
    let code = state
        .account_code(&address)
        .map_err(unavailable)?
        .ok_or_else(|| unsupported("deployment code missing"))?;
    let code = code.original_bytes();
    if !accepted.iter().any(|expected| matches(&code, expected)) {
        return Err(unsupported(format!(
            "unrecognized estimation deployment at {address}"
        )));
    }
    Ok(())
}

fn proxy(
    state: &dyn StateProvider,
    address: Address,
    accepted: &[&Code],
) -> Result<(), EstimateError> {
    check(state, address, &PROXY)?;
    let implementation = state
        .storage(address, IMPLEMENTATION_SLOT)
        .map_err(unavailable)?
        .unwrap_or_default();
    check_any(
        state,
        Address::from_word(B256::from(implementation.to_be_bytes::<32>())),
        accepted,
    )
}

pub(super) fn validate(
    e: &GasEstimator,
    s: &Snapshot,
    rollup: Address,
) -> Result<(), EstimateError> {
    let state =
        e.l1.chain_provider()
            .provider
            .state_by_block_hash(s.l1.anchor.hash())
            .map_err(unavailable)?;
    validate_at(
        state.as_ref(),
        e.l1.manager_address(),
        rollup,
        e.verifier,
        e.post_batch_target,
    )
}

/// Recognize the supported contracts before a conditional proof result is used
/// in disposable wallet or fee estimation state.
pub(in crate::composer) fn validate_at(
    state: &dyn StateProvider,
    registry: Address,
    rollup: Address,
    verifier: Address,
    post_batch_target: Address,
) -> Result<(), EstimateError> {
    proxy(state, registry, &[&REGISTRY])?;
    proxy(state, rollup, &[&ROLLUPX, &ROLLUPX_BASELINE])?;
    if post_batch_target != registry {
        check(state, post_batch_target, &POST_BATCHER)?;
        let code = state
            .account_code(&post_batch_target)
            .map_err(unavailable)?
            .ok_or_else(|| unsupported("posting wrapper code missing"))?;
        let bytes = code.original_bytes();
        let registry = registry.into_word();
        // The wrapper's constructor embeds its registry in each immutable
        // reference. Check the binding at the same parent as setup execution.
        if POST_BATCHER
            .immutables
            .iter()
            .any(|start| bytes.get(*start..*start + 32) != Some(registry.as_slice()))
        {
            return Err(unsupported("posting wrapper registry differs"));
        }
    }
    check_any(state, verifier, &[&VERIFIER, &PROTOCOL_VERIFIER])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_recognize_both_equivalent_managers_and_reject_changed_execution() {
        // Each fixture is the unpatched runtime from the pinned Solidity 0.8.34,
        // via-IR, optimizer-runs=1 build; proxy upgrades preserve the registry immutable.
        for (fixture, expected) in [
            (include_str!("testdata/rollupx.hex"), &ROLLUPX),
            (
                include_str!("testdata/rollupx-baseline.hex"),
                &ROLLUPX_BASELINE,
            ),
        ] {
            let mut code = alloy_primitives::hex::decode(fixture.trim()).unwrap();
            assert!(matches(&code, expected));
            for start in expected.immutables {
                code[*start..*start + 32].fill(0xa5);
            }
            assert!(matches(&code, expected));
            for offset in [0, expected.immutables[0] - 1, expected.immutables[0] + 32] {
                let mut changed = code.clone();
                changed[offset] ^= 1;
                assert!(!matches(&changed, &ROLLUPX));
                assert!(!matches(&changed, &ROLLUPX_BASELINE));
            }
        }
        let unrelated = alloy_primitives::hex::decode(
            include_str!("testdata/mainnet-protocol-ecdsa-proof-system.hex").trim(),
        )
        .unwrap();
        assert!(!matches(&unrelated, &ROLLUPX));
        assert!(!matches(&unrelated, &ROLLUPX_BASELINE));
    }

    #[test]
    fn fingerprint_metadata_mask_does_not_hide_adjacent_code_changes() {
        let mut canonical = vec![0x5b; 40];
        canonical[4..36].fill(0);
        let expected = Code {
            length: 40,
            immutables: &[],
            metadata_hashes: &[4],
            hash: keccak256(&canonical),
        };
        canonical.extend([0, 0]); // Empty outer metadata trailer.
        assert!(matches(&canonical, &expected));
        canonical[4..36].fill(0xa5);
        assert!(matches(&canonical, &expected));
        for offset in [3, 36] {
            let mut changed = canonical.clone();
            changed[offset] ^= 1;
            assert!(!matches(&changed, &expected));
        }
    }

    /// The verifier deployed on Ethereum mainnet at 0x1Ac760d5…721E.
    #[test]
    fn fingerprint_recognizes_the_protocol_verifier_deployed_on_mainnet() {
        let code = alloy_primitives::hex::decode(
            include_str!("testdata/mainnet-protocol-ecdsa-proof-system.hex").trim(),
        )
        .unwrap();
        assert!(matches(&code, &PROTOCOL_VERIFIER));
        assert!(!matches(&code, &VERIFIER));
    }

    #[test]
    fn fingerprint_rejects_missing_malformed_and_unknown_code() {
        for bytes in [
            vec![],
            vec![0],
            vec![0xff, 0xff],
            vec![0; VERIFIER.length + 2],
        ] {
            assert!(!matches(&bytes, &VERIFIER));
        }
    }
}
