# Direct V0 caught-revert native capture

Unmodified successful Prove request/response from the first caught revert in
`cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes`,
re-captured on 2026-09-30 under the block-start protocol balance burn. The source
process test passed in 114.90s: three caught reverts retained their original
outcomes without destination writes, followed by a successful inbound call. This
request covers blocks 1–100 and one failed lookup entry. The terminal block has no applying transaction; its B0 includes the normal
pre-execution state updates.

`response.json` binds request sizes and digests. The cold request expands original
preparation references with exact captured witnesses. Full signer replay compares
the original public input and signature. These are producer/stateless-replay
oracles, not an independent EVM implementation.

Source base: `feat/zero-system-balances` `1d3ee4530205c259a54b288fd940e7b65436177d`;
protocol `9d13218a355e7d543ca8a9927eea2962600b716d`. `capture.json` records the
executable and file digests. Local-only run, logs and all four exported requests:
`artifacts/protocol-balance-burn/capture/lookup/`. The previous capture (base PR #42
`c8260ce`) predates the burn and is superseded; see the
[proposal](../../../../../docs/proposals/protocol-balance-burn.md).
