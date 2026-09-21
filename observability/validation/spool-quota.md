# Physical lifecycle spool validation

2026-09-17, `feat/network-observability`, after checkpoint `c36bbd6` and the frozen
main baseline `5469c88860408daf776ee812156dd280c1a9e262`. No protocol encoding,
production spool implementation, native runtime, operator volume or live service
was changed. This is R27's isolated physical-volume/reader drill, not full R27
acceptance.

The [machine-readable report](spool-quota.json) records **six passed groups**.
A fixed **3,221,225,472-byte (3 GiB) ext4 image** inside a disposable
container imposed the storage bound. Filesystem metadata and held-unlinked data
consume that image. This was not a tmpfs-size or process-memory substitute.
The fixture had a separate 1 GiB memory guard; OOM counters were zero and Docker
reported `OOMKilled=false`. Page-cache reclaim reached that guard without killing
the writer, Alloy or filesystem driver. Only owned containers/images and read-only
fixture inputs were used; the container and private mount were removed afterward.

## Requirement mapping and observations

| Requirement | Mechanism | Observed evidence |
| --- | --- | --- |
| Hard volume capacity includes metadata | Exact-size ext4 image, mounted privately with fuse2fs | Image stayed 3 GiB throughout. Ext4-reported capacity/usable data are lower because metadata and reserved space consume capacity; exact values are in JSON |
| Retention is not physical usage | Real Writer at 64 KiB segments/128 KiB retained files; separate stopped Alloy and lagging descriptor | Visible lifecycle files totaled 56,522 bytes while a zero-link retired lifecycle inode retained 36,864 allocated bytes |
| Open-unlinked files remain charged | Write through a file descriptor after unlink, then try another allocation | Held ballast reached 2,804,604,928 allocated bytes; another allocation returned ENOSPC without growing the image |
| Observer reader does not pin rotation | Real `reader::read_segment` before/after rotation, with returned buffers still alive | Descriptor counts remained 3→3; native fixture held no old-inode descriptor after rotation while deliberately stopped Alloy still did |
| Native API remains nonblocking during filesystem loss | Real `Publisher::publish` and asynchronous Writer | 100 calls accepted in 0.518 ms; first observed failure reported 94 dropped records and 1 I/O error(s), with explicit sequence bounds |
| Alloy closes disappeared targets | Both active/closed patterns, five-second discovery, SIGSTOP/SIGCONT of only the owned child | Deleted descriptor closed in 0.051 s after resume in this run; not a production timing guarantee |
| Loss survives recovery | Release ballast, existing writer backoff, new segment/control, successful publication and shutdown | Recovery controls retained cumulative dropped count/sequence bounds and I/O errors; recovered record count increased and shutdown drained |

The total recovery loss includes publications deliberately attempted during the
existing I/O backoff, not only the initial 100-event burst. A queued publication
acknowledges acceptance to the telemetry channel, not durable persistence.

FUSE uses an internal `.fuse_hidden*` backing entry for open-unlinked files. The
actual open inode reports link count 0 and remains allocated until its last reader
closes. The physical cap includes this internal backing storage; the writer's
visible `.active`/`.closed` retention inventory does not. This behavior and
filesystem reserved blocks explain why visible file sums and `f_bavail` are not
sufficient physical-cap proofs.

## Executed artifacts and commands

The fixture reuses the public native Writer/Publisher and bounded reader APIs;
its event/config shape follows the existing telemetry transport tests. It adds
no production dependency or protocol task. The image contains pinned Alloy and
exact signed-repository Debian filesystem tools, recorded in JSON.

```sh
cargo +1.94.0 build --locked -p eez-telemetry --example spool-fixture
docker build --tag eez-spool-quota-fixture:local observability/tests/spool-quota
python3 observability/tests/smoke-spool-quota.py \
  --report /tmp/eez-spool-quota-final.json
python3 -m py_compile observability/tests/smoke-spool-quota.py
```

- Native executable SHA-256: `b59395ff27ecaf9be395b17acd3e4711210c43ea4c29ee85c0766c851a6a449f`.
- Disposable image ID: `sha256:4755662acb4d03b769d09d71696a3ecdaec6eaf76aa8abdcfc8760c712364e6b`.
- Binary build log: `/tmp/eez-funding-spool-example-build.log` (coordinated Cargo slot).
- Full run: `/tmp/eez-spool-quota-final.log`; native and Alloy diagnostics are
  `/tmp/eez-spool-quota-final-native.log` and `-alloy.log`.
- The harness copies immutable binary/script/config snapshots before bind mounting;
  the container independently verifies the mounted executable digest. Earlier
  intermediate runs used the shared target path, which could be replaced by
  another feature-set build; they are not the final executable-provenance record.

## Preserved failures and corrections

The [failed reports](spool-quota-failures.json) retain both initial physical-drill
failures. The first 4 MiB allocation failure left room for a 4 KiB write, correctly
failing the saturation assertion. The fixture now exhausts remaining extents at
filesystem-block size. The second run saturated before unlinking its pressure
file; FUSE needed metadata space for its hidden open-file backing entry and
returned ENOSPC. The fixture now unlinks before filling through the open handle.
Neither correction reduces capacity or weakens the allocation-failure assertion.

Setup evidence also retains `/tmp/eez-spool-quota-image-build.log`: the Debian
website's then-listed 1.47.2-3+b11 package was absent from the signed repository.
The repository exposed 1.47.2-3+b12, which is now pinned in the test Dockerfile.
The initial capability-stripped apt query could not switch to its sandbox user;
the query was rerun with ordinary disposable-container capabilities. No host
package was installed. A separate 32 MiB capability/unlink probe preceded the 3 GiB
run; it is not counted as physical-cap acceptance.

## Remaining R27 acceptance

This evidence proves the stated isolated disk/reader and native telemetry-API
behavior. It does **not** prove canonical protocol progress during ENOSPC,
collector outage or a stalled reader by itself. The separate
[native canary](native-telemetry-failure.md) now verifies canonical progress with
startup storage unavailability and a stalled trace collector; dynamic ENOSPC and
lagging-reader protocol workload acceptance remains open. The provisional <5%
overhead budget, mutex hold times, sustained-load behavior and deployment-host
quota/mount provisioning remain open. Native library/process correctness and
this fixture must not be presented as those missing measurements.

See the [repeatable storage runbook](../runbooks/lifecycle-storage.md).
