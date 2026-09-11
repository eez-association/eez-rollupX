# Backport regression and production-readiness review

**Assessment: not ready for production deployment.** The backport preserves the
fork behavior reviewed below, but passing tests and complete upstream coverage
do not establish production readiness. This follow-up starts from
`401a2dc90961d3cbe020f9a6adfc11d012422845` and fixes a reproduced availability bug.
It does not certify the absence of other bugs.

Correction commit: `880acab00663127a029ed331240967b8ea934372`.

**Confirmed finding and correction.** The backported `await_l1_head` loop allowed
30 polls separated by two-second sleeps, but did not bound time inside an RPC
probe. The existing L1 reader used the HTTP client's default without a request
timeout. A stalled response could hold the new catch-up wait indefinitely; slow
responses could stretch the advertised wait far beyond a minute. The subsequent
canonical-hash read had the same underlying transport problem.

The fix gives the catch-up probe and its sleeps one 60-second deadline. Every
`L1Reader` HTTP request now has a 30-second timeout covering response-body reads
as well as headers. Transport failures retain their `L1Error::Provider` class
and flow through the existing recovery policy. In `correlation_seed`, that error
returns before the code that removes noncanonical correlation history. Existing
successful-head, temporary-lag, source-incomplete, and canonicality rules remain.

Four regression cases failed with the old behavior: a never-finishing probe,
slow probes that consumed the catch-up budget, a server that accepted an RPC but
sent no response headers, and a server that stalled halfway through its response
body. They pass with the fix. Three additional controls cover immediate success,
transient error followed by catch-up, and a successful RPC returning no block.
The scheduling cases use Tokio's virtual clock; the transport cases use real
loopback HTTP fixtures.

The new 30-second request bound is an operational change: an otherwise valid
read that takes longer will now fail and enter recovery. Validate historical
scan latency and peak load on the intended production endpoint. This patch
covers `L1Reader` and the metadata catch-up wait; broader outage and crash recovery
still need the live acceptance matrix below.

**Fork compatibility checked.** Source comparison against fork `main` and review
of the changed runtime paths established the following limited conclusions:

| Area | Finding |
| --- | --- |
| Protocol pin and ABI | The pin is preserved and content-identical to the audited protocol main. Protocol implementation files were not replaced by the older upstream version. |
| Native blobs and block-hash commitments | Blob/payload codecs, protocol materialization, execution inspector, and observation/witness implementation remain unchanged by the backport. Proof-response validation uses the fork's hash construction and physical blobs. |
| Ingress and settlement provenance | Ingress and witness-source implementations remain unchanged. Extracted correlation RPC and payload-builder files are byte-identical to their former locations. |
| Grouped/nested calls and caught reverts | The fork's supported behavior is retained in the adapted scenarios. Caught reverts retain authenticated return bytes and roll back destination writes. |
| Persistent correlation recovery | Full same-L1-block tail restoration remains; transport failure does not establish an orphaned tail. The single-record checkpoint safeguard does not replace persistent history. |
| Roles and system identity | Composer/follower separation keeps system-sender filtering and mandatory key/address consistency. The follower now requires its reconstruction configuration; the dev-node binary was removed. |
| Retry recovery | One charge per failed proving/relay episode, request-bound actionable hints, sender/direction nonce cascades, reservation release, and FIFO requeue remain. Local construction/observation failures retain the fork policy. |

These checks support compatibility for the inspected paths. They do not replace
runtime validation for every combination of load, reorg, failure, and partial
settlement.

**Intentional behavior requiring operational support.** Three failed settlement
episodes can evict a valid held transaction, for example during repeated prover
unavailability. Its nonce-dependent suffix is also evicted. This is the upstream
liveness policy: admission is not a promise of eventual inclusion. Clients and
operators need receipt monitoring, visible eviction diagnostics, and ordered
resubmission. Startup configuration must also include the real prover endpoint,
attester, and each role's required system identity; configurations relying on
the removed mock/dev-node paths need migration.

**Validation in this review.** Before the correction, the unchanged backport
passed a fresh run of 767 Rust unit/vector tests and 170 Python CI tests (four
opt-in Python cases were skipped). A sandboxed attempt first failed local socket
creation; the unrestricted loopback-fixture run passed without retries.

After the correction, all 117 L1/deriver unit tests passed, including the seven
new cases. Rust 1.94 workspace/all-target Clippy with warnings denied passed;
formatting and whitespace checks passed. Rebuilt composer, follower, and signer
binaries then passed all three targeted native scenarios without retries:

- `happy_case_blob_l1_reorg_replaces_correlations`: 53.182 seconds.
- `happy_case_composer_sustained`: 59.774 seconds, including restart.
- `happy_case_follower_l1_derived`: 39.200 seconds.

[Validation evidence](production-review.json) records commands, source identities,
binary hashes, log hashes, and scope. Runtime tests used Rust 1.96.1 development
builds, with composer/follower optimization lowered to fit local memory. Release
acceptance still requires the exact final commit and release artifacts. The
other 41 native integration cases were not rerun with the corrected runtime.

The earlier [completion ledger](completion.json) remains historical evidence:
its 44 integration passes used the runtime before this follow-up fix. Recheck
that snapshot explicitly with:

```bash
python3 docs/audits/2026-09-11-upstream/verify_completion.py \
  --repo . --revision 401a2dc90961d3cbe020f9a6adfc11d012422845
```

The verifier's default current-revision check intentionally rejects runtime
changes beyond the frozen audit. It now also rejects uncommitted changes to
tracked code, instead of accidentally presenting old evidence as covering them.
Update the completion ledger after validating the final runtime; do not waive
the identity check or relabel historical test passes as current passes.

**Remaining release gates, in order.** The existing
[production checklist](../../../PRODUCTION_READINESS_TODO.md) contains unresolved
P0 blockers; the backport does not close them.

1. Validate the final code and configurations in the complete release CI:
   runtime artifacts, lint, contracts, embedded E2E, cross-chain E2E, full
   Kurtosis workload, and the aggregate `CI Result`. Retain commit, compiler,
   image, binary, log, and test identities together. Refresh the coverage ledger.
2. Close the missing correctness evidence: positive resumed execution with
   valid derivation sidecars, partial/deferred settlement, the complete live
   nested/static/revert/rollback semantic matrix and adversarial mutations,
   six-blob/gas-limit boundaries, competing submissions, and real-client reorgs.
   The synthetic root-only resumption test establishes rejection and recovery,
   not successful valid-sidecar resumption.
3. Resolve the trust-boundary requirements: verify signer/attester/verifier/vkey
   configuration against L1 during startup, define scheduling guarantees for
   immediate-entry counts, and authenticate the remaining L1-origin transaction
   metadata/boundaries/order against independently observed L1 transactions.
4. Establish durable historical blob storage and snapshot/bootstrap recovery;
   exercise pruning, missing data, process crashes, interrupted persistence,
   corrupted storage, fee spikes, retry/replacement, and complete restore drills.
5. Complete deployment hardening and public-testnet evidence: authenticated
   signer transport, production key custody, monitoring and resubmission,
   capacity/soak tests, and restart/resynchronization on the intended clients.
6. Obtain independent security review of commitments, reconstruction,
   materialization, settlement, and signer trust. Close findings, rerun affected
   acceptance gates, and approve a specific release artifact with a tested
   recovery/rollback procedure.

The current decision remains no-go for production until those gates have
evidence. This review found and corrected one availability defect; no new
settlement-corruption or fund-loss defect was demonstrated by the checks run.
