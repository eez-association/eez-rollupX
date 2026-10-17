# Claude review of the v1 fee proposal — 2026-10-14

This is model-generated review evidence, not protocol authority or executed
validation. Claude received the proposal text only, with all tools, repository
access, MCP, customizations and saved sessions disabled. Its observations and
recommended remedies require the source checks and adjudication recorded in
[the audit](README.md#claude-v1-review-and-adjudication--2026-10-14).

| Field | Value |
| --- | --- |
| CLI | Claude Code 2.1.296 |
| Reported model | `claude-opus-5-5` |
| Source baseline | `fa916b13254a36423cad130080dde367bb18a7d6` |
| Protocol gitlink | `855fe0602484750861b4c31502a1a91355d33ed6` |
| Reviewed proposal SHA-256 | `920afed80cee6b627552a66e13854957123fe21fb07f96106a9a710443aa07f8` |
| Review prompt SHA-256 | `37f6e4b88177ba257bcdcf7e323483f17de620bb2406f7101cfb7d401175f304` |
| Response JSON SHA-256 | `51e4c1e29a5df7d35dd019db6ab4494b3c9ad3b54e2f1d63a1c4b9b2fd756195` |
| Elapsed time | 241.606 seconds |
| Model turns | 1 |

The following initial response is retained verbatim. In particular, its suggested
queue eviction/destruction rules and cross-transaction warming claim were not
adopted; see the audit for the reasons and corrected proposal.

## Initial response

# Review: cross-chain fee recovery, first iteration (L1→L2 and L2→L1)

This review covers only the proposal text you supplied. I did not inspect the repository, run any payment or bundle test, or check the cited sources myself. Code-path claims (for example `PostingRuntime::finalize`, `block_fees`, `HeldPool`), provider API behavior, probe results and Relayscan shares are therefore **reported evidence**, and each needs independent verification before anyone relies on it. This session runs read-only with no tools, so no plan file was written.

## Verdict

**L2→L1 is a viable bounded v1, subject to conditions. L1→L2 has insufficient evidence for enforcement. Neither direction needs a material redesign, but L1→L2 needs a narrower scope and one interface decision.**

**L2→L1 is viable if:**
- the deployed L2 header beneficiary is verified as operator-controlled;
- the L2-native asset's redemption value `x` and the sweep path are established;
- the native-receipt indexer defect is fixed;
- the accepted-work underfunding policy (finding 2) is specified.

**L1→L2 can proceed through accounting, shadow quotes and builder qualification only.** Enforced admission waits on:
- a defined route by which the user's signed L1 transaction reaches the Composer without public broadcast (finding 1);
- at least one receipt-backed refund payout on the real native-blob post at an exact target;
- per-attempt costs priced against actual qualified-builder inclusion probability (finding 3).

I recommend making L1→L2 v1 **one L1-origin source group per post** (finding 4).

The broad-builder-coverage goal stays a measured, unmet milestone. Nothing in the proposal establishes it, and the proposal correctly says so.

## Most important findings

### P1-1 (NEW; the proposal treats it only as a test item). The L1 user transaction's route to the private bundle is unspecified under the ordinary wallet flow

**Excerpt:** "L1 recovery depends on the Composer's qualified private bundle route; public-mempool broadcasting … is not an equivalent payment path. Test the actual wallet transport."

**Failure scenario:**
1. The dApp calls `eth_sendTransaction` on the user's Ethereum account.
2. The wallet signs and broadcasts through its own configured L1 RPC. Mainstream wallets generally do not expose `eth_signTransaction`, so the dApp never receives the signed bytes. Some wallets also have their own private-routing features for L1.
3. The transaction reaches the public mempool or another relay before any post exists. It executes against the wrong state or fails, the strict bundle becomes invalid because the nonce is spent, the operator loses preparation work, and no refund arises.

Even with the wallet's L1 RPC pointed at the Composer front, other problems remain:
- The wallet's nonce view and the stuck "pending" display diverge from the privately held transaction.
- A later wallet "speed up" or "cancel" is a second signature.

Standard endpoints cannot fix this. It is a transport property, not a fee property.

**Minimal remedy:** Make the supported L1 transport a v1 design decision. One candidate is a documented one-time wallet network/RPC configuration pointing Ethereum at the L1 front; it is setup rather than a per-transaction action, but it should be confirmed acceptable under your constraints. Then:
- Verify per wallet (MetaMask, Rabby) that signed bytes actually arrive at the front and that wallet private-routing features do not bypass it.
- Detect public appearance of a held source and treat it as a terminal ownership event.
- Make the front's `eth_getTransactionCount` (pending) and receipt behavior consistent with held transactions.

L2 transport is less exposed, because the wallet's L2 RPC is the front by construction.

### P1-2 (NEW gap in an acknowledged area). Accepted work whose coverage lapses before dispatch has no defined funder, and "ordinary replacement" breaks the one-signature constraint

**Excerpts:** "retain accepted work under the existing ownership contract if a subsequent price change blocks dispatch" and "If signed caps no longer cover the candidate, defer or request an ordinary replacement."

**Failure scenario:**
1. L1 user A is quoted and signs, and admission passes.
2. Proof preparation and slot waiting take several blocks. Blob and execution base fees rise; blob base fee can compound per block. A's effective tip is also truncated by `maxFeePerGas − baseFee`.
3. The dispatch gate fails. The proposal now allows only:
   - indefinite hold, with the user's nonce effectively stuck;
   - a replacement, which is a second signature;
   - an implicit subsidy, which F5 and F8 forbid unless reserved.

F8's backlog reserve covers protocol-owed settlement. It is unclear whether an *accepted but unposted* L1 inbound request is "owed." Signed Ethereum transactions also never expire, so a dropped-but-retained signed transaction stays executable later.

The same pattern applies on L2: held outbound sources block the sender's successors.

**Minimal remedy:**
- Quote against a price ceiling over an explicit validity window, measured as a percentile of base-fee and blob-fee movement over that window.
- Designate an **accepted-work reserve** that funds shortfalls inside the window.
- Define a maximum hold, after which the request leaves the owned queue with a structured status and its signed bytes are destroyed.
- State explicitly whether accepted L1 inbound is "owed" under the ownership contract.

### P2-3 (NEW linkage). Exact-target attempt cost is not priced against qualified-builder inclusion probability

**Excerpts:** "Require every endpoint receiving that candidate to satisfy the recovery check independently" and "the child expires next block."

**Failure scenario:**
1. A strict exact-target bundle is sent only to qualified refund builders, initially perhaps one.
2. Each slot the qualified set does not win is a missed attempt.
3. If proofs bind the parent or target, each retry repeats proving and preparation.

The selected user's single tip pays once. With a qualified set that rarely wins slots, expected cost per success multiplies, and the per-group cumulative risk budget is exhausted before inclusion. `C_risk` is described as "historical failed-attempt costs," but it is not tied to route coverage. That makes the broad-coverage goal an economic input, not just a reach metric.

**Minimal remedy:**
- Model `E[attempts] ≈ 1 / P(qualified route wins and selects bundle at target)` from measured landing rates.
- Price `C_risk` per route set.
- Report preparation cost per attempt separately from posting cost.
- Make "maximum attempts per group" an admission input rather than only a retry limit.

### P2-4 (NEW). Shared L1 batches change the selected payer's own receipt gas, and unselected high tips are pure user loss

**Excerpts:** "use the final source transaction as the common default" and "Add other groups only while that check remains satisfied."

**Failure scenario:**
1. Groups B and C are inserted before payer A to keep A last for the default selector.
2. B deploys a proxy or warms persistent storage that A would otherwise pay for. A's receipt gas `g_A` falls, and refund `p·g_A·tip − H` falls below `Q(batch)` even though the batch's cost rose.
3. Meanwhile B and C were each quoted a standalone price, because they could have been alone. Their high tips go entirely to the builder.

Recomputation catches the first problem only at dispatch, where the only outcomes are deferral or blocking (finding 2). The proposal acknowledges the second problem in its economic table but does not prevent it.

**Minimal remedy:** For v1, use **one L1-origin source group per post**, with the payer as the only external transaction in a strict bundle and its hash selected explicitly. Under the low-traffic assumption this costs little and removes order, selection and state-interaction coupling. Mixed-direction posts still sum held L2 tips and the single L1 refund once, as the proposal specifies. Defer multi-L1-group batches until pooled refunds are qualified.

### P2-5 (partly acknowledged). L2→L1 poison groups and attacker-controlled L1 state are not isolated within a batch

**Excerpts:** "L1 application work can instead execute inside PostBatch and is then poster-funded" and "Independent eligible work progresses."

**Failure scenario:**
1. An attacker's L2 source calls an L1 contract whose result or gas depends on state the attacker controls.
2. After composition, a public L1 transaction earlier in the target block changes that state.
3. The post's committed result mismatches, so the strict bundle is dropped, or the post reverts and still pays blob fees.
4. Every co-batched group fails with it. Repeated attempts burn preparation cost, and the attacker's L2 tip is never retained.

Per-group cumulative budgets are acknowledged. Batch-level isolation and the rule for deciding which group to evict are not. The point at which an L2-origin source's tip becomes "retained" relative to Live L2 blocks also needs stating.

**Minimal remedy:**
- Attribute a post failure to its divergent group using the final ordered simulation against the builder-observed or next-parent state.
- Evict or quarantine that group, and recompose the rest without it.
- Count prepared-but-failed attempts against the group's sender and globally.
- State when an L2-origin source and its tip become canonical obligations.

### P3-6 (EDITORIAL conflict with real consequences). Exploratory scope leaks into the v1 rollout and acceptance, and stage sequencing silently disables L1→L2

- Stage 2b and the acceptance matrix include sponsorship and combined sponsorship/refund routes. The "Native L2 settlement fees" section calls itself "the recommended bounded prototype." Your scope excludes both. Move them to stage 4 or a separate design.
- **Design consequence:** stage 2a ("Enabling L2 enforcement must also select this inbound policy") forces either rejection of all unfunded L1-origin work or an explicit subsidy before L1 qualification (2b and 3). In practice L1→L2 is unavailable or subsidized between those stages.

**Minimal remedy:** State the interim inbound policy and its reserve size explicitly, and label the non-v1 sections as such.

### P3-7 (partly acknowledged). Refund receipt attribution is underspecified for durable, idempotent accounting

**Excerpts:** "Delayed rebates remain receivables until paid" and "can combine eligible simple recipient payouts at block end."

**Failure scenario:** Builder refunds arrive as plain ETH transfers to the treasury, but there are complications:
- they can be coalesced, delayed (BuilderNet), or arrive in a reorged block;
- the treasury can also receive sweeps, operator funding or unrelated deposits.

A ledger keyed only on attempt cannot uniquely match a coalesced transfer. That leads either to double recognition, or to unattributed income used as coverage.

**Minimal remedy:**
- Use a dedicated treasury recipient per network and route.
- Attribute each receipt using provider refund-query records together with canonical block and transaction evidence.
- Book unmatched receipts as "unidentified," with zero coverage credit.
- Make the receivable lifecycle `expected → observed → canonical-final` per attempt.

## What the proposal gets right

- **Meter separation.** It forbids summing inclusive parent and child traces. It distinguishes Composer interception gas from materialized native receipt gas, and keeps gas limit, expected charge and service contribution as three outputs.
- **Execution location decides payer.** An L1 callback in a user companion is user gas; the same work inside PostBatch is poster gas. Bridged principal is never revenue or expense.
- **Correct revenue arithmetic.** It uses effective tips, not caps, and actual receipt gas, not limits. A refund of the poster's own tip is an expense reduction, not revenue. `p·T − H` is used conservatively, and provider-specific deduction order is retained.
- **Separate dispatch tests.** Economic coverage and downside funding are separate gates, with variable redistributions given zero credit and ACKs not treated as evidence.
- **Clear separation of fact from proof.** It states that a strict bundle is builder policy, not on-chain atomicity, and keeps declared support apart from receipt-backed coverage.
- **Strong research.** The repeated-droppable-prefix counterexample, the analysis of the dependent-bundle admission limit, and the outcome-based admission rule are useful.
- **A concrete accounting defect.** The native-receipt indexer path is identified, with the correct remedy: type-check, do not clamp.
- **Honest framing.** It claims no zero-loss guarantee and treats the coverage milestone as unmet.

One addition to the estimator: L2 receipt gas `g_i` can be measured pre-signing from a materialized L2 draft, because proof bytes are not needed for L2 execution. The L1 payer's receipt gas can also be simulated with a proof-independent placeholder verifier. Only post verification and calldata need calibrated allowances. This makes the pre-signing quote more practicable than the proposal implies.

## Smallest work packages and measurable launch criteria

| WP | Content | Acceptance (measurable) |
| --- | --- | --- |
| 0. Accounting | Native-receipt indexer fix; beneficiary verification per block; durable ledger (`quoted → reserved → dispatched → reconciled`), keyed on source, generation, attempt and target; dedicated treasury per route | Fixture with native receipts in positive-base-fee blocks reconciles; one full posting window reconciles payer and beneficiary with no zero-filled gaps; replaying ACK, refund and reorg events changes nothing |
| 1. Shadow estimator | Materialized L2 draft `g_i`; L1 payer receipt gas via proof-independent simulation; allowances for poster, blob and proof; HTTP service; no enforcement | Per-workload p50/p95/p99 error of quote vs canonical cost, covering the proposal's matrix (nested, multi-root, static, caught revert, proxy fresh/reused, blob boundary), with the error bound chosen from an explicit tolerated-loss budget |
| 2. SDK and transport | Fee-field setting; L1 transport per finding 1; wallet tests | MetaMask and Rabby: signed fee fields equal the quote, one signature, signed bytes arrive at the front, zero public-mempool appearances in the test series, nonce and pending display consistent |
| 3. L2 enforcement | Admission and dispatch gates, `fee_blocked` extension, accepted-work reserve and window (finding 2), poison-group eviction (finding 5) | Adversarial cases from F3, F4, F5 and F8 pass; realized L2 tips minus redemption cost cover `Q` over a defined shadow-then-enforced window with zero builder-redistribution credit; `x` documented |
| 4. L1 adapter, first route | One-group strict bundle, explicit refund hash, treasury recipient, exact target; homogeneous qualified fan-out only | Receipt-backed refunds on the real native-blob post across multiple batches; wrong-source, missing, delayed, reduced and reorged refund cases handled; measured landing rate feeds `C_risk` |
| 5. L1 enforcement | Enabled per route once WP4 evidence exists | Cumulative contribution ≥ 0 including failed and prepared attempts over the evaluation window; reserve never breached |
| 6. Coverage expansion | Additional adapters; authenticated, aligned coverage measurement | Each route passes WP4 independently; receipt-backed coverage reported separately from declared support |

## Unresolved choices

**These block implementation:**
1. The L1 signed-transaction transport and whether one-time wallet RPC setup is acceptable (finding 1).
2. The accepted-work policy: quote validity window, reserve, maximum hold, and whether accepted L1 inbound is "owed" (finding 2).
3. The launch network for L1→L2. No Chiado/Gnosis refund route is established, so the L1 route may be Ethereum or Hoodi only at first.
4. The interim inbound policy while L2 enforcement precedes L1 qualification.
5. The verified L2 beneficiary, the redemption route and `x`.
6. Whether L1→L2 v1 is one group per post (recommended).
7. The tolerated-loss budget, used to size reserves and allowances.

**These can wait:**
- pooled multi-source refunds;
- sponsorship and value-payout modes;
- customer rebates;
- fair per-user allocation and batch discounts;
- the native settlement-gas fee;
- a wallet-facing `eez_quoteCrossChainFee` method;
- multi-L1-group batches;
- separate-bundle refund stacking;
- adapters beyond the first qualified route, with the coverage target tracked as a measured milestone rather than dropped.

## Follow-up response

The second pass reviewed the revised proposal sections and the initial response,
including explicit challenges to unsafe or unsupported remedies. This response
is retained verbatim; its remaining policy points were subsequently resolved as
recorded in the audit.

| Field | Value |
| --- | --- |
| Reported model | `claude-opus-5-5` |
| Reviewed proposal SHA-256 | `a475fc4cb84bb0eea47cf582b16489279b33720ea848b037b5774173832dc240` |
| Follow-up prompt SHA-256 | `c18c4fc6833fad6ef946667617ac926676b8044de857fb0fa48e0c63e18cfc85` |
| Response JSON SHA-256 | `79dc28379e0424bbacb3ff457b95fa8364a6e5ab6f825ebcd5d3d186c39f9a9e` |
| Elapsed time | 55.346 seconds |
| Model turns | 1 |

# Follow-up review: revised cross-chain fee proposal

This review covers only the supplied text. I did not inspect source code or run anything. No plan file was written, because tools are disabled. Statements about wallets, builders and code paths remain your reported evidence.

## 1. What is resolved at design level, and what still needs evidence

| Initial finding | Design status | Evidence still needed (not yet run) |
|---|---|---|
| P1-1 L1 transport | **Resolved, with a gap** (see §2b). The wallet's L1 RPC is the EEZ front, wallet setup is one-time, there is a held-status and pending-nonce view, and public appearance is a risk signal rather than a terminal event. | Per wallet (MetaMask, Rabby): signed bytes reach the front, private-routing bypass, fee edits, resend and replace behavior. |
| P1-2 accepted-work funding | **Resolved.** There is a funding-blocked status, no drop or nonce-spent fiction, a bounded preparation budget, and a replacement counts as a second signature. Your correction on signed-transaction lifetime is right. | Calibration of the price envelope and budget. |
| P2-3 attempt cost vs. route coverage | **Resolved.** Admission uses a finite monetary/work budget, preparation is priced per route set, and compute is decoupled from retry counters. | Measured landing rates and compute cost per preparation. |
| P2-4 shared L1 batches | **Resolved as a conditional feature.** Single-group qualification comes first. Shared batches are gated on ordered tests. Unselected tips are disclosed as user expense. | The ordered payer-gas tests. |
| P2-5 poison groups | **Partly resolved.** Rejecting fault attribution is correct. Cost allocation across groups is still missing (see §2c). | None. |
| P3-6 scope leakage | **Resolved.** Non-v1 items are labeled. Combined activation waits for both tracks. The L2-only pilot is labeled and rejects unfunded inbound. | None. |
| P3-7 refund attribution | **Resolved.** Payment identity is separate from attempt identity. Allocations are bounded by the verified amount. Unidentified payments get zero credit. | A receipt-backed payout on the native-blob post. |

## 2. Remaining gaps that block implementation of the selected v1

**a. Dispatch test 1 contradicts reserve-funded shortfalls.**
- Test 1 requires "projected realizable *revenue*" to cover `Q(B)`.
- Elsewhere, two passages allow reserve money to make up a shortfall:
  - "a reserve can fund a service shortfall within that budget";
  - an occupancy shortfall may "consume its reserved discount coverage."
- A reserve draw is not revenue. As written, either the reserve paths can never authorize dispatch, or test 1 is silently weakened.
- **Decide:** coverage = qualified revenue + an explicitly pre-allocated reserve draw. Record it per candidate, and cap it by the group's allocation and the global reserve. Keep test 2 (downside funding) separate.

**b. The L1 front's handling of non-EEZ and same-nonce transactions is undefined.** Once the wallet's Ethereum RPC is the front, *all* of the user's L1 transactions arrive there. The proposal specifies only the held path. Three cases need a rule:

1. **Classification.** Which raw transactions are held and which are passed through to the public mempool?
2. **Nonce successors.** Suppose a held source is at nonce n and an ordinary transfer at n+1 arrives.
   - Forwarding n+1 publicly leaves it stranded behind a gap.
   - Holding it makes the front a general-purpose private-transaction custodian.
3. **Same-nonce replacement of a held source.** A higher-fee replacement could be re-quoted and swapped into the hold. A cancel (a self-transfer) might be forwarded publicly. Either choice interacts with HeldPool ownership. The ownership contract says a *canonical* conflicting nonce terminates the request, but it does not say what the front does on *receipt* of a conflict.

These are front-behavior decisions, not tests. The wallet tests can only check a rule once it exists.

**c. Failure-cost allocation in multi-group L2→L1 candidates.** You correctly forbid fault attribution. However, per-group cumulative budgets still need a rule for charging a failed shared post. Without one:
- either no group's budget is depleted, which makes the budget unbounded;
- or allocation is ad hoc.

**Suggested fault-free rule:**
- Charge failed shared cost pro rata to the groups' allocated contributions.
- After an unattributed failure of a multi-group candidate, recompose the next attempt as smaller or single-group candidates.

This is a selection choice, not a disposition, so it fits the existing `fee_blocked`/FIFO-eligibility pattern. It also isolates a state-sensitive group without accusing it.

**d. Is accepted-but-unposted L1 inbound "already-required settlement"?** Your text implies it is not: it can become funding-blocked. Meanwhile, "already-required settlement keeps its reserve" applies elsewhere. State the classification in one sentence, so the reserve sizing for these two cases cannot be conflated.

## 3. Architecture

**This is a viable bounded integration plan. No material redesign is needed.** The shared estimator, two collection routes, two-gate dispatch, ledger-beside-HeldPool and staged activation are coherent.

The gaps in §2 are local policy decisions:
- (a) is a single definition;
- (b) is front behavior;
- (c) is an allocation rule plus a recomposition heuristic;
- (d) is one sentence.

None of them changes the user's signature flow or the ownership contract.

## 4. Your rejected or qualified remedies

| Remedy | Assessment |
|---|---|
| **Public appearance not terminal; no deletion or expiry** | **Agree.** My "destroy signed bytes" remedy was ineffective, and calling public appearance terminal contradicted the pinned contract. One addition: after public appearance, dispatch that source only in candidates whose failure exposure is already funded. The source can now be mined independently at any time, which invalidates the bundle. |
| **Percentile window is not a guarantee** | **Agree.** The reserve-within-budget framing is correct, apart from the test-1 gap in §2a. |
| **No later-parent attribution** | **Agree.** The isolation-by-recomposition rule in §2c gets the same practical benefit without attribution. |
| **Cross-transaction warmth** | **Agree.** I wrote "warms persistent storage," which conflated two effects: access warmth does not carry between transactions, but state changes do. The remaining concern, earlier groups reducing payer gas, is the one you kept. |
| **Shared batches as conditional v1** | **Accept, given the user's stated tolerance.** Two qualifications: the "earlier group deploys proxy or changes state" case should be a named test in the shared-batch exit criteria, and the disclosure that an unselected tip goes wholly to the builder belongs in the *quote shown before signing*, not only in post-hoc accounting. A user quoted standalone signs expecting that price to fund the service. |
| **Proof-independent simulation** | **Agree.** Quote-stage only, with a calibrated allowance. It was never a production validator. A placeholder verifier changes gas, so it needs calibration like any other allowance. |
| **Per-route recipients optional** | **Agree.** The many-to-one allocation rule with zero credit for unidentified payments satisfies the accounting need. |
| **Two-track activation and labeled pilot** | **Agree.** The broad-builder-coverage goal remains an unmet, measured milestone. Nothing in the revision claims otherwise. |

## Targeted closure response

The third pass checked the four final policy decisions. Its remaining wording
points were resolved after this response; notably, current classification-error
holding was preserved for pricing-disabled mode, while proposed enforcement
rejects new unclassifiable requests before ownership. No runtime behavior was
changed by this research. The response below is retained verbatim.

| Field | Value |
| --- | --- |
| Reported model | `claude-opus-5-5` |
| Reviewed proposal SHA-256 | `19afa7a0f927be8376228d75be0ed5a58d35f9bddc3bda60c06c27066ae182ac` |
| Closure prompt SHA-256 | `6e112f0cb3c3fc61856c10aba971061d00c09b89ef8803ef85a76f7bd2d6c7b0` |
| Response JSON SHA-256 | `efc178d81ff978311b6409d63cef1adb1ffdf47c238644d18c25b1918e95629e` |
| Elapsed time | 28.933 seconds |
| Model turns | 1 |

# Final targeted closure review

This review covers the supplied text only. Tools are disabled, so I wrote no plan file and inspected no code. Wallet, builder and code-path claims remain your reported evidence.

## Consistency of the four decisions

**(a) Strict v1 is consistent, with one wording gap.** The reserve and discount passages are gone. Test 1, the deferral rule and the discount rule now all require customer recovery.

- **Gap: mixed candidates.** Test 1 says revenue "from this exact candidate covers `Q(B)`." Elsewhere, an already-due post's fixed cost goes to the backlog budget, and new groups pay only incremental cost. Read literally, a new group riding on an already-due post fails test 1, because backlog funding is not candidate revenue. Read loosely, the backlog reserve quietly subsidizes new demand.
- **Fix:** add one sentence to test 1. The new groups' qualified recovery must cover their allocated shares of `Q(B)`. The pre-identified backlog share is covered only by the backlog budget, and its size is set by the actual trigger baseline.

**(b) Front rules are consistent with the ownership contract, with one ambiguity.**

- **Gap: what "fail closed" does.** It must mean *reject with an error*, not *hold*. Holding an unclassifiable, possibly ordinary transaction makes the front the general custodian you have excluded. State this explicitly.
- **Not a contradiction, but a wallet-evidence risk.** Read methods are forwarded upstream, so a wallet's pending-nonce query returns `n`, not `n+1`. A later ordinary transaction therefore often reuses `n`. It is then forwarded publicly and can canonically consume the held source. The text already disclaims a reservation-aware nonce view and limits the SDK to one unsettled submission. Wallet testing must measure this case, and the user-facing status should warn about it.

**(c) Shared-cost allocation is consistent.**

- Frozen weights, exact apportionment, single consumption of each budget and recomposition without eviction fit the no-fault model.
- **Minor wording issue:** "missing attribution data" (allocation inputs) can be misread as fault attribution. Consider "missing allocation data."
- **Implied precondition:** test 2 should reserve each group's frozen share of worst-case failure exposure before dispatch. Your "must fit remaining allocated risk budget" covers this if it is applied at reservation time, not after the loss.

**(d) Inbound classification is consistent.** Owned-but-blocked demand and separately funded required settlement are distinct, and neither can stall the other.

## Remaining concrete contradictions

None that is substantive. The two items above are definitional fixes of one sentence each:

1. how test 1 applies to mixed backlog/new candidates;
2. that fail-closed classification means reject, not hold.

## Evidence still needed (not design blockers)

| Area | Evidence |
|---|---|
| Wallets | MetaMask and Rabby: setting the front as RPC, private-routing bypass, fee edits, resend, nonce selection against an upstream-only pending count, same-nonce replacement and cancellation |
| Builders | Landing probability for our bundles specifically, retained-bid effects, behavior after the source appears publicly |
| Calibration | Price envelope, preparation budget, compute cost per proof, idle obligatory-posting cost floor, placeholder-verifier gas allowance |
| Implementation | `fee_blocked` policy-version extension, allocation ledger with integer apportionment, HeldPool dual-check replacement, front classifier reject path |
| Shared batches | Ordered payer-gas tests, including the proxy-deployment and state-change cases |
| Refunds | A receipt-backed payout on the native-blob post |

## Verdict

**"Viable bounded integration plan; no material redesign" still holds.** None of the decisions changes the signature flow, the ownership contract or the FIFO/disposition model.

This verdict concerns textual design consistency only. No implementation, wallet, builder or calibration test has run, and nothing here indicates launch readiness.
