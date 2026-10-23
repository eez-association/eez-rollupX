# EEZ Composer-to-prover gRPC specification

Status: fork Composer contract for fresh networks using permanent L1 history,
delayed synchronized PREVRANDAO and offline v2 proving. Implementation/test status
is recorded in the [feature audit](../../docs/audits/2026-10-22-l1-history-offline-proving/README.md).
The current `eez-core-protocol` gitlink and this fork's `OfflineRollup` define the
accepted on-chain ABI; upstream main is not a substitute.

This profile preserves native blobs, block-hash commitments, whole grouped calls
and position-bound L2 static entries. Composer preserves each static
`expectedEntryIndex` in its load and semantic DA; the signer independently binds
it to ordered execution. All rules are active from genesis. Prior seed rules,
v1 proving, preparation leases and provider-backed signer authorization are retired.

[prove_stream.proto](proto/prove_stream.proto) and the shared messages in
[prove.proto](proto/prove.proto) are the canonical schemas. The
[streaming specification](STREAMING_SPEC.md) defines framing, session recovery and
offline evidence. This document owns native settlement, response validation,
retry cutoff, failure attribution and canonical queue ownership. The
[signer specification](../eez-proof-signer/SPEC.md) defines its validation gates.

## 1. Composer flow

For each admitted, Sync-aligned settlement range, Composer:

1. Selects the execution predecessor and exact L1 proof anchor/mode, keeping the
   submission attempt's operational parent/deadline separate for historical mode.
2. Collects exact L2 block RLP, augmented execution witnesses and the complete
   linked L1 header evidence through the anchor, including bridge-only headers.
3. Builds one canonical native batch and physical blobs with `batch.proofs` empty.
4. Uses `Begin -> Block* -> Finalize` over `prove.v2.Prover.ProveStream`.
5. Checks response correlation, exact public-input hash and registered-attester
   signature, then fills only `batch.proofs`.
6. Rechecks the actual submission parent, limits and exclusive cutoff, signs the
   L1 transaction with the poster key and submits the same proved settlement.
7. Retains ownership until canonical reconciliation proves consumption or gives
   an explicit disposition; a proof, receipt or relay acknowledgement is insufficient.

Missing proof/evidence never authorizes an unproved submission. The
[failure procedure](#63-retry-procedure-and-settlement-cutoff) retains work when
the attempt cannot finish. The prover receives no poster key and submits no
transactions. It has no L1 or L2 RPC connection.

## 2. Service definition

`prove.v2.Prover.ProveStream` is the only proving endpoint. The
`prove.v1.Prover.Capabilities` query remains a shared discovery endpoint; its
namespace and shared witness messages do not enable v1 proving. Generate clients
from these schemas, not a second codec.

### 2.1 Validation identity and capability discovery

Capabilities returns operations tag 0, offline profile 2, the 32-byte validation
identity, effective block/byte/witness-item/message limits and timeouts. The call
neither occupies the expensive worker nor signs. Identity schema 6 binds actual
loaded L1/L2 configuration, fixed genesis/history profile, registry/manager,
rollup and validation specification version. Proof-system/vkey policy is checked
separately during settlement and response verification. Duplicate JSON keys reject before rules are loaded;
EIP-155 chain ID alone does not identify a network or its rules. Resource limits
are separate from the validation identity. The normative
[portable identity encoding](VALIDATION_IDENTITY.md) and
[external prover guide](conformance/IMPLEMENTING.md) define the non-Rust boundary.

Composer derives its expected identity locally, requires exact agreement before
node launch and intersects the reported block ceiling with local emission
settings. A ceiling below one Sync interval fails startup. Each v2 stream carries
matching `eez-validation-identity-bin` metadata; missing, malformed or mismatching
identity rejects before witnesses. Configuration-change responses cannot authorize
candidate eviction, even if they contain extraneous failure details.

Clients share a capability snapshot across successful requests. UNAVAILABLE
while opening ProveStream invalidates that connection; a stale in-flight failure
cannot discard a newer connection. Rediscovery must agree with the initially
accepted snapshot. Changed rules or limits require a producer restart and
replanning; a resource/configuration rejection does not silently adopt new limits
or authorize reuse under changed rules.

Optional prevalidation sends bounded v2 Block chunks and receives validated ACKs.
They are execution facts, not signatures or canonical progress. Finalize always
checks the exact selected range, full bridge, anchor mode, actions, accepted root,
DA and public inputs. Resume/Rewind retain exact execution and L1 evidence
checkpoints under the same identity. On session loss, a cold prover receives the
complete request from durable Composer witnesses. No prepared-range or lease API
is required. See the [resource and lifecycle rules](STREAMING_SPEC.md#6-rollupx-runtime-profile).

## 3. Constructing the request

### 3.1 Settlement batch and native DA

The request carries the exact batch and physical blobs that the Composer intends
to submit to L1, with only the proof bytes omitted. The pinned Solidity ABI,
mirrored in [`eez-protocol/src/abi.rs`](../eez-protocol/src/abi.rs), determines
field order and integer widths.

| Batch field | Composer requirement |
| --- | --- |
| `expectedRootPerRollup` | Exactly one nonzero accepted block-hash pin for the configured rollup. |
| `entries` | One anchor, then outbound hosts, then inbound effect or lookup entries. |
| `staticEntries` | Supported L1-origin static entries pinned at their exact FIFO positions. |
| `immediateEntryCount` | Complete leading run with `proxyEntryHash == 0`. |
| `immediateStaticEntryCount` | Zero. |
| `proofSystems` | Exactly the configured `ECDSAProofSystem` address. |
| `rollupIdsWithProofSystems` | Exactly the expected rollup ID assigned to index `0`. |
| `blobIndices` | `[0, 1, ..., blob_count - 1]`, selecting all supplied blobs in order. |
| `callData` | Empty; DA is transported in native blobs. |
| `proofs` | Empty in the proving request. |
| `blockNumber` | `uint64.max` for actual-parent mode or explicit nonzero historical B. |
| `bindMsgSenderInPublicInput` | `false`. |

Every mutable entry MUST contain exactly one `rootUpdates` item for the expected
rollup. The updates form one continuous commitment chain:

```text
entries[0].rootUpdates[0].currentRoot = block hash at posted
entries[i].rootUpdates[0].currentRoot = entries[i - 1].rootUpdates[0].newRoot
entries[last].rootUpdates[0].newRoot = terminal Sync block hash
```

Despite the historical ABI names, these roots are **block hashes**. Interior
checkpoints are hashes of canonical terminal-block prefixes, with transaction
root, receipt root, bloom, gas used, state root, and applicable blob gas recomputed
for that prefix. Execution state roots alone MUST NOT be used as settlement
commitments. An outbound `[system load, user]` pair ends at the user transaction;
an inbound effect ends at its system delivery transaction. The exact prefix
header profile, persistent commitment carriers for failed calls, and static pins
are specified in [signer sections 6 and 8](../eez-proof-signer/SPEC.md#6-execution-validation).

An anchor-only batch covers the whole window. Grouped, nested, reverted, and
static calls MUST use the fork's production table and symbolic trace finalizers.
One source transaction can create multiple outbound hosts and callback entries;
inbound lookup-only and static calls create no L2 delivery. Do not infer a
one-to-one relationship between user transactions and batch entries. Finalize
root-dependent rolling hashes only after commitment assignment. The exact entry
shapes and effect binding are defined in
[signer section 8](../eez-proof-signer/SPEC.md#8-block-hash-commitment-chain-and-effect-binding).

`Finalize.settlement.posting_context` is required only in actual-parent mode.
Its L1 chain ID, exact child timestamp, parent number and nonzero parent hash
retain their current meanings. The complete witness chain must end at that
parent. `batch.blockNumber == uint64.max` binds the actual inclusion parent and
timestamp through `OfflineRollup.getCustomData`. Live cross-chain work must end
at the matching target; late inclusion rejects.

Historical mode uses explicit B, excluding zero and `uint64.max`, and requires
`posting_context` to be absent. Evidence ends at B. Only ordinary catch-up is
allowed: terminal time <= timestamp(B), and timestamp(B) must exceed every source
cutoff. Static, reverted, failed and nested observations still count as actions.
The manager requires B to be earlier than inclusion, no more than 256 produced
blocks old and available through nonzero `blockhash(B)`.

Both contexts bind the domain, network/deployment, fixed genesis/profile, active
L1 rule period and actual current accepted L2 block hash. The pin must be the
replayed predecessor or a permitted fully replayed whole-group terminal checkpoint.
The prover proves consistency with these inputs; the manager authenticates their
actual L1 values at inclusion. The [exact context contract](STREAMING_SPEC.md#context-and-authority)
contains the encoding. A historical proof remains usable only while its anchor,
root, profile, complete settlement and rule period still match. Never reinterpret
a live attempt as historical mode after failure.
Composer checks the intended inclusion's rule period before choosing historical
mode. If the next inclusion crosses a known transition, it selects actual-parent
mode and proves under the new target rules from the outset.

`Finalize.settlement.blobs` MUST contain complete 131,072-byte EIP-4844 blobs in transaction
order, from one up to that target's per-transaction and per-block allowance. Their version-`00` EEZ message stream contains exactly one
leading `ChainOperation(rollup_id, payload)`, complete native semantic transaction
brackets, and a final `CloseBlobStream`, with canonical field-element packing
and zero padding. New chain-operation payloads are:

```text
0x00 || uvarint32(block_count) || count_tokens || beneficiary_runs
     || extraData_runs || transaction_lengths || exact_ordinary_transaction_bytes
```

The new network uses one format from genesis without an activation setting.
The columns cover all blocks, including terminal metadata and its ordinary
prefix. Timestamps, fixed header fields and randomness are independently derived;
there are no full-block escapes, environment overrides, sidecar arrays or group
sizes in operations. See the [direct V0 contract](../../docs/proposals/native-system-transactions.md#direct-v0-wire-contract)
for exact bounds, maximal runs and canonical decoding.

The surrounding native semantic stream reconstructs the authorized mutable and
static tables, whole outbound groups, lookup results and reverted source frames.
Its supplied values are checked and bound to independently executed transactions;
shape validation alone never authorizes a settlement or native transaction.
The complete native carrier, operations reconstruction, and terminal
transaction reconstruction rules are in the
[DA profile](../eez-proof-signer/SPEC.md#11-data-availability-and-sync-block-verification).
Calldata-only DA is not accepted by this deployment.

The Composer MUST exact-encode the complete `postAndVerifyBatch` call with
selector `0xe4a480e4` into `Finalize.settlement.abi_calldata`. After proof validation, it
MUST submit the same physical blobs in a type-3 transaction with the network's
configured propagation encoding (EIP-4844 or EIP-7594).

### 3.2 Range and predecessor

Begin identifies the nonzero registry rollup ID and exact execution predecessor
number/hash/state root. Finalize selects a nonempty contiguous range and its
terminal hash; the endpoint must be Sync-aligned. Every selected block and its
predecessor must belong to the validated session prefix. A partial-consumption
candidate may retain its original execution predecessor while binding the
manager's accepted root to a fully replayed whole-group terminal checkpoint.
Neither a height nor a claimed list of checkpoint hashes is sufficient.
Advancing the accepted root invalidates the original proof. Re-proving with a
validated checkpoint pin may retain the original execution and DA, including
for same-block resumption under the same actual-parent/time binding. The
checkpoint proof is conditional until the manager observes that exact root.

An attempt may span multiple intervals and contain prior empty Sync blocks.
It must replay all those blocks and their original schedule. Partition catch-up
into admitted Sync-aligned ranges; never split an action group or defer a required
system import across arbitrary blocks. Advancement between parts requires
canonical consumption, not proof generation or a successful receipt.

### 3.3 Block witnesses

Each Block supplies exact number/hash/parent hash, consensus RLP with no trailing
bytes and a present augmented execution witness. New blocks extend the prefix by
one with exact hash/state continuity. The witness fields are:

- `state`: account/state-trie nodes and per-account storage-trie removal closures
  needed for selected intermediate roots;
- `codes`: bytecodes;
- `keys`: hashed-key preimages;
- `headers`: unchanged L2 ancestry and the exact execution parent header;
- `l1_headers_rlp`: separate canonical raw L1 execution headers extending the
  session's complete ancestry chain.

The account and state trie are the same global trie; storage tries are separate.
Witnesses must support deletions hidden by the final endpoint state. L1 evidence
must also support inherited selections and all intermediate produced headers,
including headers never selected as origin. Finalize checks maximality and exact
anchor termination. Missing evidence is a rejection, never permission to fetch
it from an RPC source or assume an older source.

### 3.4 Stream completion and recovery

Follow the [v2 framing contract](STREAMING_SPEC.md#2-frames-and-required-validation).
Exact duplicate blocks/witnesses are idempotent; gaps and conflicting duplicates
reject without progress. Validated ACKs never authorize signing. Finalize supplies
settlement and receives a correlated Proof followed by EOF. EOF alone is failure.
Resume reconciles number and hash; Rewind restores both execution and L1 evidence.
Unknown sessions require full Begin/backfill. Every recovery shares the original
settlement deadline and exact proof inputs.

### 3.5 Prover acceptance checks

The Composer should expect the prover to return a proof only when all of the
following checks pass:

- the stream has the required v2 frame sequence, selected range, block order,
  rollup identity, field widths, and resource bounds;
- every block exact-decodes, matches its submitted number, hash, parent hash,
  and configured chain rules, and re-executes to its committed state root;
- L1 evidence, parent state, history imports, delayed origin/PREVRANDAO, anchor
  mode and accepted-root binding all agree;
- the blocks form one state-continuous window and contain settlement activity
  only in the terminal Sync block;
- `Finalize.settlement.abi_calldata` is the canonical encoding of the supported batch
  profile and matches the configured rollup and proof system;
- the entry block-hash updates form the required chain, including validated
  prefix commitments, persistent carriers, lookup-only entries, and static pins;
- every inbound and outbound entry matches the applicable executed transaction,
  receipt, event, call hash, value, and ether-delta evidence for that effect;
- the native blob stream exactly describes the validated block transactions,
  semantic call trees, and effect sidecars, including byte-exact reconstruction
  of terminal system transactions; and
- the prover can independently recompute the batch's sole `publicInputsHash`.

Any failed check returns no proof. The complete accepted profile and rejection
rules are defined in the
[`eez-proof-signer` specification](../eez-proof-signer/SPEC.md#5-stream-admission).

## 4. Transport behavior

The signer defaults to 65,536 blocks, 512 MiB of known protobuf fields and
1,000,000 witness items per session; L1 evidence consumes both byte and item
quotas. The decode ceiling is at most 256 MiB and never exceeds the aggregate
byte cap. Default idle/ordered-operation timeouts are 120/600 seconds. The
[streaming runtime contract](STREAMING_SPEC.md#6-rollupx-runtime-profile) defines
finite queues, attachment fencing, one expensive worker and permit ownership.

Witnesses may exceed gRPC's usual 4 MiB default. Clients must set finite encoding
and decoding limits consistent with the negotiated profile, enforce aggregate
admission and handle RESOURCE_EXHAUSTED explicitly. Splitting an L2 range does
not remove the L1 bridge each part must prove.

Bound the entire exchange, including lock acquisition, transport, backfill and
response verification. Composer's exclusive submission cutoff may cancel sooner.
A timeout does not free a running blocking worker's permit; it retains ownership
until exit. Reconnect/recovery never resets the settlement deadline.

## 5. Validating and using the response

The Composer MUST reject the response unless:

- `public_inputs_hash` is exactly 32 bytes;
- `Proof.proof` is exactly 65 bytes encoded as `r[32] || s[32] || v[1]`;
- the signature is valid secp256k1 ECDSA over the raw 32-byte
  `public_inputs_hash`, without an EIP-191 prefix or EIP-712 domain;
- `s` is canonical low-`s` and `v` is `27` or `28`; and
- recovering the signature over `public_inputs_hash` yields the attester
  registered for the configured `ECDSAProofSystem`.

The Composer MUST also recompute the current profile's sole
`publicInputsHash` from the exact request batch, registered vkey, and KZG versioned hashes
independently derived from the physical request blobs, then require
it to equal `response.public_inputs_hash`. This includes the exact offline context,
accepted root and selected anchor mode, not just the batch/DA. The normative hash construction is
in [proof-signer section 12](../eez-proof-signer/SPEC.md#12-public-input-recomputation),
and cross-language vectors are in
[`eez-protocol/tests/fixtures`](../eez-protocol/tests/fixtures/README.md). This
preflight avoids submitting a response that recovered to the correct attester
but was signed over the wrong batch hash.

After validation, the Composer MUST change only the proof carrier:

```text
assert batch.proofSystems == [ecdsa_proof_system_address]
assert batch.rollupIdsWithProofSystems == [{ rollupId, proofSystemIndexes: [0] }]
batch.proofs = [response.proof]
```

The Composer MUST NOT modify entries, root updates, rolling hashes,
physical blobs, blob order, `callData`, proof-system assignments, scheduling counts, or any other proved
batch field after the request. `response.public_inputs_hash` is not inserted
into the batch; EEZ recomputes it on-chain.

Finally, the Composer ABI-encodes `EEZ.postAndVerifyBatch(batch)`, signs the L1
transaction with its poster key, and sends it to the deployment's EEZ contract.
The poster key and proof attester are separate authorities. EEZ recomputes the
public-input hash and calls
`ECDSAProofSystem.verify(response.proof, publicInputsHash)`.

## 6. Failure handling

No gRPC error yields a usable proof. Composer implementations SHOULD handle the
canonical status codes as follows:

### 6.1 Status classification

Only the following statuses are retryable:

| Code | Composer action |
| --- | --- |
| `UNAVAILABLE` | The prover or transport is temporarily unavailable, including a prover that is busy or has not yet reached the required state. Retry the complete request. |
| `DEADLINE_EXCEEDED` | The proving attempt did not complete before its RPC deadline. Retry the complete request if the Composer's settlement cutoff still permits it. |
| `ABORTED` | The prover could not complete the attempt against a stable snapshot or concurrent state. Revalidate or rebuild the request from fresh Composer state, then restart the complete proving operation. |

Every other non-`OK` status is non-retryable for the unchanged request. In
particular:

| Code | Composer action |
| --- | --- |
| `INVALID_ARGUMENT` | Fix malformed stream structure, bounds, widths, calldata, or DA encoding; do not retry unchanged input. |
| `FAILED_PRECONDITION` | Treat the batch, window, deployment identity, or execution evidence as rejected; do not retry unchanged input. |
| `RESOURCE_EXHAUSTED` | Treat the request as exceeding a fixed message, window, witness, or other deployed limit. Change the request or coordinate a limit change before retrying. |
| `CANCELLED` | Respect the cancellation. It normally reflects caller cancellation and MUST NOT trigger an automatic retry. |
| `INTERNAL` | Treat the failure as a prover or operator fault; do not retry automatically. |

This default also covers codes not listed in the second table. Here,
"non-retryable" means that the Composer MUST NOT automatically resend the same
complete request. It does not require the Composer process to stop: the
implementation may discard or recompose the request, correct its configuration,
or alert an operator. The Composer MUST NOT infer retryability from a status
message or implementation-specific error text.

A Composer SHOULD define a bounded recovery policy for repeated non-retryable
prover rejections so that one composition cannot indefinitely prevent
settlement progress. The policy MUST NOT rely on unvalidated failure details.
This implementation charges
selected candidates once per failed prover or relay episode with a saturating
counter; transport retries within an episode do not each charge it. At
`MAX_BUNDLE_ATTEMPTS = 3`, it evicts exhausted candidates and their same-sender,
same-direction nonce suffixes, including queued and in-flight reservations.
Independent senders and the opposite direction remain eligible. Invalid actionable
hints receive the same bounded opaque-rejection handling. Automatic submitted-attempt reconciliation has a separate six-failure bound
described in §8; both paths use the same cumulative source counter. Excluded target
slots use a separate counter (§8). A potentially valid
candidate may be evicted to restore progress and then requires resubmission.
Construction failures, skipped slots, and source-condition waits (including
insufficient escrow) do not charge this counter. Repeated funding checks cannot
exhaust a user's settlement-attempt budget; the queue retains the request while
independent eligible source chains continue.

Local native finalization distinguishes execution/effect rejection from
infrastructure uncertainty. A never-dispatched candidate with a completed native
rejection is recorded as `PreflightRejected`, bound to its exact attempt identity.
After canonical source reconciliation, candidates with multiple groups reduce
the next selection to at most half their group count without a user charge.
Whole groups and nonce chains remain owned. The learned limit survives successful
preparation and clears when canonical completion drains the queue. Numeric
admission also respects this smaller bound. A failed native companion execution
may carry its exact signed source hash; Composer validates that it belongs to
this attempt. Only a singleton with that matching attribution uses the shared
execution-attempt counter with the three-failure bound. A poster failure or
unattributed root/effect mismatch cannot penalize a singleton. This does not
charge the excluded-slot counter or imply any L1 gas spend. Exhaustion retains
the existing explicit disposition and dependent-suffix behavior. Transport,
deadline, unavailable state and funding failures do not train this native bound.
Only the typed local execution/effect error selects this path; error text and
unvalidated external hints cannot authorize it.

### 6.2 Actionable failed preconditions

A `FAILED_PRECONDITION` rejection MAY carry a protobuf-encoded
`prove.v1.ProveFailure` in `Rejected.details` (or gRPC binary status details for
a transport-level error). The Composer
MUST decode the details bytes directly as `ProveFailure`, without a
`google.rpc.Status` wrapper. The gRPC status code remains authoritative:
details attached to any other status MUST NOT authorize candidate removal or
change that status's retry classification.

`ProveFailure.actionable_failure` has two defined variants:

- `OutboundFailure.transaction_index` is the zero-based position of the
  original signed L2 user transaction in the terminal Sync block, not the
  position of its preceding system-load transaction.
  `OutboundFailure.transaction_hash` is that transaction's canonical 32-byte
  signed transaction hash.
- `InboundFailure.entry_index` indexes the complete `PostBatch.entries` array:
  the anchor occupies index zero, so the first effect is at index one.
  `InboundFailure.entry_hash` is the 32-byte
  `keccak256(abi.encode(PostBatch.entries[entry_index]))`.

The original signed L1 transaction for an inbound effect is not present in the
proving request. A Composer that supports actionable recovery MUST therefore
retain its request-local candidate-to-entry mapping until the RPC completes.

Before changing any candidate or pool state, the Composer MUST bind both fields
of the selected variant to the exact rejected request:

1. require the hash field to be exactly 32 bytes and the index to be in range;
2. recompute the transaction or entry hash at that index and require an exact
   match; and
3. resolve that exact transaction or entry through request-local state retained
   during composition to the original selected candidate.

An empty, malformed, unknown, wrong-width, out-of-range, mismatched, stale, or
unresolvable detail MUST be handled as an ordinary non-actionable
`FAILED_PRECONDITION`. The Composer MUST NOT infer a candidate identity from
the status message, diagnostic text, an index alone, or a hash alone.

After a valid detail is resolved, the Composer MUST NOT resend the rejected
request unchanged. It MAY remove the identified candidate and its same-sender,
same-direction nonce suffix, then rebuild the batch, terminal Sync block,
witnesses, and complete v2 request from the remaining candidates. The
rebuilt attempt is subject to the same settlement cutoff as any other proving
attempt.

Index-and-hash validation binds the detail to the rejected request; it does not
independently prove that the reported execution failure occurred. Candidate
removal therefore trusts the configured prover, even when the transport is
authenticated.

### 6.3 Retry procedure and settlement cutoff

The reference Composer starts backoff at 100 ms, doubles its base up to 1,000 ms,
and applies ±20% jitter. All candidates with an exact L1 target share
one exclusive deadline: target timestamp minus submission slack. Preparation
and proof settings are planning allocations. Crossing the preparation allocation
MUST NOT discard an accepted candidate or substitute an empty L1 posting while
proving and post-proof work still fit. A proof operation may exceed its planning
allocation by using time left by preparation. Its timeout, session recovery,
and retries share the original remaining window minus the post-proof reserve;
retries MUST NOT reset the deadline. The monotonic bound MUST NOT be extended by
a backward wall-clock correction; forward corrections may shorten it.

Reserve signing, response validation, ordered native preflight and delivery.
Recheck the exact target, canonical parent and exclusive deadline before relay
dispatch. A timed-out delivery after dispatch is uncertain submission, not
proof of exclusion. A timed-out signer RPC does not release the server's worker
permit before that worker exits. No parallel proof race is introduced.

Ordinary-only candidates use the same operational deadline and post-proof reserve.
The historical proof anchor is distinct from the actual submission parent.
Actual-parent proofs bind that exact parent; historical proofs retain B and
recheck its availability/root/rule period. A preparation estimate MUST NOT
select a future execution height whose parent does not yet exist. Resolve
policy targets to the observed head's immediate child; skipped slots may advance
its timestamp only within the shared bounded replacement allowance. Never
slide the target after proving. If preparation exhausts the window, retain the
unsettled history for a fresh attempt. The prior full-proof start reserve could
reject usable windows and make a slow observation select an unknown parent
indefinitely. This scheduling correction changes no wire fields or validation.

For a retryable status, each retry opens a new v2 transport and resumes the
exact retained prefix or sends Begin with complete backfill. Finalize always
revalidates the complete selected request; ACKs cannot replace authorization. The Composer SHOULD use exponential backoff
with jitter, a capped delay, and a bound on total attempts or elapsed time.
It MUST NOT sleep or begin another attempt if the remaining settlement window
is too short for a complete stream, proving operation, response validation, L1
transaction construction, and relay or submitter delivery.

The Composer therefore MUST determine its latest safe settlement cutoff before
starting an attempt and re-check it before every retry. Under the current EEZ
slot profile, the proven bundle must reach the configured relays before the
terminal Sync block's timestamp minus the configured submission slack. The
reference Composer sends one identical request to every configured relay as a
single delivery attempt; one acknowledgement suffices to begin canonical
observation and never establishes inclusion (see the
[relay fan-out contract](../../docs/configurable-settlement-policy.md#builder-relay-fan-out)). A third-party
Composer MAY organize its scheduler differently, but MUST enforce an equivalent
cutoff and reserve time for all post-proof work.

If another complete attempt would miss that cutoff, the Composer MUST stop
retrying, leave `batch.proofs` empty, and MUST NOT submit the unproved batch. It
instead produces and commits an empty terminal Sync block to L2, without the
selected effect transactions or a proof-dependent L1 submission. It preserves
or re-queues the selected effects and tries them again in the next Sync
interval. Because the L1 settlement cursor did not advance, the next successful
proof request begins at the same `posted + 1` and includes the empty Sync block
as an intermediate block.

A malformed response, wrong signer, invalid signature, or mismatch between the
locally recomputed hash and `response.public_inputs_hash` MUST be treated like a
non-retryable failure for that request. The Composer MUST NOT populate
`batch.proofs` or submit the batch.

## 7. Composer conformance

A Composer implementation SHOULD test:

- exact header-first stream ordering and complete block-range emission;
- block hash widths, parent adjacency, exact RLP, and augmented witness
  generation;
- accepted anchor-only, inbound, outbound, and mixed batch construction;
- response length, signature encoding, hash, and registered-attester checks,
  including rejection of a valid registered-attester signature over a hash for
  a different batch;
- proof insertion without mutation of any other batch field;
- the closed retryable-status allowlist and default non-retryable handling;
- accepted outbound and inbound actionable details, plus fail-closed handling
  of details on the wrong status and empty, malformed, unknown, wrong-width,
  out-of-range, mismatched, stale, or unresolvable details;
- request-local actionable-failure resolution, dependent-candidate handling,
  complete recomposition, exponential backoff, and settlement-cutoff fallback
  to an empty Sync block; and
- an end-to-end request whose response is accepted by the Composer and whose
  final `postAndVerifyBatch` succeeds against the deployed EEZ and
  `ECDSAProofSystem` contracts.

The fork's current positive and negative cases live in
[`eez-prover-client`](../eez-prover-client/src/lib.rs),
[`eez-proof-signer`](../eez-proof-signer/src/settlement/tests.rs), and
[`eez-node-e2e`](../eez-node-e2e/tests/cross_chain.rs). Historical captured
calldata-only requests are not positive fixtures for the mandatory native-blob
profile.

## 8. Settlement ownership after submission

A proof authenticates the full proposed execution. It does not promise that
every source transaction or execution entry will be consumed on L1. The
Composer MUST retain each submitted signed user in memory until canonical
derivation and source-chain evidence determine its disposition. A confirmed L2 height or an
observed postBatch receipt alone MUST NOT release all users in an attempt.

The reference implementation builds a local settlement manifest from the final
survivor compositions. It retains signed identity, direction, nonce, FIFO order,
attempt count, whole outbound groups, producing inbound call coordinates and
complete endpoint hashes for virtual producing effects, including callbacks.
Semantic forests, source tables and physical checkpoint positions are validated
during construction. Static and reverted lookups produce no L2 deliveries; their
users remain represented even when they have no producing effects. This manifest is
local recovery metadata; it does not change the authenticated native-blob format.

The deriver publishes batch evidence before its scheduling cursor and marks a
canonical scan complete only after all same-block batches and safe-head updates.
Evidence includes the L1 block hash, postBatch transaction and position, calldata
hash, applied virtual slice, exact L2 endpoint and selected replay groups. Empty
application is distinct from incomplete derivation. Recovery MUST recheck the
canonical generation and block identity before changing ownership.

An immediate entry skipped at runtime does not inherit an earlier carrier's
checkpoint unless the proven chain assigns the same commitment. Authenticated
inbound lookups have zero replay weight. Their no-op root logs may persist or
roll back with the source frame; this must not change the count or ordering of
producing effects. Same-height
resumption retains the validated prefix and authorizes only the appended
transactions against the resumed table. The new suffix still requires exact
outbound-call coverage; retained prefix events cannot authorize new calls.

Inbound users require source receipts and nonces at the checked canonical L1
state. Mutable consumption must belong to the correct batch's queue window.
An included revert or successful source transaction with missing intended calls
spends its nonce and MUST NOT be retried as identical signed bytes. Static-only
source completion does not require a root change. Outbound users instead use
the validated L2 endpoint and its canonical source nonce; optimistic L2 receipts
are insufficient. A signed outbound user and all its loads/callbacks MUST remain
one indivisible replay group.

Eligible omitted users return to the held pool in FIFO order. A conclusive
settlement failure charges each eligible user's bounded retry policy once.
A landed postBatch that leaves the user incomplete is a settlement failure:
automatic admission permits six cumulative such failures at reconciliation,
so five consecutive confirmed incomplete settlements can trigger a smaller
sixth attempt. Numeric admission retains three. Prover/immediate relay rejection
keeps the §6.1 bound on the same source counter; no transition resets that counter.
An excluded target slot (the target block was built without the bundle, so no
postBatch gas was spent) charges a separate per-source counter bounded by
`MAX_EXCLUDED_SLOT_ATTEMPTS = 12` in both admission modes. Such slots include
proposer-built blocks and builders that never held the bundle, which the Composer
cannot influence; counting them toward the small settlement bound evicted valid
users after a few unreachable slots. Either bound exhausts the user.
The per-rollup admission streak resets only after complete canonical settlement
of a nonempty source batch, and skips uncertain or canonically skipped targets.
Admission preparation, acknowledgement and progress elsewhere are not source
completion. See the [admission policy](../../docs/configurable-settlement-policy.md).
Infrastructure-only failures do not charge that policy. Only an exhausted,
still-unspent source nonce creates a dependent nonce gap. The pool transfers
ownership and reservations under one lock. Historical attempts cannot
release or recover users owned by a newer attempt.

The reference node tracks submitted attempts in memory. It retains signed source
bytes, the exact pooled postBatch envelope, target, manifest, retry counters and
ownership throughout reconciliation and recomposition within the process.
Invalid ownership transitions leave the prior state unchanged.

At the next slot's preparation deadline, the Composer MAY replace its active
speculative L2 suffix if the expected L1 block is still absent. The current
implementation permits up to four such replacements per observed L1 head and requires
completed canonical derivation through the unchanged parent. This deadline is
not a failure verdict: retain the original envelope, manifest, target, observer
and signed users until canonical reconciliation. Returning those users to the
held pool for recomposition MUST preserve reservations and retry counters. Old
results MUST NOT release users owned by a newer attempt. Canonical source
evidence can still complete users whose replacement preparation did not finish.

Each replacement advances exactly one slot from the current attempt, with at
most five scheduled opportunities on one parent including the original. The
scheduler and submission gate share the same bound. After exhaustion, wait for a
new canonical head; do not release users or charge a failure from timer expiry.
Retain every historical attempt until canonical reconciliation, including when
the earliest original settles after several replacements. Older Composers retain
the previous one-replacement bound; this extension needs no wire, signer, follower
or storage migration and rollback restores the smaller scheduling allowance.

The replacement uses a fresh proof, the next slot's exact timestamp and the same
poster nonce. Replace the original nonempty Sync with an empty Sync and rebuild
its speculative descendants before placing complete cross-chain groups in the
new terminal Sync. An original Sync that is already empty, as for
historical-prefix and anchor-only attempts, stays canonical with its
descendants; only the proof and bundle are rebuilt. A rebuilt empty block can
differ by header fields such as the coinbase, so switching to it would unwind
retained history. Retain ordinary transactions through normal reorg recovery.
Final bundle execution and target checks remain mandatory. If the original
settles late, canonical replay replaces the active branch and source
reconciliation determines which users remain pending. The scheduling deadline
does not consume a user failure attempt or establish settlement progress.

This replacement scheduling rule preserves the offline profile's encoding,
signer validation and historical replay. Every participant must implement that
profile; it does not permit older seed-based implementations. Restart retains
the in-memory recovery limits below. See the
[accepted replacement design](../../docs/proposals/skipped-slot-speculative-retry.md).

On restart, canonical derivation recovers settled state and rechecks retained
ordinary history against canonical L1 evidence. It discards unowned cross-chain
or inconsistent descendants before accepting new cross-chain submissions.
Held requests and retry counters are lost; clients may need to resubmit unsettled
cross-chain work. Expired postBatch transactions are
not replayed. Reorg invalidation within the process follows L1 identity even when
L2 height is unchanged. Inconclusive finality audits retain history.
Finalization never releases newer reservations.

Partial consumption is always enabled. Every composer requires a timestamp-pinned
bundle relay. Only actual bundled L1 user hashes may revert.
postBatch stays first and required; outbound L2 hashes are excluded.
Blob sidecars and timestamp pins are preserved. No `droppingTxHashes` field is
sent. A builder configured with `discard_txs=true` may nevertheless omit an
allowed transaction that fails transaction validation, so receipt and nonce
reconciliation remain mandatory. Submission has no raw-mempool fallback.

There is no runtime mode that disables partial-consumption reconciliation and
no settlement storage requirement.
