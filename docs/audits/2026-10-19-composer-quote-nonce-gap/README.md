# Composer quotes can misclassify a lagging L1 snapshot as a nonce gap

Status: confirmed against the unchanged release runtime on 2026-10-19.
This is an investigation and reproduction, not a deployed fix.

| Identity | Revision or reference |
| --- | --- |
| RollupX checkout | `b488f7a8c46cbc69f71322583d1edbbdfb108a26`, `feat/pr54-kurtosis-fees` |
| Protocol pin | `f46af7c9833ac12a54f14d52f4da1f318a0f6525` (PR 54) |
| Runtime source | `110f952143853d0edbc51d6d8c2cb67be4d828f3`; affected production sources match the checkout |
| Node image | `ghcr.io/eez-association/eez-rollupx-node@sha256:2fc8c6e121d1bd2116a0f78766ebd7c99b305e8e01720a2c91e693b21908a0b6` |
| Full runtime/deployment provenance | [Previous validation record](../2026-10-19-kurtosis-fees/runtime-provenance.json) |
| Isolated enclave | `eez-nonce-gap-20261019`, L1 7331 / L2 696990, both fee routes enabled |
| API contract | [Advisory quotes and worker bounds](../../configurable-settlement-policy.md#cross-chain-fee-configuration) |
| Prior finding | [Initially unattributed nonce-gap evidence](../2026-10-19-kurtosis-fees/nonce-gap-evidence.json) |

A read-only inspection of the current `eez-node-chiado` container returned the
same node image reference and image ID. The reproduction therefore exercises
the currently deployed Composer bytes, using isolated private-chain state.
No transaction was sent to Chiado and its service was not changed.

## Confirmed behavior

A mined ordinary L1 transaction can advance the account nonce before the L1
watcher/deriver finishes scanning that block. The quote intentionally uses the
last completed, canonical L1/L2 pair. However, the L1 nonce-prefix path treats
the older account nonce as if it described current source readiness:

- With an explicit current nonce, it looks for the already-mined ordinary
  transaction in Composer's held pool. Those bytes are not owned there. It
  returns `-32602`, `invalid_transaction`, `queued nonce prefix has a gap`.
- Without an explicit nonce, it can return successful wallet fields using the
  already-spent nonce from that older snapshot.

This is a Composer snapshot-freshness/error-classification bug. It is separate
from PR 54's classification of temporary `-32004` errors. Fixing the protocol
runner's retry logic alone does not fix this invalid-input response or the
successful-looking stale-nonce quote.

The held pool's contiguous-prefix requirement is correct. The error is using
it without accounting for source state that advanced beyond the advisory
snapshot. The correction must preserve genuine gaps, predecessor ownership,
the eight-predecessor bound and native execution validation.

## Reproduction and controls

The [successful reproduction script](reproduce.mjs) uses the standard fee-enabled
profile and two prefunded private-network fixture accounts. One deploys a
stable L2 setter and its L1 proxy, obtains a successful baseline quote, then
mines an ordinary zero-value self-transfer. A second account submits a quoted
cross-chain call, creating a native blob settlement in the same L1 block.
Only the companion cross-chain transaction is broadcast; the tested source's
cross-chain calls remain unsigned quotes.

| Observation | Result |
| --- | --- |
| Six ordinary-transfer control rounds without the companion settlement | No gap reproduced; explicit and omitted nonces matched current state |
| First round with a native settlement and ordinary self-transfer in one L1 block | Gap reproduced |
| Canonical L1 block 40, read RPC and Composer front | Source nonce `10` |
| Quote's sampled L1 block 39, independently read by block | Source nonce `9` |
| Explicit nonce `10` | `-32602 / invalid_transaction / queued nonce prefix has a gap` |
| Concurrent omitted-nonce request | Successful option with nonce `9`, bound to block 39 |
| Identical explicit nonce-10 request after catch-up | Successful quote with nonce `10` |
| Genuine future nonce `11` after catch-up | Still rejected as a nonce gap |
| Compact `eez_quoteFee` after catch-up | Successful nonce-10 option |
| Companion source | Canonical `Committed`, successful receipt in the same block as the ordinary transfer |
| Tested account nonce after all quotes | Still `10`; quoting consumed no nonce |
| Destination state after quotes | Companion's value `84`; quoted value `42` was not executed |

Both failure and stale-success observations used `eez_debugQuoteFee`; compact
and debug share `Composer::quote_fee` and differ only in final projection. The
post-catch-up compact request was also checked directly.

The watcher logged a temporary beacon-sidecar HTTP 404 for block 40, retained
block 39 as its completed anchor, then successfully reconciled block 40.
The initial invalid response took about 2 ms: exhausting quote capacity or
hitting the ten-second timeout did not produce this failure. This is a
correctness observation on this fixture, not a throughput benchmark.

The script exits successfully when it reproduces the bug and its controls.
That exit must not be reported as a passing regression for a corrected runtime.
The six-round control invocation exited nonzero because it did not reproduce
the bug; all six individual quotes were successful.

An earlier readiness-only probe was stopped before fixture execution. A second
probe used the wrong option name (`types` instead of `transactionTypes`); its
input-validation responses were preserved and excluded. The successful script
uses the documented field. No protocol source or production Rust was edited.

## Connection to the original failure

The saved `nestedCounter` run has the same sequence:

1. Setup nonce 0 was mined in block 132; setup nonce 1 in block 133.
2. Both nonce RPCs returned 2, which the runner preserved in its quote request.
3. The watcher temporarily failed to obtain block 133's beacon sidecars.
4. A pricing event sampled block 132 immediately before the worker wrote its
   nonce-gap failure. The worker log's modification time is 2.739 ms after
   that pricing event.
5. The scan caught up; the same unsigned request subsequently succeeded.

This strongly explains the historical failure. The original error has no
request-correlated snapshot diagnostic, so the exact request-to-pricing-event
binding remains inferred. The fresh reproduction independently confirms the
Composer defect and captures both sampled and current account state.

## Production path and why optimization did not prevent it

- [`prepare_fee_quote`](../../../crates/eez-composer/src/composer/fees/quote.rs)
  selects `L1CanonicalHead::estimation_anchor`, the last completely scanned pair.
- `fee_snapshot` opens execution against that pair. It has an outbound L2
  source-nonce freshness check, but no corresponding inbound L1 check.
- `replay_quote_predecessors` reads the sampled nonce, calls the held pool and
  maps `QuotePrefixError::Invalid` directly to `FeeQuoteError::Invalid`.
- [`quote_predecessors`](../../../crates/eez-composer/src/held_pool.rs) cannot find
  an ordinary transaction already mined outside that old snapshot in its owned
  queued/in-flight prefix. The error is returned before a useful refresh.

The earlier optimizations remain present: four active quote workers and sixteen
waiting slots per direction, separate signed-admission capacity, hash-bound
cached posting-price history, and advisory snapshots that remain usable during
scan progress. These reduce repeated work and unnecessary blocking. They do not
make the latest nonce RPC and an older simulation snapshot the same state.
Requiring equality with every latest head would undo the advisory behavior;
the missing check concerns state needed by this source request.

The existing L1 pending-prefix process tests deploy their fixtures with
`TARGET_DEPLOYER` and quote from `INBOUND_USER`. They cover unmined owned
predecessors, but do not explicitly synchronize an ordinary same-account nonce
advance with a delayed settlement scan. That boundary needs its own regression.

## Correction needed

1. Detect that relevant L1 source state advanced beyond the sampled state before
   reporting a permanent gap or selecting an omitted nonce. Cover the early
   error paths and advancement during a quote, not just its successful return.
2. Refresh the complete coherent L1/L2 context within the original bounded
   request budget where possible. While it cannot be refreshed, return the
   existing retryable context/availability category with sampled/current block
   and nonce diagnostics. Keep the transaction nonce unchanged.
3. Preserve permanent errors for genuine future gaps and unsupported prefixes.
   Do not substitute a newer nonce into an older EVM state or relax final
   settlement checks.
4. Add controlled process regressions for explicit and omitted nonces, compact
   and debug quotes, and the shared signed-admission path. Preserve the genuine
   gap, predecessor ownership and non-mutation controls.

The protocol engineer's retry correction remains necessary for correctly typed
temporary failures. This Composer correction is additional work in RollupX.

## Evidence and limits

[Sanitized evidence](evidence.json) contains canonical receipts/nonces, exact
requests and responses, selected original/fresh log events, and SHA-256 hashes
for the full local artifacts. Raw logs and full quote diagnostics remain under
`artifacts/composer-nonce-gap/` in `/tmp/rollupx-pr54-fees-20261019`; they are
local-only and have no durable retention guarantee.
The owned enclave was stopped after canonical companion consumption, with its
datadirs and diagnostics retained. It was not removed.

Executed from that worktree: the standard Kurtosis `start.sh` with an unused
enclave and explicit cached image references, then
`node artifacts/composer-nonce-gap/reproduce.mjs`. The checked-in script is an
exact copy of that successful run's script. It requires explicit endpoint,
registry and two fixture-key environment variables, and refuses chain IDs other
than 7331/696990 before broadcasting. Use the [Kurtosis guide](../../../testing/kurtosis/README.md)
to prepare a new owned enclave; it is not a public-network diagnostic.

No fix, fresh Rust build, full CI rerun, independent follower replay or Chiado
deployment is claimed by this investigation. The remaining runtime change must
pass its own before/after regressions and applicable checks.
