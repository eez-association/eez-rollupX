# Observed finality validation

2026-09-17, dirty worktree after `c36bbd6`; no production rollout is claimed.
See [the contract](../contracts/finality-timing.md) for exact assurance,
cross-host clock assumptions, missing-history behavior and upgrade limits.

The isolated PostgreSQL gate applied actual migrations 1–17 with the pinned
database image, 512 MiB, one CPU, no network, temporary storage and the unchanged
10-second statement deadline. `tests/finality_timing.py` passed 15 timing-state
assertions, two additional invariant blocks and its canonical-anchor precondition.
It reuses the existing canonical-outcome fixture, whose native decisions are
synthetic trusted-cache inputs; this is not an independent replay claim.

Coverage includes canonical completion before finality, exact P/Q hash
distinction, L1 finality insufficient for L2, exact finalized source-only revert,
repeat/restart timestamp idempotency, late parent conflict, finality freeze,
stale lifecycle, producer/observer clock inconsistencies, pre-admission finality,
historically missing observation time, unfinalized reorg and absent admission.
The full database smoke suite invokes this same helper after canonical fixtures.

Final SQL log: `/tmp/eez-finality17-sql-boundary.log`, including maximum-u64
admission UTC without timestamp overflow. The initial log retains an
unrelated draft schema-18 column-name failure before these tests ran; subsequent
standalone runs restricted the migration range to 17. Final schema-17 SHA-256:
`5bfbe1bbdfe6fd0e004874552c80de9397712e31d26bf1aad3d59d8437758047`.

Python compilation, `test_prepare.py` (15 tests), and `git diff --check` passed.
The real Grafana query helper executes both finality panels and exercises measured
percentiles/counts, source-revert taxonomy, clock gaps and frozen evidence. Its
first combined run correctly returned `stale_lifecycle` after earlier Prometheus
phases exhausted the 90-second fixture freshness window. The helper now refreshes
only owned synthetic producer/chain/correlation timestamps before each assertion;
production freshness gates and intentional failure/freeze fields are unchanged.
The corrected combined dashboard gate passed 82 real queries, including both
finality panels and their state transitions. A further bounded numeric-cohort
filter change avoids timestamp conversion for maximum-u64 admission values; its
SQL regression and final joint dashboard rerun both passed. Final dashboard
log/report: `/tmp/eez-observability-schema18-dashboards-final.{log,json}` (82
generated queries plus dynamic state assertions). Final binary/database
validation remains separately tracked.
