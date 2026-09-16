# Lifecycle transport validation

2026-09-16, `feat/network-observability`, Rust 1.94.0. No running node, public RPC,
signer, database or live deployment was changed by these tests.

Commands (all passed):

```bash
cargo +1.94.0 test --locked --offline -p eez-telemetry
cargo +1.94.0 clippy --locked --offline -p eez-telemetry --all-targets -- -D warnings
cargo +1.94.0 fmt -p eez-telemetry --check
```

Ten deterministic tests cover strict configuration/defaults, bounded chunk
encoding, reordered/exact-duplicate replay, incomplete chunks, conflicting
duplicates, digest corruption, unknown versions, scope mismatch, oversized
claims, extra secret-bearing fields, writer fencing, restart without traffic,
permissions, concurrent sequence assignment/publication, immutable IDs across
rotation, byte/age retention, full queue with control bypass, late shutdown
publication, and actual Linux ENOSPC via `/dev/full` with a preserved partial
segment and recovery into a new segment ID. Fault tests do not fill shared disks.

An initial test run correctly rejected overly permissive temporary directories.
Fixtures now explicitly provide private directories; production validation was
not weakened. No existing tests were removed or skipped.

Remaining gates: native transition/configuration wiring, independently verified
metric exposition, PostgreSQL chunk/offset transactions and gap classification,
hard-volume and lagging-reader drills, writer process crash, runtime performance
comparison, and complete stage-2/3 deployment acceptance. Library tests do not
establish native protocol neutrality or production readiness.
