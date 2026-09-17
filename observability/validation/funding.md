# Funding evidence validation

[Machine-readable process evidence](funding.json) retains the source hash and
acceptance groups.

The optional [funding contract](../contracts/funding.md) has two independent
sources: explicitly selected observer account balances and covered canonical
burn, and the actual Composer poster candidate's reserve check. A configured
account is never assumed to be the runtime signing account. Missing balances,
missing fee history and zero burn produce unknown runway rather than zero.

The observer uses EIP-1898 canonical block-hash balance queries, verifies chain
identity and the block again after SQL measurement, and sums complete canonical
wallet receipts over the configured window. Schema 15 adds a persisted canonical
revision: append-only progress preserves a still-canonical historical window;
orphaning or deleting canonical history retracts it. Runtime publication checks
that revision. SQL arithmetic decides the exact one-day funding boundary;
floating-point gauges are display values.

Native candidate instrumentation preserves existing fee arithmetic and RPC work.
A bounded generation token prevents an older asynchronous attempt from restoring
validity after a newer attempt starts. Disabled telemetry remains inactive;
unknown balance stays unknown. Native reserve failure and configured-account
runway have separate alerts and dashboard panels.

Executed checks:

- `cargo +1.94.0 test --locked -p eez-observer -p eez-telemetry -p eez-composer --features eez-telemetry/native`:
  Composer **240**, observer **75**, telemetry **40** passed. This includes exact
  full-width funding comparison, missing/stale/reorg evidence, generation
  interleavings, contention recovery, configuration and defaults.
- `python3 observability/tests/smoke-funding.py --binary target/debug/eez-observer --report /tmp/eez-funding-revision-process.json`:
  **two process acceptance groups passed**, cleanup verified, binary
  `723555148a6afa72fc00d58053a59cc5e1c6c6d9fbe1c6cd28b2a49a734b1099`.
  Real PostgreSQL and the actual observer exercised balances, positive burn,
  exact 24-hour boundary, canonical reorg, RPC failure, actual zero balance and
  incomplete fee history. Revision tests covered append/no-op, rollback,
  concurrent canonical changes, deletion and checked overflow.
- The full-stack funding alerts and exact PromQL checks are recorded in
  [protocol alert validation](protocol-alerts.md). Their metrics are synthetic;
  the process fixture above validates the actual producer.

The first standalone funding fixture passed on schema 14, but the subsequent
combined database run exposed cursor-phase starvation: requiring exact equality
with the latest cursor discarded an otherwise valid historical burn anchor on
every normal append. The failed combined run is preserved locally at
`/tmp/eez-observability-costs-funding-database.log`. Schema 15 fixes the semantic
fence using canonical revision; neither fixture block timing nor deadlines were
relaxed. The reviewed process evidence is at
`/tmp/eez-funding-revision-process.{json,log}` and the focused Rust output at
`/tmp/eez-funding-revision-generation-tests.log`.

The combined schema 16 database gate subsequently passed **59 acceptance groups**,
including the explicit live assertion that normal cursor advancement retains the
funding sample and upgrades from saved schemas 1/4/5/6/8/13/14. Evidence:
`/tmp/eez-observability-schema16-database-reviewed.{json,log}`, same binary hash as
above. The initial isolated snapshot run failed because a fixture source helper
had not been copied; that failure is preserved separately. This result does not
validate later migrations 17/18 or the pending joint dashboard run.

The fixture uses private synthetic RPC/account values and does not
establish production burn coverage, threshold calibration or account identity
matching with an operator's deployment.
