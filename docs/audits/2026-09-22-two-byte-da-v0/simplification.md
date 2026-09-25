# Two-byte V0 simplification validation

Status: the byte-string revision passed scoped validation, including all nine
selected process cases on their first attempt. A subsequent size-only follow-up
is recorded [below](#allocation-free-sizing-follow-up); earlier executable hashes
and process results apply to the preceding revision. This record does not erase
[earlier failures and evidence](README.md).

## Scope and identity

Same branch, PR #34 base and unchanged protocol pin as the parent record:
`feat/two-byte-da-v0`, base `68fde5bb5b0f9743e55a5de98cdff682e9076a49`,
protocol `3c0cdf4955941a5c23639346932b55b33babebbd`. Changes are uncommitted.
Rust 1.94.0, Foundry 1.7.1, locked/offline Cargo builds.

User-approved refinements:

- Terminal and entry buffers use RLP byte strings, preserving all contents,
  ordering, group sizes and execution/authentication rules. Old integer-list
  wrappers are rejected; only new networks using this format from genesis are
  supported. No historical decoder or migration is added.
- Remove unused record counters and first-derived empty-state bookkeeping.
- Reuse measured column/entry lengths during encoding.
- Validate full-block bodies once in `Payload::new`; compare its canonical count
  column to the decoded count column before returning a payload. Coverage and
  canonical-token checks still precede allocation. This does not authorize signing.
- Update producer admission, retained-history indexing and codec sizing together.
  Preserve the maximum of full-block and derived-transaction byte-string costs:
  many legacy transactions can make the latter larger.
- Strengthen transaction-limit tests to extend with valid signed transaction
  bytes, check the ceiling and assert the specific overflow error.

## Checks and local evidence

Artifacts are local-only under `artifacts/two-byte-da-v0-simplified/`; ignored
files are not available to another checkout. Working directory and shared Cargo
target are the same as the parent record.

| SHA-256 identity | Value |
| --- | --- |
| Changed Rust source manifest (`rust-source.sha256`) | `da80aed5ed537324bcef88fec4c0702d8d96b8e987b275beb5a78ad278a12156` |
| `codec-final.log` | `4555c4abc57263e62ef3377b1e38ba9249eda91426673151cd929e4f7cc26736` |
| `components.log` | `66807d3356cd59098875e16a57dae35d6d8c8254c303e5341cf8aa5279c118df` |
| `clippy-final.log` | `9131baebd99452f0f2c85df8feaecec9438f1f2c63bc6e0862ee22fd19ddba48` |
| `process.log` | `44208fa519b47b5600a32cc5304772b05e6f2d8a3d5a1ddfa2fa3f560efca893` |
| Six process executables plus composer, follower and signer manifest (`process-executables-final.sha256`) | `4d8a34ec1518d49e37771eb321f081aec410aaee8f4521aed99fd11147ef477d` |

The source manifest includes the final test-only clone correction and an API-doc
clarification made after component tests; neither changes component/runtime logic.

| Check | Current result | Log |
| --- | --- | --- |
| `cargo +1.94.0 test --locked --offline -p eez-payload-codec` | 43 passed | `codec-final.log` |
| Eight affected component packages, command below | 832 passed, 3 manual ignored | `components.log` |
| Workspace all-target clippy with `-D warnings` | Passed after test-only lint fix | `clippy-final.log` |
| `cargo +1.94.0 fmt --all --check`; `git diff --check` | Passed | Console |
| Source-based focused process checks | 9 passed, no failures/retries | `process.log` |
| Integration contract build | Passed | `contracts.log` |

```sh
export CARGO_TARGET_DIR=/home/edu/Development/eez-rollup0-fork/target
cargo +1.94.0 test --locked --offline \
  -p eez-driver -p eez-composer -p eez-proof-signer -p eez-prover-client \
  -p eez-control-rpc -p eez-protocol -p eez-blob-codec -p eez-node-common
cargo +1.94.0 clippy --locked --offline --workspace --all-targets -- -D warnings
```

The nine selected process tests cover Ethereum/Gnosis independent blob replay,
bidirectional cross-chain execution, whole-group blob cuts, ordinary admission
capacity, and Ethereum/Gnosis fee/gas cases. They run serially with fresh
source-built binaries, no prebuilt overrides, and retained logs/datadirs:

```sh
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
forge build --root contracts
export EEZ_TEST_LOG_DIR="$PWD/artifacts/two-byte-da-v0-simplified/process-logs"
export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/two-byte-da-v0-simplified/process-datadirs"
cargo +1.94.0 test --locked --offline --no-fail-fast -p eez-node-e2e \
  --test derivable_operations --test cross_chain --test dynamic_admission \
  --test settlement_policy --test settlement_policy_cost --test settlement_policy_gas \
  -- native_v3_post_replays_from_blobs minimal_bidirectional_cross_chain_smoke \
  automatic_blob_cut_keeps_complete_nonce_ordered_groups \
  ordinary_data_admission_tracks_operator_capacity \
  receipts_use_bounded_fee_history_priority low_byte_expensive_new_proxies \
  --test-threads=1
```

| Process target | Selected result |
| --- | --- |
| `cross_chain` | 1 passed, 21 filtered |
| `derivable_operations` | 2 passed, 4 filtered |
| `dynamic_admission` | 1 passed, 15 filtered |
| `settlement_policy` | 1 passed, 30 filtered |
| `settlement_policy_cost` | 2 passed |
| `settlement_policy_gas` | 2 passed, 2 filtered |

The selected replay cases reached their semantic assertions and independent
blob/follower reconstruction. Composer/follower paths had no Nextest/Cargo
binary override; the harness built the signer at this revision. Process sources
were unchanged throughout the run. These source-based checks do not establish
the release-artifact production-feature matrix.

### Regressions and first failures

The retained `codec-first-failure.log` reproduced an invalid count-binding fixture:
its full exception directly preceded a terminal with an inconsistent parent hash.
Moving count comparison after shared full-block validation exposed that separate
placement error first. The fixture now leaves a derived position between them and
checks the precise count mismatch for empty, one-transaction and 128-transaction
full blocks, plus successful byte-identical roundtrips for their correct counts.
No production placement check was weakened. A test-only Alloy transaction type
mismatch was corrected during compilation.

The first clippy run (`clippy.log`) caught an unnecessary clone in that fixture;
it now uses `std::slice::from_ref`. Additional independent wire vectors cover raw
zero/high bytes, empty and 56-byte entries, unchanged group encoding and explicit
rejection of integer-list wrappers. A many-legacy-transaction regression proves
that both admission-size helpers cover the larger flat transaction column.

## Limits

The complete 78-case process selection was not repeated for this refinement;
the earlier 75 first-attempt passes and three startup flakes do not validate this
revision. Three manual component tests remain excluded. Full release-artifact CI,
Kurtosis, ignored long-history process cases and live deployment remain unverified.
The residual subprocess-port race is not fixed here. No services, wallets,
deployments, protocol pins, commits, pushes or PRs were changed.

## Allocation-free sizing follow-up

The second user-approved review follow-up replaces transaction serialization in
`BlockDaSizer::retained_item_bytes` with `encode_2718_len()` and checked RLP-prefix
arithmetic. Signed transactions are always longer than one byte, so the string
prefix is unconditional. The conservative maximum of full-block and flattened
transaction sizes is unchanged. No wire or execution behavior changes.

The sizing test generator now selects full/derived representation independently
from transaction count, including 127/128-transaction cases. Randomized ranges
check associativity, exact encoded size, reserve bounds and decode roundtrips;
deterministic cases also cover terminal promotion and the 16,383 ceiling for
nonempty full exceptions. The test helper no longer needs to mutate/re-encode
individual blocks to construct those cases.

The existing typed/legacy input-size matrix now checks the optimized retained
estimate against actual serialization. Its first run exposed a previously
encode-only Shanghai fixture with a withdrawals root but no base-fee field;
the decoder interpreted the root as a fee and rejected it. The fixture now
includes the required preceding field. No production decoder check was changed.
The initial failure is retained in `artifacts/two-byte-da-v0-lengths/first-failure.log`.

Current follow-up checks (local-only logs under `artifacts/two-byte-da-v0-lengths/`):

Changed Rust source manifest SHA-256:
`3612ad33a3b2bd619f22fd8112c3c72a72e0feacf6721e4a6a95457f92db3110`
(`rust-source.sha256`). Codec log SHA-256:
`5546b5b3aa16a3fff9c461a22fe05c061e869f40300da96c29169a95e332b2ea`.

- Codec: 43 passed (`codec.log`), including the expanded 256-case property test.
- Codec all-target clippy with `-D warnings`: passed (`codec-clippy.log`).
- Composer unit tests: 221 passed, one manual test ignored (`composer.log`).
- Workspace all-target clippy with `-D warnings`: passed (`clippy.log`).
- Formatting and diff whitespace checks: passed.

Process suites and the release/Kurtosis matrix are not repeated for this
arithmetic-only refinement; equivalence is checked against real serialization
across typed/legacy encodings and RLP size boundaries. Previous process results
remain evidence for the previous source fingerprint, not newly executed tests.
