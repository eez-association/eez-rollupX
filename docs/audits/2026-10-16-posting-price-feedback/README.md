# Posting-price feedback correction and funding diagnostics

Status: implemented, qualified and deployed. The exact release images passed the
pre-deployment gates and live Chiado protocol/Falcon/replay checks. Deployment is authorized only after the applicable checks pass.

| Field | Value |
| --- | --- |
| Source baseline | `eez-association/eez-rollupX`, `17fe00fa9420ad3b9ddec9e003a9a9a6e35f6e04`, fee-feature checkout |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized and unchanged |
| Release candidate | `aa8d8a5894c69502c67dcae35d5184bc9a79b036` |
| Prior deployed source | `be8477bc7ba48d3f287d33cd6c010089458885ec` |
| Contract | [Authorized proposal](../../proposals/cross-chain-fee-recovery.md#posting-price-feedback-correction-authorized-on-2026-10-16), [pricing and diagnostics](../../configurable-settlement-policy.md#bounded-next-block-priority-estimation) |
| Historical evidence | [HackMD incident review](../2026-10-16-hackmd-fee-review/README.md) |
| Tools | Rust 1.95.0, Foundry 1.7.1; locked dependencies |

## Correction

With L1 refund recovery enabled, dynamic posting prices exclude the configured
poster's payments and transactions that call the configured registry. The
sampler retains unrelated transactions in the same blocks, uses actual charged
gas for the per-block percentile, and preserves the existing 20-block selection,
relay floor and independent ceiling. The 20% quote target and final native cost
floor are unchanged. Without refund recovery, and with an explicit priority
override, the previous respective paths remain in use.

Receipt events alone were insufficient: two successful historical sources caught
reverts and therefore had no registry logs. The regression initially failed with
receipt-only filtering. Canonical `callTracer` frames cover these cases, including
reverted frames. Hash-bound receipt and trace coverage is checked before caching;
missing data cannot silently restore unfiltered pricing. A bounded 64-block cache
shares observations across quotes/posting and needs no persistence or queue journal.

Funding rejection/status diagnostics now separate source gas-price cap failures,
measured settlement-payment shortfalls and unavailable settlement pricing. They
include required/offered payment, shortfall, payment chain, source gas/base fee,
posting prices and both sampled chain references. Prefix deferrals explicitly
identify their aggregate scope. Successful compact quote responses are unchanged.
This does not alter ownership, nonce dependencies, retry budgets or eviction.

## Reproduced baseline and regression inputs

The checked-in [sanitized receipt/trace fixture](../../../crates/eez-composer/src/composer/posting/priority-fixture.json)
contains canonical Chiado blocks 23544818–23544820, with calldata/proof payloads
removed. Expected raw rewards were independently retained from `eth_feeHistory`:
3,316,607,925; 4,460,534,753; and 4,276,213,674 wei. The earlier 17 rewards in the
same historical window remain unchanged in the comparison, conservatively
including the two 1-gwei samples.

The original estimator produces a 2-gwei clamped post bid. Filtering the sixteen
service sources and three posts produces a 1-gwei clamped bid. A separate mixed
block regression retains unrelated 2/3-gwei competition and verifies gas-weighted
selection. Mocked RPC tests cover incomplete receipts/traces, hash/order/pricing
mismatches, same-height replacement, cache bounds and reconstruction after restart.

Historical trace reads used the configured local L1 RPC with a block hash and
`callTracer`. They returned complete coverage (10, 9 and 6 transactions). These
reads establish availability for those blocks, not an RPC latency guarantee.

## Validation record

Working directory: the existing isolated fee-feature checkout. Build artifacts
use the operator's existing Cargo target directory; no dependency/pin upgrades.

| Check | Result |
| --- | --- |
| Composer, L1 and node library suites | 375 + 193 + 90 passed; one manual profiling test explicitly ignored |
| SDK `node sdk/l2-fees.test.mjs` | 6 passed |
| Integration `forge test --root contracts` | 86 passed; no skipped tests |
| Rust/Foundry formatting | Passed after correcting pre-existing Foundry formatting in two integration files |
| Workspace Clippy | Passed with warnings denied, including the queue metric labels |
| Native `fee_quotes::` suite | All 16 passed, including exact-quote caught revert and an actual external posting-price increase |
| Queue/policy process regressions | 3 `queue_liveness::` and 2 `settlement_policy_cost::` tests passed |
| Exact Docker release regressions | Both new process tests passed using the extracted shipping Composer/signer |
| Exact-image Kurtosis | Both fee directions; 12 L1 sources (legacy/type-2 nested/Safe and two shared batches), two L2 sources, native blob authentication and one independent genesis replay passed |
| Drained Chiado upgrade | Qualified images running; canonical safe hash/state root, both queues, runtime environment and public HTTP routes verified |
| Live protocol semantic canaries | All 34 enabled scenarios passed, with 36 canonical sources; six pinned exclusions retained |
| Live Falcon Safe canaries | Four creations and eight transfers, six legacy/six EIP-1559, plus three negative authorization checks passed; setup/report corrections retained below |
| Live native blob and independent replay | All 17 protocol/Falcon posts authenticated; follower matched 50 L2 receipts, four state calls and final safe hash/state root |
| Post-load public APIs | HTTP/WS routing and both compact quote formats/debug diagnostics in both directions passed; queue empty, runtime identity unchanged |

Kurtosis recorded 78 filtered posting-price samples, excluding up to 20 service
transactions per sampled window while keeping the 1-gwei bid. The initial
ordinary post was excluded several times while the relay reported no bid; a
canonical post was then included at L1 block 34 without code/configuration
changes. No test assertion timed out. This is retained as operational evidence,
not a deterministic startup-latency claim. Blobscan was explicitly disabled;
native KZG/blob verification and independent follower replay did run. Only this
task's enclave was stopped after qualification; its data and logs were retained.

The first receipt-only historical regression failed and was corrected by adding
canonical traces. Initial compilation/test-fixture errors were corrected before
the successful unit run. A sandboxed unit invocation could not bind mock RPC
sockets: 22 L1 tests failed with `Operation not permitted`; the same suite passed
with local socket access. This was an environment restriction, not a retry-only
behavioral pass. First failures are retained alongside the later results.

The initial workspace Clippy run identified an oversized sampler future; boxing
that future at its existing async boundary resolved the propagated warnings.
Two source process-test builds exceeded the shared host's available memory. The
source test runtime therefore uses one compiler job, disabled incremental
compilation and `profile.dev.package.eez-node.opt-level=0`. Shipping Docker builds
retain the existing release profile and only cap compiler concurrency.

The new process fixtures also exposed assumptions requiring correction. An
enclosing revert restores the L2 lookup cursor, so the caught-revert fixture must
retry the same entry, as required by the pinned protocol's accepted support
limits. Its unrelated unquoted L1 companion is omitted when refund enforcement
is enabled; priced L1 roundtrips remain separate coverage. The market-increase
fixture now has zero fixed operating allowances: the generic fixture's large
allowances otherwise leave enough margin to absorb the increase, correctly
allowing the old payment. These are fixture corrections, not relaxed admission
or protocol checks. First failed runs remain part of the evidence.

The initial Foundry formatting invocation failed on `ECDSAProofSystem.sol` and
`DeployRollup.s.sol`; its status was initially obscured by a later successful
command. Both files received formatting-only corrections and the full check and
86-test integration suite were repeated successfully. Formatting can change
metadata in future deployments; existing Chiado contracts/genesis are preserved.

Raw RPC extracts and source test logs are local-only under
`artifacts/posting-price-feedback/` in the fee-feature checkout. Image, Kurtosis
and deployment evidence is under
`artifacts/posting-price-feedback-20261016.c0c17e86/` in the operator checkout; the
directory suffix names the first implementation commit, not the final runtime. They are ignored,
are not available in another checkout and have no guaranteed retention. Sanitized
fixtures and the final result manifest accompany this record. The local-only
file digests identify retained evidence but do not make those files available to
another checkout.

## Reproduction and retained commands

Source checks used Rust 1.95.0 and locked dependencies:

```bash
cargo +1.95.0 test --locked -p eez-composer -p eez-l1 -p eez-node --lib
cargo +1.95.0 fmt --all --check
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
node sdk/l2-fees.test.mjs
forge fmt --check --root contracts
forge test --root contracts
```

The source process runtime was built with one compiler job, incremental disabled
and `profile.dev.package.eez-node.opt-level=0` after the two retained OOM failures.
The archived final `node_e2e` binary ran `fee_quotes::`, `queue_liveness::` and
`settlement_policy_cost::` serially with its explicitly identified Composer,
signer and follower binaries. The shipping image check used the same test harness
and `EEZ_TEST_PREBUILT=1`, pointing `EEZ_TEST_RUNTIME_BIN_DIR` and
`EEZ_TEST_PROOF_SIGNER_BIN` at executables extracted from the exact Docker images:

```bash
"$EEZ_TEST_RUNTIME_BIN_DIR/node_e2e" \
  fee_quotes::exact_quote_with_caught_nested_revert_is_admitted_and_replayed \
  fee_quotes::external_posting_price_shift_rejects_old_quote_with_measured_shortfall \
  --test-threads=1
```

The task-owned enclave used `KURTOSIS_ENCLAVE=eez-posting-price-20261016`.
[The repository Kurtosis procedures](../../../testing/kurtosis/README.md) ran
`verify-eezl2-deployment.sh`, `verify-l1-fees.mjs`, `verify-l2-fees.mjs`,
`verify-blob-da.sh` and `kurtosis_policy_replay::` with the exact image executables.
The replay test had one selected ignored-by-default live test, which executed and
passed; it was not an ignored-only result. Raw logs, the selected args file,
source archive manifests, executable hashes and local one-time handover/canary
harnesses remain in the local-only artifact directory identified above. The blob
helper retains its original source identity `ce6574788682c54bdd3f9742870f5dbdbd9a4a9c`;
its transitive compiler inputs were checked unchanged at the candidate. Its hash
and the final source-built test harness hash are included in the manifest.

## Boundaries

Quotes remain advisory. Genuine external competition, changed execution or source
base fees can still invalidate an old signed payment. The new diagnostics explain
that rejection; they do not reserve a price or subsidize an underfunded L2 source.
The existing explicitly accepted largest-payer L1 batch subsidy remains unchanged.
No contract/genesis/database/signing-wire migration is required. Older binaries
restore unfiltered pricing, so rollback can defer a lower-priced outstanding
source. Accepted pending requests remain in memory and must be drained before
the authorized handover or explicitly reconciled/resubmitted.

## Deployment record

The runtime is built from `aa8d8a5894c69502c67dcae35d5184bc9a79b036`, with
the exact image and executable identities in [the evidence manifest](evidence.json).
Docker builds used an archived exact source tree and recursively pinned
submodules, with one compiler job and the unchanged locked release profile. The
node, signer and deploy images were qualified locally; no public deployment tags
were promoted. Later audit-only commits do not change the running binaries.

Public HTTP/WebSocket ingress was paused before the handover. Both queues drained
and the pre-handover L2 watermark became canonical before either writer stopped.
Both exited cleanly without OOM; complete stopped L1/L2 copies were made and the
original databases retained. The candidate runs from separate copies. The previous
safe hash/state root, all runtime environment values, genesis and deployment
bindings were checked unchanged before reopening the four HTTP RPC routes.

The [measured rejection example](market-shift-diagnostic.json) comes from the
external-price-shift process regression. It reports a required payment of
1,440,970,950,168,840 wei, offered payment of 881,420,384,810,214 wei and shortfall
of 559,550,565,358,626 wei after unrelated L1 traffic raises the posting bid from
1 to 2 gwei. This is controlled regression evidence, not a live user rejection.

The live protocol run prepared all 34 enabled scenarios, accepted all 36 sources
at their exact quoted fees with no submission errors, and passed every semantic
verifier. No source replacement or same-byte setup rebroadcast was needed. Its
12 posts were independently authenticated through native blob evidence. The six
pinned `all` exclusions remain exclusions; the earlier 770-job/870-source stress
wave was not repeated for this candidate. Preparation actually used the profile's
`PREPARE_PARALLEL=200` (34 jobs available), with four source workers; the profile
value overrides the outer harness's attempted preparation cap of eight.

The first Falcon invocation failed its funding preflight before recording a run
start, deploying controllers or signing any workload source. The existing test
funder had approximately 0.007754 L2 tokens, below the 0.025-token preflight floor
for four actors. A quoted L1-to-L2 bridge topped up only the measured shortfall
(0.020245959922671676 tokens) to a 0.028-token target under both workload wallet
locks. Its canonical balance was verified before starting the Falcon suite again.
The initial failure log remains separate; this is an explicit funding correction,
not an unexplained retry-only pass.

The second Falcon invocation executed and verified all four creations and eight
transfers, including actual browser Falcon signatures, exact execution/Safe
events, ownership/module/guard configuration, nonces and recipient values. Gas
used ranged from 5,453,051 to 5,999,402. All twelve quotes succeeded on their first
attempt, with six transactions of each format. The three negative checks reached
the intended authorization errors for nonce replay, unauthorized Safe access and
changed signed calldata.

Its final report still asserted the earlier 16-actor workload's 48-source count,
although this run explicitly configured four actors. That assertion failed after
all semantic, authorization and cost checks had completed. The harness assertion
was corrected to `N * 3`; a read-only recheck reran fresh canonical receipt,
ownership, Safe/controller state, public L1 receipt and whole-post cost checks
against the same twelve sources. It also required one creation/two transfers per
actor, unique and complete signed/settled sets and zero per-wave verification
errors. No transaction was resent. Both original failure records and the corrected
report are retained, so the invocation is not represented as an uninterrupted pass.

All five Falcon posts covered actual execution/blob costs. Across the protocol
run, all L2-funded posts covered those costs and four shared L1 posts were
undercovered under the previously accepted largest-payer subsidy policy. Exact
base-unit totals are in the evidence manifest. These figures exclude proving,
operations, treasury and ordinary setup costs; they do not establish net profit
or an exact realized 20% margin.

Independent L1-only follower replay from the retained pre-upgrade checkpoint
matched the final safe hash/state root, 50 canonical L2 receipts and four Falcon
controller nonce calls. This is checkpoint replay, not a fresh Chiado genesis
replay. The separate Kurtosis check did replay from genesis. All 17 live protocol
and Falcon posts were authenticated with the unchanged, hash-verified blob helper.
The final queues were empty; exact runtime identities were unchanged; both public
HTTP/WebSocket chains and compact/debug fee routes passed.

The live Composer recorded 201 filtered pricing samples, excluding up to 43
service transactions in a sampled 20-block window. Every recorded posting bid
remained at 1 gwei; no funding rejection occurred during this canary. The 20%
quote target and final cost/risk floor were preserved. This is finite-workload
correctness evidence, not a sustainable throughput or cold-cache latency claim.
The full CI matrix, additional builders and the previous 770-job stress wave were
not rerun for this bounded correction. No durable queue/recovery journal or
contract/genesis migration was introduced.
