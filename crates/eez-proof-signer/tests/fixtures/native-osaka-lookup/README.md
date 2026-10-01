# Direct V0 caught-revert native capture

Unmodified successful Prove request/response from the first caught revert in
`cross_chain::inbound_reverting_destination_preserves_caught_result_without_destination_writes`,
re-captured on 2026-09-30 after L1-origin semantic transactions stopped publishing
their signed L1 transaction: the bracket's `tx_data` is now empty, and the signer
rejects any other value. The source process test (now the `cross_chain` module of
the `node_e2e` target) passed in 130.56s: three caught reverts retained their
original outcomes without destination writes, followed by a successful inbound
call. This request covers blocks 1–115 and one failed lookup entry. The terminal
block has no applying transaction; its B0 includes the normal pre-execution state
updates. The captured replay test asserts that only L2-origin brackets carry
`tx_data`.

`response.json` binds request sizes and digests. The cold request expands original
preparation references with exact captured witnesses. Full signer replay compares
the original public input and signature. These are producer/stateless-replay
oracles, not an independent EVM implementation.

Source: `feat/system-tx-remaining-gas-cap` `637c445` plus the commit adding this
capture; protocol `9d13218a355e7d543ca8a9927eea2962600b716d`. `capture.json` records
the executable and file digests. The run, its logs and all four exported requests
were local-only and are not retained. The previous capture (base
`1d3ee4530205c259a54b288fd940e7b65436177d`, under the protocol balance burn)
published a 246-byte signed L1 transaction in the inbound bracket and is superseded;
the signer now rejects it.
