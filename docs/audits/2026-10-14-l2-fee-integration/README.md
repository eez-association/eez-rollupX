# L2 fee integration and Chiado trial

| Field | Value |
| --- | --- |
| Date and status | 2026-10-14; implementation and validation in progress; no deployment yet |
| Scope | Signed L2 tariff admission, final native-receipt coverage, dashboard wallet integration, isolated Kurtosis validation, existing Chiado runtime upgrade |
| Source | Runtime base `3f5bc03fcf9383c6b425dbe16d859395c28ede59`, branch `feat/l2-fees-e2e`; candidate identities and image digests recorded with validation below |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`; initialized and unchanged |
| Dashboard base | `b757fb09375b829dda53b42e52886fdd292ff583`; the actual running dashboard's newer base, ported in an isolated checkout |
| Environment | Rust 1.95.0, Foundry 1.7.1; bounded host Cargo compilation (four jobs) |
| Related records | [Proposal](../../proposals/cross-chain-fee-recovery.md), [earlier research and implementation](../2026-10-10-cross-chain-fee-research/README.md) |

## Scope and preservation

The user authorized completion, local images, isolated Kurtosis testing and an
upgrade of the existing Chiado L2 after validation. The original fee checkout
predates the running recovery fixes. Fee work is therefore ported onto the exact
running runtime base. Existing operator configuration, containers, data and
unrelated work remain intact. No core-contract or genesis change is intended.

This trial enforces a configured tariff. It does not implement durable treasury
reservations or prove that collected L2 balances can be realized as L1 funds.
The conversion policy is an explicit test assumption. Default-disabled behavior
must remain compatible, including historical follower replay.

## Checks and first failures

| Check | Evidence type | Result |
| --- | --- | --- |
| Checkout, protocol and existing resources | Static / operator inspection | Running node and signer use the recovery base above; existing enclaves and service resources identified and preserved |
| Initial recovery bundle clone | Tooling | Failed: incremental bundle required an absent ancestor. Full local clone supplied the exact base instead |
| Recursive protocol initialization | Tooling | Initial external lookup failed under restricted DNS. Initialized exact pins from existing local mirrors; no protocol edit |
| Three-way fee port | Static | API conflicts resolved while retaining the newer estimation, RPC and shutdown paths; validation pending |
| First focused Cargo check | Compile | Failed: the newer posting quote API requires an explicit final-parent flag. Corrected to require the target parent for transaction quotes; subsequent package/process builds passed |
| Dashboard clone | Tooling | Initial local clone failed across filesystems; repeated with `--no-hardlinks` successfully |
| Composer/node unit tests before cumulative composition guard | Unit / HTTP process | 341 Composer tests passed (one profiling test ignored), 81 node tests passed. Initial node run had seven sandbox socket failures; the permitted local-socket run passed |
| First fee process run | Source-based process | Two selected tests passed: advisory and enforced nested calls, underpayment rejection and independent follower replay. This run preceded the separate-direction regression and does not validate that addition |
| Integration / core contracts | Foundry | 86 integration tests passed; 772 core tests passed, three skipped. Core excludes scripts and `test_Doc_OriginalValue_SeedInSetup\|test_PostCost_Incremental` as required by AGENTS |
| Configuration/tooling | Static / mocked | 370 tests passed with seven optional-environment skips; all three Compose policy profiles passed. After gate transport assertions, 20 Kurtosis configuration tests passed with two real-resource skips |
| SDK and dashboard | Unit / production build | Five SDK tests and one dashboard payment test with multiple rejection cases passed. Dashboard TypeScript/Vite production builds passed; the pre-existing bundle-size warning remains |
| Indexer | Unit / isolated PostgreSQL | 33 regular tests passed. Both database-backed tests passed serially in an owned PostgreSQL 18.6 container. The initial database run correctly refused the wrong database name before schema mutation; repeated against the required `eez_indexer_test` database |
| Cumulative composition guard first build | Compile | Failed on an unnecessarily concrete history-provider bound and missing error conversion. Both corrected |
| Current fee process tests | Source-based process | Both selected tests passed in 177.29 seconds: advisory and enforced nested calls, precise underpayment rejection, concurrent inbound work settled separately, receipt costs and independent follower replay |
| Formatting / workspace Clippy | Static / compile | Passed at runtime revision `a7fee49d04be78a01220a85d37042489ba082e35`. Two initial style failures (constructor field order and identical match arms) were corrected; first logs retained |
| Dashboard browser regressions | Chromium / mocked RPC and wallets | Six fee scenarios, 25 bridge scenarios, 16 wallet-routing scenarios and 25 cross-chain scenarios passed, including eight responsive widths. This is not actual wallet-extension or real-network evidence |
| Gateway fee route | Isolated container | Caddy accepted the prepared exact fee endpoint route; live gateway unchanged |
| First Kurtosis startup | Exact local images / private network | Failed before node RPC startup: the profile set the L1 fee recipient but omitted `EEZ_L2_FEE_RECIPIENT`, so policy validation rejected the beneficiary mismatch. Corrected the selected test profile; first startup and node logs retained |
| Final focused package matrix | Unit / integration | 1,038 passed, three ignored manual measurements/inventory utilities; no failures |
| Kurtosis demo startup | Exact deploy image | The running-base wrapper used the removed `BRIDGE` export and failed after fixture deployments. Updated it to pinned `BRIDGE_L1`/`BRIDGE_L2`; the same deployment procedure then passed and started the dashboard. Bash syntax and seven deployment-preflight tests passed |
| Failed-enclave stop | Container lifecycle | Kurtosis stopped part of the owned enclave, then failed because the never-started node was absent from its registry. Remaining containers were stopped by the exact owned enclave UUID; no operator services were stopped |

The first Cargo log is local-only at
`artifacts/l2-fee-integration/port-check-first.log` in the integration checkout.
It is retained for diagnostics; an ignored path is not evidence available to
other checkouts. Earlier fee tests establish behavior only at their recorded
older revision and do not validate this port.

## Trial behavior and remaining acceptance work

Enforcement checks signed fees before queue ownership, cumulative native receipt
payments while composing, and complete measured post/blob costs before dispatch.
A source that becomes underfunded is deferred without charging a settlement
attempt, while independent eligible work can proceed. L1-origin requests settle
separately from funded L2-origin batches in this mode; existing FIFO prefix
rebuild preserves order. The disabled mode keeps existing mixed batching.

Test pricing assumes 1:1 base-unit conversion, 20% risk allowance and zero fixed
proving/operations/treasury charges or profit. This is explicitly a testnet
assumption, not a market conversion or a profitability claim. Existing Chiado
uses a 150-unsettled-block limit, below the bounded quote engine's 256-block cap.

Current cumulative/direction scheduling has source-based process evidence.
Exact-image Kurtosis semantic assertions, real UI proxy/signing integration,
independent image-based follower replay, restart, and controlled existing-chain
deployment remain unverified. Source-based and mocked browser checks do not
establish deployment. Image/executable identities, test counts, exclusions and
deployment evidence will be recorded as checks execute.
