# Fee ceiling and automatic cross-chain admission

## Scope and rollout status

Requested on 2026-09-14: raise the active Chiado execution-price ceiling to
500 gwei, then replace the fixed count of 12 cross-chain users per bundle with
automatic packing based on available resources.

Both requested settings are active on Chiado: the 500 gwei ceiling was deployed
first, and the final automatic-admission build started at 16:52:05 UTC. Accepted
work was drained before restarting, eight canonical anchors were checked across
the restart, and public submissions resumed at 16:52:12 UTC. The final executable
passes all 17 native integration cases and a fresh 64-call live Chiado wave.
The fresh full protocol run passes 28 of 28 cases, with independent receipt,
cross-chain execution, and authenticated semantic-blob checks complete. A final
read-only check verifies successful canonical receipts and settlement accounting
for all 305 sources accepted through the end of that run, including background
traffic. No accepted source in that interval is missing settlement.

An earlier candidate passed its controlled wave but failed one of 28 live
protocol cases. It was rolled back at 15:44 UTC, retaining the 500 gwei ceiling.
That failure exposed an orphaned ordinary L2 predecessor and eviction of its
dependent cross-chain source for a future nonce. The final candidate fixes both
recovery paths and adds three native regressions; the failed run remains in the
evidence and is not counted as passing validation.

All task evidence is under `artifacts/dynamic-admission-20260914/`. Final-build,
native, deployment, and fresh live evidence is under its `v2/` directory.

The operator-facing configuration and semantics are documented in
[configurable-settlement-policy.md](../../configurable-settlement-policy.md).

## Behavior and compatibility

`EEZ_L1_POSTBATCH_MAX_FEE_PER_GAS=500000000000` is a ceiling in native base units
per gas. Network fee estimation, the existing priority bounds/margin, blob fee
ceiling, and actual receipt-based charging retain their existing behavior.
It is not a fixed payment of 500 gwei and does not alter signed user fees.

`EEZ_MAX_USER_TXS_PER_BUNDLE=auto` removes the fixed transaction-count ceiling
when the settlement policy is enabled. A per-rollup estimator selects a finite
FIFO prefix using the previous preparation measurement and the existing proof
deadline. It begins with one group after restart, grows at most twice the last
measured size, and reserves time for final construction, effect-prefix
commitments, encoding, KZG and witnesses. These are admission estimates; the
existing proof and submission guards remain authoritative.

Each complete group must fit L2 gas, the resolved L1 post-transaction and whole
bundle gas budgets, and the target network's remaining blob allowance, including
companion blobs. The incremental DA estimate includes actual transaction bytes,
metadata, semantic messages and the current RLP integer-list representation.
Only unknown header fields and framing are rounded up. Exact final encoding
and ordered bundle simulation remain in place.

Unselected and capacity/time-deferred groups retain their ownership and nonce
ordering without a failed-attempt charge. Cuts reduce subsequent selection so
canonical outbound-first execution cannot continually bypass an older inbound.
An excluded or partially consumed submission reduces subsequent selection after
canonical reconciliation; existing source dispositions decide what retries.
A relay error alone is never treated as evidence that no transaction executed.

Numeric values retain the previous fixed-count behavior. The absent value
remains 50, and the general Chiado Compose default remains 12; this deployment
explicitly opts into `auto`. Disabled-policy `auto` is rejected at startup.
Ordinary L2 block construction, settlement timing, wire formats, contracts,
signer validation, and canonical derivation rules retain their existing behavior.
The recovery corrections are described below.

## Requirement coverage

| Requirement | Implementation | Automated evidence |
| --- | --- | --- |
| Raise the live fee ceiling first | Private runtime environment; fee-only node restart | `fee-500-config-validation.json`, `fee-500-rollout.json`, preserved L1/L2 anchors in `chain-before.json` / `chain-after.json` |
| Automatic mode reaches runtime; invalid combinations fail early | `composer/config.rs`, `compose_sync_slot`, existing Compose forwarding | Configuration unit tests; `scripts/check-settlement-policy-config.py` |
| Preserve numeric and default behavior | `ComposerSettings::parse`, fixed `pop_n` branch | Default/numeric/invalid configuration tests; existing settlement and gas integration tests |
| Select using measured preparation time and existing deadlines | `composer/admission.rs::AutomaticAdmission`, shared `proof_start_cutoff_ms` | Growth beyond 50, zero/unknown target, smaller time budget, cutoff reserve and restart unit tests |
| Check real DA without altering the wire format | `BlobAdmission`, `extend_blob_admission`, `remaining_posting_blob_allowance` | Actual codec comparisons at RLP boundaries, typed/legacy transactions, multiple blobs, companion allowance and rejected-candidate tests |
| Preserve whole groups and ownership on local capacity/time cuts | Both cross-chain composition phases and existing held-pool paths | Blob-cut native case; mixed native cases assert no attempt charge for local deferral |
| Back off after canonical exclusion/partial consumption | `reconcile_attempt` and `submission_failed` | Submission-backoff unit checks; excluded-bundle native case verifies actual smaller retry and all source outcomes |
| Complete mixed traffic beyond the old count | Existing composition, final gas/blob preflight, submission, reconciliation | 64 inbound + 64 outbound native calls per Ethereum/Gnosis profile; exact source nonce advances, successful receipts and independent follower state |
| Retain L1 transaction and whole-bundle gas checks | Existing gas resolver and per-group budget, committed only on acceptance | Four existing native gas cases across Ethereum/Gnosis |
| Preserve same-block partial-consumption behavior and restart semantics | Existing reconciliation and historical policy paths | Existing partial-consumption and settlement-policy regression cases |
| Safe deployment without losing accepted work | Pause new ingress, wait for a demonstrably empty held pool and canonical reconciliation, restart, verify anchors, resume | `v2/upgrade/activate-auto.py`, `v2/auto-rollout.json`, `v2/live-wave.json` |
| Keep future nonces retryable | Enabled source simulation returns `Unavailable` for `NonceTooHigh` | `enabled_future_nonce_waits_for_its_predecessor_without_changing_legacy`; regression fails on the previous implementation |
| Recover ordinary transactions after an excluded optimistic Sync | `reconcile_attempt` commits the replacement branch directly | `automatic_admission_recovers_an_orphaned_ordinary_l2_predecessor`; previous runtime reproduces the live failure |
| Preserve descendants when an excluded static-only attempt already has an empty Sync | Compare replacement hash before moving the head | `automatic_admission_static_only_exclusion_preserves_ordinary_descendants` |
| Prevent reinserted cross-chain sources entering ordinary sequencing | Existing held pool/attempt ledger queried by `EezPoolBuilder`'s validation gate | Pool gate unit test, classification-through-finality unit test, outbound reorg native case |
| Preserve protocol execution and native DA on the real Chiado relay | Existing contracts, signer, preflight, submission and derivation | Fresh pinned protocol suite: 28/28; receipt, causal and semantic-blob evidence complete |
| Verify deferred work actually completes | Existing retained ownership and canonical retry paths | `v2/final-observation.json`: all 305 accepted sources through suite completion have successful canonical receipts and settlement accounting |

## Reorg defect found by live protocol validation

The first automatic runtime passed the controlled live wave. The full 28-case
protocol workload then exposed a failure in `nestedStaticCounterL2`: its ordinary
L2 proxy deployment at sender nonce 2 lost its canonical receipt, and its
dependent cross-chain call at nonce 3 was evicted as a deterministic simulation
failure (`nonce 3 too high, expected 2`). The 27 successful protocol cases do
not make that run a passing validation.

The isolated regression reproduces this with a held submission, an ordinary
transaction included in a later Live block, and a dependent cross-chain source.
Only after observing the ordinary receipt does the test reject the submission.
The previous runtime loses the predecessor and evicts the dependent source.
Evidence is retained in `nonce-recovery/baseline-logs/` and the failing test log.

The composer previously sent a bare ancestor rewind before constructing its
empty replacement Sync. In pinned Reth `fd59fd2`,
`apply_canonical_ancestor_via_reorg` updates in-memory state but does not call
`notify_canon_state`; normal `on_canonical_chain_update` does. Consequently pool
maintenance never sees the removed transactions. Constructing the replacement
against its explicit parent and switching directly to that branch takes the
normal reorg path, including removed persisted blocks.
If the proposed replacement already equals the original empty Sync, recovery
leaves the head and its ordinary descendants in place. This covers static-only
inbound attempts that have no L2 execution to remove.

That also reinjects source transactions from the old Sync. They must remain
under composer control: executing them without their system transactions in an
ordinary block could revert and consume their nonce. The pool's existing
validation gate now queries the same held pool and unfinalized attempt ledger.
This includes resolved sources until finality so asynchronous pool reinjection
cannot race an ownership transfer. Followers keep their existing pool behavior.
No second ownership registry or transaction queue is introduced.

Future nonces are separately classified as temporarily unavailable when the
settlement policy is enabled, for both numeric and automatic admission.
Disabled-policy nonce handling and actual EVM revert handling retain their
existing semantics. The new regression tests cover both inbound
and outbound excluded Syncs, the static-only case, recovery without resubmission,
exact nonce advances, successful receipts, and independent follower convergence.

This correction targets optimistic submission recovery. The separate bare
ancestor retreats used by the deriver for startup/L1 reorgs should receive their
own pool-notification audit; they are not changed or claimed as validated by
these admission tests.

## Native fixture defects found by the larger workload

The first stress run reached 31 users in a post and uncovered two fixture
limitations. The private native bundle endpoint inserts users through reth's
public pool, whose default per-account limit of 16 rejected a larger nonce
chain after accepting part of it. The blob-capture stub recorded data only on a
successful RPC response, so an accepted post in that prefix could lack fixture
DA when a later companion was rejected.

Testing and TestingGnosis now remove that per-account spam limit while retaining
global pool and actual block gas bounds. Real Chiado and private PoS profiles
retain their pool configuration. The capture stub retains submitted blob
carriers before forwarding; its beacon fixture only serves data with canonical
receipts. A Python regression covers both an error after partial acceptance and
a lost response. Production beacon fetching and blob decoding are unchanged.

The failed diagnostic run is preserved under `first-pass/`; it must not be
confused with validation of the corrected runtime. The native test fixture
remains non-atomic, so live Chiado relay testing is also required.

## Validation and limits

Completed checks: 137 composer and three pool unit tests; 324 related
L1/codec/protocol unit tests; nine builder fixture tests; 26 native artifact
checks (one skipped); four Compose/configuration forwarding checks. Formatting
and whitespace checks pass. Clippy passes with the pre-existing Rust 1.96
`clippy::duration_suboptimal_units` warning allowed; the strict invocation is
also recorded, rather than changing unrelated duration code.

All 17 native integration cases pass against the frozen runtime: four automatic
admission workloads, three recovery regressions, four gas-limit cases, three
partial-consumption cases (including restart), and three settlement-policy cases.
The image is built from that exact executable
(`c2b9e213c1793fef55ace971d39b058933bef13bedbc17bb20db3112cd5e3c78`),
with unchanged signer and follower binaries. Cargo's integration build also
produces a normal node executable through feature unification; the candidate
image was rebuilt from that frozen, tested artifact rather than the earlier
standalone build. `v2/image-identity.json` records the verified match.

Deployed image: `eez-node:chiado-auto-admission-20260914-v2`, image ID
`sha256:c26e4476a0fe74b3a0aaacca738fd53cfa2dac35edfdf6b4cd5cebcafde84c83`.
The source hashes, test executable hashes, and normal (non-test) composer build
artifact identity are recorded in `v2/runtime-identities.json` and the Cargo
JSON build manifest. Deployment verified these identities before changing the
runtime configuration. Signer and follower executables remain unchanged.

### Observed packing and live receipts

| Workload | Successful source calls | Canonical post sizes (users) | Largest post |
| --- | ---: | --- | ---: |
| Native Ethereum, 64 inbound + 64 outbound | 128 | 1, 2, 4, 7, 14, 28, 31, 25, 16 | 31 |
| Native Gnosis, 64 inbound + 64 outbound | 128 | 1, 2, 4, 8, 16, 32, 52, 13 | 52 |
| Live Chiado, 32 inbound + 32 outbound, alongside existing background traffic | 64 test calls | 1, 2, 4, 8, 12, 23, 28, 20, 10 in the observation window | 28 |

The native tests verify successful source receipts, exact nonce advances,
expected effects, and independent follower convergence. Canonical post sizes
join the dispatched intended Sync hash to the deriver's L1-confirmed safe
advance. The Ethereum test exits before the final asynchronous composer
reconciliation log, so that last post is identified by its canonical safe
advance, not by an assumed dispatch success. The measurement collector records
which composer reconciliation callbacks were observed.

The live wave started at 16:52:23 UTC. All 64 sources have canonical receipts and
settlement accounting; inbound receipts contain `ExecutionConsumed`, both
source nonce chains advance exactly 32, and both target values are 91032, with
the L2 value checked at the safe head. `v2/live-wave.json` and
`v2/live-admission-summary.json` record all hashes and joins. Live post counts
also include unrelated background traffic; they must not be interpreted as
nine posts needed solely for the 64-call wave.

The 28-user post is
`0xe9884d5db09bc89e76550616043026d976b3c91d80465c8b70a4d181d601ee06`
in Chiado block 23055770. Its receipt uses 2,165,047 execution gas and one blob
(131,072 blob gas), with an effective execution price of 8.916100467 gwei. The
500 gwei setting is a ceiling, not that receipt's price.

The observation window contains one excluded one-user submission immediately
after startup, before the test wave began. Canonical reconciliation kept its
source retryable, retained the minimum selection of one, and subsequent posts
completed it. There were no logged runtime errors in the wave window. Relay
inclusion is still not guaranteed for every proposed opportunity.

The public UI and monitor health endpoints returned HTTP 200 after the rollout
(`v2/public-health.json`).

### Full protocol and closing checks

The unchanged protocol suite at pinned commit
`3c0cdf4955941a5c23639346932b55b33babebbd` ran all 28 scenarios in parallel:
28 passed, none failed, none skipped. Runtime was 217.5 seconds; independent
verification took a further 22.695 seconds. Its accounting has no missing,
duplicate, or unexpected scenario. Receipt/table evidence, causal execution
coverage, KZG-authenticated blob DA, and decoded native semantics are complete
for every scenario, including `nestedStaticCounterL2`.

The local protocol summary at
`artifacts/chiado-validation-20260913/protocol-auto-admission-v2-20260914/protocol-e2e/summary.json`
and `v2/protocol-result.json` record the final passing result. Full node and
signer logs were captured throughout, before any later container replacement
could remove them.

This does not mean the stress run had no transient errors. From activation
through protocol completion, the node logged three proof-attempt deadline
errors, three skipped-slot and two excluded-bundle observations, and one unsent
preflight deferral after its target/window changed. The signer logged no errors.
Two unrelated, underfunded ingress attempts were rejected before acceptance.
Reth's system-transaction reinjection warnings show its validation gate refusing
system transactions from orphaned blocks as intended.

The closing check joins every accepted source through 16:57:54 UTC to canonical
settlement and independently re-reads its successful source receipt and block
hash: **305 of 305**, with none missing. This covers background work as well as
the controlled wave and protocol suite, including retained work from the
deadline/retry paths. The local `v2/final-observation.json`
also verifies the deployed image, both requested settings, source hashes,
zero container restarts since activation, advancing L1/L2 heads, and HTTP 200
from the public UI and monitor.

## Changed components

- `composer/admission.rs`: adaptive finite FIFO selection, preparation timing,
  submission backoff, and incremental encoded-DA bounds.
- `composer/config.rs`: startup parsing and validation of numeric/automatic
  admission while preserving defaults.
- `composer.rs`: integrate admission with existing composition and resource
  checks; replace failed optimistic branches without losing ordinary work.
- `composer/policy.rs` and `prover_retry.rs`: expose existing blob allowance and
  deadline calculations to admission without duplicating them.
- `local/client.rs`: keep enabled-policy future nonces retryable; preserve
  legacy simulation and actual revert handling.
- `held_pool.rs`, `optimistic.rs`, `eez-node-common/src/pool.rs`, and
  `eez-node/src/lib.rs`: share existing source classification with the ordinary
  pool's validation gate; test classification through canonical finality.
- `eez-node/src/l1_embedded.rs`, `scripts/builder-stub.py`, and its Python tests:
  correct the private native fixture's account-slot and DA-capture limitations.
- `eez-node-e2e/tests/dynamic_admission.rs`: seven native cases covering both
  network profiles, capacity/time admission, exclusion, and recovery.
- `.env*.example`, `docs/configurable-settlement-policy.md`, and
  `scripts/check-settlement-policy-config.py`: operator configuration,
  compatibility, and forwarding checks.
- `scripts/ci/native-tests.py` and `scripts/ci/tests/test_native_artifacts.py`:
  include the new integration target in the existing native CI inventory.
- This audit and task artifacts: requirement mapping, failed diagnostics,
  runtime identities, rollout records, and live receipt evidence.

No test can guarantee inclusion in every future L1 opportunity. Network delays,
opaque relay restrictions, competing block traffic and unusually expensive
transactions can still require retries. Automatic admission does not promise a
mathematically maximal pack, raise network limits, or change the existing retry
and oversized-single-group rules. Measurements are in memory and relearn after
restart. The existing fee-estimator feedback behavior is outside this change.

Commands and detailed outputs are recorded in the artifact directory:

```text
cargo test --locked --offline -p eez-composer --lib
cargo test --locked --offline -p eez-node-common --lib
cargo test --locked --offline -p eez-l1 -p eez-blob-codec -p eez-payload-codec -p eez-protocol --lib
cargo clippy --locked --offline -p eez-composer -p eez-node-common -p eez-node -p eez-node-e2e --lib --tests -- -D warnings -A clippy::duration_suboptimal_units
python3 -m unittest discover -s scripts/ci/tests -p test_builder_stub.py
python3 -m unittest discover -s scripts/ci/tests -p test_native_artifacts.py
python3 scripts/check-settlement-policy-config.py
cargo fmt --all --check
git diff --check
```

The final native runner uses frozen binaries and checks their hashes, rather
than rebuilding a different executable between suites:

```text
python3 artifacts/dynamic-admission-20260914/v2/recovery-checks.py
python3 artifacts/dynamic-admission-20260914/v2/validate-followups.py
node artifacts/dynamic-admission-20260914/v2/live-wave.cjs
python3 artifacts/dynamic-admission-20260914/v2/collect-live.py
python3 artifacts/dynamic-admission-20260914/v2/run-live-protocol.py
python3 artifacts/dynamic-admission-20260914/v2/final-observation.py
```

The live runners submit transactions and deliberately refuse to overwrite their
existing run evidence. They are records of this validation, not instructions to
repeat the same funded workload.

Local TCP tests require the sandbox's local-network permission. The initial
sandbox-restricted unit run is retained separately; the permitted rerun passed.
