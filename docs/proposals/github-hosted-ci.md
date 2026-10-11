# GitHub hosted CI and public release preparation

| Field | Value |
| --- | --- |
| Design status | Implemented for review at the maintainer's request; local tooling checks pass. Hosted execution remains to be validated. |
| Date | 2026-10-11 |
| Source baseline | `eduadiez/rollupX` main, `e15bb412420b36bfa40a0e2a73233ac4f213441d` |
| Protocol baseline | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, unchanged |

## Scope and contract

Public pull requests must not execute on the maintainer's homelab. All CI jobs,
including reusable producer and Kurtosis workflows, will select `ubuntu-24.04`
directly. Remove manual runner selection, repository runner overrides, and the
homelab-only retained Cargo-state workflow. Keep GitHub-hosted source, compiler,
and registry caches optional; cached outputs do not replace candidate validation.

| ID | Required behavior | Acceptance |
| --- | --- | --- |
| H1 | Every job uses GitHub-hosted Ubuntu with no runner selector or homelab cache input | Workflow contract regressions; inspect actual PR job labels |
| H2 | Fork/Dependabot jobs cannot publish images; all existing validation remains required | Existing publication and gate tests; hosted CI run |
| H3 | Shipping bytes, protocol pin, release feature selection, exact-revision verification, shard inventory, and promotion gates remain intact | Existing artifact/tooling regressions and CI validation lanes |
| H4 | License and contributor/security guidance are available before public release | Documentation/link review and explicit remaining operator checklist |
| H5 | Organization image migration preserves working deployment references until replacement images are verified | Organization namespace artifact regression and release runbook |

## Decisions and alternatives

A fixed hosted label is sufficient. Retaining an arbitrary runner variable would
allow old settings to select the homelab again. Removing all cache implementations
would widen this change: local tooling can retain those capabilities, while the
shipping workflow explicitly disables retained Cargo state and exposes no input
to enable it.

Standard hosted Ubuntu has different capacity for private and public repositories.
Both protocol execution paths cap concurrent scenarios at two without reducing
the discovered inventory. Lint and debug unit jobs allow 120 minutes for cold
source builds, while their existing validation steps remain required.
The initial run must establish whether the existing build and test timeouts fit;
do not reduce test selection, change dependency pins, or alter release features
to make a smaller runner pass. A failure requiring different resources must be
recorded before selecting another hosting arrangement.

Keep the current image defaults until organization packages exist. The producer
already follows `GITHUB_REPOSITORY`; exercise the organization namespace in its
tests and document the coordinated package/template switch. Repository visibility,
ownership, organization access, runner deregistration, branch protection, and
package administration are separate operations from merging this PR.

## Compatibility and validation

Existing `CI_RUNNER`, `CI_AUX_RUNNER`, and `CI_CARGO_STATE_CACHE` variables become
unused. Manual dispatch has no runner/cache arguments. Existing deployments,
operator environment files, genesis, protocol pin, and database formats are
unaffected. Reverting the workflow change can restore homelab scheduling and
must not be used as a public-repository rollback without reviewing runner access.

Reproduce the hosted-only contract failure against the baseline workflow, then
repeat after the change. Run the full CI tooling suite, shell syntax checks for
edited scripts, workflow validation, configuration checks, and the real PR CI.
Record actual results and pending checks in the PR; local mocked tests do not
establish hosted capacity or public fork execution.

Gitleaks 8.30.1 scans fetched history including merge diffs in the lint lane,
with its archive checksum pinned. Exact fixture-value exceptions come from the
readiness audit; new keys beside those fixtures remain detectable. Inline
suppression comments and fingerprint ignore files are disabled. This complements,
but does not replace, disclosure review of GitHub logs, attachments and packages.
