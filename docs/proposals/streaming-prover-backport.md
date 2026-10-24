# Streaming prover backport

This is a historical design record for the #198/#205/#206 dependency integrated
into RollupX. Its exact upstream/fork revisions, review findings and validation
limits remain in the [backport audit](../audits/2026-10-19-streaming-backport/README.md).

The [fresh-network offline-history design](l1-origin-and-offline-proving.md)
supersedes the backport's signer header providers, v1 fallback, preparation
leases and prior validation identity. None of those paths is retained in the
current implementation. Existing native DA, whole-group checkpoints, bounded
streaming, duplicate handling, rewind/resume, deadlines and worker ownership
remain required.

The current contract is defined by the [v2 streaming specification](../../crates/eez-control-rpc/STREAMING_SPEC.md)
and [signer specification](../../crates/eez-proof-signer/SPEC.md). Both anchor
modes use `Begin -> Block* -> Finalize`, complete L1 header witnesses and no
signer L1/L2 RPC. Current conformance recordings and their reproducible generator
are documented in the [conformance guide](../../crates/eez-control-rpc/conformance/README.md).
Historical backport passes do not establish validation of this new profile.
