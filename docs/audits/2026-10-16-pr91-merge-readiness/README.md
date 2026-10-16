# PR 91 CI correction and fee/ownership review

| Field | Value |
| --- | --- |
| Date | 2026-10-16 |
| Scope | Fix the PR's CI blockers and trace fee enforcement and accepted-request ownership; no deployment or protocol upgrade |
| PR base / head inspected | `2c19e36bed2720c00f89841ccfca47ec7c921e06` / `d69cccf299275276c1873d100c0928228e32a7ec` |
| Exact fork baseline | `eez-association/eez-rollupX`, local audit-only successor `264bfad81d52789b68dc842c40178f2426c33c4a` |
| Protocol gitlink | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, initialized, clean and unchanged |
| Existing runtime qualification | [Posting-price correction](../2026-10-16-posting-price-feedback/README.md), source `aa8d8a5894c69502c67dcae35d5184bc9a79b036` |
| CI baseline | [Run 38913810526](https://github.com/eez-association/eez-rollupX/actions/runs/38913810526) |

This is a focused engineering review, not an independent security audit. The
fee and queue implementation is unchanged by these corrections. The dependency
audit also identified a required TLS patch, described below. Final merge
readiness requires all required CI lanes to pass for the proposed revision;
older live results do not replace that gate.

## Confirmed CI failures

The lint job stopped at Gitleaks 8.30.1, before formatting, dependency checks,
tooling tests and Clippy. Its six findings are three occurrences repeated in
branch history and the PR merge diff:

- `EEZ_L1_FEE_REFUND_POLICY` is the public environment-variable name in the
  configuration test, not a credential.
- Two fields in the simplified-fee evidence manifest contain the same SHA-256
  digest of `local-api-check.log` and `public-api-check.log`. Both retained files
  were hashed again and match the recorded digest.

The correction adds exact-value **and** exact-path exceptions. Default rules,
history/merge scanning and rejection of inline suppression remain enabled. A
regression scans real temporary Git commits: reviewed values pass, a synthetic
credential in each same file still fails, and the reviewed values outside their
approved paths still fail. The corrected regression also fails against the old
configuration. Initial directory-mode test fixtures did not exercise Git's
relative-path matching correctly; those failed runs are retained separately.

The cross-chain shard 2/4 ran 28 tests: 27 passed and
`fee_quotes::compact_ordinary_calls_boundaries_and_fee_sensitive_rejection`
failed at the `FeeQuoteProbe` artifact read. Its compile-time
`CARGO_MANIFEST_DIR` names the producer filesystem, not the runner executing
the relocated Nextest archive. The existing `eez_testkit::repo_root()` honors
Nextest's runtime workspace remapping. The test now uses that resolver, as the
other integration fixtures do; no missing-artifact rebuild or skipped assertion
is introduced.

The same test passed locally with a fresh source-built harness and a separate
runtime fixture workspace: one selected test, no ignored tests. An `openat`
trace confirms it read the relocated `FeeQuoteProbe` artifact. This check used
the previously qualified release Composer, follower and signer executables
after verifying their hashes; it qualifies the fixture correction, not the
subsequent TLS dependency update. Source/harness and runtime identities are
recorded separately in the validation manifest.

Running the remaining lint checks locally exposed four additional failures:
`RUSTSEC-2026-0346` through `RUSTSEC-2026-0349` affect the locked `rustls 0.23.45`.
Both `cargo deny check advisories bans sources` and `cargo audit` reproduced them.
The upstream [0.23.46 release](https://github.com/rustls/rustls/releases/tag/v%2F0.23.46)
fixes all four; the correction is a targeted lockfile patch update,
without adding advisory exceptions or moving Reth, Stateless, Gnosis, REVM,
Alloy or the core protocol. This changes the candidate's TLS dependency; the
previously deployed binaries cannot qualify that dependency update.
Cargo also reselected the macro crate's permissive `syn` dependency during
resolution; that unrelated edge was restored. A structured comparison confirms
that only the `rustls` version and checksum differ from the original lockfile.

Protocol PR 56 is not a dependency of these fixes. The original Kurtosis CI job
passed on the existing pin without PR 56. Its general protocol network suite runs
with fee policies unset, and the fee-specific native tests use fork fixtures.
Enabling tariffs in the general protocol driver would require its
quote-capable client, including the [explicit nonce correction](../2026-10-16-protocol-pr56/README.md).
Changing the protocol pin is outside this correction.

## Fee enforcement review

The accepted contract is the [settlement policy](../../configurable-settlement-policy.md),
including the operator-authorized largest-payer L1 subsidy. The review traced:

| Invariant | Production path and existing regression coverage |
| --- | --- |
| The 20% markup is a quote target; cost plus configured risk is the L2 floor | `Policy::required_payment` / `check_tariff`; rounding, clipped effective tips, conversion, exclusions, beneficiary and expiry tests; 1%/5% margin process cases |
| Signed admission uses the actual sender, nonce, gas and fee fields | Front `intercept_send_raw` → `FeeQuotes::check_signed` → `quote_signed_fee`; predecessor replay, nested native replay and underpayment process cases |
| A quote cannot authorize final dispatch | Composition remeasures prefix settlement cost; `observe_bundle_outcome` finalizes the actual signed bundle and checks native receipt revenue before submission |
| Ordinary L2 settlement remains subsidized | Ordinary quote classification and dispatch's outbound-source guard; no fee requirement is inferred from zero-priced native callbacks |
| L1 revenue comes from an external source | Final native receipts select the largest recoverable total payment; poster/recipient/excluded senders do not count; submitter binds the mandatory source to the exact signed bundle |
| Missing refunds cannot stop dispatch or become received revenue | Cost observations remain separate from admission/ownership; duplicate attribution, reorg retraction and continuation-without-payment tests |
| Quote cancellation cannot release a still-running worker's permit | Separate bounded quote/admission pools, permits owned by blocking execution, cancellation and shutdown regressions |

No additional runtime defect was identified in these paths. The quote estimator's
private assumed proof result does not reach real dispatch, which still requires
the signer and final native validation. Fees remain subject to the configured
conversion and service-cost assumptions, and do not guarantee realized profit.

## Queue ownership review

The [RPC ownership contract](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission)
and [queue requirements](../../proposals/cross-chain-queue-liveness.md) govern this
review. The traced boundaries were `HeldPool`, `OptimisticallyIncluded`,
composition's tariff deferral and submitted-attempt reconciliation.

- A tariff failure removes speculative effects, defers the request and its nonce
  dependents, and permits independent eligible sources to continue. It does not
  charge a settlement attempt.
- Queue/reservation transfers share the pool lock. Reconciliation checks canonical
  identity/generation and whole replay groups; a post receipt alone is insufficient.
- Late observations match the exact attempt identity. Resolution cannot release
  users owned by a newer generation; replacement and explicit readmission retain
  that protection.
- Native preflight backoff preserves whole groups. Only an attributed singleton
  companion rejection can charge the bounded execution counter; funding, transport
  and state uncertainty cannot train that bound.
- Refund accounting does not release requests. Partial consumption, reorg and
  restart continue to follow the existing ownership contract.

No additional runtime defect was identified. Pending requests and refund plans
remain in memory; a restart is not durable queue recovery. Broad builder support,
durable accounting and the previously excluded protocol profiles remain separate
qualification work, not coverage claimed by this review.

## Validation evidence

See [validation.json](validation.json) for executed checks, exact artifact hashes
and the limits of the retained evidence. Raw diagnostics are local-only under
ignored `artifacts/pr91-merge-readiness-20261016/`; they are not available in another
checkout and have no guaranteed retention. The original failed CI logs remain
linked above. No live network was restarted or modified for this correction.
