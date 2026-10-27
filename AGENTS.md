# Agent instructions

Run commands from the repository root unless stated otherwise. Review this fork
and its pinned protocol revision; upstream `main` is not a substitute.

## Scope and shared resources

- Inspect `git status --short` before editing; preserve existing work and operator
  configuration/state. Keep implementation, refactoring, and cleanup within the
  requested scope.
- Prefer existing Cargo, Foundry, Docker Compose, `gh`, `jq`, and shell commands,
  using the repository's scripts and configuration. Inline Python is appropriate
  for structured-data checks when clearer or less error-prone than an equivalent
  command or pipeline. Add a persistent script or abstraction only for repeated,
  error-prone work or a concrete gap in existing tools; state the reason. Simple
  configuration does not need extra selectors, lock formats, or execution modes.
- Deploy, fund wallets, restart services, or recover data only within the task's
  authorization. Reading a runbook does not authorize those operations.
- Worktrees do not isolate container names, ports, databases, or wallets. Identify
  existing resources before lifecycle operations and clean up only owned resources.
  Chiado uses fixed container names and host ports; never share its writable datadirs.
- For commands that use `KURTOSIS_ENCLAVE` to select a Kurtosis enclave, export
  the intended value in each shell that runs them; choose an unused name for a
  new enclave. Ordinary Chiado Docker Compose operations do not require this
  variable. Kurtosis scripts default to `eez-ci`, including destructive
  `testing/kurtosis/stop.sh`. Follow the
  [Kurtosis guide](testing/kurtosis/README.md).
- Concurrent protocol Anvil suites need disjoint `BASE_PORT` ranges: the
  [runner](scripts/ci/run-protocol-anvil.sh) allocates two ports per scenario.
- Respect the [staged Chiado runner](scripts/run-chiado-staged-performance.sh)'s
  wallet lock; do not bypass it with the upstream runner. The lock is per protocol
  checkout, so separate worktrees must still coordinate use of shared wallets.
  Its default `--no-verify` workload measures execution/throughput, not semantic
  correctness. Use its `--verify` flag when semantic assertions are required.
  For sustained mixed-load canary checks, follow the
  [Chiado validation procedure](docs/runbooks/chiado-mixed-validation.md);
  its driver holds both known checkout locks and requires explicit test funding.
- Keep private keys, authenticated URLs, private hostnames, and bulk logs/databases
  out of committed records. Do not treat public fixture/workload keys as private
  operator credentials. Published release images must support anonymous pulls;
  do not add authentication to public setup instructions. Follow the
  [public release runbook](docs/runbooks/public-release.md) for the separate
  repository and package visibility changes.

## Establish the baseline and read the applicable contract

Before code changes or PR/port reviews, identify the checkout and protocol pin:

```bash
git remote -v
git rev-parse HEAD
git ls-tree HEAD eez-core-protocol
git submodule status --recursive
```

For reviews/ports, record the PR base/head, target fork commit, and protocol gitlink.
Trace changes through production callers and affected tests. Normative specifications
define the accepted contract; implementation and tests establish current behavior.
Report disagreements instead of silently choosing one. Check proposal status:
a draft `MUST` does not establish acceptance.

Before protocol inspection, Solidity builds, or source-based E2E work, inspect
submodule edits and initialize the pinned checkout when needed:
`git submodule update --init --recursive eez-core-protocol`. Preserve edits and
change the pin only within scope. If initialization is unavailable, inspect the
exact gitlink revision in another initialized checkout and report remaining gaps.
`git -C eez-core-protocol rev-parse HEAD` inside an empty submodule directory can
report the superproject's HEAD; it does not prove the protocol revision.

Preserve Reth, Stateless, reth_gnosis, REVM, and Alloy compatibility in
[Cargo.toml](Cargo.toml) and `Cargo.lock`. Use `--locked`; do not upgrade dependencies
incidentally to bypass a build failure.
The [vendored Reth packages](crates/vendor/reth/README.md) retain v2.7.0 with
recorded shutdown and shared-RPC-quota patches. Review those diffs when changing
dependencies; preserve cleanup ordering and the aggregate RPC quota, and do not
edit Cargo's cache.

For protocol changes, read the relevant **pinned core specification first**, then
its Solidity implementation/tests and the applicable fork contract:

| Change | Required sources |
| --- | --- |
| Execution, ABI, cross-chain entries | [Core protocol](eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md), [execution entries](eez-core-protocol/docs/EXECUTION_ENTRY_SPEC.md), [static/reverted calls](eez-core-protocol/docs/STATIC_ENTRY.md), [caveats](eez-core-protocol/docs/CAVEATS.md) |
| Proof verification/public inputs | [Multi-prover specification](eez-core-protocol/docs/MULTI_PROVER_SPEC.md), pinned verifier implementations |
| Native blob encoding | [Blob format](eez-core-protocol/docs/blobs/BLOB_FORMAT_SPEC.md), [blob/table tools](eez-core-protocol/script/blob/README.md) |
| Composer/prover boundary | [RPC specification](crates/eez-control-rpc/SPEC.md), [signer specification](crates/eez-proof-signer/SPEC.md), [signer contributor guide](crates/eez-proof-signer/CONTRIBUTING.md) |
| Settlement, retries, queues, reorg/restart | [Settlement policy](docs/configurable-settlement-policy.md), RPC [failure handling](crates/eez-control-rpc/SPEC.md#6-failure-handling) and [settlement ownership](crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission), [queue requirements and status](docs/proposals/cross-chain-queue-liveness.md) |

## Implementation boundaries and invariants

- The [2026-10-23 branch amendment](docs/proposals/l1-origin-and-offline-proving.md#accepted-branch-scope-after-divergence-review)
  is the owner-selected fresh-network profile: native `0x45`, reserved system sender, source identity
  without a system-account nonce, and ordinary base fees credited to a collector.
  The [exact native/profile specification](docs/proposals/native-system-transactions.md)
  owns E1/E2; the [native-v1 audit](docs/audits/2026-10-24-native-v1/README.md)
  tracks implementation and E3 evidence. There is one version-1 native profile;
  do not add fallback execution, migration or old-format validation paths.
  Preserve history, execution and ownership invariants and keep owning runtime
  specifications, identities and fixtures consistent.

- The fresh-network history update is transaction zero at its scheduled position:
  native `0x45` version 1, zero value, history predeploy destination, authenticated
  source identity, no system-account nonce and a normal metered receipt. It belongs to public V0 ordinary data, not cross-chain framing.
  Replay validates its recorded choice; later headers cannot invalidate a repeat.
  Keep v2 and the offline prover. Ordinary local production must not wait for L1,
  proof/settlement work or the speculative-depth/replacement bounds. Keep remote
  reads outside the shared production lock and retain local generation/parent
  fences inside it. See the [current proposal](docs/proposals/l1-origin-and-offline-proving.md).


- Keep ABI types, commitments, and shared protocol rules in `eez-protocol` and the
  codec crates, without dependencies on runtime roles. Put role-neutral bootstrap
  in `eez-node-common`, request scheduling/ownership in `eez-composer`, runtime
  wiring in `eez-node`/`eez-follower`, the RPC contract in `eez-control-rpc`, and
  independent proof validation in `eez-proof-signer`.
- Preserve **block-hash commitments**, native blob DA, whole grouped/nested calls,
  and each mode's exact L1 binding. Actual-parent live proofs bind the inclusion
  parent and timestamp; historical ordinary catch-up proofs bind an explicit
  recent anchor, independently of the submission attempt's parent/cutoff.
  Never turn a failed live attempt into historical mode. An authorized contract
  change must update every affected Solidity/Rust producer and consumer, including
  ABI/codecs, Composer, signer, and deriver/follower.
- The [offline history profile](docs/proposals/l1-origin-and-offline-proving.md)
  is active from fresh genesis only. Retain v2, complete L1-header witnesses,
  the shared delayed origin/PREVRANDAO schedule and on-chain context checks.
  Do not add signer chain RPCs, seed-selection fallbacks, v1 proving or an
  existing-network migration path. Composer and followers still need L1 access.
- The signer accepts a narrower profile than the core protocol; upstream support
  alone does not authorize extending it. An authorized extension must update the
  applicable specification, validation, and regressions together. Preserve exact
  decoding/byte comparisons, bounded parsing of Composer-controlled input, and
  independent public-input recomputation. Reject unsupported input; never bypass
  checks, truncate input, or report skipped validation as success.
- Reuse existing checked domain types at trust boundaries. In the signer, keep
  [admitted input, validated execution, and authorized effects](crates/eez-proof-signer/docs/data-provenance.md)
  distinct; decoding alone does not authorize signing. Return errors for malformed
  external input and expected operational failures. Production `unwrap`/`expect`
  needs a locally demonstrable invariant, explained when non-obvious.
- Keep [integration compiler settings](contracts/foundry.toml) aligned with the
  pinned protocol: Solidity 0.8.34, optimizer runs 1, `via_ir`, relative remappings.
  Compiler/metadata drift changes bytecode, CREATE2 addresses, and genesis commitments.
- For changes affecting wire, configuration, or storage compatibility, document
  mixed-version behavior, historical decoding/follower replay, upgrade order,
  migrations, and rollback limits in the affected specification and operator docs.
- For configuration changes, trace defaults and precedence through supported
  inputs and every affected runtime/Compose consumer. Test relevant absent, empty,
  invalid, and conflicting inputs against the setting's contract; parsing alone
  does not prove that the intended value reaches the node or signer.
- For async task changes, reuse Reth's task executor in [node wiring](crates/eez-node/src/lib.rs)
  and follow the signer's [request lifecycle](crates/eez-proof-signer/docs/request-lifecycle.md)
  when changing its workers. Keep blocking I/O and expensive execution/witness work
  off async executor threads; bound new concurrent work and specify its owner,
  cancellation, and shutdown behavior.
  A timed-out RPC does not stop an already-running blocking worker: the signer
  worker retains its request permit until it exits.
  V2 session changes must preserve the [streaming profile](crates/eez-control-rpc/STREAMING_SPEC.md#6-rollupx-runtime-profile),
  including independent provenance retrieval, shared worker ownership and epoch fencing.
  Run the frozen native session transcripts without regenerating expectations.
  Keep the [validation identity](crates/eez-control-rpc/VALIDATION_IDENTITY.md)
  language-neutral. One `VALIDATION_VERSION` covers identity encoding, canonical
  JSON and validation semantics; there are no separately selectable versions.
  The manager uses one `Profile`/`OfflineProfile` and the domain
  `EEZ_ROLLUPX_OFFLINE_PROFILE`. Keep the single `prove.v2.Prover` service.
  Dependency changes require the local review guard and a
  protocol-version change only when validation semantics change.
- For settlement changes, preserve policy-disabled behavior and separate scheduling
  triggers from gas/blob/timing admission limits. Derive budgets from active network
  rules or measured work; admission estimates are not consensus limits. Retain final
  execution validation. Only validated canonical commitment advancement establishes
  settlement progress.
- For queue/reconciliation changes, retain ownership of every accepted request
  until canonical consumption or explicit disposition, including replacement and
  resubmission. Preserve attempt identity, whole replay groups, and sender/direction
  nonce dependencies. **Partial consumption is supported**: a receipt, relay
  acceptance, or matching height does not complete an attempt. Inconclusive chain
  observations do not authorize execution or eviction; blocked sources must not
  starve independent eligible work.
- For prover/retry changes, preserve the RPC contract's closed retry-status list,
  settlement cutoff, validated request-local failure attribution, and bounded
  prover/relay episode accounting. Transport retries do not each charge an attempt;
  do not classify retryability by error text. Handle invalid hints as bounded opaque
  rejections; they cannot authorize targeted eviction. During submitted-attempt
  reconciliation, charge eligible users once only for conclusive settlement failure;
  infrastructure uncertainty and source deferrals retain work without that charge.
  The RPC contract permits up to four deadline-based speculative replacements on an
  unchanged L1 parent. Preserve the old submission history and the single
  current owner; the timeout authorizes recomposition, never canonical completion
  or eviction. Keep the scheduler and submission gates on the shared bound;
  validate consecutive skipped slots, exhaustion and late-original inclusion.
- Pending requests and retry counters are **in memory**. Restart reconciles
  canonical state and may discard speculative blocks; clients may need to
  resubmit unsettled work. Database persistence does not provide durable queue recovery.
- All `eez_*` JSON-RPC methods and virtual `eth_call` discovery are served only
  on Composer fronts, never regular node/follower transports. Use front URLs
  for queue/correlation/witness observations; keep standard `debug_*` calls on
  the node RPC. See the [discovery contract](docs/proposals/composer-wallet-discovery.md).

### Generated files

- Edit the single v2 schema in [prove.proto](crates/eez-control-rpc/proto/prove.proto).
  Cargo's `eez-control-rpc` build generates `$OUT_DIR/prove.v2.rs` using `protoc`; the
  [include module](crates/eez-control-rpc/src/generated.rs) loads it. Regenerate
  with `cargo +1.95.0 check --locked -p eez-control-rpc`; do not hand-edit the output.
- The core pin tracks published `feature/e2e_edge_cases`; RollupX's
  [history predeploy](contracts/src/L2/L1BlockhashContract.sol) and
  [offline manager](contracts/src/OfflineRollup.sol) live in this repository.
  The manager implements the pinned `IRollupContract` interface directly;
  do not restore a private core patch or the context-free initializer.
- Regenerate EEZL2 runtime/funding fixtures with `bash scripts/update-eezl2-genesis.sh`,
  then run
  `bash scripts/update-eezl2-genesis.sh --check`. The script updates the production
  base `genesis.json` (predeploys only, with EEZL2 as an `EEZProxy`, its
  implementation and ProxyAdmin, plus the non-upgradeable FeeVault with
  constructor-derived owner/recipient storage, at fixed addresses; it fails if
  that file allocates ETH), the
  development base `genesis.dev.json` (public test accounts; Kurtosis and Chiado
  test networks select it with `EEZ_GENESIS_BASE`),
  `crates/eez-node-e2e/tests/fixtures/genesis.json`, and
  `testing/kurtosis/l2-genesis-profile.json`. It requires the initialized protocol,
  Foundry 1.7.1, `jq`, Python, and this revision's `eez-genesis-state-root` on `PATH`
  or a local Cargo build; CI requires the verified helper and disallows that fallback.
  Deployment uses `--render` to create separate files, not rewrite these fixtures.

## Build and validate

For source builds, use Rust **1.95.0** (CI; manifest minimum 1.95) and Foundry
**1.7.1**. Native dependencies are listed in [CI](.github/workflows/ci.yml), including
`libclang-dev`, `pkg-config`, `libssl-dev`, `cmake`, and `protobuf-compiler`.
Unmodified Reth v2.7/Gnosis defaults also require LLVM 22, Polly, Clang 22 and
`m4`. On Debian/Ubuntu, use `sudo bash scripts/ci/install-llvm.sh`, then export
`LLVM_SYS_221_PREFIX=/usr/lib/llvm-22` and prepend its `bin` directory to `PATH`.
Docker producers install these dependencies through the same helper. This does
not change the custom L2 interpreter's execution mode.
Build workspace binaries with `cargo +1.95.0 build --locked --workspace`.

During iteration, run focused packages/targets. Before completing a change, run
the applicable checks below and regressions for changed behavior; do not repeat
workspace checks after every edit. Match regressions to the failure mechanism:
malformed/boundary inputs, state-transition invariants, controlled synchronization
for task lifecycle, or process tests for cross-component behavior. CI owns the
full release-artifact and Kurtosis matrix; `make test` does not cover every lane.
Prose-only changes need documentation checks, not runtime suites. Run `bash -n`
on edited shell scripts. Live procedures require their prepared environment and
task authorization.

| Changed area | Expected checks |
| --- | --- |
| Rust code | `cargo +1.95.0 fmt --all --check`; `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings`; affected package tests, e.g. `cargo +1.95.0 test --locked -p eez-composer --lib` |
| ABI, commitments, blob codecs | `cargo +1.95.0 test --locked -p eez-protocol -p eez-blob-codec -p eez-payload-codec`; affected Solidity/process tests. Preserve independent `public_inputs_hash_vectors`, `rolling_hash_vectors`, and `ethereum_kzg_compatibility` oracles. |
| Signer or prover RPC | `cargo +1.95.0 test --locked -p eez-proof-signer -p eez-control-rpc -p eez-prover-client`; the signer contributor guide's checks; `cross_chain` process tests for rejection/status/signing changes |
| Settlement/admission | Composer unit tests; relevant `eez-node-e2e` modules: `settlement_policy`, `settlement_policy_gas`, `settlement_policy_cost`, `dynamic_admission` |
| Application fee quotes / SDK | Composer, L1 and node unit tests; `node sdk/l2-fees.test.mjs`; `eez-node-e2e` module `fee_quotes`. Validate advisory snapshots during pending settlement, bounded concurrent quotes, standalone admission floors, mixed FIFO packing and final native receipts. Mixed posts must bind the outbound-only cost reference to the exact final sources and L1 quote; outbound-only posts retain the whole-post cost/risk floor. L1 refunds cannot fund L2 admission. L1 batching selects the largest recoverable tip without a batch funding floor; missing/insufficient refunds are observation-only. Cover multi-source selection, continued service after missing payouts and idempotent/reorg-corrected refund metrics. For L1 collection require the [real builder refund exercise](testing/kurtosis/README.md#exercise-l1-builder-refunds); the development mempool adapter cannot validate ordered refunds. See also the [L2 fee exercise](testing/kurtosis/README.md#exercise-l2-settlement-fees). |
| Queue, partial consumption, reorg/restart | Relevant `eez-node-e2e` modules: `partial_consumption`, `queue_liveness`, `queue_reorg`; `cross_chain` / `chained_interstate` when group semantics change |
| Integration contracts | `forge fmt --check --root contracts`; `forge test --root contracts` |
| Protocol pin or core-contract integration | Core tests below; `forge build --root eez-core-protocol`; `bash scripts/ci/run-protocol-anvil.sh`; affected fork/E2E checks |
| EEZL2 runtime, compiler settings, genesis | `bash scripts/update-eezl2-genesis.sh --check`; affected contract tests; deployment verification in the [Kurtosis guide](testing/kurtosis/README.md#run-the-pinned-protocol-e2e-suite) |
| Chiado/Ethereum/settlement configuration | `python3 -m unittest discover -s scripts/ci/tests -p 'test_chiado_config.py' -v`; `python3 -m unittest discover -s scripts/ci/tests -p 'test_ethereum_config.py' -v`; `python3 scripts/check-settlement-policy-config.py`; affected runtime tests for parsing/behavior changes |
| Deployment scripts | `python3 -m unittest discover -s scripts/ci/tests -p 'test_deploy_preflight.py' -v`; affected genesis/configuration checks. Mocked preflight tests do not validate a broadcast. |
| CI tooling/workflows | `python3 -m unittest discover -s scripts/ci/tests -v`; exercise the changed producer/consumer path |
| Dependencies/lockfile | `cargo deny check advisories bans sources`; `cargo audit`; affected builds and compatibility tests |
| Cargo features | Build affected shipping targets with the production feature selection in [Dockerfile.runtime](Dockerfile.runtime). Test/`--all-features` builds alone do not establish that configuration works. |
| Documentation | `git diff --check`; resolve edited links/paths at the protocol pin; run `bash -n` on edited shell examples. Syntax checking does not execute the procedure. |

Python configuration checks use Docker Compose **5.5.1**, pinned and verified in
CI, without a daemon or live deployment. Older Compose versions can still read
service `env_file` contents despite `config --no-env-resolution`; 2.38.2 fails the
static-render regression. The full tooling suite also needs the initialized
protocol, `jq`, and PyYAML; inspect skipped tests rather than treating a successful
exit as full coverage.

### Source-based process tests

For local core tests, retain these explicit exclusions and report them as
exclusions, not coverage:

```bash
(
  cd eez-core-protocol || exit
  forge test --no-match-path 'script/*' \
    --no-match-test 'test_Doc_OriginalValue_SeedInSetup|test_PostCost_Incremental'
)
```

Local process tests require `forge` and `anvil`. Clear overrides that can select
stale prebuilt executables, build integration contracts, and retain diagnostics:

```bash
(
  set -euo pipefail
  unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
  forge build --root contracts
  EEZ_TEST_LOG_DIR="$PWD/artifacts/native-e2e/logs" \
  EEZ_TEST_DATADIR_DIR="$PWD/artifacts/native-e2e/datadirs" \
  cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- cross_chain:: --test-threads=1
)
```

All `eez-node-e2e` process tests are modules of the single `node_e2e` target; select
affected modules from the table with a `<module>::` filter and run them serially;
[Nextest](.config/nextest.toml) also isolates `eez-node-e2e`. Do not add `--ignored`
indiscriminately: `kurtosis_policy_replay` requires a live enclave. Follow the
Kurtosis guide for topology/DA/PBS validation.

### CI artifact changes

Read the [runtime producer](.github/workflows/build-runtime.yml),
[artifact verifier](scripts/ci/prepare-runtime-artifacts.sh), and
[native runner](scripts/ci/native-tests.py) before changing this pipeline.
Consumers must execute verified, exact-revision runtime/test artifacts; do not
rebuild missing artifacts or substitute cached binaries. Archive E2E, protocol-vector
and `verify_blob_evidence` tests in one release Cargo invocation whose dependency
feature set equals the shipping build's; the archive step fails if it compiles any
non-workspace crate. Workspace lib/bin unit tests, whose dev-dependencies enable
test-only features (Tokio `test-util`), run in CI's separate debug unit lane.
Test-only features must not reach shipping bytes; the producer freezes the
release candidate before archiving and rechecks its hashes afterwards.
All workflow jobs use GitHub-hosted `ubuntu-24.04` runners. Do not restore runner
selectors or use self-hosted resources for public PRs. Docker-native retained
Cargo state is disabled in CI; the optional local tooling still follows the
[test-archive split's compile cap](docs/proposals/ci-test-archive-split.md#compile-cap).
Registry and compiler caches remain optional and cannot substitute for validation.
Protocol network scenarios run with at most two workers on hosted VMs, preserving
the full scenario inventory. See the [hosted CI proposal](docs/proposals/github-hosted-ci.md).
Kurtosis completes cold protocol and integration fixture compilation before starting the live
network. Preserve the relay housekeeper readiness gate: its post-migration
metrics endpoint must answer before the API starts against the same database.
`scripts/ci/run-native-suite.sh` requires the producer's configured artifact
environment; it is not a standalone local build command.

Published native E2E suites each use four Nextest slice partitions on separate
runner jobs, with serial tests inside each partition. All partitions must pass
before promotion; matrix failures do not cancel the remaining partitions. Each
job consumes the same verified payload without recompiling Rust. The no-publication
fallback still runs the complete suites serially in the producer job. Sharding
reduces elapsed time only when the runner pool has capacity; queued jobs do not
consume their execution timeout. Keep partition inventories complete when changing
the matrix, and retain unique diagnostic artifact names for each shard.

Promote deployment tags only from tested digests after **all** CI validation lanes
pass. Never rebuild during promotion or let older runs roll `latest` backward.
The [CI gate and promotion](.github/workflows/ci.yml) own this procedure.

### Evidence and completion

- For a bug fix, reproduce at the requested baseline and repeat the same check
  after the change; state when that comparison is unavailable. Preserve first
  failures, logs/datadirs, and executable identity before cleanup.
- Require nonzero intended test selection. Empty, ignored-only, skipped, or
  unrelated passing tests do not validate the change. Rejection tests must reach
  the intended component and assert its specific error or state transition.
- CI runs each test once; explicit diagnostic retries retain `flaky-result = "fail"`.
  Report retry-only passes as flakes. Distinguish static, mocked, process, and live
  evidence, and state whether semantic assertions and independent replay ran.
- For performance claims, compare the requested fork baseline and candidate with
  comparable workloads, build profiles, concurrency, and environments. Report
  noise and secondary regressions; benchmark success does not replace correctness checks.
- Report checks/results and applicable checks not run, including reasons and what
  remains unverified. Passing throughput, receipts, or startup does not establish
  semantic correctness, durable recovery, or deployment of the edited revision.

## Documentation and operations

When an authorized change invalidates instructions, update the owning
specification/runbook and affected `AGENTS.md` guidance in the same change. Link
authoritative procedures instead of duplicating them.

### Comments and API documentation

- Explain what a reader needs to safely change the current code: non-obvious
  behavior, invariants/preconditions, ordering/ownership/lifecycle constraints,
  protocol limitations or asymmetries, and meaningful resource trade-offs. Retain
  necessary compatibility/migration rationale and why a simpler alternative fails.
- API docs describe behavior, inputs/outputs, errors/panics, side effects, and
  guarantees where relevant. Implementation comments explain non-obvious behavior
  and rationale; they should not translate each statement or compensate for unclear code.
- For introduced or materially changed unsafe blocks/impls, add a substantive
  `SAFETY` explanation of the actual obligations and how they hold, such as
  ownership, lifetimes, bounds, alignment, or synchronization. Explain the protected
  relationship for non-obvious synchronization/memory ordering; naming a primitive
  or saying “safe” is insufficient.
- Omit obvious restatements, edit narration, and unsupported performance/safety/
  protocol claims. Links may support an explanation, not replace it; distinguish
  inferred intent from established requirements. Keep change history in commits/audits.
- Update nearby comments and API documentation when behavior changes, preserving
  relevant compatibility rationale. Add implementation comments when the code
  alone does not make its behavior, constraints, or rationale sufficiently clear;
  do not add comments to meet a quota.

### Records and operations

For the optional Prometheus/Grafana stack and Composer queue metric semantics,
see [monitoring/README.md](monitoring/README.md).
The optional standalone [settlement indexer](monitoring/indexer/README.md) has its
own Cargo workspace and focused tests; it does not change the node lockfile or CI matrix.
Its optional [posting lifecycle collector](monitoring/indexer/README.md#posting-lifecycle-history-optional)
replays retained JSON logs; preserve that volume and its PostgreSQL checkpoints
when restarting. It is operational history, not durable queue ownership.

| Record | Use |
| --- | --- |
| [Proposal](docs/proposals/TEMPLATE.md): `docs/proposals/<topic>.md` | Before substantial design/behavior changes, record scope, invariants, alternatives, acceptance checks, and actual draft/accepted status. Extend a relevant existing proposal. |
| [Audit](docs/audits/TEMPLATE.md): `docs/audits/YYYY-MM-DD-<topic>/README.md` | Record substantial investigations/validation and executed operations that need reproducible evidence: exact source/protocol/executable identity, commands, results, limits, and unmet criteria. |
| [Runbook](docs/runbooks/README.md): `docs/runbooks/<task>.md` | Maintain repeatable operations: prerequisites, ordered commands, expected results, recovery boundaries, and verification status. Consult before operating; add newly verified steps. |

Existing task authorization applies; these records add no approval gate. Small
fixes and documentation edits need no new proposal/audit pair. Link requirements,
implementation, and evidence. Preserve dated failures/corrections; older results
do not validate newer binaries. Keep small sanitized evidence or a retrievable
artifact location/digest; label local-only, missing, or expired evidence. Ignored
`artifacts/` paths are unavailable to another checkout.

For Chiado operation or deployment changes, read the
[quick start](docs/runbooks/chiado-quickstart.md) and
[deployment/recovery runbook](docs/runbooks/chiado-deployment.md). Before adding or
replacing a Composer on a Chiado L2 created with this offline profile, read the
[existing-rollup procedure](docs/runbooks/chiado-existing-rollup.md). Preserve these boundaries:

- Normal deployment uses the published deploy image's bundled contracts/tools;
  host builds and `:local` images are for development. Match node/signer/deploy
  source revisions with the configuration checkout and protocol. The default
  `:latest` can change on Compose startup. Commit-SHA tags name the intended source
  revision by convention and can move; digests pin image content. Neither alone
  establishes source provenance. Verify the source-to-digest mapping through CI's
  artifact manifest and [verification checks](scripts/ci/prepare-runtime-artifacts.sh).
- Copy `.env.chiado.example` to `.env` only during initial setup when `.env` does
  not exist. Preserve existing settings and secrets; Compose loads `.env`. Keep
  private inputs mapped per service; never inject the proof-signing key into the node.
- Genesis rendering/tool checks must precede the first deployment broadcast.
  Startup requires existing bindings and preserves datadirs/genesis; contract
  deployment is a separate, non-idempotent operation.
- Keep quick starts to normal setup/run/check commands. Put optional diagnostics
  and recovery in the detailed runbook, not the normal setup sequence.

The offline-history profile requires matching contracts, bootstrap, genesis,
Composer, signer and followers from network creation. Native transaction support
alone does not make an earlier network compatible. Follow the
[compatibility boundary](docs/runbooks/chiado-deployment.md#offline-v2-proving-and-fresh-network-boundary);
same-profile restart and recovery do not authorize migration of an older network.
