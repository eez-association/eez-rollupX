# V0 sequencing-size CI regression — 2026-09-25

Baseline: PR #37 commit `f2c59b6354df2acaba8079f8bd780a0670dc33b0`,
protocol `9d13218a355e7d543ca8a9927eea2962600b716d` (unchanged).

## Failure and correction

[CI run 36108289606](https://github.com/eduadiez/rollupX/actions/runs/36108289606)
failed three settlement-policy process tests:

- Ethereum and Gnosis ordinary bursts repeatedly failed the completed-payload
  size invariant and timed out. The estimator used V0 byte strings, but cached,
  completed and empty payload checks still called RLP `length()` on `Vec<u8>`:
  the historical integer-list wrapper. A single local helper now measures the
  byte-string wrapper at all three sites. Limits and deferral rules are unchanged.
- The oversized-transaction test expected 70,000 bytes of calldata to exceed one
  blob. That assumption became false with V0. Its calldata now fills the entire
  usable blob capacity before transaction/framing overhead, with sufficient gas
  to reach the DA guard. The existing specific rejection and nonce assertions
  remain mandatory.

The burst count now derives from the tested network's blob capacity, so calldata
alone overflows one interval (11 transactions on Ethereum, 4 on Gnosis, each
70,000 bytes). Its interval oracle serializes a valid all-full V0 payload rather
than the unsupported historical integer-list shape. Boundary, posting-policy,
nonce, settlement and independent follower assertions remain in place.

The original run passed lint, release unit tests, all cross-chain shards,
Kurtosis and two embedded shards. Its failed diagnostic uploads hit GitHub's
artifact quota; that separate infrastructure issue is not changed by this fix.
Raw failed-job logs are retained locally, not committed.

## Local validation

The new payload-size regression failed with the old calculation: an empty block
measured 591 rather than 504 bytes. After the fix, 7 node-common and 43 codec tests
passed. The test covers empty/nonempty blocks, low/high byte values, RLP length
boundaries, the old 70 KB fixture and blob-sized calldata. An initial test-helper
compile error (sealed versus unsealed block type) was corrected before this
red/green comparison; its log is retained separately.

The first E2E build also caught a test-helper type mismatch (`Vec<u8>` versus
`Bytes` in `FullBlock`), corrected before any process test executed. Those
compile-only failures are retained, not counted as runtime test retries.

Commands use Rust 1.94.0, `--locked --offline`, and the shared root `target`
directory. Process checks clear prebuilt/runtime/signer overrides, build the
integration contracts, retain child logs/datadirs, and run serially:

```bash
cargo +1.94.0 test --locked --offline -p eez-node-common -p eez-payload-codec
cargo +1.94.0 test --locked --offline -p eez-composer --lib
cargo +1.94.0 clippy --locked --offline --workspace --all-targets -- -D warnings
cargo +1.94.0 fmt --all --check
git diff --check
forge build --root contracts
cargo +1.94.0 build --locked --offline -p eez-node -p eez-follower -p eez-proof-signer --bins
cargo +1.94.0 test --locked --offline -p eez-node-e2e --test settlement_policy ordinary_ -- --test-threads=1
```

Node-common/codec: **50 passed**. Composer: **233 passed**, with the existing
`profile_blocked_backlog_discovery` manual timing profile ignored. Workspace
clippy with warnings denied, formatting, diff whitespace, runtime build and
integration-contract build passed. Foundry emitted existing dependency/typecast
warnings; no contracts or generated fixtures changed.

Process tests: **4 passed, 0 failed, 0 ignored, 27 filtered out**, in 636.59 s
(including harness dependency builds/lock waits). All three CI failures and
`ordinary_data_admission_tracks_operator_capacity_without_losing_valid_work`
passed on their first execution with the corrected source. Both network bursts
verified interval bounds, scheduled settlement, exact independent follower
replay, and subsequent cross-chain progress. The operator-capacity test also
verified restart and replay. The rejection test reached the DA-specific guard,
preserved the nonce, and settled a subsequent valid transaction. Ethereum
retried stale-target preparation within the original timeout; no retry rules,
timeouts or safety checks were changed.

Logs/datadirs are local-only under `artifacts/two-byte-da-v0-ci-fix/`, unavailable
from a fresh checkout. SHA-256 identities:

| Evidence | SHA-256 |
| --- | --- |
| `tested-source-final.sha256` (both modified Rust files) | `863e266f8debc1d538d9b2dcb7920b1bad544bd2c5b7aafdc9d556e682b0c810` |
| `process-binaries-final.sha256` (composer, follower, signer, test executable) | `4085182bb522fd3781aa53340fd36fdc6ab46128d3b89b946110a7e878cb9203` |
| `regression-before-assertion.log` | `dff2694294e9e534bf4ed0b6c15231004d19d13d1f1128909af341ec93dcbfa2` |
| `unit-tests.log` | `72d0db094df09f952378d62a756381918cac0f2ffae708deee8346abfb0a70ca` |
| `composer-tests.log` | `9a9d6eb6273af8684daebc3f0bdd15ffd17a6be88c54716d087e020d804159ff` |
| `clippy-final.log` | `c1d55100559ce8f46561044391a933db0bec23acfb65b1e486ec3d8f243a164a` |
| `ordinary-process-tests-final.log` | `dd9091b276795fc3a6840d9eebda1d69c79803eafccc54b77753deb8a1b0d3f9` |

These scoped checks do not claim a full release-artifact CI rerun, Kurtosis run,
or deployment validation. No live services or operator state were modified.

## Follow-up: RPC availability is not sequencing readiness

[Run 36124570516](https://github.com/eduadiez/rollupX/actions/runs/36124570516),
head `d7f699c59d2ce07d81a3e0b7163c9124d5dc7129`, passed the three original
capacity regressions. Embedded shard 3 instead failed
`node_fixture_ignores_parent_dotenv`: its first transaction reached RPC before
the composer bound `SequencingDataGuard`. The pool correctly returned
`sequencing data limits unavailable before composer readiness`.
The base remains `f128b6d9bfba7f9edb251490f4a3a01b918c753e`; protocol pin unchanged.

`NodeHandle::start_with_datadir` promises only a responding L2 RPC. In production,
that RPC starts before guard binding, canonical catch-up, bootstrap validation
and sequencer startup. The test now waits for height 1 before its single transfer.
This is a fresh, unposted fixture: there is no historical block to replay, and
the sequencer cannot produce its first block before those readiness gates.
This condition is specific to this fixture, not a general restart-readiness API.
No sleep, transaction retry, timeout increase or production gate change was added.
The invalid-rollup test additionally asserts that RPC remains at height 0 after
five L1 blocks. The shared startup helper stays usable for negative startup tests.

The unmodified test passed one Cargo invocation and five additional fresh-process
runs locally. Therefore the release-CI race was **not reproduced locally**; those
passes do not negate its recorded failure. The original log is retained under
`artifacts/ci-36124570516/embedded-shard-3.log`, SHA-256
`3001b843c8f61f49a5657dd05a8ccc32086cf2b6eb98c9b51200d0e9772b56d9`.
CI diagnostic artifacts were unavailable because the upload hit the storage quota.
All local evidence is ignored/local-only, not available from a fresh checkout.

The final CI result has no other test failures: lint, release build/unit tests,
the other three embedded shards, all four cross-chain shards and Kurtosis
passed. `CI Result` failed because the embedded shard failed.

Local correction validation uses Rust 1.94.0, locked/offline Cargo, the shared
root target directory, cleared prebuilt/runtime/signer overrides, rebuilt
integration contracts and retained per-run diagnostics. The complete startup
target passed **3/3**, including the strengthened wrong-rollup assertion:

```bash
cargo +1.94.0 test --locked --offline -p eez-node-e2e --test e2e -- --test-threads=1
cargo +1.94.0 clippy --locked --offline --workspace --all-targets -- -D warnings
cargo +1.94.0 fmt --all --check
git diff --check
```

Workspace clippy, formatting and whitespace checks passed. Startup-suite log
SHA-256: `4d3c3ea905b8473da26c639222773fce8b5b0bb688a7e06aba5ed05b7762c268`;
clippy log: `3837f1185577b8bef2b041f8c27f6e970291c45bf5e8820893cdb167c83c9c56`.
`fixed-identity.sha256` captures the modified test source, startup test executable,
composer and signer after the suite; its SHA-256 is
`efe39a6e92427b63f03cfe47007037bbb54420418aa738fa271e52ef1d005c9f`.

Five additional fresh-process executions of that same test executable with
`node_fixture_ignores_parent_dotenv --exact --test-threads=1` all passed: **5/5**,
zero failures or ignored tests (two unrelated tests filtered per invocation).
The shell stopped on any failure; these were repetitions, not retry-until-green.
Each had a distinct diagnostic directory and retained the default admission
guard. Source/executable identities still matched after all repetitions.
`repeated-results.sha256` identifies the five logs; its SHA-256 is
`0aacff05bd6c782abfc2ffa13b2f7a50c349de205aec783b1788fc31cbbb7994`.

This removes the observed pre-binding submission race by enforcing startup
ordering, rather than claiming statistical certainty from successful reruns.
The corrected source has not run the full release CI matrix or been deployed.
No production source, dependencies, protocol pin, live services or operator state
changed. Validation was completed before committing; the user subsequently
approved committing and pushing the correction to PR #37.
