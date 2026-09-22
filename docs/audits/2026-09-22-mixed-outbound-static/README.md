# Top-level read-after-write investigation

Date: 2026-09-22. Source baseline: `339bfd578eb9352f0dfbdd604740bb6a1a347e12`.
Pinned core: `3c0cdf4955941a5c23639346932b55b33babebbd`, initialized recursively,
with no protocol source edits. Investigation branch: `fix/mixed-static-outbound`.

## Reproduction

[TopLevelStaticEpoch.t.sol](../../../contracts/test/TopLevelStaticEpoch.t.sol)
loads one successful mutable prediction and two static predictions with identical
call identities but values 10 and 9. It invokes the actual proxy using STATICCALL,
CALL and STATICCALL. Both reads return 10, despite successful mutable consumption
between them. This is a characterization of the current limitation, not a passing
support regression. The desired post-write value is 9.

The mutable return is a cached successful prediction, matching the source-side
protocol mechanism. No L1 transfer is executed in this isolated test. It does not
claim to reproduce the user's full transaction or authorize the supplied table.
It demonstrates that the deployed protocol format cannot select the second
prediction for this top-level read sequence.

Command, from the fork checkout:

```bash
docker run --rm --user "$(id -u):$(id -g)" -e HOME=/tmp \
  -v "$PWD:/work" -w /work --entrypoint forge \
  ghcr.io/foundry-rs/foundry:v1.7.1 \
  test --root contracts --match-contract TopLevelStaticEpochTest -vv
```

Result: **1 passed, 0 failed, 0 skipped**. Solidity 0.8.34, optimizer runs 1,
via IR. Forge 1.7.1 commit `4072e48705af9d93e3c0f6e29e93b5e9a40caed8`.
Compiler emitted existing proxy fallback/no-receive and unchecked low-level-call
warnings. No network services, wallet balances or deployed contracts were changed.

## Contract and specification evidence

- `eez-core-protocol/src/L2/EEZL2.sol`, `staticCrossChainCall`: the top-level branch
  scans `staticEntries` from zero and returns the first `proxyEntryHash` match.
  The nested branch instead includes the active rolling hash in the lookup key.
- `eez-core-protocol/docs/STATIC_ENTRY.md` section 4.2 and core spec F/G.2:
  L2 top-level lookup is hash-only and same-block bounded.
- Core spec E.4's multiple-read disambiguation describes reads during an active
  execution, not the top-level static pool.
- Fork signer `settlement/outbound.rs` independently rejects mixed source kinds;
  extending that gate alone cannot repair the contract lookup behavior.

## Limits and next work

The sender's complete calldata was not available in the supplied message. This
is a minimal contract reproduction of the lookup limitation, not evidence of every
failure the original transaction might encounter. No Rust, process, full protocol,
or live semantic regression has run. No fix has been implemented or deployed.
See the [draft design](../../proposals/mixed-outbound-static.md) for acceptance,
versioning and migration requirements. A protocol change is necessary for general
top-level read-after-write support; it must not be represented as a signer-only fix.

## Signer-only extension (subsequently authorized)

The user narrowed the requested fix to supporting mixed groups that the existing
protocol can represent. This supersedes the earlier statement that no signer
implementation existed; the protocol limitation characterization above remains
unchanged.

Implementation: a shared source-group verifier walks host calls in their original
order while separately consuming mutable and static source rows. It recomputes
the host rolling hash, verifies every source, checks mutable receipt events and
value, and rejects conflicting results for the same static key. Mixed multi-root
hosts must cover the complete load. Their mutable roots are successful and flat;
reverted/nested mixed groups and separate callbacks remain unsupported. The
previous mutable-only nested/revert path is preserved.

No contract runtime, ABI, codec, RPC schema or protocol pin changed. Solidity
changes in `contracts/src` are test fixtures only. The live network was not
modified. Queue scheduling and retry ownership are unchanged.

### Baseline comparison

With only the new test fixture added to source baseline `339bfd5`, running
`cargo test --locked -p eez-proof-signer
mixed_outbound_authorizes_reads_before_and_after_write` selected one test and
failed in outbound authorization with:

```text
ExecutionTableMismatch { entry_index: 1,
  reason: "mutable outbound group mixes static source entries" }
```

The same regression passed after applying the verifier/signer change. It covers
reads before and after writes, leading/trailing writes and multiple static-only
roots. Additional tests reject changed host outcomes, conflicting cached results,
wrong read identities, and missing/extra mutable events.

An initial `--lib` invocation selected no target because the signer is a binary
crate; that invocation is not test evidence. The corrected command above reached
the intended component and established the baseline failure.

### Checks so far

Using Cargo/Rust 1.94.0 in the local `eez-fee-recipient-tools:1.94.0` container,
with locked dependencies and six build jobs:

- Signer, control RPC, prover client, protocol, blob codec and payload codec:
  **586 passed**, including independent hash vectors and Ethereum KZG tests.
  Two signer utilities were intentionally ignored: native-fixture inventory and
  long-span cold-proving feasibility. Zero-test doc targets add no coverage.
- Foundry 1.7.1 integration contracts: **34 passed**, including the protocol
  limitation characterization. Existing compiler/lint warnings were reported.
- Further test strengthening and native-process/static checks are in progress;
  these initial totals do not substitute for final-revision checks.

Local logs (ignored, not available in another checkout):
`artifacts/mixed-validation/{baseline,candidate,contracts-build,contracts-tests,process}.log`.
The process runner retains diagnostics under `artifacts/native-e2e/`.

### First process run exposed a replay restriction

The first source-built process test selected one scenario and **failed** after
L1 execution because `eez-deriver` still rejected the mixed source table:

```text
outbound settlement/source tables are inconsistent:
unsupported protocol operation: mixed mutable/static roots in one outbound settlement host
```

The test's safe-chain reconciliation check prevented treating signer acceptance
or the L1 transaction as completion. Diagnostics are retained in local
`artifacts/mixed-validation/process.log` and
`artifacts/native-e2e/datadirs/eez-cross-chain-5836-gIEr22`, with node/signer logs
in `artifacts/native-e2e/logs/*5836*`. These ignored artifacts are local-only.

Correction: the deriver now invokes the same shared source-group verifier and
requires mixed multi-root hosts to cover one complete signed group. The process
test additionally starts a clean follower and compares its reconstructed safe
history and recorded read results. Its repeat run is after a code change, not a
retry-only pass or a claim that the first result was flaky.

Old replaying nodes cannot consume the newly accepted groups; operator/spec
compatibility guidance now requires updating Composer/followers before enabling
the signer. Existing protocol and DA encodings remain unchanged.

### Final package and static checks

The final package suites passed **635 tests** (the original six packages plus
48 deriver tests and the additional static-pool partition rejection). Composer's
separate test-util build passed **220 tests**, with its manual backlog timing
profile intentionally ignored. Together with the two signer utilities above,
three explicit utilities/profiles were excluded, not counted as coverage.

The following also passed on the final production-code changes:

```bash
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo clippy --locked -p eez-proof-signer --all-targets --all-features -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --locked -p eez-proof-signer --no-deps
cargo fmt --all --check
```

Tool image ID: `sha256:7335715efd8b672d267ecc2e8a9fc11c449e35413b1a98ee26ba06065f9aab86`.
The toolchain is Rust/Cargo 1.94.0. The successful log is
`artifacts/mixed-validation/final-checks-fixed.log` (local-only).
An earlier final-check attempt caught a test-only U256 literal compile error;
it was corrected before these suites ran. Its log is retained separately.

The final Solidity suite passed **34 tests**. Changed Solidity files pass
`forge fmt --check`. Repository-wide `forge fmt --check --root contracts` still
reports existing formatting differences in the untouched
`contracts/src/ECDSAProofSystem.sol` and `contracts/script/DeployRollup.s.sol`.
Those unrelated files were preserved. Logs:
`artifacts/mixed-validation/contracts-final.log` (format failure) and
`artifacts/mixed-validation/contracts-final-fixed.log` (successful tests).

No full protocol Anvil suite, complete native/Kurtosis matrix, live deployment,
or entire historical Chiado replay is claimed. The focused process acceptance
includes independent native-blob follower replay; final-source results follow.

### Final process acceptance

The final source and strengthened fixtures passed **2 process tests**, with
21 unrelated cases filtered out:

```bash
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
EEZ_TEST_LOG_DIR="$PWD/artifacts/native-e2e/logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/native-e2e/datadirs" \
cargo test --locked -p eez-node-e2e --test cross_chain outbound_m -- --test-threads=1
```

- `outbound_mixed_static_reads_and_write_settle_together`: two distinct reads,
  a real L1 state write, and both reads again; each read asserts its own expected
  result. The test checks L1 value, source call count, source read sum, exact safe
  inclusion, and a fresh follower's reconstructed block hash and read sum from
  canonical L1/native blob data.
- `outbound_multiple_proxy_calls_in_one_transaction_preserve_ordered_results`:
  existing mutable-only two-call compatibility and ordered return-value checks.

Result: **2 passed, 0 failed, 0 ignored**, 150.44 seconds. Log:
`artifacts/mixed-validation/process-final.log` (local-only). The prior successful
process repeat is retained in `process-replay.log`; the final run additionally
covers the strengthened full-load guard and per-read fixture assertions.

SHA-256 identities of the source-built executables used by the final run:

| Executable | SHA-256 |
| --- | --- |
| `target/debug/eez-composer` | `6f9cd8d1a31853051ad30bafa11a341762c1396e3400047b3361b91a00b562ea` |
| `target/debug/eez-follower` | `9c1a58a44d8d2d65a2285562704ef85ee87d78089a36f28ab7d7734dd7e26138` |
| `target/debug/eez-proof-signer` | `33a0e9c6d697a95cc502661c99a9b9a51f87a6ff2caf80a6e61911f0b878de05` |

These are local source-test binaries, not published release artifacts or a claim
of deployed image provenance. Implementation and focused validation are complete;
CI release promotion and live deployment have not run. Conflicting-result reads,
reverted/nested mixed roots, and unsupported-request scheduling remain outside
this fix.
