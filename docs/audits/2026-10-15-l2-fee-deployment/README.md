# Legacy fees and wallet compatibility: Docker, Kurtosis and Chiado

| Field | Value |
| --- | --- |
| Date and status | 2026-10-15 UTC; local image validation and authorized ROG Chiado deployment complete |
| Runtime source | `d2e0a24d043c052c77f3556670014aa988896004`, clean at build time; implementation through `4e84688cfcaba3b5c689cf0bf0e84f8026c67174` |
| Dashboard source | `4957a1c9cba1a3ad6bbc94d70d85cfd4574c0b82`, clean at build time |
| Protocol | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized and unchanged |
| Prior live images | Runtime `030765e36a9e`, signer `41dd5d8102ac`, dashboard `0cf7315cc116` |
| Candidate images | Exact local image IDs, source labels and runtime executable hashes in [results.json](results.json) |
| Environment | Rust 1.95.0, Foundry 1.7.1; Docker development release profile; isolated Ethereum PBS enclave and existing Chiado L2 |
| Related evidence | [Legacy quotes and JSON-RPC](../2026-10-15-legacy-fee-quotes/README.md), [static discovery and estimate retries](../2026-10-15-wallet-rpc-compatibility/README.md), [initial fee deployment](../2026-10-14-l2-fee-integration/README.md) |

## Findings and checks

The runtime, signer and dashboard now run the tested fee-integration versions.
The custom quote RPC supports protected legacy and EIP-1559 intents; static
wallet discovery accepts standard block selectors; the dashboard retains the
bounded retries for explicit temporary estimation errors. Fee funding guards
and the accepted ordinary-L2 settlement subsidy remain unchanged.

| Check | Evidence and result |
| --- | --- |
| Local images | Node/follower, signer, deployment tools and UI built successfully from the source revisions above. The deployment image uses the candidate node's genesis utilities. These are local development builds, not CI-promoted artifacts |
| Live baseline reproduction | Before upgrade, discovery at `pending` and `0x1` returned `-32602` with the latest-only message. The new quote RPC returned `-32601` |
| Kurtosis identity | Fresh owned enclave `eez-l2-fees-20261015-d2e0`; pinned EEZL2 deployment, runtime code, system/rollup binding and genesis commitment checks passed |
| Candidate discovery | Seven standard block selector forms returned the same metadata, including a syntactically valid unknown block hash with `requireCanonical`. An invalid selector was rejected. This is static metadata, not a claim that the selected block exists |
| Nested fee exercise | Two independent sources, one legacy and one EIP-1559, quoted through `eez_quoteL2ToL1Fee` and settled in the same native block. The underpaid legacy variant received `l2_tip_underfunded`, with unchanged nonce/target state and no accepted queue ownership. Paid receipts, nested effects, beneficiary and canonical disposition passed |
| Collected fees | The mixed posting cost `837752203318142` L1 base units including blobs. The configured tariff required `1005302643981771` L2 base units; actual source tips were `2549270373617800` |
| Real browser transactions | Two successful Chromium-to-settlement flows through the actual dashboard image: a MetaMask-style EIP-1559 adapter and a Rabby-style legacy adapter. Each requested one ordinary transaction signature, preserved the quote fields and changed the canonical L1 target. These adapters are not installed wallet extensions |
| Blob authentication | All four sources across three postings passed independent KZG, native payload/settlement-table and exact signed-source-byte checks |
| Independent image replay | One selected process test passed, zero failures/ignored, 143 filtered out. The follower extracted from the candidate image derived the chain from L1 and genesis, checked 72 blocks across the selected settlement ranges and all four source receipts, and reached the expected safe hash at L2 276 |
| Live fee canary | Reused the owned nested fixture and funded workload account, holding both checkouts' wallet locks. No fixture deployment or funding transfer was needed. One legacy source settled; the underpaid variant was rejected before ownership/execution. Actual tips covered the complete post/blob tariff |
| Live independent evidence | Native blob/KZG verification passed. A separate exact-image follower replayed the new settlement from a complete pre-upgrade checkpoint and matched block/state/transaction/receipt roots, legacy receipt and fees, sender balance/nonce and nested callback count |
| Public service checks | Both HTTP chain fronts and node RPCs, both TLS WebSocket routes, custom-method placement, `latest`/`pending`/numeric discovery and dashboard TLS/render checks passed. Queue empty; gateway out of maintenance; node/signer/UI had zero restarts at final inspection |

The live legacy source was
`0xcc4df28a520c3cd8e6d01d5c2097eeafd8ea585a445b8f229da91124aaa19159`,
in L2 block 658675. Its posting was
`0x8864d738579147662620c3ea6a5998b4f1cebc615a683e32f3c148f57af03423`.
Posting/blob cost was `304767001215865` L1 base units; required tariff was
`365720401459038` L2 base units; collected source tips were `1400774408758711`.
The existing 1:1 trial conversion, 2000-bps risk margin, and zero service/profit
charges were preserved. These figures do not establish liquid L1 proceeds or
production exchange-rate adequacy.

## Execution and retained diagnostics

Images were built with `Dockerfile`, `Dockerfile.signer`,
`testing/kurtosis/Dockerfile.deploy`, and the dashboard Dockerfile. Kurtosis
startup reused those images; every enclave operation explicitly selected the
owned enclave. The fee and browser scripts were local copies of the existing
exercises, extended to cover both transaction envelopes and the custom RPC.
The runtime sources were unchanged during deployment validation.

The semantic blob verifier ran once per posting. Independent replay used
`testing/kurtosis/scripts/verify-policy-replay.sh` with explicit timing values,
`RUSTUP_TOOLCHAIN=1.95.0`, and `EEZ_TEST_RUNTIME_BIN_DIR` pointing to the
follower extracted from the image. This explicitly selected image bytes;
it was not a source-runtime test. Earlier source unit/process/Clippy results
remain documented in the linked implementation records and were not rerun as
a full workspace suite for this operational deployment.

Retained setup failures are not counted as validation:

- The first verifier build named the wrong package; the corrected
  `cargo +1.95.0 build --locked -p eez-l1 --example verify_blob_evidence` passed.
- The first replay harness invocation selected the host's default Rust 1.96.1.
  Its owned compilation was interrupted before tests. The explicit Rust 1.95.0
  invocation passed; all Docker runtime builds already used Rust 1.95.0.
- The first maintenance validation found a file-permission error because a
  private backup's mode had been copied into the gateway mount. Validation
  failed before restarting/stopping any service. Restoring the original
  readable mode fixed the prepared operation; the failure log is retained.

## Handover and rollback

The existing user authorization covered deployment and its bounded test call.
The dirty operator checkout was not switched, reset or used as a build source.
Only the intended image/datadir overlays and their `COMPOSE_FILE` selections
changed; candidate service environments were compared with the running services.

All public HTTP and WebSocket transaction routes were verified in maintenance,
and the accepted queue was empty before shutdown. Node and signer both exited
cleanly. Complete stopped L1/L2 copies became the candidate's writable state;
the original databases remain untouched. A separate stopped L2 copy was used
for independent replay. Genesis, deployment bindings, signer/poster identities,
timing and fee settings, Lighthouse, posting-log volumes, gateway certificates,
monitoring and unrelated services were preserved.

The matching dashboard was upgraded after runtime readiness checks. Gateway
configuration was restored and verified over public HTTP/TLS/WebSocket routes.
The owned Kurtosis enclave and temporary live replay follower were stopped
after retaining evidence. No contract deployment or genesis regeneration ran.

Rollback requires stopping candidate writers and restoring the saved runtime
configuration/images with the original stopped databases. Restore the older
dashboard before an older Composer: it cannot serve the new quote RPC or legacy
quote profile. Never run the older binary on candidate-modified databases.
Database snapshots do not preserve in-memory pending requests; reconcile receipts
and nonces before resubmission. This deployment retained an empty accepted queue.
Rollback restoration itself was not rehearsed, and collected fees/settlements
cannot be undone by reverting binaries.

## Availability and limits

The checked-in [summary](results.json) contains sanitized source/image identities,
fee arithmetic and replay results. Full build/test logs, the exact exercise
copies, source bundles and a digest inventory are retained locally under
`artifacts/l2-fee-release-20261015/` in the operator checkout. They are not
uploaded CI artifacts or available from a fresh checkout. Private configuration
backups and database copies remain in the owned operator upgrade directory.

Real wallet extensions were not exercised. The full CI release/Kurtosis matrix
was not run for these local development images. Live replay trusts the complete
pre-upgrade checkpoint; it is not a fresh replay of all historical Chiado data.
Quotes remain advisory, final native funding validation remains mandatory, and
the existing maximum quote gas limit remains 5,000,000. No new durability,
exchange-rate, treasury-liquidity or unrestricted-transaction guarantee is made.

## Subsequent dashboard wallet-quantity fix

The reported MetaMask bridge popup, `Expected a canonical hex quantity`, was
reproduced against dashboard `4957a1c` with a numeric pending nonce and separately
with zero-padded hex. Both attempts stopped before requesting a signature.
MetaMask's [pending middleware](https://github.com/MetaMask/metamask-extension/blob/af2aef8d69f6031eb75c07473cf795ac26abdc58/app/scripts/lib/middleware/pending.ts)
returns the pending-nonce callback result, whose implementation returns a number.
The previous SDK applied canonical wire-format validation to that wallet response.

SDK `331b27aca409d21dcd64ebd766e8a222796caf40` now compares bounded hex strings
and non-negative safe integers numerically during wallet preflight. Invalid,
missing and unsafe values still fail before submission; quoted transaction
fields remain canonical and unchanged. Dashboard
`80c51458d49384aa2c9a752465af4512b7539f95` includes this SDK and places the fee
notice below the Bridge card.

The authorized dashboard-only update deployed local image
`sha256:75ca25f68491bd46bddd973eb0fa896e5f385355e73da71b5434c760a7ead886`.
Validation: 11 SDK tests, two payment-wrapper tests, production build, and 16
browser scenarios against both source and this exact image passed. The browser
scenarios cover MetaMask/type-2 and Rabby/legacy, native/ERC-20 bridges, numeric
and padded nonces, changed-nonce rejection, quote failure and the app fee ceiling.
The screenshot's 0.01 native transfer also reached exactly one mocked wallet
request after the fix. Layout checks passed at 1440, 390 and 320 pixels.

Public desktop/mobile rendering passed, and the served JavaScript hash matched
the tested image: `041e25cdb4d0d13ac09680f4ffcab5f70ca87f494c7ab0e2593c394c86b2168d`.
The UI environment, node/signer container identities and start times were
preserved. UI/signer were healthy; all three services had zero restarts.
Rollback changes only the dashboard image to the previous recorded digest;
there is no protocol, storage or configuration migration.

The `artifacts/l2-fee-wallet-fix-20261015/` directory in the operator checkout
contains local-only before/after logs, screenshots,
source bundles and image/bundle verification. These browser tests use wallet
fixtures, not installed extensions. No transaction was broadcast for this fix;
Rust/Kurtosis/replay suites were not rerun for this JavaScript-only change.
An attempted read-only type-2 quote using the earlier live nested canary returned
`unsupported_intent`; it is not counted as successful live quote/settlement
validation. The initial browser launch used an unset browser-cache path and
failed before execution; selecting the existing browser installation fixed the
harness setup. The normal Vite bundle-size warning remains.

## Bridge fee reconciliation

The subsequent user bridge `0x3bc2940e0e3737f66384e06fd81ff5e8358fd8f07b92d194af3b7e7812b89076`
settled through posting `0x9b8040557d45331260f40f802fab9f77c4f862a6b102fd45844ef02a33815d16`.
Read-only RPC receipts, the posting call trace, decoded native events and before/after
balances reconcile the complete 0.001 xDAI principal and all transaction fees.
The L2 user paid `1399760428773168` base units; `1399760408966976` reached
the L2 beneficiary. The poster spent `314017001280615` L1 base units, comprising
`182945001280615` execution and `131072000000000` blob fees. This is the full
posting cost, without allocating part to another fee-paying user.

Under the retained 1:1 conversion, the tip covers 4.45759 times the actual L1
cost. The excess comes from the conservative quote projection: 1,035,395 L1 gas
versus 182,945 actually used. Reconstructing the projection, blob cost and 20%
risk allowance reproduces the signed priority fee exactly: `3462970572` base
units per gas. The original quote response was not retained, so this is a model
reconstruction, distinct from the measured receipt and balance evidence.

The [fee report](bridge-fee-analysis.json) records exact amounts, source/protocol/image
identity, policy, reconstruction components and validation limits. Full receipts,
trace, extracted fee observations and a SHA-256 inventory are local-only under
`artifacts/bridge-fees-3bc2940e-20261015/` in the operator checkout. This analysis
did not broadcast a transaction, change services, run independent KZG verification
or replay a follower. It does not establish off-chain profitability or liquid L1
proceeds: income is on L2, and configured proving/operations/treasury/profit
allowances remain zero. The current fee policy has no automatic rebate of the
difference between projected and actual L1 settlement costs.
