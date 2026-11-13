**Static verdict: no actionable finding remains in the test code. One small wording fix is left in the record, plus the final-run append you already plan.** I executed nothing and hashed nothing. This is not runtime approval, and I make no claim that the final test version passed.

What this change does: the paired-genesis test now waits until the anchor error appears on a line from each of the two owning emitters, and only then checks the negatives. The audit README and `evidence.json` record the superseded one-error step, the corrected height wording and the correction review. Production code is untouched.

## The final wait (`l1_identity.rs:528-562`)

- **Order:** the wait at `:528-552` comes before every negative at `:555-562` (no attestation, no relay dispatch, no eviction, no batch).
- **Each emitter maps to one production path:**
  - The ERROR `minimal batch unavailable…` is logged only at `composer.rs:6140-6141`. That call passes the just-built block (`:6122`), so its anchor text comes from the in-memory witness call at `:7589`.
  - The WARN `retained ordinary settlement deferred` is logged only at `sequencer.rs:1064`. It comes from `settle_retained` (`composer.rs:1952-2040`), then `emit_historical_chunk` with `None` (`:6357`), then the stored-witness call at `:7606`.
- **One slot runs both:** one worker runs `advance` and then `settle_retained` (`sequencer.rs:1052-1065`).
- **Anchor text has one source:** only `composer.rs:493` produces it. A missing height gives `L1 header outside supplied evidence` (`l1_history.rs:245-257`) or `proof target's canonical L1 parent is unavailable` (`composer.rs:7397`).
- **The per-line pairing is needed, and the retained logs show why:** both emitters logged the missing-height error in earlier slots before the anchor error.
  - `process-final` paired-node log: lines 48, 68, 88, 97, then the anchor lines 117 and 127.
  - `review-correction` paired-node log: lines 231-288, then 309 and 318.
  - `process-corrected` paired-node log: anchor lines 117 and 126.
  - An OR filter (`eez-testkit/src/lib.rs:1781-1784`) or a whole-file check would have passed too early. The final code cannot.
- **Fails closed:** if either path loses its fence, its line never appears and the wait times out. Poll errors are retried (`lib.rs:1998-2002`).
- **Compiles as read:** `line.contains(message)` with `&&str` is valid, and the lines fit rustfmt width. Both are by reading only.

## Corrections verified

| Correction | Status |
| --- | --- |
| Duplicated-log premise was wrong, dated in README | Present at `README.md:220-223`. See finding 1 for two leftover wordings. |
| Reports unaltered | `fable-review.md` reads the same as `final.md`; `fable-correction-review.md` reads the same as `correction.md` and the `result` field of `correction.json`. Compared by reading, not by hash. |
| Intermediate one-error run recorded separately | `evidence.json:205-237` has the command, commit `9f58ce89`, log and digest, exit 0, summary, four executable digests and `superseded_assertion`. Log line 173 matches the 1 passed / 220 filtered summary. |
| Final source digest separate | `post_review_test_correction` (`4f19…`), `intermediate_test_correction` (`a606…`, superseded) and the pre-review `code_source_sha256` (`59ee…`) are distinct entries. The 7-pass matrix stays tied to the pre-review source (`README.md:139`). |
| Height wording | `README.md:86-87` and `limits.anchor_fence` match `composer.rs:485-491`, `:7396` and `:7562-7563`: the anchor is the target's parent. |
| Fixture does not isolate genesis | Stated at `README.md:87-89` and in `limits`. |
| Other limits | Unchanged in the patch; only `anchor_fence` was edited. |
| Correction review metadata | `correction.json` shows only `claude-fable-5-2`, first-party, standard speed, no fallback credit, zero spawned agents. |

## Should fix (record only, non-blocking)

1. **Two places still misstate the first review** (`README.md:214-215`, `evidence.json:583`)
   - **What this is:** the README sentence lists what the first review asked for; the JSON entry describes the superseded one-error change.
   - **Problem:**
     - The README now says the accepted correction was to "name both owning anchor rejections explicitly". The first review asked for the opposite: it said one attempt logs twice and recommended `> 0` (`fable-review.md:11-15`).
     - The JSON `change` text still states "two log lines emitted for the same rejected attempt" as fact. The retained logs contradict that.
   - **Fix:**
     - README: say the first finding was applied as `> 0` at `9f58ce89`, and that its mechanism was later found mistaken (see the dated paragraph below it).
     - JSON: add a short "premise mistaken; see `post_review_test_correction`" note to that entry.
   - **If we skip it:** a reader who compares the README with the retained report sees a contradiction, and the superseded entry repeats the wrong mechanism.

## Still owed (already planned)

2. **Final-version execution record.**
   - When I read `artifacts/issue145/logs/both-anchor-paths.log`, it showed only compile lines and no `test result` line.
   - `clippy-post-review.exit` holds `0`, but `evidence.json` has no entry for it, and I cannot tell which source it covered.
   - When appending, record:
     - commit `20746f3c`, the command, exit code, summary line and log digest;
     - that run's own test and runtime executable digests, because the paths are reused;
     - a Clippy result tied to the final source;
     - one README sentence that the 7-pass matrix and the five regressions ran the pre-review file and only this test was rerun.

## Scope and limits

- **Source:** the branch ref resolves to `20746f3c2241062a6aacc4ae74a785ab660cef2e`, read from the ref file. Base `0d46d648` and Core `84a5b520` are taken from the record. I could not check that the tree is clean.
- **Read in full:** `both-paths.patch`, the current `l1_identity.rs`, the audit README, `evidence.json`, `final.md`, `correction.md`, both audit copies and `review-SKILL.md`.
- **Read in part:**
  - `correction.json` metadata; `final.json` was not re-read.
  - `composer.rs:440-509`, `:1940-2041`, `:6070-6415`, `:7370-7619`.
  - `sequencer.rs:985-1084`.
  - `eez-testkit/src/lib.rs:1692-1784` and `:1955-2011`.
  - `l1_history.rs:245-257`.
  - `VALIDATION_IDENTITY.md:96-119` and RPC `SPEC.md` §6.1.
- **Contracts:** nothing in the parts I re-read requires a genesis-specific anchor check. §8 and the bootstrap profile were not re-read; I rely on the two prior reports for them.
- **Not checked:**
  - Any SHA-256, rustfmt or Clippy output, and the final run.
  - Which fence term fired. The third term (`selected.number > anchor_number`) gives the same text without a hash difference. Before height alignment the logs show the embedded chain lagging the canonical target, so the anchor hash is the embedded chain's. If the proof history is canonical, a hash term must have fired, but I did not trace that wiring.
- **Reviewer:** this session's declared model is `claude-fable-5-2`, with no sub-agents and Read/Grep/Glob only. Take the actual model metadata from the CLI result, not from this line.
- The Rust-skills hook format was not applied: this is a review and no Skill tool exists here.

Verdict: fix 1 in the record and append 2. No change to the test is needed.
