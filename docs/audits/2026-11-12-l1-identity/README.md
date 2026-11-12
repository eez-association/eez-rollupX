# Canonical and embedded L1 identity investigation

Status: bounded investigation for [#145](https://github.com/eez-association/eez-rollupX/issues/145).
This records distinct checks, rather than treating a common chain ID as proof of
a common genesis. No runtime behavior, dependency, protocol pin or deployment
profile changes are proposed here.

## Baselines and trust limits

| Input | Revision |
| --- | --- |
| Frozen rollup0 review source | `1b66d9bda675e6f8e83f514153a97136909acc06` |
| Frozen source protocol | `143d87e62cade98bfc122d612638dcaaf1c85633` |
| Frozen rollupX review target | `31e148f50dd79730b17d113ac78795eb377eb5c5` |
| Frozen target and current protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Issue's later current-code check | `716a8a3bad4415ade7e9a6bc3af4a1b35419b232` |
| Handoff main | `20243f70839ec69df9312731e960cd311e060050` |
| Freshly fetched investigation base | `dbe1cdb9047e7f3502505f381aaa602e6ebf552f` |

The fresh tip adds board-status documentation to the handoff main. The dedicated
worktree initially contained clean `bad8d7eef7e16ac872f354d31af257f8646b2fac`;
it was switched to the fresh tip before inspection. The protocol was initialized
at its unchanged gitlink. Frozen source/target configuration was read with
`git show`; those old revisions were not built or run. This is a focused N11
follow-up, not a fresh review of all rollup0 changes. The original review's input
provenance remains **not independently verified**.
The saved handoff described by
[#135](https://github.com/eez-association/eez-rollupX/issues/135) records manifest
digest `d032aeb948629918739e6e36def65978465ddeb56fdbe802018ec50f450181dc`.
This investigation read the issue records and frozen source, but did not
independently authenticate that original manifest or its inputs.

The frozen source's `eez-node::launch` explicitly compared the submission RPC
chain ID with the embedded chain when `blob_da` was enabled. The frozen target
already had profile construction, canonical readiness and deriver authentication.
The missing comparison at the identical source-code location therefore does not
establish a missing equivalent check. N11's disposition is **partially covered**:
there are concrete rejection paths, with limits on complete provider-pair and
genesis coverage described below. No backport is selected by this investigation.

Three different identities matter:

1. **L1 block zero:** the genesis header in the embedded chainspec. The effective
   L1 rules hash commits to it, alongside chain ID, forks and blob rules.
2. **L2's initial L1 reference:** `rollupX.l1Reference` in L2 genesis. It may be
   block zero or a later finalized L1 block. Runtime derivation authenticates
   this reference's number, hash, timestamp and randomness against canonical RPC.
3. **L2 genesis/profile:** the L2 genesis hash and full chain-document digest,
   used with the effective L1 rules to bind Composer/signer configuration and
   the deployed verification key. They are not an RPC endpoint certificate.

The owning contracts are the accepted
[offline profile](../../proposals/l1-origin-and-offline-proving.md),
[history contract](../../../contracts/L1_HISTORY_SPEC.md), and
[portable identity](../../../crates/eez-control-rpc/VALIDATION_IDENTITY.md).
The pinned [Core settlement model](../../../eez-core-protocol/docs/CORE_PROTOCOL_SPEC.md#settlement-model)
retains conditional execution checks; its unchanged manager supplies the
timestamp/hash context, not a new endpoint-genesis attestation.

## Production boundary trace

Paths and behavior below refer to the investigation base, not the frozen target.

| Boundary | Check and consequence | Limit |
| --- | --- | --- |
| Configuration | `L1ReaderConfig::from_env` selects `EEZ_L1_RPC_URL`; `SubmitterConfig` optionally selects `EEZ_L1_TARGET_RPC_URL` for target preparation/observation. Embedded selection uses `EEZ_L1_CHAIN` and, for private kinds, `EEZ_L1_CHAIN_PATH`. | Parsing an endpoint does not authenticate it. Target RPC is a third input, not the embedded provider. |
| Embedded database initialization | Reth's engine launcher calls `with_genesis`, which compares an existing stored genesis hash with the selected chainspec and returns `GenesisHashMismatch` on disagreement. Its explicit debug skip-validation option bypasses that comparison. | This is a database-versus-configured-spec check, not a comparison with the separately configured canonical RPC. Source-inspected only; this investigation did not launch a database mismatch. |
| Embedded rules → L2 schedule | `offline_l1_rules` commits to embedded chain ID, genesis header hash, forks and blob rules. `OfflineL1Rules::profile` rejects `L1 reference/profile chain mismatch` and an initial reference before required block-based forks. | Profile construction does not query canonical RPC or compare its block zero with embedded block zero. |
| Composer → signer | `eez-node::launch` builds the profile from the embedded spec and L2 schedule, then `composer_proving_from_env` calls `RemoteProver::capabilities` with the expected portable identity and verification key before L2 sequencing. | Agreement with independently configured signer rules is stronger than chain ID equality; mutually wrong operator inputs are not repaired or independently certified by this handshake. |
| Follower profile | The follower validates profile construction but does not retain its commitment or perform the Composer/signer handshake. `Deriver::new` likewise validates the profile and retains `offline_rules.headers`. | A same-ID, different-genesis local L1 spec is not rejected by these profile checks when the remaining rules are compatible. Canonical reference authentication is separate. This follower configuration was not process-tested. |
| Canonical readiness | Composer passes the embedded chain ID to `wait_for_l1_ready`; follower first checks its local L1 profile against `EEZ_L1_CHAIN_ID`. Readiness rejects a different RPC chain ID, waits for deployment height and reference finality. Block zero needs no finalized checkpoint. | No reference hash or deployment receipt authentication here. This deliberately permits checkpoint-aware restart with pruned receipts. |
| Deriver construction | Both roles call `Deriver::new` → `L1Reader::derivation_source` → `DerivationSource::authenticate`. Wrong chain, wrong recomputed reference header or insufficient finality rejects; only unavailable source errors retry. | A nonzero reference authenticates that reference; it does not fetch an ancestry proof all the way back to block zero. RPC canonicality/finality remains relative to the operator-selected provider. |
| Derivation after startup | `canonical_header` fetches by canonical number and requires the expected hash. History construction checks ancestry and later canonical endpoints; retained-suffix recovery revalidates origins. | A one-time chain-ID read does not establish permanent endpoint identity. Later checks bind concrete headers, not a certificate for the URL. |
| Front upstreams | After deriver construction, `validate_cross_chain_front` requires each configured upstream's chain ID to match its source role (embedded L1 ID or local L2 ID). Ports can bind earlier, with submissions gated until catch-up completes. | This validates another configured URL by chain ID, not genesis. Front startup/retry behavior is not exercised by this investigation. |
| Fee quote | `PostingRuntime::quote_sample` compares the current RPC chain ID to both `CrossChainExecCtx::l1_chain_id` and the embedded EVM chainspec, returning `PostingError::Unavailable` on disagreement. | This is a runtime guard, including against an endpoint changing chain ID after startup. The signing context ID was read from that RPC during startup; readiness separately compares RPC and embedded IDs. Same-ID forks require hashes/state. |
| Composition | `L1SlotState::open` anchors embedded state. Composer requires its hash to equal the canonical quote sample; otherwise it requeues the drain and attempts the ordinary minimal-post path. | A mismatch is not permission to consume/evict the cross-chain users. This check is at composition, not a startup genesis comparison. |
| Final bundle simulation | `PostingRuntime::simulate` requires the local header at the quote height to hash to the canonical fee sample, then opens `state_by_block_hash` and executes the signed bundle. Missing state or a hash mismatch is `PostingError::Unavailable`. | Reaching startup does not imply the embedded provider can execute against a later canonical state. |
| Submission target | `Submitter::send_bundle` reads target-provider chain ID; inbound envelopes and the final quote/target binding are checked. `FeeQuote::is_current` binds chain, exact sample and target. | This is not a general startup genesis check for `EEZ_L1_TARGET_RPC_URL`. Relay acceptance is not canonical settlement. |
| Standalone replay | `verify_blob_evidence::prefixes::replay_spec` checks supplied complete L2 genesis against reported genesis hash and effective configuration; `replay_history` derives L1 rules/profile and checks the verification key. | Existing genesis-replay tests concern evidence validation, not a live pair of canonical/embedded providers. |

Relevant implementation:
[node wiring](../../../crates/eez-node/src/lib.rs),
[front upstream check](../../../crates/eez-node/src/ingress.rs),
[follower wiring](../../../crates/eez-follower/src/lib.rs),
[readiness](../../../crates/eez-node-common/src/lib.rs),
[effective rules](../../../crates/eez-l1/src/chain_config/offline.rs),
[canonical authentication](../../../crates/eez-l1/src/derivation.rs),
[deriver construction](../../../crates/eez-deriver/src/deriver.rs),
[posting](../../../crates/eez-composer/src/composer/posting.rs),
[composition](../../../crates/eez-composer/src/composer.rs),
[submission](../../../crates/eez-l1/src/submitter.rs), and
[replay](../../../crates/eez-l1/examples/verify_blob_evidence/prefixes.rs).

The database check is wired by the vendored
[Reth engine launcher](../../../crates/vendor/reth/reth-node-builder/src/launch/engine.rs)
and [launch context](../../../crates/vendor/reth/reth-node-builder/src/launch/common.rs).
Its dependency implementation was inspected in the locked Reth v2.7.0 checkout
(`3d592ece`, `crates/storage/db-common/src/init.rs`); Cargo's cache was not edited.

## Bounded mismatch matrix

`A` is the fixed synthetic L1 reference and chainspec. `C` differs only in chain
ID; `G` keeps the chain ID but changes genesis extra data and hence its hash.
The L2 schedule always names A. The new
`canonical_and_embedded_identity_matrix` invokes the production profile builder
and canonical authentication as two independent component checks, using mocked
RPC responses. Authentication receives only the canonical provider and fixed
schedule, never the embedded spec. Each row is iterated with `pruned=false` and
`pruned=true`; neither profile rejection nor canonical chain-ID rejection
fetches a header. Those iterations do not exercise raw-header fallback.

| Embedded spec | Canonical source | Expected component result |
| --- | --- | --- |
| A | A | Profile and canonical reference accepted; control |
| A | C | `Mismatch("genesis reference chain identity")` |
| A | G | `Mismatch("L1 genesis reference header")` |
| C | A | `L1 reference/profile chain mismatch`, before RPC |
| C | C | Same profile rejection: agreement between providers cannot override L2's configured reference |
| C | G | Same profile rejection |
| G | A | Canonical reference accepted, but profile commitment differs from A; **not proof of supported end-to-end startup** |
| G | C | Different profile commitment; canonical chain rejection |
| G | G | Different profile commitment; canonical reference-hash rejection |

There are 18 loop iterations, not 18 distinct provider-pair validations. Six
stop at profile construction. Of the twelve authentication calls, four stop at
chain ID, four use normal headers, and four use the `4444`/raw-header fallback.
The A and G embedded rows repeat identical authentication inputs; the four
fallback calls represent two distinct inputs. The chain-ID rejection also
repeats across both values of `pruned`. The table records the combined outcomes
of independent checks, not a comparison of live providers. Existing derivation
tests already cover the canonical-side rejections; the added profile assertion
shows that changing embedded genesis changes its commitment. No embedded
database is instantiated. The
genesis mutation changes only extra data, holding timestamp/randomness constant,
so the negative control specifically exercises hash identity rather than a
different schedule. Matching ID with changed genesis cannot produce the same
profile commitment.
All 18 iterations completed as expected in the fresh L1 suite. These results qualify the component
boundary; they are not a full provider-pair startup or settlement matrix.

The new `reference_hash_is_authenticated_after_chain_readiness` uses a
localhost JSON-RPC server on an OS-assigned port and the production `L1Reader`.
It serves chain 1 and a finalized reference at height 5, rejecting history-log
queries like the existing pruned-source fixture. Both cases pass readiness:
the exact reference is accepted by `reader.derivation_source`, while a changed
configured reference hash must fail with
`Mismatch("finalized genesis reference header")`. This is a two-stage component
check, not a launched Composer/follower process or a claim to have replayed data.

Existing tests retain additional orthogonal cases: wrong chain on a pruned
source, insufficient finality, missing headers, malformed/trailing raw RLP,
wrong returned height, wrong timestamp/randomness, stale canonical hashes and
genesis replay inputs. Fresh selection/results are listed below.

The new `posting_quote_checks_rpc_signing_and_embedded_chain_ids` calls the
production `PostingRuntime::quote`, with its actual `CrossChainExecCtx` and
embedded `ChainProvider` configuration. Three synthetic single-field mutations
(RPC, signing context, embedded chain ID) must return the exact
`PostingError::Unavailable("submission RPC and selected L1 profile have different chain IDs")`
before consuming any header response. The matching control must produce a
quote tied to the canonical sample hash. RPC responses are mocked, fees use
the supported fixed-tip setting, and no transaction is broadcast. This does
not claim that the subsequent embedded-state simulation path was executed.
These are not three independent startup inputs: the signing context ID comes
from the canonical RPC, and readiness compares that RPC with the embedded ID.
The runtime check protects against changed or inconsistent endpoint responses
after the earlier reads. This fixture uses the Ethereum EVM configuration
(`gnosis_evm_config: None`); no Chiado quote fixture was run here.

| RPC chain ID | Signing context ID | Embedded spec ID | Expected quote result |
| --- | --- | --- | --- |
| 1 | 1 | 1 | Valid quote bound to the supplied canonical header |
| 2 | 1 | 1 | Exact identity-disagreement error |
| 1 | 2 | 1 | Exact identity-disagreement error |
| 1 | 1 | 2 | Exact identity-disagreement error |

## Supported configurations and remaining gaps

- Canonical and embedded handles may be distinct, but must refer to compatible
  rules and the exact canonical state needed for the operation. The node's
  comment about using Anvil for submission does not waive the later readiness,
  profile and posting checks.
- Public Ethereum and Chiado presets, and configured private Ethereum/Gnosis
  profiles, are intentional configuration paths. Their existence in source is
  not a claim that every network permutation was launched in this investigation.
  The normal [Ethereum deployment](../../runbooks/ethereum-node.md) uses the
  embedded RPC itself as canonical input.
- Receipt-pruned restart is supported when retained canonical headers and the
  selected recovery range are available. Error `4444` permits raw-header fallback
  by the same number/tag. It does not waive chain ID, hash, timestamp or finality.
- An RPC returning the expected chain ID but a different initial reference must
  reject during derivation, even though readiness can pass. A common chain ID
  alone never proves common genesis.
- A same-ID embedded chainspec with a different genesis changes the rules hash
  and portable/deployed-key identity. Profile construction alone does not reject
  it against an independently supplied L2 reference. The Composer configuration
  handshake rejects disagreement with a correctly provisioned signer; this is not evidence
  that every mutually misconfigured pair is rejected during startup.
- Followers do not perform that signer handshake. They validate the profile
  but retain only the header rules in the deriver. A change to local L1 genesis
  extra data alone, with the same chain ID and header rules, is not rejected by
  these checks; canonical reference authentication still uses the L2 schedule.
  This does not demonstrate incorrect follower execution, but its complete
  startup/replay behavior under that configuration remains untested.
- Full process injection across canonical RPC, target RPC, embedded database,
  follower, signer and registered contracts remains separate from component tests. A
  missing direct startup comparison is not, by itself, a demonstrated settlement
  safety defect. Any additional startup rule needs a separately reviewed narrow
  follow-up, including its effect on private fixtures, pruned restart and trusted
  nonzero bootstrap references. No production fix is included here.

## Validation and review

All commands ran from the repository root with Rust/Cargo 1.95.0, the locked
dependency graph and `LLVM_SYS_221_PREFIX=/usr/lib/llvm-22`. Test builds used
`CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`, `CARGO_INCREMENTAL=0`
and `CARGO_TARGET_DIR=/tmp/issue145-target` (the manifest retains its optimized
test profile). Subsequent commands limited builds to two jobs. Foundry 1.7.1
was present but no Solidity code changed or Solidity test was run.

The clean initial workspace had only about 2 GiB of disk free. Compilation
therefore used a task-owned target under `/tmp`. The sandbox prohibits
`socket()`, confirmed by both a Python probe and the first Rust run. Tests with
local RPC fixtures were rerun outside it. The host then exhausted `/tmp`'s
per-user quota despite reporting free filesystem space; a tool invocation also
failed with `Quota exceeded` while creating its mount metadata.

Only this task's artifacts were moved to `/dev/shm/rollupx-issue145-3c94`, with
the original target path retained as a symlink. The L1 ELF's SHA-256 was identical
before and after relocation. Logs similarly moved behind the existing
`/tmp/issue145-evidence` path; subsequent tests set
`TMPDIR=/dev/shm/rollupx-issue145-tmp-3c94`. No other task's resources were cleaned.
These paths are **local-only, ephemeral evidence**, not downloadable artifacts.

| Check | Actual result |
| --- | --- |
| Initial existing derivation selection: `cargo +1.95.0 test --locked --offline -p eez-l1 --lib --features chain-spec derivation::tests:: -- --test-threads=1` | 6 passed, 1 failed, 205 filtered out, 0 ignored; HTTP wire test failed at localhost bind with `PermissionDenied`. Exit 101, retained `baseline-l1.log`. This filename denotes the initial selection, not an untouched historical build: new test code was added while dependencies compiled, and production code remained unchanged. |
| First unsandboxed full L1 suite | Exit 101 during the quota incident. `l1-full.log` stopped at 4096 bytes, before final failure details/counts. Three file-loader tests visibly failed; their full diagnostics were not recoverable from the truncated output. Do not count this run as a pass or assert that its complete failure set is known. |
| Isolated full L1 rerun: `cargo +1.95.0 test --locked --offline -p eez-l1 --lib --features chain-spec -- --test-threads=1` | **212 passed, 0 failed, 0 ignored, 0 filtered out**, 7.23 seconds; exit 0, `l1-full-retry.log`. Includes all original derivation tests and the new matrix. This is a retry-only full-suite pass, not first-run-green evidence. |
| Shared startup: `cargo +1.95.0 test --locked --offline -p eez-node-common --features l1-startup --lib -- --test-threads=1` | **34 passed, 0 failed, 0 ignored, 0 filtered out**, 0.18 seconds; exit 0, `node-common.log`. Includes both new reference-hash cases and the existing pruned-source chain/finality checks. |
| Genesis/prefix replay: `cargo +1.95.0 test --locked --offline -p eez-l1 --features chain-spec --example verify_blob_evidence -- prefixes::tests:: --test-threads=1` | **19 passed, 0 failed, 0 ignored, 1 filtered out**, 0.06 seconds; exit 0, `genesis-replay.log`. Includes matching nonzero genesis, malformed/mismatched genesis and altered history genesis tests. These test the replay validator, not a fresh independent replay of a live chain. |
| Workspace lint: `cargo +1.95.0 clippy --locked --offline --workspace --all-targets -- -D warnings` | **Passed**, exit 0, `clippy.log`; 10m 05s including native dependency rebuild. No warnings suppressed for this change. |
| Composer: `cargo +1.95.0 test --locked --offline -p eez-composer --lib -- --test-threads=1` | **454 passed, 0 failed, 1 ignored, 0 filtered out**, 2.86 seconds; exit 0, `composer.log`. Includes all four new quote cases. The ignored manual timing profile is named below. |
| Formatting and documentation | `cargo +1.95.0 fmt --all --check` and `git diff --check` passed. All 19 local documentation links resolve at this checkout, including the initialized protocol pin. No shell script or executable shell example changed. |

The L1 validation history is recorded as **flaky/retry-only**, with the observed
sandbox/quota limitations above. The incomplete quota log prevents attributing
every failure from that run to a specific assertion; the later pass does not
erase it.

No production executable was launched. The L1 unit-test executable was
`eez_l1-052e89f88f51486b`, SHA-256
`7e69c5dc0ecb133e9f28c76c94a1aa517e2d72f22c65ee8fe924b0ffb1387c05`.
The shared startup unit-test executable was
`eez_node_common-f2c22872951ceb8b`, SHA-256
`bceb8ca016cfa9847f74a94d1c5a951201bfd39eb207dd8e63cc843f31d826b7`.
The replay-example test executable was `verify_blob_evidence-16b0b89de220aa22`,
SHA-256 `53cebd8daecdcad44b2cd1bc8bf25a3ecaed9436ec6316fa76d2f9591d320194`.
The Composer unit-test executable was `eez_composer-6a2f0e4ead0cf3ba`, SHA-256
`bf3dd20b5c079b1bb9138818b9d2e135425b00a6b7131dc7ca81b2e834563391`.
The unchanged lockfile SHA-256 is
`f771a77eb4a5b17259157fd91829096015897e24979a4fda0df583d45719294f`.
The compact [evidence manifest](evidence.json) records source, log and executable
digests, selections, results and evidence limits. Log digests identify the
retained local files; they do not make those ephemeral files publicly available.

The replay filter excludes
`tests::settlement_endpoints_require_the_forks_continuous_single_rollup_chain`;
it is not claimed as executed. Composer's
`held_pool::tests::profile_blocked_backlog_discovery` is an ignored manual timing
profile and was not executed. No process E2E, live network, signer/RPC package
suite, Solidity suite, genesis regeneration or release-artifact/Kurtosis lane
was run for this test/documentation change. Full process mismatch injection and
the signer/registered-contract boundary remain explicitly unverified here;
source inspection and component passes cannot substitute for those checks.
The separately monitored main CI run `43188514265` was not changed, cancelled
or rerun by this task.

No production services were deployed or restarted, no operator wallets used, and no independent
full-chain replay, vulnerability scan, performance result or live settlement is
claimed. Unit/mocked semantic assertions ran; a passing unit suite does not
establish execution against a real mismatched embedded database.

### Ponytail self-review

Reviewed using the
[Ponytail skill](https://github.com/DietrichGebert/ponytail/blob/main/skills/ponytail/SKILL.md)
and [review command](https://github.com/DietrichGebert/ponytail/blob/main/commands/ponytail-review.toml).
This is a self-review, not an independent reviewer or agent approval.

The change adds three focused tests and this bounded assessment. It reuses the
existing provider mock, startup fixture and posting configuration; no production
abstraction, dependency or persistent helper is added. Rejections assert the
specific production error, and each boundary retains a matching control. The
startup fixture now reports its actual computed header hash so authentication
can proceed beyond readiness. The new local server uses an allocated port and
is stopped after each bounded check; posting tests do not broadcast.

The review traced the startup, derivation, quote, composition, simulation and
submission callers and checked that the matrix does not represent profile
acceptance as successful end-to-end startup. No actionable defect was found in
the test change. Runtime cost is bounded by 18 profile/authentication loop iterations,
two startup-reference cases and four quote cases. No production performance
claim follows from those counts.

Verdict: suitable as investigation evidence, with N11 still **partially covered**.
No backport or production fix is justified by this change alone. The principal
unverified boundary remains a full process pairing of same-ID/different-genesis
canonical and embedded providers through signer agreement and final state
simulation. This PR does not close #145 or replace its acceptance criteria.

### Opus 5.5 review corrections

Claude Opus 5.5 reviewed commit `4e6385d524bebb8610164a2c14ab6b941994edcd`
using read-only source tools. It found no blocking code defect and requested
clearer follower coverage, independent matrix legs, fallback counts and quote
input provenance. Those descriptions are corrected above. The startup test is
renamed to distinguish readiness from subsequent reference authentication; its
assertions are unchanged. The manifest now supplies the exact diff command for
reproducing the original test-patch digest and scopes original results to the
reviewed commit. Follow-up validation is recorded separately, preserving the
original failures and executable identities.

Follow-up checks passed: all 34 shared-startup tests (including the renamed
test), workspace Clippy with warnings denied, formatting, diff checks and all
19 local links. The other suites were not rerun because their source and the
renamed test assertions are unchanged. Original executable digests identify
the earlier builds; the startup rebuild reused its output path and has a new
digest recorded under `review_followup`.
