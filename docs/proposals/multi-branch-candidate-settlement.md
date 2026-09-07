# Hybrid Multi-Branch Candidate Settlement

| Field | Value |
| --- | --- |
| Status | Draft for protocol-engineering discussion |
| Related issue | `eduadiez/rollupX#12` |
| Scope | Composer, prover workers, L1 submitter, deriver, held-pool policy |
| Consensus change | None required for local quarantine; candidate semantics require protocol review |

## 1. Abstract

For `n` ordered atomic transaction groups, this proposal constructs `m`
independently re-executed and proved settlement branches. Each branch has its
own `postBatch`, data-availability payload, proof, and candidate L2 block. The
same branch proof may then be reused across strict bundle prefixes of that
branch. Proofs are never reused across independently recomposed branches. This
is the hybrid: limited branch-level re-execution broadens failure coverage,
while prefix candidates multiply settlement choices without multiplying proofs.

The relay may select any submitted candidate. Safety therefore cannot depend on
the relay preferring the longest, highest-value, or least-filtered candidate.
Every candidate must independently describe a valid canonical outcome.

The proposal also defines optional post-settlement attribution and quarantine.
Candidate non-selection is not evidence of abuse. Only a deterministic failure
reproduced against the relevant canonical execution environment may create a
strike, and sender quarantine requires repeated independently attributable
failures.

## 2. Goals

1. Allow an independent later transaction to settle when another transaction
   is invalid or economically undesirable.
2. Reuse proof work safely where candidates share the exact same `postBatch`.
3. Keep every relay candidate strict and atomic without
   `droppingTxHashes` or `revertingTxHashes`.
4. Remain correct for any candidate selected by the builder.
5. Bound proof, execution, memory, network, and relay-simulation work.
6. Attribute repeatable candidate poisoning without penalizing ordinary
   builder selection, congestion, or transient failures.

## 3. Non-goals

- Guaranteeing that a builder selects the most inclusive candidate.
- Enumerating all `2^n` transaction subsets.
- Treating local quarantine as a consensus or Sybil-resistance mechanism.
- Reusing one proof across different state transitions or public inputs.
- Allowing the relay to remove or tolerate transactions inside a candidate.

## 4. Terminology

- **Base order**: deterministic transaction order produced by the scheduling
  policy, including gas/settlement bid ordering and deterministic tie-breaking.
- **Atomic group `G_i`**: one signed user transaction plus its optional
  poster-signed root checkpoint. A group is never split.
- **Branch `B_b`**: an independently selected, dependency-closed ordered subset
  of the base order, re-executed from the common canonical parent.
- **`P_b`**: the branch-specific signed `postBatch`, including its branch data
  and proof.
- **Prefix candidate `C_(b,k)`**: strict bundle `P_b` followed by the first `k`
  atomic groups of branch `B_b`.
- **Primary candidate**: preferred optimistic candidate, normally the complete
  highest-ranked branch. It has no special safety status.
- **Selected candidate**: the candidate whose `postBatch` is canonically
  included on L1.

The key words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative.

## 5. Candidate construction

### 5.1 Base order

The composer MUST construct one deterministic base order. The ordering function
MUST include deterministic tie-breaking and MUST preserve dependency closure,
including per-sender nonce order.

An atomic group whose prerequisite is excluded MUST also be excluded unless the
branch is independently re-executed and proves that the dependency is no longer
required. A same-sender nonce gap always invalidates the branch.

### 5.2 Branches

Each retained branch MUST be re-executed from the same canonical L2 parent and
the same target slot context. Re-execution MUST produce branch-specific:

- system and user transaction sequence;
- intermediate and final roots;
- candidate L2 block;
- settlement entries and checkpoints;
- data-availability payload;
- proof/public-input commitment; and
- signed `postBatch` transaction.

No root, block hash, proof, or public input from another branch may be copied
unless it is independently recomputed and equal by construction.

The initial polynomial branch generator MAY use suffix branches:

```text
B0 = [G1, G2, ..., Gn]
B1 = [G2, G3, ..., Gn]
...
Bn-1 = [Gn]
```

More general dependency-valid subsets MAY be retained by a deterministic
bounded search. The generator and its limits are policy inputs, not builder
inputs.

### 5.3 Prefixes within one branch

For branch `B_b = [G_b0, ..., G_b(ℓ_b-1)]`, the composer may submit:

```text
[P_b, G_b0]
[P_b, G_b0, G_b1]
...
[P_b, G_b0, ..., G_b(ℓ_b-1)]
```

All candidates in the branch MUST contain the byte-identical `P_b`. Therefore,
they MAY reuse `P_b`'s proof. The proof and batch MUST commit to every permitted
prefix endpoint required for deterministic reconciliation.

`[P_b]` SHOULD NOT be submitted when a non-empty group is available because it
allows selection with no user-transaction progress. A postBatch-only candidate
remains valid for a genuinely empty branch or explicit backlog-only settlement.

Proofs MUST NOT be reused between `P_b` and `P_c` when `b != c`, because the
branch block, entries, data payload, roots, or public inputs differ.

### 5.4 Candidate count

For `n` ordered groups and `m <= n` independently proved suffix branches
starting at `G1` through `Gm`, submitting every non-empty prefix requires:

```text
proofs     = m
candidates = n + (n - 1) + ... + (n - m + 1)
           = mn - m(m - 1) / 2
```

Example for `n = 20`, `m = 5`:

```text
proofs     = 5
candidates = 20 + 19 + 18 + 17 + 16 = 90
```

The `m` proofs MAY be produced concurrently by `m` prover workers, but the
proposal does not require `m` distinct proof systems or trust domains.

If suffix branches are retained for every start position and every non-empty
prefix is submitted, the family covers every contiguous interval and requires:

```text
proofs     = n
candidates = n(n + 1) / 2
```

This does not cover non-contiguous subsets.

The implementation MUST enforce deterministic resource budgets. Relevant
limits include:

- admitted groups per Sync slot;
- independently proved branches;
- total relay candidates;
- total serialized request bytes;
- concurrent proof and simulation work;
- memory; and
- a generation deadline before the target L1 block.

If a limit is reached, truncation MUST be deterministic and observable. Safety
must not depend on retaining a particular candidate.

## 6. Mutual exclusion and relay submission

All branch `postBatch` transactions for one decision MUST:

1. use the same poster address and nonce;
2. target the same exact L1 block and timestamp;
3. bind the same canonical parent/settlement cursor; and
4. be submitted only through strict atomic bundles.

The shared poster nonce makes branch `postBatch` transactions mutually
exclusive. The exact target prevents an unselected branch from landing later.

Every bundle MUST leave `droppingTxHashes` and `revertingTxHashes` empty. A
relay acknowledgement is not a settlement verdict. The canonical L1 block and
receipts are authoritative.

The protocol MUST NOT assume that the builder prefers:

- a longer prefix;
- a branch that excludes fewer groups;
- the highest total tip;
- the highest gas-price density; or
- the first or last submitted candidate.

## 7. Selection validation

The submitter MUST map every `postBatch` hash to immutable candidate metadata:

```text
decision id
branch id
prefix endpoints
candidate L2 block identities
included held-transaction identities
excluded held-transaction identities
expected L1 roots
target block and timestamp
```

After one `P_b` is included, validation MUST establish:

1. `P_b` succeeded in the exact target block.
2. The following candidate transactions form exactly one offered complete
   prefix of atomic groups.
3. Every included transaction succeeded and is contiguous and ordered directly
   after `P_b`, modulo only behavior explicitly guaranteed by the relay API.
4. No transaction from an excluded group appears elsewhere in the block.
5. L1 emitted the expected root for the selected branch/prefix endpoint.
6. The deferred-entry authorization is scoped to the selected L1 block, so an
   excluded raw transaction cannot consume a stale queue later. The current
   contract's `lastVerifiedBlock == block.number` gate is the expected mechanism.

Partial groups, reordered groups, multiple branch `postBatch` inclusions,
unexpected candidate transactions, or an unknown root are protocol invariant
violations. The node MUST fail closed and MUST NOT guess a candidate identity.

## 8. Canonicalization and held-pool accounting

The composer MAY optimistically commit the primary candidate. It MUST keep the
settlement gate closed until the deriver canonicalizes the selected candidate.

If another branch or a shorter prefix lands, the deriver MUST reconstruct and
commit that exact independently proved candidate at the Sync height. Descendants
of a replaced optimistic block MUST be reorged or replayed through the existing
canonical reconciliation path.

Held-pool accounting MUST occur exactly once after canonical confirmation:

- release nonce reservations for selected groups;
- requeue every excluded group that remains admissible;
- preserve each sender's nonce dependencies;
- avoid requeueing any transaction with a canonical receipt; and
- retain selected groups until finality for L1-reorg recovery.

An L1 reorg MUST restore only groups whose selected settlement was rolled out.
Finality audit MUST verify the selected `postBatch` receipt before permanently
discarding recovery metadata.

## 9. Post-settlement failure attribution

### 9.1 Evidence boundary

The following are never sufficient evidence of malicious behavior:

- a candidate was not selected;
- a shorter or more filtered candidate was selected;
- relay rejection, timeout, or transport failure;
- a skipped target slot;
- base-fee movement or block gas pressure;
- builder profitability decisions; or
- transient or non-reproducible state contention.

### 9.2 Reproduction

An excluded transaction may be classified only by replaying the exact signed
candidate in the relevant execution context. The replay SHOULD reproduce:

- canonical target-block parent state;
- target number, timestamp, base fee, gas limit, and fee rules;
- the exact branch `postBatch` and preceding group sequence;
- sender balances and nonces; and
- protocol-specific queue/root state.

Where builder insertion position or private order flow makes the counterfactual
state unknowable, the result MUST be classified as inconclusive. An
implementation MUST NOT manufacture certainty from a different state snapshot.

Eligible deterministic classifications include:

- invalid signature, chain id, or envelope;
- intrinsic-gas or protocol fee invalidity;
- invalid or poisoned nonce dependency;
- insufficient balance in the reproduced state;
- deterministic contract revert;
- protocol root, entry, checkpoint, or ordering violation; and
- repeated candidate poisoning by independently signed transactions.

Replay output SHOULD record a stable reason code, candidate id, failing group,
state/block reference, and diagnostic hash. Revert strings are diagnostics, not
stable policy identifiers.

## 10. Quarantine policy

Quarantine is local admission policy and MUST NOT affect proof validity or
consensus acceptance.

The policy SHOULD escalate in layers:

1. reject or quarantine the exact transaction hash after one reproducible
   deterministic failure;
2. temporarily quarantine the affected sender/nonce chain after repeated
   failures at that chain position; and
3. quarantine `(chain_id, direction, sender)` only after a configurable number
   of independent attributable poison events.

Expiry SHOULD use L1 block height rather than wall-clock time. Backoff SHOULD be
exponential and capped, for example:

```text
duration(strikes) = min(base_blocks * 2^(strikes - 1), max_blocks)
```

Successful canonical execution SHOULD decay strikes. Replacement policy MUST
be explicit: a different signed transaction at the same nonce may be admitted
only if doing so cannot bypass an active sender-level quarantine.

The quarantine store MUST be:

- bounded in entries and per-sender history;
- resistant to unbounded attacker-controlled reason strings;
- observable through structured logs/metrics;
- persistent or deliberately reset on restart;
- pruned by canonical L1 height; and
- safe under L1 reorgs near an expiry boundary.

Quarantine does not prevent address rotation. Admission pricing, per-sender
limits, and candidate-work budgets remain necessary.

## 11. Required invariants

1. **Independent branch validity:** every `P_b` proves only its own branch.
2. **Within-branch proof identity:** every prefix of a branch uses identical
   `P_b` bytes.
3. **Atomic groups:** a transaction and checkpoint are never split.
4. **Mutual exclusion:** at most one branch `postBatch` can be canonical.
5. **Exact selection:** canonical receipts identify one offered branch/prefix.
6. **No stale execution:** excluded groups cannot execute after the target
   block using stale deferred state.
7. **Any-choice safety:** every builder-selectable candidate is canonicalizable.
8. **One-time accounting:** every held transaction is released, requeued, or
   evicted exactly once per decision.
9. **Evidence before penalty:** non-selection alone never creates a strike.
10. **Bounded work:** user input cannot cause work above configured limits.

## 12. Testing requirements

### Candidate generation

- deterministic base ordering and tie-breaking;
- nonce/dependency closure;
- branch-independent roots, blocks, blobs, and proofs;
- byte-identical `P_b` across one branch's prefixes;
- different public inputs across different branches;
- deterministic truncation under every resource budget.

### Relay and selection

- builder chooses full, short, and filtered candidates in arbitrary order;
- same-nonce branch mutual exclusion;
- strict rejection of partial/reordered atomic groups;
- no reliance on `droppingTxHashes` or `revertingTxHashes`;
- exact target/timestamp expiry;
- ambiguous transport response followed by canonical observation.

### Canonicalization

- replacement of the optimistic primary by every retained branch/prefix;
- excluded-group requeue and included-group release exactly once;
- sender nonce dependencies after requeue;
- L1 reorg before and after cursor confirmation;
- finality-audit recovery;
- stale deferred entries rejected outside the target block.

### Attribution and quarantine

- deterministic poison produces stable evidence and a strike;
- builder non-selection, skipped slot, fee movement, and transport failure do
  not produce strikes;
- inconclusive replay fails open for admission policy;
- exponential expiry, cap, decay, persistence, pruning, and reorg behavior;
- sender replacement and address-rotation scenarios;
- bounded storage under adversarial unique senders and reason payloads.

## 13. Rollout

1. Land candidate metadata and observation without enabling alternate branches.
2. Add deterministic candidate generation behind a feature flag.
3. Run shadow generation/replay and measure proof latency, serialized bytes,
   relay acceptance, and selection behavior.
4. Enable a small candidate budget on devnet and exercise arbitrary builder
   selection and L1 reorgs.
5. Enable quarantine in observe-only mode.
6. Activate transaction-level quarantine, then sender quarantine only after
   false-positive review.
7. Increase budgets from measured slot-deadline and relay-rate headroom.

## 14. Open questions

1. Which branch generator gives the best coverage per proof: suffixes, bounded
   beam search, branch-and-bound, or a hybrid?
2. Should all branches be submitted immediately, or only after evidence that a
   higher-ranked branch is invalid?
3. What are the production limits for proofs, candidates, serialized bytes,
   concurrency, and generation deadline?
4. Which exact public inputs must commit every permitted prefix endpoint?
5. What replay source is authoritative enough for sender attribution when the
   builder's private insertion state is unavailable?
6. What strike threshold, decay, and maximum quarantine duration are acceptable?
7. Should quarantine state be shared among composers or remain strictly local?
8. How should candidate-generation work be priced so one sender cannot obtain
   quadratic work at negligible cost?
