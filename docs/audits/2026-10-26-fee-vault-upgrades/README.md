# Upgradeable FeeVault and explorer fee labels

| Field | Value |
| --- | --- |
| Date/status | 2026-10-26 UTC; local validation passed, no deployment |
| Scope | Transparent treasury proxy, two-step recipient rotation, payment accounting/threshold/events, genesis and explorer configuration |
| Source | `641e768f2e6605f7563c808a00937d374e98e8fb` plus this branch's implementation; `codex/fee-vault-upgradeable` |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged and clean |
| Environment | Rust 1.95.0, Foundry 1.7.1, Solidity 0.8.34, optimizer runs 1, via IR |
| Contract | [Accepted proposal](../../proposals/fee-vault-upgrades.md), [operation and upgrade procedure](../../runbooks/fee-vault.md) |

## Findings and coverage

The implementation reuses pinned OpenZeppelin transparent proxy and ProxyAdmin
contracts. The implementation disables initialization; proxy construction
initializes the treasury atomically. Current and pending owners/recipients,
threshold, counter and balance survive the tested compatible upgrade. A failed
migration restores the previous pointer and state. Only the upgrade administrator
can replace the implementation. Treasury ownership transfer does not transfer
the independent ProxyAdmin authority.

Governance and payment events are emitted from the proxy address. Recipient
contracts in the unit regressions accept the proposal through actual calls;
rejecting/reentrant payment paths preserve the required funds and counter rules.
Protocol fee credits remain direct balance updates and do not emit contract logs.

| Requirement | Check | Result |
| --- | --- | --- |
| F1–F5 | `forge test --root contracts --match-contract FeeVaultTest -vv` | 18 passed; initialized/locked implementation, upgrade authorization and storage preservation, failed-migration rollback, governance/payment events, recipient acceptance/replacement/cancellation, threshold boundary and withdrawal rollback/reentry |
| Existing integration contracts | `forge test --root contracts` | 148 passed, none skipped, before the additional event test and receiver helper refinements; final focused treasury suite passed afterwards |
| F6 | `bash scripts/update-eezl2-genesis.sh`, then `--check` | Passed; exact code/immutables, slots, nonces, account set and computed state root. Only the three FeeVault accounts changed in each genesis allocation; other genesis fields and protocol runtimes were unchanged |
| F6, custom governance | Renderer `--render` with public owner `0x3333…3333` and recipient `0x2222…2222` | Passed; exact vault and ProxyAdmin ownership, active recipient, recomputed profile root and ETH-free production allocations |
| Deployment wiring | `python3 -m unittest discover -s scripts/ci/tests -p 'test_deploy_preflight.py' -v` | 15 passed; mocked preflight, not broadcast evidence |
| F8 | Compose `config --format json` with public fixture environment and no operator env file | Both hidden-field arrays reach `blockscout-web`; no container restart or browser verification |
| Formatting/docs | Cargo fmt, Forge fmt, shell syntax, `git diff --check`, local link targets | Passed |
| F7 | Source-built `native_system::treasury::` process test | 1 passed, 186 filtered, none ignored; signed proposal/acceptance, implementation deployment, upgrade, withdrawal, offline-proven settlement and cold follower replay; 354.45 seconds including fixture/runtime preparation |
| Rust lint | `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` | Passed |
| Shared protocol/codec regressions | `cargo +1.95.0 test --locked -p eez-protocol -p eez-blob-codec -p eez-payload-codec` | 263 passed, none failed or ignored; includes the independent public-input, rolling-hash and Ethereum KZG vectors |

The preliminary genesis check used the published helper whose source algorithm
and lockfile are unchanged. The helper was then built from this checkout; all
three generated genesis roots matched and `--check` passed with this newly built
helper. The helper identities and roots are recorded in
[validation.json](validation.json). These local checks do not qualify a release
artifact for deployment.

The final diff review traced initialization, independent upgrade ownership,
recipient acceptance, payment rollback, storage preservation, generated state and
the full follower/prover execution path. No additional correctness issue was
identified. The core protocol pin, Cargo dependencies, fee-credit execution rule
and proving wire were not changed. This is a feature addition; there is no
baseline bug reproduction claimed for the new upgradeability behavior.

## Reproduction and retained evidence

The isolated worktree is `artifacts/fee-vault-upgradeable-20261026` under the
repository. Local logs/datadirs live under `artifacts/fee-vault-validation/` within
that worktree. They are ignored, local-only evidence, not available in a fresh
checkout; small result summaries and SHA-256 digests are in
[validation.json](validation.json).

The source process check uses the test package's Composer/follower wrappers,
compiled against the current node libraries, and its locally built signer. Their
executable digests are recorded; these debug/test binaries are not promoted release
artifacts. For reproduction, unset `EEZ_TEST_PREBUILT`,
`EEZ_TEST_RUNTIME_BIN_DIR` and `EEZ_TEST_PROOF_SIGNER_BIN`; build integration
contracts, export `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`, prepend its `bin`, and
set `TMPDIR` to an owned directory. The executed command is:

```bash
CARGO_BUILD_JOBS=1 \
EEZ_TEST_LOG_DIR="$PWD/artifacts/fee-vault-validation/logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/fee-vault-validation/datadirs" \
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  native_system::treasury:: --test-threads=1 --nocapture
```

The first compile attempt failed in `protoc` because `/tmp` exceeded its quota;
no process test ran. A second compile was terminated with SIGKILL in `eez-node`.
Both diagnostics were retained. The completed attempt used owned temporary
storage and one Cargo build job. It passed on its first executed assertion run;
the previous build failures are not assertion-level flaky passes.

## Operations and remaining limits

No deployed node, signer, explorer, wallet or database was changed. The user
confirmed a new-network deployment: the proxy is installed in genesis with no
legacy-vault migration or compatibility path. Future storage-compatible proxy
upgrades can be ordinary L2 calls.

Full final-head CI and release-artifact verification remain required before
deployment. Browser rendering, live proxy source verification, a live upgrade,
and native Blockscout fee-income history are not covered. Full workspace tests,
the core protocol's complete test suite and live Kurtosis/Chiado suites were not
run for this amendment; the pin and execution rules are unchanged, and focused
contract/protocol and source-process checks cover the changed path. Hidden frontend fields
leave generic backend `burnt_fees` responses unchanged. A configured upstream
address widget can link to a future accounting dashboard without a frontend fork;
that service and historical indexing are outside this implementation.
