# Native offline caught-revert fixture

Captured from
`cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes`
with `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`. This is the first successful request,
starting at L2 block 1 and including one caught, failed destination observation. The full
native process test also checked subsequent observations and follower replay.

`request.v2.protobuf` contains the complete cold request, execution witnesses and
L1 ancestry. `response.json` pins its bytes/Keccak, native digest and signature.
The local genesis documents and public deployment identities are sufficient to
repeat validation without either chain's RPC or a preparation cache.

The [implementation audit](../../../../../docs/audits/2026-10-27-group-cleanup-and-history/README.md)
records commands, source identity and validation limits. This capture uses the
fresh-network Profile with cross-chain 0x45/version-1 transactions and optional metered
history 0x46/version-1 updates. Only cross-chain transactions carry a sourceHash,
bound to the original mutable settlement row and raw pre-group prefix;
complete groups clear protocol residue before checkpoints. Ordinary fees reach
FeeVault and native execution does not change the system-account nonce.

This capture includes the fixed FeeVault predeploy, its initial owner/recipient,
the single `Profile` commitment and validation-contract version 2. The sole
prover service remains `prove.v2.Prover`; its protobuf messages are unchanged.
