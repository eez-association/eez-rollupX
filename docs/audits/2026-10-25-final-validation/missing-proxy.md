# Missing static-callback proxy validation

| Field | Value |
| --- | --- |
| Date and status | 2026-10-25; static analysis and stronger process assertions prepared; execution pending |
| Scope | L1-origin and L2-origin `topLevelStaticReentrantMissingProxy` rejection, with admission and terminal composition outcomes distinguished |
| Source | Preparation started at `1109dbab7f12fc735a6bf6986e7d7e7f987495d7`, plus the test-only changes in `crates/eez-node-e2e/tests/cross_chain.rs` |
| Protocol | Clean initialized `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Related record | [Earlier live fee-admission results](../2026-10-24-extended-validation/kurtosis/missing-proxy.json) |

## Required behavior

The pinned [core execution contract](../../../eez-core-protocol/docs/EXECUTION_ENTRY_SPEC.md)
requires a static callback's source proxy to exist before the read. A static
frame cannot create it. The
[static-entry specification](../../../eez-core-protocol/docs/STATIC_ENTRY.md)
also explains why an empty-code address must be rejected: a plain `STATICCALL`
to that address could otherwise appear to succeed without executing the callback.

Two supported front configurations enforce this invariant at different stages:

| Configuration | Expected outcome | Required evidence |
| --- | --- | --- |
| Fee collection enabled | Signed fee simulation rejects before queue insertion | RPC `-32003`/`quote_unavailable`, the exact missing proxy and chain in executor evidence, no ownership/receipt, unchanged source state |
| Fee collection disabled | Front accepts the envelope; composition records a deterministic terminal rejection | Matching source hash, `lifecycle: terminal`, `ownership.state: terminal`, `SimulationFailed`, exact executor error, no canonical completion or source receipt |

The distinction follows `intercept_send_raw` in
[`ingress.rs`](../../../crates/eez-node/src/ingress.rs): `check_signed` runs before
`gate_and_hold` when tariff enforcement is enabled. The
[queue contract](../../proposals/cross-chain-queue-liveness.md#simulation-rejection-diagnostics)
requires deterministic composition failures to retain a bounded terminal
diagnostic. No runtime change or relaxation of simulation/proof validation is
required to test the second path.

The pinned staged runner requires the second outcome. Its ordinary sending path
also requires a successful fee quote, so directly invoking the complete staged
runner cannot reach its terminal oracle with either fixed MissingProxy trigger.
The retained earlier failure and earlier fee-enabled admission passes remain
valid evidence of those respective paths; neither establishes terminal eviction.

## Test preparation

The existing two process tests now check the full error, including the exact
proxy and source chain, through the matching source-chain front. They additionally
require the matching transaction hash, both terminal state fields, no canonical
completion, no direct/front receipt, unchanged sender nonce/balance, no proxy
deployment and no reader write. The previous rejection assertions remain.

For live terminal validation, use a fresh owned enclave with both supported fee
collection flags explicitly disabled. The unmodified pinned
`network-scenario.sh` preparation stage deploys each upstream fixture and writes
its signed trigger without sending it. Submit that exact prepared trigger through
the normal front and run the pinned `_check_missing_proxy_rejection` function
without modifying its body. Recheck state and absence of receipts after two more
source blocks.

Task-local orchestration is retained under `artifacts/missing-proxy-20261025/`.
It closes the concrete gap between the quote-requiring staged sender and its
terminal negative oracle; it adds no production ingress path or persistent test
framework. These ignored artifacts are local-only. The checked-in result will
record exact runtime identity, source, executable digests and commands after
execution. No live pass is claimed by this preparation record.

## Checks executed so far

- `rustup run 1.95.0 rustfmt --check --edition 2024 crates/eez-node-e2e/tests/cross_chain.rs`: pass.
- `bash -n artifacts/missing-proxy-20261025/run-terminal.sh`: pass.
- `python3 -m py_compile artifacts/missing-proxy-20261025/observe-terminal.py`: pass.
- `git diff --check`: pass before adding this record; repeated at closure.

The extracted pinned terminal-oracle body has SHA-256
`6277082fbf5483422199a2bdea275c6297384f78216b19fd88759f3557362e5f`.
Parser checks used a retained public fixture envelope and made no RPC calls.

## Pending execution

The stronger process assertions and isolated live terminal cases have not yet
run. Resource scheduling is shared with the admission investigation and the
comparison workload; concurrent builds or networks must not contaminate that
comparison. Any new runtime correction requires updated executable identity
before these results can be attributed to it.
