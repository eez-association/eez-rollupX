# Offline v2 conformance vectors

The corpus uses [versionless cross-chain transactions and authenticated fee collection](../../../docs/proposals/native-system-transactions.md),
execution-group source identities, optional HistoryUpdate 0x46/version 1,
complete-group and block-finalization cleanup, network-bound verification keys and
validation-contract version 8 with the unmodified Core `Rollup`. The proving transport remains v2.
Separate [encoding vectors](native-profile-v1.json) freeze exact native bytes,
source identities and profile commitments; they are not execution proofs.

For Nethermind/C# and other external implementations, start with the
[integration guide](IMPLEMENTING.md) and
[portable identity vectors](validation-identity.json).

The [29 frozen cases](../../eez-proof-signer/tests/fixtures/streaming-v2/manifest.json)
exercise the [v2 offline profile](../STREAMING_SPEC.md) through the real TCP
service and Stateless executor. They contain complete L1/L2 witnesses, native
blobs, exact ABI inputs and the posting context required by actual-parent mode.

The corpus has 24 expected successful proofs across 29 cases. Exact frame/event
counts and input digests are recorded in its manifest. The recordings and
transcripts must match versionless `0x45`, complete-group cleanup, optional
history and the execution-group source identity. The earlier
[execution-fixture record](../../../docs/audits/2026-10-27-block-end-cleanup/README.md)
records the baseline execution evidence; the Core proof-policy adaptation is
described below.

The separate [scheduled-history observation fixtures](../../eez-proof-signer/tests/fixtures/native-history-observations/README.md)
add six complete v2 requests for a history import at the terminal ordinary
prefix: correct static/lookup outcomes must sign, and pre-history or
pre-ordinary outcomes must not. These deterministic inputs are checked by signer
unit tests and the standalone `cross_chain::history_observations` process test;
they are not part of the 29-case transport transcript corpus.

## Coverage

Each case is one JSON file under
[`eez-proof-signer/tests/fixtures/streaming-v2`](../../eez-proof-signer/tests/fixtures/streaming-v2).
`category` distinguishes baseline, optional controls and RollupX resource policies.
The full suite is not a requirement for a minimal sequential implementation.

| Cases | Assertions |
| --- | --- |
| `baseline-lookup`, `baseline-static` | Full native execution and lookup/changing-static observation validation; exact independently reconstructed proof digest and trusted signer; Proof then EOF. |
| `sync-range-proof`, `terminal-before-tip` | Incomplete single-block range rejected; complete Sync range proved; selected terminal before validated tip; Resume retains the later suffix. |
| `begin-boundaries` | Absent/31/33-byte hash and root; zero/unsupported rollup; number overflow; invalid initial binding; unknown operation; repeated Begin; valid recovery. |
| `block-boundaries` | Missing data/witness; hash widths; zero/max/gapped numbers; wrong parent/hash/RLP; duplicates at/before tip; changed duplicate witness; zero/max request IDs. |
| `invalid-witness-retry` | Cold real execution rejects missing state, then accepts valid witnesses and signs the independently reconstructed digest. |
| `finalize-ranges` | Empty prefix; invalid start/end; missing/wrong-width/wrong terminal; unchanged cursor and valid retry. |
| `settlement-required-inputs` | Missing settlement, context, blobs or ABI; trailing calldata; recovery with the complete original input. |
| `settlement-blob-encoding` | Empty/short/long physical blob; noncanonical field element; extra unselected blob; valid recovery. |
| `settlement-abi-binding` | Missing/gapped blob selection; calldata DA; wrong blockNumber, current/final root, proof system, rollup assignment or proof-system index; sender binding. |
| `posting-context-boundaries` | Zero/wrong chain and zero timestamp; parent-height overflow; missing/31/33/zero parent hash. |
| `pipelined-gap-recovery`, `half-close-drains` | Four outstanding requests; a queued gap reports only accepted progress; ordered recovery; half-close drains ACKs before EOF and Resume. |
| `resume-after-proof`, `resume-boundaries` | Retained proof range; repeated proof; malformed/unknown/retired session; nonzero initial epoch; Resume on a bound stream. |
| `resume-takeover-fencing` | Live takeover; old transport denied with both old and copied new epoch; disconnect and another Resume preserve progress. |
| `rewind-boundaries` | Tip/intermediate/anchor; malformed/unknown/pruned ancestors; stale epoch; pruned suffix cannot finalize; full recovery. |
| `cancel-session-isolation` | Cross-session Cancel denied; bound Cancel acknowledged then EOF; retired session cannot Resume; another Composer completes. |
| `concurrent-composers` | Independent IDs/cursors for the same branch; both Finalizes queued before either proof is read; two correct proofs. |
| `competing-branches` | Actually executed alternative first block; contradictory same-height replacement and wrong-parent suffix rejected; rewind and recovery to the original branch. |
| `subrange-promoted-anchor` | Derived native blob subrange; authenticated fixture predecessor selection; Resume preserves tip; old range/anchor pruned; repeat proof. |
| `block-quota`, `byte-quota`, `witness-quota` | Exact quota admitted; duplicates remain idempotent; next block rejected; per-session accounting; Rewind releases only its session's retained quota. |
| `frame-size-limit`, `finalize-byte-quota` | Exact-size Begin admitted; one byte over the configured transport ceiling rejected by tonic; another session remains usable. Finalize one byte over its retained-byte quota preserves execution; exact-size Finalize then signs. |
| `stream-capacity`, `detached-session-eviction` | Eight attached transports; ninth RPC rejected; oldest detached session evicted after capacity; latest session resumes. These fixed bounds are RollupX policy. |

## Fixtures and expected results

The [bidirectional](../../eez-proof-signer/tests/fixtures/native-offline-bidirectional),
[lookup](../../eez-proof-signer/tests/fixtures/native-osaka-lookup) and
[changing-static](../../eez-proof-signer/tests/fixtures/native-osaka-changing-static)
fixtures supply exact fresh-network genesis, deployment identity, full v2 inputs
and expected proof digests. Execution bytes come from the original native process
scenarios; the lookup fixture supplements its terminal witness by exact replay
from genesis, as documented in its README. Stable signing-domain-7 keys and proof expectations
are recomputed from those inputs,
then checked against the pinned Core Solidity verifier. Their READMEs distinguish
these adaptations from new on-chain captures. The signing key is intentionally public test material.

The signer needs no chain provider or seeded database. It validates the supplied
L1 chain and independently reconstructs execution and the complete public input.
A signature remains conditional on the bound context: the deployed manager
checks the actual L1 anchor; Core checks entry roots during consumption. These
recordings do not establish that a fresh isolated server independently knows L1
consensus, and ECDSA attestation is not a ZK proof.

Each vector declares its exact `server_profile` limits. Start a fresh service
with the selected fixture's local genesis/deployment inputs and those limits for
each case. Byte accounting includes Begin and newly accepted frames; exact
duplicates do not charge again. Finalize needs its declared quota headroom.
The runner does not configure, deploy or fund a server.

The [generator](../../eez-proof-signer/src/service/sessions/tests/vectors.rs)
constructs expected cursors and roots from captured consensus headers. Full
native proof expectations use independently reconstructed digests; ordinary Sync subranges use
request-side ABI/blob/public-input construction. These derived oracles are not
additional on-chain captures. Expected status codes are explicit case assertions.
No expected digest, cursor or status is learned from a server response.

## Replay and regeneration

Use Rust 1.95.0, pinned dependencies and `protoc`. The wire-only client does not
link Stateless or Reth. Set `VALIDATION_IDENTITY` to the 0x-prefixed hash
independently computed from that fixture's local inputs using the
[identity contract](../VALIDATION_IDENTITY.md), never a value copied from the server.
Against an independently provisioned fixture service:

```bash
cargo +1.95.0 run --locked -p eez-control-rpc --example replay_streaming -- \
  http://127.0.0.1:50051 \
  crates/eez-proof-signer/tests/fixtures/streaming-v2/baseline-lookup.json \
  "$VALIDATION_IDENTITY"
```

The signer's tests create fresh services with each declared profile and replay
all 29 frozen cases without changing their expected results:

```bash
cargo +1.95.0 test --locked -p eez-proof-signer -- service::sessions::tests
python3 crates/eez-control-rpc/conformance/check_vectors.py
```

Regenerate only after reviewing a fixture/profile or case change. Generation is
an explicitly ignored artifact utility; it is not skipped behavior coverage.
The manifest writer records source and fixture digests, never server responses:

```bash
cargo +1.95.0 test --locked -p eez-proof-signer -- \
  --ignored --exact service::tests::derivation::captured::regenerate_native_fixture_commitments
cargo +1.95.0 test --locked -p eez-proof-signer -- \
  --ignored --exact service::sessions::tests::vectors::regenerate_offline_vectors
python3 crates/eez-control-rpc/conformance/check_vectors.py --write-manifest
cargo +1.95.0 test --locked -p eez-proof-signer -- service::sessions::tests
```

The [manifest](../../eez-proof-signer/tests/fixtures/streaming-v2/manifest.json)
records the source snapshot used for generation, including uncommitted file
content where indicated. Its fixture/schema/vector hashes must match this
checkout. Other source hashes preserve generation provenance and need not match
later validator edits. Review regenerated expectations and replay them; changing
a manifest alone does not validate behavior.

## Format and limits

Format 2 contains ordered `events`. Client/server events contain one protobuf
message in `protobuf_hex`, without the gRPC prefix. The first client event opens
its numbered transport. `half_close` ends input; `disconnect` drops the RPC;
`eof` requires closure without another response. RPC errors compare numeric gRPC
codes, including tonic's `OUT_OF_RANGE` for the transport size boundary.

Session IDs normalize to distinct 32-byte tokens. The replayer binds them to
actual Ready responses, checks increasing epochs and isolation, and preserves
literal malformed bindings. It ignores only diagnostic `reused` and rejection
wording. IDs, response order/type, typed status details, cursors, hashes, roots,
proof digests and EOF are assertions. A proof must recover the configured
attester over the expected digest; ECDSA nonce variation is allowed.

The unit suite also rejects deliberately corrupted expected IDs, cursors, roots,
digests and attesters. Wire recordings do not establish in-flight worker permit
retention, process restart, network denial, L1 reorgs, anchor expiry or sustained
catch-up capacity. Those require the separate controlled and native process
suites. No cross-implementation interoperability or public deployment is claimed.
