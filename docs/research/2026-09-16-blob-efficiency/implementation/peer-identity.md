# Validation identity and peer agreement checkpoint

This is a change-specific soundness argument for CO-14/45 and its configuration
plumbing, not a general security review or a completed long-span release claim.

## Invariant and construction

The producer and signer independently obtain execution rules and a canonical
source-document digest from the same read. Composer's existing Clap `--chain`
selection uses a strict value parser; it constructs the actual ChainSpec from that
parsed document and carries the digest to launch. `--dev` does not override an
explicit `--chain`. Non-node subcommands keep their existing parser. A built-in
preset is usable only if its complete ChainSpec can be reproduced from its
Genesis document; otherwise it fails explicitly and requires a new-network
Genesis file. The source spelling is not guessed from a defaulted ChainSpec.

The canonicalizer rejects duplicate decoded keys at every level, preserves array
order and numeric tokens without a floating-point conversion, and sorts object
keys. The digest precedes aliases/defaults. The identity binds this digest,
canonicalizer/schema versions, rollup ID, EIP-155 ID, system/EEZL2 addresses,
explicit validator semantic revision/core-protocol pin, and the exact dependency
lockfile digest. Unrelated lock changes may conservatively invalidate agreement.
The hash is namespace compatibility, not an attestation of running executable
code, canonical state or a private key.

## Authorization and lifecycle

Capability discovery accesses only local immutable metadata and enforced quotas.
It does not take the proof worker, inspect witnesses, construct an attestable
value or invoke signing. A producer cannot start from a missing/incompatible
capability response or silently guess a profile. Startup intersects the signer
block ceiling with local emission settings and fails if no complete Sync interval
fits. The client applies the effective message bound on the transport too.

Each Prove header includes its expected identity. The signer checks it before
reading blocks, and still runs every existing execution and settlement gate on
an agreeing request. Identity agreement does not replace any authorization check.
An identity mismatch never emits an actionable source hint. Client classification
also ignores extraneous actionable details on the explicit change response.

The client owns one shared channel/snapshot. It does not hold its connection mutex
during proof execution. Failed connections clear only the snapshot they used,
so a late failure cannot discard a newer handshake. Existing attester recovery
and independently recomputed request-digest comparisons remain unchanged.
Cancellation drops an in-progress handshake/stream without adding detached work.

## Evidence and remaining obligations

Initial server/document step: 303 signer, 137 L1, 18 client, 6 control-RPC tests
passed; affected all-target Clippy passed before subsequent peer-wiring changes.
The existing full-window proof oracle and captured native fixture assertions are
retained. Follow-on peer tests cover explicit CLI selection, wrong profile and
identity, malformed limits, lower block caps, successful proof channel reuse,
stale failure invalidation, and zero signing calls for header identity rejection.
The integrated peer check passed 304 signer tests (two explicitly ignored
measurement utilities), 209 Composer, 22 client, 12 shared-bootstrap and six
control-RPC tests. The command enabled `eez-node-common/l1-startup`, so the actual
strict CLI parser tests ran. All affected all-target Clippy checks passed after
boxing the already-large composer launch future at the new wrapper boundary.
Shipping node/follower/signer builds and native posting/replay are being renewed
for this source checkpoint; older frozen executables do not validate these edits.

Subsequent work now advertises and enforces 64-block preparation, cached ranges,
eight bounded leases, one-block worker preemption, a durable 128-identity producer
queue and typed cache-miss repair. Reconnect with changed capabilities fails
closed and requires producer restart/replanning. Maximum retained signer
checkpoints pass both profiles, but maximum posting, follower recovery, lifecycle
stress and the final release claim remain open.

## Commands

All run from the implementation worktree with its isolated `target` directory:

```sh
CARGO_TARGET_DIR="$PWD/target" cargo +1.94.0 test --offline --locked \
  -p eez-proof-signer -p eez-control-rpc -p eez-prover-client \
  -p eez-node-common -p eez-composer --features eez-node-common/l1-startup \
  --lib --bins -j3
CARGO_TARGET_DIR="$PWD/target" cargo +1.94.0 clippy --offline --locked \
  -p eez-proof-signer -p eez-control-rpc -p eez-prover-client -p eez-l1 \
  -p eez-node -p eez-node-common -p eez-composer -p eez-testkit -p eez-node-e2e \
  --all-targets -j3 -- -D warnings
```

The initial default-feature L1 test invocation failed 13 loopback-socket cases
because the sandbox denied binding; the same full L1 suite with local socket
access passed all 137 tests. No semantic failure was skipped. The first peer
Clippy check reported a large launch future; the final check passed after the
localized `Box::pin` change. Dependency lock edits add existing dependency edges;
no dependency version or git pin was upgraded.

## Early format-bound enforcement

Review of the changed admission path found that an operator setting above 65,536
could admit a larger logical header even though the operations decoder would later
reject that span. The service and structural assembler now intersect the local
quota with the format bound before retaining witnesses. The existing lazy-storage
test keeps its allocation assertion at the largest valid span; its old billion-block
header is now a rejection case alongside 65,537 and u64::MAX. The capability RPC
regression also sends a 65,537 header to a signer configured above the ceiling and
checks resource rejection with zero signing calls. This strengthens early
admission; it does not remove the later complete DA/window binding.
