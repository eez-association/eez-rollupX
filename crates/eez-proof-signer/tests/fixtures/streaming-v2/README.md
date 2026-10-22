# RollupX streaming v2 test vectors

This directory contains 29 implementation-derived request/response cases with
full execution witnesses, physical blobs, calldata and posting context.

See the [conformance guide](../../../../eez-control-rpc/conformance/README.md) for
the complete coverage matrix, exact fixture/server profiles, wire-only replay,
generation provenance, normalization rules and limitations. The
[manifest](manifest.json) hashes the separate generating source, fixture inputs and vectors.

The cases use real Stateless execution and settlement checks with **synthetic
L1 authority**. They do not establish cross-implementation interoperability or
establish live restart, reorg or parent-authentication behavior.
