# Bounded front submission retries — issue #142

The task owner explicitly authorized implementing the safe harness fix in
[#142](https://github.com/eez-association/eez-rollupX/issues/142), superseding its
older requirement for a separate implementation issue. This completes handoff
N8's focused transport investigation and implements its supported subset.
[#120](https://github.com/eez-association/eez-rollupX/issues/120)'s broader runtime
admission policy remains separate.

## Identities and preserved evidence

| Input | Exact identity |
| --- | --- |
| Frozen source | `eez-association/eez-rollup0` `1b66d9bda675e6f8e83f514153a97136909acc06`; protocol `143d87e62cade98bfc122d612638dcaaf1c85633` |
| Source context | [rollup0 #239](https://github.com/eez-association/eez-rollup0/pull/239), base `61a3cb95285a248e3689882dc43776016387adb1`, head `ed8c45ddf32cb2f03e2ff6338d7331b2ca265c85` |
| Frozen review target | `eez-association/eez-rollupX` `31e148f50dd79730b17d113ac78795eb377eb5c5`; protocol `84a5b5207880f192a6a5896cbfe0bac5249db47f` |
| Previous current-code check | `716a8a3bad4415ade7e9a6bc3af4a1b35419b232`; same protocol |
| Implementation / PR base | Association main `601ebfa9c7e1a779f1afbd077bd31a3207e7a61b`; same protocol, initialized recursively and clean |

The original review's input provenance was **not independently checked**. These
frozen identities and N8 come from the existing issue; this is not a new audit of
the source range. Source #239's retry of resets and empty replies is context,
not accepted target policy. Its node-startup movement is omitted: the target
already binds both fronts before `wait_for_l1_ready` in
[node wiring](../../../crates/eez-node/src/lib.rs). No peer-sync gate is added.

## Supported policy

The shared [helper](../../../testing/kurtosis/scripts/lib.sh) keeps one 120-second
budget, including every send, status lookup and one-second retry interval. Each
curl is capped at the lesser of ten seconds and the remaining budget.

| Observation | Action |
| --- | --- |
| curl 7: connection could not be established | Retry the identical signed bytes/hash; no request reached admission on that connection. |
| Exact startup refusal, `quote_busy`, `stale_context`, or the three existing recognized reconciliation/predecessor `quote_unavailable` responses | Retry the identical bytes; these responses refuse admission. |
| Timeout, partial transfer, send/receive failure, empty reply or successful HTTP with empty body | Acceptance is ambiguous. Switch permanently to read-only status polling on the same front. |
| Matching hash in active ownership (`queued`, `in_flight`, `composing`, `awaiting_evidence`) or terminal `Committed` | Confirm admission only; the caller still checks execution and settlement. |
| Matching terminal failure | Fail without another admission. |
| Missing, malformed, unavailable, unknown or mismatched status | Keep polling within the original deadline, then fail with unresolved acceptance. |
| Tariff/profile rejection, unrecognized RPC refusal, changed response hash or other curl failure | Fail; no implicit success or new signature. |

A null observation cannot exclude an in-progress admission or state lost on
restart. Resending after terminal disposition could create a new admission and
reset its policy accounting. Therefore even an apparent duplicate error does
not authorize success or resubmission after ambiguity. A normal identical-hash
acknowledgement remains successful. No raw transaction is regenerated or requoted.

The status schema and semantics were traced through
[front RPC](../../../crates/eez-node/src/queue_rpc.rs),
[ingress](../../../crates/eez-node/src/ingress.rs), and
[held-pool observations](../../../crates/eez-composer/src/held_pool/status.rs),
against [queue ownership](../../proposals/cross-chain-queue-liveness.md) and the
[settlement RPC contract](../../../crates/eez-control-rpc/SPEC.md#8-settlement-ownership-after-submission).
These are read-only observations; the harness does not change runtime ownership,
retry counters, or canonical completion. Ordinary forwarded transactions without
held-pool status, pruned history and restart loss can remain unresolved. No wire,
storage, protocol, dependency or runtime upgrade is needed; older harnesses retain
their immediate transport-failure behavior.

## Validation

Environment: Linux x86_64, Python 3.14, Bash, curl/jq, Foundry/cast 1.7.1
(commit `4072e48705af9d93e3c0f6e29e93b5e9a40caed8`), Compose 5.5.1
with CI's verified SHA-256, ShellCheck 0.10.0. Fixtures use only loopback and
OS-assigned ports. Tests run serially; no live network, wallet, service or
container lifecycle operations were used. Initial free checkout storage was
1.7 GiB, so no Rust/native rebuild was attempted.

| Check / selection | Result |
| --- | --- |
| Existing baseline `python3 -m unittest discover -s scripts/ci/tests -p test_kurtosis_fee_quotes.py -v` | 13 passed after initializing the protocol. |
| Final `test_front_submission.py` fixtures against the exact baseline helper exported from `601ebfa9` | 12 selected tests; 19 expected failing subcases. Refusal failed immediately, lost replies never reached status polling, and unrelated startup text retried. |
| Same final fixtures against candidate | 12 passed. Real HTTP/TCP refusal, reset, no reply, empty body, partial body, acceptance before loss, delayed ownership, timed-out send/lookup, temporary RPC refusal, shared deadline, duplicate acknowledgement/error and terminal/invalid status. |
| Candidate `test_kurtosis_fee_quotes.py` | 13 passed, including all existing temporary admission classifications and permanent refusal/deadline checks; real extracted outbound wave caller stops on helper failure. |
| `DOCKER_CONFIG=/tmp/issue142-tools python3 -m unittest discover -s scripts/ci/tests -v` | 561 selected: 552 passed, seven opt-in probes and two Gitleaks checks skipped; 363.651 seconds, exit 0. Compose 5.5.1; Docker build opt-ins disabled. |
| `python3 -m unittest discover -s scripts/ci/tests -p test_secret_scan.py -v`, with CI's verified Gitleaks 8.30.1 on `PATH` | Both previously skipped checks passed; only the seven opt-in probes remain unexecuted. |
| `bash -n testing/kurtosis/scripts/lib.sh`; `python3 -m py_compile` on both affected Python files; `git diff --check` | Passed. |
| `shellcheck -S warning testing/kurtosis/scripts/lib.sh` | Passed with no diagnostics. |
| `python3 scripts/check-settlement-policy-config.py` | Chiado, Kurtosis and Ethereum passed. |

First environment failures were retained: the uninitialized protocol caused six
existing quote-test failures, and the default sandbox denied local sockets and
Git metadata writes. Initialization and socket tests succeeded with sandbox
escalation; no test assertion failure was retried away. Final baseline and
candidate comparison uses exactly the same fixture file. Deadline tests advance
Bash's retry clock deterministically; stalled HTTP requests also exercise real
curl timeouts. They do not wait 120 wall-clock seconds. Fixture duplicate
accounting models front idempotence; no new runtime deduplication is claimed.
Raw first-failure and result logs remain task-local in `/tmp/issue142-evidence`
(not portable, with no retention guarantee); the table is the committed summary.
Full discovery ran after the final helper change; subsequent fixture-only
handshake/partial-body assertions passed in the final focused run.
The seven opt-in exclusions are one real Docker payload, two Kurtosis relay
resource probes, one native archive relocation and three runtime Docker builds.

CI selection was checked through `.github/workflows/ci.yml`'s unfiltered
`unittest discover -s scripts/ci/tests -v` command and its prior Foundry install.
The full local discovery includes this new module. Runtime caller selection is
`testing/kurtosis/run-ci.sh` → `verify-cross-chain-waves.sh` → each wave mode and
state-chaining → `send_front`. The separate combined-history caller also uses the
shared helper. Callers retain their receipt/state/settlement checks and fail on a
nonzero helper return. No workflow edit is needed.

Not run: Rust fmt/clippy/package tests, Solidity/native process suites, live
Kurtosis/Chiado, independent execution replay or performance benchmarks. No
Rust, Solidity, protocol or runtime behavior changed. These transport fixtures
establish harness behavior, not chain semantics, durable queue recovery or a
live upgrade. Existing main CI run `43237662996` was not modified or rerun.

## Ponytail review

What this change does: retries definite pre-admission failures under the existing
budget and checks unchanged source ownership after ambiguous sends. It preserves
caller failure reporting and separate semantic assertions.

The task agent performed its own review using
[Ponytail](https://github.com/dietrichgebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail/SKILL.md)
and [Ponytail review](https://github.com/dietrichgebert/ponytail/blob/96608e078b2d5ea607c5c3b8b5a54d1c1fba6846/skills/ponytail-review/SKILL.md),
current at review time. No additional agent or independent reviewer was used.
Review traced every helper caller, status producer, refusal allowlist and CI
selection. Expected load is the existing serial verification workloads; no
parallel submissions, new service, abstraction or dependency is introduced.

Review corrected a fixed-delay assumption in the refusal fixture with an
explicit listener-ready handshake. The implementation also tightens startup
recognition to the exact defined refusal, verified with a negative regression.
No remaining in-scope correctness, ownership, load or unnecessary-code finding.
Verdict: Ship, subject to the validation limits above.
