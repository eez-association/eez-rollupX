# Derive blocks instead of publishing full blocks

The authoritative implementation plan is now [Derivable ChainOperation](../../proposals/derivable-chain-operations.md), with a [coverage ledger](../../proposals/derivable-chain-operations-coverage.md). It specifies the new 65,536-block signer default and incremental validation; references to 512 below describe the inspected pre-change code.

This is the primary recommendation after clarifying the objective: **extend the
history that fits in one allowed blob transaction before DA capacity forces a
`postBatch`**. Purchased-blob utilization and current receipt cost are secondary.
The allowed change remains inside `ChainOperation`; cross-chain data and its
encoding remain unchanged. No general-purpose compression is proposed.

## Answer

Information that a follower can deterministically reconstruct from the previous
authenticated state, published execution inputs, and canonical protocol rules
does not also need to appear verbatim in the blob. The blob must make those inputs
available. Existing settlement/proof commitments must still authenticate the
result and the exact applied prefix.

EEZ currently publishes full canonical blocks in tag `0x02`. This provides a
simple way to bind the payload to validated blocks, but it is not a fundamental
requirement of data availability. A new operations format can publish the recipe
for the blocks instead. The previous byte-string and header-sharing experiments
are useful baselines; they are not the limit of what can be removed.

## The roughly 30-minute limit has a plausible structural explanation

The retained devnet corpus contains 109 empty blocks. Their current encoded
block-item sizes range from 754 to 774 bytes, with median **764 bytes**. Their
median raw block RLP is 614 bytes; the current integer-list wrapper adds overhead.

For an illustrative six-blob allowance and a two-second L2 block interval:

```text
capacity = 6 × 126,976 = 761,856 bytes
empty-block data rate ≈ 764 / 2 = 382 bytes/second
time to exhaust capacity ≈ 761,856 / 382 ≈ 1,994 seconds ≈ 33.2 minutes
```

This excludes batch/semantic overhead and user transactions; it is not a measured
time-to-settlement for the user's run. Six blobs is the example allowance, not a
claim about a universal current Ethereum limit. The actual allowance comes from
the configured receiving-network profile and operator cap.

The important finding is that **elapsed empty history itself consumes hundreds
of bytes per block**. If a format encodes the number of empty blocks instead of
their headers, that source of linear DA growth can disappear. Replay work still
grows with block count.

## Field-by-field derivability

| Published full-block information | Proposed treatment | Required source/rule |
|---|---|---|
| Parent hash | Derive | Previous reconstructed block; authenticate the starting anchor. |
| Block number | Derive | Anchor number plus block position. |
| Timestamp | Derive for the fixed-cadence ordinary path | Anchor timestamp plus configured block interval; encode exceptions if other paths permit them. |
| State root | Recompute | Execute transactions and fork-specific pre/post execution operations. |
| Transactions root | Recompute | Exact ordered transaction list for that block. |
| Receipts root and logs bloom | Recompute | Execution receipts and logs. |
| Gas used | Recompute | Execution under the exact block environment. |
| Base fee | Derive | Parent header, active fee rules and chain configuration. |
| Withdrawals root | Recompute | Retained/derived withdrawal list; a root alone cannot supply withdrawal inputs. |
| Blob gas used / excess blob gas | Derive under the active fork | Included transaction types, parent values and fork parameters. |
| Requests hash | Recompute under the active fork | Requests produced by the defined execution transition. |
| Difficulty, nonce, ommers hash | Derive only under a specified supported profile | Post-merge constants and an enforced empty-ommer rule; do not assume for arbitrary block formats. |
| Fee recipient | Retain or derive from an authenticated rule | It is configurable in the attributes builder; a local default is not consensus. |
| Randomness / `prev_randao` | Retain or derive from an authenticated rule | Current ordinary builder supplies zero, but code explicitly anticipates future L1 derivation. |
| Gas limit and extra data | Retain or fix in the versioned profile | Current ordinary payload builder supplies shared constants. |
| Parent beacon root | Retain or derive from an authenticated rule | Ordinary builder currently supplies fork-gated zero; later L1-derived values need an origin rule. |
| Withdrawals / other variable body inputs | Retain unless the profile proves a deterministic value | Current ordinary builder supplies empty withdrawals. |
| User transactions and ordering | Retain | Signed transaction bytes and their block placement are not derivable from state. |
| Cross-chain transactions, entries, groups and semantic messages | Preserve | Outside the permitted optimization scope. |

Future fork fields need explicit handling. In particular, existing size accounting
already anticipates fields beyond those in the captured headers. A new profile
must not silently treat an unsupported field as absent.

## Why this is supported by the implementation

- [Ordinary sequencing](../../../crates/eez-driver/src/sequencer.rs),
  `commit_one`: advances timestamp by `l2_block_time`; `EthAttributesBuilder::build`
  supplies fee recipient, zero randomness and fork-gated body/environment fields.
- [Payload builder](../../../crates/eez-node-common/src/payload.rs): fixes shared
  builder gas limit and extra data.
- [Follower execution](../../../crates/eez-deriver/src/deriver.rs),
  `execute_block_with_template`: constructs the execution environment, applies
  pre-execution changes, runs transactions, and calls the block builder's
  `finish`. The reconstructed header is then compared with the supplied template.
  The template's output roots and bloom are not used as the inputs that generate
  those same outputs.
- The follower's `check_claimed_state` already checks the starting commitment and
  finds/verifies the actual settled block-hash endpoint, including partial
  settlement. These checks must remain effective under the new format.
- [Signer DA checking](../../../crates/eez-proof-signer/src/settlement/da.rs),
  `verify_self_contained_da_payload`: currently demands byte equality with full
  validated block RLP. This is the deliberate format-specific check that must
  change for a derivation format. The signer already has a validated block window
  against which to check the published recipe and its reconstruction.

The old transaction-only codec branches demonstrate that replay without a full
template exists, but they are **not a shortcut to a safe upgrade**. Their defaults,
metadata assumptions and cross-chain handling must not be presumed sufficient.
Nor may the new decoder synthesize headers with zero roots to satisfy the existing
full-template interface: it needs an explicit execution-input representation.

## Proposed initial format: derive ordinary history, preserve the terminal block

Keep the first version narrow. Represent preceding ordinary blocks by execution
inputs, and retain the existing full terminal/Sync block and all cross-chain
metadata. Intermediate blocks already have a composer guard against system
transactions. If an intermediate block does not fit the derivable profile, use
an explicit full-block record rather than assuming its inputs.

A conceptual operations payload is:

```text
new operations version
ordinary block count
versioned execution profile / any required shared inputs
ordered nonempty-block records:
    block offset, exact ordered transaction list
environment exceptions or full-block fallback records, if needed
unchanged terminal block representation
unchanged l2_entries encoding
unchanged outbound_group_sizes encoding
```

The authenticated anchor and range already supplied by settlement should not be
duplicated unnecessarily, but their relationship to these counts must be checked.
Missing nonempty-block records mean empty blocks only under a precisely specified
sparse-list format with ordered, unique, bounded offsets. Empty-block runs can
then take a few count bytes rather than hundreds of bytes for every block.

An empty block is **not** a no-op: pre-execution system calls, fee evolution,
timestamps, fork activations, and block-hash history can change. The follower must
build every intervening block. It must preserve transaction boundaries as well:
moving a transaction into a neighboring block can change execution.

This applies the useful principle in
[OP span batches](https://specs.optimism.io/protocol/delta/span-batches.html):
derive metadata from an anchor and defined sequencing rules, while retaining
transaction placement. It is not a proposal to adopt OP's cross-chain behavior,
transaction omission rules, hash truncation, or generic compression.

## Proof and follower requirements

### Existing on-chain check for a reconstructed Sync

For this rollup, the fields named `currentRoot` and `newRoot` commit **L2 block
hashes**, not just EVM state roots. `EEZ._applyRootUpdates` writes `newRoot` to
`rollups[rollupId].root` and emits `L2ExecutionPerformed(rollupId, newRoot)`.
The follower can therefore recompute the Sync header hash and compare it with
the commitment actually applied by L1. That header binds the state, transaction
and receipt roots, and its parent hash links the preceding history.

The existing pipeline already authenticates blob bodies against transaction
versioned hashes (`eez-l1/src/blob_source.rs`), attributes applied root updates
to batches (`eez-l1/src/scan.rs`), and checks the reconstructed starting and
settled endpoint hashes (`eez-deriver/src/deriver.rs::check_claimed_state`).
Partial/deferred settlement matters: use the observed applied endpoint, not
merely the last claimed root in postBatch calldata or the current registry root
after later settlements. An outer successful receipt alone is insufficient.
The follower also checks emitted L2 outbound-call hashes against the expected
settlement effects. These cross-chain checks remain unchanged.

This is an independent replay consistency check, not a new on-chain execution
proof. The repository's ECDSA proof-system contract verifies an authorized
signature; it does not itself execute the L2 EVM. Omitting derivable fields must
preserve signer validation and follower rejection of mismatches, without claiming
to replace the existing proof-system trust model. The new operations format still
needs replay-equivalence validation; existing checks do not automatically make
an unimplemented decoder correct.

### Requirements for the new operations version

1. Define deterministic execution-input rules for the supported ordinary profile,
   including changes across fork boundaries. Publish every unconstrained input.
2. Have the signer validate the recipe against its authenticated block window.
   Verify full reconstructed block hashes and anchor continuity; checking only
   transaction lists or the final state root is insufficient.
3. Let the follower replay from its authenticated anchor using only DA, protocol
   configuration and authenticated L1 inputs, without querying the sequencer for
   omitted headers. Retain existing terminal cross-chain reconstruction and
   applied-prefix/endpoint checks.
4. Keep all existing operations versions readable and specify activation for the
   new one. Do not weaken existing per-version acceptance rules.
5. Update composer, interval reservations and DA sizing to charge the new format.
   Otherwise an old conservative full-header estimator can still force early
   settlement even when the new bytes would fit.
6. Bound block counts, transaction counts and replay work separately from encoded
   bytes. A tiny payload representing millions of empty blocks must not create an
   unbounded replay/proof task. Existing unsettled-block/time policies remain
   independent reasons to settle; byte reduction alone does not override them.

The next decisive experiment is **independent replay equivalence**, not another
byte-sharing benchmark: encode long ordinary spans as execution inputs, reconstruct
them without the original headers, and compare every block hash against the
original chain. Include empty history across forks, nonempty blocks, reverted
transactions, environment changes, reorgs and the unchanged terminal Sync.
Measure bytes per minute, maximum history per allowed transaction, replay/proof
time and the precise trigger that closes the batch. No implemented derivation
format or production savings percentage is claimed yet.
