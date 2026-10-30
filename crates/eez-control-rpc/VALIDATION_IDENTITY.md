# Portable validation identity

Status: normative for validation contract 8, capability profile 2 and v2 proving.
This is a compatibility namespace, never authorization to execute or sign.
No Rust dependency, Cargo lockfile, source revision, binary hash, transport URL,
signing key or resource limit participates.

Version 8 retains the unmodified Core `Rollup` manager and timestamp/hash
context introduced in version 7. The registered vkey binds the validation program and network profile.
It removes automatic accepted-root/rule-period proof expiry. The L2 execution
rules, including the upgradeable history store, 0x46 imports and block-end balance
cleanup, remain unchanged. Known compatible L1 fork crossings are allowed;
incompatible changes require coordinated on-chain key revocation/replacement.

Use matching node, follower, signer and deployment artifacts for fresh networks.
Previous manager/context/key formats and sessions are unsupported. The v2
protobuf, cross-chain 0x45 and protocol pin are unchanged.

Version 8 extends the accepted cross-chain profile to recursive static callbacks,
failed mutable calls with descendants, nested rollback scopes and authenticated
mutable lookup replay. Reused rows retain exact callback and execution
commitments. The protobuf, protocol pin and native encodings remain unchanged.
The version is bound into the network profile and registered key, so version-7
runtime artifacts and keys cannot be mixed with this profile. The coordinated
upgrade boundary is documented in the
[full protocol proposal](../../docs/proposals/full-protocol-e2e.md#compatibility-and-recovery).

## Encoding

Compute Ethereum **Keccak-256**, not NIST SHA3-256, over one canonical RLP list
with these ten elements in this exact order:

| Position | Element | Encoding/value |
| --- | --- | --- |
| 0 | domain | UTF-8 bytes of `eez.validation-identity` |
| 1 | validation-contract version | unsigned integer `8` |
| 2 | rollup ID | locally configured nonzero u64 |
| 3 | L2 chain ID | u64 from the locally loaded L2 chain document |
| 4 | system address | exactly 20 raw bytes |
| 5 | EEZL2 address | exactly 20 raw bytes |
| 6 | L2 chain document digest | exactly 32 bytes: Keccak of canonical JSON below |
| 7 | network profile hash | exactly 32 bytes: registered-vkey network commitment |
| 8 | registry address | exactly 20 raw bytes |
| 9 | manager address | exactly 20 raw bytes |

RLP integers use minimal unsigned big-endian bytes; zero is the empty byte string
(encoded `80`), never a zero byte or padded integer. Addresses and hashes are fixed
width raw byte strings, including leading zeros. Use RLP's canonical string/list
length prefixes; do not ABI-encode the tuple, encode hexadecimal text, or hash
JSON describing the tuple.

[Portable vectors](conformance/validation-identity.json) include the input fields,
exact RLP preimage and expected hash. Integer strings in that JSON fixture avoid
JSON floating-point loss; they are fixture notation, not a new wire encoding.
Synthetic zero-byte vectors test encoding and do not waive runtime configuration
validation.

The network profile commitment is Ethereum Keccak over this static ABI tuple:

```text
abi.encode(
  keccak256("EEZ_ROLLUPX_NETWORK_PROFILE_V1"), uint64(8),
  address(historyAddress), uint64(64), uint64(2000000),
  keccak256(historyProxyRuntime),
  (uint256 l1ChainId, uint256 l2ChainId, bytes32 genesisHash,
   uint64 genesisTimestamp, uint64 blockInterval, uint32 syncInterval,
   uint32 updateOffset, bytes32 l1ConfigHash, address feeCollector)
)
```

The registered verification key is:

```text
keccak256(abi.encode(
  keccak256("EEZ_ROLLUPX_VERIFICATION_KEY_V1"), networkProfileHash,
  address(registry), address(proofSystem), address(attester)
))
```

[`NetworkProfile`](../eez-protocol/src/l1_history/context.rs) is the shared encoder.
Core separately binds the rollup ID in its accumulator. The manager address is
not in the key: its CREATE2 address may depend on the initializer carrying that
key. Deployment registers the key; Composer derives it locally and the signer
rejects a configured key that differs. Signature recovery alone cannot replace
this check. The signer still trusts operator-provided chain configuration.

Capability profile ID 2 is the unchanged v2 transport discriminator. There is
one current validation version with no fallback. Genesis binds initial FeeVault
and history implementation/authority state; compatible later contract upgrades
change state without changing this fixed identity.

The [effective L1 rules producer](../eez-l1/src/chain_config/offline.rs) defines the
existing `l1ConfigHash` inputs: chain/genesis, named fork coordinates, header
layouts and effective blob rules, including defaults and scheduled updates. Its
canonical JSON is configuration data, not a build fingerprint. An external
implementation must match that complete effective profile, including preset
semantics; hashing only the input L1 JSON file is not equivalent.
Independently provision and validate that profile and local chain rules. Never
accept a Composer-selected profile hash, the server's
reported identity, or the fixture's toy digest as production configuration.

The full L2 document digest additionally binds fields not in a genesis header.
Equal chain IDs or genesis hashes alone are insufficient. Proof-system/vkey policy
is checked separately during settlement and response verification; identity
agreement does not replace those checks.

## Canonical JSON

Canonicalize the complete original UTF-8 L2 chain document before deriving defaults,
resolving aliases or converting it into execution types:

1. Parse strictly one JSON value. Reject invalid UTF-8/JSON, trailing input,
   unpaired UTF-16 surrogates, non-JSON numbers and nesting beyond 128 containers.
2. Decode object keys and reject duplicate keys at every depth, including escaped
   aliases. Sort keys lexicographically by their **UTF-8 bytes**. C# UTF-16 ordinal
   ordering differs for some supplementary characters; the vectors cover this.
3. Preserve array order. Emit no insignificant whitespace.
4. Decode strings, emitting `"` and `\` escaped, short escapes for backspace,
   form feed, newline, carriage return and tab, and lowercase `\u00xx` for remaining
   U+0000–U+001F controls. Emit all other characters directly as UTF-8, including
   non-ASCII characters, U+2028/U+2029 and `<>&`. Do not escape `/`.
5. Preserve each number's original lexical token exactly. Never pass through
   floating point, decimal rounding or normalized integer formatting.
   `1`, `1.0`, `1e0` and `-0` remain distinct. Emit `true`, `false`, `null` as written.

Hash the resulting bytes. Formatting and object order differences disappear;
numeric spelling differences can conservatively yield different identities.
Schema/default validation still runs independently; canonicalization alone does
not admit a genesis configuration. Hash and load the same document snapshot.

## Versioning and operations

Validation-contract version 8 covers the RLP tuple above, canonical JSON rules
and the complete offline validation contract:
the fixed Osaka L2 profile, compact V0 operations, versionless cross-chain 0x45 and optional history 0x46/version-1 transactions
(execution-group source identities belong only to cross-chain
transactions), no system nonce,
per-transaction base-fee collection, complete-group SYSTEM/EEZL2 balance cleanup and block-finalization cleanup, permanent history, recorded history transactions and delayed synchronized
PREVRANDAO, both anchor modes, execution/settlement validation, canonical native blobs
and public inputs. The owning specifications are
[RPC](SPEC.md), [streaming](STREAMING_SPEC.md),
[signer](../eez-proof-signer/SPEC.md), and the pinned core ABI/specification.

A change to identity encoding, JSON canonicalization, accepted execution, decoding,
evidence or settlement semantics MUST revise this single version and update the
specifications/vectors together, even if
protobuf fields are unchanged. A different language implementing the same
semantics uses the same version. An implementation must not simply advertise the
version without enforcing its rules. Identity is not remote attestation of code.

The Rust unit suite records a reviewed lockfile digest **only as a build-review
guard**. A dependency change fails that test until execution/decoding impacts are
reviewed. If semantics change, bump the validation-contract version; if they remain
equivalent, record the evidence and update only the guard. An unrelated dependency
change must not invalidate another language's wire compatibility. This guard
detects dependency drift, not arbitrary semantic bugs; ordinary review and
conformance tests remain necessary.

Composer compares the hash with its own local result at Capabilities and sends
the same 32 bytes as binary `eez-validation-identity-bin` metadata on every stream.
The server rejects missing/malformed/mismatching identity before receiving a
Block. The remote hash is never an input to the local calculation. To diagnose a
mismatch compare each local preimage field; the unchanged wire hash does not
identify a differing field.

Different validation-contract versions fail the exact comparison.
Deploy matching Composer/prover/follower artifacts and newly generated genesis
and registered verification key together. The network profile binds the collector and execution profile;
the service uses v2 and validation-contract version 8. Discard retained sessions
when changing identity. This is a fresh-network profile, with no migration or
rollback path for existing blocks/databases.
Pending requests/counters remain in memory; restart is not durable queue recovery.
