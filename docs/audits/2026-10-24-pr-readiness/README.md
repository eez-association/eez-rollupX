# Native-v1 and offline history: PR closure and extended validation

Status: in progress, 2026-10-24 UTC. This record supersedes the implementation
snapshot in the [compliance review](../2026-10-24-specs-compliance/README.md) only
where a correction and new result are recorded. It does not turn earlier failed
or unexecuted checks into successful coverage.

## Scope and identity

- Worktree: `/home/edu/Development/rollupX/artifacts/l1-history-offline-proving`.
- Branch: `codex/l1-history-offline-proving`; initial HEAD
  `3e23955dc477c9e675364776f118b1cecda75774` plus the existing accepted changes.
- Latest fetched target `origin/main`: `a50c326d99f7cd1c21406eedbf370a344015a6b5`;
  the starting branch is nine commits ahead and zero behind.
- Core gitlink/checkout remains clean at
  `84a5b5207880f192a6a5896cbfe0bac5249db47f` (`feature/e2e_edge_cases`).
- Fresh networks only; native-v1, the local compact V0, one profile/API, both
  anchors, no prover RPC and independent ordinary L2 production remain unchanged.
- The user authorized a new isolated Kurtosis network and extensive live protocol,
  fee and cross-chain validation. Existing networks are not deployment targets.
- Rust 1.95.0, locked dependencies, Foundry 1.7.1. Local Cargo uses a private
  `bwrap` bind for `/tmp`, because the shared temporary directory has exhausted
  its quota. No shared temporary directories are removed.

## Corrections and independent review

| Finding / requirement | Change | Fresh result |
| --- | --- | --- |
| R01: genesis exact-account assertion omitted FeeVault | Include the approved sixth predeploy; assert the configured collector points to it and has executable code. Keep zero funding, exact account set and proxy assertions. | `cargo +1.95.0 test --locked --offline -p eez-evm --lib`: **37 passed**, no ignored/failed cases. |
| R02: driver default build omitted a required dependency | Make `eez-primitives` unconditional; remove redundant feature gates for that dependency. No dependency version or lockfile change. | `cargo +1.95.0 test --locked --offline -p eez-driver --lib`: **52 passed**, no ignored/failed cases. |
| Unambiguous external-client contract | Link native-v1, local DA count grammar, profile and RPC contracts directly from the Nethermind integration guide. No new selectors, versions or compatibility modes. | 713 local links across 63 changed/new Markdown documents resolve after fixing two historical schema links. The indexer guide explicitly documents the `burnWei` to `baseFeeWei` field rename. |
| Fresh independent review | [Review and 27 source hashes](independent-review.md), including correction of an initially missed treasury process test. | No additional confirmed implementation defect. Static review; runtime evidence is recorded separately. |

The baseline failures are retained in the compliance audit. The corrected tests
reach the original assertions; no test was skipped or weakened to obtain these
results. Neither R01 nor R02 changed Cargo.lock.

### Additional findings from the extended validation

The first full process run selected 179 tests. Its first two chained-interstate
cases failed before submitting cross-chain work, and the run was deliberately
interrupted during the third case (exit 130). A separate invocation of the first
case with the same executable reproduced the precise failure: `composer did not
report a fresh sync-slot invocation boundary: timed out after 90s` (0 passed,
1 failed, 178 filtered). Node logs show continued ordinary production and accepted
idle cutoff decisions. An idle cutoff no longer invokes Composer; waiting for
that invocation before enqueueing work deadlocked the test's collection barrier.

The five chained-interstate tests now synchronize on an accepted idle cutoff.
The corresponding idle-boundary assertion in `settlement_policy` and Kurtosis's
partial-consumption canary were adapted to the same signal. The canary's Python
regression failed before the correction and **7/7 passed** afterwards. Co-batching,
state/order, partial-consumption, pacing and replay assertions remain intact.
The corrected final process run has passed all five chained-interstate cases;
the completed suite has **172 passed, 2 failed and 5 ignored**, with both failures
explained below. These runs deny outbound network access
to the signer and use four separately frozen source-built executables, whose
hashes were checked before execution.

The later general-block consolidation test timed out on an obsolete zero-transaction
predicate. Its retained logs show six successful canonical 30-block settlements;
mandatory history transactions made the included-transaction denominator nonzero,
including in the later ranges with zero pure activity. The [focused investigation and test
correction](general-rule-native-history.md) preserve the existing cost metric and
policy. The corrected fixture has no application transactions and verifies the
exact applied range, native history bodies, transaction roots and receipt-derived
costs. Its general-rule assertion does not assume that a later activity snapshot
equals the finite range retained for publication. This test-only correction passed its first isolated execution (**1/1**, 178
filtered, 37.23 seconds) against the same frozen pre-R03 production roles, with
outgoing signer connections denied. The original full-run failure remains recorded.

Review also found stale NatSpec claiming the native history import consumes an
account nonce. The comment now states the actual gas-metered, nonce-free behavior.
Solidity metadata changes the runtime identity even though the history contract's
executable instructions are byte-identical. The canonical generator updated the
history runtime/hash and genesis fixtures. The OfflineRollup runtime fixture and
its exact estimation fingerprint were updated together; normalized length and
immutable offsets did not change. The new fixture state root is
`0x76df720c92bb7bf7ff3a42031b3af51b9a2db7d91abf2da3a810ca00a89273bf`.
This requires new release images and offline captures. Earlier binary-dependent
results below describe the preceding snapshot, not these regenerated bytes.

The first Kurtosis deployment attempt exposed a packaging omission before any
broadcast: `Dockerfile.deploy` did not include the generated history `runtime.bin`
now required by `--render`. Independent review also found that the Dockerfile's
specific ignore excluded its source directory. The producer now copies the exact
file at its consumed path and includes it in the effective build context; the
renderer remains strict. The real producer failed with `CopyIgnoredFile` before
the ignore correction and built afterwards. Its Docker-context probe also passed.

A subsequent deployment broadcast succeeded, but the non-root signer could not
read the rendered genesis. `deploy.sh` had used a `mktemp` file with mode 0600 as
the renderer's base. It now preserves the original base's mode before rendering,
without changing bytes, credentials or the signer's user. Preflight tests cover
0644, 0664, 0640 and 0600, and all **15 passed**. The live-validation record retains
the failed attempts and distinguishes their chains from the fresh successful
startup. The final deploy image's runtime/helpers match their source inputs; all
seven Rust binaries remain byte-identical. The archived failed deployment does
not include its L1 datadir and cannot reproduce that full chain history.

The final process run also exposed a collision between the two validation tracks:
Docker bound port 32843 for the new enclave while a source-test signer was
initializing KZG after releasing its port lease. The harness's TCP-only readiness
probe mistook Docker's listener for the signer; node discovery then failed, and
the signer reported `EADDRINUSE`. The attester-mismatch test did not reach its
signature check. Container identity/timestamps establish this cause. Its isolated
diagnostic repetition passed **1/1** in 37.71 seconds with the same four frozen
executables: two real signatures, two wrong-attester rejections and the original
zero-submission/nonce/commitment assertions. This retry-only pass is a diagnosed
infrastructure/concurrency flake; the full-run failure remains a failure.
[Collision evidence](signer-port-collision.json) records the attribution. Topology
creation is suspended during source-test startup.

The live blob baseline exposed **R03: pending RPC simulation at a history update**.
Forge deployment asks for `eth_estimateGas` at `pending`; Reth's environment-only
fallback has the parent state and correctly refuses to omit the mandatory native
import. Forty controlled Counter-initcode probes confirmed working default/latest
estimation and four pending rejections at one stable parent (L2 374). The
[baseline evidence](pending-rpc-baseline.json) preserves that failure. The
[RPC adapter](pending-rpc.md) now executes the real native prefix before public
call, gas and access-list simulation. Node-common unit tests pass with both the
default features (**15**) and `l1-startup` (**31**); control-rpc tests (**7**) and
focused Clippy also pass. The focused process regression passed **1/1** in
9.78 seconds with one blocking RPC permit and signer network denial;
[pinned inputs and executable hashes](pending-rpc-validation.json) distinguish
this corrected test from its first failed probe of a nonexistent Solidity getter.
No production bytes changed for that probe correction. The R03 release has since passed 60 live Counter probes, including ten stable
samples at five history-update boundaries. The original smoke also deployed and
executed through Forge successfully; its final summary then exposed an oversized
command-line argument, corrected below. These R03 binaries precede the later
shipping-feature correction; the corrected final release payload remains pending.
Existing frozen process executables and live images below precede that RPC-only
correction. [Dependency comparison](pending-rpc-dependency-edge.json) confirms
one direct edge to an existing dependency, with no external version or public
validation-identity change.

A separate review of the RPC author's implementation checked the exact pinned
Reth `3d592ec` call, estimator, access-list and blocking-task paths, the shared EEZ
prefix executor, production registration and the new regression assertions.
The reviewed `pending.rs` SHA-256 is
`b49a0af32d69dcdee043113ad5d08933e815ed15f5938462b20270b8a7db83c1`.
No additional defect was confirmed: the parent state and checked header snapshot
stay paired, the worker retains its permit, parsing and diagnostic overrides
remain compatible, and failed prefix execution cannot become a successful
simulation. The scope is explicitly limited to three public RPC methods; Engine
auth and other debug/simulation methods retain their existing guarded fallback.
This static review is separate from the focused process result and does not
substitute for the pending live results.

The live smoke's final summary passed accumulated blob evidence to `jq` through
`--argjson`. After the maintenance catch-up, its 37 verified batches exceeded the
OS per-argument limit and the script exited 126 despite having completed its
protocol checks. The correction reads both accumulated JSON values from files
with `--slurpfile`, preserving every row and field. A regression above 300 KB
reproduced the failure before the fix; the affected Python suite then passed
**10/10**, and shell syntax passed. A read-only repetition used the same three
transactions and 37 blobs, with no new broadcasts, and verified both correlation
directions, Counter 42 and the canonical Registry/hash at safe block 4950. This
does not erase the failed original invocation or claim a fresh whole-script run.

The R03 network passed all 34 default eligible protocol scenarios with the real
signer. Explicit additional selection passed `staticThenNestedWrites`; the
`staticLocalWrite` execution succeeded but exposed a missing independent oracle
in the evidence collector. Its [specific correction](static-local-write-oracle.md)
checks candidate retry/reuse, exact callback results, semantic actions and the
authenticated root. Thirty focused tests passed, and a read-only continuation
validated the original canonical transaction without a new broadcast. The
original two-scenario runner remains recorded as one pass and one failure.
The two missing-proxy negative scenarios were rejected during fee quotation,
before their intended terminal queue-rejection oracle could run; they are not
passing coverage of that oracle.

The cross-chain wave workload also exposed a configuration-reader error:
single-quoted YAML `true` was interpreted as disabled fees. The node correctly
rejected the resulting underfunded transaction before queue ownership. The
[reader correction](kurtosis/yaml-workload-regression.json) accepts paired scalar
quotes, retains explicit empty values and refuses to enable malformed literals.
Its 13 focused tests pass. The four repeated wave modes passed with unchanged
node configuration: 40 cross-chain operations plus six ordinary transfers. The
state-chaining stage initially hit the unchanged quote admission bound before
queue ownership; a produced Sync had not yet settled. The corrected harness
collects after a fresh accepted idle cutoff. Its four focused tests and the live
state-chaining repetition passed, preserving same-block and ordered-return
assertions. The first complete wrapper remains failed; details are retained in
the [wave record](kurtosis/cross-chain-waves.json) and
[barrier regression](kurtosis/state-chaining-regression.json). The combined tooling suite then passed
**406 tests**, with **7 opt-in infrastructure/artifact exclusions** (413 selected),
in 293.54 seconds using the private temporary-directory bind. All changed tooling
input hashes remained unchanged during the run. This validates the YAML, lookup
and blob-summary corrections together; it does not replace live execution.

The live [partial-consumption canary](kurtosis/partial-consumption.json) first
failed to induce its scenario: the original three-user bundle settled completely.
The fee-enabled audit copy had raised source tips without raising its replacement
bid. A bounded higher replacement bid, with captured dispatch/pool timing, then
passed the unchanged assertions: committed first source, spent nonce for the
second, reverted third, distinct canonical applied prefix, and healthy next-nonce
progress. No runtime or repository canary bid was changed.

The first [cold follower replay](kurtosis/cold-replay-diagnostic.json) reconstructed
from genesis and checked all 183 selected full batches, but the test's Ethereum
receipt decoder rejected native type `0x45`. The [test-only correction](replay-receipt-fix.json)
uses Alloy's existing open receipt type for that RPC response. The same input and
R03 follower then passed **1/1** in 24.30 seconds: **3384 unique selected block
heights and 719 receipts**, through canonical endpoint 7194. All 112 maintenance
parts are preserved. The partial post's original full claim is retained separately;
it is not rewritten into an applied-prefix claim. The later full endpoint requires
derivation through that earlier partial execution. This is state reconstruction
with the same follower implementation, not implementation diversity. The
[contract review](closure-review.json) explains the selection and receipt limits.
Final CI shipping artifacts and their live repetition remain pending.

At 11:35:28 UTC, overlapping host and Docker compilation exhausted host memory.
Kernel diagnostics confirm a global OOM killed the host `eez-node` compiler and
other user-session processes. The owned Docker build was cancelled with exit 130;
its cancellation is distinct from the host build failure. The user was informed.
No unrelated service was deliberately stopped or restarted. Subsequent builds
are serialized with one Cargo job, an available-memory precheck and monitoring.
The first build and kernel diagnostics remain local evidence under
`artifacts/pr-readiness/pending-rpc/`; private machine details are not committed.

## Validation tracks

| Track | State / evidence |
| --- | --- |
| Core Solidity | `FOUNDRY_ISOLATE=false forge test --root eez-core-protocol --no-match-path 'script/*' --no-match-test 'test_Doc_OriginalValue_SeedInSetup|test_PostCost_Incremental'`: **778 passed, 3 skipped**. The exclusions are required by the root instructions; the three skipped mainnet-upgrade tests need explicit RPC/manager/owner fork inputs, absent here. They are not coverage. |
| Integration contracts | Final regenerated snapshot: `forge fmt --check --root contracts` and `forge test --root contracts` passed; **142 tests**, zero failures/skips. |
| Core Anvil scenarios | `scripts/ci/run-protocol-anvil.sh`: **64/64 passed**, serially, using `BASE_PORT=62000`. No failures, timeouts or unexecuted scenarios. These core-local scenarios do not establish real signer acceptance; live validation is separate. |
| Exact runtime/helper build | First shipping-selection debug build terminated with signal 15 (exit 143), without a compiler error. No cause established; log retained. The diagnostic repetition passed in 7m26s. Seven binaries/helpers were copied and SHA-256 recorded before further Cargo invocations. This is debug-profile evidence, distinct from Kurtosis's release images. |
| Workspace Rust | `cargo +1.95.0 test --locked --offline --workspace --exclude eez-node-e2e -- --test-threads=2`: **1585 passed, 3 ignored**, both before and after final regeneration/recapture, including integration oracles and doctests. Ignored cases are a manual timing profile and the two explicit fixture generation/inventory utilities, not runtime validation; the latter two were separately invoked for the final corpus. Process tests are separate. |
| Genesis | Initial check passed with state root `0xbf4ddb74585d342600860a07cebac1774c86c0fabfdde683fe696ac701d7e896`. After the NatSpec regeneration above, `bash scripts/update-eezl2-genesis.sh --check` passed again with root `0x76df720c92bb7bf7ff3a42031b3af51b9a2db7d91abf2da3a810ca00a89273bf`. |
| Configuration / tooling | Initial discovery: **390 passed, 8 skipped** (398 total); after canary/genesis changes: **393 passed, 6 skipped** (399 total). Final packaging/permission snapshot: **395 passed, 7 skipped** (402 total). The added opt-in Docker-context probe passed separately; the other six opt-in infrastructure/artifact probes remain exclusions. An intervening run without the private `/tmp` bind failed with quota/I/O errors and missing downstream outputs (400 selected, 41 subtest failures, 13 errors); its log is retained. Repeating with the established private bind passed. Settlement-policy configuration passes for Ethereum, Chiado and Kurtosis. CI's pinned Gitleaks 8.30.1 scanner is installed only in task artifacts, with its download checksum verified. |
| Candidate secret scan | Initial scan reported four SHA-256 source digests as API keys because of their adjacent path names. All four were independently verified against the named files. The audit manifest now labels every digest `sha256`, preserving all 1,725 recorded values; a repeat scan of 367 candidate files passed with no findings. All nine existing branch commits were also scanned with no findings. No scanner exclusion was added. The pre-draft scan covered 387 candidate files with no findings; staged blob hashes matched the scanned inputs. The test-probe correction was separately scanned. Final evidence-file and new-commit scans remain pending. |
| Dependency gate | **Failed on both the candidate and the fetched main lockfile.** `cargo deny --locked check advisories bans sources` passes bans/sources but fails advisories; `cargo audit --no-fetch` exits 1 for both lockfiles. [Comparison and identities](dependency-gate.json). Existing versions `crypto-bigint 0.5.5` and `tokio 1.52.3` are unchanged. This remains an integration blocker, not a successful check. |
| SDK / indexer fees | Node SDK **7 passed**; separate indexer workspace `--test l2_fees`: **6 passed**. Indexer evidence uses simulated RPC, not a production database. |
| Final native captures / signer | Bidirectional, caught-revert lookup and changing-static process captures passed with the final genesis and signer network denial. The rebuilt signer suite passed **320 tests**, with its two explicit generation utilities ignored. All 29 input-derived conformance cases and 24 proof expectations were preserved. A stale embedded test fixture initially caused three failures; byte-level inspection established that Cargo had reused a binary containing the old genesis after a timestamp-preserving fixture copy. Updating the fixture mtime and rebuilding corrected this without any content/assertion change; both executions are retained in the [fixture provenance record](final-fixtures.md). |
| Format / wire / shell | Rust workspace format passes, all **29** conformance vectors/fixture inputs/schemas match, and `bash -n` passes for all three edited shell scripts. |
| Clippy / full process suite | Final workspace format and `cargo +1.95.0 clippy --locked --offline --workspace --all-targets -- -D warnings` passed. The first complete `node_e2e` attempt exposed the collection-barrier defect above and was interrupted for correction. Fresh captures and process validation use cleared prebuilt overrides, `EEZ_TEST_PROOF_SIGNER_NO_NETWORK=1`, and retained logs/datadirs. The [completed frozen process run](native-process-suite.json) exited 101 in 9845.87 seconds: **172 passed, 2 failed, 5 ignored**, with zero filtered cases. Both failures are investigated above. All four executable hashes were rechecked after completion. Four ignores require optional large-history/retained-history/four-day inputs; the fifth requires a live Kurtosis enclave and belongs to that validation track. These executables precede R03; its changed positive RPC test has separate evidence. |
| Kurtosis | [Dedicated live-validation record](kurtosis/README.md). New fee-enabled enclave with source-built images; existing enclaves and the old network are preserved. Optional UI/explorers are outside this run. |
| PR / final diff | [Draft PR #101](https://github.com/eez-association/eez-rollupX/pull/101), based on `main`. Integration commit `a9bfb99a7e5318255ae3ea511309d7a62f01b94f`; test-probe correction `d1838526480b89651f759d34e27bfbcfca43fb55`. Final validation updates remain pending; the PR is not merge-ready. |

Logs are local-only ignored artifacts under `artifacts/pr-readiness/`; the
[portable check results](validation-results.json) retain commands, counts and log
digests for completed checks. The [complete process summary](native-process-suite.json) records module counts,
all exclusions, failure classifications and exact executable identity. Live and
corrected-test results remain separate.
An [independent fee-accounting review](fee-accounting-review.json) recomputed the
four single-source and two shared-batch L1 refunds from the retained receipts,
including the payout transaction fee. It also checked the pooled L2 tips, bridge
margins and FeeVault balance arithmetic. The two intentionally subsidized L1
batches retain their measured shortfalls; this review is not new live execution
or a profitability claim.
The first draft snapshot additionally passed Rust format and a secret scan of
387 candidate files. A path check covered 858 local links in 58 changed Markdown
documents; one retired schema reference was corrected to the single `prove.proto`.
That path check does not assert external URL availability or fragment validity.
These are pre-publication checks, not evidence that the pending runtime suites
have completed.
An invocation that passes only on a diagnostic retry remains distinguishable
from a first-pass result. Unit, mocked, process and live evidence are separate.

GitHub CI run [40092563758](https://github.com/eez-association/eez-rollupX/actions/runs/40092563758)
also failed its dependency-advisory check on the two unchanged versions below;
bans and sources passed. The [job/step observation](ci-draft-observation.json)
records successful format/history scanning and subsequent skipped checks. Its lint job ended before subsequent checks.
That initial observation preceded the [successful unit job](ci-unit-validation.json):
**1569 workspace lib/bin tests passed, 3 explicit ignores**, plus **1/1** Reth
shutdown regression (206 other Reth tests filtered out). The job ran on synthetic
merge `83a3af601a5828f8d5814f1e6baa0c76bcfec972`, whose Git tree exactly matches
head `d1838526480b89651f759d34e27bfbcfca43fb55`. Its logs confirm the new RPC
prefix/state and dependency-identity regressions passed. The runtime job later failed its release archive guard: the test archive
recompiled 136 non-workspace crates after the shipping build. The downstream
embedded, cross-chain and Kurtosis CI lanes were skipped; they are not coverage.
The [feature investigation and correction](ci-runtime-archive.md) identified
Tokio `test-util` and a redundant trie-common `default` feature introduced by signer
dev-dependencies. The corrected exact selections have identical external unit
graphs, including dependency edges. The focused deadline test passed **1/1**,
and its same executable passed **320 signer tests** with two utility ignores.
The archive guard and external versions remain unchanged. New CI run
[40098162224](https://github.com/eez-association/eez-rollupX/actions/runs/40098162224)
validated the tree of `0e34a326536befc31ee62a92647bfc3dca9ddee2`: shipping compilation
and the archive guard passed. All 1569 lib/bin unit tests and the Reth shutdown
regression also passed, with their explicit exclusions. The release producer then
failed two of 30 protocol-vector/blob-evidence tests while constructing a private-L1
genesis fixture: its reference was after the bootstrap cutoff, and its history
storage retained a different calendar. Publication and downstream lanes were
skipped; no admissible final payload exists for that run.

The [fixture correction](ci-prefix-fixture.md) changes only the example test
module. It installs a coherent bootstrap and adds specific rejections for an
invalid reference and three inconsistent history slots. The same example command
reproduced **10 passed, 2 failed** before editing and passed **13/13** afterwards.
Production validation is unchanged. The earlier workspace and lib/bin selections
did not run these example tests; their passing totals did not cover this fixture.
The next release producer and final artifact/replay validation remain required,
separately from the unresolved dependency advisories.

After the RPC and cost-test corrections, the complete workspace Clippy command
passed again with all targets and `-D warnings` in 39.62 seconds. This invocation
used one Cargo job; its memory guard observed a minimum of 16.08 GiB available
and did not cancel any process. Its exact source and log hash are recorded in
[the portable results](validation-results.json). After the feature correction,
Clippy passed again in 160.02 seconds with one job and at least 15.52 GiB
available; no memory cancellation occurred. After the example fixture correction,
the same full-workspace check passed in 3.00 seconds with one job and at least
15.83 GiB available. The changed Rust file hash was unchanged during that check.
The later receipt-only test correction passed full-workspace Clippy in 4.00
seconds and compiled its test target in 41.01 seconds, one job with at least
15.01 GiB available; only the workspace test target was recompiled.

The dependency findings are [RUSTSEC-2026-0387](https://github.com/RustSec/advisory-db/blob/21d909e4c8bd4e14c7aea58bf980c881a0053ab1/crates/crypto-bigint/RUSTSEC-2026-0387.md)
(constant-time arithmetic) and [RUSTSEC-2026-0386](https://github.com/RustSec/advisory-db/blob/21d909e4c8bd4e14c7aea58bf980c881a0053ab1/crates/tokio/RUSTSEC-2026-0386.md)
(`AsyncFd` I/O safety). The former's documented fixed line is `crypto-bigint >=0.7.6`,
whereas the pinned `elliptic-curve 0.13.8` dependency uses 0.5.5. Remediation needs
a separate compatibility assessment of the pinned cryptographic/runtime stack;
this protocol validation neither establishes exploitability nor waives the gate.
No dependency upgrade or new advisory exception was introduced. The
[remediation assessment](dependency-remediation.md) identifies a compatible Tokio
candidate and the unresolved cryptographic-family migration/backport requirement;
it does not claim that either future update has been built or validated.

## Completion obligations

1. Complete the applicable unit, protocol, process and live checks; investigate
   each failure before interpreting a passing subset as readiness.
2. Verify fees against actual charged gas and recipient balances, treasury
   rotation/withdrawal and independent settled replay.
3. Verify both cross-chain directions, grouped/static/reverted behavior and
   canonical settlement with exact live contract/runtime identity.
4. Validate recorded history/PREVRANDAO and continuous ordinary L2 production
   under missing or delayed L1 data within the owned environment.
5. Preserve the first evidence of any defect and rerun affected checks after
   corrections. Rebuild deployed images if production inputs change.
6. Record checks not executed and why, remaining risks, final source hashes and
   the reviewable PR result. No live-network or Nethermind interoperability claim
   is established solely by unit tests, receipts or startup.
