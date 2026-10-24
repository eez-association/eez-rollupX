# Offline proving review against the published protocol

| Field | Value |
| --- | --- |
| Date and status | 2026-10-23; local review and targeted validation complete; ready for PR review, subject to CI |
| Scope | Preserve the authorized fresh-network offline design while using the published `feature/e2e_edge_cases` protocol branch. No public deployment or existing-network migration. |
| Source | `codex/l1-history-offline-proving`, implementation checkpoint `b6dca63a2c956ddba82ed2874c9286e4b91a146e`, plus the patch identified in the validation record |
| Integration base | `8621b0368f4f42dc871dd84c0aaf5b6c4599bfbd`: `origin/main` at `a50c326d99f7cd1c21406eedbf370a344015a6b5` plus required v2 backport `00fe7bc2541a56f26f088d0a7baf567dac5059de` |
| Protocol | Published `feature/e2e_edge_cases` at `84a5b5207880f192a6a5896cbfe0bac5249db47f`; no submodule edits |
| Worktree | `/home/edu/Development/rollupX/artifacts/l1-history-offline-proving`; initially clean |
| Executables | SHA-256 identities for the process test, role builds, executed roles, Anvil and Forge are in [validation.json](validation.json). |
| Environment | Rust 1.95.0, Foundry 1.7.1, Solidity 0.8.34 / optimizer runs 1 / via IR, LLVM 22, locked dependencies, one Cargo build job |
| Related records | [Proposal](../../proposals/l1-origin-and-offline-proving.md), [requirement coverage](../2026-10-22-l1-history-offline-proving/coverage.md), [earlier implementation evidence](../2026-10-22-l1-history-offline-proving/README.md) |

## Findings and adaptation

The previously pinned local protocol commit `b5d94eaf...` could not be fetched
from the public remote. The requested branch still pointed to its parent,
`84a5b520...`. This was a delivery blocker even though the feature behavior had
local test evidence.

The integration now pins that published commit exactly (also the protocol pin
on `origin/main` at `a50c326d`). `.gitmodules` names
`feature/e2e_edge_cases` for deliberate future updates; ordinary checkout still
uses the immutable gitlink. A fresh bare repository successfully fetched the
exact commit from the public HTTPS remote.

The permanent history contract and its seven tests moved unchanged into
[contracts/src/L2](../../../contracts/src/L2/L1BlockhashContract.sol) and
[contracts/test](../../../contracts/test/L1BlockhashContract.t.sol). Its owning
[contract specification](../../../contracts/L1_HISTORY_SPEC.md) moved with it.
The genesis generator copies the local source into a disposable build tree at
its canonical Solidity source name. This keeps runtime metadata reproducible
without extending or editing the core checkout. The deployment image already
copies `contracts`, so this source reaches the image's generator.

[OfflineRollup](../../../contracts/src/OfflineRollup.sol) implements the published
`IRollupContract` interface directly. The core reference manager's initializer,
registration callback and `getCustomData` are not virtual at this pin. Owning the
profile manager locally avoids patching that reference or introducing a second
policy contract. Its eight proof-policy, owner-operation and helper bodies match
the published reference exactly. The only initializer is `initializeOffline`;
there is no context-free initialization entrypoint. Registration, initialization
locking, ownership, threshold, sorted membership/key lookup and the registry's
active-batch root-escape guard have dedicated regression coverage, including a
fuzz comparison against the pinned reference policy.

The registry's existing public `rollups(uint64)` getter is declared through a
small local interface. Both anchor modes retain their exact public-input bytes,
accepted-root binding and active-rule checks. Composer recognizes the resulting
manager through its new exact executable fingerprint; unknown or modified code
still fails recognition. Deployment uses this manager explicitly.

The source branch also retains its explicit unmerged dependencies: the v2
streaming backport at `00fe7bc2`, including conformance PR #89 at `759ae5f5`.
The merge at `8621b036` combined these with the current main base before feature
implementation. This review does not replace or discard that dependency history.

## Compatibility and review boundaries

- This remains a fresh-network profile active from genesis. No hardfork switch,
  legacy initializer, RPC-backed signer fallback or existing-database migration
  was added.
- The v2 transport, typed L1 witness extension, delay, PREVRANDAO, canonical header
  validation and both anchor-mode rules are unchanged by this adaptation.
- Genesis generation reports the same runtime, genesis artifacts and state root
  `0xfe1ff3f82c1f2b42fb8fdb1d72d5698282e55e609075fa7f572af8b0d2798817`.
- The manager's executable bytes change. New deployments and Composer's strict
  fingerprint must use this revision together; no upgrade of an existing manager
  is claimed or required. Owner and proxy-admin authority remain as documented.
- Frozen v2 vectors retain their generation provenance and expected responses.
  Their old source manifest is a historical capture, not the current gitlink.
  The manifest writer now uses the local history source on future regeneration.
- No Cargo dependency version/source/checksum or RPC schema changed in this
  adaptation. The old protocol bundle remains only as historical audit evidence.

## Validation

The [portable validation record](validation.json) identifies exact commands,
source and executable hashes, results, log digests and exclusions. Raw logs/datadirs under the
worktree's ignored `artifacts/` are local-only, not available in a fresh clone.

Completed against the source hashes in that record:

- Integration contracts: 120 passed, zero failed/skipped, including all 64 gas
  bound cases, history/storage-proof tests and the manager-policy regressions.
- Published core: 778 passed, zero failed, three live-fork skips, with the two
  required named exclusions. The seven history tests now run in the integration
  suite instead of the core suite.
- Genesis `--check`: passed without changing fixtures or runtime bytes.
- All 64 pinned Anvil scenarios passed on an unused owned port range,
  20000–20127, with chain IDs starting at 71337.
- Tooling: 387 passed and eight skipped (395 selected); the skips include
  optional live/image checks and two checks requiring CI-installed Gitleaks.
- Frozen v2 manifest/schema/fixture integrity: all 29 cases verified statically.
- Rust workspace tests: 1,564 passed, zero failed, three intentional ignores.
  All 29 frozen v2 cases replayed against real services, and strict manager
  recognition passed with instruction-mutation rejection intact.
- Workspace/all-target Clippy and the production feature selection build passed.
  This local build uses the debug profile and is not a release artifact.
- Native source-based process suite: all seven selected cases passed, zero
  failures/ignores, 171 filtered, in 715.09 seconds including the harness signer
  build. Signer network access was denied. The cases cover bidirectional native
  calls, nested CREATE2 fee quotes/settlement/replay, real-signer tampered-calldata
  rejection, both multipart scenarios, restart after partial consumption and
  same-block middle-group resumption.
- The expired-source case settles 2,000 L2 blocks in four parts. It requires
  rejection of an old anchor beyond the 256-block window, a complete bridge to
  a recent anchor, cold offline signers, both anchor modes, an L1 rule transition
  and complete independent replay. Assertions compare every block hash, state
  root and PREVRANDAO, every installed L1 hash and 32 application observations.
  Successful checkpoint stdout was captured by libtest; no per-part timing
  measurement is claimed.
- Rust/contract formatting, shell syntax, documentation links and whitespace
  checks passed; final counts are in the portable record.

Initial integration compile errors identified three imports needing the new local
history path. The first expanded test run then rejected an empty-initialized
OpenZeppelin proxy (`ERC1967ProxyUninitialized`); the new initialization test was
corrected to exercise the actual locked implementation and atomically initialized
proxy. The complete unchanged production implementation then passed all 120
contract tests. These are corrected test-development failures, not retry-only
passes.

The new manager runtime first failed the existing Composer recognition test
with the previous fingerprint. Its exact compiled executable hash and immutable
offsets were then updated, preserving the fail-closed recognition rule and the
regression's instruction-mutation rejection assertions.

The first tooling invocation failed because several existing shell procedures
use absolute `/tmp` templates and this host's `/tmp` quota is exhausted. The
successful invocation used the existing local-only `mktemp` shim, which replaces
an absolute `/tmp/` prefix with the owned `TMPDIR` and delegates unchanged to
`/usr/bin/mktemp`. No repository procedure, assertion or test selection was
changed for that result. The failing log is retained alongside the successful
run; this is an explicit environment correction, not an unexplained retry pass.

An initial Anvil launch was stopped with SIGTERM after detecting an occupied
port later in its candidate range. It had completed one scenario and started a
second, both on otherwise unused ports. Only that owned runner/process group was
stopped. The full successful run used the preflight-checked range above; existing
listeners and unrelated resources were not stopped or cleaned up.

## Final review and limits

The fresh review traced the published interface through deployment, registration,
proof policy, both context modes, Composer recognition and genesis generation.
The exact policy-body/context comparisons and compiled runtime fixture check
are recorded alongside the passing regressions. No unresolved implementation
finding remains in this adaptation. This is a self-review with reproducible
checks, not an external security audit.

The explicit production-feature build and the source-based process suite use
different Cargo feature graphs. All three un-hashed role executables changed
between those steps; both sets of hashes are retained. The process suite's
results validate its recorded debug binaries. Exact shipping release-artifact
execution remains a CI requirement. The validated source/config/test files did
not change during these checks.

- The complete native inventory was not repeated. Its earlier failures and
  separate corrected runs remain in the dated implementation audit. These seven
  targeted cases do not replace the full CI matrix.
- Live fork, real Docker/Kurtosis/PBS, release archive/promotion and public
  deployment checks were not performed. Core's three live-fork skips and named
  exclusions, and tooling's eight skips, are explicit in the record. Gitleaks
  was unavailable locally for two tooling checks.
- The optional four-day stress is not an acceptance requirement and was not
  repeated. The expired-blockhash boundary is covered directly by the passing
  multipart case. No performance or L1-finality claim follows from these tests.
- Existing-network activation/migration and legacy compatibility are outside
  this fresh-network profile. The ECDSA attester trust assumption is unchanged.

The branch is prepared for PR review with the published protocol dependency.
Merging still requires the repository's full CI gate. No public deployment,
wallet operation or unrelated working-tree modification was performed.

## Portable prover follow-up

The subsequent [portable prover review](../2026-10-23-portable-prover/README.md)
applies the approved cleanup and PR #73 adaptation on top of `21fc597e`.
It supersedes the handshake/compatibility description with identity schema 6,
keeps v2 and the same protocol pin, and records validation of the changed source.
The results above remain evidence for their recorded revision.
