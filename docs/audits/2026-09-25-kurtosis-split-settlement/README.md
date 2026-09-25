# Local Kurtosis validation of multi-settlement evidence

The local Kurtosis CI lane passed on 2026-09-25, including all 32 eligible
protocol scenarios and their independent causal and semantic blob checks.
This validates the patched evidence tooling with the unchanged CI runtime.
The live run did not reproduce split settlement; that behavior has synthetic
Rust and mocked collector regression coverage.

| Field | Value |
| --- | --- |
| Source | `feat/two-byte-da-v0`, `6fb2ef198a719d73a3aec399e751e0648a6e7d24`, plus the uncommitted patch identified below |
| Protocol | `9d13218a355e7d543ca8a9927eea2962600b716d`, clean |
| Tested patch SHA-256 | `cba0fd0715565e89f8b022bcc5729a2ff8b425842d58c5dcda8e4519f5e3d6c0` (`git diff --binary HEAD`, before this record) |
| Environment | Local Docker devnet; Rust 1.94.0, Foundry 1.7.1, Kurtosis 1.20.0 |
| Runtime identity | Release node/signer images from CI merge `25ad58d342857b256649ea24e63c437ca3bc99d2`; its tree equals the branch HEAD tree, `150e738c15f376985ae0139b8c445e0e23efa44f` |
| Local builds | Deployment image from this checkout; patched causal verifier and blob verifier built with Cargo's dev profile; genesis helpers extracted from the CI node image |
| Related run | [GitHub Actions job 108086618849](https://github.com/eduadiez/rollupX/actions/runs/36135031718/job/108086618849) |

## Finding and change

The GitHub job passed upstream scenario checks but failed independent verification
of `counter-multi-tx` with `no ordered L1 entry causally verifies L2 entry 2`.
The pinned upstream runner supports multiple matched settlements while its final
summary names only the first. The fork collector and verifier assumed one batch.
That mismatch is a plausible cause of the historical failure, not a confirmed
diagnosis: GitHub artifact quota prevented retention of the original causal input.

The collector now retains every matched settlement, sorts canonical transactions,
and binds each trigger to its preceding settlement in the same L1 block. It checks
each batch's authenticated blob against the corresponding trigger bytes. The
verifier pairs the complete ordered batches and retains batch-local entry indices.
Missing, unused, reordered, ambiguous, or corrupt evidence fails verification.
Single-batch inputs remain supported. Runtime node and signer code is unchanged.
See the [evidence contract](../../../testing/kurtosis/README.md#run-the-pinned-protocol-e2e-suite).

## Executed checks

Commands ran from
`/home/edu/Development/eez-rollup0-fork/target/worktrees/two-byte-da-v0`.
Cargo used `CARGO_TARGET_DIR=/home/edu/Development/eez-rollup0-fork/target`.

| Check | Evidence type | Result |
| --- | --- | --- |
| Baseline verifier with new split-batch regression | Synthetic Rust unit | Failed: original parser rejects the calldata array; retaining only the first batch cannot pair the third entry |
| `cargo +1.94.0 test --locked --offline -p eez-protocol --example e2e-causal-verify` | Synthetic Rust unit | 10 passed, including split-batch acceptance and malformed-evidence rejection |
| `cargo +1.94.0 test --locked --offline -p eez-protocol` | Unit and fixture integration | 183 passed |
| `python3 -m unittest discover -s scripts/ci/tests -v` | Tooling, including mocked collector processes | 251 passed, 4 opt-in tests skipped |
| `cargo +1.94.0 clippy --locked --offline --workspace --all-targets -- -D warnings` | Static | Passed |
| `cargo +1.94.0 fmt --all --check`; `bash -n testing/kurtosis/scripts/collect-protocol-e2e-evidence.sh`; `git diff --check` | Static | Passed |
| `bash testing/kurtosis/run-ci.sh` | Live local devnet | Passed once, exit 0; 14:18:21–14:47:45 UTC (29m24s) |

The live command ran through the retained `run-local.sh` launcher, with
`KURTOSIS_ENCLAVE=eez-ci-two-byte-20260925-1411` explicitly exported,
`KURTOSIS_RESOURCE_CHECK=true`, `testing/kurtosis/ci-args.yaml`,
`EEZ_PROTOCOL_MAX_PARALLEL=all`, and explicit image/helper/verifier paths.
It skipped image rebuilds after preparation and used an outer 9000-second timeout.
Exact image digests, executable hashes, configuration, and summarized results are
in [validation.json](validation.json).

Live results:

- Inbound, outbound, mixed, and mixed-pure waves passed (40 cross-chain operations).
- State chaining, poison-middle eviction, and repeated-call checks passed.
- Protocol suite: 36 discovered, 32 eligible/selected, 32 passed, 0 failed, 0 skipped.
  Four upstream-marked static/local-write scenarios were excluded from eligibility.
- Independent causal coverage and authenticated semantic blob coverage were
  complete, with valid suite accounting. These checks are not standalone follower
  replay.
- `counter-multi-tx` passed with three triggers in one settlement. The live run
  therefore establishes compatibility but does not exercise the split-batch path.

## Evidence and operational scope

The small [validation summary](validation.json) is included with this record.
Bulk evidence is **local only**, under the ignored worktree directory
`artifacts/kurtosis-local-20260925-1411/`; it is unavailable in another checkout.
It contains `run.log`, `results/`, `tested.patch`, `identity.json`, image identities,
the exact launcher, copied verifier binaries, and validation logs. Digests of the
principal files are recorded in the summary. No retention guarantee applies.

The user authorized local testing before pushing. The run created one isolated
enclave; the runner removed it after saving results, and a subsequent enclave
listing was empty. Existing Chiado, monitoring, and benchmark resources were
preserved. Local images, binaries, and evidence were retained. No commit, push,
image publication, or public-network deployment was performed.

## Limits

The original job's diagnostic artifacts are missing, so exact historical
reproduction remains unavailable. A live split-settlement reproduction remains
unverified; the deterministic regression covers that mechanism.

This was the local Kurtosis lane, not a rerun of the complete CI producer/native
matrix. The CI runtime payload and deploy image could not be pulled; the unchanged
node/signer images were available by digest, and deployment/evidence tools were
built locally as documented. Four opt-in tooling tests were not enabled: the
Docker payload fixture, isolated Kurtosis resource probe, native archive
relocation, and Docker runtime build fixture. A standalone follower replay was
not run. No test failure was retried to obtain this passing result.
