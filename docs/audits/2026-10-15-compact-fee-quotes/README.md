# Compact fee quote API validation

Date: 2026-10-15 UTC. Local validation completed; no deployment was performed.
This records implementation evidence, not an independent security review.

| Field | Value |
| --- | --- |
| Backend baseline | `4fc21ca100392daebd2852bedc345aa5fad46620` |
| Backend implementation | `149968a373696a5f01cfb1ef8d6aae176f122a42` |
| Initialized protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, unchanged |
| Dashboard baseline / implementation | `d2e85905864ec88de493829d763a8cfeb9a45b99` / `6cc89033064276843325af5607bb3bfd724cc44c` |
| Local branch in both repositories | `feat/compact-fee-quotes` |
| Toolchain | Rust 1.95.0, LLVM 22, Foundry 1.7.1; locked Cargo dependencies |

The accepted scope and API contract are in the [proposal](../../proposals/compact-fee-quotes.md)
and [settlement policy](../../configurable-settlement-policy.md#compact-fee-quotes).
The previous [transaction-reference validation](../2026-10-15-transaction-references/README.md)
does not establish validation or deployment of this API.

## Implementation and boundaries

`eez_quoteFee` and `eez_debugQuoteFee` replace the old RPC, HTTP quote route and
SDK compatibility helpers. Missing gas, nonce and chain ID are resolved inside
the service. Legacy and EIP-1559 variants are priced independently against the
same pinned context, with final gas/fee-sensitive native replay. The diagnostic
method adds calculation details to the identical compact result. Unknown or
changed context cannot return partially successful variants.

The SDK validates the intent binding, selected format, network, expiry, maximum
cost and wallet pending nonce before one normal wallet request. The reference
dashboard and known protocol/browser test drivers use the new methods. Wallet
quantity normalization remains for supported wallet observations; this does not
preserve any old quote API.

Measured settlement pricing, signed tariff admission and final dispatch funding
checks are preserved. Ordinary L2 execution receives no settlement surcharge.
L1-origin quotes explicitly report `unsupported_direction`; a builder-refund
recovery policy remains outside this implementation. No deployed contracts,
genesis, protocol pin, dependency versions, signer RPC or storage changed.
`FeeQuoteProbe.sol` is a local process-test fixture only.

This is a coordinated server/client API replacement. Old clients fail on the
new Composer and new clients fail on old servers. Upgrade or roll back both;
no data migration or historical follower replay migration is needed. See the
[operator runbook](../../runbooks/chiado-deployment.md).

## Reproduction and evidence

Work used `/tmp/rollupx-l2-fees-e2e.8oFEZzNX` and
`/tmp/eez-ui-l2-fees.Qm8H2JyN`. The operator checkouts' existing work and settings
were preserved. Raw logs, frozen executables and failure datadirs are local-only
under `artifacts/compact-fee-quotes-20261015/` in the operator backend checkout;
they are ignored and unavailable from another checkout. [results.json](results.json)
retains small sanitized results, executable hashes and log hashes.

The runtime was built and copied before building the process-test harness.
Its source was the code subsequently committed as the implementation above:

```bash
export CARGO_TARGET_DIR=/home/edu/Development/rollupX/target
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
export PATH=/usr/lib/llvm-22/bin:$PATH
export CARGO_BUILD_JOBS=2
cargo +1.95.0 build --locked --config 'profile.dev.package.eez-node.opt-level=0' \
  -p eez-node -p eez-follower -p eez-proof-signer --bins
cargo +1.95.0 test --locked --config 'profile.dev.package.eez-node.opt-level=0' \
  -p eez-node-e2e --test node_e2e --no-run --message-format=json
```

The command-line optimization override applies only to the local `eez-node`
development package. It limits compiler resource use and changes no dependency
or feature selection; these are not shipping release/Docker binaries. The test
runner used `EEZ_TEST_PREBUILT=1`, the frozen `runtime-final` directory for
`EEZ_TEST_RUNTIME_BIN_DIR`, its signer for `EEZ_TEST_PROOF_SIGNER_BIN`, and
separate retained log/datadir directories. Thus it could not select stale host
executables. Tests ran serially on disposable local Anvil chains.

The first new ordinary-call process test failed because the code checked for
empty actions after protocol finalization, which already rejects empty calls.
Both gas discovery and pricing now inspect the recorded call count before
finalizing and replay ordinary execution natively. The identical test passed
after rebuilding the runtime and harness. The first failure, datadir and
executable identities are retained; this is a code correction, not a retry-only
pass. Initial node unit socket failures under the filesystem/network sandbox
passed once the tests had permission to create local sockets.

All **12 selected process tests passed**: the new ordinary/boundary regression,
eight nested/Safe fee scenarios, and three wallet estimator regressions. No
selected process tests were skipped or ignored. The latter cover both chain
profiles and the post-batch wrapper. Native selectors were:

```bash
artifact_dir=/home/edu/Development/rollupX/artifacts/compact-fee-quotes-20261015
"$artifact_dir/node_e2e-final" fee_quotes::compact_ordinary_calls_boundaries_and_fee_sensitive_rejection --exact --nocapture --test-threads=1
"$artifact_dir/node_e2e-final" fee_quotes:: --skip compact_ordinary_calls_boundaries_and_fee_sensitive_rejection --test-threads=1
"$artifact_dir/node_e2e-final" cross_chain::wallet_gas_estimates_ --test-threads=1
```

The executables in these commands reside in the local artifact directory; the
working directory was the backend clone and the environment was as above.
Composer units passed **351**, with one existing manual backlog timing test
ignored; node units passed **86**. Contract tests passed **86**. The SDK passed
**5**, dashboard payment checks **4**, discovery **27** and debugger **78**.
Mocked browser suites passed **16 fee scenarios** and **27 bridge scenarios**.
The dashboard production build passed with its existing large-chunk warning.
Workspace Clippy (`--locked --workspace --all-targets -- -D warnings`), Rust and
contract formatting, edited-link checks and `git diff --check` passed.

The final checks and log identities are recorded in `results.json`.
Nested and Safe process tests compare native receipts, tariff rejection, target
state and independent follower replay. Browser tests use mocked EIP-1193
providers, not actual MetaMask/Rabby extensions, and broadcast no transactions.
The UI production build, SDK tests, payment-boundary tests, discovery/debugger
regressions, contract tests, Rust formatting and workspace Clippy are included.

## Remaining qualification

No new Docker image, Kurtosis topology/PBS run, public-network transaction,
operator restart or live deployment was performed. Kurtosis scripts were
migrated and syntax-checked, not executed. The full protocol staged/stress
commands and complete CI release matrix were not run. Actual Falcon signature
workloads remain unqualified: the larger gas limits in the tests validate the
request/admission profile, not Falcon execution cost. Quotes remain advisory;
this evidence does not establish durable queue recovery or future-price coverage.
