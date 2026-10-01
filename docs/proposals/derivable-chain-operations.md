# Derivable ChainOperation: implementation contract

Current-network contract (2026-09-28): [native transactions and direct V0](native-system-transactions.md)
supersedes the historical wire/environment/system-transaction descriptions below.
The old envelope is not a runtime option. Earlier validation remains evidence for
its recorded revision only; current validation is in the linked implementation audit.

Wire revision (2026-09-22): [two-byte RollupX V0](two-byte-da-v0.md) supersedes
this document's tag-3 wire shape, sparse-record framing, terminal/entry
integer-list wrappers and u16 transaction ceiling. The active format is tag
0/profile 1, with byte-string terminal/entries and a 16,383 transaction ceiling.
The execution, reconstruction, richer-action, caching and authentication
requirements below remain in force. Historical tag-3 evidence does not validate
the new encoding; its validation is recorded separately. The old wire description
below is retained as design history, not an alternative runtime mode.

Status: implementation in progress, 2026-09-16. Reviewed against
`ddf40bc2eb5fac8f7973b112eb383256cd8aac74`; the implementation is now aligned with
`5469c88860408daf776ee812156dd280c1a9e262` ([integration evidence](../research/2026-09-16-blob-efficiency/implementation/main-alignment.md)). No runtime change or deployment is
performed by this document. Revised after a two-round Claude review and independent
code checks; see the [review disposition](../research/2026-09-16-blob-efficiency/claude-review/README.md). It supersedes the ordering in the exploratory
[research](../research/2026-09-16-blob-efficiency/README.md).

Scope revision (2026-09-16): the user confirmed this is a **new network**.
Implement one operations format from genesis: tag `0x03`, profile 1. There is no
historical-format compatibility, timestamp activation, network upgrade, old signer
interoperability, writer rollback, or old witness-database migration requirement.
Historical research/review artifacts describe the earlier proposal; this revision
supersedes their compatibility requirements. Chain reorg, crash recovery, partial
settlement, and execution-fork correctness remain required. The unchanged terminal
and cross-chain field encodings below are protocol constraints, not legacy support.

Implementation and focused signer soundness work are in progress on the feature
branch. Codec, reconstruction, cold/prepared signer, producer, and follower paths
have component and native evidence. The coverage ledger records the remaining
release-evidence gates separately. Completion of code does not authorize
production deployment.

## 1. Objective and fixed scope

Fit substantially more ordinary L2 history in one permitted blob transaction by
publishing execution inputs rather than complete ordinary blocks. Empty ordinary
blocks must not contribute a header-sized DA cost. Follower execution must produce
the original blocks and match the commitment actually applied on L1.

Only `ChainOperation.operations` changes on the DA wire. Preserve:

- The standardized outer message version, semantic messages, signed cross-chain
  transaction bytes, call/result/rollback data, and message order.
- The terminal Sync block's existing encoding and all `l2_entries` and
  `outbound_group_sizes` encodings, values, and order.
- Physical blob packing, KZG and public-input binding, contract ABI/storage,
  ECDSA/proof-system semantics and cross-chain scheduling/ownership rules.
- Independent follower validation and catch-up from the new network genesis.

No zlib, Brotli, or other general-purpose compression. No cross-chain deduplication,
new calldata carrier, shared blob channel, hash truncation, or omitted system
transaction is part of this change. Internal proving RPC extensions and signer
limits are in scope because long spans must be usable at settlement time.

### 1.1 Signer soundness is a non-negotiable release condition

The user explicitly prioritizes signer soundness over space savings, capacity and
latency. A smaller payload, a passing follower, an AI review or a favorable
benchmark cannot justify removing or bypassing a signer authorization check.
Follower rejection detects a discrepancy after publication; it cannot undo an
incorrect attestation already accepted by L1.

The required safety statement is: **every signature from any new signer path
must authorize the exact request that complete authenticated execution and all
current settlement/DA gates would authorize under the selected protocol rules.**
For a cached request, fully validating its referenced blocks with their original
witnesses must yield the same accepted execution facts, effect binding and final
public-input digest for that same request. Cache state, resource hints, timing and
retry history must not enlarge the set of semantically authorized requests.

This comparison is about authorization, not old operational caps: the deliberately
raised block limit and the new payload tag are explicit changes. For an equivalent
legacy-format batch, compare reconstructed blocks and effects, not identical raw
public-input hashes: changing DA bytes changes blob commitments and the digest.
For cold versus cached validation of the **same** v3 request, require equal
public-input digests and the same recovered attester; signature byte identity is
not the oracle.

Required implementation discipline:

1. Preserve the existing private-constructor `AttestablePublicInputsHash` boundary.
   Only completion of the full settlement job may construct it. Preparation,
   reference resolution, capability responses and cache entries cannot construct
   an attestable value or invoke a parallel signing path.
2. Inventory every current execution, identity, witness, intermediate/terminal,
   checkpoint, semantic, value, DA/KZG and public-input gate. Map each to the new
   cold and cached paths. Record any deliberate strengthening. No check may be
   replaced by trust in composer-supplied claims or an assumed cache invariant.
3. Keep cached validated facts immutable and constructible only after successful
   validation. Preserve all evidence and context needed by downstream gates.
   Malformed data, unsupported fields, missing facts, wrong validation identity,
   cancellation, timeout and internal invariant failure must never yield a
   signature. Larger limits must not remove bounds or introduce unchecked math.
4. First establish the complete cold v3 path. Introduce caching/reference changes
   in separately reviewable changes with a specific soundness argument, not bundled
   with removal of validation. Optimize repeated work only after proving which
   validated facts remain reusable and which request-specific gates must rerun.
5. Maintain a separate full-execution authorization oracle in tests. Compare it
   against warm, cold and mixed-reference requests, including permutations of cache
   preparation, eviction, restarts, reorgs, environment changes and retries.
   Test shared helpers against independently generated expected blocks/fixtures so
   signer/follower agreement does not hide a shared implementation error.
6. At the actual attester boundary, instrument tests to prove zero signing calls
   for rejected/mutated requests. Exercise single-field and combined tampering,
   parser fuzzing, missing evidence, alternate parents, forged event classification,
   partial settlements and cancellation races. Returning an RPC error after signing
   is not a passing negative test.
7. Review the soundness of the implemented signer changes and their gate
   mapping before production deployment. Per the user’s 2026-09-17 instruction,
   this is a change-specific correctness check, not a general security review. Track findings to resolution;
   no unresolved soundness issue is waived for performance. The two Claude rounds
   reviewed a design and do not satisfy this implementation-review gate.

Every change to signer authorization must carry a written argument identifying trusted
inputs, authenticated commitments, authorization invariants, and why any reused
fact remains valid in the new context. Tests support that argument; they are not
a mathematical proof that arbitrary future code has no bugs. Where the argument
or evidence is incomplete, retain full validation and keep the affected new path
disabled. Failure to meet the performance target is preferable to an unauthorized
signature. This priority overrides any performance-driven interpretation of later
sections, including the large-window release targets.

## 2. Decisions

| Decision | Specification |
|---|---|
| New inner payload tag | `0x03`, named `TAG_DERIVABLE_ORDINARY`. Tags `0x00..0x02` are not production formats for this new network. Research-only hypothetical tags were never production assignments. |
| First profile | `profile_id = 1`: derive supported post-Merge ordinary Ethereum blocks using explicit common execution inputs; retain full-block exceptions. |
| Terminal treatment | Always exactly one full terminal block, with the old integer-list encoding intact. Never apply ordinary omission rules to it. |
| Ordinary representation | Total count, common environment, sparse records for transactions, environment changes and full exceptions; gaps mean empty blocks under the active environment. |
| Format block ceiling | 65,536 total blocks, including the terminal. Thus at most 65,535 ordinary blocks per v3 payload. This is a format safety bound. |
| Signer block default | Change `EEZ_PROOF_SIGNER_MAX_REQUEST_BLOCKS` from 512 to **65,536**. Explicit lower operator values remain effective for local admission. |
| Other signer quotas | Keep current bounded byte/item/time defaults, detailed in §7. Do not multiply all quotas by the block-count increase. Incremental validation and cached references handle long ordinary history. |
| Long-span validation | Prepare and validate ordinary blocks ahead of settlement; reference immutable signer-local validated facts at final proving. Terminal validation remains fresh. |
| Network format | Tag `0x03`, profile 1 from genesis; no activation flag or older-format deployment mode. Production deployment remains a separate operational action after release gates pass. |

65,536 blocks span about 36 h 24 min at two seconds per block. This is an admission
ceiling, not a claim that 36 hours of arbitrary transactions fits DA, memory or
execution budgets. The supported workload envelope is established by §11.

## 3. Exact v3 data shape

Use canonical Ethereum RLP and explicitly typed Rust `Bytes` for new opaque byte
strings. Do not rely on `Vec<u8>` to encode a byte string. Preserve the legacy
types for the three unchanged terminal/cross-chain fields.

```text
0x03 || rlp([
  profile_id,                 # integer, exactly 1
  ordinary_block_count,       # integer, 0..65535
  common_environment,        # fixed-arity list below
  ordinary_records,          # ordered list below
  terminal_block_legacy,     # old Vec<u8> integer-list item, exact original bytes
  l2_entries_legacy,          # old Vec<Vec<u8>> representation, unchanged
  outbound_group_sizes       # old Vec<u16> representation, unchanged
])

common_environment := [
  timestamp_step_seconds,     # nonzero u64
  fee_recipient,              # byte string, exactly 20 bytes
  prev_randao,                # byte string, exactly 32 bytes
  gas_limit,                  # positive u64
  extra_data,                 # byte string, 0..32 bytes
  cancun_parent_beacon_root   # byte string, exactly 32 bytes
]

ordinary_record := [empty_gap, kind, data]
kind 0: data := [signed_tx_bytes, ...]   # nonempty, Bytes / EIP-2718
kind 1: data := canonical_full_block_rlp # one Bytes value, exact RLP
kind 2: data := [new_environment, [signed_tx_bytes, ...]] # tx list may be empty
```

`empty_gap` is the number of default empty ordinary blocks since the preceding
record, or since the anchor for the first record. After the gap, the record consumes
one block. Remaining positions up to `ordinary_block_count` are default empty
blocks. For example, count 100 and a transaction record with gap 99 puts that
record in ordinary position 99, using zero-based positions. Kind 0 cannot encode
an empty transaction list. Kind 2 may: after executing its gap under the old
active environment, install the new environment for the record's one block and
all following gaps/records until the next change. Reject a no-op environment
change. Kind 1 does not alter the active environment; subsequent timestamp
arithmetic starts from its actual header. Every record consumes exactly one
ordinary position. Records cannot overlap, run backwards, or consume the terminal.

A repeated fee-recipient, timestamp-step, randomness or beacon-root change therefore
costs one environment record rather than full headers for the rest of the window.
No environment change creates a new legal Sync boundary. Profile changes and
unsupported block shapes still require supported full records or rejection.

The sparse representation is intentional. A flat RLP list of 65,535 empty-block
counts costs 65,538 bytes, approximately 8.6% of six blobs, 25.8% of two blobs, or
51.6% of one blob under current useful capacity. Keep a flat-count implementation
only as a benchmark baseline; the selected format removes that linear empty-history
cost. Both layouts must preserve transaction placement and the same derived blocks.

Counts and integers use canonical unsigned RLP. Reject negative/oversized integer
representations, noncanonical RLP, wrong list arity, trailing bytes, unknown tags,
unsupported profile IDs/kinds, malformed transaction/block encodings, arithmetic
overflow and record coverage beyond the declared count. Apply the block ceiling
before allocations or expansion. Retain the existing per-block transaction-count
bound (`u16::MAX`) and fork-aware transaction validity. Bound list parsing by the
actual input slice; do not allocate from unchecked declared RLP lengths.

No arbitrary data-byte expansion is needed during parsing: keep sparse records
and borrowed/owned input slices. A bounded block-plan iterator produces ordinary
positions. Arrays of block counts may contain at most 65,536 elements; full headers
are built by execution, not synthesized by the codec.

The 65,536 format bound applies to the network operations format. Operational
signer request limits remain separate.

The batch's effective anchor/range remains established by existing settlement
logic. The payload count must agree with the proof window and placement rules.
Do not introduce a second source of authority for the starting block hash.

### 3.1 Common environment and full-block exceptions

The composer builds the initial common environment and subsequent changes from
its actual sequencing inputs;
the environment is published and bound by DA. It is not inferred from the
follower's local operator defaults. The signer checks it against every block
represented by the derived path.
Select the initial environment from the first profile-compatible ordinary position
in the actual retained history, not from the node's potentially newer current
configuration. A range with no derived ordinary positions may retain the writer's
syntactically valid default environment because no block consumes it. A different but valid environment can therefore
be represented without silently changing a block.

For a derived block:

- Number is the parent's number plus one, with checked arithmetic.
- Timestamp is the parent's timestamp plus the explicit positive step, with
  checked arithmetic. The step must reproduce the validated block's actual
  timestamp; it is not permission to violate active consensus timestamp rules.
- Use the explicit fee recipient, randomness, gas limit and extra data.
- Pre-Cancun parent beacon root is absent. From Cancun onward use the explicit
  root in the common environment. This permits the current zero input without
  treating zero as an undocumented consensus constant.
- Ommers are empty. Withdrawals are absent before Shanghai and empty afterward.
- Parent hash, execution outputs and remaining header values come from the
  authenticated parent, chain rules and actual execution.

Support the pinned execution stack's post-Merge rules through Osaka initially,
including supported boundaries within that range. Profile 1 does not derive
pre-Merge blocks, blocks with nonempty withdrawals/ommers, or blocks requiring
newer slot-number/block-access-list fields. Such ordinary blocks may use full
records only when both the signer and follower can validate and reproduce their
exact canonical block. Consensus validity of a supplied header alone is not
sufficient. Otherwise reject them as unsupported. A future execution-fork upgrade
must add conformance tests for both the derived
and full-record execution paths before claiming support. In particular, the
current follower sets `slot_number` to `None` and uses a builder with empty ommers;
a full record does not automatically make other values reproducible.

An ordinary block with a supported changed environment uses kind 2 when the
shared assembler can reproduce it. Other supported exceptions use a full record.
The following block derives from that full block's actual header and the still
active environment. The encoder prefers derivation when its complete
eligibility check succeeds; readers may accept an otherwise valid full record
even when a shorter representation would have been possible. Full-record
acceptance still enforces the existing ordinary/system-sender restrictions.

For a terminal-only range, retain the normal common environment chosen by the
writer; it is syntactically checked but has no ordinary execution effect.

## 4. One execution-input interpretation, independent execution checks

Introduce a typed `Environment` and `BlockPlan` in
`eez-payload-codec`; add the shared translation/assembly helper under
`eez-driver::derivation` (feature-gated as needed). Signer and follower already
depend on the driver. Avoid a dependency cycle or a second hand-written Ethereum
header implementation.

Use the pinned Reth `EthBlockAssembler` and next-block environment construction.
Its assembler builds transaction/receipt/withdrawal roots, bloom, gas fields,
requests hash and the other header fields from parent, execution context and
execution outputs. Expose the minimal adapter inputs needed to call that same
assembler from signer validation. Do not copy roots from a submitted header into
the reconstruction and call that derivation.

### 4.1 Signer

The signer still receives complete block RLP and witnesses during cold validation
or ordinary preparation. Existing Stateless validation authenticates execution,
state roots, transaction signatures and witness pre-state. For each derived
ordinary block it must additionally:

1. Obtain and authenticate the parent header against the block's parent hash,
   using the accepted prior block or witness ancestor for the first block.
2. Construct the EVM environment from that parent, the recipe and trusted chain
   rules. Confirm it matches the environment used for the validated execution.
3. Assemble the complete block using that environment, the exact transaction list,
   and the successful Stateless output (state root, receipts, requests and gas).
   The current backend exposes these outputs before reducing them to settlement
   facts; perform the check there rather than discarding them prematurely.
4. Require byte-for-byte canonical full-block RLP equality with the validated
   original block. Check no intermediate system-sender transaction is admitted.
5. Preserve window parent-hash continuity and state-root telescoping. Keep all
   existing terminal checkpoints, lookup observations and settlement gates.

This validates the omitted header outputs, not just inputs or transaction lists.
Full ordinary exceptions take the existing exact full-block path. The terminal
always takes the existing checkpoint-capable path. DA/semantic binding and
public-input recomputation still precede attestation.

The initial implementation retains assembler equality. A later field-comparison
optimization requires an exhaustive field classification of the pinned `Header`
(no `..` in the Rust destructuring), including explicit absence checks for unsupported
optional fields, with per-fork conformance evidence. Producing code being honest
is not a validation premise. Changes to the pinned Reth/Stateless revision require
re-running this audit; shared code does not eliminate upgrade review.

### 4.2 Follower

Refactor `execute_block_with_template` into an execution-input path plus the
existing full-template validation wrapper. A derived block has an explicit
recipe; it must not appear as a fake full template with zero output fields, or
fall accidentally into the old no-template default branch.

Run all pre-execution transitions, transactions and post-execution transitions,
including for empty gaps. Build every intervening block so `BLOCKHASH`, system
contract state, base fee and fork transitions remain correct. Reuse locally
present blocks only if the existing ancestry and new recipe/environment checks
establish equivalence; matching transaction lists alone is insufficient.

Keep terminal reconstruction, source authorization, partial settlement,
same-height resume and `check_claimed_state` behavior. The L1-applied endpoint may
be a prefix of the advertised Sync; do not replace it with the last root claimed
in calldata. New-format metadata must also flow through catch-up, correlation,
batch evidence, restart and safe/finalized-head paths, not only live replay.

### 4.3 Claimed proof window versus the L1-applied range

`ordinary_block_count` describes the original claimed/proved window; an L1 outcome
does not rewrite its bytes. A post may contain many ordinary blocks and later be
observed as resumed because another same-L1-block post applied its leading steps.
Do not require its ordinary count to become zero. `batch_l2_range` maps a resumed
outcome to the existing Sync height independently of that claimed count.

Retain the original proof-window anchor and full terminal. For a resumed outcome,
resolve the actual applied prefix using existing `Settlement.start/len/entry_state`
and `final_state`; replay only the selected terminal suffix against the existing
Sync content. Do not derive the advertised ordinary history again from the
same-height prefix commitment, or apply pre-execution changes a second time.
A genuinely terminal-only claimed window uses count zero; that is distinct from
a multi-block payload whose applied range happens to be one resumed Sync.
The signer's `window_pre_block_hash` stays the hash preceding the original claimed
window, and still binds the leading claimed `currentRoot`. A resumed `entry_state`
is instead the hash of a prefix of the already existing Sync at height H; it is
not substituted as the parent of the claimed ordinary window or as a new H+1
execution parent. Reconstruct the retained Sync transactions and selected suffix
from the canonical parent at H-1, preserving the existing one-block execution
semantics. Require the retained terminal's parent hash to match that canonical
parent. Competing prefixes with different environments cannot be accepted merely
because their heights or transaction lists match.
Tests must cover both shapes and verify original bytes, actual endpoint and
system nonce/effect selection after competing prefix settlement.

No signer cache is needed or trusted by a follower. A clean follower must derive
from the canonical anchor, available L1/DA inputs and chain configuration alone.

## 5. Incremental signer validation for long spans

Increasing the block ceiling alone would leave the whole ordinary history to be
re-executed inside a Sync deadline. Include incremental validation in this plan,
using the existing persistent per-block witness store as the producer-side source.
It changes internal proving work, not what cross-chain settlement authorizes.

### 5.1 Internal RPC extensions

Extend `prove.v1.Prover` and its Prove chunks where measurements require it.
Preserve existing settlement message semantics:

- `Capabilities`: the operations/profile identity,
  maximum total window blocks, request bytes/witness items, preparation chunk
  ceiling, effective per-message limit, timeout values, and validation identity.
  The identity binds rollup ID, EIP-155 chain ID, chain/fork configuration digest,
  expected system address and a validator-schema/revision identifier. It does not
  expose private keys.
- `PrepareOrdinary(stream ...)`: complete ordinary block RLP/witnesses, in bounded
  chunks, with parent linkage. It performs actual Stateless validation, extracts
  authenticated execution inputs and verifies supported-profile reconstruction
  using §4.1. Preparation does not accept a caller-selected recipe environment.
  Return only verified block hashes and validation identity; **never a settlement
  signature or a safe-head claim**.
- `CachedOrdinaryRange` Prove chunk: inclusive first/last numbers, first/last
  block hashes and validation identity. Validate ordered bounds and charge the
  full logical count before walking local cached parent links. Verify every
  number/hash/parent connection, including the prior chunk or authenticated anchor,
  and resolve only immutable locally validated facts. Adjacent ranges and witnessed
  blocks must form exactly the declared request window, with no overlap, gaps or
  duplicated positions. A range claim does not prove its own validity.

The final `Prove` request may mix cached ordinary references and fully witnessed
ordinary blocks, but the terminal is always fully witnessed and freshly checked.
Every logical block counts against the total window ceiling even if it uses a
reference. Unknown/evicted references return a structured retryable cache-miss
result with the missing block identity, not an actionable cross-chain failure.

Preparation uses a header-first stream: rollup ID and expected validation identity,
followed by ascending `BlockWitness` values.
It requires a nonempty contiguous ordinary range, at most 64 blocks. Each block's
parent is authenticated by its witness or the preceding validated header; no
client-supplied state-root shortcut is accepted. A fully validated ordinary block
that needs the full-record path can be cached too, with exact validated fields and
its supported derivation classification. At final proving, compare the published
active environment at each position against the cached actual inputs. Preparation
failures return no reference for a failed
block; successfully validated immutable earlier facts may remain cached and can
be discovered/reused on retry. No partly validated entry is ever published.

Capability calls do not acquire the expensive execution permit. Preparation
streams have bounded ingestion queues and may not hold that permit while waiting
for more network input. Admission of a final proof preempts preparation between
block executions; one already-running block may finish, which is included in the
deadline tests. Retain the existing single active final-proof admission behavior.

Use `FAILED_PRECONDITION` with a dedicated structured cache-miss detail containing
validation identity and the first missing block identity. Introduce a separate
`ProverError::CacheMiss` branch and intercept it before both generic retry and
cross-chain `Actionable` mapping.
Allocate a distinct new variant (field 3) in the existing `ProveFailure` oneof;
keep original outbound/inbound fields 1/2 unchanged. A miss detail must name a
block/hash/identity in the exact outstanding request before the client repairs it.
Unknown/malformed details remain non-actionable errors, never source eviction. Existing handling of either gRPC status is not
sufficient. The remote client schedules bounded preparation for missing facts. It does not turn a cache miss
into eviction of a cross-chain source or repeatedly send the same unresolved
reference through `prove_with_retry`. If repair cannot fit the current target,
follow the existing unsent/failed-attempt reconciliation rules and keep ownership.
Malformed references and mismatched block identities are validation errors, not
cache misses that can be repaired by guessing another block.

Capabilities are advisory for resource planning, not cryptographic authority.
The existing returned-signature/public-input verification remains authoritative.
Reuse the tonic channel instead of reconnecting for every proof. Check capabilities
on initial connection, transport reconnect, and an explicit identity/limit-change
response; do not add a mandatory capability RTT to every final proof. Each prepare
response echoes validation identity and each proof request includes its expected
identity; a mismatch forces refresh and invalidates incompatible cached assumptions.
The composer requires the same operations/profile and validation identity as the
signer; it must not guess limits when the endpoint is unavailable. There is no
old-server mode.

Define `validation_identity` as Keccak-256 of a domain-separated canonical RLP
list containing an identity-schema version, rollup ID, EIP-155 chain ID, expected
system/EEZL2 addresses, the hash of canonicalized loaded chain-document JSON, and
an explicit validator/derivation schema revision bound to pinned dependency
revisions. Canonical JSON recursively sorts object keys, preserves array order
and scalar values, rejects duplicate keys before loading, and uses compact UTF-8
serialization. Different semantic source representations may conservatively
produce different identities; they must never be assumed equivalent by chain ID
alone. Publish golden identity vectors, including whitespace/key-order equivalence
and changed-fork/revision rejection. Resource settings are separately reported capability fields, not executable-state
facts. The implemented identity schema is:

```text
keccak256(rlp([
  Bytes("eez.validation-identity"), 2, canonical_json_version=1,
  rollup_id, chain_id, system_address20, eez_l2_address20,
  canonical_chain_document_digest32, Bytes(validator_revision), dependency_digest32
]))
```

Schema 2 accompanies the position-bound L2 static ABI. The validator revision
currently binds protocol `855fe0602484750861b4c31502a1a91355d33ed6`; peers
using the earlier `8d45891`, `9d13218` or `820e50f` pins or cached facts have a different identity.
Canonicalizer v1 is unchanged.

Canonicalizer v1 decodes and sorts object keys by UTF-8 bytes, emits strings with
compact serde_json escaping, preserves array order and original numeric token
spelling without floating-point conversion, and rejects duplicate decoded keys at
any level and nesting over 128. It hashes the source before existing spelling
aliases/defaults are applied. Numeric spellings such as `42` and `"0x2a"` may
therefore deliberately have different identities even if the chain loader gives
them the same meaning. Loading the rules and digest uses one file read.
`validator_revision` is an explicit semantic revision with the core-protocol pin;
`dependency_digest` is Keccak-256 of the exact build's Cargo.lock bytes. The latter
conservatively changes on unrelated lockfile changes too. Changes to local
validation/derivation semantics require a revision bump even with unchanged
locked dependencies. Both peers must build from the same lockfile/revision. This
namespace agreement is not authorization or proof of executable code identity.

### 5.2 Cache identity and integrity

Key cached facts by full block hash and validation identity. Store canonical block
RLP, authenticated parent/header facts, pre/post-state commitments,
the decoded transaction count, and complete `SettlementBlockEvidence` (including
system-sender flags and observed outbound events). Store authenticated actual
execution inputs and supported-profile assembly facts, not a boolean tied to one
caller environment. Preparation must run the same intermediate-block gate that
rejects privileged transactions **and outbound-event observations** before
publishing ordinary facts. Final proof runs the same gate over its actual
intermediate set for cached and cold blocks; no cached terminal is accepted.
Never synthesize empty observations for data whose observations were not checked.
Different recipe inputs must be checked against those exact facts; a validation for one environment
does not authorize another. Never key validity by height alone.

On final proof, verify the exact linked sequence, pre/post-state telescoping,
recipe match, requested terminal parent and all existing settlement binding.
Cached blocks can belong to another fork; their hashes are reusable facts, but
their place in this request is checked anew. Cached validation never marks data
canonical or settled.

Use a volatile byte-accounted cache with default ceilings **131,072 entries and
512 MiB of retained data/accounted metadata**, whichever is reached first. Include
an allowance for map/node overhead in accounting and separately measure process
RSS. Protect the pending ordinary window with bounded retention leases, then add
short-lived pins during final proof. LRU applies only to unreserved, unpinned facts.
If capacity cannot retain the required pending window, fail preparation explicitly
and constrain further admission; do not cycle between evicting its oldest facts
and re-preparing them.

Add `RetainOrdinaryRanges` to the preparation control surface: create/renew/release
an opaque lease over explicit hash-linked ranges and validation identity. Allow at
most eight leases per configured rollup (the current window plus bounded overlap
from retries, forks, or rapid composer restarts),
charge their union against the cache ceilings, and refcount overlap. Each lease is
bounded to 65,535 ordinary positions. Default lifetime is the larger of 120 seconds
and twice the signer request timeout, renewed every 30 seconds; expiry makes data
eligible for eviction and does not immediately delete it. An admitted proof pin
outlives lease expiry until proof completion/cancellation. Preparation may extend
an existing lease atomically as newly validated facts are published, avoiding a
publish-then-evict race before the composer can renew.

Leases are bounded resource requests, not evidence that an anchor is canonical.
The composer releases/advances its reservation only after existing canonical
settlement reconciliation, or explicit abandonment of that local candidate; a
signature or relay acceptance alone must not advance it. Reorgs select retained
hashes and re-prepare evicted ones. Final proof rechecks all cryptographic bindings
regardless of the lease. No untrusted anchor hint changes consensus validity.

Restart clears volatile facts and reservation leases, and triggers bounded
re-preparation from the durable witness store. References name hashes and validation
identity, not a generation token; a missing local fact remains a cache miss. A chain
configuration or validator revision change invalidates the whole namespace.
Cache loss is a recovery event, not permission to sign unchecked data. The bounded
recovery gate in §11 applies; merely reporting a long outage does not pass.

### 5.3 Scheduling, cancellation and storage

Prepare promptly after a committed ordinary block's witness is durable; do not
wait to accumulate a full 64-block chunk. Flush at least once per L2 block while
connected and work is available. Default preparation
chunks contain at most **64 blocks**, additionally bounded by current byte/item
quotas; adapt to fewer blocks when needed. A single oversized block is reported
explicitly. Keep the preparation queue bounded and deduplicate by block hash.

Set the producer's preparation queue to at most 128 block identities; on overload
keep a canonical catch-up watermark instead of accumulating witnesses in RAM.
After catching up, verify the watermark's hash and rewind it to matching ancestry
after reorgs. Preparation drains from the existing durable witness store. A
separate lightweight reference index may span the bounded pending history, but
must not retain a second witness copy.

Track the reconciliation floor as the exact canonical `(number, hash)`, including
a partially applied terminal prefix. Stop the ancestry walk before that floor:
an applied prefix is a valid parent commitment but need not have a standalone
full-block witness. A missing predecessor during an in-flight reconciliation is
a soft prewarm retry and MUST NOT make engine reconciliation depend on preparing
the speculative branch it is replacing. Refresh to the newest durable watermark.
Corrupt records, invalid ancestry, and an authoritative final-window witness miss
remain hard failures; no soft prewarm miss can authorize a proof or signing call.

When the newest durable watermark is more than one format window above the
canonical floor, walk its exact ancestry with bounded memory and prepare the
oldest 65,535-block prefix first. Keep that one lease until the authenticated
settlement floor advances; do not extend it into a second window. Resume from the
exact prepared endpoint after floor advancement. If a new watermark does not
descend from that endpoint, release the stale branch lease and rebuild from the
canonical floor before publishing new preparation hints.

Use one shared validation worker/permit as today. Final proof requests have
priority at block boundaries; ordinary preparation checks cancellation between
blocks and cannot indefinitely starve a terminal proof. Streaming ingestion and
cache admission are also quota-bound. Timeout/cancellation must release permits,
pins and buffers; signing after deadline/cancellation remains forbidden.

Before selecting cross-chain work for an exact slot, require the ordinary prefix
to be prepared or demonstrably cold-validatable within the remaining budget.
Otherwise prewarm and use the existing readiness deferral; never move already
selected cross-chain effects to a different target or relax submission slack.
Keep owned sources queued. An anchor-only candidate may use the existing eligible
future-target selection after preparation; it is still bound to the final target
when signing.

Change composer witness assembly and `RemoteProver::chunks_for` to use lightweight
references for prepared ordinary blocks and move/stream full witnesses for cache
misses and the terminal. Do not build duplicate full-window witness vectors.
Retain witnesses durably until the existing L1-finalized purge floor. The current
height-keyed store overwrites competing branches; extend it to a versioned
hash-keyed witness table plus a height/hash index for finalized garbage collection.
Lookup for preparation/proving takes an expected `(number, hash)`, and verifies
both identities. A canonical-height lookup is only a selector, not the storage
key. Retain competing unfinalized branch witnesses; on a branch switch do not
replace the only durable copy of the previous branch.

This new network starts with the hash-keyed schema; no historical database
migration is required. Validate stored height/hash/RLP consistency on reads.
Purge by the authenticated
L1-finalized floor and retain the boundary witness; use bounded deletion batches.
The witness trait, capture worker, restart path and tests change together. Test
branch A -> B -> restart with pruned state -> A before declaring retention safe. Disk-full/missing-witness conditions fail closed and must not silently prune
unsettled history. The existing MDBX map ceiling is not a free-disk guarantee.
Apply the global witness-store/map and actual disk-capacity bounds to all retained
branches and use bounded garbage collection. On exhaustion, stop dependent
sequencing/preparation and report a resource failure; do not delete required
unfinalized witnesses to satisfy an arbitrary per-height branch count.

## 6. Encoding and resource accounting in the composer

Implement `DerivablePayloadSizer` beside the codec. The same shape definitions
must drive exact encoding and sizing; no empirical compression ratios are allowed.
It accounts for initial/changed environments, count/list prefixes, sparse record gaps,
transactions, full exceptions, unchanged terminal/entries/groups, and native
message framing/semantics.

Replace the old full-block item prefix accounting for v3. The activity index needs
a composable range summary containing block count, first/last record positions,
encoded record data lengths, gap-prefix lengths, active-environment transitions,
and transaction activity. On
re-anchoring select the environment active at the new first position and adjust
the first record's gap; when a historical endpoint becomes
the terminal remove its ordinary representation and add its unchanged full
terminal representation. Simply subtracting old encoded-byte prefix sums is wrong.
Index/cache identity includes format, profile, initial environment digest and anchor/hash.
Summaries carry the environment at both ends of each composed range. If an
environment-change position is promoted to terminal, remove that ordinary record;
the next batch's explicit initial environment is authoritative. Neither promotion
nor re-anchoring may delay an environment change in execution by one block.
Reorgs rewind to matching ancestry; configuration changes invalidate summaries.

Use exact completed-record sizes and conservative bounds for unknown future
inputs. Derived ordinary blocks no longer reserve unknown output-root/bloom bytes.
Full exceptions and the future terminal still use full-block bounds. Account for
integer/list-prefix growth and explicit exceptions when they are actually possible.
Known supported builder inputs use the derived/changed-environment bound; do not
reserve an imaginary full header for every future empty block. If eligibility
cannot be established, use the conservative full bound and label it as such. Check the actual final encoding before committing a
newly built block/candidate; retain overflow transactions and nonce dependencies.
Keep active/future-fork blob maxima and companion consumption checks.

Update both ordinary pool admission/interval guards and final candidate sizing.
Preserve legal empty Sync boundaries used to split retained history. Include
logical block ceilings, signer capabilities and exact request-resource estimates
in feasible-prefix selection. A pending prefix must have a legal endpoint before
any applicable hard limit; reserve through that boundary. Add an explicit hard
resource due reason (`proof_window_capacity` / `proving_resource_capacity`) at the
last feasible legal boundary, independent of elective interval/fullness/count
triggers. Otherwise sequencing could stop at the cap while no existing due rule
requests a post. This reason drains only legal history and retains existing
cross-chain target/ownership rules. Test an idle chain with the elective count
threshold above the signer cap and a non-divisible K residue to exclude deadlock. If no single legal
interval fits, reject startup/admission with a specific error rather than accepting
history that cannot settle. Existing incompatible retained history must be kept
and reported, never truncated.

Local resource failure may split ordinary history only at existing legal boundaries
and cannot split a cross-chain group. Missing/stale capability data pauses dependent
admission until refreshed. A lowered signer limit is detected before dispatch;
already accepted history drains through feasible historical prefixes where possible.

## 7. Defaults and the meaning of “large enough”

| Limit | Planned default / behavior |
|---|---|
| Signer total blocks per request | **65,536**, replacing 512; operator override retained. |
| V3 format maximum total blocks | **65,536**, enforced by every decoder, independent of local operational caps. |
| Aggregate Prove/Prepare request bytes | Existing **512 MiB**; references are charged by their encoded request size, cached data remains bounded separately. |
| Aggregate streamed witness-array items | Existing **1,000,000** per request; every cold block is validated, across as many bounded preparations as required. |
| Effective server message maximum | Existing **256 MiB**, or a smaller request limit; client honors the advertised maximum instead of assuming its current 1 GiB setting is accepted. |
| Stream idle / total request deadline | Existing **120 / 600 seconds**; target-slot deadline takes precedence for final posting. |
| Preparation chunk | At most **64 blocks**, further reduced by byte/item limits. |
| Validation concurrency | One worker, terminal priority between blocks; no concurrent unbounded warmup. |
| Signer cached facts | At most **131,072 entries / 512 MiB accounted memory**. |

These are implementation decisions. Validation must show they meet the targeted
empty/light-history workloads. If the acceptance workload cannot be prepared or
proved under these limits, the release gate fails: fix the implementation or amend
the explicit, reviewed resource envelope and rerun tests. Do not quietly claim
65,536 usable blocks after testing only header admission.

Signer reference resolution remains bounded by the same total-block maximum;
preparing blocks in several calls must not bypass the final request's count cap.
On-chain follower acceptance uses format/consensus limits, not another operator's
RPC/cache settings. A follower with insufficient local resources reports inability
to sync, not a different validity rule for an otherwise valid batch.

Existing ordinary time/count posting triggers and speculative-depth configuration
remain effective. Document configurations that allow the intended multi-hour
history and validate their consistency with the signer and legal grid. Do not
silently increase latency settings or disable cross-chain due rules.

### 7.1 Posting cost, retained state and recovery envelope

Admission uses a measured upper-bound cost model with separately recorded fixed
request/KZG cost, range overhead, per-logical-block fact checks, recipe-record
checks, cold validation, terminal execution/observations, and waiting for one
already-running preparation block. The configured final posting budget covers
the whole request, not only terminal EVM execution. Unknown measurements require
a bounded calibration/readiness path, not optimistic acceptance of a huge window.
The 4-second/200-ms examples in `timing.rs` are examples/test profiles, not asserted
live budgets. Record the selected release settings and hardware in every run.

`EEZ_PROOF_TIME_MS` and `EEZ_PREPARATION_TIME_MS` may be explicitly calibrated for
the release deployment, while preserving all timing validation and submission
slack. Report resulting Future/Live slot counts and throughput tradeoffs; do not
silently alter them as an encoding side effect. The same configuration is used for
before/after semantic comparisons. If a whole legal window cannot meet its actual
deadline, select a smaller legal prefix or improve proving; a larger block cap is
not permission to miss the target.

A finite speculative depth of at least `N + K - 1` is supported; zero is not
required. Verify that state at the oldest required replay/rollback anchor and its
needed block-hash history survives the selected pruning settings, restarts and
maximum unresolved history. Durable witnesses solve signer reconstruction, not
follower state availability. Test with actual release pruning settings and report
safe-head lag and rollback exposure; do not infer safety from a depth flag alone.

The baseline release recovery SLO is 600 seconds from ready node/transport inputs
to renewed cross-chain readiness after a signer restart at the largest tested
empty/sparse backlog. It is a proposed, measured acceptance bound, not a promise
already demonstrated or a 600-second steady-state posting allowance. Measure
follower full-batch replay and crash-resume against the same 600-second baseline
on the named release hardware. A stricter deployment SLO tightens these gates.
If volatile recovery cannot meet the bound, add versioned durable validated facts
or another demonstrated recovery improvement and update the ledger before release;
do not merely label the outage a cache miss. Normal target-slot deadlines remain
unchanged by this recovery allowance.

## 8. New-network deployment

The producer, signer and follower use tag `0x03`, profile 1 from genesis. Do not
introduce an activation timestamp, a legacy writer mode, or mixed-format history.
Reject unsupported tags/profiles explicitly. Existing encoding helpers may remain
only as offline size baselines or test fixtures; they confer no production support.

All participants must share reviewed chain/fork rules and the operations profile.
Signer capabilities still establish identity and resource limits; removing upgrade
compatibility does not permit guessing limits or trusting producer execution claims.

Run a clean follower from genesis and exercise reorgs, restart, resumed settlements
and finalized pruning. These remain required on a new network. No migration from
an earlier witness schema is needed; test durable witnesses and alternate ancestry
using the new schema directly. Production deployment is a separate operational task,
after the focused soundness, behavior, capacity and recovery gates pass.

## 9. Work packages and ownership by component

| Order | Components | Deliverable / dependency |
|---|---|---|
| A | `eez-payload-codec`; proposal/golden fixtures | Exact v3 types, parser, encoder, iterator, size algebra and bounded decode; unchanged terminal/cross-chain field fixtures. |
| B | `eez-driver::derivation`; pinned Reth adapter; `eez-proof-signer/validate/stateless.rs` | Shared execution inputs and ordinary full-block reconstruction from validated outputs. Depends on A. |
| C | `eez-deriver` live/catch-up/reconciliation/correlation; `eez-l1` evidence/example tools | New block plans throughout follower and tooling; preserve terminal and L1-applied endpoint gates. Depends on B. |
| D | `eez-proof-signer/settlement/da.rs`, service/limits/config; `SPEC.md` | V3 recipe-to-window checks, terminal binding unchanged, 65,536 default, bounded count/resource failures. Depends on B. |
| E | `eez-control-rpc/proto/prove.proto`, generated modules; `eez-prover`, `eez-prover-client`, signer service/cache | Capabilities, preparation, range references/retention leases and dedicated cache misses; quota/cancellation/priority behavior. Depends on D. |
| F | `eez-node` proving/witness integration; composer proof-context construction | Durable-witness-triggered preparation, bounded retry/restart/reorg behavior, eliminate duplicate witness-window allocations. Depends on E. |
| G | `eez-composer` policy/index/sequencing/admission/encoding; `eez-node-common` payload/pool sizing | Exact v3 estimates, terminal promotion, feasible prefixes, capability checks and overflow prevention. Depends on A, E, F. |
| H | Composer/signer/follower launch configuration and deployment templates | Single-format startup, signer identity/resource checks, and new-network operator instructions. Depends on C–G. |
| I | `eez-node-e2e`, signer integration tests, CI and research artifacts | Complete equivalence/adversarial/performance evidence and operator capacity envelope. Release depends on all gates. |

Review in these bounded packages; no package may silently change the wire shape or
shared defaults. A discovered omission updates this specification and its coverage
row before implementation proceeds. Production activation remains disabled until
package I passes.

The initial requirement-to-component mapping is in the
[coverage ledger](derivable-chain-operations-coverage.md). Every entry starts as
planned; implementation must replace that status with code and evidence links.

### 9.1 Early feasibility checkpoint before proving-RPC expansion

After A–D, build a test-only v3 writer and exercise the existing cold `Prove` path.
This is possible before the production capability/cache integration. Run independent
replay and cold-proof timing at the legal span targets, including the largest,
under the intended release configurations. This checkpoint must precede E/F work.
A bounded byte/item rejection is evidence that the cold path is infeasible; do not
allocate an oversized window or raise quotas merely to force a timing baseline.
It establishes actual costs and avoids designing a proving service from byte
counts alone.

If cold proving demonstrably meets every long-span deadline/resource/recovery gate,
E/F cache/RPC work may be omitted only through an explicit specification/ledger
amendment with that evidence and an equally explicit composer/signer limit agreement.
Otherwise E/F remains required. Do not reduce the promised tested history merely
to avoid implementing a necessary fast path. The cache is a performance option;
its safety rules are mandatory whenever that option is implemented.

## 10. Correctness and failure coverage

| Area | Required cases and assertions |
|---|---|
| Codec | Empty ordinary range; one/65,535 ordinary blocks; reject 65,536 ordinary blocks; RLP prefix boundaries; malformed/truncated/overflow inputs; unknown tags/profile/kind; gaps past end; extra records; wrong terminal count; signed transaction byte identity. |
| Scope preservation | Compare encoded terminal, entries, group sizes and outer semantic suffix byte-for-byte before/after. Verify the same physical packing rules and public-input/KZG binding. |
| Derived execution | Empty/nonempty/reverted blocks; legacy/2930/1559/7702 and every other currently accepted transaction type; block boundaries and ordering; zero and changed environment; fallback before/after derived blocks; contract reads of NUMBER/TIMESTAMP/COINBASE/PREVRANDAO/GASLIMIT/BASEFEE/BLOCKHASH; pre/post-execution system transitions. |
| Forks | All currently supported post-Merge L2 configurations, first Shanghai/Cancun/Prague/Osaka blocks where applicable, supported boundary crossing, optional field presence, future unsupported profile transition uses full records or rejects. |
| Signer tampering | Valid txs with altered environment/count/gap; header outputs differing from assembly; wrong parent/config; system sender hidden as ordinary; missing transactions; wrong terminal/entry binding; no attestation on any mismatch. |
| Settlement | Full, partial, deferred, reverted, resumed and competing same-L1-block paths; canonical applied-prefix endpoint, outbound event gate and value effects unchanged. Include terminal-only and fallback ranges. |
| Cache trust | Wrong block/height/parent/environment/validation identity; stale fork, eviction before/after pinning, corrupt/missing entries, reused reference in incompatible window; cold and cached results identical. Cache cannot authorize system transactions or terminal observations. |
| Resources | Every quota boundary; count checked before expansion; Prepare chunks cannot evade final count; cache accounting includes overhead; cancellation/timeout releases memory/permits; cold validation and preparation cannot starve final proof. |
| Persistence/reorg | Node/signer restarts with pruned parent state; rebuild from durable witnesses; witness hash mismatch; no purge above finalized floor; L1 rollback and alternate L2 ancestry; reader catch-up without producer RPC. |
| Sizing | Encoded size equals estimate for completed candidates; conservative future reservation; new first gap on re-anchor; ordinary-to-terminal promotion; historical splits; all RLP thresholds; next-nonce preservation and individually unpostable transactions. |
| Operational limits | Lower signer/operator limits, capability unavailability/reconnect/change, stale target slot, missing witness, disk/cache full; retain owned sources and accepted history with explicit bounded recovery. |
| New-network format | Tag/profile mismatch rejection; signer identity/resource mismatch prevents launch; no activation configuration; clean follower from genesis; unchanged cross-chain byte fixtures. |

Run affected unit/integration tests, the existing native settlement/cross-chain
suite on Ethereum and Gnosis profiles, all-target Clippy, formatting and whitespace
checks. Unit round trips do not substitute for independent follower execution.
Use property tests for record/gap size composition and parser bounds and fuzz the
new parser/iterator, including maximal declared runs with minimal input.

## 11. Performance gates and evidence

Use fixed chain configuration and immutable transaction inputs. Compare old full
blocks, the new recipe with cold proof validation, and the prepared-reference path.
Measure physical blob counts from actual final encodings, not corpus-wide sums.

Required logical-count targets: **512, 1,024, 4,096, 10,000, 43,200 and
65,536 total blocks**. Exact format/admission boundaries are tested directly.
For native posts, use the greatest existing legal empty Sync endpoint at or below
each target, preserving the scheduler's actual grid residue and anchor. Record
both the requested ceiling and achieved count. When anchor and endpoint share a
residue, the largest span is `K * floor(ceiling / K)`: 65,532 at K=6 and 65,535
at K=5. Nonzero genesis offsets and resumed ranges use the existing placement
rules. Never change the Sync grid merely to obtain a round-number benchmark.
For each, exercise empty history and a sparse deterministic transfer workload that
fits the same six-blob example allowance; also run boundary-filling and high-entropy
data workloads whose expected result may be a smaller feasible prefix. Do not
claim every workload should fit 65,536 blocks. Account for the unchanged terminal
and semantic bytes explicitly.

Required measurable outcomes:

1. The greatest legal native empty-history spans under the 43,200 and 65,536
   targets encode within the actual allowed blob count, with no per-empty-block
   header bytes. They independently replay to exact original block hashes.
   Also round-trip an exact 65,536-block codec fixture without requiring it to
   be an off-grid native settlement.
2. The signer accepts the fully valid largest legal native window with the
   planned defaults using bounded preparation. Separate ingress tests accept
   the exact 65,536 declared count and reject 65,537, without equating structural
   admission with successful execution of an arbitrary window. The
   terminal still receives full fresh validation.
3. Prepared-path final proof plus remaining preparation fits the configured
   submission deadline for both native L1 profiles in the test environment; record
   at least 20 warm repetitions, median/p95/max and misses. All repetitions must
   fit the release profile's deadline; no fixed hardware-independent seconds are
   invented in this plan. Cold recovery and follower crash-resume must meet §7.1
   separately; a warm pass cannot substitute for either recovery gate.
4. Existing cross-chain latency/deadline tests remain passing under concurrent
   background preparation. No group changes target, order or semantic bytes to
   achieve the timing result.
5. Record peak composer/signer/follower RSS, cache-accounted bytes, witness DB
   growth, request bytes/items, disk reads and cache hit/miss rates. Cache and
   request quotas must hold under adversarial and cancellation cases; the report
   specifies the minimum tested machine resources and configuration.
6. Steady-state block accounting/preparation processes new work incrementally;
   no full-history rescans each L2 tick or full witness copies at each proof retry.
   Document catch-up/reorg costs separately.

Emit structured measurements keyed by anchor hash, candidate terminal hash and,
after dispatch, postBatch hash. Include payload/profile versions, ordinary total,
derived-empty/nonempty/full-exception counts, unchanged terminal/entry/semantic
bytes, encoded stream bytes, actual blobs, cache hit/miss counts, witness request
bytes/items, preparation/proof/replay times, and the precise limiting constraint.
Do not count retries as additional settled capacity. Never log witness contents,
private keys or full user transaction data merely to measure their lengths.

If any required gate fails, the implementation is not complete. Resolve the
failure without reducing scope, weakening verification, dropping cross-chain tests
or silently lowering the claimed supported empty-history span. Any proposed
resource/default or architecture amendment is recorded and revalidated explicitly.

Artifacts include repository/dependency/binary identities, chain/runtime configuration,
input hashes, per-block replay comparisons, receipt/commitment checks, byte tables,
timing/memory logs and exact reproduction commands. The final operator guide
separates the 65,536 format/default ceiling from measured transaction-volume and
hardware-dependent capacity.

## 12. Definition of done

- The signer soundness obligations in §1.1 have a complete gate mapping,
  adversarial attester-boundary evidence and change-specific soundness review,
  with no unresolved soundness findings.
- Every normative rule above has an implementation and test/evidence link in a
  checked-in coverage ledger; no unassigned design questions or missing rows.
- New on-chain data differs only within `ChainOperation.operations` and consequent
  carrier lengths/commitments. All original blocks and cross-chain results agree.
- Full history is derivable by an independent follower from authenticated inputs;
  no local signer cache or sequencer header service becomes a DA dependency.
- Signer defaults, preparation, admission and posting deadlines support the tested
  long-empty-history spans without relocating cross-chain effects or accepting
  unbounded work.
- Single-format startup, partial settlement, reorg, restart and finalized pruning
  are verified; no live deployment is implied by finishing the code.

This plan removes unowned implementation decisions. It does not assert that
untested code has no defects; the release gates are how the remaining empirical
questions are resolved before the change is considered complete.
