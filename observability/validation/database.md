# Canonical database/scanner evidence

2026-09-16. Branch `feat/network-observability`; isolated PostgreSQL 18.6 and
synthetic read-only RPC fixtures only. No Chiado services, transactions, keys,
chain data or production databases were touched. This is receipt-indexer
acceptance evidence, not completion of WP3b or the full observability release.

The machine-readable [report](database.json) records the exact image digest and
successful scenarios. `smoke-database.py` first exercises the real opt-in Compose
profile, then creates its own internal Docker network and temporary database/RPC
containers. It removes only its own containers, volumes and networks. All
notification destinations in profile preparation are unused fixture names.

| Requirement | Implementation | Behavior checked |
| --- | --- | --- |
| R08 | indexer.rs, indexer/ledger.rs | Identity before indexing; default/nondefault batch propagation; restart/duplicates; fork during receipt fetch; atomic block/receipt/cursor rollback on killed scanner; concurrent workers and retry |
| R09 | 0001-canonical.sql and scanner | Same-height fork/reinclusion retains original receipt; one canonical charge; finalized contradiction freezes; L2 RPC finalized tag never queried |
| R10 | accounting.rs and canonical SQL views | Exact U256 products and wider lifetime sum; reverted receipts paid; missing fees stay NULL gaps; complete charges immutable; receipt/type mismatch refused |
| R13 | database.rs, Compose profile, prepare.py | Explicit/idempotent migration; unmigrated/unknown/newer/checksum-changed schema refusal; no startup DDL; administrative credentials refused for indexing; read-only viewer cannot mutate or access raw tables |
| R06/R29 | postgres profile, dedicated volume and secrets | Private port, generated separate logins, real reader authentication, persistence after restart; clean-host restore still open |
| R28 | observability.yml | Database recovery suite runs after binary build; only sanitized reports uploaded |

Commands:

```bash
cargo test --locked --offline -p eez-observer -p eez-telemetry --all-features
cargo clippy --locked --offline -p eez-observer -p eez-telemetry --all-features --all-targets -- -D warnings
cargo fmt --all --check
cargo build --locked --offline -p eez-observer
python3 observability/tests/test_prepare.py
python3 -u observability/tests/smoke-database.py
python3 -u observability/tests/smoke-stack.py --report observability/validation/backend-regression.json
git diff --check
```

Observer: 18 tests passed. Shared telemetry: 14 tests passed. Preparation: six
tests passed. Socket tests require local binding permission; the initial sandbox
run failed at bind and the authorized isolated-fixture run passed. Clippy,
workspace formatting and diff whitespace checks passed. PostgreSQL recovery and
profile checks passed; the default backend regression report is recorded separately.

The database suite deliberately covers a failed mid-fetch fork, missing history,
contradictory receipt type, wrong chain identity, and a finalized contradiction.
Failures must not create false zeros or duplicate spend. It kills a scanner while
its receipt insert is paused before commit, verifies both the new block and cursor
rolled back, then restarts successfully. A partial HTTP request held across
SIGTERM verifies the bounded drain. An early test found a PostgreSQL parameter
inference mismatch in chain-gap insertion; explicit text casts fixed it, and the
pruned-history fixture now verifies persistence and resolution.

The driver is locked at `tokio-postgres` 0.7.18. Existing SHA-256 users retain
0.10.9; the PostgreSQL driver's authentication dependencies also use 0.11.3.
The lockfile's libredox update is required by its whoami dependency's minimum;
it does not change Linux protocol code. `cargo deny check advisories bans sources`
and `cargo audit` passed the repository's unchanged policy; audit retained the
14 pre-existing allowed warnings. No advisory exceptions were added.

Remaining acceptance includes exact settlement/companion/source relationships,
applied L2 range finality, blob authentication/length, lifecycle ingestion,
clean-host restore and measured larger backfills. The saved Chiado audit remains
the arithmetic/reference-membership corpus; synthetic RPC fixtures do not
substitute for that corpus or independent replay proof.

Schema-2 follow-up: the observer now reuses the existing protocol ABI for posting
classification, with an allocation-free token-layout preflight before decoding.
All 22 observer tests passed, including nested calls/static root arrays, forged
array lengths, overlapping tails, trailing data and explicit separation of the
historical pre-int192 selector. The current empty-call RPC fixture is generated
by `cargo run -p eez-observer --example posting-fixture` and checked against the
encoder in the test; the old-selector fixture is reused from the signer corpus.

The database suite now upgrades a populated v1 schema, verifies exact costs are
unchanged, and backfills missing method evidence through RPC. With
`--previous-binary /tmp/eez-observer-schema-v1`, it also uses the saved milestone
binary to install v1 and proves that old migration/indexing commands refuse v2.
Unavailable method data, invalid calldata and noncanonical layouts keep wallet
charges while reducing posting classification coverage. Wallet and posting costs
are overlapping views, never two amounts to sum. Exact applied settlement ranges,
companion membership and source allocation still remain open.

Schema-3 follow-up, 2026-09-16, aligned to frozen main `5469c88` via merge
`5cbdd02`: 25 observer tests and 14 telemetry tests pass with Rust 1.94.0 and
`--all-features`. Their all-target/all-feature Clippy check and workspace format
check pass. The default-feature run has 36 tests, not 39; the extra three exercise
native telemetry integration. A first sandbox attempt could not bind its three
local RPC fixture listeners; rerunning with socket permission passed without a
code/test relaxation. Observer build succeeds with default shipping features.

The latest [database report](database.json) records the tested binary and all three
migration hashes. It verifies populated v1-to-v3 upgrade and old-binary refusal,
multiple posts in one block without fee/finality multiplication, exact L2 hash
membership, unfinalized forks and derived-finality-only freeze. Empty, duplicate,
wrong-anchor and conflicting range evidence cannot fabricate finality. Seventeen
same-block posts prove the sixteen-range validation/promotion budget resumes on
the next invocation. The node's `l2Finalized` flag is ignored in both directions.
An earlier schema-3 fixture assertion used an ambiguous unqualified SQL column;
qualifying the source fixed the test query. The full suite passed thereafter,
including after adding persisted bounded promotion.

Executed current commands (from the implementation worktree):

```bash
cargo +1.94.0 test --locked --offline -p eez-observer -p eez-telemetry --all-features
cargo +1.94.0 clippy --locked --offline -p eez-observer -p eez-telemetry --all-features --all-targets -- -D warnings
cargo +1.94.0 fmt --all --check
cargo +1.94.0 build --locked --offline -p eez-observer
python3 -u observability/tests/smoke-database.py --previous-binary /tmp/eez-observer-schema-v1
```

The saved v1 executable is local-only; without it, the test constructs the exact
v1 schema/checksum and still exercises populated migration. The fixture suite
uses real observer processes/PostgreSQL but synthetic RPC history. It does not
prove independent protocol replay, live deployment capacity or per-source success.
Lifecycle ingestion, companion/source allocation, authenticated blobs and the
remaining [acceptance gates](../requirements.md) are still open.


Schema-4 lifecycle follow-up, 2026-09-16: optional source configuration now reaches
independent file workers, shared wire validation, PostgreSQL chunk/control/raw
storage, logical assembly and HTTP metrics. The current [report](database.json)
records the final default-feature executable and all four migration digests.
27 observer tests and 21 all-feature telemetry tests pass. Workspace all-target
Clippy, formatting, preparation tests (six), `cargo deny` and `cargo audit` pass;
audit retains the existing 14 allowed warnings with no new exception or package
version change. The observer now directly depends on the existing telemetry crate.

The process suite still runs every canonical/settlement regression. Added lifecycle
checks use public synthetic JSONL on a local read-only container mount and real
filesystem locks/PostgreSQL/observer processes:

| Requirement | Observable evidence |
| --- | --- |
| R16/R18 | Out-of-order chunks assemble only when complete; repeated identical chunks and concurrent readers preserve one event; `.active` to `.closed` retains the checkpoint |
| R18 | Killing the importer after chunk staging but before commit rolls back event, chunk and byte offset; restart imports exactly once |
| R17/R18 | Killing a separate lock-holding fixture process establishes end; partial trailing bytes become explicit gaps; a missing lock remains unknown |
| R18 | Holes are provisional, then suspected using an injected database observation age; late valid records repair them; two successful absent inventories establish retention loss |
| R16/R18 | Unknown wire/payload schemas preserve raw bytes and positions; a conflicting late chunk retracts transport success; opaque evidence stays quarantined |
| R18 | A clean ended epoch with a full-u64 high-water value yields one interval, without enumerating sequences |
| R01/R13 | Header/config identities agree; reader views are allowed while raw quarantine bytes remain inaccessible to Grafana |
| R18/R28 | Continuous metrics show one working and one unavailable spool while chain scanning stays healthy; pending segments are visible; SIGTERM completes within the fixture bound |

An initial real database run exposed PostgreSQL type inference for an untyped
parameter inside `SELECT DISTINCT`. Explicit `bigint` casts fixed inventory
registration; the full suite passed thereafter. Local first-failure evidence is
`/tmp/eez-observability-lifecycle-database-first-failure.{log,json}` (not guaranteed
retention). Later review tightened unknown-schema gap classification and added
full-u64 and unread-segment backlog regressions; the final report includes those
changes. No production node, database or wallet was changed.

The [ingestion contract](../contracts/lifecycle-ingestion.md) describes bounds,
health versus coverage, immutable evidence and reviewed quarantine replay. Tests
cover the running importer; they do not establish native queue/attempt capture,
canonical outcome assurance, native restart ownership history, workload capacity,
physical-volume limits or the production baseline. Those remain explicit gates.
