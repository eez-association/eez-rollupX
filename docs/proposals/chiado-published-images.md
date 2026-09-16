# Published images for Chiado

Status: proposed implementation of the requested deployment/docs change.
Baseline: fork `ddf40bc2eb5fac8f7973b112eb383256cd8aac74`, protocol
`3c0cdf4955941a5c23639346932b55b33babebbd`.

## Behavior and acceptance

- Default `EEZ_NODE_IMAGE` and `EEZ_PROOF_SIGNER_IMAGE` to the fork's `:latest`
  images. Offer the full Git commit and image digest as explicit
  pins. Use the images' source revision for the deployment checkout.
- After every validation lane passes on `main`, add plain commit tags to the
  exact tested node/signer/deploy digests. Update `latest` only if that commit is
  still the `main` head; serialize promotion to prevent an older run rolling it
  back. Keep CI's existing per-run candidates and digest handoffs unchanged.
  Implement promotion in the existing workflow, without another operator script.
- The base Compose definition has no build rules or implicit local-image fallback.
- Keep `eez-node:local` / `eez-proof-signer:local` for explicit development builds
  using existing Dockerfiles. No new operator scripts or image-lock format.
- Startup requires existing bindings and locally available selected images;
  it never deploys contracts, deletes datadirs, builds, or pulls images.
  Contract deployment remains an explicit, non-idempotent Foundry operation.
- Copy genesis helpers from the selected node image to avoid host Rust builds.
  Preserve current keys, ports, data paths, blob DA, and settlement configuration.
- Provide a [quick start](../runbooks/chiado-quickstart.md) and keep detailed
  verification/recovery in the [operational runbook](../runbooks/chiado-deployment.md).

## Compatibility and validation

Existing local deployments must set both `:local` references before startup or
teardown. Published images and local tags use the same Compose service definitions.
Changing images is an upgrade; preserve `.env.chiado`, bindings/genesis/databases
and assess protocol/storage compatibility. No automatic data migration, downgrade,
or durable pending-request recovery is introduced.

Validate default/latest and explicit image selection, absence of build rules,
secret isolation, non-destructive missing-bindings startup, promotion after the
validation gate, and older-run protection. Run existing Chiado, settlement
configuration, and CI tooling checks. Real registry promotion and a fresh-host
rehearsal are separate evidence; a CI candidate is not a fully passed release.

## Initial explicit-image validation — 2026-09-16

Evidence for `a90f53e`, before adding commit/latest promotion:

- Chiado configuration: 11 tests passed; static blob and Chiado/Kurtosis settlement
  configuration checks passed.
- CI tooling: 228 tests selected; 224 passed, four opt-in integration tests skipped.
  Eighteen initially failed setup because this worktree's protocol checkout was
  empty; all 18 passed after initializing the unchanged protocol pin.
- Documentation: 25 Bash examples passed syntax checks; 63 local links/anchors
  resolved, including pinned protocol documents; `git diff --check` passed.
- No real image pull, contract broadcast, stack restart, or fresh-host rehearsal
  was performed.

## Commit/latest validation — 2026-09-16

- `python3 -m unittest discover -s scripts/ci/tests -v`: 233 selected, 229 passed,
  four opt-in integration tests skipped. Offline promotion cases cover exact
  source digests, bare commit/latest tags, older runs, missing references, API
  failure, and publication failure. Every required lane remains in the CI gate.
- Chiado blob and Chiado/Kurtosis settlement configuration checks passed; default
  `latest`, explicit commit/digest references, and local images rendered correctly.
- Both workflow YAML files and inline shell passed parsing/syntax checks; 26 Bash
  examples and 64 relative links/anchors passed; `git diff --check` passed.
- Registry promotion and deployment were not executed. New tags appear only after
  the workflow lands on `main` and its validation/promotion steps succeed.
