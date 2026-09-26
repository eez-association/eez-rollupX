# Companion gas and canonical receipt coverage

**Status: implementation and validation in progress.**

Baseline checkout `20e81da05c33fd6e829477652fa4e52ea91a50a1` on
`feat/cross-chain-admission-throughput`; deployed runtime
`85f3691cb9c7ccbf8c0b8700576733283e5af7fc`; clean protocol
`9d13218a355e7d543ca8a9927eea2962600b716d`.
Scope/acceptance are in the [authorized proposal](../../proposals/cross-chain-admission-throughput.md).
Existing operator runbook edits and unrelated audit directories are preserved.

The [baseline verified wave](../2026-09-26-admission-deferred/rerun2/README.md)
completed 870 sources and 770 semantic checks. It records the 50 source-receipt
gaps, conservative reverted storage estimates and missing companion overhead.

Baseline regression reproduced with:

```bash
cargo +1.94.0 test --locked -p eez-composer --lib \
  reverted_flagged_storage_is_priced_with_bounded_payloads
```

One selected test failed because a bounded reverted entry received
`DeferredReverted` instead of a storage estimate. The unmodified estimator was
tested with only the regression added. The first failure is retained locally at
`artifacts/admission-companions/baseline-reverted-storage.log`; ignored artifacts
are unavailable in another checkout. No runtime changes have been deployed by
this follow-up yet.

## Implementation and local checks

Estimator `companion-storage-4` extends storage inventory/result bounds to 256
bytes, prices packed call flags and reverted/static nested hashes, and separates
companion simulation from a bounded protocol reserve or signed-limit fallback.
Final native gas/effect validation is unchanged. The monitoring collector binds
source receipts to the canonical posting block and order, including receipts
without registry logs; it does not add these sources to settlement cost totals.

- Composer: **256 passed**, one existing manual backlog timing profile ignored.
- Integration contracts: **62 passed**. Gas pins: **43 passed on each of
  Cancun, Prague and Osaka**, including reverted/flagged retained storage,
  256-byte inputs/results, cold callbacks, and 126 skipped queue entries.
- Indexer: **25 passed**, including all opt-in PostgreSQL/recovery cases against
  the task-owned `eez-admission-companions-test-db`, port 19439. No operator
  database was used for destructive tests.
- Workspace clippy and standalone indexer clippy passed with warnings denied.
  Rust formatting passed. The edited Solidity file passes its configured
  formatter; the full formatter retains unrelated pre-existing differences in
  `contracts/src/ECDSAProofSystem.sol` and `contracts/script/DeployRollup.s.sol`.
- Four native admission/settlement suites are building/running; image build,
  runtime deployment and the new live wave remain pending.

First failures are retained locally. An initial Rust test used `Default` for an
ABI type without that implementation; a later compile caught an argument wired
to the adjacent trace call while tightening companion evidence. Both were
corrected before the passing Composer run. The first indexer test invocation
could not bind mock RPC sockets under the sandbox; the network-enabled run used
the same locked dependencies and passed. These are not flaky assertion retries.

## Additive monitoring upgrade

The first atomic live schema application rejected a view column change from
`numeric(78,0)` to unconstrained `numeric`. PostgreSQL rolled back the transaction.
The expression now explicitly preserves the historical type; the database test
first creates the old typed projection and then applies the new schema. After
all 25 indexer tests passed again, the schema upgrade committed at
**2026-09-26 18:45:23 UTC**. Admission event/file-checkpoint counts remained
**24,080 / 3** before and after. This makes new mounted dashboard columns
queryable; the old collector/runtime continue running until the image upgrade.

Local evidence is under `artifacts/admission-companions/`: `composer-final.log`,
`gas-pins-{cancun,prague,osaka}.log`, `contracts-final.log`, `indexer-final.log`,
`clippy-final.log`, `indexer-clippy.log`, and `schema-upgrade.json`. Raw runtime
configuration and first schema diagnostics remain private under `data/`.
