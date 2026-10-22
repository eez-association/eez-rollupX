# Executed RollupX streaming RPC vectors

The [29 saved cases](../../eez-proof-signer/tests/fixtures/streaming-v2/manifest.json)
exercise the [shared v2 contract](../STREAMING_SPEC.md) using RollupX's native
blob settlement profile. They include complete execution witnesses, physical
blobs, proofless ABI calldata and posting context. Every case was recorded through
the actual TCP signer after checking its expected behavior, then replayed against
a second fresh service before the generator wrote it.

The suite contains **2,235 requests, 2,234 correlated replies, one terminal
transport error, one RPC admission error, and 24 successful proofs** (4,547
total events). Generation and standalone replay of all 29 cases passed in the separate
implementation checkout, and repeated generation was byte-identical. The
[manifest](../../eez-proof-signer/tests/fixtures/streaming-v2/manifest.json)
preserves those vector, fixture and generating-source digests.

The organization and wire-only replay approach follow
[Rollup0 PR #205](https://github.com/eez-association/eez-rollup0/pull/205), inspected
at `d3c94296bd6f23b4a5e0104950948492ab36398b`. These are independently generated
RollupX cases; Rollup0's calldata-only profile cannot accept their settlement data.

## Coverage

Each case is one JSON file under
[`eez-proof-signer/tests/fixtures/streaming-v2`](../../eez-proof-signer/tests/fixtures/streaming-v2).
`category` distinguishes baseline, optional controls and RollupX resource policies.
The full suite is not a requirement for a minimal sequential implementation.

| Cases | Assertions |
| --- | --- |
| `baseline-lookup`, `baseline-static` | Full native execution and lookup/changing-static observation validation; exact captured proof digest and trusted signer; Proof then EOF. |
| `single-block-proof`, `terminal-before-tip` | Derived one-block settlement; selected terminal before validated tip; Resume retains the later suffix. |
| `begin-boundaries` | Absent/31/33-byte hash and root; zero/unsupported rollup; number overflow; invalid initial binding; unknown operation; repeated Begin; valid recovery. |
| `block-boundaries` | Missing data/witness; hash widths; zero/max/gapped numbers; wrong parent/hash/RLP; duplicates at/before tip; changed duplicate witness; zero/max request IDs. |
| `invalid-witness-retry` | Cold real execution rejects missing state, then accepts valid witnesses and signs the captured digest. |
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

## Fixture and server profiles

The existing [native lookup](../../eez-proof-signer/tests/fixtures/native-osaka-lookup)
and [changing-static](../../eez-proof-signer/tests/fixtures/native-osaka-changing-static)
fixtures supply L1/L2 genesis, proof-system identity, full witnesses and captured
attestations. Use the exact selected fixture's `identity.json` and genesis files.
The fixture signs with the intentionally public Anvil account #5; its expected
address is explicitly in each vector's `oracle.attester`.

**L1 authority is synthetic in this fixture profile.** The validator executes
real Stateless EVM work and the native DA/settlement/observation gates. These
files do not supply a canonical L1 history or seed a complete node database.
A production signer using live L1/L2 sources cannot accept the fixture merely
by loading its genesis. Do not bypass production authentication for replay.
An independent implementation needs a test authority with the same declared
fixture semantics, or separately provisioned equivalent canonical evidence.

Every file declares exact `server_profile` limits. Defaults are 128 blocks,
16 MiB retained bytes, 100,000 witness items, 30-second idle timeout and 60-second
absolute request timeout. Limit cases override one quota: 2 blocks, **6,960
bytes** for two retained blocks, or **41 witness items**. Transport/Finalize byte
boundaries declare their exact encoded limits in their own `server_profile`. Byte accounting includes the encoded Begin
and each newly accepted ClientFrame; duplicate frames do not charge twice.
Finalize also needs quota headroom. Start a fresh fixture service for each case.
The runner does not configure, deploy or fund a server.

## Replay

Use Rust 1.95.0 and the repository's pinned dependencies. The wire-only example
requires `protoc` for the RPC schema build; it does not link Stateless or Reth.
Against an independently provisioned fixture server:

```bash
CARGO_INCREMENTAL=0 cargo +1.95.0 run --locked -p eez-control-rpc \
  --example replay_streaming -- http://127.0.0.1:50051 \
  crates/eez-proof-signer/tests/fixtures/streaming-v2/baseline-lookup.json
```

Run each case against a fresh server configured for its declared fixture and
`server_profile`. The same command accepts any of the 29 JSON files. The schema,
Rust wire exports and replay client are included here. The signer's
`service::sessions::tests` launches a fresh synthetic-authority server with each
recorded profile and replays all 29 cases without regenerating expectations:

```bash
cargo +1.95.0 test --locked -p eez-proof-signer -- service::sessions::tests
```

Production serves the [RollupX runtime profile](../STREAMING_SPEC.md#6-rollupx-runtime-profile),
including native settlement and independently retrieved provenance. Its upgrade
and rollback requirements are specified there.

Verify the saved bytes and the existing fixture/schema inputs without contacting
a server:

```bash
python3 crates/eez-control-rpc/conformance/check_vectors.py
```

The verifier performs read-only checks. It fails on changed/missing/additional
vectors or mismatched fixture/schema digests. Its small script makes the repeated
extraction check reproducible; it never obtains expected replies from a server.

## Format and independent expectations

Format 2 contains ordered `events`. `client`/`server` events carry one protobuf
message in `protobuf_hex`, without the gRPC compression/length prefix. The first
client event opens its numbered transport. `half_close` ends only client input;
`disconnect` drops the RPC; `eof` requires server closure without another message.
`rpc_open_error` asserts a failed RPC admission; `rpc_error` requires a terminal
stream error. Both compare numeric gRPC codes. The small transport-size case
records tonic's `OUT_OF_RANGE` (11), distinct from application quota rejections
and stream admission's `RESOURCE_EXHAUSTED` (8).
`check` labels identify the tested condition.

Issued session IDs normalize to distinct 32-byte repeated tokens. The runner
binds each token and epoch to actual Ready responses, checks session isolation
and increasing epochs, and preserves literal unknown/malformed input bindings.
No other opaque data is substituted. It ignores only diagnostic `reused` and
rejection wording. Request IDs, response kind/order, status/details, cursors,
block hashes/state roots, exact proof digest and EOF remain assertions.
ECDSA signatures must recover the configured attester over the expected digest;
a different valid nonce is allowed.

The recorder derives expected ACK identities/state roots by decoding fixture
consensus headers, and tracks progress from accepted requests. Full-window proof
expectations are the original captured digest and signer oracle. One-block and
subrange digests use request-side native payload/ABI/public-input construction;
these are derived oracles, not additional on-chain captures. The checker never
learns an expected digest or cursor from the response it records.

Decode a frame independently:

```bash
jq -r '.events[0].protobuf_hex' \
  crates/eez-proof-signer/tests/fixtures/streaming-v2/baseline-lookup.json |
  xxd -r -p |
  protoc -I crates/eez-control-rpc/proto --decode=prove.v2.ClientFrame \
    crates/eez-control-rpc/proto/prove_stream.proto
```

## Provenance and coverage limits

Generation used a separate uncommitted RollupX implementation based on
`4f58341cef307b95dbf6ac46ffc33220c8789b0c`, with protocol pin
`9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. The manifest's `source_sha256` map
identifies that generating source snapshot, not the source of this vector-only
branch. That original generator/runtime snapshot is not included or yet
addressable by a published implementation commit; this backport supplies a
separately adapted runtime, with validation recorded in the
[backport audit](../../../docs/audits/2026-10-19-streaming-backport/README.md). The vector and fixture digests are independently
verifiable here; generation cannot be rerun from this branch alone.

The original generator independently checked expected behavior, then replayed
each recording against a second fresh real service before writing it. Its final
run was `af48fd3a-cd78-4d73-a275-29281b22b37e`; all 29 files matched their previous
SHA-256 digests byte-for-byte. The independently built CLI passed all 29 configured
cases in run `975392b0-57fb-4da1-a222-e7d2b45ff32c`. The implementation's affected
suite passed 576 tests, including corrupted expected cursor/request-ID/root/
digest/attester rejection checks, in run
`189abc11-1a7f-48e2-b19e-9fc278c30aa2`. Those historical results identify the
separate implementation, not this branch's production runtime. Original logs
remain local-only; the saved expectations and source/fixture hashes are retained
in this PR.

Extraction validation on `main` base `2c19e36bed2720c00f89841ccfca47ec7c921e06`
rebuilt the standalone example from this branch and replayed all 29 cases against
the separate fixture server (run `614391e6-a36d-4c3c-a5c8-83b70fd246ce`). Its
vectors were byte-identical to the saved corpus, and the generating source still
matched every manifest digest. The RPC package's six unit tests, all-target
Clippy, workspace formatting, manifest checks and documentation checks passed.
The integrity checker also rejected changed, missing and extra vectors, and
mismatched fixture/schema digests. RPC doctests selected zero tests.

Dependency checks ran, but `cargo deny` and `cargo audit` failed on four newly
published advisories for the unchanged `rustls 0.23.45`: `RUSTSEC-2026-0346`
through `RUSTSEC-2026-0349`. Auditing the unmodified base lockfile reproduced all
four. Dependency versions and advisory policy remain unchanged. Full workspace
Clippy, runtime/process and deployment suites were not rerun for this extraction;
its runtime changes are limited to additive generated wire types.

Wire vectors do not establish timing-sensitive in-flight cancellation, worker
permit retention, response backpressure, the 256 MiB transport ceiling, process
restart, canonical L1 reorg/target authentication, settlement deadlines or soak
capacity. Those require controlled runtime/process tests. No independent Rollup0
interoperability run or live deployment is claimed by this suite.
