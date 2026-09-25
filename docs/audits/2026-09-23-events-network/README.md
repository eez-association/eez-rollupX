# Chiado feature/events network replacement

Status: deployed and live-verified on 2026-09-23. The operator authorized a recoverable backup,
retirement of the current network and a fresh official network using the same
accounts and public routes. After live compatibility checks, the operator
explicitly authorized new L1 contracts, rollup ID 1 under the new registry and
a new L2 chain ID. The new L2 chain ID is 6291; the previous chain ID was 6290.

Source: fork `1832f970884ca9460646a4cde87cca70eaffb3e1` with the validated local
feature/events port and preserved settlement/timing work. Protocol:
`820e50f749f63165be6a5797d4c5fe52cbbbbe34`. See the
[port validation](../2026-09-23-events-protocol/README.md).

## Compatibility decision

The old registry `0xc412aae1361bcd8da4581153df182f293437fdd7` is incompatible
with the new manager registration callback, settlement events and proxy code.
A read-only `eth_call` with a temporary selector-only manager code override
succeeded for `rollupContractRegistered(uint64)` (`0x421946d4`) and reverted
for `rollupContractRegistered(uint64,address)` (`0x50f2d187`). No transaction
was broadcast by this check. A new registry was authorized instead of an
untested hybrid protocol implementation. Rollup numbers are registry-local;
ID 1 under the new registry is a distinct identity.

## Backup and recovery boundaries

A private recovery bundle is retained at
`artifacts/network-backups/chiado-rollup-1-20260923T184346Z/` in the host's main
checkout. It contains the exact current image archive (20 images), container
and image identities, deployment/configuration/credential files, genesis and
broadcast records, and a cold filesystem archive of all 13 writable mounts:
L1, L2, Beacon, posting logs, monitoring databases and explorer/gateway state.
Files are copied only after their writers stop, compared against those stopped
sources, then SHA-256 checked. The private parent directory has mode 0700.
Raw configuration, credentials and database contents are not included in Git.

The first attempt failed on archive destination permissions before copying data;
its recovery handler restarted the original services. A permission probe passed
after granting the archive container the required filesystem access. The first
complete backup passed comparison and checksums, and the original services were
restarted during release preparation. The final cold snapshot completed at 19:11:26 UTC after all writers stopped
at 19:06:48 UTC. Its 40,125,521,920-byte state archive passed comparison and
checksums; preliminary hashes are retained as superseded evidence.

The existing L2 directory and old indexer/Prometheus/Blockscout volumes are
retained separately from the new network. L1 and Beacon databases can be reused
serially because they remain on Chiado. Grafana keeps its login/configuration;
new network data uses separate stores. Blobscan remains a Chiado-wide explorer.

A filesystem backup does not preserve pending in-memory request ownership or
retry counters. Recovery must use the saved old binaries, keys, bindings and
6290 genesis, stop current writers, restore into empty paths according to the
private mount map, and reconcile canonical settlement before accepting work.
Do not point new-protocol binaries at old L2 data. Archive integrity and original
container restart have been checked; a full restore-from-archive rehearsal has
not been performed. The saved Beacon state and complete L2 correlations are
part of recovery; an execution-only snapshot is insufficient for blob replay.

## Candidate checks before broadcast

- Rust 1.94.0, `--locked`, release build with the same package/target/feature
  selection as `Dockerfile.runtime`: passed in 12m 59s.
- The exact release Composer, Signer and follower passed
  `outbound_changing_static_reads_and_write_settle_and_replay`: one test,
  zero ignored, 47.33 seconds. The previously compiled matching test harness
  explicitly selected these release executable paths; no fallback build or
  binary from another revision was used. This verifies 5/5 → write 79 → 79/79,
  canonical settlement and independent follower reconstruction on a fresh
  disposable local network. That process test is local evidence; separate live results appear below.
- Matching node, signer, deploy, indexer and public monitor images built locally.
  These are locally validated development artifacts, not CI-promoted images.
- The deployment image has Foundry 1.7.1, Solidity 0.8.34 and compiled matching
  contracts. Offline genesis rendering and block-hash helper checks passed.
  The first offline preflight lacked the compiler; the image was amended to
  compile/cache the pinned compiler before repeating the successful check.
- Public monitor event filtering updated to `BatchPosted(bytes32,uint64[])`;
  all 58 monitor tests passed. The previous image remains in the backup.
- The live Grafana dashboards, including query improvements, inclusion ratios,
  preparation failures and proving/preparation timing, were carried into the
  new configuration. This preserves operator customizations missing from the
  source checkout's original dashboards.
- Initial funded EOA allocations and operator keys are retained. This is a
  fresh network, not migration of deployed application contracts or current
  account balances/nonces. The independent deployment RPC was checked on
  Chiado chain 10200 before stopping its local counterpart.

## Evidence

Local-only deployment evidence is under
`artifacts/events-deployment-20260923/` in the source checkout: working-tree
patch, release binary hashes, image IDs, build logs, preflight output and process
test logs/datadirs. Private deployment inputs are kept there with restricted
permissions. Ignored artifacts are unavailable to another checkout and have no
external retention guarantee. Final sanitized identities, live results, executable/image identities and backup
digests are recorded in [deployment.json](deployment.json). Image IDs identify
local content; these artifacts were not published or promoted by CI. Some base
image labels are inherited and stale; the recorded source patch and running
binary SHA-256 comparisons establish this operation’s identity instead.

## Live cutover and checks

The official L2 is **6291**, rollup **1**, registered at
`0x5d6b8fc56e43528c219de7eed257564c05923e08` on Chiado 10200.
Deployment block: **23196319**. Proof system:
`0xb3c27735413385ba7149b48c6b528ef64eff07b7`; manager:
`0x0205d8d23200b59120d2886979752af80af20ca8`.
The wallet bridge is `0xCa0a7803C6d982628A48B1Cc82009D6493e3c8fD` on both
chains; runtime bytes match, managers and rollup IDs match their chains, and
both administrators equal the original operator. The L2 genesis hash is
`0xeabf267893422f6a5605ffe9c95c6ae88e0d8863e65b7d11d8a6873335ea06d8`.

- The unchanged operator accounts are used by Composer and Signer. Chain 6291
  has its own L2 directory, posting logs, indexer database, Prometheus store and
  Blockscout database. No old L2 data or old monitoring volume was removed.
- The startup procedure verified synchronized Lighthouse, L2 production,
  canonical commitment advancement and a type-3 settlement whose selected
  versioned-hash blob is retrievable. An initially idle fresh network did not
  settle until ordinary user activity was submitted; the preserved policy is
  pure-L2 interval 120,000 ms and general maximum 7,200 unsettled blocks.
- The exact mixed case ran live: four static reads surrounding one mutable
  call to one L1 host. Transaction
  `0xd9910b1913eb527a8c3c59d5a1cced15a105cfdc70abe5b7cc27a668b13a5d9c`
  settled in canonical L2 block **920**. Reads returned **5/5 → write 79 →
  79/79**, stored sum **168**, and one completed write. L1 state is 79 and
  the verified L2 safe state agrees. The initial receipt belonged to an earlier
  speculative block; verification fetched the settled receipt and checked its
  block hash against the canonical chain. No replacement probe was submitted
  to obtain this result.
- L1→L2 transaction
  `0x0f072908b31d886d42dc5dc7e53c25fa0dc5ff9e68f71a01e700f0822dd93d3f`
  succeeded in L1 block **23196515**; the receiver has exactly **1,000 wei**
  in L2 safe state. This checks the minimal native bridge, not every wallet
  bridge/ERC-20 flow.
- Public `/rpc/l1`, `/rpc/l2`, `/composer/l1` and `/composer/l2` report
  10200/6291 as appropriate. The UI reports the new registry, proof system,
  manager, bridge and protocol revision. The network monitor snapshot is
  healthy with no errors and decodes the new settlement event.
- Grafana retains the existing password and provisions the preserved dashboards.
  Both data sources pass their health checks. All **12 SQL panel queries**
  returned nonempty results with no errors. Observed query times were 5–33 ms;
  Posting timing & outcomes took 8 ms. This is a fresh-database health check,
  not a comparison against the old network’s larger history. Dashboard links
  now use the current explorer/blobscan domains.
- Prometheus’s Composer target is up. Its public route still requires the
  existing authentication (unauthenticated readiness returns 401). Grafana,
  Blockscout and Blobscan public routes respond successfully. Blockscout’s
  configured chain ID is 6291 and it is indexing the new chain. Blobscan keeps
  its Chiado-wide history. All 20 expected long-running services are running.

## Integration corrections and limits

The initial indexer source lacked live receipt-by-block-hash lookup, an optional
independent L1 enrichment RPC and a transaction-hash SQL index. These operational
changes were carried forward while retaining the new event/fee decoder. Final
validation: **22 tests passed**, including the explicitly enabled disposable
PostgreSQL recovery test; formatting, all-target Clippy with denied warnings,
and the release build passed. The disposable test database was removed.
Preserved failures cover a stale Cargo artifact after copied source timestamps,
a missing optional-field test initializer, and an outdated dashboard expectation.
Fresh compilation and corrected fixtures passed; these were integration fixes,
not retry-only test passes. Monitor validation had 58 passing tests.

Other recorded setup corrections: the first bridge helper used a login shell
that hid Foundry from PATH and made no broadcast; the normal shell succeeded.
Monitoring was first invoked from the wrong Compose directory, then corrected.
The indexer initially inherited the old posting-log mount through a second env
file; both env files now agree and new events are ingested. Blobscan’s initializer
initially ran while the public gateway was stopped; starting the gateway and
rerunning initialization succeeded. Initial UI configuration checks exposed old
proof-system/manager/bridge fields, all corrected before final verification.

The first semantic checker queried L1 before settlement and then compared an
obsolete speculative receipt. Both failures are preserved. The final checker
waits for settlement, refetches the transaction receipt, checks its canonical
block hash and reads L2 state at `safe`. This avoids mistaking optimistic execution
for settlement. Live independent follower replay was not run; independent replay
was covered by the exact-release local process test. No performance benchmark,
full archive restoration, full upstream live suite, or new explorer source
verification was performed. New-chain application balances/contracts/nonces
were not migrated. The archive and current new state must remain separate.

## Operator handoff

The active configuration checkout is `artifacts/fix-mixed-outbound-source` in the
main host checkout. Its `.env` and `deployments.env` point at the main checkout’s
active files. Node Compose includes the posting overlay. Monitoring lives under
that source checkout’s `monitoring/`; use both `.env` and `.env.indexer` there.
Explorer configuration remains in the main checkout’s `.env.explorers`. UI and
public gateway retain their existing repositories and domains. Private recovery
instructions are `RECOVERY.md` inside the backup directory. The normal startup
script’s explicit base Compose selection omits the operator overlay; this
operation’s `artifacts/events-deployment-20260923/start-network.sh` preserves
both files and all startup checks. Use the active Compose files for routine
restarts. Do not rerun deployment: it would create another registry.

## Subsequent full protocol network suite

After cutover the operator requested the pinned protocol’s complete staged
network validation. The [full-suite record](../2026-09-23-events-staged-all/README.md)
reports **32/32 passed** with semantic verification enabled, including all four
new read/write/read cases, on this same deployment. The first attempt’s external
RPC rate-limit failures are preserved separately. This supersedes the earlier
statement that the full upstream live suite had not yet been performed; other
unrun checks and recovery limits remain unchanged.
