# L2 continuity during unavailable L1 delivery

| Field | Value |
| --- | --- |
| Date and status | 2026-10-23; static investigation and proposed revision; runtime fix not implemented |
| Source | `codex/l1-history-offline-proving`, `d2f3d7c51e698f33cf7c92843494ed77f0e96e73`; initially clean |
| Worktree | `/home/edu/Development/rollupX/artifacts/l1-history-offline-proving`; original checkout untouched |
| Protocol | Published `feature/e2e_edge_cases` at `84a5b5207880f192a6a5896cbfe0bac5249db47f`; initialized and clean |
| Requirement | L1 missed slots/delivery unavailability must not interrupt ordinary L2 block production |
| Proposal | [Required revision](../../proposals/l1-origin-and-offline-proving.md#required-revision-production-independent-of-l1-delivery) |
| Evidence type | Source/specification inspection only; no new runtime, process, deployment or performance tests executed |

## Findings and integration points

| Finding | Production source | Consequence |
| --- | --- | --- |
| History requires resolution of the scheduled cutoff unless an owned replacement authorizes the exact endpoint. | [CanonicalHistory.headers](../../../crates/eez-l1/src/derivation/history.rs) | Ordinary and empty blocks can wait for L1 evidence. |
| The EVM history hook and next-block environment both require that selection. | [history hook](../../../crates/eez-evm/src/history.rs), [environment derivation](../../../crates/eez-driver/src/derivation.rs) | Changing only the producer would diverge from replay/signing. |
| Live ticks require an L1-derived target; exhaustion leaves the last target fixed and a closed watcher exits the scheduler. | [scheduler](../../../crates/eez-driver/src/slot.rs) | Removing only the history wait would still eventually stall production. |
| The sequencer stops at its live/Sync boundary and enforces a global speculative-depth cap (default 64). | [sequencer](../../../crates/eez-driver/src/sequencer.rs) | Settlement-independent ordinary production needs separate scheduling/admission. |
| Four replacements already existed at the integration baseline `8621b036`. | [shared target bound](../../../crates/eez-l1/src/submitter.rs), [Composer replacement](../../../crates/eez-composer/src/composer.rs) | Keep finite settlement work; do not treat removal of that bound as the continuity fix. |
| V0 operations already carry exact extraData with canonical metadata runs. | [codec](../../../crates/eez-payload-codec/src/v0.rs), [producer projection](../../../crates/eez-driver/src/derivation/encoding.rs) | A recorded origin can use existing public bytes and block commitments. |
| Core treats RollupOperation.operations as opaque rollup-defined bytes. | [pinned blob specification](../../../eez-core-protocol/docs/blobs/BLOB_FORMAT_SPEC.md#22-rollupoperation) | The proposal does not require a new outer blob message. |
| The stateless adapter supplies the RollupX EVM configuration to the library. | [adapter](../../../crates/eez-proof-signer/src/validate/stateless.rs) | Shared execution changes can remain in RollupX; signer and independent replay must be updated. |
| Current full ancestry bridges have a 131,072-header cap plus aggregate proof quotas. | [history acquisition](../../../crates/eez-l1/src/derivation/history.rs), [current limits](../../proposals/l1-origin-and-offline-proving.md) | Continued production does not establish unlimited-outage settlement recovery. |

The existing `late_delivery_waits_and_missed_slots_do_not_create_headers` test in
[history tests](../../../crates/eez-l1/src/derivation/history/tests.rs) intentionally
asserts the wait. This investigation read that regression and its production path;
it did not rerun it or claim a before/after runtime fix.

## Design conclusion

A fixed latest-source cutoff, arbitrarily late L1 delivery and guaranteed
nonblocking local production cannot all hold together: the producer cannot
distinguish a missed slot from delayed delivery before additional information.
The proposed revision retains a recorded origin until an eligible, bounded
advance is supplied. Exact source authentication and delay checks remain;
mandatory maximum freshness is replaced explicitly.

Recording every block's selected origin in existing public metadata is necessary
for follower determinism. Recording it only in a private witness would not let
an independent follower reproduce when the producer advanced. A larger fixed
delay, more speculative retries, or a stale-data fallback without public
selection would not resolve all production gates or the replay ambiguity.

## Work and validation status

Only this record and the existing proposal are changed. No runtime checks were
relaxed, no identities/genesis artifacts were regenerated, no external service
was changed, and no newer runtime behavior is claimed. The prior
[portability validation](../2026-10-23-portable-prover/README.md) remains evidence
for its exact revision.

The proposed eight-byte origin metadata, retained update positions, continuous
clock and separation of cross-chain scheduling require implementation and the
L1–L10 acceptance checks. Freshness discretion and the bounded ancestry recovery
envelope are explicit tradeoffs. The branch's earlier PR-ready status does not
satisfy the newly accepted continuity requirement.

Documentation validation: `git diff --check` and resolution of relative file links
at the initialized protocol pin. No shell procedures are introduced.
