# Bounded final-runtime latency comparison

Status: prepared, awaiting the final runtime and the exclusive local measurement
window. No new network or workload has been started. The admission investigation
may change runtime behavior, so measurements against the preceding `1109dbab`
runtime would not close the final-candidate criterion.

## Question and boundary

Repeat a small, matched baseline/candidate comparison after the settlement
preparation correction, preserving semantic settlement and fee checks. The
previous 900-second mixed campaign observed canonical-settlement p95 of
25.415 seconds on `a50c326d` and 31.599 seconds on `28049ff7`. Those first results
remain evidence for their exact executables; the later correction does not
retroactively explain that difference.

The [previous decomposition](../2026-10-24-extended-validation/stabilization.md#settlement-latency-decomposition)
found no missed invocation after a `prepare=true` decision in either campaign.
Longer observed intervals overlapped deliberate no-prepare decisions. This is
an association, not a demonstrated cause or a counterfactual settlement result.

## Prepared experiment

- Baseline: `a50c326d99f7cd1c21406eedbf370a344015a6b5`; protocol
  `84a5b5207880f192a6a5896cbfe0bac5249db47f`.
- Candidate: the final verified release payload after this validation round's
  runtime fixes. Its identity must be recorded before deployment.
- Two fresh owned enclaves, run serially: `eez-latency-base-20261025` and
  `eez-latency-final-20261025`. Earlier enclaves and their data remain untouched.
- Identical argument values except node, signer and deployment image digests;
  each revision uses its own deployment/genesis and startup wiring. The accepted
  history, system transaction, fee-recipient and signer-RPC changes remain active.
- A 300-second sequential ordinary offer schedule at 5 transactions/second,
  alongside four existing mixed cross-chain waves: 1,500 scheduled ordinary
  offers and 32 cross-chain sources. Ordinary transmission starts at phase two
  seconds of an L1 slot, after its preparation RPCs; actual deviation is retained.
- Use the existing sender, wave checks and authenticated batch collector.
  Local copies change only the planned totals, duration, start-phase alignment
  and corresponding exact count assertions. Signed submission and receipt,
  nonce, balance, cross-chain value/return, ownership, KZG and fee checks remain.
- Preserve every rejected or ambiguous submission and first failure. Record
  offered, accepted, locally included and canonically settled work separately.
  Measure local inclusion from the exact receipt hash's commit event and
  settlement from authenticated, hash-bound canonical advancement events.
- Start with at least 12 GiB host memory available; stop owned producers below
  5 GiB. No concurrent owned build, process test or load campaign during the
  measured intervals. Stop only owned networks and preserve their data.

This shorter experiment is a new paired comparison, not a repeat of the earlier
three-stage campaign. One pair on a shared host cannot establish maximum
capacity, an SLA, or the cause of a historical p95 difference. Cross-chain
completion timing and intentional protocol costs can still differ.

## Checks completed before live execution

The seven retained baseline runtime executables, archived test executable and
three cached immutable images were rechecked against the original verified
handoff. Their hashes, source, protocol and release profile all match. There was
no build or container lifecycle operation.

The read-only latency analyzer was checked against both frozen previous
campaigns. It exactly reproduces all recorded plan counts, missing-invocation
counts and aggregate/conditional settlement percentiles, as well as the previous
local-inclusion results. This validates the analysis against existing evidence;
it does not validate a new runtime.

Local preparation and evidence are under ignored
`artifacts/latency-validation-20261025/`, including `plan.json`, the baseline's
`reverified-identity.json`, exact workload adapter diffs, and both historical
analysis checks. These files are local-only until compact final results and
their digests are recorded here. No independent replay or live pass is claimed
for this new comparison yet.
