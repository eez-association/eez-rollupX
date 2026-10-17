# Static discovery and temporary estimation unavailability

Deployment follow-up: the [October 15 Docker/Kurtosis and Chiado record](../2026-10-15-l2-fee-deployment/README.md)
supersedes the deployment-pending status below and repeats the original wallet
selector checks against the upgraded live server.

| Field | Value |
| --- | --- |
| Date and status | 2026-10-15 UTC; implementation and local validation complete; not deployed |
| Scope | Investigate wallet discovery rejection and reconciliation-time gas estimation; preserve settlement safety checks |
| Runtime baseline / candidate | `897ec9d64b0b32b1daf2804f4ab5e1f546d08a32` / `4e84688cfcaba3b5c689cf0bf0e84f8026c67174` |
| Dashboard baseline / candidate | `7e10114e8690dff467eb432746b227284efd7148` / `4957a1c9cba1a3ad6bbc94d70d85cfd4574c0b82` |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized and unchanged |
| Environment | Rust 1.95.0, Foundry 1.7.1, LLVM 22, four Cargo jobs; isolated process chains and mocked Chromium wallets |
| Related contracts | [Discovery](../../proposals/composer-wallet-discovery.md), [gas estimation](../../proposals/composer-gas-estimation.md), [prior fee integration](../2026-10-15-legacy-fee-quotes/README.md) |
| Evidence | [Sanitized results and identities](results.json); full logs/datadirs are local-only |

## Findings

The live Composer image was still
`sha256:030765e36a9ef21fee9cac7a673dd91d0547c3644a6a9ca87a32928c7a9437fb`,
with source label `a7fee49d04be78a01220a85d37042489ba082e35`. The previous local
legacy/RPC changes had not been deployed. Both live fronts answered the virtual
discovery call at `latest`, but rejected the same call at the current numeric
block with `-32602` and `Composer discovery supports only the latest block tag`.
The restriction exists in that older source too.

MetaMask's [block-reference middleware](https://github.com/MetaMask/core/blob/main/packages/eth-json-rpc-middleware/src/block-ref-rewrite.ts)
resolves `latest` to its tracked numeric block before forwarding. This reproduces
the reported server rejection; the user's original request payload and exact
extension version were not available. Discovery returns static process metadata,
so selecting chain state is unnecessary. The fix accepts validated Alloy block
IDs and ignores their state-selection semantics. It does not assert historical
configuration, block existence or canonicality. Destination/selector matching,
nonce echo, IDs, notification handling and value/override rejection remain.

The reconciliation message is a deliberate gas-estimator guard: an unresolved
submitted attempt prevents borrowing speculative state for a new estimate.
Five read-only probes of the same existing public nested-call fixture returned
four identical gas estimates (`0x1a01a`) and one exact reconciliation-pending
error; the following request recovered without intervention. The live queue had
no users and both chains were advancing. This establishes a temporary window
in that sample, not the cause of every historical failure or the user's unsupplied
transaction.

The guard and its message/code remain. Typed pending/context-changed errors now
add an explicit JSON-RPC retry hint. The dashboard retries only those two
reasons, on the same Composer, with at most four requests and 1/2/4-second
backoff. Input changes cancel obsolete bridge requests and waits. Unknown
errors, reverts and older unstructured errors still fail; no guessed gas,
ordinary-node fallback, signature or submission is introduced by a retry.

## Reproduction and checks

| Check | Evidence type | Result |
| --- | --- | --- |
| Discovery baseline | Live read and unit | Both live fronts reproduced numeric-tag rejection. One selected regression failed before the source fix; after the fix it passes within the node suite |
| Dashboard retry baseline | Mocked unit | Three new cases failed and one passed before implementation: recovery, retry bound and cancellation were absent |
| Composer/node units | Unit and mock HTTP | 343 Composer passed, one existing profiling utility ignored; 85 node passed. Includes numeric-tag calls on both fronts before readiness with an unreachable upstream, selector validation and typed retry mapping |
| First unit build | Compile failure | A new test omitted the `json!` macro qualification. Corrected the test and formatting, retaining the initial log; not counted as executed coverage |
| Static checks | Static | Workspace formatting and locked Clippy with all targets and warnings denied passed |
| Dashboard units/build | Mocked and static | Four new retry/bound/cancellation cases, existing raw gas/error checks and 27 discovery cases passed; TypeScript/Vite build passed with the existing bundle-size warning |
| Browser | Mocked Chromium | 27 bridge scenarios, 16 wallet-discovery scenarios and six fee scenarios passed; zero real transactions broadcast |
| Native wallet processes | Source-based process | Three passed, zero failed/ignored, 141 filtered out, 240.98 seconds. Ethereum, Gnosis and posting-wrapper profiles exercised bidirectional estimates, signing at returned gas limits, canonical effects/settlement and independent follower replay. The real reconciliation guard emitted structured hints and recovered during these scenarios |
| Documentation | Static | Diff checks, 21 local links/anchors and two shell examples passed |

Executed runtime commands, from the isolated integration checkout:

```bash
export CARGO_BUILD_JOBS=4
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
export PATH=/usr/lib/llvm-22/bin:$PATH
cargo +1.95.0 test --locked -p eez-node --lib -- composer_rpc::tests::discovery_accepts_wallet_block_selectors_without_reading_chain_state --exact
cargo +1.95.0 test --locked -p eez-composer -p eez-node --lib
cargo +1.95.0 fmt --all --check
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
export EEZ_TEST_LOG_DIR="$PWD/artifacts/wallet-rpc-compatibility/process/logs"
export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/wallet-rpc-compatibility/process/datadirs"
forge build --root contracts
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- cross_chain::wallet_gas_estimates --test-threads=1 --nocapture
```

The runs reused the retained Cargo target from the October 14 integration.
Runtime logs/datadirs live under `artifacts/wallet-rpc-compatibility/` in the
integration checkout; dashboard logs are `/tmp/eez-wallet-errors-ui-*.log`.
These full artifacts are local-only and unavailable in a fresh checkout.
The owned loopback preview was stopped after browser checks.

## Compatibility and limits

Discovery's ABI/schema is unchanged; old latest clients still work. Older
Composers retain the numeric-tag incompatibility. Estimation messages and codes
are unchanged, with additive data only for the two typed temporary contexts.
New clients surface older unstructured errors rather than interpreting text;
old clients may ignore the new hints. No protocol, genesis, storage, historical
decoding or signer-wire changes are involved. Restart retains the documented
in-memory queue recovery limitations.

Live operations were read-only: no signatures, funding, broadcasts, restarts or
deployment. Real extension execution, new Docker images and Kurtosis validation
were not performed for this fix. Mocked wallets do not establish an installed
extension's behavior. The new binaries still require deployment before users of
the existing live service benefit from these changes.
