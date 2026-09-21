# Lifecycle spool physical storage

Lifecycle retention is a visible-file policy, not a disk quota. Production node
hosts must provide a dedicated filesystem or quota-backed volume with a hard
**3 GiB total capacity**, including filesystem metadata and deleted files still
held open by readers. The native defaults retain at most 2 GiB of segment files
and 24 hours, with 64 MiB segments; those defaults do not themselves impose the
physical bound. Do not place this spool on an unbounded protocol-state volume.

The bounded observer reader opens files only for one read pass. Alloy must match
both `*.active` and `*.closed`, refresh discovery and stop readers for disappeared
paths. Alloy's [file discovery documentation](https://grafana.com/docs/alloy/latest/reference/components/local/local.file_match/)
defines the refresh setting; the pinned image is exercised by this drill.
The tested five-second discovery configuration is
[`tests/spool-quota/alloy.alloy`](../tests/spool-quota/alloy.alloy). Its empty
forwarding list isolates descriptor handling in the drill; a deployed node-host
agent needs its separately configured private Loki receiver and persistent
positions. It is not a replacement for the normal backend configuration.

## Disposable physical-cap drill

Run from the repository root on Linux, with Docker and `/dev/fuse` available.
Coordinate Cargo builds with other work. No running node or operator directory is
used. The fixture creates its own fixed-size ext4 image inside a disposable
container and removes that container on success or failure. Only the reviewed
fixture binary, script and Alloy configuration enter through read-only mounts.

```sh
cargo +1.94.0 build --locked -p eez-telemetry --example spool-fixture
docker build --tag eez-spool-quota-fixture:local observability/tests/spool-quota
python3 observability/tests/smoke-spool-quota.py \
  --report /tmp/eez-spool-quota-validation.json
```

The test image uses the existing pinned Python and Alloy images plus exact Debian
filesystem-driver package versions. A package-version mismatch fails the build;
do not silently replace a version. The report captures the built image ID, native
binary digest and installed filesystem-tool versions. This locally built test
image is not a published monitoring image.

The isolated mount requires `SYS_ADMIN`, access to `/dev/fuse` and an unconfined
AppArmor profile inside that test container. It receives no host block devices,
Docker socket, network access or writable host mounts. The script checks available
host memory and private backing storage, then limits the container to one GiB RAM,
no additional swap, two CPUs and 128 processes. The physical storage limit comes
from the fixed **3,221,225,472-byte ext4 image**, not the memory limit. OOM is a
failure. Filesystem metadata and reserved space reduce usable data capacity.

The drill checks these actual operations:

1. Start the real telemetry `Writer` with its validated 64 KiB segment /128 KiB
   retention fixture settings and make a bounded native read pass.
2. Let pinned Alloy open the active segment, then deliberately stop that reader.
   Keep a separate read-only descriptor open as an independent lagging reader.
3. Publish enough records to rotate beyond retention. The old inode has zero
   links and remains charged to the filesystem while visible retained bytes stay
   within their much smaller limit. The native reader retains no old descriptor.
4. Fill an already-unlinked file through its open descriptor. Exhaust the tail at
   filesystem-block granularity, since a failed large allocation may leave room
   for small writes. A further allocation must return `ENOSPC` without growing
   the image.
5. Publish 100 native events under filesystem exhaustion. Calls must return within
   the fixture's one-second bound; cumulative writer I/O errors, drops and dropped
   sequence bounds must become explicit. This is a local API check, not an
   overhead benchmark or protocol-progress assertion.
6. Close the pressure descriptor, resume Alloy and verify it closes the deleted
   lifecycle inode within 30 seconds. Read passes retain no descriptors even while
   returned record buffers remain live.
7. Wait through the existing writer I/O backoff. A new segment must carry controls
   preserving the cumulative loss, accept another event and shut down cleanly.

FUSE may preserve an open-unlinked file with an internal `.fuse_hidden*` backing
entry. The test checks the actual open inode's zero link count and charged blocks;
visible `.active`/`.closed` retention excludes that internal entry. This is one
reason retention counts cannot prove physical usage. Unlinking an open file when
already full can itself need metadata space, so the pressure file is unlinked
before saturation.

The JSON report and sibling native/Alloy logs survive container removal. Preserve
failed reports alongside the corrected run. A failed capability check or missing
driver is not a passed quota test; it requires an appropriate isolated test host.

## Deployment and remaining acceptance

This drill establishes an isolated, real filesystem capacity boundary and the
transport/reader behavior stated above. It does not provision the deployment's
node-host volume or verify its host-level quota after restart. Record the chosen
filesystem/quota mechanism, mount identity, owner/read-only group, capacity,
retention, inode pressure and reader configuration in the deployment manifest.

The [native fault canary](../validation/native-telemetry-failure.md) verifies real
protocol work with unavailable startup storage and a stalled trace collector.
A full R27 run must additionally exercise real native protocol work while
collectors are unavailable, the spool is full and a reader lags, checking that
accepted work still reaches its intended canonical outcomes. It must compare
telemetry-disabled/enabled performance and measured lock hold time against the
proposal's provisional overhead budget. File counters and a fast `publish` call
alone cannot establish either result. See the [validation record](../validation/spool-quota.md).
