# Native Osaka empty-block witnesses

Unmodified blocks 2–4 and their exact execution witnesses from the source-based
`native_system::two_composers_two_keyless_followers` run, re-captured on
2026-10-01 (passed in 127.51s) after `EEZL2` moved to the upgradeable `EEZProxy`
genesis predeploy `0x4B2760F84B83d6b3829E30A7573B0172B6d6402C` (protocol
`855fe06`), with the mandatory genesis schedule, authenticated L1 seed and native
execution profile. Each empty block includes its own EIP-2935 history write and
the block-start protocol balance burn, whose witness covers `EEZL2` and
`SYSTEM_ADDRESS` even though no transaction touches them. These files are
block-only messages and make no claim about the DA format.

`blocks.protobuf` contains three length-delimited `ProveChunk::Block` messages.
`block.rlp` and `witness.json` are the exact first block and a lossless JSON
projection of its witness. `oracle.json` records header values and file SHA-256
hashes. `l2-genesis.json` is the exact runtime/signer input, including its timestamp
and required `rollupX` schedule. A bare ChainConfig cannot replace that genesis.

Source: `feat/protocol-main-8d45891` `a01b34c` (the signer's own test-vector edits
were uncommitted and test-only); protocol `855fe0602484750861b4c31502a1a91355d33ed6`.
Capture command, from the fork root after `forge build --root contracts` and with
`EEZ_TEST_PREBUILT`, `EEZ_TEST_RUNTIME_BIN_DIR` and `EEZ_TEST_PROOF_SIGNER_BIN`
unset:

```sh
EEZ_TEST_PROVE_FIXTURE_DIR=<dir> EEZ_TEST_LOG_DIR=<logs> EEZ_TEST_DATADIR_DIR=<datadirs> \
  cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  native_system::two_composers_two_keyless_followers --exact --test-threads=1
```

The files were extracted from the exporter's `preparation/<hash>.protobuf` chunks
for heights 2–4 (one candidate per height, linked by parent hash, sealed hashes
recomputed from the header RLP); `blocks.protobuf` concatenates those chunk files
unchanged, and `chain-config.json` / `l2-genesis.json` are the exporter's copies.
The same extraction reproduces the previous committed files byte-for-byte from
the 2026-09-30 capture. The run, logs and executable digests are local-only and
not retained. This is a stateful-producer/stateless-replay oracle, not an
independently implemented EVM.

History:

- 2026-09-30: captured at `feat/zero-system-balances`
  `1d3ee4530205c259a54b288fd940e7b65436177d` with the updated `native_system`
  burn assertions, protocol `9d13218a355e7d543ca8a9927eea2962600b716d` (passed
  in 97.33s; local-only evidence `artifacts/protocol-balance-burn/capture/empty/`
  and `artifacts/protocol-balance-burn/binaries.sha256`). Its witnesses cover the
  old `0x42…07` manager and are superseded by the 2026-10-01 re-capture for the
  upgradeable EEZL2 proxy at protocol `855fe06`.
- The capture before that (base PR #42 `c8260ce`) predates the burn and is
  superseded; see the [proposal](../../../../../docs/proposals/protocol-balance-burn.md).
