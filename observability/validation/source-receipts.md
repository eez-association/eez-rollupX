# Source receipt enrichment validation

Source `48c748b` implements the [schema-5 source receipt contract](../contracts/source-receipts.md).
The [artifact/check report](source-receipts.json) records the exact observer
binary, prior schema-4 binary, migration checksums and 34 database check groups.
This is isolated fixture evidence, not production or full WP4 acceptance.

| Requirement | Implementation | Observable checks |
| --- | --- | --- |
| Preserve original configured spend | Selected-transaction membership plus shared immutable receipt facts | Upgrade preserves old totals; unselected source receipt costs appear separately while selected/wallet spend stays 147,000 fixture units |
| Never multiply a receipt by its admissions | One request per chain/hash; source receipt views use `EXISTS` | Repeated admissions retain distinct links but yield one canonical receipt and one charge |
| Keep missing data explicit | Separate lookup states and nullable accounting fields | Null receipt is unobserved with zero lookup failures; missing blob price creates an accounting gap; recovery fills exact charges without changing selected coverage |
| Validate exact source inclusion | Source scope match, block-body hash at transaction position, receipt/transaction checks, cursor recheck and lock | Wrong source genesis creates no request; wrong block-body membership is rejected; normal receipt parser keeps its earlier type-error precedence |
| Handle scoped/replayed relationships | Immutable source-object and configured-context links; current-event agreement in views | A wrong rollup projection cannot poison the correct context; conflicting admission links retract without deleting independently fetched receipt facts |
| Preserve reorg/reinclusion history | Shared canonical flags, immutable inclusion guards and cursor transaction | Orphaned unselected receipt retracts; reinclusion retains two observations and one canonical charge |
| Stay inside the declared interval | Existing bootstrap/cursor checks; no historical block insertion | A source before bootstrap reports the boundary and adds no outside block or receipt |
| Isolate failures and bound work | Per-chain health/backoff, shared RPC semaphore, 30-second chain deadline | Injected L1 failure and deliberately slow L1 lookup both allow L2 progress; the next pass recovers remaining L1 fees without duplicates |
| Separate receipt truth from settlement assurance | Explicit evidence class and nullable verified L2 settlement/finality flags | Valid L2 receipt remains receipt-verified while missing applied-range evidence stays unknown |
| Explicit safe schema upgrade | Schema 5 migration and old-version refusal | Saved schema-4 executable upgrades a populated v1 fixture to v4; current migration preserves its fees/lifecycle record and the old executable refuses downgrade |

30 observer tests passed, including source identity/freshness/chain isolation and
shared-parser compatibility regressions. Workspace all-target Clippy passed with
`-D warnings`; formatting, diff whitespace, Python syntax, local documentation
links and shell examples passed. The Docker database suite passed 34 named checks,
including all pre-existing canonical, lifecycle, quarantine, crash/replay and
reader-permission checks. Empty binary/doc-test targets are not additional tests.

The first observer run hit the restricted sandbox in three existing socket tests;
the authorized socket-enabled run passed. The first focused lint found an
unnecessary mutable database argument, which was removed. Review subsequently
separated ordinary unobserved receipts from RPC errors and preserved existing
receipt-parser error precedence; the final unit/database gates ran after those
changes. Earlier 31/33-check database passes do not substitute for the final run.

Reproduce from the implementation worktree with Rust 1.94.0 and Docker:

```bash
cargo +1.94.0 test --locked -p eez-observer
cargo +1.94.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.94.0 build --locked -p eez-observer
python3 observability/tests/smoke-database.py \
  --binary target/debug/eez-observer \
  --report /tmp/source-receipts-database.json
```

The optional `--previous-v4-binary` exercises the saved executable's upgrade/refusal
path; its hash is in the report. The suite otherwise constructs the v1 history
explicitly and checks forward migration. Docker resources have unique test names
and cleanup checks; no live service, operator configuration, wallet or chain data
was changed. Logs are local-only under `/tmp/eez-observability-source-receipts-*`,
including `-tests.log` and `-clippy.log` first failures and the final
`-reviewed-tests.log`, `-reviewed-clippy.log`, `-reviewed-build.log` and
`-database-reviewed.log`. No retention is promised.

No dependency version, signed protocol format or native binary changed in this
observer slice. Native source/process results remain tied to their
[separate local-disposition checkpoint](native-local-dispositions.md). Full
canonical outcome validation, restart status, broader lifecycle reference recovery,
companion allocation, native metrics/traces, clean-host restore and rollout
calibration remain open in the [requirement ledger](../requirements.md).
