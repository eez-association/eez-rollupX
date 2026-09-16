# Chiado settlement and L1 arrival review

For the latest deployed candidate, its 870-source public-network run and the
manual launcher, see [bounded admission reserves](shaped-admission.md). It
raised observed peak packing to 39 sources, while the complete mixed workload
used more posts and gas; the audit records both results. The preceding
[L1 gas attribution](l1-gas-attribution.md) corrects destination-gas accounting.
The preceding
[staged gas-accounting comparison](gas-staged.md) records its two earlier runs.
The earlier [existing-Chiado performance comparison](chiado-performance.md)
covers preparation and skipped-slot handling. The investigation below records
the preceding settlement and peer-discovery findings.

The audited settlements are correct. Canonical receipt/blob verification and
an independent follower cover **28 posts and 5,360 consecutive L2 blocks**,
124,116–129,475, through L1 block 23,066,987. The late L1 arrivals exposed a
separate beacon-networking problem. The discovery-seed filename and local ENR
advertisement are corrected and deployed. Discovery now adds peers, but complete
networking recovery is **not** established; invalid bootnode responses remain.

All times below are UTC on 2026-09-15. Compact results and executable identities
are in [summary.json](summary.json). Detailed local evidence and verification
scripts are retained under
[`artifacts/chiado-settlement-review-20260915`](../../../artifacts/chiado-settlement-review-20260915).

## Why three opportunities were skipped

For bridge transaction
[`0x862f…05bc`](https://gnosis-chiado.blockscout.com/tx/0x862f2562ef2f490a220f488441c97f6fc22690ddad1a00dfa77b3efba8af05bc),
ingress accepted the request at 09:36:14.681811. The relevant L1 parent arrivals
at the embedded execution client were:

| Parent L1 block | Parent slot time | Local arrival | Next target slot |
| --- | --- | --- | --- |
| 23,066,609 | 09:36:15 | 09:36:18.727094 | 09:36:20 |
| 23,066,610 | 09:36:20 | 09:36:29.354229 | 09:36:25 |
| 23,066,611 | 09:36:25 | 09:36:29.365691 | 09:36:30 |

The configured preparation allowance is 1,500 ms, with another 700 ms before
the target slot for submission. Each arrival was too late for that target
under `Sequencer::sync_block_misses_l1_slot`. The guard avoids preparing a
bundle that cannot meet its configured deadline. Canonical notifications reached
the scheduler promptly; its fallback polling interval did not cause these
measured delays.

The next parent arrived at 09:36:30.493799. Composition began at 09:36:32.009875,
dispatch occurred at 09:36:32.373785, and the bridge was included in the block
whose slot timestamp is 09:36:35. Canonical L2 reconciliation completed at
09:36:36.891351. A slot timestamp is not a precise block-publication timestamp.
The signed transaction was repriced before first dispatch; this was not a
failed submitted bundle followed by a retry.

The beacon node had only two connected peers. Throughout 09:36:00–09:36:36,
its direct `beacon_block` gossip mesh contained zero peers. Logs show delayed
blocks retrieved through peer RPC. Discovery repeatedly reported no known
closest peers, including immediately after its original startup.

The concrete configuration defect is a filename mismatch. The installed
[Lighthouse v8.1.3 network loader](https://github.com/sigp/lighthouse/blob/v8.1.3/common/eth2_network_config/src/lib.rs#L35)
reads `bootstrap_nodes.yaml` as an optional file. Our mounted Chiado directory
contained `bootnodes.yaml` and the older `boot_enr.yaml` alias, but lacked
`bootstrap_nodes.yaml`. Thus no discovery seeds were loaded. The configured
direct TCP peers allowed synchronization to continue, masking the defect.

## Correction and its measured limits

`configs/chiado/bootstrap_nodes.yaml` now links to the existing canonical
`bootnodes.yaml`; no ENRs or chain parameters were changed. The Chiado launcher
fails early if the Lighthouse seed file is absent or empty. The static profile
check requires the shipped file to match the canonical list. Both Compose
profiles document the required filename.

Only `eez-chiado-validation-lighthouse` was restarted, at 10:09:19. The same
Lighthouse image loaded all four seeds at 10:09:23. The composer PID and start
time remained unchanged, and confirmed L2 block 129,475 retained its hash.

The bootnodes answered `WHOAREYOU`, and Lighthouse sent handshake packets, but
subsequent `FINDNODE` requests timed out. The local "Session established" message
did not prove that the remote bootnode accepted the handshake. Nine samples over
two minutes showed three connected peers,
no execution-layer outage, and sync distance zero except one sample at distance
one. The three connected peers came from the configured direct endpoints; this
is not evidence of a healthy, replenished discovery peer pool. The precise cause
of the lookup timeouts was unresolved at that point. Challenge replies establish
that some bidirectional UDP traffic works; they do not establish that all discovery
traffic is delivered. No router or firewall change was made.

| Canonical payload-arrival sample | Blocks | Median | 95th percentile | Maximum | Later than 2.8 s |
| --- | --- | --- | --- | --- | --- |
| 09:30:00–09:40:00, before restart | 106 | 424 ms | 6,822 ms | 9,358 ms | 20 |
| 10:09:40–10:12:15, after restart | 29 | 204 ms | 1,306 ms | 4,299 ms | 1 |

These are different live windows, not a controlled performance experiment.
Reconnecting the direct peers also changed the gossip mesh. The improvement
cannot be attributed solely to the filename correction, and late opportunities
remain possible. A follow-up should trace the timed-out discovery exchanges,
establish a sustainable set of compatible gossip peers, and observe a longer
arrival-latency window. Lowering the scheduler allowance would not repair peer
discovery. The subsequent investigation below identifies another local cause.

## Follow-up: official ENRs, client versions, and public advertisement

The four ENRs supplied directly by the Gnosis team exactly match the checked-in
`bootnodes.yaml`. Both that file and `config.yaml` were byte-for-byte identical
to the current [Gnosis Chiado configuration](https://github.com/gnosischain/configs/tree/main/chiado)
when checked on 2026-09-15. These are discovery-only UDP bootnodes, distinct from
the direct TCP endpoints in Compose. Adding direct endpoints does not disable
automatic discovery. Upstream also uses `--testnet-dir`, host networking, and
direct endpoints; the fork did not introduce a discovery-disable flag.

The custom network directory is still necessary with the pinned v8.1.3 binary.
Its [built-in Chiado profile](https://github.com/sigp/lighthouse/blob/v8.1.3/common/eth2_network_config/built_in_network_configs/chiado/config.yaml)
sets `FULU_FORK_EPOCH` to `18446744073709551615`, whereas the active Gnosis
configuration sets it to `1353216`. Simply replacing the directory with
`--network chiado` would therefore select the wrong fork schedule. Lighthouse
loads `config.yaml`, `deposit_contract_block.txt`, `genesis.ssz`, and the optional
`bootstrap_nodes.yaml`. The execution-client files in the imported snapshot do
not influence this beacon.

Fresh isolated probes kept the same custom chain configuration and never
connected to the live Engine API. Each was observed for two minutes after its
Beacon API became ready, then removed. These test startup and discovery, not
execution validation or settlement latency:

| Probe | Direct TCP endpoints | Result |
| --- | --- | --- |
| v8.1.2, semi-supernode, automatic ENR | Existing list | 3 manual peers; no additional discovery peers |
| v8.1.3, semi-supernode, automatic ENR | Existing list | 3 manual peers; no additional discovery peers |
| v8.1.3, ordinary node, automatic ENR | Existing list | 3 manual peers; no additional discovery peers |
| v8.1.3, semi-supernode, automatic ENR | None | No node-list responses; 0 connected peers |
| v8.1.3, semi-supernode, explicit public IP and UDP port | None | Node-list responses and 3 newly discovered connected peers |

Both client versions use discv5 0.10.2. The v8.1.2 downgrade did not improve
discovery. v8.1.3 remains deployed and includes the
[mandatory security patch](https://github.com/sigp/lighthouse/releases/tag/v8.1.3).

The live ENR had no `ip` or `udp` fields. Packet decoding confirmed four
`WHOAREYOU` replies followed by outgoing handshakes containing that incomplete
record and no subsequent node-list replies. With an explicit public address and
UDP port, the isolated probe received authenticated discovery responses and
found compatible peers. This establishes that the supplied seeds are usable
and that automatic ENR advertisement could not bootstrap on this host. It does
not establish that unsolicited inbound TCP/UDP connections are reachable or
that a particular router setting caused the behavior.

At **10:57:46.925 UTC**, only `eez-chiado-validation-lighthouse` was recreated
with `--enr-address=<verified public IPv4>` and `--enr-udp-port=9205`. Its image,
database, peer identity, fork configuration, semi-supernode mode, and direct
endpoints were preserved. No firewall or router settings changed. The composer
process was unchanged. The repository now exposes optional `LH_ENR_ADDRESS`
and `LH_ENR_UDP_PORT` in both Compose profiles; unset/empty values preserve the
previous argument list. The machine's public address is confined to local
runtime configuration.

The five-minute observation, 10:58:21–11:03:21, showed the live peer count rising
from **3 to 5**. All 31 samples reported execution online, non-optimistic sync,
and `is_syncing=false`. Thirty samples had sync distance zero or one; one had
distance three and recovered by the next sample. Slot distance alone does not
distinguish a delayed local block from missed network slots. Previously finalized
L2 block **130,755** retained its hash and state root. A post after recreation,
[`0xe1bb…5ddb`](https://gnosis-chiado.blockscout.com/tx/0xe1bbfe7226ebc3e51a84608306d2a8a86a8ac0e4d0ccd1e2eea525cc10b95ddb),
was independently checked through the live full-blob API: its one blob, native
KZG/versioned-hash evidence, and all **135 L2 block hashes (132,636–132,770)**
verified. The cross-chain queue was empty. No test transactions were sent.
At the final check, **11:08:17 UTC**, the beacon had **6 connected peers**, sync
distance zero, and execution online. All temporary probe containers were removed.
The subsequent [live bridge measurement](bridge-latency.md) at 11:14 UTC completed
in 25.284 seconds, including historical consolidation and one excluded bundle.
The later [24-bridge wave](bridge-wave-latency.md) excluded a separate warm-up
that cleared prior history. All requests settled correctly; observed latency was
24.865 seconds median and 32.212 seconds p95, with admission ramp-up and a missed
slot contributing to queueing.
The subsequent [admission and scheduling correction](bridge-latency-fix.md) was
deployed at 13:18 UTC. Its repeated wave delivered all 24 bridges correctly,
including independent follower replay, with observed median 6.012 seconds and
p95 9.813 seconds. That improvement does not establish a usual sub-five-second
target; the linked report separates slot waiting, one exclusion and canonical
delivery, and records the completed regression validation.

At 14:06 UTC, the live general consolidation threshold was reduced from 1,800
to 300 L2 blocks while retaining pure-L2 `always`. The
[five-minute consolidation check](five-minute-consolidation.md) confirmed a
trigger at exactly 300 unsettled blocks and a single successful two-blob post
covering 315 empty blocks after two skipped Chiado slots.

The later [staged workload measurement](protocol-staged-wave.md) sent 30 source
transactions from all 28 protocol scenarios with semantic verification skipped,
as requested. Canonical placement and settlement checks passed. The sources
settled in groups of 3, 3, 10, 10 and 4 across five consecutive L1 blocks; setup
settlement waiting and preparation estimates accounted for the main delays.

The subsequent [packing correction](packing-improvement.md), deployed at
16:11 UTC, reduced the repeated wave to two successful posts (17 and 13
requests), with median acceptance-to-safe latency 4.180 seconds at the original
500 ms preparation setting. A separate 1,000 ms experiment exposed a remaining
conservative gas projection and three genuinely skipped Chiado slots. The audit
records both runs, all canonical checks and the restored 500 ms configuration;
it does not claim all 28 mixed scenarios fit one Sync block.

The [larger staged workload](large-staged-wave.md) then settled 870 sources from
770 jobs across 47 posts with the same executable and settings. Preparation-time
cuts were more frequent than gas-projection cuts, changing the next measurement
priority to release-build preparation performance. All sources passed canonical
placement checks; semantic verification was stopped at the user's request after
541 jobs passed, with none failed at cancellation. The report distinguishes
queue latency, capacity use, setup work, skipped slots and test-runner overhead.

A separate bootnode response issue remains. All four supplied bootnodes returned
some records outside the requested discovery distance buckets. For example,
57.129.70.222 returned a node at distance 256 for a request for distances
`[254, 255, 253]`. The distances were independently recomputed from the signed
ENR public keys. Lighthouse's discv5 implementation correctly filters these
records and temporarily bans the responder. That validation was not weakened.
This evidence is suitable for Gnosis to investigate; the small live block gossip
mesh and remaining invalid responses prevent claiming full networking recovery
or guaranteed settlement latency.

Reproducible probe scripts, public packet-header decoding, exact client image
identities, invalid-distance examples, deployment records, and live validation
are retained in:

- `artifacts/lighthouse-version-comparison-20260915/`
- `artifacts/chiado-upstream-peer-investigation-20260915/`
- `artifacts/chiado-builtin-profile-20260915/`
- `artifacts/chiado-enr-address-comparison-20260915/`

Validation after the final configuration change: seven Chiado configuration
tests passed; the full Python suite ran 223 tests with 219 passing and four
existing opt-in skips. Both profiles' default rendered argument lists exactly
matched `HEAD`. Explicit IPv4/DNS settings reached Lighthouse, and an invalid
UDP port was rejected by the client at startup. The static deployment check
and `git diff --check` passed.

## Why the explorer shows intermediate posts

The live policy is enabled, pure L2 mode is `always`, general consolidation is
1,800 L2 blocks, L2 block time is one second, and the fullness trigger is off.
Chiado permits two blobs per transaction and two per block in the active
profile. Each blob provides 126,976 usable encoded bytes.

| Canonical posting period | Posts | Explanation |
| --- | --- | --- |
| 09:08:25–09:09:50 | 6 | Empty-network general consolidation |
| 09:33:10–09:33:55 | 5 | Historical prefix required to settle newly arrived pure L2 activity |
| 09:34:25–09:36:35 | 11 | Cross-chain activity, including the user's bridge |
| 10:06:50–10:07:45 | 6 | Next empty-network general consolidation |

At 09:08:22 the general rule became due at exactly 1,800 unsettled blocks, with
zero pending pure transactions. The estimated stream was **1,378,540 bytes**;
two blobs hold **253,952 bytes**. Therefore the consolidation required six
posts. The first five each covered 330 L2 blocks; the last covered 235. The
full chunks used about 113,000 execution gas and split for the blob limit,
rather than a transaction gas limit.

The fixed required endpoint remained L2 block 125,915 while the backlog drained.
The final candidate also included 85 newer empty blocks, ending at 126,000.
This matches the proposal's allowance to include newer blocks without extending
the fixed drain obligation. Posting stopped when the obligation was covered.
A 30-minute trigger therefore produces multiple L1 transactions with this
encoding and network capacity.

The later period was not empty: **60 ordinary L2 transactions** appeared in
blocks 127,396–127,398, including 34 contract deployments and 22 cross-chain
proxy creations. Another **30 cross-chain requests** arrived at
09:33:34.679773–09:33:35.135, followed by the user's bridge at 09:36:14.
Gateway logs show `foundry/1.7.1` and curl clients using the composer endpoints
immediately before the burst. The sender accounts match earlier test activity.
This strongly indicates a protocol-test workload; the initiating person or
process has not been established. This investigation generated no transactions.

The pure activity correctly triggered posting before the next general deadline.
Its earlier empty history had to be published first, so some of those posts
contain only empty blocks. Counting receipt observations or submission attempts
as separate successful posts would also overcount: this audit counts only
ordered canonical `BatchPosted` logs and verified receipts.

## Correctness and validation

- All 28 canonical post receipts succeeded. Their KZG evidence, versioned blob
  hashes, decoded batch semantics, and every covered L2 block hash were checked.
- The range 124,116–129,475 is contiguous, with no repeated covered blocks and
  no duplicate hashes among 107 encoded transactions. The registry commitment
  matches the last covered block hash at the pinned L1 audit endpoint.
- An independent follower, using an isolated copy of its earlier verified
  database, reproduced block 129,475 and state root
  `0xbebcdc84c2f97e022e6c36aac9955adf86765fa741e2b565adda390d5354e2ed`.
- All 31 investigated cross-chain requests have successful canonical source
  receipts: 18 inbound on L1 and 13 outbound on L2. The queue inspection showed
  zero queued, in-flight, ready, blocked, or unknown requests. Expired per-hash
  queue telemetry was not treated as transaction failure or success.
- `bash scripts/check-chiado-blob-config.sh`: PASS.
- `python3 -m unittest scripts.ci.tests.test_chiado_config -v`: seven tests PASS,
  including missing, empty, and mismatched discovery seeds and optional ENR
  argument rendering in both deployment profiles.
- `python3 -m unittest discover -s scripts/ci/tests -v`: 223 tests run, 219 PASS,
  four existing opt-in Docker/Kurtosis/relocation checks skipped.
- `git diff --check`: PASS. No Rust execution or settlement-policy code changed.

The blob-verification JSON explicitly leaves `complete_replay_checked` false
because that script authenticates published data. Full replay is established
separately by `independent-follower-after-repair.json`, which records a passing
result and the verified endpoint. This audit is scoped to the captured canonical
range and does not promise that future networking delays cannot occur.
