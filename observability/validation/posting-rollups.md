# Posting verification-table validation

Source `9a95af0` implements the [schema-7 verification-table contract](../contracts/posting-rollups.md).
The [artifact report](posting-rollups.json) identifies the exact source, build base,
observer executable, unchanged main/protocol baselines, migration hashes and saved
schema-4/5/6 executables. Runtime source was tested before committing and was not
changed afterward.

| Requirement / invariant | Implementation | Evidence |
| --- | --- | --- |
| R11/R19 exact association prerequisite | Existing bounded ABI decoder retains the declared verification rollups with the calldata hash | Wire order, duplicates, empty list, uint64 maximum and canonical fixture round-trip tests |
| Unknown must not mean empty | Nullable typed array; only exactly decoded calls produce a list | Historical NULL remains NULL on unavailable calldata; empty decoded list is `{}`; trailing/invalid calldata produces no list |
| Decoding is separate from successful execution | Existing receipt success and method state retained in reader views | Reverted fixture retains its list and exact receipt costs without an application claim |
| R13 safe populated upgrade | Append-only migration 7; old-version refusal; existing four-row repair lane extended | Actual saved schema-6 executable creates decoded calls, new schema preserves them, unavailable lookup retries, restart fills exact list without fee changes |
| Immutable exact data | Existing fact guard extended to the typed verification array | Changed list rejected; full-width IDs preserved; null elements, out-of-range IDs and multidimensional arrays rejected |
| R09 restart/reorg compatibility | Metadata belongs to logical transaction; views follow canonical receipt flags | Repeated process invocations expose one row; fork/reinclusion changes canonical fee from 591216 to 657216 wei without changing the table |

**37 observer tests** and **40 isolated PostgreSQL/RPC check groups** passed.
Workspace all-target Clippy with `-D warnings`, formatting, Python syntax, diff
whitespace and documentation-link checks passed. The full database matrix includes
all previously recorded canonical, lifecycle, log-conflict and source-receipt
regressions. The initial matrix passed; the final matrix additionally checked the
new array/domain constraints. No failing tests or lint diagnostics occurred in
this slice, and no existing tests were weakened.

Reproduce with Rust 1.94.0, local socket access and Docker:

```bash
cargo +1.94.0 test --locked -p eez-observer --lib
cargo +1.94.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.94.0 fmt --all --check
cargo +1.94.0 build --locked -p eez-observer
python3 observability/tests/smoke-database.py \
  --binary target/debug/eez-observer \
  --report /tmp/posting-rollups-database.json
```

`--previous-v6-binary` runs the actual older producer before upgrade and its
refusal afterward; without that option those saved-executable checks are not
counted. Earlier v4/v5 options remain supported. The hexadecimal fixture was
produced by the pinned protocol encoder and is checked byte-for-byte in the Rust
test; it tests ABI evidence, not a valid on-chain proof submission.

Logs are local-only under `/tmp/eez-observability-posting-rollups-*`, especially
`-tests.log`, `-clippy.log`, `-build.log` and `-database-final.{json,log}`.
Docker tests use isolated resources and verify cleanup. No deployment, protocol
state, native producer, dependency version or signed format changed. Native E2E
was not rerun for this observer-only metadata change. Full R19 outcome validation
and the other open [release gates](../requirements.md) remain required.
