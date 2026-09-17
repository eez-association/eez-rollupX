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

Bounded-reader follow-up, aligned to main `5469c88`: the telemetry crate now has
21 passing all-feature tests (seven additional reader/probe regressions). The
reader tests preserve partial-tail offsets across append and rename, reproduce
multi-pass replay, retain unknown raw records and oversized prefixes, reject
truncated/non-boundary checkpoints, enforce canonical filenames, and reject
symlink/non-regular paths. A real writer publishes 80 events, closes its segment,
and the reader replays all events with unchanged IDs/headers and exact payloads.
A replaced epoch lock remains unknown rather than following a symlink.

Executed with Rust 1.94.0:

```bash
cargo +1.94.0 test --locked --offline -p eez-telemetry --all-features
cargo +1.94.0 clippy --locked --offline -p eez-telemetry --all-features --all-targets -- -D warnings
cargo +1.94.0 fmt --all --check
cargo deny check advisories bans sources
cargo audit
```

All passed. The added direct `libc` dependency reuses the existing locked version;
no package versions changed. Audit retains the same 14 pre-existing allowed
warnings with unchanged policy. These tests use isolated temporary files and
local sockets. SQL ingestion, process-kill recovery and hard-volume drills remain
unverified; unlinking a fixture is not evidence of a physical storage quota.


Physical-volume follow-up (2026-09-17): [spool-quota.md](spool-quota.md) records six
passed groups on a fixed 3 GiB ext4 image: open-unlinked accounting, real native
writer ENOSPC/loss/recovery, bounded-reader descriptor release and deliberately
stopped Alloy cleanup. Earlier outstanding physical-cap statements above describe
their dated checkpoints. Native protocol progress under telemetry faults and the
overhead/deployment gates remain open.
