# Native offline changing-static fixture

Complete cold v2 request captured from
`cross_chain::outbound_changing_static_reads_and_write_settle_and_replay` with
`EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`, using the fresh offline-history genesis.
The process test settled an outbound write interleaved with position-bound static
reads, and a fresh follower independently replayed the accepted result.

`request.v2.protobuf` contains the exact `Begin -> Block* -> Finalize` transcript,
including every execution witness and L1 bridge header. `response.json` pins the
request Keccak, byte count, native signature and public-input digest. Deployment
identities and complete local genesis documents are public test inputs. The
signer used no L1/L2 RPC or prior preparation cache.

The [implementation audit](../../../../../docs/audits/2026-10-27-group-cleanup-and-history/README.md)
records source identity, commands and limits. This capture uses the fresh-network
Profile with cross-chain 0x45/version-1 transactions and optional metered
history 0x46/version-1 updates. Only cross-chain transactions carry a sourceHash,
bound to the original mutable settlement row and raw pre-group prefix;
complete groups clear protocol residue before checkpoints. Ordinary fees reach
FeeVault and native execution does not change the system-account nonce.

This capture includes the fixed FeeVault predeploy, its initial owner/recipient,
the single `Profile` commitment and validation-contract version 2. The sole
prover service remains `prove.v2.Prover`; its protobuf messages are unchanged.
