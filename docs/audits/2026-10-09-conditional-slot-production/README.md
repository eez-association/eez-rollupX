# Conditional slot production: design review

| Field | Value |
| --- | --- |
| Date and status | 2026-10-09 UTC; design review completed after three successful Claude rounds. No remaining design blockers identified; implementation and runtime validation are outstanding. |
| Scope | Plan a simpler preparation trigger and normal idle-slot cadence; no implementation or deployment. |
| Source | `eduadiez/rollupX`, `6b1e281a418f845605744c09a6bc9e8782eb6b61`, branch `feat/composer-wallet-gas-estimation`. |
| Protocol | Initialized gitlink `855fe0602484750861b4c31502a1a91355d33ed6`, without protocol edits. |
| Executables | Claude Code `2.1.295`; no node, signer, follower, or test binary executed for this review. |
| Environment | Local source inspection; Claude API review explicitly authorized by the user, limited to relevant source/design and excluding secrets, operator configuration and unrelated local changes. |
| Related record | [Draft implementation plan](../../proposals/conditional-slot-production.md). |

## Source findings

1. [`advance_anchored_live_tick` and `advance_sync_slot`](../../../crates/eez-driver/src/sequencer.rs)
   unconditionally divide a slot into a paced prefix and an early suffix.
   `commit_one` sends ordinary Future positions to the transaction pool, but
   grid boundaries go through the empty/Composer path.
2. [`dispatch_minimal_postbatch`](../../../crates/eez-composer/src/composer.rs)
   can construct an empty Sync and decline posting when policy is not due. Thus
   reserving a Future suffix does not imply settlement is actually needed.
3. [`sequencing_da_limit`](../../../crates/eez-composer/src/composer/sequencing.rs)
   reserves fixed `K`-block intervals and requires preceding boundaries to be
   empty or owned by an existing optimistic attempt. A pool-filled skipped
   boundary needs more than a scheduler change.
4. Historical splitting in Composer selects empty on-grid endpoints. The
   [RPC contract §3.2](../../../crates/eez-control-rpc/SPEC.md#32-header-and-window-bounds)
   requires an empty terminal for anchor-only posting. Retaining empty
   boundaries avoids changing this accepted contract.
5. The [`position/seed schedule`](../../../crates/eez-protocol/src/l2/derivation.rs)
   is genesis-defined. Ordinary tick phase must not depend on process startup;
   canonical seed reconciliation and exact L1 parent binding remain necessary.
6. Signer capabilities include block, request-byte and witness-item limits;
   [`composer_proving_from_env`](../../../crates/eez-node/src/lib.rs) currently
   forwards the block ceiling into Composer emission limits. A DA-byte bound is
   not proof that an enlarged indivisible segment fits all proof resources.

These are static observations at the recorded fork commit. They do not establish
the deployed mainnet revision, configuration, or historical incident cause.

## Review process and evidence

The user requested review with Claude and iteration. The initial sandboxed
invocation failed before any API inference with `EAI_AGAIN` (exit 1; reported
API usage zero). A network-enabled retry was rejected by the security reviewer
pending explicit authorization to share repository source. The user then
approved sharing scoped source and design with Anthropic; no restriction was
bypassed.

Claude was invoked in print/read-only mode, without custom project hooks,
session persistence, MCP connections, shell tools or write tools:

```bash
claude -p --safe-mode --strict-mcp-config \
  --tools Read,Grep,Glob --allowedTools Read,Grep,Glob \
  --permission-mode dontAsk --no-session-persistence --output-format json \
  < /tmp/rollupx-idle-cadence-review-1.txt
```

The temporary prompt is local-only. It specified the exact source/protocol pins,
the user's full behavior goal, a conditional two-path scheduler, shared policy
planning, a proposed segment-capacity reserve for filled skipped boundaries,
and the requirement to challenge gas/proof/recovery/compatibility assumptions.
It prohibited reading secrets, `.env`, `.claude`, logs, databases, unrelated
operator artifacts, external browsing or delegation. It requested concrete
source references and the smallest coherent implementation sequence.

All successful reviews used the CLI-reported model `claude-opus-5-5`; no model
override was requested. The same read-only command was used with round-specific
temporary prompts. CLI results reported successful completion and no permission
denials for all three rounds. Temporary prompts/raw CLI metadata are local-only;
the substantive reviewer outputs are retained below.

| Evidence | Contents | Availability |
| --- | --- | --- |
| E1 | [Review 1](review-1.md): broad design critique and proposed filled-boundary approach | Retained in this repository change; reviewer claims are not accepted automatically. |
| E2 | [Review 2](review-2.md): response to the primary reviewer's objections and review of the narrower draft | Retained in this repository change. |
| E3 | [Review 3](review-3.md): final consistency check; no remaining design blockers, one target-rule wording clarification | Retained in this repository change; requested clarification incorporated. |

Round 2 explicitly challenged review 1's fullness-only monotonicity claim,
equivalence of maximum batch span and minimum recoverable prefix, treating missing
evidence as permission to build empty, and the requirements of a tolerant reader.
It asked Claude to choose between the broader feature and the independently
useful narrower first release. Claude recommended the narrower scope and
corrected the first review's overstatements.

Round 3 requested a final consistency check of the revised plan, not another
repository-wide investigation or a production-readiness certification. It calls
out the inclusive delivery reserve and the distinction between trigger offset
and remaining preparation allocation. Claude confirmed both corrections and
reported no remaining design blockers. Its requested explicit rule—ordinary
ticks require a known or bounded replacement target, otherwise hold—was added
to the proposal and G2. No further behavioral change followed that review.

## Review dispositions

| Issue | Disposition in the plan |
| --- | --- |
| Fill skipped sync boundaries in the first release? | Deferred. Fix cadence/position first and keep empty on-grid endpoints. State clearly that the original filled-sync goal is not delivered by this first release. |
| Shared plan for payload jobs and segment admission | Necessary for the deferred filled-boundary feature; not added to the first release, whose boundary DA behavior remains unchanged. The sequencer still owns one branch-bound decision. |
| First review: only fullness can become due after the cutoff | Rejected and corrected in round 2. Late ordinary activity in `always` mode, new cross-chain requests and changed eligibility also wait for the next eligible cutoff. |
| First review: equal maximum span means no new proof risk | Too strong. Current shrinking handles block/blob/gas budget failures only; there is no existing general proof/quota liveness guarantee. Losing empty endpoints still enlarges the minimum possible recovery prefix. Do not add that change to this release. |
| First review: unavailable evidence can always produce empty | Rejected and corrected in round 2. Empty fallback requires fresh authenticated context and normal readiness/admission; unknown evidence is not a capacity result. |
| Timed-out replacement and canonical recovery | Explicitly run before the cutoff decision; replacement gets an eligible preparation opportunity, never canonical-completion authority. Preserve the four-replacement bound and late-original handling. |
| Proof-start timing | Explicit-position mode removes the scheduler seed-based late-entry gate in favor of the actual deadline and existing pipeline admission; legacy mode remains unchanged. Add a conservative startup-fit check. |
| Round 2 adds delivery reserve to post-proof reserve | Corrected: the existing 150 ms initial post-proof reserve already includes delivery. Reuse the existing constant once, with the check in Composer/node configuration rather than a driver-to-Composer dependency. |
| `preparation_time()` and telemetry | Derive remaining allocation using the new trigger offset; they do not equal the offset. |
| EndTick/late-head behavior | Add a context-checked empty boundary completion at wall time; bounded structural catch-up and existing outage/readiness guards still apply. |
| Final review: target availability | Explicitly require a known or one of at most four replacement targets for ordinary tick-through-boundary production; no target means hold. |

The proposal retains the five release gates identified in review 2. Design
agreement is separate from implementation, measurements and runtime validation.

## Executed checks and limits

Baseline commands ran from the repository root:

```bash
git status --short
git remote -v
git rev-parse HEAD
git ls-tree HEAD eez-core-protocol
git submodule status --recursive
claude --version
git diff --check
```

Existing modifications to `docker-compose.chiado-node.yml` and
`docker-compose.chiado.yml`, plus unrelated untracked operator/review artifacts,
were left untouched. This work adds only the proposal, this review record and
the retained reviewer outputs.

Documentation validation checks linked local paths, trailing whitespace and
added-file whitespace with `git diff --no-index --check`; shell examples are
parsed with `bash -n`, not executed. The first checker incorrectly treated the
normal added-file diff exit code 1 as failure; after correcting that distinction,
it identified extra EOF blank lines in the retained reviewer output. Those
formatting-only lines were removed before repeating the checks. Reviewer wording
was not altered. The final repeat passed for all five new Markdown files, and
`git diff --check` passed for the tracked worktree changes.

No runtime tests, benchmark, semantic assertions, independent replay, live
configuration inspection, service restart, or deployment ran. Those are
implementation/release gates in the proposal. Static design review is not an
independent security audit and does not establish production readiness.

## Open work

- The documentation-only draft PR starts from `main` at
  `4c4fec08d8a713497e1c8fc23bc0cf6fe0273e0f`, whose protocol gitlink is
  `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. This differs from the reviewed
  source/protocol pair above. The three reviews remain historical design evidence,
  not validation of that newer revision. Recheck affected callers and contracts
  at the implementation baseline before changing runtime behavior.
- Implementation, G1–G5 runtime/performance validation and rollout remain
  unperformed. The chosen first release still leaves every non-settling grid
  boundary empty; evaluate the broader feature separately from measured need.
- This draft recommendation is not user acceptance of the reduced first-release
  scope. No implementation or deployment was undertaken.
