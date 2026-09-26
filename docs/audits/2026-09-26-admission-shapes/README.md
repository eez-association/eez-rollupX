# Admission causes and bounded multi-call gas estimates

| Field | Value |
| --- | --- |
| Status | Implemented, locally validated, deployed and measured; semantic wave and registered-proxy repeat passed |
| Authorization | User selected explicit Grafana selection reasons and improved nested/multi-call estimation; earlier authorization includes local testing, image build, deployment and measurement |
| Branch / baseline | `feat/cross-chain-admission-throughput`; `a1f6a5dfad9c8525ed41f6e2bbe75d6f64aa804e` (baseline runtime `b352124`) |
| Implementation | `036b50a3c7d0dee45e6308388a6233ded7a7d2ca`; subsequent changes are audit/docs and test formatting only |
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
They are unavailable to another checkout. The [gas ladders](gas-ladders.json) and
[local image identities](images.json) retain small sanitized evidence.
[Process selections](process-validation.json), [check hashes](check-evidence.json),
and the live evidence below identify the completed validation.

| Check | Evidence | Result |
| --- | --- | --- |
| Original pinned gas fixture | Embedded EVM | 26 passed at baseline (`gas-baseline.log`) |
| Multi-call/nested boundary ladders | Embedded EVM | 8 passed with maximum data/result sizes (`gas-complex-max-data.log`) |
| Composer unit suite | Unit | 245 passed, 1 existing manual profile ignored (`composer-final.log`) |
| New/legacy history and every dashboard query | Disposable PostgreSQL | 2 passed, zero skipped, including malformed caps (`indexer-admission-final.log`) |
| Workspace Clippy, all targets | Static | Final pass with `-D warnings` (`workspace-clippy-final.log`) |
| Indexer Clippy, all targets | Static | Final pass (`indexer-clippy-final.log`) |
| All integration contracts | Embedded EVM | 53 passed (`contracts-final.log`) |
| Final gas fixture, Cancun / Prague / Osaka | Embedded EVM | 34 passed on each fork (`gas-final-*.log`) |
| Broader native regressions | Process | 39 passed, zero failed/ignored: all dynamic-admission/gas/cost cases plus six settlement-policy and six cross-chain cases ([exact selection](process-validation.json)) |
| Native multi-call/follower regression | Process | 1 selected, passed; canonical settlement, target assertions, follower replay and source telemetry (`native-multicall-instrumented.log`) |
| Live Grafana | Browser / PostgreSQL | 7 panels, 7 queries, zero browser/SQL errors; new fields present against retained history (`grafana-browser-cached.log`); candidate wave also passed with populated values ([browser evidence](grafana-verification.json)) |
| Local release images | Shipping feature selection | Node, signer, indexer and deploy image built; source/protocol labels, executable hashes and existing genesis verified ([identities](images.json)) |

First fixture failures are retained: the initial nested Solidity helper exceeded
the compiler's stack allocation; splitting construction fixed it. Combining all
large ladders in one test exceeded Foundry's test-transaction gas budget; splitting
the cases preserved all measurements. The first native run could not bind local
RPC ports in the sandbox and reached no node behavior; the local-server rerun is
tracked separately. These are not reported as passing correctness checks.
The first local-server run settled and independently replayed its first source,
then failed its telemetry assertion because the harness's default stdout filter
excluded `eez_posting`. The harness now includes that stable event target; the
corrected process test passed. The full Rust formatting check also caught the
added test logger line; formatting is corrected. The integration-root Solidity
formatter was applied to the changed fixture. The repository-wide Solidity check
still reports two unchanged baseline files (`ECDSAProofSystem.sol` and
`DeployRollup.s.sol`); these are excluded, not claimed as formatting passes.

## Operations

The node and signer were updated to `036b50a` at 13:08 UTC ([rollout](runtime-rollout.json));
image-only configuration differences, empty queue, clean stop and preserved
mounts/environment were verified. The owned disposable database
`eez-admission-shapes-db` used port 19433 and database
`eez_indexer_test`; production remains the separate `eez-monitoring-indexer-db-1`
on port 19432. The disposable container was removed after its tests passed.
Tested additive admission views were applied to production in one
transaction so the mounted Grafana dashboard can query the new columns. All
history/checkpoints were retained. The tested indexer image was installed at
12:12 UTC with identical environment, command and mounts; collector health and
the 34 retained candidate histories were verified. Runtime rollback must keep the new indexer
views/binary: an old startup view definition cannot remove appended columns.

The operator's existing Chiado runbook edits and September 25 recovery directory
are outside this change and preserved.

Automatic approval review initially rejected the combined rollout/funding action
because the transfer payload had not been approved explicitly. The image-only
rollout proceeded separately. The user then approved the 0.4 Chiado xDAI transfer;
it confirmed while the poster was stopped ([receipt](funding.json)). No approval
was bypassed. Restart left a 27-block gap with zero execution peers. All prepared
payloads validated through the normal Engine API while Lighthouse retained fork
choice; both heads became current ([recovery](catch-up.json)).

The first workload attempt failed on external RPC HTTP 429 during worker balance
reads, before funding or source submission ([retained failure](initial-setup-failure.json)).
A fresh run used local reads plus a task-owned loopback process forwarding the
identical, locally accepted ordinary L1 signed payloads to the configured upstream,
limited to five requests per second. Cross-chain ingress, 200-way preparation,
28 send workers, four semantic verifiers and the wallet lock were preserved. This
operational workaround is local-only, not a runtime or operator-profile change.
All 2,850 setup transactions mined before the measured wave. The forwarding
process recorded 1,399 identical raw broadcasts, zero upstream rate limits, and
2,339 generic local request/connection exceptions; individual exception causes
were not retained. Those exceptions are not counted as successful calls. The
process was stopped after verification ([counters and limits](setup-forwarding.json)).
The task-only profile points at that stopped port and is not a reusable runbook.
The original operator profile was preserved. Setup RPC failures and setup
duration are excluded from the source window; they are not reported as
a passing first attempt.

## Measured wave

Canonical attribution covers blocks **23,239,159–23,239,184**, including seven
corrected L2 source heights after reconciliation. All **870 sources** (520 inbound,
350 outbound) map to exactly one canonical postBatch. See the
[estimate-versus-receipt table](candidate-admission-postings.csv),
[canonical summary](candidate-summary.json), [coverage](admission-coverage.json)
and [comparison](comparison.json). All **770 job-level semantic checks passed**,
with zero failed jobs ([semantic results](semantic-verification.json)). A later
canonical check found all source attributions unchanged and independently matched
all 25 L1 receipts through the configured external RPC ([stability](canonical-stability.json)).

| Metric | Previous `b352124` | Candidate `036b50a` |
| --- | ---: | ---: |
| Source transactions | 870 | 870 |
| Send + settlement-monitor window | 167.797 s | 147.557 s |
| Sources / second | 5.185 | 5.896 |
| Canonical postings carrying sources | 27 | 25 |
| Receipt gas across those postings | 179,639,982 | 177,949,152 |
| Weighted admission / receipt gas | 1.919× | 1.845× |
| Skipped slots between first/last posting | 4 | 3 |

The observed source window is 12.1% shorter. This is one live wave per revision,
with different slots, retained storage and host/network conditions, plus an extra
local RPC hop for setup/receipt reads. It does not isolate a causal throughput gain.
Entry reserves change admission capacity, not the protocol's execution cost.

Fifty `nestedCounterL2` sources exercised the new immediate shape: one L1 call
plus one nested result row, reserving **105,000** instead of **370,000** gas per
entry. Other fresh or unsupported proxy shapes retained their general reserve.
The registered two-call case is checked separately below.

| Canonical admission stop reason | Postings |
| --- | ---: |
| L1 transaction gas budget | 13 |
| Whole-bundle gas budget | 3 |
| Pipeline time | 5 |
| FIFO fairness cap | 2 |
| Submission backoff | 1 |
| Snapshot exhausted | 1 |

Gas bounded 16/25 postings; timing bounded 5/25. Nine candidates had a fairness
cap configured, but it actually stopped only two: another constraint stopped the
others earlier. Keeping cap origin separate from the actual stop reason exposes
that difference. All 29 attempts have explicit reasons, complete event sequences,
and acknowledgements before their deadlines; 25 became canonical. There are no
blank canonical reasons. All 870 settled sources have accepted estimates; three
other attempts stopped on `pipeline_time` before simulation and explicitly lack
an estimate.

Preparation peaked at 725 ms and proving at 201.15 ms; minimum observed canonical
acknowledgement headroom was 171.23 ms. No nominal 1,000 ms preparation overrun
occurred in this wave, so soft-budget borrowing remains covered by the controlled
regressions rather than this live sample. Required gas stayed within signed limits
and accepted counts matched canonical source counts ([invariants](admission-invariants.json)).

## Registered-proxy repeat and final state

One **new** zero-value source transaction reused the just-verified workload's
`multi-call-twiceL2-1` contract with its next nonce; no old signed transaction was
resent. The proxy was independently confirmed registered before submission.
At canonical L1 block **23,239,324**, the counter advanced **2 → 4**. The accepted
entry reserve was **110,000**, versus the previous **370,000** general reserve.
Admission projected 358,868 gas; full projection was 370,592; final execution and
the canonical postBatch receipt both used **176,867**, under the signed limit of
185,711. The external RPC independently confirmed the receipt and counter.
See the [repeat result](registered-proxy-repeat.json). This extra source is
excluded from the 870-source throughput comparison.

Final [deployment checks](deployment-final.json) confirm the intended node,
signer and indexer image IDs, healthy signer, fresh nonsyncing heads, original L2
genesis, empty cross-chain queue and configuration unchanged except for the two
runtime image references. One initial final-state check raced an external RPC
that trailed the local tip; its failure is retained. The corrected check compares
a shared recent canonical height and separately checks both head ages.

The disposable database and temporary forwarding process are removed/stopped.
Runtime and monitoring remain deployed, with logs, indexer checkpoints, datadirs,
operator settings and unrelated working-tree edits preserved. The local build is
from implementation `036b50a`; the later evidence commit adds documentation and
formats only test sources. No shipping implementation changed after image build.

## Scope and remaining limits

The supplied engineer benchmark is not independently reproduced without its
revision and harness. The pinned fixture comparison uses identical measured
work with the old and new reserve formulas; it does not claim that a reserve is
actual execution gas. Unsupported immediate shapes and deferred nested storage
retain their existing model. Final ordered execution remains authoritative.

The full CI release/Kurtosis matrix and unchanged protocol suite are not rerun
for this follow-up. Local native cases provide semantic assertions and independent
follower replay; a verified Chiado workload provides per-job semantic assertions,
but does not itself run an independent live follower. Pending queue ownership
remains in memory; this change adds no durable recovery guarantee.
