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

The [implementation audit](../../../../../docs/audits/2026-10-24-pr-readiness/final-fixtures.md)
records commands, source identity and validation limits. This capture uses the
fresh-network Profile with 0x45/version-1 transactions, authenticated fee
collection and recorded, metered native history imports. Native transactions
have a sourceHash and no nonce.

This capture includes the fixed FeeVault predeploy, its initial owner/recipient,
the single `Profile` commitment and validation-contract version 1. The sole
prover service remains `prove.v2.Prover`; its protobuf messages are unchanged.
