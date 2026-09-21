# Authenticated blob measurements

2026-09-17. Dirty implementation worktree based on
`5183f56acd0903a10ad085e97d03aef4070e82a3`, with the agreed main baseline and protocol
pin unchanged. This records R12 observer/codec work; it is not a live deployment,
archive-availability claim, independent source replay, or production overhead test.

## Implemented scope

Schema 12 records immutable ordered blob/carrier metadata and authenticated exact
byte measurements independently of receipts. The optional worker uses bounded
Beacon REST retrieval, existing KZG settings/helpers, the shared versioned parser,
monotonic retrieval duration and canonical cursor/inclusion rechecks. Receipt fees
survive missing/pruned/authentication gaps. Measured-subset SQL ratios preserve
reverts, coverage and null unknowns. Actual active allowance remains native
attempt evidence rather than an invented network constant.

`validated_blob_len` uses the existing decoder without retaining decoded messages.
It measures the original terminator position, preserving valid nonminimal varints
and zero payload bytes. Physical KZG authentication accepts non-EEZ blobs that can
share a Beacon block; only selected transaction hashes undergo EEZ packing and
stream checks. That mixed-traffic distinction was found during final review and
has a regression test.

## Executed evidence

- Final focused tests: **56 observer**, **24 blob-codec**, **15 payload-codec**, and
  **169 protocol** unit tests passed. Independent compatibility targets also passed:
  one Ethereum KZG oracle, three public-input vectors and two rolling-hash vectors.
  Empty binary/doc-test targets are not counted as additional coverage.
- Observer build passed at schema 13, including the separately reviewed canonical
  outcomes, JIT session configuration and reader metadata view. Binary SHA-256:
  `9de5c53e16545e8e72c5001eb3e8360ccdcf5adb63da9d36eea6b006a6c4f86e`.
- Initial schema-12 process suite passed **48 check groups**, including optional
  disabled behavior, authenticated full-blob/legacy retrieval, 404/410 distinctions,
  KZG failure/recovery, weighted utilization across one- and two-blob posts, reverts,
  missing receipt counts, restart/no-refetch, canonical reorgs and immutable facts.
  That earlier candidate did not contain the final duration/mixed-traffic changes;
  it is not used as evidence for them.
- Python fixture/helper syntax and `git diff --check` passed.

Commands:

```bash
cargo +1.94.0 test --locked -p eez-blob-codec -p eez-observer \
  -p eez-protocol -p eez-payload-codec
cargo +1.94.0 build --locked -p eez-observer
python3 observability/tests/smoke-database.py \
  --previous-v4-binary /tmp/eez-observer-before-source-receipts \
  --previous-v5-binary /tmp/eez-observer-before-protocol-logs \
  --previous-v6-binary /tmp/eez-observer-before-posting-rollups \
  --previous-v8-binary /tmp/eez-observer-before-source-references \
  --report /tmp/eez-observability-final-reader-database.json
```

The final schema-13 database suite passed **49 check groups**. Its
[machine-readable report](blob-measurements.json) records the exact binary and all
migration hashes. This final run verifies actual monotonic duration, source
identity failure/recovery, missing transaction metadata recovery and reader
metadata-access/privacy assertions alongside the earlier cases. Schema-12 SQL
SHA-256 is `7aac21922eefdacacb015321f66776acd0b2a1550f3bbf2dd89d262cba968183`;
schema-13 SQL SHA-256 is
`45cd1d96a2443a0984e5c2752b2856707cec34e8a6e9f2a5ce22a843b1cab2c3`.
No disposable resources remained after cleanup. The database suite was refreshed
after necessary schema-13 reader-budget hardening. Its fresh authenticated reader
assertions verify JIT off, a 10-second statement deadline, 5-second lock deadline
and 10-second idle-in-transaction deadline. Earlier schema-13 snapshots remain in
`/tmp/eez-observability-blobs-database-final.{json,log}`; the current report comes
from `/tmp/eez-observability-final-reader-database.{json,log}`.

## First failures and limitations

Initial local mock RPC/Beacon tests hit sandbox socket restrictions: 50 observer
tests passed and five socket fixtures failed to bind. The identical socket-enabled
run passed all 55 then-existing tests; the mixed-blob regression raised the final
count to 56. Targeted Clippy initially found two blob-worker style/conversion
warnings and two existing-in-this-slice outcome-render formatting warnings; fixes
are included, and the coordinator owns the final workspace Clippy check.

Local evidence is retained under `/tmp/eez-observability-blobs-*`, including the
initial failures, final reviewed tests, candidate build and both database reports.
The [synthetic carrier fixture](../tests/fixtures/blob-carriers.json) is reproducible
from the existing encoder/KZG implementation and contains no actual transaction
or proof. Its SHA-256 is
`820293de4c9f628efda1fe2ab75bab6a8b4c81d8cb50a4596bf7f0b46b0d5caf`.

The retained unsupported-carrier/version and resource-limit states are explicit
coverage gaps. Production historical availability, capacity/latency distributions
and operating budgets remain deployment/performance gates. Separately, the current
L1 derivation helper validates every fetched Beacon body as EEZ-packed before
selection; mixed-application derivation deserves its own regression/review. This
observer change does not modify that protocol runtime behavior.

See [the requirement/implementation/test mapping](../contracts/blob-measurements.md).
