# Runtime packaging and observability CI wiring

2026-09-17, dirty worktree based on `5183f56`. This is scoped R28 evidence, not a
claim that the complete runtime image or remote GitHub Actions workflow was built.
See the [machine-readable input/check record](runtime-packaging.json).

The native archive producer builds workspace library tests, including the observer.
Its compiler inputs include 13 SQL migrations and three external public fixtures.
The existing runtime Docker allowlist excluded all observability inputs, and the
Dockerfile copied only the accounting fixture, omitting the two ABI fixtures.
The fix allows only the required migration SQL/fixture paths and copies the three
fixtures explicitly. It changes no runtime binary inventory, promotion rule,
producer/consumer archive identity check, protocol pin, or dependency version.

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

Full runtime Docker compilation, the opt-in archive relocation test and execution
of the new workflow on GitHub remain separate validation gates. The existing
runtime producer owns exact-revision shipping artifacts; this context test does
not substitute for that build. Backend functional evidence is recorded separately
by the Tempo and protocol dashboard validation lanes.
