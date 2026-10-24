# Pending Ethereum simulation correction

Source: `3e23955dc477c9e675364776f118b1cecda75774` plus the local patch in
`codex/l1-history-offline-proving`; core remains
`84a5b5207880f192a6a5896cbfe0bac5249db47f`. The earlier frozen source binaries and
live images are the **baseline**, not evidence for this correction.

## Reproduced failure and adaptation

The [portable live baseline](pending-rpc-baseline.json) records Counter creation
estimates on a stable canonical parent: default/latest succeeded, while explicit
pending rejected the scheduled history-import position. Forge 1.7.1 uses Alloy's
pending estimation path. `latest` is a valid committed-state control, not a fix
for pending semantics. The later protocol network run encountered the same
rejection while funding its faucet, before its scenario suite could start.

`eez-node-common/src/rpc/pending.rs` adapts only the three public Ethereum
single-transaction simulation methods. It executes the existing block builder's
real pre-application prefix against an exact L2 parent and a frozen checked L1
header snapshot, then supplies that state/environment to Reth's call, gas
estimation and access-list algorithms. An already executed pending payload is
delegated to Reth. No history rules, estimation search or access-list inspector
are copied. Registration respects the configured HTTP/WS/IPC `eth` namespace.
Engine auth's separate `eth_call` and other simulation/debug paths retain their
existing environment-only rejection. Testkit's L2 provider and the affected
Kurtosis application/protocol scripts use public HTTP; auth ports serve Engine
and consensus connections. No new method is exposed on the Engine endpoint.

Preparation and application execution use Reth's blocking executor and one RPC
semaphore permit, retained by the worker through completion even if the request
is dropped. Overrides are applied after the valid prefix with Reth's existing
counterfactual semantics. There is no persisted synthetic block or pool queue.
The owning contract is [L1_HISTORY_SPEC](../../../contracts/L1_HISTORY_SPEC.md#ethereum-rpc-simulation).

## Dependencies and validation identity

The only lockfile change for this correction is the direct
`eez-node-common -> reth-tasks` edge; that dependency was already fixed and used
transitively. `eez-protocol` moves from dev-dependency to normal dependency in the
same package. No external version, checksum, source or public validation identity
changes. The conservative local lockfile test changes its keccak from
`6250a73d002bc15d5dc3724ba268ebcab84c5dd2d64d9f70c46ba2c10cd3327e` to
`ee4e2fdddf84af928f69b8c730783d67c5edf4836a03ca6bee6cb2e72fb44071`.
Removing that one lock edge reproduces the earlier digest byte-for-byte.

A manifest-only baseline copy restores the prior dependency declarations, with
source directories linked to this worktree. Running the following in both trees
produces **909 identical external package/feature rows**, no additions/removals:

```sh
cargo +1.95.0 tree --locked --offline \
  -p eez-node -p eez-follower -p eez-proof-signer -p eez-protocol -p eez-l1 \
  --edges normal,build --prefix none --format '{p} [{f}]'
```

This covers normal/build features for the shipping package selection; it does
not replace the release producer's exact-artifact/archive checks. Full trees and
comparison JSON are local-only under `artifacts/pr-readiness/pending-rpc/`.

## Validation in progress

Commands use Rust 1.95.0, `--locked --offline`, LLVM 22 and a private `/tmp` mounted
with `bwrap`. Unit/check runs use two Cargo jobs; the process build now uses one
after the resource incident below. No baseline binary is overwritten.

- Initial `cargo check -p eez-node-common --features l1-startup` passed after
  correcting adapter types. First RPC unit compilation exposed four fixture
  type errors; first executable run passed 9/10 and rejected a probe transaction
  with its default chain ID instead of the fixture's ID. These development
  failures are retained in the local logs, not counted as successful tests.
- `cargo test -p eez-node-common --lib`: **15 passed**, default features.
- `cargo test -p eez-node-common -p eez-control-rpc
  --features eez-node-common/l1-startup --lib -- --test-threads=2`:
  **31 node-common and 7 control-rpc passed**, including the added no-update,
  EIP-2935 and cleanup assertions and the validation-identity guard.
- `cargo clippy -p eez-node-common -p eez-control-rpc
  --features eez-node-common/l1-startup --all-targets -- -D warnings`: **passed**.
  The first check requested `expect_err` instead of `err().expect` in a test;
  that style correction and the first diagnostic are preserved.

The first `cargo test -p eez-node-e2e --test node_e2e --no-run` was terminated by
a [kernel-confirmed global OOM](pending-rpc-build-incident.json) while host and
Docker builds overlapped. It never ran a candidate test. The Docker owner
cancelled its build; the repeated host build is serial, requires at least 12 GiB
available at launch, and cancels its own process group if availability falls
below 5 GiB. No unrelated services were restarted. This is a build-resource
failure, not a successful test or a flaky application test.

The serial build completed with Cargo's `build-finished: success=true`. Its
initial monitor exited on TERM while the child Cargo continued; the replacement
monitor retained the memory guard through completion. The original monitor's
exit is not presented as Cargo's exit status. The three resulting executables
are retained under `artifacts/pr-readiness/pending-rpc/executables/`, with source
and binary hashes in `source-freeze.json` and `candidate-executables.json`.

The first process invocation selected exactly one test with one RPC blocking
permit and the standalone signer in a network namespace without external
network access. It **failed in 7.31 seconds**: observation 27 called the nonexistent
`nextCutoff()` getter. The contract stores `nextCutoff` internally at slot 3, so
its unknown selector correctly reverted (RPC error 3). Startup reached L2 block
2 and the signer validated blocks 1 and 2; this was not a transport failure or
the old missing-prefix rejection. The corrected test reads slot 3 using a
state override of reader code, applied after the real prefix, and retains both
cutoff assertions. Production, contract ABI/runtime and genesis remain identical.
The [first-failure record](pending-rpc-process-first-failure.json) retains this
test defect; it is not counted as a flaky application pass. Rebuilding only the
test executable waits for the separate shipping build to release the host.

The process regression retains a stable-parent check, tests advancing history,
actual PREVRANDAO, `nextCutoff`, named/positional parameters, latest/hash controls,
Counter creation, application revert propagation and explicit diagnostic
overrides. RPC errors panic immediately; only a changed parent permits another
observation. Unit coverage includes repeat imports and malformed parent history,
and preserves the test proving that an environment without executed history is
invalid. Corrected process results will be appended after execution;
no live or full-suite candidate pass is asserted here.
