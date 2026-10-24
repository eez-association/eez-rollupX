# Composer-to-prover streaming RPC

Status: one fresh-network offline service, `prove.v2.Prover`.
[prove.proto](proto/prove.proto) defines both methods (`Capabilities` and
`ProveStream`) and every execution-witness, framing and error message.
[SPEC.md](SPEC.md) defines Composer settlement ownership,
retry policy and native settlement validation. Implementation and validation
status are recorded in the [feature audit](../../docs/audits/2026-10-24-fee-vault-rpc/README.md)
and [single-profile amendment](../../docs/audits/2026-10-24-profile-simplification/README.md).

The contract separates three things: required streaming behavior, optional
session controls, and settlement inputs. A prover may support blob settlement
without supporting resumable sessions, or resumable sessions without blobs.
Neither requires shared caches or a particular execution backend.

## 1. Required interface

Every implementation supports `ProveStream` with this baseline flow:

```text
Begin(anchor) -> Ready(anchor)
Block(anchor + 1) -> Validated
Block(anchor + 2) -> Validated
...
Finalize(anchor + 1 .. terminal, settlement) -> Proof -> EOF
```

A baseline prover needs only one stream-local prefix and sequential execution.
It MAY limit capacity to one active stream/session, keep no global block cache,
and discard the prefix when the stream ends. Its epoch can be the constant 1.
It need not implement subrange finalization, Resume, Rewind, application-level
Cancel, durable recovery, parallel execution, deduplication or cross-session sharing.

At the common transport layer, optional session controls are attempted directly.
RollupX additionally requires the profile/identity preflight in section 6. Unsupported operations, including a
frame with no recognized operation, return a correlated Rejected with
UNIMPLEMENTED. A rejection must not change progress or end a bound stream.
Unsupported Resume on an unbound stream returns an unbound rejection.

Unsupported blobs or posting context also return UNIMPLEMENTED; they must never
be silently discarded. A client may fall back to the baseline
session flow, but must not remove proof-relevant inputs to obtain a proof.

Server capacity, retention, deadlines, chain/rollup rules, ABI and proof system
are deployment parameters. RollupX advertises its effective limits and validation
identity through the capability query; clients must agree before opening a stream. A server must not
reuse retained validation under different rules.

### Baseline client behavior

Keep the stream open from Begin through Finalize and wait for each Block ACK.
On disconnect, open a fresh stream, Begin at the selected execution predecessor and
backfill from durable block/witness storage. On a fork change, cancel the RPC,
Begin again and resend the selected branch. After Proof, start a new stream
for the next range. Clients using optional controls fall back to this flow on
UNIMPLEMENTED: Begin and backfill after unsupported Resume, replace the stream
after unsupported Rewind, and use transport cancellation after unsupported Cancel.

Streaming remains useful without these optimizations: ordinary block execution
happens before finalization. It does not guarantee a fixed proof latency.
Prevalidation and backpressure MUST NOT block L2 commitment or durable witness
capture. Bound the final proof attempt including any reconnect/backfill and
leave time for proof verification and L1 submission.

## 2. Frames and required validation

A ClientFrame contains exactly one operation and an unambiguous outstanding
request_id. Every response echoes that ID. Begin is the first frame on an
unbound stream, with an empty session_id and epoch zero. Its Ready returns a
new opaque 32-byte session ID, a nonzero epoch and the exact anchor cursor.
Thereafter frames carry that stream's binding; the baseline simply echoes the
same epoch throughout. A session ID is not a block/proof identity or an
authentication credential. Shared deployments must authenticate and isolate
Composers independently of these fields.

A response without an authorized binding has empty session_id, zero epoch,
zero validated_through and empty validated_hash. Otherwise Ready/Rejected
report the authoritative fully validated cursor and its 32-byte hash, not
merely the last received block. Clients correlate ID, epoch, request ID and
response kind before changing local progress. Ready and Validated are not
attestations.

### Begin and Block

Begin supplies the nonzero rollup registry ID and exact anchor number/hash/
state root. Both hashes are 32 bytes; anchor_number must permit adding one.
The anchor is an execution predecessor, not a claim of L1 acceptance. Admission
checks the configured rollup and the profile's execution-parent rules. For
RollupX, Finalize separately authenticates the selected predecessor; a current
registry root or a caller-supplied state root alone cannot authorize it.

Each Block contains one present BlockWitness: declared number, 32-byte hash
and parent hash, exact consensus header/body RLP, and a present execution
witness sufficient for the configured validation rules. Witness acquisition
is backend-independent at the wire boundary; witnesses cannot be omitted.

New blocks must extend the validated prefix by one with exact hash and
execution-state continuity. Bind wire claims to decoded consensus bytes and
complete execution/evidence validation before emitting Validated. Its number,
hash and 32-byte post_state_root identify that block. Ordinary blocks require
ordinary execution; settlement checkpoints belong to blocks that need them.
These ACKs establish execution facts, not L1 authorization or correctness for
an as-yet-unknown posting target. Finalize checks target-dependent constraints
against those facts, including when the execution came from a cache.

An exact duplicate of an already validated block is idempotent. A contradictory
block or gap is rejected without advancing or replacing the prefix. Rejected
reports the current cursor so the client can supply missing blocks on the same
stream. No cache is necessary to implement this rule. `reused` is diagnostic;
a conforming implementation may always return false.

### Finalize and completion

Finalize selects a nonempty contiguous inclusive [from_block, to_block] range
and its 32-byte terminal_hash. Every selected block and its predecessor must
belong to the validated prefix. The hash must match the selected terminal.

The baseline starts immediately after the session anchor, initially
`from_block == Begin.anchor_number + 1`. That anchor is its settlement parent.
The terminal may be any validated block, including one before the tip.

Starting at a later from_block is optional subrange finalization. Its predecessor
must already be a validated block in the session. A prover that does not support
this selection returns a correlated Rejected with UNIMPLEMENTED, leaving the
anchor, epoch and validated prefix unchanged. The client can open a new stream,
Begin at the desired predecessor and backfill the same requested range; it must
not silently widen the range or change the settlement input.

The first selected block's witness supplies its exact L2 parent header; a
subrange predecessor also has retained execution evidence. RollupX checks its
number/hash/state root against Begin or the retained predecessor, then binds
the accepted L2 commitment to that predecessor or a fully replayed whole-group
terminal checkpoint. The manager supplies the actual accepted commitment when
settlement verifies the proof. Neither a caller-supplied root nor a Validated
ACK supplies this authority. Missing or inconsistent request evidence is a
validation failure; the prover does not fetch it from another source.

Finalize binds the complete settlement input in section 3 to that exact range.
Only successful execution, settlement, DA, proof-system and public-input checks
may yield Proof. A failed finalization yields no proof and does not consume
validated progress. The client independently verifies the returned public-input
hash and proof against the exact request and configured verifier.

Proof closes the stream successfully. Without Resume support the session ends
there; with Resume support it retains the selected range and any later validated
suffix, promotes the selected predecessor to anchor and may release earlier data.
After resuming, the promoted anchor is the baseline parent for that session;
Resume support alone does not require support for selecting a later parent.
Proof never advances the Composer's L1 settlement cursor by itself.

Client half-close drains accepted work; EOF without a Proof is not success.
Transport cancellation stops replies and releases stream-local work. Only
Resume support grants retention after stream teardown. No proof may be used
after the Composer abandons its corresponding batch or settlement cutoff.

## 3. Offline evidence and settlement input

Every Finalize supplies the exact proofless `EEZ.postAndVerifyBatch` calldata
and complete physical blobs. There is no authoritative caller-supplied public
input, custom data, KZG commitment, accepted-checkpoint list or randomness claim.
The prover derives each from checked configuration, witnesses and execution.

### L1 witness extension

`ExecutionWitness.l1_headers_rlp`, field 5, contains canonical raw L1 execution
headers, oldest first. Fields 1–4 keep their meanings; `headers`, field 4, remains
L2 `BLOCKHASH` ancestry. L1 headers cannot be smuggled into that field.

The first Block starts at the authenticated previous L2 history endpoint.
Later nonempty segments append consecutive produced headers to the same session
chain; repeated sources need no repeated header. The Composer may remove an
identical shared endpoint when joining its durable per-block witnesses. A
conflicting endpoint, missing intermediate block, wrong parent link, duplicate
number, non-increasing timestamp, noncanonical RLP or wrong fork layout rejects.
Every produced intermediate header is required, including headers used only to
bridge to the proof anchor. Skipped L1 slots have no invented headers.

Block execution checks history storage/code, schedule and the selected source.
It exact-validates the history transaction in `Block.rlp`, including repeat,
position, version, sourceHash, zero value, exact gas and canonical calldata, and uses the selected
header's `prevRandao` for both L2 header and EVM. Finalize authenticates every
recorded selection against the complete bridge and exact anchor. No latest-source
or arrival-time inference can replace the transaction's choice. The terminal Block's witness carries any
remaining bridge; Finalize does not introduce an unchecked evidence channel.
Each ACK checkpoints the exact evidence prefix as well as L2 execution. Rewind
restores both. A duplicate is idempotent only when the whole Block/witness is
identical. To change a terminal bridge, rewind before that Block and resubmit.

Session loss or restart requires complete backfill from the selected predecessor
through the chosen anchor. A retained prefix is an optimization, never a required
source of truth. Large L2 ranges may be split into separate proofs, but each part
still needs its complete L1 bridge.

### Physical blobs and public reconstruction

`blobs` is the full transaction blob array in transaction order. Each blob is
131072 bytes of canonical field elements. The prover derives KZG commitments and
versioned hashes independently, checks native stream framing and padding, and
requires empty `batch.callData` and indices `[0, 1, ..., blobs.len() - 1]`.
Extra, omitted, duplicated or reordered blobs reject. The physical format is the
[pinned Core blob specification](../../eez-core-protocol/docs/blobs/BLOB_FORMAT_SPEC.md);
RollupX's native payload codec binds the ordinary transactions and whole nested
cross-chain action groups to the selected execution. Hashing blobs alone is not
sufficient validation.

L1 imports, hash lists and randomness records are derived from public canonical
L1 headers and the authenticated predecessor. They are not duplicated in blobs.
Followers use the same genesis profile, schedule, anchor rules and execution
hooks. The settlement blob is not required to contain the prover's full witness.

### Two anchor modes

`batch.blockNumber` selects the mode. Zero is reserved and rejects.

| Mode | Selector and evidence endpoint | `posting_context` | Execution eligibility |
| --- | --- | --- | --- |
| Actual parent | `uint64.max`; chain ends at the named actual inclusion parent | Required; unchanged chain ID, child timestamp, parent number/hash meanings | Ordinary catch-up may end before the target; any cross-chain action requires the exact matching live target |
| Historical | Explicit B; chain ends exactly at B | Must be absent | Ordinary catch-up only; terminal time <= timestamp(B), and timestamp(B) exceeds every relevant source cutoff |

The manager checks actual-parent number/hash and exact inclusion timestamp.
For historical mode it requires `B < block.number`, age at most 256 produced
blocks, and nonzero `blockhash(B)`. The prover checks header ancestry and time
eligibility; it cannot predict future inclusion or anchor expiry. The actual
manager also binds the active L1 rule period and current accepted L2 block-hash
commitment. A proof becomes unusable if those differ, even before B expires.

Historical mode rejects every cross-chain action, including static, failed,
nested and reverted observations with no L2 transaction. It is never an automatic
fallback for live work. Its proof anchor is separate from the submission attempt's
actual L1 parent, deadline, queue ownership and replacement accounting.

### Context and authority

The exact flat ABI context is defined by
[`OfflineContext`](../eez-protocol/src/l1_history/context.rs) and
[`OfflineRollup.getCustomData`](../../contracts/src/OfflineRollup.sol):

```text
common = domain, L1 chain ID, registry, manager, rollup ID, L2 chain ID,
         fixed profile hash, active L1 rules ID, current accepted L2 block hash
actual = abi.encode(common..., uint8(1), parent number, parent hash, inclusion time)
history = abi.encode(common..., uint8(2), B, block hash at B)
```

Addresses/profile are independently loaded locally. The requested root pin must
identify exactly this rollup and bind to the selected predecessor or a fully
replayed whole-group terminal checkpoint. Height equality and receipts do not
establish consumption. The manager reads the actual registry commitment before
queue mutations and incorporates it into the verified public input.
A changed accepted root requires a new Proof, even if execution and DA are
unchanged. Fully replayed checkpoint authorization permits same-block
resumption; it does not make the original root-bound proof reusable.

The prover's result is conditional on this exact context. The contract binds it
to actual L1 facts at inclusion. This permits proof validation without an L1 or
L2 RPC source and without assuming the Composer is honest. It does not turn the
ECDSA proof backend into a zero-knowledge proof: settlement still trusts the
configured attester to perform these validation gates correctly.

The configured canonical L1 profile supplies fork-aware header decoding and
blob limits. Actual-parent mode evaluates the target rule period; historical
mode evaluates B's period and the manager requires the same period at inclusion.
A rule transition requires new evidence/finalization, not a relaxed limit.

Absent settlement, malformed encodings or missing required mode inputs reject.
Unsupported proof-relevant inputs cannot be stripped on retry. Retaining the
v2 service name does not make old implementations compatible: mandatory offline
profile/identity preflight rejects peers that do not implement witness field 5
and these proof rules.

## 4. Optional advanced session controls

Resume retains progress subject to published limits. It is the first frame
on a new stream, with the retained ID and epoch zero. Ready returns the same
ID, a fresh nonzero epoch and the retained cursor. This atomically fences the old
attachment; copying the new epoch on an old stream cannot regain authorization.
Missing/evicted sessions return NOT_FOUND, requiring Begin and backfill.
A lost ACK can mean more progress was validated
than the client saw; reconcile both cursor number and hash.

Rewind accepts only the anchor or a fully validated ancestor in this session's
active prefix, not a block merely cached elsewhere. It truncates the suffix,
fences pending old-prefix work, advances the epoch and returns Ready at the
ancestor. It cannot replace the anchor. A rejected rewind changes neither
epoch nor prefix. Without Rewind support, replace the stream and backfill instead.

Cancel atomically retires the bound session, fences pending work, returns the
correlated Cancelled at the accepted epoch and closes the stream. No later ACK
or Proof follows. Without Cancel support, use transport cancellation; if Resume
is supported, retained data remains subject to normal eviction. Cancellation never
authorizes deleting another session's data.

Clients may pipeline dependent Block/Finalize frames in order, bounding
outstanding data and draining responses while sending. Servers may read and
execute sequentially, apply backpressure, or reject excess work with
RESOURCE_EXHAUSTED; no unbounded queue or parallel execution is required.
Accepted Resume/Rewind/Cancel fences earlier work before its control response;
stale work cannot mutate the active prefix or emit a usable Proof afterward. Results superseded
by such controls need not receive individual replies. A rejected gap cannot
be skipped by later queued blocks.

Shared caches, singleflight, multiple sessions and parallel execution are
implementation choices, not protocol requirements. If used, bind reused
evidence to exact consensus bytes, execution parent and validation rules.
Isolate session progress and results; cancellation of one waiter must not
cancel work still needed by another. Nothing in this protocol mandates Reth
forkchoice, a database layout, or a second execution store.

## 5. Errors, limits and conformance

Rejected.code is the standard numeric gRPC status code; message is diagnostic,
not machine-readable control flow. details may contain the existing ProveFailure
encoding when an actionable settlement candidate is known. Clients must bind
such details to the exact request's index/hash before changing candidates.
RPC transport errors and operation rejections never constitute proofs.

UNAVAILABLE, DEADLINE_EXCEEDED and ABORTED may permit bounded retries after
reconciliation. NOT_FOUND permits session-loss recovery. UNIMPLEMENTED means
the operation or input is unsupported: use the baseline only if it still expresses
the same complete proving request, otherwise select a suitable prover. Unsupported
settlement data must never be converted into a weaker request. Capacity
rejections use RESOURCE_EXHAUSTED; malformed input is not retried unchanged.

Publish finite message/queue/session/byte/witness limits and bound connect,
send, receive and operation deadlines. The minimal server can reject a second
session at capacity without disturbing the first. No 500 ms or other fixed
deadline is guaranteed by the wire contract.

Conformance should cover baseline and optional exchanges separately. Codec
vectors pin field numbers and request/response encoding; they are not substitutes
for executed-block and on-chain proof oracles. Recorded live request/response
transcripts belong with each runtime implementation. Keep vector fixtures and
their test harness separate from this protocol proposal.

## 6. RollupX runtime profile

This profile is active from a new network's genesis. The signer serves only v2
proving plus the shared capability query. Startup uses local L2 genesis, local
L1 chain rules, registry/manager identities and proof policy. It has no L1/L2
provider, provenance lookup, prepared-range cache or lease API.
Obsolete signer RPC settings reject, including explicitly empty environment
values. Composer, followers and transaction senders retain their own chain
connections.

Capabilities must advertise offline profile 2, operations tag 0 and an exact
matching validation identity. Validation identity binds the fixed offline profile,
L1/L2 rules, runtime, registry/manager, rollup and the validation-contract
version. The exact language-neutral encoding is defined in
[the identity contract](VALIDATION_IDENTITY.md); build dependencies are not wire inputs.
The signer separately checks its configured proof-system policy during settlement;
the client checks the returned digest and configured attester's signature.
Every ProveStream call must carry matching `eez-validation-identity-bin` metadata. Missing
or mismatching metadata rejects before frames are admitted. Identity agreement
establishes compatible validation rules; it never authorizes a signature.

The Rust signer implements Resume, Rewind, Cancel and subrange Finalize, including a
terminal before the tip. Success promotes the selected predecessor and prunes
older L2 data while retaining evidence needed by the remaining prefix. Invalid
ranges, gaps, conflicting duplicates or unretained ancestors reject without
advancing progress. Unauthorized/retired attachment epochs return
PERMISSION_DENIED without disclosing a cursor.

| Resource | Bound and owner |
| --- | --- |
| Active streams / retained sessions | 8 / 8 per daemon; evict only detached sessions, oldest first |
| Session blocks, bytes, witness items | Configured `MAX_REQUEST_*`; defaults 65,536 / 512 MiB / 1,000,000; L1 headers count as witness items and bytes |
| L1 evidence | At most 131,072 headers per session, further limited by its witness-item quota; each raw header is at most 2,048 bytes |
| Incoming message | Configured decode ceiling; default 256 MiB |
| Pending frames | At most block limit + 1, twice the byte limit and one witness-item limit per stream |
| Response channel / client pipeline | 8 frames / at most 8 outstanding operations, draining both directions |
| Expensive work | One shared validation worker; finalizations acquire the shared final-proof permit |
| Deadlines | Configured absolute timeout for each ordered Block/Finalize, including worker admission; idle timeout bounds input silence and output backpressure |

Blocking workers retain permits until they actually exit, even after an RPC
expires. Cancellation and attachment epochs fence publication. Half-close drains
accepted work; EOF without Proof is failure. Duplicate outstanding request IDs
terminate the ambiguous transport. A successful Finalize returns one Proof and
EOF. Sessions are volatile; restart requires full witness backfill.

Composer's existing bounded preparation worker prevalidates v2 blocks in chunks
of at most 64 and releases the exchange lock between chunks. Failure defers this
optional work without blocking commitment or durable witness capture. Complete
proving backfills missing blocks and final evidence. Each exchange opens a new
transport and attempts Resume. Missing sessions or unsupported optional controls
cause Begin/backfill of the same complete request.
The client remembers the oldest retained predecessor from Begin and successful
subrange proofs. If a requested predecessor predates that point or has another
hash at the same height, it opens a complete new session before Finalize. This
also covers background prevalidation starting after a still-unsettled proof;
neither the session anchor nor a proof establishes canonical settlement.
One bounded fresh-Begin repair handles retained quota or pruned-prefix rejection;
actionable settlement failures cannot be converted into that repair.

The client bounds the whole exchange, including lock acquisition, transport,
backfill, all frames and EOF. Composer's original settlement cutoff can cancel it
earlier and remains authoritative. Session recovery and prevalidation do not
individually charge a settlement attempt. The closed retry list and canonical
ownership/replacement rules in [SPEC.md](SPEC.md#6-failure-handling) still apply.
Clients verify response correlation, ACK number/hash/state root, full native
public inputs and registered-attester signature before using a proof.

Fresh genesis, contracts and datadirs are required. There is no old/new activation
height, database migration or mixed-profile operation. Reverting to the prior
execution rules cannot replay this network. Implementation and current test
coverage, including unresolved checks, belong in the
[feature audit](../../docs/audits/2026-10-22-l1-history-offline-proving/README.md).
