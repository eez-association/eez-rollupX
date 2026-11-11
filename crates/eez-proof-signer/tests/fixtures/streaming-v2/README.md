# Offline v2 test vectors

These 29 frozen cases contain full L1/L2 evidence and native settlement inputs
for the fresh-network offline profile. Expected values are reconstructed from
retained native execution inputs and the current Core proof-policy commitments,
never learned from the service responses under test.

The [conformance guide](../../../../eez-control-rpc/conformance/README.md) explains
coverage, generation, replay, normalization and limits. The [manifest](manifest.json)
records the generator, fixture, schema and vector digests. Signatures remain
conditional on the on-chain context checks; this suite does not claim independent
L1 consensus authentication or a ZK proof backend.
