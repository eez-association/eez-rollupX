# Chain writer and enrichment reference locking

A live native→observer run captured PostgreSQL `40P01`: sorted chain/cursor
writers using `FOR UPDATE` conflicted with cost-check foreign keys taking
`KEY SHARE` in L1/L2 role order. Concurrent registration can assign L2 the lower
database row ID, so role order and writer row order are not interchangeable.
The original three-process cycle is retained locally in
`/tmp/eez-native-observer-first-deadlock-postgres.log`.

Canonical scanner/finality/repair operations do not change chain identity keys.
Their chain-row locks now use `FOR NO KEY UPDATE`: canonical writers still
exclude one another, while independent foreign-key references remain compatible.
Cursor locks and transactional canonical/cursor updates are unchanged. Registration
updates at most one chain in its own transaction; its existing identity-key
upsert is not a transaction holding both chain rows.

The actual PostgreSQL regression `tests/chain_locks.py` executes the production
lock statement, assigns the lower ID to L2, and performs a real cost-check insert
with reversed context order. It first demonstrates the old `FOR UPDATE` blocks
the insert, then proves the fixed insert commits while the first writer still
holds its lock. A second canonical writer is positively observed blocked through
`pg_blocking_pids`, then proceeds only after the first commits. Predicate/pipe
barriers establish ordering; no successful outcome is inferred from a sleep.
The focused regression passed in `/tmp/eez-chain-lock-first.log`.

The rebuilt schema20 binary passed the full 63-group database suite, including
this lock regression (`/tmp/eez-observability-schema20-database.{json,log}`;
[combined evidence](database.md)). The live native integration is a separate
acceptance run. Earlier schema19 process passes predate this Rust change.
