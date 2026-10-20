# 64 KiB source calldata validation

| Field | Value |
| --- | --- |
| Date | 2026-10-20 UTC |
| Status | Local implementation and validation complete; no live deployment. |
| Scope | Raise the application calldata profile to 65,536 decoded bytes, including ABI encoding, for compact/debug quotes, signed tariff admission and the SDK. |
| Base | `eduadiez/rollupX` main `b889326f03d0894dc45eb3fc1c9211895c14a9e9`, verified with `git ls-remote` when this branch was created. |
| Candidate | Branch `feat/64k-calldata`; implementation and regressions accompany this record. |
| Protocol | `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774`; initialized and unchanged. |
| Environment | Rust 1.95.0, Foundry 1.7.1, Node 26.7.0, LLVM 22; local process fixtures, not a public-network deployment. |
| Contract | [Application limits and compatibility](../../configurable-settlement-policy.md#calldata-limit-compatibility). |

The binaries were compiled on the base commit with the uncommitted implementation.
The runtime, SDK and test-source patch has SHA-256
`5c620f43545ead3f7cadd6565799c8189042bb23b141712db001efbb6db8d496`,
computed with `git diff --cached --binary -- . ':(exclude)docs/**'`.
Those source files were unchanged throughout the post-change validation.
Documentation-only edits after compilation do not change the captured binaries.
After committing, the same patch is the binary diff from the base to this
record's commit, excluding `docs/**`.

## Change and boundaries

The shared `L2FeeRequest` parser accepts at most 64 KiB. Compact/debug quotes
and signed tariff admission use that parser, for both transaction types and
source directions. The quote front accepts at most 160 KiB of serialized
transaction/options JSON, accommodating hex encoding and fields. The SDK applies
the same decoded calldata limit before quoting and preparing wallet fields.

The pinned [blob format](../../../eez-core-protocol/docs/blobs/BLOB_FORMAT_SPEC.md)
already carries `u32`-bounded byte lengths and permits messages to span blobs.
No codec, protocol, signer profile, genesis or database change is needed.
Existing gas, DA, frame, nesting, deadline and worker limits still apply.
The fixture commits `keccak256(msg.data)` on the destination chain, so successful
execution alone cannot mask payload truncation.

## Baseline reproduction

Before changing the limits, the new Composer boundary regression selected one
test and failed with `Invalid("calldata must be hex and at most 12 KiB")`.
The SDK suite selected seven tests: six passed and the new boundary test failed
with `Invalid calldata`. Both regressions include the original 12,288-byte
boundary, a 13,000-byte ordinary payload, and 65,535/65,536/65,537 bytes.

An initial SDK invocation using `node --test --test-name-pattern` failed at the
test-file runner without a useful assertion diagnostic. The repository's
documented `node sdk/l2-fees.test.mjs` command produced the baseline assertion
above. The runner failure is retained separately and is not boundary evidence.

## Reproduction and checks

Working directory: `artifacts/calldata-64k-pr-20261020`, an isolated worktree
inside the original checkout. The original checkout's uncommitted work and
operator settings were preserved. Commands ran from the worktree root.

Rust commands used:

```bash
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
export PATH="/usr/lib/llvm-22/bin:$PATH"
export CARGO_TARGET_DIR=/home/edu/Development/rollupX/target
export CARGO_BUILD_JOBS=2
```

| Check | Result |
| --- | --- |
| `cargo +1.95.0 test --locked --offline -p eez-composer --lib quote_and_signed_admission_enforce_64_kib_calldata_boundary` | One selected test passed after the change; both quote parsing and signed admission preserve accepted bytes and reject the first oversized length. |
| `node sdk/l2-fees.test.mjs` | Seven passed, none skipped; oversized input fails before RPC and wallet field preparation. |
| `forge test --root contracts` | 89 passed, none skipped. |
| `cargo +1.95.0 test --locked --offline -p eez-composer -p eez-node -p eez-l1 --lib` | 389 Composer, 193 L1 and 93 node tests passed. The Composer's manual `profile_blocked_backlog_discovery` timing profile was ignored by design; it is not coverage. Node checks include both HTTP quote methods at 65,535/65,536/65,537 bytes and serialized JSON at 163,839/163,840/163,841 bytes. |
| `fee_quotes::calldata::` process cases | Both legacy and EIP-1559 cases passed in the isolated full-suite invocation, including semantic destination assertions, signed rejection and independent replay. |
| Full `fee_quotes::` source-based process suite | 20 passed, zero failed or ignored, 144 other tests filtered out; 991.56 seconds. Includes the two new boundary cases and three existing Safe regressions, including Gnosis. |
| `cargo +1.95.0 clippy --locked --offline --workspace --all-targets -- -D warnings` | Passed. |
| `cargo +1.95.0 fmt --all --check`; `forge fmt --check --root contracts`; `git diff --check` | Passed. Local documentation link targets resolve; added shell examples pass `bash -n`. |

The initial source-build/process command was:

```bash
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
forge build --root contracts
EEZ_TEST_LOG_DIR="$PWD/artifacts/calldata-validation/e2e-logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/calldata-validation/e2e-datadirs" \
cargo +1.95.0 test --locked --offline -p eez-node-e2e --test node_e2e -- \
  fee_quotes:: --test-threads=1
```

While this compiled, another build in the original checkout targeted the same
shared Cargo runtime paths. The newly built Composer and follower were copied
into this worktree's `artifacts/calldata-validation/runtime/`, with SHA-256
checks. The newly compiled test runner was captured and its initial invocation
stopped before validation; Cargo's resulting signal exit is intentional, not
test coverage or a flaky assertion. No fixture child process had started.
The other checkout's build and services were left running.

The signer was built from this worktree with
`cargo +1.95.0 build --locked --offline -p eez-proof-signer --bin eez-proof-signer --message-format=json`;
its compiler-artifact event identifies the worktree manifest and executable
copied into the same isolated directory. This avoids using any runtime later
replaced in the shared target directory. These are locally source-built debug
executables, not published CI artifacts. The isolated invocation is:

```bash
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
export EEZ_TEST_RUNTIME_BIN_DIR="$PWD/artifacts/calldata-validation/runtime"
export EEZ_TEST_PROOF_SIGNER_BIN="$EEZ_TEST_RUNTIME_BIN_DIR/eez-proof-signer"
export EEZ_TEST_LOG_DIR="$PWD/artifacts/calldata-validation/isolated-e2e-logs"
export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/calldata-validation/isolated-e2e-datadirs"
"$EEZ_TEST_RUNTIME_BIN_DIR/node_e2e" fee_quotes:: --test-threads=1
```

`--list` selects 20 process tests, including the two new boundary cases and
three existing `safe_fee_quotes::` cases matched by the same filter.

The new process cases use the Ethereum Osaka fixture with tariff admission
enabled and real local Composer, proof-signer and follower processes. They
check compact and debug quotes, reject 65,537 bytes without queue ownership or
nonce consumption, then settle 13,000, 65,535 and 65,536 bytes. Assertions require
the complete destination calldata hash, a canonical successful posting receipt
with the quoted blob count, and independent follower replay with byte-identical
source input. The two near-limit payloads require at least two blobs.

Logs and failing datadirs are retained under the worktree's ignored
`artifacts/calldata-validation/`. These are **local-only**, unavailable from
another checkout. Successful datadirs follow the harness's normal cleanup.
The captured executable identities are:

| Executable | SHA-256 |
| --- | --- |
| Composer | `621c252f5e88c51bd0e0f55bc00699962e90fe58bf7226699ea1191db1739c88` |
| Follower | `3c45372550760eee52398541b55f78a9118e15414a41cb62e27ace1162966476` |
| Proof signer | `eb58c024aa60936e3a54656b74a4fff35232ac2d720c99e1f4aab243f957f94e` |
| Process test runner | `46bc33fbfedf6b4b6922e59ceb1e030ae99f24a0914e33f0fa834df6ddd25bc3` |

All four hashes matched again after the successful process run.
The local-only `isolated-fee-quotes-e2e.log` has SHA-256
`3e31d4ad098c8855ff02805caa8a97eb85fc7fa6fd4d9f10dcab6c4c27b0c85c`.
No assertion retries were needed in the isolated run.

No live deployment, production-load benchmark, application-specific zk-proof
verification, or durable queue recovery is established by these local checks.
The CI release-artifact/Kurtosis matrix and real-builder refund exercise were
not run. This change does not modify L1 collection or bundle ordering; the
local refund regressions do not establish ordered builder payments.
