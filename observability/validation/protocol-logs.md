# Protocol receipt logs: implementation and validation

Source `1be022c` adds the [schema-6 log collector](../contracts/protocol-logs.md).
The [artifact report](protocol-logs.json) records the exact implementation commit,
build base, observer digest, unchanged main/protocol baselines, migration digests
and saved schema-4/schema-5 executable identities. The source was tested in the
observability worktree before committing; no runtime source changed afterward.

| Requirement / invariant | Implementation | Verification |
| --- | --- | --- |
| R19 evidence prerequisite: retain exact occurrences without inventing success | Fixed ABI parser; immutable log claims and separate canonical views | Exact roots, repeated call hash at distinct queue positions, emitters/rollups, full-width queue indices and ordering checks |
| Invalid or partial data cannot imply absence | Whole-receipt parser; missing/malformed/limit gaps; complete empty sets | Missing/null/invalid arrays, removed/foreign/duplicate/out-of-order logs, reverted receipts, ABI bounds, 4,096-event boundary |
| R10 costs survive weaker or contradictory evidence | Independent worker and tables; fee parsers unchanged | Missing, malformed, capture-limited and conflicting logs retain 738216 wei; reinclusion retains history and exposes 804216 wei once |
| R09 canonical retraction and immutable history | Existing receipt/block canonical flags; unambiguous-claim views | Same-height L1 fork retracts prior conflicting occurrence; new inclusion has its own claims and visible logs |
| Contradictions remain unresolved | At most two different digests per receipt under chain lock | Repeated claim deduplicates; second claim hides stronger evidence; third/earlier repeats cannot erase conflict; runtime writer update is rejected with the expected table permission error |
| R13 explicit upgrade and diagnostic roles | Migration 6; existing migration verification; reader grants | Populated saved v4/v5 executable upgrades preserve costs/lifecycle, old binaries refuse downgrade, reader can inspect retained raw claims/attempts |
| Bounded independent recovery and disabled compatibility | 32-receipt oldest-attempt pass, separate connection/deadline/backoff; existing contract selection | 33 receipts require two passes; persisted backlog resumes across process restarts; no-contract configuration creates no claims; RPC failure preserves costs and earlier immutable evidence |
| R04 bounded RPC and truthful metrics | 8 MiB receipt helper, unchanged 1 MiB aggregates; separate freshness/error metrics | 2 MiB receipt accepted, same aggregate response rejected, above-8-MiB receipt rejected; failed/stale/future metrics omit counts but retain last success |

The final build passed **36 observer unit/component tests**, **37 disposable
PostgreSQL/RPC check groups**, workspace all-target Clippy with `-D warnings`,
formatting, Python syntax, diff whitespace and relative documentation-link checks.
The database suite also reran the pre-existing receipt, correlation, lifecycle,
source-enrichment, concurrency, kill/replay and schema-checksum regressions.
Docker fixtures used unique names/private networks and verified cleanup.

First failures are retained in the artifact report: restricted local sockets,
one redundant-closure lint, a text/varchar SQL parameter mismatch, and an ambiguous
column in a new test assertion. No existing test was removed or weakened. An
intermediate database pass preceded the final read-only diagnostic grant; only
the final report validates the committed schema.

Reproduce with Rust 1.94.0, local socket access and Docker:

```bash
cargo +1.94.0 test --locked -p eez-observer --lib
cargo +1.94.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.94.0 fmt --all --check
cargo +1.94.0 build --locked -p eez-observer
python3 observability/tests/smoke-database.py \
  --binary target/debug/eez-observer \
  --report /tmp/protocol-logs-database.json
```

Optional `--previous-v4-binary` and `--previous-v5-binary` exercise actual saved
executables. Without them, the suite still constructs the populated initial
schema and checks forward migration, but does not count saved-binary checks.
Logs are local-only under `/tmp/eez-observability-protocol-logs-*`, notably
`-database-final.{json,log}`, `-final-tests.log`, `-final-clippy.log` and
`-final-build.log`. No retention is promised.

The interpreted boundary is conservative: a configured L1 contract enables
collection from already indexed receipts, including source dependencies. This
step retains observations for every emitter; consumers must filter the configured
contract/rollup and validate batch membership. No source completion is inferred
from a root, a log set, receipt status, or matching height. The codec, dependency
pins, signed protocol and native runtime binaries are unchanged. Native E2E and
independent replay were not rerun for this observer-only slice. R19 outcome views,
exact companion allocation and all other open release gates remain in the
[requirement ledger](../requirements.md).
