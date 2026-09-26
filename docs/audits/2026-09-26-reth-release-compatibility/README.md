# Reth release compatibility investigation

| Field | Value |
| --- | --- |
| Date and status | 2026-09-26 UTC; source/history investigation complete, migration not validated |
| Scope | Explain the Reth pin; assess official releases for standalone Ethereum mainnet sync, database handoff, and the embedded EEZ SDK |
| Source | `eduadiez/rollupX` at `85f3691cb9c7ccbf8c0b8700576733283e5af7fc` |
| Protocol | Initialized gitlink `9d13218a355e7d543ca8a9927eea2962600b716d`; unchanged |
| Executables | No candidate Reth/EEZ binaries or images built or executed |
| Environment | Local Git/Cargo source caches, an isolated Reth comparison clone, GitHub release/PR/comment APIs; no running node or operator database accessed |

## Subsequent validation

The [official v2.6.0 port validation](../2026-09-26-reth-v2.6-validation/README.md)
records the later fresh-network implementation, builds and local runtime tests.
The observations below remain the earlier source-only investigation; they do
not describe the newer candidate's executed validation or establish database
migration compatibility.

## Decision

An official release is a viable migration target, but a drop-in replacement for
the existing EEZ stack or its databases is **not established**. Upstream has
released the partial-persistence functionality behind the original branch.
Nevertheless, storage compatibility, coupled SDK dependencies, and relevant
unreleased fixes prevent approving a blind version replacement.

| Proposed use | Finding |
| --- | --- |
| Fresh standalone Ethereum mainnet node, permanently independent of EEZ storage | Can use an official Ethereum Reth release without EEZ's SDK dependencies. This investigation did not run that deployment. |
| Existing `fd59fd2` datadir opened by an official binary | Do not switch in place. There is a concrete checkpoint decoding obstacle in the older tags examined below; `v2.6.0` removes that obstacle but still needs a cold-copy migration test. |
| Official Reth pre-sync followed by moving its datadir into today's embedded EEZ | Reverse compatibility is unverified. An official release's ordinary downgrade guidance does not establish compatibility with this development branch. |
| Replace the Reth revision in EEZ's Cargo workspace | Requires a coordinated port and semantic/reorg/restart validation, including Stateless and Gnosis. Changing only the workspace Reth revisions is insufficient. |

Keeping the pin is not proof that the current engine is safe: the fork-choice
lookup defect described below is present in both the pin and `v2.6.0`.

## Why this commit was selected

[Cargo.toml](../../../Cargo.toml) identifies
`fd59fd2222b51239abebd9aa234f28b0b5f336eb` as the tip of
`mediocregopher/partial-persistence-clean` and attributes the choice to a
receipts/reorg truncation fix. The same comment and pin already occur in the
repository's initial commit `4eb798feb4d623193f8273df31333cc855959bb8`.

The [pinned upstream commit](https://github.com/paradigmxyz/reth/commit/fd59fd2222b51239abebd9aa234f28b0b5f336eb)
is a May 19 merge of upstream main into that development branch. Although its
Cargo package version is `2.2.0`, it is not the official `v2.2.0` release:

- Official `v2.2.0` is an ancestor, 265 commits behind the pin.
- Relative to its upstream-main parent
  `f12b36aba9351aedf447176d7ab87eb775dfb182`, the branch changes 42 files,
  with 4,461 insertions and 1,384 deletions. This is substantially more than a
  standalone receipt fix.
- The exact pin is not an ancestor of the official tags examined. Later upstream
  work landed through separate PRs, so ancestry alone does not establish which
  behavior is missing.

The contemporary discussion supports a temporary SDK branch dependency. In
[PR #24100](https://github.com/paradigmxyz/reth/pull/24100#issuecomment-4553321080),
`debjit-bw` says they intend to pin the work for a proof-of-concept SDK consumer
while awaiting upstream integration. The
[maintainer's preceding reply](https://github.com/paradigmxyz/reth/pull/24100#issuecomment-4553304712)
says the work is active but needs other optimizations. This is not a compatibility
or production-readiness guarantee. Original
[PR #23592](https://github.com/paradigmxyz/reth/pull/23592) and replacement #24100
were closed without merging as single PRs.

**Evidence limit:** the manifest's specific receipts-fix claim has not been
matched to a reproducer or a uniquely identified upstream patch. The branch
contains disk-reorg/trie-unwind changes, including
[`0644ee6f0`](https://github.com/paradigmxyz/reth/commit/0644ee6f002673c8389a5052f701650773b1bc85),
but that is not enough to identify the historical receipt failure. Do not equate
it with the later, unmerged receipt PR #26058 discussed below.

## What reached official releases

Git ancestry checks against exact release tags show this progression:

| Upstream change | Result as of investigation | First examined release containing it |
| --- | --- | --- |
| [#26447: Finish checkpoint](https://github.com/paradigmxyz/reth/pull/26447) | Merged July 20; decoding initially feature-gated | `v2.5.0` |
| [#26457: split-frontier overlays](https://github.com/paradigmxyz/reth/pull/26457) | Merged July 22 | `v2.5.0` |
| [#26533: partial persistence](https://github.com/paradigmxyz/reth/pull/26533) | Merged July 31; opt-in feature | `v2.5.0` |
| [#26543: partial trie unwinds](https://github.com/paradigmxyz/reth/pull/26543) | Merged August 4; includes crash/restart recovery work | `v2.5.0` |
| [#26800: remove feature gate](https://github.com/paradigmxyz/reth/pull/26800) | Merged August 25 | `v2.6.0` |
| [#27220: enable production defaults](https://github.com/paradigmxyz/reth/pull/27220) | Merged September 15 | `v2.6.0` |

GitHub's latest stable release endpoint returned
[`v2.6.0`, published September 17](https://github.com/paradigmxyz/reth/releases/tag/v2.6.0).
Its default persistence threshold/state-masking/buffer are `50/30/5` blocks;
the pin's are `2/0/0`. Therefore even a compiling port changes persistence timing
unless settings are chosen explicitly. The release instructs out-of-process DB
readers to disable masking with `--engine.num-state-masking-blocks 0`; that is
not a general database migration or downgrade procedure.

## Database compatibility obstacle

The pin adds `StageUnitCheckpoint::Finish(FinishCheckpoint)` with an
`Option<BlockNumber>` partial-state/trie frontier. Its
[`save_blocks` implementation](https://github.com/paradigmxyz/reth/blob/fd59fd2222b51239abebd9aa234f28b0b5f336eb/crates/storage/provider/src/providers/database/provider.rs#L836)
writes this variant on full persistence even when masking is disabled. Setting
masking to zero therefore does not remove the compatibility issue in an already
written checkpoint.

| Exact tag inspected | Reading the pin's Finish variant |
| --- | --- |
| `v2.2.0`, `v2.3.0`, `v2.4.0` | The enum has no Finish variant. These are not compatible decoders for this stored value. |
| `v2.5.0` | The variant exists, but its Compact decoder explicitly panics without the `partial-persistence` feature. This feature is absent from the binary's default features. |
| `v2.6.0` | Finish decoding is unconditional and has the same field/variant shape. This clears this specific obstacle; it does not prove compatibility of every table, static file, configuration, snapshot, or recovery path. |

Sources: checkpoint definitions at
[`v2.2.0`](https://github.com/paradigmxyz/reth/blob/88505c7fcbfdebfd3b56d88c86b62e950043c6c4/crates/stages/types/src/checkpoints.rs),
[`v2.5.0`](https://github.com/paradigmxyz/reth/blob/189c0df32617afc488e0f091dbface1bd72cceb4/crates/stages/types/src/checkpoints.rs#L473),
and [`v2.6.0`](https://github.com/paradigmxyz/reth/blob/73a3a00862a8f14f89e30da8de001456f18cfae0/crates/stages/types/src/checkpoints.rs#L442).
This is a static finding; no actual operator datadir was decoded. A downloaded
snapshot's format/provenance also needs checking independently of the downloader's
version. Patch tags not named here were not individually inspected.

## Relevant upstream defects and discussion

### Reorg acknowledgment can disagree with the actual canonical head

[#27429](https://github.com/paradigmxyz/reth/pull/27429) merged on September 25,
after `v2.6.0`. A fork-choice sequence from persisted block A to sibling B and
back to A can return `VALID` without restoring A while disk cleanup is pending.
The root cause is treating a header found on disk as proof of current canonical
membership. The original
[issue #23205 discussion](https://github.com/paradigmxyz/reth/issues/23205#issuecomment-4135561892)
describes the same timing window; its earlier closed status did not demonstrate
a released fix.

The unsafe fallback is textually present in `find_canonical_header` in both
[`fd59fd2`](https://github.com/paradigmxyz/reth/blob/fd59fd2222b51239abebd9aa234f28b0b5f336eb/crates/engine/tree/src/tree/mod.rs#L3180)
and [`v2.6.0`](https://github.com/paradigmxyz/reth/blob/73a3a00862a8f14f89e30da8de001456f18cfae0/crates/engine/tree/src/tree/mod.rs#L3363).
EEZ's [block committer](../../../crates/eez-driver/src/block_committer.rs)
`process_reorg_to` calls fork-choice update and updates its own tracked head on
`VALID`. This establishes a relevant integration path, not a locally reproduced
EEZ failure. The two new upstream regressions cover same-height and shorter
sibling branches with disk removal pending.

The PR has maintainer approvals. Its automated review also calls out changed
behavior under the two unwind flags: the repaired path may return `SYNCING`
transiently. EEZ enables both flags in
[node-common](../../../crates/eez-node-common/src/lib.rs), so port validation must
check that response as well as the false-`VALID` regression. Approval of the
upstream fix does not prove EEZ handles its changed responses correctly.

### Receipt truncation remains an unresolved validation question

[#26058](https://github.com/paradigmxyz/reth/pull/26058) proposes handling receipt
static files that remain ahead of the database after an interrupted unwind. It
was closed by the stale bot on August 26, **not merged**. An
[initial approval](https://github.com/paradigmxyz/reth/pull/26058#pullrequestreview-4627002330)
was followed by a
[changes-requested review](https://github.com/paradigmxyz/reth/pull/26058#pullrequestreview-4799218485)
questioning the failure scenario and existing startup recovery.

The proposed helper and regressions are absent from both the pin and the examined
release tags. In particular, both the pin and `v2.6.0` still return early from
`remove_state_above`/`take_state_above` when the database block range is empty,
without truncating an independently ahead receipt file in that branch. This
requires a targeted recovery/reorg test; it does not establish that the disputed
scenario occurs in EEZ, nor identify the manifest's original receipt fix.

### Additional validation gaps

- [#27441](https://github.com/paradigmxyz/reth/pull/27441), merged September 25,
  corrects upstream E2E reorg tests that had not actually persisted beyond their
  fork point. Our tests must assert that disk persistence has happened before
  claiming to exercise a disk reorg.
- [#27380](https://github.com/paradigmxyz/reth/pull/27380), merged September 22,
  addresses retention of sibling overlays during persistence stalls. This is
  relevant to repeated speculative branches but is not in `v2.6.0`.
- [#27397](https://github.com/paradigmxyz/reth/pull/27397), open and unmerged at
  inspection, proposes waiting for workers during shutdown following a reported
  `v2.6.0` shutdown crash. Graceful shutdown and crash recovery remain acceptance
  checks, not assumptions based on a successful initial sync.

## The SDK upgrade is coupled

[Cargo.lock](../../../Cargo.lock) contains 101 packages from the pin and another
101 from official `v2.2.0`. The latter enter through `gnosis-primitives`; they do
not mean EEZ's engine is already using the release. Duplicate sources alone are
not evidence of a bug.

EEZ, Stateless at `4fc3806bdd0e6b296c761ef4d4b260938365cf45`, and `reth_gnosis`
at `7e588a7b951a3517e7d98a4bc5d9fae1cf961eaf` use the development pin. Both external
manifests explicitly select it. Stateless's
[alignment commit](https://github.com/eez-association/stateless/commit/4fc3806bdd0e6b296c761ef4d4b260938365cf45)
and EEZ's [stateless validation adapter](../../../crates/eez-proof-signer/src/validate/stateless.rs)
show why matching Reth/REVM/Alloy types matters across the signer boundary.

Manifest requirements at the examined tags, not a claim about a newly resolved
EEZ lockfile:

| Reth | Rust minimum | REVM | Alloy consensus | alloy-evm | reth-primitives-traits |
| --- | --- | --- | --- | --- | --- |
| Current pin | 1.93 | 38.0.0 | 2.0.5 | 0.34.0 | 0.3.1 |
| `v2.2.0` | 1.93 | 38.0.0 | 2.0.4 | 0.34.0 | 0.3.1 |
| `v2.3.0` | 1.93 | 40.0.3 | 2.0.5 | 0.36.0 | 0.4.1 |
| `v2.4.0` | 1.95 | 41.0.0 | 2.1.1 | 0.37.0 | 0.5.0 |
| `v2.5.0` | 1.95 | 42.0.1 | 2.3.0 | 0.38.0 | 0.6.0 |
| `v2.6.0` | 1.95 | 43.0.2 | 2.4.2 | 0.39.0 | 0.7.1 |

EEZ builds with Rust 1.94.0 and explicitly fixes several Alloy crates at 2.0.5.
Official [reth_gnosis `v2.1.0`](https://github.com/gnosischain/reth_gnosis/releases/tag/v2.1.0)
targets Reth `v2.5.1`, not `v2.6.0`; simply choosing the latest version of each
project does not align the graph. EEZ also forces the legacy state-root task in
[embedded L1 wiring](../../../crates/eez-node/src/l1_embedded.rs), with a comment
documenting incorrect Gnosis roots under the pin's async path. Preserve or
revalidate that workaround in a coordinated port.

## Evidence and executed checks

All findings above are source/history evidence or explicitly attributed upstream
reports. No unit, process, semantic replay, performance, or live migration tests
ran. This was not a bug-fix implementation with a before/after reproduction.

From the repository root, the investigation inspected status, remotes, HEAD,
protocol gitlink/submodule status, manifests, lockfile sources, production callers,
and initial pin history. Read-only `gh api` requests inspected release records,
PR bodies/status, issue comments, code reviews, and closure timelines. An isolated
local Reth clone was used for `git show`, `git diff`, `git log`,
`git rev-list --left-right --count`, and `git merge-base --is-ancestor` checks.

Exact release objects used for those checks:

| Tag | Commit |
| --- | --- |
| `v2.2.0` | `88505c7fcbfdebfd3b56d88c86b62e950043c6c4` |
| `v2.3.0` | `9384bc53d8c0c77e59cac83fdaaf3b372c6d2216` |
| `v2.4.0` | `943af245c4d69c6c1df241df016c278ffb5d15df` |
| `v2.5.0` | `189c0df32617afc488e0f091dbface1bd72cceb4` |
| `v2.6.0` | `73a3a00862a8f14f89e30da8de001456f18cfae0` |

Successful ancestor checks returned 0; non-ancestor checks returned the expected
1. GitHub's comparison of `v2.6.0` with #27429's merge commit
`444e867155c2a7a4c028e479ca7201b0b6871579` returned diverged histories
(78 ahead, 1 behind); the release's actual source was inspected to establish
that the fix is absent. PR closure dates alone were not treated as merge evidence.

Public source/PR links above are the retrievable evidence. Raw API responses and
the comparison clone under ignored `artifacts/reth-release-review.ZVaHa1/` are
local-only working material, unavailable to another checkout. No operator logs
or credentials are included.

Representative executed queries (the Git commands require the local comparison
clone, or a replacement clone containing the exact objects listed above):

```bash
git -C artifacts/reth-release-review.ZVaHa1/reth rev-list --left-right --count v2.2.0...fd59fd2222b51239abebd9aa234f28b0b5f336eb
git -C artifacts/reth-release-review.ZVaHa1/reth diff --stat f12b36aba9351aedf447176d7ab87eb775dfb182 fd59fd2222b51239abebd9aa234f28b0b5f336eb
git -C artifacts/reth-release-review.ZVaHa1/reth show v2.5.0:crates/stages/types/src/checkpoints.rs
gh api repos/paradigmxyz/reth/releases/latest
gh api 'repos/paradigmxyz/reth/issues/24100/comments?per_page=100'
gh api 'repos/paradigmxyz/reth/pulls/27429/reviews?per_page=100'
```

Documentation verification passed: `git diff --check`, the untracked record's
`git diff --no-index --check /dev/null` check (expected exit 1 for the added file,
no whitespace diagnostics), and resolution of its seven local Markdown links.
The shell example passed `bash -n`; it was not re-executed as a deployment
procedure. An initial check wrapper treated the expected no-index exit 1 as an
error; checking its status and empty diagnostic output corrected that wrapper.

## Remaining acceptance work

1. Identify the original receipt/reorg failure behind the manifest comment, or
   explicitly retire that claim in favor of a documented, executable regression.
2. Select an exact official release containing #27429 or an equivalent verified
   fix. Recheck #27397 and the receipt scenario. An arbitrary upstream `main`
   commit would still be an unreleased pin.
3. Port the complete SDK graph and toolchain together; retain `--locked` builds
   after intentionally generating the new lockfile. Build shipping feature sets
   and run the checks required by [AGENTS.md](../../../AGENTS.md).
4. Exercise actual disk reorgs, repeated same-parent composition, the two #27429
   cases, transient `SYNCING`, receipt/static-file divergence, graceful shutdown,
   and interrupted persistence. Assert canonical hashes, state roots, receipts,
   and persisted heights, rather than relying only on an Engine API status.
5. Run affected EEZ signer, driver, queue/reorg, cross-chain, and restart tests,
   including independent replay. Verify the pinned protocol's existing
   commitments and historical decoding; an SDK upgrade does not authorize a
   protocol/genesis change. Cover Ethereum and Gnosis separately.
6. On an authorized cold copy, validate the exact minimal-mainnet datadir and
   snapshot provenance in the intended handoff direction, then advancing the
   chain, restarting, and reading retained history. Preserve an untouched copy
   for rollback; do not assume the old binary can read data after a newer writer
   has modified it. Pruned history remains unavailable regardless of version.
7. Document the tested node/signer upgrade order and any replay/migration limits.
   Existing pending queues remain in memory; clients may need to resubmit after
   restart. Database compatibility does not establish durable queue recovery.

## Workspace and operational changes

Only this investigation record was added. Existing edits to
`docs/runbooks/chiado-deployment.md` and the untracked September 25 Chiado resync
audit were preserved. No dependency, lockfile, protocol pin, service, wallet, or
datadir was changed. The `deploy/mainnet-sync` Compose files from the earlier
conversation are absent at this checkout, so no current image configuration or
database handoff from that setup was validated.
