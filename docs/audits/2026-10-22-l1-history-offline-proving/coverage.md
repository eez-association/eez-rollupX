# Requirement, implementation and regression map

This maps the authorized [proposal](../../proposals/l1-origin-and-offline-proving.md)
to code and tests. Execution results, failures and remaining gaps belong to the
[validation record](README.md); a test listed here is not itself a passing result.
The implementation applies to fresh networks from genesis.

| Requirement | Implementation | Meaningful regressions |
| --- | --- | --- |
| O1: permanent complete history | [L1BlockhashContract](../../../eez-core-protocol/src/L2/L1BlockhashContract.sol), shared [storage and transition rules](../../../crates/eez-protocol/src/l1_history.rs) | Core `L1BlockhashContractTest`; shared history vectors; EVM `delayed_import_runs_before_opcode_and_retains_unused_headers_without_transaction_gas`; multipart comparison of every imported hash. |
| O2: exact mandatory import | [EVM history hook](../../../crates/eez-evm/src/history.rs), bounded system call in [evm.rs](../../../crates/eez-evm/src/evm.rs), existing hook order in [block.rs](../../../crates/eez-evm/src/block.rs) | Actual system-call budget, caller balance/nonce and atomic failure tests; wrong runtime/state/RANDAO, duplicate and conflicting import tests. Ordinary transaction receipts and gas remain covered by the existing executor suite. |
| O3: complete checked ancestry | `HeaderChain` in [shared history](../../../crates/eez-protocol/src/l1_history.rs), [canonical acquisition](../../../crates/eez-l1/src/derivation/history.rs), [stream validation](../../../crates/eez-proof-signer/src/validate/streaming.rs) | Canonical encoding, fork layout, gaps, conflicting branches, inherited/non-maximal source and exact endpoint tests; real denied-signer rejection of missing source and a bridge-only omitted header. |
| O4: independently bound public input | [OfflineRollup](../../../contracts/src/OfflineRollup.sol), [OfflineProfile/OfflineContext](../../../crates/eez-protocol/src/l1_history/context.rs), [public inputs](../../../crates/eez-protocol/src/public_inputs.rs) | Independent flat ABI/profile word vectors; deployed manager/Rust byte comparisons; registry proof-context substitution, accepted-root, anchor and rule-period tests. |
| O5: accepted parent/checkpoints | [Offline candidate context](../../../crates/eez-proof-signer/src/service/l1.rs), [settlement authorization](../../../crates/eez-proof-signer/src/service/settlement_job.rs), existing whole-group binding | Frozen native v2 accepted-root substitutions; checkpoint/effect tests; `partial_consumption` process module, including restart and a middle-group resume in the same L1 block. |
| O6: offline prover | Local-only [configuration](../../../crates/eez-proof-signer/src/config.rs) and [service](../../../crates/eez-proof-signer/src/service.rs); [network-denial launcher](../../../scripts/ci/run-offline-signer.py) | Actual-parent bidirectional/static/revert native tests under kernel network denial; historical cold multipart signers; omitted evidence fails before attestation. Old RPC settings, including empty values, fail startup. |
| O7: public reconstruction | [Deriver inputs](../../../crates/eez-deriver/src/deriver/inputs.rs), [deriver](../../../crates/eez-deriver/src/deriver.rs), shared executor, existing native V0 DA | Ethereum/Gnosis native blob replay without sequencer headers; multipart clean follower compares every header/hash/state root/RANDAO plus application observations and permanent history. No derived import list is added to blobs. |
| O8: bounded work/lifecycle | [Signer sessions](../../../crates/eez-proof-signer/src/service/sessions.rs), [window admission](../../../crates/eez-proof-signer/src/window.rs), existing request worker permits | V2 quotas, exact duplicates, changed witness, Rewind/Resume, prefix contradiction, cancellation, detached-worker retention, controlled admission deadline and successful same-epoch retry; frozen conformance transcripts. |
| O9: v2 witness extension | [Shared witness schema](../../../crates/eez-control-rpc/proto/prove.proto), [v2 service](../../../crates/eez-control-rpc/proto/prove_stream.proto), [identity](../../../crates/eez-control-rpc/src/identity.rs), [client](../../../crates/eez-prover-client/src/streaming.rs), [durable witnesses](../../../crates/eez-node/src/witness_source.rs) | Capability/identity rejection, both ancestry fields, byte/item accounting, changed-anchor terminal resend, rewind and persistent branch identity tests. Generated protobuf code comes from Cargo/protoc. |
| O10: recovery and canonical progress | [Canonical head](../../../crates/eez-l1/src/l1_canonical_head.rs), history invalidation, deriver unsafe-suffix checks, existing [Composer ownership](../../../crates/eez-composer/src/composer.rs) | Reorg with no removed settlement or RANDAO change; six missed-slot/late-original replacement process tests; `queue_reorg`, `partial_consumption`, and shared-signer `streaming` reorg scenarios. |
| O11: fixed delay | `HistorySchedule` in shared history, [genesis schedule](../../../crates/eez-protocol/src/l2/derivation.rs), [configuration loading](../../../crates/eez-l1/src/chain_config/l2.rs) | q=0/intermediate/k−1, bootstrap, repeated source, delayed delivery versus missed slots, aligned timestamps and arithmetic boundaries; deployment/configuration consumer tests. |
| O12: synchronized PREVRANDAO | Shared history transition drives [producer context](../../../crates/eez-l1/src/derivation/history.rs), EVM environment and replay | EVM opcode/header/state agreement across Sync; correct origin with wrong RANDAO rejects; multipart independent schedule oracle and 32 ordinary application records in the long fixture. |
| O13: multipart replay beyond L1's blockhash window | Existing range/admission/ownership machinery, complete final anchor bridge, [multipart process exercise](../../../crates/eez-node-e2e/tests/derivable_operations/multipart.rs) | Passed: 2,000 L2 blocks in four parts, source age 304, exact contract rejection of the old source as an anchor, cold denied signers, BPO1, both anchor modes, 32 application writes and complete follower replay of every block and all 401 imported L1 hashes. The existing short case also passed; together two passed, zero failed/ignored in 213.61 seconds. The optional four-day stress run was intentionally stopped after 16 settlements / 160,000 L2 blocks; it did not reach final replay. |
| O14: two anchor modes | [Composer selection before proving](../../../crates/eez-composer/src/composer.rs), manager context, shared `EvidenceAnchor`, signer `CandidateContext`, [L1 rule profile](../../../crates/eez-l1/src/chain_config/offline.rs), follower inclusion checks | Actual-parent target/chain/parent rejection; historical posting-context/action rejection, including static/reverted observations; ages 1/256/257 and reserved anchors; fork-period invalidation and first-unsupported-inclusion regression. |
| O15: ordinary contract storage proofs | [L1StorageProof](../../../contracts/src/L1StorageProof.sol), exact permanent-history lookup | Solidity synthetic and independently captured EIP-1186 fixtures cover block/address/slot/root substitution, canonical absence/zero, inline nodes, malformed/truncated/extra nodes, limits and fork-aware headers. |

## Integration and cleanup

- Shared protocol types own schedule, storage layout, header validation, context
  and commitment encoding. Runtime roles acquire or transport evidence; the
  signer independently validates it. No role-specific dependency enters the
  shared protocol crate.
- [Genesis generation](../../../scripts/update-eezl2-genesis.sh),
  [rendering/deployment](../../../scripts/deploy.sh),
  [manager deployment](../../../contracts/script/DeployRollup.s.sol), generated
  runtime/code hashes, base/test genesis and Kurtosis profile change together.
  Bootstrap is authenticated before rendering and broadcast. Existing output
  is rejected rather than overwritten or migrated.
- Composer/follower still retain their chain connections. Signer Compose inputs
  contain local L1/L2 profiles and explicit deployment bindings, without either
  chain RPC setting. The signer does not load a repository `.env` implicitly.
- V1 proving, Prepare/Retain and their leases/cache/provider code are removed.
  `prove.v1` remains the namespace of current shared messages and Capabilities;
  `prove.v2.Prover.ProveStream` is the only proving service. Retired tests and
  fixtures, and the surviving requirements that replace them, are recorded in
  the validation record.
- No dependency version, source or checksum changed. Seven workspace package
  dependency lists changed to name the shared types/helpers actually consumed.
  The lockfile digest therefore changes validation identity.
- Conditional fee/gas estimation recognizes the new manager through the strict
  [deployment fingerprint](../../../crates/eez-composer/src/composer/estimate/deployment.rs).
  Its compiled-runtime regression rejects modified executable bytes; the native
  nested-CREATE2 fee scenario retains its quote, settlement and follower assertions.
- [Admission projection](../../../crates/eez-composer/src/composer/gas.rs) and
  [ABI framing](../../../crates/eez-composer/src/composer/gas/framing.rs) account
  for exactly one mandatory accepted-root pin. The focused red/green regression
  preserves incremental/final estimate agreement and conservative fallback for
  invalid pin shapes. The [contract gas fixture](../../../contracts/test/PostBatchGasPins.t.sol)
  measures the deployed offline manager in both anchor modes, with 64 rule
  epochs and up to 64 blob references. All 28 native admission cases passed,
  including Ethereum/Gnosis fitting waves and final follower replay; the
  validation record retains the first failures and post-fix executable identity.
- The v2 client tracks the oldest retained predecessor when background
  prevalidation opens a later session. The client regression
  `prevalidation_after_proof_preserves_the_next_requested_predecessor` requires
  complete backfill before finalization of an earlier unsettled range; it
  rejects an unnecessary failed Finalize and checks every retransmitted hash.
- The [pending RPC environment](../../../crates/eez-node-common/src/rpc.rs)
  rejects a synthesized update position when Reth would otherwise use the
  parent's unmodified state. A complete-evidence red/green regression covers
  both a changed and repeated source; installed-source inheritance and reorg
  tests remain. Executed-block reads use the executed state and header.
- L2 ingress classification uses that executed state/header pair, matching the
  local gas classifier; L1 ingress retains pending tracing. The HTTP regression
  reproduces ordinary work being misrouted when a scheduled import makes pending
  simulation unavailable, and also checks nested/caught dispatch guarding.
  The unchanged ordinary Safe process regression supplies native verification.
- Partial consumption requires a new root-bound proof. The same-block resume
  case replays complete captured v2 evidence against the permitted checkpoint,
  checks rejection of the original proof with the changed pin, and retains
  ordered group consumption and independent follower replay. Delayed settlement
  preserves ordinary block hashes; an actual imported-history reorg separately
  exercises recovery of twenty ordinary transactions without resubmission.
  All four focused process cases passed with signer egress denied, including
  the specific stale-signature rejection and independent final followers;
  the first full native rerun was stopped after 15 passes to rebuild the
  fork-boundary fix below. The subsequent 177-entry inventory completed with
  168 passes, four failures and five ignored cases. The failures reached the
  corrected observation helper or cold restart of a persisted correlation.
  All three helper callers and all eight selected recovery cases subsequently
  passed on the final explicit role build; the audit distinguishes the original
  failed inventory from those corrected process runs.
- Composer compares the historical anchor's L1 rule epoch with the intended
  inclusion epoch before proving. A known boundary selects actual-parent mode
  immediately; it does not convert a failed live attempt. The focused regression
  failed before the fix, then all 406 Composer unit tests passed. Existing
  manager tests independently check that inclusion-time rule changes alter the
  authenticated context.
- Restoring a persisted settlement also reconstructs its canonical L1 evidence
  before accepted-endpoint validation. Three unchanged native restart cases
  exposed the missing preparation; after the correction, all 44 deriver unit
  tests and workspace Clippy pass. The eight-case process suite also passed on
  the explicit role build, including all three original failures, retained
  witnesses, partial consumption and imported-history/shared-signer reorgs.

## Coverage limits

The multipart fixtures use Ethereum execution with the five-second L1 and
one-second L2 cadence. The user clarified that four days was an example of
sources outside L1's blockhash window, not a required stress duration. The
focused expired-source exercise supplies the corresponding acceptance gate;
the interrupted dense run supplies partial stress evidence only. These are not
four-day native Gnosis results. Public deployments,
live Kurtosis topology/PBS validation and CI
release-artifact promotion are outside these local results. Consult the
validation record for the actual completed commands and exclusions.
