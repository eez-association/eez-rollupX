# Failed canary: builder, relay and proposer evidence

Investigated 2026-09-22; all times below are UTC. This is a read-only analysis of
the existing failed canary, not another traffic wave or deployment.
Runtime/source identities and the original failure are recorded in the
[parent audit](README.md#follow-up-implementation-diagnosed-outcomes-and-prompt-enrichment).

## Findings

The six produced-but-excluded targets separate into two observed paths:

- **One target had no usable relay slot data at the builder.** Target 23178664,
  slot 24938911, proposer 10911: three payload-attribute attempts were discarded
  with `no MEV-Boost relay data`. No bid was observed. This is not evidence of a
  gas-limit failure. Missing registration, duty/cache freshness and relay fetch
  failure remain possibilities; the logs do not distinguish them.
- **Five targets had relay-validated bids, but no recorded proposer retrieval.**
  The expected proposer pubkeys match the actual Beacon proposers. Builder slot
  registrations also match the actual execution fee recipients. No `getHeader`,
  bid delivery or payload delivery was found for those target slots in the retained
  relay interval. Registration is not proof that a validator queried this relay.
- **Two other attempts targeted skipped timestamps.** Adjacent canonical headers
  prove both gaps. Beacon block lookup returns 404, and historical proposer duties
  identify the scheduled validators; neither row represents a produced L1 block.

All eight local final simulations report completion of the intended effects.
All eight bundle calls return a non-error JSON-RPC response with `result: null`;
there is no returned bundle identifier or proof of candidate acceptance in that
reply. Do not equate that response with inclusion in a builder candidate.

## Per-attempt evidence

The bid count refers to **all validated bids for that slot**, not necessarily bids
containing our exact transaction. Candidate membership is corroborated below but
cannot be proven without the undelivered payload bodies. No failed target has a
recorded header request or payload delivery in this relay's retained logs.

| Target L1 | Target time | Beacon slot | Proposer index | Slot bids | Last verified stage / result |
| --- | --- | --- | --- | ---: | --- |
| 23178663 | 15:27:30 | [24938910](https://beacon.chiadochain.net/slot/24938910) | 8606 | 2 | Bid validated; produced block omitted transaction |
| 23178664 | 15:27:35 | [24938911](https://beacon.chiadochain.net/slot/24938911) | 10911 | 0 | Builder discarded attributes: no relay slot data |
| 23178665 | 15:27:40 | [24938912](https://beacon.chiadochain.net/slot/24938912) | 10324 | 1 | Bid validated; produced block omitted transaction |
| 23178666 | 15:27:45 | [24938913](https://beacon.chiadochain.net/slot/24938913) | 9178 | 1 | Bid validated; target timestamp skipped |
| 23178667 | 15:27:55 | [24938915](https://beacon.chiadochain.net/slot/24938915) | 8996 | 4 | Bid validated; produced block omitted transaction |
| 23178668 | 15:28:00 | [24938916](https://beacon.chiadochain.net/slot/24938916) | 9211 | 1 | Bid validated; target timestamp skipped |
| 23178669 | 15:28:10 | [24938918](https://beacon.chiadochain.net/slot/24938918) | 10283 | 2 | Bid validated; produced block omitted transaction |
| 23178670 | 15:28:15 | [24938919](https://beacon.chiadochain.net/slot/24938919) | 10573 | 1 | Bid validated; produced block omitted transaction |
| 23177867 | 13:30:25 | [24937505](https://beacon.chiadochain.net/slot/24937505) | 8895 | 1 | Canonical inclusion; header and payload delivered |

The final row is the successful control from the earlier staged run, not a success
of the new binary. Its canonical L1 block and exact transaction hash were rechecked.
Full transaction hashes, proposer pubkeys and stage evidence are in
[excluded-canary.csv](excluded-canary.csv).

### Candidate membership and timing limits

For targets 23178665, 23178666, 23178668, 23178669 and 23178670, at least one
candidate's gas equals the exact local ordered-bundle gas plus the 21000-gas
builder payment, with the expected transaction count. For 23178663, the next
candidate increases gas by exactly the local bundle gas and adds two transactions.
For 23178667, the candidate gas increases by exactly 1479689, matching the local
bundle, and adds one transaction. These are strong correlations, not
cryptographic transaction-membership proof. Other bids in those slots contain
additional work; the first slot bid is not always our candidate.

The earliest candidates with that corroborating gas/count evidence were eligible
before their target timestamps:

| Target L1 | Lead time before target |
| --- | ---: |
| 23178663 | 0.883 s |
| 23178665 | 2.925 s |
| 23178666 | 2.992 s |
| 23178667 | 2.965 s |
| 23178668 | 2.661 s |
| 23178669 | 2.366 s |
| 23178670 | 2.434 s |

No documented proposer-specific deadline was available. These numbers do not
prove every proposer had enough time. They do establish that the matched bids
were not first accepted after the target timestamp. Raising Composer gas limits
or tips is not supported by this evidence as the next fix.

### Successful control

Target 23177867 / slot 24937505 / proposer 8895:

- Eligible bid: 3.358 seconds before the target.
- `getHeader` / bid delivered: 434 ms into the slot.
- Payload retrieval started: approximately 452 ms into the slot.
- Delivered payload hash equals the canonical block hash; our exact transaction
  is in that block.

The relay logged Nimbus as the requesting client for this control. It also logged
four `getHeader` requests with `no bid found` for the slots immediately after the
failed canary, 24938920–24938923. Thus the captured interval does include header
logging, but none for our eight failed targets. This does not rule out a request
lost before reaching the relay or omitted by its logging.

The two failed Nethermind-labelled blocks share a fee recipient with the successful
control. Three failed Erigon-labelled blocks share another recipient; the
missing-relay-data case has a third. Client strings and fee recipients do not prove
validator ownership, local block production or use of another relay.

## Retry-policy review

The behavior matches the current contract, rather than a regression introduced by
monitoring:

1. `BundleDropReason::Excluded` includes auction loss, builder discard and relay
   simulation failures, and is the only drop reason that charges the attempt limit
   (`crates/eez-l1/src/submitter.rs`).
2. Canonical reconciliation charges eligible sources once per episode
   (`Composer::reconcile_attempt`), after checking receipts, nonce and ownership.
3. `OptimisticallyIncluded::resolve` changes an unspent source to
   `AttemptsExhausted` at three charged episodes and handles its nonce-dependent
   suffix. It retains history; the source is no longer queued for retry.
4. Skipped slots do not charge. Duplicate resolution does not charge twice.
   Explicit accepted resubmission is a new admission, not an automatic recovery.

This is explicitly described by the [RPC failure contract](../../../crates/eez-control-rpc/SPEC.md#6-failure-handling)
and [queue-liveness proposal](../../proposals/cross-chain-queue-liveness.md).
The canary's source nonces 69 (inbound) and 53 (outbound) each accumulated three
charged exclusions; the outbound also had two uncharged skipped slots. This
explains why the queue became empty without either source receipt.

**Assessment:** canonical omission establishes non-inclusion, not user invalidity.
The current shared failure budget consequently stops retrying potentially valid
requests during proposer/relay delivery problems. A larger limit merely postpones
this behavior. Exempting every omission without a bounded backoff/fairness design
could let an invalid or unsendable bundle loop indefinitely.

Recommended follow-up design, not applied here: distinguish delivery retries from
validated source/proof rejection, keep delivery failures owned with bounded
backoff and explicit stalled status, and preserve protection against repeated
invalid compositions. Any change needs a contract update and tests for fairness,
nonce suffixes, partial consumption, reorgs and resubmission. The smallest useful
visibility improvement first is to emit the terminal source disposition, so an
empty queue does not conceal `AttemptsExhausted`.

## Concrete operator handoff

For the builder/relay operator:

- Slot **24938911**, validator **10911**: inspect proposer-duty data, validator
  registration, registration-cache freshness and relay fetch errors at
  **15:27:30–15:27:35 UTC**. The builder discarded all three received attribute
  updates for the slot. Establish which prerequisite was missing.
- Slots **24938910, 24938912, 24938915, 24938918, 24938919**: inspect ingress/access
  logs for the exact proposer pubkeys in the CSV. Did any header request reach the
  gateway? Was it forwarded, timed out, rejected, or routed elsewhere?
- For the corresponding validator operators, inspect configured relay URLs,
  Beacon/MEV-Boost logs and the local-versus-builder selection decision at those
  exact slots. Only these records can separate configuration, connectivity,
  fallback and another relay/production path.
- Supply candidate transaction hashes keyed by bid block hash, or retained
  candidate payloads. This closes the current membership gap; aggregate gas and
  transaction counts are insufficient as a permanent tracing contract.
- Supply the correct read-only relay Data API base URL or fix its public routing.
  The public relay website links its Data API, but all tested bidtrace/registration
  endpoints returned HTTP 404, including the known successful control. A 404 is
  missing API access, not an empty query result or absence of historical bids.

No message was sent to another operator. This handoff is ready to share.

## Sources, validation and reproducibility

- [Operator-supplied Chiado log viewer](https://logs.chiado.gcd.ovh/): bounded
  15:27:00–15:28:40 UTC collection, 380 builder groups / 4481 messages, 266 relay
  records and 24 execution-client groups / 82 messages. Requests above the
  viewer's 500-row limit were split; boundary duplicates were removed.
- Local canonical execution RPC: consecutive parent/hash checks, transaction
  absence for produced targets, and independent successful-control recheck.
- Local Beacon API: genesis/slot configuration (5 seconds, 16 slots per epoch),
  canonical block/proposer lookup, immutable validator pubkeys and historical
  duties for skipped slots. Relay proposer pubkeys and produced-block parents
  were checked against Beacon execution payloads.
- [Public relay website](https://relay.chiado.gcd.ovh/) and
  [standard Data API specification](https://flashbots.github.io/relay-specs/).
  API requests were read-only, bounded and explicitly recorded as 404 failures.
- Upstream rbuilder reference at
  [6037fa728b13bf1806e16fff2586414216f6b8fa](https://github.com/flashbots/rbuilder/blob/6037fa728b13bf1806e16fff2586414216f6b8fa/crates/rbuilder/src/live_builder/payload_events/mod.rs):
  that diagnostic follows failed slot-data resolution. The matching
  `relay_epoch_cache.rs` handles cached registrations. This reference explains the
  code path; it does not establish the remote builder's deployed source revision.
- Existing Composer ownership/retry tests: **27 passed** with
  `cargo +1.94.0 test --locked -p eez-composer --lib optimistic::tests::` and the
  shared target directory. No implementation or test logic changed.

Private/local-only raw evidence, scripts, JSON/CSV correlation, canonical and Beacon
responses, source reference and checksums are retained at:

```text
.runtime/chiado-observability-20260918/validation/posting-diagnostics-20260922T151311Z/exclusion-investigation
```

The scripts reuse the earlier bounded log collector and validate their joins.
No new service, monitoring dependency, traffic, fee setting, retry policy or
runtime deployment was introduced by this investigation. Availability of proposer
logs, full candidate payloads and the proper relay API endpoint remains an external
evidence gap. The report deliberately does not label an unobserved proposer
choice as a proven root cause.
