# Canonical outcome anchors and selection

2026-09-17. Uncommitted work in `feat/network-observability`, based on `5183f56`.
The agreed main baseline remains `5469c88`; protocol gitlink remains
`3c0cdf4955941a5c23639346932b55b33babebbd`. No live deployment or operator data
was modified.

## Implemented behavior

Schema 11 independently checks the available canonical anchors of native-coherent
terminal claims, retains exact outcome taxonomy, selects one consistent outcome
per admission/context, and retracts assurance on reorgs, conflicts, incomplete
claims or stale sources. A historical same-height applied prefix is anchored to
its exact canonical L1 post/root occurrence without requiring it to own the current
L2 height. Source receipt facts and exact fees remain separate.

The observer publishes recomputed outcome/progress gauges with closed labels,
explicit gaps and current-cursor/freshness fences. Original admission view columns
and local-policy semantics are preserved; no admission is fabricated from later
membership. See [the contract](../contracts/canonical-outcomes.md).

## Evidence

- `tests/canonical-outcomes.sql` passed **40 anchor assertions, 7 admission
  assertions and 5 supplemental invariant blocks**, with the unchanged 10-second
  statement deadline. Log: `/tmp/eez-observability-canonical-outcomes-sql.log`.
- The complete disposable database suite then passed **48 check groups**, including
  that SQL fixture, real lifecycle import/projection, healthy projection-error
  checks, actual saved-binary upgrades, authenticated blob fixtures and a fresh
  authenticated reader connection showing `jit=off`.
  Report: `/tmp/eez-observability-blobs-database.json`; corresponding `.log` retained.
- That tested observer binary SHA-256 is
  `3a6c3d8f7ce5f37cdc8d1284e0a67a9789976c31fdfa20f417c2937551ef1531`.
  Its report's schema-11 hash matches the current migration:
  `341a9171e1bf78f8269e5a833922f3560cbb1ecf18c41c2476a737bfe839a052`.
- 55 observer tests and 24 blob-codec unit tests plus the Ethereum KZG oracle
  passed in the subsequent focused review. Observer additions include the pinned
  L2 snapshot boundary regression and canonical progress cursor/staleness fencing.
- Native-coherence inputs were independently exercised in the earlier source-built
  bidirectional lifecycle and same-block resumption tests, as recorded in
  [native outcome validation](native-outcomes.md). That earlier run does not
  validate the later metrics/tracing source changes.

The anchor fixture explicitly supplies synthetic coherence-cache/wire stubs to
isolate SQL selection. It is not a native importer test or independent execution
replay. Real transport import and checker behavior have their separate tests;
new native restart/reorg workload acceptance remains required.

## Findings and corrections

The initial tiny query took 13.371 seconds of execution, of which 13.251 seconds
was JIT compilation. Running the same query/input with `jit=off` took 159.6 ms
execution (planning measured separately at 455.8 ms and 912.6 ms respectively).
Both produced one row. Plans and buffers/settings are retained in
`/tmp/eez-observability-canonical-outcomes-explain.log`. Observer sessions and the
dedicated roles in this database now disable JIT; no deadline was increased.
This establishes a query-compilation correction, not production capacity.

The first intervening-post fixture omitted its selected-transaction row. The
fixture was corrected to match the ordinary scanner, and review then removed the
unnecessary selection assumption from posting-window checks: any independently
known canonical receipt and decoded posting table can close a window, including
a source dependency. Both selected and unselected boundary cases now pass.

Earlier logs retain the initial default-JIT run and tracked-only fixture checkpoint.
Local `/tmp` artifacts require separate archival to be available on another host.

## Coverage and remaining acceptance

The SQL matrix covers exact receipt/hash/index/calldata, all five canonical native
outcomes, root emitter/rollup and consumed call/index checks, relevant/unrelated/
unknown intervening posts, log/correlation conflicts with fees preserved, historical
P/current Q, outbound source receipt reverts, competing inclusion without original
application, duplicate claims, newly imported unchecked claims, positive orphan
selection, local policy compatibility and absent admissions.

Production data-volume/query scaling, integrated native restart/reorg/fault
canaries, finality latency reporting and the final combined build/Clippy/native
canary remain open. Schema 12's later retrieval-duration addition and schema 13's
metadata view have a separate final rerun; the 48-group report above describes its
recorded binary/schema snapshot, not those later edits.
