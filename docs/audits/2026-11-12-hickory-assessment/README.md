# Hickory 0.26.3 assessment — issue #139

Recommend a narrowly scoped update of Hickory net/proto/resolver to **0.26.3**,
subject to implementation approval and the runtime checks below. This PR is an
assessment, not that update. The ordinary UDP response-handling and resolver
changes are relevant to supported DNS discovery. Fresh scans find no Hickory
vulnerability in the current shipping lockfile; do not describe this as an
established exploitable vulnerability or an emergency security release.

## Scope and identities

| Field | Value |
| --- | --- |
| Date/status | 2026-11-12 UTC; investigation complete, implementation and runtime acceptance outstanding |
| Tracking | [#139](https://github.com/eez-association/eez-rollupX/issues/139), original handoff N4; [#133](https://github.com/eez-association/eez-rollupX/issues/133) remains the separate broader review |
| Fresh target / PR base | `eez-association/eez-rollupX` at `20243f70839ec69df9312731e960cd311e060050` |
| Target protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`; unchanged, uninitialized locally; no Solidity/protocol execution inspected or tested |
| Source PR | [rollup0 #241](https://github.com/eez-association/eez-rollup0/pull/241): base `2caf6cec5b5861a2dd8f37dd28379f9602d4ab61`, head `006f625aeb83e8dc530655db77b6203bc9bf964e`, merge `1b66d9bda675e6f8e83f514153a97136909acc06` |
| Reth | v2.7.0, `3d592ece6de8c4559987416a544fc215fd6d6921`; cached tracked source clean (only Cargo's untracked `.cargo-ok`) |
| Environment | Linux x86_64; Rust/Cargo 1.95.0; cargo-audit 0.22.2; cargo-deny 0.20.2 |
| Candidate | Archive of fresh target in `/tmp/issue139-candidate`; only root lockfile's three Hickory versions/checksums changed; [exact scratch patch](candidate.patch), never applied to the assessment branch |

The worktree initially had clean status at `bad8d7eef7e16ac872f354d31af257f8646b2fac`
with an unrelated `origin`. The assessment branch was created from the explicit
fork URL's fetched main. That remote configuration was preserved. No services,
wallets, other worktrees, issue metadata or existing CI runs were changed.
The PR head is the commit carrying this record; review it against the PR base above.

The frozen comparison remains:

| Observation | Revision | net / proto / resolver |
| --- | --- | --- |
| Frozen source | `1b66d9bda675e6f8e83f514153a97136909acc06`, protocol `143d87e62cade98bfc122d612638dcaaf1c85633` | 0.26.3 / 0.26.3 / 0.26.3; also proto/resolver 0.25.2 |
| Frozen target | `31e148f50dd79730b17d113ac78795eb377eb5c5`, target protocol above | 0.26.1 / 0.26.1 / 0.26.1 |
| Earlier issue observation | `716a8a3bad4415ade7e9a6bc3af4a1b35419b232` | 0.26.1 / 0.26.2 / 0.26.1, as recorded in #139 |
| Fresh shipping lock | fresh target above | 0.26.1 / 0.26.2 / 0.26.1 |
| Fresh vendored-Reth test lock | same target, `crates/vendor/reth/Cargo.lock` | 0.26.1 / 0.26.1 / 0.26.1 |

Frozen lockfiles were read at their exact revisions. Original review provenance,
its full inventory and merge-base ancestry were not independently re-audited.
The [scoped manifest](manifest.json) and [rendered handoff](handoff.md) preserve N4,
the frozen revisions and #139's existing completion criteria; they do not restart
or replace the broader audit. The new manifest digest is
`054666c13c01671ec451a5541254cfd81e1d3ca6fce2ece97fa11e42678f72cc`.

## Dependency and runtime reachability

Locked Cargo tree/metadata show one Hickory family in the target. The direct path
is `reth-network → reth-dns-discovery → hickory-resolver → hickory-net/proto`.
Reth's [manifest](https://github.com/paradigmxyz/reth/blob/3d592ece6de8c4559987416a544fc215fd6d6921/crates/net/dns/Cargo.toml#L30)
enables Tokio; its workspace requirement is `hickory-resolver = "0.26.1"`, which
admits 0.26.3. The graph reaches Composer, follower and signer through shared
Reth/Gnosis dependencies. Dependency presence is not proof a role executes DNS.

The production-package tree (`-p eez-node -p eez-follower -p eez-proof-signer`,
normal/build edges, default shipping features) selects:

| Crate | Features |
| --- | --- |
| hickory-resolver | `default, serde, system-config, tokio` |
| hickory-net | `tokio` |
| hickory-proto | `access-control, serde, std` |

No DNSSEC, recursor, HTTPS, HTTP/3 or QUIC feature is selected. Reth's
[`DnsResolver::from_system_conf`](https://github.com/paradigmxyz/reth/blob/3d592ece6de8c4559987416a544fc215fd6d6921/crates/net/dns/src/resolver.rs#L83)
builds the system-configured Tokio resolver; `lookup_txt` calls `txt_lookup`,
then extracts EIP-1459 entries. This parses external DNS before authenticated
discovery-tree processing; subsequent tree authentication does not by itself
protect a DNS decoder from malformed messages.

Supported runtime paths differ:

- **Embedded public Ethereum L1:** [network construction](../../../crates/eez-node/src/l1_embedded.rs#L295)
  preserves `NetworkArgs::default()` discovery flags. Reth defaults DNS discovery
  on, constructs the resolver in
  [network discovery](https://github.com/paradigmxyz/reth/blob/3d592ece6de8c4559987416a544fc215fd6d6921/crates/net/network/src/discovery.rs#L250),
  and [adds known public DNS trees](https://github.com/paradigmxyz/reth/blob/3d592ece6de8c4559987416a544fc215fd6d6921/crates/net/network/src/config.rs#L722).
  Locked Alloy chains supplies trees for mainnet, Sepolia and Hoodi, the public
  networks selected by the embedded L1 configuration. This is a concrete
  source-level path to DNS; no live packet capture was performed.
- **L2 Composer/follower:** the shared [node components](../../../crates/eez-node-common/src/node.rs#L50)
  use `EthereumNetworkBuilder`. CLI configuration controls discovery. The
  supplied Ethereum/Chiado/Kurtosis node Compose commands disable L2 discovery;
  that flag does not disable the separately constructed embedded L1 network.
  A private chain without a known DNS tree need not issue bootstrap DNS queries.
- **Signer:** [startup](../../../crates/eez-proof-signer/src/main.rs#L1) loads local
  profiles and serves the prover API. It does not start a Reth peer network or
  have chain RPC clients. No signer execution path to Hickory was found; graph
  inclusion is not a finding of signer DNS exposure.

The existing `deny.toml` and `.cargo/audit.toml` comments saying DNS discovery is
not enabled are too broad for the supported public L1 path. Their Hickory
exceptions are also unnecessary for the versions in either current lockfile:
RUSTSEC-2026-0118 concerns pre-0.26 proto; 0119 is fixed in proto 0.26.1; the moved
net issue 0120 is fixed in net 0.26.1. Correct/remove those two stale exceptions
as part of the approved update, not by copying rollup0's 0.25.2 exceptions.

## Compatibility and behavior review

All six inspected published crate archives matched their lockfile checksums;
every packaged file matched the extracted source used for review. Upstream VCS
revisions are `f09321075b1f97902b7bc4ca4ffda7816fcf2971` (0.26.1),
`48323ef64b5a868734efdc584f089ae390ff436d` (0.26.2), and
`bd37cafbdd86112a6da32878f8c9bab22e14377e` (0.26.3). All have Rust minimum 1.88,
below this fork's 1.95.0. Source differences, not patch numbering alone, informed
this assessment. The [0.26.3 release notes](https://github.com/hickory-dns/hickory-dns/releases/tag/v0.26.3)
also identify fixes to 0.26.2 regressions.

| Area | Difference and significance |
| --- | --- |
| UDP client | [0.26.3 receive loop](https://github.com/hickory-dns/hickory-dns/blob/bd37cafbdd86112a6da32878f8c9bab22e14377e/crates/net/src/udp/udp_client_stream.rs#L216) checks source and message ID before fallible DNS parsing, ignores short/wrong-ID packets, requires equal question counts, and waits for a matching response under outer timeouts instead of a three-packet limit. Relevant to ordinary discovery; supports updating, but no exploit or load test ran here. |
| Resolver | [Unix configuration](https://github.com/hickory-dns/hickory-dns/blob/bd37cafbdd86112a6da32878f8c9bab22e14377e/crates/resolver/src/system_conf/unix.rs#L34) becomes lenient and logs parse errors. Name-server error/retry handling and cache behavior also change. Test malformed configuration and negative/transient DNS responses; compile success is insufficient. |
| DNSSEC/recursion/encrypted DNS | Substantial fixes in net/resolver, including multiple signatures and insecure delegation; not selected in this shipping feature graph. Do not claim their protections are exercised here. |
| proto 0.26.2 → 0.26.3 | Source change is in DNSSEC summary handling/tests (skip RRSIG records when summarizing), outside selected features. The five proto advisories fixed in 0.26.2 are already addressed by the shipping lock, but not the older vendored test lock. |
| Cargo resolution | Initial offline update selected the three versions but also rewired 12 unrelated package dependency lists (including `syn` and Windows dependencies). Restoring those unrelated blocks left an accepted `--locked` candidate with only six changed version/checksum lines. Never ship the initial broad resolver output without review. |

The candidate retains Reth, Stateless, Gnosis, REVM, Alloy and all dependency
edges. Reth DNS compile checks test the caller API. Existing vendored shutdown
and shared-RPC-quota patches remain unchanged; their
[maintenance contract](../../../crates/vendor/reth/README.md) requires common
package alignment when updating the separate test lockfile too.

### Validation identity and mixed versions

Hickory decodes DNS and changes network discovery behavior. No route was found
from its decoder to accepted EVM execution, native transactions, blobs, witness
validation, public inputs or canonical chain-document JSON. The
[normative identity contract](../../../crates/eez-control-rpc/VALIDATION_IDENTITY.md#versioning-and-operations)
excludes lockfiles/build identity. Retain validation version **9**, capability
profile **2**, signing domain **7**, Core ABI, protocol pin and fixtures for this
candidate unless behavioral testing discovers an accepted-semantics change.
Do not reinstate the removed lockfile-digest guard. AGENTS.md's generic reference
to a local review guard is interpreted through that current normative contract;
it is not evidence that the historical digest check still exists.

An approved Hickory-only update is expected to retain wire/storage compatibility,
historical V0/V1 replay and the existing network profile. It needs new binaries
to take effect, no data migration, genesis change, key rotation or semantic
upgrade order. Keep Composer/follower/signer artifacts coordinated under the
existing runbook. Rollback to the current versions restores their DNS behavior;
unchanged identity does not prove that a binary has the fixes. Mixed-version
interoperability and historical replay have not been executed in this assessment.

## Fresh scans and checks

[results.json](results.json) records inputs, hashes, findings, warning categories
and check outcomes. RustSec database: `727b6ca6978e8f9037df3124cff51fec6a9a7941`,
1,471 advisories, fetched 2026-11-12. The `--no-fetch` JSON reports null database
revision/time; the revision is established separately from the fetched database,
not inferred from those null fields. Audit scans outside the repository do not
load its exception file. No exceptions were added or widened.

| Scan | Result |
| --- | --- |
| Current root, repository cargo-audit policy | Exit 0; zero unignored vulnerabilities; warnings remain |
| Current root, unfiltered cargo-audit | Exit 1; five vulnerabilities: crypto-bigint 0387, zstd 0391, zstd-safe 0398/0403, zstd-sys 0410; no Hickory finding |
| Final root candidate, unfiltered cargo-audit | Exit 1; same five vulnerabilities and warning categories; no Hickory finding |
| Current root and final candidate, cargo-deny advisories/bans/sources | Both exit 0 under existing policy; warnings remain, not a clean bill of health |
| Current vendored-Reth test lock, unfiltered cargo-audit | Exit 1; eleven vulnerabilities: same five, Hickory proto 0487–0491, and libz-sys 0418. This is a separate test-workspace finding, not the shipping root graph. |

The unrelated crypto/compression risks retain their existing acceptance limits;
this assessment neither renews them nor claims non-reachability. The libz-sys
test-workspace finding needs separate compatible remediation/triage and must
not be suppressed merely to make a Hickory update pass. Vulnerability scans are
package/version matching, not runtime exploitability tests.

Executed from the repository root unless noted:

```bash
cargo +1.95.0 tree --locked --offline -i hickory-proto
cargo +1.95.0 tree --locked --offline -p eez-node -p eez-follower \
  -p eez-proof-signer -e normal,build -f '{p} [{f}]'
cargo +1.95.0 metadata --locked --offline --format-version 1
cargo audit --json
cargo deny --format json check advisories bans sources
# From a scratch directory outside repository policy; repeat for candidate/vendor lock:
cargo audit --no-fetch --json --file /path/to/Cargo.lock
# In the archive copy only; inspect and narrow unrelated resolution changes:
cargo +1.95.0 update --offline --manifest-path /tmp/issue139-candidate/Cargo.toml \
  -p hickory-net -p hickory-proto -p hickory-resolver --precise 0.26.3
# Repeat these with the candidate manifest:
cargo +1.95.0 check --locked --offline -p reth-dns-discovery
cargo +1.95.0 test --locked --offline -p eez-control-rpc --lib \
  -- identity::tests --test-threads=1
```

Both identity selections execute two tests (portable vectors and namespace
sensitivity), with four filtered, zero ignored. They are narrow conformance
checks, not complete prover validation or proof of DNS equivalence.
The first attempted `cargo test --locked --offline -p reth-dns-discovery --lib --
resolver::tests --test-threads=1` failed on both baseline and initial candidate:
Cargo disallows testing that external non-workspace package's dev-dependencies.
Zero tests executed. The subsequent `cargo check` is compile-only, not a retry
pass for those tests. Initial sandbox GitHub reads also failed; explicit-network
reads succeeded. An unsupported `gh pr view` JSON field was replaced by the API.

No full Rust workspace build/clippy, signer suite, frozen native sessions, E2E,
Solidity, network/live tests, independent replay or performance measurements
ran. This is a documentation change, so those are implementation acceptance
work below, not claimed coverage. No timing result here measures runtime speed.
Raw logs and source diffs are local-only in `/tmp/issue139-evidence`; retained
summaries and the candidate patch are checked in. No deployed executable was used.

## Approval-dependent implementation and acceptance

1. Update only the three Hickory packages to 0.26.3 in root **and** vendored-Reth
   test lockfiles. Preserve dependency pins, features, source identities and
   unrelated dependency edges. Review Cargo's actual diff; do not assume a
   targeted update cannot alter unrelated resolution. The enclosed root patch
   is an experiment, not an approved production patch; the vendor candidate
   remains to be resolved/tested.
2. Remove the two stale Hickory exceptions from both audit policy files and
   correct their rationale. Do not import source libp2p/proto 0.25.2 exceptions,
   change network defaults, upgrade Reth or silently remediate unrelated findings.
3. Run fresh root/vendor scans with and without policy exceptions and compare
   locked shipping feature trees. Check the existing vendored patch diff and
   common package alignment. Resolve/report the independent libz-sys finding.
4. Build shipping targets with the exact `Dockerfile.runtime` feature selection;
   run required formatting/clippy and affected node/follower/RPC/signer tests.
   Run portable identity and canonical-JSON vectors, frozen native session
   transcripts, and affected `cross_chain` process/replay tests with nonzero
   selections. Preserve version 9 only if accepted semantics remain unchanged.
5. Exercise Reth's TXT lookup integration and Hickory UDP regressions in a
   suitable test workspace with reviewed locks: valid multi-string TXT,
   wrong-ID/short packets before a valid response, empty/mismatched questions,
   timeouts, negative replies and malformed `resolv.conf`. Include actual
   public-L1 DNS startup/discovery configuration and L2 discovery-disabled
   behavior using owned local resources. Existing `test_ignore_invalid_message`
   and resolver tests identify useful upstream cases; none ran here.

Deferring implementation leaves current shipping scans without a Hickory match,
but leaves the older UDP handling, resolver behavior, stale policy comments and
the test-workspace proto findings. The remaining uncertainty is behavioral
validation, not Cargo semver compatibility. #139's requested assessment and
implementation/check definition are supplied; implementation approval, merge,
issue closure and deployment remain separate actions.

## Review

Applied the [Ponytail skill](https://github.com/DietrichGebert/ponytail/blob/main/skills/ponytail/SKILL.md)
and [review procedure](https://github.com/DietrichGebert/ponytail/blob/main/skills/ponytail-review/SKILL.md)
to the assessment, connected runtime callers and evidence. The change adds only
the bounded assessment/evidence/handoff; no production fix or general audit
framework. Review verdict: suitable to publish as an assessment, with no blocking findings
in the documentation change. Nine local links resolve, the shell block passes
`bash -n`, JSON parses, the scoped handoff validates, and `git diff --check` passes.
No runtime safety or implementation approval is implied by that verdict.
