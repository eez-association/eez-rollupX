# Network-derived ordinary sequencing budget — 2026-09-14

Status: implemented, validated, and deployed to Chiado during the separately
authorized [recovery](../2026-09-14-chiado-capacity-recovery/README.md). The live
network, public monitor, and explorers are operating again.

The [incident](../2026-09-14-chiado-blob-capacity-stall/README.md) showed that
checking individual blocks and final batches was insufficient: ordinary data
could exceed Chiado's allowance before the first legal historical terminal.
The user initially requested prevention based on the receiving network, then
separately authorized restoring the existing network and resubmission of its
volatile pending cross-chain requests. Proposal section 6.5 records the prevention
extension; the linked recovery audit records the operational cutover.

## Mechanism and boundaries

The composer owns the allowance calculation. It resolves the selected L1
profile at the earliest target and all announced later fork activations, takes
the smallest outgoing allowance, and applies the existing optional operator
cap. Capacity is measured using the existing payload and blob codecs.

Before ordinary block construction, it walks the supplied parent's hash-linked
ancestors back to the previous scheduled boundary. It deducts their exact
encoded size and reserves a conservative maximum header/empty-body size for
all remaining blocks through the next empty terminal. The driver publishes the
actual scheduler target; both paths preserve its residue relative to L2 genesis,
including nonzero offsets. Hash-based accounting
avoids a stale height-only cache after a reorg. The payload builder retains
Reth's EVM execution and pool ordering and defers overflow through its existing
best-transaction iterator. It checks the final block's actual encoded size too.

Pool admission checks whether one transaction fits a fresh interval. Temporary
lookup/readiness errors remain retryable errors; individually oversized data
gets a descriptive rejection. A valid transaction that does not fit the current
remaining space stays in the pool, including its nonce dependencies. The driver
preserves empty intermediate Sync boundaries when catching up.

Native six-blob testing also exposed a preparation timing failure: sidecar and
witness preparation could consume the available time before proof generation
even began. Anchor-only posting now records that preparation time and reserves
its per-rollup high-water mark, plus the existing proof budget, through the
existing L1 target selector. It can choose a later eligible slot without
splitting for time, relaxing deadlines, or retargeting cross-chain effects.
The measurement is volatile and does not introduce a configuration option.

This is a local sequencing limit, not a new consensus encoding or settlement
trigger. Policy-disabled composers and followers keep their existing behavior.
Cross-chain composition keeps its full-bundle gas/blob checks. A pending owned
Sync may anchor the next interval; an unexplained nonempty boundary fails closed.
Funding, unavailable proofs, L1 exclusion, and existing oversized history are
not repaired by this guard. Startup continues to reject an operator cap that makes retained history
unsplittable. This prevents accepting a contradictory configuration.

## Requirement coverage

| Requirement | Implementation | Automated coverage |
| --- | --- | --- |
| Use receiving-network maxima, optional operator cap, codec capacity | `composer/policy.rs::resolved_blob_allowance`, `composer/sequencing.rs::sequencing_da_limit` | Existing `BlobLimits` tests; native Ethereum and Gnosis ordinary bursts; RPC operator-cap rejection |
| Account for delayed inclusion and announced fork reductions | `allowance_through_future_forks`; active profile's fork and BPO schedules | `future_reductions_constrain_admission_but_old_limits_and_future_increases_do_not` |
| Reserve the whole shortest legal interval, including framing and empty terminal | `item_limit`, exact ancestor RLP and `BlockDaSizer` | `interval_budget_accounts_for_framing_and_all_reserved_blocks`; native exact interval encodings |
| Safely bound execution-derived fields and all transaction encodings | `eez-payload-codec::BlockDaSizer`; final actual-block assertion | Codec tests across legacy/typed transactions, byte values, RLP length boundaries, arithmetic overflow |
| Retain overflow and sender nonce dependencies; continue other senders | `node-common/payload.rs::DaTransactions` | Iterator selection and gas-rejection refund tests; native sequential nonce bursts |
| Reject individually unpostable transactions; fail closed on missing inputs | `SequencingDataGuard`, `SystemAddressGate` | Retryable-unavailable pool test; native rejection followed by a valid same-nonce transfer |
| Preserve intermediate boundaries during catch-up/rebuild | `driver/sequencer.rs::commit_one` | `guarded_catchup_preserves_every_intermediate_empty_boundary` across every grid residue, including unavailable/stale outcomes; `interval_bounds_follow_every_scheduler_residue_and_check_overflow`; native canonical-reader outage/catch-up workloads |
| Preserve policy triggers and independent derivation | Guard installed only for enabled composer; unchanged derivation | Burst remains unposted until the configured interval is due, then drains; exact follower receipts/state and subsequent cross-chain call; existing always-mode regressions |
| Fitting multi-blob candidates progress despite preparation exceeding the next slot's available time | `EnabledPolicy::note_preparation`, `preparation_budget`, `preparation_fits_target`; existing fresh L1 target selector | `measured_preparation_reserves_time_without_changing_proof_or_network_budgets`; native full-capacity posts and observed preparation reservation |
| Preserve existing incompatible history without silently truncating it | Existing final candidate validation and error paths | Existing `lowering_blob_cap_rejects_retained_unsplittable_history_without_losing_it`; the former runtime-stall test now proves rejection before sequencing, valid queued work, and acceptance after a supported cap increase |
| Preserve disabled-policy behavior and existing settlement features | Default trait methods and absent guard | Existing driver fallback, native protocol/policy, gas, and dynamic cross-chain tests |

## Changed components

- `eez-payload-codec`: conservative block-item sizing beside the actual codec.
- `eez-composer`: interval reservations using the same outgoing limit resolver
  as final posting; no second policy evaluator.
- `eez-driver`: budget hooks and empty intermediate boundaries.
- `eez-node-common`: guarded payload transaction selection and pool validation.
- `eez-node`: wire one shared guard into the enabled composer; weak binding avoids
  a reference cycle through the composer's payload-builder handle.
- `eez-node-e2e`: burst, admission, and retained-history regressions in the
  existing native test target; manifests add required codec/builder dependencies.
- `eez-testkit`: route the canonical reader through the existing fault-capable
  relay from startup, allowing observation outages without stopping the L1 miner.
- Proposal/operator documentation: prevention contract, automatic activation,
  deferral semantics, and explicit limits of this change.

## Validation

All **201 affected unit tests** passed: composer 141, driver 39, payload codec
15, and node-common 6. **38 selected native cases** passed across settlement
policy (25), automatic cross-chain admission (7), bundle gas (4), and legacy
composer/follower behavior (2). Tests use the existing native infrastructure
with actual node, signer, execution, and follower processes on Ethereum and
Gnosis profiles; they are not a new Kurtosis deployment.

| Check | Result and evidence under `artifacts/sequencing-da-budget-20260914/` |
| --- | --- |
| `cargo build --offline --locked -p eez-node -p eez-follower -p eez-proof-signer --bins -j 3` | Passed; `build-results.json`, `runtime-final.log` |
| `cargo test --offline --locked -p eez-node-e2e --tests --no-run --message-format=json -j 3` | Passed; `native-build.log`, `native-build-executables.json`; the policy target was rebuilt after test-only fixture corrections |
| `cargo test --offline --locked -p eez-composer -p eez-driver -p eez-node-common -p eez-payload-codec --lib --no-run --message-format=json -j 3`, then each emitted test executable with `--test-threads=1` | 201 passed; `unit-preparation-final-results.json` and its four logs |
| `python3 artifacts/sequencing-da-budget-20260914/run-tests.py settlement_policy prevention-ordered ordinary_` | Four capacity/admission cases passed; `prevention-ordered-results.json` |
| Policy target with `--skip ordinary_`, then the two `companion_keeps` cases | 19 cases passed before interrupting a stale diagnostic assertion; the two remaining cases passed after correcting that assertion. See `policy-ordered-settlement_policy.log` and `companions-final-results.json` |
| `python3 artifacts/sequencing-da-budget-20260914/validate-remaining.py` | Both companion cases, all seven dynamic-admission cases, all four gas cases, and both legacy cases passed; `remaining-regressions-results.json` records all five successful steps |
| `cargo +1.94.0 clippy --offline --locked --workspace --all-targets -- -D warnings` | Passed; `clippy-final.log` |
| `cargo check --offline --locked -p eez-node-common -p eez-driver --no-default-features` | Passed; `no-default-final.log` |
| `cargo fmt --all -- --check`, `git diff --check` | Passed; `fmt-final.log`, `diff-check-final.log` |
| Live Chiado recovery and public-service checks | 10455 captured signatures replayed and authenticated in L1 DA; independent follower parity, fresh bidirectional calls, public RPC/WSS, monitor reconnect/fallback, and complete explorer suffix verification. See the linked recovery audit |

The interrupted policy invocation is retained with its nonzero exit status;
it is not reported as a single clean 21-case run. Its 19 completed cases plus
the two corrected reruns cover the 21 existing cases; the four focused cases
complete the 25-case policy target. No production code changed between the
frozen runtime build and these final tests.

The frozen composer SHA-256 is
`131ed913c14ecc89be578713bff3f2ea287b9f421b90b2ddeb42f7bb12d2cbba`;
follower `3196bd2bdd1b7d1d61d13f9ac51175b8621b0498af436c20100e9baf6b8889ef`;
signer `581d795fc300f9ec9a43dd5e543b71840e69d307c9d117418f1c6de18c57f174`.
`runtime-identities.json` records these binaries. Per-invocation result files
retain the test executable hashes, including the final test-only rebuild.

## Interpretations and remaining limits

- The guard reserves the smallest announced future allowance because accepted
  data may settle after a fork; an announced increase never expands today's
  admission limit. Execution-derived header sizing is conservative, so this
  change does not promise perfect byte utilization.
- The former oversized-single-block test deliberately expected admission to
  strand the chain. Its replacement checks the newly required early rejection,
  valid queued work and nonce preservation, then acceptance after a supported
  cap increase. The separate retained-history/cap-reduction regression remains.
- Fault injection now interrupts only canonical L1 reads. Pausing the entire
  native composer would also pause its embedded L1 miner and change the test's
  timing assumptions. The burst fixture also uses the established native relay
  tip-order convention; production fee settings are unchanged.
- Native six-blob tests exposed the preparation deadline problem described
  above. The proposal explicitly records this measured timing correction. A
  cold first attempt can still miss its deadline before a measurement exists.
- During live replay, one otherwise valid ordinary transfer remained pending
  despite available space and later executed after a controlled restart and
  identical resubmission. Its pool-selection cause remains unestablished; this
  capacity fix does not claim to resolve it. Evidence and recovery limits are
  in the linked audit. Investigating that specific case is a useful follow-up.

No new operator option, consensus encoding, or persistent policy record is
introduced. Existing gas checks, due triggers, disabled-policy behavior, and
independent derivation remain covered by the regressions above. This evidence
does not assert that every possible workload or failure has been eliminated.
