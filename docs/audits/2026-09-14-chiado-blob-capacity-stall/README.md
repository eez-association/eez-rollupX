# Chiado settlement capacity stall — 2026-09-14

**Investigation status at 18:31 UTC: root cause reproduced; recovery not yet performed.**
The subsequent [prevention change](../2026-09-14-sequencing-da-budget/README.md)
and [authorized recovery](../2026-09-14-chiado-capacity-recovery/README.md) track
the current implementation and operational status. The observations below are
the preserved incident findings.

The last validated settlement was L2 block 72425. At 18:31:17 UTC the
cross-chain pool held 860 sources before its next unsuccessful selection.
The investigation used read-only runtime operations and an offline codec
reproduction. It did not restart the node, change limits, rewind history, or
submit transactions.

## Cause and measured evidence

The active beacon configuration and the canonical
[Chiado configuration](https://github.com/gnosischain/configs/blob/main/chiado/config.yaml)
both allow two blobs per L1 block. The composer resolves a two-blob transaction
allowance, no outgoing operator cap, and zero required companion blobs. The
existing codec carries 126976 stream bytes per blob: 253952 bytes in this batch.
At 18:28 UTC the poster still held 49.755881628935944136 test xDAI. This failure
is independent of the earlier funding exhaustion.

L2 blocks 72426–72430 contain 570 ordinary EIP-1559 transactions from 260
senders, including 320 direct creations and ten calls to the deterministic
deployment factory. None is signed by the system account. Together they use
138597214 execution gas and occupy 383195 bytes of raw block RLP. The current
payload format adds framing and its existing RLP representation of byte
vectors; even the raw block data already exceeds two blobs.

The exact checked-in payload and native blob codecs reproduce these prefixes:

| Pending prefix from 72426 | Transactions | Native stream bytes | Physical blobs | Valid historical terminal |
| --- | ---: | ---: | ---: | --- |
| Through 72426 | 170 | 78038 | 1 | No: nonempty and off-grid |
| Through 72427 | 324 | 158396 | 2 | No: nonempty and off-grid |
| Through 72428 | 405 | 301307 | 3 | No: nonempty and off-grid |
| Through 72430 | 570 | 490292 | 4 | No: nonempty |
| Through 72435 | 570 | 494108 | 4 | Yes: first empty on-grid block |
| Through 72440 | 570 | 497932 | 4 | Yes |

Each captured individual block fits two blobs when encoded alone. Their combined
range does not fit before the first permitted terminal. More recent full-range
errors report five or more blobs because additional blocks and, when applicable,
terminal metadata also need encoding. The smallest valid prefix already needs
four blobs, so trying shorter permitted prefixes cannot make progress.

## Trigger and timeline

- At 18:16:52 the prior batch reconciled through L2 block 72420.
- The next bundle, targeting a Sync block at 72425, was dropped with
  `slot_skipped`. Its four cross-chain sources were retained for retry.
- At 18:17:07 the sequencer logged `Catchup { live: 13 }`, starting from
  height 72426. The catch-up path produces ordinary Live blocks until its
  terminal Sync. It can therefore fill intervening scheduler-grid heights
  with ordinary transactions; observed block 72430 is nonempty.
- Exact-hash ingress records account for 250 of the 570 ordinary transactions,
  first observed between 18:16:58.787678 and 18:17:05.505243 UTC. The remaining
  transactions have no matching ingress record in the captured node log.
  The traffic resembles a test/deployment burst, but its originating process
  or operator has not been established. No local load-generator process was
  identified from the inspected containers and sanitized process metadata.
- The historical planner successfully isolated the empty prefix 72421–72425.
  It settled in L1 block 23056675, transaction
  `0xfdcf7e348e1b41bcf2c807503d475ddb743e89ae2f6c6c758c674cc3e7146ae9`.
  Derivation advanced at 18:17:41.497368 UTC. Both the endpoint hash
  `0x1373edd60923022182e86cefcae9e4dabece6f1aec13e63389410d0528a315da`
  and its finalized status were independently queried through the correlation RPC.
- At 18:17:42 the remaining range first reported `unpostable_range`. By
  18:31:17 the planner rejected all 171 available empty on-grid endpoints.
  Selecting one queued cross-chain source is only a preparation attempt;
  these failed attempts do not mean one source is settling per block.

## Architecture and existing coverage

`crates/eez-composer/src/composer.rs::empty_chunk_boundaries` requires an empty,
scheduler-aligned historical terminal. `emit_historical_chunk` tests those
prefixes against the actual allowance. `crates/eez-driver/src/sequencer.rs`
produces ordinary blocks without an aggregate DA budget across that interval.
Automatic admission in `composer/admission.rs` applies to held cross-chain
sources and their composed terminal; it does not limit ordinary mempool blocks.

This is the same class of limitation recorded in the earlier
[settlement-targeting audit](../2026-09-14-settlement-targeting/README.md), where
individually fitting data calls blocked settlement after block 39500. That
incident was manually recovered; general prevention remained outstanding.
Proposal sections 6.2 and 6.5 explicitly retain the historical boundary rule
and report an unpostable shortest range. Section 6.5 excludes silently adding
a strict sequencing byte-budget project. Consequently, successful policy
acceptance tests do not establish liveness for unrestricted ordinary traffic.

## Reproduction and validation

Local evidence is retained in
`artifacts/bridge-incident-20260914/blob-limit-investigation/`:

- Exact RPC block objects and raw RLP for blocks 72421–72440, range-size
  summary, beacon limits, canonical/finalized anchor, funding and queue samples.
- Node events spanning the failure and exact-hash ingress matches.
- `codec-repro/`: an isolated diagnostic executable depending on the checked-in
  codec crates, using dependencies seeded from the repository lockfile.
- `codec-reproduction.log` and `codec-reproduction-identity.json`: successful
  reconstruction, input/source/executable hashes, and exit status.
- `existing-boundary-regression.log` and its result JSON: the existing
  `individually_fitting_blocks_can_exceed_capacity_before_the_first_sync_boundary`
  test passed again using the previously built composer unit-test executable
  (one passed, 136 filtered out).

The diagnostic checks contiguous canonical block decoding, exact payload
round trips, agreement between native size measurement and actual encoding,
physical blob packing/decoding, and that each individual block fits while the
first legal prefix requires four blobs. Its first draft incorrectly asserted
five blobs for that shortest prefix, based on the full-range log; actual codec
output corrected that expectation to four and 494108 bytes. Production code
was not changed to satisfy the reproduction.

Run the diagnostic from the repository root:

```bash
CARGO_TARGET_DIR="$PWD/target" CARGO_BUILD_JOBS=2 cargo +1.94.0 run \
  --locked --offline \
  --manifest-path artifacts/bridge-incident-20260914/blob-limit-investigation/codec-repro/Cargo.toml \
  -- artifacts/bridge-incident-20260914/blob-limit-investigation
```

## Required repair and recovery considerations

The recommended prevention is to budget ordinary sequencing over the complete
range between permitted settlement boundaries using the active network's hard
DA allowance, with space reserved for framing and a valid terminal. It must
also govern catch-up and reorg replay, preserve sender nonce dependencies,
defer excess valid transactions, and explicitly handle a transaction that
cannot fit by itself. Preserving empty scheduler-grid boundaries during
catch-up helps retain split opportunities, but does not alone solve the byte
overflow: this incident crosses the allowance before reaching the first grid
height even if that terminal were empty.

This requires an explicit extension of the proposal's sequencing scope and
tests for ordinary traffic across several blocks, skipped slots, recovery,
both network profiles, and eventual cross-chain progress. A separately
validated protocol change allowing additional settlement boundaries is another,
larger design; it must not be assumed compatible with current attestations.

For the currently accepted history, funding, fee margins, a lower soft posting
threshold, or increasing a local blob setting cannot fix the immutable two-blob
network bound. A controlled recovery would need to preserve the original
database and all accepted signed work, then rebuild only the unsettled suffix
after 72425 into postable intervals. That changes provisional block hashes and
requires nonce/conflict accounting and independent receipt/state verification.
The held cross-chain pool is volatile; preserving only block bodies is not a
complete backup of accepted work. A live cutover has not been performed here.

This live failure supersedes any broad production-readiness inference from the
earlier successful workloads. Prevention and a validated recovery are required
before claiming readiness for unrestricted traffic on Chiado.
