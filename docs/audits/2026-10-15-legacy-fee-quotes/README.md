# Legacy fee quotes and Composer JSON-RPC

| Field | Value |
| --- | --- |
| Date and status | 2026-10-15 UTC; implementation and local validation complete; not deployed |
| Scope | Legacy L2-to-L1 quoting/admission, a custom quote RPC, SDK and dashboard integration |
| Baseline | Runtime repository `728a30de7c56396abe7a764c3480829fbd3ec36b`; dashboard `75617b8a46108914ff786b158707f005c35f4ca4` |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized and unchanged |
| Dashboard candidate | `7e10114e8690dff467eb432746b227284efd7148`; local `feat/l2-fees-rog` branch; no live deployment |
| Retained evidence | [Results and source/executable hashes](results.json); full logs remain local-only |
| Environment | Rust 1.95.0, Foundry 1.7.1, LLVM 22, four Cargo jobs; isolated native chains and mocked Chromium wallets |
| Related contracts | [Quote API](../../configurable-settlement-policy.md#unsigned-l2-fee-quotes), [increment scope](../../proposals/cross-chain-fee-recovery.md#sixth-development-increment-legacy-transactions-and-json-rpc-quotes) |

## Findings and changes

The previous API was an application HTTP route, not a protocol requirement.
`eez_quoteL2ToL1Fee` now serves the same response through JSON-RPC on the L2
Composer front, sharing readiness, simulation bounds, cancellation and the sole
worker with the retained HTTP route. The SDK accepts a direct Composer RPC
function or an HTTP quote URL. Wallet submission still uses ordinary
`eth_sendTransaction`, with one signature.

The baseline rejected valid EIP-155 legacy sources with
`Unsupported("tariff admission requires type 2")`. The new profile preserves
the actual legacy envelope in simulation and blob sizing, returns only
`gasPrice`, and rejects replay-unprotected sources. Its contribution is
`charged source gas * (gasPrice - actual L2 base fee)`. The burned base fee and
unused gas-limit allowance do not finance settlement. Existing type-2 behavior,
whole-post pricing, cumulative composition and final dispatch checks remain.
The native replay helper's type allowlist now admits legacy as well as type 2;
its expanded regression compares both types with real signed execution,
including GASPRICE-visible state, reversions, balance and nonce rejection.

Rabby's inspected `develop` source configures custom networks with
`eip[1559] = false` in its [custom-network service](https://github.com/RabbyHub/Rabby/blob/develop/src/background/service/customTestnet.ts)
and submits `gasPrice` from its [test-network signing view](https://github.com/RabbyHub/Rabby/blob/develop/src/ui/views/Approval/components/SignTestnetTx/index.tsx).
Its regular signing view also contains EIP-1559 support; this is a custom-network
distinction, not a statement that every Rabby transaction is legacy. The
dashboard defaults to legacy when its selected provider identifies as Rabby;
other providers default to type 2. Explicit supported types are preserved.
Neither source inspection nor mocked wallet tests establish the behavior of a
particular installed extension version.

## Validation

| Check | Result |
| --- | --- |
| Baseline reproduction | One selected Composer regression failed with the existing type-2-only rejection. An earlier short filter combined with `--exact` selected zero tests and is not counted as evidence |
| Composer and node units | Final run: 343 Composer tests passed, one profiling test ignored; 83 node tests passed with loopback sockets enabled. Includes signed-versus-unsigned legacy replay, EIP-155 protection, RPC readiness/IDs/notifications/mixed batches, absent policy, input bounds and a shared worker permit |
| First unit environment | Eight node HTTP tests failed because the filesystem sandbox denied socket creation. The same source passed with local sockets permitted; the initial log is retained |
| SDK | Seven tests passed: legacy fee arithmetic/ceiling, EIP-1559, RPC and HTTP transports, exact intent, expiry and standard wallet submission |
| Dashboard | Two payment tests and TypeScript/Vite build passed; the existing bundle-size warning remains |
| Mocked browser | Six fee scenarios and 25 bridge scenarios passed; legacy `gasPrice` for Rabby and EIP-1559 fields for MetaMask; no real transactions broadcast |
| First native fee/Safe process run | Five passed, one failed, zero ignored, 138 filtered out, 537.77 seconds. The legacy scenario reached composition but failed in native quote replay: `invalid unsigned quote gas/type`. This exposed a second type-2-only guard in `SyncBlockState::preview_unsigned`; first logs and failed datadir retained. A focused signed-versus-unsigned replay regression reproduced that exact guard failure before the fix, then passed after it |
| Second native fee/Safe process run | Five passed, one failed, zero ignored, 138 filtered out, 332.83 seconds. The legacy scenario rejected zero-contribution and gas-limit-based underpayment, then executed the funded source. Its receipt tip exceeded the quoted tip because the inclusion base fee had fallen; the test incorrectly required the type-2 fixed-tip equality. Corrected the legacy assertion to check the fixed total gas price and separately verify quote versus inclusion base-fee accounting. This is a test correction, not an unchanged retry |
| Final legacy process regression | One selected test passed, zero failed or ignored, 143 filtered out, 117.68 seconds. Validated custom-RPC placement, both underpayment rejections, signed legacy execution and fee arithmetic, nested L1 target and L2 callback effects, canonical settlement and independent follower replay |
| Workspace formatting/Clippy | Passed, including `--workspace --all-targets -- -D warnings`; affected E2E package Clippy passed again after the test assertion correction. Initial Clippy found one lossless-cast style error in the new sizing test; corrected and rerun |
| Documentation | Diff, local link/anchor and shell-example syntax checks passed |

Runtime checks use the repository's existing target directory with
`CARGO_BUILD_JOBS=4`, `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`, and LLVM on `PATH`:

```bash
cargo +1.95.0 test --locked -p eez-composer -p eez-node --lib
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
export EEZ_TEST_LOG_DIR="$PWD/artifacts/legacy-fee-quotes/process-2/logs"
export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/legacy-fee-quotes/process-2/datadirs"
forge build --root contracts
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- fee_quotes:: --test-threads=1 --nocapture
export EEZ_TEST_LOG_DIR="$PWD/artifacts/legacy-fee-quotes/process-3/logs"
export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/legacy-fee-quotes/process-3/datadirs"
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- fee_quotes::legacy_rpc_quote_rejects_underpayment_settles_and_replays --exact --test-threads=1 --nocapture
cargo +1.95.0 fmt --all --check
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.95.0 clippy --locked -p eez-node-e2e --all-targets -- -D warnings
node sdk/l2-fees.test.mjs
git diff --check
```

The final single-test invocation follows the legacy fee-assertion correction;
the five unchanged scenarios passed in the second run using the same runtime
sources. Those scenarios cover both type-2 quote transports, nested Safe
deployment on Ethereum and Gnosis, and ordinary L2 subsidy. The substring
process filter also selects `safe_fee_quotes::`. Logs are retained
under `artifacts/legacy-fee-quotes/` in the isolated integration checkout. Those
logs are local-only and unavailable from a fresh checkout. Dashboard logs are
the local `/tmp/eez-legacy-fees-ui-{build,unit,browser,bridge}.log` files; browser
tests used the owned loopback preview with fixture RPCs.

## Compatibility and limits

Upgrade the Composer before the dashboard starts using the custom method or
legacy quotes. Existing type-2 HTTP clients remain supported. Roll back the
dashboard before restoring an older Composer. There is no protocol, genesis,
storage, follower-decoding or signer-profile change.

This change has not been deployed to Chiado, packaged into new Docker images,
or run in a new Kurtosis enclave. Real Rabby/MetaMask extensions have not been
tested here. The prior deployment's evidence does not validate these new bytes.
Ordinary L2 settlement subsidy remains accepted; the configured tariff still
does not establish durable L1 reserves or realizable L2 revenue.
