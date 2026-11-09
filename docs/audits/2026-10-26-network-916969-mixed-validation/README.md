# Network 916969 sustained mixed validation

| Field | Value |
| --- | --- |
| Date and result | 2026-10-26 UTC; complete workload and independent verification passed |
| Scope | Authorized live test of the deployed Chiado network; L2 traffic, cross-chain effects, fee accounting, L1 history and independent replay |
| Runtime/source | `9e3599abf2f1f08c4bcfff41f496f3365b5c4561`, unchanged throughout |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, initialized and clean |
| Networks | L1 10200; L2 916969 |
| Deployment provenance | [Runtime update and CI artifact verification](../2026-10-26-network-916969-runtime-update/README.md) |
| Procedure | [Sustained mixed-load runbook](../../runbooks/chiado-mixed-validation.md) |

The published Composer, signer and follower binary hashes were rechecked against
the previously verified release payload. All matched. The source, protocol and
immutable image digests are in [evidence.json](evidence.json). No production
binary, configuration, contract binding, genesis or database was replaced.
The signer had no configured L1 or L2 RPC source.

## Executed validation

The existing `scripts/run-chiado-mixed-validation.sh` ran unchanged from the
isolated implementation worktree. It sent 30 mixed waves, 30 seconds apart plus
submission time, alongside 5 ordinary L2 transactions/second for 900 seconds.
The two test actors were each funded with 0.15 test native units from the
separate workload account. The ordinary stream's fee ceiling was 50,000,000
wei/gas and its tip was 10,000,000 wei/gas. Both cross-chain fee directions
were enabled. Production timing remained P=2 / 250 ms; the genesis history
interval/delay remained 5 / 3 seconds. Those are separate settings.

All known local protocol-checkout wallet locks were held. A private prepared
profile copied the existing deployment bindings and selected workload account,
and retained the generated actor keys. Unused actor balances were retained;
there was no automatic recovery or funding retry. The existing host `/tmp`
quota required a private `TMPDIR` and the same scratch-directory redirection
used during deployment. Neither changed test assertions.

| Check | Live result |
| --- | --- |
| Full runner | `launch.exit`, `run.exit`, `live-mixed.exit` and `independent-verification.exit` all 0; no workload rerun or manual resubmission |
| Sustained ordinary traffic | 4,500 accepted and 4,500 canonical successful receipts; 900 seconds; zero skipped ticks |
| Ordinary settlement/DA | 72 covering canonical type-3 settlements; 72 full blobs retrieved; independent L1 event endpoints checked, including deferred companion execution |
| Ordinary replay | Separate L1/Beacon-only follower matched all 4,500 receipts and 668 containing/terminal blocks, nonce and balance; its blob reconstruction performs KZG verification |
| Cross-chain | 30 waves / 240 successful source receipts: 120 inbound, 120 outbound; 168 distinct source blocks checked against external L1 or the independent follower |
| Application semantics | Direct/wrapper setters, no-return calls, exact deposits and withdrawals checked on both destination observations; final setters 330/230 on L2 and 630/530 on L1 |
| Value delivery | Deposits totaled 0.00465; withdrawals totaled 0.002325 test native units; fresh recipient balances matched exactly |
| Quote/fee accounting | All 240 signed hashes, intents, nonces, gas limits and fee fields matched retained quotes; effective EIP-1559 prices and actor balance deltas matched exactly |
| Fee destinations | 300-block sample: exact FeeVault base-fee credit and beneficiary priority-fee credit across 1,167 paid transactions; 92 native transactions paid zero |
| History/PREVRANDAO | 300 blocks matched follower hashes, roots, ordered transactions and randomness; 60 mandatory first-position history transactions, including 10 empty repeats; 50 imported headers, 51 consecutive retained L1 hashes checked |
| History boundaries | Exact scheduled cutoffs, canonical calldata, import bounds, successful zero-price receipts, minimum delay, parent continuity and missing-hash reverts checked |
| Opcode probe | 300 read-only `eth_call` PREVRANDAO probes per node matched the selected L1 header and L2 mixHash; state override only, not new proven probe transactions |
| Availability during load | Public L1/L2 RPCs and fronts returned correct chain IDs; dashboard and explorers returned HTTP 200; this is routing/availability evidence, not a browser UI test |
| Final state | Queue empty; node/follower agreed at safe height 55,395; production processes and artifacts unchanged, zero restarts |

The first-inclusion observation p50/p95/max was 1.38 / 3.79 / 4.19 seconds over
4,365 samples. These are sampled provisional inclusion times, not settlement or
finality latency. Five transactions/second is the offered workload, not a measured
maximum capacity. There was no comparable baseline run in this task.

The history sample covered blocks 53,391–53,690. All adjacent L2 timestamps
were one second apart. L1 header timestamp gaps included 10 and 15 seconds;
selected-origin ages ranged from 3 to 17 seconds. This confirms the recorded
repeat behavior and canonical timestamp continuity in that sample, without
claiming a controlled L1 outage or uninterrupted wall-clock block delivery.

## Rejections, retries and observations

The captured run logs contained 36 bundle-drop warnings (19 `slot_skipped`,
17 `excluded`). All requested operations ultimately settled. The observer
recorded 33 movements of provisional outbound L2 receipts and zero observation
transport errors. These observations are sampled lower bounds. Canonical
verification ran after the workload and rechecked pinned blocks afterwards.

Pool-recovery warnings referenced 21 distinct cross-chain sources. Every source
was reconciled to its independently checked successful canonical receipt. Node
and follower logs also recorded database/execution lock waits. No ERROR-level
messages were found in the captured logs; the signer had no warnings or errors.
The wave runner's separate drop counter includes its complete retained node log,
so it is not the same scoped count as the 36 warnings above.

The supplemental fee-destination checker initially assumed a single beneficiary
for every block and failed before accounting. The sample actually contains nine
empty Sync blocks with zero beneficiary; they have no transactions and zero gas.
The corrected read-only checker requires every paid transaction to use the
configured beneficiary, checks both recipient balance deltas exactly, and checks
zero fees/zero balance change for the zero beneficiary. It passed. The original
checker, failure and corrected checker are retained separately; the primary
workload/checkers were never changed or rerun.

At the initial report, this metadata observation differed from the runbook's
statement that the configured beneficiary appears in every Live and Sync header.
The producer-path cause had not yet been established. No fee loss or replay
disagreement was observed. The follow-up below closes the cause investigation;
no runtime change was made to hide the observation.

### Empty Sync beneficiary follow-up — 2026-10-26

All nine headers were rechecked through read-only L2 RPC and still matched the
sample. Each hash was correlated with its original populated provisional block,
replacement commit and recovery event in the retained node log:

| Replacement cause | L2 heights | Producer path |
| --- | --- | --- |
| Observation deadline elapsed before canonical resolution | 53,475; 53,480; 53,530; 53,535 | `replace_timed_out_attempt` |
| Canonical reconciliation after bundle exclusion | 53,550; 53,555; 53,605; 53,610; 53,615 | `reconcile_attempt` |

At deployed revision `9e3599ab`, both paths explicitly pass `Address::ZERO` and
an empty transaction list to `build_sync_block` in
[`composer.rs`](https://github.com/eez-association/eez-rollupX/blob/9e3599abf2f1f08c4bcfff41f496f3365b5c4561/crates/eez-composer/src/composer.rs#L2456).
This is a producer choice in recovery, not a missing environment setting. Ordinary production and
`build_empty_slot_block` use the configured fee recipient. Git blame traces the
zero in canonical reconciliation to `4e9beb7366027ca2272f84f1a25e2c7cb673dbec`
(2026-08-07); it predates the current fee changes.

The first four events retain the original submission history and release work
for speculative replacement; a deadline does not prove exclusion. The other
five have `ExcludedSlot` reconciliation and retryable source outcomes. These
replace provisional L2 execution, not an already settled L2 endpoint. The
original workload's independent verification confirmed eventual successful
settlement of all submitted operations.

The replacements contain no transactions and use zero gas. The original fee
accounting established zero credit to the zero beneficiary and exact credits to
the configured recipient and FeeVault. The original follower replay matched all
300 sampled headers, including these nine. Variable beneficiary metadata is
published in V0 and checked against exact reconstruction by the
[signer profile](../../../crates/eez-proof-signer/SPEC.md#11-data-availability-and-sync-block-verification);
followers consume the published value rather than substituting an operator
default. Zero is not required by that profile. It changes the header hash even
for an empty block, so existing headers must not be rewritten.

[Sanitized per-block evidence](zero-beneficiary-evidence.json) records hashes,
recovery events, source locations and the retained log digest. The initial
follow-up documented the recovery exception. This follow-up used source/history inspection,
retained live-log correlation and nine canonical RPC header rechecks; it did not
rerun the workload, restart the follower or force a new recovery. No executable
or configuration was changed. The metadata/configuration inconsistency remains
in the deployed code; its origin and lack of fee loss in this sample are resolved.

### Recovery beneficiary correction — local validation

The subsequent source correction uses `self.inner.fee_recipient` in both recovery
calls to `build_sync_block`. Those are the only two production-code changes.
The guards preserving an already empty Sync and the recorded-beneficiary path
in `settle_retained` are unchanged. See the
[settlement policy](../../configurable-settlement-policy.md) for construction,
historical replay and rollback behavior. This changes producer policy, with no
hard fork, contract/genesis change, wire change or new validation exception.

The base is `9e3599abf2f1f08c4bcfff41f496f3365b5c4561` on the isolated
`codex/mixed-direction-fees` worktree; the protocol pin is unchanged. The code
correction is committed as `f3f996e2e5d7613c2c163136520251e5c90cb0c5`. The
[correction record](recovery-beneficiary-fix.json) identifies the code patch,
source files, debug binaries and local-only retained evidence. These results
are separate from the preceding live validation of the deployed revision.

| Check | Result |
| --- | --- |
| Baseline: exclusion and timeout regressions | Both failed at the intended beneficiary assertions: observed zero, expected configured `0x3e…3e`. The original Composer binary hash was verified before and after both cases. |
| Corrected source-based process regressions | 4 passed, none ignored: exclusion, skipped-slot timeout, preservation of an already empty historical boundary, and delayed original inclusion. Source recovery and independent follower checks remained enabled. |
| `cargo +1.95.0 test --locked -p eez-composer --lib` | 413 passed; the existing manual `held_pool::tests::profile_blocked_backlog_discovery` timing profile remained ignored and is not correctness coverage. |
| `cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings` | Passed |
| `cargo +1.95.0 fmt --all --check`; `git diff --check` | Passed |

The process command used `cargo +1.95.0 test --locked -p eez-node-e2e --test
node_e2e --` with the exact case names in the correction record and
`--exact --test-threads=1`. Prebuilt-runtime overrides were cleared, integration
contracts were built with Foundry 1.7.1, and logs/datadirs were retained under
`artifacts/sync-beneficiary-20261026/`. The first compilation with eight jobs
was killed by `SIGKILL` before any test ran; the next owned compilation was
interrupted to reduce concurrency. With one build job, both intended baseline
failures were reproduced and the corrected functional run passed on its first
execution. These build events are retained separately from test results.

No deployment occurred. Read-only inspection confirmed that production Composer
and signer kept their original images, process start times and restart counts.
Final-head CI, release-artifact qualification and deployment of the correction
remain pending; the live workload above does not validate a corrected release
binary.

## Commands, evidence and cleanup

Executed from the implementation worktree, with the private identity/settings
profile and explicit `KURTOSIS_ENCLAVE=eez-mixed-fees-20261026`:

```bash
bash scripts/run-chiado-mixed-validation.sh
python3 -m unittest discover -s scripts/ci/tests -p test_settlement_evidence.py -v
python3 -m unittest discover -s scripts/ci/tests -p test_chiado_mixed_validation.py -v
python3 artifacts/rog-network-validation-20261026/check-history.py
python3 artifacts/rog-network-validation-20261026/check-fee-distribution.py
python3 artifacts/rog-network-validation-20261026/check-fees.py
```

The two tooling suites passed 11 and 5 offline tests respectively; they supplement,
and do not replace, the live results. The three supplemental read-only checkers
and their exact hashes are retained with the evidence. Documentation validation:
`git diff --check`. The live operation used unchanged runtime code, so it needed
no Rust/Foundry rebuild or repetition of the already-qualified CI matrix. The
subsequent source correction has its own validation results above.

The follower resumed its own retained checkpoint, with separate writable data,
no sequencer RPC, no peers and no posting/signing keys. Its existing Compose
service was started using `up -d --no-build --pull never follower`, then stopped
with `stop -t 90 follower` after verification. It exited 0; its data and logs
were retained. Composer, signer, public ingress and other networks stayed running.

[evidence.json](evidence.json) contains compact sanitized results and SHA-256
fingerprints. Raw logs, exact quotes, receipts, signed intents, blobs and scripts
remain local-only under the implementation worktree's ignored
`artifacts/rog-network-validation-20261026/`. They are not available in another
checkout; private profiles and keys are excluded from committed evidence.

This validates the deployed revision under the stated load. It does not establish
maximum throughput, durable pending-queue recovery, backup restoration, forced
reorg behavior, prolonged L1/prover outage behavior, fresh-genesis follower
bootstrap, or finality of every recently checked L1 receipt. No intentional
restart, reorg or outage was injected into the shared network.
