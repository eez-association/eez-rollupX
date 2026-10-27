# CI validation and deterministic runner fixture

| Field | Value |
| --- | --- |
| Date and status | 2026-10-25; original failed CI preserved; corrected candidate's full CI and aggregate gate pass |
| Scope | Full candidate CI and the failed L1-profile propagation fixture |
| Original CI source | Branch head `1109dbab7f12fc735a6bf6986e7d7e7f987495d7`; CI merge `305f3dc26d53f96e529f4b026f7d44c88ab49c72` |
| Corrected runtime | Branch head `b1881cf737cdc64ee30a9767807f53d21a3daa86`; CI merge `57d212d80b89079f4ed9123ed379e94790a59bf7` |
| Base | `a50c326d99f7cd1c21406eedbf370a344015a6b5` |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, initialized and clean |
| Original source equivalence | Original head and CI merge have tree `d40ef299db6206cc919ed6d90384a5c92389e038`; corrected source equivalence is recorded below |

## Original CI result

[Run 40189358764](https://github.com/eez-association/eez-rollupX/actions/runs/40189358764)
completed with a failing aggregate gate. The runtime producer, debug unit tests,
quote viewer, all four embedded E2E partitions, all four cross-chain E2E
partitions and Kurtosis passed. Lint passed formatting, secret scanning and both
dependency checks, then failed `Test CI scripts`. Its subsequent settlement
configuration check and Clippy step were skipped, not passed.

The authorized `crypto-bigint` exception was effective in CI. It remains an
[accepted unresolved dependency risk](../2026-10-24-extended-validation/dependencies.md#temporary-crypto-bigint-exception-2026-10-25),
not a cryptographic fix.

The sole failing Python test was
`ProtocolNetworkRunnerTest.test_local_l1_profile_is_required_and_reaches_the_collector`.
The suite selected 417 tests and reported one failure and seven exclusions.
Its second successful-profile subcase reused the first subcase's upstream run
directory because both invocations started in the same second. The wrapper
correctly rejected that directory as stale even though the scenario printed
`PASS`. This was a fixture collision, not an L1 profile propagation failure.

## Reproduction and correction

The reproduction replaces the fixture's `date` executable with a constant
`20000101-000000` and executes the original test. Against the unchanged fixture,
the second subcase deterministically fails with the same stale-directory error.
The correction assigns a distinct deterministic timestamp to each successful
profile subcase. The same reproduction then passes both subcases. Production
runner code, freshness checks, profile validation and assertions are unchanged.

The separate existing stale-directory test still checks that old logs are
rejected and preserved. It passes in the complete tooling suite after the fix.

An initial local invocation hit the shared `/tmp` filesystem quota before the
intended checks. Its six environmental failures are preserved separately and are
not evidence for the log collision. The controlled before/after runs bind a
task-owned temporary directory to `/tmp`; they fail/pass in 0.95/1.04 seconds.

## Retained evidence

Ignored local-only evidence is under
`artifacts/pending-validation-20261025/ci-40189358764/`:

- `run.json`, `jobs.json`, `artifacts.json`, and `pr.json`: GitHub state at review.
- `lint.log`: SHA-256 `760eb16797d3f524b99b697132587f499fd4945cbbae750071f5cf84d7d32d5a`.
- `producer.log`: SHA-256 `5410ea0debcb61abd340a100a1e6cc54a417fc07b5a14fba99b72397b54a64ea`.
- `fixture-before.log`: initial temporary-filesystem quota failure.
- `fixture-before-isolated-tmp.log`: deterministic original fixture failure.
- `fixture-after-isolated-tmp.log`: same check passes after correction.

The full local command,
`python3 -m unittest discover -s scripts/ci/tests -v`, passes through the existing
task-local resource/temporary-directory guard: 417 selected, 408 passed, nine
excluded, 329.08 seconds, exit zero. The guard did not cancel it; minimum
available host memory was 23,636,815,872 bytes. Its output and result are under
`artifacts/extended-validation/checks/pending-ci-tooling-20261025/`.

The exclusions are seven existing opt-in live Docker/Kurtosis/native-archive
probes and two Gitleaks-dependent tests because Gitleaks is absent locally. They
are not counted as passing coverage. The original CI installed Gitleaks and ran
those two tests; at this local checkpoint, the new full CI still needed to run
them again. The mocked tooling suite
does not establish live deployment or independent replay.

At that checkpoint, a new full CI result was required for the combined
implementation; the earlier CI results did not validate subsequent runtime
changes. The completed result is recorded below.

## Combined candidate

The runtime/test correction is committed as
`b1881cf737cdc64ee30a9767807f53d21a3daa86`.
[Run 40195871216](https://github.com/eez-association/eez-rollupX/actions/runs/40195871216)
tests merge `57d212d80b89079f4ed9123ed379e94790a59bf7`; both commits have tree
`2f6d561c097cec4b1e4b874dacd0ba5d643e7dd2`. The protocol and lockfile remain
unchanged. An isolated checkout of that exact merge, with its pinned submodules,
is prepared for the repository's immutable artifact verifier.

The full Lint job passes, including formatting, secret scanning, both dependency
checks, settlement configuration propagation and strict workspace/all-target
Clippy. Its tooling suite selects 417 tests: 410 pass and seven existing opt-in
probes are skipped. Both Gitleaks-dependent tests execute in CI. The original
profile-propagation fixture and the separate stale-directory rejection test
pass without relaxing production freshness checks.

The exact Lint log is retained locally at
`artifacts/pending-validation-20261025/ci-40195871216/lint.log`, SHA-256
`7a3df7a474d1db2b9215ce3f3eeb58d2dc2f4ea416cbb51f028edc372e927293`.
The quote viewer also passes. Workspace unit tests pass **1,576/1,576**, including
both new controlled admission regressions. Three existing explicit utilities are
excluded: `profile_blocked_backlog_discovery`, `regenerate_offline_vectors` and
`inventory_native_proving_fixtures`. Frozen conformance expectations were not
regenerated. The separate Reth cleanup and shared-tracing-quota regressions each
select and pass one test; 206 and 34 unrelated tests respectively are outside
those targeted filters.

The unit log is retained beside the Lint log as `unit.log`, SHA-256
`38c070d416743113a7763ef41a7c64025fe79fde7c6dd9e1a4bf5646ee48fd93`.
The runtime producer also passes its 38 release protocol/vector/blob-evidence
tests, with no exclusions. Shipping compilation took 24m 46s; the separate
release test archive took 15m 47s. The seven shipping executables were frozen
before test archiving and retained their hashes, as required by the producer.

The exact runtime and E2E payloads are:

- `ghcr.io/eez-association/eez-rollupx-ci-runtime@sha256:40ec3c0114eb91689b6a54f8109591762f4b9dfc0a27ed963fbb80ec1bbaf257`
- `ghcr.io/eez-association/eez-rollupx-ci-tests@sha256:d68e7a4827da18cf6c3bd2e2ba2e0a8e6b41603f0d1e2a7c73d55334def3018e`

The existing `ci-payload.sh` and `prepare-runtime-artifacts.sh` consumers verify
the exact merge/protocol/lockfile, Linux amd64 release profile, archive checksum
and three immutable image identities. The native verifier checks all 30 E2E
input files and confirms that its role executables and runtime archive seal
match the separately prepared shipping bytes. No missing artifact was rebuilt
or substituted. `verification.json` beside the logs records the full handoff;
its runtime archive SHA-256 is
`0a5f84fde0e191dcf5d2803c871dc77c2d830897e9620a4cf316144e66c3b664`.
The producer log SHA-256 is
`6ed7fbd4d1f5c119f98ab8b480b7a7f8f9f6ed01be5514f2944c870748b7f29a`.

All four embedded E2E partitions pass: slices 1–4 execute 11, 11, 11 and 10
tests respectively, for **43 passing process tests**. Their per-slice skipped
counts (168, 168, 168 and 169) include tests outside each partition/filter and
the explicitly ignored live Kurtosis replay case. The latter requires a
prepared enclave and separate execution; these counts are not additional
passing coverage. Exact job logs are retained alongside the
producer log as `job-121187111387.log`, `job-121187111351.log`,
`job-121187111311.log` and `job-121187111436.log` respectively.

All four cross-chain partitions also pass: slices 1–4 execute 33, 33, 33 and 32
tests respectively, for **131 passing process tests**. Their exact logs are
`job-121187111352.log`, `job-121187111423.log`, `job-121187111327.log` and
`job-121187111297.log`. No diagnostic retry was required. The combined process
inventory is 174 passes and five existing explicit exclusions:

- `derivable_operations::ethereum_native_large_history_cold_checkpoint`
- `derivable_operations::gnosis_native_large_history_cold_checkpoint`
- `derivable_operations::multipart::four_days_settle_in_parts_with_cold_offline_provers_and_independent_replay`
- `derivable_operations::retained_native_history_cold_checkpoint`
- `kurtosis_policy_replay::kurtosis_policy_batches_replay_independently_from_l1`

The first four optional retained/large-history cases were not executed in this
follow-up. The final case needs a live enclave and passed separately against
the final archive during the [local comparison](latency.md); it is not counted
as an executed case of these CI partitions.
The inventory comes from the verified final test executable's `--list --ignored`,
retained as `ignored-process-tests.txt`; per-partition skipped counts also include
all other slices and suite filters and must not be summed as separate exclusions.
Comparing the final executable's full `--list` inventory with all eight PASS
logs confirms that every one of its 174 non-ignored tests ran exactly once:
zero missing cases and zero duplicates. [The compact record](ci.json) retains
the inventory counts, explicit exclusions, job identities and log hashes.

The unchanged Ethereum delayed mixed-wave admission test passes in 62.270 s
and its Gnosis counterpart in 95.870 s. The strengthened inbound and outbound
MissingProxy tests pass in 53.162 s and 48.116 s, respectively. These are
explicit selected tests, not inferred coverage from an aggregate success.

Kurtosis also passes, including the four wave modes and their proof-signer
checks, state chaining and **34/34 selected protocol scenarios**, with no
failure or skip inside that selection. Its independent verification reports
complete evidence, causal coverage and semantic blob checks. The pinned
default suite excludes **30 of 64 discovered scenarios** before selection;
these are exclusions, not passing coverage. Their classification is documented
in the [Kurtosis guide](../../../testing/kurtosis/README.md). The two excluded
MissingProxy scenarios are separately exercised by this follow-up's explicit
live controls. The standalone cold-follower archive test is separate local
evidence, not inferred from the protocol collector's success.

The Kurtosis log SHA-256 is
`499e4a2de74f54715fd99d25495509dfa0583a866ee8de07b5bfd54a4d4a8a91`.
The aggregate `CI Result` job `121201692050` confirms all required lanes passed;
the run completed successfully at **14:52:21 UTC**. Its `gate.log` SHA-256 is
`c848dc3dfd8ed9f251216e9f88e5b7f7677f590638a595078e8422b5d9f65162`.
This is a PR run; it did not promote production image tags.
