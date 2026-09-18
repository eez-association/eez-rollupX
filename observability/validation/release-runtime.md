# Local release validation and acceptance-test corrections

Status: **historical release runtime built and exercised; full acceptance remains open**.
The branch is now being integrated on top of PR #26; see the
[PR26 integration record](pr26-integration.md) for current-source validation.
The artifacts and results below are retained for their original revisions and do
not validate the new derivable-operations baseline.
The last pre-integration application runtime is `45e5c015`; its results are recorded in
[the current checkpoint](#current-schema21-checkpoint) and the companion JSON's
`schema21_release_checkpoint`. Timing/performance acceptance is explicitly
postponed at the user's request until a suitable window or host is available.
No further benchmark preflight or comparison should run before explicit resumption.

## Historical schema20 checkpoint

The earlier runtime revision is `03a3caa1b92ca39e0119f7417e347c78c8f530c9`, with the
unchanged protocol pin `3c0cdf4955941a5c23639346932b55b33babebbd`. Frozen main is
`5469c88860408daf776ee812156dd280c1a9e262`; it is included in the branch.

These are local results, not published CI or deployment evidence. Full logs,
manifests and retained databases live under the ignored
`target/observability-release-03a3caa1-20260917T234016Z/` directory in the
implementation worktree. They are unavailable to another checkout. Compact
results in [release-runtime.json](release-runtime.json) and below preserve failures rather than treating a retry or supplemental
run as a clean original suite.

| Check | Observed result |
| --- | --- |
| Standard release producer and exact-revision consumer | Passed; Rust 1.94.0, locked dependencies, default shipping features, no publication |
| Archived release unit/vector tests | 1,194 passed: 948 main archive, 246 separate Composer archive |
| Release observer and real PostgreSQL | 64 grouped checks passed, including migrations and older schema upgrades |
| Embedded release suite | 50 clean passes, one retry-only pass, one standard excluded live-Kurtosis target; CI failure retained |
| Cross-chain release suite | 60 clean passes and one retry-only pass before the 90-minute deadline; seven cases unfinished |
| Supplemental unfinished cross-chain cases | All seven passed, serially against the same verified runtime and test artifacts |
| Physical ENOSPC handoff | Passed with the release runtime: bidirectional semantics, canonical state and signature while full; two fresh committed records persisted after space release; no writer restart |
| Full Kurtosis release lane | Passed in 1,728.74 seconds: all 28 upstream scenarios, four wave modes, state chaining, 133 signed windows, zero root divergence, safe-head convergence; owned enclave removed |
| Native trace-loss alert | Actual notification and recovery passed without resetting the loss counter |

Kurtosis used the supported four-worker scenario limit and overlapped functional
tests. This is not a performance or maximum-concurrency result. The physical
quota fixture had no OOM kill but recorded memory-limit reclaim pressure; it
does not establish memory headroom.

## Failures and corrective scope

The unchanged fullness test expected exactly two blobs. Frozen-main release
binaries reproduced the same three-versus-two failure and passed only on retry.
Additional empty history before a successful exact target legitimately grows the
encoded range. The corrected oracle requires multiple blobs, one canonical post
from the registration root covering both large transactions, their successful
independent replay at the same block hashes, and no historical chunk or follow-up
post. Existing early-fullness decision assertions remain.

The trace test required a retried RPC and successful signing in one preparation.
Its retained first failure contains 169 spans and complete ancestry for all 31
signer spans. The outage exhausted window 65; the same two exact admission
identities signed in window 70. RPC SPEC section 6.3 permits this requeue. The
corrected checker requires complete retry and signing graphs joined through
native lifecycle records and exact linked admission/source identities. The
fixture now holds the outage until the first preparation closes, exercising
window rollover deliberately. Five deterministic checks cover both valid paths,
missing ancestry/links, reordered records, changed admissions and later retries.
An ancestry failure now retains its lifecycle spool as well as raw OTLP capture.

Initial verification of the new fullness oracle exposed a stale pre-restart RPC
provider in the new assertion. That diagnostic failure is retained. After
refreshing the provider, the corrected oracle passed against frozen-main release
binaries in 110.89 seconds and candidate release binaries in 109.30 seconds.
The native trace rollover passed in 86.87 seconds, with complete parent chains
for all 15 signer spans and exact lifecycle membership across windows 75 and 85.
These corrected harnesses use the optimized-development test profile against
unchanged verified release runtimes; they are not rebuilt release test archives.
Production runtime behavior is unchanged.

The cross-chain job allowance increases from 90 to 120 minutes. No-publication
mode increases from 180 to 360 minutes because it runs artifact production and
all integration lanes serially; measured lane durations total about four hours.
Test selection, serial execution, strict retry failure policy and release gates
remain intact. Seventeen workflow contract checks and 26 artifact-tooling tests
passed; one optional tiny-workspace relocation test was not rerun. The full
release producer/consumer execution above remains separate evidence. The enlarged
complete job has not yet been run as one uninterrupted release suite.

## Current schema21 checkpoint

The later schema-21 release at `45e5c015` completed artifact verification,
1,194 release unit tests, 58 PostgreSQL groups and 96 actual Grafana queries.
Its complete local network lane passed all 28 scenarios without skips, with
130 signed/attested windows, zero root divergence and confirmed enclave removal.
The single enabled capacity diagnostic reconciled 2,722 sources without OOM,
but reached its 512 MiB memory ceiling; it establishes no spare capacity or
comparative overhead result.

That release's embedded suite finished with 50 passes and one retry-only failure
in the ordinary-burst test. The contract permits recovery into a future `Steady`
slot, whereas the old assertion requires `Catchup`. The correction committed in `17fb4c77`
checks the actual missed empty boundaries and retains the existing semantics.
The original frozen-main test passed on its first attempt in 172.14 seconds;
the failure did not reproduce in that run. All four corrected process checks
passed against baseline and candidate release runtimes on Ethereum and Gnosis.
The candidate Ethereum case had seven `Empty` and 27 `Steady` invocations, with
no `Catchup`: the corrected assertion checks the missed empty boundaries and
all retained receipt/replay invariants on the path the old assertion rejected.
These are functional results from the optimized-development test executable;
the original candidate failures remain recorded.
The cross-chain lane completed in 6,126.25 seconds: 73 passed, one retry-only
failure and two standard skips. The failure is the Ethereum fitting-40-request
test. Its first wave was
fully queued, but measured-time admission split it into 39+1 sources across two
L1 blocks. Admission took 163 ms in the failure versus 93/107 ms in the retry's
two waves. The admission rule and test helper match frozen main; other measured
paths contain new instrumentation, so host contention and candidate overhead
remain unresolved. Keep the single-Sync assertion and timing budgets unchanged
until the baseline/candidate diagnostic establishes stronger evidence. The
failure prevented the CI runner's automatic physical-quota handoff. Its separate
exact-artifact check passed in 129.47 seconds: protocol effects, canonical state
and signatures remained valid with the 3 GiB volume full and Alloy stopped;
two fresh committed records persisted after space release without restarting
the writer. The owned container was removed. There were no OOM kills, but 8,589
memory-limit events occurred; this establishes no memory headroom. The original
failed CI result remains unchanged. Terminal suite/watcher review confirmed no owned native
processes or PostgreSQL containers remained. The fixed-order baseline/candidate
diagnostic stopped before launching a workload: its 60-second preflight observed
57 swap-in pages, exceeding its declared zero-swap prerequisite. A second
preflight observed 14.50 busy CPU cores and 20 swap-in pages; it also stopped
before launching a workload. The user then requested postponement because no
quiet window or dedicated host is available. No runtime comparison result exists.
The separate burst regressions are functional checks,
without a performance claim.
The companion JSON
preserves the current hashes and explicitly local-only artifact locations;
earlier results and failures above remain unchanged.

The controller cleanup correction committed in `f55e0a8e` separately passed 50 focused
Python tests, three native configuration tests, affected harness compilation,
strict workspace Clippy and formatting. Subsequent ownership review reproduced
visible session-leader PID reuse reaching the signal path in two mocked cases.
The added generation guard passed all 31 runner tests, including four new mocked
cases and the existing real exited-leader sibling cleanup regression. It detects
visible reuse; it does not prove atomic session ancestry across unobserved
replacement lifetimes. All five native cleanup cases then passed in 348.21
seconds: successful A/B runs, forced termination after readiness, a real early
deadline, and the original returned-error fixture API. The four controller cases
verified process termination, container absence and removal of both owned parent
directories. The forced-after-ready case retained a 311,430-byte dump whose
contents were readable with the pinned `pg_restore`; failed native chain datadirs
remain intentionally retained as evidence. These checks do not amend the sealed
release's test result or establish performance acceptance.

The release test-harness checkpoint at `f55e0a8e` is complete. Shipping and
probe artifacts reuse the exact sealed45 runtime binaries, with separately
recorded test-source provenance. All six pure configuration tests passed (three
per variant, none ignored), including absent/default settings, invalid limits
and owned-fixture compatibility. Artifact hashes were checked before and after
execution. This did not run a chain workload or performance comparison.

The first probe export failed verification: it contained the older two-test
executable despite updated source inputs. A dependency-free Cargo reproduction
confirmed that preserved source timestamps can predate inherited fingerprints.
The corrected local recipe cleans only the overlaid `eez-node-e2e` release
package and requires the exact three current tests before export. The rebuilt
probe passed all three; the original failed artifact and verification remain
retained and must not be used for measurement. The already verified shipping
artifact was retained. The unbound v3 performance draft predates this correction
and needs fresh artifact bindings before any future use after explicit resumption.

The five trace regressions, affected-target strict Clippy, workspace formatting
and diff checks passed after the corrections; the focused diff was reviewed.
The fitting40 timing comparison, shipping overhead, lock-probe calibration and
paired confidence checks are **deferred by the user** until explicit resumption
with a suitable window or host. Limits and acceptance criteria remain unchanged;
deferral is not a passing result.
Publication approval, private rollout inputs, an independent heartbeat receiver,
off-host recovery and the seven-day deployment baseline also remain open.
