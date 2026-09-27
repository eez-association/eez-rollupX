# Upstream Reth migration validation

[PR #41](https://github.com/eduadiez/rollupX/pull/41) passed local validation and
all twelve CI jobs at `ab5060afb29f7fe25dc2ae19413f281d5840520c`.
The subsequent cleanup removes unused vendor files and condenses documentation;
its own CI result is required before merge.

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

## Cleanup verification

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
