# Quote trace viewer

Open [index.html](index.html) in a browser and import a debug quote JSON file, or
paste its contents. All parsing and display stay in the browser. There are no
RPC calls, uploads, analytics, CDNs or runtime dependencies.

For browsers that restrict local scripts, serve this directory locally:

```bash
python3 -m http.server 8000 --bind 127.0.0.1 --directory tools/quote-trace-viewer
```

Then visit `http://127.0.0.1:8000`. The file picker accepts either the entire
JSON-RPC response, its `result`, or the `diagnostics.trace` object alone. The last
form has no quote outcome; the viewer explicitly says so. Imports are limited
to 8 MiB and require trace version 1.

The dashboard can bundle these same static assets and load
`index.html?embedded=1` in an iframe with `sandbox="allow-scripts allow-downloads"`.
Embedded mode hides the standalone brand header. Keep the sandbox's origin
isolation and the viewer's content policy so imports cannot access dashboard
wallet state or make network requests. No parent-page messaging is required.
The file picker is keyboard accessible. If file reads overlap, only the most
recent import or paste can update the displayed result.

For a recorded example, import [nested-revert.json](examples/nested-revert.json).
It comes from the local nested-call process test on 2026-10-20 using Composer
SHA-256 `cc1378765533dd14952905e716b43bc9a17d0a346e2daf026a4e23af7e465820`
and protocol `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774`. It contains only public
fixture accounts and synthetic local state. The L2 source catches an L1 revert
after a successful L2 callback: source execution succeeds, while the quote
rejects that topology because reverted cross-chain calls with descendants are
outside the current settlement profile.

## Capture with cast

Run against the Composer front for the transaction's source chain. `/composer/l1`
quotes an L1 source; `/composer/l2` quotes an L2 source. Replace the sample
addresses, calldata and public endpoint:

```bash
COMPOSER_RPC='https://YOUR_HOST/composer/l2'
TX='{"from":"0x1111111111111111111111111111111111111111","to":"0x2222222222222222222222222222222222222222","data":"0x","value":"0x0"}'
cast rpc --rpc-url "$COMPOSER_RPC" eez_debugQuoteFee "$TX" \
  '{"transactionTypes":["0x2"],"trace":"calls"}' > quote-trace.json
```

`gas` and `nonce` remain optional. Request `["0x0"]` for legacy, or omit
`transactionTypes` to calculate both formats. Each format executes independently.
Tracing is supported only by `eez_debugQuoteFee`; the regular quote method
rejects the option. Older Composer versions reject it too.
Send traced quotes individually: batches containing a traced quote are rejected
with `-32600` before execution to bound response accumulation.

## Reading an export

The quote summary is separate from individual simulation outcomes. Gas discovery
can intentionally retry a failed probe. Choose a run to inspect its own call
tree; do not concatenate iterations as if they were one transaction.

- `composition` links actual synchronous EVM instances across both chains.
- `native_source` replays the user transaction under native execution rules.
- `native_system` records system table loading or delivery.
- `conditional_setup` and `settlement` use assumed/metered proof execution for
  estimation. A successful run does not establish proof verification.

Click a call for caller, target/storage, code address, input, output, gas and
error details. Click its arrow to expand or collapse its children. Search by
address, selector, call kind or decoded error. A successful call beneath a
failed ancestor is marked as rolled back. A failed inner call beneath a
successful root is marked as a failure inside a successful simulation.

L1 and L2 gas have different prices. Frame gas is inclusive EVM execution gas,
excluding intrinsic transaction gas, refunds and transaction-level gas floors.
Do not sum nested frames or repeated runs to calculate a fee. Use the quote's
existing cost diagnostics for pricing.

The viewer decodes complete standard `Error(string)` and `Panic(uint256)`
payloads. Custom errors keep the unknown selector and raw bytes. ABI upload,
source maps, variables, state diffs and opcode stepping are outside this version.
The export is a simulation record, not a mined receipt, proof or replay witness.
The snapshot can include pending predecessor effects; a later request can run
against different state. RPC-level failures may not return an export.

## Version 1 schema

The authoritative RPC behavior is in the
[fee quote contract](../../docs/configurable-settlement-policy.md#compact-fee-quotes).
`diagnostics.trace` contains:

| Field | Meaning |
| --- | --- |
| `format`, `version` | `eez-quote-call-trace`, `1` |
| `request` | Original caller-supplied transaction |
| `context` | L1/L2 chain IDs, pinned parents, L1 target, L2 timestamp, resolved nonce and pending predecessor count |
| `runs[]` | Ordered executions, each with an ID, quoted type, phase, pass, purpose, chain ID, actual transaction, `complete` and `frames[]` |
| `truncated`, `omittedFrames`, `omittedRuns`, `limits` | Explicit capture limits and omissions |
| `gasAccounting` | `inclusive_frame_execution_gas` |

Run IDs are zero-based and contiguous. Frame IDs are zero-based within each run.
`pass` increments when composition begins within a phase/type; its subsequent
native replays carry the same pass. The run's quoted `type` and actual
`transaction.type` can differ for synthetic/system executions. Transaction fee
fields retain their legacy versus EIP-1559 meaning.
`parentId` is null for a run root, otherwise an earlier frame in that run.
`crossChain` identifies a different chain from the parent; `executionRoot`
identifies a new EVM instance, including synthetic remote calls. Frames distinguish
`from`, `to` (target/storage), `codeAddress`, `kind`, `isStatic`, `value`, forwarded
`gas`, `gasUsed`, `status` and the EVM `result`. CREATE addresses come from the
observed outcome and can be null on failure.
`from` is the EVM caller (`msg.sender`), which DELEGATECALL preserves. Use
`parentId` to identify the frame that issued a call, and its code/storage
addresses to distinguish a library from the contract using it.

`input` and `output` are `{hex, length, truncated}` objects. `length` is the
original byte count; `hex` may hold only a prefix. A frame that did not complete
has `status: "incomplete"` and may have no output/result/gas-used fields. Run
`complete` means all observed frames ended; it does not override global
truncation or imply success. Context and IDs use JSON integers where bounded;
chain IDs, transaction quantities, values and gas use hex strings.

The collector keeps at most 128 runs, 4096 frames, 1 MiB of raw captured data
and 4096 bytes per field. It continues the quote when capture is full. No
collector is allocated for ordinary quotes or admission. One scoped collector
lives on the blocking quote worker; synchronous callbacks share it. Guards
restore the prior scope on success, error or unwinding. Queue ownership and
worker-permit cancellation rules are unchanged.

## Tests

From the repository root:

```bash
npm ci --ignore-scripts --prefix tools/quote-trace-viewer
tools/quote-trace-viewer/node_modules/.bin/playwright install chromium
npm test --prefix tools/quote-trace-viewer
npm run test:browser --prefix tools/quote-trace-viewer
```

The browser test checks local-file import, navigation, caught failures, ancestor
rollback, truncation, invalid input, hostile text, export and absence of network
requests. Synthetic unit fixtures do not establish Composer behavior; the
checked-in nested-revert example is an actual export with the provenance above.

The source-based process regression is
`fee_quotes::tracing::nested_call_traces_link_both_directions_and_preserve_quote_results`.
Follow the repository's source-based E2E prerequisites and build current role
binaries first. When `EEZ_TEST_LOG_DIR` is set, it retains real responses in
`$EEZ_TEST_LOG_DIR/quote-traces/`. Run the browser test with that same environment
to verify every retained process export and save a screenshot under the ignored
`test-results/` directory.
