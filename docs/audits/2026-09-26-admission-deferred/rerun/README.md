# Repeat of the deferred-estimator wave

**Status: rerun finished; full-wave completion failed.** The runner exited 1.
753 jobs passed their semantic checks; 17 failed the receipt prerequisite.
Only 853 of 870 intended sources completed canonically.

The user requested another full wave after the [first candidate measurement](../README.md).
Checkout `61611e399c89f2e33c5c4ea1c7999f20beb876fd`, runtime
`85f3691cb9c7ccbf8c0b8700576733283e5af7fc`, protocol
`9d13218a355e7d543ca8a9927eea2962600b716d`. No implementation or runtime-configuration
changes, image rebuild, service restart, contract/genesis migration or datadir
reset were performed. Node/signer start times and zero restart counters confirm
that process-local timing observations were retained ([process identity](runtime-processes.json)).

The staged runner uses `--verify`: 770 jobs, 870 sources, 200 preparation workers,
28 send workers and four semantic verifiers, with its shared wallet lock.
Readiness initially found a 0.02856530226193719 xDAI shortfall under the prior
0.0045 worker-prefund setting. Task-only prefunding/floor of 0.00445 passed using
existing funds. This affects setup balances; transaction gas limits, source mix
and concurrency remain unchanged. No additional poster-to-source funding was
performed. The original profile and runtime environment are preserved;
[run parameters](rerun-plan.json) retain their hashes.

As in the previous wave, the zero-peer execution setup uses a task-owned
loopback helper for ordinary L1 transaction forwarding at five upstream requests
per second. Cross-chain ingress is unchanged. Private profiles, raw logs and
bulk snapshots are local-only under ignored `artifacts/admission-deferred-rerun/`
and the pinned protocol's `tmp/e2e-staged-net/` directory. They are unavailable
from another checkout. Small sanitized summaries are retained here.

This repeats a live workload with retained process state. It is not an isolated
baseline/candidate benchmark: calibration, storage, source ordering, ordinary
history backlog, chain slots and host load can differ.

## Canonical and semantic results

The original complete-wave measurement failed on **17 absent canonical L2
sources** ([first failure](initial-measurement-failure.json)). The unchanged
helper searched heights 79,520–79,765; the 17 receipts were also absent when
queried directly. This was not treated as a search-window correction or a pass.
A local diagnostic copy explicitly asserted that exact missing set and measured
only the remaining **853 sources**: 520 inbound and 333 outbound. Its output is
labelled incomplete. The original 870-source success assertion remains unchanged.

[Repeated canonical attribution](canonical-stability.json) preserved all 853
source mappings, all 26 postings and the same 17 missing sources. All 26 L1
receipts independently matched the external RPC. All 870 signed source hashes
were fresh and disjoint from the first candidate wave. The measured source
postings cover L1 blocks 23,241,061–23,241,094.

The monitor recorded 870 initially mined receipts and finished after **207.092
seconds** of send plus monitoring. Reconciliation later removed optimistic L2
receipts. This duration is **not full-wave settlement latency**: 17 sources never
completed. Empty queue state also did not establish success; the retry policy
had retired those sources.

| Metric | First candidate wave | Repeat |
| --- | ---: | ---: |
| Intended sources | 870 | 870 |
| Canonically completed sources | 870 | **853** |
| Semantic jobs passed | 770 | **753** |
| Jobs failing before semantic assertions | 0 | **17** |
| Postings carrying completed sources | 28 | 26 |
| Send + provisional receipt-monitor window | 191.278 s | 207.092 s |
| Admission / receipt gas, completed subset | 1.550× | 1.559× |
| Admission / required gas, postings containing inbound work | 1.844× | 1.716× |
| Median canonical-candidate preparation | 547.5 ms | 407 ms |
| Maximum canonical-candidate preparation | 791 ms | 782 ms |
| Median canonical-candidate proof | 112.48 ms | 104.35 ms |
| Absent slots between first/last source postings | 5 | 6 |

The [three-cohort comparison](comparison.json) also retains the `036b50a`
baseline. The repeat's gas ratios describe a smaller completed cohort; they
must not be presented as a complete 870-source throughput improvement.
[Per-posting estimates and receipts](admission-vs-receipt.csv) and
[coverage](admission-coverage.json) show all 853 completed sources with accepted
estimates and complete history. The failed sources also have estimates: 85
accepted occurrences across their five submissions each.

## Why the 17 sources did not complete

The affected jobs are 11 `multi-call-twiceL2` and six `revertCounterL2` jobs.
The [exact attempt history](failure-analysis.json) shows five submissions each:

| Candidate suffix | Target L1 height | Actual target slot | Retry counter charged |
| --- | ---: | --- | --- |
| `:55` | 23,241,075 | Absent | No |
| `:56` | 23,241,075 | Produced, no inclusion | Yes |
| `:57` | 23,241,076 | Absent | No |
| `:58` | 23,241,076 | Produced, no inclusion | Yes |
| `:59` | 23,241,077 | Produced, no inclusion | Yes |

All five relay acknowledgements succeeded, with dispatch headroom remaining.
All final preflight gas requirements fit the signed limits. The two absent slots
were correctly uncharged. Three charged `Retryable` resolutions reached the
existing `MAX_BUNDLE_ATTEMPTS = 3` rule in the
[RPC recovery contract](../../../../crates/eez-control-rpc/SPEC.md#61-status-classification).
[Composer](../../../../crates/eez-composer/src/composer.rs) supplies the charge and
limit; [ownership resolution](../../../../crates/eez-composer/src/optimistic.rs)
transitions those sources to `AttemptsExhausted`, and the held pool removes their
reservations. This terminal diagnosis is traced from the three recorded charged
resolutions and deployed code; individual terminal ledger state has no public
query RPC. It is not inferred solely from empty queue counts.

These were immediate/outbound entries using the **unchanged 370,000-gas reserve**
and `immediate_evidence_unavailable` pricing reason throughout all 85 occurrences.
They did not use the new deferred-storage discount. Their preflight and signed
gas evidence supplies no indication of gas underestimation.

The three produced exclusion blocks had gas usage **0 / 116,183 / 0** and no
blobs. Their available gas exceeded the affected postBatch signed limits
([capacity evidence](exclusion-block-capacity.json)). Visible chain capacity
therefore does not explain those exclusions. A relay acknowledgement and valid
local preflight do not identify the external builder/proposer's reason for
excluding a batch; that cause remains unverified.

Independent [state checks](missing-effects.json) establish actual missing effects:
all 17 source nonces remain 20 and their receipts are absent; all 11 affected L1
counters remain **0 instead of 2**, while six L2 `lastCallFailed()` flags remain
**false instead of true**. No failed source was resubmitted by this investigation.

Across the whole wave, [41 attempts](attempt-summary.json) have complete event
sequences: 26 canonical postings and 15 uncommitted submissions. Six uncommitted
attempts targeted absent slots; nine targeted produced slots. One delivery timed
out. Every canonical acknowledgement preceded its deadline, with minimum
headroom 44.42 ms. All canonical bundle transactions fit their signed gas limits,
and no canonical preparation exceeded its nominal 1,000 ms allowance.

## Verification harness failure and intervention

The pinned `network.sh` verifier reloads each trigger with an unbounded
`cast receipt ... blockNumber` call. Its monitor had cached optimistic receipts,
so the affected jobs were launched as mined; the disappeared receipts then
blocked all four verifier slots indefinitely. The runner had made no progress
beyond 476 passing checks while those waits remained active.

Only `cast receipt` descendants of this task's recorded runner PID were stopped,
for the exact known missing hash set. Before each cancellation both the canonical
L2 RPC and ingress RPC returned null. Script ancestry, command arguments and PID
identity were checked; Linux PID handles bound the signal to the checked process.
The shell then reached its explicit `ERROR: no receipt for sent tx` failure.
All other verifiers continued unchanged under the same wallet lock.

The [17 cancellation records](receipt-wait-cancellations.json) are **failed
receipt checks**, not successful semantic assertions. The final
[verification summary](semantic-verification.json) records 753 semantic passes,
17 prerequisite failures, zero unaccounted jobs and runner exit 1. Independent
state checks establish the 17 missing effects separately. No protocol source,
operator profile or verification assertion was changed to force a pass.

## Final state and follow-up

The [final runtime check](deployment-final.json) confirms unchanged images,
healthy signer, current nonsyncing heads and preserved genesis/settings. Queue
counts are zero because completed and terminally failed sources have both left
the active queue; they do not mean 870 successful completions. Source/terminal
ownership remains in memory, without a durable recovery guarantee.

The temporary helper was stopped after verification and its loopback port is
closed. It forwarded 1,397 identical ordinary transactions with zero upstream
rate-limit responses and recorded 2,367 `BrokenPipeError` client disconnects
([cleanup](setup-forwarding.json)). Those disconnects are not successful calls.
The private task profile references the stopped helper and is not reusable as an
operator runbook. The runtime environment and original operator-profile hashes
are unchanged. Existing unrelated working-tree edits were preserved.

No implementation changed, so native/Rust/contract suites were not repeated.
This rerun does not add independent live follower replay or CI/Kurtosis coverage.
[Local diagnostic hashes](diagnostic-evidence.json) identify retained first
failures and logs; they remain local-only.

The next investigation should address external inclusion failures and the
consequence of charging ordinary non-inclusion to the source eviction budget.
Any policy change must preserve bounded recovery and nonce dependencies, and
update the accepted recovery contract and regressions. Monitoring should expose
terminal source disposition explicitly. The workload checker also needs bounded
receipt reads and canonical completion checks so optimistic mining cannot be
mistaken for finished cross-chain work. These are findings, not implemented
policy changes in this rerun.
