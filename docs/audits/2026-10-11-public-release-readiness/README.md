# Public release and organization transfer readiness

The recommended sequence is to prepare the repository while private, transfer it
to `eez-association`, validate organization CI and package publication, then make
it public. The remaining work concerns CI isolation, licensing, disclosure review,
package ownership, and repository governance. This assessment does not authorize
or perform the transfer, publication, history rewriting, or a deployment.

| Field | Value |
| --- | --- |
| Date and status | 2026-10-11 UTC; analysis complete, publication criteria remain open |
| Scope | Public release and transfer of `eduadiez/rollupX`; assumed destination `eez-association/rollupX`, pending naming decision |
| Local source | `fa916b13254a36423cad130080dde367bb18a7d6`; 53 commits behind observed GitHub `main`; existing local changes preserved |
| Reviewed release source | GitHub `main` at `e15bb412420b36bfa40a0e2a73233ac4f213441d`, also inspected from a fresh temporary mirror and archive |
| Protocol | Local checkout: `855fe0602484750861b4c31502a1a91355d33ed6`; reviewed GitHub source pins `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. Exact protocol objects inspected without changing the checkout; existing untracked protocol lock preserved |
| Tools | GitHub CLI 2.46.0; Gitleaks 8.30.1; Cast 1.7.1, commit `4072e48705af9d93e3c0f6e29e93b5e9a40caed8`; Python and Git |
| Evidence limits | Static inspection, automated secret scans, read-only GitHub API queries, anonymous dependency and registry metadata requests. No new runtime, process, semantic, replay, or deployment validation |

## Findings and required work

### Public pull requests need an isolated CI environment

The [CI workflow at the reviewed commit](https://github.com/eduadiez/rollupX/blob/e15bb412420b36bfa40a0e2a73233ac4f213441d/.github/workflows/ci.yml)
accepts `pull_request` events and defaults to `homelab-rust` through
`inputs.runner`, `vars.CI_RUNNER`, and, for lint, `vars.CI_AUX_RUNNER`.
The latest successful main run used `homelab-rust` for all build/test jobs;
only `CI Result` used `ubuntu-24.04`.

The producer already disables image publication for fork PRs and Dependabot,
uses checkout without persisted credentials, and provides a local validation
path without registry publication. Actions are pinned to commit SHAs. These are
useful controls, but fork code still executes build scripts, Docker operations,
and tests on the selected runner. Compiler caching can also use runner-provided
Redis access. The live runner topology, credentials, and network isolation were
not accessible to this review.

Before public PRs can run, route untrusted work to GitHub-hosted runners or a
separately isolated disposable pool with no access to trusted runner state,
private networks, credentials, or writable shared compiler caches. Restrict
trusted pools through runner access policy as well as workflow conditions.
Approval settings alone do not isolate executed code. Validate the full fork
path, including its serial E2E fallback and resource requirements; a hosted
option in the workflow does not prove that this workload fits its capacity.
See [GitHub's runner security guidance](https://docs.github.com/en/actions/reference/security/secure-use#hardening-for-self-hosted-runners).

### Formalize the license and inherited attribution

The workspace and standalone indexer declare MIT, but there is no repository
license file, and GitHub reports `license: null`. There is also no root
`SECURITY.md`, `CONTRIBUTING.md`, or `CODEOWNERS`; only the signer's contributor
guide exists. The pinned protocol's sampled `src/EEZ.sol` declares MIT through
SPDX, while its repository has no root license file. The upstream
`eez-association/eez-rollup0` also reports no detected license.

Confirm the intended license and authority to publish inherited contributions,
add the complete license and accurate attribution, and preserve applicable
third-party notices in source and binary distributions. Reth and Stateless are
among the dependencies reporting Apache-2.0. GitHub's license detection is an
inventory signal, not a conclusion about all source rights. Existing CI invokes
`cargo deny check advisories bans sources`; it does not run the license check.
Establish a reviewed license policy for both Cargo workspaces and distributed
contract/tool dependencies. See [GitHub's licensing guidance](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/licensing-a-repository).

### Finish disclosure review beyond the working tree

The fresh mirror contains 62 branches, 75 PR refs, no tag refs, and 806 reachable
commits. The reviewed main tree contains 1,368 files, including 644 audit/research
files and 21 `.log` files. It includes a retained agent transcript. GitHub lists
330 workflow runs and 25 artifact records; retained content was not downloaded
or cleared. There are no releases, wiki, Pages site, or configured environments
reported by the queried endpoints.

The automated scan's findings were classified as follows. Counts include
repeated occurrences across history and merge diffs.

| Classification | Initial history scan | History including merge diffs | Current main snapshot |
| --- | ---: | ---: | ---: |
| Anvil/Hardhat fixture keys, exact comparison with derived accounts 0–2 | 29 | 58 | 17 |
| Documented deterministic CI/devnet system key | 8 | 14 | 0 |
| Gnosis vector key, exact comparison with public upstream source | 2 | 4 | 0 |
| Image-tag false positives | 2 | 4 | 1 |
| Development Blockscout session-secret default | 1 | 4 | 1 |
| Total | 42 | 84 | 19 |

No finding confirmed a live operator credential. Known fixture keys should remain
identified as public development inputs. This classification does not establish
that an operator never reused a fixture key, or that all sensitive information
has been found. Supplemental current-tree checks found userinfo URLs in test
locations and a documented container-address example; they found no matches for
the checked PEM/GitHub/AWS credential signatures or credential query parameters.

Before publication, review audit/transcript content for information outside
secret-scanner patterns, all relevant branches and PR discussions, issue text,
Actions logs/artifacts/caches, and published image contents. GitHub explicitly
states that Actions history and logs become public when visibility changes.
If a real credential is discovered, revoke/rotate it before coordinating any
history or retained-content cleanup; removing it from the latest tree is not
sufficient. See [GitHub's visibility effects](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/managing-repository-settings/setting-repository-visibility#changing-from-private-to-public).

### Migrate container packages and their consumers together

CI derives names from `GITHUB_REPOSITORY`, so the assumed transfer changes the
prefix to `ghcr.io/eez-association/rollupx`. It produces node, proof-signer,
deploy, CI runtime/test payloads, and build-cache references. GHCR packages have
separate ownership and visibility; a repository transfer does not move their
account namespace and can remove repository access associations. See
[GitHub's package transfer rules](https://docs.github.com/en/packages/learn-github-packages/about-permissions-for-github-packages#about-repository-transfers).

The following current files still select the personal namespace:

- `.env.chiado.example`, lines 41–43, and `.env.ethereum.example`, lines 33–35.
- `docker-compose.chiado-node.yml`, lines 10 and 38.
- `docs/runbooks/chiado-deployment.md`, the runtime image table and CI link.
- Relevant configuration and artifact tests under `scripts/ci/tests/`.

The producer and [artifact verifier](https://github.com/eduadiez/rollupX/blob/e15bb412420b36bfa40a0e2a73233ac4f213441d/scripts/ci/prepare-runtime-artifacts.sh#L38)
bind images to both namespace and exact OCI source/revision/protocol labels.
An unchanged copy of an old image would still carry the old source URL. Use the
existing organization CI producer and complete validation gate for new release
images; preserve promotion from tested digests. Do not weaken identity checks.

Provision organization package permissions, set intended package visibility
explicitly, and verify anonymous access to release images. Coordinate switching
templates/runbooks with package availability. Keep old packages available for
existing digest/tag consumers and preserve operator `.env` files and datadirs.
Historical audit links can retain their original provenance.

Anonymous manifest requests succeeded for all three existing personal-namespace
images at the reviewed commit tag. This establishes anonymous manifest access,
not source-to-image provenance or a successful full image pull.

### Confirm organization permissions and establish maintenance ownership

GitHub reports a private standalone repository (`fork: false`) and admin access
for the authenticated user. The organization exists. Looking up
`eez-association/rollupX` returned 404; that does not exclude a private repository
hidden from this token. Organization membership/create permission was not
verifiable. An organization owner must confirm name availability, permission to
create/receive the repository, applicable plan, and inherited organization
policies. Repository admin access alone does not establish transfer eligibility.

GitHub reports `main` as unprotected. Configure team access, required PR reviews,
the `CI Result` required check, and appropriate force-push/deletion restrictions.
Configure available secret scanning and push protection, a private vulnerability
reporting route, and owners for protocol, signer, and CI changes. Verify Actions
permissions, runner access, secrets/variables, integrations, deploy keys, and
package access after transfer. API permission failures prevented inspecting
most of these settings; they are unknown, not absent.

A transfer preserves repository history and collaboration records and provides
Git redirects; it remains subject to destination policies. Repository-associated
secrets, hooks, and deploy keys stay associated, so verify their intended
ownership and use. Do not recreate the old repository path and invalidate its
redirect. See [GitHub's transfer documentation](https://docs.github.com/en/repositories/creating-and-managing-repositories/transferring-a-repository).

Update public-facing project naming: the README currently calls this
`eez-rollup0`, which is also an existing association repository. Add concise
contribution/security guidance and explain the relationship between the projects.
A branding change should preserve the existing `config.rollupX` genesis field;
changing that field has compatibility and commitment consequences.
`AGENTS.md` currently describes the repository as public, while GitHub reports
it as private. Reconcile that statement with the actual publication date.

## Proposed execution sequence and acceptance checks

These steps are recommendations, not executed operations.

1. Decide the destination name, license/attribution, maintainers, and public
   support scope. Prepare focused CI isolation, license/community, and namespace
   changes against current main. No protocol upgrade is needed for the transfer.
2. Finish disclosure review of the published refs and GitHub-hosted content.
   Resolve confirmed sensitive findings before visibility changes. Re-scan the
   final candidate and retain sanitized dispositions for intentional fixtures.
3. Confirm organization permissions, plan, runner capacity/access, and package
   administration. Transfer while private. Update local remotes and integration
   references; verify migrated settings and inherited organization access.
4. Run the existing full CI pipeline under the organization, using its verified
   exact-revision artifacts and all native/Kurtosis validation lanes. Publish
   release tags only after the gate passes. Verify the new namespace, OCI
   provenance, package permissions, and anonymous image access before changing
   default image consumers. Keep existing operator pins usable.
5. Complete the public visibility change, confirm protection/security settings
   under the destination plan, then verify an anonymous recursive clone, pinned
   dependency access, documented build/onboarding path, and a real fork PR on
   isolated runners with no publication rights or trusted cache access.

For the planned workflow/configuration changes, run the tooling suite
`python3 -m unittest discover -s scripts/ci/tests -v`, the Chiado and Ethereum
configuration regressions, `python3 scripts/check-settlement-policy-config.py`,
and `bash -n` on edited shell scripts. Preserve all existing CI release gates.
Validate namespace handling for an organization owner in artifact tests. If
license checks are added, cover the independent indexer workspace too.

Public source availability should not be presented as production security
certification. Review the public-facing readiness language alongside
[the existing production checklist](../../../PRODUCTION_READINESS_TODO.md);
this assessment did not revalidate its individual runtime items.

## Executed checks and retained evidence

The working directory was the repository root. Network operations were reads.
A fresh mirror and source archive were created in a task-owned temporary
directory, preserving the operator checkout. Gitleaks used its bundled rules,
100% redaction, no project allowlist, and `--ignore-gitleaks-allow`.

| Check | Result |
| --- | --- |
| Baseline Git identity/status/submodules | Local and GitHub source/protocol differences recorded above; existing edits preserved |
| GitHub repository, branches, community profile, releases, environments, CI runs/jobs and artifact metadata | Successful reads; observations recorded above |
| Organization membership and most Actions/secret/runner/hook/key/package administration APIs | HTTP 403; rights/configuration remain unverified. Rulesets endpoint also reported a plan limitation |
| Anonymous GitHub API access | Pinned protocol and all six locked Git dependencies accessible at their exact commits |
| Gitleaks `git MIRROR --log-opts=--all` | Exit 1; 669 commits reported, 37.35 MB scanned, 42 classified findings |
| Gitleaks `git MIRROR --log-opts='--all -m'` | Exit 1; 803 commits reported, 109.51 MB scanned, 84 classified findings. Mirror inventory contains 806 reachable commits; scanner counts are reported as emitted |
| Gitleaks `dir MAIN_ARCHIVE` | Exit 1; 24.25 MB scanned, 19 classified findings |
| Fixture attribution | Cast derived public Anvil/Hardhat accounts for exact comparison; Gnosis vector values matched the public pinned upstream script; deterministic system identity identified by its source documentation |
| Latest main CI run | [38141871060](https://github.com/eduadiez/rollupX/actions/runs/38141871060), all 13 jobs successful for `e15bb412420b36bfa40a0e2a73233ac4f213441d`; existing CI evidence, not a new test run |
| Runtime validation in this assessment | Not run: no runtime changes; no independent semantic/replay/deployment claims |
| Documentation validation | Tracked diff whitespace check clean; added report checked for whitespace and its three local/commit-specific source links resolved. No shell examples were added |

The Gitleaks [v8.30.1 release archive](https://github.com/gitleaks/gitleaks/releases/tag/v8.30.1)
was verified against its GitHub release asset digest before extraction:
`551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb`.
The executable SHA-256 was
`88f91962aa2f93ac6ab281d553b9e125f5197bbbce38f9f2437f7299c32e5509`.

Raw redacted reports, the mirror, and the source archive are local-only under
`/tmp/rollupx-public-readiness.sbbfXWtj/`; they have no durable retention guarantee
and are not included in this repository. The sanitized findings above are the
retained evidence summary.

| Local report | SHA-256 |
| --- | --- |
| `gitleaks-history.redacted.json` | `43a3435c0f79fd26ce650a1eedbb454929396053b5ce980746c12336b968d129` |
| `gitleaks-history-merges.redacted.json` | `39f6fbb9dc8d933c839177f1c193df9dfa8d97a5075a1897058d13195c35b339` |
| `gitleaks-main.redacted.json` | `fb5b2a23efbd70938403e0b1cfef2e75d3ba067f8083961dabe2cc076e2eac55` |

Remaining acceptance work is the five-step sequence above, especially review of
GitHub-hosted content, organization permission verification, and actual public
fork/anonymous onboarding checks. No ownership, visibility, package, service,
wallet, remote configuration, or existing source file was changed.
