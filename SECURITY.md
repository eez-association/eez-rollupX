# Security policy

This is experimental rollup software. The proof signer produces an ECDSA
attestation after independent execution; it is not a succinct validity proof.
See the [README](README.md) and [signer specification](crates/eez-proof-signer/SPEC.md)
for the supported profile and trust assumptions. There is no stable release
support matrix yet; include the exact root and protocol commits in a report.

Use this repository's **Security → Advisories → Report a vulnerability** action
to send a private report when it is enabled. See
[GitHub's private reporting instructions](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/report-privately).
Do not include vulnerability details, exploit transactions, private keys, or
operator credentials in a public issue or PR. If the private reporting action is
unavailable, open an issue asking maintainers to enable it without disclosing
the vulnerability. Maintainers must enable and verify this channel as part of
the [public release procedure](docs/runbooks/public-release.md).

Include the affected component, source/protocol revisions, expected and observed
behavior, impact, and a minimal reproduction using disposable test accounts.
Sanitize logs and configuration. Do not test against other people's deployments
or move funds without their authorization. Coordinate disclosure and any fixes
in the private advisory.
