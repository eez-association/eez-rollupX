# Two-byte DA counts for new RollupX networks

Current-network contract (2026-09-28): [native transactions and direct V0](native-system-transactions.md)
supersedes the historical wire/environment/system-transaction descriptions below.
The old envelope is not a runtime option. Earlier validation remains evidence for
its recorded revision only; current validation is in the linked implementation audit.

| Field | Value |
| --- | --- |
| Design status | Implemented; based on main after PR #34 merged; validation is revision-specific below |
| Date | 2026-09-25 |
| Source baseline | main, `f128b6d9bfba7f9edb251490f4a3a01b918c753e` (same source tree as PR #34 head `4956d754`) |
| Protocol baseline | `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Related work | [Issue #35](https://github.com/eduadiez/rollupX/issues/35); latest scope narrows that issue |

## Scope and requirements

Implement the agreed count tokens without changing execution rules, signed system
transactions, or richer cross-chain actions. No new candidate-profile validator,
draft consensus adoption, migration, or legacy runtime decoder. The payload is
RollupX-specific, not a claim of compatibility with the draft's complete V0.

| ID | Requirement | Acceptance |
| --- | --- | --- |
| R1 | Canonical two-byte counts and maximal empty runs | Boundary, exhaustive count, malformed and mutation tests |
| R2 | Exact reconstruction inputs retained | Existing driver and independent signer replay tests |
| R3 | Exact composable admission sizing | Associative summaries equal encoded bytes, including environment/full exceptions |
| R4 | Bounded parsing | Check declared range before allocations, exact coverage, no per-empty-block allocation |
| R5 | Richer entries unchanged | Preserve entry bytes, order and group sizes; existing cross-chain regressions |

## Wire design

Inner operations are `00 || RLP([profile=1, ordinary_count, initial_environment,
columns, terminal_block, l2_entries, outbound_group_sizes])`.

`columns = [counts: bytes, overrides: list, transactions: list(bytes)]`.
The environment and group representations retain the existing RollupX encodings.
`terminal_block` is an RLP byte string containing the exact full consensus block
RLP; `l2_entries` is an RLP list of byte strings containing the exact entry bytes.
Neither uses per-byte integer lists. Only these outer wrappers change: terminal
contents, entry contents/order and group sizes are preserved in full.
Outer native blob framing and commitments are unchanged.

Counts cover ordinary positions, excluding the retained full terminal:

| First byte | Meaning |
| --- | --- |
| `00` | One empty block |
| `01..7F` | One block with 1..127 transactions |
| `80..BF` plus raw suffix | Count `((first & 63) << 8) | suffix`, 128..16,383 |
| `C0..FE` | 2..64 empty blocks (`first - C0 + 2`) |
| `FF` plus shortest u32 Base128 varint | At least 65 empty blocks |

Adjacent empty tokens, short two-byte counts, short/overlong/overflowing long
runs, truncation and under/overcoverage are errors. Runs cross environment
changes and empty full-block exceptions: those are metadata, not transaction
activity. The full block's transaction count must match its count-column value.
All blocks, including full exceptions and terminal, have a 16,383 transaction
ceiling. The existing 65,536-total-block resource limit remains in force.

An override is `[gap, kind, data]`, with gap measured from the position after
the previous override (or start). Kind 1 carries exact full-block RLP as bytes;
kind 2 carries the new environment and applies at that position. Kind 0 is not
used on wire. Overrides are strictly ordered and inside the ordinary range;
no-op environment changes are rejected. Full exceptions do not change the
active environment. Derived transactions are flattened in position order;
full exceptions carry their own transactions, never duplicated in this column.
All transaction bytes and full blocks are checked for exact canonical encoding.

The existing sparse in-memory record and block-plan interfaces remain useful:
wire decoding reconstructs only explicit transaction/override records. Empty
stretches do not allocate one record per block. Execution and authentication
remain independent consumer obligations; decoding does not authorize signing.

The alternative of attaching a count column to unchanged transaction records
would duplicate framing. Dropping environments/full terminal would instead
require the execution changes explicitly deferred by the user.

## Compatibility and recovery

New networks only. All producers, signers and followers must use this format
from genesis. Old payloads/networks are unsupported, and tag reuse does not
authorize decoding historical tag-0 data. There is no mixed-version rollout or
downgrade/replay guarantee. Existing deployments are not being modified.

## Validation plan and status

Run focused codec tests first, then affected driver, signer and composer tests,
formatting and workspace clippy. Run affected source-based process tests with
the pinned protocol. CI owns the full release/Kurtosis matrix. Compare actual
complete RollupX payload sizes, not draft V0's metadata-only examples: previous
31-byte or 47-byte examples do not describe this retained-terminal format.

The following historical results were obtained on the previous PR #34 base
(`68fde5bb`) and protocol pin (`3c0cdf49`); they do not validate the September 24
base update. See the [base-update record](../audits/2026-09-22-two-byte-da-v0/pr34-update.md)
for checks on the current baseline.

The byte-string revision passed 43 codec tests, 832 component tests, workspace
all-target clippy, formatting and nine focused process cases on their first
attempt. Three manual component tests remained ignored. The subsequent size-only
optimization passed 43 codec tests with expanded randomized full-block coverage,
221 composer tests (one manual test ignored), workspace clippy and formatting;
process tests were not repeated for that arithmetic-equivalent follow-up.
The full 78-case process selection and release/Kurtosis matrix were not repeated.
Earlier evidence, including three startup flakes, applies only to the earlier
integer-list revision.
See the
[validation record](../audits/2026-09-22-two-byte-da-v0/README.md) for the separate
revision results, retained failures and remaining checks. No deployment has been
performed.

### Complete payload byte comparison

The deterministic codec fixture `complete_payload_size_comparison_with_pr34_record_framing`
compares the previous PR #34 writer with this writer, including the same published
environment, canonical full terminal, empty sidecars, tag and RLP framing. Each
nonempty ordinary position has one identical signed transaction. These are codec
size fixtures, not execution-valid throughput benchmarks (gas/signatures are not
execution-validated by the codec).

| Ordinary workload (plus one terminal) | PR #34 bytes | V0 bytes |
| --- | ---: | ---: |
| No ordinary blocks | 697 | 611 |
| 600 empty | 699 | 616 |
| 600 nonempty | 13,301 | 11,419 |
| 600 alternating empty/nonempty | 7,001 | 6,319 |

The existing RollupX encoding already compressed empty gaps. The new count column
adds a few fixed bytes for an all-empty interval, but the byte-string terminal
wrapper saves 89 bytes in this fixture, making all four complete payloads smaller.
Dense intervals also save per-nonempty-block framing. Savings depend on the actual
terminal/entry bytes; these are examples, not a universal 89-byte reduction.
This is distinct from the comparison against the upstream draft's per-block counts.
