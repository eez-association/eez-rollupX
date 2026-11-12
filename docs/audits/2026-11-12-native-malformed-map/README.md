# Bounded native malformed-input case map (#143)

| Field | Value |
| --- | --- |
| Date / scope | 2026-11-12; bounded assessment complete; investigation of [#143](https://github.com/eez-association/eez-rollupX/issues/143), handoff N9 only |
| Frozen source | `eez-association/eez-rollup0` at `1b66d9bda675e6f8e83f514153a97136909acc06`; protocol `143d87e62cade98bfc122d612638dcaaf1c85633` |
| Frozen target | `eez-association/eez-rollupX` at `31e148f50dd79730b17d113ac78795eb377eb5c5`; protocol `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Issue's later source check | Target `716a8a3bad4415ade7e9a6bc3af4a1b35419b232`; not the frozen review baseline |
| This PR's base / tested implementation | `dbe1cdb9047e7f3502505f381aaa602e6ebf552f` (fresh association main; its parent is handoff main `20243f70839ec69df9312731e960cd311e060050`) |
| Protocol / dependencies | Same target protocol pin, initialized recursively; no protocol edits, dependency upgrades or production changes |
| Starting checkout | Clean, detached `bad8d7eef7e16ac872f354d31af257f8646b2fac`; moved to branch `codex/143-native-malformed-map` at the PR base before investigation |
| Review / tracking | [Backport procedure](../../runbooks/backport-review.md), parent [#135](https://github.com/eez-association/eez-rollupX/issues/135), original review [#133](https://github.com/eez-association/eez-rollupX/issues/133) |

The completion criterion is unchanged: **Classify source negative cases as
target-equivalent, intentionally inapplicable or an evidenced gap; record tests
actually run and any focused follow-up.** This is the selected-case assessment,
not a restart of the backport audit or a replacement of its frozen inventory.
The original review's input provenance was **not independently checked**.
Fetching the named Git objects here does not repair that provenance limitation.

## Result and boundary

The map covers all 16 invalid span fixtures, all three invalid action-manifest
fixtures, and all five mutations in `action_manifest_grammar_rejects_each_invalid_boundary`
at the frozen source: 22 target-equivalent cases and two intentionally
inapplicable source restrictions. There is no demonstrated functional decoder defect in
this map. The identified gap was direct raw-byte test coverage for the mapped
outer action boundaries; this PR adds two focused tests to close that coverage
gap using the existing production decoder. They are wired into the library
test module so the existing debug unit CI lane (`--workspace --lib --bins`)
executes them. No second decoder or source grammar is introduced.

“Target-equivalent” means the analogous invalid input is rejected at the owning
target boundary, not that source bytes, error names or failure precedence must
match. “Intentionally inapplicable” identifies a source restriction which is not
the accepted target contract. “Evidenced gap” distinguishes missing validation
evidence from a demonstrated functional failure. Test names below identify
**test-source evidence**; only the execution record establishes what ran.

The target payload and blob codec directories have no diff between the frozen
target and this PR's base (including the [frozen target negative tests](https://github.com/eez-association/eez-rollupX/blob/31e148f50dd79730b17d113ac78795eb377eb5c5/crates/eez-payload-codec/src/v0/tests.rs)). The current accepted profile already includes both
inner operations versions: V0 preserves historical static rows; V1 is emitted
by new producers and permits completed static-row reuse. Their columns are
identical. The outer Core message stream remains version 0. Do not transplant
the source's “version 1 is unknown” assertion into the inner target decoder.

## Owning contracts and production path

Read against the target's exact protocol pin:

- [Core blob format](../../../eez-core-protocol/docs/blobs/BLOB_FORMAT_SPEC.md),
  §§1.1, 1.2, 2.2–2.8 and 5.1: fixed-width scalar fields, bounded u32 byte lengths,
  bracket discipline, complete fields, permitted non-minimal outer varints,
  zero padding only after close. The
  [Core codec tests](../../../eez-core-protocol/test/blob/BlobCodec.t.sol) include
  non-minimal-length acceptance, truncation, nesting and padding rejections;
  these were inspected, not executed here.
- [Accepted native contract](../../proposals/native-system-transactions.md#direct-v0-wire-contract):
  positive u32 block spans; compressed count tokens; shortest inner integers;
  maximal metadata runs; extraData at most 32 bytes; exact transaction bytes and
  full exhaustion. Source dense per-block count varints are not this grammar.
- [Signer specification §11](../../../crates/eez-proof-signer/SPEC.md): one leading
  ChainOperation, complete semantic brackets, one final close, no calldata tail;
  L1-origin `tx_data` empty, L2-origin `tx_data` equal to the signed positioned
  user transaction. Neither syntactic acceptance nor a published blob proves
  execution, authorization, or canonical consumption.

The [Composer producer](../../../crates/eez-composer/src/composer.rs) combines
`eez_driver::derivation::encode_operations` and
`eez_protocol::blob_semantics::encode_semantic_transaction` through
`eez_blob_codec::encode_native_stream`.

The signer's [DA admission](../../../crates/eez-proof-signer/src/settlement/blob_da.rs)
checks physical blobs and carrier selection, calls `decode_native_payload`, then
`decode_native_semantics`. [Operations verification](../../../crates/eez-proof-signer/src/settlement/da.rs)
calls `Payload::decode`; [semantic binding](../../../crates/eez-proof-signer/src/settlement/semantics.rs)
independently binds origin data and effects. The L1 reader's
[blob source](../../../crates/eez-l1/src/blob_source.rs) uses the same native and
semantic decoders; [deriver](../../../crates/eez-deriver/src/deriver.rs) replay
uses `Payload::decode`. The shared [semantic parser](../../../crates/eez-protocol/src/blob_semantics.rs)
reads the operations version for construction policy; reading that prefix alone
is not full operations validation.

The [blob decoder](../../../crates/eez-blob-codec/src/lib.rs) owns the outer stream,
physical packing and native carrier shape. The
[payload decoder](../../../crates/eez-payload-codec/src/v0.rs) owns borrowed column
validation; it does not authorize settlement. This separation explains why the
source's single action-container decoder does not map to one target function.

## Span fixture map (16 cases)

Source inventory: [payload_v0_conformance.json](https://github.com/eez-association/eez-rollup0/blob/1b66d9bda675e6f8e83f514153a97136909acc06/crates/eez-payload-codec/tests/fixtures/payload_v0_conformance.json),
loaded by [conformance.rs](https://github.com/eez-association/eez-rollup0/blob/1b66d9bda675e6f8e83f514153a97136909acc06/crates/eez-payload-codec/tests/conformance.rs).
The fixture's `specCommit` is `7c99bcf923e0ee0f867549a750d1ab1373bf845d`;
it is an identifier in the inspected fixture, not independent verification of
that draft's acceptance or provenance.

Target test abbreviations refer to
[v0/tests.rs](../../../crates/eez-payload-codec/src/v0/tests.rs):
**U** = `all_shortest_u32_boundaries_and_count_tokens_are_checked_without_expansion`,
**M** = `metadata_must_cover_exactly_and_be_maximal`,
**L** = `lengths_and_transactions_fail_at_their_boundary`,
**V** = `wire_version_is_explicit_and_columns_are_unchanged`,
**E** = `independent_empty_vectors_include_terminal_and_full_u32_runs`.

| Source invalid fixture | Disposition | Target rule / evidence |
| --- | --- | --- |
| `d4.2-vector-3-empty` | Target-equivalent | `take(1)` rejects empty input as `Truncated`; L tests every proper prefix, including empty. |
| `d4.2-vector-4-unknown-version` | Intentionally inapplicable for byte `01` | Target inner V1 is supported; a bare `01` is truncated, not an unknown version. V asserts V0/V1 acceptance and rejection of 2/255. Outer V1 remains unsupported; `native_wire_truncations_lengths_and_padding_are_bounded` explicitly asserts `UnknownProtocolVersion(1)` on both raw and packed paths. |
| `d4.2-vector-5-non-shortest-block-count` | Target-equivalent | Inner `varint` rejects a terminating zero after a continuation (`86 00`); U exercises the shared integer reader with `80 00`. No claim that every source vector was executed verbatim. |
| `d4.2-vector-6-impossible-block-count` | Intentionally inapplicable as a dense-count bound | `00 06` still fails (missing count token), but the source's one-count-byte-per-block prerequisite must not be ported. E admits compact spans through `u32::MAX` without expansion; count coverage must instead match. |
| `d4.2-vector-7-trailing-byte` | Target-equivalent after count-column translation | Exact body exhaustion rejects a suffix as `Trailing`; U appends a byte to a valid compact 600-block body. Source's six dense zero counts are not a native encoding. |
| `zero-block-count` | Target-equivalent | `EmptyRange`; U asserts `00 00`. |
| `truncated-block-count-varint` | Target-equivalent | Missing continuation byte is `Truncated` through `varint` → `take`. U covers exhausted integer input and L covers all prefixes of a valid body; source bytes `00 80` were not separately executed. |
| `six-byte-block-count-varint` | Target-equivalent | Fifth continuation byte cannot fit a u32 and fails as `Varint`; U exercises five continuation bytes. |
| `fifth-byte-overflow` | Target-equivalent | Fifth byte `10` exceeds u32; U checks `ff ff ff ff 10`. |
| `zero-beneficiary-run` | Target-equivalent after count-column translation | `validate_metadata` requires positive run length; M asserts `Metadata`. |
| `beneficiary-run-overshoots` | Target-equivalent after count-column translation | Checked cumulative coverage cannot exceed block count; M asserts `Metadata`. |
| `adjacent-beneficiary-runs-repeat` | Target-equivalent after count-column translation | Equal adjacent values violate maximality; M asserts `Metadata`. |
| `extra-data-over-32-bytes` | Target-equivalent | `metadata_value` rejects length 33 as `ExtraData`; M asserts it. |
| `adjacent-empty-extra-data-runs-repeat` | Target-equivalent after count-column translation | Empty byte strings are equal values too; M asserts `Metadata`. |
| `zero-length-transaction` | Target-equivalent | Positive transaction length required; L asserts `TransactionLength` with sufficient remaining bytes to pass count admission. |
| `transaction-body-shorter-than-length` | Target-equivalent | Claimed length cannot exceed remaining bytes; L checks the oversized length and every proper prefix of an encoded transaction. Target additionally validates exact EIP-2718 decoding/re-encoding. |

The source's `frozen_span_vectors_cover_every_reachable_rejection_class` checks
its own error inventory, not target completeness. No target coverage percentage
is inferred from it. Native-specific count-token rejection is separately covered
by `rejects_noncanonical_truncated_overflowing_and_adjacent_runs` in
[v0/counts.rs](../../../crates/eez-payload-codec/src/v0/counts.rs).

## Action fixture / mutation map (3 + 5 cases)

Source fixtures: [action_manifest_v1_conformance.json](https://github.com/eez-association/eez-rollup0/blob/1b66d9bda675e6f8e83f514153a97136909acc06/crates/eez-payload-codec/tests/fixtures/action_manifest_v1_conformance.json).
Source mutations: `action_manifest_grammar_rejects_each_invalid_boundary` in
[conformance.rs](https://github.com/eez-association/eez-rollup0/blob/1b66d9bda675e6f8e83f514153a97136909acc06/crates/eez-payload-codec/tests/conformance.rs#L223).
Source `decode_container` reads a calldata container without a closing blob
marker. Target native DA is blob-only, with a required close and a compact inner
payload. Copying source fixtures unchanged would reject at an unrelated boundary.

**B** below is the new `mapped_action_boundaries_fail_at_the_raw_decoder` in
[malformed_native.rs](../../../crates/eez-blob-codec/src/tests/malformed_native.rs).
It constructs native bytes independently of the stream encoder, checks a valid
control and asserts both raw unpadded `decode` outcomes and padded
`decode_native_payload(pack(...))` outcomes. Production native callers unpack
full blobs, so padding can complete a field before another check rejects it.
The following rows distinguish these paths; rejection equivalence does not
require identical error precedence.

| Source case | Disposition | Target rule / evidence |
| --- | --- | --- |
| fixture `non-empty-initiating-tx-data` (L1 origin, one byte) | Target-equivalent at semantic binding | The outer field is opaque, so the byte decoder may accept it. `verify_native_semantics` rejects nonempty L1 data as `InboundTransactionData`. Existing `native_inbound_semantics_bind_both_sides_and_reject_result_mutation` asserts that error with a 21-byte L1 copy. Test-source evidence only here. |
| fixture `missing-finish-marker` | Target-equivalent; direct byte-test gap filled | B removes Finish while retaining the required close; `UnclosedBracket`. Removing the entire suffix including close yields `CloseBlobStreamMissing` on raw unpadded input, but `Codec(InvalidMessageType(0))` after packing. B asserts both paths. |
| fixture `second-chain-operation` | Target-equivalent; native-profile byte-test gap filled | A lone second type byte is `TruncatedStream` on raw unpadded input; padding completes a chain-0 operation before `Codec(InvalidMessageType(0))` rejects the next zero byte. B asserts both, then supplies a complete second operation between brackets and close to reach the intended native-profile rejection, `UnexpectedMessages`, after generic Core decoding succeeds. |
| mutation `wrong_call` (Call becomes Initiate) | Target-equivalent; direct byte-test gap filled | B reaches `NestedTransaction`. Native supports nested calls, not nested transaction roots. |
| mutation `unknown_return` (`ff`) | Target-equivalent; direct byte-test gap filled | B reaches `InvalidMessageType(255)`. |
| mutation `wrong_finish` (Finish becomes Initiate) | Target-equivalent; direct byte-test gap filled | B asserts `TruncatedStream` on raw unpadded input and `Codec(NestedTransaction)` after packing: padding completes the new Initiate fields, then placement rejects the still-open transaction. |
| mutation `multibyte_tx_data` (L1 origin, two bytes) | Target-equivalent at semantic binding | Same nonempty L1 check as the one-byte fixture; existing 21-byte test shows it is not a one-byte-only restriction. The source's blanket ban is intentionally inapplicable to L2 origins, whose signed transaction bytes are required. |
| mutation `truncated_tx_data` (length 2, zero bytes remain) | Target-equivalent; direct byte-test gap filled | B asserts `TruncatedStream` at the length boundary on raw unpadded input. After packing, two padding zeros complete `tx_data`; the next zero yields `Codec(InvalidMessageType(0))`, also asserted before semantic binding. |

The existing signer tests are in
[settlement/tests/semantics.rs](../../../crates/eez-proof-signer/src/settlement/tests/semantics.rs).
`native_outbound_semantics_reject_tx_data_and_call_mutations` separately checks
exact L2-origin data binding. The existing
`only_l2_origin_transactions_publish_their_signed_transaction` test in
[blob_semantics.rs](../../../crates/eez-protocol/src/blob_semantics.rs) checks
producer behavior. Their presence is not a claim of fresh execution here.

## Canonicality, length and trailing-data cross-check

The second new test, `native_wire_truncations_lengths_and_padding_are_bounded`,
rejects outer version `01` on raw and packed paths and checks all 141 proper
unpadded byte prefixes of the independent native control, plus all
four outer byte-length fields (`operations`, `tx_data`, `data`, `return_data`):
non-minimal equivalent lengths remain accepted, fifth-byte overflow (including
a continuation at the fifth byte of a six-byte input) is rejected, and `u32::MAX` cannot claim unavailable bytes.
It also distinguishes permitted zero padding from a nonzero trailing byte.
These are bounded examples, not exhaustive fuzzing or a proof of parser safety.

Existing outer tests `accepts_non_minimal_varint`,
`rejects_u32_overflowing_fifth_varint_byte`, `rejects_non_zero_padding_after_close`,
and `rejects_non_zero_field_element_msb` retain their roles. Existing inner tests
U/L require exact body exhaustion; `ordinary_column_preserves_history_bytes_and_rejects_cross_chain_and_blobs`
also rejects trailing bytes inside a transaction. Outer padding acceptance must
not be carried into either inner rule. The source error enum also names classes
with no negative fixture in this selected corpus (for example compact-value
errors); an enum arm alone is not an additional executed conformance case.

## Execution record and remaining limits

See [validation.txt](validation.txt) for commands, nonzero selections, tool and
executable identities, first failures and final outcomes. Source fixtures and
Solidity tests were inspected only. The source's historical passes and the
original review's source-only checks do not establish a current test pass.

No deployments, service changes, wallet operations, CI reruns or issue closure
were performed. No semantic execution replay, process E2E, vulnerability scan or
performance measurement is claimed. The map leaves broader source cases,
exhaustive malformed grammars, allocator limits, and end-to-end signer/follower
rejection coverage outside this investigation. No confirmed functional defect
requires a separate implementation follow-up; any later such finding should be
tracked separately rather than expanding #143 silently.

## Ponytail review

What this change does: records the selected source-to-native case map and adds
two raw-wire regressions. It preserves the accepted grammar and all production
behavior.

Review used [Ponytail review instructions](https://github.com/dietrichgebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail-review/SKILL.md)
(SHA-256 `33fe4ebe22a460924e5d4a261e223df22e409cc94ab76182bee9fb91a3ef022e`).
This was the task agent's own review, not an independent reviewer or subagent.
It checked the diff, production decode/profile/semantic callers, accepted format
rules, source fixtures, test failure boundaries and CI test selection. The
expected load for the added code is small fixed unit-test fixtures; no runtime
path changes.

The first review found that a standalone integration-test file would not be
selected by the debug library/binary lane. The tests were moved into the library
test module before completion. No remaining correctness, safety, resource, or
unnecessary-abstraction finding was identified in the bounded change.

Verdict: Ship, subject to the validation limits above. Not checked: independent
security review, source test execution or live end-to-end rejection behavior.

## Opus review corrections (2026-11-12)

Claude Opus 5.5 independently reviewed candidate `3405b40c` and found an
evidence-labeling error (P2) and a missing outer-version regression (P3). The
earlier test assertions were correct for unpadded input, but the assessment
overstated their correspondence to padded production input. The action map now
distinguishes those outcomes, and the same fixtures assert both paths. Outer
version `01` now has exact raw and packed rejection assertions. This changes no
production behavior, grammar, case dispositions, protocol pin or dependencies.
The initial Ponytail verdict above is historical; it did not identify these two
findings. See the appended validation record for the correction checks.
