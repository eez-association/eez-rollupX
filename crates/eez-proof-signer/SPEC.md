# EEZ proof signer specification

Status: normative current profile

This document specifies the behavior of `eez-proof-signer` for the currently
supported single-rollup profile. It is intentionally narrower than the complete
EEZ protocol.

The protocol source used by this profile is the `eez-core-protocol`
submodule at commit
`855fe0602484750861b4c31502a1a91355d33ed6`. Stateless execution uses the
[EEZ Stateless fork](https://github.com/eez-association/stateless/tree/0bda6b200c1eb4689c147c080c3c248b50be2ea8),
pinned to commit `0bda6b200c1eb4689c147c080c3c248b50be2ea8`. Its
[recovered-block and checkpoint APIs](https://github.com/eez-association/stateless/tree/0bda6b200c1eb4689c147c080c3c248b50be2ea8#eez-reth-compatibility-branch) preserve the transaction
checkpoints required by this profile; the root lockfile selects the runtime
dependency graph. This dependency port does not extend the admitted proof profile.

The words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative. Solidity
defines on-chain behavior. This document defines the additional conditions the
signer applies before producing an attestation. If the pinned Solidity ABI or
hash construction changes, the signer, shared Rust mirrors, vectors, and this
document MUST be updated atomically.

For Composer request construction, response validation, and recovery, see the
[Composer-to-prover RPC specification](../eez-control-rpc/SPEC.md).

Partial L1 consumption does not relax this proof profile. The signer validates
the full proposal and the original native-blob commitments. Canonical prefix
selection and signed-user recovery belong to the deriver and Composer, as
specified in [settlement ownership](../eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission).

## 1. Purpose and supported profile

The signer accepts one Composer-supplied L2 block window, validates every block
with witness-backed Stateless/Reth execution, binds one canonical
`postAndVerifyBatch` payload to the validated execution, recomputes the one
supported public-input hash, and signs that hash.

The only supported mutable-entry sequence is:

```text
anchor, outbound*, (inbound-effect | inbound-lookup)*
```

where:

- the anchor commits the validated L2 state transition before settlement
  effects;
- each outbound effect represents one mutable L2-origin settlement host; flat
  sibling roots may share a host, callback-bearing roots use separate hosts,
  and calls rolled back by a source frame plus a subsequent retry reuse their
  original host;
  and
- each inbound effect represents one successful, source-committed mutable
  L1-origin call tree; and
- each inbound lookup represents a failed destination call or a call whose L1
  source frame later reverts. It retains the source-visible outcome but has no
  L2 effect.

Top-level L1-origin static calls are carried separately in `staticEntries` and
are pinned at their exact FIFO positions relative to mutable entries.

The profile supports exactly one nonzero rollup ID, one nonzero proof-system
address, and one nonzero proof-system verification key. It supports nested and
reentrant calls alternating between L1 and that L2, flat multi-root outbound
transactions, terminal failed top-level calls, terminal static calls,
successful top-level static reads whose only descendants are terminal,
successful static callbacks to the reading chain, and terminal failed nested
rows with no descendants. Calls rolled back inside a recovered source-chain
frame are represented by canonical, non-overlapping `revertNextNCalls` windows.
It does not support failed calls with descendants, any other static call with
descendants, explicit call gas, overlapping or out-of-bounds rollback windows,
L1 block binding, sender binding, other L2 targets, or multiple proof systems.

No failed stage may produce a signature. A successful signature authorizes the
locally recomputed public-input hash only under all rules in this document.

### 1.1 What the signature does not establish

The signer independently authenticates the accepted parent, live-anchor seed,
registry root and target L1 context through its configured canonical source.
The reads are checked against the named canonical L1 block before replay.
Signing does not query L1 again or require that block to remain the tip. The
on-chain latest-context binding enforces the exact parent hash and inclusion
timestamp. Authentication is not a finality or future-inclusion guarantee.

The signature does **not** establish:

- future canonicality, finality, or sequencer authorization;
- future on-chain applicability of a root update;
- successful future execution of an L1 target call;
- the results of an L1-origin static read's callbacks to L1. Replay answers
  them with their claimed results (section 11); L1 executes them when it
  resolves the static entry and rejects results that differ from the entry's
  static rolling hash;
- proof-system registration, threshold configuration, or verification-key
  equality in the deployed rollup manager;
- code identity at the fixed EEZL2 address;
- immediate-versus-queued dispatch, eventual execution, or availability of
  every proved entry;
- the contents of `proofs`;
- a standalone on-chain commitment to the RPC window bounds, block hashes,
  block RLP, witnesses, or transaction checkpoints; or
- caller authentication or transport confidentiality.

Latest-context binding prevents reusing the signature under a different L1
parent/timestamp. It does not add transaction-scoped wrapper authorization or
replace the pinned registry's consumption rules.

Operators MUST supply those guarantees separately where required.

The window data in the previous list is nevertheless security-critical input
to the signer's decision: it is validated and used to authorize the entries
and DA bytes that are committed by the final digest. The distinction is that a
verifier cannot recover or independently inspect that execution evidence from
the signature alone.

### 1.2 Scheduling boundary

`immediateEntryCount` and `immediateStaticEntryCount` are unproven dispatch
parameters in the pinned protocol. They are absent from the public-input hash.

Before signing, this implementation MUST require:

```text
immediateEntryCount == number of leading entries with proxyEntryHash == 0
immediateStaticEntryCount == 0
```

This is an admission policy, not a cryptographic commitment. Changing either
count after signing does not change the signed digest. Verifiers and operators
MUST NOT infer immediate or queued execution from the signature alone.

The pinned contract intentionally lets the poster select the dispatch path for
eligible entries beyond the mandatory complete leading zero-proxy run. A
deployment that requires dispatch integrity MUST add an independent binding or
submission policy.

[`docs/unbound-scheduling-counts-assessment.md`](docs/unbound-scheduling-counts-assessment.md)
records the accepted protocol semantics and the mutation evidence behind this
boundary.

## 2. Authority and data provenance

Values are classified as follows:

| Source | Authority and required treatment |
| --- | --- |
| Operator configuration | Authoritative for execution-chain rules, expected rollup ID, proof-system address and vkey, attester identity and resource limits. The deployment MUST use the fixed EEZL2 and native system addresses. System reconstruction has no private-key input. |
| Composer stream | Untrusted, including range, hashes, RLP, witness, settlement calldata, and claimed public-input hash. |
| Stateless/Reth output | Security-critical derived evidence. It MUST be consumed and checked against the corresponding admitted blocks before settlement uses it. |
| Pinned EEZ contracts | Authoritative for on-chain ABI layouts, selectors, hashes, and execution behavior. |
| `eez-protocol` mirrors | Shared local implementations used by the signer; they MUST match the pinned contracts and independent vectors. |
| Configured L1 source | Supplies canonical headers, accepted settlement occurrences and registry state at the named L1 parent. It is operator-trusted, never selected by Composer. |
| L2 execution state | Supplied as witnesses and independently replayed; no live L2 RPC or full L2 database is required. |

Names in code and diagnostics SHOULD preserve this distinction. Composer values
SHOULD be described as `claimed`, `declared`, or `submitted`; replay results as
`computed`, `recomputed`, or `validated`; and values that passed a complete
direction-specific gate as `authorized`.

## 3. Startup configuration

Every option is available as a CLI flag with the listed environment fallback.
The CLI value takes precedence.

| CLI flag | Environment variable | Default | Requirement |
| --- | --- | --- | --- |
| `--listen-addr` | `EEZ_PROOF_SIGNER_ADDR` | `127.0.0.1:50061` | Valid socket address. |
| `--chain-config` | `EEZ_CHAIN_CONFIG` | none | Path to the complete, strict L2 `Genesis` document. |
| `--l1-chain-config` | `EEZ_L1_CHAIN_CONFIG` | none | Required trusted L1 profile: `mainnet`, `hoodi`, `sepolia`, `chiado`, or a Genesis/ChainConfig path. |
| `--l1-rpc-url` | `EEZ_L1_RPC_URL` | none | Required operator-trusted HTTP(S) canonical L1 source; credentials are redacted. |
| `--registry-address` | `EEZ_REGISTRY_ADDRESS` | none | Required nonzero deployed EEZ registry. |
| `--rollup-id` | `EEZ_ROLLUP_ID` | none | Nonzero L1 registry rollup ID. This is not the L2 EIP-155 chain ID. |
| `--vkey` | `EEZ_VKEY` | none | Nonzero 32-byte proof-system vkey. |
| `--signer-key` | `EEZ_PROOF_SIGNER_KEY` | none | Valid secp256k1 attestation private key. |
| `--attester-address` | `EEZ_ATTESTER_ADDRESS` | none | MUST equal the address derived from `--signer-key`. |
| `--l2-system-address` | `EEZ_L2_SYSTEM_ADDRESS` | `0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0076` | MUST equal the fixed native system address embedded in EEZL2. |
| `--proof-system` | `EEZ_PROOF_SYSTEM` | none | Nonzero expected proof-system contract address. |
| `--max-request-blocks` | `EEZ_PROOF_SIGNER_MAX_REQUEST_BLOCKS` | `65536` | Nonzero; lower explicit limits remain effective. |
| `--max-request-bytes` | `EEZ_PROOF_SIGNER_MAX_REQUEST_BYTES` | `536870912` | Nonzero. |
| `--max-request-witness-items` | `EEZ_PROOF_SIGNER_MAX_REQUEST_WITNESS_ITEMS` | `1000000` | Nonzero. |
| `--stream-idle-timeout-secs` | `EEZ_PROOF_SIGNER_STREAM_IDLE_TIMEOUT_SECS` | `120` | Nonzero; representable as an `Instant` deadline and u64 milliseconds. |
| `--request-timeout-secs` | `EEZ_PROOF_SIGNER_REQUEST_TIMEOUT_SECS` | `600` | Nonzero; representable as an `Instant` deadline and u64 milliseconds. |

Private-key syntax errors MUST be reported without echoing the supplied secret.
Debug formatting MUST NOT reveal private keys.

The attestation key MUST NOT derive `SYSTEM_ADDRESS`. Attestation authorizes
validated settlement; native reconstruction is keyless and MUST NOT load or
require a system private key. The removed `--l2-system-key` flag is rejected.

The chain document MUST contain an explicit `chainId`. Unknown supported-level
and extension fields MUST be rejected rather than silently ignored. A complete
Genesis document is required for L2; a bare `ChainConfig` is rejected. The
L2 loader checks the fixed Osaka-from-genesis profile, 30,000,000 gas limit,
1 gwei initial base fee, and canonical EIP-2935 history predeploy. It applies
RollupX's fixed EIP-1559 elasticity 2 and denominator 50 to that same document.
These rules do not change the separately configured L1 profile. Duplicate keys, including
escaped aliases and nested objects, MUST be rejected before deserialization.
The loader derives the canonical document digest and execution rules from the
same read. The digest precedes spelling normalization/defaults; see the
[identity contract](../eez-control-rpc/SPEC.md#21-validation-identity-and-capability-discovery).


The active deployment bindings are:

```text
SYSTEM_ADDRESS = 0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0076
EEZL2_ADDRESS  = 0x4B2760F84B83d6b3829E30A7573B0172B6d6402C
```

The signer rejects a noncanonical `EEZ_L2_SYSTEM_ADDRESS`. Matching EEZL2's
immutable system address to this fixed value remains a genesis/deployment
invariant; the signer does not independently query deployed code.

`EEZL2_ADDRESS` is an upgradeable `EEZProxy` genesis predeploy, as in the pinned
protocol's `deployment/Deploy.s.sol`. Its genesis implementation is
`0x4C3D971b1079E58843D0E1af155A2e54b9E54dbA` and its ProxyAdmin is
`CREATE(EEZL2_ADDRESS, 1)`, owned by the deployment's L2 upgrade owner. The proxy
holds EEZL2's storage, balance and events and deploys its cross-chain proxies,
so every rule below applies to `EEZL2_ADDRESS`. An upgrade is an ordinary L2
transaction from that owner, and execution validation uses whatever
implementation is in state: the upgrade owner is therefore trusted with the L2
manager's behaviour. An implementation that changes the ABI, events or table
semantics this profile assumes requires a coordinated Composer/signer release
before the upgrade transaction is sequenced.

Native transactions are `0x76 || rlp([chainId, nonce, to, value, input])` with
the fixed sender, EEZL2 destination and zero gas price. Their metered gas budget
is position-dependent: `min(16,777,216, block gas limit - cumulative gas used by
the preceding transactions)`, where the cap is Osaka's EIP-7825 per-transaction
limit. The budget is not reserved against the block; only metered use counts. A
budget below the transaction's intrinsic gas (including EIP-7623's floor) makes
the block invalid. Replay derives the budget from block contents alone, so the
signer's stateless execution and the Composer agree without any carried field.
The nonce and chain ID MUST match execution state/rules. Encoding contains no
signature or configurable gas fields. Ordinary Ethereum transactions retain
normal fees. Receipts retain type `0x76`. Public RPC/txpool admission MUST reject
native transactions; only independently authorized reconstruction can supply
them for settlement.

A native transaction's `value` is minted to `SYSTEM_ADDRESS` immediately before
execution and removed again if the call reverts or halts. As a pre-execution
change of every L2 block, after the EIP-2935 history write, execution sets the
balances of `EEZL2_ADDRESS` and `SYSTEM_ADDRESS` to zero; an absent or
zero-balance account is left untouched. `SYSTEM_ADDRESS` receives EEZL2's
outbound value but can spend only freshly minted value, and an EEZL2 table is
consumable only in the block that loaded it, so both balances are dead at the
next block start. Proxy recovery sweeps go to EEZL2's separate
`RECOVERY_ADDRESS` genesis immutable (the deployment's L1 poster by default)
and are not burned: they move ether already held at an undeployed proxy
address and mint nothing. Together these rules are the node-side accounting the
pinned core protocol requires for
[L2 funding and inventory](../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md#l2-funding-and-inventory):
each load's funding is minted for its own transaction, and unused or reverted
inventory is never reused across blocks. The burn is not a post-transaction
change: prefix block hashes derived from per-transaction state roots include it
through the pre-transaction state.

Validated inbound transport remains an `InboundSidecar` through reconstruction.
Conversion rejects settlement root updates, wrong routing/source, malformed
root shape and call identity. Flat rolling hashes are checked at conversion;
nested/reverted tables additionally require complementary settlement
verification. A checked shape alone never authorizes signing or value minting.

This native format and its genesis are a fresh-network compatibility boundary.
Old signed-system captures do not establish native authorization, mixed-version
nodes are unsupported, and no in-place database migration is provided. See the
[operator compatibility contract](../../docs/runbooks/chiado-deployment.md#recovery-and-upgrades).

## 4. Service and RPC behavior

The service exposes capability discovery, preparation/retention control, and
the final proving stream:

```protobuf
rpc Capabilities(CapabilitiesRequest) returns (CapabilitiesResponse);
rpc PrepareOrdinary(stream PrepareChunk) returns (PrepareResponse);
rpc RetainOrdinaryRanges(RetainOrdinaryRangesRequest)
    returns (RetainOrdinaryRangesResponse);
rpc Prove(stream ProveChunk) returns (ProveResponse);
```

One final stream contains exactly one header followed by full block/witness
chunks and/or cached ordinary ranges covering every declared position. Cached
ranges cannot cover the terminal. Its `expected_validation_identity` MUST be exactly 32 bytes and
match the local configured namespace before witness ingestion. A width error is
`INVALID_ARGUMENT`; an identity mismatch is `FAILED_PRECONDITION` with metadata
`eez-capabilities-changed: 1`, no actionable details, and no signing call.

`Capabilities` reports immutable local validation identity and effective limits
without acquiring the active proof slot. It never constructs an attestable digest
or calls the signer. Preparation reports a 64-block ceiling and releases the
shared validation worker after each block so a final proof can proceed at a block
boundary. It cannot reach settlement or the attester. Fully validated facts are
held in a volatile cache bounded to 131,072 entries and 512 MiB of accounted
block/evidence data. Final proving rechecks continuity, state-root telescoping,
recipe/DA equality and the intermediate gate, and always validates a fresh
terminal witness. Preparation creates/extends an opaque reservation and final
proof replaces it with the exact ordinary range. At most eight leases are active,
so rapid crash/restart sequences remain live while abandoned reservations expire;
overlap is refcounted, expiry only makes facts evictable, and cloned final facts
outlive lease expiry. The client reduces each preparation group below 64 when
the negotiated aggregate-byte, per-message, or witness-item quota requires it;
one individually oversized block fails explicitly. An expired or restart-lost
lease renewal/release clears only client resource hints and triggers durable
reconstruction. The producer renews the exact retained ranges every 30 seconds;
final proving and renewal serialize with preparation state changes.
The producer now feeds a bounded 128-identity queue from
the durable witness store and retains only an exact latest catch-up watermark on
overflow; once overflow exists, every newer endpoint replaces that watermark
until consumed. Startup submits the persisted canonical head through capture so
an existing backlog prewarms without waiting for another block. Full lifecycle/native
recovery evidence remains open.

### 4.1 Single-flight execution

The daemon initializes the codec's default KZG settings before opening its RPC
listener. This process-wide initialization runs in one awaited blocking worker;
failure prevents readiness. It creates no request, validation result, or
attestation, and changes neither the trusted setup nor any per-request check.
Initialization time belongs to startup, outside Composer's per-window timing
samples. No calibration postBatch is required. This startup behavior is wire
compatible with older Composers; rollback moves the cost back to the first
request without changing historical decoding or follower replay.

Exactly one `Prove` request may be admitted at a time. A second request MUST be
rejected immediately with `Unavailable`; it MUST NOT wait while holding an
open stream.

The active-request slot remains held through validation, settlement, signing,
and response construction. If a blocking worker outlives its RPC future, the
slot remains held until that worker exits. Graceful shutdown stops accepting
new connections and waits for the slot to become idle.

### 4.2 Message and request limits

The maximum decoded request-message size is:

```text
min(max_request_bytes, 256 MiB)
```

The maximum response-message size is 1024 bytes.

The aggregate byte quota is the sum of `prost::Message::encoded_len()` for
decoded known fields of every header and block chunk, including fields later
discarded. Integer overflow MUST reject the request. This quota does not undo
allocations made while decoding the current message; the per-message limit
bounds that stage.

The witness-item quota counts all `state`, `codes`, `keys`, and `headers`
elements across the request. The block quota covers the inclusive declared
range.

### 4.3 Deadlines and cancellation

The stream idle timeout applies independently to every wait for a message or
EOF. The request timeout is one absolute deadline covering ingestion,
validation, settlement, signing, and response construction.

CPU-bound validation and settlement run in one blocking worker. A worker that
has not started MAY be aborted. Running EVM execution is non-interruptible; a
running worker observes cooperative cancellation before block executions and
at selected settlement phase boundaries. A timed-out or disconnected request
MUST NOT release the active slot while its worker is still running.

If the absolute deadline expires during signing, the signature MUST NOT be
returned even if local signing completed.

`PrepareOrdinary` uses the same absolute request bound from its first header
through response construction, including every chunk/EOF read, shared-worker
wait, validation, and cache publication. Progress MUST NOT reset that deadline;
the per-message idle timeout remains independent. An ingestion timeout releases
the preparation slot without executing, publishing, or signing. A running
validation worker retains its shared-worker permit until it exits.

Cache publication runs off the async executor and retains the preparation slot
until its worker exits. After acquiring the cache lock, it MUST reject an expired
or cancelled request before starting the atomic update. An update already admitted
under that lock may finish atomically after cancellation/deadline expiry; the RPC
MUST NOT return a successful response after its deadline. An unacknowledged lease
remains bounded by the normal cache capacity and lease expiry rules.

Enforcing these existing time bounds changes no wire fields, validation identity,
genesis, or database format. Existing clients remain compatible; a signer upgrade
needs no coordinated node upgrade or migration. Rollback restores the old timeout
behavior without changing historical decoding or follower replay.

## 5. Stream admission

Admission proves stream shape and quota compliance only. It does not prove
block, witness, or settlement correctness.

### 5.1 Header

The first chunk MUST be a `ProveHeader`. It MUST contain `post_batch` and
`post_batch.posting_context` and `parent_provenance`. Missing context is rejected before reading blocks.
Before execution, its chain ID MUST match the trusted L1 profile, and its target
timestamp MUST select active blob rules. There is no legacy blob-limit fallback.

The header is admitted only if:

- `rollup_id` equals the operator-configured expected rollup ID;
- `from_block != 0`;
- `from_block <= to_block`;
- the inclusive span `to_block - from_block + 1` is representable and no larger
  than `max_request_blocks`; and
- the posting context contains a nonzero chain ID and timestamp, a nonzero
  32-byte parent hash, and a parent number below `uint64.max`.

PostBatch fields 2 and 3 (the claimed hash and optional block hash) are removed
and reserved. The required context is a claim until independently authenticated.
`post_batch.abi_calldata` and the physical `post_batch.blobs` are retained for
settlement validation.

The provenance carries the exact canonical RLP of the L2 parent, its containing
acceptance height (zero only for genesis), and the complete ordered list of
accepted endpoint headers from the canonical L1 block at the parent's timestamp.
Each header is limited to 1024 bytes before decoding; exact RLP round-trip and
exhaustion are required. Headers count against aggregate witness items as well
as the protobuf byte quota. Decoding confers no acceptance authority.

Under the existing request permit and deadline, the signer authenticates the
finalized genesis reference, configured genesis or parent acceptance through the
shared complete settlement-attribution scanner, named canonical L1 parent, and
registry state at that exact target hash. Canonical timestamps locate at most
one possible live slot. Every accepted endpoint in that slot must match the supplied list;
an earlier live variant refreshes descendants even if the parent was later
accepted as catch-up. The target hash is checked at its canonical height both
before and after these reads; normal head extension does not invalidate it.
Source unavailability returns `Unavailable`; a replaced target during
authentication returns `Aborted`.
Neither error carries an eviction hint.

The genesis reference is authenticated when its exact header is canonical and
the source's `finalized` block is at or above its height. A reference at L1
block zero is final by definition and only has to match the canonical block
zero, so a fresh private devnet can be served before its first finalized
checkpoint. Deployment permits that reference only on private networks.

All replayed headers, including prepared cache hits, must extend the authenticated
parent and carry its independently established descendant seed. The registry
root must equal that parent or a terminal prefix independently bound by execution
and settlement gates. No registration-to-tip or lifetime `RootUpdated` scan is
performed. Operator configuration establishes the genesis trust root; a
non-genesis parent still requires an actual accepted settlement occurrence.
A manual root assignment alone cannot authorize a parent or bypass the registry
root comparison. This does not implement node recovery from owner resets.

After blocking execution and observation replay finish, the signer constructs
an attestable digest without another L1 read. A head change or RPC outage after
authentication does not invalidate completed work; the returned signature may
already be unusable for inclusion, which the on-chain context check enforces.
The request deadline, exclusive permit and all execution/settlement gates remain
in force. Startup and `Capabilities` do not require a ready RPC or grant signing
authority.

### 5.2 Blocks

After the header, every chunk MUST be a `BlockWitness`; duplicate headers and
empty chunk kinds MUST be rejected.

For block index `i`, admission MUST require:

- `number == from_block + i`;
- `hash` and `parent_hash` are exactly 32 bytes;
- a witness is present;
- the aggregate quotas remain within limits; and
- for `i > 0`, the claimed parent hash equals the preceding claimed block hash.

EOF is accepted only after exactly the declared number of blocks. Extra,
missing, duplicated, or reordered blocks MUST be rejected.

The first block's parent is not compared with a trusted external anchor during
admission.

## 6. Execution validation

The production backend is always the pinned in-process Stateless/Reth backend.
The operator-configured chain document determines fork rules and the EIP-155
chain ID; the Composer cannot override either.

### 6.1 Decode and identity binding

For every admitted block, the backend MUST:

1. exact-decode the supplied consensus RLP with no trailing bytes;
2. match the decoded number and parent hash to the admitted claims;
3. recompute the block-header hash and match the admitted hash; and
4. recover every transaction signer using the configured fork rules, including
   the Homestead low-`s` rule when active.

Signer recovery alone is not execution validation.

### 6.2 Local checkpoint plan

Only the terminal block may request transaction-state checkpoints. For each
recovered terminal transaction define:

```text
system[i] = tx[i].type == 0x76
            && signer(tx[i]) == SYSTEM_ADDRESS
            && tx[i].to == EEZL2_ADDRESS
```

Let `p` be the first native transaction position (or the transaction count if
there are none). Ordinary positions before `p` are not action candidates. The
locally selected action checkpoint positions are:

```text
C = { i >= p | !system[i] || system[i + 1] || i is the last transaction }
```

An absent `system[i + 1]` at block end is treated as `true`. Thus an outbound
`[system-load, user]` pair ends at the user transaction, while a standalone
inbound system transaction ends at itself.

The Composer MUST NOT nominate checkpoint positions. The complete plan MUST be
derived before earlier blocks are replayed. The backend MUST return exactly the
requested positions in strict order. The replay plan additionally selects
`p - 1` when the ordinary prefix is nonempty and captures the state immediately
after pre-execution changes. This independently derives B[0], even for an empty
ordinary prefix. It is kept separate from action checkpoints. Preceding blocks
MUST return no settlement checkpoints.

### 6.3 Stateless replay guarantees

For every block, successful Stateless validation MUST establish against the
supplied witness and configured chain rules:

- consensus/header validity;
- executable witness-backed pre-state;
- transaction execution and receipt results;
- the computed block hash;
- the computed post-state root and its header commitment; and
- any selected post-transaction state roots.

The pre-state root of each block after the first MUST equal the preceding
block's computed post-state root. This state-root telescope is an internal
execution-consistency check; state roots are not settlement commitments.

For every selected terminal transaction boundary, the signer MUST construct a
canonical prefix block header from the exact validated block, replacing the
transaction root, receipt root, logs bloom, gas used, state root, and blob gas
used with values independently derived for that prefix. The resulting header
hash is the effect checkpoint. A final prefix MUST equal the validated full
block hash. Prefix commitments MUST reject nonempty withdrawals, ommers, or
EIP-7685 requests until their per-prefix post-block semantics are represented.

#### Supported prefix-commitment block profile

These restrictions apply to every L2 block, including empty ordinary blocks
and terminal blocks without effect-prefix checkpoints. The exact decoding and
receipt/state-root checks remain required.

- Osaka MUST be active from genesis; a later execution fork needs an explicit
  L2 lifecycle and activation rule before it can be enabled.
- The ommer list, difficulty and nonce MUST be empty or zero as applicable.
- The block gas limit MUST be 30,000,000. Ordinary and native transactions
  consume the same budget. The base fee uses the actual parent's gas usage
  with EIP-1559 elasticity 2 and denominator 50, starting at 1 gwei in genesis.
- EIP-4895 withdrawals MUST be present and empty. These validator withdrawals
  are unrelated to EEZ bridge or outbound ETH calls.
- `parentBeaconBlockRoot` MUST be present and zero. Execution MUST NOT call
  the EIP-4788 beacon-roots contract.
- EIP-7685 requests MUST be empty, committing SHA-256 of the empty byte string.
  Execution MUST NOT extract EIP-6110 requests or run the EIP-7002/7251 calls.
- L2 blob transactions MUST be rejected before replay. `blobGasUsed` is zero;
  standard parent-derived `excessBlobGas` stays zero from genesis.
- The real EIP-2935 parent-hash update MUST run before the first transaction in
  every block, including an empty block. Reusable prefix checkpoints include
  this update exactly once and remain before block finalization.

L1 source simulation and posting validation retain their own Ethereum/Gnosis
rules. No L2 placeholder field disables an L1 system call.

The validated window exposes:

- `window_pre_block_hash`: parent hash of the first validated block;
- `settling_pre_block_hash`: parent hash of the terminal block; and
- `window_post_block_hash`: validated hash of the terminal block.

These hashes bind ancestry and complete header commitments. They do not by
themselves prove that the first parent belongs to the live canonical chain.

### 6.4 Settlement evidence

From the validated block and receipts, the backend MUST retain:

- one receipt-success flag per transaction;
- one recovered `SYSTEM_ADDRESS` sender flag per transaction;
- every log emitted by `EEZL2_ADDRESS` whose first topic is the current
  `CrossChainCallExecuted` signature, ordered by transaction and log position;
  and
- the decoded event's `crossChainCallHash` and `callGas` when the complete log
  encoding is canonical.

A signature-matched but malformed or noncanonical event MUST remain an
observation without decoded fields. It MUST NOT be silently discarded.

Before settlement, a shared consuming check MUST bind one backend output to
each admitted block and verify block count, identities, hashes, exact RLP
transaction counts, receipt and sender-flag coverage, event coordinate bounds
and strict ordering, and checkpoint bounds/order. A backend that claims success
with malformed output is an internal failure, not an input rejection.

### 6.5 Derivable-operation reconstruction evidence

Successful Stateless replay additionally attempts
whole-block reassembly before discarding receipts and execution requests. The
parent is the exact ancestor consumed by replay, hash-bound to the child and
state-bound to the replay pre-state. Reth's pinned assembler takes the actual
execution inputs, replay receipts/requests/gas and independently recomputed
post-state root. Its complete canonical RLP MUST equal the accepted original.
The adapter forbids state-provider reads; it is not another execution backend.

Only this successful check constructs reconstruction evidence, bound to the
computed block hash at the consuming backend boundary. It retains the actual
beneficiary and extraData. Fixed fields and omitted timestamp/seed inputs remain
bound by the genesis profile, authenticated parent and canonical L1 context.
Unsupported shapes and failed reconstruction retain no such evidence; every
operations position MUST reject missing evidence. Reconstruction evidence does not authorize system transactions,
outbound observations, terminal effects, DA or signing.

## 7. Canonical batch and public-input profile

`PostBatch.abi_calldata` MUST exact-decode as the current
`postAndVerifyBatch(ProofSystemBatchPerVerificationEntries)` ABI with selector
`0xe4a480e4`. Re-encoding the decoded argument MUST reproduce every byte after
the selector. Alternate, partial, or trailing encodings MUST be rejected.

Canonical decoding establishes byte identity only; every decoded claim remains
untrusted until the corresponding gate below succeeds.

The decoded batch MUST satisfy all of these profile rules:

1. `proofSystems` contains exactly the configured proof-system address.
2. `rollupIdsWithProofSystems` contains exactly one item with the expected
   rollup ID and `proofSystemIndexes == [0]`.
3. `expectedRootPerRollup` is empty.
4. Every mutable entry has `destinationRollupId == expected_rollup_id`.
5. Every static entry has the canonical supported shape, targets the expected
   rollup, and contains exactly one pin for that rollup.
6. `immediateStaticEntryCount == 0`.
7. `immediateEntryCount` equals the complete leading run of mutable entries
   whose `proxyEntryHash == 0`.
8. `blockNumber == uint64.max` (the pinned manager's latest-context sentinel).
9. `bindMsgSenderInPublicInput == false`.

`blobIndices` is checked together with the mandatory physical DA carrier in
section 11 and MUST equal `[0, 1, ..., blob_count - 1]`.

`proofs` is not inspected and is absent from the public-input hash. The caller
is responsible for inserting the returned signature into the correct proof
carrier and satisfying the on-chain `proofSystems.length == proofs.length`
requirement.

This profile requires the pinned reference manager's latest-context behavior:
`abi.encode(uint256(target_timestamp), parent_hash)`. The same proof cannot be
included under a different parent or timestamp. Timeless and specific-block
contexts are rejected; changing the target requires a new request.

## 8. Block-hash commitment chain and effect binding

### 8.1 Commitment chain

The batch MUST contain at least one mutable entry. Every entry MUST contain
exactly one `RootUpdate` and that update's `rollupId` MUST equal the expected
rollup ID.

For entries `E[0..n)` with updates `U[0..n)`, the signer MUST require:

```text
U[0].currentRoot == window_pre_block_hash
U[i].currentRoot == U[i - 1].newRoot       for every i > 0
U[n - 1].newRoot == window_post_block_hash
```

The ABI retains the historical `RootUpdate`, `currentRoot`, and `newRoot`
names, but these `bytes32` values are block hashes in this profile. The checks
bind the continuous Composer claim to validated endpoints. Interior `newRoot`
values are bound to prefix-block hash checkpoints and their persistent
carriers below. Every static-entry pin MUST equal the commitment live at that
call's exact FIFO position.

### 8.2 Terminal-block framing

Every preceding block MUST contain neither:

- a native transaction of reserved type `0x76`;
- a transaction recovered from `SYSTEM_ADDRESS`; nor
- an observed EEZL2 outbound event.

In the terminal block:

- a transaction recovered from `SYSTEM_ADDRESS` MUST target `EEZL2_ADDRESS`;
- every such transaction MUST have a successful receipt; and
- a system-to-EEZL2 transaction beginning with selector `0xc3fb5f3d` is an
  inbound candidate even when the remainder of its calldata is malformed.

The number of effect-shaped entries after the anchor MUST equal the number of
locally derived candidate positions `C`. Lookup-only entries consume no
candidate. Effect claims and candidates are joined in order; skipping,
duplicating, or reordering either side MUST fail.

A candidate is classified from execution as:

```text
inbound  if its ending transaction is recovered from SYSTEM_ADDRESS
outbound otherwise
```

The claimed entry at the same position MUST have the same direction.

### 8.3 Entry classification

Every item in `entries` MUST be an anchor, a mutable effect, or an exact
lookup-only inbound entry. An outbound or applying inbound effect MUST be
successful. A lookup MAY report failure only as a terminal top-level call
without nested rows. A nested row MAY fail only when its call sub-array is empty and its
`revertedOrStaticRollingHash` is nonzero; successful rows MUST have a zero
reverted hash. Every flat call, including calls in row sub-arrays, MUST have
`gas == 0`. A static call MUST carry zero value and no revert span. A static
call nested in a mutable call MUST have no semantic descendants; a top-level
static read MAY have the static callbacks described in sections 10 and 11. A mutable call MAY start
a rollback window by setting `revertNextNCalls` to the number of consecutive
calls in that window, including itself. The window MUST fit in its current call
array and no later call inside it may start another window.

The leading entry is a canonical anchor only if:

- `proxyEntryHash == 0`;
- `l2ToL1Calls` is empty;
- `destinationRollupId` is the expected rollup;
- `returnData` is empty; and
- `rollingHash` equals the L1 entry seed over its sole root update and a zero
  proxy hash.

Every later entry MUST be one of:

- **outbound:** `proxyEntryHash == 0` and a nonempty top-level
  `l2ToL1Calls` array; or
- **inbound effect:** `proxyEntryHash != 0`, successful, and exactly bound to a
  replayed L2 system transaction; or
- **inbound lookup:** `proxyEntryHash != 0`, empty call arrays, a zero-value
  no-op root update, and the exact L1 seed for that proxy hash and current root.

Classification deliberately does not infer nested execution from one entry
alone. Sections 9 and 10 bind every effect to its exact complementary L2
execution table and reject extra roots, rows, or flat calls. Section 11 binds
lookup results and any source-reverted nested messages to the signed native
semantic blob; lookup entries have no complementary L2 execution table.

A second anchor or any other shape MUST be rejected.

The anchor's `etherDelta` MUST be zero. If no effects exist, the anchor alone
covers the complete window transition.

For every effect at candidate position `C[i]`, the backend checkpoint MUST
target `C[i]`. Immediate outbound commitment updates retain the following
persistent-carrier rule, because a failed immediate L1 entry would roll back
its root update together with the intentionally reverted call:

- a leading run of failed effects is carried by the successful anchor, whose
  `newRoot` equals the last checkpoint in that run;
- every successful effect carries its checkpoint and every immediately
  following failed effect checkpoint, so its `newRoot` equals the last such
  checkpoint; and
- every failed effect uses the root already persisted by that carrier, making
  its own stitched update a no-op (`currentRoot == newRoot`).

With no leading failures, the anchor's `newRoot` equals
`settling_pre_block_hash`. With no failures, every effect directly carries its
own computed prefix-block hash. Inbound lookups are not effects and never
participate in this carrier rule.

## 9. Inbound authorization

An inbound candidate is authorized only if all rules in this section pass.

### 9.1 Executed L2 transaction

The system transaction MUST have succeeded and its calldata MUST be the exact
canonical encoding of current `executeIncomingCrossChainCall`, selector
`0xc3fb5f3d`.

The decoded call MUST satisfy:

- `_entries` contains exactly one L2 mutable entry;
- `_staticEntries` is empty;
- that entry contains exactly one `incomingCall`;
- entry `success == true`;
- native transaction value equals the checked sum of every flat incoming call
  and every incoming sub-call in that entry's expected-outgoing rows;
- every `expectedOutgoingCall` and every call in its `incomingCalls` sub-array
  has the supported mutable shape from section 8.3;
- the first inner call has `sourceRollupId == 0`, and its target, value, data,
  source address, and source rollup are the canonical inbound identity; and
- that inner call has `isStatic == false` and `gas == 0`; because the entry has
  one root call, its `revertNextNCalls` MUST be either zero or one.

The signer MUST recompute the common mutable call hash with source rollup `0`
and target rollup `expected_rollup_id`. The hash MUST be nonzero and equal the
L2 entry's `proxyEntryHash`.

The complete L2 entry is retained as the target-side table. Its rolling hash,
rows, position keys, outcomes, and calls are verified recursively with the L1
source entry in section 9.2.

### 9.2 Claimed L1 entry

The corresponding batch entry MUST:

- target the expected rollup;
- have `success == true`;
- carry the recomputed nonzero call hash as `proxyEntryHash`;
- carry exactly the L2-observed root outcome; and
- pair with the executed L2 target table under the recursive verification
  below.

Starting from the L1 root-update seed and the L2 proxy seed, the signer MUST
inverse-execute both tables. It MUST consume unified rows in forward order,
require each row key to equal
`keccak256(sourceCallHash || currentDestinationRollingHash)`, apply the exact
`CALL_BEGIN`, `NESTED_BEGIN`, `CALL_END`, and `NESTED_END` transitions, and
consume every flat call and row exactly once. Both final rolling hashes and
every nested outcome MUST match. The update's `etherDelta` MUST equal the sum
of values on all L1-to-L2 calls minus the sum on all L2-to-L1 calls, within the
`int192` range.

Every L2-origin call recomputed from an inbound nested pair MUST have one
matching current six-field `EEZL2.CrossChainCallExecuted` event in the inbound
system transaction, in execution and receipt-log order. The event's `callGas`
MUST be zero, no matching event may remain unclaimed, and no L2-origin call may
use `SYSTEM_ADDRESS` as its source.

There MUST be exactly one canonical inbound candidate for every claimed inbound
effect and no unclaimed inbound candidate.

## 10. Outbound authorization

All outbound effects MUST precede all inbound effects.

An outbound candidate transaction MUST be immediately preceded by a canonical,
successful, zero-value `SYSTEM_ADDRESS` call to `loadExecutionTable` containing
one or more L2 source entries. A group MAY contain only mutable sources,
or terminal static sources interleaved with successful flat mutable roots in
one outbound host. A multi-root host containing static sources MUST cover the
complete load and user transaction; it MUST NOT carry separate callbacks,
reverted mutable roots, root revert spans, or nested mutable roots.

The load serializes mutable sources first, then static sources. Verification
MUST traverse the host's original call order, advancing the mutable cursor only
for mutable roots and matching static roots against their static source order.
Every source row MUST be accounted for exactly. Static sources use the native
L2 ABI (without L1 routing or root-pin fields). A static source MAY carry
`incomingCalls`: the callbacks to L2 made by the L1 target of the read. Each
MUST be static, with zero value, zero gas and no revert span, and a source
without callbacks MUST have a zero rolling hash. While verifying the static
root, each callback MUST resolve the host's next row, whose position key is
the callback's hash and the host rolling hash after the root's `CALL_BEGIN`
(static rows do not advance it). That row MUST be successful, with no
sub-calls and a zero reverted/static hash, and the untagged static fold of the
rows' results MUST equal the source's `rollingHash`. L2 executes the callbacks
and checks that fold, so the values L1 returns are the values L2 computes. In a
group containing static sources, host rows are allowed only for these
callbacks. Each `expectedEntryIndex` MUST equal the mutable
cursor independently reconstructed at that root in the host's call order. The
complete `(expectedEntryIndex, proxyEntryHash)` lookup key MUST have identical
success and return bytes wherever repeated. Different positions MAY carry
different results for the same hash. Local writes without successful mutable
cross-chain consumption do not advance this cursor; different results at one
position remain unsupported. Static rows do not advance the mutable cursor.
The full position-bearing static ABI MUST be reconstructed from authenticated
native-blob semantics and compared with the executed load table. An L1-shaped
projection that drops the position MUST NOT authorize signing or replay.

For mutable-only groups the existing cursor/revert verification remains in
force. Every committed mutable L2-to-L1 call recomputed from the group MUST
have one matching current six-field
`EEZL2.CrossChainCallExecuted` event at the candidate transaction, in execution
and receipt-log order. Every matching EEZL2 event in the terminal receipts
MUST be claimed exactly once; malformed, missing, reordered, or extra
observations MUST fail.

The event's `callGas` MUST be zero. The batch entry MUST:

- target the expected rollup;
- contain one or more root `l2ToL1Calls`; a multi-root host MUST be flat, while
  a one-root host MAY contain valid nested rows;
- use `proxyEntryHash == 0`;
- have `success == true`; and
- carry only calls and rows in the supported mutable shape from section 8.3.

Every L2-origin call source address MUST NOT be `SYSTEM_ADDRESS`.

The signer MUST verify each L1 host and its exact consecutive slice of the
loaded L2 source table with the same recursive inverse execution defined in
section 9.2. Each L2-origin source hash uses target rollup `0` and zero
`callGas`; mutable-call hashes MUST equal the ordered event hashes. Static roots
MUST contribute to the host rolling hash but MUST NOT require receipt events
or contribute transferred value. The update's
`etherDelta` MUST equal total L1-to-L2 value minus total L2-to-L1 value across
all roots covered by that host, within the `int192` range.

### Mixed outbound profile compatibility

The pinned feature/events protocol already changed L2 selectors, static-entry
encoding, events and proxy/runtime bytecode. This backport retains that pin and
adds keyless native transactions, direct V0 operations, the required genesis
profile and authenticated L1 context. Prove requires parent provenance and
posting context; obsolete claimed-hash fields are removed. Validation identity
schema 4 rejects mixed-version peers. Old selectors and wire formats MUST fail
closed; no historical reader or in-place contract/genesis migration is provided.

Deploy matching contracts, rendered genesis, Composer, Signer and followers as
one protocol set on a new network. Existing networks must retain their matching
binaries until a separately validated migration exists. Do not mix old and new
signers/replaying nodes, or downgrade after new-format history. Pending requests
remain in memory and restart may require resubmission. This port does not change
retry attribution, queue ownership or scheduling.

### Static-callback compatibility

Validator revision v8 accepts top-level static reads with static callbacks
(sections 10 and 11). It changes no contract, wire field, DA encoding or
storage, and historical blobs decode and replay unchanged. Peers before v8
reject these reads: an older Composer refuses them at admission, and an older
Signer or follower rejects a batch or blob that contains one. The revision bump
makes a v7/v8 Composer–Signer pair fail the identity check at startup instead.
Upgrade followers no later than the Composer and Signer, which upgrade
together. Downgrading after a static callback has settled leaves the older
followers unable to replay that history.

Upstream prose in `STATIC_ENTRY.md` and parts of the core specification still
describes hash-only matching; pinned `IEEZL2.sol`, `EEZL2.sol` and
`EEZL2StaticCursor.t.sol` establish the position-bearing contract implemented
here. The discrepancy is recorded in the [port proposal](../../docs/proposals/l2-static-execution-position.md).

## 11. Data-availability and Sync-block verification

The new network uses direct RollupX V0 (`0x00`) from genesis under the
[wire and action contract](../../docs/proposals/native-system-transactions.md#direct-v0-wire-contract).
There is no activation option, old-format acceptance, profile selector, or
full-block escape in operations. The RPC capability's profile identifier is a
separate namespace.

The count, beneficiary and extraData columns cover every position, including the
terminal. Every pure transaction is carried as exact EIP-2718 bytes with a
positive u32 length. Pure counts are limited to 16,383 per position; total block
count uses the full u32 range subject to complete Sync intervals and configured
replay quotas. Empty runs still execute every block. Fixed headers, timestamps,
RANDAO and outputs are independently derived under sections 6 and 7.

There are no sidecar, group-size or checkpoint arrays in operations. The enclosing
native semantic stream supplies exact origin transactions and whole call trees,
including supported static and rollback regions. Checked reconstruction derives
all tables and outbound groups and binds them to settlement authorization. No
missing, reordered, unused, byte-different or unsupported action can be normalized
into a valid candidate.

V0 MUST cover exactly the independently replayed window. Published metadata and
ordinary transaction bytes MUST match every block's reconstruction evidence. For
the terminal, operations contain the ordinary prefix before its first native
transaction. The remaining actions MUST reconstruct byte for byte to the exact
validated suffix. B[0] includes the ordinary prefix and pre-execution history
updates; it is a terminal-height block hash, not the H-1 parent hash.

Native reconstruction uses the fixed contract:

```text
type         = 0x76
sender       = 0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0076
signature    = none
target       = EEZL2_ADDRESS
chain id     = operator-configured L2 EIP-155 chain ID
gas price    = 0, including nested execution
gas limit    = min(16_777_216, block gas limit - preceding cumulative gas), metered
rollup id    = expected_rollup_id
first nonce  = nonce of the first validated native transaction
```

The canonical order is:

```text
[outbound load 0, outbound user 0,
 ...,
 outbound load K-1, outbound user K-1,
 inbound delivery 0,
 ...,
 inbound delivery M-1]
```

System nonces increase through that order. Outbound loads carry zero native
value. Inbound deliveries carry their authorized inbound value. The reconstructed
suffix length and every raw transaction byte MUST equal the validated
terminal block after its published ordinary prefix.

With no authorized effects, no Sync sequence is reconstructed.

The payload is transported only through native blobs. `batch.callData` MUST be
empty, at least one physical EIP-4844 blob MUST be supplied within the trusted
L1 profile's allowance at `post_batch.posting_context.target_timestamp`, and
`batch.blobIndices` MUST be exactly the ordered identity selection
`[0, 1, ...]`. Missing blobs and calldata-only batches are rejected.

The blobs MUST decode as the EEZ version-`00` message stream containing exactly
one leading `ChainOperation(expected_rollup_id, payload)`, zero or more complete
`InitiateCrossChainTransaction` … `FinishCrossChainTransaction` brackets, then
exactly one `CloseBlobStream`, with canonical field-element packing and zero
padding. The transaction brackets use the protocol-native `Call`, `StaticCall`,
`ReturnSuccess`, `ReturnFail`, `Snapshot`, and `Revert` messages. Carrier markers
cannot appear inside the semantic section. Calls with nonzero `gas` are rejected
because this deployment pins manager `useGasLeft = false` and supports only the
zero-gas table identity.

The signer decodes each semantic transaction back into the production call
forest and materializes it with the same table builder used by the Composer.
For every L2-origin transaction it requires:

- origin chain equal to `expected_rollup_id`;
- `tx_data` byte-for-byte equal to the positioned signed user transaction in
  the validated Sync block;
- the exact authorized L2 source and callback sidecars; and
- the exact authorized L1 settlement host entries. The signer copies only the
  independently authorized `rootUpdates` into the materialized table, runs the
  production symbolic trace finalizer, and then requires byte-for-byte equality
  of every root-dependent rolling hash, nested position key, destination, and
  remaining entry field.

All remaining semantic transactions MUST originate on L1. Their aggregate L1
source entries are finalized from independently authorized `rootUpdates` and
MUST byte-for-byte reproduce every authorized inbound entry; their L2 target
sidecars MUST likewise reproduce every authorized inbound delivery, with no
extra or foreign-chain target. Top-level L1 static transactions have no mutable
L2 delivery; their materialized L1 static-pool entries receive only the
independently validated FIFO-position root pins and MUST byte-for-byte reproduce
`postBatch.staticEntries`, including the `l2ToL1Calls` and untagged rolling hash
of any static callbacks to L1. Replay of such a read executes its L2 target at
the pinned prefix and answers each authorized-proxy dispatch only if it is the
next semantic callback, with the same caller, target and calldata, in a static
frame, with zero value. It returns the callback's claimed output without
charging gas, as live simulation does. Any other dispatch, or a callback that is
never made, rejects the request. The callback results themselves are enforced
on L1 (section 1.1). A lookup-only mutable action with a semantic descendant
remains unsupported.

For each L1-origin semantic transaction, top-level source actions are projected
in their original order. A successful mutable action advances the live root to
its independently replayed transaction checkpoint only when its source frame
also commits. A failed destination call, or a call covered by a later source
frame revert, is lookup-only: it has no L2 delivery, a zero-value no-op root
update, and retains the signed return or revert data for the L1 caller. A static
action likewise creates no L2 transaction and pins the root live at its exact
position. Root-only entries are valid only for the leading anchor; no trailing
entry may advance the commitment without an executed effect. Every deferred
entry and static entry MUST be consumed exactly once by this schedule, with no
reordering or remainder.

An L1-origin transaction's `tx_data` MUST be empty; any other value is rejected.
The originating L1 transaction is not part of any L2 block, is not in the Prove
request, and is already published on L1, so a copy in the blob could never be
bound to anything. (The blob format leaves the field chain-defined; this
deployment defines it as empty for an L1 origin.) This signer validates the
`postBatch` transaction and L2 window but does not execute the later L1 user
transactions in the atomic bundle. Consequently, the aggregate effects are
exact, but L1 semantic transaction boundaries and the relative ordering of
transactions that land in different mutable/static pools are descriptive rather
than independently authenticated by this signer.

The signer recomputes every KZG versioned hash from physical blobs. Mixed,
missing, surplus, reordered, malformed, wrong-rollup, semantically unbound, or
oversized blob carriers MUST be rejected.

## 12. Public-input recomputation

Let `H = keccak256`. Solidity ABI rules are normative.

For every mutable entry:

```text
entryHash[i] = H(abi.encode(batch.entries[i]))
staticEntryHash[i] = H(abi.encode(batch.staticEntries[i]))
```

For the supported profile:

```text
staticEntryHashes = ordered staticEntryHash values
blobHashes        = versioned hashes selected by the canonical identity
                    blobIndices
customData        = abi.encode(uint256(target_timestamp), parent_hash)
customDataHash    = H(abi.encode(uint64(expected_rollup_id), customData))
customDataHashes  = dynamic bytes32[] containing only customDataHash
boundSender       = address(0)
```

The shared input is:

```text
shared = H(
    abi.encode(entryHashes)
 || abi.encode(staticEntryHashes)
 || abi.encode(blobHashes)
 || H(bytes(""))                 // batch.callData is profile-pinned empty
 || abi.encode(customDataHashes)
 || packed_address_20_bytes(boundSender)
)
```

The sole proof-system accumulator is:

```text
acc = H(abi.encode(
    bytes32(0),
    uint64(expected_rollup_id),
    bytes32(configured_vkey)
))
```

The signed digest is:

```text
publicInputsHash = H(shared || acc)
```

where `shared || acc` is the 64-byte packed concatenation.

The computation MUST return exactly one hash. The Composer-supplied hash MUST
not participate.

### 12.1 Fields not bound by the digest

The full mutable and static entries and selected physical blob contents through
their KZG versioned hashes are bound as above. Empty `callData` is profile-pinned
and its empty hash remains part of the on-chain formula. The following
batch carriers are not directly included in the digest:

- `expectedRootPerRollup`;
- `immediateEntryCount` and `immediateStaticEntryCount`;
- proof-system contract addresses;
- proof bytes; and
- the literal `blockNumber`, `blobIndices`, and
  `bindMsgSenderInPublicInput` carriers.

Some of these fields affect derived hash inputs in broader protocol profiles.
This signer pins them to the values in section 7 before hashing. The configured
proof-system address is also checked before signing. Those checks constrain
what the signer accepts, but they do not add the omitted carriers to the
signature preimage.

## 13. Attestation

Only the complete validation-and-settlement pipeline may construct an
attestable hash. A helper that merely decodes a batch or recomputes a profile
hash MUST NOT be able to invoke the production attester.

The signer signs the raw 32-byte `publicInputsHash` using secp256k1 ECDSA:

- no EIP-191 prefix;
- no EIP-712 domain;
- no additional chain, contract, or request domain separator;
- canonical low-`s`; and
- `v` normalized to `27` or `28`.

The signature is exactly 65 bytes:

```text
r[32] || s[32] || v[1]
```

The response contains the locally recomputed 32-byte hash and that signature.
It MUST NOT return the Composer-supplied hash.

## 14. Public failure behavior

The service MUST fail closed. Internal diagnostics may be detailed, but public
messages SHOULD remain stable and must not expose secrets.

| gRPC code | Principal cases |
| --- | --- |
| `InvalidArgument` | Malformed stream structure; invalid widths or bounds; noncanonical/invalid PostBatch calldata; malformed or trailing DA payload. |
| `FailedPrecondition` | Rollup identity mismatch; Stateless input rejection; unsupported batch profile; state/effect/inbound/outbound/DA/native-blob-semantic rejection. |
| `Unavailable` | Another request is active. The same complete request may succeed after the active request releases the slot. |
| `ResourceExhausted` | A decoding, block, byte, or witness-item limit was exceeded; or block-vector storage could not be reserved. |
| `DeadlineExceeded` | Stream idle timeout or absolute request deadline. |
| `Cancelled` | Cooperative stop after request cancellation. |
| `Internal` | Backend-success output violates its contract; local invariant failure; impossible public-input computation/cardinality; reconstruction failures attributable to already validated internal evidence; signing failure. |

A malformed candidate that could otherwise disappear from consideration MUST
be retained and rejected at its authorization gate.

### 14.1 Actionable failed preconditions

A `FailedPrecondition` response MAY carry one protobuf-encoded `ProveFailure`
in the gRPC status-details field when the validated execution identifies one
cross-chain candidate that the Composer can safely remove. The status code,
not the details payload, remains authoritative for retry classification.

`ProveFailure.actionable_failure` has exactly two supported variants:

- `OutboundFailure` identifies the original signed L2 user transaction by its
  zero-based index in the terminal Sync block and its canonical 32-byte
  transaction hash. When the preceding synthetic load transaction reverted,
  the failure still identifies the paired user transaction; rebuilding the
  Sync block regenerates or removes both halves together.
- `InboundFailure` identifies the claimed effect by its zero-based index in
  `PostBatch.entries` and the 32-byte keccak hash of that entry's canonical ABI
  encoding. The original signed L1 transaction is not present in the proof
  request, so the Composer MUST resolve it through the request-local
  entry-to-held-transaction mapping retained during composition.

The signer MUST attach an outbound detail only when validated terminal-block
execution safely identifies the original user transaction: a reverted
canonical synthetic load/user pair, or a positioned outbound observation
failure attributable to the user transaction. It MUST attach an inbound detail
only for a positioned inbound delivery transaction that reverted. Structural,
ordering, envelope, claim-only, missing-candidate, extra-observation, DA, and
state-chain failures MUST remain non-actionable even when their diagnostic
contains an index. A mismatch between a claimed entry's call hash and an
execution observation is claim-only in both directions and MUST remain
non-actionable.

Before changing pool state, the Composer MUST verify both fields against the
exact rejected request: index and transaction hash for outbound, or index and
canonical entry hash for inbound. Empty, malformed, unknown, wrong-width, or
mismatched details MUST be handled as an ordinary non-actionable rejection.
The Composer MUST NOT retry an unchanged request after an actionable failure.
It MAY remove the resolved held transaction and its same-sender,
same-direction nonce suffix, then rebuild and submit a smaller batch within the
remaining slot budget. This recovery does not authorize bisection or eviction
for failures that carry no valid typed detail.

The index-and-hash checks bind an actionable detail to the rejected request;
they do not independently prove that the reported execution failure occurred.
The Composer therefore trusts its configured prover not to falsely attribute a
failure. A buggy or compromised prover can cause valid held transactions and
their nonce suffixes to be evicted, requiring users to resubmit. Authenticating
the Composer-prover transport prevents response injection but does not remove
this configured-prover trust.

## 15. Conformance and change control

Changes to the ABI, selectors, call-hash formulas, rolling-hash formulas,
public-input formula, system-transaction bytes, checkpoint semantics, or
supported profile are security-sensitive.

A compatible implementation MUST test at least:

- every stream ordering, identity, and quota boundary;
- strict chain-document parsing and secret redaction;
- exact RLP identity/hash binding and backend-output association;
- checkpoint selection, returned positions, and roots;
- canonical PostBatch decoding and every profile pin;
- state-chain endpoints, continuity, effect count/order/kind, and checkpoints;
- inbound outer/inner equality, call hash, rolling hash, nested outbound
  events, value, and canonical calldata;
- outbound event provenance, canonical encoding, zero-`callGas` hash, L1
  rolling hash, ordering, source, and value;
- exact DA projection, sidecars, and mixed Sync-block reconstruction;
- actionable outbound/inbound failure attribution, reference validation, and
  non-actionable fallback;
- public-input vectors against the pinned Solidity formula; and
- raw-digest ECDSA recovery, low-`s`, and `v` encoding.

The root Kurtosis gate MUST also observe at least one successful signer
attestation and one node-side acceptance of a remote attestation while running
the inbound, outbound, and mixed settlement modes. A wave-only success is not
sufficient end-to-end proof evidence.

The current captured `fresh-chain-inbound-2175` PostBatch uses a legacy selector
and is a rejection regression, not a positive current-protocol fixture. The
captured `nonzero-outbound-630` receipts use the former five-field outbound
event and likewise test rejection by the current six-field decoder. The
`stateless-block-13` and `stateless-checkpoint-2175` fixtures validate execution
and checkpoint behavior only; they do not by themselves establish a positive
current-protocol settlement attestation.

Updating either pinned external revision MUST include:

1. review of its source diff and security invariants;
2. updates to the shared Rust ABI/hash mirrors;
3. independent vectors or contract differential tests;
4. signer unit, integration, clippy, documentation, and end-to-end tests; and
5. an explicit review of this document's attestation and non-claim boundaries.

## Annex A. Current ABI surface

Field order and integer widths are part of the ABI.

```solidity
struct RootUpdate {
    uint64 rollupId;
    int192 etherDelta;
    bytes32 currentRoot;
    bytes32 newRoot;
}

struct ExpectedRootPerRollup {
    uint64 rollupId;
    bytes32 root;
}

struct L2ToL1Call {
    uint16 revertNextNCalls;
    bool isStatic;
    uint64 gas;
    address sourceAddress;
    uint64 sourceRollupId;
    address targetAddress;
    uint256 value;
    bytes data;
}

struct ExpectedL1ToL2Call {
    bytes32 expectedL1toL2Hash;
    L2ToL1Call[] l2ToL1Calls;
    bytes32 revertedOrStaticRollingHash;
    bool success;
    bytes returnData;
}

struct ExecutionEntry {
    RootUpdate[] rootUpdates;
    bytes32 proxyEntryHash;
    L2ToL1Call[] l2ToL1Calls;
    ExpectedL1ToL2Call[] expectedL1ToL2Calls;
    bytes32 rollingHash;
    uint64 destinationRollupId;
    bool success;
    bytes returnData;
}

struct StaticExecutionEntry {
    ExpectedRootPerRollup[] expectedRoots;
    bytes32 proxyEntryHash;
    L2ToL1Call[] l2ToL1Calls;
    bytes32 rollingHash;
    uint64 destinationRollupId;
    bool success;
    bytes returnData;
}

struct RollupIdWithProofSystems {
    uint64 rollupId;
    uint64[] proofSystemIndexes;
}

struct ProofSystemBatchPerVerificationEntries {
    ExpectedRootPerRollup[] expectedRootPerRollup;
    ExecutionEntry[] entries;
    StaticExecutionEntry[] staticEntries;
    uint256 immediateEntryCount;
    uint256 immediateStaticEntryCount;
    address[] proofSystems;
    RollupIdWithProofSystems[] rollupIdsWithProofSystems;
    uint256[] blobIndices;
    bytes callData;
    bytes[] proofs;
    uint64 blockNumber;
    bool bindMsgSenderInPublicInput;
}
```

The L2 inbound envelope uses the separate L2 ABI family:

```solidity
struct CrossChainCall {
    uint16 revertNextNCalls;
    bool isStatic;
    uint64 gas;
    address sourceAddress;
    uint64 sourceRollupId;
    address targetAddress;
    uint256 value;
    bytes data;
}

struct ExpectedOutgoingCrossChainCall {
    bytes32 expectedOutgoingHash;
    CrossChainCall[] incomingCalls;
    bytes32 revertedOrStaticRollingHash;
    bool success;
    bytes returnData;
}

struct L2ExecutionEntry {
    bytes32 proxyEntryHash;
    CrossChainCall[] incomingCalls;
    ExpectedOutgoingCrossChainCall[] expectedOutgoingCalls;
    bytes32 rollingHash;
    bool success;
    bytes returnData;
}

struct L2StaticExecutionEntry {
    uint256 expectedEntryIndex;
    bytes32 proxyEntryHash;
    CrossChainCall[] incomingCalls;
    bytes32 rollingHash;
    bool success;
    bytes returnData;
}

function loadExecutionTable(
    L2ExecutionEntry[] _entries,
    L2StaticExecutionEntry[] _staticEntries
) payable;

function executeIncomingCrossChainCall(
    L2ExecutionEntry[] _entries,
    L2StaticExecutionEntry[] _staticEntries
) payable returns (bytes);

event CrossChainCallExecuted(
    bytes32 indexed crossChainCallHash,
    address indexed proxy,
    address sourceAddress,
    bytes callData,
    uint256 value,
    uint64 callGas
);
```

Current selector locks are:

| Function | Selector |
| --- | --- |
| `postAndVerifyBatch` | `0xe4a480e4` |
| `executeL2Txs` | `0xdc6d11fa` |
| `staticCrossChainCall` | `0x31344ade` |
| `loadExecutionTable` | `0xbf2eebd3` |
| `executeIncomingCrossChainCall` | `0xc3fb5f3d` |

## Annex B. Hash formulas used by the supported profile

Let `H = keccak256`. `abi.encode` below is standard Solidity ABI encoding;
`packed` means the exact byte concatenation shown.

### B.1 Cross-chain call hash

```text
H(abi.encode(
    bool isStatic,
    address sourceAddress,
    uint64 sourceRollupId,
    address targetAddress,
    uint64 targetRollupId,
    uint256 value,
    uint64 callGas,
    bytes data
))
```

All supported call paths use `callGas == 0`.

### B.2 Mutable L2 outbound event hash

```text
H(abi.encode(
    false,
    address sourceAddress,
    uint64 sourceRollupId,
    address targetAddress,
    uint64 targetRollupId,
    uint256 value,
    uint64 callGas,
    bytes data
))
```

This is B.1 with `isStatic == false` and the `callGas` observed in the event.
The supported deployment requires `callGas == 0`, so the event hash is also
the identity folded into the corresponding L1 entry rolling hash.

### B.3 Entry rolling hashes

For L1:

```text
states[0]   = bytes32(0)
states[i+1] = H(states[i] || uint64_be(update[i].rollupId)
                          || update[i].currentRoot)
l1_seed     = H(states[n] || proxyEntryHash)
```

For L2:

```text
l2_seed = H(bytes32(0) || proxyEntryHash)
```

Supported event folds are:

```text
CALL_BEGIN(prev, callHash) = H(prev || 0x01 || callHash)
CALL_END(prev, success, returnData)
                           = H(prev || 0x02 || bool_byte(success)
                                    || raw_return_data)
```

`uint64_be` is exactly eight big-endian bytes, and the success flag is exactly
one byte (`0x00` or `0x01`).

## Annex C. Source anchors

The principal sources for this specification are:

- `src/config.rs`, `src/service.rs`, and `src/service/` for configuration,
  request lifetime, deadlines, and error mapping;
- `src/window.rs` and `../eez-control-rpc/proto/prove.proto` for wire admission;
- `src/validate.rs`, `src/validate/stateless.rs`, and
  `src/validate/stateless/chain_config.rs` for replay evidence;
- `src/settlement/` for canonical decoding, profile, state, effect, inbound,
  outbound, DA, and system-transaction gates;
- `src/attest.rs` and `../eez-protocol/src/signer.rs` for attestation;
- `../eez-protocol/src/abi.rs`, `action.rs`, `rolling_hash.rs`,
  `public_inputs.rs`, and `system_tx.rs` for shared protocol mirrors; and
- `../../eez-core-protocol/src/interfaces/IEEZ.sol`, `EEZ.sol`,
  `src/interfaces/IEEZL2.sol`, `src/L2/EEZL2.sol`, and
  `src/rollupContract/Rollup.sol` for pinned protocol behavior; and
- `../../contracts/src/ECDSAProofSystem.sol` for the deployed ECDSA verifier.
