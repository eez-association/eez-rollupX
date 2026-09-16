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
  and working tree before protocol or E2E work. Read its documentation and code
  at that pin, including in a worktree with an uninitialized submodule; do not
  substitute upstream `main`. Preserve local changes. Change the dependency
  only when the task includes that scope, and validate downstream compatibility.
- Place shared behavior with its lifecycle owner. Keep roles and required
  dependencies explicit; add abstractions and public APIs for real callers.

## Specifications

For protocol semantics, start with the pinned `eez-core-protocol` specifications,
then read the fork's integration requirements. Select the relevant material:

| Area | Sources |
| --- | --- |
| Execution, ABI, and cross-chain entries | [Core protocol](eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md), [execution entries](eez-core-protocol/docs/EXECUTION_ENTRY_SPEC.md), [static/reverted calls](eez-core-protocol/docs/STATIC_ENTRY.md), and [caveats](eez-core-protocol/docs/CAVEATS.md) |
| Proof verification and public inputs | [Multi-prover specification](eez-core-protocol/docs/MULTI_PROVER_SPEC.md), checked against the pinned contract implementations |
| Native blob encoding | [Blob format](eez-core-protocol/docs/blobs/BLOB_FORMAT_SPEC.md) and [blob/table tools](eez-core-protocol/script/blob/README.md) |
| Fork request construction and verification | [Composer/prover specification](crates/eez-control-rpc/SPEC.md), [signer specification](crates/eez-proof-signer/SPEC.md), and [signer contributor guide](crates/eez-proof-signer/CONTRIBUTING.md) |

Cross-check the relevant specifications against the pinned Solidity interfaces,
implementations, and tests. The fork's signer profile intentionally supports
less than the complete protocol; upstream support alone does not justify
relaxing its checks. Resolve disagreements explicitly across code, tests, and
specifications.

## Code Review Rules

- Preserve block-hash commitments, native blob DA, grouped/nested call semantics,
  and the binding of proving and submission to the same L1 target unless the
  task explicitly changes those contracts.
- For ABI, hashing, or encoding changes, trace all affected producers and
  consumers: Solidity, shared Rust types/codecs, Composer, signer, and
  deriver/follower. Update compatibility vectors and affected specifications
  together; a local encoder/decoder round trip alone cannot prove compatibility.
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

## Proposals and audits

- For substantial design or behavior changes, create or update
  `docs/proposals/<topic>.md` before implementation. Record status, date, source
  baseline, problem, scope/non-goals, invariants, design/tradeoffs, failure and
  recovery behavior, and acceptance checks. Distinguish draft ideas from accepted
  requirements; a draft containing `MUST` is not an adopted contract. Use the
  task's existing authorization; writing a proposal creates no extra approval gate.
- For investigations, substantial validation, or operational changes, record
  evidence in `docs/audits/YYYY-MM-DD-<topic>/README.md`. Include source and
  protocol revisions, relevant uncommitted changes, tested binary/image identity,
  environment/configuration, exact commands, results, evidence locations, and
  limitations. Separate observed facts, reproduced behavior, and hypotheses;
  reproducing a possible failure path does not prove it caused a past incident.
- Link proposals, implementation/coverage records, and audits in both directions.
  Map important requirements to production paths, checks, and results. Use small
  machine-readable evidence summaries when useful; retain sanitized reproduction
  inputs and diagnostics without committing databases or bulk logs.
- Keep intended design, implementation, validation, and deployment status
  distinct. Preserve dated findings and failed checks; add dated corrections or
  links to superseding evidence. Older results do not validate later changes.
- Maintain current behavior and operator instructions in `docs/<topic>.md` or
  the relevant crate specification/README. Link supporting proposals and audits
  instead of duplicating their history. Small fixes and documentation edits can
  update existing records without creating a proposal and audit pair.

## Validation

- Choose checks from [CI](.github/workflows/ci.yml) and its reusable workflows.
  Consult the [native test runner](scripts/ci/native-tests.py),
  [suite wrapper](scripts/ci/run-native-suite.sh), and
  [Nextest configuration](.config/nextest.toml) for native suites and runtime
  artifact requirements; use the [Kurtosis guide](testing/kurtosis/README.md)
  for the devnet. `make test` is not all CI.
- For behavior changes, use regressions that exercise production code and would
  fail without the fix. Cover changed process/RPC boundary semantics with
  cross-process evidence. Keep compatibility oracles independent. Confirm the
  intended tests actually ran; zero selected tests or ignored-only runs do not
  validate the change. Documentation edits need no new runtime tests.
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
- Summarize checks and relevant skips, linking the audit when present. Distinguish
  static, unit, mocked, embedded, cross-process, and live evidence. Report whether
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
