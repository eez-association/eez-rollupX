# Individual failed posting attempts in the fixed-version repeat

Status: read-only investigation complete to the available log evidence. All 24
non-included events have an individual record below. Two composition questions
remain unresolved; no claim of universal composition correctness is made.

| Field | Value |
| --- | --- |
| Run | Chiado staged `20260923-152613`, 2026-09-23 UTC |
| Source | Fork `1832f970884ca9460646a4cde87cca70eaffb3e1` plus deployed historical-target working-tree patch |
| Protocol at run | `3c0cdf4955941a5c23639346932b55b33babebbd` |
| Node image | `sha256:d8bac330a05aa0fe3653960ebc170a55c0608e65892ded85bd0477f3079059e7` |
| Composer SHA-256 | `57a55d8f114e5581a7b2ce5e34c82881c9144c5f39547a54c95a26810e2df37b` |
| Related result | [Deployment and repeat audit](../2026-09-23-historical-target/README.md#authorized-deployment-and-comparable-repeat) |

The repeat succeeded with 870/870 source receipts and no resubmission. Across
setup and source phases there were 116 included posting attempts, thirteen
produced-block exclusions, ten skipped-target attempts and one preparation
failure. A successful receipt workload can still encounter failed individual
posting attempts. It used `--no-verify`, so this is not scenario semantic or
independent follower validation. The current protocol port is separate work;
its results cannot be inferred from this older deployed executable.

## Findings

Eight produced exclusions had no proposer registration in the observed relay
duty lists and no bid from this builder. Their canonical blocks all identify
Erigon and use fee recipient `0x60a69ad71c6cb3c1fbc7afa8be898ba6f0988022`.
This establishes lack of this relay path, not the validator operator's motive.
It also does not independently validate the unselected Composer input.

At 15:33:05, this relay validated three bundle-containing candidates and delivered
one header to the proposer, but no payload was requested. The canonical block
has a different hash. This is observed non-selection of this relay's candidate;
local building, another relay, value comparison or fallback cannot be separated
from these logs alone.

Three exclusions immediately after actual skipped slots show the builder missing
the proposer's request. The new target and parent were correct, and Composer
had dispatched 1.357–1.595 seconds before target:

| Target UTC / execution block | Builder payload attributes | Composer dispatch | Proposer request: no bid | First candidate | First bundle candidate |
| --- | --- | --- | --- | --- | --- |
| 15:38:00 / 23193981 | 15:37:58.337260 | 15:37:58.405 | +127 ms | +260.725 ms | +260.725 ms |
| 15:40:45 / 23194011 | 15:40:43.340601 | 15:40:43.643 | +23 ms | +550.582 ms | +1,318.589 ms |
| 15:44:40 / 23194052 | 15:44:38.337166 | 15:44:38.407 | +124 ms | +872.889 ms | None observed |

For 23193981 the relay explicitly rejects the late candidate as a past-slot
submission, not a proof or contract-validation error. Builder jobs began within
approximately 10 ms of receiving payload attributes in all three cases. Most of
the delay was before the candidate's final fill/seal phase; public logs do not
show per-order scheduling/simulation durations. Two cases eventually contain a
bundle. The third remains open as to whether the replacement bundle would have
built successfully. Old and replacement bundles share an execution height;
aggregate `two received / one simulated` counts cannot identify which failed.
The builder's missing bundle-hash warning further limits exact input attribution.

The setup exclusion at 15:33:00 also remains open: its single bundle was counted
as successfully simulated, but candidate assembly counted one failed bundle and
submitted only non-bundle candidates. A header was delivered, then the proposer
used a different canonical block. The setup's L1 deployment traffic was active
at this time. Gas capacity, state/nonce conflicts with other transactions,
ordering, and Composer input require the exact build failure to distinguish;
none is established by the aggregate counter. Treating this as simply network
conditions would overstate the evidence.

All ten skipped targets are strict timestamp gaps between consecutive canonical
execution headers with matching parent hashes. Every one is also marked empty
by the remote Lighthouse. For example, the chain jumps from 15:41:20 to
15:41:40, omitting :25, :30 and :35. The first two slots had posting attempts;
:35 did not because the existing one-replacement-per-observed-head limit had
been reached. The [public explorer](https://gnosis-chiado.blockscout.com/block/23194019)
independently confirms the latter block hash and timestamp. The 48 checked local
header observations were 112–919 ms after their timestamps. Those observation
times do not measure end-to-end propagation, but local reception delay cannot
explain absent canonical timestamps. Validator-side propagation delays could
still cause a genuine canonical skip or orphaned proposal.

At 15:34:40 the replacement never reached submission: Composer requested proof
at 15:34:38.472 and timed out at 15:34:39.223 after its 750 ms budget. Signer
ordinary-range preparation completed chunks at 15:34:38.858817 and
15:34:39.374302, with the final five-block chunk at 15:34:39.386120. This is a
local preparation/proof deadline failure, with no structural validation rejection
in the retained logs. They do not fully account for every millisecond or prove
cache behavior caused the delay. The next attempt dispatched at 15:34:43.753297.

## Every non-included event

IDs in this table are the suffix of `1-1790176983308395403:1:`. Full attempt IDs,
postbatch transaction hashes, canonical/parent hashes, dispatch times, candidate
hashes and individual relay results are in [summary.json](summary.json). Times
are UTC on 2026-09-23. Rows before 15:35:20 are setup.

| ID | Target UTC | Target block / slot | Outcome | Individual finding |
| --- | --- | --- | --- | --- |
| 3 | 15:32:55 | 23193924 / 24956255 | excluded | Absent from observed relay proposer duties; no builder bid or relay header/payload delivery; canonical Erigon block. |
| 4 | 15:33:00 | 23193925 / 24956256 | excluded | Single bundle simulated OK but candidate assembly marked one bundle failed; no bundle candidate. Non-bundle header delivered; proposer selected another block. |
| 5 | 15:33:05 | 23193926 / 24956257 | excluded | Three bundle candidates relay-validated; bundle-containing header delivered; no payload delivery; different canonical block. |
| 8 | 15:34:35 | 23193943 / 24956275 | slot_skipped | Canonical gap 15:34:30–15:34:40; remote Lighthouse empty. |
| 0:0:0 | 15:34:40 | 23193943 / 24956276 | prover_deadline | No submission: 750 ms Composer proof budget expired while signer ordinary-range preparation continued. |
| 22 | 15:36:25 | 23193963 / 24956297 | excluded | Absent from observed relay proposer duties; no builder bid or relay header/payload delivery; canonical Erigon block. |
| 32 | 15:37:15 | 23193973 / 24956307 | excluded | Absent from observed relay proposer duties; no builder bid or relay header/payload delivery; canonical Erigon block. |
| 40 | 15:37:55 | 23193981 / 24956315 | slot_skipped | Canonical gap 15:37:50–15:38:00; remote Lighthouse empty. |
| 41 | 15:38:00 | 23193981 / 24956316 | excluded | After skipped slot, proposer received no bid; builder first candidate arrived after proposer request. Underlying scheduling/simulation delay remains unattributed. |
| 45 | 15:38:20 | 23193985 / 24956320 | excluded | Absent from observed relay proposer duties; no builder bid or relay header/payload delivery; canonical Erigon block. |
| 50 | 15:38:45 | 23193990 / 24956325 | excluded | Absent from observed relay proposer duties; no builder bid or relay header/payload delivery; canonical Erigon block. |
| 51 | 15:38:50 | 23193991 / 24956326 | slot_skipped | Canonical gap 15:38:45–15:38:55; remote Lighthouse empty. |
| 58 | 15:39:25 | 23193997 / 24956333 | slot_skipped | Canonical gap 15:39:20–15:39:30; remote Lighthouse empty. |
| 71 | 15:40:30 | 23194009 / 24956346 | excluded | Absent from observed relay proposer duties; no builder bid or relay header/payload delivery; canonical Erigon block. |
| 73 | 15:40:40 | 23194011 / 24956348 | slot_skipped | Canonical gap 15:40:35–15:40:45; remote Lighthouse empty. |
| 74 | 15:40:45 | 23194011 / 24956349 | excluded | After skipped slot, proposer received no bid; builder first candidate arrived after proposer request. Underlying scheduling/simulation delay remains unattributed. |
| 82 | 15:41:25 | 23194019 / 24956357 | slot_skipped | Canonical gap 15:41:20–15:41:40; remote Lighthouse empty. |
| 83 | 15:41:30 | 23194019 / 24956358 | slot_skipped | Canonical gap 15:41:20–15:41:40; remote Lighthouse empty. |
| 95 | 15:42:40 | 23194031 / 24956372 | slot_skipped | Canonical gap 15:42:35–15:42:45; remote Lighthouse empty. |
| 97 | 15:42:50 | 23194032 / 24956374 | excluded | Absent from observed relay proposer duties; no builder bid or relay header/payload delivery; canonical Erigon block. |
| 103 | 15:43:20 | 23194038 / 24956380 | slot_skipped | Canonical gap 15:43:15–15:43:25; remote Lighthouse empty. |
| 118 | 15:44:35 | 23194052 / 24956395 | slot_skipped | Canonical gap 15:44:30–15:44:40; remote Lighthouse empty. |
| 119 | 15:44:40 | 23194052 / 24956396 | excluded | After skipped slot, proposer received no bid; builder first candidate arrived after proposer request. Underlying scheduling/simulation delay remains unattributed. |
| 130 | 15:45:35 | 23194063 / 24956407 | excluded | Absent from observed relay proposer duties; no builder bid or relay header/payload delivery; canonical Erigon block. |

## Evidence and executed checks

Public source: [builder, relay and Lighthouse logs](https://logs.chiado.gcd.ovh/).
The web browsing tool could not open this service; read-only HTTPS retrieval
succeeded. Dozzle API requests used explicit stdout/stderr and all level filters,
bounded ten-second windows, recursively split when a response approached 500
entries. Final maxima were 327 builder, 485 relay and eight Lighthouse entries
per retained window. Retrieved interval: 15:32:00–15:46:40 UTC. The service log
coverage is 47,537 builder lines, 4,864 relay records and 636 Lighthouse lines;
no final window reaches the 500-entry cap. Adjacent-window duplicates were removed.

The requested API path was `/api/hosts/{host}/containers/{container}/logs`, with
`from`, `to`, `stdout=1`, `stderr=1`, and repeated `levels` parameters. Exact
public IDs, commands and bounded retrieval logic remain in local `fetch.py`.
Canonical headers were fetched with `eth_getBlockByNumber` for each failed
height and its neighbors, selecting hashes and timestamps without copying data
directories. A public explorer request initially returned HTTP 403 through
Python; curl then succeeded. No tests or transactions were re-run for this audit.

Local-only evidence is under `artifacts/chiado-staged-fixed/failure-audit/`.
[summary.json](summary.json) records file hashes, sizes and availability limits.
These ignored raw logs/scripts are unavailable in a fresh checkout and have no
retention guarantee. The small sanitized per-attempt summary is the durable
record. Evidence checks assert nonzero inventory (24), ten strict canonical
skips with matching parents, ten remote empty-slot observations and thirteen
produced exclusions. `git diff --check` checks the documentation change.

No services, runtime configuration, wallets, protocol sources or databases were
changed by this investigation. No database copies were taken.

## Remaining concrete checks

1. Builder tracing keyed by postbatch transaction/bundle hash for execution
   block 23193925, target 15:33:00: obtain the candidate assembly rejection
   reason and exact transaction ordering. Its postbatch hash is in the summary.
2. The same tracing for 23194052, target 15:44:40, distinguishing the old :35
   bundle from the replacement: determine why no replacement-containing
   candidate was produced before cancellation.
3. For 23193981, 23194011 and 23194052, measure per-order queue/simulation and
   builder scheduling latency after payload attributes. Exact targeted raw
   bundle replay against the recorded parent/timestamp would separate execution
   validity from scheduling delay; those bundle bodies are not in the public
   logs, so that replay was not performed.
4. Proposer logs for the two setup header-delivery cases are needed to explain
   why a different payload was selected. Relay logs cannot reveal that decision.

No proof of a malformed composition appears in the available logs. Conversely,
absence of an explicit error and eventual source receipts do not establish that
every unsuccessful candidate was valid. These limits remain open rather than
being assigned to network conditions by default.


## Focused follow-up on the two composition questions

The public builder's complete all-level retrieval has INFO, DEBUG and WARN
records but no TRACE records. Additional explicit trace-only queries across
15:32:45–15:33:15 and 15:44:25–15:44:55 returned zero records. Focused queries of
public `chiado-reth-1` returned 54 records; these cover canonical processing and
do not expose bundle simulation or candidate insertion errors. All windows are
below the response cap. No logging settings were changed.

Both disputed bundles passed Composer's final signed-gas/fee execution preflight
against their exact target. This is local execution evidence, not an independent
replay of the builder's transaction ordering.

**15:33:00:** preflight used 113,305 gas and signed a 135,966 gas allowance. The
builder's final non-bundle candidate used 16,886,343 gas against a 17,000,000
parent/canonical block limit, leaving only 113,657. Appending the signed allowance
to that candidate would not fit without removing or reordering transactions.
This materially strengthens gas pressure as an explanation, while the exact
insertion position and rejected transaction are still absent from the logs.
The final candidate includes the builder payment, so its remaining gas is not a
trace of the gas pool at the failed insertion. The relay successfully validated
all seven candidates, each with zero blobs. It never validated our two-blob
postbatch in this slot; those successful validations cannot clear our bundle.

**15:44:40:** preflight used 4,389,089 gas and signed a 5,652,827 allowance. The
first submitted candidate still had zero bundles, but additional unfinished
candidates were chosen at 15:44:40.849721 (ID 371390) and 15:44:40.924241
(ID 371391), after the proposer's +124 ms request. They were not subsequently
submitted in the available logs. The first candidate's value is numerically
consistent with the replacement's measured gas plus the ordinary transaction's
fee, supporting late processing as a hypothesis. There is no transaction list
or candidate hash to establish that identity. Do not upgrade this observation
to proof that the replacement bundle was valid or that the old bundle caused
its delay. The old and new payload jobs overlap; their cancellation counters
remain insufficient to identify individual failures.

The conclusions above remain bounded: no explicit malformed-composition error
was found, gas pressure is a concrete setup hypothesis, and builder timing is
observed after the skip. Exact rejection reasons still require information not
published at this log level. The [upstream trace request](upstream-trace-request.md)
contains the minimum identities and questions; it is a prepared request only,
not a message sent to anyone.
