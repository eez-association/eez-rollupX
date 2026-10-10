# Canonically bound receipts in protocol network tests

| Field | Value |
| --- | --- |
| Status | Draft suggestion for upstream test tooling; not implemented or accepted |
| Fork baseline | PR #79, `aaf25716d84353de7f26e6b6f517667a8e0b1f61` |
| Protocol baseline | `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f`, unchanged |
| Date | 2026-10-10 |
| Evidence | [First and repeated CI failures](../audits/2026-10-09-conditional-slot-production/final-validation.md#remaining-ci-blocker-optimistic-receipt-used-as-stable-evidence), [live receipt movements](../audits/2026-10-09-conditional-slot-production/live-mixed-validation.md) |

## Problem and scope

The pinned `script/e2e/lib/E2EBase.sh::_poll_receipt` returns the first receipt.
`publish_user_tx` saves its block number. The full `network-scenario.sh` L2
trigger path then uses that cached height for settlement discovery and evidence.
An optimistic receipt can disappear or move after an excluded attempt; a
by-height lookup can refer to a replacement block without the transaction.
The independent verifier correctly rejects that stale evidence. Green runs
without this timing event do not remove the assumption.

Suggest a **test-runner-only** upstream change. No Solidity, wire, consensus,
scheduler, proof validation, or protocol pin change is included in this PR.
Do not patch the submodule at runtime or weaken the fork's strict gates.

## Proposed acceptance contract

1. Track the original transaction hash under one monotonic deadline. Re-read
   receipts after disappearance or movement; do not resend and do not restart
   the deadline after each observation.
2. For an L2 trigger, require its receipt block hash to match the canonical
   block at that height and be at or below the safe head. Bind settlement
   discovery to that **hash**, not the first observed height. A successful
   receipt, matching height or correlation object alone is insufficient.
3. Authenticate the corresponding L1 settlement and consumed commitment;
   recheck receipt and canonical block identity before fixing the verification
   target. Reorg during those reads invalidates the snapshot. Correlation
   unavailable/off needs an equally strict canonical scan path, not a shortcut.
4. Preserve all existing status, event/table, posted calldata, causal, blob,
   independent replay and complete test-selection assertions. Partial consumption
   must not be mistaken for completion. Preserve first failure evidence.
5. Apply consistently to full and staged verification, single and multiple
   triggers; keep L1-origin handling distinct from optimistic L2 inclusion.

Regression scenarios should cover first-optimistic → absent → relocated →
settled, same-height/different-hash replacement, unrelated by-height correlation,
partial consumption, reverted receipt, invalid identity, L1 reorg during reads,
missing correlation, and deadline exhaustion. Stable receipts should pass without
an unconditional sleep. A real excluded-attempt scenario must subsequently pass
the unchanged semantic and independent verification gates, not just a mock.

## Alternatives and merge decision

Longer sleeps or rerunning CI hide the race; accepting whatever is at the old
height can validate the wrong transaction. A fork-side RPC filter or dynamic
submodule patch obscures the actual tested upstream contract. Neither is proposed.

Keep the pin unchanged and report this limitation explicitly until upstream
accepts a repair and a separately authorized pin update passes the complete
matrix. The fork characterization test documents the current failure; its pass
means the assumption was reproduced, **not that the bug was fixed**. PR #79 stays
draft while this release gate remains unresolved. No upstream PR has been opened
by this record.
