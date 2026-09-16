# Settlement targeting and reconciliation latency — 2026-09-14

This document records the historical validation run. Detailed run artifacts are
preserved locally under `artifacts/audit-evidence/2026-09-14-settlement-targeting/`;
generated logs, deployment snapshots, and screenshots are not part of the source distribution.

The later [ordinary-sequencing budget](../2026-09-14-sequencing-da-budget/README.md)
implements prevention for the oversized-interval incident described here. This
report retains the timing fix's original observations and recovery evidence.

**Status: timing fix deployed; consecutive historical posts verified on Chiado.**
After approval, the oversized data-wave stall was recovered from safe block
39500. The original database and all 2545 unsafe signed transactions were
preserved. The recovery drain authenticated 2280 L2 blocks in seven two-blob
posts, including two consecutive-block pairs. All 2488 compatible originals
were replayed and settled with matching status, gas and logs; 57 nonce conflicts
from one sender remain preserved as signed data. One missing gas-funding
transfer was compensated explicitly. Original runtime settings were restored.
See the final outcome (`artifacts/audit-evidence/2026-09-14-settlement-targeting/recovery-outcome.json`, local evidence),
[cause and recovery analysis](incident-recovery.md) and
authenticated live drain (`artifacts/audit-evidence/2026-09-14-settlement-targeting/recovery-post-verification.json`, local evidence).
The complete recovery data audit (`artifacts/audit-evidence/2026-09-14-settlement-targeting/recovery-all-post-verification.json`, local evidence) verified
193 posts, 200 blobs and 4310 contiguous L2 blocks, including every restored
transaction hash. The final runtime check (`artifacts/audit-evidence/2026-09-14-settlement-targeting/final-runtime-check.json`, local evidence) passed.

This audit covers the change requested after the Chiado consolidation posted in
L1 blocks 23044715, 23044717, 23044719, 23044723, 23044727 and 23044729.
The prior measurement remains in the
[Chiado deployment audit](../2026-09-13-chiado-settlement-policy/README.md).

## Implementation

Policy historical submission previously chose `latest + 2`. It now chooses the
earliest future L1 slot with strictly enough time for the configured proof
budget and submission slack, sampling the clock after the head RPC. At an exact
deadline it selects the following slot. Missing heads and arithmetic overflow
return errors; timestamp pins, canonical-parent checks, final fee validation,
nonce ownership and the one-outstanding-attempt gate remain in force.

The embedded Ethereum/Gnosis node also wakes the existing L1 watcher on canonical
changes when settlement policy is enabled. Notifications are hints: the same
single scanner still reads canonical blocks, verifies blob data and emits
ordered events before derivation and reconciliation release ownership.
Notifications coalesce; lagged or closed subscriptions retain the two-second
periodic fallback. Finality refreshes count only periodic ticks, preserving the
existing refresh cadence.

No new configuration, codec, contract, proof format or fee parameter is added.
The policy-disabled legacy path retains its existing targeting and polling.
Explicit Sync targets are unchanged. Both supported embedded L1 implementations
are wired to the notification path.

| Changed component | Purpose |
| --- | --- |
| `crates/eez-l1/src/submitter.rs` | Authoritative earliest-feasible target selection, checked arithmetic, timing diagnostics and four deterministic regression tests |
| `crates/eez-composer/src/composer.rs` | Pass the configured proof budget at both historical target-selection call sites |
| `crates/eez-composer/src/composer/policy.rs` | Apply the same selection during startup capability validation |
| `crates/eez-l1/src/l1_watcher.rs` | Coalesced canonical-change wakeups, independent periodic finality cadence and three deterministic regression tests |
| `crates/eez-node/src/lib.rs` and `Cargo.toml` | Subscribe to the existing embedded provider notifications on both chains; make the existing provider dependency available to production code |
| `crates/eez-node-e2e/tests/settlement_policy.rs` | Correct fault-test setup to await settled history, because arbitrary newer empty blocks need not settle under `always` |
| `docs/configurable-settlement-policy.md` | Explain targeting, wakeups, compatibility and retained safety gates |
| `crates/eez-composer/src/settlement_policy/index.rs` | Add a deterministic regression showing why individually fitting blocks can exceed the first valid Sync prefix's blob budget; production logic unchanged |

## Coverage

| Required behavior / invariant | Automated evidence |
| --- | --- |
| Use the immediately next slot when preparation fits, on five- and twelve-second profiles | `policy_target_uses_the_first_slot_with_a_complete_preparation_budget`; `policy_drain_can_prepare_and_submit_in_consecutive_l1_blocks` follows selection through proof preparation and the real dispatch gate for six successive posts |
| Exact deadline, late preparation, stale heads and skipped slots cannot silently alter a proof target | Boundary tables in the first test; `selected_policy_target_still_rejects_late_preparation_and_skipped_slots` |
| Missing canonical head and overflow fail without fallback | `policy_target_missing_head_or_overflow_fails_without_a_fallback` |
| A notification wakes scanning without itself advancing canonical state | `canonical_notification_wakes_scan_before_periodic_deadline` |
| Coalescing, lagged/closed subscriptions and periodic recovery remain safe | The preceding test and `lagged_and_closed_notifications_preserve_scan_and_timer_fallback` |
| Extra scans neither duplicate unchanged heads nor accelerate finality refreshes | `notification_scans_do_not_accelerate_finality_refresh_or_duplicate_heads`; existing watcher reorg and failed-scan retry regressions |
| Canonical reconciliation, complete due ranges and retry ownership survive integration | Native tests for capacity drains, late historical work, dropped bundles, fee RPC and prover failures, restart, and independent follower replay |
| Multiple settlement events and partial consumption within one L1 block preserve order and accounting | `policy_partial_prefix_keeps_unconsumed_users_and_receipt_costs`; `policy_valid_blob_batch_resumes_at_middle_group_in_same_l1_block` |
| Both execution profiles and multi-blob-first behavior remain valid | Native Ethereum and Gnosis policy tests, including a fitting range above 300 blocks and a Prague batch with more than six blobs |
| The byte index detects a combined interval that exceeds capacity even though each block fits | `individually_fitting_blocks_can_exceed_capacity_before_the_first_sync_boundary`; exact reconstruction of preserved Chiado blocks 39501–39510 |

## Findings during validation

The initial live update removed the fixed offset but retained periodic watcher
polling. Its authenticated posts landed at 23050326, 23050328, 23050330 and
23050332: **no speed improvement was demonstrated by that run**. They covered
1210 contiguous L2 blocks, 35776–36985, in eight blobs.

The first post's L1 timestamp was 08:25:50 UTC. The watcher fetched it at
08:25:52.576 and the composer reconciled at 08:25:53.029. The next slot's proof
cutoff was 08:25:52.800 with the existing 1500 ms proof budget and 700 ms slack.
Choosing the later slot was therefore correct. This exposed polling latency as
the second avoidable delay and motivated canonical-change wakeups.

The first native suite with wakeups passed nine cases; the prover-fault case
timed out in setup, before fault injection. Both nodes agreed on safe block 55,
while empty unsafe blocks advanced to 240. The test had required an arbitrary
`latest` block to become safe under `always`. Setup now requires convergence on
the current safe prefix. All fault, no-post, no-split, reverted-receipt, replay
and eventual-single-settlement assertions are retained. The failed run is kept
alongside the corrected test result.

The unrestricted unit run passed 252 tests: 127 composer and 125 L1. The final
watcher-only run passed all 19 tests after a test-only duration-style cleanup.
Formatting and whitespace checks passed. Strict Clippy initially stopped at an
existing `Duration::from_secs(60)` in `eez-deriver/src/l1_catch_up.rs`; the same
check passed with only `clippy::duration_suboptimal_units` excluded. That
unrelated source file was not changed by this fix.

The corrected prover test passed in 92.49 seconds. Across the initial suite,
that corrected case and the partial-consumption suite, all 12 selected distinct
native cases passed. The original setup failure is not counted as a passing
run. See native validation (`artifacts/audit-evidence/2026-09-14-settlement-targeting/native-v2-validation.json`, local evidence),
initial suite (`artifacts/audit-evidence/2026-09-14-settlement-targeting/native-v2-tests.log`, local evidence), corrected case (`artifacts/audit-evidence/2026-09-14-settlement-targeting/native-v2-corrected-tests.log`, local evidence)
and partial-consumption tests (`artifacts/audit-evidence/2026-09-14-settlement-targeting/partial-v2-tests.log`, local evidence).

## Deployment and initial live failure

The final runtime update completed at 09:02:27 UTC and preserved canonical L1
and safe L2 history. Settings remain `always`, 1800 L2 blocks, 1500 ms proof
budget, 700 ms slack and 7500 ms builder lead. The exact image is
`eez-node:chiado-next-slot-20260914-v2`, ID
`sha256:733bed3c3f860f803afe99f643c20528d14331f90091eca6b1f64129c42a00c0`.
The image's composer hash is
`7a2535d840f03b1c7a3639779d89b4d4cc45f2c619580d56bae87ab5e83430f9`;
signer and follower were unchanged. See runtime identities (`artifacts/audit-evidence/2026-09-14-settlement-targeting/runtime-v2-build.json`, local evidence)
and upgrade (`artifacts/audit-evidence/2026-09-14-settlement-targeting/upgrade-v2.json`, local evidence).

Public HTTPS, both RPC fronts, chain IDs, UI bindings and monitor configuration
passed after the upgrade, **before the data-wave stall**. That health snapshot
must not be interpreted as current settlement health.

Other cross-chain traffic prevented an isolated large empty backlog. The next
probe therefore sent four ordinary EOA calls, each with 100164 calldata bytes,
1 wei value, a 1500000 gas limit and a 2 gwei maximum gas price. They entered L2
blocks 39501, 39502, 39503 and 39506. The preceding empty history settled through
39500, but the first three data calls together exceed two blobs before the next
empty, scheduler-aligned terminal at 39505. No valid progressing prefix can fit.
The runtime correctly reports `unpostable_range`; it does not bypass the network
limit or generate invalid attestations. The live verification timed out.

This probe was incorrectly sized for the existing boundary constraint, which
is explicit in proposal section 6.2. It exposed the operational consequence of
accepting more data between valid boundaries than the network can post. The
timing change does not create or remove this constraint. An admission budget
across the full Sync interval, or a separately validated protocol change to
settlement boundaries, is needed to prevent this case generally. Neither is
silently introduced in this timing fix.

See failed live probe (`artifacts/audit-evidence/2026-09-14-settlement-targeting/chiado-v2-data-drain.json`, local evidence),
probe log (`artifacts/audit-evidence/2026-09-14-settlement-targeting/chiado-v2-data-drain.log`, local evidence) and
target and reconciliation evidence (`artifacts/audit-evidence/2026-09-14-settlement-targeting/chiado-v2-target-events.json`, local evidence).
These logs show next-block target selection, not a successful consecutive-post
measurement. Inclusion also remains subject to preparation latency, canonical
data availability, skipped slots and builder selection.

## Recovery preparation and approved cutover

A consistent private backup of the stopped L2 database was taken; the original
node was restarted on its original data. No L1 data or settled L2 history was
rewound. An isolated copy was successfully unwound with the matching pinned
Ethereum CLI to L2 block 39500. Its retained block hash and state root match the
L1-validated safe endpoint:

- Block hash: `0x9dae584e8eedab8715cddffcc5cea1c439564234598abaa4e4bb42f119d96685`
- State root: `0xc58a8c7e5d8e3b716dbad7344cbd59460696a16f58df44e627d2ab95ee7ec22d`

The transaction capture through block 39835 contains 52 signed transactions,
including the four probes and other traffic. Every reconstructed signed
envelope hashes to its original transaction hash. A later stopped database
backup includes history through 39970. Any approved cutover must refresh this
capture for transactions arriving subsequently, preserve ordering and pending
ownership, retain the original database, and resubmit the signed transactions
with sufficient settlement boundaries. Their provisional block hashes would
change. This initial plan was subsequently executed using a fresh stopped
snapshot through 41705, containing 2545 signed transactions. The copy was
promoted at 09:44:56 UTC after approval; the node resumed and settlement
advanced. The four probes were resubmitted unchanged and settled separately.
See the initial recovery plan (`artifacts/audit-evidence/2026-09-14-settlement-targeting/recovery-plan.json`, local evidence),
approved recovery (`artifacts/audit-evidence/2026-09-14-settlement-targeting/approved-recovery.json`, local evidence),
[incident and replay analysis](incident-recovery.md) and
copy unwind (`artifacts/audit-evidence/2026-09-14-settlement-targeting/recovery-candidate-ethereum-unwind.log`, local evidence).

The production wrapper injects node-only flags into utility commands, so its
`stage unwind` invocation failed before touching the copy. The available
Gnosis CLI also rejected this Ethereum L2 genesis. A small artifact-only helper
uses the already-built pinned Ethereum CLI and refuses any datadir whose name
is not `data-wave-recovery-candidate`; only that helper performed the successful
copy unwind. These utility findings do not change production code.

The recovery drain's authenticated L1 sequence was
23051176, 23051178, 23051179, 23051181, 23051183, 23051185, 23051186:
gaps **2, 1, 2, 2, 2, 1**. All seven posts used two blobs. KZG verification,
every covered canonical L2 hash, receipts and the historical registry endpoint
matched. This demonstrates removal of the imposed gap; it does not guarantee
builder inclusion or timely canonical data on every slot.

The eight prefix-index tests passed, including the new interval regression.
An additional existing native no-prefix/recovery case passed in 97.99 seconds,
including independent follower replay. That brings the selected distinct
native cases that passed with these runtime binaries to 13. See
prefix tests (`artifacts/audit-evidence/2026-09-14-settlement-targeting/prefix-index-regressions.log`, local evidence),
native recovery test (`artifacts/audit-evidence/2026-09-14-settlement-targeting/atomic-recovery-native.log`, local evidence) and
its executable identity (`artifacts/audit-evidence/2026-09-14-settlement-targeting/atomic-recovery-native.json`, local evidence).
Formatting, whitespace checks and Clippy including the new test passed; the
Clippy run retained only the previously documented duration-style exclusion.

## Validation commands and review scope

```text
cargo fmt --all -- --check
git diff --check
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p eez-composer --lib settlement_policy::index::tests
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p eez-composer -p eez-l1 --lib --features eez-l1/chain-spec
CARGO_BUILD_JOBS=2 cargo test --locked --offline -p eez-l1 --lib --features chain-spec l1_watcher::tests
CARGO_BUILD_JOBS=2 cargo clippy --locked --offline -p eez-l1 -p eez-composer -p eez-node --lib --features eez-l1/chain-spec -- -D warnings -A clippy::duration_suboptimal_units
```

The loopback-dependent unit suite required unrestricted local sockets. Cargo
builds were locked and offline. Native cases used the frozen composer, existing
signer and existing follower; their exact commands and test-binary hashes are
in the validation JSON. Production runtime and integration-target builds both
succeeded. No contract or encoding changes were required by the timing fix.

The workspace already contained extensive unrelated work. The
scoped implementation diff (`artifacts/audit-evidence/2026-09-14-settlement-targeting/implementation.diff`, local evidence) compares the eight affected
files to copies taken before this task; the later
interval regression diff (`artifacts/audit-evidence/2026-09-14-settlement-targeting/atomic-interval-regression.diff`, local evidence) records the test-only
addition separately. Source identities (`artifacts/audit-evidence/2026-09-14-settlement-targeting/source-identities.json`, local evidence) record those
before/after hashes. The raw local evidence remains under
`artifacts/settlement-target-20260914/`; curated public artifacts here have
checksums (`artifacts/audit-evidence/2026-09-14-settlement-targeting/SHA256SUMS`, local evidence). Private database backups and keys are not published.
