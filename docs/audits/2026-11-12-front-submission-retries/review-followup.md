# PR #163: Ponytail and Claude Opus 5.5 review

What this change does: keeps the single admission deadline and unchanged signed
source identity while reconciling lost replies. This follow-up fixes confirmed
harness errors found while reviewing original head
`fa56a022cb80a722323f821d9c11822f629fe434` against association main
`601ebfa9c7e1a779f1afbd077bd31a3207e7a61b`. Protocol gitlink remains
`84a5b5207880f192a6a5896cbfe0bac5249db47f`, initialized and clean.

Review used the same pinned [Ponytail review instructions](https://github.com/dietrichgebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail-review/SKILL.md).
It traced every caller, the status producer, admission-before-reply ordering,
terminal reasons, curl configuration, parser errors and clock boundaries.
Expected load remains serial verification workloads. The original frozen
review-input provenance was not independently checked.

## Confirmed findings and fixes

| Finding | Concrete case and correction |
| --- | --- |
| Hidden POST retries | A task-owned `.curlrc` enabling `retry-all-errors` caused two `eth_sendRawTransaction` requests after acceptance and reply loss. `curl -q` now disables implicit config loading for this helper. |
| Malformed responses could claim admission or retry | Valid JSON followed by garbage, a hash ending in a newline, or concatenated hash/refusal replies could pass the old stdout-based comparison or authorize another send. Success, refusal and status predicates now require one complete JSON document; hash comparison stays inside jq, with parse failure preserved. |
| Parsing could cross the deadline and return success | Advancing the retry clock inside jq reproduced a successful return at `SECONDS=121`. Both positive admission paths now recheck the original exclusive deadline after parsing. |
| A deadline tick could disable curl timeouts | The clock can advance after the loop condition but before computing `left`, producing `--max-time 0` (unlimited). A controlled Bash DEBUG trap reproduces the extra request. A positive-remaining-time guard now stops before curl. |
| A final timeout could be mislabeled as admission refusal exhaustion | An actual curl timeout with one second remaining exhausted Bash's real clock before ambiguity was recorded. Ambiguous-send classification now occurs before the deadline exit, retaining unresolved acceptance without starting another call. |
| Successful static completion was treated as failure | `composer.rs:2721-2725` assigns `SourceCompleted` to successful source receipts with no mutable effects. It is now recognized alongside `Committed`; `SourceCompletedWithMissingEffects` and every other terminal failure remain failures. No new runtime outcome is introduced. |

The wave-caller regression now invokes the function in an AND-list, where Bash
suppresses `errexit`, proving that its explicit `|| exit 1` protects the caller.
The startup wire-contract comment now distinguishes the testkit substring match
from the harness's exact code/message match. That is the only Rust edit.

## Independent adversarial review

Claude Code 2.1.322 ran with explicit model `claude-opus-5-5`, confirmed in both
initialization and assistant response metadata. The first pass had only Read,
Grep and Glob tools; no subagents, commands, mutations, runtime access or tests.
It inspected the full PR diff, callers, status/admission code and terminal
classification. It did not read AGENTS.md or the linked contracts; the task
agent checked those and independently reproduced the findings.

| Opus finding | Disposition |
| --- | --- |
| F1: terminal failure can abort a deliberate poison workload after reply loss; suggests accepting any matching record | The conservative abort is retained and documented. An old removal observation survives a new submission rejected before pool insertion; `QueueTelemetry::admitted` clears it only after admission. Accepting every matching terminal record could therefore claim a fresh admission without evidence. `SourceCompleted` is a successful outcome and was corrected separately. Unknown statuses also remain inconclusive. |
| F2(a): final timed-out send loses ambiguous classification | Confirmed and fixed, with real curl timeout/real Bash clock evidence. |
| F2(b/c): accept a positive answer even after the deadline | Declined: the task requires one exclusive deadline, including parsing. Late answers cannot authorize success. |
| F3: frozen clocks hide time spent in curl/parsing | Confirmed. Added the real-clock exhaustion regression and explicit parsing-clock advancement. The existing frozen-clock rc=28 test is labeled as transport-classification evidence, not real remaining-time evidence. |
| Nits: caller test ignores suppressed `errexit`; startup wire comment is stale | Both corrected as described above. The separate runtime reconciliation refusal allowlist remains outside #142. |

Opus's initial statements that malformed replies and duplicate admission were
safe were source-inspection judgments, superseded by the concrete parser and
`.curlrc` reproductions above. Its Docker-proxy exit-code speculation remains
unverified and does not authorize resend after a reset.

A second tool-free Opus 5.5 pass checked the corrective helper/diff and the
explicit policy dispositions. It confirmed the parser, curl-config and strict
success-deadline fixes, and agreed with retaining conservative terminal failure.
It found two remaining issues: the zero-timeout race and a dropped explicit
return-status assertion in the conditional caller test. Both were corrected;
the latter now captures `rc` in an OR-list and asserts zero. The race regression
failed before its guard and passed afterward. A contradictory single reply with
both a hash and an error was also added to malformed-reply rejection coverage.
The final two small corrections were validated locally, not sent for a third
Opus pass. Neither Opus pass executed tests or established live behavior.

## Validation and limits

All commands ran from the dedicated worktree root. New HTTP fixtures use
OS-assigned loopback ports and task-owned temporary files, with no live network,
wallet or service operations. Test tool versions remain those in the initial
record; the comment-only Rust edit uses rustfmt 1.95.0.

| Check | Actual result |
| --- | --- |
| Final socket fixtures against original `fa56a022` helper | 19 selected tests, 12 failing subcases; the identical corrected fixtures pass. |
| Final socket fixtures against corrected helper | 19 tests pass as part of the final affected suite. |
| Affected suites: `test_front_submission`, `test_kurtosis_fee_quotes`, `test_wave_settlement`, `test_kurtosis_preparation`, `test_workflow_contract` | 83 passed in 24.758 seconds, exit 0, after all corrections; real helper/caller and CI discovery paths selected. |
| Full Python tooling discovery with Compose 5.5.1/Gitleaks 8.30.1 | First attempt aborted with failures and no final summary during host filesystem/mount quota pressure; exit 1. Diagnostic rerun: 567 selected, 560 passed, seven opt-in probes skipped; 383.300 seconds, exit 0. Original failure remains recorded. |
| Bash syntax, ShellCheck, Python compilation, Rust fmt, diff whitespace | Passed. |

First failures are retained in task-local `/tmp/issue142-review` (no retention
or cross-checkout availability guarantee). An isolated diagnostic rerun of the
first affected configuration test passed without a source change; this is
reported as environmental instability, not an erased failure. The first broad
attempt remains a failure (`flaky-result = "fail"`) even if its diagnostic rerun
passes. The new behavior regressions failed before their fixes and passed
afterward. No other task's storage, caches, containers or services were removed.

No Rust/native rebuild, live Kurtosis/Chiado, independent replay or performance
measurement was run. Runtime admission, retry accounting and protocol remain
unchanged. Matching successful status confirms admission only; callers still
check execution and settlement. Intentional poison workloads may conservatively
fail after reply loss when only a terminal failure is visible.

The broad diagnostic run loaded the 18-test fixture version before the final
zero-timeout regression and last assertions. The final 83-test run includes the
current 19 fixtures, positive-timeout guard and explicit caller-status check.
The seven broad exclusions remain the Docker payload, two Kurtosis resource
probes, native archive relocation and three runtime Docker builds.

Ponytail verdict: Ship the bounded harness changes, with the initial broad-suite
failure retained as environmental instability. No confirmed in-scope finding
remains after the documented corrections and final affected checks.
