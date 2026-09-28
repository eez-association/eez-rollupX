# Direct V0 changing-static native capture

Exact successful Prove request/response exported by
`cross_chain::outbound_changing_static_reads_and_write_settle_and_replay` on
2026-09-28. The source-based test passed in 95.37s, including semantic state
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

Source base: PR #42 `c8260cec8e76a267c96cea1313d31b55a8383f75`; pinned protocol
`9d13218a355e7d543ca8a9927eea2962600b716d`. Local-only complete source/executable
inventory and log:
`artifacts/native-system-backport/e2e/direct-v0-cross-chain-2-executed-identity.json`
and `artifacts/native-system-backport/logs/direct-v0-cross-chain-2.log`.
See [the implementation audit](../../../../../docs/audits/2026-09-28-native-system-backport/README.md).
