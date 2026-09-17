# Admission-history and local-policy validation

Source `cd58116` implements the [schema-8 status contract](../contracts/admission-status.md).
The [artifact report](admission-status.json) identifies the exact source, build
base, tested observer digest, unchanged main/protocol baselines and migration
hashes. Source was tested in the worktree before committing; runtime source was
not modified afterward.

| R19 requirement / invariant | Implementation | Verification |
| --- | --- | --- |
| Only observed admissions form admission history | Views keyed by configured source, epoch and logical sequence | Missing/conflicted admission is not reconstructed from a terminal or later readmission |
| Restart does not imply a terminal source outcome | Existing positive ended-epoch evidence produces unresolved_restart | Live, stale and unknown liveness remain unresolved; positively ended history changes status; late policy evidence can resolve it |
| Explicit local dispositions retain their limited assurance | Closed local outcome set and exact admission/source/epoch checks | All four policy outcomes; mismatched source, foreign instance/epoch, invalid owner epoch, unexpected anchor and missing fields remain unknown |
| Parent and child must agree | Exactly one matching reconciliation resolution; earlier same-process parent | Missing/conflicted parent, duplicate partition and mismatched duration stay unresolved; complete matching parent is accepted |
| Conflicts cannot be resolved by arbitrary ordering | Matching owner, parent link, outcome and duration across all terminal claims | Consistent duplicate observations yield one admission; conflicting owner/outcome/duration suppress assurance; UTC reversal does not choose a winner |
| Readmission has a separate identity | Explicit previous/replaced links retained as observations | Same source hash in two epochs remains two admissions, even with reversed UTC timestamps |
| Transport completion is not canonical success | Canonical outcomes retain unknown evidence class | A node-reconciled committed claim is not promoted by these views |
| Compatibility and access | Append-only migration; read-only views over existing event history | Reader access and the complete existing migration/receipt/lifecycle regression matrix |

The final source passed **37 observer tests**, **41 disposable database check
groups**, workspace all-target Clippy with `-D warnings`, formatting, Python syntax,
diff whitespace and relative documentation-link checks. The status SQL fixture
runs in a rolled-back transaction and verifies the query contract; it does not
stand in for a native restart canary. Existing process/liveness/importer evidence
remains tied to its earlier checkpoints.

No test or lint failure occurred in this slice. Final review tightened SQL NULL
handling and required matching ownership/parent/duration across duplicate outcomes;
the reviewed matrix reran after those changes. Earlier passing matrices are not
substitutes for the final artifact report.

Reproduce with Rust 1.94.0, local socket access and Docker:

```bash
cargo +1.94.0 test --locked -p eez-observer --lib
cargo +1.94.0 clippy --locked --workspace --all-targets -- -D warnings
cargo +1.94.0 fmt --all --check
cargo +1.94.0 build --locked -p eez-observer
python3 observability/tests/smoke-database.py \
  --binary target/debug/eez-observer \
  --report /tmp/admission-status-database.json
```

The saved-v4/v5/v6 options account for the extra old-executable checks listed in
the report; without them those checks are not counted. No saved-v7 executable
upgrade was exercised in this slice. The migration itself adds only views and an
index and preserves existing posting verification tables, events and charges.
Logs are local-only under `/tmp/eez-observability-admission-status-*`, particularly
`-database-reviewed.{json,log}`, `-reviewed-tests.log`, `-reviewed-clippy.log` and
`-reviewed-build.log`. Docker fixtures use unique isolated resources and check cleanup.

The interpretation follows native process-scoped tickets: a terminal must follow
its admission in that epoch, while explicit readmission links can name another
actual admission. No UTC comparison creates cross-process causality. Local-policy
assurance reports an observed local action with consistent ownership references;
it does not require chain availability or independently validate a canonical
source scope. Receipt truth and canonical completion remain separate.

Full canonical outcome validation, reorg-aware selection, broader source-reference
recovery and native restart canaries are still open in the
[requirement ledger](../requirements.md). No native runtime, chain state, dependency
pin, signed format or production deployment changed in this slice.
