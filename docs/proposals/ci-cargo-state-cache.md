# Portable Cargo state for the release producer

Status: opt-in homelab implementation in [PR #39](https://github.com/eduadiez/rollupX/pull/39).
Both normal CI modes passed at `e01b74d6` on 2026-09-26. Base: `7c90266a`;
protocol pin unchanged: `9d13218a355e7d543ca8a9927eea2962600b716d`.

## Enable and roll back

The existing Docker builder remains the default. Set repository variable
`CI_CARGO_STATE_CACHE=true` to opt in for `homelab-rust` producers only. Hosted
runners retain the Docker builder. For a single branch validation, use:

```bash
gh workflow run ci.yml --repo eduadiez/rollupX --ref perf/ci-cargo-state-cache \
  -f runner=homelab-rust -f cargo-state-cache=true
```

The manual boolean defaults to false; it adds an opt-in rather than overriding
the repository variable. Roll back by unsetting the variable and leaving the
manual input false. No artifact format or consumer changes are required.

Compilation containers use four CPUs, 20 GiB RAM, no swap, and 1,024 PIDs. The
Docker host must have at least 24 GiB RAM and the workspace 50 GiB free disk.
These checks do not reserve resources against other jobs. The implementation
does not change cluster configuration or clean up shared storage.

## Build and cache contract

- Retain source, Cargo downloads, and target state together. Reconcile the clean
  tracked checkout with checksum rsync; preserve unchanged mtimes, not changed
  source timestamps. Always invoke locked Cargo builds and rerun tests.
- Key compatibility on the toolchain/native packages, build settings, and source
  inventory. Reject incompatible, corrupt, oversized, or future-dated state
  before compilation and fall back cold. Never retry a failed build as a fallback.
- Extract in isolated staging and reject links outside cached state before
  accepting it. Cached data must not overwrite the authoritative checkout.
  Cache writers remain inside the repository's trusted CI boundary.
- Fetch dependencies separately, then compile offline without runner credentials,
  the Docker socket, or shared Cargo storage. Both builders use the same shipping
  command; freeze shipping bytes before the separate native/Composer archives.
- Seal artifacts for the actual commit and protocol pin. Keep normal verification,
  publication, all test shards, and the final promotion gate unchanged.
- Restore from the current-ref/default-branch cache scopes. Save only after unit
  success on default-branch pushes; do not re-upload exact hits. Manual/PR runs
  do not save Cargo state, and manual branch runs do not promote `latest`.
- Limit transport to 4 GiB compressed / 32 GiB extracted. Optional restore,
  compression, and upload each have a ten-minute timeout. Eviction and cache
  misses are expected; cache availability never substitutes for validation.

## Validation and trade-offs

At `e01b74d6`, the [default pipeline](https://github.com/eduadiez/rollupX/actions/runs/36224181469)
and [optimized pipeline](https://github.com/eduadiez/rollupX/actions/runs/36224206945)
passed lint, release units, GHCR publication, all eight native E2E shards,
Kurtosis, and the final gate. The optimized producer had a cache miss and took
94m40s versus 67m17s for the default producer. These overlapping runs are not
a controlled performance comparison. **Main-only cache saving remains a rollout
check; neither run exercised that event condition.**

The [isolated transport validation](https://github.com/eduadiez/rollupX/actions/runs/36198290775)
at `1834e067` passed cold build/upload and fresh-runner restore with the same build
implementation. Preparation through verified local images took 64m22s cold and
6m56s warm, excluding publication; the archive was 3,170,944,223 bytes. Warm reuse
is the benefit, with slower cold builds and substantial cache transfer/storage.
This is not a promise of six-minute full CI or equivalent hosted performance.

Retained regressions cover source identity, extraction isolation, resource limits,
failure propagation, shipping-byte freezing, and workflow gates. The opt-in
Docker fixture (`EEZ_TEST_CARGO_STATE=1`) checks transported reuse, changed shipping
output, and execution of a newly failing test. Existing suite exclusions remain
unchanged and are not counted as coverage.

An [earlier experiment](https://github.com/eduadiez/rollupX/actions/runs/36194784589)
failed follower convergence after `scan completion block is unavailable` during
restart. Later passes do not fix or reclassify that intermittent failure.
Development also caught and fixed permission normalization and extraction-boundary
bugs; their regressions remain. The full research record, including the local
Anvil timeout flake and Docker Hub 429, is preserved in
[the pre-cleanup document](https://github.com/eduadiez/rollupX/blob/e01b74d64eb0f57e06730407dea368da81bd8b0d/docs/proposals/ci-cargo-state-cache.md).
