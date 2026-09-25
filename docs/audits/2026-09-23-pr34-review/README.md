# PR #34 integration review

Status: original review completed on 2026-09-23, with the scope correction below
requested by the user. The subsequent authorized integration is recorded at the
end; the original findings and evidence are retained. This is an engineering
review, not an independent security certification.

## Scope correction: our local additions, not the existing PR

The user's intended scope is the local delta from PR head `68fde5b` to the
candidate working tree, including its intended untracked files. The initial
review mixed this with existing PR issues. Its broader findings are preserved
below as context, but the 200-worker default, all-history SQL aggregation and
missing indexer CI coverage **predate our local changes**. They are not new
regressions or prerequisites imposed on this narrower task.

The assessment of our additions is:

| Local change | Assessment and next action |
| --- | --- |
| Mixed static/mutable authorization and `feature/events` port | Keep together with ABI, DA, deriver, validation identity, genesis and fixtures. Strong semantic/tamper/replay evidence; no demonstrated new signing bypass found. Preserve the narrower supported profile. |
| Timeout replacement and historical-prefix target preservation | Keep the single-active-branch design. Repeat controlled skip, late-original, failed-replacement preparation and queue/reorg checks at the final protocol pin; earlier retry-process evidence used the previous pin. One replacement per observed head is an intentional limit, not a guarantee of posting through every consecutive skip. |
| Preparation/proof timing and 750 ms budget | Keep. Times describe the client proving operation, not signer CPU time. Correct the stale 1,000 ms preset example. |
| Indexer block receipts, separate enrichment RPC, transaction-hash index and bounded latest-settlement query | Keep the focused changes. The remaining whole-history aggregation is inherited; the new timing fields do not establish a measured throughput improvement. |
| New outcome-ratio panel | Counts distinct slots per category; multiple attempts in one slot collapse, and categories can overlap. Clarify that meaning before treating it as a distribution of attempts. |
| New posting-log overlay | Local packaging gap: our deployment needs this overlay, but the supported startup script omits it. Integrate a supported invocation without duplicating the complete node command/startup script. |
| New workload scale and resend modes | Fix the profile-precedence validation bug below; retain the wallet lock. Resend skips the normal network identity preflight and needs an identity check suitable for an existing run. |

### New P2 finding: profile loading bypasses the workload-scale limit

The added `EEZ_STAGED_SCALE` check in
[`scripts/run-chiado-staged-performance.sh:50`](../../../scripts/run-chiado-staged-performance.sh)
runs before `source "$DEVNET_ENV"`. The profile can subsequently set zero,
negative, or greater-than-ten values. The Python preflight converts the final
value to an integer but does not reapply the 1–10 bound. This defeats the stated
workload cap and may produce an unintended workload or late failure.

Reproduction executed the unchanged argument/profile-loading prefix from the
candidate script in a temporary directory, with mocked Cast/Forge/bc and a
fixture profile setting `EEZ_STAGED_SCALE=11`. It exited zero and reported
`SCALE_AFTER_VALIDATION_AND_PROFILE=11`. No RPC or transaction was performed;
this is a configuration-boundary reproduction, not an execution of the full
performance runner. Validate the effective value after sourcing the profile.

The new `--resend` branch also executes upstream before reaching the normal
preflight. Upstream restores saved URLs and contract addresses, but those URLs
can now serve our replacement network. This is a static validation gap, not an
observed mis-send. Recheck chain/genesis/registry identity before resubmission;
do not require an empty queue, since recovering pending work is its purpose.

For simplification, prioritize sharing the empty-Sync rebuild operation between
timeout and canonical recovery while retaining their different evidence gates;
use one supported logging startup path. Do not expand this integration into a
general Composer refactor or require fixes to inherited monitoring architecture.
The required untracked signer fixtures and the final PR scope description still
need inclusion when packaging these local changes.

## Scope and identity

| Item | Revision / scope |
| --- | --- |
| PR | [rollupX #34](https://github.com/eduadiez/rollupX/pull/34), open, `feat/submission-outcomes-metrics` |
| PR base | `3f1c8cba78c604e80c4c2b67b3104e6ad16544b2` |
| PR head | `68fde5bb5b0f9743e55a5de98cdff682e9076a49` |
| Candidate checkout | `artifacts/fix-mixed-outbound-source`, branch `fix/mixed-static-outbound` |
| Candidate HEAD | `1832f970884ca9460646a4cde87cca70eaffb3e1`, plus existing working-tree changes |
| HEAD protocol gitlink | `3c0cdf4955941a5c23639346932b55b33babebbd` |
| Candidate protocol | `820e50f749f63165be6a5797d4c5fe52cbbbbe34`; initialized, no submodule edits |
| Tracked diff versus PR head | 73 files, +3,110 / -621 lines |
| Tracked diff versus PR base | 114 files, +17,619 / -1,338 lines; includes independent indexer lockfile |
| Additional candidate inputs | 30 untracked files, 940,499 bytes before adding this review; includes required signer fixtures and operational records |

The SHA-256 of `git diff --binary 68fde5b` was
`60e0dd86909bfccf23addadec8b7d9e12149169a10cd1aa2334a2d0e89e35afb`.
This identifies tracked changes only. An untracked-file digest inventory is
retained locally with the review artifacts.

The root checkout is a different branch at `c18fea0692b0bdb8659b7eee39c85b28db1ca05e`.
Its other dirty runbooks/proposals were inventoried, not included automatically.
The separately deployed dashboard also has substantial uncommitted work in a
different repository; pushing this PR will not publish that work. No branches,
index entries, existing source changes, services or databases were modified.

## Findings to address

### P1: the default performance command retains the known memory-exhaustion setting

[`scripts/run-chiado-staged-performance.sh:45`](../../../scripts/run-chiado-staged-performance.sh)
still defaults to `PREPARE_PARALLEL=200`; line 47 defaults to 28 send workers.
This is already present at the PR head and remains in the proposed integration.
The [stopped workload](../2026-09-23-chiado-staged-performance-750ms/README.md)
records 200 solc processes at approximately 36.98 GiB aggregate RSS, another
6.35 GiB in Forge processes, and the kernel killing Composer on the shared host.
The successful latest run used four preparation workers, two send workers and
1.5-second pacing, inside a separately resource-limited container. Those safe
operational settings are not the helper's defaults.

Recommendation: make the default match the validated light workload, retain
explicit stress overrides, and document the bounded runner invocation. Check the
effective settings after loading the profile. Do not reproduce the host OOM to
validate this finding. This concerns test-runner resource use, not proof validity.

### P1: the normal startup path drops posting-log collection

[`scripts/chiado-up.sh:7`](../../../scripts/chiado-up.sh) selects only
`docker-compose.chiado-node.yml`. The candidate's untracked
[`docker-compose.chiado-postings.yml`](../../../docker-compose.chiado-postings.yml)
provides the dedicated log mount and file logging flags. A normal Compose `up`
through the startup script can recreate the node without them; the collector
then receives no new posting events and Grafana's posting history stops updating.

Read-only Compose rendering confirmed:

| Configuration | Posting mount | Posting file filter |
| --- | --- | --- |
| Startup script's base file | absent | absent |
| Base plus posting overlay | present | present |

The [deployment record](../2026-09-23-events-network/README.md) acknowledges that
cutover used an ignored copy of the startup script with the overlay included.
Recommendation: provide one supported startup path that preserves the selected
monitoring configuration, and test its rendered node configuration. Avoid a
permanent second full copy of the node command or an artifact-only startup script.
No service was restarted to demonstrate this finding.

### P2: posting queries still aggregate the complete retained event history

[`monitoring/indexer/schema.sql:85`](../../../monitoring/indexer/schema.sql)
and its `posting_timing` view at line 122 group all posting events by attempt.
Grafana's target-time restriction applies to a timestamp calculated by that
aggregation. The transaction-hash index fixes the expensive receipt lookup, but
does not bound this aggregate's work to the displayed time range.

A read-only `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` of the actual five-minute
timing-panel query scanned all 783 retained events in both aggregates. The target
aggregate produced three selected attempts and rejected 195 after aggregation.
Execution took 6.862 ms on this small, fresh database. That is not a current slow
query incident or a controlled benchmark; it demonstrates the remaining scaling
dependency. No database copy or synthetic load was created.

Recommendation: select attempt IDs through an indexed target-time field first,
then aggregate all events for those attempts. Preserve events outside the visible
window that belong to a selected attempt, so late outcomes and missing-sequence
checks remain correct. Start with this query change; a maintained summary table
is an alternative only if measurement justifies its extra write/reorg logic.
Measure fixed five-minute and 24-hour panels against increasing retained history.

### P2: CI does not cover the new monitoring workspace

[`monitoring/indexer/Cargo.toml:9`](../../../monitoring/indexer/Cargo.toml)
declares its own workspace. The root workspace's tests, Clippy, formatting and
dependency audit therefore do not cover it. Inspection of `.github` and
`scripts/ci` found no indexer build/test invocation. Its PostgreSQL recovery test
is explicitly ignored unless run against a disposable database.

The local indexer validation is useful evidence, but a green PR pipeline would
not protect this new service against future compilation, SQL or recovery
regressions. Recommendation: add a separate locked indexer build/lint/test lane,
including explicit execution of the PostgreSQL recovery test with an isolated
database, and include it in the required gate. Audit its separate dependency
lockfile as well. This gap originates in the monitoring PR, not the protocol port.

## Metric semantics and remaining observability gaps

- The inclusion panel counts **distinct observed target timestamps**, not user
  transactions, every network slot, or preparation attempts. This is documented
  in its description. Multiple failures in one slot collapse to one, and a slot
  can count in both preparation failures and successful inclusion. A read-only
  SQL fixture with two failures followed by one inclusion at the same timestamp
  returned `1/1 (100.0%)` for both categories. This is consistent with the query,
  but should not be presented as mutually exclusive outcome percentages. Either
  label these as independent slot incidences or give preparation attempts their
  own numerator/denominator. Do not silently change the user's denominator.
- `preparation_failed` is emitted at the proving boundary in
  [`composer.rs:6029`](../../../crates/eez-composer/src/composer.rs).
  Earlier construction, witness, pricing and admission returns do not all emit
  it. Thus zero recorded preparation failures does not prove that every candidate
  prepared successfully. Normal capacity deferrals should remain distinct from
  genuine failures; clarify the panel or complete that instrumentation.
- Proof request/receipt points bracket the complete client operation, including
  RPC waits and retries. They are useful elapsed-time measurements, not signer
  CPU time. The current producer comment states this correctly.

## Runtime and soundness assessment

The reviewed mixed-call path is materially stronger than merely relaxing a
signer shape check. Composer retains native L2 static entries with execution
positions; the shared verifier independently walks host order, checks the
mutable cursor, rejects conflicting results at the same key, and binds rolling
hashes. Signer authorization requires complete loads for groups with static
roots. Native DA reconstruction and the deriver retain and revalidate those
positions. Exact decoding/encoding, public-input recomputation and the narrowed
profile remain in place.

The requested two-reads/write/two-reads case is supported and tested. This does
not claim arbitrary core-protocol support: mixed groups with nested mutable
roots, reverted mutable roots, root revert spans or separate callbacks remain
outside the signer profile. Local writes alone do not advance the cross-chain
cursor. Upstream `STATIC_ENTRY.md` still contains hash-only prose that disagrees
with the pinned position-aware interface/implementation; the fork's accepted
proposal records this disagreement.

For timeout replacement, the implementation retains one active speculative
branch and historical attempt identity. It checks the unchanged L1 parent and
completed derivation, rebuilds the removed suffix, preserves users/reservations,
and reuses the poster nonce with a fresh target-bound proof. Old observers cannot
release a newer owner's users. Historical-prefix splitting retains the selected
target. The reviewed code did not reveal a demonstrated new signing bypass or
transaction-loss defect. That conclusion is bounded by the tests below.

The retry rule deliberately permits only one extra opportunity per observed L1
head. Two consecutive missing slots can still cause a posting gap. The fix
cannot make a proposer produce a block or select this builder's bid. Keep this
limit explicit instead of claiming that all skipped-slot gaps are eliminated.

## Evidence and coverage

| Evidence | Result / limit |
| --- | --- |
| Tooling checks repeated during this review | 246 selected; 242 passed, four explicit opt-in infrastructure skips; 85.121 seconds |
| Static review checks | `git diff --check` passed; read-only Compose rendering and PostgreSQL query checks described above |
| [Protocol-port validation](../2026-09-23-events-protocol/README.md) | Previously recorded 1,021 affected Rust tests, 463 selected core tests, 40 integration tests, all 32 Anvil scenarios, 34 native cross-chain/chained/partial-consumption process tests; includes independent replay and tamper rejection |
| [Timeout replacement](../2026-09-23-timeout-replacement/README.md) and [historical target](../2026-09-23-historical-target/README.md) | Controlled skipped-slot, late-original and prefix regressions passed; queue/reorg and surrounding policy coverage recorded at the previous protocol pin |
| [Current live protocol suite](../2026-09-23-events-staged-all/README.md) | 32/32 scenarios passed with semantics enabled, including all four new read/write cases; earlier external-RPC rate-limit failure retained |
| [Current live performance run](../2026-09-23-events-performance/README.md) | 770 jobs, 2,850 setup and 870 source receipts, no resubmission or node/signer restart, queue drained; `--no-verify`, not semantic coverage |

The full timeout/late-original/queue-reorg combination has not been re-executed
at the final protocol pin in this review. Run it on the integrated revision,
including replacement preparation failure and partial consumption. The current
port's partial-consumption suite and live semantic scenarios do not by themselves
exercise every replacement/reorg interleaving. Full native CI/Kurtosis validation
and source-to-image promotion for the eventual committed revision remain pending.
The running local images are not evidence that those CI gates passed.

Core tests retain their documented exclusions: `script/*`,
`test_Doc_OriginalValue_SeedInSetup`, and `test_PostCost_Incremental`.
Four review tooling skips were Docker payload validation, real Kurtosis resource
checks, real native archive relocation, and Docker runtime build. No additional
Rust/process suite, restore, deployment or live transaction workload ran during
this review.

Latest workload timings identify where time went: planning 390.136 seconds,
setup mining 137.129 seconds, paced source sending 664.928 seconds, total
1,253.721 seconds. The send duration is intentionally dominated by pacing.
Source-window inclusion was 114/136 at target (83.82%), 12 skipped targets and
10 produced blocks without inclusion. These are posting-attempt statistics.
Different protocol, chain state, pacing and proposer conditions prevent a causal
performance comparison with earlier runs.

Local-only review artifacts live at `artifacts/pr34-review-20260923/` inside the
candidate checkout; they are ignored, unavailable in another clone, and have no
retention guarantee:

| Artifact | SHA-256 |
| --- | --- |
| `tooling-tests.log` | `a4879cfe5d4058a892a096ca29cf931b1055b25bfc9d62c5db546481f76e0ac4` |
| `timing-explain.json` | `5bd89b515ae2302a24789ebd6c484f4ddab646e86346aa79d6b7cdb36c49a18a` |
| `ratio-example.log` | `329ba04fcec2b91c254605133f2726d5a3d70a0ad5c862f03297a2d2d0c3e0cb` |

## Suggested integration and simplification

Keep PR #34 if desired, but organize its added work into reviewable commits:

1. Monitoring correctness/performance and reproducible logging startup, with
   the separate indexer CI lane.
2. Timeout replacement and historical-prefix correction, including their
   process regressions, accepted contract and 750 ms proof-budget propagation.
3. Mixed-call authorization and the protocol pin/ABI/DA/genesis/identity update,
   including the complete new portable signer fixture and all consumers.
4. Safe performance-runner defaults and consolidated operational documentation.

Dependencies must stay together where a historical intermediate commit would
otherwise fail compilation. No need to introduce another retry subsystem:
retain the current deadline rule, one active branch and existing reconciliation.
The duplicate empty-Sync rebuilding code in timeout replacement and canonical
recovery could share a small helper, provided their different authorization gates
remain explicit. Avoid a large Composer refactor in this integration.

For query performance, retain the transaction-hash index and block-receipt
fetching; address history selection before introducing caches or summary tables.
For workload planning, measure bounded concurrency and whether repeated Forge
planning recompiles shared dependencies before increasing worker counts. There
is no matched benchmark justifying a claimed runtime throughput improvement.

The current PR title/body still describe an observational monitoring change and
explicitly disclaim retry-policy and protocol-pin changes. Rewrite them around
the final scope. This protocol update changes selectors, event topics, static DA,
proxy addresses and genesis; it requires a fresh compatible network, not an
in-place upgrade of the old database. Point to the executed new-network and
recovery records, and update current-status proposals without erasing historical
failures. The packaged-timing example still calls a 1,000 ms proof budget the
Chiado preset; reconcile it with the actual 750 ms setting.

Do not use `git commit -a` as the integration procedure: it would omit the new
fixture files referenced by `include_bytes!`, the proposals and the logging
overlay. Select the intended files explicitly, exclude private operator state
and bulk artifacts, verify the staged protocol gitlink, then run the final gate
on that exact revision. No PR update, merge or publication was performed here.


## Authorized integration follow-up

The operator subsequently authorized the scoped fixes and an update to PR #34.
The original findings above describe the pre-fix snapshot; inherited PR issues
were not expanded into this integration.

- Validate the workload scale after profile loading, using a bounded decimal
  match that also rejects oversized shell integers. The original profile value
  `11` passed the earlier guard; the real-script regression now rejects it before
  key derivation or RPC. Cover absent/empty/invalid values and conflicting profile
  versus environment settings.
- Remove the newly introduced `--resend` shortcut. Saved endpoint URLs cannot
  establish the original network identity after cutover. The existing helper
  remains a fresh-run tool with its wallet lock; recovery is a separate operation.
- Move the bounded posting-log mount and file options into the main Chiado
  Compose service. Preserve `POSTING_LOGS_DIR` and the old overlay as a compatible
  volume-only configuration; the node command now has one definition. Static
  rendering proves that normal startup and the legacy overlay produce the same
  service with an explicit existing log directory. Invalid mount syntax fails.
- Label the new Grafana panel **Posting outcomes by target slot**, explaining
  overlapping categories, repeated failures within a slot and the proving-stage
  scope of preparation failures. Denominators and SQL outcome classification
  remain unchanged.
- Correct the packaged timing example to 750 ms / 3.05 seconds and update current
  deployment status and operator instructions. Preserve historical audit results.

Runtime algorithms were retained; the final-pin process regression work checks
that the earlier protocol and replacement changes operate together. The existing
failed-replacement-preparation ledger regression remains unit-level evidence;
no new combined process scenario for that exact failure was added. Full native
CI/Kurtosis and clean-host deployment remain separate gates. No live deployment,
restart, wallet funding, database copy or new performance workload was performed
as part of this integration.

Final checks and published commit identities are recorded in the PR update and
its integration validation manifest.

Final integration checks passed without diagnostic retries:

- **13 native process tests** at protocol `820e50f`: four targeted settlement
  scenarios, one real L1 reorg, three queue-liveness cases and five
  partial-consumption cases. Semantic and independent-follower assertions ran.
- **420 unit tests**: Composer 222, driver 57, L1 141. One existing manual
  profiling utility was ignored. Failed replacement preparation and late-original
  completion passed their ledger regression.
- **247 Python tooling tests passed**, with four explicit infrastructure opt-ins
  skipped (251 selected). Final Chiado configuration coverage includes invalid
  mount syntax as well as defaults, empty values and explicit overrides.
- Workspace formatting/all-target Clippy, shell syntax, settlement configuration
  checks, portable fixture hashes and edited documentation links passed.
- Generated genesis matches the pinned contracts, state root
  `0x8b19c93d207113a489efb0eb449b14aee1d30ad94cdbe125cf1e0127bec12bcf`.

The runtime source is committed as `c6cd4f5a88619a069d6afcd54db38af632e015fc`.
[Integration evidence](integration.json) records exact test executables, selected
cases, commands, tool identity and local-only log hashes. The final PR update
also includes the monitoring/configuration changes and these records; publication
is not a merge, deployment or claim that the full CI matrix has passed.
