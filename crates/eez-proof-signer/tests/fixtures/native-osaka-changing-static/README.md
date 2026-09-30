# Direct V0 changing-static native capture

Exact successful Prove request/response exported by
`cross_chain::outbound_changing_static_reads_and_write_settle_and_replay`,
re-captured on 2026-09-30 under the block-start protocol balance burn. The
source-based test passed in 89.87s, including semantic state
assertions and independent follower reconstruction from authenticated L1/blob DA.
It covers one outbound native load plus its signed user transaction: mutable
write and four static reads, with positions/results retained in the native
semantic stream. No raw sidecar array or full terminal block is published in V0.

The files are unmodified captures. `response.json` binds both request sizes and
Keccak digests; `cold-request.protobuf` expands only the original captured cache
references with exact preparation witnesses. The signer tests execute that full
window and compare the resulting public input and signature with the original.
The exact L1/L2 genesis and public test identities are included. This is a
stateful-producer/stateless-replay oracle, not an independent EVM implementation.

Source base: `feat/zero-system-balances` `1d3ee4530205c259a54b288fd940e7b65436177d`;
pinned protocol `9d13218a355e7d543ca8a9927eea2962600b716d`. `capture.json` records
the executable and file digests. Local-only run and logs:
`artifacts/protocol-balance-burn/capture/changing-static/`. The previous capture
(base PR #42 `c8260ce`) predates the burn and is superseded; see the
[proposal](../../../../../docs/proposals/protocol-balance-burn.md).
