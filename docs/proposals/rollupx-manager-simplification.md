# Use the original Core Rollup

Status: accepted for PR #116. This replaces the earlier byte-compatible manager
refactor. The owner selected fresh deployments without old-format support.
Core remains pinned at `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged.

## Design

Deploy Core `Rollup.sol` directly behind its existing proxy, using its original
four-argument initializer. Remove the custom manager, generated profile-code
helper, old runtime fingerprints and manager-upgrade compatibility tests.
Keep Core ownership, proof threshold, registration, root escape and key policy.

Core authenticates the anchor through its original `getCustomData`:

- Actual parent: `abi.encode(uint256(inclusionTimestamp), parentHash)`.
- Historical: `abi.encode(uint256(0), blockhash(B))`, with a nonzero available
  hash and `0 < B < uint64.max`. The Core EVM blockhash window is 256 blocks.

The signer still independently executes every supplied L2 block, validates the
complete L1 header chain, recorded history, delayed PREVRANDAO, native DA and
cross-chain effects, and recomputes the exact Core public-input digest. It has
no L1 or L2 RPC. Historical mode still forbids every cross-chain action and must
end at or before the anchor timestamp. Actual-parent mode retains exact target
binding and live-action timing. v2 protobuf fields remain unchanged.

The registered verification key replaces the former on-chain profile payload.
It commits to the local network profile, registry, proof system and signing
address. The profile commits to validation version 7, L1/L2 identity, genesis,
schedule, history runtime/limits, fee collector and canonical L1 rules.
[The identity specification](../../crates/eez-control-rpc/VALIDATION_IDENTITY.md)
defines the exact preimages. Deployment, Composer and signer derive the same
key; signer startup rejects an arbitrary or mismatched configured key. Core
includes its registered key and rollup ID in the signed accumulator.

## Accepted behavior changes

The manager no longer adds the current accepted L2 root to each proof. Core
checks signed entry roots when consuming entries and enforces immediate-entry
rules. `expectedRootPerRollup` remains an unsigned admission assertion, not an
authentication source. Advancing the root alone does not invalidate a signature.
Reuse and partial consumption remain subject to Core eligibility, complete
signed entries, unchanged DA and the available anchor.

Historical proofs may cross a known compatible L1 rule transition. Each L1
header uses its own fork layout; actual included transactions remain subject
to the containing L1 block's blob limits. Historical signing bounds work by
the largest configured blob allowance because inclusion time is not known.
Actual-parent signing uses the exact target's rules. No proof may omit required
ancestry or change its anchor mode after signing.

An incompatible rule/program upgrade requires coordinated replacement or
revocation of the registered vkey/proof system before incompatible rules become
active. Stopping old signers does not revoke existing signatures. A compatible
known L1 fork does not by itself require a new key.

## Compatibility and acceptance

This is a fresh-deployment change, not a transparent proxy upgrade from the old
custom manager. Deploy matching node, follower, signer and contract artifacts;
discard old sessions. No old manager, context encoder, key derivation or runtime
fingerprint is retained. The L2 transaction formats, history contract, genesis
state layout and database format are unchanged by this change. Followers change
their historical-inclusion policy and must run the matching release.

Required checks are focused Core/ECDSA anchor and key-revocation tests, independent
profile/key/context vectors, signer startup rejection, existing signer/RPC/client
regressions, Composer and follower tests, partial-consumption/source process
coverage, formatting and workspace Clippy. The PR's complete CI matrix is the
release gate. Dated evidence for the superseded refactor remains historical and
does not validate this implementation.
