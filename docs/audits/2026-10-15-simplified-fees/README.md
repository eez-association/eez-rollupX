# Simplified fee pricing and advisory availability

Status: implemented, tested and deployed on the existing ROG Chiado L2 on
2026-10-15. Public routes reopened at 20:02:43 UTC after live validation.
L1-origin fee collection remains unsupported. Dashboard source publication is
still deferred by the operator; its matching local image was deployed.
The accepted scope is in the
[compact quote proposal](../../proposals/compact-fee-quotes.md#simplified-pricing-and-availability-accepted-scope-2026-10-15).

## Identity and scope

Baseline: `83f12825d831d6ce60a8f967b6c1be89a0b6ce1e`, branch
`feat/compact-fee-quotes`, [draft PR #91](https://github.com/eez-association/eez-rollupX/pull/91).
The implementation is `5d07a8519114bd2588325f8ffcace7945445bcc8`; the deployed
backend/SDK revision is `b2a82536e87d2543ce9839eb3ba15a9ceefdaefc` after the SDK
expiry correction below. The UI revision is
`dcf4ac71b62ebc7bf5ac10cdb426e1f48ce11e00`.
Protocol `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` is unchanged and initialized.
The separate operator checkouts and existing configuration/state were preserved.

[result.json](result.json) records exact image IDs, executable hashes, test
counts, receipt amounts and evidence hashes. These are local development builds,
not CI-promoted release artifacts. Rust 1.95.0, the locked dependencies, the
existing release profile and Foundry 1.7.1 were retained. The Docker recipes
only received `CARGO_BUILD_JOBS=2` for host capacity; no dependency, feature,
protocol, contract or genesis change was introduced. Runtime executables were
extracted from the candidate images and used by the process tests. The harness
was built at `5d07a851`; its Rust source and manifests are identical to the
subsequent SDK-only revision. The UI contains the identical SDK module.

## Reproduced baseline and corrections

The new cost-floor regression failed on the original implementation:
`l2_tip_underfunded` rejected payment above configured costs when it missed the
target profit. This establishes the target/floor coupling independently of live
fee changes.

The deployed `a9313f9ad4eb54e79d628172381866accb3f7da7` runtime received three
read-only bursts of eight identical internally estimated type-2 requests through
its local Composer front. There were 3 successful quotes and 21 `quote_busy`
responses. No transaction was signed or submitted. The same request pattern on
the candidate returned 24 successful quotes without retries. This is bounded
first-attempt availability evidence, not maximum sustainable throughput; shared
host load and chain state were not controlled for a latency comparison.
[baseline.json](baseline.json) preserves the initial identities and counts.

The first unit invocation could not bind a listener inside the sandbox. Its
failure was retained and followed by the permitted loopback invocation. Initial
Clippy found a collapsible conditional, fixed without relaxing lint settings.
During integration preparation, the SDK still rejected expiry beyond eleven
seconds while the server issued thirty-second quotes. A regression reproduced
that mismatch; the SDK and dashboard copy were corrected before deployment.
The expiry regression now covers both envelopes, valid remaining lifetime,
expiry and excessive clock skew. These are recorded corrections, not hidden
retry-only test passes.

## Implemented behavior

`profit_bps` is a quote target. Signed admission, growing composed prefixes and
final native dispatch require measured posting/blob costs, configured service
costs and `risk_bps`. Debug/observation output exposes the floor separately from
the target. A target shortfall may coexist with covered costs. Ordinary L2
settlement remains subsidized.

Advisory estimates use a completed canonical L1/L2 scan pair and may include its
ordinary L2 history while another settlement is pending. Unresolved native
suffixes are excluded. The request keeps one snapshot for gas discovery, both
encodings and native replay. Reorgs, stale samples and unresolved nonce/code
dependencies still fail. Proof/submission targeting, native execution and queue
ownership remain authoritative. Quote execution and waiting capacity are bounded
separately from signed admission, including cancellation and shutdown.

The compact response contains `requestHash`, wallet fields and a thirty-second
expiry. Enforcement metadata remains in `eez_debugQuoteFee`. The SDK, UI and
manual consumers must use this contract; the retired API/response aliases were
not restored.

## Validation

All selections below were nonzero. The process/Kurtosis suites executed the
frozen candidate runtime and signer, and retained logs/datadirs.

| Check | Result |
| --- | --- |
| Composer / L1 / node unit tests | 353 / 189 / 88 passed; one pre-existing Composer timing profile ignored |
| Workspace formatting and locked Clippy, all targets | Passed |
| SDK | 6 tests passed |
| Chiado configuration / policy rendering | 18 tests passed; Chiado, Ethereum and Kurtosis render checks passed |
| UI image and mocked wallet browser scenarios | Build passed; 43 scenarios passed |
| `fee_quotes::` including Safe module matches | 11 passed |
| `settlement_policy_cost::` | 2 passed |
| `settlement_policy_gas::` | 8 passed |
| `dynamic_admission::existing_proxy_multicall_estimate_reaches_final_post_and_replays` | 1 passed |
| Kurtosis independent L1-to-L2 follower replay | 1 passed |

The new process cases settled a legacy source with 5% margin and a type-2
source with 1% margin below a 20% quote. They rejected underpayment, asserted
final cost coverage despite target shortfall, checked canonical posting receipts
and independent replay, and each required eight successful first-attempt quotes
while a source was `awaiting_evidence`. Existing tests cover nested execution,
ordinary L2 behavior, both encodings, fee-sensitive rejection and Safe creation;
recorded Falcon gas limits are parser coverage rather than actual Falcon
signature execution.

The isolated enclave `eez-simple-fees-20261015-5d07` used native blob DA and the
real builder/relay/consensus topology. Its fresh relay initially had zero
registered proposers; the exercise waited for canonical settlement. Once duties
were available, two independently funded nested sources settled in the same
native block. Underpayment was rejected before queue ownership or execution.
The inbound bridge deposit, type-2 and legacy withdrawals, and a browser-issued
paid call passed receipt and target-state checks. Withdrawal posting estimates
were 492835/182957 gas against 492823/182945 actual gas; their tip/cost ratios
were approximately 1.20/1.2004.

Five blob postings were authenticated for five outbound sources and the inbound
deposit. Physical transaction references for five source groups and nine L2
transactions agreed on both Composer fronts. A fresh independent follower
replayed the authenticated settlement evidence. The enclave was stopped after
capture; the unrelated pre-existing enclave was preserved.

Commands used the existing repository paths, including:

```bash
cargo +1.95.0 test --locked --config 'profile.dev.package.eez-node.opt-level=0' \
  -p eez-composer -p eez-node -p eez-l1 --lib
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.95.0 fmt --all --check
node sdk/l2-fees.test.mjs
```

The process harness used `EEZ_TEST_PREBUILT=1` with
`EEZ_TEST_RUNTIME_BIN_DIR` and `EEZ_TEST_PROOF_SIGNER_BIN` pointing to extracted
candidate executables, and selected the table's modules serially. Kurtosis
followed its [guide](../../../testing/kurtosis/README.md), using
`verify-l2-fees.mjs`, `verify-l2-bridge-fees.mjs`, the UI's
`tests/l2-fees-kurtosis.cjs`, semantic blob authentication and
`kurtosis_policy_replay:: --ignored --test-threads=1` with the prepared live
evidence. The full protocol staged/stress matrix and CI release matrix were not
rerun; no actual wallet-extension or Falcon-workload qualification is claimed.

## Live handover and receipt evidence

Both public transaction fronts were paused and both accepted queues were empty.
The node and signer exited cleanly. Complete stopped L1/L2 datadirs were copied
to new paths, retaining the original directories. The exact candidate pair and
matching UI image were installed without deploying contracts, regenerating
genesis, changing keys or changing the fee beneficiary. Configuration comparison
allowed only image IDs, new data paths and the reviewed margin allocation:
`risk_bps: 2000, profit_bps: 0` became `risk_bps: 0, profit_bps: 2000`.

The first local API verification failed because the L1 sample was stale. Logs
show the consensus client's execution hash already stopped advancing at
19:55:42 UTC, before the 19:56:12 handover. The embedded engine was at height
23537109 while the external RPC had reached 23537140. The quote correctly
reported `L1 fee sample is too old`. Lighthouse later reported a growing sync
distance. Restarting only that existing consensus container, with its image and
data preserved, restored sync. The engine then had a two-second-old head;
Beacon reported neither syncing nor optimistic nor execution-offline state.
The original failed verification was retained, and the same API check passed
after this operational recovery. No freshness safeguard was weakened.

The live nested legacy canary rejected a zero-tip source without queue ownership,
then settled a source deliberately priced at 5% above the debug cost floor:

- L2 source: `0x9ba378f5e90614cd737ac38141e9c1abe58f5ce2d7fb10ad53e00fc9a296b685`.
- Native-blob L1 post: `0xe4c97ca548a945892c8dfcedc898bec30b39bb7c97c3a7dedf129f17ff218d36`.
- Collected L2 tip: `320017951284263` base units.
- Canonical L1 posting plus blob cost: `304767001215865` base units.
- Posting surplus: approximately 5%, below the quoted 20% target.

This comparison uses the retained 1:1 conversion assumption. Proving,
operations and treasury fixed costs remain configured as zero; it does not
establish realized treasury profit or redemption. The canary checked actual
L1 target state, L2 call count, canonical receipts, complete transaction references
on both fronts and fee coverage. A previously recorded canonical L2 receipt was
preserved unchanged. The old L1 receipt was already unavailable from the embedded
RPC before the upgrade and is not claimed as a preservation check.

Local and public API checks passed for compact/debug separation, both types,
internal estimation, explicit 16777216 gas, ordinary-L2 zero tariff, removed old
methods and L1's explicit unsupported direction. Public dashboard HTML matches
the tested image and fee quoting is enabled. Public routes were restored only
after those local health/canary/history checks; public API verification followed.

## L1 collection boundary and remaining limits

The existing submitter has no requested-refund fields or receipt-backed recovery
qualification for the deployed Chiado builder. The official
[Titan API](https://docs.titanbuilder.xyz/api/eth_sendbundle) and
[Beaverbuild API](https://beaverbuild.org/docs.html), rechecked on 2026-10-15,
document selected-source percentage refunds with a separate recipient. Their
deductions and selection rules require provider-specific verification; docs do
not establish a successful native-blob payout on this deployment. Prior
[research](../../proposals/cross-chain-fee-recovery.md#common-funded-single-source-profile)
already distinguishes a funded user refund from returning the poster's own tip.

L1 `eez_quoteFee` remains explicitly unsupported until that collection path is
implemented and qualified. L1 gas estimation and existing subsidized inbound
execution remain available, including the validated Kurtosis deposit. A nominal
quote alone would neither recover the user's L1 tip nor satisfy the cost floor.

No durable queue recovery, treasury redemption, majority-builder refund
qualification or general production-capacity claim is added. Pending requests
remain in memory. The floor uses refreshed simulation and configured economic
assumptions; subsequent price/state changes may still defer work. Rollback
restores the old hard-target behavior and must account for accepted work, as
specified in the [runbook](../../runbooks/chiado-deployment.md).

Detailed commands, image builds, browser/process logs and retained data are
local-only under ignored `artifacts/simplified-fees-20261015/` and the recorded
private operation directory. They are unavailable to another checkout. The
small committed JSON records preserve sanitized identities, results and hashes;
they do not substitute for the full local evidence or validate a newer binary.
