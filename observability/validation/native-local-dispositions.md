# Local disposition capture validation

Source `c7c2782` closes the direct local-removal gap left by
[native reconciliation](native-reconciliation.md). The
[capture contract](../contracts/native-lifecycle.md) describes the exact scope.
Executable identities and process results are in the
[scenario report](native-local-dispositions.json).

| Requirement | Implementation | Behavioral evidence |
| --- | --- | --- |
| Capture pre-submission disposal before metadata expires | Bounded local candidate snapshot around existing pool mutations | Queued/in-flight root and suffix retain original admissions; other direction remains owned |
| No duplicate terminal from repeated calls | Compare actual active reservations before and after mutation | Duplicate release input, repeated release/eviction, and fresh readmission preserve distinct identities |
| Ordinary release cannot establish canonical completion | Exclude unresolved canonical owners; local outcomes carry local-policy assurance | Queued-only release is a no-op; awaiting-evidence owner remains unresolved until native reconciliation |
| Retain recovery semantics | Snapshot recovery candidates without changing recovery conflict rules | New queued replacement survives; actually removed suffix has an explicit local disposition |
| Cover unsubmitted victims of a submitted root's nonce gap | Separate local snapshot around the existing reconciliation reservation update | Root remains in its linked reconciliation; unsubmitted suffix gets exactly one local nonce-gap terminal |
| Bounded capture never becomes an eviction limit | Deduplicate candidates, cap snapshots at 4,096 and count whole-snapshot loss | 4,097 unique users still evict; one capture gap and no partial terminal list. Repeated copies of one hash do not exhaust the unique-member bound |
| Missing or closed telemetry does not retain discarded protocol work | Original pool mutation runs independently | Missing admission context counts a gap; closed writer emits no terminal while eviction completes |

232 Composer tests passed, including six new local-disposition regressions.
The existing readiness test now also checks the newly emitted terminal; its
three readiness assertions remain. Observer and all-feature telemetry suites
passed 27 and 22 tests. Workspace all-target Clippy passed with `-D warnings`,
as did formatting, diff whitespace, edited links and shell examples.
The first local test compile caught ownership of the shutdown-consuming writer
in a conditional fixture; an `Option` now permits exactly one shutdown. The first
failure remains in `/tmp/eez-observability-local-disposition-tests.log`; the final
component results are in `-tests-bounds.log`, `-consumers.log` and `-clippy.log`
with that same prefix.

Source-based process validation passed five tests, none ignored: three
bidirectional canaries (21 filtered) and both Ethereum/Gnosis queue-liveness
cases (none filtered). The canaries retain semantic, proof and lifecycle
receipt/endpoint checks. The existing queue-liveness suite verifies escrow
blocking, independent sender progress, later inbound funding and follower
convergence with telemetry disabled. It is protocol compatibility evidence;
forced local-disposition emission remains component-level evidence.
Both source runs cleared all prebuilt overrides and used the pinned contract
build, isolated testkit ports/processes, and retained local logs/datadirs under
`artifacts/native-observability/2026-09-17-local-disposition/`. Detailed logs have
no promised retention. Durations include harness source-build work and are not
performance measurements.

No queue owner, durable admission journal, protocol decision, configuration knob,
wire schema or dependency version was added by this local slice. Observer outcome
validation, source/companion allocation, native metrics/tracing and operational
acceptance remain open in the [requirement ledger](../requirements.md).
