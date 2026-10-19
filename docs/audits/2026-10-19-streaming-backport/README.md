# Upstream streaming prover semantic backport

| Field | Value |
| --- | --- |
| Date and status | 2026-10-19; investigation and implementation in progress |
| Scope | Rollup0 #198/#205/#206 behavior adapted to the current fork |
| Source | Fork main `51ef1ba08d5c506c8909bc8be2e16e8d25c0a7a8` |
| Protocol | `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774`; isolated submodule initialized recursively |
| Branch / worktree | `backport/upstream-206-205-198`; `/tmp/rollupx-backport-206-205-198` |
| Initial status | Clean new worktree; protocol initially uninitialized. Original `feat/l2-fee-recovery` checkout at `fa916b13254a36423cad130080dde367bb18a7d6` contained extensive existing tracked/untracked work, left untouched |
| Dependency | Existing fork #89 `759ae5f5cbf87c16b52cedfecc05aae737e90a87` merged into latest main before runtime changes |
| Related record | [Requirements and design](../../proposals/streaming-prover-backport.md) |

## Source identity and review

| PR | Inspected base | Inspected head | Status |
| --- | --- | --- | --- |
| [#198](https://github.com/eez-association/eez-rollup0/pull/198) | `fede75fc185f11a0b330de999dbffeae6d49d777` | `efcbb9a10086c94c9a79c8187faaef5cdbeffd32` | Merged as `a270a06a92131d1987d49903a4174eef04ce959c` |
| [#205](https://github.com/eez-association/eez-rollup0/pull/205) | `f5f99ec04a2e0c627dc3473bf704d17d141caa33` | `d3c94296bd6f23b4a5e0104950948492ab36398b` | Open |
| [#206](https://github.com/eez-association/eez-rollup0/pull/206) | `d3c94296bd6f23b4a5e0104950948492ab36398b` | `21e0805628f4bd80bcf41c9d2b8f58c179984072` | Open; review requests changes |
| Fork [#89](https://github.com/eez-association/eez-rollupX/pull/89) | `2c19e36bed2720c00f89841ccfca47ec7c921e06` | `759ae5f5cbf87c16b52cedfecc05aae737e90a87` | Open draft; schema/native vector dependency |

Full public PR descriptions, commit lists, reviews, line discussions, timelines
and diffs were retrieved through `gh api` into
`artifacts/backport-investigation/` in the isolated worktree. These raw records
are local-only; authoritative PR links and exact commits above remain retrievable.
The configured SSH fetch failed without a key; public HTTPS refreshed all fork
refs. An automatic recursive fetch encountered an unavailable unrelated protocol
ref; a subsequent nonrecursive fetch succeeded, and the target gitlink initialized.
The final review retrieved all three PR descriptions and review discussions
again: heads and discussions remained unchanged. Commit histories contain six,
two and eleven commits respectively. The merged schema predecessor #168 and
closed, superseded #188 were also checked. Upstream's existing ignored ingress
issue #148 is closed; the fork already checks the Osaka transaction gas cap in
ingress and local execution, so this backport imports no such exclusion.

Review requirements include authenticating registry state at the target parent
(#198), retaining reconciliation ownership across timeouts (stateful-only),
handling stale retained transports, and fresh-anchor recovery after retained
quota exhaustion or L1 rollback (#206). The final #206 head contains the latter
two recovery fixes; its stateful stress claim still depends on an unpublished
Reth receipt fix and cannot be imported as fork evidence.

## Evidence and execution

Rust 1.95.0 and Foundry 1.7.1 are installed. Baseline affected-package tests run
with `--locked --offline`, isolated build output, six build jobs and debug info
disabled. Logs are retained locally in `artifacts/backport-investigation/`.
The nonempty ordinary-terminal regression was run with the baseline's
unconditional checkpoint branch restored temporarily, with automatic source
restoration. Exactly one test failed at the intended checkpoint-backend call
counter (one call versus zero expected). The identical command then passed with
the optimization restored, also comparing independently executed block hash and
post-state root. This reproduces the mechanism, not a separate full main binary:
`cargo +1.95.0 test --locked --offline -p eez-proof-signer validate::stateless::tests::nonempty_ordinary_terminal_skips_checkpoints_without_changing_execution -- --exact`.
Both logs are retained as `checkpoint-{baseline,fixed}-regression.log`.

The source build uses:

```bash
export CARGO_BUILD_JOBS=6 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export CARGO_INCREMENTAL=0 LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
```

Baseline after integrating #89, before runtime changes:
`cargo +1.95.0 test --locked --offline -p eez-proof-signer -p eez-control-rpc -p eez-prover-client`
passed 338 signer, six RPC and 30 client tests; two signer fixture utilities were
ignored. A first sandboxed run could not bind TCP listeners; the same command
outside that socket restriction passed. This environmental failure is retained,
not counted as a passing test or a timing flake.

Completed intermediate checks (final checks are recorded below when complete):

| Command / check | Result and limits |
| --- | --- |
| `cargo +1.95.0 build --locked --offline --workspace` | Passed; debug profile, no release-image claim |
| `cargo +1.95.0 clippy --locked --offline --workspace --all-targets -- -D warnings` | Passed |
| Affected-package tests including `eez-l1` | Six RPC, 199 L1, 370 signer and 38 client tests passed; later signer lifecycle additions passed in the next row |
| `cargo +1.95.0 test --locked --offline -p eez-proof-signer -p eez-node -p eez-composer` | 374 signer, 93 node and 381 Composer passed; two fixture utilities and one manual performance test excluded |
| `forge build --root contracts`; `forge fmt --check --root contracts`; `forge test --root contracts` | Build/format passed; 89 tests in 11 suites passed, including Solidity commitment vectors |
| `python3 crates/eez-control-rpc/conformance/check_vectors.py` | All 29 saved vectors and their fixture/schema hashes unchanged |
| `python3 -m unittest discover -s scripts/ci/tests -v` with pinned Gitleaks 8.30.1 on `PATH` | 388 selected: 382 passed, six explicit opt-in exclusions (Docker payload, two Kurtosis resource/readiness, archive relocation, two runtime Docker probes) |
| `python3 scripts/check-settlement-policy-config.py` | Chiado, Kurtosis and Ethereum mandatory-policy checks passed |
| Gitleaks 8.30.1 scan of all changed files, with repository rules and full redaction | 91 files / 36.86 MB; no findings; no allowlist added |
| `cargo deny check advisories bans sources`; `cargo audit` | Both exit 0; audit reports existing unmaintained/unsound/yanked dependency warnings, detailed below |
| Relative Markdown links and edited shell examples | 197 local paths resolved; 26 shell snippets passed `bash -n`; procedures were not executed by that check |

The first full tooling run exposed a missing expected streaming module in the
native inventory test; the consumer expectation was updated, its 34 enabled
focused tests passed, then the full suite above passed. Development compile and
wire-status failures are retained in the local logs; none was resolved by
weakening saved expectations. Zero-test doctest runs are not coverage.

The final affected-package run selected six RPC, 199 L1, 380 signer and 38 client
tests and passed (two signer utilities remain ignored). Its preceding run exposed
a controlled stub whose zero pre-state root disagreed with its empty-trie header;
the fixture was corrected, preserving the positive execution assertions. This
was a test construction failure, not a retry-only pass.

Final validation completed so far:

| Command | Result |
| --- | --- |
| `cargo +1.95.0 test --locked --offline --workspace --lib --bins -- --test-threads=4` | 1,552 passed, zero failed; two signer fixture utilities and one Composer manual performance test ignored |
| `cargo +1.95.0 test --locked --offline -p eez-protocol -p eez-blob-codec -p eez-payload-codec` | 242 passed, including independent public-input, rolling-hash and Ethereum KZG oracles; no ignored tests |
| `cargo +1.95.0 clippy --locked --offline --workspace --all-targets -- -D warnings` | Passed after the final mirror and regression changes |
| `cargo +1.95.0 clippy --locked --offline -p eez-proof-signer --all-targets --all-features -- -D warnings` | Passed |
| `RUSTDOCFLAGS='-D warnings' cargo +1.95.0 doc --locked --offline -p eez-proof-signer --no-deps` | Passed |
| `forge build --root eez-core-protocol`; core test command below | Build passed at the unchanged pin; 778 tests passed across 63 suites, three mainnet-fork tests skipped |
| Pinned Gitleaks scan of final changed/new files | 93 files / 36.90 MB; no findings |
| Final scoped Markdown and shell checks | 30 added/edited local links and 12 anchors resolved; all 28 shell snippets in changed documents passed `bash -n` |

Core tests retained the required exclusions:

```bash
(
  cd eez-core-protocol || exit
  forge test --no-match-path 'script/*' \
    --no-match-test 'test_Doc_OriginalValue_SeedInSetup|test_PostCost_Incremental'
)
```

The three skipped `MainnetUpgrade` tests are `testForkL1UpgradeAndRepeat`,
`testForkL2UpgradeAndRepeat` and `testForkRejectsWrongSigningKey`; their setup
requires a configured fork RPC. Neither these skips, the named exclusions nor
the excluded `script/*` paths count as coverage. Core build lint warnings are
retained in the local log; the protocol checkout itself remains clean.

The broader documentation scan resolved all 230 local paths and 87 of 88
anchors. The remaining `chiado-deployment.md:163` link to
`configurable-settlement-policy.md#migration` is already broken at the target
base and is unchanged; it is outside the edited links. This limitation is not
reported as a passing full-document anchor check.

The fresh review further strengthened the client malformed-response regression
to require the exact session-binding, state-root or public-input mismatch error.
The focused rerun passed one intended test (37 filtered out):
`cargo +1.95.0 test --locked --offline -p eez-prover-client tests::streaming::cross_session_wrong_state_and_wrong_digest_fail_closed -- --exact`.

The first shared-Composer process test failed before the second Composer could
prove: its external L1 reader followed the source while its embedded simulation
L1 remained independent. Logs show the existing “proof target's canonical L1
parent is unavailable” guard firing. The test now uses the existing native
Engine-API mirror and follows source reorgs through a verified common ancestor;
no production target check was changed. That initial test returned an error
instead of panicking while its handles were alive, so normal teardown removed
its datadirs; its full logs are retained. The corrected test panics on failure
so the repository's failure-datadir mechanism can preserve future failures.
The first failing fixture executable was rebuilt before its separate digest
was recorded; the final executable identities below do not identify that failure.

The corrected shared-signer test passed on its first execution after that
fixture correction: both independent Composers observed the L1 reorg, converged
on a canonical safe prefix, advanced from height 95 to 110, and increased actual
v2 signing events from eight to ten. Both Composer logs contain post-reorg
attestations and derivation retreat signals. The minimal bidirectional native
smoke also passed. These are source-built local process checks, including
semantic assertions; they are not throughput measurements.

The existing pruned-history restart test explicitly starts its fault-injected
signer without an L2 source and therefore exercises v1 compatibility. It does
not establish v2 availability when canonical acceptance logs are pruned. V2
must fail closed when its independent retrieval cannot obtain that history.

The combined native selection contains 41 `cross_chain` cases, plus the
two-Composer/two-keyless-follower case, queue reorg recovery, partial-consumption
restart and pruned-history restart. The source-built executable's `--list`
confirmed 45 selected tests and every requested filter was nonempty. Command:

```bash
(
  set -euo pipefail
  unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
  export CARGO_NET_OFFLINE=true
  export EEZ_TEST_LOG_DIR="$PWD/artifacts/backport-investigation/e2e/logs"
  export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/backport-investigation/e2e/datadirs"
  forge build --root contracts
  cargo +1.95.0 test --locked --offline -p eez-node-e2e --test node_e2e -- \
    cross_chain:: queue_reorg:: native_system:: \
    partial_consumption::restart_after_partial_keeps_accounting_and_rebuilds_intent \
    settlement_policy::restart_with_pruned_bodies_and_logs_settles_and_replays_from_checkpoint \
    --test-threads=1 --nocapture
)
```

The separate new regression uses the same environment and the
`streaming::shared_stateless_signer_composers_recover_from_native_l1_reorg`
filter. All 45 combined tests printed PASS, with zero failed or ignored and
117 filtered out, in 2,746.44 seconds. However, the outer command returned 143
after that summary; its cause is not established. This is evidence for those
45 semantic assertions, not a clean exit for the combined command. No owned
fixture processes remained afterward. The complete local `e2e-integration.log`
has SHA-256 `e033c9af8ecc9b066c4219954f32bbf639bc377a6a07836f6a8248540fdfd71c`.

An exit-status diagnostic invoked the same source-built test executable directly.
Unlike `cargo +1.95.0 test`, that invocation did not initially export
`RUSTUP_TOOLCHAIN=1.95.0` to its nested signer build. The resulting default-stable
build was deliberately interrupted before the scenario ran; the retained
`e2e-pruned-exit-diagnostic.log` reports this setup failure as a SIGINT build
error. Direct executable diagnostics must export the required toolchain too.
With that export corrected, the same source-built executable and an identical
rebuilt signer passed the exact pruned-history restart test and exited 0
(one passed, 161 filtered out, 132.81 seconds). Its local
`e2e-pruned-exit-diagnostic-2.log` has SHA-256
`1a4b1fc54d4d9296ee4c7696cffd5637a248013170abf90d8e28aa210ad5802d`.
This diagnostic does not establish the cause of the earlier command status.

Native executable SHA-256 identities (rechecked after rebuilds):

| Executable | SHA-256 |
| --- | --- |
| `target/debug/deps/node_e2e-fee25cec27d6df60` | `ff901c03ad269e7cce189608d9872a7f01b8d0b2c278634140e2539d8bd9db45` |
| `target/debug/eez-proof-signer` | `fb209480bf96d88954b932755da006dd9ec67c89d4528f03e8a3b175e67f2659` |

Dependency package identities (name/version/source/checksum) are identical to
fork main. Cargo.lock adds four direct runtime dependency edges, plus the four wire-tool
dev-dependency edges supplied by #89. `cargo audit`
reported the existing unmaintained atomic-polyfill, bincode, derivative,
number_prefix, paste and proc-macro-error2 warnings, lru unsoundness
`RUSTSEC-2026-0253`, and yanked chacha20/spin. No dependency/advisory-policy changes
were made as part of this backport.

No operator services, wallets, deployed contracts or state were modified.
Source-based process tests own their local fixtures and public test keys.

## Semantic integration and coverage

Integration order is #198 → #205 → #206. The merge of fork #89 supplies the
already adapted wire schema and native transcripts; runtime work builds on that
commit rather than recreating or replacing it. #206's base is exactly #205's
head. Their overlapping replay code is one shared module, used by the CLI and
signer tests. #168 is the merged schema predecessor; superseded #188 is not an
additional dependency. Issue [#157](https://github.com/eez-association/eez-rollup0/issues/157)
motivates incremental validation before the settlement cutoff. This backport
makes no deadline or throughput guarantee from upstream's unpublished stress runs.

| Upstream requirement | Classification and implementation | Regression evidence |
| --- | --- | --- |
| #198 shared v2 schema, baseline flow, optional controls, unchanged v1 | Already adapted by #89; generated by the existing Cargo/protoc build, exported alongside v1 | RPC serialization tests; all native baseline/control transcripts |
| #198 complete physical blobs and posting context | Requires adaptation: v2 Finalize calls the same `settle_validated` pipeline as v1 | Native blob encoding/ABI/context negative vectors; lookup and changing-static positive proofs; existing signer DA/observation tests |
| #198 prover-owned parent provenance and registry-at-parent correction | New `eez-l1::derivation::proof::retrieval`; exact L2 header hash/number/root, canonical acceptance attribution, complete live-slot endpoints, existing independent authentication at the target parent | `owned_genesis_headers_are_hash_bound_and_source_failures_are_not_authority`, `owned_catchup_provenance_fetches_the_complete_live_slot_and_reauthenticates_it`, `an_execution_log_without_an_attributed_accepted_endpoint_cannot_authorize_parent`; native process tests |
| #205 portable wire replay and saved expectations | Already adapted by #89: retain all 29 native cases and their hashes; no fixture regeneration | 2,235 requests, 2,234 replies, 24 proofs, transport/admission failures; `check_vectors.py`; `wire_replayer_rejects_corrupted_cursor_binding_root_digest_and_attester_oracles` |
| #206 incremental checked execution and terminal-only settlement work | Adapted into existing stateless signer; `ValidatedStreamBlock` retains immutable checked evidence; `from_streamed_prefix` rechecks continuity before the existing settlement gates | Native transcript cold/warm and duplicate cases; nonempty ordinary checkpoint regression; existing native effect/checkpoint and observation replay tests |
| #206 Resume/Rewind/Cancel, epoch fencing, peer isolation | `service/sessions.rs`; active-prefix selection, per-session charges, detached-only eviction, exact duplicate witnesses, no stale publication | Frozen takeover, competing-branch, cancel, rewind, subrange and eviction cases; controlled in-flight Resume/Rewind/Cancel and backpressure tests |
| #206 bounded queues, execution, deadlines and teardown | Existing v1 request/worker permits shared with v2; bounded pending/output channels; timed-out blocking workers retain permits; daemon shutdown closes live streams before draining workers | Count/byte/item pending-queue test; rejected Rewind preserves queued work; same-epoch Finalize timeout retry, active-work versus idle timeout, shutdown and epoch-exhaustion tests |
| #206 Composer prevalidation, backfill and pipelining | Existing node preparation worker, durable ancestry walk and Reth task executor; v2 chunks release the exchange lock; at most eight outstanding frames | Node witness-store/notifier tests; client skipped-prefix and duplex pipeline tests; native cross-chain tests |
| #206 fallback and retry correctness | Client uses fresh transport plus Resume; unsupported controls Begin/backfill with unchanged native settlement; old peers use v1; one bounded fresh-anchor repair | Client optional-control/blob preservation, server restart, wrong binding/root/digest, deadline and v1 fallback tests; consecutive 300-block windows against a 512-block retained quota; identical-block rollback below promoted anchor |
| #206 shared-session L1 reorg recovery | Adapted to this fork's native posting fixture and exact simulation/submission L1; stable header-source port on Composer restart | `streaming::shared_stateless_signer_composers_recover_from_native_l1_reorg`; relevant existing cross-chain/restart/follower replay modules |
| #206 stateful Reth backend, follower embedding, backend selector and stateful branch tests | Not applicable: no stateful prover crate exists in this fork. Preserve existing independent signer and follower; do not import the local unpublished receipt patch | Existing stateless/native execution and follower checks; no stateful coverage claimed |
| #206 backend-wide execution cache and four parallel validators | Superseded by the fork's checked preparation cache/lease architecture and single shared execution worker. V2 retains facts per session; cross-session cache reuse is optional in the common contract | Session-local reuse and bounded worker ownership tests; all existing v1 preparation/lease regressions retained |
| #206 additional `ProvingAnchor`/generic prevalidate trait | Superseded by existing complete `ProvingContext.parent_provenance` and concrete bounded node preparation worker | Existing Composer context/target tests; client reads the same predecessor; no duplicate anchor type at the Composer boundary |

The upstream refactor also retains decoded blocks to avoid repeated decoding.
This fork keeps its existing admitted/validated/authorized types and exact RLP
checks. The adaptation reuses execution and settlement evidence without
reorganizing the native codecs or relaxing their validation. Upstream accepts
calldata-only timeless conditional proofs; this fork continues to reject that
profile. Upstream's repeated-block pseudo-window transport stress is replaced
by legitimate contiguous-range pipelining and wire duplicate tests: local
`ProvingContext` ranges must still match their block count.

## Files and ownership

| Area | Purpose |
| --- | --- |
| `crates/eez-control-rpc/{proto,src,build.rs,conformance,examples,STREAMING_SPEC.md,SPEC.md}` | Existing dependency's generated common contract and wire tooling; runtime profile, limits, registry-at-parent correction and rollout |
| `crates/eez-proof-signer/src/service/{sessions.rs,sessions/tests.rs,l1.rs,settlement_job.rs}` | Session transport/state machine, concurrency regressions, own provenance and shared final authorization |
| `crates/eez-proof-signer/src/{service.rs,validate.rs,validate/streaming.rs,validate/stateless.rs,window.rs}` | Runtime ownership, retained checked facts, ordinary checkpoint optimization and shared admission accounting |
| `crates/eez-l1/src/derivation/proof{.rs,/retrieval.rs,/tests.rs}` | Canonical parent-evidence retrieval and mocked source failures/reorg/attribution checks |
| `crates/eez-prover-client/src/{lib.rs,streaming.rs,connection.rs,tests/streaming.rs}` | v2 preference, v1 fallback, exact response verification, fresh transport, bounded repair and pipelining |
| `crates/eez-node/src/lib.rs` | Connect streaming to existing durable preparation without blocking witness capture or marking skipped validation successful |
| Signer configuration/main/tests, three node Compose files, Kurtosis `main.star`, `.env.chiado.example` | Optional trusted L2 header endpoint and default service wiring; precedence/redaction/invalid-input tests |
| `crates/eez-testkit/src/{lib.rs,native.rs,ports.rs}` and native E2E modules | Owned ports, stable source across restart, existing independent-L1 mirror extended for reorgs, shared signer fixture and cross-role regression |
| `scripts/ci/{native-tests.py,tests/test_native_artifacts.py,tests/test_chiado_config.py}` | Include streaming in the existing release E2E inventory; configuration propagation checks |
| Cargo manifests/lockfile | Direct edges to already locked Alloy, getrandom, primitives and tokio-stream; no package/version/source/checksum change |
| Signer specs/contributor/docs, Chiado detailed runbook, AGENTS and proposal | Current trust boundaries, shutdown, mixed-version behavior, recovery and required checks |

## Compatibility and deviations

- Protocol gitlink, Solidity, compiler settings, genesis, commitments, native
  transaction/blob encoding, queue ownership and database schemas are unchanged.
  The older transcript provenance names core `9a1f815`; comparison with the
  pinned `abcf9d8` changes no Solidity source (only `src/TODO.md` under `src`).
  Historical fixture provenance remains intact; current validation uses the
  initialized pin.
- The daemon enables v2 only with `--l2-rpc-url` /
  `EEZ_PROOF_SIGNER_L2_RPC_URL`; absence serves v1, empty/invalid input fails,
  CLI wins over environment. Compose defaults to its existing local L2 RPC;
  Kurtosis uses the node's service RPC. The key remains signer-only. No deployment
  or service startup was performed through these configurations.
- Upgrade signer first, then Composer. Old matching-identity peers use v1.
  Clients remember unsupported v2 until restart. Rolling back a binary using
  the new CLI flag requires removing that flag. Sessions are volatile and are
  backfilled after eviction/restart; no queue persistence is introduced.
- The common wire has no validation-identity field. Fork clients send optional
  binary gRPC identity metadata, which the signer checks when supplied, and
  retain the existing capability handshake. Independently configured generic
  clients still use the common schema; all execution/signing rules remain local. Like upstream #206, the fork client
  requires bounded pipelining from its configured v2 peer; it is not a universal
  client for every minimal common-wire implementation.
- Session statuses and fixed capacities follow the already reviewed fork
  vectors where upstream differs. Client recovery recognizes the fork's
  FAILED_PRECONDITION range rejection without converting actionable failure
  details into a transport retry. Only the existing closed retry list escapes.
- Source retrieval uses bounded 2,048-block log pages under the operation
  deadline, not a new index/database. Deep or pruned histories can remain
  unavailable or miss a posting cutoff; they cannot authorize signing.
- Source tests use an isolated local debug build. Neither successful unit tests
  nor native Anvil process tests establish release-image provenance, Chiado
  deployment, Kurtosis/PBS topology, or a production latency guarantee.

## Fresh correctness review

The review re-read the complete runtime changes through their production
callers, session/client tests, and final upstream descriptions and discussions.
The common schema matches both final #198 and #206 after removing comments and
formatting; v1 tags and all saved native vectors remain unchanged. Fork #89 is
retained in branch ancestry rather than reconstructed.

Specific checks covered immutable admitted/validated/authorized boundaries,
native intermediate-block restrictions, exact blob and target binding, fresh
parent authentication, epoch checks at append/sign/publication, cancellation
while blocked on worker admission, permit ownership after timeouts, bounded
duplex queues, and recovery without changing canonical settlement ownership.
The review also checked source startup ordering, stable restart ports, the
independent L1 mirror, v1-only mutation/pruning fixtures, and CI module inventory.
No production validation gate was weakened. The final negative client test was
strengthened to exclude unrelated backend failures; its focused rerun passed.
The final documentation pass corrected a stale test comment about allocating
new L2 ports: the fixture now retains the signer's source address across
Composer restarts. That comment-only edit did not change the tested behavior.

## Validation limits and remaining risks

- The combined 45-case process command returned 143 despite every case printing
  PASS. Its cause remains unknown; the exact final restart case subsequently
  passed with exit 0 and identical executable hashes. Preserve both results;
  CI still needs a clean run of its complete published process suites.
- Release-image construction, artifact archive/relocation, the four-shard
  published native matrix, Kurtosis/PBS topology and live Chiado deployment were
  not exercised. No deployment, benchmark, production latency or release-byte
  provenance claim follows from these local checks.
- The full unfiltered workspace/process matrix was not run. The affected
  semantic, rejection, shared-signer/reorg, native follower and restart paths
  above were selected explicitly. Core tests retain the named exclusions and
  three fork-RPC skips; tooling retains six opt-in exclusions. The protocol
  Anvil runner and genesis regeneration check were not run: the protocol pin,
  Solidity sources, compiler settings and genesis are unchanged.
- V2 availability through a pruned canonical acceptance history remains
  unproven; unavailable evidence fails closed. The pruned restart test covers
  v1 compatibility, not a bypass of v2 provenance requirements.
- Existing dependency advisories and the unchanged baseline documentation
  anchor defect remain outside scope, as listed above. The initial shared-
  Composer fixture failure retains logs but lacks its datadirs and separately
  captured executable digest.

Historical vector-generation results from fork #89 are provenance only. The
current source's unit/transcript runs, static checks and process assertions
provide the implementation evidence recorded here. Raw logs, diagnostics and
datadirs under `artifacts/` are local-only and are not present in another checkout.
