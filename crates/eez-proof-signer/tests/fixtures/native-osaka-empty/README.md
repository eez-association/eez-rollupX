# Native Osaka empty-block witnesses

Unmodified blocks 2–4 and their exact execution witnesses from the source-based
`native_system::two_composers_two_keyless_followers` run on 2026-09-28 with the
mandatory genesis schedule, authenticated L1 seed and native execution profile.
Each empty block includes its own EIP-2935 history write. The older operations
envelope in that capturing run does not affect these exact block-only messages;
these files make no claim about the old DA format.

`blocks.protobuf` contains three length-delimited `ProveChunk::Block` messages.
`block.rlp` and `witness.json` are the exact first block and a lossless JSON
projection of its witness. `oracle.json` records header values and file SHA-256
hashes. `l2-genesis.json` is the exact runtime/signer input, including its timestamp
and required `rollupX` schedule. A bare ChainConfig cannot replace that genesis.

Source base: PR #42 `c8260cec8e76a267c96cea1313d31b55a8383f75`; protocol
`9d13218a355e7d543ca8a9927eea2962600b716d`. The source/executable inventory and
process log are local-only at
`artifacts/native-system-backport/e2e/l1-auth-phase-four-node-1-executed-identity.json`
and `artifacts/native-system-backport/logs/l1-auth-phase-four-node-1.log`.
This is a stateful-producer/stateless-replay oracle, not an independently
implemented EVM. See [the implementation audit](../../../../../docs/audits/2026-09-28-native-system-backport/README.md).
