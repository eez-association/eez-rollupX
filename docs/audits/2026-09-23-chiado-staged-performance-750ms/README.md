# Staged Chiado performance run with a 750 ms proof budget

Status: stopped at the user’s request during setup. No measured source wave ran. All broadcast setup transactions subsequently mined successfully; investigation below.

The user requested another `run-chiado-staged-performance.sh` run against the
existing network. The previous fourfold workload was retained: 3,080 jobs,
3,480 source transactions and the same 14 scenarios. This is `--no-verify`:
source receipts and settlement observations do not establish scenario semantics
or independent follower replay.

## Identity and settings

- Fork base: `1832f970884ca9460646a4cde87cca70eaffb3e1` plus the deployed
  [timeout replacement](../2026-09-23-timeout-replacement/README.md) and
  [posting-stage timing](../2026-09-23-posting-stage-timing/README.md) changes.
- Protocol: `3c0cdf4955941a5c23639346932b55b33babebbd`, clean initialized pin.
- Node image: `eez-node:posting-timing-f873c0e1e62d`, immutable local ID
  `sha256:ecd62f1c96ca8970055132815b7db8f419c7f0c70a5e950795cefc152c269edd`.
- Prover timeout: 750 ms; preparation 500 ms; submission slack 700 ms.
- Host workload tools: Foundry/Cast 1.7.1 extracted from the existing pinned image.
- Scale 4; preparation parallelism 200; MultiSend batch 220; send workers 4;
  per-worker send pause 1.5 seconds; polling interval 1 second.
- Full-run wallet lock retained. No concurrent upstream runner was present.
- Started: `2026-09-23T13:35:49.707490+00:00`.

The first check found no default `chain.env`; the explicit existing private profile
was then selected. L1 funding was sufficient. L2 needed 4.4705 additional test xDAI;
5 test xDAI was sent from the existing public development fixture to the dedicated
workload source. Its receipt succeeded. The funded preflight passed empty-queue,
chain/ingress/code, nonce alignment and funding checks. Worker funding completed
without repeating the prior 31-funding-transaction burst: batch 220 uses 14 per
chain. Source pacing matches the previous successful recovery; the earlier run's
unpaced failed send phase is not a matched throughput baseline.

Local-only private diagnostics: `artifacts/chiado-staged-750ms/` and
`eez-core-protocol/tmp/e2e-staged-net/20260923-153550/` in the source
checkout. Raw profiles/logs can contain private endpoints or worker credentials
and are not committed. Sanitized results and digests follow after completion.

## Interrupted run and recovery boundaries

Planning completed for all 3,080 jobs in 323.601 seconds. Sending 11,400 setup
transactions took 30.624 seconds. L1 receipt collection was still progressing
near the default 300-second setup deadline. Before that deadline, the runner was
stopped (exit 143) and resumed with `DEPLOY_MINE_TIMEOUT=1800` using its supported
saved-transaction path under the same helper lock. No setup transactions were
regenerated; previously recorded hashes prevented duplicate broadcasts.

The user then requested stopping and analysis. The resumed runner and its owned
workers were terminated (exit 143), and its full-run lock was released. No node,
signer or monitoring service was intentionally restarted during this test.
There was no `sent.csv` or measured cross-chain source transaction: **0/3,480
sources sent**. No further workload was launched after the user's stop.

The upstream EXIT trap wrote `pass` to interrupted phase rows despite signal
exit 143. Those rows do **not** establish successful completion. Resume also
rewrote the receipt-monitor CSV while rescanning already mined receipts, so its
partial row count is not evidence of disappearing receipts. Independent RPC
receipt queries below establish final counts.

## Confirmed host out-of-memory termination

At **13:39:14 UTC**, while the runner was still planning (before the setup-send
phase), the kernel recorded global OOM and killed `eez-composer`. Docker restarted
that same container at 13:39:15 UTC. Its restart count became one; the signer
remained at zero. The image did not change. Current `OOMKilled=false` after restart
did not reveal the earlier kill; retained kernel evidence established it.

The kernel's process table contained 200 `solc-0.8.34` processes with approximately
36.98 GiB aggregate RSS and 200 Forge processes with 6.35 GiB aggregate RSS.
Composer had 6.74 GiB RSS. The host has about 61 GiB RAM and no swap. RSS aggregates
can include shared pages; these are diagnostic process-table totals, not exact
exclusive memory attribution. The global OOM record and selected killed process
are explicit. Running the 200-way preparation workload on this shared network
host was unsafe; the operator agent should have reduced that concurrency.

Recommended next workload settings: move preparation to another host or reduce
parallelism substantially (for example 8–16, then measure peak memory). Keep a
resource boundary between the runner and network processes. Do not interpret this
run as a throughput test of the 750 ms prover configuration.

## Confirmed historical-prefix retry gap

All four canonically skipped target slots before the stop were followed by a
posting attempt at **+10 seconds**. The next actual dispatched targets were
**+20, +20, +20 and +30 seconds**. This was the historical-prefix path under
setup-data backlog; the earlier native cross-chain replacement regression did
not exercise this combination.

Concrete sequence, 13:44 UTC:

1. Target 13:44:30, execution block 23,192,761, was skipped.
2. At 13:44:33.053 the replacement hook correctly requested the same execution
   height with timestamp 13:44:35.
3. Accumulated data required splitting into a historical prefix. At 13:44:33.238,
   `emit_historical_chunk` called `next_policy_target` afresh instead of retaining
   the supplied replacement target. The latest observed header was height
   23,192,760 at 13:44:25. The selector chose block **23,192,763** at **13:44:40**
   with `block_offset=3` and a 1,186 ms preparation/proof allowance.
4. The new candidate was built, but the necessary parent did not appear before
   that target's cutoff. At 13:44:39.771 it was deferred as
   `target_parent_unavailable`. The missed slot must not be counted as a produced
   execution block when choosing the retry's height.

Relevant callers: `crates/eez-composer/src/composer.rs::emit_historical_chunk`,
`replace_timed_out_attempt`, and `crates/eez-l1/src/submitter.rs::next_policy_target`.
The fix is incomplete for this fallback path. Carry the pinned retry target through
historical-prefix preparation, and test full/oversized DA plus a skipped slot and
late-original inclusion. If the candidate cannot meet its remaining preparation
window, account for that explicitly rather than infer produced block heights from
elapsed slots. This investigation did not modify or deploy that fix.

## Other posting failures

Before the stop, the index recorded these **attempt** outcomes (not fractions of
all Chiado slots):

| Outcome | Attempts |
| --- | ---: |
| Block produced without inclusion | 5 |
| Deferred before dispatch | 6 |
| Included at target | 7 |
| Network slot skipped | 4 |
| Preparation failed | 4 |

Four failed preparations reported `prover_deadline`: the Composer exhausted 750 ms
proof attempts/retries. Six pre-dispatch deferrals reported `target_parent_unavailable`.
Additional candidate failures occurred before a normal posting lifecycle event:
the whole range exceeded the two-blob allowance, then the prefix attempt either
timed out or was discarded because canonical scan generation changed during proof.
Four such generation failures in the retained logs had the same settlement cursor
at the next check. `mark_scanned` increments that generation even on ordinary L1
head advancement; this is a candidate for a focused stale-anchor regression, not
permission to remove the final consistency guard.

The signer's 31 successfully completed request timings had a 60 ms median and
81 ms maximum. These describe successful requests once admitted at the signer,
not Composer's full operation or timed-out requests; they do not identify where
failed operations spent their budget. No signer WARN/ERROR appeared in the sampled
logs. No unsupported-call validation error was observed, and no cross-chain
source wave was sent, so this run cannot validate those scenario semantics.

## Final read-only observation

At **13:56:13 UTC**, independent receipts showed:

| Chain | Setup transactions sent | Successful receipts | Pending | Reverted |
| --- | ---: | ---: | ---: | ---: |
| L1 | 5,560 | 5,560 | 0 | 0 |
| L2 | 5,840 | 5,840 | 0 | 0 |

Earlier L1 sampling found no unknown hash among 30 pending samples; transactions
were still mining. Across the sampled 80-block setup interval, median whole-block
gas fullness was 99.2% (mean 60.2%, including sparse blocks). L1 capacity explains
much of the ordinary setup wait; it is distinct from the Composer retry gap.

The final L2 setup block, 427,561, resolved to a canonical finalized settlement
via `eez_getSettlementByL2Block`, transaction
`0x1bc28933cf9215c92996c6a9d7e78324f3ceaaff7dac0c14843fc612366bf5ee`.
L2 safe height was 427,975 and latest 428,370. Queued, in-flight, blocked and unknown
cross-chain counts were all zero. Settlement recovered after the interruption.
These checks establish receipt/settlement progress, not scenario postconditions.

## Retained evidence

Sanitized kernel totals, independent receipt summaries, stopped-window posting
outcomes and exact runtime identity are retained in `summary.json` beside this record.
The raw ignored diagnostics remain local-only. No runtime code changes, service
restarts, new performance run or semantic/follower replay were performed during
the stop-and-analysis phase.
