# Chiado Beacon blob selection

| Field | Value |
| --- | --- |
| Date and status | 2026-09-18; regression fixed and tested; fresh deployment pending |
| Source | Patch following `b9c47f1317b116784ce49f5564b0ebfca6970a18` |
| Protocol | `3c0cdf4955941a5c23639346932b55b33babebbd` |
| Scope | Beacon response authentication and selection, both supported formats |

The old Chiado composer was repeatedly failing startup while applying EEZ packing
rules to a Beacon response blob. A synthetic regression reproduces the mechanism:
a valid EIP-4844 blob from another application can violate EEZ's narrower packing
rules. This does not establish which application produced the historical blob.

The source now authenticates physical blobs, selects the transaction's exact
execution versioned hashes, and only then applies EEZ packing and native decoding.
Legacy advertised commitments still must match physical bytes. Selected invalid
blobs, malformed physical data and missing expected hashes remain rejected.

## Executed checks

From the worktree root using Rust 1.94.0 and the locked dependency graph:

- Baseline mixed-response regression failed with the expected packing error.
  An earlier sandbox attempt failed to bind loopback; it is not the reproduction.
- `cargo +1.94.0 test --locked -p eez-l1 --lib`: 145 passed, none ignored.
- `cargo +1.94.0 clippy --locked -p eez-l1 --all-targets -- -D warnings`: passed.
- Formatting and diff whitespace checks passed.

Regression coverage includes unrelated valid blobs before and after selected EEZ
blobs, both Beacon API formats, selected invalid packing/native streams, missing
hashes, malformed length/field elements and incorrect legacy commitments.

Local-only logs (not portable CI artifacts):

| Path | SHA256 |
| --- | --- |
| `/tmp/eez-blob-selection-baseline-unsandboxed.log` | `3eb0ede031bc856d2a71fa1eb80d3cb78e63baf42fe0a19dbc7ec5ef8f1fe9fa` |
| `/tmp/eez-blob-selection-fixed.log` | `91a90bc167447a009fa9ba19bf0f01cfc483112f0e33bfc4ab03b74afe859bd6` |
| `/tmp/eez-blob-selection-clippy.log` | `bfe066c5c5bf998180a181160d0f499aa7a587882773c58da67ba5f2413612dc` |

No performance comparison was run; the user has deferred those measurements.
This record does not claim a complete CI release matrix or a live deployment.
