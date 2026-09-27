# Candidate framing reserves and the 5% default

| Field | Value |
| --- | --- |
| Status | Implementation and validation in progress |
| Source baseline | `a4f09f17e777590a7a599ddb675cfd894449257d`; branch `feat/cross-chain-admission-throughput` |
| Deployed baseline | `aa515c8720f7943df2d9d0ec147e2252ace1a21f` |
| Protocol | Clean initialized `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Contract | [Accepted follow-up](../../proposals/cross-chain-admission-throughput.md#fixed-admission-overhead-follow-up-accepted-for-implementation) |
| Baseline evidence | [Previous verified wave](../2026-09-27-admission-nested-delivery/README.md) |

The user authorized replacing the fixed admission overhead with candidate sizing
and changing the default gas margin from 20% to 5%. The baseline already runs an
explicit 5% override. Preserve that and all operator state during rollout.

The baseline's 25 source posts had a 25,264–122,808 gas gap between incremental
and full sizing. Every gap was additional calldata gas; the fixed reserve was
600,000. The observed maximum was only 59 entries/two blobs, so these measurements
do not establish a universal replacement constant.

Implementation accounts incrementally for native batch framing, root/position
hash rewrites, dynamic offsets, proof allowance and the target fork's maximum
blob indices. Calldata floor and execution projection must both fit. Static
entries require the existing general execution projection rather than retaining
discounts whose final batch guard would revoke. Final ordered execution, original
signed source limits, group boundaries and settlement ownership remain required.

Validation will compare bounds with actual ABI encoding over entry/blob/offset,
return-data, nested/static/revert and discount boundaries; verify default and
override propagation; run affected unit/process/configuration checks. Then build
exact-revision local shipping images, deploy to the existing authorized devnet,
repeat the wallet-locked verified workload, and compare canonical sources,
cutoffs, preparation and gas estimates. Full logs/datadirs remain local under
`artifacts/admission-framing/`; portable evidence will be added after execution.

## Initial validation

- Rust 1.94.0 locked unit suites: Composer 275 passed, one existing manual test
  ignored; L1 144 passed. Six framing regressions cover complete ABI encoding,
  calldata floor activation, transactional discount revocation, static-only cheap
  checks, the 65-blob general fallback with cleanup, and overflow rejection.
- Workspace/all-target Clippy passed with warnings denied; formatting passed.
- All 14 Chiado configuration checks passed; both mandatory settlement policy
  configuration checks passed. Explicit overrides remain unchanged.
- Grafana panels 1/3 executed against retained history; additive columns are null
  for historical estimator versions. No schema migration or collector restart.
- First unit attempt exposed three test fixtures tied to the old fixed reserve;
  corrected their derived boundary expectations. A later sandboxed test command
  could not bind localhost servers (15 L1 tests); the same suite passed with
  network access. Logs of these failures are retained locally, not counted as
  successful validation. No production failure was hidden by a retry.

Native process validation, exact-source images, deployment and the comparable
live workload are pending at this implementation checkpoint.

Nine focused native process cases passed at `bee97db`: repeated 45-source waves
on both networks, static-only exclusion, existing-proxy multicalls, middle-group
partial consumption, actual companion gas on both networks, and both receipt
fee-cost cases. The latter assert the absent margin setting reaches final signing
as exactly 5% above native required gas. Wave assertions compare the incremental
bound with production full batch sizing and retain independent replay checks.

A configuration review also found explicit 2000 values in all six shipped
Kurtosis profiles and its example. These are updated to 500. Kurtosis tooling:
13 passed, one opt-in real relay-resource test excluded (unrelated to margin
transport); both Compose policy propagation checks passed. Custom strings retain
runtime validation and explicit overrides, including 2000.
