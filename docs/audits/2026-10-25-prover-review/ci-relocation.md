# History observation fixture relocation in CI

Status: the fixture lookup failure is reproduced by CI at `7558372b`; the
corrected process test passes locally with the original fixture path hidden.
The final PR head still needs its own complete CI matrix.

## Source and artifact identity

| Field | Value |
| --- | --- |
| PR head used by the failed run | `7558372be60b0f9edcd986f41e6676368bc04634` |
| CI merge commit | `4306be37743829705f910b3ef14161a4ffe185ac` |
| Identical tree in both commits | `0613dc704c051070be29cd1580532adccd4cc3f4` |
| Protocol gitlink | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged |
| Corrected test executable SHA-256 | `2dac5500b03e9cb159608d1af576a32a9bb4998869686a46d2f0d3d5e4af90fd` |
| CI signer executable SHA-256 | `43680285f6ab00260e91201bf80d545a3b332a2094ab6a525135176b8a52c1cf` |
| Signer image | `ghcr.io/eez-association/eez-rollupx-proof-signer@sha256:2dd43610fe904d17b4a96cc8bb6f99c2475318b82310d857ddf221b8c02ac932` |

The local test executable includes the uncommitted fixture lookup fix and combined
history test support on top of `7558372b`. The signer is the release artifact from
that commit's CI producer, without a local rebuild. The existing artifact verifier
checked its manifest against the exact CI source checkout, protocol and lockfile.
The producer and head have the same tree; a commit-SHA image tag alone was not used
as provenance evidence.

## Failure and correction

[Cross-chain slice 2 in CI run 40220870906](https://github.com/eez-association/eez-rollupX/actions/runs/40220870906/job/121261184398)
ran 33 tests: 32 passed and the history-observation process test failed immediately
with `No such file or directory`, before starting the signer. The test used the
compile-time `CARGO_MANIFEST_DIR` from the producer's build filesystem. That path
is not the consumer's checkout after Nextest archive relocation.

The run is complete: all other validation jobs succeeded, including Kurtosis;
the aggregate gate failed because of this shard. The retained final job summary
is `artifacts/prover-review/ci-7558372b/final-run-status.json`. These results bind
to `7558372b`, not the later Composer fixes.

The [process test](../../../crates/eez-node-e2e/tests/cross_chain/history_observations.rs)
now uses the existing `eez_testkit::repo_root()` helper. It resolves the runtime
`CARGO_MANIFEST_DIR` supplied by the test runner, retaining the existing fallback
for local execution. No wire, signer, protocol or CI artifact policy changed.

## Executed relocation check

From the isolated worktree, the check created a separate minimal workspace with
the committed history fixtures and `scripts/ci/run-offline-signer.py`. It set
`CARGO_MANIFEST_DIR` to that workspace's `crates/eez-node-e2e`, selected the verified
CI signer explicitly, and ran the compiled test directly in a single Bubblewrap
namespace. An empty read-only directory covered the original fixture path. This
prevents an accidental pass by reading the original build checkout's fixtures.

The executed test filter was:

```text
--exact cross_chain::history_observations::scheduled_history_observations_are_bound_before_offline_signing --nocapture --test-threads=1
```

Result: **1 passed, 0 failed, 181 filtered**, in 2.24 seconds. Its assertions
validated all 36 block acknowledgements, recovered the expected attester from
two correct-result signatures, and rejected four earlier-state results at
`observation_replay` without extra signing calls. The signer's outbound network
access was denied by the existing seccomp launcher. This is process evidence;
it does not replace the complete Nextest archive lane at the final commit.

Two earlier local harness setup failures are retained: a nested Bubblewrap
namespace was unavailable, and the first minimal workspace omitted the offline
signer launcher. They failed before exercising observation validation. The
single-namespace check with the complete fixture workspace passed; neither setup
failure is counted as a product defect reproduction or a flaky product test.

## Evidence and remaining gate

All paths below are relative to the isolated worktree and **local-only**. They
are not present in another checkout. The committed fixture and process test are
the portable regression inputs.

| Evidence | Path / SHA-256 |
| --- | --- |
| Failed CI shard log | `artifacts/prover-review/ci-7558372b/cross-chain-2.log`; `dcde90d38526c3afdcc48c39e978606e20ae682030ecd52900a5a7f32b0f76af` |
| Producer log | `artifacts/prover-review/ci-7558372b/producer.log`; `526493236dd6228c09e154f96e08df546496423f0560ccb52d0c615fb657297b` |
| Relocation command, identities and outcome | `artifacts/prover-review/ci-7558372b/fixture-relocation-complete/result.json` |
| Passing process output | `artifacts/prover-review/ci-7558372b/fixture-relocation-complete/output.log`; `d758c3d6492d653f0a66fa503ad07d31c453ed8a68e9daf72c10e9bfafbb2ecb` |
| Local reproduction driver | `artifacts/prover-review/ci-7558372b/check-relocated-history-complete.py` |
| Runtime verification | `artifacts/combined-history/runtime-verify.log`, `artifacts/combined-history/ci-runtime/manifest.json` |

Re-run the complete CI matrix on the final head, including the archived process
test. Success of other shards at `7558372b` does not make that failed run green
or validate a later Composer artifact. No operator service was restarted and
no deployed signer upgrade is claimed by this check.
