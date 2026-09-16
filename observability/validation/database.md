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
