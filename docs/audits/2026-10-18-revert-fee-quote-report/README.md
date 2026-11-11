# Rollback-and-call-again quote rejection: insufficient source balance

Status: cause identified from the original logs and reproduced with read-only
Chiado probes on 2026-10-18. The original investigation made no runtime change
or transaction submission; the subsequent Composer correction is recorded below.

| Field | Value |
| --- | --- |
| Report | [Devnet fee-quote rejection](https://hackmd.io/@Yu7RfKfWRP6MwaKqIFALjw/SyPXLFMhfe) |
| Scenario | `revertFromOtherChainAndCallAgainL2` |
| Reviewed source | `eez-association/eez-rollupX`, PR #91 head `7cb64b52c71083e575363ac0f73b344fca888028` |
| Deployed runtime | CI merge `a5b122a743e4468a2e83e5db45713d33689d7ec8`, tree `4e977395e42348ebc55a474543f918c05c97fa54`, equal to the PR head's tree |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`; clean initialized checkout, no pin change |
| Environment | Chiado L1 `10200`, EEZ L2 `906969`; L2 tariff enabled, 20% quoted profit, zero risk uplift, 1:1 test conversion |
| Executable provenance | Running node/signer image IDs and start times rechecked during this review; recorded in [evidence.json](evidence.json) |
| Working directory | `/tmp/rollupx-issue208-review-20261018`; existing proposal and audit edits preserved |

## Findings

The report accurately describes a persistent quote-stage failure and correctly
does not claim that a submitted transaction reverted. The original failure was
an EVM transaction **balance validation rejection** after Composer calculated
the settlement tip. It was not an uncaught contract revert.

At 17:32:51.241530 and 17:32:51.245468 UTC, the original Composer logs report:

```text
lack of funds (999999946856400) for max fee (1204265508506016)
```

The exact report request reproduced the same application error on both formats
at 18:08:46 UTC. The validation log again identifies insufficient funds, with
the same available balance and a slightly newer required maximum of
`1204280818708113` wei. The initial source simulations succeeded; the remote
counter returned `1` for both calls, including the call after the caught revert.

Using the previously funded canary's public address as `from`, with the same
deployed target, calldata and value, produced successful legacy and EIP-1559
debug quotes. This changes the sender and its resolved nonce; it is a control,
not a top-up/retry of the original sender. The scenario's pinned caller and
counter do not branch on the outer user's address. No signing key was used.

The successful control measured:

| Quantity | Value |
| --- | --- |
| Original sender balance | `0.000999999946856400` xDAI |
| L1 settlement execution estimate | `0.000812779005689453` xDAI, 812,779 gas |
| Blob estimate | `0.000131072000000000` xDAI, one blob |
| Posting subtotal | `0.000943851005689453` xDAI |
| 20% profit target | `0.000188770201137891` xDAI |
| Required L2 tip | `0.001132621206827344` xDAI |
| Estimated source transaction fee, including L2 base fee | `0.001132621215910437` xDAI |
| Maximum source fee reservation | `0.001204280818708113` xDAI |
| Source receipt gas estimate / submitted gas limit | 182,017 / 193,533 |

The maximum reservation is `gasLimit * maxFeePerGas` (or `gasPrice` for legacy),
not an actual charge. Even the estimated consumed fee exceeds this sender's
balance, so removing gas headroom would not solve its funding problem. These
are estimates, not settlement receipts; the report's trigger remains unsubmitted.

## Where the diagnostics lose the cause

1. [`simulate_source_env_on`](../../../crates/eez-composer/src/local/client.rs)
   logs `InvalidTransaction::LackOfFundForMaxFee`, then returns `success = false`
   with zero gas and empty output when `retry_unavailable_conditions` is false.
2. [`quote_in_context`](../../../crates/eez-composer/src/composer/fees/quote.rs)
   collapses that result into `Unsupported("source validation or execution failed")`.
3. The compact projection returns that error inside each option. A live
   `eez_debugQuoteFee` request from the original sender has the same generic
   error, with no required/available balance diagnostics.
4. The report's client then reports `Malformed fee quote option`, obscuring the
   valid option-level failure further. The report does not identify its client
   revision, and the referenced fee-client helper is not present at this
   protocol gitlink; that client's exact implementation was not independently
   reviewed here.

The [normative quote contract](../../configurable-settlement-policy.md#unsigned-l2-fee-quotes)
explicitly retains normal source nonce, balance and fee validation. Rejecting an
unfunded executable quote is consistent with it; categorizing insufficient
funds as an unsupported transaction is misleading. An advisory quote that can
be returned before funding would be a separate contract/design change.

## Reproduction and checks

| Check | Evidence type | Result |
| --- | --- | --- |
| Exact report `eez_quoteFee` request | Live, read-only | Both formats reject; correlated validation logs identify balance shortfall |
| Original incident logs | Retained live logs | Same funding reason in the report's original time window |
| Same request through `eth_estimateGas` | Live, read-only | `0x2f3fd` (193,533); estimation without the settlement tip succeeds |
| Same call from funded canary | Live, read-only | Both formats quote successfully, including native replay and measured settlement pricing |
| Original sender through debug endpoint | Live, read-only | Same generic failures; debug does not recover the validation reason |
| Post-probe scenario state | Live, read-only | Original nonce `0x27`; L1 counter and L2 `lastResult` still zero |
| Exact scenario in retained CI evidence | Prior Ethereum-profile Kurtosis execution | `PASS`, table/causal checks passed at the same protocol revision; different deployment and configuration |

Two preliminary control-account checks are retained: the standard Anvil address
had zero balance on this deployment, so it was not quoted; the block beneficiary
was correctly rejected as an excluded payer. The previously funded, non-excluded
canary was then used. No failing application test was retried until it passed.

Reproduction input, response excerpts, four validation log records, image
identities and prior CI summary are in [evidence.json](evidence.json). Raw probe
responses and bounded log slices are local-only under
`/tmp/rollupx-report-SyPXLFMhfe-20261018/`; their digests are recorded and retention
is not guaranteed. The report download is also local-only and its digest is
recorded; its complete text is not copied into this audit.

The exact request is read-only and can be repeated without credentials or keys:

```bash
cast rpc --rpc-url https://eez.asuscomm.com/composer/l2 eez_quoteFee \
  '{"from":"0xf0208ed478e7aa5c4c1c62145e5be5c7b91fec54","to":"0x2ad716eecb0bf954be2baae4c29c6299e9604351","data":"0x61461954","value":"0x0"}'
```

The review executed the equivalent JSON-RPC through Python's standard HTTP
client, not this `cast` command. Quotes depend on current state and expire;
the addresses may cease to apply after a devnet reset.

## Follow-up

The initial read-only investigation identified these follow-ups; the subsequent
section records the completed Composer work.

- Preserve typed source-validation failures and expose an insufficient-funds
  code with available balance, required balance and shortfall. Do not weaken
  admission or confuse source balance with an underpaid settlement tip.
- Make clients inspect `option.error` before validating successful fee fields.
- Fund test workers for the quoted maximum fee plus transaction value and other
  planned transactions. The pinned upstream staged runner defaults to 0.001;
  the RollupX Chiado wrapper already defaults to 0.005 and retains wallet locks.
  A target change alone does not top up an account above the configured floor.
- Add regressions for balance sufficient at the initial zero-tip simulation but
  insufficient at the priced pass, for both transaction formats; retain the
  rollback-and-call-again case with sufficient funding.

No wallet was funded, transaction submitted, service restarted, or code deployed
in this review. No new source build, full stress suite, or independent follower
replay ran. A funded, signed Chiado execution of this exact reported fixture
remains unverified; the quote control and prior CI case do not replace it.

## Composer correction, 2026-10-18

The user subsequently authorized fixing the Composer diagnostic. The correction
reuses the simulator's typed `SourceCondition::Balance` and carries its available
and required amounts through `SourceBalanceError`, gas discovery, priced
simulation and the compact/debug response. Failed options use `insufficient_funds`
with `sourceChainId`, `availableBalance`, `requiredBalance` and `shortfall` under
`error.details`. Signed tariff admission and standard gas estimation retain the
same balance diagnostics. No error-text parsing is used.

Normal balance and settlement-payment checks remain enabled. Successful response
fields, pricing, source-state isolation, queue ownership, protocol pin and signer
validation are unchanged. The [API contract](../../configurable-settlement-policy.md#compact-fee-quotes)
records mixed-version behavior; no storage/configuration migration is needed.

The process regression exercises a nested call whose first remote effect is
rolled back before the caller retries. It verifies both transaction formats,
both quote endpoints, omitted and explicit gas, unchanged nonce/balance/remote
state after rejection, and successful quotes after funding the same sender.
The fixture asserts that its low balance covers twice the sampled base-fee gas
reservation, isolating the later settlement-tip requirement.
It uses the existing synthetic pricing policy with fixed cost allowances and
non-parity conversion; its numerical amounts are not Chiado tariffs.

The first fixture used only `10^12` wei. It reproduced the same generic error,
but its logs showed a base-fee reservation of about `1.21 * 10^13` wei, so it did
not isolate the priced-tip pass. That initial failure is retained. The fixture
was strengthened to `10^15` wei with the explicit affordability assertion;
the final before/after comparison uses this same strengthened test.

For the controlled baseline comparison, the updated test runner deliberately
selects copies of the original, hashed Composer/follower/signer executables.
Candidate process checks clear all prebuilt/runtime overrides and build from
the edited source. Baseline overrides are confined to their shell and are not
used as candidate validation. Local build output and retained failure datadirs
are under `artifacts/source-balance-quote-fix-20261018/` in the operator workspace;
they are not shipped or guaranteed to remain available.

Validation completed against the original source plus the correction. The same
strengthened regression failed on the baseline with `unsupported_transaction`
instead of `insufficient_funds`: the wallet had `10^15` wei, covered the initial
zero-tip reservation, but needed `13188746293590770` wei after pricing. The fixed
runtime reports that required balance and the exact shortfall for all eight
format/endpoint/gas combinations. Funding the same sender then permits both
formats at nonce zero.

| Check | Result |
| --- | --- |
| Composer and node unit suites | 379 + 92 passed; one existing manual timing profile ignored |
| SDK checks | 6 passed |
| Focused native process suite | 6 passed, 0 failed, 0 ignored; no diagnostic retries |
| Workspace/all-target Clippy with `-D warnings` | Passed |
| Rust formatting, diff whitespace, local documentation links, shell example syntax | Passed |
| Integration contract build | Passed |

The process suite covers the new diagnostic and funding recovery, initial
value-balance rejection, genuine fee-sensitive execution rejection, Gnosis
bidirectional gas estimation, and existing caught-revert, legacy underpayment
and L1 refund settlement flows. The latter three also verify independent
follower replay. This is source-built local evidence, not a live Chiado run.

Exact commands, test selection, source patch digest, baseline/candidate executable
hashes, response excerpts and retained-log digests are in
[fix-validation.json](fix-validation.json). The Composer binary changed; follower
and signer hashes match the baseline. Only the scoped Composer correction and
its tests/documentation are part of this change; existing unrelated review edits
were preserved.

The full protocol stress matrix, Falcon Safe workloads, Kurtosis/PBS validation,
release image build and live deployment were not repeated for this diagnostic
correction. The full CI gate remains separate. The report client's error display
and worker funding are still client/operator follow-ups. No Chiado service has
been changed as part of this correction, and the original sender was not funded
or submitted here.
