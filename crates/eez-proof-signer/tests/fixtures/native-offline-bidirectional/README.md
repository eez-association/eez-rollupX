# Native offline bidirectional settlement fixture

Captured from `cross_chain::minimal_bidirectional_cross_chain_smoke` with
`EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`. The source-built signer ran under the
outbound-network-denying launcher, and the native manager accepted its signature.
The process test independently checked the public input and canonical L2 safe hash.
The captured execution includes recorded native history transactions and their
metered gas with native 0x45/version 1, no system nonce, and authenticated
fee collection under Profile.

`request.v2.protobuf` is the complete length-delimited `prove.v2.ClientFrame`
sequence reconstructed by the test proxy. It contains all L2 witnesses and L1
ancestry. `response.json` fixes the native signature, digest, byte count and request
Keccak. The local genesis documents and public Anvil deployment identities are
required for cold independent replay. No RPC endpoint, operator secret or cache is
part of this fixture.

Source details and the executed command are recorded in
[the implementation audit](../../../../../docs/audits/2026-10-24-pr-readiness/final-fixtures.md).

This capture includes the fixed FeeVault predeploy, its initial owner/recipient,
the single `Profile` commitment and validation-contract version 1. The sole
prover service remains `prove.v2.Prover`; its protobuf messages are unchanged.
