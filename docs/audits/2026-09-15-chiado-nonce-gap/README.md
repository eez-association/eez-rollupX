# Chiado cross-chain nonce-gap investigation

Historical investigation of the deployment and PR code at `71be1a5`, dated
2026-09-15. The [subsequent correction and deployment](../2026-09-15-cross-chain-queue-liveness/README.md)
are recorded separately. All live operations during this investigation were read-only. No node was
restarted, configuration changed, or transaction broadcast during this check.

## Finding

L2 production and L1 settlement continue, but cross-chain composition is stuck
on an outbound candidate with nonce **14,517**, while its sender's canonical
L2 nonce is **14,514**. The affected sender is
`0x6b930b92ec7311d568dd50dd9c0903a2baa08ce8`.

This is a transaction-processing liveness failure. Advancing safe blocks and
successful postBatch receipts do not establish that pending cross-chain users
are being processed.

Two mechanisms that sustain this failure were reproduced against the current
PR code:

1. After the source nonce decreases, admission still accepts higher nonces
   using the highest reservation. It rejects a missing lower predecessor that
   could repair the sequence.
2. A transient failure restores the same FIFO head. Automatic admission can
   remain at one candidate, preventing other senders from being considered.

The exact original event that left the predecessor transactions out of
executable order is **not established**. An eviction, incorrect recovery order,
or admission against a provisional/pending nonce must not be presented as the
proven initiating cause. The retained node logs begin after the incident, and
the held pool has no read-only inspection/export RPC. In particular, a null
transaction lookup does not prove that a transaction is absent from this
separate in-memory pool.

Further analysis produced a [concrete correction proposal](../../proposals/cross-chain-queue-liveness.md)
and additional regressions against the current code:

- Reconciliation across two attempt generations places a higher nonce ahead
  of an already queued predecessor: `[1, 0, 2]` instead of `[0, 1, 2]`.
- Explicit resubmission after local termination is acknowledged but can be
  removed by an old accounting callback. Replacing a cheaper queued transaction
  with that previously terminal signature has the same issue.
- The older unguarded outer-error recovery can recreate the observed missing
  prefix: terminal records subsequently remove 14,514–14,516, leaving 14,517.
  The current `requeue_composing` guard prevents this particular sequence.

A temporary pool prototype corrects first-hole admission, recovery order, and
local-terminal readmission. It passes 149 composer tests, including 1,440
recovery-order permutations in both directions. This does not establish which
path initiated the live incident, and does not implement the full production
fix. Source patches, baseline failures, prototype results, and identities are
preserved in `artifacts/chiado-nonce-investigation-20260915/deep-analysis/`.

The final comparison used:

```sh
# Original pool with six temporary diagnostic tests: 1 passed, 5 failed.
cargo +1.94.0 test -p eez-composer --lib investigation_ --offline --locked -j 3 -- --nocapture --test-threads=1

# Pool prototype with diagnostics: 149 passed.
# Original sources and tests restored: 143 passed.
cargo +1.94.0 test -p eez-composer --lib --offline --locked -j 3 -- --test-threads=1
```

The one passing baseline diagnostic verifies the difference between the older
unguarded restoration and the current guard; it does not assert that the older
path is correct. `deep-analysis/baseline-final-diagnostics.log`,
`prototype-v2-composer-tests.log`, `restored-baseline-tests.log`,
`verification-results.json`, and `prototype-v2-identity.json` record the
comparison. The prototype test executable has SHA-256
`033dd709c1eadb3d951cd52b5256a3033ee62f2f85dd7887a836fafd3c079419`.
These are composer-level experiments, not native integration validation.

## Verified timeline

- **2026-09-14 23:03:45 UTC:** canonical L2 block **89,635** executes sender
  nonces 14,512 and 14,513. Historical RPC queries locate this as the last
  canonical advance to nonce 14,514.
- Blockscout retains three pending transaction records for nonces
  **14,514–14,516**, first indexed at 23:03:38. Those transactions are absent
  from canonical transaction/receipt RPC lookups during the investigation.
- Nonce 14,514 appears in five retained orphan-block records:

  | L2 block | Block timestamp (UTC) | ECDSA validation time |
  | --- | --- | --- |
  | 89,640 | September 14, 23:03:50 | 13 ms |
  | 89,720 | September 14, 23:05:10 | 111 ms |
  | 89,730 | September 14, 23:05:20 | 12 ms |
  | 89,745 | September 14, 23:05:35 | 17 ms |
  | 89,770 | September 14, 23:06:00 | 27 ms |

  Each orphan hash matches a successful signer attestation. This establishes
  successful proof generation for those candidates; it does not establish why
  their L1 submission or settlement failed. These L2 branch replacements are
  not evidence of an Ethereum/Gnosis L1 reorg.
- **September 15, 00:09:10–05:14:04:** the preserved Docker log window contains
  **1,028** occurrences of `nonce 14517 too high, expected 14514`. All **1,047**
  held-pool drains selected one candidate. All **2,252** recorded canonical
  reconciliations have an empty source-outcome list.
- During that window, the same sender submitted **9,772** accepted outbound
  requests. The first and last recorded pool depths were **2,654** and
  **12,419**; these samples have different boundaries from the ingress count.
- **05:28:36 UTC:** L2 latest **112,725**, safe **112,720**, sender nonce still
  **14,514**. The 05:28:32 drain reports **12,905** queued entries and a selection
  limit of one.

The earlier health check independently verified three successful recent L1
postBatch receipts and approximately 26.50 test xDAI in the poster account.
Neither lack of L1 funding nor a stopped chain explains the observed nonce
validation error.

## Code paths and reproduction

| Mechanism | Implementation | Evidence |
| --- | --- | --- |
| Admission obtains a pending source nonce before acquiring the held-pool lock | `crates/eez-node/src/ingress.rs`, `gate_and_hold` | A provisional/pending observation can decrease before a later opportunity; the original incident's admission snapshot is unavailable |
| Expected admission nonce is one above the highest queued/in-flight reservation | `crates/eez-composer/src/held_pool.rs`, `next_expected_nonce`, `push_contiguous` | Real-pool offline reproduction accepts 14,518 after rollback to 14,514, then rejects resubmission of 14,514 with expected nonce 14,519 |
| Source `NonceTooHigh` is classified as unavailable for enabled policy | `crates/eez-composer/src/local/client.rs`, `simulate_source_tx_on_with_retries` | Matches the repeated live error |
| Transient composition abort restores the original FIFO order and takes the minimal posting path | `crates/eez-composer/src/composer.rs`, `compose_cross_chain_batch`, `requeue_unprocessed` | Reproduction selects the same blocked head 100 times; an unrelated sender is never selected |
| Incomplete preparation shrinks the next selection, with a minimum of one | `crates/eez-composer/src/composer/admission.rs`, `AutomaticAdmission::finish` | Live drains remain at one; this is a preparation selection limit, not the network's gas/blob capacity |

The diagnostic uses the actual `HeldPool` implementation, with synthetic
transaction identities and no network or signing keys. Its control case
restores retained predecessors in the correct order, showing that ordinary
FIFO recovery alone is not sufficient to reproduce the original loss of order.
It does **not** claim to reproduce the full live initiating sequence.

Command executed with a temporary example target:

```sh
cargo +1.94.0 run -p eez-composer --example nonce_gap_diagnostic --offline --locked -j 3
```

Output:

```text
higher_nonce_after_rollback=Ok(())
missing_predecessor_after_rollback=Err(Nonce { expected: 14519, on_chain: 14514 })
blocked_head_retries=100; unrelated_sender_selected=0; queue_len=3
control_recovered_nonce_order=[14514, 14515, 14516, 14517]
```

The diagnostic completed successfully because its assertions check that these
failure mechanisms exist. This is not a passing correctness regression. The
temporary example was removed after archiving its source and executable hash.
Existing held-pool tests (**15**) and optimistic-accounting tests (**18**) also
pass; they do not cover the reproduced rollback/admission/liveness combination.

## Runtime and evidence identities

- Current PR head tested: `71be1a5c1930d1adb440bc059672b89b63c5d399`.
- Live image: `eez-node:chiado-da-guard-20260914`, image digest
  `sha256:3dbf236f8a1e4c8e7f4557e219d4c868dd11bcdd63149d3c006e38e2bf4e3580`.
- Live container started September 14 at **21:11:11 UTC**, restart count zero
  at observation. It predates the PR's composition-error ownership correction.
- Archived live-build sources for held pool, composer, optimistic accounting,
  and automatic admission were recovered and matched against the build's
  recorded SHA-256 identities.
- The current PR's `requeue_composing` correction does not change either
  highest-reservation admission or transient FIFO retry. The diagnostic proves
  those mechanisms remain in the PR; it does not prove that deploying that
  correction would have prevented this live incident.

Local evidence is under `artifacts/chiado-nonce-investigation-20260915/`:

- `node-metadata.json`, `node-full-available.log`, `log-summary.json`,
  `queue-impact.json`, `drain-history.json`, `sender-ingress-history.json`.
- `last-nonce-advance.json`, `explorer-predecessor-forks.json`,
  `explorer-nearby-transactions.json`, `predecessor-rpc.json`.
- `signer-initial-incident.log`, `signed-orphaned-predecessors.json`.
- `nonce_gap_diagnostic.rs`, `offline-reproducer.log`,
  `offline-reproducer-identity.json`, `baseline-results.json`.
- `final-status.json`, `final-queue-events.json`.

The three predecessor envelopes were reconstructed from Blockscout's public
transaction/signature fields. Their hashes and recovered sender were verified
exactly and saved in `reconstructed-predecessors.json`. No private key was
needed and none of these envelopes was broadcast. Their hashes are:

| Nonce | Transaction hash |
| --- | --- |
| 14,514 | `0x395302ea18674518ceb0e7c095c885badf37ba3cb35c1d7e72bf89c8771ad200` |
| 14,515 | `0x587b3fd41046459cdc22712733d1816ff08f0bf2213d2a01c609cbbfa269cc44` |
| 14,516 | `0xeb4b8be87f56f49ccd6918852df3007cb639c4692047e6978cfeae62e89d6518` |

## Required correction and validation

Keep the PR in draft while addressing this finding. The smallest correction
must preserve existing source ownership, in-flight replacement restrictions,
direction-specific nonces, and atomic execution groups:

1. Reconcile admission and selection against a contiguous source nonce chain
   when provisional/pending state changes. A queue must not keep growing past
   a missing predecessor while refusing that predecessor's resubmission.
2. Distinguish a blocked sender from a slot-wide infrastructure failure. Retry
   or defer the blocked nonce chain without starving independent eligible
   senders or the opposite direction. Preserve the existing canonical
   outbound/inbound execution ordering and resource budgets.
3. Add deterministic regression coverage for admission racing rollback,
   predecessor recovery/resubmission, attempt exhaustion, partial settlement,
   repeated transient failures, and progress by an independent sender. Include
   a component/native test that creates the rollback instead of assuming its
   resulting nonce state.
4. Expose queue depth, oldest pending age, blocked nonce/reason, and last
   successful cross-chain completion in operational health. Advancing chain
   heads alone must not conceal a stalled cross-chain queue.
5. Retain enough structured attempt/disposition evidence to identify the
   initiating event in a subsequent reproduction. This does not require a
   durable queue or a new restart-recovery protocol.

No gas/blob cap increase addresses a source nonce mismatch. Empty/minimal
posts may still be legitimate for pending pure L2 traffic or the general
consolidation rule; blocked cross-chain work alone must not manufacture posts
that cannot make it executable.
