# Bounded postBatch admission reserves

The improvement replaces the uniform per-entry estimate only for measured,
bounded shapes under the enabled settlement policy. Final ordered L1 execution,
complete-effect validation, active gas/blob limits, exact calldata charging and
the configured gas margin remain mandatory.

The validated release is deployed on the existing Chiado network. All 870
sources in the exact public workload passed the canonical audit. Peak observed
packing increased from 34 to 39 sources per Sync, but the complete mixed run
used 38 posts instead of 37 and 7.69% more execution gas. This establishes a
bounded capacity improvement, not an overall efficiency improvement for that
heterogeneous workload.

## Scope and interpretation

| Work | Admission reserve | Evidence required |
| --- | ---: | --- |
| Leading anchor | 160,000 | Native single-rollup/single-ECDSA batch; one own root update, no calls/value/result; recognized previous queues |
| Clear previous deferred storage | 65,000 per stored entry | At most 127 old entries; at most one root update each, no calls/reentrant rows, at most 64 result bytes; no static queue |
| New simple deferred entry | 270,000 | Successful own-rollup entry, at most one own update, no inline/reentrant calls, at most 64 result bytes |
| Simple established-proxy call | 70,000 plus measured L1 target gas | Exactly one matching successful mutable leaf action; proxy registered at the canonical parent; no value/revert marker/gas override; input at most 256 bytes and actual target result at most 64 bytes; entry result remains empty |
| Other entries | Existing 370,000 plus measured L1 target gas | Retains the existing coarse estimate; final native execution is authoritative |

Intrinsic/calldata gas, the applicable calldata floor, required L1 companions,
the 600,000 admission framing allowance and the existing percentage margin are
separate. These reserves are estimates of protocol work, not consensus limits
or a promise of a fixed number of source transactions per Sync.

Consumption advances the queue cursor but does not delete its stored entries.
The next post clears both normal and static queues. Treating a fully consumed
queue as empty would understate subsequent posting work. The implementation
reads the canonical storage lengths and bounded shapes on every attempt;
unsupported queue shapes or unavailable reads retain the legacy projection.
The storage offsets are specific to the current native EEZ contract and are
asserted against populated Solidity storage; this does not detect arbitrary
contract-layout changes at runtime.
No durable estimation journal or speculative creation cache is introduced.

Existing-proxy eligibility belongs to each accepted outbound occurrence. It
cannot be inherited from another call with identical calldata, from a rejected
candidate, or from a proxy created earlier in the uncommitted candidate.
Final sizing rechecks the entry shape and exact call ABI after root stitching.

Discounts apply to at most 128 total entries, including the anchor. Crossing
that measurement boundary restores the old reserve for all non-anchor entries;
it does not impose a source-transaction count cap. Disabled policy keeps the
legacy estimator. Enabled numeric admission keeps its configured count limit.

A source that fits without previous-queue cleanup remains held if that cleanup
temporarily consumes its budget. The existing independent settlement policy
must clear the queue before retry; there is no additional empty-post trigger.

## Automated coverage

| Requirement | Implementation | Tests |
| --- | --- | --- |
| Measured shape and outcome gates | `composer/gas.rs` | Deferred/immediate boundary tests; registered/new/multi-proxy/reverted Solidity fixtures |
| Prior storage cleanup including consumed entries and static queues | `gas::previous_cleanup` | Rust storage-read/unsupported-shape tests; actual Solidity storage-layout and consumed/unconsumed cleanup ladders |
| Same admission/final estimate, occurrence isolation | `Projection::cost`, `Projection::batch_gas`, accepted immediate positions | Admission/final equality and identical-call/different-outcome regression |
| Size boundary and safe arithmetic | `PostBatchGasBudget`, `Projection::batch_gas` | Whole-prefix reserve restoration, atomic rejection and overflow regression |
| Temporary cleanup pressure retains sources | `PostBatchGasBudget::blocked_by_cleanup`, existing deferral path | Cleanup-only pressure versus truly oversized source/companion test |
| Larger complete Syncs and consecutive queue replacement | `dynamic_admission.rs` | Existing 30-source cases plus two consecutive 40-source waves on Ethereum/Gnosis, exact receipts/nonces/effects and independent follower |
| Complex work and real gas caps remain protected | Existing native posting validation | Retained expensive-new-proxy and 128-source mixed-wave fixtures |

The composer unit suite passes 194 tests and all-target Clippy passes with
warnings denied. Contract tests pass 14 cases on each of Cancun, Prague and
Osaka. Contract measurements use caller-frame gas before refunds, excluding
transaction intrinsic/calldata gas. At 127 prior entries with 64-byte results,
cleanup plus a 64-blob-reference anchor measured 7,150,333 gas against an
8,415,000 reserve. The worst measured deferred marginal is 256,665 gas;
existing-proxy bookkeeping is 46,148 rounded up, and cleanup is 58,262.
All remain below their respective reserves. The 65-byte deferred control
exceeds its reserve and remains excluded. All prior contract tests are retained.

The first release candidate passed both existing 30-source native cases, but
failed the new Ethereum 40-source packing assertion: it settled 32 then 8.
The existing setter returns `(bool,uint256)`, or 64 ABI bytes; the initial
32-byte bound correctly fell back after the warmup left that result in the
prior queue. The revised bound covers arbitrary successful results through
64 bytes, supported by the expanded contract measurements. No fixture,
workload, native assertion or contract runtime was changed to obtain that
coverage. The failed candidate was never deployed.

The revised normal Rust 1.94 release passed all six native scenarios:

- Existing 30-source single-Sync cases on Ethereum and Gnosis.
- Two consecutive 40-source waves on each profile, each wave in one Sync,
  with exact receipts, nonces, values and independent follower convergence.
- The Gnosis 128-source mixed wave after a smaller post.
- Gnosis expensive/new-proxy calls, complete-group splitting and explicit
  oversized-group rejection, with independent follower replay.

Both 40-source tests used the bounded model for the warmup and both waves.
Before the second wave the model charged 1,560,000 gas for clearing 24 stored
inbound entries. No native workload or assertion changed between the failed
and passing release. All 633 snapshot files were compared before reusing the
same test harnesses; the only differences were the runtime result bound and
the Solidity gas-test source. Composer, signer and follower were launched from
the separately frozen release directory.

At 23:58:36 UTC on September 15, the composer was replaced with SHA-256
`c55872ba63d584c4f147b106fd57b4456e872dd74cd9ee01f62bee049473fd56`.
The follower and signer are byte-identical to the preceding deployed version.
Assigned configuration and original data mounts were preserved; the prior
safe block retained its hash and state root. Both heads advanced, the queue
was empty, and the beacon was synced with 25 peers. The standard runtime
Compose file now points to the same tested composer binary.

## Public Chiado workload

The unchanged `PREPARE_PARALLEL=200` workload ran from 00:00:39 to 00:08:36 UTC
on September 16: 770 jobs, 870 sources, the same 14 scenario/count arguments,
28 send workers and `--no-verify`. All 520 inbound and 350 outbound sources
have canonical receipt, layer, post/Sync association and ownership evidence,
with zero audit gaps. Outcomes were 820 `Committed` and 50 expected
`SourceCompleted` from `revertCounter`. All 770 worker nonce checks passed on
both chains; runtime, protocol inputs and the prior safe state were preserved.
Protocol semantic assertions and an independent live follower were not run;
the native tests above cover independent replay.

| Measurement | Previous deployed `1eae…` | Bounded model `c558…` |
| --- | ---: | ---: |
| Verified sources | 870 | 870 |
| Source-bearing posts / physical blobs | 37 / 37 | 38 / 38 |
| Maximum sources per Sync | 34 | 39 |
| Mean sources per source-bearing post | 23.51 | 22.89 |
| PostBatch execution gas | 202,157,413 | 223,094,098 |
| Complete bundle gas, including L1 companions | 275,858,229 | 297,064,402 |
| Source-bearing settlement charges, xDAI | 0.207182078 | 0.346860570 |
| Median / p95 held-to-canonical completion, seconds | 126.250 / 256.602 | 146.747 / 272.286 |
| First source accepted to last completion, seconds | 262.406 | 308.123 |
| Gas-budget / preparation-time cuts | 23 / 8 | 22 / 12 |
| Excluded attempts / canonically skipped slots | 2 / 8 | 12 / 9 |

The three largest canonical Syncs carried 39, 38 and 37 sources and used the
bounded projection. The 39-source post is
`0x858e7edd67bc99c5b46254557fd800e8fb8b15c0a9f15cd39845729044a1d5fa`,
in L1 block 23,076,031, covering Sync 179,850. All 39 sources committed: 35
`counter-multi-tx` sources and four `multi-call-twice` sources. Its complete
bundle used 10,954,152 gas, 63.13% of that L1 block's gas limit; 96,743 encoded
bytes used 38.09% of the active two-blob allowance. It posted one physical
blob. These are observed workload sizes, not a universal maximum.

Of 51 projection openings, 30 enabled the bounded model and 21 retained the
legacy model. Canonical handoff joins associate 22 source-bearing posts and
556 sources with the bounded model, and 16 posts / 314 sources with fallback.
Opening the model does not imply that every entry receives a discount. Six
gas cuts explicitly recorded nonzero entry discounts. This change also charges
known old-queue cleanup, so it does not promise uniformly larger batches for
every preceding queue and call mix. The logs do not identify every reason for
fallback or every existing-proxy eligibility decision.

The aggregate gas increase is mostly postBatch work: 20,936,685 additional
post gas versus 269,488 additional companion gas. This sequential public pair
does not isolate the cause of that increase or justify a general throughput
or cost-saving claim. Parallel source order, grouping, accumulated state and
available submission opportunities differ. Setup mining took 91.736 seconds
versus 182.241 before, and the first source-bearing post also covered 26 setup
L2 transactions; baseline source posts shared none. No setup-drain barrier,
scenario-specific shortcut or source-fee adjustment was introduced.

Currency charges are a separate result: the unchanged fee policy selected
about 1.2 gwei post tips instead of 1.0 gwei. Companion effective prices were
about 1.0 gwei, versus 8 wei to about 0.5 gwei before; receipt blob prices also
varied. All captured paid posts, including setup/history, totalled 43 posts,
47 physical blobs, 297,630,040 execution gas and 0.348651168 xDAI. These totals
exclude standalone setup transactions and refundable worker funding.

The only recorded preflight failure was a setup-time parent/window change;
the unsent work was retained. The audit found no partial-settlement, effect
mismatch, poison-eviction or unexpected terminal-ownership outcome. At
00:14:24 UTC both chains were current, the beacon was synced with 25 peers,
the queue was empty and the prior safe hash/state were preserved. A separately
journaled 0.01 xDAI top-up restored manual-run readiness after measurement:

```sh
bash scripts/run-chiado-staged-performance.sh --check
bash scripts/run-chiado-staged-performance.sh
```

Compact results and evidence identities are in [shaped-admission.json](shaped-admission.json).
Broader estimates for unsupported entry/queue shapes and a controlled
explanation of aggregate post gas remain useful follow-up work. The validated
change keeps the conservative fallback and final execution checks intact.

Raw evidence, exact commands and executable identities are retained under
`artifacts/chiado-shaped-admission-v2-20260915/`; its `runbook.md` describes the
release, native, existing-network deployment and measured workload sequence.
The first candidate and its failed assertion remain in
`artifacts/chiado-shaped-admission-20260915/`.
