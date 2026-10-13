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

## Preparation PR follow-up, 2026-10-11

The maintainer subsequently requested a preparation PR and retirement of homelab
CI. Implementation commit `a31dcd7e9d083a55409555d3fb7520fe650a3db0` is based on
the reviewed `e15bb412420b36bfa40a0e2a73233ac4f213441d`; the protocol remains
`9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. Work was performed in an isolated
clone on `codex/public-release-preparation`, preserving the original operator
checkout. See the [proposal](../../proposals/github-hosted-ci.md) and
[release runbook](../../runbooks/public-release.md) for scope and pending cutover.

| Check | Result |
| --- | --- |
| Baseline hosted-only regressions | Two tests selected against the original workflows; 12 failing assertions expose runner overrides and enabled retained Cargo state |
| Full tooling suite, `python3 -m unittest discover -s scripts/ci/tests -v` | 361 tests selected, 356 passed, five skipped, exit 0; includes hosted-only contract, organization payload/runtime handoff, old-owner rejection, and actual Gitleaks fixture-adjacent secret detection |
| Skipped opt-in checks | Real Docker CI payload, real Kurtosis relay resources, native archive relocation, and two real Docker runtime fixture tests; dedicated execution/build environments not prepared |
| Workflow validation | Actionlint 1.7.12 passed all three workflows; official release archive verified against published checksums |
| Configuration propagation | `python3 scripts/check-settlement-policy-config.py`: Chiado, Kurtosis and Ethereum passed |
| Secret scan of implementation commit and fetched history | Gitleaks 8.30.1, `git . --config .gitleaks.toml --log-opts='--all -m' --redact=100 --ignore-gitleaks-allow --gitleaks-ignore-path /dev/null`; 800 commits reported, 102.06 MB scanned, no unsuppressed findings, exit 0 |
| Documentation and syntax | 97 local path links resolved; `bash -n` passed 59 workflow shell steps with Actions expressions replaced by placeholders, plus added documentation examples; `git diff --check` passed |
| Hosted builds and E2E | Pending the PR Actions run; local results are static/mocked tooling evidence, not runtime semantic, replay, capacity or deployment validation |

The clone's fetched history excludes PR-only refs present in the earlier mirror;
the two scan inventories are distinct. Scanner exceptions are exact reviewed
public values, with path constraints for historical/default fixtures and image
tags. New secrets in fixture files still fail. The initial exception check left
six already-classified findings: the archived deterministic CI identity needed
its exact audit path, and the image rule captures only the tag. Correcting those
exceptions produced the passing scan above; no newly discovered operator key
was suppressed. An initial focused test command misspelled the runtime test class;
the corrected command executed and passed both intended namespace cases.

Logs remain local-only with no retention guarantee. The full tooling log is
`/tmp/rollupx-public-pr-tooling.log` (SHA-256
`88e9fc907cc6f9fbb1e4ce4e11b5c9cc72d6abc5510baacf02df83b8f92ff8fe`), and
the baseline regression log is `/tmp/rollupx-public-pr-baseline.log` (SHA-256
`62d017f228b8b1d028540e6141b65eb3bc031110c409616b509bad13db6b7dc0`).
The PR retains this sanitized summary. Transfer, visibility, rulesets, private
reporting, runner deregistration and organization package publication remain
unexecuted administrative steps. Existing image defaults remain usable until
verified organization packages exist.

## Hosted Compose regression and correction, 2026-10-11

At PR head `0dbb123489d161bcb11ce425074477a24ccca6d4`, the hosted
[lint job](https://github.com/eduadiez/rollupX/actions/runs/38154147332/job/114513522899)
failed `test_existing_bindings_are_not_read_by_static_render`. The
[runner image inventory](https://github.com/actions/runner-images/blob/ubuntu24/20261004.327/images/ubuntu/Ubuntu2404-Readme.md)
lists Compose 2.38.2. That CLI still reads and interpolates `deployments.env`
under `config --no-env-resolution`, so the test's deliberately invalid binding
causes an error. The same focused test fails with 2.38.2 at both the PR head and
the original base; the earlier successful local tooling run used Compose 5.5.1.
This is a tool-version dependency exposed by the runner change.

The correction installs [Compose 5.5.1](https://github.com/docker/compose/releases/tag/v5.5.1)
in a job-private plugin directory before the tooling checks. It verifies the
official Linux x86_64 asset SHA-256
`db1889184726840f75c4f9c001048430d4f25b3be3cb084d3ddd762bc0aed576`, checks the
selected CLI version, and exports that directory through `GITHUB_ENV`.
The existing test and its assertion are unchanged. No daemon or deployment is
needed for these configuration checks; the protocol pin remains unchanged.

The exact installer shell body was extracted from the edited workflow and
executed with task-owned temporary paths. Its exported `DOCKER_CONFIG` was then
used for the after-check and configuration propagation checks:

| Check | Result |
| --- | --- |
| Same focused regression with Compose 2.38.2 | One test selected, failed with the hosted error, exit 1 |
| Same focused regression with workflow-installed Compose 5.5.1 | One test selected, passed, exit 0 |
| `check-settlement-policy-config.py` with the exported plugin directory | Chiado, Kurtosis and Ethereum passed |
| Full tooling suite with the exported plugin directory and Gitleaks on `PATH` | 361 tests selected, 356 passed, the same five opt-in integration checks skipped, exit 0 |
| Workflow/documentation checks | Actionlint 1.7.12, installer `bash -n`, 55 local path links, and `git diff --check` passed |

The focused command was `PYTHONPATH=scripts/ci/tests python3 -m unittest test_chiado_config.ChiadoConfigTest.test_existing_bindings_are_not_read_by_static_render -v`.
Before/after logs are local-only at `/tmp/rollupx-pr84-compose-fix-before.log`
(SHA-256 `ed3c5adba76f9f1642b83408c96b4412a05653dccc2d5b72f2398eb0f41be6c8`)
and `/tmp/rollupx-pr84-compose-fix-after.log`
(`25a9844b372346305423b3160f3cea2576d5946d86d05021cdadfe6f9921591e`).
The full suite log is `/tmp/rollupx-pr84-compose-fix-tooling.log`
(`488164743379381be3829e335ee0d009827524a79cd29a4f264278e65a205c66`).
The updated hosted run remains required; these local checks do not establish
full Rust, E2E, protocol replay or fork-path capacity on the new runner.

## Hosted Kurtosis failures and correction (2026-10-12)

The run at `f20dbc7941a1e9f03d8849c819df11c1cc0c6236` passed lint,
debug unit tests, the release producer, and all eight native E2E partitions.
The protocol pin remains `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`.
[Attempt 1](https://github.com/eduadiez/rollupX/actions/runs/38157778162/attempts/1)
passed the inbound wave but failed outbound canonical settlement after its
60-second deadline. Cold protocol compilation overlapped the network checks
for 905 seconds. Contention is a hypothesis, not an established explanation for
the settlement failure.
[Attempt 2](https://github.com/eduadiez/rollupX/actions/runs/38157778162/attempts/2)
failed earlier: the relay API exited with a duplicate `custom_migrations_pkey`
while applying migration `005-remove-blocked-validator`. Neither attempt could
upload failure artifacts because the account's artifact storage quota was full.

The exact [Ethereum package launcher](https://github.com/ethpandaops/ethereum-package/blob/199620b24ac979c676010c5a68b2893c2bce4f1f/src/mev/flashbots/mev_relay/mev_relay_launcher.star)
starts a housekeeper without ports or readiness, then immediately starts the API.
The pinned relay image's labels identify Flashbots source
`fddb05e974622396c1f0c03f7285617ba13b6e0f`; both commands call the same
[database initializer](https://github.com/flashbots/mev-boost-relay/blob/fddb05e974622396c1f0c03f7285617ba13b6e0f/database/database.go),
which applies migrations without coordinating the two processes. The
[housekeeper](https://github.com/flashbots/mev-boost-relay/blob/fddb05e974622396c1f0c03f7285617ba13b6e0f/services/housekeeper/housekeeper.go)
opens `/metrics` only after database initialization and beacon readiness.

The correction inserts a bounded loopback health wait immediately after the
upstream housekeeper launch. A small Plan wrapper is necessary because the
pinned launcher exposes no readiness hook; a real probe established that a
later `set_service` readiness override does not delay the original creation.
The wrapper preserves service configuration, return values, optional arguments
(including the upstream Reth launcher's `force_update`), and every other method.
A failed wait prevents API creation. It does not skip migrations or retry a
failed suite.

Both host Foundry roots, `eez-core-protocol` and `contracts`, now finish compiling
before network startup. Each retains one compiler thread, a 20-minute limit,
the pinned compiler settings and process-group cancellation. Workload assertions,
settlement deadlines, scenario selection and artifact provenance remain intact.
Failure console output now includes bounded builder, relay, beacon, validator
and MEV-Boost tails as well as node/signer logs. This preserves useful diagnostics
when the separate artifact upload fails; it does not clear account storage.

Local validation:

- The preparation-order regression failed at the previous parallel shell and
  passed after serialization. The final focused suite selected 28 tests:
  26 passed, two opt-in Kurtosis tests skipped.
- The final full tooling suite selected 363 tests: 357 passed and six opt-in
  checks skipped. Two of those six are the real Kurtosis checks below; the
  remaining four require separate Docker payload/runtime or native archive
  fixtures and were not executed. The local-only full log is
  `/tmp/rollupx-pr84-kurtosis-final-tooling.log` (SHA-256
  `c0bd99c32615091604de14e71c0bf3be7e593717fc91cd96629142dca01deae5`).
- Both opt-in checks were run separately in uniquely named, disposable enclaves:
  the existing real CPU-override check passed, and the readiness before/after
  check failed without the wait and passed with it. These exercise real Kurtosis
  ordering with an HTTP fixture, not the complete relay/database topology.
- The exact `wget -q -T 2 -O /dev/null` probe also passed in the pinned relay
  image against an isolated loopback HTTP fixture.
- Full pinned-package planning passed and placed the wait between housekeeper
  and API creation. Node/signer/deploy images were inert placeholders for this
  dry run; no application services or runtime binaries were executed.
- Actionlint, shell syntax, configuration propagation and `git diff --check`
  passed. Claude's final read-only review resolved both findings and found no
  blocking issue; full hosted E2E remains required.

The original job logs are retained locally at
`/tmp/rollupx-pr84-kurtosis-failure.log` (SHA-256
`f7e53896832de71bc010e40209033fc5e82831019d9bed17ced8b2fb112e2a46`) and
`/tmp/rollupx-pr84-kurtosis-attempt2.log`
(`d886341a61cd7e2c0eb5d29565f5652b8430b1a18ab19e822080751e391be0dd`).
They are not committed evidence and depend on local retention; the run links
provide the remote job logs. No existing enclave, deployment, wallet or
operator configuration was changed. A new hosted run must still establish
full settlement and protocol-replay success before merge.

## Deployment output and native shutdown investigation (2026-10-12)

The [next hosted run](https://github.com/eduadiez/rollupX/actions/runs/38239878942)
tested PR head `ccb393d621763ce296edab7903820c7b22704b04` through merge revision
`c80b0ef677d872a25d0f270c132a7216ba1467d9`, against base
`e15bb412420b36bfa40a0e2a73233ac4f213441d`. The protocol remained
`9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. All jobs used hosted runners.
Lint, debug units, the release build and seven native partitions passed. The
embedded partition 1/4 and Kurtosis failed, so promotion remained blocked.
The relay readiness gate passed, as did the inbound, outbound, mixed and
proof-signer semantic checks preceding the new Kurtosis failure.

### Deployment parser: reproduced SIGPIPE

The mixed-pure scenario failed while extracting a deployed address:
`lib.sh: printf: Broken pipe`. `grab_address` used `grep -m1`, which closed its
input after the first match. A successful Forge command with substantial later
output made the producer's `printf` fail under `pipefail`.

The regression runs the real helper with a stub Forge producing more than a
megabyte after its address: baseline exit 141, corrected helper success. The
parser now consumes the complete stream while returning the first match. The
21 wave tooling checks pass, including missing/short addresses and a Forge
failure after printing an address. No deployment was broadcast by this check.

### Native restart: worker cleanup outlived termination

The hosted restart test failed at `restart_composer_with_env` with SIGABRT and
`pthread lock: Invalid argument`. The original CI test payload could not be
pulled with the available registry authorization. Exact published Composer,
follower and signer images were anonymously retrievable and verified against
the CI merge revision and protocol labels; their runtime bytes were retained.
One diagnostic using those bytes and a source-built driver passed. This was
not a rerun of the inaccessible CI test archive and did not establish absence
of the race.

The exact fork baseline was then built locally with Rust 1.95.0, Foundry 1.7.1,
LLVM 22, locked dependencies and a separate target directory. Two diagnostics
observed RocksDB's abort path on a thread named `persistence`; one still had a
zero process status because process exit outran that worker. It is not recorded
as clean validation. A further diagnostic added a bounded 200 ms `atexit`
delay and an abort backtrace hook **only to owned test subprocesses**. It
reproduced the hosted SIGABRT and resolved this stack:

```text
rocksdb::port::PthreadCall / Mutex::Lock
rocksdb::DBImpl::CancelPeriodicTaskScheduler
rocksdb::DBImpl::CloseHelper / CloseImpl / ~DBImpl
rocksdb_optimistictransactiondb_close
RocksDBBalStore / ProviderFactory<EezNode> destruction
PersistenceService<EezNode>::run
```

In pinned Reth `3d592ece6de8c4559987416a544fc215fd6d6921`, the engine sends its
termination acknowledgement before dropping itself. The CLI can return while
the persistence thread is still closing RocksDB. The stack identifies L2
cleanup, not a settlement assertion or the signer's database. The instrumentation
amplifies the exit race; it is not production code or a performance measurement.

A separate defect was found in the embedded L1 wiring: sharing the Tokio handle
does not share Reth's shutdown counter/signal or panic monitor. Linking those
lifecycles fixes the missing L1 drain, but the same instrumented process still
aborted with that correction alone. Both first-attempt failures are retained;
the L1 helper is not claimed as the RocksDB fix.

The [shutdown proposal](../../proposals/reth-shutdown-backport.md) therefore
includes a focused backport of the still-open
[upstream Reth correction](https://github.com/paradigmxyz/reth/pull/27609), head
`00a894ab7925d9af094689478abcb8aaaa410459`. It drops engine resources and joins
workers before acknowledging termination, and waits for explicit shutdown in
the launcher. Only `reth-engine-tree` and `reth-node-builder` are replaced with
local v2.7.0 copies. All other lockfile package entries, dependency versions and
dependency lists are unchanged. Their source diff and licenses are retained in
[the vendor record](../../../crates/vendor/reth/README.md).

The resulting lockfile source change also changes prover validation identity.
An initial diagnostic reusing the old CI signer was rejected at
startup, before reaching restart. It is not shutdown coverage. Composer and
signer must be built from the same new lockfile for the comparison and upgrade.

### Restart fixture: a one-shot hold did not stop later proposals

One baseline diagnostic instead failed with two `BatchPosted` events where the
test expected one. The relay held the second proposal, but a third recomposition
escaped while the test waited for the first L1 block to reach disk. The new
900-second policy had reached the restarted node correctly; the additional
settlement predated that restart.

The fixture now atomically allows one bundle and rejects every subsequent
bundle until resumed. It resumes delivery after restart **before** checking
policy inactivity, so the barrier cannot manufacture that result. The canonical
anchor, eventual convergence and independent follower assertions remain intact.
The existing one-shot controls retain their previous behavior. All 18 builder
control tests pass, including repeated recompositions and delivery recovery.

### Local validation and remaining limits

- The production L1 helper's two lifecycle tests failed against the old wiring
  and passed with the correction. The final `eez-node --lib` suite passed all
  79 tests. Its first sandboxed attempt passed 72 and could not bind sockets for
  seven HTTP fixtures; rerunning with local sockets permitted passed all 79.
  These are environment-blocked attempts, not retry-only flaky passes.
- With the same 200 ms abort instrumentation, the original restart driver
  passed against the backport (100.36 s), followed by the corrected fixture
  (100.83 s). No abort marker or RocksDB failure appeared. Detailed logs for
  the latter contain three graceful restarts: each terminates both engines,
  completes the embedded L1 drain and records no shutdown timeout or persistence
  failure. These timings identify runs; they are not performance comparisons.
- Clearing all prebuilt overrides, the three `settlement_policy::restart_`
  source-based process tests passed, with their canonical settlement and
  independent follower-replay assertions (309.19 s including fixture builds).
- The upstream `test_engine_termination_waits_for_cleanup` passed for both
  successful persistence and persistence error. It also passed through the new
  CI Nextest command: one test selected, 206 others filtered. The separate
  locked vendor workspace adds four test-only packages without changing any
  shared package version/source. The upstream Ethereum process regression
  retained in `shutdown.patch` was not separately executed.
- `cargo +1.95.0 fmt --all --check`, workspace/all-target Clippy with
  `-D warnings`, `cargo deny check advisories bans sources`, and `cargo audit`
  passed. Audit reported nine already-allowed warnings for the root lock and
  six for the separate vendor test lock. Both copied crate
  directories, including manifests, were byte-compared against the exact pinned
  checkout plus the recorded patch: 52 engine files and 29 builder files match.
- The full tooling suite selected 369 tests: 363 passed and six opt-in environment
  checks skipped. After adding the targeted CI regression step, all 20 workflow
  contract checks and Actionlint passed. Shell syntax, edited links and
  whitespace checks passed. The vendored-source secret scan found no leaks.
- Claude's read-only review found no blocker in the worker joins or acknowledgement
  ordering. Its shutdown-budget clarification and fixture startup-dispatch
  assertion were added: the restarted process must emit no bundle dispatch even
  during barrier removal. The regression is now part of the debug unit CI lane.

The Linux-only [abort hook](abort-trace.c) is retained to reproduce the diagnostic,
not linked into production or CI tests. Build it with
`cc -shared -fPIC -g -O0 -o /tmp/eez-abort-trace.so abort-trace.c -ldl -pthread`
from this audit directory, and preload it only in an owned test Composer wrapper.
The wrapper must exec the recorded Composer binary; use the native restart
filter above with separately retained logs/datadirs and matching node/signer
validation identities. Build baseline and candidate from the recorded sources
before selecting these diagnostic overrides. The ordinary source-based check
clears overrides and has no preload or exit delay.

Retained diagnostic executable identities (SHA-256):

| Executable | Baseline | Backport |
| --- | --- | --- |
| Composer | `f90291ca09a48dc7ab2830471db1e2ed2318688e93d70bc327e6ff3f450b87c8` | `da128ddd37e73053dbcc814559adca843fcb10a1e9572eb60f119d3c807d53e2` |
| Follower | `6b71f1b782fa0f2a3b784a0bee5f75981e15ed9885af5603473fe5417afcafd6` | `5169a734ab0bc75d10e8854f66cbe7a42089445475dfd03606cb2899080afb0c` |
| Signer | CI runtime `df36c7f5b81594856db5f84f0577879a3b14bb58ebac4f4223612140af29679a` | `4f133ee6aa480c66cb8394f4c5f2a2f65fc559efd013e4cfbdb752e0cdf7d0bc` |

The ordinary source harness rebuilt the signer under its normal feature selection
(`fd03ff48d9c93548dc3c1f9c64d3cc1880ba1f6efa08d5ffca611bd0b4124f6a`).
Neither local diagnostic nor debug-source binaries are the new hosted release
artifacts. Full hosted shipping-build, artifact-verification, native partitions,
Kurtosis and public-fork acceptance remain required before merge/transfer.

Selected local evidence SHA-256:

| File under `/tmp/` | Digest |
| --- | --- |
| `rollupx-pr84-native-abort-backtrace.txt` | `294b7234f074fe50f1f3ff38138d243f01a99e87d9de798423802b717e6a644d` |
| `rollupx-pr84-native-before-4.log` | `073e09b9ce8a8e68fcb527152b071ad9ef0a175746f4975e2da57e9c4297a181` |
| `rollupx-pr84-native-backport-traced-3.log` | `de98f0dede8ce85c111b9a4e0dd184f6fa74d89bfb85e7a3e341e12bec74c0f9` |
| `rollupx-pr84-final-source-restarts.log` | `ff00093b963faa12413d75e55faef7f7b662eaf6660419a2ad8503bc9bb6605b` |
| `rollupx-pr84-final-tooling.log` | `abcce0e1675e55075eed3b435cf79cc424d2d6cef732aed8d59bdbbaea619be0` |

Local first-failure evidence is retained under
`artifacts/pr84-investigation/baseline-{1,2,3,4}` and
`artifacts/pr84-investigation/candidate-traced-1` in the operator checkout.
The symbolized backtrace is `/tmp/rollupx-pr84-native-abort-backtrace.txt`.
These ignored/local files are not portable evidence; the hosted run link above
retains the original CI failures. No shared deployment, wallet, container,
enclave, operator configuration, repository visibility or organization setting
was changed. Hosted acceptance and publication criteria remain open.

## Native CI failure diagnostics follow-up (2026-10-12)

The [hosted run for `711e554`](https://github.com/eduadiez/rollupX/actions/runs/38259566576)
completed with successful lint, debug units (including the upstream shutdown
regression), release-artifact production, Kurtosis, and six of eight native
partitions. The earlier shutdown/restart and Forge-parser failures did not recur.
Two different native tests failed; this follow-up does **not** establish their
root causes or clear them for merge.

| Failure | First retained evidence | Unresolved question |
| --- | --- | --- |
| `settlement_policy::conditional_position_paces_late_pool_transactions_and_replays` | [Embedded partition 3](https://github.com/eduadiez/rollupX/actions/runs/38259566576/job/114881807256): fixture setup returned RPC `-32000`, `replacement transaction underpriced`, before the scenario began | Did a rollback of unsafe deployment receipts let the next helper reuse a nonce whose earlier transaction had returned to the pool? The surviving log cannot identify the sender or nonce. |
| `dynamic_admission::consecutive_skipped_slots_keep_the_full_group_and_reach_the_next_block` | [Cross-chain partition 1](https://github.com/eduadiez/rollupX/actions/runs/38259566576/job/114881807280): original block-9 target `1791819424` and first replacement `1791819429` dispatched; expected replacement `1791819434` was absent | Which replacement/preparation step missed the available window? The later `canonical settlement evidence changed during proving` error occurred after the target L1 block arrived; it does not identify the earlier cause. |

The second trace shows retained users and a subsequent dispatch for block 10 at
`1791819439`. That later progress does not satisfy the test's exact-slot assertion.
No assertion, retry policy, nonce selection, timing budget, or production
scheduling behavior has been relaxed.

GitHub's artifact storage quota rejected uploads, so the CI node/datadir artifacts
are unavailable. The console records remain available at the job links above.
The test harness now retains useful failure evidence in that console:

- A setup guard emits bounded node/signer and reconciliation tails before an
  early `Result` error drops the processes. Previously the node's panic-only
  destructor ran before the caller panicked, losing its diagnostic output.
- Rejected fixture submissions include transaction/chain/sender/nonce identity
  and best-effort latest/pending nonce and conflicting pool hashes. Three reads
  share a two-second deadline; unsupported/unavailable diagnostics retain the
  original RPC error. These observations are not an atomic snapshot. Pool
  inspection is enabled only in the isolated loopback test fixture, and output
  excludes transaction input. Successful setup receipts retain hash/block
  identities for correlating later rollback evidence.
- Separate bounded slot/replacement and proof-deadline/posting timelines survive
  verbose blob records. The fixture enables scheduler logging, and individual
  printed records are limited to 4,096 characters; full retained files are
  unaffected. Failure assertions include the requested target identities.

### Exact baseline and local reproductions

PR base remains `e15bb412420b36bfa40a0e2a73233ac4f213441d`, branch head
`711e55476eda9889b7f074127055c27addf3a6a6`, and protocol gitlink
`9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. CI built the synthetic merge
`0c6a0b4de6744ec5e386a501911d033ba4e89a86`; its tree and the branch head's
tree both equal `d275149601085397df0719bd744ed9b70dbbb10c`.

Published runtime images were pulled by the digests recorded in the producer's
log. OCI revision/protocol/profile/platform labels were checked and binaries
extracted from owned containers which were never started and were then removed.
The private test payload was inaccessible, and the artifact manifest upload was
unavailable. Consequently these checks used a local source-built test driver,
not the verified CI release test archive or the complete artifact consumer path.

| Runtime image | Digest |
| --- | --- |
| `ghcr.io/eduadiez/rollupx-node` | `sha256:02ab43df338104a04ec15f12a30860b43c4a49222c9fd318f5907b453dcd8924` |
| `ghcr.io/eduadiez/rollupx-proof-signer` | `sha256:89331c846c2bb123147f384fceda6217ab85e9e7e6622185ebe1482cd6216767` |

| Executable | SHA-256 |
| --- | --- |
| Extracted Composer | `c5542e924fddcea05bd2b08a0fdb70f742904d547701d14f7734e633c0c2cc94` |
| Extracted follower | `209f346f64d79d6ad849d923f61fefe2d0c672cf20954f07c541413e1fdc85b3` |
| Extracted signer | `40409ee2feeceaff0e8301e2cd635e34cec417e98c08d5dc94afdddc60bd55f4` |
| Unchanged debug-profile test driver | `5bc1a62df3e05074aaf9d8c9be224c51cd830460e718a9e452583b06c83488cb` |
| First diagnostic test driver | `5e61d026b9528b4b34afa41d16424b0a0e1b5dae840a28499c694ebe43ffd825` |
| Final diagnostic test driver | `d544423060e31b8c85af1f4b911038360be7797c125f648d1282c7eb276b3787` |

Each failing test passed three local runs with the unchanged driver and release
runtime restricted to two CPUs (`taskset -c 0,2`). Both also passed three one-CPU
runs with the first diagnostic driver. These were bounded diagnostic series,
with every result retained; they do not replace the first CI failures. CPU
affinity does not reproduce hosted hardware, memory, disk, or scheduling.
Earlier unmodified source-based checks also passed each test individually.

Reproduction uses each full test name above with `--exact --test-threads=1
--nocapture`, `EEZ_TEST_PREBUILT=1`, and both runtime override paths pointing at
the recorded extracted binaries. Each invocation has a 300-second process
timeout and fresh `EEZ_TEST_LOG_DIR`/`EEZ_TEST_DATADIR_DIR`; `LD_PRELOAD` is unset.
This is a diagnostic executable override, not a CI fallback or release validation.

The local-only evidence is under `artifacts/pr84-investigation/` in the operator
checkout: `hosted-baseline-{fixture,skipped}-{1,2,3}`,
`one-cpu-diagnostic-{fixture,skipped}-{1,2,3}`, and `final-diagnostic-*`.
The first two series' result inventories are `hosted-baseline-results.json` and
`one-cpu-diagnostic-results.json`. These ignored files are unavailable to another
checkout. Downloaded console logs in `/tmp/` have these identities:

| File | SHA-256 |
| --- | --- |
| `rollupx-pr84-ci-38259566576-embedded-3.log` | `73029b629cec5a1b04254573a0661ebe4421b53effac309683c20378af3d0d97` |
| `rollupx-pr84-ci-38259566576-crosschain-1.log` | `f1c0f8414fdf286ad97d4f4c883c89f4d718702ab4489dfa461034fc51cb2ebf` |

The final mock RPC/log unit suite passed 18 tests, including rejection identity,
original error preservation and bounded Unicode output. Workspace/all-target
Clippy, Rust formatting and whitespace checks passed. The final diagnostic driver
passed both failing scenarios and
`settlement_policy::ordinary_rpc_rejects_unpostable_data_without_consuming_nonce`
on two CPUs, retaining their semantic assertions and the skipped-slot follower
comparison. A direct query verified the fixture's L2 pool RPC, and its logs
contained scheduler trigger/decision records. The first auxiliary RPC probe
missed children launched by worker threads; that fixture test passed, and the
probe succeeded after selecting the owned endpoint from its log instead.

Claude's read-only follow-up review found no blocker and checked the error
consumers, log selectors and fixture-only API changes. Its requested nonce/pool
and deadline diagnostics were incorporated. The embedded L1 and cross-chain
fronts still do not expose pool inspection; failures there retain nonce/error
observations and explicitly report unavailable pool data. Review output is
local-only at `/tmp/rollupx-pr84-diagnostics-final-claude.txt`, SHA-256
`82d4898f4f4a4879e10561db09dbc4b44f4908c6e95b2df1d5881e0ad474bb6a`.
No new dependency, protocol or production behavior changed. The inaccessible
release test archive and unreproduced first CI failures remain gaps; the fresh
full hosted matrix remains the acceptance gate.

## Post-merge CI correction (2026-10-13)

PR #84 merged as `51d5cd792ee7d570d00d6a53ae4b5f1d168bf440` with tree
`613801166dfb2b16c0e01254bbef40cde7ddbfec`, identical to the passing PR head
`0a0d46af11fc4f415e3652cb06b7f9776cf924a4`. The protocol remains
`9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. The subsequent
[main run](https://github.com/eduadiez/rollupX/actions/runs/38303851672) failed
the vulnerability check and embedded slice 1/4. The producer, debug units,
other seven native partitions and Kurtosis passed. The final gate correctly
skipped commit/latest image promotion.

### Dependency audit

`cargo audit --json` reproduced three advisories for locked `quinn-proto 0.11.17`:
`RUSTSEC-2026-0338`, `RUSTSEC-2026-0340`, and `RUSTSEC-2026-0343`. The local
RustSec database was `d5f46becf4d442d460e1fce5e12f2c0f00924868`. Its reviewed
advisories identify `0.11.18` as patched. Both the main lockfile and the separate
Reth test lockfile update that package's version/checksum only; all other package
entries, sources, dependency edges and the shutdown backport remain unchanged.
The package is retained through optional dependency resolution; the current
workspace's resolved graph contains no active path (`cargo tree --locked
--workspace --all-features --target all -i quinn-proto`). This does not waive the lockfile
audit or establish safety for a future QUIC-enabled build.

The full lockfile contributes to validation identity. Composer and signer must
be rebuilt/upgraded or rolled back together; an old/new pair fails capability
agreement. There is no new wire, contract, genesis, database or follower replay
format. Rolling back also restores the vulnerable locked version. Existing
in-memory queue recovery limits still apply; no deployment was performed. See
the [operator guidance](../../runbooks/chiado-deployment.md#restart-and-stop).

### Settlement test synchronization

The failing `interval_preserves_execution_but_waits_below_all_thresholds` test
assumed twelve additional L2 blocks implied a successful planning hint. During
startup catch-up, already-expired boundaries legitimately use the Empty recovery
path without calling the planner. In the retained CI trace, the last cutoff
reported `prepare=false, skip_post=false`; that is not a successful not-due hint.

The unchanged source test reproduced the same missing-hint assertion locally
on two CPUs (one selected test failed, 153.57 seconds including fixture binary
compilation). The initial synchronization fix passed the same command before the
dependency update (one selected test passed, 156.02 seconds). Claude's review
identified that a cancelled planner may still log an unused result. The final
test therefore waits, under the existing three-minute bound, for a not-due
`eez.composer.slot.plan` after the observed execution height and a matching
`eez.sequencer.slot.decision` with `prepare=false, skip_post=true`. It then waits
through that plan's boundary and the original twelve-block progress threshold.
It retains the zero-postBatch and absence-of-expensive-preparation assertions.
The initial successful trace contains a real not-due hint and `skip_post=true`
for target 78. No production scheduling, retry, timeout, or settlement rule changed.

Both checks used Rust 1.95.0, Foundry 1.7.1 and the initialized protocol, with
`EEZ_TEST_PREBUILT`, `EEZ_TEST_RUNTIME_BIN_DIR`, and `EEZ_TEST_PROOF_SIGNER_BIN`
unset. After `forge build --root contracts`, the test command was:

```bash
taskset -c 0,2 cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  settlement_policy::interval_preserves_execution_but_waits_below_all_thresholds \
  --exact --test-threads=1 --nocapture
```

Each run used separate `EEZ_TEST_LOG_DIR` and `EEZ_TEST_DATADIR_DIR` under the
operator checkout's ignored `artifacts/postmerge-ci-investigation/`, with the
existing task-owned Cargo target, `CARGO_BUILD_JOBS=4` and LLVM 22. These are
local source-based process checks, not the published release archive or an
independent follower replay. Local-only baseline executables, logs and datadirs
are preserved there; another checkout cannot retrieve them. The original driver
SHA-256 is `d544423060e31b8c85af1f4b911038360be7797c125f648d1282c7eb276b3787`,
Composer `f62fff3b09900331c8ca3b9c0d614161462269bd8ed29b28a0effe4a22a19018`,
and signer `fd03ff48d9c93548dc3c1f9c64d3cc1880ba1f6efa08d5ffca611bd0b4124f6a`.
The baseline failure log hashes to
`936d2eb7f34443fb16cd2272077c8ff017365ebfbcea7f0491ee827e35059210`.

### Artifact storage and remaining checks

The failed main job's diagnostic upload hit the account artifact quota. The
repository inventory on 2026-10-13 contained only one unexpired artifact,
3,501,006 bytes, already on three-day retention. No artifact was deleted: this
small retained failure record does not explain the account quota by itself.
The account billing-usage endpoint returned HTTP 403 for the current token;
account usage/budget review and a successful fresh upload remain unverified.
The downloaded lint and embedded failure logs are preserved locally with
SHA-256 `d95109c6e407d556a79bf6a98c9f4e5787fc0c484a7695db88f3049731e6558a`
and `860e6d6340e261a7b70243086ad4778a51f0f53f1c562c225514a151496f9d4d`.

Both updated lockfiles pass `cargo audit` with zero vulnerabilities, retaining
the existing nine allowed warnings in the root and six in the vendored workspace.
`cargo deny check advisories bans sources` passes. Composer, RPC and prover-client
library tests pass 353 tests, with `profile_blocked_backlog_discovery` excluded
as its existing manual timing profile. Signer binary units pass 338 tests, with
the existing `inventory_native_proving_fixtures` utility and
`cold_span_feasibility` measurement explicitly excluded.

The final source-based `settlement_policy::interval_` selection passed both tests
with the patched lockfile and the stronger accepted-decision assertion: two
passed, none ignored, 135 unrelated tests filtered out, 270.31 seconds including
fixture compilation. This covers the no-posting assertions plus actual interval
settlement and independent follower safe-prefix convergence. It does not cover
the complete cross-chain/Kurtosis matrix. The local `final/` evidence directory
retains the tested binaries and log, including these SHA-256 identities:

| File | SHA-256 |
| --- | --- |
| `Cargo.lock` | `66bd0cdc1005f28790ca09a564edd4b4111a74d627139e14f50ac42600f1871c` |
| `node_e2e` | `37f4301df2340544353a1f405340ef42ec8d132863005eed415cfabe8d739d0d` |
| `eez-composer` | `bc80a04bc84ddcfe0f085b106623fab5d0f1fd1cea19557ddc7c905bddb32f46` |
| `eez-follower` | `94399d69836565832e86f4a57abaa487bfa68e4e895b9397f58c323d02ed2a60` |
| `eez-proof-signer` | `c168d18a7c5292a324a9b8fb172b5efc92226a11ee00f263fd07a24e6bca9f61` |
| `test.log` | `765122e3244fa444ada22968e053ba5130527c724b102201eec1ab3d29c28f7a` |

Claude's final read-only review inspected the actual baseline diff and found no
blocking issue. Its outstanding documentation-status note is incorporated here;
the review did not execute validation. The local report is
`/tmp/rollupx-postmerge-claude-final.txt`, SHA-256
`04829fd6f972795e6b889ff4249eb2ae2bbdb7f47118beae378b0dfaeebe197f`.
Formatting passes. At recording, the production-selection build, final Clippy
recheck and fresh hosted release matrix had not completed; later results belong
in the follow-up PR's validation record. The two earlier intermittent failures
recorded above remain separate investigations;
this correction does not establish their root causes or complete the licensing,
disclosure, organization, fork, package or publication gates.
