# Gas estimation on the Composer L1 and L2 fronts

| Field | Value |
| --- | --- |
| Design status | Wallet-focused scope accepted by user on 2026-10-07; implemented; validation recorded below |
| Date | 2026-10-07 |
| Source baseline | `eduadiez/rollupX`, `main`, `fa916b13254a36423cad130080dde367bb18a7d6` |
| PR integration base | `ab6e44c4710c80a06687ff28f39c635c9419636b` (current fork main when prepared) |
| Protocol baseline | `855fe0602484750861b4c31502a1a91355d33ed6` |
| Working tree at inspection | Existing edits to `docker-compose.chiado-node.yml`, `docker-compose.chiado.yml`, and untracked `.claude/`; preserved |
| Related contracts | [Settlement policy](../configurable-settlement-policy.md), [signer profile](../../crates/eez-proof-signer/SPEC.md), [RPC failure handling](../../crates/eez-control-rpc/SPEC.md#6-failure-handling), [queue ownership](../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission) |

## Decision and scope

Implement both directions using **read-only composition followed by native
source-chain execution**. The successful high-gas execution supplies the search hint; admission's
planning model is not used as a wallet answer. Return a gas limit that successfully executes the user transaction and
its intended cross-chain outcomes in the prepared local context.

| Front | Preparation on disposable state | Execution used to size the user's limit |
| --- | --- | --- |
| Composer L1 | Run the real registry's batch publication, with one explicitly assumed proof-verification result during setup only | Run the L1 user transaction through the real proxies, queues and callbacks |
| Composer L2 | Run the real native execution-table setup and other required group transactions | Run the L2 user transaction through the real EEZL2 execution path |

The L1 result is **conditional on a valid proof being supplied later**. This is
not proof validation, a usable settlement artifact, or a claim that the batch
can be published. No proof is generated for a wallet estimate. The signer and
final signed-bundle preflight continue to perform all existing checks without
any simulation exception.

The implementation is limited to wallet estimation on both fronts. Queue,
retry, admission scheduling, proof generation and final preflight behavior are
unchanged. The estimate is advisory for its sampled context, not a promise about
future packing or inclusion.

## Baseline and constraints

At the baseline, the [front](../../crates/eez-node/src/ingress.rs) forwarded `eth_estimateGas`.
It only intercepts submissions and discovery. Its submission classifier already
builds an unsigned `debug_traceCall` request from a signed transaction.
An ordinary node does not prepare tables for a new cross-chain call; missing
lookups can fail or be caught by the application. Merely accepting a successful
upstream estimate therefore does not establish correct cross-chain execution.
The baseline wallet process test reproduced an L1 protocol revert before signing;
see the validation record below.

[Source composition](../../crates/eez-composer/src/local/client.rs) intercepts
cross-chain dispatch. The [companion model](../../crates/eez-composer/src/composer/gas.rs)
uses measured work and reserves but caps predictions at the declared limit and
falls back to that limit for unknown shapes. These are planning semantics;
neither outcome demonstrates a sufficient user gas limit.

[Native L1 preflight](../../crates/eez-composer/src/composer/posting.rs) already
separates receipt gas from gas required before refunds and observes actual
cross-chain outcomes without intercepting them. [Native L2 execution](../../crates/eez-composer/src/local/build.rs)
already provides disposable Sync forks. Reuse these execution and observation
primitives, but not `capacity_probe` alone: that helper can skip execution and
intentionally leaves non-capacity failures for the later real append.

The pinned [core contract](../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md),
[entry specification](../../eez-core-protocol/docs/EXECUTION_ENTRY_SPEC.md),
[static entries](../../eez-core-protocol/docs/STATIC_ENTRY.md) and
[multi-prover contract](../../eez-core-protocol/docs/MULTI_PROVER_SPEC.md) govern
execution. The [caveats](../../eez-core-protocol/docs/CAVEATS.md) explicitly exclude
indistinguishable gas-dependent outcomes. `USE_GAS_LEFT=false` removes gas from
some keys; it does not make outcomes gas-independent. Keep the narrower current
signer profile, including its restrictions on static/failed descendants,
explicit call gas and rollup/proof-system count.

## Public behavior

Intercept `eth_estimateGas` on both Composer fronts. Preserve the Ethereum
hex-quantity response, request IDs and ordinary upstream error/revert payloads.
The returned limit covers only the source user transaction:

- L1 includes its actual proxy consumption and L1 callbacks. It excludes the
  poster's transaction, proof verification, remote L2 execution and blob fees.
- L2 includes work charged inside the L2 user transaction. Separate native system
  transactions and L1 posting costs are not added to its gas limit.

For cross-chain requests, omitted selector, `latest` and `pending` all select
Composer's next eligible execution context. Document this Composer-specific
meaning: none reserves a slot or reproduces the full pending queue. Explicit
historical selectors and state/block overrides are unsupported for cross-chain
estimation in v1. Ordinary calls retain upstream estimation after local classification.
The classifier supports numeric historical blocks and state overrides. It rejects
`safe`, `finalized` and EIP-1898 objects as context-unavailable in v1. For ordinary
`pending` calls it observes the current local head, not the upstream mempool; the
upstream still selects the ordinary estimate. Gas-dependent or pending-only
dispatch therefore remains outside the supported equivalence claim.

Require an explicit `from` for cross-chain estimation. Normalize transaction
fields using the pinned source node's call rules; preserve explicit sender,
nonce, value, fee/access-list fields and type. Reject inconsistent chain IDs and
unsupported transaction types. A supplied gas limit is a ceiling. Otherwise
use the active transaction/block limits and RPC allowance rules. Never clip a
failed or over-cap estimate into success. Requests depending on queued prior
nonces fail against the isolated context; do not synthesize those predecessors.

An estimate is conditional on state, execution context and subsequent table
preparation. Clients still submit the signed transaction through the matching
Composer front. Head changes, different preceding transactions or different
fees can require a new estimate. No result promises future canonical inclusion.

### Classification and batches

Classify calls locally on the source provider with a bounded call-frame
observer; do not add a mandatory upstream `debug_*` dependency to ordinary
wallet estimates. Reuse the dispatch-selector/address recognition from ingress,
including nested and reverted frames. Share this recognition logic rather than
forking its list of entry points. The unsigned adapter is also shared.

For ordinary calls, forward the original estimate request upstream. For an
observed cross-chain call, use the service below. An inconclusive trace is an
explicit error, not proof that the call is ordinary. A successful upstream
estimate is not a shortcut past classification. Source-native classification
must honor the requested context/overrides for ordinary calls; if that context
cannot be inspected locally, return context-unavailable rather than silently
classifying against another block. Document this availability boundary.

Observe the final ordinary candidate limit locally as well. If that execution
reaches cross-chain dispatch, route through composition. A gas-dependent path
change that cannot be resolved consistently is unsupported, not silently
ordinary. This is bounded detection, not a proof about all possible gas values.

Split incoming JSON-RPC batches into items so batched estimates reach the same
handler. Forward other allowed `eth_*` items with IDs intact. Keep the current
batch-submission prohibition, empty/malformed batch rules and notification
semantics. Test mixed batches, not just standalone requests.

## Shared preparation and ownership

Add a narrow estimation service in `eez-composer`, wired to both fronts by
`eez-node`. Separate three operations from the existing production functions:

1. Normalize an unsigned source call into an EVM transaction environment.
   Signed admission still verifies/recover its signature before using the same
   execution body. Do not create a fake signed user envelope.
2. Compose one request on isolated sessions and construct its real L2 replay
   group and L1 entries using existing ABI, root stitching and semantic builders.
3. Execute a prepared source call/group on disposable state and collect gas plus
   actual effects. Keep request input, composition and successful replay distinct.

Resolve the canonical L1 parent and correlation history, plus a validated L2
predecessor using existing next-block context construction. Reject outstanding
optimistic attempts while settlement reconciliation is pending; do not borrow
an optimistic prefix for a wallet estimate. Snapshot hashes, target timestamp, native transaction positions and
context generation are explicit inputs. Do not combine independently sampled
`latest` heads or fall back to latest target sessions mid-probe.

If no coherent context exists during catch-up/reconciliation, return unavailable.
Hold provider snapshots, not scheduler locks, during execution. All probes start
from the same pre-group state with fresh journals, sessions and inspector
channels. Recheck the context generation before replying; invalidation returns
a retryable context error. Do not publish hypothetical L2 blocks to obtain their
hashes: use the existing local block/group builders.

Reuse the protocol builders used by
[`prepare_post_batch_raw`](../../crates/eez-composer/src/composer.rs), keeping
wallet preparation separate from its DA/proof/signing work. Reuse ABI encoding,
root plans, trace finalization, native system transactions and public-input
hashing; use the same actual-effect observer as final preflight. There are two different outputs:

- Production preparation continues to construct the real native-blob payload,
  commitments, witnesses and signed batch through the unchanged validation path.
- Wallet preparation constructs only a private execution fixture. On L1 this
  has the real prefix and inbound-effect block-hash trajectory, but no blob
  indices and an empty proof placeholder. The registry's ABI allows an empty
  blob list; the production signer profile does not thereby admit that fixture.
  Compute the expected verification digest for this exact fixture. Never label
  it a valid production batch or pass it to a proof/submission API.

This distinction avoids inventing signatures or commitments for an unsigned
request. An L1 user's signature is not needed to build the native L2 delivery
effects. An unsigned L2 user is executed through the EVM call adapter; do not
claim its hypothetical block hash is the eventual signed transaction's block
hash. No raw simulated user transaction is fed to canonical replay or DA codecs.
No wallet estimate builds blobs/KZG proofs or witnesses. Real batch preparation
and native blob DA are unchanged. No manual Solidity queue storage writes.

The L1 fixture's omission of DA and its assumed proof are both explicit
simulation differences. They affect the setup digest/event and setup gas, not
the queue contents for equal entries. This is an inference from the pinned
registry implementation, to be established by the differential tests before
release. If a deployment makes user-visible execution depend on these setup
differences, reject that deployment/shape rather than silently approximating it.

## L1 execution: prepare through the actual registry

Run the real `postAndVerifyBatch` on a request-owned fork with the actual
configured registry/manager/proxy code and source network executor. This creates
queues, root progression, proxy registrations and other setup effects through
the real contract implementation. The prepared candidate contains the required
anchor/prefix and the requested complete group, not a live held-pool snapshot.

### One explicit proof assumption

The only exceptional execution behavior is the direct proof-verifier call during
this setup transaction. Implement a private setup inspector with all of these
checks:

- Restrict v1 to the configured single ECDSA verifier/current supported deployment.
  Validate registry/manager implementation identity, verifier code and registration
  at the pinned parent. An unknown upgrade or custom verifier returns unsupported.
- Match the exact setup transaction, registry caller/storage context, `STATICCALL`
  scheme, verifier address, selector, full expected calldata and independently
  recomputed public-input digest. Arm for exactly one expected call and require
  exactly one match. A different, missing or repeated call fails preparation.
- Supply ABI `true` for that matched verification call only. This is an assumed
  condition, represented by the private `AssumedProof` inspector, never as verified proof evidence.
  No key is loaded, no signature fabricated and no proof validation is reported.
- Keep structural, membership, threshold, root and context checks executing
  normally. The fixture declares no blobs; this does not claim to validate DA.
  Failed checks stay failures. All other calls execute normally.
- Destroy the setup inspector before the user transaction. Do not override verifier
  bytecode or install a globally permissive verifier. A callback/user call to the
  verifier must see its real code and response.

Keep this capability in a private Composer preparation module. Its only output
is ephemeral execution state plus an explicitly conditional observation; no raw
signed post, proof, authorization token or broadcastable artifact is returned.
Neither the signer nor final preflight accepts this state/type. Production final
preflight has no flag that enables assumed verification. Test this separation.

The substituted call and omitted fixture DA mean setup gas, its receipt and
fee-dependent effects are not an exact posting simulation. Do not report them
as posting estimates or use them as proof that a complete bundle fits. Retain existing posting budgets and
final native validation. User gas is measured in a separate transaction with
fresh warm-access, transient-storage and refund state. The supported deployment
uses a view verifier whose only relevant setup result is acceptance; custom
verifier behavior is excluded.

Applications depending on exact hypothetical poster/beneficiary fee balances
are outside v1's supported equivalence claim. Setup fees differ from a genuinely
posted batch. This is documented as an estimate limitation, not implemented as a
new balance-tracking subsystem.

### Measure the user

Execute the unsigned L1 user as the correct sender through the real queue/proxy
path on that prepared fork. Source interception is off. Reuse the final preflight's
actual-outcome observer and endpoint checks, including caught static failures.
Observe callbacks, reverted work and the complete expected source occurrences;
outer success or a final matching height alone is insufficient.

The wallet estimate uses the isolated group's real queue position. Do not add
128 entries of hypothetical scan cost. Within-group scans and cursor restoration
are executed normally. Existing final preflight remains authoritative when the signed request joins
other work at a different position.

## L2 execution: replay the complete native group

Construct and execute the real setup/user/callback group in production order on
a disposable Sync fork. Keep native transaction position, funding, table lifetime
and block environment rules. Use an unsigned-call adapter only at the user
transaction boundary; preserve its actual transaction gas and fee semantics.
System setup gas is measured separately and excluded from the user's limit.

Require successful native user execution and agreement with the composition's
call identities, outcomes, consumption and resulting effects. Share or extend
observational helpers; root/cursor checks alone do not cover static/caught paths.
Check whole-group capacity even though only the user's limit is returned.

## Gas selection used in both directions

Use the existing posting preflight pattern: a successful high-gas probe,
pre-refund/floor gas plus conservative headroom, and native execution at the
exact proposed returned limit. Recompose on the same pinned state for each
chosen gas limit. A lower probe is only a candidate; its failure does not prove
source invalidity or need classification as out of gas. If it fails, revalidate
the known successful high bound as the third and final pass. Context/provider
errors and deadline expiry return errors, not fallback success.

Never return a limit above the request ceiling, balance allowance or applicable
transaction/block limits. Count setup separately from source gas and preserve
whole-group capacity. Compare actual cross-chain outcomes with the composition;
outer success alone is insufficient. Reject unsupported gas-sensitive ambiguity.
The returned value is a verified conservative limit, not a minimum. Common process fixtures assert that the result is below twice receipt gas plus
21,000; no fallback-rate telemetry is introduced in v1.

## Resource bounds and lifecycle

Use Reth's task executor for blocking execution. Share two active wallet-estimate
permits across the two fronts, with no waiting queue initially; return a structured
busy error. Use a five-second request deadline, three paired probes and existing
profile/input/output bounds. Target dispatch is additionally limited to 64 recorded calls per probe,
including reverted calls. Each target frame retains its existing gas limit.
Measure these limits before expanding the supported workload. Do not add a cache, online calibration or configuration
matrix as a workaround for a slow implementation.

Wallet workers do not own the settlement worker or its permit. Two workers can
still contend for CPU/storage, so measure estimation latency and interference.

The response deadline bounds waiting; workers check the deadline between probes
and target calls. A timeout or disconnect cannot terminate an in-flight blocking
operation, and disconnect alone does not cancel it before that deadline. The worker keeps its permit until exit. Provider calls can outlive the response deadline; their worker still owns the
permit. Let the task executor own teardown. Snapshot/database state is discarded on every exit path.
Wallet requests never touch held-pool ownership, nonce reservations, retry charges,
block publication or proof/relay clients. The estimator does not add entries to accepted-source histories; its private
setup type and this contract identify the conditional-proof basis.

## Concrete implementation sequence

These are ordered reviewable changes, not separate releases that leave L1 open.

| Step | Components and deliverable | Exit criterion |
| --- | --- | --- |
| 1. Baseline and shared call input | Front process fixtures; `local/client.rs` unsigned adapter and isolated snapshot/session builder | Reproduce current failure in both directions; signed admission remains identical; unsigned sender/nonce/fee validation tested |
| 2. Shared preparation | Reuse protocol ABI/group/block builders without DA/proof/signing | Same entries, roots and table positions as production for signed reference fixtures; explicit unsigned input adapter and simulation-only L1 envelope; no signer/network side effects |
| 3. L1 conditional native executor | Private exact-match verifier assumption, ordinary source execution, actual-effect observation | Differential test against the same batch with a real valid proof: equal user gas requirements/outcomes/state; negative isolation tests pass |
| 4. L2 native executor and shared search | Unsigned user in real Sync group, per-probe composition, retained successful bound | Both directions return a limit that passes fresh native replay, including refunds/forwarding and profile boundaries |
| 5. Front integration | Local classification, parameters, both endpoint handles, batches, executor limits | Wallet-style estimate/sign/send succeeds on both fronts; ordinary compatibility and load/cancellation tests pass |
| 6. Release validation and docs | Focused and workspace checks; RPC/operator/settlement contracts | Wallet fixtures pass on Ethereum/Gnosis; report remaining matrix gaps explicitly; no production proof/preflight shortcut |

Step 3 is the early architectural checkpoint. If conditional setup cannot match
real-proof execution for the supported deployment, do not ship a model fallback
or broaden the assumption. Record the concrete mismatch and revise this design.
This is a falsifiable acceptance gate, not an unresolved choice of L1 strategy.

## Validation matrix and evidence

The matrix describes the broader desired coverage. The executed validation below
is the release evidence for this wallet-focused implementation; unexecuted rows
are not implied by successful basic wallet fixtures.

| ID | Required behavior | Test/evidence |
| --- | --- | --- |
| G1 | Ordinary wallet behavior | Both fronts: transfer, contract call, creation, `latest`/`pending`/historical/overrides, fee/type/nonce errors; upstream `debug_*` disabled; original upstream estimate semantics |
| G2 | Estimate before signing in both directions | Estimate unsigned request, sign with exactly returned gas, submit through matching front; successful receipt, application outcomes and independent follower replay |
| G3 | User gas is not total group gas | Separate L1 posting, L2 system and user receipts; compare required pre-refund gas and receipt gas; storage refunds, calldata floor and deep supported callback forwarding |
| G4 | L1 conditional setup is faithful and isolated | Paired real-proof/native-DA and conditional/no-DA setup runs at same context with equal entries/root trajectory; equal user results/required gas; wrong caller/digest/verifier/count fails; user verifier calls not intercepted; setup-fee limitation documented |
| G5 | Gas-sensitive/partial effects do not masquerade as success | Multi-root, supported nested/caught/static paths, low-gas catch branches, unused entries, differing outputs with identical lookup keys; assert actual effect mismatch or unsupported result |
| G6 | Exact returned limit really runs | Cap/ceiling boundaries, non-converged search, final-probe failure, changed composition, zero-value and supported value transfers; never return a clipped or merely declared limit |
| G8 | No mutation from wallet estimates | Compare canonical state, published blocks, queue, nonces/retries and proof/relay call counts before/after success, revert, cancellation and reorg |
| G9 | Batches and bounded workers | Mixed JSON-RPC batches/notifications, malformed inputs, two busy workers, disconnect/timeout retains permit until worker exit, shutdown and no blocking work on async threads |
| G10 | No production verification relaxation | Invalid proof still rejected by signer/final native preflight; simulation state/type never accepted as authorization; verifier assumption absent from production submission executor |
| G11 | Context and load behavior | Catch-up, optimistic-prefix invalidation, canonical reorg and pending dependency; same-context failures stable; compare settlement latency with/without bounded estimation traffic |

Use the current Ethereum and Gnosis execution profiles and current signer-supported
shapes. Required fixtures include cold/warm storage, callback depth, refund-heavy
calls, maximum supported data/return sizes, repeated queue identities and restored
cursors. Performance comparisons use the same baseline, profile, workload and
concurrency; report actual preparation/probe costs and timeout rates, and confirm
wallet estimation performs no KZG/witness/proof work. No
throughput or gas-tightness improvement is claimed before measurement.

Build with Rust 1.95.0 and Foundry 1.7.1. Run formatting, locked workspace Clippy,
affected Composer/node unit tests and serial `node_e2e` modules `cross_chain::`,
`chained_interstate::`, `dynamic_admission::` and relevant queue/settlement cases.
Build integration contracts and clear prebuilt overrides according to AGENTS.md.
The differential oracle uses genuine proofs in tests; production estimation does
not contact the signer. Run targeted negative tests for every assumption boundary,
not just happy-path receipts.

## Compatibility, alternatives and completion

Only Composer fronts gain cross-chain estimation; direct node/follower RPCs retain
their existing behavior. No protocol pin, contract deployment, signer wire schema,
database migration or historical follower decoding changes are proposed. Native
execution still uses the configured network's rules and pinned runtime identities.
L1 conditional setup rejects unrecognized registry, rollup, verifier and optional
posting-wrapper bytecode. The pinned `EEZPostBatcher` wrapper is executed when
configured, with its immutable registry binding checked at the sampled parent.
Code identity is not separately whitelisted for L2 native replay.

Old fronts continue forwarding estimates. Upgrade the Composer as one binary;
no signer-first upgrade is required. Rollback removes estimation, restoring prior behavior. Queue/retry state remains
in memory and restart retains its existing resubmission/reconciliation limitations.
No estimate is a durable reservation or recovery journal.

Rejected alternatives: model-only success (no execution evidence), per-wallet
real proofs (cost and signer coupling), handwritten queue storage (layout drift),
a replaced permissive verifier contract (too broad), fixed high limits (no effect
validation), and shared speculative mutable state (ownership races). The chosen
setup-call assumption is narrower than replacing verifier code and explicitly
conditional; it is still a material simulation assumption that must be tested.

Implementation must update the owning RPC/operator docs for selector meanings,
supported shapes, errors and endpoint differences. No AGENTS.md invariant changes are needed:
real proof validation, final execution validation and canonical ownership remain.

Status: implemented on `codex/composer-wallet-gas-estimation`.

### Initial-baseline validation (before main integration)

- Baseline fork `fa916b1`, protocol `855fe060`: the unsigned L1 wallet request
  failed with RPC code 3, protocol revert `0x9a499f3b…`. The first sandbox run
  could not bind local ports and did not reach the RPC; the subsequent process
  run did reach it. Logs/datadirs are retained locally under
  `artifacts/gas-estimation/baseline-*` (not portable checkout evidence).
- Composer library: 300 passed, 1 existing ignored manual profiling test.
  Includes native unsigned/signed equivalence, nonce spending on reverts, exact
  verifier-call matching and unchanged final-preflight regressions.
- Integration contracts: 82 passed. The new conditional-setup oracle compares
  cold user execution gas, return data, queue cursor and final root against a
  genuine ECDSA proof with blob-hash inputs for zero, one and two callbacks.
  This is a contract execution oracle using Foundry blob-hash cheatcodes, not
  a KZG/DA-availability test or a test of the Rust batch builder.
- The full contract formatting check reports pre-existing differences in
  `contracts/src/ECDSAProofSystem.sol` and `contracts/script/DeployRollup.s.sol`;
  these files were left unchanged.
- Node library: 64 passed, including worker-permit retention after timeout and
  JSON-RPC revert-data/request-ID handling. Rust formatting and locked workspace
  Clippy with all targets and warnings denied passed.
- Wallet process tests: 2 passed (Ethereum Osaka and Gnosis Prague), with unsigned
  estimate/sign/exact-gas submission in both directions, value transfers, mixed
  batches/notifications, ordinary-transfer upstream parity, invalid request
  rejection, nonce non-mutation and independent follower block/state replay.
  Zero-value setter fixtures returned L1 96,539 versus 89,641 receipt gas and
  L2 94,599 versus 87,794 receipt gas on both profiles. These are fixture results,
  not universal overhead bounds.
- Earlier candidate failures (omitted fees, internal lookup chain ID, pending
  settlement context, measured setup capacity) and a unit fixture missing Prague
  blob fields are retained locally under `artifacts/gas-estimation/`. The final
  process helper retries only explicit unavailable-context responses; it does not
  retry execution/setup failures. Earlier failed runs are not passing evidence.
- The full validation matrix above is not covered locally: dedicated estimator
  reorg/overload/settlement-interference tests, exhaustive fee/type/override cases,
  large-data/refund/deep-callback estimation fixtures and complete Kurtosis/release
  artifact suites remain unverified. No live deployment or performance comparison
  was performed. Existing production nested/static/caught-call regressions are
  separate from coverage of those shapes through the new wallet RPC.


### Main integration validation

The integration branch merges fork main `ab6e44c` without changing the protocol
pin. It preserves the newer queue retry/status and priority-fee changes and
executes the configured posting wrapper during conditional L1 setup. The original
working tree and its local Compose settings remain untouched; integration used
an isolated worktree.

Validation results for this integrated revision are recorded below. Earlier
baseline results above do not independently validate the merged revision.


- Final implementation revision: `f41f34788a1f762b5f73bcc5e5cae5900280ef0f`.
  Subsequent changes to this record are documentation only. Rust formatting,
  locked workspace Clippy (`--all-targets -- -D warnings`) and Composer unit tests
  passed: **305 passed, 1 existing ignored manual profiling test**.
- Node unit tests at integrated revision `6894f1d`: **65 passed**. The initial
  sandbox attempt failed five HTTP-binding tests before socket access was allowed;
  that attempt is retained separately, not counted as passing coverage.
- Integration contracts: **86 passed**. Changed Solidity file formatting and
  documentation links/diff checks pass. The full contract formatting check still
  reports only the two unchanged pre-existing files named above.
- Process regressions at `6894f1d`: **9 passed**: direct calls in both directions,
  nested calls in each direction, caught destination reverts in each direction,
  both mixed/static-read scenarios, Gnosis existing-proxy multicall admission,
  and Ethereum actual companion-gas admission. These exercise existing production
  paths, whose code was unchanged by the final fingerprint correction.
- All three wallet process tests at the final implementation revision passed:
  direct Ethereum, direct Gnosis and Ethereum through `EEZPostBatcher`, each in
  both directions. Every test covers zero-value calls, value transfers, exact
  returned-gas signing/submission, mixed batches, invalid requests and independent
  follower replay. The setter gas figures match those recorded above.
- The first merged wallet run rejected the registry's fingerprint. Comparing
  artifacts from two checkout paths found that only an IPFS hash in embedded
  `CrossChainProxy` metadata differed. Recognition now masks exactly that
  non-executable 32-byte field; an additional unit regression rejects adjacent
  executable-byte changes. Native replay still uses the original deployed bytes,
  including actual CREATE2 addresses. The corrected wallet tests repeat the same
  failing scenarios; the earlier failure remains recorded in `process.log`.
- An earlier pre-integration regression run was stopped after a node/signer
  revision-identity mismatch caused by committing while that run reused runtime
  binaries. It is invalid validation evidence. The integrated runs above use a
  fixed source revision throughout. A superseded two-fingerprint implementation
  was also stopped during compilation, before its wallet tests ran.

Local-only evidence lives under `artifacts/gas-estimation/` in the integration
worktree: `clippy-portable.log`, `composer-portable.log`, `node-unit-ports.log`,
`contracts.log`, `process.log`, `wallet-portable.log`, and the corresponding
retained process logs/datadirs. These ignored artifacts are not available in a
fresh checkout. No live deployment or full release/Kurtosis matrix was run.

Final local development executable SHA-256 values (not release provenance):

| Executable | SHA-256 |
| --- | --- |
| `eez-composer` | `2c80904a47ac325651bf869f21d6a69be2564430da197f137baf1c582ac388c5` |
| `eez-follower` | `4f4ce64442da2f8e7337219bc9b6c0274e495f1b3a69bc53cce925b24eff83bc` |
| `eez-proof-signer` | `1b7b3d252224752a79d522363c438bcc5020885fbc9595b29687fe6d4f8f5fb8` |

Reproduce the focused checks from the repository root after the AGENTS.md
prerequisites (including LLVM 22, initialized protocol and integration contracts):

```bash
cargo +1.95.0 fmt --all --check
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.95.0 test --locked -p eez-composer --lib
cargo +1.95.0 test --locked -p eez-node --lib
forge test --root contracts
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
EEZ_TEST_LOG_DIR="$PWD/artifacts/gas-estimation/process-logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/gas-estimation/process-datadirs" \
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- \
  cross_chain::wallet_gas_estimates \
  cross_chain::inbound_nested_contract cross_chain::outbound_nested_contract \
  cross_chain::inbound_reverting_destination cross_chain::outbound_reverting_destination \
  cross_chain::outbound_mixed_static_reads cross_chain::outbound_changing_static_reads \
  cross_chain::both_directions_zero_value \
  dynamic_admission::existing_proxy_multicall_estimate \
  settlement_policy_gas::ethereum_actual_companion_gas \
  --test-threads=1 --nocapture
```
