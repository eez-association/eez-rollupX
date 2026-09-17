# Native protocol progress under physical ENOSPC

The required isolated native quota runner passed four groups using the actual
Composer/signer and archived-compatible cross-chain executable. Evidence and
exact artifact/input hashes are in [native-enospc.json](native-enospc.json).
This is semantic fault/recovery evidence, not an overhead measurement.

The fixture first proves the writer is enabled and has persisted real records
without loss. It then fills its private, fixed 3 GiB ext4/FUSE image using a held,
unlinked ballast file, with Alloy stopped while retaining a descriptor for the
actual native spool. It waits for the native writer's own I/O-error metric before
submitting both signed sources. Both destination effects, receipts, canonical
L1-root/L2-safe convergence and the independently checked posting signature
complete while the filesystem remains full. Writer losses remain explicit.

After capacity release, the test requires a newly written control record with a
timestamp at or after that release. It then submits two new nonce-distinct
signed sources, repeats semantic/signature acceptance and reads their exact
assembled `Admitted` and `Terminal(Committed)` records from the real spool.
Recovery therefore cannot be satisfied by an old control or a count accumulated
before ENOSPC. Cumulative loss bounds remain present, without restarting the node.

The container has no network, a 4 GiB memory limit, four CPUs and bounded process
count. The fixed filesystem image does not fill a host filesystem. The run hit
memory-limit pressure from filesystem page cache, but recorded no OOM or OOM
kill; this native workload budget is separate from the 512 MiB observer/database
budgets. Only owned fixture resources are removed. Logs and failed native
datadirs remain alongside the selected local report.

The runner executes the resolved immutable image ID, verifies actual Anvil
1.7.1 and snapshots only public runtime inputs: builder stub, genesis fixture and
compiled contract JSON. It checks the 88 input hashes again after execution.
Native test and runtime binaries are copied and hashed before mounting. The
shared checkout and operator configuration are not mounted.

The dedicated Rust test is intentionally ignored in ordinary native execution;
`smoke-spool-quota.py --native-test ... --runtime-bin ...` must invoke it explicitly
and fails on missing controls, missing test selection, missing artifacts or
missing filesystem prerequisites. The required cross-chain CI runner prepares
the pinned quota image. `native-tests.py` then passes the already verified
cross-chain executable and sealed runtime into this gate without a Cargo or
runtime-build fallback. Local success does not claim that remote CI has run.

Logs: `/tmp/eez-native-enospc-recovery-reviewed.log` and the adjacent JSON/native
log directory. The first attempt failed before native execution because a
read-only snapshot directory was inaccessible with all capabilities dropped;
only that owned public directory's traversal permissions were corrected. The
next run proved during-full semantics, but its recovery count could predate
capacity release. That initial recovery claim is superseded by the fresh-record
regression above. Those earlier logs/reports remain retained.
