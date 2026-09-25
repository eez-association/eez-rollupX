# RollupX two-byte V0 implementation validation

Revision notice: the evidence below validates the earlier integer-list
terminal/entry revision, not the subsequent byte-string and sizing simplifications.
Those preserve full contents and execution rules but change wire bytes and capacity
accounting. Follow-up evidence is recorded in [simplification validation](simplification.md).
The September 24 base/protocol update is tracked separately in the
[PR #34 update record](pr34-update.md).
Historical results and failures below are retained unchanged.

| Field | Value |
| --- | --- |
| Date / status | 2026-09-22; local validation complete with three recorded process-startup flakes |
| Branch | `feat/two-byte-da-v0` |
| Base | PR #34, `68fde5bb5b0f9743e55a5de98cdff682e9076a49`, plus the accompanying uncommitted implementation patch |
| Protocol | `3c0cdf4955941a5c23639346932b55b33babebbd`, initialized recursively, unchanged |
| Toolchain | Rust 1.94.0; Foundry 1.7.1 (`4072e48705af9d93e3c0f6e29e93b5e9a40caed8`) |
| Design | [Scope and wire contract](../../proposals/two-byte-da-v0.md) |

## Scope and review findings

The runtime accepts only RollupX V0 tag 0/profile 1. Ordinary transaction counts
use the agreed tokens, with exact coverage, maximal empty runs, shortest varints,
and a 16,383-transaction ceiling. The existing 65,536-total-block bound remains.
Environment/full-block overrides preserve the previous reconstruction inputs;
terminal, cross-chain entry and group encodings are retained. No draft execution
rules, inbound-only restrictions, candidate-profile-validator layer, dependency
upgrade or historical runtime decoder were added.

The change reaches the producer and follower through the existing shared codec
and sparse block-plan API. Range sizing combines empty runs across joins and
sizes metadata and derived transactions separately. Ordinary sequencing's
`BlockDaSizer` and terminal admission check the new transaction ceiling.
Capability discovery now agrees with the new operations tag.

### Authorization argument

Counts only assign ordinary positions and transaction-list lengths; they do not
authenticate execution. The decoder checks range coverage before allocating
records or transactions, checks full-block counts against their bodies, and maps
the columns into the existing validated sparse record model. Empty spans skip
directly between overrides during parsing; expansion remains the bounded lazy
block-plan iterator. No-op environments, malformed/nonexact transactions and
blocks, inconsistent full-block placement and unused transaction bytes fail.

The signer still compares each derived environment and exact transaction sequence,
or each complete full block, with independently validated reconstruction evidence.
Terminal system-transaction, sidecar, grouped/nested/static/rollback, native
semantic, KZG and public-input gates are unchanged. A new service regression
passes malformed count columns through the real RPC and checks zero signing
calls. Cached execution facts confer no authority to change the DA claim.
This is a local implementation review and regression argument, not an independent
security audit or a proof of correctness for every possible input.

## Evidence and executed checks

Working directory:
`/home/edu/Development/eez-rollup0-fork/target/worktrees/two-byte-da-v0`.
Commands use `cargo +1.94.0` and `--locked --offline`. The codec uses the worktree's
own target; broader builds use `CARGO_TARGET_DIR=/home/edu/Development/eez-rollup0-fork/target`.
Artifacts below are **local-only**, ignored, and unavailable in another checkout.

Recorded SHA-256 identifiers:

| Local artifact | SHA-256 |
| --- | --- |
| `rust-source.sha256` (tested sources, before the final crate-doc clarification) | `fbb6142fb74d2b4aaf8c13a6164d6a863e8cc6a053be701f8c787a169a9b7aaf` |
| `rust-source-final.sha256` (after the documentation-only clarification) | `cdd7d04c3eab30afcc28344300629cab0557734b0b7b17a650ea5f25bc0de5eb` |
| `codec-tests.log` | `19042f6cb3ac1b5abe58fd75fd156f135c022b2e1fbe831e7ef2a869dd3f59db` |
| `component-tests-final.log` | `a446ff83fd6f87e3fe3d1d438b9a6658331a29a9179e18f8b8dca268c26e1303` |
| `clippy-final.log` | `228d1040f17a981a00e2d7d1b7304a973f509ec471cc5b7297a4b6e1b6cb590e` |
| `process-tests.log` (first run, failed) | `a1bb6ebac0fc83d2cfefbb08691ce1f2f8a7c18673abd91cdd789c4e09f00bf5` |
| `process-cost-gas.log` | `22f587298a620c85916b7a956c824a390ddc6a6dc369ee8018d362499c2060de` |
| `process-retries.log` | `43970b2b4c131b2d2ab0f3e5994bbd46d4133a076aa68a1aeec549c9b0403507` |
| `runtime-binaries.sha256` (initial runtime snapshot) | `3bd5cd7a59f63f794a961e5db499063147a7d82e4d5e7a39000bc10d954a8c35` |
| `runtime-binaries-final.sha256` (runtime and policy-test identities after retries) | `f41d1c8b01ee97691b25567f728da7179fac20728cbe50b123abfa4874124e0f` |

Runtime binaries were rebuilt from unchanged sources by the source-based test
harness between commands; the two snapshots are not a claim that every process
used one identical binary hash. The final source manifest differs from the
tested source manifest only in the crate-level documentation clarification.

| Check | Result | Local evidence |
| --- | --- | --- |
| `cargo test -p eez-payload-codec` | 42 passed, 0 ignored | `artifacts/two-byte-da-v0/codec-tests.log` |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed, including final fixture edits | `artifacts/two-byte-da-v0/clippy-final.log` |
| `cargo fmt --all --check`, `git diff --check` | Passed | Command output |
| `forge build --root contracts` | Passed, existing Solidity lint warnings retained | `artifacts/two-byte-da-v0/contracts-build.log` |
| Affected component suites | 832 passed, 3 intentionally ignored | `artifacts/two-byte-da-v0/component-tests-final.log` |
| Six affected process suites, serial tests | 75 first-attempt passes, 3 startup failures, 3 intentionally ignored | `artifacts/two-byte-da-v0/process-tests.log`, `process-cost-gas.log` |
| Exact diagnostic retries of startup failures | 3 passed, 28 filtered out; original flaky result remains failed | `artifacts/two-byte-da-v0/process-retries.log` |

Codec coverage includes all 16,383 positive count values, u32 run boundaries,
truncation/noncanonical/overflow cases, adjacent empty-token rejection, exact
column coverage, full-record count mismatches, missing/extra transactions,
constructor/decoder symmetry, full and derived transaction ceilings, arbitrary
bytes and mutations, independently assembled wire vectors, dense-model replay,
associative sizing, and environment/full exceptions across run joins. The size
comparison against PR #34 is [recorded in the proposal](../../proposals/two-byte-da-v0.md#complete-payload-byte-comparison);
it measures deterministic codec bytes, not execution throughput.

Component command:

The passing run includes 319 signer, 221 composer, 56 driver, 30 prover-client,
6 RPC, 6 node-common, 167 protocol-unit, 5 independent public-input/rolling-hash
vector, 21 blob-unit and 1 Ethereum KZG compatibility test. The three exclusions
are `profile_blocked_backlog_discovery`, `inventory_native_proving_fixtures`, and
`cold_span_feasibility`, each explicitly manual. The 42 codec tests are additional.

```sh
cargo +1.94.0 test --locked --offline \
  -p eez-driver -p eez-composer -p eez-proof-signer -p eez-prover-client \
  -p eez-control-rpc -p eez-protocol -p eez-blob-codec -p eez-node-common
```

Process command (local socket/process permissions required):

```sh
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
export CARGO_TARGET_DIR=/home/edu/Development/eez-rollup0-fork/target
export EEZ_TEST_LOG_DIR="$PWD/artifacts/two-byte-da-v0/process-logs"
export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/two-byte-da-v0/process-datadirs"
cargo +1.94.0 test --locked --offline -p eez-node-e2e \
  --test derivable_operations --test dynamic_admission \
  --test settlement_policy --test settlement_policy_gas \
  --test settlement_policy_cost --test cross_chain -- --test-threads=1
```

The first command stopped after the failing policy target. The two remaining
targets were then run, with the same environment:

```sh
cargo +1.94.0 test --locked --offline -p eez-node-e2e \
  --test settlement_policy_cost --test settlement_policy_gas -- --test-threads=1
```

| Process target | First-attempt result |
| --- | --- |
| `cross_chain` | 22 passed |
| `derivable_operations` | 3 passed, 3 ignored |
| `dynamic_admission` | 16 passed |
| `settlement_policy` | 28 passed, 3 startup-port failures |
| `settlement_policy_cost` | 2 passed |
| `settlement_policy_gas` | 4 passed |

The three ignored process cases are `ethereum_native_large_history_cold_checkpoint`,
`gnosis_native_large_history_cold_checkpoint`, and
`retained_native_history_cold_checkpoint`; these require explicit long-history
or retained-data runs. Regular Ethereum/Gnosis independent blob replay passed,
as did the live 315-block, single-batch/single-blob replay case. Existing test
names containing `v3` are historical names: these binaries use the new V0 codec.

The runtime source remained unchanged throughout these process runs. No failing
assertions were weakened, and the port allocator was not changed as part of this
encoding implementation. After validation, only the codec's crate-level prose
was clarified to distinguish the new tag 0 from unsupported historical formats;
formatting and whitespace checks were repeated after that documentation edit.

Diagnostic retries used the same policy test executable, with the three exact
names selected (a prior `--list` confirmed three tests):

```sh
export EEZ_TEST_LOG_DIR="$PWD/artifacts/two-byte-da-v0/process-retry-logs"
export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/two-byte-da-v0/process-retry-datadirs"
/home/edu/Development/eez-rollup0-fork/target/debug/deps/settlement_policy-00aaf7fea633f000 \
  --test-threads=1 --exact \
  ethereum_fee_waits_do_not_consume_the_first_outbound_selection \
  interval_posts_accumulated_activity_using_the_registration_l1_timestamp \
  native_gnosis_always_settles_with_native_blob_fees_and_independent_replay
```

This exact executable is a local artifact, not a portable path. Runtime overrides
remained unset; the testkit built or checked its signer from source. All three
retries passed without source changes. Under the repository's
`flaky-result = "fail"` rule, the original aggregate remains failed; these retries
establish semantic coverage, not first-run reliability.

### Failures and corrections retained

- The first sandboxed component run could not bind localhost sockets. A focused
  signer test confirmed `PermissionDenied` at the test listener, not a DA failure.
  Subsequent component/process runs use approved local-test permissions.
- The old projection oracle's test-only tag 0 collided with the new runtime tag.
  It now uses test-only tag `0x80`; its independent wire assertion was updated.
- Immutable captured native requests use tag 3. Initial component runs rejected
  them, as required. A test-only, capture-specific adapter verifies original
  request hashes, original public-input digest and signature, retains original
  blocks/witnesses/effects, and repacks the ordinary recipe using V0. The new
  digest is independently recomputed from the new physical blobs. Captured files
  are not regenerated and shipping code does not accept historical operations.
  The lookup capture includes kind-0 transaction records; the adapter preserves
  those, rather than assuming every ordinary block is empty.
- An added sizing test initially changed its transaction list without changing
  the corresponding full-block fixture used by the conservative reserve oracle.
  Correcting that fixture restored agreement; no production bound was weakened.
- Shared-cache clippy initially used old codec metadata without `CountEncoding`.
  A package-scoped `cargo clean -p eez-payload-codec --profile dev` removed only
  74.4 MiB of regenerable build artifacts, then workspace clippy passed.
- The first process run's
  `ethereum_fee_waits_do_not_consume_the_first_outbound_selection` failed during
  embedded L1 startup, before its semantic assertions. The node could not bind
  WebSocket port 40972 (`Address already in use`, OS error 98). Its original log
  is `process-logs/eez-node-node-909186-22.log`. The same startup failure affected
  `interval_posts_accumulated_activity_using_the_registration_l1_timestamp`
  on WebSocket port 35998 (`process-logs/eez-node-node-909186-67.log`), and
  `native_gnosis_always_settles_with_native_blob_fees_and_independent_replay`
  on port 43218 (`process-logs/eez-node-node-909186-88.log`). All three diagnostic
  retries passed; they do not turn this first run into a clean pass. The
  existing testkit reserves HTTP/WebSocket pairs but releases them
  before child startup, so it cannot eliminate the subprocess bind race.

Earlier failure logs are retained as `component-tests.log`,
`component-tests-v0-fixtures.log`, and `clippy.log` in the same artifact directory.
The source corrections above are distinct from the explicitly reported,
unchanged-source diagnostic retries of the process-startup failures.

## Operational changes and remaining work

No deployment, live service restart, wallet operation, protocol pin change, PR
creation, push or migration was performed. The new branch/worktree is based on
PR #34, not the root checkout's older local main. Changes remain uncommitted.

Implementation and scoped local validation are complete, with the process
startup flakes retained as failures. A follow-up is to harden subprocess port
allocation; this encoding change does not claim to fix that race. Full
release-artifact CI, Kurtosis and live-network validation were not run and remain
unverified. Existing ignored calibration and inventory tests are not enabled as
general conformance tests. These results are not a claim that full CI is green.
