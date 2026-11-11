# Contributing

Start with the [README](README.md), then read [AGENTS.md](AGENTS.md) for the
repository's architecture, invariants, toolchain, and checks for each changed
area. Review this fork and its pinned `eez-core-protocol` revision; upstream
`main` may implement a different contract.

## Issues and triage

Search existing issues, then use the bug or task template and follow
[Writing issues](docs/issue-guidelines.md) for structure, context, and updates.
Templates set the type; maintainers correct it (Bug, Feature, or Task) during
triage. Use labels only for additional context. Describe the problem,
RollupX impact, and relevant evidence; completion criteria can wait in Backlog.
The [PM repo](https://github.com/eez-association/eez-rollupx-pm) and shared board are
private to the organization. External contributors open issues here; maintainers
handle internal coordination. During triage, maintainers manually add issues we
track to the board.

Maintainers follow the [shared workflow and Priority/Effort rules](https://github.com/eez-association/eez-rollupx-pm/blob/main/CONTRIBUTING.md#organize-work).
Maintainers set the shared issue fields Priority after triage and Effort before
Ready. The [Project](https://github.com/orgs/eez-association/projects/1) displays these
same values; do not create duplicate Project fields. Ready also requires a
human decision, an assignee, and completion criteria. Reporters may leave estimates
unset; AI suggestions need a reason and human confirmation before setting values.

## Backports

For any backport review or implementation, follow the
[backport procedure](docs/runbooks/backport-review.md).

## Development

Initialize the pinned contracts before working on source or running the CI
tooling suite:

```bash
git submodule update --init --recursive eez-core-protocol
```

Use Rust 1.95.0, Foundry 1.7.1, and the native dependencies documented in
[AGENTS.md](AGENTS.md#build-and-validate). Keep Cargo dependencies locked.
The Python configuration tests use Docker Compose 5.5.1, also pinned in CI;
they render configuration without a Docker daemon.
Run focused checks while iterating and the applicable checks from that guide
before submitting a PR. Signer changes also follow the
[signer contributor guide](crates/eez-proof-signer/CONTRIBUTING.md).

Explain the problem, resulting behavior, and validation in the PR. Include
unrun checks and limitations. Significant behavior changes need a
[proposal](docs/proposals/TEMPLATE.md) with explicit acceptance criteria.
Update the owning specifications and operator instructions when behavior changes.
Keep refactoring and dependency upgrades within the requested scope.

CI uses GitHub-hosted runners. Fork and Dependabot runs validate locally without
publishing container images; runtime artifacts and every test lane remain
required. CI results do not establish that a live deployment was upgraded.

Keep operator keys, authenticated URLs, private infrastructure details, and
bulk logs out of commits and issues. Known local test keys are public fixtures,
not funded accounts for a live network. The pinned Gitleaks scanner checks Git
history in CI; changes to its exact-value exceptions require review.
Report vulnerabilities through [SECURITY.md](SECURITY.md).

This repository declares [MIT](LICENSE) for its own code. Preserve third-party
license notices and attribution; dependencies and the protocol submodule retain
their own terms. Submit only work you have the right to contribute under the
applicable license.
