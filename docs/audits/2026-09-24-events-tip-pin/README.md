# feature/events tip pin and PR #34 integration

| Field | Value |
| --- | --- |
| Date and status | 2026-09-24 UTC; local source validation passed with noted exclusions, not deployed |
| Scope | Pin the latest `feature/events` protocol revision in PR #34 and adapt fork gas pricing, genesis and validation identity |
| Source | `eduadiez/rollupX` PR #34 checkout, baseline `112410002d2b800467a3ea4b07dabfc25e6d2fdb` plus working patch SHA-256 `a53bd8ef43c42748fcc65051340445e12096ae8a6760bb546ba7e0d37f46a3ad` (`git diff --binary` before adding this audit) |
| Protocol | `820e50f749f63165be6a5797d4c5fe52cbbbbe34` → `9d13218a355e7d543ca8a9927eea2962600b716d`; `feature/events` tip confirmed by `git ls-remote` on 2026-09-24 |
| Toolchain | Rust 1.94.0 and Foundry 1.7.1 in `eez-staged-tools:1.94.0-bc`; solc 0.8.34; offline compiler/build inputs |
| Local source-built binaries | `eez-composer` SHA-256 `9acdab1acc6030d0efccb27a8e0eb5a54e4c680591e3918d116d74bd2a59f7e2`; `eez-proof-signer` `4c39f23866a2194d81be5a5b5c2c63f7f2c83732b720ba3267c667fcfd0462c2`; `eez-follower` `d205a791927a41fba1e1a340ce14dca8cb904915e0c0a7cbc4ce5df9a2875dd1` (local only) |
| Related work | [Accepted static-position port](../../proposals/l2-static-execution-position.md), [old-pin gas reserve evidence](../2026-09-16-chiado-result-reserves/README.md), [PR #34 CI repair](../2026-09-24-pr34-ci-repair/README.md) |

## Findings and adaptation

The protocol adds four commits after the old pin. L1 `EEZ` now stores deferred
entries in mappings with active length counters. Verification resets those
counters but retains inactive payload slots for reuse. Both L1 and L2 static
lookups try later candidates when a callback rolling hash differs. External
entry ABI and block-hash commitments remain unchanged. The upstream
`STATIC_ENTRY.md` still describes `RollingHashMismatch` on the first callback
hash mismatch, contrary to the pinned implementation and tests. This fork
follows the implementation and records the specification discrepancy in the
accepted position proposal; it does not extend the signer's narrower profile.

The old Composer cleanup reserve and storage probe assumed dynamic queue
arrays. At the new pin, those offsets read unrelated mapping data. A focused
integration test initially failed 11 of 21 cases on that layout boundary. The
Composer now probes each potentially discounted mapping slot at the canonical
L1 parent. A slot receives the measured deferred return-size reserve only if
its retained payload is empty or has one root update, no callback/reentrant
rows, and at most 64 result bytes. Otherwise it keeps the 370,000 gas entry
reserve. The active queue length is never used as evidence that an inactive
slot is empty. Accepted deferred groups advance the same index used by final
batch sizing. The old per-entry cleanup term is zero: queue replacement only
resets active bounds. Final ordered execution and preflight still decide
whether the actual batch fits.

The EEZL2 runtime and canonical proxy creation bytecode changed. The generated
fixtures now have genesis state root
`0x465f8a5fe77e19d7e12db1c29ca2b983f62b4e63a61386d8a8514fcd1440c84f`.
The runtime code hash is
`0x1f9014f8b29402628f97a671a79f5ae830080a1589a12b19fc5359db0b7844b1`.
The proxy init-code hash vector is
`0x62aa17a3312ccb683291e34b3fc0969177ecf64c0fd7fb321892e8655aa25d3c`,
giving address `0xa23698cefade17c7a50c1a7e0e0eb54ae6e5aa12` for the fixed
vector. Generator and Kurtosis deployment verifier use the same values. The
Composer/Signer validation identity binds the new protocol revision, so cached
facts from old peers do not share a namespace.

The operator supplied upstream microbenchmarks of 48,173 gas for an empty
post and 33,078 gas for the first no-call entry. They are useful for breaking
down contract work but do not include this fork's proof, leading anchor and
specific result storage. In the local Foundry full-batch fixture, the leading
anchor used 90,154 caller-frame gas; fresh 31-byte deferred entries grew from
325,470 gas for one to 24,558,171 gas for 127, and a later empty queue reset
used 115,913 gas regardless of that length. A direct overwrite regression
measured 45 short-result retained slots at 1,978,241 gas on the second wave,
and 127 64-byte retained slots at 5,979,253 gas. These are local Foundry
measurements, not Chiado receipt gas or a universal discount factor.

## Executed checks

| Check | Type | Result |
| --- | --- | --- |
| `forge build --root eez-core-protocol` and `forge build --root contracts` with pinned solc | Static build | Passed |
| Pinned core tests with required `script/*` and two named test exclusions | Solidity unit/integration | Passed; excluded tests are not counted as coverage |
| `bash scripts/ci/run-protocol-anvil.sh` with isolated port range | Local process | 36/36 passed, including new static local-write paths |
| `forge test --root contracts` | Solidity integration | Passed with the added overwrite case; gas/layout suite 22/22 passed |
| `forge test --root contracts --match-path test/PostBatchGasPins.t.sol` | Solidity gas/layout | Baseline 10 passed/11 failed at new pin before adaptation; after layout adaptation and overwrite regression 22/22 passed |
| `bash scripts/update-eezl2-genesis.sh` followed by `--check` | Generated fixtures | Passed; state root above |
| `cargo test --locked -p eez-composer --lib composer::gas::tests` | Rust unit | 11/11 passed after the final regression refinement |
| Rust Composer, control-RPC, protocol, blob and payload package tests | Rust unit | Passed; final Composer library suite had 233 passed and 1 ignored |
| Rust proof-signer and prover-client package tests | Rust unit | Signer 329 passed, 2 ignored; client 30 passed before final Composer edit |
| `cross_chain::outbound_changing_static_reads_and_write_settle_and_replay` | Source-built process and independent follower replay | Passed at new protocol pin before final Composer pricing edit |
| `dynamic_admission::gnosis_short_result_forty_five_request_waves_replace_busy_queue_and_replay` | Source-built process and independent follower replay | Passed twice at new pin: first with conservative pricing, then with final mapping-aware estimator; 45 sources in each wave |
| `settlement_policy::interval_preserves_execution_but_waits_below_all_thresholds` | Source-built process | Passed at new pin with Composer readiness signal fix; the earlier GitHub CI run failed before this signal was added |
| `cargo +1.94.0 fmt --all --check`, `clippy --locked --workspace --all-targets -- -D warnings`, `build --locked --workspace` | Rust static/build | Passed in the offline Rust 1.94.0 tool image |
| `python3 -m unittest discover -s scripts/ci/tests -v` | Mocked/tooling | 253 passed, 4 intentionally skipped, with Foundry `cast` on PATH; without `cast`, three unrelated public-network mock tests failed |
| `forge fmt --check --root contracts` | Solidity formatting | Reports two pre-existing formatting differences in untouched `ECDSAProofSystem.sol` and `DeployRollup.s.sol`; edited `PostBatchGasPins.t.sol` passes targeted format check |
| GitHub Actions run `35994614812` Kurtosis lane | CI at earlier PR commit | Passed; overall run failed in embedded slice 1/4 due to Composer readiness race addressed by separate local patch |

No new live Chiado deployment or wallet operation was performed. Existing
`820e50f` Chiado contracts and genesis remain tied to that deployed revision.
The new source pin needs a separate matching deployment; replacing binaries
on that existing network is unsupported.

## Open work

Record the final PR commit identity after pushing. The upstream
static-entry specification discrepancy needs an upstream correction. This
local-only audit and ignored `artifacts/native-e2e` logs are not independently
retrievable from another checkout until the branch is pushed and CI publishes
new artifacts.
