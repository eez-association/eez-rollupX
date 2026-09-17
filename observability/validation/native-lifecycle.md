# Native admission and readiness validation

Source `e1a098c` implements the [native capture contract](../contracts/native-lifecycle.md).
The [audit](../../docs/audits/2026-09-17-network-observability/README.md) records
the first process failure and correction. Exact artifact hashes and selected
process results are in [native-lifecycle.json](native-lifecycle.json).

| Requirement | Implementation | Behavioral evidence |
| --- | --- | --- |
| Accepted ownership gets a distinct admission | `HeldPool::push_contiguous`, optional pool lifecycle context | Active duplicates and rejected nonce/fee replacements emit no admission; successful replacements link the old admission and its local terminal disposition |
| Recovery retains identity; explicit readmission is distinct | Capture metadata follows existing queue/ledger retention | Retry/requeue emits no admission; readmission after local termination links retained identity; a late old callback cannot erase the new owner |
| Readiness records actual evaluated hashes | Existing ready/blocked updates and legacy group-success path | Identical observations deduplicate; same-height hash changes emit; terminal metadata alone cannot emit readiness |
| Optional capture preserves compatibility | Node startup derives actual embedded chain identity; disabled path remains empty | Ended writer still permits ingress; legacy readiness capture preserves queue RPC fields; enabled/disabled source-built canaries both pass |
| No protocol success inferred from telemetry | Lifecycle events remain diagnostic claims | Both canaries separately require successful receipts, destination values 41/43, safe-state convergence and a recomputed proof signature |
| Exact native event scope | Existing publisher identity plus embedded chain specifications | Enabled canary compares admission chain/genesis/rollup/direction against actual RPC values and links readiness to exactly one admission per source |

The full Composer library suite passed: **210 tests**, none ignored or filtered.
Five new component regressions cover the cases above. The selected process suite
passed **2 tests**, with **21 other cross-chain tests filtered out**. This is not
coverage of the full E2E matrix, policy/failure scenarios, or an independent
follower replay. A process test does not establish the provisional <5% overhead
budget; no throughput comparison is claimed.
The node and native telemetry suites passed **33** and **21** tests respectively.
Workspace all-target Clippy, formatting, dependency deny/audit and document checks
passed; audit retains 14 existing allowed warnings.

Reproduce from the isolated worktree with Rust 1.94.0, Foundry 1.7.1 and the pinned
initialized protocol. The existing testkit allocates its fixture processes/ports:

```bash
unset EEZ_TEST_PREBUILT EEZ_TEST_RUNTIME_BIN_DIR EEZ_TEST_PROOF_SIGNER_BIN
forge build --root contracts
CARGO_BUILD_JOBS=4 \
EEZ_TEST_LOG_DIR="$PWD/artifacts/native-observability/admission/logs" \
EEZ_TEST_DATADIR_DIR="$PWD/artifacts/native-observability/admission/datadirs" \
cargo +1.94.0 test --locked --offline -p eez-node-e2e \
  --test cross_chain bidirectional_cross_chain_smoke -- --test-threads=1
```

`--offline` requires already cached locked dependencies. The harness builds the
signer from source and retains process logs/failing datadirs. The lifecycle test
retains its dedicated temporary spool when the event assertion times out.
Successful fixture state is disposable. No configured operator wallet, external
network, active deployment or main checkout is used by this test.

Attempt/preparation membership, dispatch variants, canonical outcome assurance,
other terminal transitions, metrics/traces and mixed fault workloads remain open
in the [requirement ledger](../requirements.md). The basic capture canary does not
close those gates.
