# Direct V0 caught-revert native capture

Unmodified successful Prove request/response from the first caught revert in
`cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes`,
captured on 2026-09-28. The source process test passed in 110.05s: three caught
reverts retained their original outcomes without destination writes, followed by
a successful inbound call. This request covers blocks 1–95 and one failed lookup
entry. The terminal block has no applying transaction; its B0 includes the normal
pre-execution state updates.

`response.json` binds request sizes and digests. The cold request expands original
preparation references with exact captured witnesses. Full signer replay compares
the original public input and signature. These are producer/stateless-replay
oracles, not an independent EVM implementation.

Source base: PR #42 `c8260cec8e76a267c96cea1313d31b55a8383f75`; protocol
`9d13218a355e7d543ca8a9927eea2962600b716d`. Local-only source/executable inventory:
`artifacts/native-system-backport/e2e/direct-v0-lookup-1-executed-identity.json`.
Logs: `artifacts/native-system-backport/logs/direct-v0-lookup-1.log` and
`direct-v0-lookup-inventory-1.log`.
See [the implementation audit](../../../../../docs/audits/2026-09-28-native-system-backport/README.md).
