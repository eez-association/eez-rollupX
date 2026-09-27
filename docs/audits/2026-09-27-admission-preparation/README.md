# Preparation profiling and prefix execution reuse

Status: implementation and validation in progress. No candidate devnet result
is established by this record yet.

| Identity | Value |
| --- | --- |
| Branch | `feat/cross-chain-admission-throughput` |
| Baseline checkout | `2b9ca04e87f278a1e9bed0d25420f3fa20b86a08` |
| Baseline running source | `7fdb3fc4dae1156051a77bcbdf00301914085f5b` |
| Protocol | `9d13218a355e7d543ca8a9927eea2962600b716d` |
| Baseline wave | Protocol run `20260927-083400`, 870 sources, 26 canonical posts |

The user authorized focused validation before the local-image Chiado trial,
followed by broader applicable tests before completion. Design and invariants:
[preparation follow-up](../../proposals/cross-chain-admission-throughput.md#september-27-preparation-follow-up-accepted-for-implementation).
The previous [runtime/wave audit](../2026-09-27-admission-refinement/README.md)
establishes the baseline image identities and semantic assertions.

## Where preparation time goes

Canonical candidates only. Mean total 453.29 ms, median 456.44 ms, maximum
672.87 ms. Proof and finalization are outside these preparation intervals.

| Interval | Mean per post |
| --- | ---: |
| Accepted-source lowering, prefix hashes and canonical append | 198.81 ms |
| KZG blob sidecar construction | 172.31 ms |
| Witness and remaining proof-request preparation | 22.50 ms |
| Final block build, encoding and full gas projection | 15.09 ms |
| Refused-source checks/simulation and subsequent rejection work | 14.35 ms |
| Accepted-source checks and simulation | 13.19 ms |
| Other admission setup/scheduling | 10.91 ms |
| Cheap rejection checks and outcome recording | 6.13 ms |

These wall-clock intervals include scheduling and logging. The first row cannot
be attributed entirely to prefix hashing from historical events. KZG comes from
integer-millisecond `admission.prepared` logs, joined to candidate identity and
cross-checked against accepted counts and preparation time. The new runtime
adds separate prefix, blob and witness timing to existing retained event kinds.

Reproduce using [profile-preparation.py](profile-preparation.py). The
[summary](baseline/preparation-summary.json) and
[per-post intervals](baseline/preparation-per-post.csv) are sanitized evidence.
Detailed admission history remains local at
`artifacts/admission-refinement/candidate/detail/events.private.json` and the
sanitized retained timing records at
`artifacts/admission-preparation/baseline/prepared.json`; these ignored local
artifacts are unavailable in another checkout.

Blob admission already estimates the complete group's encoded size without
KZG. Final sidecar generation is cryptographic work required for submission;
estimating blob count cannot replace it. All baseline posts used one blob and
none stopped on blob capacity. Moving or overlapping KZG is a separate
optimization requiring bounded worker ownership and shared-deadline accounting.

## Change and initial evidence

The existing `hash_group` rebuilds every requested growing prefix from its
parent. The candidate retains one checkpoint before post-execution changes,
executes only the newly added suffix, and finalizes a copy with Reth. Changed
or shortened prefixes discard reuse, and construction errors invalidate it.
Final canonical append, block rebuild, witnesses, signer checkpoint derivation
and ordered signed-bundle validation remain independent.

Initial focused local tests passed all three new regressions. A 64-prefix
comparison executed 2,080 transactions in the baseline helper versus 64 in the
candidate: 79.16 ms versus 6.40 ms in that run. This uses a mock provider and
does not measure trie work or establish live speedup. Complete pre-finalization
state and receipt/gas equality are checked separately from header equality
because the mock provider does not compute real state roots. Native and devnet
evidence are required before concluding the live improvement is correct.

Composer unit suite: 267 passed, one existing manual diagnostic ignored.
Focused Composer Clippy passed with warnings denied. Broader tests, local image
build, rollout and live measurements are pending.

## Readiness correction

The first unchanged readiness check rejected the next trial for a supposed
0.201012662866298632 xDAI L1 funding shortfall. Inspection at the protocol pin
showed that `_plan_funding` already funds only deficient workers, and
`_topup_faucet` reserves gas only for nonempty funding chunks. The fork's staged
preflight still assumed 770 empty wallets.

The helper now reads balances under the existing wallet lock and applies the
pinned calculation per chain. The same profile passes: L1 needs 520 worker
top-ups and a 0.739265059018197372 xDAI faucet budget; L2 needs none. Existing
source/faucet funds cover both. Worker targets remain 0.0033 xDAI and no extra
poster transfer is required. Eight focused offline runner tests and Bash syntax
checks passed. Retained before/after checks are local in
`artifacts/admission-preparation/readiness-{before,corrected}.log`.

## Focused process gate

Five source-based native cases passed across `chained_interstate`,
`dynamic_admission`, `partial_consumption`, and `settlement_policy_gas`:
interdependent mixed-direction work, existing-proxy multi-call work, a Gnosis
mixed wave, middle-group resumption, and independent packing after a gas refusal.
These exercise real state/trie construction, signer validation and follower
replay. All selected cases ran once; no failure or ignored case was counted as a
pass. The test run started before the implementation commit, so its original
checkout field names `2b9ca04`; recorded hashes of all three changed Rust files
match committed candidate `6c171fdb3221a6d6183e113e9d05e4c7f657099b` exactly.
