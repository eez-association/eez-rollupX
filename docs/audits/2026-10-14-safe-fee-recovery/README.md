# Safe deployment fee recovery

| Field | Value |
| --- | --- |
| Date and status | 2026-10-14–15 UTC; targeted validation complete; outbound tests passed and ordinary-L2 zero-tip execution reproduced; settlement subsidy accepted in the [2026-10-15 scope decision](#2026-10-15-scope-decision) |
| Question | Can a Safe deployment initiated on L2 underpay the L1 poster? Distinguish deployment on L1 through an L2 proxy from deployment entirely on L2. |
| Source | `99ced2f28536c98891e701cb60ccc37a826c9188` plus the new `safe_fee_quotes` process regression and fixtures; no runtime changes |
| Runtime source | Shipping changes through `a7fee49d04be78a01220a85d37042489ba082e35`; later source commits change tests, deployment tooling and records |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized and unchanged |
| Environment | Rust 1.95.0, Foundry 1.7.1; isolated native process fixtures, four Cargo jobs for the final run |
| Related records | [L2 trial](../2026-10-14-l2-fee-integration/README.md), [tariff contract](../../configurable-settlement-policy.md#opt-in-l2-tariff-admission), [fee proposal](../../proposals/cross-chain-fee-recovery.md#fifth-development-increment-l2-tariff-integration-and-chiado-trial) |

## Findings

The enforced outbound tariff covers the tested Safe factory call on L1 initiated through
its EEZ cross-chain proxy on L2. Both Ethereum and Gnosis process runs settled a normal
Safe and a Safe whose initializer delegated to MultiSend and created four more
initialized Safes. The receipt-derived priority payments covered each complete
canonical L1 posting transaction, including blobs, plus the configured 20% risk
allowance. The beneficiary's balance increase equaled the collected receipt tips.
Independent follower replay matched both source receipts and blocks in each profile.

Four deliberately underfunded signed sources were rejected with
`-32003 / l2_tip_underfunded` before queue ownership: zero tip; calculating the
payment from a five-million gas limit instead of charged gas; a fee cap leaving
an insufficient effective tip; and replacing a normal initializer with the
costly MultiSend initializer while retaining the cheaper fee fields. Rejections
did not consume the source nonce or deploy the target Safes.

The quotes are conservative, not exact-cost billing. In the initial Ethereum run:

| Deployment | Actual L1 gas | Charged L2 source gas | Actual L1 post + blobs | Required tariff (+20%) | Collected L2 tip |
| --- | ---: | ---: | ---: | ---: | ---: |
| One Safe | 684,913 | 92,504 | 892,330,919,520,057 | 1,070,797,103,424,069 | 1,966,833,555,711,592 |
| Outer Safe + four nested Safes | 1,250,716 | 130,042 | 1,584,276,012,677,924 | 1,901,131,215,213,509 | 3,312,420,738,095,502 |

Amounts are native base units under an explicitly configured 1:1 conversion.
They are measured private-network samples, not price quotes for a live network.
The full standalone-post projection overestimates actual cost in these samples;
unused gas-limit reservations are returned by EVM accounting, but excess
priority fees actually charged are not automatically refunded.

**Ordinary L2 traffic is outside this tariff.** The corrected test deployed and
initialized a Safe entirely on L2 with zero priority tip: 236,464 gas charged at
the L2 base fee, with no outbound queue ownership. This establishes ordinary
local execution, not canonical L1 posting or a measured loss on that post.
Both top-level CREATE and a Safe
factory call that stays on L2 use ordinary gas rules. The configured tariff does
not impose a minimum posting/blob contribution on these transactions. A dApp
adding a tip cannot enforce payment by other clients. Requiring settlement
payment from that traffic would need a separate ordinary-L2 admission/sequencing
or execution-rule policy.
This is the documented scope of the current trial, not a change made here.

## 2026-10-15 scope decision

After this review, the user explicitly accepted subsidizing settlement of
ordinary L2 traffic. The initial review classified the absence of a mandatory
ordinary-L2 settlement contribution as a coverage gap; under the clarified
scope it is intentional and is not an unmet acceptance criterion. Ordinary L2
execution fees still apply. L2-to-L1 transactions, including nested Safe
deployment and initialization, remain subject to the outbound tariff.

The existing behavior matches this scope decision. This clarification changes
neither runtime/configuration nor the measurements and test results below;
the whole-post tariff and final dispatch checks remain in effect.

## Why the outbound checks matter

- [Ingress](../../../crates/eez-node/src/ingress.rs) identifies the dispatch call
  and validates the exact recovered signed transaction before acquiring queue
  ownership. A quote response is not a payment authorization or proof of funding.
- [Quoting](../../../crates/eez-composer/src/composer/fees/quote.rs) composes the
  complete supported call group and replays native L2 execution. L1 target gas,
  the entire post and blob allowance are separate from charged L2 source gas.
- [Composition and dispatch](../../../crates/eez-composer/src/composer.rs) check
  cumulative source coverage, then actual native receipts against the fully
  priced final post. Gas limits, burned base fees and free native system gas do
  not count as collected tips. Unsupported or unavailable pricing fails closed
  for the outbound path.
- [Pricing](../../../crates/eez-composer/src/composer/fees/pricing.rs) excludes
  the poster, beneficiary and configured excluded senders from eligible revenue.
  Conversion is a policy assumption; collected L2 balances are not proof of
  realized L1 funds.

Safe's own `setup(paymentToken, payment, paymentReceiver)` reimbursement is a
separate contract transfer, not the EEZ priority payment. These scenarios set
that payment to zero. Owner and threshold initialization occurs atomically in
the factory transaction and is asserted on the destination chain. Safe permits
arbitrary delegatecall initialization; this test exercises its official MultiSend,
not every possible initializer, module or fallback handler. See the official
[setup reference](https://docs.safe.global/reference-smart-account/setup/setup)
and [1.4.1 factory source](https://github.com/safe-global/safe-smart-account/blob/v1.4.1/contracts/proxies/SafeProxyFactory.sol).

## Evidence and reproduction

The [regression](../../../crates/eez-node-e2e/tests/safe_fee_quotes.rs) uses
[pinned Safe fixtures](../../../crates/eez-node-e2e/tests/fixtures/safe-1.4.1/README.md).
The npm archive SHA-256 is
`8803f7cf26e2c58300b8136342faa7b8bce59436e300f52115fa25b095e0bf79`.
Safe and factory runtime hashes are checked against the official deployment
metadata after actual deployment. No npm install scripts run and no Safe
contracts are recompiled with different compiler settings.

Executed from the isolated integration checkout, with an initialized pinned
protocol, LLVM 22 on `PATH`, and the existing Cargo target directory:

```bash
export CARGO_BUILD_JOBS=4
export LLVM_SYS_221_PREFIX=/usr/lib/llvm-22
export PATH=/usr/lib/llvm-22/bin:$PATH
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
export EEZ_TEST_LOG_DIR="$PWD/artifacts/safe-review/logs"
export EEZ_TEST_DATADIR_DIR="$PWD/artifacts/safe-review/datadirs"
forge build --root contracts
cargo +1.95.0 test --locked -p eez-node-e2e --test node_e2e -- safe_fee_quotes:: --test-threads=1 --nocapture
cargo +1.95.0 fmt --all --check
cargo +1.95.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.95.0 test --locked -p eez-composer --lib -- composer::fees::
python3 -m unittest discover -s scripts/ci/tests -v
git diff --check
```

Actual retained logs use the same environment with the parent workspace's
`artifacts/l2-fee-integration-20261014/safe-review/run-{1,2,3}` directories.
Those logs and datadirs are **local-only**, not available
from a fresh checkout. Small canonical receipt summaries, fixture/source hashes
and executable identities are retained in [results.json](results.json).
The fixture builder/Beacon transport is controlled
by the native harness; this is not a new Kurtosis or live Chiado Safe deployment.

| Check | Result |
| --- | --- |
| Initial Ethereum Safe regression | One selected test passed in 108.86 seconds, including all four rejection cases, two canonical paid settlements and independent replay |
| First expanded run | Two outbound tests passed, including Gnosis and actual beneficiary balance deltas. The ordinary L2 test failed: the harness disabled `debug_traceCall`, so ingress failed closed and the fee service refused its non-cross-chain simulation. Logs and the failing datadir are retained in `run-2`; this timeout is not evidence of ordinary routing or funding protection |
| Final Ethereum/Gnosis regressions and ordinary L2 scope | Three selected tests passed, zero failed/ignored, 140 filtered out; 219.53 seconds. Eight precise admission rejections, four paid canonical posts, two independent followers, and one ordinary zero-tip deployment |
| Beneficiary balance credit | Passed for both outbound sources in each L1 profile; historical balance delta equals receipt priority payment |
| Formatting, workspace Clippy, diff/link checks | Passed, including Bash syntax for this document's reproduction commands |
| Focused fee arithmetic unit tests | 21 passed, zero failed/ignored, 321 filtered out; includes controlled fee-cap clipping, actual receipt gas, native callbacks, excluded revenue and final tariff checks |
| CI inventory / tooling | The existing inventory regression caught the newly added module missing from CI selection. Added `safe_fee_quotes` to the cross-chain suite and its expected inventory. Full tooling discovery: 370 tests, 363 passed, seven skipped; zero failures |

The tooling exclusions are three real Docker payload/runtime fixture tests, two
live Kurtosis resource/readiness probes, the tiny-workspace native archive
relocation test, and a secret-scanner regression requiring Gitleaks (not installed
locally). The Docker, Kurtosis and archive tests require explicit opt-in
environments; their mocked/static counterparts ran. No new release archive,
real Kurtosis probe or Docker image was produced by this investigation.

The harness correction adds its existing `EEZ_TEST_L2_DEBUG_RPC=true` setting.
Chiado's Compose command already enables `debug`; shipping runtime/configuration
was not changed. The corrected run exercises trace classification rather than
depending on ingress's conservative trace-error fallback. This is a changed
test configuration, not an unchanged-test retry reported as a pass.

## Boundaries

No live service, operator configuration, contract or genesis was changed. The
previously deployed image identity and tariff enablement were checked read-only.
No Safe was deployed on the live Chiado rollup in this investigation.

This establishes targeted fee behavior, not an independent security audit or a
zero-loss guarantee. The trial still has assumed conversion, zero fixed proving,
operations and treasury costs, no configured profit, no durable financial
reservations and no guaranteed L2-to-L1 treasury realization. Private-network
tests do not qualify adversarial builders, untested initializers, arbitrary
reorgs or future state and price changes. The full CI/release/Kurtosis matrix was
not repeated for these regression, CI-inventory and documentation changes.
