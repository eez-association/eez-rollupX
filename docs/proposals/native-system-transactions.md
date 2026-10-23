# Native transactions and canonical derivation

| Field | Value |
| --- | --- |
| Design status | Implemented for the owner-selected RollupX profile; native-format conflict resolved by the task owner on 2026-09-28. This is not acceptance of the upstream draft specification. Validation and wider-draft limits are recorded in the audit. |
| Date | 2026-09-28 |
| Source baseline | Merged PR #42: `main` at `de5030c782e29ff07150b2728c7b564fd607f802`, with the same tree as final PR head `cf8500e80cc10b58ab6acd59b2125dd098aa770c` (2026-09-29) |
| Protocol baseline | `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Related work | [Upstream #154](https://github.com/eez-association/eez-rollup0/pull/154), [upstream #162](https://github.com/eez-association/eez-rollup0/pull/162), [issue #35](https://github.com/eduadiez/rollupX/issues/35), [implementation audit](../audits/2026-09-28-native-system-backport/README.md) |

## Contract and scope

The task owner explicitly waived backward compatibility and requested the simplest
implementation on 2026-09-28. Ship one native transaction contract, one V0 DA
format and one required genesis profile. Remove old-format readers and fallback
paths rather than maintaining dual operation. Existing networks/databases are
not upgrade targets; start a new network with fresh genesis and datadirs. No
historical-format replay, database migration or compatibility mode is required.
This does not waive authentication, same-format restart/reorg recovery, or #42's
queue and partial-consumption invariants.

Replace signed L2 system transactions with canonical, keyless native transactions.
Preserve the fork's grouped outbound calls, nested callbacks, static lookups,
reverted frames, independent settlement authorization, and block-hash commitments.
Implement the remaining canonical derivation requirements of issue #35 on that
foundation. PR #37's count codec alone does not resolve #35: its surrounding RLP
envelope, terminal block representation, and environment transport still differ.

**Explicit amendment to #35:** the owner selected #154's nonce-bearing type `0x76`,
sender `0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0076`, and fixed 2,000,000 metered gas
(the gas budget is superseded by the [remaining-gas native budget](system-tx-remaining-gas-cap.md)).
The issue's `0x45`, absent nonce, and remaining-block-dependent gas requirements
are superseded for this implementation. The remaining issue requirements remain
in scope. The draft at `7c99bcf923e0ee0f867549a750d1ab1373bf845d` is a design
reference, not an accepted replacement for the pinned core protocol.

## Requirements and acceptance

| ID | Required invariant | Acceptance evidence |
| --- | --- | --- |
| N1 | Canonical `0x76 || rlp([chainId, nonce, to, value, input])`, fixed sender and EEZL2 destination; no signature or key | Independent wire vector, round trip, hash, malformed/target tests |
| N2 | Fixed 2M gas (superseded: the budget is now the remaining block gas up to 16,777,216, see [remaining-gas native budget](system-tx-remaining-gas-cap.md)), zero fees and nested `GASPRICE`; normal transactions retain fees | Execution, nested guard and reimbursement, paid comparison, noncanonical raw environment tests |
| N3 | Mint committed inbound value before execution; outer revert/halt removes mint while retaining metered gas and nonce behavior | Success, preexisting balance, revert, OOG, overflow, invalid nonce and preview tests |
| N4 | Native receipt identity and storage round trip; native admission unavailable to public RPC/pool; blobs excluded from L2 | Receipt/RPC/storage and production admission regressions; L1 blob simulation retained |
| N5 | Composer, follower, replay and signer agree on bytes and state | Shared reconstruction, consumed-prefix nonce tests, multiple actions/restart, independent process replay |
| T1 | Supplied inbound ABI becomes a checked shared type at the first meaningful boundary and stays checked | Malformed shapes and wrong destination/source rejected without normalization; production-call trace |
| T2 | Shape validation is distinct from settlement authorization | Substituted settlement ABI, call/rolling hash, wrong identity and reconstructed bytes rejected |
| D1 | Direct V0 body, canonical count/metadata runs and transaction length stream over the whole interval including terminal block | #35 exact size vectors and malformed/boundary/fuzz cases; baseline comparison |
| D2 | Bounded parsing of full u32 wire runs, no eager empty-run expansion or artificial 65,536 wire cap | Allocation/resource-limit and canonical integer tests |
| D3 | Deterministic headers and fixed timestamp step; delayed origin/PREVRANDAO and recovery follow the current offline-history contract | Header/hash vectors and independent derivation with catch-up, live anchor and rollback |
| D4 | Canonical terminal pure prefix and authorized native action reconstruction, preserving the fork's richer explicit profile | Signer, DA, partial-consumption, nested/static/reverted process tests |
| D5 | Fixed L2 execution/header profile including EIP-2935 and empty requests hash, independently of L1 rules | Empty/nonempty block tests, pre-execution state, fee and gas/header checks |
| B1 | Preserve #42's FIFO prefix rebuilding, candidate-local execution cache, shared deadline and canonical-only queue completion | Existing composer, dynamic admission, queue and prefix equivalence tests |
| C1 | Fresh-genesis compatibility is explicit; keys removed only from system transaction construction | Genesis generation/check, configuration consumers, deployment and rollback documentation |

## Architecture and upstream mapping

| Upstream change | Classification and target |
| --- | --- |
| #154 native envelope, receipt and engine primitives | Adapt into new `eez-primitives` on the existing Reth/Alloy graph |
| #154 minting EVM and L2 admission | Adapt into `eez-evm` and node integration; retain L1 execution separately |
| #154 deterministic system reconstruction | Adapt `eez-protocol::system_tx`; preserve whole groups and committed nested value |
| #154 composer integration | Adapt existing `local/build` and its #42 prefix cache, never replace with upstream's old builder |
| #154 Stateless generic replay | Adapt to the pinned Reth-aligned Stateless checkpoint API, not the upstream PR's older dependency graph |
| #154 separate stateful/stateless prover roles | Obsolete here: integrate into the independent proof signer |
| #154 key and prefunding removal | Applicable across runtime, genesis generation, fixtures and operator configuration |
| #162 checked inbound sidecar | Adapt to the fork's nested-capable validated profile; preserve independent authorization |
| #162 explicit inbound/outbound direction | Existing settlement counts and grouped outbound structures already provide direction; retain them |
| #162 native gas semantics | Port nested gas-price, guard and reimbursement regressions |
| #35 count tokens | Already partially present via #37; retain canonical codec behavior while replacing the enclosing format |

The checked inbound type must not erase the distinction between admitted input,
validated execution, and authorized effects in the signer. An ABI decode or a
shape check alone never authorizes minting. The fork already verifies nested
complementary tables; a narrower upstream one-call representation cannot replace
those checks. Explicit direction comes from the existing grouped reconstruction
and authenticated settlement framing, not `proxyEntryHash == 0`.

Native adapters use the currently pinned Reth, REVM and Alloy interfaces. The
execution snapshot cache remains candidate-local under the same parent and
environment. No new queue persistence or unrelated scheduling changes are planned.

## Compatibility and recovery

### RollupX action-profile extension

The new network retains the pinned fork's outbound, grouped, nested, static and
caught-revert semantics. It therefore does **not** claim the draft's inbound-only
candidate V1 profile. Its operations body is the direct columnar V0 specified in
#35, with the two-byte count amendment. There is exactly one complete local
`ChainOperation`; no entries, groups, full blocks or environment overrides are
serialized inside that body.

The authenticated enclosing native EEZ stream supplies the existing bracketed
`InitiateCrossChainTransaction` / call / result / snapshot / revert / finish
messages. The initiating chain is required for every transaction. The exact
signed origin transaction is required for an L2 origin, where it is the Sync
block's user transaction and is published nowhere else; an L1 origin publishes
empty `tx_data`, because its transaction is already on L1 and cannot be bound
by the signer or used by followers. The shared semantic materializer derives the
ordered L1/L2 tables and whole outbound groups from those messages. Both the
signer and follower must bind those derived tables to the actual settlement,
origin identities and transaction bytes; decoding an action alone is not
authorization. No unused table rows or discarded semantic suffix are permitted.

The terminal operations position contains only its ordinary transaction prefix.
Authorized actions append canonical native transactions and, for outbound
groups, the exact signed origin transaction after their native table-load
transaction. Inbound actions retain checked sidecars through reconstruction.
Each accepted prefix is independently executed and committed by block hash.
Failed outer inbound actions append no L2 transaction; caught failures and static
observations within supported whole groups retain the pinned core's semantics.
This explicit extension replaces the draft's restriction against outbound or
nested actions; it does not relax authentication or introduce another wire
selector. Protocol transaction identity follows the owner's `0x76` amendment.

### Direct V0 wire contract

```text
00 || uvarint32(block_count)
   || count_tokens
   || beneficiary_runs
   || extraData_runs
   || transaction_lengths
   || exact_EIP2718_ordinary_transaction_bytes
```

All columns include the terminal position. There is no profile selector in this
body. `block_count` is positive and at most u32::MAX. A positive count describes
one position: bytes `01..7f` encode 1..127; `80..bf` followed by one raw byte
encode `((first & 63) << 8) | second`, restricted to 128..16,383. `00` is one
empty position; `c0..fe` encode 2..64 empty positions; `ff` followed by a shortest
u32 varint encodes 65 or more. Empty runs are maximal, even across metadata
changes, and must cover exactly `block_count` with no overshoot.

Beneficiary runs are `uvarint32(length) || address[20]`. Extra-data runs are
`uvarint32(length) || u8(byte_length) || bytes`, at most 32 bytes. Each column's
positive runs cover the whole range, and adjacent equal values are forbidden.
Each transaction length is a positive shortest u32 varint. The following exact
EIP-2718 bytes must decode/re-encode canonically, exhaust their length, and belong
to supported ordinary transaction types. Native and blob transactions cannot
appear in this column. The whole body must be exhausted. All u32 varints have
at most five bytes with fifth byte at most 0x0f; overlong encodings are rejected.

Parsing and terminal lookup are proportional to supplied bytes, not expanded
empty positions. Transaction counts and lengths are checked before allocation;
`T <= remaining_bytes / 2` bounds the length/data suffix. Operator replay quotas,
timeouts and cancellation are separate from wire validity. Exact producer
summaries merge count and metadata runs at range boundaries before measuring the
outer native EEZ stream. The signer defaults to 65,536 blocks per request, but
that quota is configurable and is not the u32 format bound.

For 600 empty positions with one beneficiary and empty extraData, the complete
body is `00 d8 04 ff d8 04 d8 04 || address[20] || d8 04 00`: 31 bytes. Persistent
codec vectors and an independent producer test protect this contract. The
upstream draft still requires its separate count amendment through the specs
team's process; this implementation does not publish or imply that acceptance.

The terminal variant B[0] contains its ordinary prefix, including all mandatory
pre-execution changes. B[i] additionally contains the first i authorized action
groups. These are block-hash commitments at the same height; B[0] is not the
preceding block. The fork's existing failed-outbound carrier schedule remains
necessary because reverted L1 effects cannot persist root updates.

### Delayed history and offline authentication

The [L1 history and offline proving contract](l1-origin-and-offline-proving.md)
supersedes this proposal's settlement-driven seed selection and signer RPC
provenance. A fixed genesis schedule selects a maximal L1 source and installs
its history and PREVRANDAO together before applications. Settlement does not
refresh the seed. The same rules apply to pending calls, terminal variants,
restart and follower replay.

The signer receives complete L1 ancestry through v2 witnesses. The deployed
`OfflineRollup` binds the current accepted block-hash commitment, profile, L1
rules and chosen anchor in `getCustomData()`. Parent or whole-group checkpoint
authentication remains required; height equality is insufficient. Actual-parent
mode retains the exact live target; historical mode is ordinary catch-up only.
There is no signer chain RPC, v1 fallback or previous seed-selection path.
The dated audit retains evidence for the earlier implementation; those results
do not validate the replacement profile.

### Wider draft conformance dispositions (#35 section F)

These are explicit limits of this fork's selected profile, tracked by
[issue #35](https://github.com/eduadiez/rollupX/issues/35); implementing V0 does
not claim full draft Rollup0 conformance. Protocol revisions and current validation are recorded in the offline-history audit.

| Requirement | Current contract and disposition |
| --- | --- |
| F1: Appendix D.2 manager/wrapper | The fresh-network `OfflineRollup` binds deployment, profile, rules, accepted commitment and both anchor modes. The draft's additional transaction-scoped wrapper, unsigned-prefix enforcement and one-settlement-per-L1-block restriction are not implied by these bindings. The registry still permits another verified batch to replace a same-block queue; whole-group partial consumption remains supported. |
| F2: Chapter 7 trigger rules | Retain the pinned [registry's](../../eez-core-protocol/src/EEZ.sol) authorized-proxy and verified-this-block gates, exact call/rolling hashes, grouped action boundaries and ordered canonical applied-prefix attribution. This does **not** enforce every draft restriction on who triggers consumption. Broader trigger-policy alignment remains under #35, alongside [#12](https://github.com/eduadiez/rollupX/issues/12) and [#13](https://github.com/eduadiez/rollupX/issues/13); nested/static support is preserved rather than silently narrowed. |
| F3: Validators, parents and delivery | Offline validation checks complete ancestry, delayed source selection, execution and eligible parent/checkpoints, conditional on the manager's on-chain context. Proof-system membership and threshold still govern attestation; no validator committee or production private-delivery guarantee is introduced. |
| F4: Announcements/bootstrap/peers | Existing Engine transport, native block/receipt serialization, authenticated bootstrap and source-based late-follower replay are in scope and tested at the revisions recorded in the audit. General draft unsafe-announcement/gossip and peer-distribution conformance remains separate work under #35. The local devnet result is not a production availability or private-delivery claim. |

No external issue was opened or amended by this task. The local specification
amendments and this table are reviewable dispositions; publishing them or closing
#35 remains a separate repository action.

### Fresh-network boundary

This is a coordinated fresh-network protocol change. Native envelope/storage
types and V0 DA bytes are not compatible with the previous signed-system history.
Mixed composer, follower and signer versions are unsupported. Regenerate genesis
and deploy mutually matching runtime, signer and contract configuration before
starting a new chain; do not reuse old node databases or change a live chain's
genesis. No in-place database migration or historical-format fallback is promised.
L1 posting and proof attestation still require their own keys.

The implementation must document the precise final dependency changes, genesis
commitments, upgrade order and rollback boundary in owning specifications and
runbooks before completion. Reverting binaries cannot restore an incompatible
history, and database restoration does not restore the volatile request queue.

## Validation and status

Run focused codec/primitives/EVM/protocol tests first, then signer, composer,
follower and source-based process regressions. Final validation includes locked
workspace checks, production feature selections, contract/genesis checks and the
applicable replay/Anvil/devnet lanes. Record unavailable lanes as unverified,
never as success. The linked audit distinguishes plans from executed evidence.

No deployment is authorized or performed by this implementation task. Larger
manager/wrapper and operational requirements mentioned in #35 must have explicit
tracked dispositions rather than being silently credited to the codec change.
