# Native history cold feasibility checkpoint

Historical checkpoint: its pending/open statements record the architecture
selection stage and are superseded by the
[final prepared checkpoint](native-prepared-checkpoint.md) and
[coverage ledger](../../../proposals/derivable-chain-operations-coverage.md).

Status: debug-source preflight passed; the larger debug-source run was stopped and replaced with the actual durable witness reader. Corrected measurements pending. This does not close
CO-30/31/38/44/46. In particular it is not yet a largest-window posting or follower
recovery test.

The existing `derivable_operations` native checkpoint now has two explicitly
invoked large-history variants. `EEZ_TEST_DERIVABLE_HISTORY_CEILING` selects a
ceiling from 5 through 65,536, default 65,536. Both existing native fixtures use
K=5; the actual span is the greatest multiple of five at or below the ceiling.
This preserves the configured grid, so 65,536 selects 65,535 actual blocks.
Ethereum K=6 deployment geometry still needs its separate release run.

A test-only past L2 genesis triggers the existing capped production catch-up.
L1's current genesis and slot clock remain intact, and the registered L2 genesis
commitment is recomputed. The producer uses finite speculative depth 65,540.
Catch-up production time is not reported as follower recovery time. Real native
system-contract transitions execute in these historical blocks. Existing setup
contract deployments form a sparse ordinary workload; this is not labeled an
all-empty benchmark.

The test pins a terminal and walks its parent hashes back to the exact genesis,
then independently encodes its input recipe. Block inputs and witnesses are read from the production hash-keyed durable store
by exact identities, with a channel capacity of two. The original debug-RPC
regeneration prototype is superseded. The harness does not hold
a whole witness window or raise the signer's quotas. It reports exact encoded
operations bytes, full-block baseline bytes, physical blobs, generated request
bytes/items, and stream-plus-proof time. If a quota rejects the request, generated
counts can include a small buffered suffix beyond what the signer admitted.
A rejection fails the checkpoint; it must not be reclassified as success or
avoided by raising quotas just to obtain a cold timing baseline.

The large feasibility variants stop after verifying the actual signer's digest
and recovered identity. They do not claim that this conditional request was
applied on L1, that the target deadline was met, or that a clean follower replayed
it. The existing small native variants still perform actual blob posting and
clean follower replay; those remain separate checks. Full largest-native posting,
empty-workload, 20-repetition, resource, crash/restart and reorg gates remain.

Example execution after building the current native role binaries and contracts:

```sh
EEZ_TEST_DERIVABLE_HISTORY_CEILING=512 \
  cargo +1.94.0 test --locked -p eez-node-e2e --test derivable_operations \
  ethereum_native_large_history_cold_checkpoint -- --ignored --nocapture --test-threads=1
```

At a 65,536 ceiling, the test now measures all specified prefix ceilings
512, 1,024, 4,096, 10,000, 43,200 and 65,536 from one captured history per
profile. It records every prefix result and fails the test if any checkpoint
fails; a smaller passing prefix does not hide a rejected larger window. Keep the source and executable identities, logs, quota failures
and timings. Any incremental proving implementation must still satisfy the final
full ledger; this checkpoint selects architecture, not a narrower definition of done.


## Ethereum preflight result

The initial 512-ceiling run passed with 510 actual blocks on K=5. Actual operations
were 1,754 bytes versus 390,910 full-block baseline bytes, in one physical blob.
The stream generated 3,371,620 known protobuf-field bytes and 19,223 witness items;
streaming plus cold proof took 3,478 ms. History/setup took 58,680 ms and the whole
test took 64.44 seconds. These are observations on the
[recorded host](checks/native-history-host.json), not a maximum-window or release
performance claim. The run verified the actual signature/digest, and intentionally
did not perform L1 posting/follower replay. [Log](checks/native-history-512.log),
[executables](checks/native-history-512-binaries.sha256).

The subsequent grid run adds per-candidate hashes, timestamps and digest fields
and reuses the captured history for every smaller required prefix. Its
[executable identities](checks/native-history-grid-binaries.sha256) differ from
the first harness run; those first results are not claimed for the later test
executable. Role binaries are unchanged. Final timing runs must also record peak
RSS/disk and exclude unrelated build activity; current compiler checks run during
history creation, before the grid's proof measurements begin.


Resource collection uses [measure_process_resources.py](measure_process_resources.py)
with the native test process PID and an output path. It reads only process status
and I/O counters, follows that test's descendants, and exits when the original
root process exits (including a start-time check against PID reuse). It records
sampled RSS, process high-water marks and read/write counters for the test,
Composer/embedded L1, and standalone signer. It does not inspect environments,
command arguments, witness contents or operator keys. Sampling every two seconds
adds no execution authority. This small collector is retained because the existing
byte measurements and Cargo/native harness do not collect per-role resource peaks
across both fixture lifetimes. Raw samples stay local; retain their hash/location
and the final compact peak report with the executed run.


## Corrections exposed by the maximum-history run

Ethereum generated 65,535 genuine native blocks, retaining the genesis settlement
cursor, in 1,090,545 ms. The first historical prefix then spent 498,425 ms
regenerating 510 old witnesses through debug RPC. This diagnostic does not measure
production witness-store reads or establish signer replay infeasibility. The run
was stopped before completing the grid; no Gnosis capacity result was obtained.
[Diagnostic record](checks/native-debug-history-diagnostic.json).

The source history and configured genesis are retained locally at
`/tmp/derivable-native-history-retained/2562469`, including a manifest and the
actual genesis/65,535 terminal hashes queried before shutdown. The original writer
has exited. `witness-db/mdbx.dat` preserves its final committed database; it is not
an independently invented witness corpus. The artifact is local-only and must be
made retrievable or regenerated before release evidence is considered complete.

The corrected native harness uses the production `WitnessDb` reader, on blocking
workers, for both ancestry and streamed witnesses. A retained-history mode starts
a fresh standalone signer and exercises the same proof helper without a live
producer or old parent state. This permits a genuine cold resource check against
the preserved native corpus. Signature/public-input checks remain required.

The run also exposed a producer bug: after a missed opportunity or catch-up burst,
a range above 65,536 was classified as a generic construction failure. Policy
mode consequently never attempted the existing bounded historical-prefix path.
`EmissionLimits::checked_window_span` now returns the typed `window_limit` capacity
failure; stale/backwards endpoints remain construction errors. The discriminating
unit regression uses the observed 66,630-block backlog, obtains the legal 65,535
prefix, and covers exact and invalid boundaries. All 208 Composer tests passed;
[log](checks/window-overflow-tests.log). The stopped run used the pre-fix binary;
this fix still requires renewed native validation.


## Corrected durable-source result

The preserved Ethereum native corpus was replayed through a fresh standalone
signer using `WitnessDb`, after the original producer was stopped. The 10.99-second
checkpoint test **failed its acceptance assertion**: 43,200 and
65,535 blocks cannot fit the unchanged one-million-witness-item request quota.
Both failed during ingestion at 1,000,024 admitted/attempted items. No quota was
increased. The 65,535-block recipe was 3,442 bytes in one physical blob versus
50,139,220 bytes in the exact full-block baseline. The smaller 510/1,020/4,095/10,000
requests independently validated and signed; 10,000 took about 2.7 seconds.
The first request includes cold cryptographic initialization and is not a warm
latency estimate. Every successful response's digest and attester were verified.

[Structured results](native-durable-cold-checkpoint.json),
[full checkpoint log](checks/native-durable-cold-checkpoint.log),
[actual native corpus endpoints](checks/native-history-corpus-metadata.json),
[retained file identities](checks/native-history-corpus-manifest.json).
The database artifact remains local-only at the path above. The test streamed
a bounded suffix beyond the rejected item because tonic transport can prefetch;
`*_generated` values are not claimed to be fully admitted request totals.

This is concrete evidence that merely raising the block-count ceiling cannot
satisfy the accepted largest-window requirement under existing quotas. It selects
the planned incremental validation/reference work; it does not authorize raising
quotas, weakening validation, treating a rejected largest window as complete, or
waiving the remaining actual posting/follower and recovery gates.

## Gnosis checkpoint at the observed Sync residue

The retained Gnosis corpus was checked with its actual Sync residue (reference
height 65,551, K=5). The legal endpoints were 511, 1,021, 4,096, 9,996, 43,196 and
65,536. The earlier zero-residue endpoints were a harness assumption and must not
be used as native capacity evidence. `native_capacity_uses_the_observed_sync_residue`
now covers both residues and K=6 independently.

The first four cold requests validated and signed; the two largest failed during
ingestion at 1,000,016 witness items against the unchanged one-million limit.
The 65,536-block recipe occupied 3,447 bytes and one blob, versus 50,138,637 bytes
for complete ordinary blocks. Exact terminal hash:
`0x887cdcc502b76f0148d25cbe015ac19096d2ab933ee9f36628802c72379afd03`.
The corrected test failed its largest-window acceptance assertion in 16.91 seconds.
Clippy was concurrent, so its timings are diagnostic, not release benchmarks.
Structured Gnosis results are included in the companion JSON. No successful
maximum-window posting, follower replay or cache-recovery claim follows from this run.
