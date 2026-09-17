# Later lifecycle member source-receipt recovery

2026-09-17. This records schema-9 source-reference recovery, a prerequisite for
R10/R11/R19. It does not establish companion allocation, independent replay, or
canonical cross-chain outcome assurance.

The implementation expands the existing receipt dependency projection to explicit
scoped Source objects in readiness/terminal members, preparation/attempt members,
and reconciliation resolution members. An actual admission record is not required
for independently fetching a source receipt. Admission history remains an
admission-only view. Links retain producer/event identity, exact Source JSON and
configured chain context; complete-current-event joins retract disputed or changed
relationships without deleting independently verified receipt facts.

## Executed evidence

- A fresh isolated PostgreSQL container applied migrations 1–10 successfully.
  Migration 10 was the concurrent native-outcome projection snapshot available
  during this check, not evidence of its full acceptance.
- `tests/source-references.sql` passed using `eez_observer_reader`: all six eligible
  event kinds, duplicate members, absent legacy reconciliation details, empty
  members, bare-hash rejection and unsupported-event rejection.
- Existing `tests/admission-status.sql` passed after those migrations.
- `python3 -m py_compile observability/tests/smoke-database.py` and
  `git diff --check` passed.
- The disposable container used the repository's pinned PostgreSQL image and was
  removed after the run. No operator services or state were modified.

The local diagnostic log is
`/tmp/eez-observability-source-references-sql.log`.

## Runtime and upgrade checks

The `smoke-database.py` process case uses imported chunk records, the real observer
and deterministic RPC fixture. It checks missing-admission receipt recovery for all
five later-member event kinds, duplicate-source deduplication, a 71-reference
manifest progressing 64 then 71 links across two passes, unchanged selected spend,
independent source costs, exact source replay retraction, and transport-conflict
retraction. Existing tests continue to cover scoped identity/context fencing,
chain-isolated failures, fee gaps, interval boundaries, deadline recovery and
reorg/reinclusion.

The optional `--previous-v8-binary` lane preserves an actual existing admission link
through the explicit upgrade and checks that the old binary refuses a downgrade.
The saved local executable is `/tmp/eez-observer-before-source-references`, SHA-256
`cee2f10c83ddb74551e1a13659cde7506792189c86226d4efdc1a74a2a75249f`.
The coordinated schema-10 candidate passed all 47 observer package tests and the
observer build. Four local mock-RPC socket tests initially failed under sandbox
socket restrictions (43 passed); rerunning the same suite with socket access
passed all 47. The first disposable process run stopped at the new fixture's
projection-count assertion: the fixture incorrectly encoded a 65-member manifest
as a single chunk, exceeding the existing 6 KiB payload/16 KiB record bounds. The
fixture now emits bounded chunks and verifies all six events assembled before
asserting the 64-reference projection budget. No runtime checks were weakened.

The corrected complete suite passed all **45 check groups**, including the later
source references, saved-v4/v5/v6/v8 upgrades, existing reorg/accounting/importer
regressions, and the concurrently implemented native-coherence projection cases.
The latter still reports canonical assurance as unknown. The machine-readable
[report](source-references.json) records the exact candidate and migration hashes;
the candidate executable SHA-256 is
`f9ae02bf9753c4893cfb4ccc66ec081faf79bdba3803da77b556c4528fd4bf3c`.

Commands executed:

```bash
cargo +1.94.0 test --locked -p eez-observer
cargo +1.94.0 build --locked -p eez-observer
python3 observability/tests/smoke-database.py \
  --previous-v4-binary /tmp/eez-observer-before-source-receipts \
  --previous-v5-binary /tmp/eez-observer-before-protocol-logs \
  --previous-v6-binary /tmp/eez-observer-before-posting-rollups \
  --previous-v8-binary /tmp/eez-observer-before-source-references \
  --report /tmp/eez-observability-source-references-database-reviewed.json
```

Local logs retain the initial failures and corrected evidence under
`/tmp/eez-observability-source-references-{tests,tests-socket,build,build-final,database,database-reviewed}.log`.
Workspace formatting/Clippy and the final integrated revision remain the
coordinator's validation gate; this record does not claim those checks for later
concurrent edits.

See [the contract and requirement mapping](../contracts/source-receipts.md).
