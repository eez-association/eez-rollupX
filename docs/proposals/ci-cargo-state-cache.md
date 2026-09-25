# Portable Cargo state for the release producer

Status: production-integration draft. Not enabled, not merge-ready, and not a
replacement for the existing CI gate. Base: main `7c90266a5fe8cfdb46b249c61079badc2dce71a3`;
protocol: `9d13218a355e7d543ca8a9927eea2962600b716d`.

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

The [full correctness experiment](https://github.com/eduadiez/rollupX/actions/runs/36194784589)
is pending. It packages local node/signer/deploy images from the cached candidate
and uses the normal runtime verifier, units, full native suites and Kurtosis.
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
and hosted runners retain the Docker builder. The variable has **not** been set.
Rollback is to unset it; artifacts and consumer interfaces do not change.

`cargo-state.py prepare` builds the existing Dockerfile's tools-only stage,
records compatibility and stages tracked inputs. The workflow restores one
optional `.ci-cargo-state/transport.tar.gz`; `build-runtime-artifacts.sh` invokes
the bounded producer, then seals and packages artifacts normally. Only a
default-branch push after successful unit execution attempts to archive/save
state, and exact cache hits are not re-uploaded. Both Actions operations use the
same transport path, because paths participate in Actions' cache version.

The initial resource policy is four CPUs, 20 GiB memory, no swap, and 1,024 PIDs
per compilation container. Preparation requires a Docker host with at least
24 GiB RAM and 50 GiB free disk; free disk is checked again after restore.
These checks do not reserve node resources or guarantee freedom from contention.
No cluster scheduling or resource configuration is changed here.

## Integration work so far

The shipping command/export sequence now lives in `build-native-runtime.py`,
called by the default Docker builder. Its command was compared token-for-token
against this main revision; package, feature, profile and binary selections are
unchanged. Both paths will use the existing native archive producer. Docker's
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

The homelab experiment has passed cached production, local image packaging,
normal runtime verification and both release unit/vector archives. Native E2E
and Kurtosis are still pending; this is not a complete correctness result.

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

Real-workspace final-integration validation remains pending. It requires a
fresh cold build, cross-job cache transport and complete warm correctness
suites; the existing experiment uses different orchestration and also carries
the separate CLI help/exit-code fix, which is not included in this cache change.
The optional data-only payload, standalone native relocation and Kurtosis
resource probes were not rerun. Local logs are under
`artifacts/cargo-state-validation/` and are not available from another checkout.
