# Operations

> This is an implementation guide. [`SPEC.md`](../SPEC.md) is authoritative for
> protocol behavior, configuration requirements, and compatibility.

## Startup trust roots

The operator supplies the required fresh-network genesis, expected rollup ID,
canonical L1 RPC source and registry address, proof-system address and vkey, and
attestation key. Startup validates local relationships. Each final proof independently
authenticates its accepted parent, live-slot seed and target L1 context through
that source before authorizing settlement. V2 prevalidation can replay earlier. Signing performs no further L1 reads; an already
stale signature cannot be used under a different L1 parent or timestamp.
Deployment automation must still verify the manager's proof-system registration,
vkey and attester bindings.

The daemon uses the attestation key only after all execution, settlement and L1
checks pass, signing with raw/prehash ECDSA and no EIP-191 prefix. Native L2 system
transactions have a fixed sender and require no key or fee funding. Prefer secret
manager environment injection over command-line flags. The checked-in
[`.env.example`](../.env.example) contains placeholders only.

## Network exposure

Loopback is the safe default. The gRPC protocol does not authenticate peers, so
a non-loopback listener needs an authenticated transport, private network,
firewall, or equivalent control. Bounded admission limits concurrent
work; it is not authentication and does not stop an authorized-but-buggy client
from holding the slot with a slow stream.

## Capacity model

The service has one global active-request slot. The following controls remain
independent:

- per-message gRPC decode size;
- aggregate known-field protobuf bytes;
- declared block count;
- aggregate witness item count;
- per-message idle timeout; and
- end-to-end request deadline.

A v1 overlap returns `Unavailable` immediately and may be retried later. Local
quota violations return `ResourceExhausted` and require changing the request or
the configured limit. Clients can therefore classify the failure from the
canonical status code without parsing its message.

## Deadlines and shutdown

Before opening the RPC listener, the daemon initializes the default KZG tables
used by the blob codec. Allow this initialization in startup/readiness timeouts;
`eez.signer.kzg.ready` records its duration as `initialization_ms`. It runs once
per process in an awaited blocking worker and does not submit a calibration
batch or skip proof validation. Initialization failure prevents readiness.
The existing request deadlines and graceful shutdown rules begin after serving.
Validation identity schema 4 requires matching Composer and signer binaries;
an old peer is rejected during capability negotiation. This simplification
changes no genesis, database, DA bytes or historical replay rules. Upgrade the
Composer and signer together; rollback requires reverting both. The original
native V0 format still requires a matching fresh-network deployment. See the
[fresh-network boundary](../SPEC.md#mixed-outbound-profile-compatibility).

For v1, the idle timeout covers each wait for the next chunk or EOF, and the
absolute request deadline covers ingestion through signing. Both are checked for
monotonic-clock overflow at startup.

EVM execution and individual settlement gates are synchronous. When an RPC
times out, cancellation asks the worker to stop at the next safe boundary; it
does not interrupt the current work unit. Until the worker exits, v1 retries are
rejected because the slot is occupied; v2 queues under its operation deadline.
The [streaming profile](../../eez-control-rpc/STREAMING_SPEC.md#6-rollupx-runtime-profile)
defines its per-operation, queue and backpressure bounds.

Ctrl-C, and SIGTERM on Unix, stop new acceptance and begin tonic's drain. The
signal lets an admitted v1 RPC run until completion or its request deadline.
V2 attachments close and cancel their async operations, including continuously
active or non-reading clients. The process then waits for any detached worker
to reach a cancellation boundary or complete and release its permits.
Supervisors should therefore allow the remaining request deadline plus the
worst-case non-interruptible cancellation tail. A forced kill trades bounded
shutdown time for abandoning validation mid-computation.

## Tracing

`RUST_LOG` controls filtering and defaults to `info`. Invalid filter syntax is
a startup error. Request spans carry a request ID, peer address, validator,
expected and wire rollup IDs, range, and block count once known.

- `info` records startup configuration without secrets and successful
  attestations.
- `warn` records expected operational or composer-controlled refusals.
- `error` records invariant failures, worker failures, and signing failures.
- `debug` adds admission, pipeline, progress, and operator-configured chain detail.
- `trace` adds per-block validation and derived execution summaries.

Use `request_id`, `phase`, `grpc_code`, and `gate` when present to correlate a
refusal. The daemon intentionally returns stable public status messages while
retaining the detailed internal cause in logs.

## Verification

The full contributor command set is maintained in
[`CONTRIBUTING.md`](../CONTRIBUTING.md#verify). A change to the pinned
[`eez-association/stateless`](https://github.com/eez-association/stateless)
fork must pass that repository's tests and strict Clippy checks before the
signer updates its commit pin; the pin update must then pass the signer checks.

## Streaming source and upgrades

`--l2-rpc-url` / `EEZ_PROOF_SIGNER_L2_RPC_URL` enables v2 on the existing
listener. The standalone default is absent (v1 only). Packaged Chiado, Ethereum
and local Kurtosis Compose services set `http://127.0.0.1:18688`; Kurtosis's
service network uses `http://eez-node:18688`. Compose preserves explicit empty
values so the signer's parser rejects them. CLI overrides environment; an
invalid HTTP(S) endpoint fails startup with a redacted error.

The source must retain exact-hash predecessor and live-slot endpoint headers.
It need not be available when the signer binds its listener, but Finalize cannot
succeed without independently authenticated provenance. Keep its address stable
across source restart. Archive/pruned-history gaps and deep acceptance-history
scans may consume the operation deadline; no receipt or root equality bypasses
the canonical checks. Tune existing request limits for the bounded eight-stream,
eight-session profile; retained-memory allowances are per session, not global.

Follow the [runtime compatibility contract](../../eez-control-rpc/STREAMING_SPEC.md#upgrade-and-rollback)
for signer-first upgrades, v1 fallback and rollback. Sessions are volatile, and
restart can increase backfill work within the unchanged settlement cutoff.
`eez.proof_signer.block_validated` records prevalidation only;
`eez.proof_signer.window_signed` with `rpc_version=2` records completed native
settlement validation/signing. Neither establishes L1 inclusion or durable queue
recovery. Deployment and latency claims require separate live evidence.
