# Native transactions and canonical derivation

| Field | Value |
| --- | --- |
| Design status | Implemented for the owner-selected RollupX profile; native-format conflict resolved by the task owner on 2026-09-28. This is not acceptance of the upstream draft specification. Validation and wider-draft limits are recorded in the audit. |
| Date | 2026-09-28 |
| Source baseline | PR #42, `feat/cross-chain-admission-throughput`, `0b7beed5ca48157d720d6685435ba2121c1e9c1e` (advanced from `ca37074` through `dc9f50f` and `c8260ce`; the final base adds bounded consecutive-slot recovery) |
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
sender `0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0076`, and fixed 2,000,000 metered gas.
The issue's `0x45`, absent nonce, and remaining-block-dependent gas requirements
are superseded for this implementation. The remaining issue requirements remain
in scope. The draft at `7c99bcf923e0ee0f867549a750d1ab1373bf845d` is a design
reference, not an accepted replacement for the pinned core protocol.

## Requirements and acceptance

| ID | Required invariant | Acceptance evidence |
| --- | --- | --- |
| N1 | Canonical `0x76 || rlp([chainId, nonce, to, value, input])`, fixed sender and EEZL2 destination; no signature or key | Independent wire vector, round trip, hash, malformed/target tests |
| N2 | Fixed 2M gas, zero fees and nested `GASPRICE`; normal transactions retain fees | Execution, nested guard and reimbursement, paid comparison, noncanonical raw environment tests |
| N3 | Mint committed inbound value before execution; outer revert/halt removes mint while retaining metered gas and nonce behavior | Success, preexisting balance, revert, OOG, overflow, invalid nonce and preview tests |
| N4 | Native receipt identity and storage round trip; native admission unavailable to public RPC/pool; blobs excluded from L2 | Receipt/RPC/storage and production admission regressions; L1 blob simulation retained |
| N5 | Composer, follower, replay and signer agree on bytes and state | Shared reconstruction, consumed-prefix nonce tests, multiple actions/restart, independent process replay |
| T1 | Supplied inbound ABI becomes a checked shared type at the first meaningful boundary and stays checked | Malformed shapes and wrong destination/source rejected without normalization; production-call trace |
| T2 | Shape validation is distinct from settlement authorization | Substituted settlement ABI, call/rolling hash, wrong identity and reconstructed bytes rejected |
| D1 | Direct V0 body, canonical count/metadata runs and transaction length stream over the whole interval including terminal block | #35 exact size vectors and malformed/boundary/fuzz cases; baseline comparison |
| D2 | Bounded parsing of full u32 wire runs, no eager empty-run expansion or artificial 65,536 wire cap | Allocation/resource-limit and canonical integer tests |
| D3 | Deterministic headers, active timestamp step and authenticated live-anchor RANDAO, correct restart/reorg behavior | Header/hash vectors and independent derivation with catch-up, live anchor and rollback |
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
messages. The initiating chain and exact signed origin transaction remain
required for this extension. The shared semantic materializer derives the
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

### Authenticated seed context: implementation constraints

The fixed block profile alone cannot remove transported `prevRandao`. The
signature now binds the latest L1 context, but the signer still needs independent
authentication of an accepted L1 anchor. A Composer-supplied header or block hash
is insufficient to establish the seed. The implementation uses the independent
source described below; final combined replay validation is recorded in the audit.

The extension uses an operator-selected canonical L1 source to bind
accepted root advancement and its containing execution header independently of
Composer. The genesis document must identify the finalized L1 reference and
its chain identity. The signer must also bind attestation to the intended L1
parent and child timestamp: the pinned reference Rollup already supports the
latest-context sentinel and `(timestamp, parentBlockHash)` custom data. Extending
that currently rejected profile requires updated shared public-input helpers,
RPC/specification, posting and independent Solidity vectors together. A timeless
signature is insufficient across an L1 reorg that re-includes the same terminal
L2 hash under a different anchor RANDAO. No trusted-header shortcut or unverified
Composer context is acceptable.

Seed history must record accepted commitment transitions, including partial
consumption at an existing terminal height. Inferring the seed only from the
last L2 header is wrong: accepting a live anchor's `B[0]` refreshes descendants,
then a later accepted `B[1]` at the same L2 height can be a catch-up transition.
That catch-up retains the seed established by `B[0]`, while both terminal
variants themselves still contain the preceding seed. Reorg and restart must
recover this provenance, not just a height or last observed receipt. Candidate
and prefix-cache environments must capture a fixed seed for their actual parent;
a mutable global seed cannot change a draft already being executed.

A bounded implementation can use the existing canonical accepted block-hash
commitment as its parent trust boundary, without downloading historical blobs or
revalidating the whole accepted L2 chain in the signer. An accepted parent's
header already commits its execution seed. The descendant seed differs only if
that height had an accepted live variant: use that containing L1 header's RANDAO
even if the latest variant at the same height was accepted later as catch-up.

To establish that no same-height live variant was omitted, use the fixed
schedule rather than a full historical header proof. A live acceptance at L2
height `n` can occur only in the unique canonical L1 block whose timestamp equals
`timestamp(n)`. The signer independently locates that block at or before the
named target parent and reads its complete relevant settlement occurrences. Each
supplied endpoint header must match an actually applied commitment; the set must
be exhausted exactly. Any accepted endpoint at height `n` in that block refreshes
the descendant seed from that L1 header. Missing slots or a block with no such
acceptance retain the independently accepted parent's committed execution seed.
Later same-height catch-up acceptances cannot erase a live refresh. Reorgs change
the canonical slot and invalidate its former occurrences.

The parent itself still needs independent canonical acceptance (or the configured
registered genesis). The timestamp lookup does not turn a claimed header or a
manual registry root reset into an accepted endpoint. Candidate blocks require
full independent execution. This avoids an additional historical-provenance wire
format while preserving the same-height partial-consumption safety boundary.

The current registry root at the named L1 parent must agree with the candidate's
authenticated parent or an independently reconstructed, already-consumed terminal
prefix. The latest-context public-input binding prevents reusing authorization
after the L1 parent or target child timestamp changes. See the audit for
revision-specific validation and remaining acceptance checks.

Seed lookup is by the block being reconstructed, not a mutable global value.
A live anchor at L2 height `n` affects only blocks greater than `n`. Revalidating
or extending a partially consumed terminal variant at `n` still uses the older
seed even if its `B[0]` was accepted live. Its replay may therefore need provenance
from the preceding live anchor as well as the current one. Existing prefix
selection and nonce offsets remain valid; a current-seed shortcut would break
them. Retained provenance must survive the node's validated checkpoint/restart
path, and reorg handling must discard removed L1 occurrences. A minimal active
proof retains a preceding lower-height accepted endpoint and all subsequent
occurrences; older replay ranges need their own authenticated parent provenance.
Operational header/log budgets and cancellation remain separate from the u32 V0
wire range. The complete one-slot occurrence check remains required even when a
parent was accepted later as catch-up. This fresh-network implementation needs
no old checkpoint migration.

The wider Appendix D.2 manager/wrapper domain and trigger restrictions remain
separate requirements to audit and report. Reusing the pinned latest-context
primitive would not establish full draft manager conformance.

### Wider draft conformance dispositions (#35 section F)

These are explicit limits of this fork's selected profile, tracked by
[issue #35](https://github.com/eduadiez/rollupX/issues/35); implementing V0 does
not claim full draft Rollup0 conformance. The protocol gitlink remains unchanged.

| Requirement | Current contract and disposition |
| --- | --- |
| F1: Appendix D.2 manager/wrapper | Latest-context sentinel and exact `(timestamp, parentBlockHash)` customData are implemented end to end. The pinned [reference manager](../../eez-core-protocol/src/rollupContract/Rollup.sol) supplies those bytes. The draft's additional transaction-scoped wrapper/domain, unsigned-prefix enforcement, pre/post anchor checks and one-settlement-per-L1-block rule are **not implemented**. The pinned registry deliberately permits another verified batch to replace a same-block queue; removing that behavior would conflict with #42's replacement/partial-consumption architecture. A separate manager/profile change must resolve this before claiming Appendix D.2 conformance. |
| F2: Chapter 7 trigger rules | Retain the pinned [registry's](../../eez-core-protocol/src/EEZ.sol) authorized-proxy and verified-this-block gates, exact call/rolling hashes, grouped action boundaries and ordered canonical applied-prefix attribution. This does **not** enforce every draft restriction on who triggers consumption. Broader trigger-policy alignment remains under #35, alongside [#12](https://github.com/eduadiez/rollupX/issues/12) and [#13](https://github.com/eduadiez/rollupX/issues/13); nested/static support is preserved rather than silently narrowed. |
| F3: Validators, parents and delivery | The signer independently authenticates the canonical accepted parent, complete possible live-slot endpoint set, seed, target parent and timestamp. The reference manager enforces configured proof-system membership and threshold; this task does not introduce a validator committee, change owner-controlled threshold governance, or establish production private-delivery guarantees. Those remain deployment/protocol requirements under #35, not consequences of the codec. |
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
