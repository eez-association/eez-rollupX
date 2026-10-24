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

## Next check

The same unmodified test is selected by the Tokio candidate's
[run 40116791686](https://github.com/eez-association/eez-rollupX/actions/runs/40116791686).
Preserve its first result and, on failure, full admission/prepared summaries,
warmup proof and finishing samples, actual selection widths, capacity reasons,
and native preflight outcomes. A synchronized fixture is justified only if
those records show that the required offered workload was absent at selection.
Otherwise investigate the concrete resource or timing rule. Keep **accepted > 12**,
all semantic assertions, and the shared execution/deadline guards intact.

Only source tracing, Git comparisons, GitHub reads and structured log extraction
have run for this investigation. No local build, process reproduction, retry,
service restart, or dependency modification was performed.
