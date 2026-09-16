# Published images for Chiado

Status: proposed implementation of the requested deployment/docs change.
Baseline: fork `ddf40bc2eb5fac8f7973b112eb383256cd8aac74`, protocol
`3c0cdf4955941a5c23639346932b55b33babebbd`.

## Behavior and acceptance

- Set `EEZ_NODE_IMAGE` and `EEZ_PROOF_SIGNER_IMAGE` directly in `.env.chiado`.
  Operators copy digests from one successful `main` CI run and use its checkout.
  The base Compose definition has no build rules or implicit local-image fallback.
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

Existing local deployments must set the two image variables before startup or
teardown. Published images and local tags use the same Compose service definitions.
Changing images is an upgrade; preserve `.env.chiado`, bindings/genesis/databases
and assess protocol/storage compatibility. No automatic data migration, downgrade,
or durable pending-request recovery is introduced.

Validate rendered image selection, missing-image rejection, absence of build rules,
secret isolation, and non-destructive missing-bindings startup. Run existing Chiado,
settlement configuration, and CI tooling checks. Real image pulls and a fresh-host
rehearsal are separate evidence; a published candidate is not a fully passed CI run.

## Validation record — 2026-09-16

- Chiado configuration: 11 tests passed; static blob and Chiado/Kurtosis settlement
  configuration checks passed.
- CI tooling: 228 tests selected; 224 passed, four opt-in integration tests skipped.
  Eighteen initially failed setup because this worktree's protocol checkout was
  empty; all 18 passed after initializing the unchanged protocol pin.
- Documentation: 25 Bash examples passed syntax checks; 63 local links/anchors
  resolved, including pinned protocol documents; `git diff --check` passed.
- No real image pull, contract broadcast, stack restart, or fresh-host rehearsal
  was performed. Select a fully successful CI candidate before live deployment.
