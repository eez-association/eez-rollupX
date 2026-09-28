# Pending RPC and API simplification follow-up

This follow-up starts at backport commit
`a0e8e3eb4b5c77edba3148801527c5ff466388e7`, in the same dedicated
`backport/native-system-txs-issue-35` branch and worktree. PR #42 remains the
required base, `0b7beed5ca48157d720d6685435ba2121c1e9c1e`; the pinned protocol
remains `9d13218a355e7d543ca8a9927eea2962600b716d`. The owner authorized all three
follow-ups identified in review. No deployment or existing network was modified.

## Changes and regression

The previous pending RPC fallback used the parent's RANDAO. A live acceptance
refreshes only descendants, so that value can differ from the actual next block's
seed. The new regression on the pre-fix implementation failed with seed `…07`
where the live anchor required `…08` (`followup-rpc-baseline-3.log`). Earlier
baseline attempts failed during test setup/compilation and are not reproduction
evidence. The baseline is a unit regression using a mocked canonical L1 RPC;
it is not an end-to-end reproduction.

Both runtime roles now pass their existing canonical L1 history and correlation
readiness gate to the RPC builder. The builder reads the next timestamp and seed
together on each fallback request. It rejects absent provenance and unfinished
boot catch-up; genesis history is installed before historical anchors finish
rebuilding. Executed pending payloads still use their own execution environment.
No second seed cache or legacy fallback is introduced.

The regression checks the live refresh, the actual EVM environment, a later
same-height catch-up retaining that refresh, and a reorg/history rebuild removing
it. Separate checks cover unconfigured/uninitialized history, boot readiness,
deterministic genesis defaults and rejection of nonzero beacon overrides.

The EVM caches the immutable genesis schedule, including decoding errors, in a
shared `OnceLock` so clones reuse it. All four execution entry points retain
per-block validation. L1 simulation does not consult this cache. A regression
checks repeated missing-profile errors across clones and profile-free L1
simulation; existing header/Engine mutation tests continue checking each block.

The checked V0 payload's unused `encode()` method only copied its retained wire
bytes and could not fail. It was removed; its two test callers now use the
borrowed `encoded()` view. Independent producer vectors and property tests still
exercise the actual encoder. No transaction, genesis, database or DA format
changes were introduced. Two test-only dependency edges use versions already in
Cargo.lock; no dependency revisions changed.

## Evidence

Detailed logs under `artifacts/native-system-backport/logs/followup-*` are
local-only. The [validation record](follow-up-validation.json) lists commands, results and digests.
The source manifest and process logs are also local-only; they are not available
merely by checking out this audit. This record supplements the original audit;
its earlier full-suite results do not establish validation of these edits.


| Validation | Result |
| --- | --- |
| EVM, node-common, payload/protocol/blob codec packages | 267 passed; 0 failed/ignored |
| Node-common after adding boot-readiness guard | 17 passed; 0 failed/ignored |
| Node-common without L1 startup feature | 9 passed; 0 failed/ignored |
| Workspace Clippy, all targets/features, warnings denied | Passed |
| Composer/follower/signer production feature build | Passed, optimized dev profile |
| Source-based changing-static cross-chain scenario | 1 passed; 23 unrelated tests filtered; settlement, RPC state reads and fresh follower replay asserted |
| Format, diff whitespace, integration contract build | Passed |
| Offline dependency checks under existing policy | Passed; audit retained 9 allowed inherited warnings |

The 365 inventoried Rust/schema/manifest files were unchanged through the process
run. Executable and source-manifest digests are in the validation record. The
baseline's failing seed assertion is fixed; no test required a retry without a
code correction. Earlier setup/compiler/lint and sandbox-lock failures remain
in local logs and are not counted as coverage.

Full workspace runtime tests, other E2E targets, the full contract/protocol
suites, release builds and Kurtosis were not rerun for these narrow follow-ups.
The targeted process scenario validates semantic state reads and independent
follower replay, but is not a dedicated process-level RANDAO boundary test; that
boundary is covered by the mocked-L1 unit regression. No performance claim is
made for the schedule cache. No additional unresolved issue was found in this
follow-up review.
