# Official Reth v2.6.0 local validation

| Field | Value |
| --- | --- |
| Date and status | 2026-09-26; validation complete; unmodified v2.6.0 not accepted for release |
| Scope | Fresh-network EEZ upgrade to official Reth v2.6.0; existing database migration excluded by the user |
| Source baseline | `eduadiez/rollupX` `85f3691cb9c7ccbf8c0b8700576733283e5af7fc`; isolated branch `feat/reth-v2.6-validation` |
| Implementation commit | `af4f54ce8d538ddb3b605bb6bf7449e832b09e0e`; source edits were built before this local commit; exact runtime hashes below |
| Protocol | `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged |
| Official Reth | `73a3a00862a8f14f89e30da8de001456f18cfae0`, no source patches |
| Tools | Rust 1.95.0; Foundry 1.7.1; Bookworm release build container |
| Proposal | [Acceptance requirements](../../proposals/reth-v2.6-validation.md) |
| Earlier investigation | [Why the development pin was selected](../2026-09-26-reth-release-compatibility/README.md) |
| Dependency sources | [Vendored port provenance](../../../vendor/README.md) |

## Implementation

The old Reth development revision cannot be replaced by a version string alone.
The port updates the coupled REVM/Alloy graph and adapts witness recording,
provider errors, payload/assembler APIs, and the checkpoint-capable Stateless
validator. All Reth packages in the application lockfile resolve to the official
release commit. The Gnosis and Stateless dependencies are local source ports;
this does not imply that their upstream releases support this combination.
Official v2.6.0 requires Rust 1.95, so the workspace minimum, build images and
CI toolchain move together. Rust 1.94 cannot build this release's declared
dependency contract.

The old `legacy_state_root_task_enabled` configuration is ignored by v2.6.0.
The embedded launchers now use the supported `state_root_fallback` setting to
preserve synchronous root calculation. L1 simulation explicitly retains the
interpreter EVM with the embedded node's chain specification.

Stateless must explicitly clear destroyed storage before computing either
transaction checkpoint roots or the final root. The new regression leaves old
storage blinded and compares both paths against an independent trie, with and
without recreated storage. Witness removal closure retains read-only accesses
and provider-expanded storage resets. No signer checks or protocol commitments
were relaxed.

## Executed checks

Commands run from the isolated checkout under
`artifacts/reth-v2.6-validation/rollupX`. Raw logs under that checkout's
`artifacts/reth260/` are local-only and are not distributed by this record.

| Check | Result | Local log |
| --- | --- | --- |
| `cargo +1.95.0 check --locked --workspace --all-targets` | Pass | `check-10.log` |
| `cargo +1.95.0 fmt --all --check` | Pass after final test corrections | `fmt-final-02.log` |
| `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` | Pass after final test corrections | `clippy-final-02.log` |
| Affected library suites | 592 passed; one manual Composer timing profile ignored; signer is a separate binary test target | `unit-tests-01.log` |
| Stateless and trie suites | 9 passed, including recorded transaction checkpoint roots and blinded storage reset; empty doctest selections are not coverage | `stateless-tests-02.log` |
| Node/L1/deriver/follower library suites | 261 passed (includes two embedded-persistence configuration checks); empty prover target not coverage | `runtime-unit-tests.log` |
| Signer binary suites | 330 passed; 2 explicit inventory/performance utilities ignored | `signer-tests-01.log` |
| Commitment/KZG oracles | 16 passed across four integration targets | `oracle-tests.log` |
| Stateless Clippy | Pass | `stateless-clippy-01.log` |
| Integration Solidity | 58 passed | `forge-tests-contracts.log` |
| Pinned core Solidity | 517 passed; excludes `script/*`, `test_Doc_OriginalValue_SeedInSetup`, `test_PostCost_Incremental` as required | `forge-tests-protocol.log` |
| Cargo Chef packaging | Planner succeeds; empty cook skeleton fails without vendor sources, passes locked offline resolution with vendor copy before cook | `chef-planner.log`, `chef-skeleton-baseline.log`, `chef-skeleton-fixed.log` |
| CI tooling | 256 selected, 252 passed, 4 optional probes skipped | `tooling-tests.log` |
| `cargo deny check advisories bans sources` | Pass | `cargo-deny.log` |
| `cargo audit` | Pass with 9 allowed warnings; not an absence-of-advisories claim | `cargo-audit.log` |

The replay wrapper was also rechecked after adding `--locked`: its 11 mocked
public-network tests pass (`replay-tooling-final.log`). This is wrapper coverage,
not independent execution replay.

The complete serial native selection finished with **90 passed and 11 failed**
in 7,489.869 seconds, with zero retries. The previously corrected
partial-consumption restart test passed separately. All 24 cross-chain tests,
21 dynamic-admission tests, Ethereum/Gnosis native follower replay, queue
liveness and the real L1 reorg test passed. See the exact
[per-target accounting and failures](native-result.json) and
[original executable identities](native-test-inputs.json). The failure count is
not erased by the diagnostic controls below. Three large-history/retained-input
utilities were ignored; the separately executed restart was filtered out of
this run. The live-only Kurtosis replay target is a separate invocation.

The four tooling exclusions are the opt-in Docker payload, real Kurtosis relay
resource probe, native artifact relocation, and real Docker runtime fixture.
They are not counted as passing tests. The real candidate network validation
below is separate from these mocked/tooling checks.

Compilation failures during API adaptation are retained in `check-01.log`
through `check-09.log`. The first new storage-reset test failed to compile
because its map has a custom hasher (`stateless-tests-01.log`); it passed after
constructing that map through its iterator. These are development corrections,
not retries of a failing runtime assertion.

## Live validation

Reserved enclaves: `eez-reth260-20260926` (candidate 1) and
`eez-reth260-20260926-v2` (candidate 2). Existing host services and databases
are outside this task and are preserved. No existing network migration is
performed.

The canonical L1 image is
`ghcr.io/paradigmxyz/reth@sha256:8ce703acf113b2a20705b6e76adebed20f74ba8591bc7c9407203b2968aca70d`.
Running `reth --version` reports v2.6.0, the exact release commit above, and
build timestamp `2026-09-17T12:52:38.050294348Z`.

The local copy of `testing/kurtosis/run-ci.sh` changes only its source-directory
resolution and cleanup: the enclave remains available for diagnostics and
restart checks. Readiness, workload selection, and semantic evidence gates are
unchanged. The second runner also waits for the protocol checkout's existing
wallet lock before invoking its original runner; it does not bypass that lock.

Candidate 1 passed the complete live CI procedure: all four wave modes (40
operations), state chaining, 32/32 eligible protocol scenarios with independent
canonical receipt, call-table, commitment and authenticated native-blob checks.
Its run recorded 40 signed windows and 40 remote attestations, with no commitment
divergence. Candidate 2 also passes the complete procedure from a fresh deployment:
32/32 eligible scenarios, complete independent evidence, all four wave modes,
state chaining, 41 signed windows and 41 remote attestations, and zero commitment
divergence. [Its compact result](kurtosis-candidate-2.json) preserves the gates
and scenario accounting.

The pinned protocol discovers 36 scenarios but excludes `staticLocalWrite`,
`staticLocalWriteL2`, `nestedStaticLocalWriteL1`, and
`nestedStaticLocalWriteL2` as not live yet. These four are exclusions, not passes.
The selected 32 have no missing, duplicated, skipped or unexpected results.

## Upstream forkchoice issue

[Upstream #27429](https://github.com/paradigmxyz/reth/pull/27429), merged after
v2.6.0, fixes a return to a recently reorged-out persisted head while disk cleanup
is pending. The existing development pin also has the unsafe by-hash fallback;
that is static evidence, not a reproduced baseline result. Both upstream regression tests **fail** on official v2.6.0 with only tests added:
`VALID` is returned but the canonical block hash stays on the sibling branch.
The same-height and shorter-branch cases both reach the head-equality assertion.
These are deterministic engine unit tests using the upstream mock persisted
provider, not a live disk-timing reproduction. The [test-only patch](upstream-fcu-tests.patch) applies at the exact release
commit and selects two tests; 198 other engine tests were filtered out.

```bash
cargo +1.95.0 test --locked -p reth-engine-tree --lib test_fcu_back_to_reorged_out_head
```

As a diagnostic control, replacing only `find_canonical_header` with its exact
body from upstream commit `444e867155c2a7a4c028e479ca7201b0b6871579`
makes both tests pass. This is a code change, not a flaky retry. The complete
upstream patch does not apply directly because v2.6.0 lacks adjacent later
changes. This diagnostic is confined to a separate checkout; **the application
and Kurtosis candidate still use unmodified official v2.6.0**. Logs are
`upstream-fcu-v260.log` and `upstream-fcu-diagnostic-fix.log`.

Ordinary successful settlement does not cover this race. R4 is not satisfied
for unmodified v2.6.0, even if the remaining application tests pass.

## Private-miner restart regression

The unchanged `restart_after_partial_keeps_accounting_and_rebuilds_intent`
process test fails against candidate 1 at its retained-receipt assertion
(`partial_consumption.rs:304`). Before restart, L1 had canonically settled the
partial batch; after the fixture kills and restarts its miner, the L1 is back
at block zero and that receipt is absent. The v2.6.0 default persistence window
has not flushed this short chain. Logs and its failure datadir are retained
under `artifacts/reth260/process-01/` (local-only).

The port now explicitly restores the prior threshold/buffer/masking settings
(2/0/0) for `testing` and `testing-gnosis` miners. Consensus-driven `devnet` and
`chiado` retain upstream defaults, verified by two configuration tests. The original receipt assertion passes on candidate 2, but its next assertion
exposed a fixture race: the one-shot relay hold allowed a later replacement to
settle the omitted source before the crash (`process-02/`). Such a replacement
is permitted by the RPC contract; this failure does not demonstrate a restart
error.

The fixture now uses the existing relay fault control to reject subsequent
`eth_sendBundle` requests while holding the first retry, verifies the source is
still unsettled immediately before the crash, and restores relay access before
client resubmission. All original receipt, retry-counter, old-posting and fee
assertions remain. The same corrected test executable (SHA-256
`945577461e7115249f25f0074b3c800ce092ebaa151258644ced7ed8da4d8c8d`)
reproduces the missing settled receipt against candidate 1 (`process-03-baseline/`,
116.39 seconds); candidate 2 passes the same test in 81.27 seconds (`process-04-fixed/`). This is a changed fixture,
not a retry-only pass. The initial test executable was overwritten before it was
copied; its source revision and failure logs remain, but its binary hash was not
preserved. This change does not promise
zero loss of the most recent unpersisted blocks on an abrupt stop.

## L2 crash persistence failure

The release changes engine defaults from persistence threshold 2, state masking
0 and memory buffer 0 to 50/30/5. Compare the exact
[development pin](https://github.com/paradigmxyz/reth/blob/fd59fd2222b51239abebd9aa234f28b0b5f336eb/crates/engine/primitives/src/config.rs)
and [official release](https://github.com/paradigmxyz/reth/blob/73a3a00862a8f14f89e30da8de001456f18cfae0/crates/engine/primitives/src/config.rs).
`CrossChainWorld::restart_composer_with_env` uses `Child::kill`, so these
fixtures exercise abrupt process death, not a graceful engine shutdown.

The serial native suite's
`blob_overflow_drains_largest_empty_prefixes_without_resetting_the_interval`
failed against candidate 2: it expected two posts and observed one. The fixture
had collected about 325 KB of pending ordinary history around L2 height 60,
then deliberately killed the Composer to change its startup policy. The same
L2 database reopened at height 16, retaining only a fraction of that history.
This differs from the earlier private-L1 persistence failure: the embedded L1
now survives, but the L2 still uses official v2.6.0's larger default persistence
window. Lost unsafe history cannot exercise the intended two-post drain.

`fee_ceiling_blocks_posting_without_losing_activity_or_splitting` also failed:
after the fixture killed the Composer to lift the fee cap, its L2 reopened at
height zero and had no pending ordinary transactions. The follower convergence
assertion timed out after 180 seconds. Before the kill, the final policy event
reported L2 height 41 and seven pending ordinary transactions. The retained
datadir is `eez-cross-chain-1552375-jiNjSe`; the first failure is not retried as
part of the serial run.

The fee-RPC-outage case also reopened at L2 height zero, then timed out waiting
for a fee-history error: without pending activity, it never exercised the
intended fee admission path. These downstream timeout/assertion failures remain
failures of the default candidate, even when their cause is a shared persistence
setting.

The initial failure, logs and datadir are retained under
`artifacts/reth260/native-nextest/`; the test assertion was not relaxed. A
configuration-only control uses the identical Composer executable
with `--engine.persistence-threshold 2 --engine.num-state-masking-blocks 0
--engine.memory-block-buffer-target 0`. **All nine failed settlement-policy
cases pass** with those settings, using the identical test executable and no
assertion changes. Each invocation selects exactly one test and runs once.
[The control results](persistence-controls.json) record executable/wrapper
hashes, arguments, durations and reopened heights. This establishes a remedy
for the tested crash cases, not zero-loss durability for arbitrary crashes.
The candidate's L2 defaults have not been changed: its original run remains
failed. The normal live Kurtosis settlement run passed independently.

## Companion-gas fixture failure

Both `ethereum_actual_companion_gas_reduces_whole_groups_before_proving` and
its Gnosis counterpart timed out waiting for their companion receipts. The
native relay's L1 transaction submission returned the exact error
`tx fee (1660000000000000000 wei) exceeds the configured cap (1000000000000000000 wei)`.
The generic signing helper combines a 16,600,000 gas limit with a 100 gwei
maximum gas price, exceeding the pool's 1 ETH transaction-fee cap. Both the old
pin and v2.6.0 contain that same default cap; this is not established as a
new Reth regression. The fixtures never reached their intended receipt-based
gas assertions.

The test-only correction signs those two companions at a 50 gwei ceiling
(0.83 ETH maximum) while preserving the gas limit, storage-write workload,
priority fee, and every complete-group/gas/replay assertion. The original
test executable is retained with SHA-256
`8efea0cf258c2de2d6d3b79f7a1648967654bee42a427c8a4ba9f0c0284a065c`.
The original runtime binaries are unchanged. Both corrected process tests pass
in 112.72 seconds, selecting two tests with zero ignored tests. They reach the
original actual-gas, two-post, complete-group and follower convergence assertions.
The [fixture control record](gas-fixture-controls.json) preserves both executable
identities and the changed fee inputs. This is a corrected fixture, not a
retry-only pass; the initial native run remains recorded as failed.

## Evidence inventory

- [Candidate 1 live summary](kurtosis-candidate-1.json) contains the selected
  scenario inventory and all independent-verification completeness gates.
- [Topology images](topology-images.json) records actual local image IDs and
  available registry digests for both enclaves. Floating fixture tags alone
  are not the identity evidence.
- [Kurtosis arguments](kurtosis-args.yaml) are the final candidate's disposable
  network inputs, with only the repository's public development keys.
- [Local evidence hashes](local-evidence.json) identify retained raw outputs.
  They remain local-only; a checksum does not make the files available from
  another checkout.
- [Upstream regression input](upstream-fcu-tests.patch) is test-only and applies
  to the exact official Reth release. The diagnostic fix is not included in
  any shipping image.

Both completed enclaves were dumped before removal. Native failure logs and
datadirs remain available locally. Existing host networks and datadirs were
not operated on. The enclave dumps contain diagnostic information; they are
not claimed as restorable chain database backups.

## Additional live blob and recovery checks

The final candidate's native blob smoke check passed with Blobscan explicitly
disabled (the indexer was not deployed). It authenticated the Counter deployment
and two state changes in one canonical L1 blob batch, verified both correlation
RPC directions, and observed value 42 at settled L2 block 780, hash
`0x30eabbc66b6a2482912964f04b923e945aea683c3014ea67dec584f5d4523025`.
The L1 registry commitment matched that hash. This check authenticates blob
contents and canonical receipts; independent follower execution is separate.

The same candidate's Composer stopped with exit code 0 and restarted in the same
container. The first recovery probe kept the old host RPC port and timed out;
Kurtosis had reassigned it on start. Resolving ports again restored the check
without another restart. The original block hash, all three full receipts,
counter state and correlation record match the pre-stop snapshot. A fresh
increment transaction at L2 block 961 then settled canonically; safe block 966
has hash `0xd7a707b6b2f24dc93b2d59ce0bb9364cb0765fb20aced6d646c5e18a7c8081bc`
and counter value 43. The [restart result](restart-candidate-2.json) records the
retained image/container identity and new canonical settlement. This procedure tests retained
container storage, not replacement of a container or restoration from backup.
The failure and corrected probe are retained in `restart-v2.log` and
`restart-v2-resolved-ports.log`. The existing
[`verify-node-restart-recovery.sh`](../../../testing/kurtosis/scripts/verify-node-restart-recovery.sh)
already refreshes dynamic ports; the stale-port error belongs to the ad hoc
probe, not that maintained helper or the candidate.

A subsequent read-only comparison also matched all three pre-restart reverse
correlation records. The live L1 registry commitment matched safe L2 block 1116,
hash `0x1b828522bb7ae1b45948240e06c2daefa3671e310e2788e9c271560c11a0fac3`.
See [reverse correlation evidence](restart-reverse-correlation.json).

The standalone follower then reconstructed the live chain from genesis, L1 and
DA only, without the Composer database or unsafe overlay. Its original test
reached the expected safe block/hash, then panicked while unwrapping a missing
intermediate hash in a derived blob record. The current native verifier
intentionally reports execution inputs for derived records and a full terminal
header. This old fixture assumption also predates the Reth port.

The corrected test requires a nonempty consecutive block window, exactly one
final terminal, parent-hash continuity, matching derived execution inputs,
exact ordered transaction hashes/counts, explicit full/terminal commitments,
and nonempty canonical receipt checks. It passes against the same unchanged
follower executable: **42 blocks, including 41 derived blocks, and 3 receipts**,
with the authenticated terminal at block 780. Both first failure and corrected
run are preserved in `replay-baseline/` and `replay-fixed/`; the
[independent replay result](independent-replay.json) records their executable
identities. No production validation was relaxed.

## Shipping artifacts

Both candidates were built with `cargo build --locked --profile release` in the
Bookworm tools container, with the production feature selection and Rust 1.95.0.
The host's glibc 2.43 test harnesses are not shipped. Candidate 2 changes the
private-miner persistence configuration; follower, signer and verification-tool
binary hashes are identical across both candidates.

| Artifact | Candidate 2 SHA-256 |
| --- | --- |
| Composer | `ff9a189ce865323575cb17efbb245c458594fe703ab49386ea54045e4499922f` |
| Follower | `b66235af521ddddc30a6ca3a047395e2b8452c668da12d2c4b58ae31f6f46c5d` |
| Signer | `46e0a2fe9c3299e0375d6980b07fc8b994a5c484709e71b6ed88011a6f621c27` |
| Node image | `531eac9b1e3c9f937a5d42a44ec967e9390b0aa1fddaa02182822c8b31f20947` |
| Signer image | `ea803922fbdb1cf3bf2eded1df38cb5d8a1bc5c9e56dc178b55b2db381713170` |
| Deploy image | `de4b2f1c474c3af4919a854c0dcc07fc27708117f707dafd48247233d2a1e3fa` |

All 102 Reth git packages resolve to official release source `73a3a008`. EEZ's
lockfile uses REVM 43.0.3 (database 43.0.1), whereas Reth's release lockfile uses
REVM 43.0.2. The application therefore uses the exact official Reth source with
its own recorded dependency graph; it is not claimed to be byte-identical to the
official standalone image. The live canonical L1 uses that official image.

## Acceptance decision and limits

| Requirement | Result |
| --- | --- |
| R1: exact official source and shipping builds | Met; official source plus the recorded EEZ dependency graph, with no Reth patch |
| R2: execution, signer and commitments | Covered checks pass, including independent live replay; the two gas fixtures needed the documented fee correction |
| R3: fresh live deployment and settlement | Met twice, with all 32 eligible protocol scenarios and independent evidence gates |
| R4: reorg and restart | **Not met**: both persisted forkchoice regressions fail on official v2.6.0; the default L2 persistence settings also fail nine crash cases |
| R5: reviewable implementation and evidence | Recorded here with source, binary/image identities, commands, first failures and exclusions; raw logs/datadirs are local-only |

The nine crash cases all pass under the previous persistence settings. The two
companion-gas cases pass after correcting their test fee ceilings. These
separate controls do not turn the original 90-pass/11-fail run into a green
candidate, and they do not fix the upstream canonical-head error. Ordinary EEZ
reorg success also does not cover that pending-disk-cleanup race.

Before accepting a switch, select and validate explicit L2 persistence settings
and use an official release containing the fix for #27429, or explicitly change
the requirement to permit a reviewed backport. No such backport is included in
this branch. The old pin's similar unsafe fallback is static evidence only;
this audit does not establish that the canonical-head bug was introduced by
the upgrade.

This validation does not cover mainnet initial sync, existing database
migration (excluded by the user), maximum-size cold-history utilities, K6 load
profiles, the protocol Anvil runner, or the full CI artifact/promotion matrix.
The pinned Solidity suites, local native suites and live Kurtosis suites are
distinct coverage. No performance comparison or guarantee against all possible
failures is claimed.
