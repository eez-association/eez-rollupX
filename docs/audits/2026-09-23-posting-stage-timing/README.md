# Posting preparation and proof timestamps

Status: implemented, tested and deployed to the existing Chiado node on 2026-09-23 at 13:26 UTC; live observation below.

- Node source baseline: `1832f970884ca9460646a4cde87cca70eaffb3e1`, with the
  already-deployed timeout-replacement patch retained.
- Protocol gitlink: `3c0cdf4955941a5c23639346932b55b33babebbd`, unchanged.
- Monitoring configuration baseline: `d4d9386eccc01cafec414125f52c9698e2b7d7e9`,
  preserving existing dashboard/query changes in the active monitoring checkout.
- User requested preparation/proof points on the same target-relative axis.
- Contract: [posting lifecycle](../../proposals/posting-lifecycle-history.md).

## Change

Capture preparation start before candidate construction, proof request at the
permitted proving-operation boundary, and successful proof return. Carry these
points with the exact prepared bytes through rich, minimal and historical posting
paths. Failed preparation retains known points; skipped proof calls and unsuccessful
proofs do not fabricate request/receipt points. The existing JSON event details
and collector preserve these additive fields without schema or wire changes.

The active Grafana panel queries each attempt's events in one indexed lateral
lookup and subtracts its expected L1 timestamp. All timing points share the existing
seconds axis. Outcome markers retain their own existing hidden categorical scale;
no vertical annotations were restored. Historical events stay null.

## Checks

- Composer unit suite: 222 passed, one existing ignored manual profile.
- Workspace Clippy with `-D warnings` and format check passed after renaming internal
  timing fields to satisfy `struct_field_names`. Initial Clippy failure retained.
- Initial tools command failed before compilation because `bash -lc` reset Cargo's
  PATH; subsequent commands use `bash -c` in the same Rust 1.94.0 tools image.
- Live PostgreSQL query fixtures cover success, failed proof, pre-proof cutoff and
  old events. All expected timestamp offsets/nulls matched.
- Single 24-hour query EXPLAIN samples: original 509.6 ms; new 433.0 ms. This is a
  bounded query sanity check, not a controlled performance improvement claim.
- Grafana API confirms the three point names, left-axis timing default and no
  enabled annotations after file provisioning.

Local diagnostics: ignored `artifacts/proving-timing/` in the source checkout;
SQL plans are initially `/tmp/proving-query-{before,after}.log`. These are local-only.

## Process and configuration validation

The native Gnosis settlement-policy process test passed: one selected, one passed
(`native_gnosis_always_settles_with_native_blob_fees_and_independent_replay`). It
asserted semantic settlement and independent follower replay. Dedicated posting
tracing was enabled for this test. Five prepared attempts had nonzero, ordered
preparation/request/receipt timestamps before their prepared event. One timed-out
proof retained preparation/request points and no successful-receipt point.
The ordinary fixture had no changed traffic or protocol semantics.

The user subsequently requested reducing the prover timeout from 1,000 to 750 ms.
The Chiado example/default and operator override now use 750 ms; other timing
reservations remain 500 ms preparation and 700 ms submission slack. Runtime
`RollupTiming::from_env` feeds the Composer's existing per-proof timeout and
scheduler; no signer worker-limit setting changed. All ten Chiado configuration
tests passed, covering profile/default/override/empty/invalid values. Mandatory
Chiado/Kurtosis policy configuration checks passed. There is no performance claim
for the reduced budget; the process fixture used its existing timing profile.

## Deployment

Only the current node image and `EEZ_PROOF_TIME_MS` changed. A rendered Compose
comparison and post-start container comparison verified all other node environment,
mounts and ports; signer identity/start time were unchanged. The queue was empty
before stopping. The node exited cleanly and its two writable databases were copied
with writers stopped. Private configuration, container inventory, consistent copies
and previous image were retained; restore was not rehearsed.

The locally built image embeds source baseline/protocol, implementation patch,
release profile and executable digest. It is a local development rollout, not
published CI promotion. Dashboard provisioning updated automatically without
restarting monitoring. No contracts, genesis, registry or database schemas changed.

| Identity | Value |
| --- | --- |
| image | `eez-node:posting-timing-f873c0e1e62d` |
| image_id | `sha256:ecd62f1c96ca8970055132815b7db8f419c7f0c70a5e950795cefc152c269edd` |
| started_at | `2026-09-23T13:26:49.430650111Z` |
| composer_sha256 | `fcdaeee70b806efdb273661df362cf069c3259f43382002ce563c87a85b48029` |
| proof_budget_ms | `750` |
| signer_unchanged | `True` |

The running `/proc/1/exe` digest matches the built Composer. Startup RPC reported
not syncing, an empty cross-chain queue and no errors in the sampled recent logs.
Full workspace/runtime CI, Kurtosis and a new high-volume performance comparison
were not run. The targeted process test, unit suite, workspace Clippy, configuration
checks and direct SQL/Grafana checks are the validation for this increment.

## Live result

Grafana's authenticated datasource-query API returned non-null points for all
three new series on the shared seconds axis. A recorded attempt's values were
preparation −1.948 s, proof request −1.788 s, proof received −1.770 s relative to
the expected block. Two live attempts had all three points at verification time.
This verifies the deployed producer → retained logs → indexer → Grafana query path.

A zero-value self-transfer from the existing public development fixture account
was included successfully on L2 and settled on L1. The L1 receipt returned status
1 and its block hash matched the current canonical header. Smoke transaction:
`0x0fb4688570646eaa62a944dfb73c55fb7597763a479cde13b4840fee28646ebd`; settlement:
`0xde28934e6a694a3ba4324e84bf92951b1c9954cf1c32a1ae8c37b3e2515af09f`.
This is a small operational smoke test, not a performance comparison or new
cross-chain semantic stress run. Historical attempts cannot acquire missing
preparation/proof timestamps retroactively.
