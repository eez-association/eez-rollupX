# Admission throughput aligned with latest main

| Field | Value |
| --- | --- |
| Status | Main aligned; local correctness and paired comparison passed; full PR CI pending |
| PR base | `fd77a2ebc147850333171045dbd263ec74156268` (`main`, including PR #41) |
| Previous admission head | `6130a412ac7af9236abbc008594720d95fdb8e8a` |
| Merged source | Initial merge `5c2e73e`; runtime compatibility fix `62b8e066ef96ae54d9e4bda4636c08ba5a1c7b67`; test lint fix `ac7e0abb12914c166546cdd0afcd5ff439fc75ec` |
| Release comparison baseline | `e556e1208612d957b5732aae602c59c4ac24dd9f` |
| Protocol | Unchanged `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Merged toolchain/dependencies | Rust 1.95.0, upstream Reth `444e867155c2a7a4c028e479ca7201b0b6871579`, main's locked Gnosis/Stateless/REVM/Alloy graph |
| Prior evidence | [Admission framing/default validation](../2026-09-27-admission-framing/README.md); [main's Reth migration](../2026-09-26-reth-27429/README.md) |

The user requested pulling latest main, checking that admission performance was
preserved, and opening a PR. The pull merged without conflicts. The 5% default,
candidate framing, cheap mutable/static lower bounds, shared pipeline deadline,
FIFO preference, confirmed-failure backoff and monitoring changes remain present.
Operator runbook edits and untracked investigations are outside this PR.

## Validation contract

Use main's Rust 1.95 toolchain and locked dependencies. Check workspace unit
suites, formatting/Clippy, affected process cases and monitoring/configuration
checks. No dependency upgrade beyond the fetched main graph is intended.

Compare exact release binaries with the same native process harness on fresh,
disposable Ethereum/Osaka and Gnosis/Prague networks. Reuse the existing 30-source
cold/warm, 40-source mixed and 45-source short-result fixtures, each with two
waves, canonical receipts and independent follower replay. Alternate baseline
and candidate serially; finish compilation before measured runs. Record packing,
preparation/proving, cutoffs and gas estimates, retaining every failed attempt.
Packing must remain one source post per fitting wave. Timing differences require
paired measurements and an explicit account of noise; a passing receipt alone
does not establish performance or semantic equivalence.

The existing Chiado runtime and its databases remain at `e556e12`. Main's Reth
migration explicitly covers fresh networks and leaves existing database migration,
mixed-version rollout and rollback unverified. This comparison does not establish
an in-place Chiado upgrade procedure.

## Evidence

Local logs and build contexts are recorded under `artifacts/admission-main-alignment/`;
they are not available in another checkout. Portable executable identities,
case inventories, paired measurements and log hashes are linked below. The older
live result (870 sources, 23 posts) is prior baseline evidence and does not
validate the merged runtime.

## Compatibility corrections and first results

The conflict-free merge at `5c2e73e` did not compile. Both the release build and
workspace build exposed the same prefix-checkpoint incompatibility: main's
`StateProviderFactory` now carries chain primitives, state databases require an
`EvmStateProviderAdapter`, and REVM added a non-cloneable `state_hook`.
`62b8e06` uses main's `StateSnapshotProvider` abstraction and adapter. Prefix
execution installs no state hook; checkpoint copying explicitly rejects a hook
instead of silently discarding its observer state. Exhaustive destructuring is
retained for future dependency changes. The new regression checks rejection;
existing regressions compare complete pre-finalization state and block hashes
against independent rebuilds across Cancun/Prague/Osaka, reverted transactions,
changed prefixes and failed execution.

The first new regression compile required an explicit `MockEthProvider` type.
After that correction, workspace library checks passed: **899 tests**, with the
existing manual backlog profile ignored. The first Rust 1.95 Clippy attempt
flagged `Duration::from_millis(1000)` in an admission test; `ac7e0ab` changes it
to the equivalent `Duration::from_secs(1)`. This changes no shipping behavior.
All original failure logs are retained under the local artifact directory.

Other completed checks:

- Standalone indexer: 24 passed; two disposable PostgreSQL integration cases
  ignored (not counted as coverage).
- Tooling: 271 passed, four opt-in real Docker/Kurtosis/artifact probes skipped;
  both mandatory settlement configuration checks passed.
- Integration contracts: 82 passed. Full `forge fmt --check --root contracts`
  reports pre-existing formatting in `ECDSAProofSystem.sol` and
  `DeployRollup.s.sol`; both are byte-for-byte identical to main. Formatting of
  the changed `PostBatchGasPins.t.sol` passed. No unrelated Solidity source was
  reformatted, preserving bytecode metadata inputs.
- Rust formatting and syntax of both changed shell scripts passed.

Draft PR: [#42](https://github.com/eduadiez/rollupX/pull/42). The CI workflow is
running. GraphQL check-rollup access is unavailable to this token; REST Actions
run/job results remain readable. These are interim results, not a final CI pass.

## Exact local release comparison setup

[Executable and image identities](images.json) distinguish the old release from
the corrected merged release. Both use `release` (`opt-level=3`, thin LTO, 16
codegen units, stripped symbols) and the same shipping package/bin/example
selection. The merged recipe adds main's vendor sources, Rust 1.95 and the build
stack setting. It changes no dependency beyond main's locked graph. Source and
recursive submodules were archived before building, excluding operator edits.
These local images are development artifacts, not CI-promoted images.

The native harness is built at `ac7e0ab`; runtime images are from `62b8e06`.
Their only code difference is the equivalent duration expression inside a unit
test. Both baseline and candidate execute this same frozen harness. Each variant
supplies its exact Composer, follower and signer binaries, verified against the
recorded hashes. New disposable datadirs and fixture contracts are created per
case. Failing datadirs and all process logs are retained locally; the harness
removes successful temporary datadirs during normal teardown.

The six measured cases are the Ethereum/Osaka and Gnosis/Prague variants of:

- 30 sources: 17 inbound + 13 outbound, cold then warm, no calibration post.
- 40 sources: 24 inbound + 16 outbound, two waves replacing consumed queues.
- 45 sources: inbound calls returning no data, two waves replacing consumed queues.

The 40/45-source fixtures perform one explicit one-source setup post, excluded
from measured waves. Two repetitions reverse baseline/candidate order for each
case: **24 measured waves and 920 measured sources per variant**, plus eight
explicit setup sources. Node preparation/proof/slack settings are identical
(1500/500/700 ms), as are fixture slot cadence and four harness worker threads.
Cases run serially with no local compilation during measurement. Other existing
services on this shared host remain running; paired timings include that noise.
Both runs use `RUST_MIN_STACK=16777216` and the same inherited process environment.

The fixture verifies canonical source receipts, exactly one source post and one
Sync per fitting wave, nonce/value/log results, consumed queue bounds and an
independent follower's replay. Retained events additionally connect admission
estimates, final native preflight, declared 5% headroom and canonical receipt gas
through candidate and posting identities. Setup posts and failed candidates are
kept distinct from measured canonical waves.

## Commands and evidence boundaries

Commands ran from the repository root with Rust 1.95.0 and Foundry 1.7.1:

```bash
cargo +1.95.0 fmt --all --check
cargo +1.95.0 test --locked --workspace --lib
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.95.0 test --locked -p eez-protocol -p eez-blob-codec \
  -p eez-payload-codec -p eez-proof-signer -p eez-control-rpc -p eez-prover-client
cargo +1.95.0 test --locked --manifest-path monitoring/indexer/Cargo.toml
python3 -m unittest discover -s scripts/ci/tests -v
python3 scripts/check-settlement-policy-config.py
forge build --root contracts
forge test --root contracts
forge fmt --check --root contracts
forge fmt --check --root contracts contracts/test/PostBatchGasPins.t.sol
bash -n scripts/chiado-forge.sh scripts/run-chiado-staged-performance.sh
```

The protocol/signer/RPC command passed **614 checks** (overlapping earlier
workspace units), including independent KZG, rolling-hash and public-input
vectors. Two existing manual signer inventory/long-span utilities were ignored.
Clippy's corrected rerun passed. Initial compilation and lint failures remain
in the [local log inventory](check-log-hashes.json); no failure is counted as a pass.

Native targets were compiled with `--no-run --message-format=json`: `dynamic_admission`,
`settlement_policy`, `settlement_policy_gas`, `settlement_policy_cost` and
`partial_consumption`. Tests execute the recorded native executable directly
with `--exact TEST --test-threads=1 --nocapture`. This intentional frozen-release
comparison sets `EEZ_TEST_PREBUILT=1`, `EEZ_TEST_RUNTIME_BIN_DIR` and
`EEZ_TEST_PROOF_SIGNER_BIN` to the image-extracted, hashed binaries; it does not
fall back to stale executables or silently rebuild them. Per-case
`EEZ_TEST_LOG_DIR`/`EEZ_TEST_DATADIR_DIR` separate diagnostics. The same binaries
also run the selected correctness cases.

## Selected native correctness

All **19 first-attempt process cases passed** against the corrected release;
see [the exact inventory and native executable hashes](native-correctness.json).
Coverage includes both forks' gas-first whole-group splits and actual companion
limits, packing independent work after a gas deferral, receipt-based 5% signing,
middle-group partial consumption, blocked fee/balance senders, ordered companions,
existing-proxy multicalls, static-only exclusions, both tight preparation-budget
mixed waves, one-versus-five-exclusion backoff, skipped slots and late-original
canonical settlement. No process case required a diagnostic retry.

These are native execution/DA/signer/follower checks on fresh networks. They do
not validate existing Chiado database migration, a fresh live Chiado workload,
the full Kurtosis topology/PBS matrix, or durable pending-queue recovery.

## Paired release results

All **24 comparison process cases passed**, two repetitions of each workload
for each release. Every case asserts canonical effects and independent follower
replay. Records: [case identities/results](comparison-cases.json),
[per-wave estimate/timing/receipt table](comparison-waves.csv),
[distributions and paired ratios](comparison-summary.json), and
[local process log hashes](comparison-log-hashes.json).

| Measurement | Previous `e556e12` | Merged `62b8e06` |
| --- | ---: | ---: |
| Measured sources | 920 | 920 |
| Measured source postBatches | 24 | 24 |
| Mean preparation | 151.71 ms | 157.50 ms |
| Median preparation | 150.00 ms | 156.00 ms |
| Median proof round trip | 56.99 ms | 50.73 ms |
| Median finalization | 72.92 ms | 70.78 ms |
| Median relay delivery | 16.76 ms | 21.16 ms |
| Mean candidate start → delivery acknowledgement | 302.38 ms | 301.64 ms |
| Median candidate start → delivery acknowledgement | 298.21 ms | 301.27 ms |
| Source estimates present | 920/920 | 920/920 |
| Inbound native preflight records | 688/688 | 688/688 |
| Numeric post receipt-cost records | 24/24 | 22/24 |

Packing is unchanged in these workloads: 30, 40 and 45 sources fit one post per
wave on both networks. Every measured cutoff is `snapshot_exhausted` (queue
drained), with **zero capacity/FIFO/deadline cuts** and no posting failures after
source selection. Cold admission did not insert a calibration post. The 16
explicit setup sources across both releases are excluded from the 1,840 measured
sources and 48 measured posts.

Timing is mixed, with essentially unchanged end-to-end mean and median within
1.1%. Paired median candidate/baseline ratios are **1.020 preparation**, **0.873
proof**, and **0.980 end-to-end**; ratios of paired samples differ from ratios of
the aggregate medians in the table. Preparation is slightly slower and delivery
adds roughly 4.4 ms at the median, while proof is faster. The mean preparation
increase is concentrated in blob preparation (107.22 → 113.93 ms); prefix hashing
improves (17.60 → 16.89 ms). Main also updates `c-kzg` 2.1.7 → 2.1.8, but this
comparison does not isolate the library as the cause. The largest cold preparation
pair is 192 → 269 ms; paired end-to-end ratios range 0.826–1.250. Shared-host
noise and small samples preclude claiming a statistically significant speedup
or excluding every small timing regression. No packing or overall pipeline
regression was detected by this check. This is not a sustained live Chiado TPS
benchmark or a guarantee for larger/longer-history workloads.

### Gas and observation coverage

- Incremental full estimates and framing reserves are identical in all **24
  matched wave pairs**. Mean incremental estimate is 7,547,138 gas on both;
  framing mean is 147,925 gas (range 120,768–169,856).
- Every full-calldata projection fits its incremental admission bound. Every
  final signed post limit is exactly `ceil(native_required_gas × 1.05)`.
- All **46 retained numeric receipt-cost records** exactly match their post's
  native preflight gas. The maximum cross-release difference in paired native
  gas is 204; the admission estimates do not change. Preflight remains a
  separate measurement from receipt gas.
- Two candidate numeric receipt values remain null: the second warm 40-source
  wave in repetition two on each network. Both tests assert canonical source
  receipts, queue consumption, values/logs and independent replay. Their logs
  record inclusion; test processes exit 644/445 ms later without a retained
  receipt-cost event. Successful datadirs were removed by normal fixture teardown,
  so those two numeric receipts cannot be reconstructed from retained evidence.
  They are not filled with preflight estimates or counted as receipt-gas checks.
- Setup emitted 17 anchor-only preparation failures per release, all before the
  first source selection: baseline 11 unavailable/6 deadline, candidate 10/7.
  No failed admission candidate is omitted from the measured workload. These
  setup failures are retained and excluded from cross-chain timing samples.

## PR and remaining gates

[PR #42](https://github.com/eduadiez/rollupX/pull/42) targets `main`. Main remained
`fd77a2e` at the final alignment check. At `ac7e0ab`, CI lint and the release
artifact producer passed; native partitions were running and Kurtosis was queued
([run](https://github.com/eduadiez/rollupX/actions/runs/36341655821)). The final
evidence commit changes documentation only and starts a fresh PR CI run. Full
final-head CI is **pending**, not a pass or permission to promote images.

No additional local full-core Solidity/Anvil, full native inventory, or Kurtosis
run was performed for this alignment. The protocol gitlink is unchanged; the
independent protocol vectors, affected integration contracts, selected native
cases and shipping release binaries were checked as recorded above. Existing
manual/opt-in and PostgreSQL exclusions remain explicit. Live Chiado, operator
settings/state and unrelated local investigations were preserved.
