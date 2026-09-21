# Admission history and local-policy evidence (schema 8)

These read-only views distinguish observed admissions, reported terminal decisions
and the evidence accepted for a local disposition. They derive from currently
complete [lifecycle events](lifecycle-ingestion.md), so a late transport conflict
retracts the associated evidence immediately. No new native event, worker,
configuration or ownership state is introduced.

`lifecycle_admission_observations` contains only actual `admitted` events. Its key
is the configured source (service/instance), process epoch and logical sequence.
It retains the declared source identity and explicit `previous`/`replaced` links.
A terminal reference or a later readmission cannot fabricate an absent admission.
Identical transaction hashes in different admissions remain separate histories;
receipt costs retain their independent deduplication and canonical selection.

`lifecycle_terminal_observations` retains each complete terminal event as a claim,
including admission/member, optional owner attempt, outcome, claimed assurance,
anchor, reconciliation link, monotonic elapsed duration and observation provenance.
No claim becomes canonical assurance through this view.

`lifecycle_local_policy_evidence` recognizes the closed local outcome set:
`attempts_exhausted`, `nonce_gap`, `pool_discarded`, and `replaced`. It requires:

- An observed admission from the same configured producer and exactly matching
  source identity.
- A later logical sequence in the admission's process epoch. A supplied owner
  attempt must have that epoch. This follows the native writer's process-scoped
  tickets; no ordering is inferred between different epochs or from UTC.
- Claimed `local_policy` assurance and no fabricated canonical anchor.
- For a linked reconciliation: a complete earlier parent from that process, after
  the admission, with exactly one resolution for this admission. Member, owner,
  disposition, assurance and elapsed duration must agree with the child.

Missing, contradictory or duplicated parent resolutions remain unresolved. This
checks the recorded local transition and its ownership references, not the
protocol's execution or a canonical receipt. The parent scan watermark is not
copied into an inclusion anchor. Policy evidence therefore remains a local
historical action even if chain observations later change.

`lifecycle_admission_status` exposes one row per observed admission:

- `local_policy` only when all currently retained terminal claims have matching
  local-policy evidence and agree on outcome, owner, parent link and elapsed
  duration. Duplicate consistent
  observations do not multiply admissions.
- `unresolved_restart` when no effective disposition is established and the epoch
  has positive ended-process evidence from the existing liveness collector.
- `unresolved` otherwise. Missing or stale writer evidence does not prove an exit.

Unresolved rows have `outcome=NULL` and `evidence_class='unknown'`. The counts of
terminal claims, policy candidates, distinct outcomes and distinct decisions remain
visible.
A competing unevaluated canonical claim prevents choosing a local-policy winner;
this is conservative until canonical outcome validation is implemented. An ended
process does not prove the source failed, never reached a chain, or lost an
irrecoverable event. Late evidence can still resolve the history. These views do
not claim that an unresolved admission is currently present in the native queue.

All views are available to the existing read-only role. Transport freshness,
liveness/probe time and scan completeness remain separate diagnostic fields.
Already observed local actions do not require live chain RPC availability.
Unverified source scopes are displayed as declared in the admission; canonical
receipt views and their identity checks remain separate. Local policy never
counts as successful cross-chain service or canonical terminal progress.

Use the existing stop/backup/explicit-migrate/start upgrade procedure. Schema 8
adds views and a terminal-admission lookup index without rewriting stored events,
receipt charges or verification tables. Earlier observer binaries refuse it.
Full canonical outcome assurance, reorg-aware terminal selection and native
restart canaries remain open R19 gates.
