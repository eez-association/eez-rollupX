# Final Python, configuration and documentation checkpoint

The latest full suite, including elapsed arm/experiment budget enforcement and
the real PostgreSQL regression, passed **109/109 tests in 13.258 seconds**, with
unchanged before/after Python source hashes. Log:
`/tmp/eez-observability-elapsed-python.log`.

The preceding suite, including bounded test-only wave/budget configuration, passed
**107/107 tests in 6.375 seconds**. Before/after input hashes were unchanged;
Python syntax, adapter documentation and diff checks passed. Log:
`/tmp/eez-observability-budget-python.log`. Defaults remain one wave, 600 seconds
per arm and 9000 seconds per experiment; sample minima and statistical thresholds
were not relaxed.

The preceding frozen observability suite passed **104/104 tests in 5.403 seconds**, including
the explicitly enabled, owned real PostgreSQL resource fixture and loopback HTTP
tests. Command and unchanged before/after source hashes are retained in
[python-config-final.json](python-config-final.json). Log:
`/tmp/eez-observability-final-frozen-python.log`.

Static checks passed for 45 Python files, both changed shell scripts, all three
browser scripts, 24 Bash examples in changed Markdown files, relative documentation
links and `git diff --check`. The separate full CI tooling suite previously passed
239 selected cases, with five explicit opt-in skips; see
[runtime packaging](runtime-packaging.md). This checkpoint does not claim remote
Actions execution or a full release image build.

The actual new producer's functional A/B files also passed direct adapter-contract
checks: common semantic configurations have the same SHA-256 while the enabled
arm configurations differ; original native/effective values agree; only the
declared shared genesis timestamp changes; the existing native genesis helper
matches RPC block zero; exact JSON identities and ns/legacy-ms floor relationships
are preserved. Completeness, resource and cleanup checks passed in those files.
The retained report is `/tmp/eez-overhead-ns-adapter-wiring-AB.json`.

These were functional wiring runs concurrent with other validation. No frozen
performance plan was invented, no complete counterbalanced pilot was claimed, and
no statistical acceptance result was produced from these two runs. The separate
[adapter tests](overhead-input.md) establish refusal of incomplete/mismatched sets.
