# Composer recovery across consecutive skipped slots

| Field | Value |
| --- | --- |
| Status | Implementation and validation in progress; not deployed |
| Authorization | September 28, 2026: user requested fixing the inclusion failures under our control |
| Fork / branch / PR | `eduadiez/rollupX`, `feat/cross-chain-admission-throughput`, PR #42 |
| Baseline / PR head | `c8260cec8e76a267c96cea1313d31b55a8383f75` |
| PR base | `fd77a2ebc147850333171045dbd263ec74156268` |
| Protocol | `9d13218a355e7d543ca8a9927eea2962600b716d`, unchanged |
| Contract | [Replacement design](../../proposals/skipped-slot-speculative-retry.md), [RPC ownership](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission) |
| Live reproduction | Five-minute Chiado run, preserved in the [PR #42 description](https://github.com/eduadiez/rollupX/pull/42); detailed local report in `docs/audits/2026-09-28-chiado-five-minute-spam/` |

The previous scheduler exhausted its one-replacement allowance after two missing
slots, leaving produced targets 23272510 and 23272544 without attempts. Admission
had not rejected those targets; no trigger was emitted. Four other submitted
misses followed skipped slots and reached the relay without an eligible bid at
the proposer request. Captured builder logs show the previous slot's build
timeout at -42.736, +515.203, +486.178 and +259.511 ms relative to those targets.
Contention remains a hypothesis; this repository does not identify the deployed
builder source/configuration, so no remote builder change is claimed.

The authorized extension uses four bounded replacements on an unchanged parent,
shared by scheduler and submission gates. Each advances one slot, retains all
submission history, preserves one current owner and reuses the poster nonce.
Canonical derivation and exact-parent checks remain mandatory. The five-target
bound still suspends work during a sufficiently long outage. A structured event
reports exhaustion without declaring settlement failure.

## Validation record

Evidence is retained locally under `artifacts/consecutive-skips-20260928/`.
It is ignored and unavailable in another checkout unless explicitly copied.

- Initial new-test compilation failed because a parameterized assertion also
  touched an unrelated historical-prefix test. Corrected that test edit before
  baseline execution; retained `baseline-compile-initial.log`.
- Baseline process regression selected one case and failed as expected: no
  dispatch for the required third target after two skipped slots, timing out at
  17 seconds. Source `c8260ce`; [baseline executable identity](evidence/baseline-identity.json).
  Logs/datadirs remain local under `baseline/`, with output in `baseline.log`.
- Affected unit suites passed: 292 Composer (one manual profile ignored),
  60 driver and 145 L1 tests. This includes invalid/overflow target checks,
  exhaustion/reset and four superseded generations followed by partial canonical
  consumption and reorg. These unit results do not establish process/live behavior.
- Candidate process checks, release-image identity and live results are pending.

Existing operator runbook edits, recovery audits and deployment state are preserved.
The protocol, dependencies, ABI, follower replay and signer profile are unchanged.
Older Composers retain the one-replacement policy; an upgrade changes only future
Composer scheduling. Restart still has the documented in-memory queue limitations.
