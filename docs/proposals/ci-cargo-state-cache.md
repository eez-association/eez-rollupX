# Portable Cargo state for the release producer

Status: validated for an opt-in homelab merge (2026-09-26). Not merged and not a
replacement for the existing CI gate. Base: main `7c90266a5fe8cfdb46b249c61079badc2dce71a3`;
protocol: `9d13218a355e7d543ca8a9927eea2962600b716d`.

Implementation: `perf/ci-cargo-state-cache`, code commit `e941140e`. The
[final integration run](https://github.com/eduadiez/rollupX/actions/runs/36198290775)
passed on its first attempt at `1834e067`; its tree differs from that code commit
only by the isolated validation workflow. See final acceptance and limitations
below. No repository settings or existing deployments were changed.

## Decision supported by the experiments

Retain Cargo's source snapshot, dependency downloads and target together, then
reconcile the current tracked checkout into that snapshot with checksum rsync.
Always invoke Cargo; never treat a cache hit as permission to skip compilation
validation or test execution. Compile in bounded `docker run` containers and
package the exported binaries into the existing runtime images afterwards.

The [combined-state experiment](https://github.com/eduadiez/rollupX/actions/runs/36188091462)
restored real shipping/native/Composer builds in 1.534/1.231/1.082 seconds, all
fresh. The combined archive was 3,272,029,152 bytes, with 132.132 seconds to create
it. The entire restore job took 325 seconds, including a new failing-test canary.
Seven shipping hashes and all test executable hashes matched. A production
signer edit recompiled in 50.479 seconds and changed the executed CLI output.
These are homelab measurements, not a statistically controlled overall-CI
speedup or a guarantee for arbitrary Rust edits.

The current main baseline has a successful
[full CI run](https://github.com/eduadiez/rollupX/actions/runs/36161123764)
at `7c90266a`. Its producer step took 38m51s. The completed producer log records:

| Compiler-cache wrapper phase | Elapsed | sccache hits | Misses | Non-cacheable |
| --- | ---: | ---: | ---: | ---: |
| Shipping runtime | 799s | 1,965 | 49 | 187 |
| Native release archive | 1,229s | 171 | 44 | 35 |
| Composer unit archive | 142s | 536 | 40 | 8 |

All three phases reported zero cache read/write errors. The wrappers total
36m10s; the remaining producer time includes base resolution, export, packaging
and image publication. This confirms that an operational compiler cache still
leaves substantial work in this baseline; it does not establish the exact cause
or timing contribution of each non-cacheable request. Retaining valid Cargo
target state is intended to avoid invoking unchanged compilation/link tasks,
not merely improve the compiler-cache hit rate.

Baseline release unit/vector execution took 44s. Native suites passed in four
shards each: embedded job durations were approximately 10–16 minutes and
cross-chain jobs 14–17 minutes. The baseline Kurtosis job took approximately
33 minutes. The no-publication experiments execute the full native suites
serially and then Kurtosis, so their correctness-job duration must not be
compared directly to one production shard. Job-level timings include setup.

The [full correctness experiment](https://github.com/eduadiez/rollupX/actions/runs/36194784589)
finished with one embedded E2E failure; its cross-chain and Kurtosis suites
passed. The failure and separate final-integration pass are recorded below;
the experiment is not counted as a successful full correctness run.
Its hard-coded seed commit/cache key and fail-on-miss behavior must not be copied
into production.

## Integration boundary

- Keep the existing Docker builder as the default and explicit rollback path.
  Add one opt-in producer mode, without changing artifact schemas, test filters,
  Composer feature isolation, E2E partitions, provenance or promotion rules.
- Share the shipping command and export logic between the Docker build and the
  bounded-container build. Keep runtime image stages and deploy packaging under
  their current owner; do not introduce alternative runtime package definitions.
- Start production rollout on homelab. The small transport/invalidation fixture
  passed on hosted and homelab runners, but the real 20-GiB build was tested only
  on homelab. Do not silently impose this memory budget on standard hosted VMs.
  Hosted runs retain the current builder until independently measured.
- Use stable container source/target paths and the same locked release/toolchain
  and portable mold flags. Never mount the Docker socket, runner credentials,
  operator state or the shared Cargo home into the compilation container.
- Keep the compiler-cache backend out of the initial native-state path. The
  portable target cache already retains compiled dependencies; layering another
  remote cache can be considered after measuring misses, not assumed necessary.

## Cache contract

1. Stage exactly the tracked compilation inputs, including all embedded fixtures
   and current build/archiving scripts. No untracked files, `.git`, credentials,
   external symlinks or operator configuration. Check this inventory against the
   Docker build contract and test omissions/deletions explicitly.
2. A compatibility key includes a format epoch, architecture, actual compiler and
   native package inventory, release flags and build-selection contract. A full
   source hash identifies an exact snapshot. The lockfile/protocol can change
   only through the authoritative checkout; Cargo must see the changed inputs.
3. Restore only compatible state from the current ref/default-branch cache
   scopes. Only trusted default-branch runs write reusable state initially;
   pull requests consume but do not publish multi-gigabyte target caches.
   GitHub cache isolation does not make cached executables safe for untrusted
   runners or arbitrary cache writers: retain the repository's CI trust boundary.
4. Validate archive bounds/paths and metadata before reuse. A missing, incompatible,
   corrupt or future-dated cache falls back to a fresh owned state directory
   **before compilation**. Preserve the rejected cache/diagnostic reason. Do not
   retry a failed build or failed test as an implicit cache fallback.
5. Reconcile with `rsync --recursive --checksum --delete --perms --links`, without
   `--times`. Current sources are authoritative; unchanged content keeps cached
   mtimes and edits/deletions reach Cargo normally. Never edit Cargo fingerprints
   or backdate changed source files. Reject unsupported input types.
6. Run shipping compilation, export/freeze its exact bytes, then build native and
   Composer archives in the same target. Recreate the frozen-runtime directory
   only inside owned build state; never reuse an old test-pass marker. Seal the
   new artifacts for the actual workflow commit and initialized protocol pin.
7. Save only successfully built state, never test success. Cap compressed state
   at 4 GiB; if too large, skip the optional cache save without failing valid
   artifacts. Missing/failed cache storage is a performance issue, not permission
   to skip checks or hide an original failure. Do not delete other caches or
   increase repository storage limits automatically.

The repository reported 5,198,728,408 bytes across 67 cache entries during this
research; the two experiments account for approximately 5.18 GB. This is a
point-in-time observation, not its configured capacity. Main-only writes and
the size cap bound individual uploads, not aggregate storage: eviction and cache
misses remain expected and must work correctly. Avoid unconditional re-upload
of exact unchanged state.

## Acceptance before calling this merge-ready

- Complete the real artifact correctness run; preserve and diagnose first
  failures rather than accepting retry-only passes.
- Unit-test compatible/exact lookup, cold miss, corrupt/path-invalid/future state,
  source additions/deletions/edits, ignored private files, bounds, and cache-save
  failure. Build/test failures must not be retried or masked.
- Exercise cold producer → transported state → changed source producer → normal
  verifier and compiler-free consumers using a real small Docker workspace.
  Verify all packaged runtime bytes and execute a newly failing test.
- Run the full Python tooling/workflow-contract suite and shell/Python syntax
  checks. Check the default Docker path too, not just the new opt-in path.
- Validate the actual final integration on homelab, including the complete
  release/native/Kurtosis gates and unchanged promotion contract. A green
  experiment using different orchestration does not certify the final code.
- Report transfer/preparation/packaging plus execution times, cache footprint,
  resource limits, and real-source invalidation measurements. Do not call Cargo's
  one-second warm check a one-second CI build.
- Recheck main's revision before handing off. Keep the benchmark workflow,
  one-off seed IDs and experimental mutations out of the merge candidate.

No merge, registry publication, default-branch mutation, cluster reconfiguration
or deployment to an existing network is authorized by this draft.

## Opt-in and rollback

The proposed switch is the repository variable `CI_CARGO_STATE_CACHE=true`.
It applies only when the producer's runner input is `homelab-rust`; unset/false
and hosted runners retain the Docker builder. We have **not** set the variable.
The current token cannot read repository variables (HTTP 403); the operator must
confirm its value when arranging rollout.
Rollback is to unset it; artifacts and consumer interfaces do not change.

For pre-merge validation without a repository-wide change, the ordinary `CI`
workflow also accepts the default-off boolean `cargo-state-cache` on manual
dispatch. It forwards that opt-in to the same reusable producer; the homelab-only
guard, publication, sharded consumers, and final gate are unchanged:

```bash
gh workflow run ci.yml --repo eduadiez/rollupX --ref perf/ci-cargo-state-cache \
  -f runner=homelab-rust -f cargo-state-cache=true
```

The input is an additional opt-in, not an override: false still honors an enabled
repository variable. PR/push events do not opt in through the manual input.
Manual branch runs do not promote `latest` or save Cargo state (saves remain
default-branch-push-only). Experimental caches on another branch are not available
to this branch, so its first optimized production-pipeline run may be cold.
The earlier isolated integration pass does not establish that this normal
publication-and-shard workflow has passed; that remains a pre-merge check.

`cargo-state.py prepare` builds the existing Dockerfile's tools-only stage,
records compatibility and stages tracked inputs. The workflow restores one
optional `.ci-cargo-state/transport.tar.gz`; `build-runtime-artifacts.sh` invokes
the bounded producer, then seals and packages artifacts normally. Only a
default-branch push after successful unit execution attempts to archive/save
state, and exact cache hits are not re-uploaded. Both Actions operations use the
same transport path, because paths participate in Actions' cache version.
Optional restore, compression and upload each have a ten-minute step timeout,
so cache service or compression stalls cannot consume the whole job deadline.

The initial resource policy is four CPUs, 20 GiB memory, no swap, and 1,024 PIDs
per compilation container. Preparation requires a Docker host with at least
24 GiB RAM and 50 GiB free disk; free disk is checked again after restore.
These checks do not reserve node resources or guarantee freedom from contention.
No cluster scheduling or resource configuration is changed here.

## Integration work so far

The shipping command/export sequence now lives in `build-native-runtime.py`,
called by the default Docker builder. Its command was compared token-for-token
against this main revision; package, feature, profile and binary selections are
unchanged. Both paths use the existing native archive producer. Docker's
tools-only stage is separately addressable, but the default builder still removes
its target in the compilation RUN. No Cargo state is added to registry layers.

Local validation (2026-09-25): four new focused tests passed, including original
build-failure propagation and export-before-test-feature-rebuild ordering. The
existing real Docker fixture passed in 34.053s, building all three images and
executing compiler-free unit/E2E consumers. These use tiny fixture binaries and
do not establish real-workspace performance or correctness for the integration.

The first full tooling run had 259 tests, one failure and four opt-in skips. The
unmodified Anvil runner timeout fixture expected `TIMEOUT, NOT_RUN` but observed
`NOT_RUN, NOT_RUN`: its one-second suite budget expired before the first scenario
started. The isolated diagnostic repeat passed, establishing a timing flake, not
a clean first pass. The fixture budget is now five seconds; exact timeout,
remaining-work and child-cleanup assertions remain unchanged, as do production
deadlines. Final full tooling validation passed: 259 tests in 128.140s, 255 passed
and four opt-in skips. The Docker runtime fixture passed separately above; the
payload, native relocation and Kurtosis integration probes were not rerun here.
Logs are local-only under
`artifacts/cargo-state-validation/`, including the first failure.

The earlier homelab experiment passed cached production, local image packaging,
normal runtime verification and both release unit/vector archives. Its later
embedded failure is preserved below; it is not a complete passing result.

Completed steps from job `108268032771`: cache restore 78s, tools setup 52s,
cached validation/archive preparation 142s, image packaging 105s, runtime
verification 2s, release unit/vector execution 45s. Restore through verified
images totals 379s (6m19s), excluding checkout and correctness suites. This is
not an apples-to-apples claim against a historical 40-minute CI build.

The implemented cache integration passed its real small-workspace Docker probe
in 75.047s: cold build, all three image packages, normal verifier, 12 unit/vector
tests, four embedded tests and eight cross-chain tests with Cargo/rustc blocked
in consumers. A transported warm state was accepted without `Compiling` output;
all seven shipping executable hashes matched. The build phase took 41.518s cold
and 0.681s warm. Adding a new Composer test rebuilt in 3.087s; normal artifact
sealing succeeded, then the consumer executed the new test and failed with
`NEW_TEST_EXECUTED`, without creating a unit-pass marker. These measurements are
for the fixture, not the real RollupX workspace. Fixture images were removed;
no shared image/cache cleanup was performed.

The first focused cache-unit run exposed a real portability bug: safe tar
extraction normalized `0664` to `0644`, invalidating a full-permission hash.
Source identity now uses Git's meaningful executable bit, and staged inputs
use canonical `0644`/`0755` permissions. A regression verifies that transport
normalization preserves identity while executable/content changes do not.
Ten focused cache tests pass, including tracked-only staging, dirty/deleted
input rejection, invalid-cache quarantine and original build-failure propagation.
The full post-integration tooling suite passed: 269 tests in 127.772s, 265 passed
and four opt-in skips. The default Docker fixture passed separately in 10.923s.
The expanded cache fixture passed in 73.082s: it additionally edits the shipping
follower, runs the normal verifier for the changed candidate and executes the
new output, while still proving that a newly failing test fails. Its build
phases were 40.867s cold, 0.798s warm, and 4.656s after the combined edits.
A generated-Dockerfile default was subsequently made explicit to remove a
BuildKit lint warning; the producer still supplies the resolved base digest.

Real-workspace final-integration validation subsequently passed a fresh cold
build, cross-job cache transport and complete warm correctness suites. The
earlier experiment uses different orchestration and also carries
the separate CLI help/exit-code fix, which is not included in this cache change.
The optional data-only payload, standalone native relocation and Kurtosis
resource probes were not rerun. Local logs are under
`artifacts/cargo-state-validation/` and are not available from another checkout.

## Extraction-isolation review

Review of integration commit `8430cf51` reproduced an archive-boundary defect:
`state/link -> ../checkout` followed by a file beneath that link could overwrite
the staged checkout before cache metadata validation rejected the archive.
Python's safe extraction filter confines links to its extraction root, which
was then the whole job workspace, not just the cached state. This was a local
crafted-input reproduction, not an observed GitHub cache compromise.

Restore now extracts into a fresh isolated staging directory, validates its
root/inventory and rejects links outside state before moving accepted state
into the build location. Rejected extraction stays quarantined; the authoritative
checkout is never an extraction destination. Regression tests cover both the
pre-validation overwrite and an otherwise-valid cache retaining a link to a job
sibling; an ordinary contained relative link remains supported. Full tooling
and homelab cold/warm transport validation passed after this correction.
The first repeated Docker probe stopped before compilation because Docker Hub
returned an unauthenticated pull-limit error (HTTP 429) while resolving the Rust
base. Its log is `artifacts/cargo-state-validation/isolation-docker.log`; it is
not a passing result and image resolution was not bypassed. The later successful
homelab run validates the final extraction path; it does not erase this local
infrastructure failure.
The repeated tooling suite passed 272 tests in 128.558s (268 passed, four
opt-in skips); all 13 focused cache regressions and 16 workflow-contract tests
also passed after bounding the optional cache steps.

The final integration experiment is isolated on
`test/ci-cargo-state-integration`, with no registry publication or repository
variable changes. Its concurrency group queues it behind the existing full
correctness experiment, preserving that run's first result. It validates cold
production build → one capped cache upload → fresh-job restore → the normal
producer/verifier and all release, native E2E and private Kurtosis suites.

## Final results and acceptance

The final integration run used separate ephemeral homelab runners for cold and
warm jobs. Both used the production helper, source reconciliation, packaging
script, artifact seal/verifier and native consumers. The experimental wrapper
forced no publication and used one run-specific cache suffix; production's
main-only write/default-off conditions are covered by workflow contract tests.
No Rust source, Cargo dependency/profile, native test filter, E2E sharding or
promotion gate changed relative to main. The shipping command matches main
token-for-token. Main was rechecked and remained `7c90266a`.

| Final integration measurement | Cold | Warm |
| --- | ---: | ---: |
| Tools and compatibility preparation | 60s | 59s |
| Cache download | None | 113s |
| Producer: reconcile/build/package local images | 63m20s | 4m02s |
| Runtime verifier | 2s | 2s |
| Preparation through verified runtime images | 64m22s | **6m56s** |
| Release unit/vector execution | 44s | 43s |
| State compression / upload | 130s / 192s | Not uploaded |

The build-container phase itself took 3,683.509s cold and 19.502s warm; fetch
took 38.383s and 29.947s respectively. These are aggregate phase timings, not a
claim that every warm compiler invocation was independently logged as fresh.
The archive was **3,170,944,223 bytes** (about 2.95 GiB), below the 4-GiB cap.
Repository cache usage after testing was 8,344,005,177 bytes across 63 entries;
configured capacity was not established and no cache was deleted or resized.

**Trade-off:** the cold path is slower than the 38m51s baseline producer. It
uses four bounded CPUs and no compiler-cache backend, whereas the baseline
uses the existing compiler cache and different resource scheduling. The warm
measurement also excludes image publication. These are useful operational
measurements, not a controlled speedup ratio or a claim of faster full CI.
Warm state is the benefit; cache eviction/toolchain changes still incur the
cold cost. Default Docker behavior remains available for rollback.

Final correctness, with no test retries:

- Cold and warm release archives: **960 + 233 tests passed** in each job,
  with the same two-plus-one skips as main.
- Warm embedded suite: **36 passed**, one existing ignored test.
- Warm cross-chain suite: **63 passed**, three existing ignored tests.
- Warm Kurtosis: **32/32 eligible protocol scenarios passed**, complete suite
  accounting, complete causal evidence and authenticated semantic-blob checks;
  deployment, cross-chain waves and convergence assertions passed. The unique
  disposable enclave was removed by the normal cleanup path.
- The native ignored cases retain their existing scope: the separate
  evidence-dependent Kurtosis policy-replay target and three large-history
  feasibility/replay cases. The protocol pin excludes four of 36 discovered
  scenarios from its live `all` suite. None of these exclusions was changed or
  claimed as coverage by this work.

Acceptance evidence is cumulative: the real small-workspace fixture proves
changed shipping output, unchanged-output hashes, image-byte equality and a
newly failing test; the final real-workspace run proves corrected extraction,
cold misses, accepted cross-job restore and complete production correctness.
The 272-test tooling run and focused regressions cover rejection/quarantine,
tracked-only source inputs, dirty/deleted inputs, optional storage bounds,
failure propagation and workflow gates. Shell/Python syntax and diff checks
passed. The default Docker fixture passed separately; hosted 20-GiB real builds
remain explicitly unsupported by the opt-in switch, not an untested rollout.

### Earlier failure retained, not converted into a pass

The preceding experiment at `34cb678c` failed
`retained_backlog_posts_more_than_six_blobs_in_one_prague_batch_and_replays`
after 238.908s. Its follower first lost its L1 RPC connection during Composer
restart, then exited boot catch-up with `scan completion block is unavailable`;
the test subsequently timed out waiting for node convergence. The message is
wrapped as an L2-provider error, but its source is `finish_scan` failing to find
the canonical **L1** hash. This is not evidence of a compiler-cache miss or
stale executable acceptance. A startup/restart timing defect is a plausible
explanation, not a fully reproduced root cause.

The test and implicated deriver code are unchanged in the cache integration.
The final candidate passed that test in 60.939s on its first full run, as well
as every other selected test. This does **not** fix or reclassify the earlier
failure: preserve it as an intermittent runtime/test follow-up, separate from
this CI change. The earlier experiment also contains the independent CLI
help/exit-code fix; that change is absent from the merge candidate. No retry,
timeout increase or removed assertion was used to make the final E2E run pass.

Full logs are retrievable from the linked GitHub runs. Local copies are under
`artifacts/cargo-state-validation/{experiment-correctness,integration-cold,integration-warm}.log`.
The isolated validation workflow, experiment cache IDs and CLI change are not
part of the merge candidate. Remaining rollout decisions are whether/when to
enable the variable and how to handle cache capacity. These are operator rollout
choices, not automatic changes made by this branch.
