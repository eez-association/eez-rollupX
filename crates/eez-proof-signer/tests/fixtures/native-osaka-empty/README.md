# Native Osaka empty-block witnesses

Unmodified blocks 2–4 and their exact execution witnesses from the source-based
`native_system::two_composers_two_keyless_followers` run on 2026-09-30 (passed in
97.33s) with the mandatory genesis schedule, authenticated L1 seed and native
execution profile. Each empty block includes its own EIP-2935 history write and
the block-start protocol balance burn, whose witness covers `EEZL2` and
`SYSTEM_ADDRESS` even though no transaction touches them. The older operations
envelope in that capturing run does not affect these exact block-only messages;
these files make no claim about the old DA format.

`blocks.protobuf` contains three length-delimited `ProveChunk::Block` messages.
`block.rlp` and `witness.json` are the exact first block and a lossless JSON
projection of its witness. `oracle.json` records header values and file SHA-256
hashes. `l2-genesis.json` is the exact runtime/signer input, including its timestamp
and required `rollupX` schedule. A bare ChainConfig cannot replace that genesis.

Source base: `feat/zero-system-balances` `1d3ee4530205c259a54b288fd940e7b65436177d`
with the updated `native_system` burn assertions; protocol
`9d13218a355e7d543ca8a9927eea2962600b716d`. The files were extracted from the
exporter's `preparation/` chunks; the run, logs and executable digests are
local-only at `artifacts/protocol-balance-burn/capture/empty/` and
`artifacts/protocol-balance-burn/binaries.sha256`. The previous capture (base PR #42
`c8260ce`) predates the burn and is superseded; see the
[proposal](../../../../../docs/proposals/protocol-balance-burn.md).
This is a stateful-producer/stateless-replay oracle, not an independently
implemented EVM.
