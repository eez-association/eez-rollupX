# Reth v2.6 PR preparation on main

Status: validation in progress; draft only, blocked by upstream forkchoice #27429
and an unresolved partial-consumption crash-replay failure.

## Identity and scope

- Requested PR base: latest remote `main` fetched on 2026-09-26,
  `3346d966f3c8611be790db484abc63b3b259d367`. It advanced from
  `7c90266a5fe8cfdb46b249c61079badc2dce71a3` during validation; merge
  `ac711ea` incorporates its Docker-native Cargo cache changes.
- Frozen runtime source: `8ff44bce1d0860987267c6c456c088dfccfcc5ae`, branch
  `feat/reth-v2.6-pr`, isolated checkout under
  `artifacts/reth-v2.6-pr/rollupX`.
- Protocol: `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged and initialized.
- Reth: official v2.6.0 source
  `73a3a00862a8f14f89e30da8de001456f18cfae0`, unpatched.
- Rust 1.95.0 and Foundry 1.7.1; shipping builds use the release profile and
  production package/feature selection from `Dockerfile.runtime`.

The latest-main merge changes build tooling only; runtime Cargo manifests,
lockfile and Rust sources match the frozen runtime revision above. The only
subsequent testkit source change is Clippy's equivalent spelling of a one-minute
timeout. Image hashes in [candidate.json](candidate.json) identify the executed
binaries, not a build of the new Docker-native cache pipeline. The real Docker
fixture separately exercises that producer, artifact verifier and test consumer
with a vendored path dependency. Full EEZ CI artifact production/promotion is
not established by that small fixture.

Only the upgrade commits were transplanted from the
[previous validation branch](../2026-09-26-reth-v2.6-validation/README.md).
Its unrelated admission commits are excluded. The user's working checkout,
Chiado services, wallets and databases were preserved. This is a fresh-network
port; existing database migration and deployment promotion are outside scope.

## Restart correction

The old policy-change helper sent SIGKILL. The nine earlier failures and their
`2/0/0` controls diagnosed dependence on unflushed history; they did not establish
a production persistence requirement. The earlier audit's recommendation to
select explicit L2 settings is superseded by this test/contract clarification.
Production L2 defaults remain unchanged.

`restart_composer_with_env` now sends SIGTERM, waits for a successful exit and
fails on timeout. It does not fall back to SIGKILL. An already-exited startup
rejection can still be retried with corrected configuration. The explicit
`crash_and_restart_composer` path requires a running process and verifies SIGKILL
termination on Unix. Its partial-consumption fixture first observes the required
L1 header index and both block/state persistence frontiers in read-only MDBX
transactions. Merely advancing the latest RPC height is no longer its barrier.

The main-based pre-fix fee-ceiling test failed because a previously included
transaction's receipt was absent after restart. Its database reopened at L2
height 64; the original failure and datadirs are retained. Repeating that exact
test with the revised helper and identical frozen node/signer binaries passed
(1 selected, 1 passed). This is a controlled helper comparison, not a retry-only
pass of an unchanged candidate.

The durability boundary is documented in the
[settlement policy](../../configurable-settlement-policy.md). Neither graceful
shutdown nor a short persistence window creates durable pending-request ownership.

## Crash recovery failure

A focused `restart_after_partial_keeps_accounting_and_rebuilds_intent` check
failed (one selected, one failed) after the revised L1 disk barrier passed.
The original partial settlement survived on L1 at block 9. The restarted L2
opened at block 16 and scanned the retained batch covering blocks 1–46. During
reconstruction it reported a stalled sparse-trie task, then rejected block 46
because the self-contained block metadata did not extend the local parent.
The node refused to start L1-active tasks and the outer RPC wait timed out.
The post-failure read-only database inspection also reported Finish=16; this is
not an independent pre-crash checkpoint observation.

This used the final crash test executable and the same frozen native runtime
build used in the paired helper check. Logs and the datadir are retained at
`artifacts/reth260-pr/crash-check/`. The later full process suites use the
separately identified shipping release binaries. Neither a later pass nor the
existing short-window control erases this failure. Its relation to #27429 is
unproven; no validation guard or production persistence default was changed to
hide it. Diagnosis and a passing controlled regression remain acceptance work.

## Evidence

Local logs, failure datadirs, frozen runtime/test executables and build commands
are under this checkout's `artifacts/reth260-pr/`. They are local-only and are not
available in another checkout. Small portable summaries are recorded here when
validation completes. Prior-branch results do not validate this new base.

The initial container build was rejected by cache-directory ownership; rerunning
with the original cache paths/ownership built successfully. An initial deploy
image command supplied `NODE_IMAGE` instead of `EEZ_NODE_IMAGE` and failed before
deployment; the corrected command uses the freshly built node tools. These are
build invocation corrections, not passing network tests.

## Executed checks and reproduction

All commands run from the isolated repository root with the pinned protocol
initialized, Rust 1.95.0 and Foundry 1.7.1. Host Cargo reused the prior isolated
validation target directory and CMake 4.1.2; shipping builds ran in the retained
`eez-reth260-tools:20260926` bookworm build container. Shipping builds used
`CARGO_INCREMENTAL=0`, `RUST_MIN_STACK=16777216` and
`RUSTFLAGS="-C link-arg=-fuse-ld=mold"`:

```bash
cargo build --locked --profile release \
  -p eez-node -p eez-follower -p eez-proof-signer -p eez-protocol -p eez-l1 \
  --bin eez-composer --bin eez-follower --bin eez-proof-signer \
  --example genesis_state_root --example genesis_block_hash \
  --example e2e-causal-verify --example verify_blob_evidence
```

The frozen shipping ELF files were copied into local images; each file's SHA256
was checked inside node, signer and deploy images. This establishes the executed
binary identity. It does not replace CI's complete exact-revision artifact lane.

| Check | Result | Local evidence |
| --- | --- | --- |
| Root Rust libraries, signer binary tests and four protocol/blob vector targets | 1,195 passed, 3 intentional utility/profile ignores | `rust-unit-tests.log`, [checks.json](checks.json) |
| Standalone Stateless and tries | 9 passed | `stateless-tests.log` |
| Workspace Clippy, all targets, warnings denied | Passed after equivalent duration spelling correction; first failure retained | `clippy.log`, `clippy-02.log` |
| Rust formatting, diff whitespace, edited shell syntax, reverse application of vendor review patches | Passed | Local command results |
| Cargo deny advisories/bans/sources | Passed with unmatched-source allowance warnings | `cargo-deny.log` |
| Cargo audit | Passed with 9 allowed warnings | `cargo-audit.log` |
| Latest-main Python tooling | 261 passed, 4 opt-in checks skipped | `tooling-main-3346.log` |
| Real Docker producer/verifier/consumer fixture with vendored dependency | 1 passed; unique fixture images removed | `docker-fixture-main-3346.log` |
| Graceful restart controlled comparison | Old SIGKILL helper failed; SIGTERM helper passed using identical native binaries | `baseline/`, `fixed/` |
| Intentional partial-consumption crash | Failed after L1 disk barrier passed | `crash-check/`, [selected events](crash-replay-events.json) |

The three ignored Rust cases are the manual held-pool timing profile, signer
fixture inventory utility and long-span cold-prove feasibility utility. The four
Python opt-ins are the data-only Docker payload, real Kurtosis relay resource
probe, native archive relocation, and runtime Docker fixture. The last was then
selected explicitly and passed. The optional OCI-state corruption/warm-cache and
flat-cache probes were not selected in that fixture invocation.

```bash
cargo +1.95.0 test --locked --workspace --lib --bin eez-proof-signer \
  --test public_inputs_hash_vectors --test rolling_hash_vectors \
  --test causal_evidence --test ethereum_kzg_compatibility
cargo +1.95.0 test --locked --manifest-path vendor/stateless/Cargo.toml \
  -p stateless -p tries
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.95.0 fmt --all --check
cargo deny check advisories bans sources
cargo audit
python3 -m unittest discover -s scripts/ci/tests -v
EEZ_TEST_DOCKER_RUNTIME=1 python3 -m unittest discover \
  -s scripts/ci/tests -p 'test_runtime_docker.py' -v
git apply --reverse --check vendor/patches/*.patch
git diff --check
```

Native suites use the frozen, hashed test executables with `--test-threads=1`,
`EEZ_TEST_PREBUILT=1`, explicit `EEZ_TEST_RUNTIME_BIN_DIR` and
`EEZ_TEST_PROOF_SIGNER_BIN`, and separate diagnostic directories for each suite.
These overrides were set only after source builds and binary freezing; they do
not select an unrelated cached candidate. Baseline and corrected helper commands
select `fee_ceiling_blocks_posting_without_losing_activity_or_splitting` exactly.

## Kurtosis operation

The owned enclave is `eez-reth260-pr-20260926`. All lifecycle commands explicitly
export that name. Its actual configuration is
`artifacts/reth260-pr/kurtosis/ci-args.yaml`, generated from the previous audit's
[official-v2.6 configuration](../2026-09-26-reth-v2.6-validation/kurtosis-args.yaml)
with the frozen `eez-node`, `eez-proof-signer` and `eez-deploy` images listed in
[candidate.json](candidate.json). The canonical L1 uses official Reth v2.6.0
image digest `sha256:8ce703acf113b2a20705b6e76adebed20f74ba8591bc7c9407203b2968aca70d`;
Lighthouse is v8.1.2. The separate rbuilder fixture image reports Reth v2.2.0;
not every execution component in this PBS topology is v2.6.0.

The checked-in CI runner was copied locally with only its location resolution
and cleanup trap adjusted to retain the enclave for follow-up checks. Assertions
and workload selection were preserved. The startup initially rejected validator
registrations because the relay housekeeper had an empty validator cache. An
explicit stop/start of the owned housekeeper at 20:08:58–20:09:01 UTC refreshed
it. The runner also performed its normal refresh at 20:09:39–20:09:43. Before/after
logs are retained. This was an environment intervention; the result does not
establish unassisted cold relay startup.

## Remaining acceptance

The independently reproduced official-v2.6 forkchoice regressions from
[#27429](https://github.com/paradigmxyz/reth/pull/27429) remain a merge blocker.
This PR includes no Reth backport. Accepting an official release requires one
containing that fix plus validation of that exact candidate. Local normal-path
success cannot establish correctness of the pending-disk-reorg race.

The partial-consumption crash must also be diagnosed without weakening parent/
commitment checks, and its controlled recovery regression must pass. The full
release-artifact CI matrix, public Gnosis/Chiado operation, historical database
migration and rollback were not established by this fresh-network validation.
