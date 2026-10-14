# L2 fee integration and Chiado trial

| Field | Value |
| --- | --- |
| Date and status | 2026-10-14; implementation and validation in progress; no deployment yet |
| Scope | Signed L2 tariff admission, final native-receipt coverage, dashboard wallet integration, isolated Kurtosis validation, existing Chiado runtime upgrade |
| Source | Runtime base `3f5bc03fcf9383c6b425dbe16d859395c28ede59`, branch `feat/l2-fees-e2e`; candidate identities and image digests recorded with validation below |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`; initialized and unchanged |
| Dashboard base | `bf3f77e10faee6de63486803718494e64d68f749`; isolated checkout |
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
| Cumulative composition guard first build | Compile | Failed on an unnecessarily concrete history-provider bound and missing error conversion. Both corrected; current process/lint validation pending |

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

Current cumulative/direction scheduling, exact-image Kurtosis semantic assertions,
UI proxy/signing integration, independent image-based follower replay, restart,
and controlled existing-chain deployment remain unverified. Earlier source-based
checks do not establish deployment of these changes. Image/executable identities,
test counts, exclusions and deployment evidence will be recorded as checks execute.
