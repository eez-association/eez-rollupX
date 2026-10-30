# Prefix history replay fixtures

The two witnesses come from the same source-based bidirectional capture as
[`native-offline-bidirectional`](../../../../eez-proof-signer/tests/fixtures/native-offline-bidirectional/README.md).
Their L1/L2 genesis documents are identical to that capture. The RPC chain config
additionally materializes the three default blob schedules, as checked by
`replay_spec`. Blocks, witnesses, receipts and original accepted-root evidence
are unchanged.

The deployment fields now use that capture's registry, proof system and public
test attester with its version-8 network-bound verification key. These key fields
are derived expectations for the current Core Rollup integration, not a claim
that the original capture used the current key. The shared profile/key was
independently checked with Solidity ABI encoding and the pinned Core verifier;
the prefix tests must still independently replay both complete witnesses.
