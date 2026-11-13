# Optional same-run hosted measurements

This optional path attempts the bounded [#148 experiment](followup.md) on the
existing GitHub-hosted infrastructure. It adds measurement work on its own VM;
it never substitutes for normal validation or authorizes deployment. No quiet
criterion, runtime feature, Cargo input or observer code changes.

The standard public `ubuntu-24.04` worker has four CPUs and 16 GB RAM according
to [GitHub's specifications](https://docs.github.com/en/actions/how-tos/write-workflows/choose-where-workflows-run/choose-the-runner-for-a-job).
Its ability to sustain the unchanged CPU-idle, memory, swap, load and disk
criteria is unproven until observed. A rejected worker supplies diagnostics,
not usable latency. No affinity override or larger/self-hosted runner is used.

## Source and artifact binding

The boolean manual input adds a measurement job after `build-runtime` and all
`cross-chain-e2e` partitions, including the twelve-case correctness test of the
same exported binary. It
consumes `needs.build-runtime.outputs.tests-ref` from the SAME run through the
existing `native-test-artifact` action. There is no operator-typed external
digest, source-SHA override or consumer rebuild. The action checks repository,
immutable digest, platform and source/protocol labels; the native verifier
checks checkout = workflow = artifact SHA, protocol pin, Cargo.lock, release
profile/flags/toolchain, glibc compatibility, sealed shipping hashes and the
E2E file inventory. Producer run/attempt outputs are captured when publishing,
so a consumer-only rerun retains the original producing attempt.

The explicit `measure` operation selects only the exported `node_e2e`. It
stages a hash-identical signer under an owned `RUNNER_TEMP` directory outside
the checkout. The existing observer's release-path guard therefore accepts
verified release bytes without dirtying source or widening `.gitignore`. The
manifest and byte hashes, not the directory name, bind the profile.

Verified exported library directories support direct test execution, including
nextest's archived standard library when present. Missing or escaping library
directories reject before sampling. Ambient `LD_LIBRARY_PATH` and `LD_PRELOAD`
are not forwarded. The exported search path also applies to the observer and
its read-only tools; this is a retained scope caveat. The fixture signer uses
its existing sanitized launch environment. `eez_testkit::repo_root` uses the
observer's runtime `CARGO_MANIFEST_DIR`, rather than the builder's compiled path.

Record these hosted source/digest/build/binary identities separately from the
local `ca76bb87` glibc 2.43 binaries. The normal hosted producer uses Bookworm;
the consumer checks its actual manifest glibc floor on Ubuntu. Different hosts
and artifacts are separate characterizations, not an improvement comparison.

## Dispatch and authoritative validation

After source review, focused/full tooling checks and adversarial Claude Fable
5.2 review with verified model/no fallback, dispatch the reviewed `codex/` head:

```bash
gh workflow run ci.yml --repo eez-association/eez-rollupX \
  --ref "$EEZ_REVIEWED_CODEX_BRANCH" -f measure_streaming_deadlines=true
```

[Manual dispatch](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_dispatch)
uses that branch's commit as `GITHUB_SHA`. The normal producer builds/seals once
through the established pipeline; the measurement VM only consumes its output.
All existing validation lanes still run. `CI Result` remains unconditional
`always()` and waits for the measurement job too. Requested measurement must
succeed; not-requested measurement must be skipped. Neither outcome authorizes
a skipped required normal lane. A requested measurement without published
artifacts fails closed. A 225-minute measurement-step timeout limits the workload step within the
240-minute job. Setup and uploads share the remaining budget, so upload time
is not guaranteed. A hard timeout can also prevent the post-byte-check record;
such evidence is incomplete and cannot establish an eligible result.
The promotion condition remains unchanged and cannot
pass a failed requested measurement. Default PR/main/no-input behavior retains
all normal validation; the additional measurement job is intentionally skipped.
Existing per-run manual concurrency and all normal matrix partitions remain.

A rejected hosted window makes that measurement job and the aggregate fail;
normal passing results remain retained. A diagnostic retry may rerun only that
job and its dependent aggregate using the SAME frozen producer outputs, rather
than rebuilding. Retain every attempt and unique run/attempt artifact. Never
count a retry-only semantic pass as a clean first pass or hide an actual failure.
Exact-head full validation remains mandatory before repair or #165 merge.

## Evidence and limits

Setup, hash checks and library preparation finish before the unchanged observer
starts its full 120-second clean baseline. Every during check and 120-second
post-observation remain. All twelve groups require three retained warmups and
twenty successful measured repetitions. Incomplete, failed or contaminated
campaigns remain ineligible. The initial local failures/exclusions and frozen
N14 input-provenance limitation remain as documented in the follow-up.

Actions artifacts retain resource/attempt data, logs and summary plus pre/post
artifact provenance and byte rechecks. Successful requested measurement data
is an explicit exception to failure-only diagnostic uploads. Failed evidence
retention fails this job. Artifacts expire after three days: download promptly.
Earlier artifact/preparation rejection may have only its workflow log; never
claim its validation passed. The existing observer signal-cleanup and static
profile-string caveats remain. No runtime/test binaries are uploaded as
diagnostics. Do not commit bulk attempt streams or add scanner exceptions;
publish compact eligible distributions and rejected-run identities/counts only
after a valid result. No SLA, general tail/capacity guarantee or deployment is
implied. The conditional #165 and final-main release gates remain in force.

## Concrete compatibility inspection

A normal `export-e2e` of the already-passed local producer was made from an
actual isolated checkout at `ca76bb8744667f3045f5d839a920c011825999f9`, with
protocol `84a5b5207880f192a6a5896cbfe0bac5249db47f`. No binary was built or
modified. All twenty real `linked-paths` entries exist in that exported target;
`nextest/libdirs/host` exists and the unused target-libdir does not. The exported
test and signer SHA-256 values still equal the original retained `85eff313…`
and `ebaf852b…` identities. These are local glibc 2.43 bytes and are not evidence
of Ubuntu compatibility; hosted consumption still checks its own actual
producer glibc floor. Tests cover both present and listed-but-absent library
directories, with the latter rejected before execution.

Direct fixture lookup was traced through `Fixture::load` to
`eez_testkit::repo_root`: its runtime `CARGO_MANIFEST_DIR` overrides the builder
path. The unchanged observer supplies the checked-out `crates/eez-node-e2e`
path. The real-Git preflight regression reproduces old in-checkout staging's
untracked-file failure, then runs actual outside-checkout staging and the real
observer preflight to a controlled ineligible resource boundary. It uses stub
executables and supplies no native workload/timing evidence.

The [source validation record](hosted-validation.json) distinguishes the retained
first failures, full-suite skips, focused controls and source-only reviews.
The later [completed campaign](timing-results.md) passed the actual artifact
consumer, native utility and all fourteen jobs without changing the quiet gates.
The source-validation record remains historical; final-feature/main validation
still applies.
