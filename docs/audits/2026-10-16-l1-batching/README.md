# L1 multi-source batching and observation-only refunds

Status: implemented, deployed and validated. All 25 selected process regressions
passed. The run label uses the local calendar date; execution timestamps below
are UTC on 2026-10-15.

The operator accepts a shared L1 batch losing money when its largest user refund
cannot cover the post. Refund failures must be monitored without pausing quotes
or settlement. See the [authorized scope](../../proposals/cross-chain-fee-recovery.md#l1-batching-and-observation-only-recovery-authorized-on-2026-10-16)
and [current contract](../../configurable-settlement-policy.md#l1-builder-refund-profile).

## Identity and behavior

| Component | Revision |
| --- | --- |
| Fork / branch | `eez-association/eez-rollupX`, `feat/compact-fee-quotes` |
| Baseline runtime | `1a5a954889b8dbd9fe6ea55aac9791df1464b5b3` |
| Candidate runtime | `ce6574788682c54bdd3f9742870f5dbdbd9a4a9c` |
| Process harness | `c695ce62270881dc7351e8153ea8ff6373f22c7b` |
| Protocol, initialized and unchanged | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` |

[result.json](result.json) records Docker image identities, extracted executable
hashes, transaction hashes and sanitized results. These are local development
images, not CI-promoted artifacts. Subsequent changes only correct the probe's
continuation criterion and document evidence; runtime bytes remain unchanged.
The separate operator checkout and its preexisting edits were preserved.

The scheduler packs multiple complete L1 source groups under the existing native
gas/blob/time bounds. Source ordering, nonce dependencies and separate L1/L2
collection remain. Selection uses the largest external total priority payment,
computed from final simulated receipt gas and the capped effective tip. The
selected payer is mandatory. Other users still pay their signed gas fees.
The final L1 funding floor is observed rather than used to cap or reject a shared
batch. Standalone signed admission and quotes retain their cost/risk and profit
policies; L2 enforcement is unchanged.

Missing, ambiguous or insufficient refunds no longer gate quotes or dispatch.
Unique canonical payment attribution remains required for revenue recognition.
Prometheus/Grafana expose received refunds, posting costs, shortfalls, payment
statuses and observation errors. Duplicate reads are idempotent; reorgs retract
contributions. Finalization releases tracking without erasing accumulated totals.
Restart resets those totals and can lose pending attribution. Exact logs and
canonical receipts remain necessary for historical financial reconciliation.
See [monitoring semantics](../../../monitoring/README.md#l1-refund-observations).

## Validation

The baseline runtime with the new selection assertion failed in the actual payer
selector with `l1_tip_underfunded`. That assertion passes with the candidate.
This compares runtime behavior, not an unrelated mock or a zero-test selection.

| Check | Result |
| --- | --- |
| Composer unit suite | 358 passed, one preexisting ignored test |
| L1 unit suite | 193 passed |
| Node unit suite | 88 passed |
| Locked workspace Clippy, all targets; workspace formatting | Passed |
| SDK | Six passed |
| Chiado configuration | 19 passed |
| Settlement configuration checks | Passed |
| Grafana JSON, Prometheus allowlist and `promtool` | Passed |
| Integration fixture build | Passed |
| Exact Docker binaries: fee/Safe process tests | 13 passed |
| Exact Docker binaries: gas, cost, capacity, partial-consumption regressions | 12 passed (8 gas, 2 cost, 1 capacity, 1 partial consumption) |
| Kurtosis actual-builder exercise | Four individual canaries and two four-user shared batches verified |
| Native blob authentication and correlation | Passed; optional Blobscan excluded |
| Fresh independent Kurtosis follower | Passed; 36 explicitly checked blocks and three exercised receipts |
| Live Chiado executions and independent follower | Verified, with the original probe failure below retained |

The process tests deliberately use a development adapter that ignores bundle
ordering and pays no refund. Both transaction formats demonstrate a subsequent
quote, signed admission and refund-bearing dispatch after the missing payment.
Independent replay checks the canonical effects of that negative case. The
actual-builder tests below establish successful ordering and real payouts.

The runtime executables were extracted from the candidate images. Their hashes
were frozen before the process runs. Only the Composer executable changed; the
follower, signer and genesis helpers are byte-identical to the preceding deployed
build. The process harness was compiled separately and does not rebuild runtime
executables during tests.

Representative commands, run from the candidate checkout:

```bash
export CARGO_TARGET_DIR=/home/edu/Development/rollupX/target
export CARGO_BUILD_JOBS=1
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
export PATH="$LLVM_SYS_221_PREFIX/bin:$PATH"
cargo +1.95.0 test --locked -p eez-composer -p eez-node -p eez-l1 --lib
cargo +1.95.0 fmt --all --check
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
node sdk/l2-fees.test.mjs
forge build --root contracts
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e --no-run
```

The resulting harness was copied to the evidence directory and run serially
with `EEZ_TEST_PREBUILT=1`, `EEZ_TEST_RUNTIME_BIN_DIR` and
`EEZ_TEST_PROOF_SIGNER_BIN` selecting the extracted image executables. Explicit
filters and nonzero counts are retained in `result.json`. Logs and datadirs were
retained through `EEZ_TEST_LOG_DIR` and `EEZ_TEST_DATADIR_DIR`.

### Actual-builder evidence

The owned enclave was `eez-l1-batching-20261016`, with private chain IDs
7331/696990, 12-second L1 slots, automatic request admission, 20% quoted profit,
zero configured risk/service costs and a 90% refund. Its pinned builder was
`ethpandaops/reth-rbuilder@sha256:916ba84da2690e3e368f57da5b0c73bfd919b452de8b95f64895846e0f3a8308`.

Nested calls and standard Safe 1.4.1 creation passed in both transaction formats.
Four independently funded senders then submitted two mixed legacy/EIP-1559 waves.
Each wave used one native post, received exactly the refund from its largest
measured payer, and verified all nested effects. Both happened to lose money:

| Private batch | Refund (wei) | Post execution + blobs (wei) | Shortfall (wei) |
| --- | ---: | ---: | ---: |
| 1 | 517009664628151 | 1841050964195912 | 1324041299567761 |
| 2 | 462331882159212 | 684502542360924 | 222170660201712 |

Both were canonically observed as underfunded. The second batch and a subsequent
quote succeeded after the first shortfall. These are accepted batch subsidies,
not evidence of builder underpayment. Native blob authentication and fresh
independent replay passed. The enclave was dumped and stopped with its state
retained; unrelated enclaves were untouched.

### Live handover and receipts

The old Composer initially had accepted work in memory and was deferring some
posts with `l1_tip_underfunded`. Public transaction ingress entered maintenance
while it drained. Both fronts reported zero queued, in-flight, blocked and
unknown requests before stopping. No pending requests were discarded by this
upgrade. Both writers exited cleanly. Complete stopped databases were retained,
and the candidate started on separate copies with the original contracts,
genesis, keys and configuration.

Eight nested transactions then settled in two four-user posts. The selected
payer, exact builder payment, successful source receipts, canonical ownership,
L1 call counts and historical L2 values were independently checked:

| Live batch | Refund (wei) | Post execution + blobs (wei) | Surplus / loss (wei) |
| --- | ---: | ---: | ---: |
| 1 | 758923202047092 | 1038710003176733 | -279786801129641 |
| 2 | 619320001474276 | 473088002394112 | 146231999080164 |

The original live probe **failed** because it required both batches to lose
money. The second was profitable as posting fees changed. The first failure,
script and partial evidence were preserved. A separate **read-only** verification
completed checks against those same canonical transactions, verified the first
loss and continued service, and checked a subsequent quote. Nothing was
rebroadcast. The original probe is not reported as a passing test.

The reusable probe was corrected to require a loss in the first batch and
successful continuation afterward; the next batch may be profitable. Its signed
surplus and nonnegative shortfall fields reflect this distinction. Syntax and
both recorded private/live outcomes were checked. The corrected probe was not
rebroadcast; the stronger original private-network assertions had already passed.

A separate follower used the stopped pre-upgrade checkpoint and canonical L1
data only. It matched the live endpoint/state root, final nested value and both
existing Safe contracts' owner/threshold state. This increment did not create
new Safes on live Chiado; fresh creation was exercised in Kurtosis and process
tests.

Public access was verified restored at **23:28:05 UTC on 2026-10-15**. All four HTTP RPC paths
returned the expected chain IDs, both WebSocket paths upgraded successfully, and
both Composer fronts returned legacy and EIP-1559 compact quotes. The L2 public
probe was an ordinary read; enforced cross-chain L2 behavior is covered by the
process suite. Grafana was provisioned and healthy. Prometheus reported exactly
one underfunded and one qualified payment, with amounts matching the receipts.
The wallet dashboard image was unchanged and its source was not published.

## First failures and limits

The first host command selected unavailable `mold`; the default host linker
resolved that setup failure. A host process-harness compilation then received
SIGKILL while the Docker release build ran concurrently, before tests ran. Its
logs were preserved and the host build completed serially. A node/L1-only unit
invocation was interrupted before tests to use the established combined unit
feature set. None is counted as test coverage. The live probe's failed economic
assumption is recorded above rather than hidden by a retry.

No majority-builder coverage, maximum-throughput claim, Falcon workload, full
protocol stress/CI release matrix, real wallet-extension run or durable financial
recovery is established here. The monitoring gauges reset on process restart;
Prometheus history is not a durable queue or complete financial journal.

Large logs, stopped network state and private deployment snapshots are local-only
under `artifacts/l1-batching-20261016` and the operator's deployment directory.
They are not available from a fresh clone. Small sanitized identities, outcomes,
transaction references and first-failure information are retained in
[result.json](result.json).
