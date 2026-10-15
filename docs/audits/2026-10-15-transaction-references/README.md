# Cross-chain transaction references and fee API evaluation

Date: 2026-10-15 UTC. Implementation: `465cef299ce22ac9541ccfa3ba53d3fb7fca4490`;
native receipt test correction: `0dad223a`.
Fee API evaluation: `12fa33a7`; it is a draft, not a new endpoint implementation.
Baseline: `227b24d90a059a5af4a81b7a9fe29104bebd385e`. The initialized protocol
remained `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, without submodule edits.

Related contracts: [transaction references](../../proposals/cross-chain-transaction-references.md),
[compact fee quotes](../../proposals/compact-fee-quotes.md),
[queue observations](../../proposals/cross-chain-queue-liveness.md#local-terminal-observations).

## Findings and scope

The previous RPC exposed ownership but no physical transaction references.
The new process regression reproduced this against the previous deployed
Composer: the source was `Committed`, but `transactions` was absent. It failed
with the specific assertion `missing transaction references`, after successful
native settlement; this was not an RPC transport or fixture failure.

The extension maps each source through its physical checkpoint boundaries,
using final built transaction hashes. Outbound loads, signed sources and
callback deliveries stay together; inbound deliveries load their own tables
in one transaction. Other users in a shared batch are excluded. Pre-dispatch
poster repricing updates the posting reference; replacement generations and
finality pruning retain their existing ownership semantics.

These are references for the retained candidate attempt, not an assertion that
all candidates executed. The existing bounded in-memory lifetime remains.
The format retains individual chain IDs, but the current runtime supports one
paired L2; no foreign-rollup execution or archival index was added.

## Executables and evidence

Work used `/tmp/rollupx-l2-fees-e2e.8oFEZzNX`. The operator checkout's unrelated
changes and older protocol pin were preserved. Rust 1.95.0, LLVM 22, Foundry
1.7.1 and locked dependencies were used. No contracts, genesis, configuration,
Cargo dependencies or execution/fee policy changed.

[results.json](results.json) records executable identities and check results.
Raw build/test logs, test datadirs, the runtime source patch and frozen binaries
are local-only under `artifacts/fee-api-issue90-20261015/` in the operator checkout.
That ignored directory is not available from another checkout.

Runtime binaries were built before committing, from baseline `227b24d9` plus
`tracked-candidate.patch` (SHA-256 recorded in the results). Subsequent Rust
changes corrected tests and API comments only; production executable logic is
identical to the implementation commit. The runtime was frozen before building the test harness.
The baseline binaries are the earlier gas-cap deployment, identified separately
in [its audit](../2026-10-15-l2-fee-gas-cap/README.md).

The initial test-harness compiler was killed with SIGKILL under memory pressure.
A two-job retry remained in node-library optimization and was stopped explicitly.
The successful local harness build used two jobs and
`--config 'profile.dev.package.eez-node.opt-level=0'`. This command-line override
was not committed and did not change the frozen runtime binaries launched by
the process tests. The corrected harness was rebuilt with the same override.

## Executed checks

Runtime build:

```bash
cargo +1.95.0 build --locked -p eez-node -p eez-follower -p eez-proof-signer --bins
forge build --root contracts
```

Unit checks used `cargo +1.95.0 test --locked --lib` for `eez-protocol`,
`eez-blob-codec`, `eez-payload-codec`, `eez-composer`, and `eez-node`, with the
same local node-library optimization override. They passed **656 tests**:
188 protocol, 21 blob codec, 15 payload codec, 346 Composer, and 86 node.
One existing manual backlog timing profile was ignored; it is not coverage.
After correcting the inbound-only fixture, the protocol's 188 tests passed again.
The SDK's 13 tests passed. Workspace Clippy with all targets and `-D warnings`,
Rust formatting, and `git diff --check` passed after the test corrections.

The process harness was built with:

```bash
cargo +1.95.0 test --locked \
  --config 'profile.dev.package.eez-node.opt-level=0' \
  -p eez-node-e2e --test node_e2e --no-run --message-format=json
```

It runs with `EEZ_TEST_PREBUILT=1`, `EEZ_TEST_RUNTIME_BIN_DIR` pointing to the
frozen runtime directory, `EEZ_TEST_PROOF_SIGNER_BIN` pointing to its signer,
and explicit `EEZ_TEST_LOG_DIR` / `EEZ_TEST_DATADIR_DIR`. Thus no fixture can
select stale host executables or start a background Cargo build. The baseline
uses its separately identified previous-runtime directory with the same test.

The selected candidate modules are `transaction_references::`,
`chained_interstate::`, `fee_quotes::`, `partial_consumption::`, and `queue_reorg::`,
with `--test-threads=1 --nocapture`; the substring fee filter includes
`safe_fee_quotes::`. Selection contains 19 tests and no ignored tests.

The first candidate run exposed an incorrect new test expectation: inbound
delivery is one self-loading transaction, whereas outbound work has a load/user
pair. The mixed and inbound-batch assertions initially counted extra loads.
The nested inbound expectation had the same mistake. Those expectations were
corrected against the pinned native system-transaction builder, preserving exact
per-source membership and receipt assertions. First failures and datadirs were
retained. Both new nested tests actually stopped earlier at receipt decoding:
Ethereum's closed receipt enum rejected the existing native type `0x76`, despite
the RPC returning the correct successful receipt. Their L2 providers now use
Alloy's `AnyNetwork`; the API contract documents that client requirement.
A subsequent Clippy finding in the new singleton-slice assertion was also
corrected. These are test-code corrections, not unexplained retry passes.

After those fixes, the outbound nested test reached a further incorrect count:
its reentrant callbacks execute inline inside the source transaction through
`EEZL2._executeEntry` / `_processIncomingCalls`. The real block contains two
transactions (load and source), not four. The final test requires those exact
hashes, successful canonical receipts, completed source execution and the
expected L1/L2 state. No extra hashes are invented for internal EVM calls.

The initial 19-test run passed 15 and exposed the four new test failures above.
The corrected four-test run passed three; the last inline-callback count was
then fixed and its focused run passed. All **19 distinct intended process
regressions now have passing evidence**, against the same frozen runtime.
There were no runtime-code changes or unexplained retry-only passes between
these runs. `results.json` preserves the per-run outcomes and executable hashes.

## Limits and operations

No deployment, public transaction, contract broadcast to Chiado, wallet funding,
or operator service restart was performed. Native tests use isolated local
fixtures and exercise semantic assertions; fee/Safe and recovery suites include
independent follower replay. No new Kurtosis topology/PBS run, full protocol
staged workload, Falcon signature workload, or complete CI release matrix ran
for this observational RPC change. The proposed fee API is not implemented by
these changes; the existing quote RPC and SDK interface remain in use.
