# Candidate framing reserves and the 5% default

| Field | Value |
| --- | --- |
| Status | Deployed; live measurement and broader regressions in progress |
| Source baseline | `a4f09f17e777590a7a599ddb675cfd894449257d`; branch `feat/cross-chain-admission-throughput` |
| Candidate source | `57266aec768f8d604cf45a4eca0aedff500d33fb` (runtime implementation `bee97db`) |
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
