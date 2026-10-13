# Publication preflight without paid Actions

| Field | Value |
| --- | --- |
| Date and status | 2026-10-13 UTC; preparation complete for review, publication gates open |
| Scope | Private-to-public transfer preparation, dependency declarations, retained-text disclosure scan and the remaining CI failure; no transfer, visibility change or deployment |
| Source | `eduadiez/rollupX`; main `51d5cd792ee7d570d00d6a53ae4b5f1d168bf440`; inspected PR #86 head `a255ea83d4be0bc81a121089a57c257db91ae38e` |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized recursively and unchanged |
| Environment | Linux x86_64, Rust 1.95.0, Foundry 1.7.1, LLVM 22; source-based process tests and read-only GitHub API retrieval |
| Related records | [Earlier readiness investigation](../2026-10-11-public-release-readiness/README.md), [release procedure](../../runbooks/public-release.md), [hosted CI proposal](../../proposals/github-hosted-ci.md), [draft PR #86](https://github.com/eduadiez/rollupX/pull/86) |

The owner confirms the account can transfer the repository to `eez-association`,
but the organization cannot supply Actions minutes while it remains private.
Authority to publish inherited contributions under MIT **still needs
confirmation**. These are owner statements, not successful billing or membership
API checks; those endpoints returned HTTP 403 with the available token.
Destination-name availability, package permissions and team access remain to be
verified by an organization administrator.

The [release procedure](../../runbooks/public-release.md#route-without-paid-actions)
now permits organization CI validation after source publication when no private
allowance is available. Disclosure and licensing must be cleared first. This
does not waive checks required to merge fixes or promote deployment images.
No paid runner, budget change or Actions rerun was requested during this work.

## License inventory

The checked-in [Cargo inventory](cargo-license-inventory.csv) contains **1,269**
distinct package/version/source/license rows from three locked metadata graphs:

| Scope | Manifest | Resolved packages | Missing Cargo license declaration |
| --- | --- | ---: | ---: |
| `root` | `Cargo.toml` | 1,092 | 2 |
| `vendor` | `crates/vendor/reth/Cargo.toml` | 971 | 0 |
| `indexer` | `monitoring/indexer/Cargo.toml` | 398 | 0 |

Shared packages occupy one row with multiple scopes. These are resolved default
metadata graphs, including development, optional and platform-specific packages;
they are not an exhaustive license inventory of every entry in every lockfile.
The `runtime_normal_build_closure` column marks the **900** package/version pairs
in the Linux normal/build dependency tree of the five packages selected by
`Dockerfile.runtime`. There were no ambiguous name/version pairs in any input
graph. This column is not a binary SBOM: it does not prove which code survives
linking, enumerate examples' development dependencies or replace a review of
the exact release build. A `no` is not a claim that a package is never distributed.

| Component | Evidence at the inspected revision | Required follow-up |
| --- | --- | --- |
| First-party workspace | All 19 workspace members declare MIT; the root `LICENSE` exists | Owner confirmation that the grant covers inherited contributions remains open; metadata cannot establish that authority |
| `reth_gnosis 2.2.0` | Pin `9517ba3b3c139ed43d3e0814b60ec67b95e163d7`; no Cargo license declaration and no `LICENSE`, `LICENCE`, `COPYING` or `NOTICE` file in that git tree; reachable through production dependencies | Obtain an applicable grant for the pinned contribution from its rights holder; do not assume Reth's license covers this separate project or change the pin incidentally |
| `gnosis-primitives 0.2.9` | Pin `6ac3d3fd46213d59fc27bc36c4f273132a065811`; Cargo declaration absent, but root `LICENSE` is MIT, copyright Gnosis Chain | Preserve that source notice in the distribution; this is a metadata gap, unlike the unresolved `reth_gnosis` grant |
| Vendored Reth | Local patched crates declare `MIT OR Apache-2.0`; both texts and upstream attribution are retained in `crates/vendor/reth` | Preserve attribution and the applicable distribution notices |
| `gmp-mpfr-sys 1.7.1` | `LGPL-3.0+`; source includes LGPL/GPL texts and bundled native libraries; reached through `revm-precompile` in the runtime dependency tree | Review the actual linking and source/relinking distribution requirements before publishing binary packages |
| `imbl 7.0.2`, `imbl-sized-chunks 0.2.0` | `MPL-2.0+`; runtime dependency tree includes the Reth transaction-pool path | Include applicable notices/source with the distribution review |
| Four `test-fuzz` packages at 7.3.0 | `AGPL-3.0 WITH mif-exception`; `gnosis-primitives` brings `test-fuzz` into the normal dependency tree | Review the complete exception and actual usage; do not drop the exception or classify the dependency as development-only based on its name |
| `fastrlp 0.3.1` and `0.4.0` | `MPL-2.0`; present in resolved metadata, absent from the selected normal/build tree | Review distributions that include these packages separately |

The `test-fuzz` README explains its macros-and-inline-functions exception;
the [exception text](https://spdx.org/licenses/mif-exception.html) matters to the
review. This record reports declarations and dependency paths, not a conclusion
that all listed obligations have been satisfied. The repository's existing
`cargo deny check advisories bans sources` does not check license compliance.
No new blanket license allowlist or dependency upgrade was introduced.

### Protocol, tools and images

All **174** Solidity files inspected under the pinned protocol's `src`, `test`
and `script` directories carry MIT SPDX identifiers. The protocol has no root
license text. Its separate `trace-decoder` package declares ISC; the
[locked decoder inventory](trace-decoder-license-inventory.csv) records the
root plus nine dependencies, which declare MIT or 0BSD. These observations do
not extend a Solidity header's grant to unmarked non-Solidity files.

Recursive source dependencies retain their own notices:

| Component | Pin / source | Declared license evidence |
| --- | --- | --- |
| OpenZeppelin contracts | `5fd1781b1454fd1ef8e722282f86f9293cacf256` | MIT root license; bundled Compound code has its own BSD-style notice |
| OpenZeppelin upgradeable contracts | `7bf4727aacdbfaa0f36cbd664654d0c9e1dc52bf` | MIT root license; nested dependencies retain separate licenses |
| `erc4626-tests` | `232ff9ba8194e406967f52ecc5cb52ed764209e9` | AGPL-3.0 license text |
| `halmos-cheatcodes` | `7328abe100445fc53885c21d0e713b95293cf14c` | AGPL-3.0 license text |
| Nested Forge Standard Library | `3b20d60d14b343ee4f908cb8079495c07f5e8981` | MIT and Apache-2.0 texts |
| Protocol's bundled `lib/forge-std` | Contained in protocol pin | MIT and Apache-2.0 texts |

`Dockerfile.runtime` copies compiled binaries into Debian-based node/signer
images; it does not currently assemble a Rust dependency notice bundle.
`testing/kurtosis/Dockerfile.deploy` copies the protocol tree and integration
contracts and installs Foundry 1.7.1. Its recursive test libraries are therefore
relevant even when they are not linked into the node. Base OS packages,
Foundry/Solidity tools, CI payloads, the optional indexer image and the modified
Blockscout image need a distribution-specific notice/source review. The two
CSV files are review inputs, not that notice bundle. Optional upstream explorer
images and OpenZeppelin's separate Node development lockfiles were not resolved
into the Cargo/decoder inventories.

## Disclosure checks

The all-ref mirror was refreshed with branch, tag and PR-head refspecs. Its
inventory contained 64 branch refs and 77 PR refs, no tags, and **812** scanned
commits. The scan includes merge-parent diffs (`--all -m`), ignores inline
allow directives and uses the committed exact-value fixture exceptions in
`.gitleaks.toml`. It found no unallowlisted credential matches. Mirrored refs
can include retained local refs; this is not proof that no other GitHub content
or unreachable object can be accessed.

Read-only API retrieval covered:

| Surface | Retrieved and scanned | Result / limit |
| --- | --- | --- |
| Issues and PR bodies | 85 records: 73 PRs and 12 issues | No secret-scanner matches |
| Issue/PR conversation comments | 13 comments | No secret-scanner matches |
| Inline review comments | 0 returned | Empty inventory, not omitted pages |
| PR reviews | All 73 PRs queried; one review returned | No secret-scanner matches |
| Workflow logs | 337 workflow runs, 372 attempts; 370 archives retrieved, 5,374 entries, 561,594,845 uncompressed bytes | 451 scanner matches classified below; two archives unavailable |
| Actions artifacts | API returned zero retained artifacts | Expired or missing diagnostic artifacts could not be reviewed |

The raw workflow scan exited with findings. Classification found **448 cache
identifiers**, **two token contract addresses** and **one public relay fixture
key**, with no confirmed operator credential among them. The relay value was
compared exactly with `DEFAULT_MEV_SECRET_KEY` at the pinned upstream
[`ethereum-package` constants](https://github.com/ethpandaops/ethereum-package/blob/199620b24ac979c676010c5a68b2893c2bce4f1f/src/package_io/constants.star).
The repository scanner configuration was not relaxed to suppress log findings.
Attempts 1 of runs `33510372359` and `33510373780` returned HTTP 404; no content
was available to scan.

The text review found an operator hostname in PR #60 and in three current
documents. This preparation replaces it with chain identity or a relative
Grafana path in `deployments/chiado-906969/README.md`,
`docs/proposals/protocol-rc2-upgrade.md` and `monitoring/indexer/README.md`.
The chain IDs, historical results and dashboard route are preserved. **That
edit does not remove the hostname from history or the PR discussion.**

Thirty-one PR bodies link Claude coding sessions. Their linked transcripts
were not fetched or reviewed. No GitHub attachment URL was found by the body
URL extraction; that limited check does not inventory every attachment or
external link. Operator context in historical documents/logs, linked sessions,
and retained package layers/metadata still needs disclosure review. A clean
secret scan does not clear this broader publication gate. No history rewrite,
discussion edit, log deletion or package cleanup was performed.

## Remaining CI failure

The first attempt of [PR #86 CI](https://github.com/eduadiez/rollupX/actions/runs/38370914379)
passed **11 of 12 validation jobs**: lint, debug units, runtime producer, all
four embedded slices, three cross-chain slices and Kurtosis. Cross-chain slice
4/4 selected 23 tests: 22 passed and one failed. The aggregate CI gate failed
and promotion did not run. Later attempts 2 and 3 were blocked by the Actions
budget before executing tests; they are not additional test failures.

The failing test was
`dynamic_admission::ethereum_delayed_start_mixed_wave_after_small_post_replays`,
at `dynamic_admission.rs:285`: no observed prepared batch satisfied its
`accepted > 12` assertion. Earlier receipt, value, nonce and independent
follower convergence assertions passed; assertions after that line did not
execute. The retained console contains diagnostic tails, not the complete
admission history. Diagnostic artifact upload failed because of the account
storage quota, so the original complete admission trace remains unavailable.

Two **unchanged-source diagnostic runs** passed locally:

| CPU affinity | Selected / passed / ignored | Elapsed test time, including fixture builds | Observed accepted batches |
| --- | --- | --- | --- |
| `0,2` | 1 / 1 / 0; 136 unrelated tests filtered | 206.04 s | `1,18,1,1,1,23,84` |
| `0` | 1 / 1 / 0; 136 unrelated tests filtered | 320.25 s | `1,18,1,35,74` |

Both retained semantic assertions and independent follower safe-prefix replay.
CPU affinity does not reproduce GitHub hardware, memory, disk or the release
artifact profile. The source driver builds fixture binaries, so these elapsed
times are not throughput benchmarks. These local passes do not resolve the CI
failure or justify weakening its assertion. No production or test behavior was
changed by this follow-up; PR #86 remains draft pending the failure investigation
and a successful hosted matrix.

### Executed checks and reproduction

All commands ran from the isolated PR checkout. Metadata used the existing
lockfiles; the indexer query fetched missing pinned packages after an offline
query failed, without changing its lockfile. Equivalent commands to regenerate
the inventory inputs (the retained originals are under the ignored evidence
directory listed below) are:

```bash
cargo +1.95.0 metadata --locked --offline --format-version 1 > root-metadata.json
cargo +1.95.0 metadata --locked --offline --format-version 1 \
  --manifest-path crates/vendor/reth/Cargo.toml > vendor-metadata.json
cargo +1.95.0 metadata --locked --format-version 1 \
  --manifest-path monitoring/indexer/Cargo.toml > indexer-metadata.json
cargo +1.95.0 tree --locked --offline \
  -p eez-node -p eez-follower -p eez-proof-signer -p eez-protocol -p eez-l1 \
  --edges normal,build --target x86_64-unknown-linux-gnu \
  --prefix none --format '{p}' > runtime-dependency-tree.txt
```

The two process invocations used the task-owned Cargo target and distinct log
and datadir directories. No live Chiado or shared Kurtosis service was used:

```bash
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
export CARGO_BUILD_JOBS=4 LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
export PATH="/usr/lib/llvm-22/bin:$PATH"
forge build --root contracts
taskset -c 0,2 cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  dynamic_admission::ethereum_delayed_start_mixed_wave_after_small_post_replays \
  --exact --test-threads=1 --nocapture
# The second diagnostic used affinity 0 and fresh log/datadir directories.
```

The test driver and fixture binaries retained after the second invocation have
the same hashes as the final interval-test validation in the earlier audit:

| Executable | SHA-256 |
| --- | --- |
| `node_e2e` | `37f4301df2340544353a1f405340ef42ec8d132863005eed415cfabe8d739d0d` |
| Composer | `bc80a04bc84ddcfe0f085b106623fab5d0f1fd1cea19557ddc7c905bddb32f46` |
| Follower | `94399d69836565832e86f4a57abaa487bfa68e4e895b9397f58c323d02ed2a60` |
| Signer | `c168d18a7c5292a324a9b8fb172b5efc92226a11ee00f263fd07a24e6bca9f61` |

PR #86's earlier final workspace Clippy and production-selection **dev-profile**
build also completed successfully after the prior audit was written. Those
results do not establish a release artifact build at a later documentation head.
This follow-up changes only documentation and checked-in review inventories;
it does not rerun the complete Rust, Solidity or Kurtosis matrix.

Documentation validation matched every Cargo inventory row to the three input
graphs, checked the ten decoder rows, resolved 45 local Markdown paths and
syntax-checked nine shell examples. Whitespace and staged Gitleaks checks passed.
A sanitized packet is ready for the requested Claude review; that follow-up
review has not executed and is not counted as validation.

## Evidence inventory

CSV inventories are committed beside this record. Raw metadata, mirrored refs,
API bodies, workflow archives, redacted scan reports and classifications remain
in the isolated checkout's ignored `artifacts/public-release-followup/`.
Process logs/datadirs remain in the operator checkout's ignored
`artifacts/public-release-followup/mixed-{baseline,one-cpu}/`; console logs are
also retained locally. These ignored files are **local-only**, unavailable from
a fresh clone. GitHub run links may expire or lose log access; hashes do not
make unavailable evidence retrievable.

| Evidence | SHA-256 |
| --- | --- |
| Root `Cargo.lock` | `66bd0cdc1005f28790ca09a564edd4b4111a74d627139e14f50ac42600f1871c` |
| Vendored `Cargo.lock` | `630ce38ce0a9d5779cbedddc11d14b2e32f27c348ad7bbaec7c1d1b392844357` |
| Indexer `Cargo.lock` | `3613a60b928ca28b999578569c7709d375dae63487364dc8ee7f2d770315b2ea` |
| `cargo-license-inventory.csv` | `1064aeff2d9e27855e62cc2cb2a90238de51b3b5418b785ccdad0afe6426f85d` |
| Mirrored ref inventory | `920e18bf5431b071a5cbd57d3fb52dd40f6c59fbe49ec7dcd5d2942c43389b54` |
| Workflow archive inventory | `58d36527b3faf76a62483972a1314c72c5cb1b68959ffd36c18a398544a360f9` |
| Redacted workflow findings | `6bc5dee398bd4a52b65d52fd1a6a5b6ac5d202e6e576cb3583b2b455df1366c8` |
| Workflow finding classification | `9ed931dcc1d5a04887245b5bbd0c12d88a339a13751d6ba8e0b05c36ba10d05e` |
| Original failing job console | `7ea130d591aa6897724de19565b14aed95f54990a3a805309bfaeb1d921f5211` |
| Two-CPU local console | `7181ed6db05acdb18722c17e5e5a427ff0070da024107b485e673135df536b3d` |
| One-CPU local console | `569d99c7b9431db0b6c86a954d0279ceeb82d336fe4752c73b6d7c691369c0ad` |

## Open acceptance criteria

| Gate | Current result | Next action |
| --- | --- | --- |
| Publication authority | Owner says confirmation is still needed | Record a rights-holder confirmation covering inherited contributions; separately resolve the pinned `reth_gnosis` grant |
| Distributed licenses | Inventory available; notice bundle and exact-image review incomplete | Review selected licenses, exceptions, native linking, bundled source and notices for each intended distribution |
| Disclosure | Automated scans and classification complete for retrieved text; broader review open | Resolve historical operator context, PR #60, 31 session links, unavailable content and package layers; approve any cleanup scope separately |
| CI correctness | One original admission test failure remains unexplained | Capture complete admission evidence on a controlled reproduction; retain semantic and packing coverage; pass the hosted matrix before merging the fix |
| Destination administration | Transfer capability confirmed by owner; no private CI minutes | Verify name, teams, rules, Apps, secrets/variables by name and package creation/access; remove this repo's homelab runner access before publication |
| Public operation | Not exercised | After disclosure/license clearance, publish source, validate organization and external-fork CI, verify public image digests and only then change image defaults |

No operator configuration, wallet, running service, deployed contract or protocol
pin was changed. Repository visibility and ownership remain unchanged.
