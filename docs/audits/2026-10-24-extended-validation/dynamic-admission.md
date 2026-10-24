# Delayed mixed-wave admission: first failure

2026-10-24; investigation remains open. No production or test change has been
made for this failure. Exact identities, extracted events and hashes are in
[dynamic-admission.json](dynamic-admission.json).

The b0195b0b candidate failed
`dynamic_admission::ethereum_delayed_start_mixed_wave_after_small_post_replays`
in [CI run 40112652163, Cross-Chain 1/4](https://github.com/eez-association/eez-rollupX/actions/runs/40112652163/job/120935600473).
The assertion at `dynamic_admission.rs:435` requires at least one
`admission.prepared` with **accepted > 12**. It failed at 17:35:19 UTC after
96.143 s, before the cancellation notice at 17:35:23. The later Tokio upgrade
cancelled unfinished lanes; it did not cause or erase this individual failure.

Before that assertion, the test had checked all 64 inbound and 64 outbound
successful receipts, safe follower convergence, both final contract values and
both sender nonce advances, and nonzero real blob preparation. The failure is
the intended packing regression, not a demonstrated execution or replay mismatch.
The node and signer were still running at panic. Assertions located after the
panic, including the complete attempt-count scan, did not execute.

## Evidence and limits

The preserved tail shows finite candidate snapshots of 117, 106, 95, 84, 73,
62, 51, 40, 29, 18 and 7 sources. Thus a queue too small to offer thirteen
sources is not an explanation. Seven retained proof-request records carry
11 groups; the last carries 7. The final seven-source candidate consumed its
entire snapshot with no capacity deferrals: 253 ms preparation, including
237 ms blob work, from a 977 ms window. Its gas projection was 947,528 against
a 15,978,300 planning cap. These final figures do **not** establish why earlier,
larger snapshots stopped at eleven.

The diagnostic retains only filtered timelines and short log tails
([testkit](../../../crates/eez-testkit/src/lib.rs#L4574)). The large candidates'
`summary`, first warmup timing and possible earlier failures are missing.
The cancelled run's artifacts API returned no full node logs/datadirs. The job
log is retained locally under the ignored `artifacts/pr-readiness/ci-candidate/40112652163`
directory; its SHA-256 is recorded in the JSON. This local copy is unavailable
to another checkout. The GitHub link is subject to CI log retention.

## Static comparison with a50c326d

- `mixed_wave`, `AutomaticAdmission`, `Selection`, and the production prefix of
  `prover_retry.rs` are byte-for-byte unchanged. The test file's only other
  differences update two estimator-version expectations in `single_sync_wave`.
- The new profile adds history execution, native transactions and the offline
  proof context. Gas framing accounts for the required accepted-root pin. These
  alter actual work, so unchanged estimation code alone is not a performance
  comparison against the base.
- `signals_since` reads the whole log after the recorded cursor. `prepared` is
  emitted before proving and final native preflight. A normal persistent
  preflight cap of eleven would need an earlier prepared batch of at least
  twenty-two, which would already satisfy the assertion. No evidence of log
  loss was observed.
- Large-snapshot candidate IDs advance by two; this is consistent with the
  production FIFO reconstruction path, but the missing summaries prevent a
  conclusive attribution.
- The accepted [timing contract](../../configurable-settlement-policy.md)
  retains a slower sparse proof's extrapolation until a wider observation
  dominates it. A small cold proof slightly slower than later eleven-group
  proofs can therefore remain relevant under this test's roughly one-second
  window. This is a plausible timing/FIFO mechanism; the warmup observation is
  missing, so it is not the established cause of this run.

## Existing local comparison (no new execution)

The earlier full source suite already passed this exact case; its retained
node log is identified by the fixture's block-1 hash in both the suite output
and the node log. That warmup proved one group and fifteen witnesses in
21.13 ms, with 166.51 ms of blob preparation. The first 128-source snapshot
was rebuilt under `pipeline_time` from 25 to 17 to 16 groups, then prepared
sixteen. Later prepared candidates contained thirty and seventy-five groups.
The [source-suite identity](../2026-10-24-pr-readiness/native-process-suite.json)
and copied executable/log hashes in the JSON preserve that result.

This is evidence against a universal eleven-source protocol ceiling in the
new history/native profile. It is **not** a comparable performance baseline:
these are the earlier pre-R03 local debug executables, while the failure used
release CI artifacts on another runner. It neither proves the failed run's
stop cause nor validates the subsequent Tokio candidate.

## Subsequent CI result and exact-payload diagnostics

The same unmodified test passed on the Tokio 1.53.3 candidate `053668df` in
[run 40116791686, Cross-Chain 1/4](https://github.com/eez-association/eez-rollupX/actions/runs/40116791686/job/120951087964),
in 71.230 s at 18:42:30 UTC. The lane completed 33/33 tests; all eight process
partitions completed 174/174, with five explicit ignored tests outside selection.
This pass reaches **accepted > 12** and the later semantic assertions. The CI
log contains the passing result, but not the complete node admission/timing
logs or the actual accepted widths. It does not establish why b019 stopped at
eleven, nor show that Tokio fixed it. The first failure remains unresolved.

The root agent then ran the same case serially with each producer's verified
runtime and test payload, without rebuilding Rust. The original b019 artifacts
passed in 90.679 s; 053 passed in 137.253 s. Each selected one test, leaving 178
outside the filter, with all original assertions intact. Full logs, executable
identity, commands and hashes are retained in the JSON. These are diagnostic
reruns: the original CI failure remains an unresolved intermittent failure.

The observed prepared widths, including the one-source warmup, were:

| Payload | Prepared source counts | FIFO rebuild summaries | Empty opportunities |
| --- | --- | --- | --- |
| b019 | 1, 14, 14, 14, 1, 1, 1, 22, 61 | 27 | 3 |
| 053 | 1, 12, 1, 1, 1, 12, 1, 1, 1, 4, 1, 1, 16, 76 | 67 | 7 |

Each log records 129 source preflights (warmup plus 128 application sources),
matching proof-received/preflight counts for prepared candidates, and no proof
failure. In b019, the 27.019 ms one-group warmup remains an extrapolation term
above fourteen groups while the first two fourteen-group proofs are cheaper.
The third, at 46.456 ms, dominates that sparse sample; later candidates reach
twenty-two and sixty-one. In 053 the first twelve-group proof already exceeds
the 30.968 ms warmup, so sparse retention alone cannot explain its subsequent
empty opportunities.

Those empty opportunities explicitly stop at `pipeline_time`: the remaining
estimated work does not fit. They are not proof failures or literal exhaustion
of wall time; roughly 513–523 ms remain in b019 and 458–583 ms in 053. The logs
do not expose the full per-gate reserve breakdown, and logged proof durations
are sampled slightly after the trained duration. The numeric reconstruction is
therefore approximate. Full FIFO reconstruction also consumes time before the
same opportunity's next attempt.

This identifies a concrete admission-efficiency follow-up while preserving the
first CI failure's evidentiary limit. Neither single diagnostic is a controlled
performance comparison or evidence that Tokio improved packing. No production,
test, timeout or deadline rule was changed. A future fix must explain the
observed refusal before changing the fixture or estimator; retain **accepted >
12**, semantic checks, bounded admission and final execution validation.

The final runtime candidate `28049ff7` also passed the unchanged case in
[CI run 40123208325, Cross-Chain 1/4](https://github.com/eez-association/eez-rollupX/actions/runs/40123208325/job/120969794194),
in 76.571 s. That lane completed 33/33 tests. The log hash and exact PASS event
are recorded in the JSON; full prepared widths are not present in the passing CI
output. This is additional exact-candidate coverage, not a diagnosis or erasure
of the original intermittent failure.
