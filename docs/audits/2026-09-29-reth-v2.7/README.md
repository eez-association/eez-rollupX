# PR #44: Reth v2.7.0 and local CI validation

Status: local validation in progress; nothing pushed from this follow-up.
The [upgrade contract](../../proposals/reth-v2.7-upgrade.md) defines scope and
acceptance. No public network or operator service was changed.

## Source and environment

| Input | Identity |
| --- | --- |
| PR #44 baseline | `3f95f1848576cb19f9bf6e7ae4151c2e5eb3757c` |
| Required PR #42 head | `cf8500e80cc10b58ab6acd59b2125dd098aa770c` |
| Merged main base | `de5030c782e29ff07150b2728c7b564fd607f802` |
| Fixture-path fix | `b56fb13` |
| Initial Reth upgrade candidate | `239d32c9c7a57c4c01ed79d7feefb438b246b2dd` |
| Corrected runtime / release-artifact candidate | `6387e3d513aad09d4275b448783e32dbf10e7bc9` |
| Reth v2.7.0 | `3d592ece6de8c4559987416a544fc215fd6d6921` |
| Protocol gitlink | `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Tools | Rust 1.95.0; Foundry 1.7.1; Nextest 0.9.143 |
| Branch | `backport/native-system-txs-issue-35` |
| Dedicated worktree | `/home/edu/Development/rollupX/artifacts/native-system-backport/rollupX` |

The original operator checkout's uncommitted work was preserved. The candidate's
merge base with PR #42 is its exact head above. Main remained unchanged during
the initial review and subsequent local checks.

Reth v2.7.0 is 32 commits beyond PR #41's `444e867` pin and includes its required
forkchoice fix. No runtime Rust API adaptations were needed. All 102 Reth Git
packages in the shipping lockfile and all 19 in the standalone Stateless lockfile
use the same release commit. Required registry changes are `revm-inspectors`
0.43.0 → 0.44.0 and `alloy-eip2930` 0.2.3 → 0.2.4. REVM, core Alloy, Rust,
protocol source, and genesis fixture bytes are unchanged. Vendor patch reverse
checks pass. Adapter source links to `444e867` retain their original provenance;
the linked upstream source files are byte-identical at v2.7.0.

## CI failure and regression

[Run 36546438201](https://github.com/eduadiez/rollupX/actions/runs/36546438201)
built the release artifacts but failed 13 signer tests. Two fixture helpers
used compile-time `CARGO_MANIFEST_DIR`, pointing to Docker's `/build` after
Nextest moved execution to the host checkout. Other fixture helpers already
honored Nextest's runtime environment.

One shared test-only manifest-directory helper now handles all four runtime
fixture readers. Production signer validation is unchanged. Before the fix,
two exact tests representing both broken helpers failed with `NotFound` when
the original fixture directory was temporarily moved out of the way. The same
two checks passed afterward, reading identical fixtures from the relocated
directory. The original directory was restored after each check.

The actual Docker producer and exported release archives also ran locally:
**990 unit/vector tests plus 292 Composer tests passed** at `239d32c`, including
all 13 previous failures. The corrected `6387e3d` candidate passed **990 plus
290** after removing two obsolete preparation-estimator tests. Three manual
utilities remain ignored. These runs use the real Docker build context and
Nextest remapping, not a synthetic packaging fixture.

GitHub additionally rejected failure-diagnostic uploads because the repository's
artifact quota was exhausted. That optional upload failure did not cause the
unit-test failure. No historical artifacts were deleted.

## Fresh-devnet startup gap

The first local Kurtosis run at `239d32c` failed before its first deployment
broadcast: `scripts/deploy.sh` queried `finalized` before the new L1 exposed a
finalized block. No EEZ node or signer had started. The native genesis profile
requires that finalized reference; substituting another block is invalid.

The Kurtosis plan now waits up to 1,800 seconds for an error-free finalized-header
response before entering deployment. The deployment script retains its complete
header/genesis validation. The two new orchestration regressions fail against
the original plan and pass with the readiness gate. They cover delayed finality,
null/invalid response types, RPC errors, and absence of deployment on timeout.
The [Kurtosis wait primitive](https://docs.kurtosis.com/api-reference/starlark-reference/plan/#wait)
owns the bounded polling; no custom retry process or genesis fallback was added.

A first 900-second readiness run exhausted its bound while the mainnet-preset
L1 was still at finalized epoch zero. Its logs are retained. The corrected
30-minute bound remains fail-closed. The corrected live run passed this gate
after **25 minutes 42 seconds** (772 polls), receiving finalized execution block
64 before starting deployment. The full workload result is still pending. The
plan's exact source hash is retained beside the run, separately from the frozen
runtime candidate identity.

That run then exposed another orchestration defect: the relay had registered
128 validators before finality, but the readiness loop read only Kurtosis's
default last 200 log lines. Later duplicate-registration messages reported zero
new registrations, so a healthy relay timed out. Its complete retained log
contains the earlier successful registration; deployment bindings had passed.
The runner captured diagnostics and removed only its owned enclave.

The relay, builder and canonical-settlement readiness gates now request `--all`
logs, retaining their existing bounded polling and required success signals.
The new regression fails all three gates before this change when success is
followed by 250 unrelated lines, and passes afterward. Empty registration sets,
unavailable logs and observer receipts without canonical advancement still fail.
The follow-up devnet run uses the same verified `6387e3d` runtime images with the
corrected orchestration scripts; their exact hashes are recorded separately in
`release-final/kurtosis-readiness-source.sha256`. Runtime code is unchanged.

## Completed local checks

| Command / path | Result |
| --- | --- |
| `cargo +1.95.0 fmt --all --check`; `git diff --check` | Passed |
| `cargo +1.95.0 clippy --locked --workspace --all-targets --all-features -- -D warnings` | Passed |
| CI's default-feature workspace Clippy command | Passed |
| `cargo +1.95.0 test --locked --workspace --exclude eez-node-e2e` | Initial 1,282 passed; corrected candidate 1,280 passed; 3 manual utilities ignored |
| `cargo +1.95.0 test --locked --manifest-path vendor/stateless/Cargo.toml --workspace` | 26 passed |
| Signer Rustdoc with `RUSTDOCFLAGS='-D warnings'` | Passed |
| `cargo deny check advisories bans sources`; `cargo audit` | Passed under existing policy; 9 allowed audit warnings |
| Initial CI tooling suite; readiness extensions | 274, then 276, then 278 passed; 5 opt-in skips |
| `python3 scripts/check-settlement-policy-config.py` | Chiado and Kurtosis passed |
| `forge test --root contracts` | 84 passed |
| Pinned core tests | 517 passed, with the required exclusions below |
| Genesis fixture check with Rust 1.95.0; verified release-helper check | Passed; fixture state root unchanged |
| Real `scripts/ci/build-runtime-artifacts.sh` producer, publication disabled | Shipping binaries, both release test archives and node/signer/deploy images built |
| `scripts/ci/prepare-runtime-artifacts.sh` | Exact source, protocol, lockfile, image and binary checks passed |
| Exported release unit lane | Initial 1,282 passed; corrected candidate 1,280 passed; 3 ignored |
| Corrected Composer/L1 unit tests | 447 passed; 1 manual utility ignored |
| Source-built Ethereum/Gnosis ordinary-burst process regressions | Both passed at `6387e3d` |
| Complete embedded release suite | 36/36 passed in 2,362.646 seconds; 1 live-enclave case ignored |
| Full protocol Anvil confirmation, `BASE_PORT=24545` | 36/36 passed; initial timeout below retained as a flake |

The initial upgraded tree supplied the standalone Stateless, contract,
dependency, Rustdoc and genesis checks. Those inputs did not change afterward.
Workspace tests, both Clippy configurations, affected Composer/L1 tests, tooling,
and the complete release producer/verifier/unit path were repeated after the
scheduling and readiness corrections. The [validation record](validation.json)
retains the final runtime and test manifests, executable digests and log hashes;
its pending process entries are not successful validation claims.

Core command:

```bash
forge test --root eez-core-protocol --no-match-path 'script/*' \
  --no-match-test 'test_Doc_OriginalValue_SeedInSetup|test_PostCost_Incremental'
```

The excluded cases are not coverage. The initial native process run stopped
after reproducing two failures: 21 passed, two failed, one interrupted and 12
not run. Cross-chain had not started. Its owned processes were stopped before
rebuilding. The complete corrected embedded release suite passed; cross-chain
and Kurtosis are still running.
Both default-feature and all-features workspace Clippy passed after the
scheduling change. The source-built burst tests passed; the subsequent source
late-window test was interrupted during compilation, before test execution, to
use the now-available verified release archive. It is not counted as coverage.

The tooling suite's five opt-in skips are the data-only Docker payload probe,
relay-resource probe, tiny-workspace archive relocation probe, and two Docker
runtime fixture probes. The full production producer/verifier/archive path was
executed separately. Skipped fixture probes are not reported as passed tests.

Native archives retain four explicit manual cases: the two large-history cold
feasibility checkpoints, the checkpoint requiring externally retained history,
and `kurtosis_policy_batches_replay_independently_from_l1`, which requires its
specific live-enclave evidence. They are not selected by the normal release
suites and are not included in their pass counts.

The corrected release cross-chain run exposed a timing race in its independent
Gnosis V0 test writer. The signer successfully proved and signed the 30-block
window in 26 ms, but target selection occurred at the end of the slot: the
response arrived at `12:39:38.017 UTC` for target `12:39:38 UTC`. The test's
explicit-target check failed before submission. Its datadir is retained at
`release-final/native-datadirs/eez-cross-chain-2526519-1qnmru`; the original
output remains in `release-final/cross-chain.log`.

The test writer now waits for a new canonical L1 block after KZG/witness
preparation and before timing its proof. It keeps the exact-target and parent
checks, attestation verification, receipt/root assertions and clean follower
replay. The boundary wait is reported separately and excluded from proof timing.
Production signer and runtime code are unchanged. Validation of the updated
release test is pending; the first failed run is not a passed suite.

The same initial cross-chain run also failed
`automatic_settlement_sixth_failure_exhausts_sources_and_releases_nonce_suffix`
before fixture readiness: embedded L1 could not bind WebSocket port 34826.
The port lease already reserves HTTP and its adjacent WebSocket socket until
child launch, but cannot make the release/subprocess bind atomic. After the
failed child exited, `ss` found no remaining listener or connection on that
port; its former owner is unknown. No other service was stopped. The retained
datadir is `release-final/native-datadirs/eez-cross-chain-2567279-ohfdbz`.
This startup failure remains recorded separately from semantic test coverage.

## Retained failures and evidence limits

`BASE_PORT=23545 bash scripts/ci/run-protocol-anvil.sh` passed 35/36 scenarios.
`deepNested` timed out after 300 seconds during L1 infrastructure deployment.
Its next transaction remained queued with nonce 3 while the account nonce was
also 3. The owned Anvil state, pool and broadcast records were retained before
cleanup. An isolated retry with identical source, chain IDs and ports passed,
including entry/call verification. A complete confirmation run subsequently
passed all 36 scenarios. The first timeout remains a **flake**, not a clean
first-run suite pass; neither protocol nor Anvil source changed in this upgrade.

The first diagnostic retry had an incorrect scenario path and stopped before
execution; the verified path was then used. An initial E2E export similarly
stopped before tests because its shell lacked `CARGO_TARGET_DIR`; the subsequent
export used the actual verified unit extraction. The genesis script's first
Cargo fallback selected host Rust 1.96.1; that owned build was stopped and the
check rerun successfully with `RUSTUP_TOOLCHAIN=1.95.0`. None of these setup
attempts counts as validation.

Bulk logs, datadirs, manifests, source/executable hashes and before/after checks
are **local-only**, under `artifacts/reth-v2.7-review/` in the dedicated worktree.
They are not present in another checkout. The canonical local L1 uses official
Reth v2.7.0 image digest
`sha256:a7f059d470878a5a59f3c575f3fc81b536d0d2d56d7a1cbed73e717a4f766336`.

## Compatibility

PR #44's fresh genesis/datadir requirement remains in force. This follow-up
introduces no additional genesis or protocol-format change and no migration or
legacy decoder. Reth v2.7.0 enables persistent block-access-list storage; historical
database upgrades, mixed binaries, and downgrade of candidate-written databases
are not validated. Deploy matching Composer/follower/signer versions. No public
Chiado deployment, release-tag promotion, or operator data recovery was attempted.

## Ordinary historical-drain failure found by process validation

At runtime candidate `239d32c`,
`ethereum_ordinary_burst_is_budgeted_across_sync_intervals` failed after 242.5 s.
Preparation took about 929 ms and crossed the old proof-start cutoff by 44 ms.
The recent preparation estimate then became 2,022 ms. With only two seconds
until each target, every subsequent selection chose head + 2. The native proof
check correctly rejected the still-unknown canonical parent, and settlement
never progressed. The Gnosis variant passed in the same first run.

The correction keeps target height at the known canonical head's immediate
child, bounds missing-slot timestamp offsets, and uses the existing complete
pipeline deadline for ordinary proofs too. It removes the incompatible recent
preparation predictor, preserves measured diagnostics, and retains independent
signer authorization, exact target binding, and final submission checks.
The focused target-selection regression failed before the correction and passed
afterward. Both source-built Ethereum/Gnosis burst cases and the full embedded
release suite passed with the correction. The initial failing
logs/datadir remain under `release/embedded.log` and `release/native-datadirs/`.

The baseline `late_sync_can_post_due_history_below_three_hundred_blocks_to_a_fresh_target`
case also failed: resuming 3.5 seconds into a five-second slot selected head + 2
and failed the same parent check. Its regression now requires settlement in the
original target's remaining 800 ms, exact L1 root application and follower
convergence. The former assertion demanding a later timestamp represented the
obsolete full-proof start allocation, not the native proof contract. The
corrected release regression passed in 37.060 seconds.

## Upstream Gnosis and Stateless dependency replacement

At the owner's request, the new dependency candidate uses Gnosis PR #138
**unchanged**, pinned to `9517ba3b3c139ed43d3e0814b60ec67b95e163d7` and its
`gnosis-primitives v0.2.9` commit `6ac3d3fd46213d59fc27bc36c4f273132a065811`.
Those two vendor trees and patches are removed. The PR was still open when
inspected; descriptions, commit/diff, review threads and passing upstream checks
were reviewed. Reth manifests use `tag = "v2.7.0"` to match the upstream Cargo
source identity, and the lockfile still resolves all 102 packages to `3d592ec`.
Using a `rev` source alongside upstream's `tag` would duplicate Rust types even
when both resolve to the same Git commit.

Unmodified upstream default features add the JIT compiler build dependencies.
The lockfile pins its eight `revmc` packages to
`87ea260a9bd25759beab64753d2f36227891ca4a`, the revision in both upstream release
lockfiles. Eleven registry package/version pairs are added; no existing registry
package/version pair is removed or upgraded by this replacement. The custom L2
EVM remains interpreted. The shared Docker/CI installer adds LLVM 22, Polly,
Clang 22 and `m4`. The actual Docker `tools` stage passed. Host installation
required interactive sudo authentication, so local compilation instead uses
packages extracted inside the dedicated worktree. A local `llvm-config` wrapper
corrects the extracted package's absolute Z3 library path; the first uncorrected
check failure is retained. No host compiler defaults were changed.

A dedicated Stateless branch, `feat/reth-v2.7-native-system-txs`, is committed at
`0bda6b200c1eb4689c147c080c3c248b50be2ea8`, based on
`ffe41a37f67b7d77a4f32c19d7fcbd10ca7ea0f3`. It reuses upstream generic recovered
validation commit `5c6be98` with attribution, retains the trie fixes, aligns Reth,
and exposes the existing pre-transaction checkpoint used for B[0]. Its 26 tests,
formatting and strict Clippy pass, including the empty checkpoint-selection
case. EF/RISC-V suites were not run. GitHub rejected the push with HTTP 403;
the owner elected to publish the branch manually. The final Stateless Git pin
and vendor deletion remain pending that publication.

## Completed frozen cross-chain and Kurtosis runs

The complete `6387e3d` release cross-chain suite finished with **71 passed,
3 failed, 3 ignored**, in 3,838.841 seconds. In addition to the retained Gnosis
proof-target race and WebSocket-bind failure above, the Ethereum thirty-request
admission test failed during its warm-up. After accepting a new live endpoint,
sequencing repeatedly failed with `candidate changes its authenticated seed`.
Its node/signer logs and datadir are retained under `release-final/`; this was
not a successful thirty-request workload.

Reconciliation previously acquired a mutex but could inspect the old head while
an already-issued engine command was still executing. Sequencing also captured
its seed before locking and enqueued after unlocking. The correction waits on
an actor barrier before reconciliation, captures attributes and enqueues under
the lock, and rejects a prebuilt block whose parent or authenticated context has
changed. A controlled in-flight-sequence regression fails against the previous
implementation and passes after the fix. Both synchronization regressions and
all 46 driver tests pass. The new source-based Ethereum/Gnosis process checks
are pending; the old release failure remains a failure.

The subsequent Kurtosis readiness run passed finality in 25m44s, all startup
gates, native waves, state chaining and poison recovery. All **32 upstream
protocol scenarios passed**. Independent evidence passed for **30/32**;
`revertCounter` and `topLevelStaticCounter` failed because the collector expected
removed `.payload.blocks` fields. The run is **failed**, not a full E2E pass.
It retained 28 signed windows and 28 remote attestations before owned-enclave
cleanup. The summary and logs remain under `release-final/kurtosis-readiness/`.

The updated collector uses strict V0 counts and explicit calldata endpoint
claims, checked against canonical L2 headers. It retains KZG, exact trigger,
entry/trace/receipt, rolling-hash and absence checks; calldata claims alone do
not establish settlement. The CI prefix decoder now accepts native transaction
and receipt types. The independent follower test decodes V0 operations and
compares metadata, ordinary transaction hashes, ancestry and the final block
commitment. Its live-enclave execution remains pending.

Both positive lookup scenarios and their two-block variants fail against the
previous scripts (four retained subtest failures) and pass after adaptation.
All 16 mocked shell/RPC lookup tests and seven Rust evidence/prefix tests pass.
The full CI tooling suite has **278 passed, 5 skipped**. Workspace all-feature
strict Clippy, formatting and dependency audits pass for the new tree. Workspace
unit and affected process runs are in progress. These results do not validate a
new shipping artifact yet; a fresh producer and release/devnet run are required
before pushing #44.

The retained shared batch further establishes that both failing lookup cases
observed B[0] with zero ordinary terminal transactions, before a later native
suffix. After fixing V0 window extraction, the old post-transaction-only prefix
helper would still reject that valid position. The CI helper now replays the
node's execution witness with Stateless and the custom L2 EVM, compares actual
receipts, and includes the pre-transaction checkpoint. It reads chain settings
from the test node's `debug_chainConfig`; canonicality remains the collector's
RPC-bound integration check. It changes no production signer authorization.
Nine helper regressions and all 16 shell lookup tests pass. The helper tests are
now included in the release unit archive.

The real tiny-workspace archive/relocation check passed all 29 tests with no
opt-in skip, including a marker proving the new example regression executed
after removing the producer checkout and making compilers unavailable. Initial
setup failed because the host lacked Mold; it was extracted inside the worktree
with its shared libraries. A subsequent fixture run exposed its older package
inventory placing `issue35` in `eez-protocol`; that fixture now matches the real
`eez-payload-codec` inventory. Both failures are retained separately from the
successful relocation run.

The upgraded workspace unit run passed **1,282 tests**, with three manual
utilities ignored. The Gnosis V0 process regression passed in 112.60 seconds,
including blob-only follower reconstruction. The first thirty-request source
attempts failed before node readiness because the lockfile changed while the
separate signer/composer builds were running. The capability identity check
correctly rejected those mixed builds. No identity checks were relaxed; the
rerun uses fixed inputs. Both initial datadirs/logs are retained. All-feature
workspace strict Clippy also passed after the witness-based CI helper change.

The fixed-input source rerun passed both Ethereum/Gnosis thirty-request cases
in 191.78 seconds, each requiring settlement in one Sync block. This reproduces
the previously failing Ethereum workload after the synchronization fix. These
process binaries precede only the diagnostic stale-candidate log wording update;
no execution or validation behavior changed afterward. The nine prefix-helper
unit tests also pass. A final full shipping-artifact and Kurtosis run remains
pending publication/pinning of the prepared Stateless dependency.

Remote refs were rechecked: main, PR #42 and the published PR #44 head remain
at the identities above; the local merge base still equals PR #42's head.
The original operator checkout has exactly its previously recorded edits.

The final local tooling rerun passed **278 tests**, with five opt-in checks
skipped, in 151.357 seconds. The archive relocation opt-in was exercised
separately as recorded above. All 46 driver tests pass after the diagnostic
wording update. Their final log digests are recorded in `validation.json`.
The prepared Stateless branch was still absent from the remote at the handoff
for the owner's manual publication. PR #44 has not been pushed with these changes.

## Published Stateless dependency and final candidate

The owner published `feat/reth-v2.7-native-system-txs` over SSH. Remote
verification confirmed commit `0bda6b200c1eb4689c147c080c3c248b50be2ea8`.
The root now pins both Stateless dependencies to that revision; `zeth-mpt`
resolves from the same commit. The remaining vendor tree and patches are deleted,
and Docker contexts, Cargo-state inputs, fixture tests and signer documentation
use the published dependency. Comparing the before/after lockfiles confirms no
registry package/version/checksum changes and one Reth v2.7.0 Git source identity.

The final shipping build and full release/devnet checks below must validate this
new pin; earlier vendored-candidate results are retained as historical evidence.
