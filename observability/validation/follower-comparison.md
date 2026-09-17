# Independent follower comparison validation

2026-09-17, dirty worktree based on `c36bbd6`; deployment rollout is not claimed.
The [contract](../contracts/follower-comparison.md) defines the proof boundary,
unknown states, scope, resource limits and compatibility behavior.

| Requirement | Implementation | Evidence |
| --- | --- | --- |
| Disabled defaults and actual config propagation | Optional indexer follower config, worker, scrape rendering | Observer config/disabled-no-network tests; private HTTP collector tests |
| Actual independent execution, role/mode/scope | Deriver boot-local cache; explicit launcher diagnostic identity; additive RPC | 48 deriver tests, diagnostic RPC tests; native fresh L1-only follower and overlay canaries |
| Reused history is unknown | No cache restoration or evidence from local-match skips | Native preserved-datadir restart checks real correlation seeding and missing old proof; cache/prefix tests |
| Exact canonical reference and root | Recomputed headers, exact SQL target, L1 anchor/parent and generation rechecks | Observer agreement/disagreement/forgery/lag/reorg HTTP tests; actual production TARGET_SQL against PostgreSQL |
| Real rejected-header diagnostic path | Existing complete-template rejection carries its computed header and advertised hash without changing the native error | Typed error test plus observer rejected-candidate/private HTTP tests; no claim of an induced production STF bug |
| Bounded memory and failures | 128 in-flight / retained headers; 30s collection budget; shared RPC limits and 15/30/60s backoff | Cache eviction/prefix/reorg tests; existing RPC deadline/concurrency tests |
| Unknown never healthy | Known-gated agreement; explicit reason/lag; expiry | 73 real Grafana queries include agreement, mismatch, lag, disabled, stale and stopped-scrape fixture phases |
| Compatibility | Persistent correlation v2 and old RPC fields unchanged; original execution/recovery decisions retained | Existing RPC tests and six affected follower scenarios across the runs below |

Commands and results:

- `cargo +1.94.0 test --locked -p eez-deriver -p eez-node-common -p eez-follower --lib`:
  initial 47/16/8 tests passed. The later typed template-mismatch change raises
  deriver coverage to 48, passed in the integrated focused unit run.
- Integrated observer run: 70 tests passed, including eight follower tests and
  the separately developed funding/cost tests. Prover client 15 tests also passed.
- `cargo +1.94.0 test --locked -p eez-node-e2e --test e2e happy_case_follower_ -- --test-threads=1`:
  five existing scenarios passed (safe parity, deep backfill, L1 reorg recovery,
  rogue sequencer safe-head protection, sequencer overlay). The newly extended
  L1-only restart assertion exposed an incorrect fixture assumption, below.
- Corrected `happy_case_follower_l1_derived -- --exact --test-threads=1` passed,
  including actual STF header/hash/root agreement, Composer rejection metadata,
  a new epoch and genuinely reused persisted history remaining unproven.
- `smoke-database.py`: exact production target SELECT passed same-height Q,
  wrong EEZ scope, stale source and reorg cases in the 47-group schema-14 run.
  The separate saved-binary upgrade run passed 55 groups.
- `smoke-protocol-dashboards.py`: 73 real Grafana SQL/Prometheus queries passed,
  including dynamic follower comparison cases and failed-scrape suppression.

The original restart assertion used the newest safe block immediately before a
hard fixture kill. Reth had persisted through block 14 while correlation metadata
already included block 16. Recovery correctly removed that tail and actually
replayed blocks 15–16, so new proof was legitimate. The corrected test uses the
older first settled reference and asserts that restart really seeded from
persisted correlations. It does not weaken the independent-execution invariant
or change runtime recovery. The original failure remains in
`/tmp/eez-observability-follower-canaries.log`; the corrected run is
`/tmp/eez-observability-follower-restart-reviewed.log` (110.12 seconds).

Other logs/reports:
`/tmp/eez-observability-follower-native-units.log`,
`/tmp/eez-funding-focused-tests-reviewed.log`,
`/tmp/eez-observability-costs-database-reviewed.json`,
`/tmp/eez-observability-costs-database-upgrades.json`, and
`/tmp/eez-protocol-costs-follower-dashboards.json`.

The template-mismatch capture was found during final source review after the
native success/restart run compiled: that native pass proves successful capture
and reuse behavior; the later failure-path change has focused typed/HTTP tests.
Final combined native build and workspace Clippy are tracked by the coordinator.
Actual operator follower placement, independent infrastructure, historical
archive recovery and workload overhead remain deployment acceptance work.

## Combined native checkpoint

The final extended `happy_case_follower_l1_derived` passed **1/1 in 111.00s**
(`/tmp/eez-observability-final-follower.log`). This source build includes the later
typed rejection capture and required-blob diagnostics. It verifies real successful
STF provenance, explicit Composer identity, three repeated live Watcher blob
retrieval failures, exact receipt/EEZ references, availability recovery and safe
convergence, followed by persisted-history restart with a new evidence epoch.
It still does not claim that a real STF divergence was induced; that rejection
branch has focused typed/error and private HTTP coverage.

Both expanded native lifecycle canaries also passed **2/2 in 257.21s**, including
actual stage/policy metric scrapes and paid-attempt membership assertions. Their
sanitized metric inventory parser passed its one explicit injection-boundary test.
Logs: `/tmp/eez-observability-final-native-lifecycle.log` and
`/tmp/eez-observability-final-metric-inventory-test.log`. Runtime binary hashes are
recorded in [required blob validation](required-blobs.md); these are source-built
canaries, not Docker shipping-artifact verification. The follower test executable
hash was `a331d815a74b1f0a99ab31843d80de98d2d2e7b6300e50e2be971a391757ca49`.
