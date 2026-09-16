# Working in rollupX

This is a fork of `eez-association/eez-rollup0`. Paths below are relative to
the repository root. Keep detailed procedures in the linked documents.

## Scope and evidence

- Before changing behavior, trace the affected runtime paths, production
  callers, and tests. Specifications describe the intended contract; source
  establishes current behavior. Report and resolve mismatches explicitly.
- For PR reviews and upstream ports, identify the remotes and record the PR
  base/head, intended fork target commit, and protocol pin. Review the source
  diff and its interaction with that target; do not assume `main` is the target
  or replace fork behavior simply to match upstream files.
- Treat `eez-core-protocol` as a pinned upstream dependency. Check its gitlink
  and working tree before protocol or E2E work. Preserve local changes. Change
  the dependency only when the task includes that scope, and validate consumers
  and independent compatibility tests at the new pin.
- Place shared behavior with its lifecycle owner. Keep roles and required
  dependencies explicit; add abstractions and public APIs for real callers.

## Code Review Rules

- For protocol changes, read the [Composer/prover specification](crates/eez-control-rpc/SPEC.md)
  and [signer specification](crates/eez-proof-signer/SPEC.md). Preserve block-hash
  commitments, native blob DA, grouped/nested call semantics, and the binding
  of proving and submission to the same L1 target unless the task explicitly
  changes those contracts.
- For settlement or queue work, also read the [settlement policy](docs/configurable-settlement-policy.md)
  and [queue requirements](docs/proposals/cross-chain-queue-liveness.md). Preserve
  policy-disabled behavior; distinguish scheduling triggers from gas, DA, and
  time admission limits. Only validated canonical commitment advancement is
  settlement progress.
- Account for every accepted request through canonical consumption, retention,
  requeue, or explicit terminal disposition. Preserve attempt identity, complete
  replay groups, and sender/direction nonce dependencies. Partial consumption is
  supported; relay acceptance, one receipt, or a matching height does not prove
  completion of the whole attempt. Pending ownership is currently in memory;
  restart may require client resubmission.
- Decide retryability separately from failure attribution. Bind actionable
  errors to the exact request. Apply the existing bounded policy to conclusive
  failures; infrastructure-only failures do not charge that settlement policy.
  Do not infer execution or eviction authority from an inconclusive observation.
- Source-state and capacity deferrals retain work without charging failed
  attempts; blocked sources must not starve independent eligible work.
- Reject unsupported input explicitly when accepting it would hide divergence.
  Do not add silent truncation or success for skipped work. Avoid `unwrap` and
  `expect` on production runtime data unless the invariant is local and obvious.
- Justify limits from the applicable protocol/network rules or measured budgets.
  Preserve the affected network profiles and final execution validation;
  a measured admission estimate is not a consensus limit or a safety proof.
- Write comments and docs about current behavior and rationale. Update affected
  specifications and operator examples when their contract changes.

## Validation

- Choose checks from [CI](.github/workflows/ci.yml) and its reusable workflows.
  Consult the [native test runner](scripts/ci/native-tests.py),
  [suite wrapper](scripts/ci/run-native-suite.sh), and
  [Nextest configuration](.config/nextest.toml) for native suites and runtime
  artifact requirements; use the [Kurtosis guide](testing/kurtosis/README.md)
  for the devnet. `make test` is not all CI.
- For behavior changes, use regressions that exercise production code and would
  fail without the fix. Cover changed process/RPC boundary semantics with
  cross-process evidence. Keep compatibility oracles independent. Documentation
  changes need relevant documentation checks, not new runtime tests.
- Rejection tests must establish that the request reached the intended component
  and assert the specific error or state transition. Lack of success, a timeout,
  or logs alone do not establish correct rejection.
- For bug fixes, attempt reproduction at the requested baseline and rerun the
  same evidence after the fix. Use an isolated checkout when needed. If blocked,
  state the limitation and continue feasible checks; do not label static
  reasoning, mocks, or an environment failure as a live reproduction.
- Run process-spawning suites serially unless their harness proves isolation.
  Retain first-failure diagnostics and report retry-only passes as flakes.
  Match tested executable/artifact identities to the source being evaluated.
- Report exact commands, results, relevant skips, and evidence paths. Distinguish
  static, unit/mocked, embedded, cross-process, and live evidence. Report whether
  semantic assertions ran; receipt success and throughput alone do not establish
  all intended effects or independent follower agreement. The
  [staged Chiado runner](scripts/run-chiado-staged-performance.sh) uses
  `--no-verify`; its success alone is not semantic validation.

## Operations and delivery

- Use isolated ports, directories, and process/container identities for test
  resources. Clean up only resources created and owned by the task. Preserve
  minimal sanitized failure evidence before deleting disposable data; keep
  unrelated changes, existing deployments, databases, and shared wallets intact.
- Run funded traffic, deployment changes, or destructive recovery only within
  the task's existing authorization. For Chiado workloads, read the root
  [README](README.md) and the selected runner; respect shared resources and its
  locking rules. Keep secrets, private keys, authenticated URLs, and private
  deployment hostnames out of committed artifacts and reports.
- PR descriptions state the behavior change, motivating invariant, tradeoffs,
  exact validation, and relevant CI checks still pending. Preserve meaningful
  commits; for an authorized history rewrite, inspect `git range-diff` and use
  `--force-with-lease=<ref>:<expected-sha>` when publishing.
