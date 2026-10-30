# Native offline changing-static fixture

Complete cold v2 request captured from
`cross_chain::outbound_changing_static_reads_and_write_settle_and_replay` with
`EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`, using the fresh offline-history genesis.
The process test settled an outbound write interleaved with position-bound static
reads, and a fresh follower independently replayed the accepted result.

`request.v2.protobuf` contains the exact `Begin -> Block* -> Finalize` transcript,
including every execution witness and L1 bridge header. `response.json` pins the
request Keccak, byte count, expected signature and public-input digest. Deployment
identities and complete local genesis documents are public test inputs. The
signer used no L1/L2 RPC or prior preparation cache.

The [implementation audit](../../../../../docs/audits/2026-10-27-upgradeable-l1-history/README.md)
records source identity, commands and limits. This capture uses the fresh-network
Profile with versionless cross-chain 0x45 transactions and optional metered
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

The recorded request, execution witnesses, L1/L2 genesis and transaction bytes
are retained unchanged. `identity.json` now records the version-8 network-bound
verification key; `response.json` contains the digest and ECDSA signature
recomputed from those inputs using Core's `(timestamp, blockHash)` context.
These updated signatures are derived fixture expectations, not new deployed-network
captures. The signer replay tests must accept them after complete independent
execution; regeneration alone does not establish that validation.

Regenerate only through the explicit
`service::tests::derivation::captured::regenerate_native_fixture_commitments`
utility. It also emits local Core-verifier oracle inputs under
`artifacts/rollup-core-fixture-oracle/` for an independent Solidity comparison.
