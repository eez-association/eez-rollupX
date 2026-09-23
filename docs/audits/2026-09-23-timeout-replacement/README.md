# Next-slot deadline replacement

Status: implemented, locally validated, and deployed to the existing Chiado network on 2026-09-23 at 12:53 UTC.

- Fork baseline: `1832f970884ca9460646a4cde87cca70eaffb3e1`.
- Protocol gitlink and initialized checkout: `3c0cdf4955941a5c23639346932b55b33babebbd`, no submodule edits.
- Candidate: baseline plus the working-tree replacement patch; unrelated pre-existing
  1,000 ms proof-budget configuration and performance-runner changes preserved.
- Design: [accepted single-active-branch replacement](../../proposals/skipped-slot-speculative-retry.md).
- Baseline evidence: [fourfold live run](../2026-09-23-chiado-staged-performance-4x/README.md):
  all 15 source-phase skipped slots had a +10-second posting-target gap; all eight
  source-phase canonical exclusions had +5 seconds. This is historical live
  evidence, not a fresh matched performance comparison.

## Superseded work

The two earlier diagnostic test edits in `submitter.rs` and `dynamic_admission.rs`
were removed at the user's explicit request. The earlier two-active-branch draft
was replaced by the accepted design. The original gap-acceptance unit experiment
failed against unchanged baseline submission code; that failure motivated the
bounded exact-target gate regression. No production relaxation from that earlier
attempt had been applied or deployed.

## Validation record

Tools: isolated `eez-fee-recipient-tools:1.94.0`, Rust/Cargo 1.94.0, Foundry/Anvil
1.7.1. Builds use `/workspace`, existing Cargo download caches and `--locked`.
No live datadirs are mounted into test containers. Native fixtures use their own
loopback ports, ephemeral chains and public fixture keys.

The first unit command used `--offline` and stopped before compilation because
pinned `imbl 7.0.2` was absent from the cache. It was rerun with dependency network
access and the unchanged lockfile. This was an environment failure, not a test
failure. Affected unit suites passed: Composer 222, driver 57, L1 140. Composer's
`held_pool::tests::profile_blocked_backlog_discovery` is an existing ignored
manual timing profile, not correctness coverage. The scheduler subsequently
received a small cadence correction: arm the extra deadline without advancing
normal Live ticks ahead of the observed head; the process tests exercise it.

Integration contracts built successfully with Foundry 1.7.1; existing Solidity
unsafe-cast lint warnings remain. Chiado configuration tests passed 10 tests;
mandatory policy configuration checks passed for Chiado and Kurtosis. Python
syntax and `git diff --check` passed. Native process tests were then compiled.

Local-only build/test diagnostics: ignored `artifacts/timeout-replacement/`.
These are not retrievable from another checkout. Relevant sanitized outcomes and
executable identities will be recorded here before completion.

These initial checks did not establish deployment or performance improvement;
final deployment evidence follows below.

The full Python tooling run selected 246 tests and reported three subtest/test
failures because host `cast` was absent, plus four explicit opt-in skips
(Docker payload, real Kurtosis resources, native archive relocation, Docker
runtime build). Providing Cast 1.7.1 from the build-tools image and rerunning
`test_public_network_checks.py` passed all tests in that module. The original
failure log remains preserved; no product code was changed for this environment
issue. Tools image: `sha256:7335715efd8b672d267ecc2e8a9fc11c449e35413b1a98ee26ba06065f9aab86`.

The full tooling suite was repeated with the operator-installed host Cast 1.8.3
on PATH: 246 selected, 242 passed and four opt-in exclusions. Rust/process
builds continue to use the pinned Foundry 1.7.1 image.

The first skipped-slot process run failed in its dispatch wait: the assertion
used tracing metadata names absent from the JSON signal parser, which requires
an explicit `event_name` field. This was a test-observability defect. Its retained
logs nevertheless show replacement preparation, four unchanged source groups,
next-slot dispatch and inclusion at the same execution height. The run did not
reach its semantic assertions and is not counted as a pass. The existing
Submitter dispatch breadcrumb now exposes its stable name and target timestamp;
the regression waits for that exact target instead of an inferred event count.
The initial diagnostics remain in `skipped.log` and the corresponding native
process logs/datadirs. Subsequent test results must establish semantic coverage.

Both targeted process cases passed with semantic assertions and independent
follower convergence:

- `automatic_selection_survives_a_canonically_skipped_slot_without_shrinking`:
  original target omitted; replacement at exactly +5 seconds, same L1 execution
  height, four whole sources consumed once, selection unchanged, ordinary transfer
  retained. `skipped-second.log`: one selected, one passed.
- `late_original_settlement_replaces_the_timeout_branch_without_losing_users`:
  actual original payload held until next-slot dispatch, then imported. Original
  L2 transaction block restored, all four sources consumed once, ordinary transfer
  already executed in the replacement branch retained through recovery, independent
  follower parity. `late-process.log`: one selected, one passed.

The second Cargo invocation was stopped during a redundant dependency rebuild,
before its test started (`late.log`). The late-original case then ran directly
from the exact source-built test executable used by the passing skipped case;
prebuilt/runtime overrides remained unset. Runtime binary SHA-256 identities:

- test `dynamic_admission-72c50ceb6182f2e3`:
  `7ce9c6f53faa2326a43b04af3ae38832b7ca264971b8d22f5d06b42bf4e71db2`
- native Composer: `b39a9dfb1a3a2c96a47b774184f730c09f203d6cb1a2d1d9af1059f1281c90e1`
- native follower: `2963f5e7900a9c2df53d236ddbbb16242aaedaa5350a19115f29e1d6944b3169`

These development builds and controlled local chains establish the tested behavior,
not live Chiado performance or a production-image deployment.

Surrounding native process regressions passed without diagnostic retries:

- Four targeted `dynamic_admission` cases: ordinary predecessor recovery,
  exclusion of held outbound users from ordinary sequencing, static-only
  exclusion preserving descendants, and produced-block exclusion reducing
  automatic selection while retrying every source.
- `partial_consumption`: all five, including omitted first/last sources,
  partial-prefix accounting, restart, and same-block middle-group resumption.
- `queue_liveness`: all three Ethereum/Gnosis funding and independent-source cases.
- `queue_reorg`: one real L1 branch replacement with confirmed and unresolved
  generations recovered in nonce order.

These suites retain process logs/datadirs locally and execute their semantic
and independent-replay assertions. No `resumption` test target exists in this
revision; same-block resumption is covered by `partial_consumption`.

The remaining process checks also passed on their first run:

- `settlement_policy`: six selected (25 filtered): Ethereum/Gnosis always mode,
  overdue history retargeting, prover resource/transport failures, restart after
  an intermediate prefix, and interval restart without resetting the timer.
- `settlement_policy_gas`: all four Ethereum/Gnosis whole-group admission cases.
- `settlement_policy_cost`: both Ethereum/Gnosis bounded-fee receipt cases.

Total: 27 selected native process checks passed. The full CI/Kurtosis matrix,
remaining throughput/blob-cap policy cases, and a fresh live performance workload
were not run. This is correctness/recovery evidence, not a measured throughput
or inclusion-ratio improvement.

Fresh baseline comparison: a `git archive` of the exact fork HEAD was built with
only the new submission-gate regression inserted. A source comparison confirmed
that the baseline `submitter.rs` otherwise equals HEAD byte-for-byte. Running
`cargo +1.94.0 test --locked -p eez-l1 --lib
replacement_gate_accepts_one_missing_slot_but_rejects_late_parent_and_larger_gaps`
selected one test and failed at the intended assertion (`false` versus expected
`true` for the one-missing-slot target), exit 101. Baseline diagnostics:
`baseline-gate.log`. This is a mocked gate comparison, not a fresh baseline
process/performance run.

The same regression passed in the final candidate unit run. Final affected unit
suites passed: Composer 222 (one existing manual profile ignored), driver 57,
L1 140. `cargo +1.94.0 fmt --all --check` passed. Workspace Clippy and the
production-feature release build subsequently passed, as recorded below.

`cargo +1.94.0 clippy --locked --workspace --all-targets -- -D warnings`
passed. The final release build uses the shipping package/binary/example
selection from `Dockerfile.runtime`, Rust 1.94.0 and the unchanged lockfile.
Native process tests above use the source-built development profile; they are
not a full CI release-artifact test matrix.


## Deployment and live verification

The shipping release build passed. A local image was assembled from those exact
binaries, retaining the existing runtime base. This is a local development
rollout, not a published/promoted CI artifact.

| Identity | Value |
| --- | --- |
| Local image | `eez-node:timeout-9a1a48a2132a` |
| Immutable local image ID | `sha256:5931914094187d1c1c051697c025f4092c3fcd52eeda57491357bc368da37fea` |
| Implementation patch SHA-256 | `9a1a48a2132a7ab18d7c3b47a8e5626e31a698d09dbbcde9793e6c55bd88ebc2` |
| Running Composer executable SHA-256 | `26d118c679b13695fa9ee3bfc9ad5cb2283216586ae1a878eb3fa06a05ab72bc` |
| Release follower executable SHA-256 | `55a36ed4e18a711446e97457c39ef958b8ffa531fe430eae5281b0f88ff839ed` |
| Previous node image, retained | `sha256:1de4b3f90648f49b2cfe749c5a6a84c51ffffa6a8178dd26ef4294f58fa4ac7b` |
| Existing signer, unchanged | `sha256:1fe82bae33fd602f436c714e9e33c1776a05b5ca8bd855845907008b71a24778` |

The image embeds `/usr/share/eez/build/implementation.patch` and
`source-manifest.json`. The patch includes the changed Rust/RPC contract and
native consensus fixture; the manifest binds it to the fork baseline, protocol
pin, release profile and copied binaries. The running `/proc/1/exe` hash matches
the release Composer. These artifacts remain local-only.

The existing `rollupx` Compose project and its current Chiado node/postings
profiles were used. A rendered configuration comparison changed only the node
image. The operator's environment was preserved apart from `EEZ_NODE_IMAGE`;
container environment, mounts and port bindings match the previous node exactly,
including `EEZ_PROOF_TIME_MS=1000`. Genesis and profile hashes are unchanged.
No contracts, signer, monitoring services or network bindings were replaced.

Before stopping, the cross-chain queue had zero queued, in-flight or blocked
users. `txpool_status` is not exposed by the live RPC, so this is not an
ordinary-pool inventory. The node stopped cleanly (exit 0). Private configuration
and a stopped-writer filesystem copy of the 22 GiB L1 and 1.1 GiB L2 datadirs
were retained in an operator-private local backup. Backup restoration was not
rehearsed. The node alone was recreated with `--no-deps --no-build --pull never`;
its new start time is `2026-09-23T12:53:01.0198175Z`.

Both old and new images print version details but exit 1 for `--version` because
the existing CLI propagates Clap's display-version result as an error. This was
checked against the old image; it is not a new runtime failure. Actual startup
and transaction/settlement checks below passed.

A single zero-value L2 self-transfer used previously unused public development
account `0xa0Ee7A142d267C1f36714E4a8F75612F20a79720` (nonce 0, already funded in
genesis). No account was funded and no value was transferred to another account.

- L2 transaction: `0x06023142326d3ab52d7ae94f2bdbc3f92f26ca8c9e4c2247c4fea095c38b1543`.
- Successful canonical L2 receipt in block **424823**; source nonce advanced to 1.
- L1 settlement transaction: `0xd0277ca88ce31700f81985b438d57549212a11f6df8cafcd1cb8637d9877de5a`.
- Its successful L1 receipt and canonical block identity were checked at block
  **23192229**. Validated settlement covered **1,925 L2 blocks**, through **424825**.
- Safe head covered the smoke transaction, `canonicalL2=true`, RPC reported
  `eth_syncing=false`, and the cross-chain queue remained empty. No runtime
  ERROR events were observed after restart; initial head-catch-up warnings ended.

Live diagnostics are local-only (`live-smoke.json` and private startup logs).
This verifies the deployed executable and fresh ordinary activity settlement.
No live skipped slot was induced or observed during this short check; skipped-slot
replacement and delayed-original recovery are established by the controlled
native process cases, not by this live smoke test. A new high-volume performance
comparison and the full CI/Kurtosis matrix remain outside this validation run.
