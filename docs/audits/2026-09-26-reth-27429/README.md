# Upstream Reth migration validation

[PR #41](https://github.com/eduadiez/rollupX/pull/41) passed local release-image
and Kurtosis validation at `1888b7431eee2ae421d03e323cef72d34256758c`, consuming
the published Stateless fork. Final-head CI remains required before merge;
earlier CI successes and failures are recorded below.

## Scope and source identity

| Input | Revision |
| --- | --- |
| PR base | `3346d966f3c8611be790db484abc63b3b259d367` |
| Validated EEZ head before cleanup | `ab5060afb29f7fe25dc2ae19413f281d5840520c` |
| Reth | `444e867155c2a7a4c028e479ca7201b0b6871579` |
| Protocol (unchanged) | `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Build tools | Rust 1.95.0; Foundry 1.7.1; Cargo `--locked` |

The Reth pin is the upstream merge of [#27429](https://github.com/paradigmxyz/reth/pull/27429),
78 main-branch commits beyond v2.6.0, excluding the release version bump.
Its package version remains 2.5.2. Reth source is unmodified; the
[dependency ports](../../../vendor/README.md) preserve Gnosis support and the
signer's Stateless transaction checkpoints. Production persistence defaults,
protocol contracts, genesis commitments and signer admission rules are unchanged.
Private test miners retain the short persistence window needed by their fixtures.

Fresh networks are the scope. Public Gnosis/Chiado deployment, historical database
migration, mixed-version upgrades and rollback remain unverified. No existing
network was upgraded or release tag promoted.

## Validation

| Check | Result before cleanup |
| --- | --- |
| Shipping builds; workspace check, Clippy and formatting | Passed |
| Rust libraries, signer binary and independent vector tests | 1,195 passed; 3 manual utilities ignored |
| Stateless/tries libraries; full-state checkpoint oracle | 8 library tests and 1 checkpoint integration test passed |
| Exact upstream engine and forkchoice regressions | 205 engine tests and 3 RPC tests passed |
| Cargo deny / audit | Passed with documented allowed/unmatched-source warnings |
| Python tooling | 262 passed; 4 opt-in skips |
| Local native process cases | 34 passed, including Gnosis settlement and crash recovery |
| Local Kurtosis with the unchanged protocol runner | 32/32 eligible scenarios; bidirectional waves, state chaining, independent causal and semantic blob verification |
| Restart and fresh follower replay | Settled state preserved; 66 blocks and 3 workload receipts independently checked |
| Final pre-cleanup CI | All 12 jobs passed, including all 8 native shards and Kurtosis |

The three Rust ignores are manual profiling/inventory/feasibility utilities.
The four Python skips are opt-in Docker/Kurtosis/archive fixtures. Four protocol
scenarios are outside the pinned live runner's supported selection. These are
exclusions, not coverage. Local relay startup required intervention, so these
runs do not establish unassisted cold startup. The canonical Kurtosis L1 used
Reth v2.6.0; the embedded EEZ runtime used the candidate pin.

The successful CI runs are [3893cfd](https://github.com/eduadiez/rollupX/actions/runs/36299583785)
(99 selected native cases and 32 Kurtosis scenarios),
[f33e3b0](https://github.com/eduadiez/rollupX/actions/runs/36304470427), and
[ab5060a](https://github.com/eduadiez/rollupX/actions/runs/36310704350).
Executable hashes, image digests, tested merge commits, commands and exclusions
are in the archived records below. All owned validation enclaves were dumped and
removed; operator services and data were preserved.

## First cleanup (`252b964`)

The cleanup retains byte-identical contents for 340 Rust, Cargo manifest,
shipping/Stateless lockfile and vendor fixture files. It also restores the
protocol wrapper and its tests to the PR base, removing only the temporary
post-failure receipt diagnostic. Protocol source and pin remain unchanged.

Both root and standalone Stateless `cargo +1.95.0 metadata --locked` commands
passed, and every local Cargo target still exists. Vendor patch reverse checks,
shell syntax, documentation links and `git diff --check` passed. The Python
tooling suite ran 265 tests: **261 passed, 4 opt-in skips**. Full Rust and live
Kurtosis suites were not repeated locally for this packaging/documentation
cleanup; final-head CI remains required. Local check output is retained under
`artifacts/reth-cleanup/`.

## Stateless workspace consolidation

The subsequent cleanup makes `stateless`, `tries` and `zeth-mpt` members of the
EEZ workspace. Their separate workspace/lockfile and unused standalone EF and
RISC-V tooling are removed. The root lockfile now covers their dev/optional
dependencies too. Existing package versions and checksums are unchanged; the
shipping dependency tree, including enabled features, is identical. All 296
retained Rust source/test files are byte-identical to `252b964`.

The initial offline resolution chose an older optional `rkyv` and failed the
audit check. Carrying over the prior standalone lockfile's `rkyv` 0.8.18 and
preserving existing shipping dependency edges corrected the consolidated lock.
No advisory exceptions were added. The first failure remains in local logs.

Verification used the final lockfile: **26 Stateless/trie/checkpoint tests**,
workspace Clippy and formatting, and **261 Python tooling tests** passed
(4 opt-in skips). Cargo deny/audit passed with the existing warnings. The exact
`Dockerfile.runtime` package/target selection built locally in the host dev
profile; release-artifact and Kurtosis validation remain the final CI gate.
Vendor patches, documentation links and whitespace checks also passed.
Commands and logs are local-only under `artifacts/reth-cleanup-2/`.

## Stateless fork handoff (2026-09-27)

Prepared from EEZ `74d4cdbc577909eef3a94fad30469d32618dd94d`, the next cleanup
replaces the three vendored Stateless crates with Git dependencies pinned to
`eez-association/stateless` commit `ffe41a37f67b7d77a4f32c19d7fcbd10ca7ea0f3`,
on `feat/reth-27429`. Its parent is the former fork base `4fc3806`; its 24 runtime
source and fixture files are byte-identical to the vendored port. The Reth and
protocol pins remain unchanged. Standalone manifests and the fork's own lockfile
restore independent builds; a branch CI workflow runs all 26 checkpoint/trie
tests. EEZ workspace tests no longer execute dependency-owned tests.

The shipping dependency tree, including enabled features, matches after
normalizing source paths/identities. No retained lockfile package changed;
ten optional/test-only packages leave EEZ's lockfile. The fork retains them
where needed. Formatting, fork Clippy, all 26 fork tests and the fork audit
passed (three existing warnings; cached advisory database). EEZ's 366 signer/RPC
tests passed with two manual utilities ignored; workspace Clippy, formatting and
Cargo deny/audit passed (nine existing audit warnings).
The Python tooling suite passed 261 tests, with four opt-in Docker, Kurtosis and
archive checks skipped. The two signer ignores are inventory and feasibility
utilities. The fork's retained EF and RISC-V procedures were not run.
The exact `Dockerfile.runtime` package/target selection built successfully in
the host dev profile. This is not a release-artifact or live-network check.

The first sandboxed signer run could not bind its local test sockets (75
`PermissionDenied` failures); the same suite passed with local socket access.
Cargo deny initially could not write its advisory-cache lock; it passed with
cache access. These environment failures are retained in local logs.

The Git bundle was verified and cloned to recover the exact fork commit. At
handoff it is unpublished: local Cargo validation used a command-scoped Git URL
redirect to the prepared repository, without changing global Git configuration.
Public fetching, fork CI, final release CI and local Kurtosis validation of this
dependency switch remain pending. The full earlier Kurtosis results do not
establish that the unpublished commit can be fetched in a fresh environment.
The packet and logs are local-only at
`artifacts/stateless-reth-27429/` in the operator checkout.

Publication was subsequently verified by fetching `ffe41a3` into a fresh Git
repository over public HTTPS, with global/system Git configuration disabled.
The fork's [host CI passed](https://github.com/eez-association/stateless/actions/runs/36328488920).
The dependency switch was applied as EEZ commit `1888b7431eee2ae421d03e323cef72d34256758c`.
The preceding cleanup's [CI run](https://github.com/eduadiez/rollupX/actions/runs/36319986847)
passed Kurtosis and seven native shards, but failed the multi-blob backlog test:
the follower lost its L1 source during restart, then exited because the scan
completion block was unavailable. The same test passed locally at `1888b74`
(one selected case, 127.66s, source-built binaries and retained logs/datadirs).
The catch-up error path was not changed, so this is diagnostic evidence, not
a claimed fix for the earlier timing failure. Local publication-validation
evidence is under `artifacts/stateless-published-validation/` in the PR checkout.

Fresh node, signer and deploy release images built at `1888b74`. The genesis
fixture check passed using the helper extracted from the node image. The normal
`testing/kurtosis/run-ci.sh` then passed all four traffic modes, state chaining,
and **32/32 eligible protocol scenarios** (four explicit exclusions). Independent
causal and semantic blob verification and scenario accounting were complete;
the proof-flow summary recorded 29 signed windows and 29 remote attestations.
The standard runner handled startup without manual service intervention and
removed its owned enclave, `eez-stateless-ffe41a3`, after success.

The canonical L1 was official Reth v2.6.0 (`73a3a008`), pinned in the local args
to image digest `sha256:8ce703acf113b2a20705b6e76adebed20f74ba8591bc7c9407203b2968aca70d`.
The embedded EEZ node used `444e867`. Node/signer/deploy images carried the
`1888b74` revision label and these local image IDs:

| Image | SHA-256 |
| --- | --- |
| Node | `bd7d1c4720b45824951893167011c54871aac10ef386cbcd060a87f2549cfed6` |
| Signer | `4c5d3b7c9014f78f1b34a8f41fad0a6be8f5a3f57e825dae4526245588efd8bd` |
| Deploy | `d162aed83daafc2bfc4dfb937699ff55bbe0d8b28cf739d9220bc731a008799e` |

The independent verifiers used the same source and locked dependencies, built
in the host dev profile. `identity.json`, `kurtosis/result.json` and
`kurtosis/protocol-e2e/summary.json` in the local evidence directory retain the
environment, executable hashes and results. Bulk evidence is local-only.

## Retained failures and limits

- Official v2.6.0 reproduced a stale-persisted-head forkchoice failure. The
  candidate includes #27429 and passes its upstream regressions.
- A historical crash-replay failure remains unattributed. A corrected identical
  test passed with both v2.6.0 and the candidate, in shipping and host builds:
  persisted block/state frontiers 46/16, reopening at 16, replay through 45.
  This establishes recovery for that controlled case, not that #27429 fixed
  the earlier failure. Setup failures and graceful-restart controls remain archived.
- [An earlier CI run](https://github.com/eduadiez/rollupX/actions/runs/36275978196)
  failed a protocol table check. Controlled local reproduction showed that the
  pinned runner can verify an obsolete receipt block after relocation (882 to
  900). The historical CI transaction lacks a final receipt observation, so its
  direct attribution remains unavailable. A separately tested runner correction
  is deferred; this PR does not apply it or change the protocol pin. The known
  timing failure remains possible, and later passes do not erase earlier failures.

## Detailed evidence

The complete, dated records remain at commit
`ab5060afb29f7fe25dc2ae19413f281d5840520c`, retained by branch
`archive/reth-migration-validation`. This summary replaces their copies in the
migration diff, including the unapplied protocol experiment.

- [Candidate validation, identities, commands and failure diagnostics](https://github.com/eduadiez/rollupX/tree/ab5060afb29f7fe25dc2ae19413f281d5840520c/docs/audits/2026-09-26-reth-27429)
- [Official v2.6.0 validation and controls](https://github.com/eduadiez/rollupX/tree/ab5060afb29f7fe25dc2ae19413f281d5840520c/docs/audits/2026-09-26-reth-v2.6-validation)
- [Initial main-based PR validation and crash failures](https://github.com/eduadiez/rollupX/tree/ab5060afb29f7fe25dc2ae19413f281d5840520c/docs/audits/2026-09-26-reth-v2.6-pr)
- [Dependency release compatibility investigation](https://github.com/eduadiez/rollupX/blob/ab5060afb29f7fe25dc2ae19413f281d5840520c/docs/audits/2026-09-26-reth-release-compatibility/README.md)

Portable summaries and sanitized evidence are committed there. Bulk logs,
databases and enclave dumps remain local-only under the audit's recorded
`artifacts/` paths and are unavailable from another checkout. The original CI
failure bundle could not upload because of artifact quota; its job console and
local reproduction are retained separately.
