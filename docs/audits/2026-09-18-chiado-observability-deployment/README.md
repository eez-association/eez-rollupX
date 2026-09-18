# Fresh Chiado observability deployment

| Field | Value |
| --- | --- |
| Date and status | 2026-09-18; completed functional single-host deployment; current runtime and canonical monitoring verified |
| Scope | User-authorized backup and fresh, testable single-host Compose network |
| Initial deployment source | `3eeff740c5cd4d075a474ef676e92bb51fe1ade2` |
| Current native core shipping source | `68e5da389b0df815a0b013dea7915735f544aa93` |
| Protocol | `3c0cdf4955941a5c23639346932b55b33babebbd` |
| Networks | Chiado L1 10200; fresh L2 6291, replacing 6290 at the public endpoints |
| Related change | [Beacon blob selection fix](../2026-09-18-chiado-blob-selection/README.md) |

## Preservation and deployment

The stopped old rollup and explorers were preserved before deployment. Host-local
backup `.runtime/chiado-backups/before-observability-20260918T164046Z` contains
4,802 verified filesystem entries (76,540,837,808 logical regular-file bytes), both
PostgreSQL logical dumps, three stopped explorer volume archives, fourteen original
images, and one stopped-container filesystem fallback. The original Blobscan API
image was already unavailable; its filesystem/configuration is not represented as
an equivalent original image. This is a local backup, not an independent-host
restore test. The old composer was already unhealthy. Live gateway TLS volumes
were outside this rollup-state snapshot.

The fresh deployment uses separate writable L1/Beacon copies, an empty L2 database,
fresh proof/system keys, a separately funded functional-test wallet, and distinct
Compose/container names. The funded L1 poster/deployer is reused only while the old writer is stopped; old
L2 state and witnesses were not reused. Original rollup/explorer containers and
volumes remain retained.

All six deployment receipts were checked against canonical Chiado blocks. The
registry, manager, signer, threshold, verification key and bridge getters match the
generated bindings. Verified genesis block hash and state root agree with the
shipping helpers, rendered genesis and registered commitment.

| Binding | Value |
| --- | --- |
| Registry | `0x0ca34c68324dd5ef4bd6813af661d7817e4793b8` |
| Rollup ID / deployment block | `1` / `23118328` |
| L2 genesis block hash | `0x3f29c4f78394260473f543ae4602c3d5edc759a49303fa23c3b3497e903f61d0` |
| Initial node image | `sha256:e4155b7c16cd802f573140b0771071d02fb636a684fbbb2ee5a1ed9bfb24d5bd` |
| Initial signer image | `sha256:273f4d14f4b932ed231168d4e4c3b38fcb9883aae01cfba6ee81bd111f1b037e` |
| Initial deploy image | `sha256:412807cf0da3775561fcf3b5e205c11cc2ac45f0997082d6c54bc1738ad049eb` |

The supported no-publication producer and artifact consumer verified exact runtime
identity. All **1,311 archived unit tests passed**; two intentional utility tests
were excluded. Workspace all-target strict Clippy and formatting passed. Images
were built locally and were not published or promoted.

## Recovery and first failures

First boot exposed an inherited L1 storage inconsistency: receipt static files
ended at 23117823 while execution changesets ended at 23117821. The new failed copy
was preserved before recovery. The matching Gnosis CLI (commit `7e588a7b951a`, binary
SHA256 `5edb6fcda7d63335270f8bcd39fbfeb68d8c9a846d7cb9a7150122d77d168bd8`)
performed its supported all-stage unwind to 23117790 on only the new working copy.
No raw database edits or incompatible stock-image repair were used. Seven subsequent
read-only checks passed, including consistency and matching static segment ends.

After replay, local/public block hashes, state roots, receipt roots and all 37
receipts agreed across the old failure heights, deployment block and a common
finalized block. Registry commitment advancement matched canonical L2 blocks.
No persistence errors were found after recovery. This does not establish arbitrary
crash recovery for the old deployment.

The first functional-test setup exposed a separate harness defect: Forge's
`--gas-price` configures the simulated EVM, while `--with-gas-price` sets the
broadcast maximum fee. With a 7-wei L1 base fee, the old command estimated a maximum
fee below its explicit 1.5-gwei priority fee and submitted no L1 transaction.
The exact failure was retained; replacing only that Forge option produced a
successful setup receipt. Existing Cast fee options and semantic assertions remain
unchanged. An earlier diagnostic invocation used the wrong working directory;
that file-not-found error is not evidence of the fee defect.

## Monitoring and public services

The new monitoring project has fresh PostgreSQL/Prometheus/Loki/Grafana volumes and
only the real `chiado` / `eez-chiado-6291-20260918` namespace. Simulated demo volumes
remain retained. Composer/signer metrics and lifecycle spools are enabled; the
observer imports canonical receipts, settlement/source correlations and authenticated
blob measurements. Its supported batch size is 64 to catch up to the one-second L2.

A private Compose relay uses a shared Unix socket between host-network and bridged
Nginx services. It gives observers/explorers access to local RPC, Beacon and native
metrics without new published ports or firewall changes. Producer spool ownership
and read-only consumer mounts are explicit. Administrative database credentials are
not mounted into the continuous observer.

The existing domain, gateway and Grafana password are retained. UI/monitor bindings
now select chain 6291. Settlement interval is 30 seconds, maximum unsettled L2 blocks
150, and L2 block time remains one second. Fresh explorers use their own volumes and
the verified deployment block/hash/Beacon slot as their history boundary.

HTTP/browser checks verified the UI, monitor and both explorers; Blockscout's genesis
matches canonical RPC. A bounded Blobscan verification covered 210 finalized L1
blocks and three blob transactions, comparing exact receipt fees and blob bytes/
commitments. This check did not independently recompute KZG commitments. Grafana's
seven dashboards and three data sources passed local authenticated checks, and
native/observer Prometheus scrapes plus lifecycle ingestion succeeded. Public checks
were unauthenticated; the administrator password was not sent through the public URL.

## Functional validation and native identity correction

The corrected one-wave functional matrix passed: inbound and outbound wrapper
effects both reached 141; deposit and withdrawal deltas were exactly
11,000,000,000,000,000 and 1,000,000,000,000,000 wei. Canonical L1/L2 reconciliation
passed with no observed divergence. A graceful node restart retained the previously
safe L2 block and both heads resumed advancing. The first restart log check included
an expected shutdown message; startup-only checking uses the actual new process
start time. These are functional checks, not capacity measurements.

The subsequent live observer check exposed a native producer identity defect:
converting GnosisChainSpec to its inner Ethereum spec loses the AuRa genesis-header
override. Native L1 identities therefore reported `f438827f...` while canonical
Chiado block zero is `ada44fd8...`. The observer correctly rejected those identities.
The code correction extracts identity while the concrete chain-spec type is still
available; Ethereum/L2 behavior and posting blob rules are unchanged. Canonical
Chiado and Ethereum hash regressions cover the helper: all 46 node library tests
passed, and the selected baseline regression reproduced the wrong hash. The initial
unqualified exact-filter run selected zero tests and is not counted as evidence.
Historical bad metadata is
not rewritten, and observer identity checks remain strict.

## Evidence and remaining checks

Detailed artifacts are private, host-local, under
`.runtime/chiado-observability-20260918/validation`, `public-preparation`, and
`monitoring-preparation`; build/unit artifacts are under the worktree's
`target/chiado-runtime-3eeff74-qMd50kzT`. They are not portable published artifacts.
The first boot, first canary attempt, diagnostic failures and successful recovery
records are retained separately.

The corrected producer was built with the standard shipping feature selection,
Rust 1.94.0, locked dependencies and release flags, then verified by the standard
runtime artifact consumer. This focused local build omitted regeneration of the
full release test archives; the initial 1,311-test archive result is not presented
as a rerun on the later image. All 46 affected node tests, workspace all-target
strict Clippy, formatting and shell syntax passed for the correction. Two opt-in
Docker fixture tests were selected but skipped; the actual shipping build and
container inspection instead verified private Foundry cache/broadcast exclusion.
The one-line deployment Dockerignore correction prevents private live test cache
from entering future deployment images.

Current immutable images:

- node: `sha256:6bd87541c872b03f4d92ecdd89e984f51fae73597c1213b02238b699193d261e`
- signer: `sha256:f8d81deb3d8be778bb7faba38393cb6b09381f32c585d671033d670d716d730d`
- deploy: `sha256:2ed4179deada4996c421813c75c1637275b18bfe56f419c00f358d672ea3d1b7`

Runtime archive SHA256: `9acb917dc56ad5dc53d67aafb27f7b767434b09b6711d3509bdb0b74dc8ceaea`. Local shipping method and
source hashes are retained in private `validation/shipping-build/method.json`.

The fresh inbound/outbound canary passed canonical receipts, distinct destination
events and registry-backed values 242/343. SQL independently reports both exact
source hashes as canonical/committed with `node_reconciled` evidence and the correct
chain/genesis identities. All 62 receipt facts captured before the image upgrade
remain unique and unchanged, including their exact fees. The seven historical bad
admission payloads remain unchanged and unknown; no identity guard or historical
record was relaxed. At verification, all 23 posting blobs had authenticated
measurements and source-receipt coverage had no missing, lookup, accounting or
index-coverage gaps. These are bounded observations, not perpetual guarantees.

| Direction | Verified source transaction |
| --- | --- |
| Inbound | `0x945b191fdde04f71a76cd6dccb30dcf8db4aefb104d9f519283f3fd88ef00ad2` |
| Outbound | `0x22873cc65c9550c90e60eaee95edf55ba4633ffe25170bb0a8d8aec08696c817` |

Final public checks returned 200 for UI, monitor and unauthenticated Grafana
login/health, with RPC chain IDs 10200/6291. Original core containers remained
stopped; corrected node, signer and Beacon remained running.

Performance comparisons remain explicitly postponed by the user. Alerts
remain paused, distributed tracing and independent follower replay are not enabled,
and no off-host disaster-recovery or production-readiness claim is made.

## Requirement coverage

| Requirement | Implementation / operation | Verification |
| --- | --- | --- |
| Preserve existing rollup | Cold filesystem/SQL/volume/image backup plus retained originals | Inventory/content/metadata hashes and archive checks; original writers remain stopped |
| Fresh compatible network | New registry/profile, genesis6291, separate data/keys/Compose names | Six canonical deployment receipts, getter/genesis checks and both-direction functional effects |
| Mixed Beacon applications | `crates/eez-l1/src/blob_source.rs` selects physical hashes before EEZ decoding | Baseline regression,145 L1 tests, initial archived units, live blob handling |
| Correct native chain identity | `crates/eez-node/src/lib.rs` retains concrete Gnosis spec | Canonical-hash regressions,46 node tests, two live canonical SQL outcomes |
| Usable functional driver | `scripts/xchain-test.sh` broadcast fee option | Reproduced estimate failure, successful corrected receipt and unchanged matrix assertions |
| Private image build inputs | `testing/kurtosis/Dockerfile.deploy.dockerignore` excludes Foundry cache | Actual deploy image contains neither cache nor broadcasts |
| Real self-hosted observability | Native overlays, schema22 observer/PG, Prometheus/Loki/Grafana | Scrapes, SQL fees/blobs, lifecycle streams,7 dashboards/3 data sources, snapshot uniqueness |
| Existing public access | Preserved gateway, updated UI/monitor, isolated fresh explorers | HTTP/browser checks, actual chain/genesis/receipt/blob comparisons |
| Recovery and persistence | Supported L1 unwind, graceful restart and image rollout | Canonical replay, retained safe block, advancing heads, unchanged62-row receipt snapshot |


## Chart and canonical-progress update

The observer alone now runs source `4e60e3fd74a5aaf95a497b1afe8451b0751b9987`,
image `sha256:7d3746280f4e26306fbd90819547c0a46aa0f73a821550dfd0c84b88b43737c1`.
The local shipping-only producer and standard artifact consumer verified its exact
revision. Native composer/signer image IDs and start times remained unchanged.
Schema 22, credentials and data volumes were retained; no migration was needed.

[Seventeen new trend panels](../../../observability/contracts/protocol-dashboards.md#trend-first-layout)
cover arrivals, outcomes, queue state, latency, gas/spend, blob utilization/bytes/
fees and chain assurance. Existing exact detail tables remain below the charts.
The network instance catalog includes both observer and native Composer, so its
built-in All selection no longer silently hides native queue metrics.

The observer now captures heights and persisted reorg revisions alongside hashes,
counts and health in one SQL snapshot. Normal published appends retain fresh
canonical gauges; reorgs (including restored hashes), missing/backwards fences,
errors and stale evidence still suppress them. This remains bounded snapshot
telemetry: a snapshot ahead of scanner publication can briefly be unavailable.

| Requirement | Implementation | Verification |
| --- | --- | --- |
| More useful graphs | Five dashboard JSON files; chart-first layout with original tables | 17 new panels; 25 total chart queries and browser renders against live data; no datasource errors |
| Stable normal-append confirmation metrics | `indexer/outcomes.rs`; existing persisted revision, no schema/config change | 84 observer unit tests; actual PostgreSQL production-query fixtures for both chains, reorg/restored hashes, missing cursors, health and contradictions |
| Native queue visible in network overview | Observer/Composer instance catalog, built-in All default | Generated RPC/native profile regression; live browser Current status and queue canvas |
| Real inbound/outbound visibility | Guarded two-transaction canary on existing proxies | Canonical receipts, unique destination events, registered settled root, destination values 684/785; SQL and Prometheus committed counts 3 per direction |

The new inbound hash is
`0x93660444222192f6f9d551ebb714c1f58253999cd752f46438daa1aa58ac5521`;
the outbound hash is
`0x319262a2bea411bd4c78aa9c275523dade0bb06c2af540f969a684a96559af66`.
Both SQL outcomes are canonical/committed with `node_reconciled` evidence.
The before diagnostic had 9 invalid evaluations among 16 samples. After rollout,
30 of 30 samples retained evaluation validity over roughly one minute while both
chains advanced through five distinct scanner cursors. This bounded observation
does not promise continuous availability or independent execution replay.

Validation also passed workspace all-target strict Clippy, formatting, dashboard/
selector/annotation/preparation checks and the existing 3,137-admission SQL query
fixture. First failures were retained: sandbox denied local test sockets; the
unrestricted observer suite passed 84/84. The standalone database harness initially
used a host binary incompatible with the PostgreSQL container's libc; its final
SQL-only run applied the 22 migrations directly and tested the production queries.
This is not a new full migration-CLI or release-archive test run. Browser/HTTP test
helpers were corrected to accept empty frames and the configured Unknown display;
those were validator failures, not hidden dashboard query failures.

Private evidence is retained under the deployment's
`validation/metrics-charts-20260918T191808Z/`, including image manifests, queries,
screenshots, canary receipts and before/after samples. Performance comparison
remains postponed; no network reset, main merge or image publication occurred.


## Fee breakdown and gwei display

The Gas overview now shows configured L1 wallet execution, blob and combined fees
for the selected range. Posting, selected-receipt and blob-posting charts show all
three components over time. The total is execution plus blob fees; overlapping
wallet/posting/receipt populations remain separate and must not be added together.

All monetary dashboard values, including poster balance history/current balances,
now use gwei. On Chiado this is 10^-9 xDAI, not an ETH or fiat price conversion.
Gas quantities, bytes, percentages and durations retain their dimensional units.
Twelve monetary detail tables multiply the original numeric wei expression by
exact 0.000000001 and return decimal text. Existing integer-wei allocation floors
remain before display conversion. No storage, RPC, metrics producer, runtime image,
configuration or network state changed.

Validation: 7 dashboard/selector tests and 25 preparation tests passed. An isolated
real Grafana/PostgreSQL/Prometheus fixture passed 114 generated queries and 17
semantic groups, including one wei, zero, U256 maximum plus one, NULL, exact
component sums, allocation and denominator floors, stale sources and balance
scaling. Live checks passed 45 panels, 62 complete receipt arithmetic comparisons
and 75 fee-bucket sums. Browser verification rendered eight charts and the new
selected-range totals with no datasource errors in the visible panels.

Two existing Gas attribution tables (IDs 6 and 9) exceed the unchanged 10-second
SQL limit on the live population. Both the original wei queries and converted
queries reproduce this timeout; isolated fixture cases pass. This was retained as
a query-optimization follow-up, subsequently resolved below; limits were not
increased and that original check remains recorded as a failure. The requested fee charts, exact receipt table and totals
passed. Private evidence: `validation/fees-gwei-20260918/` under the deployment.


## Attribution-table query optimization

The previously timed-out Gas tables 6 and 9 now materialize their existing allocation
projection once for the exact selected L1/L2 contexts, before outer presentation
filters. This is confined to their dashboard SQL. Source membership, unprojected
claims, conflicts, positive orphaning, allocation denominators, exact gwei values,
filters and row limits remain defined by the existing views. No schema migration,
new configuration, longer timeout or runtime restart was required.

On the live dataset, EXPLAIN ANALYZE completed in 289.7 ms and 246.8 ms, with separate
planning times around 61 ms. The grouped membership and checked evidence each ran
once (50 grouped posts, 88 checked claims in that snapshot). Grafana reader API
requests took 318 ms and 337 ms and returned three coverage states and six source
allocations; both tables also rendered in the browser. The original queries
exceeded the unchanged 10-second deadline. These are bounded measurements on the
shared host, not a network-performance benchmark or an unlimited-history claim.

An initial API check immediately after writing provisioned files still retrieved
the previous query text and timed out. The failure is preserved. After Grafana's
dashboard reload, the validator required the actual served materialized SQL before
executing or reporting timing; neither old-query failures nor empty fast results
were counted as candidate success.

Validation includes the isolated Grafana/PostgreSQL/Prometheus datasource suite,
seven dashboard/selector tests, the existing 101-claim deadline regression, and
60 full-row baseline/candidate comparisons across 30 context/time/source and
attribution uncertainty/retraction cases. Initial comparisons require nonempty
results; explicit NULL accounting retains gaps and missing estimates.
Private plans, served-query results and table screenshots are retained under
`validation/allocation-queries-20260918/` in the deployment directory.
