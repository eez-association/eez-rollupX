# Merge-readiness validation

Status: **live validation passed; CI blocked, not merge-ready**. This record follows the
[recovery repair and review](bug-fix.md); it does not erase the
[failed live stress test](stress-failure.md).

## Candidate identity

- Implementation head: `6f8620ecfe872b27214b9fa1063012ee8bca1036`.
- CI: [37994970816](https://github.com/eduadiez/rollupX/actions/runs/37994970816).
- CI merge: `76b210bcd9cc22e429865d483fa2d948c9793780`, with base parent
  `1eab3955775aec75612a7144d70f8245771ba97d` and the implementation head above.
- Implementation/CI merge trees both:
  `a9a9657205b9064201421887268dc47f89649fd6`.
- Protocol: `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized and clean.
- Cargo.lock SHA-256:
  `ffafea021117bc17cec6f5c36ba7c022dc51aa5728cd7ae4a7777ffed434e4ef`.

The preceding `cb17ef2` CI run was automatically cancelled when the reviewed
follow-up was pushed. Its passed lint lane does not establish a full CI pass.

## Final-source local checks

Rust 1.95.0, locked dependencies, LLVM 22, four build jobs, existing local target
cache. Node-common units: **15 passed**, zero ignored or filtered. Full workspace
Clippy with all targets and `-D warnings`, formatting and whitespace checks passed.

With `EEZ_TEST_PREBUILT`, `EEZ_TEST_RUNTIME_BIN_DIR` and
`EEZ_TEST_PROOF_SIGNER_BIN` unset:

```bash
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  conditional_settlement_requeues_ordinary_stale_seed_descendants \
  historical_prefix_replacement_keeps_the_empty_boundary_with_a_configured_coinbase \
  --test-threads=1
```

**2 passed / 0 failed / 0 ignored**, 134 filtered out, 192.56 seconds including
the fixture's signer rebuild. The test executable build took 6m03s. This repeats
the decisive 20-transaction recovery and nonzero-coinbase process checks on the
final implementation, including independent follower replay in the recovery
regression. It is not the complete CI matrix or a live test.

| Local executable | SHA-256 |
| --- | --- |
| `node_e2e` | `ff13c575303959653655399876cbe7a3234971432817f54b3acc04d09eaf6403` |
| `eez-composer` | `749eaeed63ac01aec6442ef90dbc68cc75803100764571f0d30113a97648d8a7` |
| `eez-follower` | `a9e41ba1c1425aac5fe070694aa107f09a0df0217a7c76d4b5e433c5632bd2d0` |
| `eez-proof-signer` | `a71733f6f3acd747957f3feb91db5771e961a028667b9d1153e44be0ba85bd0c` |

Local-only logs are retained under `/tmp/rollupx-pr79-ci-review-y7axfhr9/`:
`composer-only-recovery-units.log`, `composer-only-recovery-clippy.log`,
`final-source-process.log`, and `final-source-logs/`. They are not available
from another checkout. No failed final-source process run was retried.
Process log SHA-256:
`f47bd1d2f0ce1b63f9fba34ce3c560d3e327447aab9a04862053e2a5408e7b1e`.

## Independent historical replay baseline

At 21:54:14 UTC, a separate follower was started from candidate A's verified
node digest `sha256:0eeb551755a060d7cd845c0cd4dbe871eb33dec1c55eba924d0e58e21da9a85c`
(source `c3061cb1883eee253890d8310e5393deeb7b4335`). Its purpose is historical
coverage and old-follower/new-Composer compatibility, not testing the new
recovery adapter. It has a fresh private datadir, the original read-only genesis,
only public L1/Beacon/deployment inputs, no supplied signing credentials,
no sequencer overlay, and zero inbound/outbound peers. Its separate loopback
ports and container are owned by this validation; no existing service was
stopped or reconfigured.

The follower authenticated L1 history and its genesis reference, then started
scanning from deployment block 23412936 with local L2 head zero. The existing
100000-L1-block scan chunk hydrates historical blobs before applying its L2
blocks. Initial height zero was not a stalled scan. Once execution started,
the owned container was capped at four CPUs and 8 GiB memory/swap to protect
the shared canary. Docker initially refused a memory-only update because the
swap limit also needed setting; that rejected update changed no resources.

Initial catch-up completed at 22:08:14 UTC: **189270 blocks** replayed from
genesis. By 22:08:24 it replayed another 755 blocks and followed the live chain.
No Composer database, peer synchronization, unsafe overlay, manual L1 backfill,
or state override was used. The follower's blob source recomputes KZG versioned
hashes before decoding and executing settlement payloads.

A read-only comparison then verified all **641** previously settled stress-test
receipts against the retained record, including exact block hashes, status and
gas. Main and follower agreed at safe L2 block **190025** on block, state,
receipt and transaction roots. Both had the workload account at nonce **692**
and balance **49025389173053250 wei**; the prior 48 requests remained unsettled.
Safe block hash:
`0x00f15ff20690297fc25254a0ec1ae7b0af254d4f078304eaf3cafc4f880ffa63`.
State root:
`0xaf3b13211304c3bdd5788c61c565fc8301e46c06a8ca9d8465759ce1e9831493`.
The local-only `replay-a-baseline.json` SHA-256 is
`dbb3e585d3e3b075dfd4adb4838210d7916492f8b5496f83d12c57595b4e003e`.
This validates the old canary's settled history, not the unreleased fix.

## CI progress

Final-head lint passed. Debug unit tests passed: **1388 passed / 3 skipped**,
49.287 seconds of test execution after compilation, with retries disabled.
The three `#[ignore]` lib/bin cases are the existing manual backlog profile,
signer cold-span feasibility measurement and captured-fixture inventory utility;
they are exclusions, not coverage. The release producer passed **30** release
vector/blob-evidence tests with no skips. All eight native release-artifact shards
passed: **40 embedded + 92 cross-chain tests**, each executed once with no retry
passes. Per-shard skipped counts mostly select other suites/partitions; the four
inventory-wide ignored cases are the live-enclave replay test (owned by Kurtosis)
and three explicit large/captured native-history feasibility tests. They are not
native-shard coverage. The delayed cross-chain arrival/replay test, displaced
20-transaction recovery and configured-coinbase regression all passed in these
release shards. Kurtosis and the aggregate gate **failed**; details below.

## Candidate-C canary handover

Only the authorized Composer and signer were gracefully stopped, both exit zero,
at 22:16:37 UTC. Their stopped databases were cold-copied with `cp -a
--reflink=auto` through read-only source mounts into separate candidate-C paths.
Original operator databases and candidate-A copies remain intact. No contract,
genesis, funding, key, Lighthouse, explorer or monitoring change was made.

The explicit private Compose overlay pins:

| Role | Published linux/amd64 release digest |
| --- | --- |
| Node | `sha256:0cd439980f354725c7a9ea314565f49e1fafbaffa857812a41e37a9e737b5f89` |
| Signer | `sha256:9e66c0ef402828d84fcfa6ab863ad40397295efb1491e12f2598d30781e9bc5d` |

These are the CI merge revision above, with the exact pinned protocol. The
producer log binds its source to these push digests; the CI Kurtosis consumer
successfully pulled the immutable runtime payload and ran the exact-revision
artifact verifier before starting its suite. Image labels, platform, build
profile and running image IDs were checked locally. The CI payload registry
denied local access with existing credentials; no credentials or package
permissions were changed, and no missing release artifact was rebuilt. The
public runtime images are the producer's bytes, not local source builds.

Rendered configuration and actual previous container settings agreed except
for the intended image/datadir changes and explicit zero delay. P remains 2,
L1/L2 intervals 5000/1000 ms, proof seed 750 ms, submission slack 700 ms, policy
interval 900000 ms and maximum unsettled span 150 blocks. The obsolete preparation
setting is removed by the overlay. Operator `.env` remains unchanged; base-only
Compose startup would not select this canary. The private local README records
the exact overlay restart command and rollback boundary.

Signer started at 22:20:05 UTC, Composer at 22:20:11. L2 RPC returned at
22:20:12, approximately 215 seconds after stop, including the cold copy and
preflight. Canonical startup waited for embedded L1 peer catch-up; the first
readiness probe correctly rejected the still-validating correlation index.
Normal peer sync completed without manual payload import. By 22:22:04 both
chains reported not syncing; subsequent readiness and all four pre-handover
safe/finalized L1/L2 sample hashes/state roots passed. No startup failure was
hidden by restarting again.

## Recovery of the failed workload

Both known workload-checkout wallet locks were held before stopping services.
The previous workload had 48 unsettled signed intents, nonces 692–739; 44 hashes
were still in the old pool. Startup logged 44 backup reinsertions, but that
upstream message counts returned results without testing whether they succeeded
(`maintain.rs` at the pinned Reth revision). It does **not** establish restoration.
After canonical catch-up none of the 48 hashes was available. The initial status
interpretation of that log as 44 successful restores was corrected.

All **48 exact retained signed intents** were consequently resubmitted once;
there were no new signatures, replacements, nonce skips, funding, or ambiguous
RPC retries. All executed successfully, advancing nonce 692 to 740 with exact
receipt gas cost **10080049392000 wei** and no other balance change. They settled
in L1 transaction
`0x3a5c1a6b99b4cbf5f234bc59115bb7e8e6c840579fd38dba4c58656521691867`.
The independent L1/Beacon-only follower reproduced all 48 receipts, nonce and
balance. These are explicit restart-recovery resubmissions, **not** evidence for
automatic no-resubmission reorg recovery. That behavior is covered by the source
regression and the separate uninterrupted load test.

Backup reinsertion may run before the DA guard becomes available. The available
startup log does not retain individual backup insertion errors, so that cause
is consistent with source ordering but not directly proven for these 44 results.
No durable queue or backup guarantee is introduced by this PR.

## Full 15-minute ordinary load: passed

From 22:25:01 to 22:40:01 UTC, the release candidate accepted **4500** zero-value
self-transfers at 5 tx/s for **900.000089 seconds**, with **zero skipped sends**,
no load error, no restart and no client resubmission. The dedicated funded
workload account was distinct from the poster; both wallet locks remained held.
The gas bound was 21000 per transaction, maximum fee 100000000 wei and priority
fee 10000000 wei. No balance top-up was needed. All 4500 successful receipts
became canonical and settled across **seven** successful L1 blob transactions.
All seven full committed blobs were retrieved. Independent external L1 receipt
and block agreement, configured poster attribution, and the actual registry
execution event's final commitment were checked; receipt success alone was not
treated as full consumption.

The fresh L1/Beacon-only follower reproduced **every receipt** (hash, block,
index, status, gas and price), **700** containing/terminal blocks and their state
roots, and the sender's exact nonce/balance. Its blob source recomputed KZG
versioned hashes before decoding/execution. This is independent follower DA
reconstruction, not a run of the locally inaccessible standalone CI verifier.
Terminal checked block: **192160**, hash
`0x943a00b20cb45becd726e88becd5ff977433520dd88d98cc904083c9aec98427`.
Nonce advanced **740 → 5240**; balance decreased by exactly receipt gas,
**945004630500000 wei**. The cross-chain queue remained empty at completion.

The observations captured one displaced block with **20 workload transactions**.
All 20 later had different canonical receipt block hashes and independently
replayed successful settlement, without any resubmission. This exercises the
specific live 20-to-16 sender-cap failure mechanism after the repair. The
one-second observation cadence may miss other replacements; 20 is a measured
lower bound, not a count inferred from final receipts alone.

In the interior measurement window, **690 of 717 ordinary blocks** contained
multiple workload transactions; 27 were empty and none contained exactly one.
Positions 1/2/3/4 carried **1860/960/840/840** transactions. All 179 measured sync
positions remained empty, as explicitly retained by this design. This does not
promise transactions in every non-settling block. Among 180 planner decisions,
168 selected ordinary continuation and 12 selected preparation. Decisions were
4–15 ms after the configured cutoff (median 6 ms), never before it. A preparation
decision is not itself a settlement count.

First-inclusion **observation** latency for 4490 sampled transactions was p50
0.783 s, p95 1.984 s and maximum 4.985 s. These are one-second poll observations,
not exact execution latency or finality latency; ten tail transactions were not
observed before the load loop ended. Their receipts and replay were still checked.
The node log during the sampled load contained no WARN/ERROR or relay-drop event.

The same-rate candidate-A test failed after 248.8 seconds, so there is no matched
successful full-duration baseline for a throughput/latency speedup claim. The
comparison establishes that the previously failing workload now completes and
settles, not a general capacity benchmark. This live load contained ordinary
transfers only; cross-chain semantics, partial consumption and controlled fault
cases are covered by the native/Kurtosis suites, not invented from this load.

The small [sanitized result](canary-summary.json) is committed with this record.
Full receipts, observations, selected blobs, logs and the workload helper remain
private/local-only under the retained canary directory; another checkout cannot
retrieve them. Helper SHA-256 for this run:
`9c47e29d75eb4046792998e599990c37d64a7f9eeaa5431b671bbed0b5bc75e1`.
Retained evidence SHA-256:

| File | SHA-256 |
| --- | --- |
| `stress-15m-c/summary.json` | `c0150ac64d37561ffa3ec3783f9374f38ce2f45d68c344080dd5c713509ff2d3` |
| `stress-15m-c/independent-replay.json` | `e4260f512cce51590ce1fcdb9335c68f3aca5b379a52836f53714411a51624dc` |
| `stress-15m-c/settlements.json` | `7fd767866833dc5a85d80e065584deb9c156d98e6abe75c7d0e66b59f2ab6302` |

## Nonzero-delay check and finalization

After draining and preserving the zero-delay run, both canary services exited
zero. Separate cold copies were made; the same node image restarted at 22:44:13
UTC with P=2/delay=250 ms. All other effective settings/images were unchanged.
Startup reported trigger offset 2250 ms. Canonical catch-up completed normally;
all four pre-change safe/finalized checkpoint hashes and state roots matched.

The bounded check sent **900 transactions in 180.000100 seconds** at the same
5 tx/s rate with zero skipped sends, no load error and no resubmission. Initial
verification failed because the external L1 provider returned `null` for a
settlement receipt already visible locally; the helper dereferenced it without
waiting. That first failure is preserved in `stress-delay-c.log`. It is not a
clean first-pass verification run and is not hidden as a successful retry.

Only the read-only validation was corrected: allow at most 120 seconds for
missing independent-provider receipt/block data, retaining exact comparisons
and failing on mismatch or timeout. The retained 900 signed-intent identities
and receipts were revalidated without loading the sender key or sending any
transaction. Separate `stress-delay-c-readonly-verification/` evidence passed:
**900 successful canonical receipts**, **two settlements/two full blobs**, exact
nonce **5240 → 6140**, and gas/balance difference **189000926100000 wei**.
Independent replay checked every receipt and **145** containing/terminal blocks
and state roots. All measured decisions honored the 250 ms cutoff; no early
planning decision was observed. The native release CI additionally passed the
cross-chain arrival-between-P-and-cutoff process/replay regression at 750 ms.

This shorter run validates configuration/timing and correctness, not a matched
performance comparison or evidence that 250 ms improves throughput. The public
summary retains its separate observation-latency and occupancy measurements.
Original delay-helper SHA-256:
`c4ae4bf5f6afa35aaf282aec026ca6fe46c96d252932e828165f40dfd737a47c`.
Read-only verification summary SHA-256:
`fd99007283996a6e12d4a1827256bc41145b03645d4f25de0517079d0ee58e1b`;
replay SHA-256:
`52f1926652a1acbb62e5df0b577e3f53de294787be733df12f0fb7b64dcd5625`.

At 22:54:02 UTC both nodes had finalized the workload terminal **192610**, hash
`0x22c94b4f57d1f1555791d200c7cb566bc6660141a6fb92ddcd4d998331c0a2ce`,
with identical state root
`0x0e0a3ad698029aa9cba38d4ec525c6287a4f5ba865b99aadbc9ae9c9133b32fd`
and workload account nonce/balance. The cross-chain queue was empty. The owned
validation follower was then stopped cleanly (exit zero), retaining its full
logs/datadir, and both workload locks were released. Composer and signer remain
running at P=2/delay=250 ms on the explicit private overlay. No local compilation
ran during either timing measurement; no contracts, genesis or operator keys
were changed.

## Remaining CI blocker: optimistic receipt used as stable evidence

The first Kurtosis failure is retained in CI artifact
`kurtosis-e2e-37994970816-1`, artifact ID **11649900943**, and locally under
`/tmp/rollupx-pr79-ci-review-y7axfhr9/kurtosis-first-failure/`. The job log is
`final-ci-kurtosis-failure.log` in its parent directory. No diagnostic retry has
been run. All four cross-chain wave modes and state chaining passed before the
protocol suite reported **32 passed / 2 failed / 0 skipped**:

- `revertFromOtherChainL2`: upstream verification retained the first optimistic
  receipt's block **906**, then found no expected table entries there.
- `topLevelStaticReentrantCounterL2`: upstream returned PASS, but the independent
  verifier rejected a current receipt block different from the logged **906**.

The node log records the first attempt at L2 906 excluded from L1 target 151,
retention of the cross-chain source under Composer ownership (not ordinary-pool
reinsertion), recomposition at L2 **912**, and canonical reconciliation at L1
152 with source dispositions. The matched settlement was
`0x250f2a7ebbb47de6f299f51c41b1603559120eb4d9eef402c5d0e469389415cc`.
The pinned test helper's `_poll_receipt` returns the first receipt without
waiting for canonical settlement; `network-scenario.sh` saves its height and
uses it later. A by-height settlement lookup can then refer to the replacement
block, not the original receipt's block hash. This explains the stale evidence;
it does not turn either failed scenario into a pass.

The clean proposed repair is to wait for a canonically settled L2 receipt before
recording the verification target, track the transaction through disappearance/
movement under one deadline, and retain the strict receipt, table, causal and
blob assertions. The helper lives in `eez-association/eez-core-protocol`; separate
authorization was requested for a dedicated test-runner-only branch/PR and pin
update. No core source or pin has been changed. Until that repair and a fresh
complete CI run pass, PR #79 remains draft and must not be merged.
