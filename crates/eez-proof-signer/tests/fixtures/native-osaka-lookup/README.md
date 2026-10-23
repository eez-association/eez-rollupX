# Native offline caught-revert fixture

Captured from
`cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes`
with `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`. This is the first successful request,
covering L2 blocks 1–45 and one caught, failed destination observation. The full
native process test also checked subsequent observations and follower replay.

`request.v2.protobuf` contains the complete cold request, execution witnesses and
L1 ancestry. `response.json` pins its bytes/Keccak, native digest and signature.
The local genesis documents and public deployment identities are sufficient to
repeat validation without either chain's RPC or a preparation cache.

The implementation audit records commands, source identity and validation limits.
Old preparation/expanded-v1 artifacts are obsolete and are removed after their
remaining fixture consumers migrate to this transcript.
