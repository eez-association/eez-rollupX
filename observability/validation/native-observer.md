# Live native → observer acceptance

The test-only `crates/eez-node-e2e/tests/common/observer.rs` helper launches the
actual observer CLI against still-running native L1/L2 RPC servers and the
original Composer spool. It does not generate replacement lifecycle events or
replay captured RPC responses. PostgreSQL uses the existing pinned image, 512 MiB,
one CPU, temporary storage, no network and a private Unix socket on the Linux
Docker host. Its container UID/GID matches the fixture owner, allowing complete
cleanup even after forced shutdown leaves socket files behind.

The observer performs real administrative migration, then uses the unprivileged
writer role. The test's linked observer library checks every migration checksum
against the database; matching only a schema number is insufficient. Archived CI
uses `EEZ_TEST_RUNTIME_BIN_DIR/eez-observer`, already bound to the verified runtime
manifest by `native-tests.py`, with no rebuilding fallback. Local tests require
an explicitly built current observer. Each run records its observer digest,
schema/checksum verification and fixture provenance beside retained process logs.

| Scenario | Required live evidence |
| --- | --- |
| Existing bidirectional lifecycle canaries, legacy and policy | Actual admitted/committed rows; L1 and L2 source receipts matched to live RPC hash/index/status and exact execution/blob amounts; each captured paid post's complete allocation population; integer conservation; observer restart retains the same admissions and receipt identities |
| Existing partial-consumption restart cases, legacy and policy | Three-source paid attempt retains committed prefix and source revert; omitted source remains unresolved before Composer crash; original admission becomes `unresolved_restart`; actual signed-transaction resubmission creates a separate boot-epoch admission; retry commits with independent receipt and one-member posting allocation |
| Existing controlled outbound queue reorg | Exact original post identified by native block correlation, indexed canonical and unfinalized before the fork; actual consensus controller replaces that occurrence; old receipt/allocation retracts; new canonical source/post receipts and outcomes agree with live RPC while Composer/follower converge |

The reorg controller remains restricted to its existing outbound, single-post
bundles. The queue's `ownership.canonical` is a reconciliation watermark, so the
test now selects the fork ancestor from the exact posting correlation instead.
There is no protocol, execution, recovery-policy or production database mutation
introduced by these test helpers.

All readiness/success waits check observable predicates. A failed SQL statement
or exited observer fails immediately. SQL assertions use the existing 10-second
statement deadline. Source receipts and posting allocations are checked
independently; an existing allocation for another paid attempt cannot satisfy a
specific post's expected population. PostgreSQL and observer processes are owned
and cleaned by the fixture, including assertion failures; logs are retained.

The native cross-chain CI entrypoint checks Docker and pulls only the pinned
PostgreSQL image before running the already archived targets. Existing test
selection, runtime artifact identity checks and failure-on-flaky-result policy
remain unchanged. This does not duplicate the full native build in the separate
observability workflow.

Initial three-target compilation passed in 3m32s before the final checksum and
paid-post assertion additions. The independent socket preflight proved actual
host observer migration. Its first forced-cleanup run exposed foreign-owned
sticky-directory socket files; matching the host fixture owner fixed cleanup,
and the exact earlier fixture residue was removed. Logs:
`/tmp/eez-native-observer-socket-check.log` and
`/tmp/eez-native-observer-socket-reviewed.log`.

## PR26 recovery checkpoint

The [PR26 integration record](pr26-integration.md) now verifies current mandatory
settlement-policy behavior with the schema22 observer. The default lifecycle
canary passed in 68.94 seconds, partial-consumption Composer restart passed in
133.29 seconds, and the actual outbound L1 reorg passed in 111.37 seconds.
The recovery harnesses were built from `2b4056b4`, with unchanged production
runtimes frozen at `5f3b36b3`. Both recovery cases used actual native RPCs and
lifecycle spools, independently checked all schema checksums, and cleaned their
owned PostgreSQL containers. The linked evidence manifest records artifact
identities and local-only report digests.

These results supersede the earlier baseline for those named cases. Historical
legacy/policy variants below retain their original scope; PR26 has mandatory
settlement policy. No performance comparison or deployment acceptance is implied.

## Historical live checkpoint

The legacy bidirectional live native→observer test now passed **1/1 in 177.27s**
after the [historical collection fix](historical-checkpoints.md). It verified the
actual two-source paid membership, exact source/post receipt fees, canonical
outcomes, integer allocation conservation and observer restart idempotence.
The observer log contained only initial bootstrap gaps before successful
collection and its clean restart; no later cursor inconsistency, deadlock or
query timeout appeared.

- Observer: `ece81ef88ea7e7798c9d379b98ead21ca8eab5980064d8c2f79e06b93c0b9f68`.
- Test executable: `4bb50187e35de9d1a7dd929a76d8a98607ef8be18c524aab407a0ccbfb808db3`.
- Invocation explicitly set Rust/Cargo 1.94.0 and cleared stale runtime overrides.
  The nested signer freshness build passed in 11.21s.
- Log: `/tmp/eez-native-observer-append-legacy.log`; per-process logs and observer
  provenance are in `/tmp/eez-native-observer-append-legacy-logs`.

All five live scenario variants now pass against observer `ece81ef8…`:

| Live variant | Result | Local log |
| --- | --- | --- |
| Legacy bidirectional + observer restart | 1/1, 177.27s | `/tmp/eez-native-observer-append-legacy.log` |
| Policy bidirectional + observer restart | 1/1, 116.44s | `/tmp/eez-native-observer-append-policy.log` |
| Legacy Composer restart after partial consumption | 1/1, 211.99s | `/tmp/eez-native-observer-partial-endpoint-legacy.log` |
| Policy Composer restart after partial consumption | 1/1, 134.58s | `/tmp/eez-native-observer-partial-endpoint-policy.log` |
| Controlled actual outbound L1 reorg | 1/1, 140.56s | `/tmp/eez-native-observer-reorg.log` |

An expanded legacy repeat also passed 1/1 in 128.99s, scraping actual observer
`evaluation_valid=1` and positive committed gauges in both directions before and
after observer restart while native blocks continued appending. Per-phase samples
are retained beside `/tmp/eez-native-observer-metrics-legacy.log`. Metric fencing
was not relaxed to obtain this result.

The final partial-consumption executable was
`cbf14949797e21db5056639e2ba36c3c84d80d06e5cec128150d98daee0cdfa6`;
its exact runtime digests are retained in
`/tmp/eez-native-observer-partial-endpoint-artifacts.sha256`. Native protocol
semantics, pinned contract artifacts and source-built signers were checked;
there was no synthetic RPC substitution.

Two fixture assumptions were corrected after retained failures. The one-shot
relay hold expires after 60s, so slow observer catch-up allowed a valid later retry
to commit before the intended crash. A five-minute control now holds only
bundles containing that exact signed source; each HTTP wait remains bounded 60s,
other bundles proceed, and the control is cleared after restart before actual
resubmission. The original expired-bundle and replay assertions remain. The
process fixture also assigns a fresh L2 RPC port on restart; the observer now
reloads only those URLs and restarts against its existing database and unchanged
chain identities. Before that correction it correctly reported transport/unknown.
The old admission is verified `unresolved_restart`, and the resubmitted same
source gets a separate new-epoch admission with actual canonical completion.

Earlier failures and immutable database/spool snapshots remain at the recorded
local paths, including `/tmp/eez-native-observer-failure-hpzh6X`, `dm4rgT` and
`lLlkU8` under the same prefix. Initial production failures are retained in the
historical-checkpoint and [cost allocation](cost-allocation.md#schema-20-live-query-regression)
records. The full isolated database suite separately passed 64 groups.
