# Public release and transfer to eez-association

This procedure prepares `eduadiez/rollupX` for transfer to
`eez-association/rollupX`, preserving the repository name. It covers repository
and package administration, not contract deployment or node migration.
The [readiness audit](../audits/2026-10-11-public-release-readiness/README.md)
records the inspected source and limits; the
[CI proposal](../proposals/github-hosted-ci.md) records the hosted runner contract.
The [publication preflight](../audits/2026-10-13-publication-preflight/README.md)
records the later dependency inventory, disclosure checks and unresolved CI
failure. The administrative operations below have not been executed.

## Route without paid Actions

Standard GitHub-hosted runners are free for public repositories. Private
repositories consume their owner's included allowance; transferring to an
organization does not provide usable private CI capacity when that allowance
is unavailable. Larger runners remain paid. See
[GitHub's billing rules](https://docs.github.com/en/billing/concepts/product-billing/github-actions).

When neither owner can supply private Actions minutes:

1. Keep the repository private while completing local validation, licensing and
   disclosure review. Preserve failed CI evidence and keep unvalidated fixes in
   draft PRs. An unchanged test passing locally does not resolve a CI failure.
2. Prepare the access inventory and destination permissions below. Ownership
   can be transferred privately once the administrators authorize it, but
   organization CI remains pending; do not describe the transfer as validation.
3. Once the publication gates are cleared, change visibility and run the
   standard hosted matrix in the organization. This moves **organization CI
   validation after source publication**; it does not waive CI acceptance for
   merging fixes, promoting images or switching deployment references.
4. Verify the public fork path and all organization validation lanes. Only then
   promote tested images and update their references together.

Retain spending limits; do not raise a budget or select larger runners for this
route. Actions artifact storage is separate from free public runner compute;
the existing diagnostic uploads are optional, failure-only and retained for
three days. Required runtime/test payloads use verified GHCR digests. Check
package access and diagnostic upload capacity separately.

Trusted local or homelab testing can continue while publication is blocked.
Before making this repository public, remove its access to homelab runners,
including the ability to rerun historical workflows with old runner labels.
Keeping the machines for other private work does not require restoring homelab
scheduling in this repository. Public fork jobs must remain on hosted VMs.

## Before transfer or publication

1. Merge CI changes only after their required lanes pass on
   GitHub-hosted runners. All jobs select `ubuntu-24.04` directly; old
   `CI_RUNNER`, `CI_AUX_RUNNER`, and `CI_CARGO_STATE_CACHE` variables are unused.
   Confirm real job labels and capacity, including the no-publication fork path.
   With no private allowance, retain that gate and perform the fresh hosted run
   after publication using the route above. Standard private Ubuntu runners
   have fewer resources than public ones; a mocked tooling test does not
   establish build or network-test capacity. See
   [GitHub's runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
2. Remove this repository's access to the homelab runner group/registration in
   GitHub settings. Cancel or finish old queued/running workflows first: reruns
   of older revisions still contain the old runner labels. Remove the unused
   variables and any repository-specific Redis/runner credentials after checking
   their other consumers. Do not delete shared runners, caches, or deployments.
   Retiring physical homelab services is a separate operator action.
3. Finish the disclosure review of **all** branches, tags, PR discussions,
   retained workflow logs/artifacts, audit attachments, agent transcripts, and
   package layers/metadata. The automated scan found public fixtures and false
   positives at the audited baseline; it did not clear every retained document
   or GitHub attachment. Review new commits since that baseline. Rotate any
   real exposed credential before deciding how to remove it from history and
   retained content. Preserve sanitized evidence of any cleanup.
4. Confirm the right to publish inherited contributions under the declared MIT
   license. The root license matches the workspace declarations; it does not
   relicense third-party source. Review notices for the pinned core protocol,
   vendored code, all three Cargo workspaces, UI, and distributed container/tool
   dependencies. Existing `cargo deny check advisories bans sources` does not
   check licenses. Start with the
   [pinned inventory and unresolved grants](../audits/2026-10-13-publication-preflight/README.md#license-inventory).
   Metadata is not a distribution notice bundle or an approved license policy;
   adding `LICENSE` alone does not complete this gate.
5. Verify the destination name is available to an organization administrator,
   the transferring account has repository admin rights, and it can create a
   repository in `eez-association`. An unauthenticated 404 does not prove name
   availability. Confirm organization Actions policy, billing capacity while
   private, package creation rights, and maintainer/team access. See
   [GitHub's transfer prerequisites and effects](https://docs.github.com/en/repositories/creating-and-managing-repositories/transferring-a-repository).

## Transfer while private

1. Save a private inventory of repository access/rulesets, webhooks and Apps,
   Actions settings, variable/secret **names**, package links/permissions, and
   package digests in use. Preserve operator `.env`, genesis, bindings and
   datadirs. Never include secret values in a public migration record.
2. Transfer ownership through **Settings → General → Danger Zone → Transfer**
   after the access inventory, destination checks and administrator authorization.
   Publication gates may remain open during a private transfer. Keep visibility
   private while verifying the resulting owner/name, default branch,
   protocol pin, and collaborator/team permissions.
3. Configure a main-branch ruleset where the plan supports it; otherwise complete
   this immediately after publication and before merging or releasing. Require
   PR review, dismiss stale approvals, require resolved conversations and the
   existing **CI Result** check, and
   restrict force pushes/deletion. Verify the observed check's exact name and
   GitHub Actions source before selecting it. Grant the intended organization
   team access and replace the temporary `@eduadiez` entry in
   [CODEOWNERS](../../.github/CODEOWNERS). Review bypass permissions explicitly.
4. Recheck Actions permissions and fork approval settings. Fork code must run
   only on hosted VMs without write tokens or repository secrets. The existing
   producer disables publication for fork and Dependabot events and executes
   all suites in its no-publication path. Do not switch to `pull_request_target`
   to gain permissions. Verify no organization runner group grants this repo
   access to the retired homelab.
5. If included private minutes are available, validate organization CI now.
   Otherwise leave it explicitly pending and follow the publication route above.

## Validate organization CI

Run CI on the transferred revision and on main after accepted fixes merge.
Do this while private only when included minutes are available; otherwise do it
after the publication gates below are cleared. The producer derives image names
from lowercased `GITHUB_REPOSITORY`, so it publishes under
`ghcr.io/eez-association/rollupx-*`. Verify all producer, lint, debug unit,
native shard and Kurtosis lanes pass, and promotion uses those tested
digests. Verify package access for the repository's Actions token. Tests
exercise namespace changes locally with mocked Docker; only this run proves
organization permissions and real publication. The existing CI gate must
remain required; a budget-blocked job is not an executed check.

## Switch package references together

Repository transfer does not itself prove container package migration or public
access. See [GitHub's package transfer behavior](https://docs.github.com/en/packages/learn-github-packages/about-permissions-for-github-packages#about-repository-transfers)
and [organization package visibility](https://docs.github.com/en/packages/learn-github-packages/configuring-a-packages-access-control-and-visibility#configuring-visibility-of-packages-for-an-organization).

1. Verify the new node, proof-signer, and deploy images from the passing
   organization run. Check the artifact manifest's root revision, protocol
   revision, lockfile hash, binary hashes, immutable digests and OCI source
   labels with [the artifact verifier](../../scripts/ci/prepare-runtime-artifacts.sh).
   Its CI caller supplies the matching checkout, `GITHUB_SHA`,
   `GITHUB_REPOSITORY`, and `EEZ_EXPECT_PUBLISHED=true`; preserve those identity
   checks. Copying old-owner images unchanged fails the new source/namespace
   checks and is not an acceptable replacement for this build.
2. Make intended release packages public and confirm anonymous pulls of the
   verified digests using a fresh, empty `DOCKER_CONFIG` on an isolated validation
   host. Test all three images. Confirm CI payload/package access separately;
   public source does not automatically make a GHCR package public. Complete
   the license/notice and package-layer disclosure review before exposing each
   package, including retained versions and CI payloads intended to be public.
3. Once those images exist, update all three `EEZ_*_IMAGE` defaults in
   [`.env.chiado.example`](../../.env.chiado.example),
   [`.env.ethereum.example`](../../.env.ethereum.example), and the fallback
   values in [the node Compose file](../../docker-compose.chiado-node.yml),
   plus image examples in [the deployment runbook](chiado-deployment.md).
   Search the current tree for other executable `ghcr.io/eduadiez/rollupx-`
   references. Preserve dated audit evidence and upstream attribution.
   Do not rename the genesis `config.rollupX` field: it is a configuration
   contract, independent of the repository owner.
4. Run `python3 -m unittest discover -s scripts/ci/tests -p 'test_chiado_config.py' -v`
   and `python3 scripts/check-settlement-policy-config.py` after template changes.
   Publish the old/new image mapping and verified digests. Keep old packages
   available for existing digest pins; operator `.env` overrides remain in
   effect. A reference change does not authorize starting containers or
   replacing deployed contracts/genesis.

The preparation PR deliberately retains working personal-namespace image
defaults until replacement packages are verified. Existing deployments do not
need a restart for the repository transfer. Any later upgrade must follow the
[deployment compatibility boundary](chiado-deployment.md#featureevents-protocol-compatibility).

## Make public and verify contributor access

1. Complete the disclosure and licensing gates above, then change repository
   visibility with the organization administrator. This makes retained source
   history and other repository content accessible; a later visibility change
   cannot recall copies. Recheck rulesets, collaborators, Apps, Actions and
   package permissions after the change. Review
   [GitHub's visibility effects](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/managing-repository-settings/setting-repository-visibility).
2. Enable private vulnerability reporting and verify **Security → Advisories →
   Report a vulnerability** is available to an external contributor. The
   [security policy](../../SECURITY.md) does not enable the feature. See
   [GitHub's configuration procedure](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/configure-vulnerability-reporting/configure-for-a-repository).
   Enable available secret scanning/push protection and dependency alerts;
   retain the pinned CI history scan and reviewed fixture exceptions.
3. From a clean environment without GitHub credentials, clone the new URL,
   initialize the exact protocol and recursive submodules, and resolve locked
   Git dependencies. Pull the three verified public runtime images anonymously.
   Check README links and installation instructions from the new location.
4. Complete [organization CI validation](#validate-organization-ci) if it was
   deferred for lack of private minutes. Verify an actual external fork PR runs
   all required checks on hosted runners
   with `published=false`, no registry writes, and the full local E2E fallback.
   Record job labels, selected/executed test counts and results. Validate the
   first organization main promotion independently of the pre-transfer PR run.
5. Record source/protocol revisions, CI URLs, image digests, visibility and
   permission checks in a sanitized [audit](../audits/TEMPLATE.md). Mark any
   unfinished criterion explicitly; green pre-transfer CI alone does not
   establish public contributor access or package visibility.

If hosted validation fails before publication, keep the repository private.
If it fails after publication on the no-private-minutes route, keep the failing
fix in draft and block image promotion and deployment-reference changes while
investigating; making source private again cannot recall existing copies.
Do not restore homelab scheduling for public PRs. Existing
deployments can continue using their verified image pins while publication is
paused; do not weaken artifact identity checks to work around the transfer.
