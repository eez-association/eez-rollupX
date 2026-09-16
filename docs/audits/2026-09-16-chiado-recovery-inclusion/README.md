# Chiado recovery: submission inclusion and lost preparation opportunities

Investigation of September 16, 2026, after the poster received 50 test xDAI at
**11:44:50 UTC**. Live operations were read-only: local node/signer logs, canonical
RPC, the monitor, and the supplied [builder/relay log viewer](https://logs.chiado.gcd.ovh/).
No transaction, restart, runtime configuration change, or production code change
was made. The live executables are the `6524c3a` release; the inspected checkout
is merge commit `ddf40bc`.

The implementation analysis below describes that revision. The subsequent local
fix and its validation are recorded at the end of this report.

Recovery was limited mainly by local scheduling and preparation. Builder
inclusion was strong for requests that actually reached an available proposer.
The denominator must separate composition opportunities, proof starts, actual
HTTP submissions, missing network slots, and canonical inclusion.

## Verified inclusion results

The main cohort contains submissions dispatched from **11:44:50 inclusive to
11:54:15 exclusive**, with outcomes checked against later canonical headers and
receipts. The raw log capture extends through 11:54:20; the last in-flight
submission is outside this cohort. Historical slots are also joined to builder
payload attributes and relay delivery records.

| Cohort | Actual submissions | Included successfully | Target slot skipped | Omitted from a produced target block |
| --- | ---: | ---: | ---: | ---: |
| First backlog-clearing posts, before 11:46:11 | 8 | 8 | 0 | 0 |
| Subsequent recovery, 11:46:11–11:54:15 | 59 | 51 | 6 | 2 |
| Complete funded cohort | **67** | **59** | **6** | **2** |

This is **88.1%** inclusion over all submitted targets, or **96.7%** over targets
that produced a block. Both produced-block omissions had **no MEV-Boost relay
data**. Every submission to a produced target block with relay data landed in
this sample. All 59 successful receipt/block identities match the relay's
delivered execution payload hashes. No included post reverted.

The earlier 11:15:00–11:22:05 dispatch cohort independently has 38 successful
inclusions and eight canonically skipped slots from 46 submissions, with no
omission from a produced target block. During the funding stall itself,
11:22:05–11:44:50, **zero bundles were dispatched**. Those local failures are not
builder exclusions. There were 212 explicit blob/execution balance-reservation
failures in the captured logs, plus earlier gas-probe failures while funds were
already low.

The user's approximately 80% validator coverage is operational context, not an
independently verified stake-weighted measurement. Validator ownership and relay
registration are different evidence. This investigation does not infer ownership
from fee recipients or execution-client labels.

## 1. Historical recovery unnecessarily ran at roughly half the slot cadence

The first eight successful posts ended at L2 block 221,780 and landed at these
UTC times:

| L1 time | L1 block | Settled L2 blocks in this post |
| --- | ---: | ---: |
| 11:44:55 | 23,083,522 | 175 |
| 11:45:05 | 23,083,524 | 185 |
| 11:45:15 | 23,083,526 | 180 |
| 11:45:25 | 23,083,528 | 185 |
| 11:45:45 | 23,083,531 | 180 |
| 11:45:55 | 23,083,533 | 180 |
| 11:46:05 | 23,083,535 | 180 |
| 11:46:10 | 23,083,536 | 180 |

Each used both permitted blobs. The initial pending encoding was approximately
1.89 MB against 253,952 meaningful bytes per two-blob post, so multiple posts
were necessary. The last of these posts also carried a cross-chain source;
they must not all be described as empty or purely historical transactions.

The avoidable scheduling mechanism is visible in the code and logs:

1. At **11:38:49.726**, an anchor-only preparation sample took **986 ms**.
2. [`EnabledPolicy::note_preparation`](../../../crates/eez-composer/src/composer/policy.rs)
   retains the maximum sample for the process lifetime. Adding the configured
   500 ms proof budget gives a **1,486 ms** preparation reservation.
3. Composition normally starts about **1.69 seconds before the next slot**.
   The retained 1,486 ms plus 700 ms submission slack needs **2,186 ms**.
   [`select_policy_target`](../../../crates/eez-l1/src/submitter.rs) consequently
   selects **N+2** for the historical chunks.
4. The next composition tick finds the preceding post still in flight and
   cannot prepare another independently advancing commitment. For example,
   11:44:53 reports `gate_blocked=true`; the post lands at 11:44:55; the next
   attempt starts at 11:44:58 and targets 11:45:05.

The seven anchor-only preparations measured after funding took **381–417 ms**;
their signer work was also within the 500 ms proof allowance. The retained
986 ms sample was conservative enough to force the slower target cadence even
after preparation became faster. This identifies why the scheduler waits; it
does not prove every counterfactual next-slot post would have landed.

The 20-second gap has an additional cause: the canonical **11:45:30 slot was
missing**. It invalidated the anticipated parent/height for the prepared
11:45:35 target. Local preflight refused that attempt at 11:45:34.681, before
dispatch; the retry landed at 11:45:45. This is not a builder rejection.

The first eight posts took **80 seconds after funding**. The public observation
of matching L2 latest/safe heads at 11:46:16 established historical head catch-up,
not completion of every held cross-chain user. The held queue still contained
**1,188** requests at 11:46:13 and was drained separately while new traffic
continued arriving.

## 2. Preparation repeatedly crosses the proof-start cutoff

During 11:46:11–11:54:15, **24 of 76 nonempty measured preparations** failed
with `Composer proof-attempt cutoff reached`. These failures occur in
[`retry_budget`](../../../crates/eez-composer/src/prover_retry.rs) **before calling
the remote prover**, rather than because the signer rejected a proof or the
builder rejected a bundle.

The runtime reserves 500 ms preparation, 500 ms proving, and 700 ms submission
slack. Actual admission starts with roughly 490 ms left for preparation.
Automatic admission fills most of that time using the previous finishing/KZG
sample, with insufficient allowance for variation at the end:

- **23 of the 24** failed preparations missed the start cutoff by less than
  **30 ms**; median lateness was **10.1 ms**, minimum **1.0 ms**.
- The remaining preparation missed by **121.9 ms**.
- Failed candidates contained **54–66 accepted groups**. Across attempts,
  **1,486 group admissions** were prepared and then returned for retry; these
  are repeated work, not 1,486 distinct lost users.
- Signer validation/signing for successful subsequent preparations had a
  **61 ms median**, **94 ms p95**, and **117 ms maximum** in this cohort.
  Signer elapsed time excludes transport and client-side work.

Example: at **11:46:38.807555**, 60 accepted groups took 498 ms against a
491 ms preparation allowance. The 11:46:40 target's proof-start cutoff was
11:46:38.800. All that preparation was discarded for missing the cutoff by
7.6 ms, even though no proof attempt had yet begun.

There is also an ineffective backoff under a large queue.
[`AutomaticAdmission::finish`](../../../crates/eez-composer/src/composer/admission.rs)
halves the number selected for consideration, which can be the entire queued
snapshot, rather than the number actually admitted before running out of time.
The example above changes the next selection from **1,164 to 582** while the
actual workload is around **60 groups**. The next attempt still accepts 63.
Across **23 of 24** failures, the next selection limit remains at least as large
as the preceding admitted workload. It therefore does little to reduce the
work that is pressing against the deadline.

These 24 failures consume 24 composition opportunities on a five-second grid.
It would be incorrect to claim exactly 120 seconds of measured causal speedup
from eliminating them: traffic, regrouping, skipped slots and later capacity
would also change.

## 3. The two actual omissions had no usable relay registration data

| Target UTC | L1 block | Beacon slot | postBatch hash |
| --- | ---: | ---: | --- |
| 11:47:05 | 23,083,546 | 24,832,585 | `0x060efda7d7e428fd4ff131b164eaa9ebfd3f327e6050f1397b9e1ef7ec3772e7` |
| 11:51:40 | 23,083,597 | 24,832,640 | `0x830daae87f818556bed56eef838145429b20b8dc0761d9fdb5f3648e4ad3d663` |

For both slots, every matching builder payload-attributes event is immediately
discarded with `no MEV-Boost relay data`. The relay's published duty lists omit
these slots, and there is no matching bid/header/payload delivery in its retained
logs. The canonical blocks identify an Erigon producer and the same fee
recipient; this is supporting context, not proof of validator ownership.

Local dispatch was approximately **967 ms** and **958 ms** before the target;
the non-error RPC replies arrived approximately **789 ms** and **774 ms** before
it. The HTTP acknowledgement alone does not promise a build or inclusion.
Missing relay data, rather than a demonstrated simulation failure or late local
HTTP dispatch, explains why this builder did not build these targets. The logs
do not establish whether the proposers were intentionally outside this relay or
had a registration/configuration problem.

## 4. Six submitted targets were missing network slots

Consecutive canonical headers prove that all six timestamps were skipped; a
height mismatch alone is not used as that proof.

| Target UTC | Beacon slot | Registered proposer index | Relay evidence |
| --- | ---: | ---: | --- |
| 11:46:20 | 24,832,576 | 9,134 | Validated bid eligible 673 ms before slot; no header/payload request |
| 11:48:50 | 24,832,606 | 9,320 | Validated bid eligible 615 ms before slot; no header/payload request |
| 11:49:55 | 24,832,619 | 9,201 | Validated bid eligible 626 ms before slot; no header/payload request |
| 11:51:20 | 24,832,636 | 9,176 | Validated bid eligible 738 ms before slot; no header/payload request |
| 11:51:55 | 24,832,643 | Unavailable in relay data | Builder discards attributes: no MEV-Boost relay data |
| 11:53:15 | 24,832,659 | 9,444 | Multiple validated bids eligible before slot; no header/payload request |

The relay recorded no matching `getHeader`/`getPayload` request for these slots.
For the five registered proposers, eligible validated bids existed before the
slot. This narrows the remaining investigation to proposer operation or the
path between proposer and relay; **offline validators are not proven**. The
available viewer contains builder, relay and their execution/consensus clients,
not these validators' signing/proposal logs. The table provides exact duties to
look up in validator-client and MEV-Boost logs.

Six other bid-header deliveries in the interval have no matching delivered
payload, but belong to different, produced slots. They must not be attributed
to these skipped targets. All statements above use exact slot/hash joins.

## Fix priorities and acceptance criteria

1. **Make automatic admission finish reliably before its cutoff.** Reserve
   finishing/KZG variability and reduce actual admitted work after a preparation
   overrun. Halving a much larger queue-selection limit is insufficient. Preserve
   atomic groups, nonce order, source ownership and hard proof/submission
   deadlines. Validate with replayed preparation spikes, a queue much larger
   than admitted capacity, and the unchanged native cross-chain workload.
2. **Remove persistent N+2 catch-up caused by a stale maximum.** Evaluate an
   earlier historical trigger or a bounded, candidate-aware preparation estimate
   with safe probing/relearning. Continue resolving the exact parent and fork
   before final dispatch; do not simply relax the timestamp or proof checks.
   Verify a slow historical sample followed by fast two-blob chunks, skipped
   slots, and ordinary source-bearing operation.
3. **Audit the five registered proposers above and relay coverage for the three
   no-data slots.** Confirm whether they are in the controlled validator set,
   then check proposal scheduling, connectivity and MEV-Boost configuration.
   The supplied logs establish the boundary of the failure, not its exact
   validator-side cause.
4. **Report separate operating rates.** Surface preparation cutoff failures,
   balance failures, actual dispatched bundles, skipped targets, no-relay-data
   targets and canonical inclusions. A single ratio hides the dominant local
   preparation losses. The captured monitor snapshot does not expose a
   postBatch attempt-success metric, so no specific UI denominator is assumed.

No implementation or timing change has been deployed by this investigation.
These are evidence-backed priorities, not a claimed measured speedup.

## Evidence and reproduction

Local evidence is in
[`artifacts/chiado-recovery-inclusion-20260916/`](../../../artifacts/chiado-recovery-inclusion-20260916/):

- `node.jsonl`, `signer.jsonl`, `metadata.json`, `canonical.json` and
  `identities.json`: retained local logs, non-secret runtime settings and
  canonical block/receipt evidence with file hashes.
- `dispatches.csv`, `dispatches.json`, `summary.json`: exact HTTP-dispatch
  identities, pinning, replies, observed lead times and canonical outcomes.
- `preparations.json`, `failure-backoff-pairs.json`: proof-start overruns and
  the following admission selections.
- `remote-builder.log`, `remote-relay.log`, `remote-builder-reth.log`, their
  readable `*-messages.log` counterparts, `remote-sources.json`, and
  `remote-identities.json`: supplied viewer evidence and request provenance.
- `relay-correlation.json`, `nonincluded-builder-evidence.json`: exact
  payload/slot identities, validated bids, registration availability and
  canonical delivery matches.

Dozzle's historical endpoint requires explicit visible log levels and limits a
response to 500 grouped entries. `collect_remote.py` fetches bounded 30-second
intervals, recursively splits any full response and deduplicates boundary
records. An empty response without log-level filters is not evidence of absent
logs. These scripts perform GETs and read-only Ethereum RPC calls only.

```sh
# Recheck retained local/canonical evidence without network access.
python3 artifacts/chiado-recovery-inclusion-20260916/analyze.py
python3 artifacts/chiado-recovery-inclusion-20260916/correlate_remote.py

# Refresh canonical reads or remote retained evidence, if needed.
python3 artifacts/chiado-recovery-inclusion-20260916/analyze.py --collect
python3 artifacts/chiado-recovery-inclusion-20260916/collect_remote.py
```

The analyzers assert canonical parent continuity, receipt/block/hash agreement,
unambiguous target-slot mapping and delivery-hash matches for all included posts.
No runtime regression suite was needed for the read-only investigation. The
subsequent implementation is validated separately below.

## Local recovery fix

Branch `fix/chiado-recovery-timing` changes admission and historical target
estimation without changing the proof/submission cutoffs or canonical gates:

- Automatic group-start, source-check and pre-append checks add 10% of the
  actual preparation window to calibrated finishing predictions. At the observed
  491 ms window this leaves 49.1 ms, compared with under 30 ms lateness in 23 of the
  24 failed preparations. Without calibration, the conservative full-pass
  fallback floors the observed non-KZG finishing cost plus jitter, rather than
  adding both allowances. This preserves a fitting wave after a small post.
  `admission.prepared` exposes the allowance as `jitter_reserve_ms`.
- Incomplete/late preparation backs off from the admitted count or the final
  prepared count after rebuilding. The observed 1,164-selected/60-admitted
  failure now limits the next attempt to 30, instead of 582. FIFO fairness and
  the one-group probe still apply; successful FIFO progress releases the cap.
- Each rollup reserves its largest of the last four anchor-only preparation
  times plus 10%, then adds the existing proof budget. A new slow observation
  takes effect immediately. Four fresh observations displace a stale peak.
  Replaying 986 ms followed by 381, 384, 393 and 417 ms reduces the preparation
  reserve to 458.7 ms, allowing the next five-second slot with 1.69 seconds
  remaining, a 500 ms proof budget and 700 ms submission slack.

The jitter allowance and recent peak are estimates, not latency guarantees.
One unusually expensive group or a fresh host/RPC stall can still miss a target;
the hard deadline and ownership/retry rules remain in force. The recent peak
is shared across anchor-only candidate sizes within a rollup, so a larger new
candidate may need another slot to update it. This fix does not establish or
repair the validator/relay issues in sections 3–4 and has not been deployed.

Four deterministic regressions failed against the original implementation
(200 existing tests passed) and pass with the change. They cover the observed
large queue, incomplete and rebuilt candidates, finishing jitter, and historical
relearning. A fifth regression covers the conservative fallback after a small
post: native tests caught an initial implementation splitting 45-source waves
by stacking two finishing allowances. The final implementation preserves both
the full-pass floor and jitter above observed finishing. Existing timing tests
retain their deadline/blob checks with the calibrated reserve.

Validation uses Rust **1.94.0**, the pinned protocol submodule, and unchanged
native fixtures. All **205 composer** and **129 L1** unit tests pass. Composer
all-target Clippy with warnings denied, formatting and whitespace checks pass.
The L1 suite includes consecutive-slot selection, missed deadlines and canonical
skipped-slot checks; its mock RPC servers require permission to bind localhost.

All six selected native cases pass against the normal **release** binaries
(opt-level 3, thin LTO, unchanged workspace profile):

| Workload | Network profile | Result |
| --- | --- | --- |
| Two consecutive 45-source short-result waves; each fits one Sync and replaces the prior queue | Ethereum Osaka | Passed |
| Same 45-source waves | Gnosis Prague | Passed |
| Small warmup followed by 128 mixed inbound/outbound sources with 500 ms preparation | Ethereum Osaka | Passed |
| Same mixed workload | Gnosis Prague | Passed |
| Largest fitting empty historical prefixes drain under a two-blob cap | Ethereum Osaka | Passed |
| One held cross-chain source makes its finite historical predecessor due, then completes | Ethereum Osaka | Passed |

These cases verify canonical completion, source effects, and independent
follower replay. The Ethereum release 45-source preparations took 316 ms and
301 ms against 492 ms and 497 ms available, respectively. They establish local
compatibility, not a measured public Chiado recovery speedup.

The development-profile run is not all green: the refined candidate passed
five of six cases, while Ethereum's strict 45-per-Sync assertion split into
34 and 11. Its full-pass fallback stopped on elapsed work even though final
preparation finished within the cutoff. The release run above passes that
assertion with the same source and unchanged test. Development timings must
not be substituted for the optimized release packing result. The initial
implementation and both development runs are retained, including failures.

Final evidence is retained in
[`artifacts/chiado-recovery-validation-release-20260916/`](../../../artifacts/chiado-recovery-validation-release-20260916/):
`verification.json`, `native-results.json`, unit/Clippy logs, per-case native logs,
`candidate.json`, runtime build records and frozen binaries. The unchanged native
harnesses launch those binaries explicitly through `EEZ_TEST_RUNTIME_BIN_DIR`
and `EEZ_TEST_PROOF_SIGNER_BIN`. `check-results.py` verifies exactly six successful
cases and unchanged source/binary hashes. The tested composer SHA-256 is
`afdc7080257fde7dd1f86a679a98e1f20959f1b622b60bc9fe068d5e488d6adf`.

The earlier development evidence is in the sibling
`chiado-recovery-validation-20260916/` and
`chiado-recovery-validation-final-20260916/` directories. All artifacts are local;
no production executable, configuration or service was changed.
