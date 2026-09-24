# Runtime cache rebuild on PR 34

Status: local diagnosis and fix; the edited revision has not run in CI.

## Identity and observation

- Fork: `eduadiez/rollupX`; source checkout `8be782a5ac6055f47ed231be8d20b5e7a0dafaa6`; pinned protocol `9d13218a355e7d543ca8a9927eea2962600b716d`.
- [Build job 107727091812](https://github.com/eduadiez/rollupX/actions/runs/36027335514/job/107727091812) built PR merge `b2cd266088464e7c9c689d33dca468a5999d1550` on 2026-09-24. Its build step took about 39 minutes: roughly 13 minutes for runtime compilation, 21 minutes for the main test archive, and 2 minutes for Composer unit tests.
- The preceding PR merge `acbf24203b7bb2f82a072cd2ecdadd6759cb037b` and this merge changed only the Chiado wrapper, its Python test, a README, and an audit. No `Dockerfile.runtime` build input changed. Both runs used the same Rust base digest `sha256:365468470075493dc4583f47387001854321c5a8583ea9604b297e67f01c5a4f`.
- The current build imported the PR cache and default-branch cache. Its first source miss was `COPY Cargo.toml Cargo.lock`; the subsequent source and compile layers ran again. The current job exported its registry cache successfully. Redis sccache reduced compilation, but it cannot reuse linked binaries or the test archive.

## Reproduction and change

The preceding PR cache is available at `ghcr.io/eduadiez/rollupx-node@sha256:61456167f224c10e9a204d593b236bdfc121148d8d5aec61914d06f32dbe9aa8`. On an isolated local Buildx `docker-container` builder with the same Rust base, compiler-cache argument, and source bytes/modes, importing only this cache marked every instruction through `COPY Cargo.toml Cargo.lock` as `CACHED`. Importing this cache plus the default-branch cache caused the compiler setup and Cargo copy to run. The local probe used a truncated Dockerfile, so it did not execute the expensive compile layer. Changing only source file timestamps retained cache hits; timestamps were not the cause in this probe.

The producer now imports a single cache per image: the ref-scoped cache if it exists, otherwise the default-branch cache. It still exports only to the ref-scoped tag. Runtime and deploy caches are selected independently. The artifact manifest, exact-revision checks, image labels, and unit/E2E handoff remain unchanged.

## Validation and limits

- The cache controls above used a fresh temporary builder for each case; builders were removed afterward. They demonstrate the import collision but use a different Buildx driver from CI's Docker driver.
- `python3 -m unittest scripts.ci.tests.test_runtime_artifacts -v`: 29 passed, including ref-scoped priority and fallback behavior.
- `PATH=/tmp/eez-events-tools:$PATH python3 -m unittest discover -s scripts/ci/tests -v`: 255 tests, 4 opt-in tests skipped. This is a local-only Foundry 1.7.1 `cast` path; another checkout needs its own installed `cast`. The first run without `cast` on `PATH` failed three unrelated public-network checks; the rerun passed.
- `bash -n scripts/ci/build-runtime-artifacts.sh` and `git diff --check`: passed.
- A CI run of the edited revision is required to establish the actual build-time improvement. The baseline Kurtosis job was still queued when this audit was written; no new revision was pushed before that result.
