# L2 fee integration and Chiado trial

| Field | Value |
| --- | --- |
| Date and status | 2026-10-14; local release validated and existing Chiado runtime upgraded; canonical paid nested settlement and independent replay passed |
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
unrelated work remain intact. The authorized runtime, dashboard, indexer and
gateway configuration were upgraded. Core contracts and genesis were preserved.

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
| Gateway fee route | Isolated container, then live TLS route | Caddy accepted the exact endpoint route; public RPC chains, dashboard configuration and actual signed fee flow passed after cutover |
| First Kurtosis startup | Exact local images / private network | Failed before node RPC startup: the profile set the L1 fee recipient but omitted `EEZ_L2_FEE_RECIPIENT`, so policy validation rejected the beneficiary mismatch. Corrected the selected test profile; first startup and node logs retained |
| Final focused package matrix | Unit / integration | 1,038 passed, three ignored manual measurements/inventory utilities; no failures |
| Kurtosis demo startup | Exact deploy image | The running-base wrapper used the removed `BRIDGE` export and failed after fixture deployments. Updated it to pinned `BRIDGE_L1`/`BRIDGE_L2`; the same deployment procedure then passed and started the dashboard. Bash syntax and seven deployment-preflight tests passed |
| Failed-enclave stop | Container lifecycle | Kurtosis stopped part of the owned enclave, then failed because the never-started node was absent from its registry. Remaining containers were stopped by the exact owned enclave UUID; no operator services were stopped |

The first Cargo log is local-only at
`artifacts/l2-fee-integration/port-check-first.log` in the integration checkout.
It is retained for diagnostics; an ignored path is not evidence available to
other checkouts. Earlier fee tests establish behavior only at their recorded
older revision and do not validate this port.

## Trial behavior and limits

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

L1-origin and ordinary L2 admission remain compatible; their costs are not
covered by a new L1 collection adapter. Pending requests and retry counters
remain in memory. Clean restart evidence does not establish crash recovery or
durable financial ownership. Actual MetaMask and Rabby extension approval flows
were not run: browser checks used EIP-1193 adapters, including real signing and
broadcast against the private network.

## Exact local release

Runtime shipping source is `a7fee49d04be78a01220a85d37042489ba082e35` on the
recovery base above. Later commits through `ca09040` change test harnesses,
documentation and the demo deployment wrapper, not node/signer shipping source.
The dashboard is `75617b8a46108914ff786b158707f005c35f4ca4`, gateway route source
is `fb358c3ba833a0832623caea67dad66e9d303d9d`, and indexer source is `d6815ae`
(unchanged by later runtime commits). Dependency locks and protocol pin did not
change. Builds used the shipping feature selection with bounded Cargo jobs;
the local indexer build recipe used Rust 1.95.0 and two Cargo jobs. Its separate
manifest and lockfile were unchanged; the exact local recipe is retained.

| Image | Exact local Docker image ID |
| --- | --- |
| Node / independent follower | `sha256:030765e36a9ef21fee9cac7a673dd91d0547c3644a6a9ca87a32928c7a9437fb` |
| Proof signer | `sha256:41dd5d8102ac4bc1b12faa9d78e35b34507ed3e5dc292c4a7a88e4770be2ce02` |
| Dashboard | `sha256:0cf7315cc116e14de7f085422403b303a62d212dea2b2d77544df63a47f604a5` |
| Indexer | `sha256:05a77a38853ed8b72e519f197c38b7bb551ee807ad539fa0fda68d8d987047b0` |
| Core test deployment image | `sha256:d40f33c250e9831a7a3fb5a24ad8586fe2477b29f675ea2176fce15c58002426` |
| Corrected demo deployment image | `sha256:b1e7b66f935adcb27b4334ff9af9c93ec5dd4ee3a62baad8c76d99eaa95d39f3` |

The extracted image binaries used for independent private-network replay have
SHA-256 hashes `c344a182bbce02db369ea8a3baa1443c30177f22c8553815eafdef69c16ba5d8`
(Composer) and `0ab2cc04629b35307180bafba84c34091228f6181aec67174ee6320a8691ba1b`
(follower). Image IDs identify locally tested content; these images were not
published or promoted through the full CI release matrix.

## Completed validation

| Check | Result and retained evidence |
| --- | --- |
| Final source-based native regressions | Ten passed: five chained interstate scenarios, dynamic existing-proxy admission, partial prefix consumption, queue reorg, Ethereum and Gnosis posting-cost checks; `native-regressions.log` |
| Private-network two-user nested settlement | Both sources were included in the same native L2 block. Zero-tip source was rejected as `l2_tip_underfunded` with no ownership, nonce consumption or target write. Actual pooled tips covered complete canonical posting/blob costs; `kurtosis-fees-canonical.json` |
| Independent blob authentication | KZG, native payload semantics, settlement-table binding and exact signed source bytes passed for pooled and browser settlements; `fee-blobs/` |
| Final compiled dashboard against private network | Actual quote, one wallet-adapter signature, broadcast, canonical source receipt and target state passed on final dashboard image; `kurtosis-browser-final.json` |
| Dashboard bridge binding regression | Original image failed the conflicting legacy/explicit binding case. Final image passed seven readiness scenarios, six fee scenarios and 25 bridge gas/fee scenarios; `ui-bridge-binding-baseline.log`, `ui-final-*.log` |
| Independent exact-image follower | Fresh private-network genesis, L1/Beacon inputs only: 60 derived blocks and three source receipts matched; `independent-replay.json` |
| Private-network fee indexer | Independent receipt recomputation matched both paid ranges, including burn, tips, ordinary gas, beneficiary and zero-priced native receipts; `kurtosis-indexer-verification.json` |
| Clean private-network node restart | RPC and correlation recovered in 13 seconds, preserving the settled block and registry/safe convergence; `kurtosis-restart/20261014T230434Z/summary.json` |

For the two-user private-network batch, actual posting plus blob expense was
`835138988972432` L1 base units, required tariff was `1002166786766919` L2 base
units, and paid source tips were `2538745459514550` L2 base units. These values
use the disclosed conversion policy and do not establish liquid L1 proceeds.

Additional first failures were retained rather than counted as passing evidence:

- The fresh PBS relay had no registrations until epoch readiness. The fee
  harness now waits for an actual canonical settled prefix before its workload.
- The first receipt verifier retained a speculative receipt through
  recomposition. It now waits for canonical consumption, rereads the receipt and
  binds it to the canonical settlement range; the repeated exercise passed.
- The first image-follower launch received a single-quoted numeric setting from
  a shell YAML reader. Explicit settings matching the profile fixed that launch;
  the original diagnostic and datadir remain retained.
- The dashboard initially preferred the legacy shared demo bridge over explicit
  per-chain bindings. The reproduced regression now passes with the explicit
  bindings taking precedence. No protocol proxy-address assumption was added.

## Existing Chiado handover

The node and signer were stopped after the public transaction routes entered
maintenance and the accepted queue was verified empty. Both exited cleanly.
Complete stopped L1/L2 database copies were created in an owned upgrade directory;
the original databases remain untouched for rollback. The new node uses the
copies. Genesis, contract bindings, signer/poster identities, settlement timing,
Lighthouse, posting-log storage and unrelated services were preserved.

Private Compose overlays pin the tested node, signer, dashboard and indexer image
IDs. Their paths were added to each service checkout's existing `COMPOSE_FILE`,
preserving all other settings. Only the node receives the pricing policy and
`EEZ_L2_FEE_ENFORCE_TARIFF=true`. Dashboard quoting is enabled with the default
`100000000000000000` L2-base-unit maximum execution-fee ceiling. The private smoke
used a tighter `5000000000000000` ceiling and held the shared workload wallet
locks in both protocol checkouts. No core deployment or genesis regeneration ran.

The gateway retains its previous image, environment and certificate volumes.
Its new exact quote path reaches the Composer application endpoint. A first
maintenance probe raced Caddy readiness, then exposed Caddy directive ordering:
an outer `respond` was ordered after the existing handlers. Before stopping any
runtime, explicit per-route maintenance handlers were validated and confirmed
to return 503 for HTTP and WebSocket transaction paths. Final routes were then
restored and verified; the gateway is not left in maintenance.

The previous indexer configuration pointed at the plain L2 node RPC. Its
settlement-range method returned `-32601`, leaving fee enrichment pending.
The private configuration was backed up and changed to the documented Composer
front; other settings and the database were preserved. Historical enrichment
has a backlog and is processed gradually. Existing L1 WebSocket warnings retain
the indexer's HTTP polling fallback; polling continued indexing canonical heads.

The first canonical live source was
`0x608536c487ffd12c75faaa722ad8d5f107a965f592ea0353034b41738caa21aa`,
in L2 block 626070. Its L1 posting was
`0x72f9e00d0da695fe3cc0d3b3f861c522077ba4443dd04f4f3ebe99e02e3ac0f6`.
Actual posting/blob cost was `606728003329592` L1 base units; the policy required
`728073603995511` L2 base units and the source paid `1422320408964231`.
The zero-tip variant was rejected before ownership or execution. The paid source
used one ordinary signed transaction and updated the nested L1 target.

An isolated follower using the exact node image started from a separate copy of
the stopped pre-upgrade checkpoint. It derived the new paid source from L1 and
Beacon data, matching block hash, state/transaction/receipt roots, fees and nested
state. Its trusted prefix is the pre-upgrade checkpoint, not a claim of fresh
replay of the entire public chain. An independent KZG/semantic verifier also
authenticated the live posting and its exact signed source bytes.

A second state-dependent call reused the same fixtures without funding or new
deployments. Source
`0xe6d99595f8baeed22160c440dda1598b322b857ecaa38a0522ade0642c647423`
used 99,149 gas in L2 block 626340, versus 116,249 for the first call. Posting
`0x5323ed762b49924d8a6398be0017228bf86c09ed797a52e6679e6f715b3d7d65`
cost `304779001215949` L1 base units; required tariff was `365734801459139`
and collected tips were `1400774408758711` L2 base units. Underpayment rejection,
canonical state, complete cost coverage, exact-image follower replay and
independent KZG/semantic verification passed again.

After correcting the indexer URL, this new posting's enrichment matched an
independent recomputation across L2 blocks 626226–626340: 99,149 ordinary gas,
74,180 native gas, `4858301` burned L2 base units and `1400774408758711` tip units
to the configured beneficiary. The native receipt had zero monetary charge.
The first posting's enrichment wait timed out behind 3,868 older due rows; that
timeout is retained and is not reported as a pass. No database retry order or
fee result was manually changed to obtain the second result.

A read-only Chromium check of the deployed dashboard passed over TLS with its
assets, runtime fee flag and no page-script errors. Initial launches used an
absent/default browser cache and then an incomplete module cache; selecting the
existing intact module and matching browser cache resolved those tool setup
failures without changing the deployed image. This check did not access a wallet.
Temporary test services and the owned Kurtosis enclave were stopped after
retaining evidence; the live deployment and unrelated enclaves remain running.

Rollback must stop the candidate writers and restore the previous runtime
configuration/images with the preserved original databases; an older binary
must not open candidate-modified databases. Restore the saved dashboard/gateway
configuration as a matched pair if rolling back their fee UI route. Keep the
new indexer when rolling back the runtime: older collectors may overwrite fee
enrichment. The PostgreSQL backup is retained, but restoring it would discard
subsequent monitoring history. Disabling the tariff is a separate policy change
and does not reverse settled transactions or collected tips.

## Evidence availability and exclusions

The checked-in [result summary](results.json) contains sanitized chain IDs,
source/posting hashes, receipt costs, replay counts and the live accounting check.
The local evidence archive is under
`artifacts/l2-fee-integration-20261014/` in the operator checkout. Build/test logs,
sanitized result JSON, image/binary identities and source bundles are retained
there with a digest manifest. This is local-only evidence, not an uploaded CI
artifact. Private operator backups, signing configuration and bulk database
copies are kept separately in the owned `data/l2-fee-upgrade-*` directory and
are not committed. Public test fixture keys are not operator credentials.

The full CI release-artifact, dependency-audit and exhaustive Kurtosis matrices
were not run; this local release used the focused suites above. Core exclusions
remain `script/*`, `test_Doc_OriginalValue_SeedInSetup` and
`test_PostCost_Incremental`; the three skipped upgrade/key core cases are not
coverage. Three ignored Rust measurement/inventory utilities and optional
configuration/resource skips are likewise not passes. No performance, crash
durability, real wallet-extension, L1 builder refund coverage, production asset
conversion or guaranteed profitability claim follows from this trial.
