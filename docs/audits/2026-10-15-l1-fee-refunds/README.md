# L1-origin fee collection and builder payout validation

Status: implemented, validated and deployed on the existing ROG Chiado L2 on
2026-10-15. Public transaction routes reopened at 22:30:31 UTC after the
configured builder paid all four canaries and independent replay passed.
This extends the completed [L2 deployment](../2026-10-15-simplified-fees/README.md)
with opt-in L1 collection; it does not qualify other builder providers.

## Identity and accepted scope

Baseline: `eez-association/eez-rollupX`, `feat/compact-fee-quotes`,
`a2da0488ec70dafe2ee8d4956be37fc27d8197b1`; initialized, clean protocol
`9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`. The separate operator checkout,
configuration, datadirs and existing networks were preserved. The implementation
follows the [authorized L1 increment](../../proposals/cross-chain-fee-recovery.md#l1-collection-increment-authorized-on-2026-10-15)
and [refund profile](../../configurable-settlement-policy.md#l1-builder-refund-profile).
Revenue may remain on its source chain. No second user payment/signature,
protocol change, genesis migration or fee conversion was introduced.

The initial profile settles one whole L1 source group, including nested calls,
per native post. It uses an externally funded user tip, a fixed percentage refund
and a dedicated EOA recipient. Both ordinary transaction encodings share the
compact quote API and internal gas discovery. The quote targets the configured
profit; signed admission and final native dispatch require costs plus risk.
Canonical receipt accounting independently observes the builder's payment.

Backend implementation: `8688d521e1a1037f3a6b49c494ad2f8fac8c7552`.
The runtime images use the unchanged release Docker recipes and locked
dependencies. Their extracted executables, rather than independently rebuilt
runtime binaries, are used by the final process tests. These are local
development images, not CI-promoted artifacts.

The matching local dashboard change is
`afd3e3b0910010b6da7cc966ee810e5555135863`, based on the deployed
`dcf4ac71b62ebc7bf5ac10cdb426e1f48ce11e00`. It adds an independent L1 quote
enable flag and fee cap, uses the same single-signature helper on both fronts,
and retains ordinary L1 sends. Its source has not been pushed, as requested.

## Findings and first failures

The first two process tests successfully quoted and admitted legacy and EIP-1559
nested sources, but their user receipts reverted. The embedded development
`eth_sendBundle` adapter forwards transactions into a normal priority-sorted
mempool. The higher user tip can therefore place the source before `postBatch`.
It also ignores refund fields. Those tests could not establish successful ordered
settlement or payment; their logs and datadirs were retained. The adapter's
misleading ordering comments were corrected, without changing its behavior.

The process cases now assert this failure explicitly, verify the serialized
refund terms and required payer, and require the canonical missing-payment
observation to pause new L1 quotes. Their initial replay check sampled `latest`
while ordinary posts were deliberately delayed; it was corrected to compare the
settled `safe` prefix. Passing accounting or a bundle acknowledgement alone is
never recorded as a successful refund.

Review also added boundaries preventing a later native post's payment from being
attributed to an earlier post, and stops new L1 funding if one canonical payment
would be credited twice. Reorgs retract previous observations. These are
in-memory operational records, not durable queue or financial recovery.

The first isolated Kurtosis startup rejected a mismatched test configuration:
the cost policy named the selected beneficiary, but the corresponding
`EEZ_L2_FEE_RECIPIENT` environment value was omitted. This happened before fee
traffic. Logs were dumped and the owned enclave's containers stopped; its data
was retained. A fresh enclave uses the explicit matching value. Unrelated
enclaves and the existing Chiado deployment were not restarted.

The first fresh-follower invocation also rejected a test configuration before
opening its RPC: the shell YAML helper retained single quotes around the
generated `EEZ_SETTLEMENT_START_AFTER_BLOCK: '3'`. The corrected invocation
explicitly exports the same numeric value, `3`, used by the deployed test node.
The original failed log/datadir is retained separately from that invocation.

## Validation evidence

The final arithmetic/configuration/attribution unit run passed 357 Composer,
193 L1 and 88 node tests; one pre-existing Composer timing-profile test remained
ignored. It includes mock RPC cases for missing, ambiguous, wrong-recipient,
wrong-payer, reverted, delegated/contract-cost, later-post and reorged payments.
Six SDK tests and 19 Chiado configuration tests passed. Initial Clippy
style/import failures were fixed without relaxing lint settings; final workspace
Clippy and formatting passed. All 13 fee/Safe process tests passed with the
frozen runtime executables, including both L1 builder-rejection cases and
independent follower replay. Nine focused settlement/admission regressions also
passed, covering both Ethereum and Gnosis fee sampling, whole-group gas limits,
mixed queues and ordering with the L1 refund policy disabled.

The real builder included four paid sources: nested calls and Safe 1.4.1
creation, each in legacy and EIP-1559 format. The checks rejected a zero-tip
source, required one signature per accepted source, verified ordered native
posts and canonical application effects, and matched each actual refund to the
recipient's balance increase and measured posting/blob cost. All four canonical
cost events report `refund_status = qualified`; the final quote remained usable.
Each post's blobs passed independent KZG/native-payload authentication. The
ordinary blob/correlation smoke test also passed. With its numeric timing input
corrected, the fresh follower reconstructed history from canonical L1 alone and
matched 114 checked blocks across six authenticated batches and six ordinary
receipts, reaching safe block 318. This includes the four refund-bearing posts;
the follower did not receive the Composer's database or speculative state.

The dashboard passed its TypeScript/Vite build, eight fee-helper tests,
32 mocked fee-submission browser scenarios and 27 mocked bridge-gas scenarios.
They cover both directions and wallet fee formats; they do not exercise real
wallet extensions or broadcast transactions. Seven isolated Docker configuration
checks covered absent, empty, enabled and invalid L1 fee settings. The matching
dashboard Docker image built successfully.

The controlled builder is
`ethpandaops/reth-rbuilder@sha256:916ba84da2690e3e368f57da5b0c73bfd919b452de8b95f64895846e0f3a8308`,
whose source label identifies `flashbots/rbuilder` revision
`6037fa728b13bf1806e16fff2586414216f6b8fa`. Its
[pinned parser](https://github.com/flashbots/rbuilder/blob/6037fa728b13bf1806e16fff2586414216f6b8fa/crates/rbuilder-primitives/src/serialize.rs)
accepts one selected refund transaction and defaults to an immediate payment.
This source inspection establishes API behavior, not a received payment or
support by every external builder.

Large diagnostics are local-only in
`artifacts/l1-fee-refunds-20261015` in the operator workspace. They are not
available to another checkout or uploaded as public artifacts. No private keys,
operator URLs, bulk logs or databases belong in this record. A sanitized result
summary and artifact identities are in [result.json](result.json). The replay
used the existing `verify-policy-replay.sh` environment preparation with its
final Cargo invocation replaced by the already built test harness; the same
extracted follower executable was retained. Blobscan, the full protocol stress
matrix and actual wallet-extension tests were not run.

## Chiado handover and received payments

Both public transaction fronts were paused and both accepted queues were empty.
The node and signer stopped cleanly. Complete stopped databases were copied to
new paths, retaining the original state, configuration and images. The candidate
node/signer pair and matching local dashboard image were installed without
changing contracts, genesis, protocol, keys or the existing L2 fee policy.
The existing poster EOA receives a 90% refund of the selected L1 source's tip.
The dashboard enables its separate L1 fee flag; its existing L2 setting remains
enabled. No dashboard source was pushed.

The already-funded operator workload wallet submitted four sources with a
per-source quote reservation cap of 0.01 test xDAI. The real Chiado route paid
each refund in the inclusion block. Accounting verified the payer, recipient,
ordered source, payout uniqueness and refreshed cost floor. Because the refund
recipient also pays the post, the balance check subtracts that actual posting
cost rather than incorrectly expecting the gross refund as its balance change.
The comparison covers the fee-bearing posts; ordinary L2 settlement remains
subsidized under the existing policy.

| Source | Posting + blobs, L1 base units | Received refund, L1 base units |
| --- | ---: | ---: |
| Nested, legacy | 365775001642921 | 459930002022592 |
| Nested, EIP-1559 | 306062001224930 | 388274401542876 |
| Safe creation, legacy | 307068001231972 | 389481601498892 |
| Safe creation, EIP-1559 | 476650001209523 | 613980001454983 |

The configured 20% profit is a quote target, not an exact receipt surplus.
The conservative payout allowance uses the posting effective gas price, while
the builder can pay the EOA transfer at a lower price; execution estimates can
also differ from the final receipt. Both deductions and projected recovery are
visible in the debug quote. This implementation does not automatically rebate
that difference to the source user.

All four live blobs passed independent KZG/native-payload authentication. A
separate follower started from a copy of the **stopped pre-upgrade checkpoint**,
with no signing keys or sequencer RPC, then derived the canary from canonical
L1 data. It matched safe block 709695, its block/state hashes, nested value 782
and both Safe owners/thresholds. This is independent incremental replay, not a
fresh replay of the entire public chain from genesis. The fresh-genesis replay
was the isolated Kurtosis check above.

Both live fronts returned successful compact legacy/EIP-1559 options before
reopening. The public dashboard reported both fee flags enabled, and all four
public Composer/direct-RPC routes returned their expected chain IDs after
reopening. Live diagnostics and consistent rollback state remain local-only
under the operator's retained upgrade directory; [result.json](result.json)
records sanitized identities, transaction hashes, amounts and evidence digests.

## Limits and remaining work

One complete L1 source group is funded per post. Other builder routes require
their own native-blob payout qualification. The pause flag and refund plans are
in memory; restart is not durable financial recovery and requires operator
requalification after a payment failure. No production throughput, full protocol
stress matrix, Falcon workload, durable financial accounting or broad builder
coverage is claimed. The tested Safe fixture is the standard ECDSA Safe 1.4.1.
