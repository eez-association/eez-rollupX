# Direct V0 changing-static native capture

Exact successful Prove request/response exported by
`cross_chain::outbound_changing_static_reads_and_write_settle_and_replay`,
re-captured on 2026-10-01 after `EEZL2` moved to the upgradeable `EEZProxy`
genesis predeploy `0x4B2760F84B83d6b3829E30A7573B0172B6d6402C` (protocol
`855fe06`). The source-based test passed in 120.21s, including semantic state
assertions and independent follower reconstruction from authenticated L1/blob DA.
It covers one outbound native load plus its signed user transaction: mutable
write and four static reads, with positions/results retained in the native
semantic stream. No raw sidecar array or full terminal block is published in V0.
The run exported one request, covering blocks 1–115.

The files are unmodified captures. `response.json` binds both request sizes and
Keccak digests; `cold-request.protobuf` expands only the original captured cache
references with exact preparation witnesses. The signer tests execute that full
window and compare the resulting public input and signature with the original.
The exact L1/L2 genesis and public test identities are included. This is a
stateful-producer/stateless-replay oracle, not an independent EVM implementation.

Source: `feat/protocol-main-8d45891` `a01b34c` (the signer's own test-vector edits
were uncommitted and test-only); pinned protocol
`855fe0602484750861b4c31502a1a91355d33ed6`. `capture.json` records the executable
and file digests. The run and logs are local-only and not retained. Capture
command, from the fork root after `forge build --root contracts` and with
`EEZ_TEST_PREBUILT`, `EEZ_TEST_RUNTIME_BIN_DIR` and `EEZ_TEST_PROOF_SIGNER_BIN`
unset:

```sh
EEZ_TEST_PROVE_FIXTURE_DIR=<dir> EEZ_TEST_LOG_DIR=<logs> EEZ_TEST_DATADIR_DIR=<datadirs> \
  cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  cross_chain::outbound_changing_static_reads_and_write_settle_and_replay \
  --exact --test-threads=1
```

`request.protobuf`, `cold-request.protobuf` and `response.json` are the exporter's
`<hash>.protobuf`, `<hash>.cold.protobuf` and `<hash>.json`; the genesis,
chain-config and identity files are its unmodified copies.

History:

- 2026-09-30: captured at `feat/zero-system-balances`
  `1d3ee4530205c259a54b288fd940e7b65436177d`, protocol
  `9d13218a355e7d543ca8a9927eea2962600b716d`, under the block-start protocol
  balance burn (test passed in 89.87s; local-only evidence
  `artifacts/protocol-balance-burn/capture/changing-static/`). Its witnesses
  prove the old `0x42…07` manager and are superseded by the 2026-10-01
  re-capture for the upgradeable EEZL2 proxy at protocol `855fe06`.
- The capture before that (base PR #42 `c8260ce`) predates the burn and is
  superseded; see the [proposal](../../../../../docs/proposals/protocol-balance-burn.md).
