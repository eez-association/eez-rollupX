# Drain engine workers before process exit

| Field | Value |
| --- | --- |
| Design status | Draft for PR #84 review; local acceptance passed, hosted validation pending |
| Date | 2026-10-12 |
| Fork baseline | `ccb393d621763ce296edab7903820c7b22704b04` on `codex/public-release-preparation`; base `e15bb412420b36bfa40a0e2a73233ac4f213441d` |
| Dependencies | Reth v2.7.0 `3d592ece6de8c4559987416a544fc215fd6d6921`; protocol `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` |
| Evidence | [Public release investigation](../audits/2026-10-11-public-release-readiness/README.md) |

The hosted restart test aborted in RocksDB cleanup. A local diagnostic captured
the L2 persistence thread dropping its database after Reth had acknowledged
termination. Separately, Composer's embedded L1 shared Tokio threads with the
CLI but had an independent, unobserved Reth shutdown signal and panic monitor.

The proposed correction backports the engine cleanup and launcher changes from
the still-unmerged [Reth PR #27609](https://github.com/paradigmxyz/reth/pull/27609)
to the exact dependency pin. Only two Reth packages are vendored; see their
[source and license record](../../crates/vendor/reth/README.md). The Composer
also links the embedded L1 runtime's graceful drain and critical-task failures
to its parent. Independent worker pools are retained so one chain cannot block
the other's named workers.

| ID | Required behavior | Acceptance |
| --- | --- | --- |
| S1 | Engine persistence and prewarming cleanup precede termination acknowledgement, including persistence failure | Upstream synchronization regression and the instrumented native restart reproduction |
| S2 | CLI shutdown reaches L1 and waits for its graceful tasks; an L1 critical panic reaches the CLI monitor | Production-helper unit tests fail with old wiring and pass with the fix |
| S3 | Restart test prevents every later settlement while preserving real policy and follower assertions | Persistent relay barrier regression; resume delivery before checking the new policy; native restart and independent replay |
| S4 | Preserve protocol, database formats, dependency versions and shipping features | Lockfile/source comparison, source builds, shipping CI artifact and E2E gates |

The L1 drain runs on a Reth blocking worker and retains the parent's graceful
guard until completion. It uses the existing five-second CLI budget, not an
additional operational timeout. Forced shutdown and persistence errors remain
possible; a shutdown notification is not a durability acknowledgement.

Delays, retry-only passes, ignoring SIGABRT, leaking database handles, modifying
Cargo's cache and updating unrelated dependencies do not establish the required
ordering. The bounded local exit delay used to capture the crash is diagnostic
instrumentation only and must not enter production code.

There is no wire, configuration, storage or historical replay migration. The
lockfile source change changes the prover validation identity, so Composer and
signer must be rebuilt and restarted together from the same revision; an old
signer is rejected at startup. Followers also need the new binary for their
shutdown cleanup. Roll back Composer and signer together. Rolling back restores the shutdown
race. Graceful shutdown does not make pending requests, retry counters or drain
intent durable; clients must still reconcile and resubmit unsettled work.

The [audit](../audits/2026-10-11-public-release-readiness/README.md#local-validation-and-remaining-limits)
records passing lifecycle and cleanup regressions, both instrumented process
comparisons, and three uninstrumented restart scenarios with follower replay.
Acceptance still requires a new hosted run of all existing validation lanes.
No deployment, transfer or visibility change is part of this proposal.
