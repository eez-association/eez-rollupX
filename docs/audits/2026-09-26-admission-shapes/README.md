# Admission causes and bounded multi-call gas estimates

| Field | Value |
| --- | --- |
| Status | Implementation and focused checks in progress; runtime rollout pending |
| Authorization | User selected explicit Grafana selection reasons and improved nested/multi-call estimation; earlier authorization includes local testing, image build, deployment and measurement |
| Branch / baseline | `feat/cross-chain-admission-throughput`; `a1f6a5dfad9c8525ed41f6e2bbe75d6f64aa804e` (running runtime `b352124`) |
| Protocol | Clean `9d13218a355e7d543ca8a9927eea2962600b716d`; no pin, ABI, signer-profile or genesis changes |
| Tools | Rust 1.94.0, locked Cargo dependencies, Foundry 1.7.1 / Solidity 0.8.34 |
| Contract | [Proposal](../../proposals/cross-chain-admission-throughput.md), [settlement policy](../../configurable-settlement-policy.md), [monitoring](../../../monitoring/indexer/README.md#admission-history) |
| Prior evidence | [870-source measured wave](../2026-09-26-admission-throughput/README.md) |

## Findings and implementation

Seven of the preceding wave's eight blank admission reasons concealed count
caps: four FIFO fairness limits and three submission backoffs. The remaining
batch exhausted its snapshot. New events preserve the effective cap and its
origin, and report a resource cut, reached count cap, snapshot exhaustion or
eligible-source exhaustion. Selection and retry behavior is unchanged. The SQL
view separates admission's stop reason from later proof/preflight failure;
historical missing causes remain unknown.

The engineer's supplied gas breakdown separates posting/storage from later call
execution. Its exact benchmark/revision/storage/proof context was not supplied,
so those numbers are profiling guidance, not substituted constants. Our native
ECDSA batch family also includes a leading root anchor and proof verification.
The existing 370,000 entry reserve includes proxy creation. Registered proxies
can avoid that allowance for a bounded successful multi-call/reentrant family.

The new immediate-entry reserve is 70,000 + 40,000 per additional L1 call +
35,000 per nested result row. Target execution and calldata are charged
separately. Examples: two top-level L1 calls reserve 110,000; one L1 call with
one L2 reentry and one L1 callback reserves 145,000. The model permits up to
four calls/four rows, 256-byte input/64-byte output, and at most one L2 reentry
layer. Every L1 source proxy must be registered at the exact canonical parent.
New proxies, deeper recursion, value, static/reverted execution and unmeasured
shapes retain the previous reserve. Deferred storage retains its existing
independent model. The 128-total-entry boundary and final native effect/gas
validation remain unchanged.

Solidity fixtures run real nested proxy resolutions and compare complete root
advancement, not merely outer receipt success. They measure caller-frame gas
before refunds with cold protocol/proxy accounts, 64 blob references, and
1/10/64/127 additional entries. Rust tests bind shape evidence through root
stitching and final sizing, reject unsupported boundaries, and preserve
per-occurrence evidence.

## Validation history

Large logs/datadirs are local-only under ignored `artifacts/admission-shapes/`.
They are unavailable to another checkout. Final sanitized results will be added
after the remaining process and live checks.

| Check | Evidence | Result |
| --- | --- | --- |
| Original pinned gas fixture | Embedded EVM | 26 passed at baseline (`gas-baseline.log`) |
| Multi-call/nested boundary ladders | Embedded EVM | 8 passed with maximum data/result sizes (`gas-complex-max-data.log`) |
| Composer unit suite | Unit | 245 passed, 1 existing manual profile ignored (`composer-third.log`); subsequent boundary additions require final rerun |
| New/legacy history and every dashboard query | Disposable PostgreSQL | 2 passed, zero skipped (`indexer-admission.log`); subsequent malformed-cap check requires final rerun |
| Workspace Clippy, all targets | Static | Passed with `-D warnings` (`workspace-clippy.log`) |
| Indexer Clippy, all targets | Static | Passed (`indexer-clippy.log`) |
| Native multi-call/follower regression | Process | In progress |

First fixture failures are retained: the initial nested Solidity helper exceeded
the compiler's stack allocation; splitting construction fixed it. Combining all
large ladders in one test exceeded Foundry's test-transaction gas budget; splitting
the cases preserved all measurements. The first native run could not bind local
RPC ports in the sandbox and reached no node behavior; the local-server rerun is
tracked separately. These are not reported as passing correctness checks.
The first local-server run settled and independently replayed its first source,
then failed its telemetry assertion because the harness's default stdout filter
excluded `eez_posting`. The harness now includes that stable event target; the
corrected process test must pass before rollout.

## Operations

The existing node/signer remain on `b352124` while validation runs. The owned
disposable database `eez-admission-shapes-db` uses port 19433 and database
`eez_indexer_test`; production remains the separate `eez-monitoring-indexer-db-1`
on port 19432. Tested additive admission views were applied to production in one
transaction so the mounted Grafana dashboard can query the new columns. All
history/checkpoints were retained. Runtime rollback must keep the new indexer
views/binary: an old startup view definition cannot remove appended columns.

The operator's existing Chiado runbook edits and September 25 recovery directory
are outside this change and preserved.
