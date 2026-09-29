# Candidate framing reserves and the 5% default

| Field | Value |
| --- | --- |
| Status | Implemented, deployed and locally validated; live semantic verification complete |
| Source baseline | `a4f09f17e777590a7a599ddb675cfd894449257d`; branch `feat/cross-chain-admission-throughput` |
| Candidate source | `e556e1208612d957b5732aae602c59c4ac24dd9f` (`candidate-framing-10`); initial implementation `bee97db`, intermediate configuration `57266ae` |
| Deployed baseline | `aa515c8720f7943df2d9d0ec147e2252ace1a21f` |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Contract | [Accepted follow-up](../../proposals/cross-chain-admission-throughput.md#fixed-admission-overhead-follow-up-accepted-for-implementation) |
| Baseline evidence | [Previous verified wave](../2026-09-27-admission-nested-delivery/README.md) |

The user authorized replacing the fixed admission overhead with candidate sizing
and changing the default gas margin from 20% to 5%. The baseline already runs an
explicit 5% override. Preserve that and all operator state during rollout.

The baseline's 25 source posts had a 25,264–122,808 gas gap between incremental
and full sizing. Every gap was additional calldata gas; the fixed reserve was
600,000. The observed maximum was only 59 entries/two blobs, so these measurements
do not establish a universal replacement constant.

Implementation accounts incrementally for native batch framing, root/position
hash rewrites, dynamic offsets, proof allowance and the target fork's maximum
blob indices. Calldata floor and execution projection must both fit. Static
entries require the existing general execution projection rather than retaining
discounts whose final batch guard would revoke. Final ordered execution, original
signed source limits, group boundaries and settlement ownership remain required.

Validation will compare bounds with actual ABI encoding over entry/blob/offset,
return-data, nested/static/revert and discount boundaries; verify default and
override propagation; run affected unit/process/configuration checks. Then build
exact-revision local shipping images, deploy to the existing authorized devnet,
repeat the wallet-locked verified workload, and compare canonical sources,
cutoffs, preparation and gas estimates. Full logs/datadirs remain local under
`artifacts/admission-framing/`; portable evidence will be added after execution.

## Initial validation

- Rust 1.94.0 locked unit suites: Composer 275 passed, one existing manual test
  ignored; L1 144 passed. Six framing regressions cover complete ABI encoding,
  calldata floor activation, transactional discount revocation, static-only cheap
  checks, the 65-blob general fallback with cleanup, and overflow rejection.
- Workspace/all-target Clippy passed with warnings denied; formatting passed.
- All 14 Chiado configuration checks passed; both mandatory settlement policy
  configuration checks passed. Explicit overrides remain unchanged.
- Grafana panels 1/3 executed against retained history; additive columns are null
  for historical estimator versions. No schema migration or collector restart.
- First unit attempt exposed three test fixtures tied to the old fixed reserve;
  corrected their derived boundary expectations. A later sandboxed test command
  could not bind localhost servers (15 L1 tests); the same suite passed with
  network access. Logs of these failures are retained locally, not counted as
  successful validation. No production failure was hidden by a retry.

Native process validation, exact-source images, deployment and the comparable
live workload are pending at this implementation checkpoint.

Nine focused native process cases passed at `bee97db`: repeated 45-source waves
on both networks, static-only exclusion, existing-proxy multicalls, middle-group
partial consumption, actual companion gas on both networks, and both receipt
fee-cost cases. The latter assert the absent margin setting reaches final signing
as exactly 5% above native required gas. Wave assertions compare the incremental
bound with production full batch sizing and retain independent replay checks.

A configuration review also found explicit 2000 values in all six shipped
Kurtosis profiles and its example. These are updated to 500. Kurtosis tooling:
13 passed, one opt-in real relay-resource test excluded (unrelated to margin
transport); both Compose policy propagation checks passed. Custom strings retain
runtime validation and explicit overrides, including 2000.

## Local images and deployment

Exact archived source and recursively pinned submodules were built with Rust
1.94.0, locked dependencies, release profile and four build jobs. The archived
local recipe matches the baseline and the production package/bin/example feature
selection in `Dockerfile.runtime`. These are local development images, not CI
promoted artifacts. The final configuration commit changes no runtime source;
its images reuse the identical compiled sources from the implementation commit.

| Image | Local image ID |
| --- | --- |
| `eez-node:57266ae-shipping-local` | `sha256:e19879e216b057dc5e761ab87a1fc11c985d95e18c87c737686c3c8f0328f6d0` |
| `eez-proof-signer:57266ae-shipping-local` | `sha256:631c7a0d747ec6102224ed8ec6d6d603937a9b6aaffcdba5bc9925c4bcd6c084` |
| `eez-deploy:57266ae-shipping-local` | `sha256:3b658300d92c0ca6e7409591fb66a722e4d573efd25958b7bb5099b6c212b58e` |

Executable SHA-256:

- `eez-composer`: `a395983fde6e240c60e6f7abd07b981a83601dfb482b9352566ee0545e90636b`
- `eez-follower`: `f6919c2445718d5a7143f9b6444a64d08e98740a8c00981aa05333dd9c0d6a3f`
- `eez-genesis-state-root`: `498f59d0c5ae5b5aad35bfb1de8f85857a205fd6014083206ea7da36c4d3f93f`
- `eez-genesis-block-hash`: `0eb6d3fb9bfcf6451cc5d12bb402370a0b375e8f69cbddac229004fdab3c3b13`
- `eez-proof-signer`: `c84a2fe6454fe95a10f67414b297720c646bc05db6c679c4779192a359de2e68`

The existing queue was empty before rollout. Signer became healthy before the
Composer restart; mounts, service environment, original genesis/state root and
operator settings were preserved. Only the two runtime image references changed.
The explicit live 500-basis-point override remains. Deploy image built and checked;
no contract deployment broadcast. Monitoring/indexer image remains unchanged.

First health check found the known peerless embedded-L1 restart gap. An initial
read-only prepare rejected missing recent raw bodies from the canonical RPC
index. No partial import occurred. A second preparation pinned an older local
Beacon checkpoint, validated every raw header/transaction hash and the full parent
chain against canonical RPC, and imported 24 payloads (23256547–23256570). All
Engine results were VALID; Lighthouse retained fork-choice ownership. Both heads
then became current/non-syncing, with empty queues and matching original genesis.
No database reset, queue recovery claim, or forced fork choice. Local evidence:
`artifacts/admission-framing/engine-rpc-gap/`, `health-after-rollout-first.json`,
`deployment-verification.json`, and `images/verification.json`.

## Preserved workload setup failure

The first setup (`20260927-161720`) was stopped before cross-chain send after
267 deployment jobs exhausted transport retries. Zero cross-chain sources
were sent; 315 ordinary setup transactions were recorded before cancellation.
Its logs, private profile and diagnostics remain under
`artifacts/admission-framing/failed-setup/` and the protocol run directory.
This attempt does not establish throughput or semantic correctness.

The task-owned forwarding helper held its rate mutex across upstream response
waits. Direct RPC reads were responsive, while setup clients exhausted their
15-second deadline in the serialized queue. The local helper now limits request
starts to five per second and permits at most eight upstream calls in flight.
A 28-call mock with 800 ms response latency completed in 6.204 seconds, with
minimum start spacing 200 ms and peak concurrency four; serial response waiting
alone would require at least 22.4 seconds. No runtime image changed. This fixture
checks scheduling, not live upstream availability. The fresh run keeps source
ingress, signed workload, funding amount and worker concurrency unchanged.
The helper adjustment affects ordinary setup transport and is a comparison limit.

## First complete candidate measurement and corrective follow-up

At `57266ae` / estimator 9, the fresh wave (`20260927-163032`) canonically
settled all 870 sources in 23 source posts, versus 25 at `aa515c8`. The measured
window was 166.277 s versus 172.541 s. All 23 final projections fit their admission
bounds, all signed limits used 5%, and preflight/receipt gas matched exactly.
Framing reserves ranged 120,768–229,440, mean 171,439; average capacity released
was 428,561 gas. These are single live runs, not a controlled statistical claim.

Preparation regressed from mean 380.3 to 434.4 ms. Full simulations rose from
3,134 to 5,901 and cheap gas refusals fell from 6,774 to 3,072. Version 9's zero
inbound-entry lower bound was safe for static-only calls but unnecessarily weak
for a discounted mutable prefix. The additional native suite was interrupted
before completion to fix this measured regression; its partial run is not a pass.
Initial logs/datadirs remain retained.

Estimator 10 tries the minimum mutable shape and, if that fails, a zero-mutable
static group that restores general pins and the anchor/entry discounts. Both
possibilities must fail before rejecting cheaply. Composition rejects empty calls.
Static-only groups also need zero additional L2 delivery gas, now reflected by
the cheap L2 check. Regression checks cover both alternatives, discount revocation
and static-only eligibility at a full L2 block. No source execution estimate is
cached across parents or accepted prefixes. A fresh live comparison and final
regressions will validate the corrected implementation.

The intermediate estimator-9 wave completed all **770 semantic jobs** with zero
failures or skips. Repeated canonical attribution preserved all 870 sources; all
23 post receipts also matched the external canonical RPC. Portable intermediate
evidence: [summary](framing9/summary.json), [framing per post](framing9/framing-per-post.csv),
[cutoffs](framing9/cutoff-summary.csv), and [companions](framing9/companion-breakdown.csv).
Its full-wave follower replay was not run; native wave tests include independent
replay of their own separate fixtures.

## Final estimator deployment and measurement

The corrected estimator was built from exact archived `e556e12` source with
the same release/feature recipe. The final node and signer images are
`eez-node:e556e12-shipping-local` and
`eez-proof-signer:e556e12-shipping-local`; [image verification](framing10/images.json)
records image IDs, embedded executable hashes, source/protocol identities and
original genesis checks. The deploy image was built and inspected, not broadcast.

Rollout at 17:07 UTC again preserved mounts, environment and genesis with an empty
queue. The existing peerless L1 gap required ten verified Engine payloads
(23257153–23257162); all returned VALID, with Lighthouse owning fork choice.
The final live workload started only after both heads were current/non-syncing.
Local evidence: `final-runtime-rollout.json`, `engine-rpc-gap-v10/`,
`health-v10-first.json` and `health-v10-recovered.log` beneath the artifact root.

Run `20260927-171022` used the wallet-locked staged runner with `--verify`,
the same 870-source workload, concurrency, funding and source ingress. All
870 sources settled (520 inbound, 350 outbound) in blocks 23257249–23257274.
Broader native process tests started after the settlement window and queue drain.

| Measurement | Baseline `aa515c8` | Intermediate `57266ae` | Final `e556e12` |
| --- | ---: | ---: | ---: |
| Canonical sources | 870 | 870 | 870 |
| Source postBatches | 25 | 23 | 23 |
| Sources per post, mean | 34.80 | 37.83 | 37.83 |
| Send-to-settlement window, seconds | 172.541 | 166.277 | 146.880 |
| Sources/second | 5.042 | 5.232 | 5.923 |
| Full source simulations across attempts | 3,134 | 5,901 | 3,339 |
| Cheap gas refusals | 6,774 | 3,072 | 7,397 |
| Cheap share of gas refusals | 75.69% | 38.98% | 76.42% |
| Canonical preparation mean, ms | 380.31 | 434.41 | 403.76 |
| Canonical preparation total, ms | 9,507.78 | 9,991.52 | 9,286.47 |
| Actual postBatch gas total | 177,509,959 | 178,052,062 | 179,414,275 |

The final wave used 8% fewer posts, carrying 8.7% more sources per post.
Preparation per post remained 6.2% above baseline, while preparation across the
whole wave fell 2.3%. Correcting version 9 reduced full simulations 43.4%.
Actual postBatch gas rose 1.1% versus baseline; this change releases admission
capacity and does not establish an execution-gas saving.

These are single live observations. Target-slot phase, chain state, block budgets
and relay inclusion vary. Final parent budgets ranged 17,676,668–18,007,470 gas,
versus baseline 17,453,689–17,762,955. Final delivery records contain 23 target
inclusions, two skipped target slots and three produced target blocks omitting
the post; acknowledgement does not explain an omission. The observed 17.5%
throughput increase cannot all be attributed to this code. Both baseline and
candidate already used an explicit 5% setting; default propagation is established
by the configuration and native signing tests.

### Gas bounds and cutoffs

All 23 final projections fit the admission bound; every signed postBatch limit
was exactly `ceil(native_required_gas * 1.05)`. Every postBatch preflight matched
receipt gas, as did all 520 inbound companion preflights. All 870 canonical
sources have accepted estimates and complete retained event sequences.

| Final canonical cutoff | Posts | Sources in those posts |
| --- | ---: | ---: |
| `l1_bundle_gas_budget` | 13 | 462 |
| `l1_tx_gas_budget` | 9 | 354 |
| `snapshot_exhausted` | 1 | 54 |

There were no pipeline-time or FIFO-limit cutoffs. Seven sources were accepted
after an earlier source's capacity deferral. Across all 29 examined candidates,
cheap checks accounted for 7,397 refusals: 3,711 transaction-budget and 3,686
bundle-budget. These are repeated attempt decisions, not unique rejected users.

Framing reserves were 120,768–229,440 gas, mean 171,439. The observed difference
between source-local and full sizing was 61,276–117,744, leaving a conservative
59,492–111,696 bound excess. Capacity released versus 600,000 was 370,560–479,232
gas per candidate (mean 428,561). Reapplying 600,000 to the same accepted prefixes,
parents and budgets makes 20 of 23 fail: nine transaction budgets and eleven
bundle budgets. This isolates the reserve effect on those prefixes; it does not
rerun scheduling or establish a counterfactual total number of posts.

Companion admission remains conservative: 136,942,998 estimated versus
73,376,313 actual gas (1.87×). Its components reconcile across all 520 inbound
sources. On bundle-limited posts, estimated bundle utilization averaged 98.9%,
while actual signed-start requirements used 58.7%. These are distinct from
postBatch framing and warrant a separate measured refinement; final ordered
execution and each original signed companion limit remain required.

Portable tables: [framing/estimate/receipt per post](framing10/framing-per-post.csv),
[cutoffs](framing10/cutoff-summary.csv), [companion families](framing10/companion-breakdown.csv),
and [same-parent old-reserve check](framing10/fixed-reserve-counterfactual.csv).
The detailed 870-source estimates and attempt history remain local in
`artifacts/admission-framing/candidate-v10/` and the retained indexer database.

### Monitoring verification

Grafana panels 1 and 3 successfully queried the final window. All 29 candidates
exposed the new framing/full-bound/calldata-floor/margin fields. Source rows
exposed the full estimate or cheap mutable/static minimum as appropriate. The
unfiltered source table's 10,000-row display cap truncates the 10,736 attempts;
filter by candidate/source for inspection. Database attribution separately
verified all 870 canonical sources. Cheap-refusal rows intentionally have no
full simulation estimate. The existing collector/indexer image and checkpoints
were preserved, and legacy rows keep null new fields. Gas-gate after-values are
provisional until the source outcome confirms acceptance.

### Final workload completion

The staged runner exited zero at 17:35:58 UTC: **770 semantic jobs passed**,
zero failed/skipped, one recorded verification per job. The last L1 verifier
took 174.827 seconds and completed without a failed assertion in its log.
These assertions cover expected entries/calls and posted table fields. They do
not establish complete rolling-hash replay: the staged workload lacks
`EXPECTED_L1_STEPS`, and some root-dependent event matching uses counts.
Full-wave independent follower replay was not run; the native fixture suites
separately exercise independent replay.

Repeated attribution after verification preserved all 870 source mappings and
23 posts, and every post receipt matched the external canonical RPC. At
17:38 UTC, final image identities, preserved configuration/genesis, current
non-syncing heads and empty cross-chain queue all passed. The task-owned ordinary
RPC forwarder was stopped and its port closed. It recorded 1,397 original-payload
broadcasts, no upstream rate limits, and 2,503 broken-pipe responses to disconnected
clients; the successful workload and final assertions are recorded separately
from these transport counters. No shared service or data directory was removed.

Portable records: [final measurement](framing10/summary.json),
[Grafana queries](framing10/grafana.json), [deployment health](framing10/deployment.json),
[forwarder cleanup](framing10/workload-forwarder-cleanup.json) and
[local-only evidence digests](framing10/local-artifacts.json).

### Preserved gas-fixture boundary failure

The broader run passed 19 additional dynamic-admission and 11 selected
settlement-policy cases, then exposed an outdated gas fixture on both networks.
The fixture expected a target with 180 fresh storage writes to be evicted under
a 5,000,000 gas cap. Its complete native post required 4,419,636 gas: the old 20%
margin would need 5,303,564, whereas the new 5% limit is 4,640,618. Both native
profiles correctly included it; the tests subsequently timed out waiting for
an eviction that was no longer required. The first failure logs/datadirs remain
under `broad-native/`, with `broad-native.log` and
`oversized-fixture-diagnosis.json` at the artifact root. This is a changed fixture
boundary, not evidence of a runtime cap violation or a retry-only pass.

The test-only correction increases the oversized target to 240 cold storage writes, whose
storage execution alone costs 5,304,000 gas. Whole-group splitting, the signed
5,000,000 cap, follower convergence, poison disposition, untouched oversized
storage and absence of manufactured historical urgency remain asserted. The
two affected native cases passed after this fixture correction; the deployed
production sources remain `e556e12`. See the [diagnosis](framing10/oversized-fixture-diagnosis.json),
[original results](framing10/broad-native-first-results.json),
[exact fixture correction](framing10/fixture-correction.json) and
[corrected results](framing10/fixture-native-results.json).

## Final validation and limits

| Check | Result |
| --- | --- |
| Composer unit suite | 278 passed; one existing manual performance test excluded |
| L1 unit suite | 144 passed |
| Selected native process cases | 42 distinct cases passed after correcting the two oversized fixtures |
| Workspace/all-target Clippy, warnings denied | Passed; repeated after the fixture correction |
| Rust formatting | Passed; repeated after the fixture correction |
| Chiado configuration | 14 passed |
| Mandatory policy propagation | Chiado and Kurtosis passed |
| Kurtosis tooling | 13 passed; one opt-in real relay-resource probe excluded |
| Live staged workload | 870 canonical sources; 770 semantic jobs passed; runner exit zero |
| Live estimate/receipt reconciliation | 23 postBatches and 520 inbound companions matched |

The native total combines nine focused cases, 31 successful cases from the
broader run, and the two corrected fixture cases. No unchanged passing cases
were rerun to hide the two original failures. Coverage includes all 23 dynamic
admission cases, all five settlement gas cases, both fee-cost/signing cases,
11 cross-chain settlement-policy cases and one middle-group partial-consumption
case. The exact commands, selected/excluded test names and evidence mapping are
in the [native inventory](framing10/native-validation.json),
[focused results](framing10/focused-native-results.json), and
[static/unit/configuration record](framing10/static-validation.json).
Executable identities are retained for the [focused](framing10/focused-native-executables.json),
[first broad](framing10/broad-native-first-executables.json) and
[corrected-fixture](framing10/fixture-native-executables.json) runs.

Twenty unchanged ordinary-only/history/restart settlement-policy cases were
excluded from this admission-focused run; their names are in the inventory.
The full CI artifact/Kurtosis matrix was not run locally, and this local image
was not promoted or published through CI. Protocol, ABI, codec, dependencies and
Solidity sources were unchanged, so their unrelated full suites were not rerun.
Native fixtures include independent follower replay; the entire live Chiado wave
was not independently replayed. Live timing comparisons remain subject to the
environment and inclusion differences described above. Small sanitized evidence
is committed here; bulk logs/datadirs and helper scripts remain local-only with
digests, and are unavailable from a fresh checkout.
