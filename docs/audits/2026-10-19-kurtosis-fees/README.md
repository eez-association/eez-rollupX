# Fee-enabled Kurtosis and canonical protocol PR 54

Status: both standard Kurtosis profiles collect fees; local fee qualification
and corrected state-chaining checks passed on 2026-10-19 UTC. The full
fee-enabled lane remains failing on the canonical protocol client's temporary
quote-error classification. No public-network configuration or runtime changes
are included, and this result does not qualify the PR for merge.

| Identity | Revision |
| --- | --- |
| Protocol PR | [54](https://github.com/eez-association/eez-core-protocol/pull/54) |
| Protocol PR base | `143d87e62cade98bfc122d612638dcaaf1c85633` |
| Candidate protocol pin | `f46af7c9833ac12a54f14d52f4da1f318a0f6525` |
| First fee-enabled runs | `5321d24464328899acbed9c85cf7fc43ece15f14` |
| Earlier head reviewed during this task | `af9255775e144f71982abcd34dc920510096dc9d` |
| RollupX candidate base | `46d694faf88ae06941764b1858373b7c13987635` plus this change |
| Published RollupX PR 91 head / base | `e5e1d97ff49e69821ba403a9ae723ab4afa2db21` / `2c19e36bed2720c00f89841ccfca47ec7c921e06` |
| Published protocol pin | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` |
| Earlier local protocol pin replaced | `0264ab1be8986093ad786782e60a28f14103a0fd` |
| Proposal | [Protocol runner integration](../../proposals/protocol-network-runner-pr56.md) |
| Tools | Rust 1.95.0 runtime artifacts, Foundry 1.7.1, Solidity 0.8.34 |

## Baseline and change

The latest protocol runner requires Composer quotes in both directions.
Against the earlier fee-disabled enclave, `eez_quoteFee` returned JSON-RPC
`-32004`, `data.code=quotes_disabled`. The actual fee helper from `af925577`
returned 75 (allowing its caller's node-price fallback); the same request through
`5321d244` returned 1 and stopped. This was a read-only reproduction; no source
transaction was signed or broadcast. The new pin therefore cannot use the old
fee-disabled CI profile unchanged.

The user authorized enabling Kurtosis fees. Default CI and interactive devnet
profiles now collect both directions with explicit parity conversion, zero
fixed/risk costs, 20% quote profit, a nonzero L2 beneficiary and a separate EOA
receiving a 90% builder refund. Both profiles expire the synthetic assumptions
on 2100-01-01. Specialized policy profiles retain disabled collection.

Normal cross-chain waves reuse the pinned protocol quote/sign client and retain
its responses and selected fields. Quotes retain the prepared nonce; failures
do not fall back to node prices. Outbound waves wait for predecessor settlement
because held future L2 nonces and unsettled source nonce changes are outside the
quote API. Ordinary setup and pure L2 transactions use node fees. Enabled
profiles assert invalid-selector/dispatcher rejection at fee admission with no
queue ownership or receipt; disabled profiles retain poison eviction. The
same-Sync state-chaining test uses a funded 100 gwei bid and three independent
outbound senders with fees enabled. Signed admission also rejects future held
L2 nonces; increasing the bid alone cannot preserve the original single-sender
L2 burst. That assertion is retained in disabled profiles; L1 retains it in both
modes. These tests do not establish accurate quote margins or queued L2 support.
The mixed-direction test asserts separate, FIFO-ordered funded posts and both
proofs with fees enabled; disabled profiles retain shared-Sync execution. This
matches the existing fee contract, not a runtime scheduling change.

## Canonical protocol scope and remaining gap

PR 54 includes PR 56 and fixes failed nonce reads becoming zero, exact prepared
quote nonces, typed option errors and malformed-response evidence. It does not
include the earlier local worker-isolated setup receipt/header/code guard. A
controlled regression passed at local `0264ab1`, then failed at canonical
`af925577`: setup used nonce 1, mocked subsequent node/front reads returned 0,
and the real preparation function invoked mocked signing at nonce 0. The helper
is unchanged in `5321d244`. See [input and result](setup-regression.json).
This does not establish the cause of the historical CI failure or a new
Composer bug. Passing ordinary network tests does not close that reorg gap.

An additional [controlled retry reproduction](quote-retry-regression.json) uses
the actual `5321d244` classification function with the full error shape emitted
by `fee_quotes::rpc_response`. It classifies `-32004` / `quote_busy`,
`stale_context` and reconciliation-pending `quote_unavailable` as terminal,
although the client contract calls for bounded retries. The early `-32004`
branch precedes transient classification. Its existing tests use simplified
text without that outer code. The protocol engineer needs to distinguish the
typed errors before treating unsupported/disabled quotes as terminal, and add
full JSON-RPC and Cast-formatted regression inputs. No protocol source was edited.
The same classifications were reproduced at `f46af7c`. Its submission classifier
also treats the live-observed `quote_unavailable` / queued-predecessor-composing
refusal as terminal; this case needs a bounded retry too.

A subsequent lookup during live validation found PR 54's newer head, `f46af7c`.
It changes only transaction-link chain-ID initialization, its documentation and
tests; the fee client is unchanged. An isolated checkout passed all five link
tests. Its new regression fails in all six partial-initialization subcases
against `5321d244`, confirming the fix. The active live checkout was not changed
mid-run. After that run finished, the candidate pin was advanced to `f46af7c` for
subsequent validation. See the [follow-up record](pr54-chain-identity-followup.json).

The [scenario inventory](scenario-inventory.json) contains 62 scenarios, of which
34 are live-eligible and 28 explicitly excluded upstream. Earlier validation at
`af925577` passed all 62 Anvil scenarios, 778 Solidity tests (three fork tests
skipped), 177 runner Python tests and 236 trace-decoder tests. Those results are
for that revision. The latest commit changes the fee client/recovery scripts,
not the Solidity, libraries or Anvil scenario definitions.

## Runtime and reproduction boundary

Work is isolated in `/tmp/rollupx-pr54-fees-20261019`, branch
`feat/pr54-kurtosis-fees`. The root working tree and its operator configuration
are preserved. Owned enclaves were `eez-pr54-validation-20261019` (fee-disabled
baseline), `eez-pr54-fees-20261019` (fee qualification) and
`eez-pr54-fees-20261019-v2` (full lane and correction checks).
All three were stopped after their checks with data and logs retained; no
enclave was removed. The final stop completed at 11:15:45 UTC.

[Runtime provenance](runtime-provenance.json) identifies the verified node,
signer and seven host tools, matched to the producer archive with SHA-256
`20fbcc55d6ed4694041c1ac2e7c1b7b5cc20f82bc48a3df5212412cb67082ac4`.
The runtime source is `110f952143853d0edbc51d6d8c2cb67be4d828f3`; its Rust,
dependencies, integration contracts and genesis are byte-identical to this
candidate. Protocol Solidity/dependency gitlinks also match. A new local
deployment image bundles `5321d244`. This is not a fresh runtime release build.

The retained driver copies `testing/kurtosis/run-ci.sh`, fixes its source path
and retains the owned enclave on exit; test assertions and scenario selection
are unchanged. It uses the new CI profile, `EEZ_PROTOCOL_MAX_PARALLEL=2`, explicit
runtime images/tools and the local deploy image. Logs are local-only under
`artifacts/pr54-fees/`, `artifacts/pr54-fees-v2/` and `artifacts/pr54-fees-final/`; they have no durable
retention guarantee. Final corrected stages and complete node/signer logs are
under `artifacts/pr54-fees-corrections/`; their hashes are retained in
[correction evidence](corrected-validation-evidence.json). No Chiado restart,
public deployment, protocol source edit or remote publication occurred.

## Validation

- Profile/transport checks: 20 passed, two optional real-infrastructure probes
  skipped. They evaluate the actual Starlark wiring and inspect node, signer and
  follower configuration.
- Wave quote/signing/admission checks: eight passed using real signed envelopes and mocked
  RPCs, including both transaction types, rejection without signing, and nonce
  sequencing. The first test run used the wrong decoded field name (`gasLimit`
  instead of `gas`); that test assertion was corrected and its log retained.
- SDK: six passed. Settlement configuration checker and shell syntax passed.
- Generated genesis consistency passed without changing fixture content.
- The first protocol Python run started before the cold Solidity build finished;
  linked-evidence tests failed because `out/EEZ.sol/EEZ.json` was absent. Its log
  is retained; rerunning after the required build is an environment correction.
- RollupX tooling after the submission retry fix: 387 selected, 379 passed, eight skipped (six opt-in
  infrastructure probes and two requiring unavailable Gitleaks). This includes
  the profile wiring, admission/deadline checks and outbound-only regression.
  Earlier 381- and 384-test runs are retained separately.
- Protocol Solidity at `5321d244` (unchanged in `f46af7c`): 778 passed, three fork tests skipped; documented
  `script/*`, `test_Doc_OriginalValue_SeedInSetup` and `test_PostCost_Incremental`
  exclusions retained.
- Protocol Python at `5321d244` after compilation: 178 of 179 passed. The timeout
  exit-status test expected two quote attempts within its three-second mock
  budget but observed one. Its identical focused rerun passed; this remains a
  flake, not a green suite. The harness increments Bash's special `SECONDS`
  variable in mocked `sleep` while wall time can also advance it. The protocol
  engineer should use a deterministic clock for exact-attempt assertions.
- Protocol runner Python at `f46af7c`: all 180 passed in 279.692 seconds. Its
  timeout-test implementation is unchanged, so this does not erase the earlier
  timing flake.
- Fee-disabled `af925577` baseline: the full local CI lane passed, including all
  four wave modes, original same-account state chaining and all 34 eligible
  protocol scenarios. Independent receipts, causality, semantic blobs and
  execution-prefix replay passed. See the [result](fee-disabled-baseline-result.json)
  and [protocol summary](fee-disabled-baseline-protocol.json).
- Both collection switches, the 20% quote margin and separate recipients were
  read from the new live node; its L2 quote endpoint returns both formats.
- First fee-enabled inbound wave: four sources settled in one native blob post.
  Its single builder refund exactly equals 90% of the largest source's actual
  priority payment less the refund receipt's gas cost. Recovery was
  `459267292442544` wei versus `966584896269500` wei post/blob cost, a
  `507317603826956` wei shortfall. This demonstrates the intentionally accepted
  largest-source-only batching subsidy; it is not a 20% aggregate batch margin.
  See [canonical receipt evidence](inbound-fee-evidence.json).
- The existing L2 fee qualification script passed on the first owned fee-enabled
  enclave. Two independently funded nested calls shared one canonical native
  post. Underpayment was rejected without queue ownership or execution. Actual
  tips totaled `1141727145480468` wei against `832452363306995` wei posting/blob
  cost, exceeding the `998942835968394` wei requirement including 20% profit at
  parity. Target state, beneficiary and canonical queue disposition were checked.
  See the [retained summary and full-evidence digest](l2-fee-evidence.json).
- The existing L1 fee qualification script passed: nested calls and Safe 1.4.1
  creation in both legacy and EIP-1559 formats, underpayment rejection, actual
  builder refunds and canonical destination state. Two subsequent four-source
  batches each used exactly one largest-payer refund and continued despite
  shortfalls of `223079456500143` and `111008265015392` wei, as allowed by policy.
  A fresh quote still succeeded afterward. See [L1 evidence](l1-fee-evidence.json).
  This uses the standard Safe EOA-owner fixture; it is not a Falcon workload.
- First fee-enabled lane passed the inbound wave, then failed before the first
  outbound submission because the new wave helper read unset `IN_NONCE` in
  outbound-only mode. The helper now selects its direction before reading the
  sender/nonce, and a regression executes that outbound-only function with
  inbound variables unset. The failed lane's logs/result remain retained.
- The second fee-enabled full lane passed inbound, outbound and mixed waves,
  then failed during mixed-plus-pure-L2 wave 2: after quoting, an L1 predecessor
  entered composition and signed admission returned `-32003`,
  `quote_unavailable`, `quote unavailable: queued predecessor is entering composition`.
  The old `send_front` treated this temporary refusal as fatal. No state-chaining
  or protocol suite ran in that invocation. Its [failed result](fee-enabled-5321-result.json)
  and complete logs remain retained; it signed and accepted 27 proof windows.
- Added controlled submission regressions reproduced six failing subcases before
  fixing `send_front`. It now retries only startup and explicitly recognized
  temporary admission refusals with identical bytes under a 120-second deadline.
  All 11 focused quote/sign/admission tests pass, including deadline exhaustion,
  underfunding/profile errors, ambiguous transport and changed response hashes.
- The subsequent mixed-plus-pure rerun at `f46af7c` completed two waves, then
  stopped while quoting wave 3's next L1 source. The actual Cast response was
  `-32004`, `quote_unavailable`, `quote unavailable: queued predecessor is entering composition`.
  This exercises the upstream classification problem, before a signed submission
  reaches RollupX's corrected helper. The [retry record](quote-retry-regression.json)
  retains the exact response and log digest. No protocol source was patched or
  quote failure bypassed.
- State chaining stopped in ordinary fixture deployment when its global 100 gwei
  test priority reached Forge's setup gas-estimation path. The test now confines
  that bid to the cross-chain source transactions and their matching fee caps;
  ordinary funding/deployment retain their previous fees. The corrected live
  run passed all nine cases: source/destination/mixed state chaining in each
  direction, invalid-call admission with ordered survivors, mixed directions
  and repeated inbound calls. Ordered returns, contract state, proofs and
  canonical commitments passed. Same-direction transactions retained one Sync
  block; the mixed directions retained their required separate posts.
- Full eligible protocol selection at `f46af7c`: 30 passed, three failed and one
  skipped. Independent receipt, causality and native blob semantic checks passed
  for those 30; applicable lookup cases also passed execution-prefix replay.
  Aggregate evidence remains
  incomplete because four selected scenarios did not pass. See the
  [sanitized first-run summary](fee-enabled-protocol-first-summary.json).
  `bridge` and `flash-loan` exhausted the former 0.005 ETH worker allocation;
  `bridgeL2` was correctly blocked by its failed prerequisite. The first bridge
  deployment required `8742100090568156` wei upfront. Kurtosis now allocates one
  test ETH per worker per chain, retaining `EEZ_PROTOCOL_FUND_ETH` override
  precedence. All 26 runner-tooling tests pass after this adjustment.
- `nestedCounter` failed with a nonce-prefix gap after both read/front RPCs
  returned source nonce 2. A later identical unsigned quote succeeds for both
  formats at nonce 2, and both original setup receipts remain canonical. This
  establishes a temporary rejection, not its cause: the original fee snapshot
  was not captured, so a Composer context race versus preparation/reorg handling
  remains unattributed. See [nonce-gap evidence](nonce-gap-evidence.json). Do not
  treat a later pass as proof that the earlier canonical-setup gap caused it.
  A subsequent [focused investigation](../2026-10-19-composer-quote-nonce-gap/README.md)
  reproduced a Composer snapshot-freshness bug and correlated the original scan
  lag. It also reproduced a successful omitted-nonce quote using an already-spent
  nonce. This supersedes the uncertainty about whether Composer has a defect;
  the original request lacks a directly correlated snapshot trace.
- After the setup-bid and worker-funding corrections, `bridge`, `bridgeL2`,
  `flash-loan` and `nestedCounter` all passed. Independent receipt, causality and
  native blob semantic checks passed for all four. See the
  [corrected subset](fee-enabled-protocol-corrected-subset.json). The funding
  failures are corrected; the nested-counter result remains a retry-only pass
  with no demonstrated fix for its earlier nonce gap. Combining this subset
  with the earlier 30 passes does not establish a clean full-suite run.
- Final shell syntax, edited Markdown path checks and `git diff --check` passed.
  The canonical PR head was rechecked after testing and remained `f46af7c`.

## Remaining work before a green full lane

The protocol engineer needs to correct quote/submission classification for the
actual typed Composer responses, preserving bounded retries and the prepared
nonce. Regression inputs must include both full JSON-RPC errors and Cast's
formatted output; `quotes_disabled` and unsupported requests must stay terminal.
The canonical setup guard and deterministic timeout-test clock also remain
outstanding. No protocol source changes were made in this worktree.

Fix the now-reproduced Composer nonce/snapshot mismatch described in the
[follow-up investigation](../2026-10-19-composer-quote-nonce-gap/README.md).
Do not turn all `invalid_transaction` failures into retries. After both the
Composer and upstream corrections, rerun the complete fee-enabled
lane, including mixed-plus-pure waves and all 34 eligible protocol scenarios.

Fresh Rust/native release lanes and standalone follower replay are not covered
by these results. Hosted CI on the final published head remains the merge gate.
