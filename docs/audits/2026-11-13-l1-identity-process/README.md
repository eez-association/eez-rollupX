# L1 identity process qualification

Status: bounded process matrix passed; regression/review closeout underway for [#145](https://github.com/eez-association/eez-rollupX/issues/145).
This extends [#162's component assessment](../2026-11-12-l1-identity/README.md)
with production entrypoint tests. Historical component results remain historical;
they are not process evidence for this change.

## Baselines and contract

The clean dedicated worktree started at association main
`0d46d64891854357865471770d5063ceb8bb3a05`, with protocol gitlink
`84a5b5207880f192a6a5896cbfe0bac5249db47f`, initialized recursively before inspection
or builds. The branch is `codex/issue145-e2e-identity`. Origin is a local source
path; GitHub operations explicitly address `eez-association/eez-rollupX`.

The frozen N11 source remains rollup0
`1b66d9bda675e6f8e83f514153a97136909acc06`, protocol
`143d87e62cade98bfc122d612638dcaaf1c85633`; the frozen target remains
`31e148f50dd79730b17d113ac78795eb377eb5c5`, protocol
`84a5b5207880f192a6a5896cbfe0bac5249db47f`. This does not restart that audit.
Its original review-input provenance remains independently unchecked.

The accepted [offline-production profile](../../proposals/l1-origin-and-offline-proving.md),
[history contract](../../../contracts/L1_HISTORY_SPEC.md),
[portable identity](../../../crates/eez-control-rpc/VALIDATION_IDENTITY.md),
[RPC failure/ownership contract](../../../crates/eez-control-rpc/SPEC.md),
[signer profile](../../../crates/eez-proof-signer/SPEC.md) and
[pinned Core settlement model](../../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md#settlement-model)
define the expectations. Current portable identity is validation version **9**;
some AGENTS wording still describes version 8. This change does not alter the
identity, signing domain 7, network profile, Core contracts/pin, dependencies,
wire/storage formats or production behavior.

Three different commitments must remain distinct: embedded L1 block zero; the
L2 genesis-bound initial L1 reference, which may be nonzero; and the portable /
deployed profile. Composer compares signer capabilities before L2 launch.
Followers do not perform that handshake. Chain readiness can precede exact
reference authentication. Missing a direct RPC-genesis comparison alone does
not establish a defect or authorize a new genesis policy.

## Process tests and expected boundaries

The new [l1_identity module](../../../crates/eez-node-e2e/tests/l1_identity.rs)
is in the existing single `node_e2e` target and cross-chain CI inventory. It uses
the existing source harness to build and launch real `eez-composer`,
`eez-follower` and `eez-proof-signer` entrypoints. No duplicate runtime binary,
new dependency or alternate execution/validation mode is introduced.

`A` is the configured private L1, `C` changes its chain ID, and `G` changes its
genesis header and an account allocation while retaining the chain ID and
compatible header rules. The final seven-test matrix passed every boundary listed below; the execution
history distinguishes the earlier failed test iterations.

| Configuration | Required observation |
| --- | --- |
| Matching local profile and canonical source | Composer signer agreement; follower authenticated catch-up, beyond mere RPC binding |
| Canonical C, local A | Exact `EEZ_L1_RPC_URL serves chain …, expected …` startup error in each role |
| Canonical G, local A, initial reference A/0 | Readiness followed by `L1 genesis reference header` rejection in each role |
| Local/configured C, initial reference A | Composer `L1 reference/profile chain mismatch`; follower explicit configured-ID error |
| Embedded G, canonical A, signer A | Capability identity disagreement before sequencing |
| Existing A database, configured G | Reth stored-genesis/chainspec error before signer agreement; original config reopens |
| Embedded G, canonical A, signer/deployed key G | Real executor reaches the canonical proof-anchor hash comparison and rejects before signing; a missing height alone cannot pass |
| Nonzero finalized A/n, matching separate embedded provider | Engine payload replay, quoting/proving, final local execution, relay dispatch, canonical settlement and independent follower replay |
| Follower local G, canonical nonzero A/n, compatible header rules | Qualify replay while documenting that this genesis difference is unobserved by the consumed header rules; do not claim matching deployed-profile authentication |
| Wrong nonzero reference, independently matching signer document | Composer handshake succeeds, then both roles reject `finalized genesis reference header`; no signature |
| Canonical RPC chain-ID drift after startup | Specific posting-quote error; owned work retained, no signature or relay dispatch |
| Third target RPC chain-ID drift | Repriced attempt followed by `target_changed` exclusion on that same attempt across two distinct targets; no relay dispatch or canonical completion |
| Drift restored, without restart/resubmission | Same admitted source completes; inconclusive attempts do not charge its user attempt counter |

Startup cases cover the private Ethereum Osaka, Ethereum Prague and Gnosis
Prague fixtures. The full paired-provider / nonzero-reference path uses private
Ethereum Osaka. Public Ethereum mainnet, public Gnosis/Chiado, other network
presets and every Cartesian provider combination are not implied by these cells.

The canonical Anvil in the startup negative is a real separate execution process.
The deeper paired case uses independent real Reth databases. The matching mirror
imports actual canonical payloads through Engine API and requires `VALID`; it is
a development consensus feed, not a production PoS/PBS simulation. The finality
control marks an actually executed local head finalized through Engine API and
compares the acknowledged number/hash with canonical RPC.

The mismatched embedded-state fixture provisions G at the real signer and uses
Core's existing owner-only `updateVerificationKey` on its owned local canonical
chain, so a key mismatch cannot hide the earlier execution-history fence. This
is a disposable test deployment, not an upgrade or migration procedure.

The existing builder stub captures/forwards real bundles and serves native blob
availability through the beacon fixture. It is not a real builder ordering/refund
exercise. `ChainIdProxy` is explicitly a fault proxy: it changes only
`eth_chainId`, forwarding other reads to the actual node. Both drift legs are
endpoint-identity tests, not independent alternate-state chains. `TargetChanged`
checks the complete quote/target binding and does not uniquely identify its
chain-ID term. Actual posting lifecycle `dispatched` records are used; a bundle
handed to a background observer is not a relay dispatch.

A request rejected by an earlier proof-anchor fence cannot supply evidence for
a later final-simulation rejection. The matching control must traverse that later
path. Offline attestations remain conditional and never establish canonical
settlement or queue completion.

## Execution evidence

The final matrix command was:

```text
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- l1_identity:: --test-threads=1 --nocapture
```

It ran **7 passed, 0 failed, 0 ignored, 214 filtered out**. This is real process
execution, including native DA, offline production signing, final local bundle
execution, canonical settlement and two independent follower databases in the
nonzero-reference control. Both followers use this Rust implementation; no
external-client or standalone full-chain evidence-verifier replay is claimed.

| Check / iteration | Result |
| --- | --- |
| Initial source test compile | Passed, normal optimized dev profile; no process coverage from compilation alone |
| First process matrix | 5 passed, 2 failed, 0 ignored, 214 filtered; wrong fixture genesis failed at capability agreement; the original mismatch probe had no pending ordinary transaction and timed out without selecting a proof |
| Corrected process matrix | 4 passed, 3 failed, 0 ignored, 214 filtered; the critical paired/replay/recovery cases passed, but matching-follower controls waited for a filtered INFO message |
| Final process matrix | 7 passed, 0 failed, 0 ignored, 214 filtered; the matching control observes the L1 watcher, which the production follower starts only after reference authentication and boot catch-up |
| Workspace Clippy | Initial check caught a test constructor missing the new endpoint field; corrected and final `--locked --workspace --all-targets -- -D warnings` checks passed |
| Current package regressions | Composer 454 passed / 1 ignored; L1 216 passed; shared startup 34 passed; testkit 21 passed; zero failures or filtered tests |
| Formatting / documentation | Workspace format and diff checks passed; local links resolve at the initialized pin |

The ignored Composer case is
`held_pool::tests::profile_blocked_backlog_discovery`, an explicitly manual timing
profile. No performance check was requested. The source/fixture corrections above
are test fixes; no production defect was demonstrated and no runtime behavior
change was selected. They do not establish an untouched-baseline-versus-runtime-fix
comparison. Production code remains the requested `0d46d648` baseline.

First failures, logs/datadirs and executable identities are retained separately.
Early builds ran while the new tests were being edited and are not an exact
commit snapshot claim. The final code-source digests, executable/log digests,
commands and limits are in [evidence.json](evidence.json). Paths under `artifacts/`
are local retained evidence, not published downloadable artifacts. The five
selected existing source-based process regressions are running separately and
serially after the matrix; their results will be recorded before closeout.

One queued regression invocation was cancelled before any role process launched
to keep process execution serial and avoid a paused fixture's aged genesis. Its
empty signer log and cancellation record are retained; it supplies no coverage.

The source test build uses Rust 1.95.0, the normal optimized dev profile, two
build jobs, Foundry 1.7.1 and LLVM 22. Targets and retained logs/datadirs are owned
by this checkout. Prebuilt overrides are cleared; dependencies stay locked.

The full tooling rerun passed 607 tests with 9 explicit skips using checksum-pinned
Compose 5.5.1. The initial sandbox run had 3 failures and 102 errors, including
socket-denied tests and the missing new-module CI inventory entry, which was
corrected. That first log is retained. The 9 skips are opt-in Docker/Kurtosis /
archive probes and two Gitleaks checks; they are not executed coverage. Static
settlement configuration rendering passed for Chiado, Kurtosis and Ethereum.

No live network, production wallet, deployment, shared-service lifecycle,
performance benchmark, complete release-artifact lane or Kurtosis validation is
claimed. Local loopback execution does not establish deployment of this revision.

## Reviews

Ponytail implementation and review guidance was read at
`96608e078b2d5ea607c5c3b8b5a54d1c1fba6846` from
[the upstream skill](https://github.com/dietrichgebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail/SKILL.md)
and [review skill](https://github.com/dietrichgebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail-review/SKILL.md).

The preliminary adversarial review returned `modelUsage` containing only
`claude-fable-5-2`, with canonical model `claude-fable-5-2` and no fallback.
It found test defects, not a demonstrated production defect: the mirror selected
the old genesis, the positive control counted the wrong dispatch signal, and the
negative expected a later guard than production permits. The tests were corrected
to honour the last fixture override, inspect actual relay/registry progress and
assert the earlier canonical-anchor hash fence. It also requested stronger
ownership/attempt assertions, longer fault observation and accurate endpoint /
follower/finality descriptions. Final execution and review must verify the
corrected scope. This preliminary static review is not runtime approval.
