# Self-hosted observability implementation evidence

| Field | Value |
| --- | --- |
| Date/status | 2026-09-16 UTC; ongoing implementation, no production acceptance |
| Source | `1ab46c8` on `feat/network-observability`; main `5469c88860408daf776ee812156dd280c1a9e262` merged as `5cbdd02` |
| Protocol | `3c0cdf4955941a5c23639346932b55b33babebbd`, unchanged |
| Environment | Rust 1.94.0; isolated local PostgreSQL 18.6 and synthetic RPC fixtures |
| Binary/images | Exact observer SHA-256 and migration/image digests in [database report](../../../observability/validation/database.json) |
| Scope | Canonical receipt accounting, method classification, applied-range correlation and exact L2 finality; earlier stack/transport milestones have separate evidence |
| Requirements | [Proposal](../../proposals/network-observability.md), [coverage ledger](../../../observability/requirements.md), [review dispositions](../../proposals/network-observability-review-disposition.md) |

The worktree is `target/worktrees/network-observability` beneath the shared
checkout. Latest main was fetched once and merged without conflicts or changes
to the uncommitted patch. The user subsequently froze main and requested no
periodic checks. All further work uses that baseline.

## Current findings and checks

The observer stores exact receipt costs independently of node-reported settlement
claims. Multiple posts in one block, repeated collection and fork reinclusion do
not multiply charges. L2 finality requires exact applied hashes and a finalized
canonical L1 anchor. Missing/conflicting claims remain gaps; a derived finality
contradiction freezes that source without preventing L2 receipt reconciliation.
The RPC provides no processed-L1 watermark, so an empty range response cannot
prove that no application occurred.

| Requirement/check | Evidence type | Result |
| --- | --- | --- |
| R08–R13: index/migrate/correlation runtime | Real observer/PostgreSQL, synthetic RPC | [Database report](../../../observability/validation/database.json): upgrade, restart, crash-before-commit, concurrent cursors, reorg/reinclusion, exact fees, conflicts, finality and bounded backlog all passed |
| Observer/telemetry all-feature tests | Unit/component | 25 + 14 passed; no ignored tests |
| Observer/telemetry all-target/all-feature Clippy | Static | Passed with `-D warnings` |
| Workspace formatting and diff whitespace | Static | Passed |
| `cargo +1.94.0 clippy --locked --offline --workspace --all-targets -- -D warnings` | Static | Passed (including native node, follower, signer and E2E targets) |
| Observer default-feature build | Build | Passed |
| Main admission/preparation regressions | Composer unit selection | `composer::`: 101 passed, 104 filtered; not the full Composer suite |
| CI tooling after main merge | Python tooling | Initial 233-test run exposed two new workflow-convention violations and four explicit optional skips; corrected in `acd120c` |
| Corrected CI artifact/cache handling | Targeted tooling and shell producer | Native artifact suite 25 passed/1 skipped; workflow contract suite 14 passed; inline summary producer exercised successfully |

Current commands and scenario details are recorded in
[database evidence](../../../observability/validation/database.md).
Full workspace build/process acceptance is still a broader goal gate; passing
workspace lint is not presented as runtime completion. The four initial tooling
skips require real data-only Docker payloads, real Kurtosis resource probes,
native archive relocation artifacts and a real runtime Docker fixture.

An initial unit run failed when sandbox socket binding was unavailable; the same
tests passed with local socket permission. A first schema-3 test query failed
because `frozen_reason` was ambiguous in its join; qualifying the intended source
fixed the test query. No tests were weakened. Later checks include the additional
bounded finality bookkeeping and 17-post backlog regression.

## Evidence and limits

Small sanitized reports, deterministic fixture scripts and migrations are checked
in. Earlier [exporter](../../../observability/validation/initial-slice.md),
[backend](../../../observability/validation/backend-stack.md),
[transport](../../../observability/validation/lifecycle-transport.md) and
[native startup](../../../observability/validation/native-startup.md) results apply
to their recorded revisions, not automatically to subsequent binaries.

Detailed build/test logs under `/tmp/eez-observability-*` and the saved
`/tmp/eez-observer-schema-v1` upgrade executable are local-only, with no promised
retention. The database suite can reconstruct v1 from its immutable migration
when that executable is absent; actual old-executable refusal used the saved binary.
No production database, chain datadir, funded wallet or live service was changed.
Only fixture-owned containers, volumes and networks were created and removed.

This does not establish independent source-effect replay, historical companion
attribution, authenticated blob byte measurement, native transition coverage,
clean-host restore, production performance, deployment or the seven-day baseline.
Those remain explicit work in the coverage ledger. Lifecycle ingestion is the
next integration boundary; accepted-request history must remain unknown after
missing evidence rather than becoming an inferred completion.
