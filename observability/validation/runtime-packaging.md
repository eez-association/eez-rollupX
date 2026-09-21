# Runtime packaging and observability CI wiring

2026-09-17, initial work based on `5183f56`, extended after `c36bbd6`.
This is scoped R28 evidence, not a
claim that the complete runtime image or remote GitHub Actions workflow was built.
See the [machine-readable input/check record](runtime-packaging.json).

The native archive producer builds workspace library tests, including the observer.
Its compiler inputs include 13 SQL migrations and three external public fixtures.
The existing runtime Docker allowlist excluded all observability inputs, and the
Dockerfile copied only the accounting fixture, omitting the two ABI fixtures.
The fix allows only the required migration SQL/fixture paths and copies the three
fixtures explicitly. That initial fix changed no runtime binary inventory,
promotion rule, producer/consumer archive identity check, protocol pin or dependency
version. The later observer executable packaging below extends the inventory.

The existing opt-in tiny-workspace Docker test now includes an observer library
with real `include_str!` inputs. A fast context-only case uses the actual Docker
matcher and production observability COPY instructions with `FROM scratch`:

1. Removing the observability exceptions reproduces the old missing-input failure.
2. The current allowlist exports all **16** compiler inputs byte-identically.
3. Unrelated fixture/config sentinels remain excluded.

The first candidate used directory exceptions and the real Docker test caught
unintended reinclusion of an unrelated configuration sentinel. File-only
exceptions fixed it. The original failure and corrected runs are retained in
`/tmp/eez-observability-runtime-context{,-reviewed,-final}.log`. The final real
context check passed using Docker Buildx `v0.37.1` (`0b265a9f62db`). It pulled no
base image, ran no Cargo, started no service and changed no existing container.

The observability workflow now includes required native build dependencies,
relevant codec/protocol/native/packaging paths, the real context regression, blob
codec tests, Tempo ingestion/log-link/recovery checks and real PostgreSQL/Grafana
protocol dashboard queries, and clean-host backup/restore using the built observer.
The restore step has a 15-minute budget and retains per-project failure logs. Backend steps fail the job on error; only failure
reports and sanitized logs are uploaded. The timeout allows the new backends to
run; there is no artifact-consumer rebuild fallback or live-network workload.
Focused workflow contracts supplement the behavioral Docker check.

Executed:

```bash
EEZ_TEST_DOCKER_RUNTIME=1 python3 -m unittest \
  scripts.ci.tests.test_runtime_docker.RuntimeDockerTest.test_observer_compile_inputs_survive_runtime_allowlist -v
python3 -m unittest scripts.ci.tests.test_runtime_artifacts \
  scripts.ci.tests.test_native_artifacts scripts.ci.tests.test_workflow_contract -v
python3 -m unittest scripts.ci.tests.test_workflow_contract.ObservabilityWorkflowContractTest -v
```

The tooling regression run selected 70 tests: **69 passed**, with the existing
explicitly opt-in real native-archive relocation case skipped. The two final
observability workflow assertions also passed after the restore gate and its
per-project diagnostics-path updates.
Python syntax and `git diff --check` passed. Logs are
`/tmp/eez-observability-ci-contract-tests.log` and
`/tmp/eez-observability-workflow-tests-final.log`; the final restore gate checks
are in `/tmp/eez-observability-workflow-restore-tests.log`.

Full application runtime Docker compilation, the separate opt-in native archive
relocation test and execution
of the new workflow on GitHub remain separate validation gates. The existing
runtime producer owns exact-revision shipping artifacts; this context test does
not substitute for that build. Backend functional evidence is recorded separately
by the Tempo and protocol dashboard validation lanes.

## Shipping observer executable and real producer/consumer fixture

The runtime producer now builds and installs `eez-observer` into the checksummed
archive and existing node image. Native artifact sealing also freezes its exact
bytes, alongside Composer, follower and signer. The consumer's strict regular-file
inventory and executable checks require it; neither a missing binary nor a
rebuild fallback is accepted. Image roles, promotion rules and node entrypoints
are unchanged. See the [node-host procedure](../runbooks/observer-host.md).

The existing real Docker fixture now supplies an observer binary as well as its
library with actual compiler inputs. Both Docker cases passed (**60.94 seconds**):
the context allowlist retained all **17** compiler inputs, and the tiny-workspace
producer built release binaries/archives, sealed them, packaged images, verified
same-job artifact consumption, compared image/archive binary hashes and ran the
exported suites with Cargo/rustc blocked on the consumer path. Owned images and
containers were removed. These were small fixture Rust programs, not the complete
EEZ application or independent protocol tests.

Commands and logs:

```bash
EEZ_TEST_DOCKER_RUNTIME=1 python3 -m unittest \
  scripts.ci.tests.test_runtime_docker.RuntimeDockerTest -v
python3 -m unittest scripts.ci.tests.test_runtime_artifacts \
  scripts.ci.tests.test_native_artifacts scripts.ci.tests.test_workflow_contract -v
```

The second command again selected 70 cases: 69 passed and the separately opted-in
native archive case remained skipped. Logs:
`/tmp/eez-observability-observer-runtime-docker.log` and
`/tmp/eez-observability-observer-runtime-tests.log`. The full real application
release build and remote CI remain required; fixture success is not a substitute.

The broader tooling snapshot subsequently selected **236 tests: 231 passed and
5 explicit opt-ins were skipped** (`/tmp/eez-observability-final-ci-tooling.log`).
The skips were data-only Docker payload extraction, real Kurtosis resources,
native archive relocation, and two runtime Docker cases. Earlier separate
Docker runs remain separate evidence; this command did not rerun them. This
snapshot predates any subsequent native live-observer test/preflight additions.


The focused follow-up after the browser CI and native observer integration wiring
selected 73 artifact/inventory/workflow tests: **70 passed, 3 explicit opt-ins
skipped** (`/tmp/eez-observability-final-artifact-tooling.log`,29.08 seconds).
The skips remain the real native-archive relocation and two Docker runtime fixture
cases; this run did not repeat their earlier separate evidence. Required browser
setup precedes both protocol navigation and trace-retention steps, with failure
screenshots/reports retained. The three observability workflow contract tests and
all 30 observability Python tests also pass.


The current schema20 compiler-input context gate also passed against the actual
Docker matcher and scratch export: all **23** external observer inputs (20
migrations and three public fixtures) survive byte-identically, while unrelated
sentinels stay excluded. Log: `/tmp/eez-observability-schema20-runtime-context.log`;
input and Dockerfile hashes are in the `schema20_context_followup` record. This
check deliberately performs no Cargo/application compilation and does not replace
the remaining full release-artifact gate.


The subsequent required native-quota archive handoff was tested with the real
relocation opt-in: **all 27 Python cases passed in 21.184 seconds**, including
compiling the tiny producer, relocating it, blocking Cargo/rustc on the consumer
path, and invoking its archived ignored quota test through the required runner.
The nested fixture suite separately reports its intentionally ignored test; the
outer gate explicitly invokes that archived test. The tiny controller proves
artifact selection and handoff, not physical ENOSPC (see `native-enospc.md` for
the separate actual native filesystem-failure proof).

Command: `EEZ_TEST_NATIVE_ARCHIVE=1 RUSTUP_TOOLCHAIN=1.94.0 python3 -m unittest discover -s scripts/ci/tests -p test_native_artifacts.py -v`.
Log: `/tmp/eez-native-quota-real-relocation-reviewed.log`. The host lacked mold,
so the pinned Rust invocation retained its existing linker flags and used mold
2.40.4 with dependencies extracted only under `/tmp/eez-mold-tool-jyphw06z`; no
host packages or project dependencies changed. This is a local dirty-worktree
fixture result, not the full EEZ release image or remote CI execution.


The final combined tooling follow-up selected **239 cases: 234 passed and five
explicit opt-ins were skipped** in 97.730 seconds. Log:
`/tmp/eez-observability-final-ci-reviewed.log`. Skips were the data-only Docker
payload, real Kurtosis probe, native archive relocation, and two runtime Docker
cases; the separate relocation/context/runtime evidence above was not rerun by
this command. The initial integrated run failed the hosted/self-hosted variants
of one resource fixture: its tiny checkout omitted the newly required Docker
preflight. The fixture now includes the real script and image lock, exercises
actual pinned-image selection with mocked Docker, and asserts readiness, all
three pinned pulls and quota-image preparation. Existing directory preservation,
spaced paths, and failure propagation assertions remain intact. The focused six
cases passed before the complete rerun. Initial log:
`/tmp/eez-observability-final-ci-integrated.log`; focused log:
`/tmp/eez-observability-ci-resources-reviewed.log`.

The adjacent observability suite passed **83/83**, explicitly enabling the owned
PostgreSQL resource fixture and local HTTP/socket tests. Log:
`/tmp/eez-observability-final-python-integrated.log`. This checkpoint precedes the
subsequent overhead-resource/harness finalization fixes; those have separate
focused validation. Static checks parsed 70 Python files, checked both edited
shell scripts and three browser JavaScript files, and validated 22 Bash examples
and relative Markdown links; `git diff --check` passed. These checks are local
configuration/tooling evidence, not remote CI execution or the full runtime build.
