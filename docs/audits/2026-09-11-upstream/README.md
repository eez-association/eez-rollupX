# Upstream backport audit and porting plan

**Implementation completed:** [completion ledger and validation](completion.md).
The historical snapshot below records the pre-port state and remains the source inventory.

Audit snapshot: **2026-09-11, 13:36 UTC**. **The fork has not backported everything.**

There are **12 mainline upstream changes** after the common base: **5 have
backports, 2 have incomplete backports, and 5 still need porting**. The two
incomplete backports contain changes added upstream after the fork's early
backports. Five existing backports have source/history evidence; that is not a
claim that their current runtime behavior has been revalidated in this audit.

[inventory.json](inventory.json) records every upstream-only commit, every
changed file in each mainline change, content identities, port ownership, all
current upstream branches, open PRs, and protocol dependency changes.
[verify.py](verify.py) checks the history and content inventory against Git.

This is a completed static audit and an implementation plan. No application
code was ported, branches merged, deployments changed, or runtime suites run.
The acceptance gates below are necessary before declaring the ports complete.

## Exact scope and baseline

| Item | Verified revision | Meaning |
| --- | --- | --- |
| Upstream rollup `main` | [`c97b83f142b947243c2d7ef6a76b627d4bfcb71b`](https://github.com/eez-association/eez-rollup0/commit/c97b83f142b947243c2d7ef6a76b627d4bfcb71b) | Includes PR #103, merged today. |
| Fork live `main` | `14e8e09a9b0355d61e2ade1207b1e8a1ac2e58e9` | Verified through the authenticated GitHub CLI. |
| Working branch / fork PR #14 | `76e7af73f143d94683c18cbde8f4f7f03885a49a` | Pending CI, harness, and protocol update; not merged into fork `main`. |
| Common ancestor | `d8a7547b311260fdc3262769429a68212fb18bf9` | Earlier upstream `main` history is inherited by ancestry. Later fork changes still require regression checks. |
| Cached upstream ref at audit start | `68a786a592bdef3e4b5cfbc565174b2b4d0287ec` | Stale; it omitted today's #103. |

The required port target is upstream rollup **`main` at the pinned SHA**.
Open branches and PRs merged into other branches have a separate intake list
below. Their existence does not make them accepted `main` changes. Arbitrary
dependency upgrades and every historical abandoned experiment are not included
as mandatory ports.

The raw ancestry difference is **43 upstream-only commits: 33 non-merge commits
and 10 merges**. These collapse into 12 mainline units, not 43 missing patches.
The ledger contains **217 file-change records and 132 net changed paths**.
Renames are represented as deletion/addition records to make both paths explicit.
All 43 commits belong to exactly one mainline unit, including merge commits.

### Protocol dependency

| Location | Protocol pin / tree |
| --- | --- |
| Upstream rollup `main` | `9735f53abbb6b9f5e863f405ad4555b4701b7fda` |
| Fork merged `main` | `171de2b7b13477f0ce17045769f9560738912a77` |
| Working branch / pending #14 | `3c0cdf4955941a5c23639346932b55b33babebbd` |
| Live protocol `main` | [`a7a0d7eb1d57b28340d16bd53877b6a08526172c`](https://github.com/eez-association/eez-core-protocol/commit/a7a0d7eb1d57b28340d16bd53877b6a08526172c) |
| Tree of both working pin and live protocol `main` | `7051b5580f346b9338a4d2039489763bb92f05a2` |

The working protocol pin is **content-identical** to protocol `main`, despite
having a different commit ID. Fork `main` lacks **13 protocol commits affecting
62 paths** that are already pinned by pending #14. No additional protocol
content needs to be copied beyond that pending baseline at this snapshot.

The fork implements the newer defensive protocol, grouped/nested calls, native
blob DA, block-hash commitments, FIFO observations, and persistent settlement
correlations. A wholesale upstream merge or file replacement would not preserve
these changes. Keep the selected protocol pin and port upstream behavior into
this implementation.

## Complete mainline inventory

“Backported” means the mapped fork commits and adaptations were checked in
history/source. It does not waive final validation. The JSON contains full SHAs
for all abbreviated commits below.

| Upstream PR | Upstream unit | Status in fork `main` and working branch | Evidence / remaining work |
| --- | --- | --- | --- |
| [#108 — actionable prover errors](https://github.com/eez-association/eez-rollup0/pull/108) | `89384d9` | Backported, adapted | `56d2a0a`; wire/error/client changes retained, grouped-entry error attribution adapted. Bound opaque failures separately in P1. |
| [#126 — Foundry action](https://github.com/eez-association/eez-rollup0/pull/126) | `c7ab8e3` | Backported | `fa30fe8`; aggregate patch ID matches. Preserve subsequent toolchain pins. |
| [#107 — explicit node roles](https://github.com/eez-association/eez-rollup0/pull/107) | `408ae49` | Backported, adapted | `6345f23`, `4598909`; role startup, witnesses, read/submission separation retained with fork ingress/overlay changes. #128 is a further change. |
| [#124 — bounded settlement failure recovery](https://github.com/eez-association/eez-rollup0/pull/124) | `5bf9b2d` | Missing | Both `3ec897b` and `36cbf54`; P1. Existing relay-drop accounting does not cover opaque prover failures. |
| [#115 — settlement hardening](https://github.com/eez-association/eez-rollup0/pull/115) | `218b7a8` | Backported, adapted | `02afd5b` through `e219912`, plus `7fda50f`. Includes late endpoint-depth and system-key/address checks; keep block-hash adaptations and regression tests. |
| [#89 — composer/prover specification](https://github.com/eez-association/eez-rollup0/pull/89) | `e4a76aa` | Missing | `crates/eez-control-rpc/SPEC.md` and README link absent; P6 requires a fork-correct specification. |
| [#117 — gas rework](https://github.com/eez-association/eez-rollup0/pull/117) | `ee596a5` | Partial | Base measurement/cap changes in `de6f37c`, `01ba319`, `b7d027c`; final `4f00fc8` safeguard and tests absent. P2. |
| [#118 — restart checkpoint](https://github.com/eez-association/eez-rollup0/pull/118) | `f1b1759` | Partial | `b6a92a4` through `a9e9686`; later startup/multi-batch guards and deployment/script fixes missing. P3. |
| [#123 — remove checkpoint quota](https://github.com/eez-association/eez-rollup0/pull/123) | `41ecac9` | Backported, adapted | `c2feba8`, `54ee819`; quota/config/service/validation changes match their upstream patches. Upstream edits to #124's test comments follow P1's harness port. |
| [#114 — remove legacy mock prover](https://github.com/eez-association/eez-rollup0/pull/114) | `e048b0e` | Missing | Mock fallback, implementation, contract, deployment script, and gas-fixture use remain. P2. |
| [#128 — isolate role builds](https://github.com/eez-association/eez-rollup0/pull/128) | `68a786a` | Missing | New shared/follower/E2E packages, dev-node removal, and related build changes absent. P4. |
| [#103 — cross-chain coverage/testkit](https://github.com/eez-association/eez-rollup0/pull/103) | `c97b83f` | Missing as an upstream unit | Some independent fork harness improvements overlap, but the new testkit, scenario coverage, signals, and state-chaining driver remain to port. P5. |

### Why the partial backports matter

**#117:** `clamp_max_postbatch_gas` still accepts the anchor-only minimum,
`projected_postbatch_gas(1, 0, 0)`. With the current constants that is **551,000**.
The drain also reserves **600,000** and needs room for a held transaction. The
upstream minimum is `projected_postbatch_gas(2, 0, 0) + POSTBATCH_DRAIN_MARGIN`,
currently **1,521,000**. Accepted settings below the latter can prevent the drain
from admitting a transaction. The missing tests also bind Solidity gas-pin
literals to Rust. Evidence: `crates/eez-composer/src/composer.rs`, especially
`PostBatchGasBudget`, `clamp_max_postbatch_gas`, and upstream `4f00fc8`.

**#118:** its final patch adds `await_l1_head`, a same-L1-block checkpoint guard
using `count_at_l1_block`, trusted-peer environment forwarding, a final funding
balance rescan, and a pinned Ethereum Kurtosis package. These are not all in the
early backport. The fork's `correlation_seed` already restores **every record in
the tail L1 block** and returns a retryable error when L1 cannot serve that tail.
That is a stronger path to preserve. The older single-record checkpoint fallback
still exists in the deriver and needs its own safety decision and regression.

## Ordered implementation plan

Use small commits with source PR/SHA provenance and an explicit per-file
disposition. Each work item must finish its acceptance checks before depending
work proceeds. The suggested order is **P0 → P1 → P2 → P3 → P4 → P5 → P6 → V**.
P1, P2, and P3 are independently reviewable after P0; the order keeps runtime
fixes ahead of the package moves.

### P0 — establish the integration baseline

Use the content of working commit `76e7af7` as the planned baseline. Pending
[fork PR #14](https://github.com/eduadiez/rollupX/pull/14) contains that commit;
its CI/protocol changes must either merge first or be the explicit parent of
the port stack. Do not count them as already on fork `main`.

Record the fork, upstream, protocol, compiler, Foundry, nextest, and runtime-image
identities. Run the existing baseline gates and retain failures as evidence;
do not silently lower gates while porting. Regenerate/check EEZL2 genesis and
fixtures against the pinned protocol. The local branch's cached tracking status
was stale after a rewritten PR branch, so derive ancestry from verified SHAs.

Acceptance: exact baseline and submodule recorded; pending changes accounted for;
generated artifacts agree; any baseline failures have a concrete resolution
before final acceptance. This audit does not assert that #14's CI is green.

### P1 — complete bounded recovery (#124)

Port the behavior of both `3ec897b` and `36cbf54` into the fork composer:

- Share saturating settlement-attempt accounting between relay and prover
  failures; charge once per failed proving episode, not per transport retry.
- Bound opaque rejections and invalid/non-matching actionable hints. Never use
  an invalid hint to select a transaction for targeted eviction.
- Evict exhausted candidates and the same sender/direction nonce suffix,
  including queued and in-flight entries; release reservations and preserve FIFO.
- Preserve the distinction between charged prover/relay failures, uncharged
  construction failures, and slot skips. Preserve the fork's locally validated
  observation mismatch attribution and its rollback/requeue behavior.
- Carry the recovery log severity, source, and outcome changes in `composer.rs`,
  the related comments in `held_pool.rs`, `optimistic.rs`, `prover_retry.rs`, and
  both harness mutation modes and end-to-end tests.

Acceptance: port the two recovery unit tests and the opaque/mismatched-actionable
E2Es; verify the bound across prover and relay episodes, independent senders,
opposite directions, nonce cascades, FIFO order, and reservation release. Check
construction failures and skipped slots do not consume the bound. Exercise valid
and forged observation failures using the fork's grouped-entry model.

### P2 — remove mock fallback and finish gas protection (#114, #117)

Require nonempty `EEZ_PROVER_URL` and `EEZ_ATTESTER_ADDRESS` for composer startup;
keep witness capture mandatory for that production path. Remove `MockEcdsaProver`,
its fixed digest and signer-only error, obsolete dependencies, the mock verifier,
its deployment script, and stale documentation. Keep purpose-built test doubles
where tests need them, without exposing an implicit production fallback.

Convert `PostBatchGasPins.t.sol` to the real `ECDSAProofSystem`. Recompute the
public-input hash using the fork's actual ABI, `blobIndices` and the selected
transaction blob hashes, grouped entries/static entries, and current commitment
fields; the old upstream hash recipe is not authoritative for this protocol. Re-measure the gas pins with
the real verifier. Then complete `4f00fc8`'s minimum-ceiling logic, its boundary
test, and its Rust/Solidity pin-consistency test using the final measured values.

Port #114's runtime binary-resolution behavior as needed by the test harness,
preserving #14's explicit shipping-binary overrides. This prepares the extraction
in P4/P5. Retain #117's existing declared-gas fitting, calldata floor, measured
target costs, bundle cap, and escrow-deferral adaptations.

Acceptance: missing/blank prover configuration fails startup; a real signer runs
the composer; attester mismatch and mutated proof inputs fail; no active path
references the legacy mock. Gas tests use the real verifier and agree across
Rust/Solidity. The lowest accepted cap admits one held transaction; below-bound
and above-maximum inputs follow the documented clamp policy. Run empty and
effect-bearing batches in calldata and blob modes.

### P3 — complete restart and operational fixes (#118)

Preserve v2 block-hash checkpoints and persistent correlation storage. Add the
missing same-L1-block protection to any retained single-record fallback, or
remove that redundant fallback from all reachable constructors with an explicit
migration/test rationale. Preserve complete correlation-tail seeding. A transient
L1 catch-up lag must cause a bounded wait/retry rather than invalidating valid
persisted progress; a real canonical mismatch must still trigger repair.

Complete all remaining #118 file-level changes:

- Forward `EEZ_L1_TRUSTED_PEERS` in `docker-compose.chiado-node.yml` and check the
  generated two-machine configuration reaches the embedded L1.
- Rescan recipient balances after the last funding retry in `scripts/xchain-test.sh`.
- Pin the compatible package default in `testing/kurtosis/scripts/12s-l1-up.sh`
  to upstream's `199620b24ac979c676010c5a68b2893c2bce4f1f`, retaining
  `ETHEREUM_PACKAGE` override support; validate the builder starts with it.
- Preserve the fork's complete scenario discovery in `run-protocol-e2e.sh`.
  Upstream's old `multi_tx`/`multi_call` allow/reject lists must be translated to
  the selected defensive protocol, which supports additional grouped flows.
- Retain already adopted checkpoint dependencies/exports and datadir resolution;
  do not replace the fork's lockfile or correlation-store wiring.

Acceptance: restart both roles after two batches in one L1 block, resume partial
settlement, restart with L1 initially behind, handle corrupted/foreign checkpoint
data, and recover from an orphaned tail. Check both correlation RPC directions
and all tail-block transaction identities. Exercise the fallback directly if it
remains. Verify final-retry funding success, genuine underfunding, trusted-peer
propagation, and the two-machine package bootstrap.

### P4 — isolate node roles (#128)

Introduce `eez-node-common`, `eez-follower`, and `eez-node-e2e`; retain the composer
in `eez-node`; remove `eez-dev-node` and the obsolete interval-sequencer surface.
Port the driver, slot/timing, L1 configuration, deriver, and composer call-site
changes. Move the existing tests and fixtures before changing their semantics.

Carry fork-specific functionality through the extraction: authenticated blob
retrieval/configuration, correlation RPCs/store initialization in both roles,
system-sender filtering, ingress routing, shared execution overlays, witnesses,
block-hash genesis helpers, and observation handling. Put the shared correlation
RPC and datadir/correlation-store helpers in `eez-node-common` for use by both
roles, alongside the shared pool/payload code; keep composer-only witness and
ingress machinery in `eez-node`. Check the resulting dependency graph for cycles
and accidental composer dependencies in the follower build.

Update **all** callers of package/binary/test identities, including upstream's
Dockerfile/README/genesis script and the fork-only paths introduced by #14:
`Dockerfile.runtime`, `scripts/ci/native-tests.py`, runtime artifact builders and
validators, nextest filters, archive manifests, workflow jobs, fixture paths,
and CI contract tests. For example, the current `package(=eez-node)` E2E filters
and `eez-node::…` identifiers must select `eez-node-e2e` after the move.

Acceptance: composer and follower build independently; follower dependencies
exclude composer/signing/witness-production machinery except what is actually
needed for following. Both role help/startup surfaces and the default composer
command work. Test discovery is nonempty and includes all four integration
binaries. Runtime images contain the production role binaries, while the E2E
wrapper binaries remain test artifacts. Both roles retain blob recovery and
correlation RPC behavior; existing resumption/reorg tests survive the move.

### P5 — extract the testkit and port the full coverage change (#103)

Port all 42 upstream changed-file pairs, including renamed files (represented
as separate records in the ledger), not just the new test functions:

- Extract `eez-testkit`, preserving #14's artifact overrides, sandboxed working
  directories, process lifetime/cleanup handling, and corrected fork fixtures.
  Bring over typed signals, reserved TCP/UDP/paired RPC ports, artifact checks,
  readiness/failure-aware waits, and the shared scenario helpers.
- Port the fixtures/contracts for empty calls, nested setters, return bytes,
  bubbling reverts, and reverting destinations; update `SetterWrapper` and
  add the proxy-policy test.
- Port production signal additions in composer, deriver, watcher, submitter,
  node, follower, and signer. The testkit consumes explicit `event_name` fields;
  tracing's event metadata name alone is insufficient for these assertions.
- Port the test changes in all four E2E files, plus
  `testing/kurtosis/scripts/lib.sh`, `verify-state-chaining.sh`, wave script,
  main package/arguments, runner integration, README, and 12-second runbook.
- Preserve the fork's runtime artifact pipeline and newer nextest pin while
  adopting upstream's retry-for-diagnostics/fail-on-flakiness policy. Verify the
  effective integration grouping: #103's upstream filter says `package(eez-node)`
  even though #128 moved the tests to `eez-node-e2e`. Use the actual package.

The inventory lists **45 upstream E2E/testkit test names**, including retained
tests, for reconciliation against the final runnable test list. A matching name
is only a search aid, not proof that assertions are equivalent. Required behavior
coverage includes:

| Scenario family | Required assertions and fork adaptation |
| --- | --- |
| Both directions; direct proxy and wrapper | Receipts, exact result, caller attribution, state effect, settlement. |
| Empty calldata, zero value, payable fallback, value + calldata | Calls are not silently skipped; effects and value are atomic. |
| Dynamic return bytes and empty return bytes | Byte-exact length/content and correct source-side result. |
| Nested wrappers and destination events | Correct source proxy attribution and event retention on the executing chain. |
| Direct unauthorized dispatcher call | Verify the intended rejection against the pinned ABI/admission policy. |
| Multiple calls in one transaction and codeless targets | Use the fork's supported/rejected shapes and assert deterministic behavior plus healthy subsequent traffic. Do not copy an obsolete blanket rejection. |
| Inbound/outbound destination reverts | Correct fork lookup/revert behavior, no unintended state/value changes, correct source outcome and queue recovery. |
| Repeated identical calls and order-dependent calls | Distinct effects chain correctly; no collapse by equal payload/root. |
| Mixed directions, same sender, poison between survivors | FIFO/prefix ordering, nonce continuity, bounded eviction, and surviving traffic settles. |
| Role/reorg/resumption tests | Canonical replay, safe/finalized convergence, no unexpected repair or fallback. |

Rewrite upstream state-root comparisons where the fork's registry commits to
**block hashes**, including prefix hashes for partial settlement. Keep genuine
execution-state-root comparisons where they check execution itself. Preserve
the fork's blob/semantic/FIFO tests and causal evidence collectors alongside
the imported coverage.

Acceptance: every inventoried test has a final runnable test ID or a reviewed
replacement covering its intent; no silent skips. Run native and Kurtosis
state-chaining tests, both directions, and fork calldata/blob profiles. Verify
signal discovery, ports, binary provenance, retry policy, and evidence completeness.

### P6 — document the actual composer/prover contract (#89)

Add `crates/eez-control-rpc/SPEC.md` and its README link. Carry upstream's stream
ordering, exact request/response validation, deadlines/status classification,
cutoff/fallback rules, actionable-hint validation, and prove-to-L1 handoff.

Rewrite the legacy profile sections against the code that will ship: defensive
ABI/selectors, grouped entries and callbacks, FIFO observation claims and
outcomes, block/prefix-hash commitments, calldata and native blob payloads,
physical blobs on `PostBatch`, public-input hashing, and remaining resource
limits. Document P1's recovery bound and distinguish it from within-request
transport retries. Update the existing signer specification and cross-links
that currently describe the older profile; connect this to the existing
`PRODUCTION_READINESS_TODO.md` specification item.

Acceptance: every normative field/behavior maps to the actual schema, builder,
client, signer validator, or contract; examples decode and match current vectors.
Document any implementation mismatch as an explicit remaining task rather than
describing desired behavior as already implemented.

## V — final acceptance and proof of coverage

Do not use “all upstream commits are ancestors” or “cherry-pick succeeded” as
the acceptance test. The fork intentionally has different protocol semantics.

1. **Inventory closure:** for every one of the 217 source file-change records,
   record the final port commit and the disposition of every source hunk:
   unchanged import, adapted implementation with evidence, or superseded code
   with a documented replacement. No unexplained omission, pending marker, or
   unresolved conflict is allowed. Link adapted behavior to tests. Keep the
   immutable audit snapshot and add a completion ledger rather than overwriting
   the historical evidence.
2. **Previously adopted backports:** revalidate #108 attribution, #107 role
   boundaries, #115 scan attribution/off-grid handling/system-sender and
   key-address checks/endpoint depth/resumption, and #123 more-than-eight effect
   checkpoints with remaining request quotas enforced. Preserve the Foundry
   action/toolchain support from #126.
3. **Build and static gates:** `cargo fmt --all --check`, workspace/all-target
   clippy with `--locked` and `-D warnings`, the repository's Cargo advisory and
   dependency checks, `python3 -m unittest discover -s scripts/ci/tests -v`, and
   `scripts/update-eezl2-genesis.sh --check`. Build production roles explicitly
   after the package split and check the generated contract artifacts.
4. **Contracts and Rust:** run both Foundry projects using the same exclusions
   as the repository CI, all workspace unit/vector/codec suites, and all four
   native integration binaries. Use the final `eez-node-e2e` package and updated
   nextest archive/suite selectors. Archive or list discovery must fail when a
   required test binary or suite is missing; zero executed tests cannot pass.
5. **Network and artifact gates:** run the complete selected protocol scenario
   suite, imported state-chaining coverage, and existing semantic blob, lookup,
   FIFO, restart, reorg, correlation, and deployment checks. Cover supported
   EIP-4844/EIP-7594 environments with the real signer. Preserve transaction,
   receipt, blob/hash, queue, and canonical L2 evidence. Check final runtime
   image and native archive provenance against the same root/protocol SHAs.
6. **Freshness and merge:** re-read live fork/upstream/protocol refs before
   accepting the final stack. Any new upstream mainline commits get new ledger
   entries and acceptance checks. Validate the actual integration/merge result;
   a green predecessor commit is insufficient. Merge in dependency order after
   the final review process.

Passing this gate establishes the defined snapshot's backport coverage with
recorded evidence. It does not replace the wider production-readiness work or
guarantee the absence of all defects.

## Non-main intake: explicitly accounted for

All **117 live upstream branch heads** and **15 open PRs** are recorded in the
JSON. These are a watchlist, not permission to merge unaccepted work.

| PR(s) | Current scope / follow-up |
| --- | --- |
| [#139](https://github.com/eez-association/eez-rollup0/pull/139) | Open on `main`: withdrawal funding deferral, bound, and failure-aware test waits. Fork `34db70e` already implements an earlier deferral approach; compare the final bound/nonce behavior when this lands. |
| [#151](https://github.com/eez-association/eez-rollup0/pull/151), [#152](https://github.com/eez-association/eez-rollup0/pull/152) | Wrapped postBatch extraction, then spec-compliant DA structures. #152 targets #151's `fix/log-decoding` branch. Review as a dependency chain against fork blob decoding and authenticated transport; not a blind codec replacement. |
| [#141](https://github.com/eez-association/eez-rollup0/pull/141) | Merged into `aditya/defer-insufficient-escrow`, **not main**. Its merge SHA is also absent from that branch's current ancestry. Track explicit L2 range binding separately; a “merged” PR label does not prove it is in the required source branch. |
| [#137](https://github.com/eez-association/eez-rollup0/pull/137), [#104](https://github.com/eez-association/eez-rollup0/pull/104) | Stateless backend/service split, followed by stateful signer; #104 targets #137's branch. |
| [#129](https://github.com/eez-association/eez-rollup0/pull/129) | Additional protocol-vector/state-machine coverage. Reconcile after P5 and when accepted upstream. |
| [#31](https://github.com/eez-association/eez-rollup0/pull/31) | Config-file migration; targets `aditya/isolate-node-builds`. Coordinate after role and deployment wiring changes. |
| [#119](https://github.com/eez-association/eez-rollup0/pull/119) | Draft P2P followers; separate trust/transport change. |
| [#130](https://github.com/eez-association/eez-rollup0/pull/130) | Proposed AGENTS.md; not present on the audited checkout/main. |
| [#52](https://github.com/eez-association/eez-rollup0/pull/52), [#54](https://github.com/eez-association/eez-rollup0/pull/54), [#55](https://github.com/eez-association/eez-rollup0/pull/55), [#57](https://github.com/eez-association/eez-rollup0/pull/57), [#60](https://github.com/eez-association/eez-rollup0/pull/60), [#64](https://github.com/eez-association/eez-rollup0/pull/64) | Open runbook, prover-trait removal, postBatch extraction, error cleanup, silent-failure hardening, and inspector-fold proposals. Reconcile accepted final patches; old heads overlap substantial later changes. |

The API also reports older off-main merges **#13, #23, #53, and #82**. Their
merge SHAs are not ancestors of today's upstream `main`; they are recorded
alongside #141 in `side_merged_prs`. They are not added to the mandatory patch
queue merely because GitHub labels them merged. Accepted effects carried by
later mainline integrations are already in the common history or mainline
inventory. This audit makes no separate behavioral-completeness claim for
abandoned/off-main branch histories.

## Reproducing the audit

The audit used a temporary bare repository, so the working checkout's refs and
files were not reset or rebased. The snapshot can be checked now with:

```bash
python3 docs/audits/2026-09-11-upstream/verify.py \
  --repo /tmp/rollup-upstream-audit-20260911/repo.git \
  --protocol-repo /tmp/rollup-upstream-audit-20260911/protocol.git
```

For a later reproduction, use disposable clones containing the pinned fork,
upstream, and protocol objects and pass those paths to the same command. Fetch
the source commits by full SHA from their respective repositories if their refs
have moved. The verifier is read-only and checks this frozen snapshot; it does
not fetch or claim current ref freshness.

The audit combined live Git refs/API metadata, common ancestry, stable patch IDs
(including merge-to-first-parent diffs), per-file patch inspection, historical
backport inspection, current source searches, and exact protocol tree comparison.
The inventory verifier checks identities and coverage, not the human judgments
about adapted behavior. No build, runtime, or network test pass is claimed here.

Verification completed: all 43 commits, 12 units, 217 file-change records, 132
net paths, the 45 test-name entries, and the protocol content comparison passed
the read-only verifier. Live fork `main`, fork PR #14, upstream `main`, and
protocol `main` were rechecked before finalizing; their recorded revisions held.
