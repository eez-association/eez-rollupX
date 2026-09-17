# Pinned client and node-host collector evidence

Candidate worktree based on `c36bbd6b4bc3c74716d4e78199bc2536db44a821`, frozen
main `5469c88860408daf776ee812156dd280c1a9e262`, protocol
`3c0cdf4955941a5c23639346932b55b33babebbd`. These are isolated fixture results,
not a production deployment or client synchronization/finality acceptance.

## Lighthouse

Command:

```sh
python3 observability/tests/smoke-client-capabilities.py \
  --report /tmp/eez-client-capabilities-owner.json
```

Both image digests in [the test lock](../tests/client-images.lock.json) passed:

| Version | Actual TYPE families | Actual sample names | File mode | Checks |
| --- | ---: | ---: | --- | ---: |
| Lighthouse 8.1.3 | 156 | 155 | 0600, explicit UID/GID 10001 | 3 |
| Lighthouse 8.1.2 | 155 | 155 | 0600, explicit UID/GID 10001 | 3 |

Each binary ran with Docker networking disabled, bundled mainnet genesis, target
peers zero and an unavailable loopback execution endpoint. A namespace-local helper
read the real Prometheus endpoint. [The sanitized inventory](client-capabilities.json)
contains metric names/types and label keys only: no metric values, addresses,
account IDs, URLs or label values. Expected initialized head/finalized/sync/process
samples were required, rather than treating endpoint reachability as coverage.
No useful sync/finality claim follows from this genesis-only sample. The raw
export includes gossip families with a `hash` label; generated Prometheus jobs
allow only the reviewed head/sync/process/network subset. `libp2p_peers` was not
observed in these zero-peer fixtures; absence remains unknown, not zero.

Actual JSON `beacon.log` records had `level`, `msg` and `ts`; only field names and
record counts were retained. Default mode 0600 was verified for both binaries.
The earlier permissive-flag experiment failed the privacy gate: it produced 0644
under umask 027. That flag is not recommended or generated. Auxiliary discv5/libp2p
files are excluded. Builtin Chiado configuration had no embedded genesis or known
download URL, so the isolated capability fixture explicitly uses bundled mainnet
and does not claim Chiado startup acceptance. Failed-run records are retained in
[client-collector-failures.json](client-collector-failures.json); full local logs
remain under `/tmp/eez-client-capabilities-*`.

## Embedded execution clients

Pinned Reth `fd59fd2` uses one process-global recorder with prefix `reth`
(`crates/node/metrics/src/recorder.rs`). Both embedded transaction pools instantiate
unlabelled `transaction_pool` families (`crates/transaction-pool/src/metrics.rs`).
Counters therefore mix events; gauges set by either pool can overwrite the other.
Neither a second scrape endpoint nor target labels can recover per-chain identity.
The embedded L1 startup has no separate metrics listener. The existing native L2
CLI endpoint exposes the shared registry. Process CPU/RSS are process-wide;
upstream pool series are mixed/unattributable, not necessarily additive totals.
Observer L1/L2 head metrics retain their separate identity verification.

The existing two native lifecycle canaries now write
`eez-client-metrics-<pid>-legacy.json` and `...-policy.json` under `EEZ_TEST_LOG_DIR`.
They require real `reth_transaction_pool_` samples and retain only sample names and
label keys. A parser regression injects quoted pseudo-label syntax and checks no
values escape. Both final native canaries passed (2/2, 257.21 seconds), and the parser regression
passed separately (1/1) against the same compiled test executable. The legacy
sample contains 880 names and the policy sample 886; each has 41 transaction-pool
sample names, and neither has an intrinsic chain label key. Their merged
[sanitized inventory](embedded-client-metrics.json) retains names and label keys
only. Logs are `/tmp/eez-observability-final-native-lifecycle.log` and
`/tmp/eez-observability-final-metric-inventory-test.log`; original sanitized captures
are in `/tmp/eez-observability-final-native-logs/`. The parent final native record
retains executable hashes and scope.

The actual scrape exposed `reth_builder_validation_disallow_hash{hash}` and four
`reth_sync_caching_precompile_*{address}` cache families. Even when upstream
precompile addresses are bounded, the EEZ collection policy excludes address/hash
labels. Generated native jobs drop complete samples carrying either nonempty
label, preserving finite EEZ metrics and unlabelled client observations. They do
not remove dimensions and combine unrelated series. The private upstream endpoint
still contains its upstream registry; this protection is in the generated scrape.
The actual Prometheus fixture exercises those observed family/label shapes and
requires both forbidden families absent and finite EEZ/client samples retained.

## Node-host collector

The generated profile passed ten real checks in `tests/smoke-node-agent.py` against pinned
Alloy/Loki with owner-only fixture files and a real Prometheus scrape. It uses the
same generated Compose/configuration as the runbook, including persistent
positions, no capabilities on the collector, readonly dedicated sources, resource
limits and loopback listener. The fixture deliberately injects message/error URL,
private-key and raw signed-body sentinels; only reviewed event names and constrained
safe scalar fields may survive. It also checks the generated Grafana Loki annotation expression matching both startup and policy markers,
rotation/restart, default group-reader reconfiguration and bounded indexed labels.
The retained report is [node-agent.json](node-agent.json). Loki adds bounded `service_name` and
`detected_level`; the first exact-label assertion detected those defaults and was
corrected to validate them explicitly. No hash/file/URL label is accepted. A stronger checkpoint assertion caught replay
of a rotated path not yet discovered before restart; that first failure is
retained. The corrected restart test first observes both file paths, then requires
zero post-restart read lines from those checkpointed files. Newly discovered
rotated paths can replay: diagnostic delivery is not exactly-once.

Runtime Alloy inventory revealed `loki_source_file_*` counters with rotating `path`
labels. The generated scrape drops these complete series, preserving local file
inspection and aggregate collector health without creating colliding label sets.
Known destination `host` and bounded component identifiers remain upstream labels.

Unsupported/limited coverage is explicit: signer diagnostic file logging has no
native adapter; filtered logs are not complete lifecycle history; unsafe arbitrary
upstream bodies are omitted; client metrics have no recovered L1/L2 attribution;
absence of a producer is not zero. EEZ package version and upstream Reth build SHA
do not establish the deployed EEZ release revision. No real operator process,
filter, directory, firewall or permissions were modified by these fixtures.

Deployment-specific remaining checks include actual private network paths,
producer file settings/ownership, host metrics availability, ingest/cardinality
and retention capacity under sustained workload, and release-to-image provenance.

Source/report digests and exact check counts are retained in
[client-collector-manifest.json](client-collector-manifest.json).
