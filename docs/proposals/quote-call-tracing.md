# Quote call tracing

| Field | Value |
| --- | --- |
| Design status | Accepted and implemented. The 2026-10-27 amendment moves viewer ownership and browser validation to the dashboard repository; RollupX retains the trace API and backend tests. |
| Date | 2026-10-20 |
| Source baseline | `eez-association/eez-rollupX` main `a2d7b0a3be89df3c0b6ac945acd41a830278b0a7` |
| Protocol baseline | `abcf9d8ffc4e0ccb5df271fc19efdf9a7fb57774` |
| Related work | [Fee quote contract](../configurable-settlement-policy.md#compact-fee-quotes) |

## Problem and scope

Debug fee quotes need the internal call context that produced revert bytes.
Opt-in call tracing on `eez_debugQuoteFee` captures actual quote execution,
including nested cross-chain callbacks. The dashboard owns rendering and import;
this repository owns trace capture, the export contract and backend validation.
Solidity source maps, opcode traces, state diffs, remote storage and transaction
broadcast are outside this iteration.

## Requirements and acceptance

| ID | Behavior | Acceptance |
| --- | --- | --- |
| R1 | Only debug quotes accept `trace: "calls"`; omission preserves current output and execution | Parser/RPC tests, quote comparisons |
| R2 | Explicit frame parents link ordinary calls, creation and synchronous cross-chain execution | Nested L1/L2 process tests and inspector tests |
| R3 | Reverted frames remain inspectable, including caught failures and successful calls unwound by ancestors | Caught and propagated nested-revert regressions |
| R4 | Types, gas probes, pricing and validation executions are separate runs with pinned context and actual transaction fields | Both transaction formats and omitted/explicit gas tests |
| R5 | Tracing observes execution without changing gas, output, state, queue ownership or worker lifetime | Traced/untraced equivalence tests and existing fee process suite |
| R6 | Capture is bounded; missing/truncated evidence is explicit; traced RPC batches are rejected before execution | Boundary and batch rejection tests |
| R7 | Dashboard imports are bounded and stay in the browser; contract text cannot execute scripts or initiate network requests | Import and browser tests in the dashboard repository |
| R8 | Operational failures after capture starts retain the partial trace without returning successful quote options | Unfunded-poster nested process tests and error-envelope assertions |

## Design

The existing bounded blocking quote worker owns an in-memory collector. A scoped
thread-local connects inspectors across synchronous, reentrant EVM instances;
it is restored on every exit, including unwinding. No trace survives into another
request. Inspectors bracket the existing inspector, observing final outcomes
after its hooks; they do not replace execution decisions. The regular quote and
signed admission paths do not allocate a collector.
Traced requests must be sent individually. The RPC front rejects a batch
containing one before executing any member, bounding accumulated trace replies.

The export has a versioned format, original request, simulation context and
separate runs. Each run identifies its phase, transaction format and execution
purpose. Frames have stable IDs, parent IDs, chain identity, code/storage
addresses, call type, bounded input/output, status and inclusive frame gas.
Cross-chain roots attach to the suspended source call. Native replays remain
separate from composition and are never presented as a single mined transaction.
Gas is not summed across nested inclusive frames or across quote iterations.

If calculation fails after capture starts, the RPC error retains the collector
at `error.data.trace`, with `partial: true` and a sanitized `failure` summary.
This does not retain pending predecessor bodies, change the error category, or
return earlier successful quote options. Failures before capture and worker
timeout, cancellation, shutdown or panic may still have no trace. The bounded
worker retains its permit until it exits; capture does not change that lifecycle.

The dashboard consumes the [export schema](../configurable-settlement-policy.md#compact-fee-quotes),
including partial exports inside error responses. Consumers must distinguish
the quote outcome from individual run/frame outcomes and retain unknown error
selectors and bytes. Viewer implementation and browser dependencies live in the
dashboard repository.

## Compatibility and recovery

This is an additive debug-only option and response field. Older Composers reject
the option; users must upgrade Composer before requesting traces. No protocol gitlink,
contract, genesis, signer, follower, storage or operator configuration change is
needed. Rollback removes trace availability without changing historical replay
or fee enforcement. A trace describes simulated execution, not a proof or a
replay witness; a later request can use a different context.
The error-envelope extension retains trace schema version 1. Removing the
standalone viewer does not change that schema or the dashboard's bundled assets.

## Validation

Run Composer/node unit tests, fee SDK tests, serial fee quote process tests with
nested calls in both directions, formatting and Clippy. The dashboard repository
owns browser validation of import, navigation, errors, truncation and malformed
data. The original implementation's checks and limitations are recorded in the
[validation record](../audits/2026-10-20-quote-call-tracing/README.md).
The [posting-reservation follow-up](../audits/2026-10-20-mainnet-quote-reservation/README.md)
records the partial-error-trace regression and its validation separately.
This design document alone is not validation evidence.
