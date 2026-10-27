# L1 history polling: issue #107

| Field | Value |
| --- | --- |
| Date/status | 2026-10-26 UTC; reference application validated locally, not deployed |
| Source | PR #103, `codex/fee-vault-upgradeable`; parent `6a776eaea80e790c7ab0a1c606dea5ee0c1e9bb1` plus this amendment; source/test hashes in [evidence.json](evidence.json) |
| PR dependency/base | #102, `codex/mixed-direction-fees`, `641e768f2e6605f7563c808a00937d374e98e8fb` |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged and clean |
| Environment | Foundry 1.7.1; Solidity 0.8.34, optimizer runs 1, via IR |
| Contract | [Application polling rules](../../../contracts/L1_HISTORY_SPEC.md#application-polling), [reference collector](../../../contracts/src/L1BlockHashCollector.sol), [regressions](../../../contracts/test/L1BlockHashCollector.t.sol) |

## Observed failure and interpretation

Read-only investigation of [issue #107](https://github.com/eez-association/eez-rollupX/issues/107)
used L2 chain 916969 at deployed source
`9e3599abf2f1f08c4bcfff41f496f3365b5c4561`. The deployed history runtime matched
the generated runtime in this checkout; the history source is identical between
those two revisions. Exact identities, historical calls and the two transaction
traces are retained in [evidence.json](evidence.json).

Two `poll()` transactions executed in L2 block 77899. The first consumed L1 block
23705164, which was the imported endpoint. The second requested 23705165 and
reverted with `L1BlockNotImported(23705165)` (`0x2cbf90b9`). The target remained
absent at L2 block 77902 and became available at 77903, four seconds after the
failure. The requested L1 header's timestamp was one second **after** the failed
L2 block's timestamp; its eventual import respected the three-second delay.
These observations agree with the strict getter's specified availability rules.

The external collector source was unavailable. The regression therefore models
its observed unguarded lookup rather than claiming to reproduce its full source.
At the same history state, the old model's second poll reverts and the reference
collector's second poll returns `(false, 0)` without advancing or emitting an event.

## Implementation and executed checks

The reference reads `current()` inside each polling transaction. It consumes at
most one covered hash; a cursor above the endpoint means no progress. A covered
but missing record, malformed ABI response or invalid history state still reverts.
Its constructor requires a start block already inside the imported range.

All commands ran from the isolated PR worktree
`artifacts/fee-vault-upgradeable-20261026` under the repository. Genesis validation
used this checkout's previously built `eez-genesis-state-root` helper, identified
in the [treasury audit](../2026-10-26-fee-vault-upgrades/README.md).

| Check | Result |
| --- | --- |
| `forge test --root contracts --match-contract L1BlockHashCollectorTest -vv` | 8 passed, 0 failed/skipped |
| `forge test --root contracts` | 157 passed across 17 suites, 0 failed/skipped |
| `forge fmt --check --root contracts` | Passed |
| `bash scripts/update-eezl2-genesis.sh --check` | Passed; all generated artifacts unchanged |

Regressions cover two polls in the same block, repeated waiting, empty imports,
resumption after a scheduled import, ordered catch-up, invalid start ranges,
invalid endpoint state, missing covered hashes, malformed ABI and exhaustion at
`uint64.max`. They assert exact hashes, cursor movement, retained state and events.
Semantic assertions ran in Foundry against the real history runtime; corrupt-state
and ABI tests deliberately inject invalid state/results. No new process test or
independent follower replay was run for this application-only amendment.

Full local logs are ignored under `artifacts/issue-107/` in that worktree and are
not available in a fresh checkout. The committed evidence records their digests
and concise results; it omits the bulk deployed bytecode. Historical RPC queries
require retained archive state and transaction tracing on the investigated network.
The checked-in Foundry regression does not require that network.

## Deployment scope and remaining work

No transaction was broadcast and no deployed service, application or genesis was
changed. The strict history getter, history runtime, protocol pin and `prove.v2`
remain unchanged. The reference is a separately deployable application, not a new
predeploy. External consumers must adopt the pattern or deploy the reference;
updating RollupX alone does not modify their existing code.

The earlier PR CI run failed its stale FeeVault genesis assertion and dependency
policy; the [treasury audit](../2026-10-26-fee-vault-upgrades/README.md#ci-follow-up)
records that separate follow-up. Final-head CI remains required before merging.
