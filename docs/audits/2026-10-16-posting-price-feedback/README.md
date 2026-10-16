# Posting-price feedback correction and funding diagnostics

Status: implementation complete; process/image qualification and deployment in
progress. Deployment is authorized only after the applicable checks pass.

| Field | Value |
| --- | --- |
| Source baseline | `eez-association/eez-rollupX`, `17fe00fa9420ad3b9ddec9e003a9a9a6e35f6e04`, fee-feature checkout |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized and unchanged |
| Prior deployed source | `be8477bc7ba48d3f287d33cd6c010089458885ec` |
| Contract | [Authorized proposal](../../proposals/cross-chain-fee-recovery.md#posting-price-feedback-correction-authorized-on-2026-10-16), [pricing and diagnostics](../../configurable-settlement-policy.md#bounded-next-block-priority-estimation) |
| Historical evidence | [HackMD incident review](../2026-10-16-hackmd-fee-review/README.md) |
| Tools | Rust 1.95.0, Foundry 1.7.1; locked dependencies |

## Correction

With L1 refund recovery enabled, dynamic posting prices exclude the configured
poster's payments and transactions that call the configured registry. The
sampler retains unrelated transactions in the same blocks, uses actual charged
gas for the per-block percentile, and preserves the existing 20-block selection,
relay floor and independent ceiling. The 20% quote target and final native cost
floor are unchanged. Without refund recovery, and with an explicit priority
override, the previous respective paths remain in use.

Receipt events alone were insufficient: two successful historical sources caught
reverts and therefore had no registry logs. The regression initially failed with
receipt-only filtering. Canonical `callTracer` frames cover these cases, including
reverted frames. Hash-bound receipt and trace coverage is checked before caching;
missing data cannot silently restore unfiltered pricing. A bounded 64-block cache
shares observations across quotes/posting and needs no persistence or queue journal.

Funding rejection/status diagnostics now separate source gas-price cap failures,
measured settlement-payment shortfalls and unavailable settlement pricing. They
include required/offered payment, shortfall, payment chain, source gas/base fee,
posting prices and both sampled chain references. Prefix deferrals explicitly
identify their aggregate scope. Successful compact quote responses are unchanged.
This does not alter ownership, nonce dependencies, retry budgets or eviction.

## Reproduced baseline and regression inputs

The checked-in [sanitized receipt/trace fixture](../../../crates/eez-composer/src/composer/posting/priority-fixture.json)
contains canonical Chiado blocks 23544818–23544820, with calldata/proof payloads
removed. Expected raw rewards were independently retained from `eth_feeHistory`:
3,316,607,925; 4,460,534,753; and 4,276,213,674 wei. The earlier 17 rewards in the
same historical window remain unchanged in the comparison, conservatively
including the two 1-gwei samples.

The original estimator produces a 2-gwei clamped post bid. Filtering the sixteen
service sources and three posts produces a 1-gwei clamped bid. A separate mixed
block regression retains unrelated 2/3-gwei competition and verifies gas-weighted
selection. Mocked RPC tests cover incomplete receipts/traces, hash/order/pricing
mismatches, same-height replacement, cache bounds and reconstruction after restart.

Historical trace reads used the configured local L1 RPC with a block hash and
`callTracer`. They returned complete coverage (10, 9 and 6 transactions). These
reads establish availability for those blocks, not an RPC latency guarantee.

## Validation record

Working directory: the existing isolated fee-feature checkout. Build artifacts
use the operator's existing Cargo target directory; no dependency/pin upgrades.

| Check | Result so far |
| --- | --- |
| Composer, L1 and node library suites | 375 + 193 + 90 passed; one manual profiling test explicitly ignored |
| SDK `node sdk/l2-fees.test.mjs` | 6 passed |
| Integration `forge test --root contracts` | 86 passed; no skipped tests |
| Rust/Foundry formatting | Passed |
| Workspace Clippy | Pending |
| Native `fee_quotes::` suite | Pending; includes exact-quote caught revert and an actual external posting-price increase |
| Exact-image Kurtosis | Pending |
| Drained Chiado upgrade and live canaries | Pending |

The first receipt-only historical regression failed and was corrected by adding
canonical traces. Initial compilation/test-fixture errors were corrected before
the successful unit run. A sandboxed unit invocation could not bind mock RPC
sockets: 22 L1 tests failed with `Operation not permitted`; the same suite passed
with local socket access. This was an environment restriction, not a retry-only
behavioral pass. First failures are retained alongside the later results.

Raw RPC extracts, build/test logs and later runtime evidence are local-only under
`artifacts/posting-price-feedback/` in the fee-feature checkout. They are ignored,
are not available in another checkout and have no guaranteed retention. Sanitized
fixtures and the final result manifest accompany this record when qualification
is complete.

## Boundaries

Quotes remain advisory. Genuine external competition, changed execution or source
base fees can still invalidate an old signed payment. The new diagnostics explain
that rejection; they do not reserve a price or subsidize an underfunded L2 source.
The existing explicitly accepted largest-payer L1 batch subsidy remains unchanged.
No contract/genesis/database/signing-wire migration is required. Older binaries
restore unfiltered pricing, so rollback can defer a lower-priced outstanding
source. Accepted pending requests remain in memory and must be drained before
the authorized handover or explicitly reconciled/resubmitted.
