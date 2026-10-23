# Operations

> This is an implementation guide. [`SPEC.md`](../SPEC.md) is authoritative for
> protocol behavior, configuration requirements, and compatibility.

## Startup trust roots

The operator supplies the complete fresh-network L2 genesis, local L1 chain
profile, registry and manager addresses, expected rollup ID, proof-system address
and vkey, and attestation key. The profile and validation identity commit to the
same execution, history and L1 fork rules as the network deployment. Startup
checks their local relationships and performs no chain reads.

Each v2 request supplies canonical raw L1 headers and L2 execution witnesses.
The signer validates their consistency, exact parent, scheduled history imports,
PREVRANDAO, effects and DA before signing the independently reconstructed public
input. It does not discover chain canonicality by itself: the deployed manager
returns the actual inclusion context and accepted L2 commitment through
`getCustomData()`. The on-chain verifier accepts only a matching public input.
Deployment tooling must verify that manager profile, proof-system registration,
vkey and attester bindings agree. ECDSA attestation remains a trusted backend.

The daemon uses the attestation key only after all execution, settlement and L1
checks pass, signing with raw/prehash ECDSA and no EIP-191 prefix. Native L2 system
transactions have a fixed sender and require no key or fee funding. Prefer secret
manager environment injection over command-line flags. The checked-in
[`.env.example`](../../../.env.example) contains placeholders only.

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

V2 admits up to eight active streams and retains up to eight sessions. One
worker serializes expensive validation, with queued work bounded by the session
limits and operation deadline. Retained byte/item limits are per session, not a
global memory ceiling. Quota violations return `ResourceExhausted`; they do not
authorize dropping user work. The Composer uses the RPC contract's closed status
list for transport retries, never diagnostic text. There are no preparation
leases or shared block-reference caches.

## Deadlines and shutdown

Before opening the RPC listener, the daemon initializes the default KZG tables
used by the blob codec. Allow this initialization in startup/readiness timeouts;
`eez.signer.kzg.ready` records its duration as `initialization_ms`. It runs once
per process in an awaited blocking worker and does not submit a calibration
batch or skip proof validation. Initialization failure prevents readiness.
The existing request deadlines and graceful shutdown rules begin after serving.
Validation identity schema 5 and offline profile 2 require agreement between
Composer and signer before any work is dispatched. Old peers fail capability
preflight; no v1 proving fallback is offered. The profile requires fresh contracts,
genesis and datadirs. Do not attach these binaries to an existing network or
reinterpret an old database. See the [fresh-network contract](../SPEC.md).

The idle timeout bounds waiting for client messages and response backpressure.
An active Block or Finalize has its own absolute operation deadline, including
waiting for the validation worker. Startup checks duration arithmetic for
overflow. A long ancestry bridge counts against byte/item limits even when the
L2 execution range is short.

Execution and settlement gates are synchronous. Timing out an RPC signals
cancellation, but a running blocking worker retains its permits until it exits
at a cancellation boundary or completes. A queued worker can be aborted before
it starts. Rewind and Resume fence superseded work; an ACK is never a signature.
The [streaming specification](../../eez-control-rpc/STREAMING_SPEC.md) defines
operation, queue and backpressure bounds.

Ctrl-C and SIGTERM stop acceptance, close v2 attachments and cancel their async
operations. Shutdown waits for detached blocking workers to release their
permits. Supervisors must allow the remaining deadline plus any current
non-interruptible work unit. Killing the process loses volatile sessions; the
Composer must begin again with complete evidence. This does not advance the
canonical settlement cursor or provide durable user-queue recovery.

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

## Offline configuration and recovery

The daemon requires neither an L1 nor an L2 RPC source. The removed
`--l1-rpc-url` and `--l2-rpc-url` options reject. Presence of `EEZ_L1_RPC_URL` or
`EEZ_PROOF_SIGNER_L2_RPC_URL`, including an empty value, fails startup with a
redacted error. Supply only signer-owned settings to its service; Composer,
follower and transaction sender services still need their own chain connections.

Set `EEZ_CHAIN_CONFIG`, `EEZ_L1_CHAIN_CONFIG`, `EEZ_REGISTRY_ADDRESS`,
`EEZ_ROLLUP_MANAGER_ADDRESS` and the proof identities documented in
[SPEC.md](../SPEC.md#3-startup-configuration). Local profile paths must be readable by the
signer. CLI flags take precedence over corresponding supported environment
variables. Do not copy node RPC variables into the signer environment.

Both actual-parent and historical anchor modes use the same v2 service. The
actual-parent request carries its exact posting context. Historical ordinary
catch-up omits that field and remains conditional on anchor availability, the
accepted commitment and active L1 rules at inclusion. It is never a fallback for
expired live cross-chain work. Operational retries and submission cutoffs remain
owned by the Composer.

After a cold restart, the Composer sends a complete `Begin -> Block* -> Finalize`
exchange. It can use retained session progress only when the service confirms
it. `eez.proof_signer.block_validated` records prevalidation;
`eez.proof_signer.window_signed` records completed settlement validation/signing.
Neither event establishes inclusion or canonical completion.
