# CI validation and deterministic runner fixture

| Field | Value |
| --- | --- |
| Date and status | 2026-10-25; original CI completed, deterministic fixture fix and local tooling suite pass; combined CI pending |
| Scope | Full candidate CI and the failed L1-profile propagation fixture |
| Source | Branch head `1109dbab7f12fc735a6bf6986e7d7e7f987495d7`; CI merge `305f3dc26d53f96e529f4b026f7d44c88ab49c72` |
| Base | `a50c326d99f7cd1c21406eedbf370a344015a6b5` |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, initialized and clean |
| Source equivalence | Both head and CI merge have tree `d40ef299db6206cc919ed6d90384a5c92389e038` |

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
those two tests; the new full CI must run them again. The mocked tooling suite
does not establish live deployment or independent replay.

A new full CI result is required for the combined implementation; the earlier
CI results do not validate subsequent runtime changes.
