# Coverage gaps found during checkpoint review

The review after `9c78b55` found missing network/infrastructure panels, missing
blocked/serviceable duration reporting, and restore evidence that predated
schema 20. This follow-up implements those requirements and strengthens the
affected fixtures. It does not close release, comparative performance or live
deployment acceptance. The [machine-readable record](coverage-followup.json)
keeps source identities, results and outstanding work separate.

| Requirement | Implementation / configuration | Evidence |
| --- | --- | --- |
| R03/R04 network observations | Optional exporter `network` config; independent bounded workers; exact canonical hash and chain/genesis checks; no new calls when absent | Observer configuration, real HTTP identity/fork/unknown tests; live backend config propagation, failure isolation and recovery |
| R03/R21 network/infrastructure coverage | Safe/settled-finalized heights, finalized block age, explorer lag, host CPU/network/I/O, client peers/sync and observed restarts; explicit capability limits | Every network/infrastructure query against real Prometheus, positive configured-producer results; existing configured-source privacy/identity contracts |
| R20/R27 separate waiting portions | At most 4,096 monotonic clocks; blocked/serviceable/unknown portions; source/reorg/expiry invalidation; retained internal recovery; missing-clock and whole-capture coverage | Deterministic timing/conservation, bounded population, sender-prefix/in-flight and recovery regressions; native canaries in both policy modes |
| R02 disabled compatibility | Optional metrics clocks and optional network workers preserve absent-config behavior; lifecycle/queue RPC formats unchanged | Existing disabled bidirectional native canary, same frozen shipping-feature runtime as enabled checks |
| R21 usable timing/coverage panels | Separate current accumulated portions, terminal percentiles and explicit unknown/capture-loss counts; quantiles are not summed | 96 real Grafana queries, 17 backend groups and existing Chromium navigation checks |
| R29 current schema and runbooks | Exact schema-20 backup/restore; six operational dashboard UIDs plus operator runbooks, with restored document contents | Fresh owned instances/volumes; retained SQL identity, roles, accounting, history and actual restored alert firing/recovery |

The final six-package Rust suite passed **674 tests**: Composer 246, observer 78,
telemetry 44, signer 287, client 15 and control RPC 4. The optional lock-probe
feature passed **251 Composer tests** and a node feature check. Workspace strict
Clippy passed. The Python suite passed **109 tests**, including the real owned
PostgreSQL resource fixture.

The enabled native canaries passed **2/2 in 259.03 seconds**, verifying that all
three readiness portions preserve the original total duration, alongside every
existing protocol effect/signature/observer assertion. The disabled native canary
passed **1/1 in 73.25 seconds**. Runtime files were copied from the shipping
packages with default features; Cargo artifact identity was checked separately
from the test package's same-named binaries. These are optimized-development
functional checks, not release-profile overhead measurements.

The prefix review found that a refreshed blocker must not reclassify an already
validated-ready or in-flight descendant as queued blocked work. The final code
uses the existing ready-source and in-flight ownership maps; the regression
covers expiry and subsequent source changes. Reconciliation retains timing
inputs before pruning but selects terminal outcomes after the existing ownership
operation, preserving locally established dispositions.

Failures were retained rather than replaced: incomplete agent edits first needed
a missing test field and lint corrections; sandboxed loopback fixtures required
the normal local-network permission; the backend fixture's added consensus
target needed its own service; and old two/six-dashboard checks needed to include
the new runbooks. The prior restore attempt also identified an incompatible
multi-role host build; the final standalone observer successfully loads in the
pinned Bookworm database image.

Interpretation: waiting portions are observed native metrics with explicit unknown
time, not invented historical SQL intervals. Node-safe tags and explorer highest
blocks are independently fenced observations, not a claim of independent finality
or gap-free indexing. The [network contract](../contracts/network-health.md)
documents supported profiles and the polling-only deviation from the proposed
subscription wakeups.

Remaining gates are the final release artifacts/CI, lock-probe calibration and
sufficient paired overhead measurements, and the private rollout manifest,
independent heartbeat receiver, physical-host recovery and baseline week. Grafana
showed memory-limit reclaim activity in the browser fixture; passing without OOM
does not establish deployment capacity headroom.
