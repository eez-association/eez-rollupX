# N13: L1 tracing coverage under the restricted wrapper profile

Date: 2026-11-12. Investigation and optional diagnostic for
[#147](https://github.com/eez-association/eez-rollupX/issues/147).
No functional runtime defect was established. The useful missing observation is
an explicit **nonempty, hash-bound block callTracer response** checked against a
receipt. The new [probe](../../../testing/kurtosis/scripts/probe-l1-tracing.py)
provides that observation without becoming a startup requirement.

## Identities and scope

| Item | Exact identity |
| --- | --- |
| Frozen N13 source | `eez-association/eez-rollup0` at `1b66d9bda675e6f8e83f514153a97136909acc06`; protocol `143d87e62cade98bfc122d612638dcaaf1c85633` |
| Frozen N13 target | `eez-association/eez-rollupX` at `31e148f50dd79730b17d113ac78795eb377eb5c5`; protocol `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Earlier issue current-code check | `716a8a3bad4415ade7e9a6bc3af4a1b35419b232`, same target protocol |
| Implementation base | Association main `601ebfa9c7e1a779f1afbd077bd31a3207e7a61b`, same target protocol |
| Source PR #210 | Base `99d3d7ef24114aa0552507c634b0da98820933ec`, head `8709d738b79400cdc3156fc8add25b8ae050b032`, merge `46515417dcd88c9c789e916ff12e9a5f2ffa8b5b` |
| Local checkout | Dedicated `issue147-tracing-probe/repo`, branch `codex/issue147-tracing-probe`; initially clean; protocol initialized recursively at its pin and unchanged |

The original review's **input provenance was not independently checked**. This
follow-up reuses N13's frozen identities and issue evidence; it does not restart
#133 or qualify unrelated source changes. The source protocol has different scan
and event capabilities; source implementation parity is not an accepted contract.

Inspected overlaps: #142 / [PR #163](https://github.com/eez-association/eez-rollupX/pull/163)
changes submission retries in `lib.sh`, its fixtures and the Kurtosis README.
#144 owns missing-blob/finality observations; #146 owns accounting evidence.
This change edits none of those shared paths or runtime components. The trace
probe compares event bytes; it makes no blob availability, accounting, settlement
completion or durable recovery claim. No other worktree/output was touched.

## What the frozen source probe actually establishes

The [frozen 12s script](https://github.com/eez-association/eez-rollup0/blob/1b66d9bda675e6f8e83f514153a97136909acc06/testing/kurtosis/scripts/12s-l1-up.sh#L92)
resolves the **external Kurtosis EL** `el-1-reth-lighthouse`. Its lifecycle code
removes an enclave, sets up forwarders and publishes endpoints. It does not probe
the Composer's embedded Ethereum/Gnosis EL. This investigation never runs that
lifecycle script.

After resolving enode/CL identity, it tries 60 times, sleeping two seconds:

1. `cast block-number` (`eth_blockNumber`) must yield a decimal height >= 1.
2. `debug_traceBlockByNumber("latest", {"tracer":"callTracer",
   "tracerConfig":{"withLog":true}})` must exit successfully.
3. It discards stdout/stderr, breaks on success, and otherwise fails startup with
   the last observed height and a debug-namespace diagnostic.

There is **no assertion on response contents, logs, frame type, transaction
identity, nonempty selection or canonical block hash**. An empty non-genesis
block can pass. The head read and `latest` trace can refer to different blocks.
The nominal 120-second sleep budget excludes RPC duration; the script does not
set an overall deadline. A failure can mean missing debug, unavailable parent
state, timeout, network error or tracer rejection; its message does not classify
those causes. These are source observations, not a reproduced backend failure.

Its real caller purpose is broader than that smoke check. The frozen
[scanner](https://github.com/eez-association/eez-rollup0/blob/1b66d9bda675e6f8e83f514153a97136909acc06/crates/eez-l1/src/scan/mod.rs#L175)
reads direct calls from outer calldata and traces contract posts once per block.
The [fetch helper](https://github.com/eez-association/eez-rollup0/blob/1b66d9bda675e6f8e83f514153a97136909acc06/crates/eez-l1/src/fetch.rs#L21)
actually calls `debug_traceBlockByHash`, requests the whole tree and logs, with a
60-second trace timeout, selects needed transaction hashes, requires callTracer
frames, distinguishes method-not-found from lag and retryable trace errors.
The source [scan contract](https://github.com/eez-association/eez-rollup0/blob/1b66d9bda675e6f8e83f514153a97136909acc06/docs/l1-batch-scan.md#calls)
uses committed frame calldata/sender/log ranges for routers that can build or
rewrite calldata, and drops reverted subtrees. The startup check does not test
those assertions.

## Mapping to this fork

Paths below were traced at implementation base `601ebfa9`; the frozen target
already has the explicit HTTP selection and log/outer-calldata scan.

| Source behavior / question | Target contract, wiring and checks | Disposition |
| --- | --- | --- |
| Enable embedded HTTP debug | [Shared builder](../../../crates/eez-node/src/l1_embedded.rs) explicitly selects `eth,net,web3,debug,trace`. Testing, Devnet/Ethereum and Chiado/TestingGnosis builders all use it. [Production launch](../../../crates/eez-node/src/lib.rs) uses those builders and `launch_with_debug_capabilities()` in every arm. Existing `embedded_l1_http_rpc_exposes_classifier_trace_method` checks configuration. | Equivalent source wiring. Configuration and launch source do **not** prove live behavior of every method/backend. No extra API exposure needed. |
| Trace arbitrary contract-generated posts and distinguish internal calls by logs | [Target scan](../../../crates/eez-l1/src/scan.rs) queries address-filtered `BatchPosted` and outcome logs, fetches outer transactions by block hash/index, checks their hash, decodes canonical outer `postAndVerifyBatch` calldata and matches verified rollup IDs. Its consumers are [reader](../../../crates/eez-l1/src/l1_reader.rs), [watcher](../../../crates/eez-l1/src/l1_watcher.rs) and derivation. | Intentionally different. [Signer §1.2](../../../crates/eez-proof-signer/SPEC.md#12-scheduling-boundary) and the [accepted follower scope](../../proposals/full-protocol-e2e.md#issue-follow-ups-2026-11-09) require verbatim canonical outer calldata and native blobs. Arbitrary wrapper ABIs, rewritten/trailing calldata and reordered/additional blobs remain excluded. Do not transplant the broader source scanner. |
| Relevant wrappers and malformed outer input | Existing scan tests include `undecodable_foreign_batch_is_skipped_but_our_no_effect_post_is_fatal`, rollup-list disagreement, hash/index source incompleteness and fork-pinned settlement windows. [Retuned-dispatch process tests](../../../crates/eez-node-e2e/tests/cross_chain/retuned_dispatch.rs) exercise the supported meta poster and canonical follower replay. | Equivalent profile-specific checks exist. Inspected here; Rust/process tests were not freshly executed. Trace parity is not a prerequisite for this restricted replay. |
| Actual target block trace consumer | [Posting priority history](../../../crates/eez-composer/src/composer/posting/priority.rs) requests `debug_traceBlockByHash` with `callTracer`, timeout `4s`, **without** `withLog`, when paid transactions need registry-touch classification. It checks complete tx-hash coverage and traverses reverted frames too: a caught revert can still identify a service payment. | Useful uncovered observation: a nonempty canonical block trace, distinct from configuration or transaction tracing. The optional new probe checks that method and additionally verifies log content for diagnostic use. Missing `withLog` alone would not establish a defect in this consumer. |
| Front classification and existing semantic traces | [Ingress](../../../crates/eez-node/src/ingress.rs) uses `debug_traceCall` (`pending` for L1); front upstream URLs are separately configured and chain-ID checked in [node wiring](../../../crates/eez-node/src/lib.rs). Submission/reader RPC comes from `EEZ_L1_RPC_URL`; embedded execution uses its local provider. Existing lookup collectors use `debug_traceTransaction` and bind specific proxy/registry calls and return/revert bytes. | Complementary existing coverage. These are different RPC methods and can use different endpoints. Neither they nor the new probe establish every backend's runtime behavior. |
| Pruned canonical header authentication | [Derivation fetch](../../../crates/eez-l1/src/derivation.rs) only falls back on code `4444`, requesting `debug_getRawHeader` at the same canonical number/tag, exact-decoding RLP and checking hash/height. [Tests](../../../crates/eez-l1/src/derivation/tests.rs) cover malformed/trailing RLP, wrong height/hash, unavailable methods and wrong error codes; a socket test checks the wire selector. [Recovery guide](../../runbooks/ethereum-node.md#recovery-with-a-pruned-l1) owns retention limits. | Complementary, not block-trace equivalence. A retained header can work while tracing fails for lack of historical execution state. Existing raw-header tests were inspected, not rerun; the new probe does not claim to exercise fallback. |
| Fresh embedded Ethereum/Gnosis and real PBS topology | Supported modes include public Ethereum, private Devnet, Testing, Chiado and TestingGnosis. The source startup smoke targets only the external Kurtosis Ethereum EL. | Evidence still missing: executing the optional probe on each identified shipping backend with retained nonempty fixtures. Owned Anvil validates the diagnostic, not embedded Reth/Gnosis, consensus, finality or PBS. |

## Optional probe and limits

Against an **owned test endpoint** with a known successful mined transaction that
emits at least one log and retains its execution state:

```bash
python3 testing/kurtosis/scripts/probe-l1-tracing.py "$OWNED_L1_RPC" "$KNOWN_LOG_TX"
```

Use the regular node RPC, not a Composer front. The probe sends only receipt,
block and trace reads. It sends no transaction, changes no API configuration and
has no enclave discovery/default. It requires a nonempty receipt; binds its
transaction to the hash/index in the containing block; requests a full hash-bound
block call trace with logs; checks complete ordered transaction-hash coverage;
compares the selected transaction's **multiset** of committed `(address, topics,
data)` logs with the receipt; then rechecks canonicality by height. Reverted
subtrees cannot supply committed logs. Log order/frame attribution, other
transactions' log contents, protocol calldata, blobs, gas accounting and execution
replay are outside this check. It trusts the queried endpoint's evidence rather
than independently authenticating a chain.

RPC reads time out after ten seconds each and responses are capped at 32 MiB.
Tracer execution is requested with `4s`, matching the target fee consumer. A
large/busy backend may fail this bounded diagnostic without proving a runtime
defect. Failures return nonzero without printing remote error text or RPC URLs.
No new runtime dependency, wrapper support, validation relaxation, startup gate,
storage/wire format, upgrade order or migration is introduced.

## Executed evidence

Environment: Linux x86_64, Python 3.14.4; Foundry **1.7.1**, commit
`4072e48705af9d93e3c0f6e29e93b5e9a40caed8`, dist profile.

| Executable | SHA-256 |
| --- | --- |
| `/usr/bin/python3` | `52e0a13e60a981d8c4b6478be2ba5176f69da07948a056bf49cf6f077e30cb41` |
| `anvil` | `10c1c727d6c1de973aeb160e59875b9a9a23464d6e74149ee8abb30b3500311b` |
| `cast` (identity only; probe does not need it) | `ccd95a4607ca3ebfcb88bb90e7235cdb7f0564f5f1afa17478d5fccabbb222cb` |

[Regressions](../../../scripts/ci/tests/test_l1_tracing_probe.py) exercise the actual
Python CLI through loopback HTTP, including exact RPC options and selectors.
The backend test starts its own Anvil on port zero, discovers its listener from
that process's log, deploys a tiny LOG1 fixture through its unlocked public test
account, calls it, and verifies one real event. A plain transfer is the negative
control for an empty receipt. Only that process is terminated. No production
service, shared enclave, wallet or other task's output is used.

The first focused run selected nine tests and passed in 12.475s. A later
sandboxed invocation selected nine but failed with 24 socket-creation errors
(`PermissionError: Operation not permitted`) before reaching RPC or Anvil.
That is an environment failure, not a backend result or a flaky assertion.
The same suite outside the socket-restricted sandbox passed in 13.213s. Final
port-isolation validation and other focused checks are recorded below.

No baseline runtime failure comparison is claimed: the optional script did not
exist at the requested base, and absence of a script is not a runtime defect.
Its negative controls establish the new diagnostic's failure behavior.

Local-only diagnostics are under ignored `artifacts/issue147/`: first test log,
sandbox failure, malformed-HTTP first failure, subsequent check logs, Anvil log
and small probe JSON. They are not retrievable from another checkout. The checked-in fixtures/test code and
results below are the reproducible evidence, not a promise to retain bulk logs.

All commands ran from the dedicated repository root:

| Check | Actual selection and outcome |
| --- | --- |
| `python3 -m unittest discover -s scripts/ci/tests -p 'test_l1_tracing_probe.py' -v` | Nine selected, nine passed, zero skipped on the final reviewed files (13.929s). Includes the real Anvil method/log assertion and HTTP negative controls reaching that same method. |
| `python3 -m unittest discover -s scripts/ci/tests -p 'test_protocol_lookup_trace.py' -v` | Seven selected and passed, zero skipped. Captured trace fixtures plus mutations; not fresh chain execution. |
| `python3 -m unittest discover -s scripts/ci/tests -p 'test_native_artifacts.py' -v` | 35 selected: 34 passed, one skipped. Archive selection/consumer tests are mocked; real tiny-workspace relocation is opt-in via `EEZ_TEST_NATIVE_ARCHIVE=1` and was not run. No native compilation or shipping artifact execution. |
| `python3 -m unittest discover -s scripts/ci/tests -p 'test_workflow_contract.py' -v` | 21 selected and passed, zero skipped (3.560s); static/mocked CI selection and gate behavior. |
| `python3 -m unittest discover -s scripts/ci/tests -p 'test_protocol_lookup_evidence.py' -v` | 33 selected and passed, zero skipped (194.691s). Real shell/jq collectors with mocked RPC/KZG/Forge boundaries; no live network or independent replay. |
| Default CI unittest discovery | Confirmed the unchanged `.github/workflows/ci.yml` discovery command selects all nine new tests, including Anvil. No workflow or native-suite inventory change is necessary. |
| Python compilation, `git diff --check`, local Markdown path resolution | Passed. The only shell example was checked with `bash -n`; no shell file was edited. |

The new probe is an optional manual consumer. CI executes its HTTP/Anvil tests
through ordinary tooling discovery. The existing `node_e2e` embedded and
cross-chain suites remain in their original verified artifact lanes. Neither
mocked archive selection nor a configuration test is described as live RPC
validation.

Resource checks found only 430–680 MiB free on the repository filesystem during
initial inspection (later as low as 157 MiB), roughly 6 GiB free on `/tmp`, about
15 GiB memory available and swap almost full. No Cargo/native build was started;
no output owned by another task was pruned. Rust formatting/clippy/package/E2E
builds and protocol/Solidity suites were not run for this Python/documentation
change. This leaves the existing Rust backend tests as source evidence only.
No full tooling discovery execution, release/Kurtosis matrix, embedded
Ethereum/Gnosis process validation, independent replay, performance comparison,
vulnerability scan or live-network procedure was run. Configuration files did
not change; Compose render suites were not run (host Compose is 5.7.0, whereas
those suites specify 5.5.1). Complete CI remains the owner's validation lane.

## Review and conclusion

Applied Ponytail's current coding and review instructions from
[`DietrichGebert/ponytail`](https://github.com/DietrichGebert/ponytail/tree/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills),
read on 2026-11-12. Self-review traced callers, HTTP evidence, error sanitization,
CI discovery, backend ownership and existing collectors. It tightened malformed
response handling and had Anvil allocate/announce its own port rather than
releasing a reserved socket before launch. A concrete negative control then
reproduced a malformed HTTP status line escaping as `BadStatusLine`, with remote
text in the traceback (one selected test, one failing subcase). Catching
`http.client.HTTPException` at the RPC boundary fixed it; the final nine-test run
includes that same control and passes without a traceback or remote text. The
final review found no remaining must-fix or should-fix finding. No delegated or
independent review is claimed. The diagnostic serves one explicit invocation at a
time; its bounds are diagnostic limits, not node admission rules.

The recommendation is to retain the restricted runtime profile and this opt-in
probe, without copying the source fatal startup check. Fresh qualification on
identified embedded Ethereum/Gnosis shipping binaries remains separate evidence
to collect when an owned prepared environment is available. A failure of a real
runtime consumer would justify a further narrow fix; source parity or an empty
smoke check would not. Parent main CI `43237662996`, issue #133, board metadata,
production services and wallets were left untouched.

Final Python source identities (SHA-256):

- `probe-l1-tracing.py`: `5a4351ae8e1fa4c685fe3f8134d4c8f7a2cbb52857153a9077f91938a494370e`
- `test_l1_tracing_probe.py`: `9704cbf43f04265029391f5c62d78089b55dcd2267a767f6138a7e0e2773a1ca`

Ponytail final review: **What this change does:** Adds an optional read-only
canonical block trace/log diagnostic and its isolated regressions. **Looks good.
Ship.** Checked the full diff, target callers, error handling, nonempty selection,
CI discovery and process ownership. **Not checked:** embedded shipping backends,
full-network semantics and independent replay.
