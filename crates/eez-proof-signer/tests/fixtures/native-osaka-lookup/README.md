# Native offline caught-revert fixture

Captured from
`cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes`
with `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`. This is the first successful request,
starting at L2 block 1 and including one caught, failed destination observation. The full
native process test also checked subsequent observations and follower replay.

`request.v2.protobuf` contains the complete cold request, execution witnesses and
L1 ancestry. `response.json` pins its bytes/Keccak, expected digest and signature.
The local genesis documents and public deployment identities are sufficient to
repeat validation without either chain's RPC or a preparation cache.

The [implementation audit](../../../../../docs/audits/2026-10-27-upgradeable-l1-history/README.md)
records commands, source identity and validation limits. This capture uses the
fresh-network Profile with versionless cross-chain 0x45 transactions and optional metered
history 0x46/version-1 updates. Only cross-chain transactions carry a sourceHash,
bound to the L2 EVM chain ID, intended L1 parent and zero-based complete-group
ordinal in the full Live Sync block;
complete groups clear protocol residue before checkpoints. Ordinary fees reach
FeeVault and native execution does not change the system-account nonce.

This capture includes the fixed FeeVault predeploy, its initial owner/recipient,
the single `Profile` commitment and the original validation-contract version 6. The sole
prover service remains `prove.v2.Prover`; its protobuf messages are unchanged.

The history predeploy is an initially empty EEZProxy with a separate ProxyAdmin.
Native history updates carry explicit `(number, blockHash)` pairs. The public
store remains zero until its first system import.

## Core Rollup proof-policy adaptation

The recorded L1/L2 genesis, block headers and transaction bytes are retained
unchanged. The terminal witness was supplemented as described below. `identity.json` now records the version-7 network-bound
verification key; `response.json` contains the digest and ECDSA signature
recomputed from those inputs using Core's `(timestamp, blockHash)` context.
These updated signatures are derived fixture expectations, not new deployed-network
captures. The signer replay tests must accept them after complete independent
execution; regeneration alone does not establish that validation.

Regenerate only through the explicit
`service::tests::derivation::captured::regenerate_native_fixture_commitments`
utility. It also emits local Core-verifier oracle inputs under
`artifacts/rollup-core-fixture-oracle/` for an independent Solidity comparison.

## Manager replay witness supplementation

Validation-contract version 8 replays caught mutable observations through the
real EEZL2 manager, including callback-table validation. The original capture
predates those additional reads. Its terminal witness lacked storage exclusion
proofs needed by the manager; missing evidence must remain a rejection.

The explicit test utility
`service::tests::derivation::captured::supplement_native_lookup_capture_witness`
reconstructs all 45 captured blocks from the published genesis allocation. It
checks contiguous ancestry, every pre-state root and exact reconstructed block
RLP before replacing only the terminal witness's trie nodes and bytecodes.
Transactions, headers, settlement bytes and expected proof remain unchanged by
this supplementation. This is derived test evidence, not a new network capture.

The request grew from 473,911 to 495,704 bytes. Its Keccak changed from
`0x328c9139930669372be9820acc85b2140d04576ee6d8cdf0b8b5afc78489328f` to
`0x73895e280887b30e3bd6d0066f3573f8a9ae7da25aefc228f6c492e56063df80`.
Run the ignored utility explicitly:

```bash
cargo +1.95.0 test --locked -p eez-proof-signer -- --ignored --exact service::tests::derivation::captured::supplement_native_lookup_capture_witness
```

Then regenerate dependent streaming vectors using the
[conformance procedure](../../../../eez-control-rpc/conformance/README.md).

Positive service replay must still reproduce the expected proof. Adversarial
service tests remove the manager implementation bytecode or forge matching
claim-side revert outputs; both must reach `observation_replay` and reject with
zero signing calls. Merely regenerating this fixture does not validate those
properties or establish full protocol E2E coverage.
