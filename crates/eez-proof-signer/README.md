# eez-proof-signer

`eez-proof-signer` is the EEZ validation and attestation daemon. It accepts one
composer-streamed L2 block window, re-executes every block with Stateless,
binds the supported settlement effects and DA payload to that execution, and
signs the independently recomputed public-input hash.

- [`SPEC.md`](SPEC.md) is the normative protocol and compatibility contract.
- [`../eez-control-rpc/SPEC.md`](../eez-control-rpc/SPEC.md) specifies how a
  Composer constructs prover requests and uses their responses for L1 submission.
- [`docs/README.md`](docs/README.md) is the entry point for architecture,
  validation evidence, settlement, lifecycle, and operations explainers.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) records contributor constraints and verification
  commands.

The current production backend is in-process Stateless. The service is
bounded and fails closed: any admission, validation, settlement,
deadline, or signing failure returns no signature.

The signer loads the complete L2 genesis, the local L1 chain profile, and the
registry/manager/proof-system identities. It accepts the offline v2 profile
through `Begin -> Block* -> Finalize`; each request carries the complete L1
header evidence needed for its chosen anchor. It never connects to an L1 or L2
RPC, including during startup, retries or restart.

The signer checks execution and public inputs conditional on that evidence.
`RollupX.getCustomData()` binds the same context to the actual accepted L2
commitment and L1 anchor when settlement verifies the proof. The ECDSA backend
still requires trust in the configured attester; it is not a ZK proof.

This execution profile is active from a fresh genesis with new contracts and
fresh datadirs. Existing networks cannot follow it as a binary-only update.
See [operations](docs/operations.md) for configuration, bounded sessions and
shutdown, and the [v2 profile](../eez-control-rpc/STREAMING_SPEC.md) for wire rules.
