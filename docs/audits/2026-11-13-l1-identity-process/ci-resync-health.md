# CI outage health-check correction

The failed embedded E2E slice was a test-observer false positive. The test
deliberately withheld canonical L1 header reads, then its final blanket health
assertion rejected the resulting resync error even though production, settlement
and follower replay had recovered. Its code change is confined to
`settlement_policy.rs`; production code and the global health dictionary remain
unchanged.

## Retained failure and accepted behavior

[CI run 43460984461, embedded slice 3](https://github.com/eez-association/eez-rollupX/actions/runs/43460984461/job/132029985896)
failed `ethereum_ordinary_burst_is_budgeted_across_sync_intervals`. The PR head
was `dbe3d556180aa088e320fef737807fcb2f864176`. CI checked out virtual merge
`24256c95a6b526e6fef6e030b31c723f907eb7fc`, whose tree is identical to that head
(`faa94f46e259d6f554e16edbe6f814db01a787ec`). Its verified test payload reference
is recorded in the [evidence manifest](ci-resync-health-evidence.json).

The retained Composer log contains one fatal-class signal:

```text
eez.deriver.resync.failed
resync failed; will retry after the next L1 event
L1 catch-up scan error: L1 provider error: get_block_by_number(6): RPC error -32000
```

The watcher had fetched a head before the injected outage rejected the
deriver's numbered-header read. Recovery then hit the same RPC fault.
`Deriver::try_recover` deliberately continues and retries after a later L1 event.
Ordinary blocks continued; both Composer and follower subsequently advanced to
matching safe heads 30, 130 and 140. The test's receipt, DA-budget, canonical
replay and post-recovery bridge assertions precede its failing health check.
The first failing assertion counted the earlier resync signal; its process-exit
check had passed. No runtime panic, divergence or stopped process was established.

The accepted [clock and ownership contract](../../proposals/l1-origin-and-offline-proving.md#clock-settlement-and-ownership)
requires ordinary production to continue through L1 unavailability. The test,
deriver, global signal dictionary and fault proxy were already identical to the
original `0d46d648` baseline. The global health assertion bodies are unchanged
too, despite other earlier testkit edits. This was an intermittent existing
test expectation, not a demonstrated production defect or Quinn regression.

CI artifact `14059788284` and its archive/log hashes are retained. That archive
contains logs and Nextest output only: no failed CI datadir or individual ELF
hashes were exported. The OCI payload digest identifies the consumed payload;
it is not an independently recovered per-binary digest.

## Narrow correction

The two existing Ethereum/Gnosis burst scenarios now:

1. Check Composer health strictly before injecting the RPC fault.
2. Capture the fault/recovery signal window through successful canonical
   safe-prefix convergence and matching ordinary receipts. The same deriver
   has then processed settlement after the failed scan.
3. Exercise subsequent healthy cross-chain service, preserving the existing
   value, receipt, blob-cap, block-grid and replay assertions.
4. Permit only the exact numbered-header `-32000` resync error in the captured
   window. Any additional fatal signal after that snapshot fails, including the
   same error again. Other fatal kinds, malformed contexts, other RPC errors
   and startup resync failures remain failures.

Node logs are append-only, so the captured count cannot absorb a later record.
Exit-status and raw engine/fatal checks remain strict over the whole run;
the follower uses the unchanged global health assertion. No global exception,
production retry classification, logging change or queue/attempt-policy change
is introduced. The exact diagnostic-text gate fails closed if wording changes.
It does not cap matching error records within recovery; canonical convergence
must still complete within the existing bounded waits.

The deterministic observer regression uses the actual CI error and checks two
accepted controls and seven specific rejections. It is one unit test of the
observer policy, not a new runtime rejection test or proof-signing exercise.

## Execution and source identity

| Check | Result |
| --- | --- |
| Original CI process run | Failed: 10 of 11 slice tests passed; this scenario reached its final health check with the injected resync record |
| Unchanged local Ethereum baseline, cold source build | 1 passed, 0 failed/ignored, 220 filtered; no resync-failure record |
| Fresh baseline with existing warm-up control before Ethereum | 2 passed, 0 failed/ignored, 219 filtered; no resync-failure record |
| Intermediate observer regression | 1 passed; superseded when window closure was tied to canonical replay |
| Final observer regression | 1 passed, 0 failed/ignored, 221 filtered |
| Corrected warm-up control, Ethereum and Gnosis processes | 3 passed, 0 failed/ignored, 219 filtered |
| Workspace Clippy and format | Passed with `--locked --workspace --all-targets -- -D warnings` and `fmt --all --check` |

The intermittent CI failure was **not reproduced locally**. The unchanged local
passes are not claimed as a fix; the failed CI run remains failing evidence.
The corrected process runs also emitted no `resync.failed` records. Their real
production/signing/DA/settlement/replay behavior passed; acceptance of the
captured resync error is qualified by the deterministic observer regression.

The source harness builds actual Composer, follower and signer entrypoints,
with prebuilt overrides cleared, normal optimized dev profile, Rust 1.95.0,
LLVM 22, two jobs and checkout-owned targets/logs/datadirs. The warm-up control
absorbs runtime compilation before the fresh timed scenario. No performance
comparison is claimed. The rebuilt three runtime ELF hashes match the baseline;
the changed test executable has its own hash.

The recorded test-source digest was frozen before execution and equals commit
`2a428467d7a8c1cfe080d1e4fcd7e1fd7ee1d8f0`. That commit was made during the
process run; the manifest records the invocation's earlier dirty-tree state
rather than claiming a clean-commit build. No code changed during execution or
review. All commands, results and source/executable/log identities are retained
in the manifest.

The earlier seven identity tests and five existing process regressions remain
their original evidence. Other source suites, the full release/Kurtosis matrix,
external clients, public networks and deployment were not rerun locally for
this test-only change. CI owns the new complete artifact/matrix qualification.
Core pin, profiles, validation version 9, signing domain 7, dependencies and
wire/storage contracts remain unchanged from the preceding PR head.

## Reviews

Ponytail self-review uses recorded guidance revision
`96608e078b2d5ea607c5c3b8b5a54d1c1fba6846`. The correction changes observation
of a known injected fault while retaining canonical progress as the recovery
boundary. The general fatal dictionary and other callers remain strict. The
captured-input regression rejects attempts to broaden the exception, and the
two process profiles retain their semantic assertions. No production policy,
new fault-injection mode or dependency is needed.

The [unaltered final Fable report](ci-resync-fable-review.md) reviews the complete
changed code/test scope and critical callers at `2a428467`, including the Quinn
follow-up whose prior review hit quota. Actual CLI metadata identifies only
canonical `claude-fable-5-2`, first-party provider, standard speed, no fallback
and zero spawned agents. It completed with no actionable code/test findings.
Its documentation finding is resolved by this record and the corrected Quinn
closeout wording. The reviewer ran no tests or hash checks and read the process
logs before they completed; their finished results are appended independently
above. This is static review, not reviewer execution approval.
