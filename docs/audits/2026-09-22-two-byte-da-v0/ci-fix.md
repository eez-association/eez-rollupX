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
