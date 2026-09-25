# Cargo target cache transport probe

Status: isolated manual experiment; not a replacement for production CI.
This standalone probe does not change production build workflows or caches.

Temporary execution branch exception: `test/cargo-cache-probe-run` adds a push
trigger restricted to that exact branch, with GitHub-hosted runners as the
fallback when no manual input exists. No PR is opened from this branch; normal
CI's push trigger remains restricted to main. PR #38 stays unmerged and retains
the manual-only workflow. These temporary trigger changes are not for merging.

## Why this step

The operator's 2026-09-25 ARC diagnostics confirm disposable workspaces and
per-pod Docker daemons. The persistent `/ci/cargo` hostPath is node-local and is
not mounted into DinD. Rust compilation was observed under `/docker/buildkit/`,
outside the Kubernetes pod's cgroup hierarchy, without CPU/memory caps. The
observed build cgroup peaked at 20,387,422,208 bytes (about 19 GiB). This is not a
bound on future builds or a performance comparison. A disposable Docker shell
confirmed that explicit `docker run` CPU/memory/swap settings reach the kernel.
No ARC settings, runner counts or shared caches have been changed.

Before another full-workspace experiment, check that a job-private Cargo target
can be transported between fresh jobs and used by a bounded container. Use the
same mechanism on homelab and GitHub-hosted runners, not a homelab-only hostPath.

## What runs

[The manual workflow](../../.github/workflows/cargo-cache-probe.yml) has two serial
jobs on the selected runner pool:

1. Seed: create a tiny dependency-free Rust package, build and test it, verify an
   unchanged repeat is fresh, and save its target directory, fixture source
   snapshot and seed metadata with `actions/cache/save`.
2. Restore: require a clean job-local directory, restore the exact seed cache,
   verify the seed executable hash, and build identical freshly created sources.
   Report Cargo's actual `fresh` flags separately from transport success. Then
   change the source and check changed executable behavior; change a test to fail
   and require that exact test failure. An independent copy of the original seed
   also runs the checksum-synchronization matrix below. The changed states are
   not saved. The runner needs `rsync`; the workflow checks availability rather
   than installing host packages or changing the homelab image.

Both use the same digest-pinned Rust 1.94.0 Bookworm image, one CPU, 1 GiB memory,
no swap, and `CARGO_INCREMENTAL=0`. Builds run offline, without a compiler cache,
external dependencies, Docker socket mounts or operator credentials. The one
writable bind mount is under the checkout workspace (visible to both ARC runner
and DinD), not `RUNNER_TEMP` or `/ci/cargo`. Container root is read-only; each
container is removed after its command. Network access is needed outside the
container for image pulls and GitHub cache/artifact transfers.

The fixture uses Cargo's default release profile and linker. It does **not** claim
to reproduce RollupX's production build flags, feature graph, linker, memory
needs or cache size. The production build experiment must preserve those inputs.

## Run and interpret

Once approved and published, this workflow must be registered on the repository's
default branch for `workflow_dispatch` to be available. Do not merge or push it
implicitly to make that happen. Select **Cargo cache transport probe** in Actions
and run once with `homelab-rust`, then with `ubuntu-24.04`.

The probe-only draft uses a [`[skip ci]` commit message](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/skip-workflow-runs) to suppress automatic
push/pull-request CI. This does not disable CI globally or skip manual dispatch.
Required checks may remain pending; the draft is not thereby merge-ready.
Before merging, deliberately run the required checks against the final revision.

Cache keys are unique per runner pool, workflow run and attempt. There are no
fallback keys or writes to normal CI caches. The restore job intentionally fails
on a cache miss: a cold rebuild must not masquerade as successful transport.
Normal production CI should retain its cold-build fallback.

Read both jobs' JSON reports in their logs and job summaries. Failed jobs also
upload individual command logs as three-day diagnostic artifacts; successful
jobs do not consume diagnostic artifact quota:

- `restored_binary_verified`: target state really arrived.
- `restored_all_fresh`: Cargo avoided recompiling the unchanged fixture.
- `source_and_test_invalidation_verified`: changed code/tests were not hidden by
  stale output.

A green raw-restore probe with `restored_all_fresh: false` means transport and
correctness passed, **not** successful compilation reuse. Fresh source timestamps
can cause recompilation; that baseline deliberately does not preserve them.
The separate `freshness` mode below tests source synchronization. Tiny fixture
timings are not RollupX performance measurements or meaningful estimates of
multi-gigabyte transfer cost.

For local validation, run `cargo-cache-probe.py seed DIRECTORY`, copy **only**
`DIRECTORY/state` to a different new directory's `state`, then run
`cargo-cache-probe.py restore NEW_DIRECTORY`. Pull the pinned Rust image first.
This validates separate container execution and target relocation, not GitHub
cache APIs or ARC behavior. Logs remain in those owned directories. No full
workspace build or public image publication is part of this step.

## Local evidence (2026-09-25)

The original experiments below ran in the `ci-build-cache-boundaries` worktree
at base `f128b6d9bfba7f9edb251490f4a3a01b918c753e`, with uncommitted probe files.
Their ignored artifact paths are local-only and unavailable in another checkout.
The standalone probe branch is based on main
`7c90266a5fe8cfdb46b249c61079badc2dce71a3`; both bases pin protocol
`9d13218a355e7d543ca8a9927eea2962600b716d`.

Local seed and restore containers passed the resource-limit assertions, initial
build, one passing test, same-job compilation reuse, restored executable hash,
changed executable behavior, and deliberate test failure checks. Both restored
runs reported `restored_all_fresh: false`. A diagnostic repeat with Cargo's
fingerprint logging established `FsStatusOutdated` for `src/main.rs`: the newly
created source's mtime was later than the cached dep-info reference. Unchanged
content alone therefore did not prevent recompilation in this fixture.

Evidence is local-only under `artifacts/cargo-transport-local-20260925/`, with
`seed`, `restore` and `restore-diagnostic` directories; each retains JSON results
and individual command logs. The first restore was repeated specifically to add
fingerprint diagnostics, not to obtain a retry-only pass. No source timestamps
were modified. This identifies a follow-up requirement; it does not justify
backdating arbitrary modified files or bypassing Cargo freshness checks.

GitHub cache save/restore and execution on ARC/hosted runners have **not** run.
No image publication is part of this experiment. Successful transport alone is
insufficient to adopt this design.

The first complete tooling run found two failures in the existing workflow
contract: successful experiments were uploading diagnostic artifacts. The probe
now follows the repository's failure-only, best-effort diagnostic upload policy;
successful results go to job logs and summaries. No production contract was
weakened to accommodate the experiment.
After that correction, all 16 workflow-contract tests and six probe-contract
tests passed, as did `git diff --check`. The complete 266-test tooling run was
not repeated after the correction (its initial run had four opt-in skips).

## Follow-up: checksum-synchronized build tree

Keep the fixture source snapshot together with its compiled state. On restoration,
copy that snapshot into an isolated build tree and synchronize the fresh checkout
into it with `rsync --recursive --checksum --delete --perms --links` (deliberately
**without** `--times` or `--archive`). The fresh checkout is authoritative:
unchanged files retain their cached timestamps; changed files are copied with new
timestamps; deleted files are removed, including inputs watched by build scripts.
This reuses ordinary synchronization rather than maintaining a custom database
of content hashes and timestamps. Cargo's own fingerprints are never edited.

The helper rejects caches whose completion timestamp is not earlier than the
current clock, so copied edits cannot appear older than the seed build due to
clock skew. It assumes a trusted, internally consistent cache and no concurrent
source modifications during synchronization/build. A production implementation
would need a cold-build fallback for unusable caches, compatibility keys, a
complete source inventory including submodules and generated inputs, and explicit
exclusion of secrets/operator state. A snapshot must never replace current-source
verification. Symlinks, file modes and arbitrary production build scripts require
further validation; this is not a general-purpose source-cache utility.

Run `cargo-cache-probe.py freshness DIRECTORY` where `DIRECTORY/state` is a copy
of the seed cache. `rsync` is required on the host. Every case receives an
independent copy of the same cached state. The fixture now includes a Rust module,
`include_str!` data, a build script watching a directory and an environment input,
and one unit test. The cases are:

| Case | Required outcome |
| --- | --- |
| Unchanged source | Production and test artifacts are fresh; cached test executes |
| Main edit, then revert | Both transitions rebuild and change executable behavior |
| Same-size edit with backdated mtime | Checksum detects it; executable changes |
| Module or included-data edit | Rebuild and changed output |
| Build input changed/added/deleted | Build script reruns; output reflects current directory |
| Build script changed | Rebuild and changed output |
| Manifest default feature changed | Cargo invalidates the old compilation |
| Watched environment changed | Build script reruns; output changes |
| Module deleted or renamed without updating its use | Compilation rejects the missing module |
| Test edited to fail | That exact test executes and fails |

The initial 14-case copy-preserving run passed. The expanded run after a real
GNU tar/gzip round trip also passed all 14 cases, including test-artifact reuse
and edit/revert assertions. Raw restoration of the same archived seed still
reported `restored_all_fresh: false`; synchronized unchanged restoration reported
`all_fresh: true`. Local-only evidence is under
`artifacts/cargo-freshness-local-20260925/`: `seed`, `matrix`, `matrix-archive`,
`raw-archive`, and `state.tar.gz`. The archive round trip tests ordinary timestamp
precision loss, not the GitHub cache service itself.

This establishes the mechanism for the small fixture only. It does not measure
full-workspace savings, network transfer cost, third-party dependency/build-script
reuse or production-memory requirements. Stable Rust 1.94.0 remains unchanged;
no nightly checksum-freshness flags or dependency changes are used. Cargo's
[fingerprint documentation](https://doc.rust-lang.org/stable/nightly-rustc/cargo/core/compiler/fingerprint/index.html)
explains the underlying mtime checks; the observations above come from the pinned
compiler, not from assuming current documentation describes every 1.94 detail.

Follow-up validation: the full tooling suite completed 269 tests in 131.287s,
with 265 passing and four existing opt-in integrations skipped (Docker payload,
runtime packaging, native archive relocation and Kurtosis resources). After the
manual workflow was wired to run both comparisons, all 16 workflow-contract and
nine probe-contract tests passed again; `git diff --check` passed. This is not a
claim that the still-separate full RollupX CI validation has completed.

### Standalone branch validation

Repeated on the standalone branch based on `7c90266a`, with the exact protocol
pin initialized:

- Full tooling suite: 264 tests in 141.283s; 260 passed and the same four opt-in
  integrations above were skipped. The five build-metrics tests in the earlier
  worktree belong to the separate runtime investigation, not this probe.
- Focused checks: nine probe-contract and 16 workflow-contract tests passed.
- Fresh local seed, tar/gzip transport, raw restore and all 14 synchronized cases
  passed. Raw restore recompiled; synchronized unchanged sources reused both
  production and test artifacts. Edits, reverts, deletions, renamed modules and
  the intentionally failing test produced their required outcomes.
- Local-only evidence: `artifacts/probe-pr-validation/` in the
  `cargo-cache-transport-probe` worktree (`tooling.log`, `seed`, `raw`, `sync`,
  `state.tar.gz`). No full RollupX rebuild or GitHub workflow dispatch was run
  for this isolated validation.
