//! The assumed proof result is supported only for this fork's pinned code.
//! Fingerprints exclude Solidity metadata and zero constructor immutables.
//! Source: protocol 855fe060, contracts/foundry.toml (0.8.34, via IR, runs=1).
use super::{
    Address, B256, EstimateError, GasEstimator, Snapshot, StateProvider, unavailable, unsupported,
};
use alloy_primitives::{b256, keccak256};

const IMPLEMENTATION_SLOT: B256 =
    b256!("360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc");

struct Code {
    length: usize,
    immutables: &'static [usize],
    hashes: &'static [B256],
}
const PROXY: Code = Code {
    length: 699,
    immutables: &[7],
    hashes: &[b256!(
        "2711c709cbaba3ef6ca5c08eb27c4f009b3479c1489e1098e3928ec7b924c6a4"
    )],
};
const REGISTRY: Code = Code {
    length: 23804,
    immutables: &[2282, 3071, 10421],
    // fa916b1 and ab6e44c builds differ only in the metadata of the embedded
    // EEZProxy initcode. Keep both exact identities: stripping embedded metadata
    // would hide CREATE2 address changes, unlike stripping the outer trailer.
    hashes: &[
        b256!("2e00d1d0ae0f677f991fd8ae9a4e06836471680f123920c785efe60b2d848568"),
        b256!("d5f9df9a9ab289a685fc20f5ed5ce4f74871d918cde0354a4c3bd514ffc034c2"),
    ],
};
const ROLLUP: Code = Code {
    length: 3295,
    immutables: &[1015, 1751, 2266],
    hashes: &[b256!(
        "9c9892a76f48f4bba2350419d252b709ef599d4128c32927980cdc72d9915a0b"
    )],
};
const VERIFIER: Code = Code {
    length: 392,
    immutables: &[176, 319],
    hashes: &[b256!(
        "440eebc16fc5e247953d790c578d9f99155c85263cd7ada2d1cbd66d94291a42"
    )],
};
// contracts/src/EEZPostBatcher.sol at fork ab6e44c, same compiler profile.
const POST_BATCHER: Code = Code {
    length: 4828,
    immutables: &[157, 222, 1031, 3141],
    hashes: &[b256!(
        "b9ede5d6a14473b896fe72b193f500250998ab99c9d3e65ad2c7153df35b61d1"
    )],
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
    for start in expected.immutables {
        normalized[*start..*start + 32].fill(0);
    }
    expected.hashes.contains(&keccak256(normalized))
}

fn check(
    state: &dyn StateProvider,
    address: Address,
    expected: &Code,
) -> Result<(), EstimateError> {
    let code = state
        .account_code(&address)
        .map_err(unavailable)?
        .ok_or_else(|| unsupported("deployment code missing"))?;
    if !matches(&code.original_bytes(), expected) {
        return Err(unsupported(format!(
            "unrecognized estimation deployment at {address}"
        )));
    }
    Ok(())
}

fn proxy(
    state: &dyn StateProvider,
    address: Address,
    expected: &Code,
) -> Result<(), EstimateError> {
    check(state, address, &PROXY)?;
    let implementation = state
        .storage(address, IMPLEMENTATION_SLOT)
        .map_err(unavailable)?
        .unwrap_or_default();
    check(
        state,
        Address::from_word(B256::from(implementation.to_be_bytes::<32>())),
        expected,
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
    proxy(state.as_ref(), e.l1.manager_address(), &REGISTRY)?;
    proxy(state.as_ref(), rollup, &ROLLUP)?;
    if e.post_batch_target != e.l1.manager_address() {
        check(state.as_ref(), e.post_batch_target, &POST_BATCHER)?;
        let code = state
            .account_code(&e.post_batch_target)
            .map_err(unavailable)?
            .ok_or_else(|| unsupported("posting wrapper code missing"))?;
        let bytes = code.original_bytes();
        let registry = e.l1.manager_address().into_word();
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
    check(state.as_ref(), e.verifier, &VERIFIER)
}

#[cfg(test)]
mod tests {
    use super::*;
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
