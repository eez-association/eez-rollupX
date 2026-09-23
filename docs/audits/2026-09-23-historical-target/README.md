# Historical-prefix target preservation and lighter Chiado workload

Status: original lighter live run failed with ten unmined sources; a separate paced resubmission recovered all ten unchanged transactions. Upstream correlation completed to the limits below. Historical-prefix correction passes its process regression and the existing skipped-slot/late-original regressions; formatting, Composer unit tests and workspace Clippy also pass. The correction is now deployed; comparable repeat run passed (870/870 source receipts; no resubmission).

Baseline: fork `1832f970884ca9460646a4cde87cca70eaffb3e1` plus the existing
working-tree deadline-replacement and posting-stage changes. Protocol gitlink
and initialized checkout: `3c0cdf4955941a5c23639346932b55b33babebbd`, unchanged.
The [previous stopped run](../2026-09-23-chiado-staged-performance-750ms/README.md)
records the observed historical-prefix failure and host memory exhaustion.
The [accepted design](../../proposals/skipped-slot-speculative-retry.md)
now explicitly covers historical-prefix target preservation.

## Lighter live workload

The running binary remains unchanged during this workload. Node image ID:
`sha256:ecd62f1c96ca8970055132815b7db8f419c7f0c70a5e950795cefc152c269edd`;
node binary SHA-256:
`fcdaeee70b806efdb273661df362cf069c3259f43382002ce563c87a85b48029`.
Proof budget remains 750 ms. The node entered this run with one prior restart;
that count is the baseline, not a restart caused by this run.

Command: `bash scripts/run-chiado-staged-performance.sh` with the existing
private profile, `EEZ_STAGED_SCALE=1`, `PREPARE_PARALLEL=4`,
`SEND_WORKERS=2`, `SEND_PAUSE=1.5`, `MULTISEND_BATCH=220`, and
`DEPLOY_MINE_TIMEOUT=1800`. The owned runner container has a 6 GiB hard
memory/swap-total limit, 2 CPU cores and a 512-process limit. The wallet lock
remains active. Funding and readiness checks passed; no additional external
funding was required.

The first container attempt exited before transactions because `bc` was absent.
Its log is retained. The second installed `bc` in the disposable container and
ran the unchanged helper. All 770 jobs prepared; all 2,850 setup transactions
mined in 97.32 seconds, and 2,830 predicted deployment addresses passed liveness.
The measured wave submitted 870 source transactions over 665.7 seconds. The
600-second receipt monitor ended at 860/870 mined; the runner exited 1 at
14:33:52 UTC. The original failure is retained. No semantic verification ran.
This helper uses `--no-verify`: receipts and throughput do not establish scenario
semantics or independent follower replay. Changing concurrency, workload size
and pacing prevents attributing differences to code performance.

Local-only evidence: `artifacts/chiado-staged-light/` and protocol run directory
`eez-core-protocol/tmp/e2e-staged-net/20260923-140401/`. These ignored artifacts
are not available in another checkout and include private runtime configuration;
only sanitized summaries belong in this audit.

## Regression method

The source-based process regression limits the batch to one blob and accumulates
three ordinary transfers across separate empty Sync boundaries before the pure-L2
interval expires. The builder captures and drops the historical-prefix bundle,
and the controlled consensus fixture omits that actual target slot. Assertions
require a dispatch at the next slot with the same execution height, exactly-once
transfer effects, and settled-prefix agreement with an independent follower.

Build/test container: `eez-fee-recipient-tools:1.94.0`, image ID
`sha256:7335715efd8b672d267ecc2e8a9fc11c449e35413b1a98ee26ba06065f9aab86`;
Rust 1.94.0 and Foundry 1.7.1. Cargo uses `--locked`, two jobs, and existing
download caches. The isolated container has a 16 GiB memory limit and four CPU
cores; it does not mount live datadirs. Prebuilt binary overrides are unset.
Integration contracts are built before the process test. Logs/datadirs are
retained under local-only `artifacts/historical-target/`.

The first fixture used 70,000 bytes of calldata per transfer, which exceeded
its one-blob admission allowance before reaching the intended component. It is
not a reproduction. Reducing each transfer to 50,000 bytes preserves a combined
150,000-byte history while admitting each individual transaction. The corrected
baseline reached the historical blob-prefix path, skipped the captured slot,
and failed the exact next-slot dispatch assertion after 12 seconds. Retained
`baseline-v2.log`: one selected, one failed. Candidate validation repeated that
same test input and passed (one selected, one passed). The correction passes the selected target through both prefix
callers and preserves an exact target for all candidates; unpinned catch-up still
selects a future target. Proof and submission cutoffs are unchanged.

## Public builder/relay and canonical-chain correlation

Evidence window: source-phase targets 2026-09-23 14:12:50–14:23:55 UTC.
Of 129 dispatched attempts, 100 included at target (77.52%), 13 targeted skipped
slots (10.08%), and 16 targeted produced blocks without inclusion (12.40%). No
preparation-failure event appears in that source-phase window. This denominator
is posting attempts, not every network slot or user transaction. Different
workload size/pacing and external proposer schedules prevent a controlled claim
that the code improved or worsened inclusion.

All 13 skipped-target timestamps lie strictly between consecutive canonical
execution headers with matching parent hashes. For example, block 23193127 is
14:18:35 UTC and block 23193128 is 14:18:50 UTC: :40 and :45 are absent, not merely
late at our observer. The public [Chiado explorer](https://gnosis-chiado.blockscout.com/block/23193128)
independently agrees with the latter block hash and timestamp. The remote
Lighthouse log also reports empty slots 24955364 and 24955365. The longer gaps
following consecutive missing slots match the accepted one-replacement-per-head
limit. Ten skipped attempts have their next attempt target at +5 seconds;
two have +10 seconds and one +15 seconds.

Read-only evidence was obtained from the public [Dozzle service](https://logs.chiado.gcd.ovh/)
for `builder-rbuilder-1`, `relay-api-1` and `chiado-lighthouse-1`. The relay's
website-linked data API returned 404, so the investigation used its service logs.
The Dozzle range API requires explicit level filters; initial requests returned
empty results without them. Final relay and Lighthouse retrieval used bounded
10-second windows with all levels, no 500-entry truncations. Bulk logs remain
local-only; no operator credentials or private configuration are included here.

Every included target has an execution-payload delivery in this relay's logs.
None of the 16 excluded targets has a bid or payload delivery from this relay,
and their canonical hashes differ from this builder's submitted candidates:

- Seven excluded slots are absent from the relay's proposer-duty list and have
  no builder bids. Their canonical blocks identify Erigon in extraData and share
  fee recipient `0x60a69ad71c6cb3c1fbc7afa8be898ba6f0988022`. The logs establish
  lack of this relay path, not the validator operator's configuration or motive.
- Six have bundle-containing builder bids and relay-validated candidates, but
  no bid/payload delivery from this relay. Example: block 23193129, slot 24955367,
  target 14:18:55 UTC. A bundle-containing candidate was submitted at
  14:18:53.879548 and validated by the relay; canonical hash
  `0x52706b1191f50e459e6c0deff0f49a746c5a2670d91bdbf1d6c15d90a8ea58c1`
  is a different block. We infer non-selection of this relay's bid; logs do not
  reveal whether the proposer used local execution, another relay or a fallback.
- Three occur immediately after skipped slots: 23193126/24955362,
  23193167/24955408 and 23193170/24955412. Payload attributes arrived at the
  builder only about 1.6 seconds before target. No timely bundle-containing
  candidate was submitted; the last case's sole non-bundle bid was at +0.885 s.
  Old/new-slot build jobs overlap at the same execution height. Their cancellation
  summaries show two received bundles but at most one successfully simulated.
  The old timestamp-bound bundle is expected to expire; that count alone does
  not establish rejection of the new bundle. These logs do not identify which
  simulation failed or why; a simulation trace is
  still needed to distinguish upstream scheduling/cache behavior from invalid
  retry input. Do not claim all exclusions are explained solely by proposers.

The builder also reports twelve file-watcher failures due to open-file limits
and two inconsistent-provider warnings in this window. These are upstream
health failures; their causal contribution to specific exclusions is unproven.
Our excluded attempts were dispatched 1.314–1.652 seconds before target, not
necessarily received or simulated at that instant. These outcomes existed in the
earlier stopped run too (five produced-block exclusions among 26 setup-window
attempts); that different workload and host OOM make its rate incomparable.

## Ten unmined sources

The ten missing receipts all belong to `multi-call-twiceL2` jobs. Each source's
last recorded dispatch carried two prior charged failures. The following
canonical exclusion charged the third failure, reaching the existing
`MAX_BUNDLE_ATTEMPTS=3` disposition. Seven reach that limit at L2 Sync 429745,
three at 429770. Their source nonces remain unspent, and the held queue is empty.
For example, source `0x2a72034aacf1b0f45886015ae5a1af9ed8c3c955205729b440d304dd5f53495f`
from `0x6EFeAa31C3f485159672Be577acd29118354AE33` has nonce 10, while latest and safe
source nonce remain 10. Earlier skipped slots did not charge those retries.
This is not evidence that the call shape is unsupported: the failed posting
opportunities are established independently above. The limit turns repeated
non-inclusion into a required client resubmission. Changing that policy is a
separate contract change; it is not silently relaxed by the historical fix.


The local node committed 120 observed canonical L1 blocks at a median 549.8 ms,
p95 952.5 ms and maximum 1,258.0 ms after their block timestamp. These are local
wall-clock observation delays, not network propagation measurements; timestamp
gaps establish canonical skips independently of this timing measurement.

After preserving the original result, the existing helper's `--resend` mode
reused only the ten unmined jobs under the same wallet lock and original snapshot.
One sender, five seconds between submissions, the same resource limits, and a
180-second receipt monitor were used. All ten original signed transactions
mined with status 1; resend took 50.3 s and monitoring 1.8 s. The deployed binary
and transaction bytes were unchanged. This is a successful recovery, not a pass
for the original performance run, and it does not provide scenario semantic
verification. The node's restart count remains at its pre-run value of one.

All ten recovered receipts were checked against canonical L2 hashes and were
covered by safe height 431525. Owned workload containers were removed only after
recording their exit statuses and preserving run artifacts. The shared network,
wallet pool, original profile and datadirs were retained.


## Candidate validation

The corrected baseline dispatched its original prefix for execution block 15 at
14:40:52 UTC. At 14:40:54.003765 the scheduler correctly chose block 15 with the
next timestamp, 14:40:57. The historical helper then replaced that with block 16
at 14:40:54.025382; its required parent did not exist and the regression failed.

With the correction, the same regression dispatched block 15 for 14:56:26 UTC,
then dispatched block 15 for 14:56:31 at 14:56:28.305588. All three transfers
settled exactly once and the independent follower agreed with the safe prefix.
The next ordinary dispatch targeted block 16 at 14:56:36. These controlled
process results establish the target-preservation fix; they are not a live
inclusion-rate comparison.

The baseline and candidate used the identical test executable, signer and
follower. Composer SHA-256 changed from
`ca86aa21d1ffdb030857fc7360325316fad6f2dde68341877637e05ada2c7769` to
`aac919589fa39583ccbddb543b300eea0f4f87b2b7fde472d2c7234f6b62b367`.
Full executable identities and local evidence digests are in
[summary.json](summary.json).

The new process test was built/run using Rust 1.94.0, `--locked`, package
`eez-node-e2e`, target `dynamic_admission`, exact test
`historical_blob_prefix_preserves_the_skipped_slot_replacement_target`, and
`--test-threads=1`. Final validation reused that source-built test executable
for these exact existing tests, serially, with prebuilt overrides unset:

- `automatic_selection_survives_a_canonically_skipped_slot_without_shrinking`:
  one selected, passed.
- `late_original_settlement_replaces_the_timeout_branch_without_losing_users`:
  one selected, passed.

There is no wire, storage, protocol or configuration migration. The change is
local target selection in Composer; signer and follower behavior is unchanged.
Deploying or rolling back a node still has the existing in-memory queue loss and
client-resubmission boundaries documented in the deployment runbook. At completion of the initial validation, no candidate image had been deployed.
The subsequently authorized rollout is recorded below; the proof budget remains 750 ms.


Additional checks:

- `cargo +1.94.0 test --locked -p eez-composer --lib`: 222 passed, none failed.
  The existing `held_pool::tests::profile_blocked_backlog_discovery` is ignored
  because it is a manual timing profile; it is not claimed as coverage.
- `cargo +1.94.0 fmt --all --check`: passed.
- `cargo +1.94.0 clippy --locked --workspace --all-targets -- -D warnings`:
  passed.
- `git diff --check`: passed, including the final audit update.

The full E2E, Kurtosis, protocol Solidity and release-artifact matrices were not
rerun. This correction changes local Composer target selection, with no protocol,
ABI, dependency, configuration or signer-validation changes. The three scoped
process regressions exercise prefix splitting, skipped-slot recovery, and late
original inclusion; they do not establish every live builder/proposer timing
case. The three unresolved live exclusions still need bundle-specific simulation
traces from the builder. Resource stability under the paced workload is not a
maximum-throughput measurement.


## Authorized deployment and comparable repeat

The user authorized deploying the correction and repeating the lighter workload.
The release build uses the same Rust 1.94.0 tools image, `--locked --release`,
`-p eez-node --bin eez-composer`, two build jobs and a 16 GiB / four-CPU limit.
This is a local development image, not published CI promotion. The previous
node image and private configuration are retained. A stopped-writer database copy
finished before the user instructed not to take database copies; that newly
created copy was removed immediately. No new database backup is retained. Only the node image changes; the proof budget stays 750 ms.

Repeat workload: scale 1, 770 jobs / 870 source transactions, preparation workers
4, send workers 2, send pause 1.5 seconds, MultiSend batch 220, setup receipt
limit 1800 seconds and source receipt limit 600 seconds. The runner retains the
6 GiB / two-CPU / 512-process limits and shared wallet lock. Existing funding and
nonce readiness checks passed. This remains `--no-verify` and is not a semantic
scenario test. Deployment and repeat-run results are recorded below.


Deployed image: `eez-node:historical-target-14613c0efabf`, ID `sha256:d8bac330a05aa0fe3653960ebc170a55c0608e65892ded85bd0477f3079059e7`.
Running Composer SHA-256: `57a55d8f114e5581a7b2ce5e34c82881c9144c5f39547a54c95a26810e2df37b`;
verified against the release build. Started at `2026-09-23T15:22:56.584721616Z`.
The previous node exited cleanly (status 0). Rendered and live comparisons confirm
unchanged node environment/mounts and unchanged signer/Lighthouse instances.
Both RPCs report synced; initial startup logs contain no errors.

The post-upgrade smoke transaction
`0xb508d3f9417f6948a8223567cf4d4528f53e1bb6d3c18ee3275030c729055c5d`
settled in canonical L1 transaction
`0x4dfa8628f7b3b41fe5b88220e55eb8c985ed3bf5b94ba6730a78cf2e79c748c3`
with status 1. Its one blob was retrieved from Lighthouse. The repeated staged
run started afterward; local-only diagnostics are in
`artifacts/chiado-staged-fixed/`.


### Repeat result

The runner exited 0 after 1,211.361 seconds. All 770 jobs prepared, all 2,850
setup receipts had status 1, and all 2,830 predicted deployment addresses passed
liveness. Preparation took 386.863 seconds; setup receipt collection took
117.408 seconds (previous run: 97.320). The paced source send took 664.128 seconds
(previous: 665.669); the receipt monitor then finished in 2.036 seconds.
All **870/870 source receipts** had status 1 on the first run, without resubmission
(previous original run: 860/870, then ten recovered separately).

Independent RPC queries verified all 870 receipt block hashes against canonical
headers: 520 L1 sources and 350 L2 sources. Safe L2 height 434980 covered every
L2 source receipt. Final queued, in-flight, blocked and unknown counts were zero.
These are receipt/canonical-settlement checks; the workload remains `--no-verify`
and does not establish scenario postconditions or independent live follower replay.

The source-send target window is 15:35:20–15:46:25 UTC. The denominator is actual
dispatched posting attempts, with target timestamps rounded to the same five-second
window boundaries as the previous comparison:

| Source-send outcome | Previous lighter run | Deployed correction |
| --- | ---: | ---: |
| Included at target | 100/129 (77.52%) | 112/131 (85.50%) |
| Canonically skipped target | 13/129 (10.08%) | 9/131 (6.87%) |
| Produced block without inclusion | 16/129 (12.40%) | 10/131 (7.63%) |
| Preparation failures | 0 | 0 |

Across setup and source phases together, there were 116 included, thirteen
produced-block exclusions, ten skipped targets and one preparation failure.
The source-send windows have comparable workload, pacing, build profile and
resource limits. They have different proposer schedules, chain state, cache state
and network conditions; the improved observed ratio is not an isolated causal
performance claim for this code change.

### Retry behavior and remaining limits

All nine source-window skipped targets were verified as actual gaps between
consecutive canonical headers. Eight had their next dispatched target at +5 s.
One had +15 s: the canonical chain went from 15:41:20 to 15:41:40, omitting :25,
:30 and :35. The original :25 attempt was replaced for :30 at the same execution
height. The existing one-replacement-per-observed-head limit then waited for a
new head, followed by dispatch for :45. This is the documented consecutive-skip
limit, not recurrence of the historical helper choosing a nonexistent parent.

Nine replacement hooks ran across the complete workload. Eight dispatched for
the exact next slot. The exception was during setup at 15:34:38: preparation took
409 ms, then the proof operation exceeded 750 ms at 15:34:39.223914. Signer logs
show ordinary-history preparation still progressing across that cutoff: a
64-block chunk completed at :38.858817 and the following 21-block chunk at
:39.374302. This supports preparation/replay work contributing to the timeout;
these logs are not a complete breakdown of every millisecond. The later attempt
dispatched at 15:34:43.753297. No proof budget was changed during the run.

Three historical-prefix splits occurred, with zero historical fallback failures
and zero `target_parent_unavailable` deferrals. This live run did not combine a
historical-prefix dispatch with a skipped target; that exact combination remains
covered by the baseline-failing/candidate-passing controlled process regression.
That fixture uses 500/1000/1500 ms preparation/proof/submission budgets, whereas
the live network retains 500/750/700 ms. Target preservation is established;
success under every live proof deadline is not claimed.

The runner's observed cgroup memory peak was 473,116,672 bytes (451.2 MiB), with
zero OOM events. The node's post-upgrade restart count remained zero; the signer
was not restarted. The owned workload container was removed after preserving its
exit status, logs and CSVs. Runtime services and datadirs remain in place.
