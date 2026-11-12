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

The wave-caller regression invokes the function in a conditional list, where Bash
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

## CI check and final Fable 5.2 review

The next review checked PR head `ebd812440960364b4ce9bd50d578f4417fac6709`.
[CI run 43256905683](https://github.com/eez-association/eez-rollupX/actions/runs/43256905683)
failed its Lint job at the history Gitleaks scan: three findings, before script
tests, formatting or Clippy ran. Runtime artifact and debug-unit jobs were still
running initially; both completed successfully by the last pre-push snapshot,
while the E2E lanes remained queued/running. PR review, inline-comment and issue-
comment endpoints were all empty. No existing workflow was manually cancelled or
rerun.

The PR-only history scan was clean. The same `--all -m` scan over local refs
reproduced three findings, all in
[PR #165](https://github.com/eez-association/eez-rollupX/pull/165)'s
`d9b974c1c3d1081afbbfde05bfad4fc352aedea0`, at `workloads.json` lines 37, 159
and 311 under `docs/audits/2026-11-12-streaming-deadlines/`. That commit is not
an ancestor of #163. Each `vkey` exactly equals its existing native fixture's
public NetworkProfile verification commitment and an already-reviewed exact
Gitleaks value exception. These are public commitments, not signing credentials.

The CI fix adds only that anchored evidence path to the three existing exact-
digest AND rules. Value regexes and the all-ref scanning policy stay unchanged.
The Git-based regression matrix now covers all three values at the new path,
one-character value mutations, a different credential beside a permitted value,
and the same public value outside its permitted paths. Original config: three
new subcases fail. Corrected config: both scanner tests pass. The all-local-ref
scan changes from three findings to zero over the same 903 commits / 205.47 MB.
CI scanned 1,126 commits; local ref coverage is not asserted to be identical.

Fable 5.2 reviewed the complete harness diff and connected callers/status
producers using only Read, Grep and Glob. Claude Code 2.1.322 initialization and
assistant responses both identify `claude-fable-5-2`; no fallback or subagents
were used. Its source-only review confirmed the prior duplicate/response guards,
and found four remaining issues:

| Fable finding | Reproduction and correction |
| --- | --- |
| Real-clock regression depends on the first RPC's wall-clock phase | Delaying the first RPC through a real tick reproduces the original test failure. The fixture now sets two remaining real ticks from the existing deadline on the first retry, once only; its stalled send lasts longer than that budget. No busy-spin phase alignment is needed. |
| Late completed replies still use the refusal-exhaustion diagnostic | Expiry immediately after curl or during positive-hash parsing now preserves unresolved acceptance. Controlled clock hooks reproduce both cases; return status remains failure and no late success/retry is allowed. |
| `--max-time 1` assertion also matches `--max-time 10` | Added the following argument delimiter. A deliberate always-ten-second timeout mutation passes the old test and fails the corrected test. |
| Startup wire comment omits `scripts/devnet-test.sh` | Source search confirms its substring consumer; the comment now names it alongside the testkit and distinguishes the Kurtosis exact match. |

The combined behavior regressions select 20 tests: three subcases failed before
these fixes, then all 20 passed. Fable executed no tests and did not validate
live behavior or the frozen review-input provenance. A tool-free Fable 5.2 pass
also checked the precise corrections and scanner exception scope. It found no
remaining behavior bug, but identified two test gaps: the expiry hook matched
the post-parse comparison rather than the new post-curl comparison, and the
scanner matrix did not pin which rules admit the workload path. Both were
corrected. The hook now targets `SECONDS >= deadline`; the paired refused-
connection case asserts refusal exhaustion, not ambiguity. Removing either the
ambiguity promotion or the curl-7 exemption fails the test. The scanner test
asserts exactly the three intended rules match the workload path; adding that
path to a fourth rule fails immediately. These final test-only changes were
verified locally, not sent for another model pass.

Validation in this follow-up:

- First complete tooling run: 568 selected, 561 passed, seven opt-in probes
  skipped; 398.477s, exit 0, before the Fable corrections.
- Corrected implementation: 569 selected, **562 passed**, the same seven opt-in
  probes skipped; 403.397s, exit 0. Compose 5.5.1 and Gitleaks 8.30.1 were used.
- Final socket/scanner selection after the last assertion/hook changes:
  **22 passed**, 31.601s, exit 0. These test-only changes followed full-suite
  discovery, so this focused result establishes their execution.
- Bash syntax, ShellCheck, Python compilation, Rust 1.95.0 format checking,
  diff whitespace and edited local links pass. No Rust/native build or live
  exercise was run for the comment/config/script changes.

Logs remain task-local in `/tmp/issue142-final-review`; earlier failures and
mutation results are retained. No model ran tests. Local scan coverage does not
claim that the future GitHub run passed. Fresh GitHub CI is required for this
follow-up revision; no existing run was manually cancelled, modified or rerun.
