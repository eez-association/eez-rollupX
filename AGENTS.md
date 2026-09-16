# Agent instructions

Run commands from the repository root. This fork's behavior and the pinned
protocol revision are the review baseline; upstream `main` is not a substitute.

## Before changing code

```bash
git status --short
git remote -v
git rev-parse HEAD
git ls-tree HEAD eez-core-protocol
git submodule status --recursive
```

- For PR reviews and ports, record the PR base/head, intended fork target commit,
  and protocol gitlink. Trace the diff through production callers and affected
  tests; identify specification/implementation disagreements explicitly.
- Before protocol or E2E work, inspect submodule changes and initialize the pinned
  checkout when needed: `git submodule update --init --recursive eez-core-protocol`.
  Preserve existing edits; change the pin only within the task's scope. If an
  isolated worktree cannot initialize it, read the exact gitlink revision from
  an initialized checkout. `git -C eez-core-protocol rev-parse HEAD` inside an
  empty submodule directory can report the superproject's HEAD; it does not
  establish the protocol revision.
- Preserve dependency alignment in [Cargo.toml](Cargo.toml) and `Cargo.lock`:
  Reth, Stateless, reth_gnosis, REVM, and Alloy must remain compatible. Use
  `--locked`; do not upgrade dependencies incidentally to bypass a build failure.

For protocol changes, read the relevant **pinned core specification first**, then
its Solidity implementation/tests and the fork integration contract:

| Change | Required sources |
| --- | --- |
| Execution, ABI, cross-chain entries | [Core protocol](eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md), [execution entries](eez-core-protocol/docs/EXECUTION_ENTRY_SPEC.md), [static/reverted calls](eez-core-protocol/docs/STATIC_ENTRY.md), [caveats](eez-core-protocol/docs/CAVEATS.md) |
| Proof verification/public inputs | [Multi-prover specification](eez-core-protocol/docs/MULTI_PROVER_SPEC.md), pinned verifier implementations |
| Native blob encoding | [Blob format](eez-core-protocol/docs/blobs/BLOB_FORMAT_SPEC.md), [blob/table tools](eez-core-protocol/script/blob/README.md) |
| Composer/prover boundary | [RPC specification](crates/eez-control-rpc/SPEC.md), [signer specification](crates/eez-proof-signer/SPEC.md), [signer contributor guide](crates/eez-proof-signer/CONTRIBUTING.md) |
| Settlement scheduling, retries, queues | [Settlement policy](docs/configurable-settlement-policy.md), [queue requirements](docs/proposals/cross-chain-queue-liveness.md) |

## Implementation boundaries and invariants

- Keep ABI types, commitments, and shared protocol rules in `eez-protocol` and
  the codec crates; do not introduce dependencies from them to runtime roles.
  Keep role-neutral node bootstrap in `eez-node-common`, request scheduling and
  ownership in `eez-composer`, and runtime wiring in `eez-node`/`eez-follower`.
  The process/RPC contract belongs in `eez-control-rpc`; independent proof
  validation belongs in `eez-proof-signer`.
- Preserve **block-hash commitments**, native blob DA, complete grouped/nested
  calls, and the binding of proving and submission to the **same L1 target**.
  A task changing these contracts must update all affected producers/consumers:
  Solidity, Rust ABI/codecs, Composer, signer, and deriver/follower.
- The signer supports a narrower profile than the core protocol. Do not relax
  its checks merely because upstream supports another case. Preserve exact
  decoding/byte comparisons, bounded parsing of Composer-controlled input, and
  independent public-input recomputation. Reject unsupported input explicitly;
  truncation, skipped work reported as success, and unchecked runtime `unwrap`/
  `expect` can hide protocol divergence.
- Keep [contract compiler settings](contracts/foundry.toml) aligned with the pin:
  Solidity 0.8.34, optimizer runs 1, `via_ir`, and relative remappings. Compiler
  or metadata drift changes bytecode, CREATE2 addresses, and genesis commitments.
  Regenerate/check affected fixtures with `scripts/update-eezl2-genesis.sh`;
  deployment uses `--render`, while the default mode rewrites committed fixtures.
- For wire/configuration/storage changes, specify mixed-version behavior,
  historical decoding and follower replay, upgrade order, migrations, and
  rollback limits. Update affected specifications and operator configuration.
- Preserve policy-disabled settlement behavior. Separate scheduling triggers
  from gas, blob, and timing admission limits. Derive limits from network rules
  or measured budgets; retain final execution validation. An admission estimate
  is not a consensus limit. Only validated canonical commitment advancement is
  settlement progress.
- Account for every accepted request: canonical consumption, retention, requeue,
  or explicit terminal disposition. Preserve attempt identity, whole replay
  groups, and sender/direction nonce dependencies. **Partial consumption is
  supported**; one receipt, relay acceptance, or a matching height cannot establish
  completion of an entire attempt.
- Separate prover retryability from attribution. Retry statuses are limited to
  `UNAVAILABLE`, `DEADLINE_EXCEEDED`, and `ABORTED`, within the settlement cutoff;
  do not classify by error text. Validate actionable `FAILED_PRECONDITION` details
  against the exact request. Invalid hints follow bounded opaque-rejection handling,
  not targeted eviction. Preserve one charge per failed prover/relay episode,
  not per transport retry; `MAX_BUNDLE_ATTEMPTS = 3` evicts exhausted candidates
  and dependent same-sender/direction nonce suffixes, even if a candidate was valid.
- During submitted-attempt reconciliation, charge eligible users once only for a
  conclusive settlement failure. Infrastructure uncertainty and source-state/capacity
  deferrals retain work without charging that policy. Do not infer execution or
  eviction authority from an inconclusive chain observation; blocked sources must
  not starve eligible work.
- Pending ownership and retry counters are **in memory**. Restart reconstructs
  canonical state and discards the speculative suffix; unsettled clients may
  need to resubmit. Do not describe database persistence as durable queue recovery.

## Build and validation

Use Rust **1.94.0** (CI's toolchain; the manifest declares minimum 1.93), Foundry
**1.7.1**, and the pinned submodule. Native Rust builds require the system packages
listed in [CI](.github/workflows/ci.yml), including `libclang-dev`, `pkg-config`,
`libssl-dev`, `cmake`, and `protobuf-compiler`. Build workspace binaries with
`cargo +1.94.0 build --locked --workspace`.

Run the applicable checks below. Add a regression for changed behavior; do not
run runtime suites solely for prose edits. `make test` does not cover all CI lanes.

| Changed area | Expected checks |
| --- | --- |
| Rust code | `cargo +1.94.0 fmt --all --check`; `cargo +1.94.0 clippy --locked --workspace --all-targets -- -D warnings`; affected package tests (for example, `cargo +1.94.0 test --locked -p eez-composer --lib`) |
| ABI, commitments, blob codecs | `cargo +1.94.0 test --locked -p eez-protocol -p eez-blob-codec -p eez-payload-codec`; affected Solidity and process tests. Preserve independent `public_inputs_hash_vectors`, `rolling_hash_vectors`, and `ethereum_kzg_compatibility` oracles. |
| Signer or prover RPC | `cargo +1.94.0 test --locked -p eez-proof-signer -p eez-control-rpc`; signer contributor checks above; `cross_chain` process tests for rejection/status/signing changes |
| Settlement/admission | Composer unit tests; relevant `eez-node-e2e` targets: `settlement_policy`, `settlement_policy_gas`, `settlement_policy_cost`, `dynamic_admission` |
| Queue, partial consumption, reorg/restart | Relevant `eez-node-e2e` targets: `partial_consumption`, `queue_liveness`, `queue_reorg`, `resumption`; `cross_chain`/`chained_interstate` when group semantics change |
| Integration contracts | `forge fmt --check --root contracts`; `forge test --root contracts` |
| Protocol pin or core-contract integration | Core tests below, `forge build --root eez-core-protocol`, `bash scripts/ci/run-protocol-anvil.sh`, and affected fork/E2E checks |
| EEZL2 runtime, compiler settings, genesis | `bash scripts/update-eezl2-genesis.sh --check`, affected contract tests, and deployment verification in the [Kurtosis guide](testing/kurtosis/README.md) |
| Chiado/settlement configuration | `bash scripts/check-chiado-blob-config.sh`; `python3 scripts/check-settlement-policy-config.py`; affected runtime tests if parsing or behavior changes |
| CI tooling/workflows | `python3 -m unittest discover -s scripts/ci/tests -v`; exercise the changed producer/consumer path |
| Dependencies/lockfile | `cargo deny check advisories bans sources`; `cargo audit`; affected builds and compatibility tests |
| Documentation | `git diff --check`; resolve relative links at the protocol pin; run `bash -n` on edited shell examples/scripts |

Core tests use CI's explicit exclusions; report them as exclusions, not coverage:

```bash
(
  cd eez-core-protocol || exit
  forge test --no-match-path 'script/*' \
    --no-match-test 'test_Doc_OriginalValue_SeedInSetup|test_PostCost_Incremental'
)
```

Local source-built process tests (requires `forge` and `anvil`; clears overrides
that would otherwise select prebuilt, potentially stale executables):

```bash
(
  set -euo pipefail
  unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
  forge build --root contracts
  EEZ_TEST_LOG_DIR="$PWD/artifacts/native-e2e/logs" \
  EEZ_TEST_DATADIR_DIR="$PWD/artifacts/native-e2e/datadirs" \
  cargo +1.94.0 test --locked -p eez-node-e2e --test cross_chain -- --test-threads=1
)
```

Select the affected `--test` targets from the table. Run process suites serially;
[Nextest](.config/nextest.toml) also isolates `eez-node-e2e`. Do not add `--ignored`
indiscriminately: `kurtosis_policy_replay` requires a live enclave. Follow the
[Kurtosis guide](testing/kurtosis/README.md) for topology/DA/PBS integration and
export the intended `KURTOSIS_ENCLAVE` in every shell; lifecycle scripts otherwise
default to `eez-ci`, including destructive teardown.

CI consumes **verified, exact-revision runtime/test artifacts**. In that configured
CI environment, the execution entrypoints are:

```bash
bash scripts/ci/run-native-suite.sh embedded "$EEZ_NATIVE_TEST_DIR"
bash scripts/ci/run-native-suite.sh cross-chain "$EEZ_NATIVE_TEST_DIR"
```

Use the [runtime producer](.github/workflows/build-runtime.yml) and
[native runner](scripts/ci/native-tests.py) for artifact setup; these commands are
not standalone local builds. Keep the separate Composer unit archive: its Tokio
`test-util` feature changes dependencies of the shipping binaries. Do not rebuild
in consumer jobs or treat an arbitrary cached binary as the tested executable.

For failures, retain logs/datadirs and executable identity before cleanup. Reproduce
bug fixes at the requested baseline and repeat the same check after the change.
Require nonzero intended test selection; ignored-only runs do not validate a fix.
Rejection tests must reach the intended component and assert its specific error
or state transition. CI's Nextest profile retries twice with `flaky-result = "fail"`;
preserve first failures and report retry-only passes as flakes. Distinguish static,
mocked, process, and live evidence; state whether semantic assertions and independent
replay ran. Throughput or receipts alone
cannot establish them.

## Proposals, audits, and operations

| Record | When and what to write |
| --- | --- |
| [Proposal](docs/proposals/TEMPLATE.md): `docs/proposals/<topic>.md` | Before substantial design/behavior changes: scope, invariants, alternatives, acceptance checks. Mark draft/accepted status; a draft `MUST` is not an adopted contract. Existing task authorization applies; a proposal creates no approval gate. |
| [Audit](docs/audits/TEMPLATE.md): `docs/audits/YYYY-MM-DD-<topic>/README.md` | Investigations, substantial validation, executed operational changes: exact source/protocol/executable identity, commands, results, limits, and unmet criteria/next actions. |
| [Runbook](docs/runbooks/README.md): `docs/runbooks/<task>.md` | Repeatable operations: prerequisites, ordered commands, expected results, restart/recovery boundaries, verification status. Consult it before operating and add newly verified steps. |

Link requirements, implementation, and evidence in both directions. Preserve dated
failures/corrections; older results do not validate newer binaries. Separate
observations from hypotheses, and implementation from validation/deployment status.
Retain small sanitized inputs/results or a retrievable artifact location and digest;
label local-only, missing, or expired evidence. Ignored `artifacts/` paths are not
available to another checkout. Keep current behavior in the relevant specification
or `docs/<topic>.md`; link the authoritative procedure from README/AGENTS instead
of duplicating it. Small fixes/docs edits need no new proposal/audit pair.

For Chiado, use the [deployment runbook](docs/runbooks/chiado-deployment.md) within
the task's deployment/funding/recovery authorization. Worktrees do not isolate its
fixed container names, ports, databases, or shared wallets. Clean up only owned
resources; preserve deployment bindings and genesis on restart. The
[staged workload runner](scripts/run-chiado-staged-performance.sh) locks shared
wallets and uses `--no-verify`; respect its locking and do not report it as semantic
validation. Keep private keys, authenticated URLs, private hostnames, and bulk
logs/databases out of committed records.
