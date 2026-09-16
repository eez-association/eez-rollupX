# Performance candidate deployed on the existing Chiado network

The validated preparation/skipped-slot candidate was deployed at **20:26:39 UTC
on 2026-09-15**. Its running composer SHA-256 is
`98d33366a3f8a6d27335ecf8f1f23b4fb32ef53a3c6b244bd7eaf233e595788b`.
This report describes that earlier executable. The separate
[native DA gas-accounting correction](gas-accounting.md) was subsequently
deployed at 21:14:52 UTC and measured with the full requested workload; see the
[staged comparison](gas-staged.md) for the current deployment and results.

The deployment reuses the existing Chiado chain, registry, databases, genesis,
proof signer and settings. No second network was started. L1 chain ID is 10200;
L2 is 6290, rollup 1, with registry
`0x2fbd78894e327a38b69a3c2f539b92d926610331`. Empty consolidation remains 300 L2
blocks, approximately five minutes; ordinary L2 transactions use `always`.
Preparation/proof/submission allowances remain 500/500/700 ms.

## What the public-network comparison establishes

Both baseline and candidate passed **122 cross-chain sources**: 120 measured
and two warmup sources. Every source was joined to its canonical receipt, exact
owner generation, L1 post, L2 Sync hash, expected destination effects and nonce.
All named L1 companion receipts and post costs were verified. There were no
missing identity or receipt-cost records in the final audit.

Each profile also used 150 ordinary setup transactions and one ordinary L2
settlement barrier, excluded from the measurements. Distinct contract contexts
exercise fresh and reused proxies in both directions. These are bounded
12/24-source performance waves, not a rerun of the large staged protocol suite
or a measurement of maximum admission capacity.

| Wave | Sources | Posts, baseline → candidate | Median held → canonical completion, seconds |
| --- | ---: | ---: | ---: |
| Inbound, fresh | 12 | 1 → 1 | 3.694 → 3.800 |
| Outbound, fresh | 12 | 1 → 1 | 3.201 → 3.214 |
| Mixed, fresh | 24 | 2 → 1 | 16.207 → 3.694 |
| Inbound, reused | 12 | 1 → 1 | 3.951 → 8.071 |
| Outbound, reused | 12 | 1 → 1 | 3.543 → 3.531 |
| Mixed, reused | 24 | 1 → 1 | 4.103 → 3.935 |
| Mixed, after empty history | 24 | 1 → 1 | 6.420 → 16.261 |

The measured workload used **8 posts/8 blobs on baseline and 7 posts/7 blobs on
candidate**. Total L1 execution gas, including companions, was 29,813,131 and
28,603,858 respectively. These are sequential public-network observations with
different slot alignment, history and fee conditions, not a general speedup or
gas-saving estimate attributable solely to the code change.

The strongest live evidence is the skipped-slot behavior. Baseline encountered
an absent 20:13:00 L1 slot, reduced selection from 24 to 12 and settled the wave
in two posts. Candidate encountered an absent 20:29:45 slot, retained the exact
ordered 24 sources and settled all of them in one post on retry, without
submission capacity backoff. Consecutive canonical parent headers establish
both absent slots independently of the relay's exclusion labels.

The candidate retry is post
`0x69f3ba47cbdc12fd3b50dd98c0669d228e0c8b035128e54086b64f5dc1135003`,
at L1 block 23,073,675, covering L2 Sync 166,805 with hash
`0x9d8078cb918f6c4fecbd45a0108bf9bdb4f57b7031fc684d24b46771ec20dc22`.
All 24 sources reconcile under ownership generation 13. Its skipped attempt
has no paid receipt in the retained canonical evidence.

The candidate's 8.071-second inbound-reused median has a different cause:
ingress reached the held pool about 9 ms after the prior empty Sync decision.
Selection therefore waited about 4.98 seconds for the next finite snapshot;
preparation then took 192 ms. This preserves a remaining scheduling opportunity
and does not establish a preparation regression. Skipped public L1 slots and
arrival relative to selection mean sub-five-second completion is not guaranteed.

## Receipt costs and fee interpretation

Measured paid cost was **0.030910859198861517 xDAI on baseline and
0.033738821800227006 xDAI on candidate**, including user-funded L1 companions.
These are full bundle costs, not exclusively poster-wallet expenditure.

The captured RPC priority estimate changed from zero to 1 gwei. The unchanged
20% priority margin and 1-gwei floor therefore selected 1 gwei on baseline and
1.2 gwei on candidate; both periods had a 7-wei execution base fee. Final quote,
signed fee and receipt agree for all 19 paid posts, including warmup. Quotes
were refreshed 72–103 ms before dispatch. The posting and fee-policy sources
are byte-identical between the frozen variants. This was neither an aged test
wallet quote nor a failed-attempt price multiplier. The underlying reason for
the RPC estimator's change was not independently established. Blob prices also
varied. Fewer posts in this run did not produce a lower xDAI bill.

## Runtime, validation and evidence limits

Baseline and candidate used the same optimized Rust 1.94 release profile and
base image. The baseline executable was
`95118f7adb754e1157b68159249033c78315aa877ed15710558b6e714f825b4f`.
Compilation and local tests finished before the timed waves. The prior
[native validation](parallel-performance.md) covers the preparation estimator,
real exclusions, controlled skipped slots, reorgs and independent follower
replay. A live independent follower was **not run** for this public comparison.
Canonicality is as verified at collection, not a claim of L1 finality.

The harness initially assumed at most five unsettled blocks after setup; the
observed reconciliation delay made that condition unattainable. It was changed
to 30 before measured sources, identically for both profiles. Baseline mixed
traffic also exposed an observer mistake: the harness treated a provisional L2
receipt as immutable. Receipt tracking now retains replacements while unsafe
and rejects changes once observed safe. Four deterministic tests cover those
rules. Read-only recollection validated the final receipts and original ingress
times without resending transactions. Original failures, harness identities and
receipt histories are retained; early baseline ingress used the earlier observer
version. These corrections did not change production behavior.

At 20:35:20–20:35:32 UTC the exact deployed executable was verified again. Both
chain heads advanced, both execution clients reported synchronized, the beacon
had 24 connected peers and zero sync distance, and queued/in-flight/blocked/
unknown source counts were all zero. Every assigned environment value and all
original data mounts matched the pre-comparison deployment. Docker entries for
unset environment variables were omitted and remain unassigned.

The active deployment definition is
`.runtime/chiado-settlement-20260913/runtime.performance-candidate-v2.compose.json`.
The standard `runtime.compose.json` also retains the identical read-only
composer bind, so recreating its node keeps this executable. The frozen binary
under `artifacts/performance-parallel-20260915/release/` must remain available.
This persistence update required no additional restart. The active Compose
definition passed `docker compose ... config --quiet`.

The [compact record](chiado-performance.json) binds the summary to the retained
evidence. Detailed paired metrics, all source/receipt joins, exact executable
identities, command journals and failed observer captures are under
`artifacts/chiado-performance-next-20260915/`: `comparison/matched-comparison`,
`comparison/{baseline,candidate-v2}-audit`, immutable phase canonical exports,
and `deployment/final-runtime-health.json`. Private environment/key files are
excluded from those public evidence records.
