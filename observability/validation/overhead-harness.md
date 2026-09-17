# Native overhead harness validation

Status: harness verification and pilot work in progress; **no less-than-5% claim**.
The bounded experiment/decision contract is in [overhead-plan.md](overhead-plan.md).

The ordinary automatic-admission `mixed_wave` tests now share their workload and
semantic oracle with an ignored explicit measurement arm. Every wave checks both
source receipt sets, destination effects, nonce deltas, independent follower
convergence, no charged transport/local-deferral attempts, no poison/canonical
mismatch and an independently recomputed posted signature. Existing automatic
packing regressions retain their greater-than-twelve and blob-work assertions.

The measurement arm uses fixed Gnosis Prague native fixtures and direct production
Composer-to-signer RPC. A disables optional EEZ telemetry and retains an initialized
idle PostgreSQL reservation. B enables Composer metrics, lifecycle and traces,
signer metrics/traces, follower metrics, and the actual observer reading native
RPC and the owned spool. Both retain the same bounded healthy OTLP sink resource.
The owned spool is mode 0700. Early runtime gauges prove the requested B profile
started; final checks require actual exported spans, zero loss/writer errors and
canonical observer coverage for every warm-up and measured source hash.

Throughput uses successful sources divided by the sum of each wave's monotonic
submission-to-canonical interval. Inter-wave nonce/provider setup and signature
verification are excluded from that clock, while broader phase/resource windows
remain explicit. Observer catch-up/health has its own resource phase. The overall
resource bracket covers all measured stages through catch-up exactly once; do not
sum overlapping per-phase sample brackets. Backend scope includes PostgreSQL,
observer, exporter/scrape transport and the OTLP sink, not Prometheus/Tempo storage.

The controller checks fixed candidate hashes before and after each arm, starts no
Cargo processes, retains failures, records bounded raw resource/log evidence, and
uses ready/start/evidence/finish barriers. It stops resource sampling before
rendering the final report and releases native processes only afterward. Failure
cleanup is restricted to the fresh child process group and its exact named private
PostgreSQL fixture; cleanup errors cannot suppress the result file.

The optional HeldPool feature measures actual guard acquisition and lifetime,
records after unlock, preserves poisoning, caps samples, identifies callsites and
reports overflow. Shipping-feature binaries are used for primary comparison;
probe-enabled and disabled variants require separate calibration. The parser
rejects sequence gaps, overflow, uncertain clocks and guards crossing phase
boundaries. Ordinary exported logs exclude benchmark records.

## Checks completed

- Probe feature: five focused guard/config tests and all 245 Composer unit tests
  passed; strict all-target Composer/node feature Clippy passed.
- Paired analysis: 20 deterministic tests, including invalid identity/order,
  insufficient samples, contamination, quantization and confidence bounds.
- Probe analysis: 11 deterministic tests, including true phase-crossing guards.
- Runner: nine tests for bounded JSON/handshakes, exact probe log selection and
  frozen candidate replacement/path escape rejection, complete-record/source-population checks and actual reservation/genesis provenance.
- Resource helper: 18 tests including actual owned PostgreSQL whole-cgroup
  accounting, bounded HTTP behavior, stopped-sampler reporting and overall cost.
- Default shipping-feature role build passed in 5m 00s with Rust 1.94.0, optimized
  development profile. Final focused measurement-arm Clippy passed in 2.64s and
  executable build in 5.91s. This profile is explicitly not release-performance
  evidence.

## Retained diagnostic attempt

The first raw A/B attempt is retained at `/tmp/eez-overhead-pilot-default` on the
validation host. A passed native semantics in 98.04 seconds and resource capture;
small-queue preparation had one sample (72 ms), busy work two (581 and 434 ms).
B passed native workload execution but correctly failed observer health because
the fixture had precreated its spool with mode 0775; the real writer refused that
unsafe path. Its native test failed after 267.53 seconds. Both owned PostgreSQL
containers were verified absent. No second pair was launched.

That attempt is diagnostic only: its controller also rendered resources before
stopping the sampler, its outer stage clock included test verification work, and
other validation overlapped startup. The corrected fixture uses mode 0700, checks
the profile early, reports after sampler shutdown, preserves exact source coverage
and separates the throughput clock. Original results remain visible and are not
silently replaced by the corrected pilot.

The first observed sample population also shows that one wave is not a p95 gate.
The initial 12-wave bound could not provide 40 small-stage samples at the observed
one preparation per wave. The final test-only capacity now permits up to 64 waves
and explicitly declared arm/experiment budgets up to 1800/21600 seconds; defaults
remain 600/9000 seconds, without any relaxation of tail or regression criteria. A prospectively revised count/window, or an explicit
inconclusive result, is required before confirmatory collection. Neither sample
requirements nor the 5% threshold may be relaxed to turn these observations green.

## Completed sizing pilot and next candidate

[overhead-pilot.json](overhead-pilot.json) retains the corrected AB/BA sizing
experiment: four native semantic arms and stopped-sampler resource reports passed
in 460.26 seconds total. Each B arm independently checked all 138 warm-up/measured
source hashes as canonical committed outcomes, with real exports and no native
loss. Every private PostgreSQL cleanup was verified. These are optimized
development-profile runs with only one wave per stage; they do not establish p95
or the provisional 5% gate. The JSON retains raw durations/group populations,
resource scope, unattributed host CPU and all methodological limits.

Before a release candidate, the producer adds `preparation_ns` from the same
already-captured `Duration`, retaining the existing floored `preparation_ms`.
Checked conversion omits a nanosecond value that cannot fit u64 rather than
wrapping; measurement rejects a missing value. A focused unit test passed at
zero, sub-millisecond values, the u64 boundary and overflow. Actual A/B native JSON validation passed: see
[overhead-native-wiring.json](overhead-native-wiring.json). No additional clock read, span, label, configuration or protocol
behavior is introduced. The ordinary node-agent field projection remains
unchanged. Old pilot milliseconds are not reinterpreted as nanoseconds.

The next harness version emits actual reviewed configuration, inherited CPU
allocation, exact public genesis JSON/header/RPC identity and explicit capture
proof. Its genesis comparison permits only the existing shared current-timestamp
policy, retaining every other template field. It drains observer warm-up before
measurement, polls cached canonical counters during catch-up and runs the exact
source-set SQL check once, removing repeated expensive validation reads from the
backend workload. The refined A/B native wiring check passed in 239.10 seconds (A 91.78s, B
143.73s), including all 138 exact B source outcomes, real nanosecond JSON and
Gnosis helper/RPC genesis agreement. Source adapter checks the actual emitted
artifact contract independently. Concurrent workspace/Python validation makes
this functional evidence only; the prior pilot remains sizing evidence. The strict offline adapter is
specified in [overhead-input.md](overhead-input.md).


The final default-feature affected-package run passed 665 tests: 241 Composer,
4 control RPC, 75 observer, 287 signer, 15 client and 43 telemetry. Workspace
formatting and strict all-target Clippy passed. Optional-feature runtime lock
samples/calibration and release-artifact confirmatory performance remain open.


The final capacity extension validates integer bounds/defaults/combinations in
both producer and CLI. Python regressions also prove that final teardown/hash
time cannot turn an over-budget experiment into `raw_capture_complete`; the
original raw runs remain in the result. Frozen analysis binds the exact workload
and budgets. This is test infrastructure capacity, not a shipping runtime option.
