# Blob efficiency: structural encoding and the useful parts of Optimism

The authoritative implementation plan is now [Derivable ChainOperation](../../proposals/derivable-chain-operations.md), with a [coverage ledger](../../proposals/derivable-chain-operations-coverage.md). It specifies the new 65,536-block signer default and incremental validation; references to 512 below describe the inspected pre-change code.

**Updated objective and primary recommendation:** the constraint is how much
history fits before blob capacity forces settlement. See
[Derive blocks instead of publishing full blocks](derivation-first.md) for the
field-by-field analysis and proposed ordinary-history derivation format. It
supersedes the implementation ordering below. This document retains the measured
encoding baselines and utilization context; current utilization is not the main
optimization target. See also the [implementation plan, gaps and capacity estimates](plan-and-capacity.md).

Research date: 2026-09-16. Repository inspected at
`ddf40bc2eb5fac8f7973b112eb383256cd8aac74`.
This is an investigation and offline sizing experiment, not a protocol upgrade.
No node configuration, contract, or runtime code was changed. The measurements
below supersede the earlier exploratory numbers that also changed entry wrappers;
those changes have been removed to enforce the requested boundary.

**Scope:** only the contents of `ChainOperation.operations` may change. No
cross-chain message, cross-chain transaction data, derivation-entry encoding,
outbound group encoding, blob packing, carrier, settlement timing, or contract
change is proposed. No general-purpose compression is used.

**Recommendation:** introduce a new inner payload version that represents block
RLP as byte strings, then evaluate a lossless multi-block header format inside the
same `ChainOperation`. Preserve all transactions, including system transactions,
exactly. Preserve existing cross-chain derivation entries and group sizes with
both their values and encodings unchanged. These borrow the structural idea of
OP span batches without changing EEZ's cross-chain protocol.

## 1. What the evidence actually shows

There are three different costs:

1. Redundant bytes in the logical payload.
2. Unused capacity in the blobs actually purchased.
3. Unused capacity under a network/operator allowance, which is not itself paid for.

The current packing carries **126,976 meaningful bytes** in a 131,072-byte blob.
`pack()` allocates `ceil(stream_bytes / 126976)` blobs. It already packs messages
across blob boundaries and allocates only the blobs needed; it does not allocate
every blob allowed by the network. Field-element overhead is 4,096 bytes per blob,
or 3.125% of physical bytes. Partial last blobs are a separate issue.

### Captured public Chiado workload

Recomputed from the September 16 result-reserves run's canonical audit:

| Measurement | Result |
|---|---:|
| Source-bearing posts / purchased blobs | 38 / 38 |
| Meaningful stream bytes | 2,548,547 |
| Purchased usable capacity | 4,825,088 bytes |
| Weighted purchased-capacity utilization | **52.82%** |
| Unused usable capacity | **47.18%** |
| Smallest / median / largest stream | 5,428 / 69,906 / 104,327 bytes |
| All captured paid posts / blobs, including setup/history | 44 / 48 |
| Posts with a matching byte measurement | 41 / 44 |
| Utilization for that measured subset, 42 blobs | 52.51% |

Do not compute whole-run utilization using 48 blobs and only the 41 known sizes:
three two-blob posts have no matching byte measurement. Their hashes are recorded
in [measurements.json](measurements.json).

The existing audit's `aggregateBlobUtilizationPercent` measures usage against the
**two-blob allowance**. For example, 104,327 bytes is 41.08% of that allowance,
but **82.16% of the single blob actually purchased**. Both metrics are useful;
they answer different questions.

Every source-bearing post was already a one-blob post. Making those exact posts
smaller cannot reduce their blob count below one. Even perfect elimination of all
redundancy leaves this floor unless posts can be grouped or use another carrier.
Dividing total source bytes by blob capacity gives an idealized 21-blob lower
bound, versus 38 observed. That calculation ignores boundaries, timing, execution
gas, new framing, and dependencies; it is **not an achievable saving estimate**.

The audit records 18 transaction-gas cuts and 11 preparation-time cuts. These
are attempt events, not unique posts or independently attributable failures.
They demonstrate that DA capacity is not the only admission constraint.

Blob charges were 0.006291456 xDAI out of 0.221751990942390605 xDAI in total
captured receipt costs: **2.84%**. This is one historical testnet workload and fee
regime. Blob-space efficiency matters, but a percentage reduction in blob bytes
must not be presented as that percentage reduction in total settlement cost.

Evidence: [result-reserves audit](../../audits/2026-09-16-chiado-result-reserves/README.md)
and the hashed input files listed in `measurements.json`. No new live workload
was submitted for this research.

### Offline structural experiments

The saved protocol devnet corpus contains 28 scenario sidecar files, representing
**24 distinct blob byte sequences**, covering 144 blocks. Duplicate samples are
deduplicated by blob SHA-256. This is a separate corpus from the public Chiado run.

| Encoding experiment | Total meaningful stream bytes | Reduction | Blobs at unchanged post boundaries |
|---|---:|---:|---:|
| Existing format | 207,609 | — | 24 |
| Byte strings for block RLP only | 178,758 | **13.90%** | 24 |
| Above, plus shared unchanged header fields and structured block bodies | 126,153 | **39.24%** | 24 |

The reductions are cumulative relative to the baseline, not additive. All variants
include the native outer framing, unchanged semantic stream, and unchanged
derivation-entry/group encodings. No generic
compression was applied. Current samples range from 4,842 to 15,035 stream bytes;
they are sparse, small test batches. **Neither percentage is a measured Chiado
reduction or a production throughput estimate.**

The script checks canonical RLP, matches block counts and raw block sizes against
the saved verification reports, and reconstructs every original block byte-for-byte
from the experimental span representation. It also asserts byte equality of the
entry/group encodings and copies the outer semantic suffix unchanged when
constructing each hypothetical stream. It consumes previously verified
evidence; it does not independently rerun KZG authentication or EVM execution.

## 2. Where EEZ spends its bytes

Current production path:

```text
canonical full L2 blocks + ABI derivation entries + outbound group sizes
  -> tag 0x02 self-contained payload, RLP encoded
  -> ChainOperation(chain_id, operations)
  + standardized cross-chain semantic transaction messages
  + CloseBlobStream
  -> 31-byte field-element packing and last-blob zero padding
  -> KZG sidecars + postAndVerifyBatch
```

Relevant code:

- [payload codec](../../../crates/eez-payload-codec/src/lib.rs):
  `SelfContainedBody`, `encode_self_contained`, `self_contained_payload_len`.
- [composer](../../../crates/eez-composer/src/composer.rs):
  payload construction around line 5852 and `derivation_metadata` around line 6345.
- [blob codec](../../../crates/eez-blob-codec/src/lib.rs):
  `encode_native_stream`, `native_stream_len`, `pack`, native profile validation.
- [signer DA validation](../../../crates/eez-proof-signer/src/settlement/da.rs):
  `verify_self_contained_da_payload` compares full block RLP byte-for-byte.
- [deriver](../../../crates/eez-deriver/src/deriver.rs):
  `execute_block_with_template` consumes advertised header metadata.

### A. Byte arrays currently become lists of integers

`SelfContainedBody.blocks` and `.l2_entries` are `Vec<Vec<u8>>`; only
**blocks** are changed by these experiments. In the pinned
`alloy-rlp`, `Vec<T>` encodes as a list, while byte slices / `Bytes` encode as byte
strings. Consequently each block's already-encoded RLP is wrapped as a list of
individual `u8` values. Bytes at least `0x80` require two bytes; integer zero is
represented by `0x80`. This is avoidable serialization expansion, beyond the
necessary outer list lengths.

The sizing code explicitly documents this existing wire behavior, so replacing
the Rust type in place would break historical decoding. Introduce a **new payload
tag** and keep the existing legacy decoders for fixtures. Runtime consumers
retain the sparse `Payload` and its block-plan iterator rather than reconstructing
dense `DecodedBatch` values, including exact original block RLP. Leave the
existing ABI entry and group representations untouched. This experiment
originally treated `0x03` as hypothetical; the final new-network proposal assigns
it to the derivable format.

### B. Every block carries a complete header

The experiment's 144 headers occupy 87,615 raw bytes, before integer-list
expansion: 42.2% of current total stream bytes. This includes repeated or
unchanged fields and 256-byte logs blooms. Empty blocks still have full headers.

The lowest-risk span design can preserve all these fields while encoding repeated
values once. Our experiment stores one full first header, then a bitmap and exact
new values for fields changed from the preceding header. It stores each block's
body as structured RLP, avoiding the integer-list wrapper. It does **not** discard
state roots, receipts roots, hashes, transaction signatures, or variable execution
inputs. Decoding recovers the exact original block RLP without executing it.

The experiment requires the same header field count within a span. A real format
must split spans at layout changes or explicitly encode schema changes, bound
counts and bitmap widths, and reject missing/extra field values. It also needs a
fallback to full headers when deltas would be larger. These production rules are
not implemented by the sizing experiment. Its `0x04` tag is hypothetical too.

A more aggressive derivation format could omit execution outputs and regenerate
them, but that changes the validation architecture. The current deriver reads
timestamp, beneficiary, randomness, gas limit, beacon root, withdrawals and extra
data from block templates. Assuming fixed values merely because a test fixture
uses them can change block hashes or execution. Prefer the exact reconstruction
design first.

### C. Explicitly unchanged cross-chain data

The payload contains full blocks including system transactions, ABI-encoded
cross-chain derivation entries, and outbound group sizes. The outer blob stream
also contains standardized cross-chain semantic messages. Some content may
appear in more than one representation. Removing it is outside this research's
allowed implementation scope.

Both experiments preserve:

- Every original signed transaction and system transaction, byte-for-byte, inside
  the reconstructed full block.
- The existing RLP encoding of `l2_entries`, including the integer-list byte
  representation; this is deliberately not included in the byte-string change.
- The exact `outbound_group_sizes` encoding and ordering.
- Every outer semantic message byte, including origin transaction bytes, calls,
  results, nested/revert markers and transaction boundaries.
- The existing chain ID, standardized stream version, physical blob packing,
  zero-padding rule, and settlement boundaries.

Only the operations field's length and contents, and consequential blob
commitments, change. Cross-chain data is not removed, referenced, or re-encoded.

## 3. What to borrow from Optimism

| OP mechanism | Applicability under the allowed scope |
|---|---|
| Span batches | Adopt the structural principle inside `ChainOperation`: encode related blocks together and share unchanged metadata. |
| Transaction field encoding | Context only; initial experiments preserve all transaction bytes rather than introducing a new transaction representation. |
| Channels, frames and posting-policy changes | Outside scope: affect transport or settlement boundaries beyond `ChainOperation`. |
| 254-bit blob packing | Outside scope: changes the physical carrier shared with cross-chain data. |
| Generic channel compression | Excluded by request. |

OP's Delta format reduces repeated block metadata, packs flags, and uses varints
for selected transaction fields. Signatures remain. Its derivation rules supply
some fields instead of publishing a full Ethereum block. These ideas can inform
an EEZ format, but the exact OP wire format assumes OP-specific derivation.
([OP span-batch specification](https://specs.optimism.io/protocol/delta/span-batches.html))

OP frames separate a logical channel from its transaction carrier; the frame
header is 23 bytes. This enables transport aggregation and splitting. It does not
eliminate the need to obtain the complete data required for derivation. Current
Holocene rules impose stricter sequential channel/frame processing than the
historical general derivation examples.
([Derivation](https://specs.optimism.io/protocol/derivation.html),
[Holocene derivation](https://specs.optimism.io/protocol/holocene/derivation.html))

OP's Ecotone packing provides 130,048 data bytes before its four-byte version and
length header, leaving 130,044. That is about **2.42%** more capacity than EEZ's
126,976. It does not repair half-empty posts. EEZ's own
[future optimizations](../../../eez-core-protocol/docs/blobs/FUTURE_OPTIMIZATIONS.md)
already discuss a different 254-bit arrangement with the same gross capacity.
Neither is supported by the existing EEZ decoder, which rejects a nonzero most
significant byte. ([Ecotone specification](https://specs.optimism.io/protocol/ecotone/derivation.html))

OP operators can trade posting latency for filling and choose blobs or calldata.
Its multi-hour configuration examples should not be transplanted into EEZ's
cross-chain path. ([Batcher configuration](https://docs.optimism.io/chain-operators/guides/configuration/batcher))

## 4. What the scoped change can and cannot improve

For unchanged posts, the correct calculation is:

```text
blobs_before = sum(ceil(original_post_bytes / 126976))
blobs_after  = sum(ceil(new_post_bytes / 126976))
```

Apply it per post, not to a corpus-wide byte sum. Blob fees are charged per blob,
including padding. ([EIP-4844](https://eips.ethereum.org/EIPS/eip-4844))

These changes can reduce a multi-blob post's blob count when it crosses a capacity
boundary. They can also make room for more block data under the existing DA
allowance. This investigation does not propose changing cross-chain group
selection or timing to exploit that space. A one-blob post remains a one-blob
post, however small its `ChainOperation` becomes.

The observed Chiado source posts were all single-blob posts, so there is no
possible blob-count reduction for those exact posts from this scoped work alone.
There may be a benefit for their multi-blob setup/history posts or future traffic;
that requires replaying their actual operations bytes through the proposed codec.

Unfilled capacity caused by the existing settlement boundaries remains. Neither
waiting longer, moving urgent work to calldata, sharing blobs between settlements,
nor altering cross-chain semantics is part of the recommendation.

EEZ already has interval/block-count/fullness policy and gas/time admission
constraints. Keep them unchanged. The distinction between allowance utilization
and purchased-blob utilization should still be made when assessing this work.
See [settlement policy](../../configurable-settlement-policy.md).

## 5. Proposed order of work

| Priority | Concrete work | Completion criterion |
|---|---|---|
| 1 | New inner payload version using byte strings for block RLP only | Exact block reconstruction; entry/group encodings and all semantic bytes unchanged; historical decoding and accurate sizing retained. |
| 2 | Lossless header sharing inside the same `ChainOperation` | Exact block hashes and independent replay across empty/busy blocks, field changes, fork transitions and reorgs; measured saving beyond priority 1. |
| 3 | Optional deterministic ordinary-block metadata derivation | Only after defining all execution inputs and proving exact original blocks are reproduced; no modification of cross-chain data. |

Priorities 1 and 2 can keep the standardized outer blob message stream unchanged:
only the rollup-defined `ChainOperation.operations` encoding changes. Consumers
must still upgrade together. Preserve legacy tags for history and define activation
by canonical protocol context, not a sender-controlled flag alone. These changes
must be designed to require no L1 ABI/storage or cross-chain semantic change.
Confirm the proof-system activation path before rollout; if it requires changes
outside this boundary, that design does not satisfy the present scope.

The implementation surface includes `eez-payload-codec`, composer construction and
incremental sizing, `BlockDaSizer`, interval reservations, shared DA-size
accounting, signer validation, and follower/deriver decoding. These consumers must
understand the new operations format; they must retain cross-chain logic and data.
Smaller final bytes will not improve admission if all estimators continue charging the old representation.
For spans, reserve conservative costs for future changed fields and use the exact
codec for completed candidates; an average saving ratio is not a safe bound.

Before shipping, collect raw sidecars for a representative public workload,
including long idle history, ordinary bursts, inbound/outbound calls, callbacks,
reverts and high-entropy transaction data. The current public audit supplies size
and receipt totals; the sidecar corpus used here supplies small devnet samples.
It does not justify extrapolating 39.24% to the live chain.

Required metrics: original/new operations bytes, semantic bytes, reconstructed
bytes, purchased blobs, purchased-capacity utilization, allowance utilization,
last-blob slack, closure reason, encode/decode time, KZG/proof/preparation time,
L1 execution gas, actual blob charges, source count and safe-head/cross-chain
latency. Associate each measurement with the exact included post hash so retries
do not inflate savings.

## Reproduction

From the repository root, with the locally retained artifacts available:

```sh
python3 docs/research/2026-09-16-blob-efficiency/measure.py > /tmp/blob-efficiency.json
cmp /tmp/blob-efficiency.json docs/research/2026-09-16-blob-efficiency/measurements.json
```

[measure.py](measure.py) uses only Python's standard library and performs no
network calls. Inputs and hashes, deduplicated per-sample measurements, public
audit coverage gaps, and aggregates are retained in
[measurements.json](measurements.json). The local source artifacts are not newly
committed or copied into this report. This research does not claim end-to-end
validation of either proposed format.
