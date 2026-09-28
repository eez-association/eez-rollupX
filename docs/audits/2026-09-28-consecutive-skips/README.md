# Composer recovery across consecutive skipped slots

| Field | Value |
| --- | --- |
| Status | Deployed; five-minute semantic checks passed; 83.3% Grafana inclusion; new-head CI pending |
| Authorization | September 28, 2026: user requested fixing the inclusion failures under our control |
| Fork / branch / PR | `eduadiez/rollupX`, `feat/cross-chain-admission-throughput`, PR #42 |
| Baseline PR head | `c8260cec8e76a267c96cea1313d31b55a8383f75` |
| PR base | `fd77a2ebc147850333171045dbd263ec74156268` |
| Protocol | `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged |
| Contract | [Replacement design](../../proposals/skipped-slot-speculative-retry.md), [RPC ownership](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission) |
| Live reproduction | Five-minute Chiado run, preserved in the [PR #42 description](https://github.com/eduadiez/rollupX/pull/42); detailed local report in `docs/audits/2026-09-28-chiado-five-minute-spam/` |

The previous scheduler exhausted its one-replacement allowance after two missing
slots, leaving produced targets 23272510 and 23272544 without attempts. Admission
had not rejected those targets; no trigger was emitted. Four other submitted
misses followed skipped slots and reached the relay without an eligible bid at
the proposer request. Captured builder logs show the previous slot's build
timeout at -42.736, +515.203, +486.178 and +259.511 ms relative to those targets.
Contention remains a hypothesis; this repository does not identify the deployed
builder source/configuration, so no remote builder change is claimed.

The authorized extension uses four bounded replacements on an unchanged parent,
shared by scheduler and submission gates. Each advances one slot, retains all
submission history, preserves one current owner and reuses the poster nonce.
Canonical derivation and exact-parent checks remain mandatory. The five-target
bound still suspends work during a sufficiently long outage. A structured event
reports exhaustion without declaring settlement failure.

## Validation record

Evidence is retained locally under `artifacts/consecutive-skips-20260928/`.
It is ignored and unavailable in another checkout unless explicitly copied.

- Initial new-test compilation failed because a parameterized assertion also
  touched an unrelated historical-prefix test. Corrected that test edit before
  baseline execution; retained `baseline-compile-initial.log`.
- Baseline process regression selected one case and failed as expected: no
  dispatch for the required third target after two skipped slots, timing out at
  17 seconds. Source `c8260ce`; [baseline executable identity](evidence/baseline-identity.json).
  Logs/datadirs remain local under `baseline/`, with output in `baseline.log`.
- Affected unit suites passed: 292 Composer (one manual profile ignored),
  60 driver and 145 L1 tests. This includes invalid/overflow target checks,
  exhaustion/reset and four superseded generations followed by partial canonical
  consumption and reorg. These unit results do not establish process/live behavior.
- The first candidate process run reached both replacement dispatches, but failed
  the exact final L2-height assertion (115 versus 120). The controlled relay had
  forwarded the first replacement into the native EL pool, which does not enforce
  relay timestamp pins; it landed in the later produced slot. That run remains a
  failure (`candidate-two-skips.log` and `candidate/` diagnostics). The skipped-slot
  fixture now discards every intentionally skipped target; delayed-original
  inclusion remains a separate scenario. The corrected fixture was repeated against
  the previously deployed `ca37074` release binaries and failed as expected with
  `no next-slot replacement dispatch` (one selected, no ignored tests). Exact
  [baseline release hashes](evidence/baseline-release-identity.json) and diagnostics
  are retained in `baseline-corrected/`; the same frozen regression passed against
  the candidate release.
- Tooling: 275 tests, 4 opt-in Docker/Kurtosis checks excluded; no failures.
- Formatting and workspace Clippy (`--locked --workspace --all-targets -- -D warnings`)
  passed.
- [Candidate local release-image identity](evidence/candidate-identity.json):
  source `ba4cbabca056302875715e9e060e7b803c5c282c`, built from an immutable Git
  archive plus pinned submodules. The production feature/profile recipe matches
  the old local release. Frozen executables extracted from these images are used
  for candidate process checks. Local builds are not CI-published artifacts.
- Exact-image two-skip regression passed (56 seconds). The first fourth-replacement
  delayed-original case reached all four replacements and canonically recovered the
  original, then failed an old assertion equating the ownership reconciliation
  snapshot with inclusion height (10 versus 9). `OptimisticallyIncluded::resolve`
  records `as_of`, so that equality is not the RPC contract. The corrected test
  independently checks the canonical observation hash and exact inclusion through
  the settlement correlation. Original diagnostics remain in `candidate-regressions/`;
  corrected execution passed. Eight final targeted cases passed with no ignored
  cases: two consecutive/fourth-replacement regressions, the three existing
  skipped-slot/historical-prefix regressions, partial consumption, restart after
  partial consumption and native L1 reorg recovery. Independent follower parity
  is asserted in the process scenarios. See [per-case results](evidence/process-results.json).
  The first failures above remain failures, with documented test corrections.
- Full workspace Clippy and formatting passed again after the assertion change.
  Live semantic checks passed as recorded below; new-head CI is pending.
  Prior-head green CI does not validate this runtime. Full settlement/cost/admission suite coverage
  beyond the focused cases, the release-artifact matrix and Kurtosis remain CI gates.
- The user confirmed the shared rbuilder is not operated by us. Its timing issue
  remains externally controlled; no builder change or mitigation is claimed.

Existing operator runbook edits, recovery audits and deployment state are preserved.
The protocol, dependencies, ABI, follower replay and signer profile are unchanged.
Older Composers retain the one-replacement policy; an upgrade changes only future
Composer scheduling. Restart still has the documented in-memory queue limitations.

## Chiado handover

The tested local images were started on cold copies of the existing L1/L2 data.
Only image references and execution data paths changed; original databases,
configuration, bindings, genesis, JWT and posting log/checkpoint storage remain
retained. The first unprivileged copy failed on the root-owned discovery key.
With the writers still stopped, the complete copy was repeated through a temporary
network-disabled container, with original directories mounted read-only.

Startup stalled across the restart gap. Forty-two raw canonical payloads were
independently checked for block/transaction hashes, parent continuity and agreement
with a local Beacon checkpoint. Every `engine_newPayloadV4` import returned the
exact expected `VALID` hash; Lighthouse retained fork-choice ownership. Both chains
were synchronized by 18:23:36 UTC, the registry commitment resolved to canonical
local settlement, and the queue was empty. This establishes assisted recovery,
not unattended restart reliability. See [deployment evidence](evidence/deployment.json).

## Five-minute comparison

The candidate ran from **18:24:15 to 18:29:15 UTC**, with 57 inbound/outbound pairs
and no send errors or additional funding. The final pair targeted 18:29:20 and
settled during drain: all 114 sources belong to 51 canonical posts, while **50 posts**
landed inside the measured 60-slot window. The queue drained at 2026-09-28T18:29:21.358628+00:00.

| Metric | Previous `ca37074` run | Candidate `ba4cbab` run |
| --- | ---: | ---: |
| Grafana included / attempted target slots | 42/58 (72.4%) | **50/60 (83.3%)** |
| Produced blocks with both directions | 42/51 (82.4%) | 50/56 (89.3%) |
| Included / all scheduled slots | 42/60 (70.0%) | 50/60 (83.3%) |
| Skipped slots | 9 | 4 |
| Produced targets without any Composer attempt | 2 | 0 |
| Attempted produced targets without inclusion | 7 | 6 |
| Preparation p95 / maximum | 419 / 481 ms | 208 / 419 ms |
| Proving p95 / maximum | 44.2 / 50.1 ms | 32.9 / 46.7 ms |
| Finalization p95 / maximum | 210.8 / 225.8 ms | 126.3 / 175.0 ms |
| Complete admission / actual postBatch gas | 1.9865× | 1.9827× |
| Inbound companion estimate / actual gas | 1.1380× | 1.1218× |

The Grafana result was queried from the deployed **Posting outcomes by target
slot** panel with the exact target-time window. All 60 targets had attempts;
Grafana reports zero preparation failures. All 61 candidates including drain
admitted their whole snapshots (`snapshot_exhausted`), with zero capacity
rejections, FIFO bypasses or nominal preparation overruns. Every candidate was
acknowledged. The 88% target was **not reached**.

All four live skips were isolated. The consecutive-skip correction is demonstrated
by the controlled baseline/candidate regression, not by claiming it caused this
live rate increase. These sequential windows use the same release recipe,
workload and runtime settings on a shared host, but different skipped slots and
proposers; they are not a controlled throughput benchmark. No task-owned build or
CI job was started during the measurement. No observed pipeline timing regression
occurred in this window.

### Remaining misses

Read-only public builder/relay logs show:

| L1 block | Finding |
| --- | --- |
| 23273526, 23273532, 23273540, 23273556 | Payload attributes discarded: `no MEV-Boost relay data` |
| 23273524 | Bundle-containing blob bid became eligible at +78 ms; header requested at +26 ms. The relay returned its earlier no-blob bid. The later blob bid was 52 ms late. |
| 23273564 | Header request at +21 ms found no bid. Builder submitted the bundle bid at +884.624 ms; relay rejected it as a past-slot submission. |

Composer acknowledgement preceded these two timing misses by 978 and 946 ms.
All six produced misses had the expected canonical parent; across them,
acknowledgement preceded the target by 946–998 ms. The timing records establish
late downstream bids; they do not establish the builder's internal root cause.
The builder/relay/proposer infrastructure is outside our control and was unchanged.
With four skips and four no-relay-data slots, this window could at best reach
52/60 (86.7%) even if both remaining timing misses were recovered.

### Correctness, gas and observability

- **114/114** canonical successful source receipts; all six source/return/destination
  counters advanced from 216 to 273. **306/306** historical semantic checks passed.
- Repeated source/post attribution was unchanged; **51/51** postBatch receipts
  matched an independent L1 endpoint. This is semantic/receipt evidence, not
  independent live follower replay or KZG recomputation. Those live checks were
  not run; independent follower assertions passed in the process regressions.
- Native used-gas predictions matched **51/51** postBatch receipts exactly.
  Signed limits retained exactly 5% over native required gas. Complete admission
  bounds remain roughly 1.98× actual postBatch gas, but did not cut this workload.
- Deployed Grafana admission panels expose all **51** canonical posts and **114**
  accepted source estimates. Null FIFO-cut and cheap-rejection fields are expected
  here because neither path ran. Every included source has a simulation estimate.
- All 183 recorded fee quotes cover base-plus-tip and blob base fee. There is no
  evidence of a fee-estimation cutoff in this run. Node/signer image IDs and start
  times remained unchanged; no ERROR logs occurred. Ten exclusion warnings match
  skipped/produced misses; 24 warnings rejected system-sender pool transactions.
  No rejected hash belongs to the workload; the system-address gate remains intact.

See [measured results](evidence/live-results.json),
[postBatch admission versus actual gas](evidence/gas-estimate-vs-actual.csv), and
[per-source gas estimates](evidence/source-gas-estimates.csv). Source receipt gas
is from its source chain; outbound source gas is L2 gas and is not an L1 postBatch
cost. Shared framing/callback costs mean these columns must not simply be summed
as independent per-transaction postBatch costs. Full local diagnostics and scripts
remain under `artifacts/consecutive-skips-20260928/`;
[artifact hashes](evidence/local-artifact-hashes.json) identify retained files,
which are not available in another checkout.
