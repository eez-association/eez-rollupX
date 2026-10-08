# eez-evm-inspector

Detects cross-chain proxy calls while an EVM transaction executes and turns
each one into a recorded, dispatched protocol call.

## Why a separate crate

Cross-chain composition needs to react to individual CALL opcodes, which only
an EVM inspector can see. That couples this code to `revm`, and the coupling
is deliberately confined here: [`eez-protocol`](../eez-protocol/) (types, ABI,
`CompositionBuilder`) stays EVM-free so verifiers and external tools can
consume it without the execution stack, while `eez-composer` wires this crate
into reth.

```text
eez-protocol            types, ABI, CompositionBuilder   (no revm)
     ↑
eez-evm-inspector       SessionInspector, overlay        (revm)
     ↑
eez-composer            reth integration, wiring
```

## How a call is detected and dispatched

`SessionInspector` hooks every mutable CALL during source simulation:

1. It reads `authorizedProxies[target]` from the **live** EVM state (revm
   journal + DB), so proxies registered earlier in the same transaction or
   block are visible — a pre-transaction snapshot would miss them.
2. If the target is a registered proxy, the call is forwarded through
   `CompositionBuilder::dispatch_call`, which executes it on the target
   rollup's session and records it for batch materialization.
3. The target's result is synthesized back into the source EVM as the CALL's
   outcome — a failed dispatch surfaces as `Revert`, so Solidity `try/catch`
   and revert accounting behave as if the call had run locally.

The inspector also brackets every EVM frame with the dispatcher's
recorded-call count. If a frame reverts after dispatching calls, that range is
annotated as a revert span, and batch materialization refuses to emit those
calls as successful entries.

## Nested re-entry: the overlay

A dispatched call may call back into the suspended rollup (flash-loan-style
patterns). Its live `State` is mutably borrowed by the paused EVM, so the
re-entered session cannot touch it directly. The overlay channel bridges that
gap with cache snapshots:

1. Before every dispatch the inspector opens a frame holding a
   journal-refreshed snapshot of its rollup's cache.
2. A session that re-enters the rollup opens preloaded with the innermost
   frame's snapshot, so it sees the in-flight state.
3. The re-entered session publishes its post-execution cache into that frame;
   a later sibling callback replaces it with its own cumulative cache.
4. When the dispatch returns, the inspector closes its frame. If the dispatched
   call succeeded and the frame has a result, the inspector applies the
   per-account, per-slot difference onto the live state as journal entries —
   so an outer revert unwinds the applied changes together with the source's
   own writes. A dispatch that did not re-enter the rollup has no result, and
   a failed one discards its result.

Frames are nested, so recursive re-entry stays paired with its dispatch and a
result is never applied by another dispatch. Mutations the diff cannot
represent (SELFDESTRUCT) fail loudly instead of miscomposing; code installation
and nonce changes are not propagated.

## Exports

- `SessionInspector` / `SessionInspectorFactory` — detection and dispatch; the
  factory is the only construction path.
- `OverlayChannel`, `OverlayChannelHandle`, `new_overlay_channel` — snapshot
  exchange for nested re-entry.
- `apply_overlay_diff`, `OverlayError` — the diff-apply primitive and its
  failure modes.

## Tests

```bash
cargo test --package eez-evm-inspector
```

The tests cover proxy lookup against live EVM state and overlay snapshot
handling. Dispatch behavior is exercised by `eez-protocol`'s composition tests
and `eez-node`'s cross-chain E2E suite.
