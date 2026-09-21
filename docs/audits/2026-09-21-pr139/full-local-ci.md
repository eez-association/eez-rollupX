# Full local CI validation

## Candidate and isolation

Requested after GitHub publication remained blocked. Candidate:
`4da1efd0e01c8226fdcad8faba6b14238cddec72`, protocol
`3c0cdf4955941a5c23639346932b55b33babebbd`. No branch push, image publication,
PR creation, dependency change or production deployment is part of this run.

The host has live Chiado/observability services. Read-only preflight confirmed
the Docker endpoint is the local Unix socket, 22 CPUs, approximately 25 GiB
available memory, and approximately 4.1 TiB free disk. Existing services and
stopped Kurtosis enclaves are not task resources and must remain untouched.

Use the existing `scripts/ci/build-runtime-artifacts.sh` no-publication path:
`EEZ_PUBLISH_IMAGES=false`, `EEZ_RUNTIME_CACHE=false`, `EEZ_COMPILER_CACHE=none`.
The local orchestration adds only Buildx RUN-container resource flags:
`--resource memory=18g --resource memory-swap=18g --resource cpu-quota=200000
--resource cpu-period=100000`. A real preflight RUN observed `memory.max`
19327352832, `memory.swap.max` 0, and `cpu.max` `200000 100000`.
No release-profile changes or Docker daemon reconfiguration.

Local run identifier: `local-pr139-zGdlZCDi` (not a GitHub Actions run).
Temporary orchestration: `/tmp/eez-pr139-ci.zGdlZCDi/build.sh`.
Producer outputs/logs: `artifacts/pr139-full-ci/{runtime,tests,producer.log}`.
These are local-only; another checkout does not contain them.

The producer resolved Rust base
`rust@sha256:365468470075493dc4583f47387001854321c5a8583ea9604b297e67f01c5a4f`
and runtime base
`debian@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251`.
Runtime and native archives must pass the existing identity/checksum gates;
previous dev-profile artifacts are not substitutes.

`/tmp/eez-pr139-ci.zGdlZCDi/after-build.sh` queues the consumer only after the
producer's completion marker (or stops on its failure). It invokes the existing
native archive verification/unit/export/suite helpers and runtime preparation,
then `testing/kurtosis/run-ci.sh`. Native suites remain serial and retain their
no-retry policy. Host consumer subprocesses are scoped to four CPUs, 18 GiB,
and no swap; unit concurrency is two. Kurtosis uses a fresh, rechecked-absent
`eez-pr139-zgdlzcdi` enclave with normal resource checks enabled and two protocol
scenario workers (all scenarios, unchanged assertions). The CI script removes
only that task enclave after capturing diagnostics. Operator RPC/wallet and
old-prebuilt overrides are cleared in the consumer subprocess.

## Full-run results

- Formatting and workspace Clippy with Rust 1.94.0, locked offline dependencies,
  all targets, and `-D warnings`: passed (existing `target/pr139-check` cache).
- `python3 -m unittest discover -s scripts/ci/tests -v`: 245 tests, success,
  **four opt-in integration tests skipped**, 110.082 seconds. Those are the
  real Docker payload, tiny native archive relocation, Docker runtime packaging,
  and Kurtosis resource probes; normal CI leaves these opt-ins disabled too.
- `python3 scripts/check-settlement-policy-config.py`: Chiado and Kurtosis passed.
- `forge test --root contracts`: **33 passed**, no failures or skips (additional
  local check; does not substitute for protocol network scenarios).
- `cargo +1.94.0 deny check advisories bans sources`: **failed advisories**;
  bans and sources passed. Diagnostic repetition preserved the same failure.
- `cargo +1.94.0 audit --json`: **failed**, one non-ignored vulnerability.
- Shipping release executable build: **passed**, 32 minutes 45 seconds, unchanged
  release profile.
- Main release test archive compilation: **passed**, 37 minutes 26 seconds;
  packaged 34 binaries (four non-test), 1,109 files.
- Separate composer-unit archive compilation: **passed**, 11 minutes 11 seconds;
  packaged three binaries (two non-test), 1,073 files.
- Runtime image packaging and producer identity/checksum sealing: **passed**;
  all three images remain local (`published: false`).
- Complete selected release unit/vector suites: **1,124 passed**, no failures or
  retries. Main archive: 917 passed in 60.394 seconds, two ignored; composer:
  207 passed in 1.029 seconds, one ignored. The ignored cases are the signer's
  long-span cold feasibility measurement and local captured-fixture inventory
  utility, plus the manual held-pool component profile. Native E2E binaries are
  deliberately excluded from this unit selection and run in the following suites.
- E2E export verified after unit success: 45 files, 382.8 MiB uncompressed.
- Complete embedded suite: **35 passed, 1 failed, 1 ignored**, 2,722.989 seconds,
  no retries. Failure detailed below. The ignored `kurtosis_policy_replay` target
  requires its explicit live evidence/endpoint fixture; this standard native
  selection does not exercise it. Log: `artifacts/embedded-e2e/nextest.log`.
- Complete cross-chain suite: **58 passed, 1 failed, 3 ignored**, 3,319.928
  seconds, no retries. The failure was the quota-contaminated fixture setup
  detailed below. All escrow-recovery, partial-consumption and queue-reorg cases
  passed. The three ignored cases are explicit large cold-history/captured-fixture
  checkpoints, matching CI. Log: `artifacts/cross-chain-e2e/nextest.log`.
- Runtime consumer image/archive identity checks: **passed**, no recompilation.
- Kurtosis: **passed**. Inbound, outbound, mixed and mixed-pure wave modes;
  state chaining; safe-head/cross-chain convergence; zero L1/L2 root divergence;
  45 signed proof windows and 45 remote attestations recorded in `result.json`.
  All **28/28** discovered upstream protocol scenarios passed, zero failures or
  skips. Accounting, evidence, causal coverage and authenticated semantic blob
  coverage are complete. Protocol execution took 253.79 seconds for the bridge
  prerequisite and 466.06 seconds for the remaining 27 scenarios; independent
  verification took 22.446 seconds. The complete network lane, including enclave
  startup and preceding wave/state tests, ran approximately 33 minutes.
  Evidence: `artifacts/pr139-full-ci/kurtosis/result.json` and
  `artifacts/pr139-full-ci/kurtosis/protocol-e2e/summary.json`.

The consumer orchestration exited **1**, preserving both first native-suite
failures despite the later passing network lane. Dependency checks also failed
independently. This is a completed local validation run, **not all-green CI**.
The unique task enclave was removed by its normal cleanup; a post-run listing
confirmed it absent, the pre-existing stopped enclaves still present, and existing
Docker services retaining their prior uptimes. No remote workflow/promotion ran.

## Sealed release artifacts

The runtime and native manifests both bind the candidate/protocol commits above.
Local image references use the suffix
`:sha-4da1efd0e01c8226fdcad8faba6b14238cddec72-local-pr139-zGdlZCDi-1`.

| Artifact | SHA-256 / local image ID |
| --- | --- |
| Runtime binary archive | `376e61175261037150d1d5e9d9e90e4b181922b30148b7dc6cce6e7375776861` |
| Main native archive | `1b2e57a2c56515d50654fe04233e708bfd32b86f47d3fe5de9e12fe6e12149ef` |
| Composer-unit archive | `3da63dab1f0ceb37f749ed7491bd088efac020aa7a3001adbc2d7dd2805000fc` |
| Composer executable | `a678b2c662626dd0c0f3120b64cdae2f511d6907b570984caab1c87feaad01b2` |
| Follower executable | `af416f42a4b199397756dc8a9f1931ea7e71f7c4c4372fe4355ac0ed61177a79` |
| Proof-signer executable | `6d4205aad3abd3ea7b7e87afa8853fa8029e1982834e59f6d83ba44ba41d2d93` |
| Node image | `sha256:02cc068b8f7468d0823bdc861b27281cc5fbffc803df17d9b957006475deff5d` |
| Signer image | `sha256:8e8c868647021989ffcdb072a80bb81b5594df39f4166d0e0e40d3402d8d500a` |
| Deploy image | `sha256:c518ce8ada77a61076c9f25fe47c0db362b6f890b64bc39e995e3e3f697a0b4d` |

## Dependency gate blocker

Both dependency commands reject `imbl-sized-chunks 0.1.3` for
`RUSTSEC-2026-0292`: double-free/use-after-free in removal methods when an
element's destructor panics. The audit reports patched versions `>=0.2.0`.
Dependency path: `imbl-sized-chunks` -> `imbl 7.0.0` -> Reth transaction pools
(both pinned Reth sources). This is the advisory's claim, not an exploitability
assessment of this application's actual element types.

`Cargo.toml`, `Cargo.lock`, and `deny.toml` are unchanged from the recorded main
base `4f4e73e6`; this backport did not add the vulnerable version. Advisory
database commit: `bd8037e5cbb8d8cc687c68cdd42ca53d742503fb`, reported last updated
2026-09-21. No new suppression or incidental dependency upgrade was made.

Diagnostics: `artifacts/pr139-full-ci/dependency-policy.log` and
`artifacts/pr139-full-ci/cargo-audit.json`. Ordinary wildcard/yanked warnings
are distinct from the blocking vulnerability. A passing runtime suite cannot
override this gate; full CI is not green while it remains unresolved.

## Embedded restart-fixture failure

`settlement_policy::restart_after_intermediate_prefix_discards_old_intent_and_uses_current_rules`
failed on its first execution after 255.936 seconds. The failing statement is
`world.held_bundle()`'s 180-second wait at `settlement_policy.rs:2057`, before
the intended intermediate-prefix restart. No retry or test adjustment was made.

The fixture first polls for a positive safe head, then calls `hold_next_bundle`.
The retained composer log shows:

| UTC time | Observation |
| --- | --- |
| 14:18:38.879781 | First safe-head advancement, through L2 block 48, from L1 block 13. |
| 14:18:39.223296 | Following minimal bundle dispatched, targeting L2 block 78. |
| 14:18:39.342041 | Embedded L1 accepted the following bundle. |
| 14:18:42.835470 | Second safe-head advancement, through L2 block 78, from L1 block 14. |

Later observations report no pending pure-L2 transactions and no due work. This
supports a fixture hold-arming race: dispatch follows the first safe-head event
by about 344 ms, within the test's 500 ms polling cadence, and the drain completed
instead of leaving a following held request. The exact hold-file write time is
not retained, so this remains an inference rather than a fully synchronized
reproduction. The test source is unchanged from base `4f4e73e6`, but this exact
failure has **not** been reproduced on the base revision. Do not call it a proven
pre-existing flake or convert the failed candidate test into a pass.

Evidence: `artifacts/embedded-e2e/nextest.log`,
`artifacts/embedded-e2e/logs/eez-node-restarted-composer-1958405-3.log`, and
`artifacts/pr139-full-ci/datadirs/eez-cross-chain-1958405-UrVpBb`.
The next `restart_reevaluates_current_interval_without_resetting_the_l1_timer`
case passed; remaining cases continue under `--no-fail-fast`.

## Build-cost investigation and validation-environment incident

In response to the user's question about build time, the main archive's package
selection was inspected. Its `eez-deriver` dev dependency already enables Tokio
`test-util`, despite the producer comment saying only composer needs that variant.
The normalized `cargo +1.94.0 tree --offline --locked --workspace --prefix none
--format '{p}|{f}'` package/feature sets, with and without `--exclude eez-composer`,
were identical. This is a feature-listing comparison, **not** a build-unit-graph
or behavioral equivalence proof. Consolidating test builds is a candidate for
separate review while preserving frozen shipping binaries, all selected test IDs,
critical feature configurations, checksums, and revision checks. No pipeline or
repository instruction was changed during this run.

Compression is negligible: 1.25 seconds for the main archive and 0.65 seconds for
the composer archive. Compiler measurements report approximately 189%, 197%, and
183% CPU usage for runtime/main-test/composer-test builds respectively, under the
two-CPU cap. Compiler caching was disabled; only source-download caches were used.
CPU allocation, approved compiler caching, and existing sharding on isolated
workers are separate options that do not require dropping test assertions.

Suggested optimization order (not implemented in this validation):

1. Use a job-owned workspace-backed temporary directory and bounded diagnostics.
   Run inexpensive dependency gates before committing to a cold build. This
   reduces wasted failed-run work, but postpones discovery of independent failures;
   every lane must still pass before promotion.
2. Enable an approved compiler cache and measure a higher CPU allowance against
   host headroom while retaining memory/swap limits. Do not assume linear speedup.
3. Validate a unified test archive against the full selected test-ID inventory,
   actual build-unit/feature configurations, and frozen runtime identities. The
   current repository instructions explicitly require the separate composer
   archive; changing that contract requires reviewing its implementation,
   verifiers, tests and instructions together. The 11-minute third compilation
   is an opportunity, not a demonstrated 11-minute saving.
4. Use the existing four-way native-suite partitions on isolated workers when
   capacity permits. Keep tests serial within each partition, require every
   partition, and preserve first failures. The local no-publication path is
   intentionally serial; enabling concurrency in one shared fixture environment
   is not an equivalent substitute for isolated runners.
5. Reuse verified same-revision artifacts for diagnostic executions. Never
   weaken the release profile, replace shipping executables with test-feature
   binaries, omit tests, or loosen settlement/follower assertions for speed.

An initial optional comparison mistakenly used `cargo tree --no-dedupe`, expanding
repeated dependency paths excessively. Its two sort processes exhausted temporary
storage quota at about 14:58:05 UTC. The corresponding cross-chain case
`dynamic_admission::ethereum_explicit_preparation_mixed_wave_after_small_post_replays`
failed after 18.445 seconds during fixture setup (`dynamic_admission.rs:51`), with
`durable witness capture unavailable: Disk quota exceeded`. Its node first logged
the quota error at 14:58:05.625704 UTC. The filesystem has a per-user quota enabled;
aggregate `df` free space alone does not establish available user quota.

This result is contaminated by the agent's concurrent diagnostic workload and
must not be classified as evidence of a queue-code regression. The optional
comparison's two Cargo processes and diff process were terminated. Only their
two verified scratch files/directories, `/tmp/uutils_sortPC1q0I` and
`/tmp/uutils_sortg64aAh`, were removed (800,813,056 bytes total). No CI fixture or
operator data was targeted. The bounded, deduplicated comparison completed in
under a second afterward. Existing Docker services retained their previous
uptimes and health-enabled containers remained healthy; this is not a deep
application-health assessment.

The first test failure remains in `artifacts/cross-chain-e2e/nextest.log`, with
node log `artifacts/cross-chain-e2e/logs/eez-node-node-2116177-2.log` and datadir
`artifacts/pr139-full-ci/datadirs/eez-cross-chain-2116177-9Wedvx`.

After Kurtosis completed and cleaned up, one diagnostic rerun of exactly that
test **passed in 57.058 seconds** (Nextest run
`31e2f86d-ad46-447a-b273-e7f25b0271bd`). The existing native helper rechecked the
same candidate/protocol identity, Nextest version and E2E payload hashes. No
Rust build occurred. The rerun used the CI profile with zero retries, one test
thread, the same frozen runtime executables, and a scope capped at four CPUs,
18 GiB and no swap. Its `TMPDIR` was changed to
`artifacts/pr139-full-ci/quota-rerun/tmp` on the workspace filesystem. Selection:
`package(=eez-node-e2e) & binary(=dynamic_admission) &
test(=ethereum_explicit_preparation_mixed_wave_after_small_post_replays)`.
The other 15 tests in that binary were filtered, not rerun. Log:
`artifacts/pr139-full-ci/quota-rerun/nextest.log`.

This diagnostic supports the environment-contamination explanation; it does not
erase the failed first full run or establish that a second full suite passed.
Future local validation should use a job-owned temporary directory on the
workspace filesystem and bound auxiliary diagnostics as well as compilation.

## Remaining gate work

- Resolve the dependency advisory without an unreviewed suppression or incidental
  Reth/protocol update; validate the selected compatibility-preserving remediation.
- Reproduce and diagnose the embedded restart-fixture timeout, including a base
  revision comparison where practical, before deciding whether a fixture or
  runtime correction is warranted. No timeout relaxation or diagnostic retry
  has been used to dismiss it.
- After any code/dependency change, rebuild and validate artifacts for the new
  exact revision. This run's artifacts must not be relabeled as that candidate.

The audit and its links are the only tracked changes made while running this
validation. Their later documentation-only commit is not the executable candidate:
all release evidence above belongs to `4da1efd0e01c8226fdcad8faba6b14238cddec72`.

Subsequent work is recorded in [main synchronization and lint repair](main-sync-lint.md).
That later revision resolves the dependency gate with a targeted security update;
it does not retroactively change this run's failures or validate new release binaries.
