# Local release validation and acceptance-test corrections

Status: **release runtime built and exercised; full acceptance remains open**.
The runtime revision is `03a3caa1b92ca39e0119f7417e347c78c8f530c9`, with the
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

## Open gates

The five trace regressions, affected-target strict Clippy, workspace formatting
and diff checks passed after the corrections; the focused diff was reviewed.
Shipping overhead, lock-probe calibration and paired confidence checks have not passed.
Publication approval, private rollout inputs, an independent heartbeat receiver,
off-host recovery and the seven-day deployment baseline also remain open.
