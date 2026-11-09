# Network 916969 runtime update

| Field | Value |
| --- | --- |
| Date/status | 2026-10-26 UTC; deployed, live checks passed, public ingress restored |
| Scope | Authorized compatible-runtime replacement on the existing Chiado test network; no contract deployment or genesis migration |
| Source | `9e3599abf2f1f08c4bcfff41f496f3365b5c4561`, PR #102 |
| Previous runtime source | `792198328f0e0efdcfd7c5a42e75ecdb281cebf5`, tree-identical to PR #101 source `c1dafa6113425fce74bdba8c32ae8b0d7a7ba5ae` |
| Protocol | `84a5b5207880f192a6a5896cbfe0bac5249db47f`, unchanged |
| CI | [40310619100](https://github.com/eez-association/eez-rollupX/actions/runs/40310619100), attempt 2; all 14 jobs successful |
| Environment | Chiado L1 10200, L2 916969; both fee directions enabled; existing timing, policy and operational accounts retained |
| Related implementation | [Mixed-direction fee validation](../2026-10-26-mixed-direction-fees/README.md) |

## Qualification and artifact identity

The first CI attempt completed all runtime validation lanes successfully, but
the viewer job was cancelled before its tests ran. The final gate correctly
rejected that incomplete matrix. Rerunning that job and its dependent gate
completed the matrix without rebuilding the runtime or repeating successful
runtime tests. This was a cancelled test lane, not a failing test retried to green.

The immutable CI runtime payload was
`ghcr.io/eez-association/eez-rollupx-ci-runtime@sha256:a3fd7884a8e2705d68e9c801a43277271a81cabd398d23a00e514b627cf2ad3c`.
The repository's `ci-payload.sh fetch runtime` and
`prepare-runtime-artifacts.sh` verified the source, protocol, lockfile, platform,
release profile, archive checksum and image labels. The binaries executing in
the node, signer and temporary follower also matched the extracted archive.

| Artifact | SHA-256 digest |
| --- | --- |
| Node image | `baa5bd9b469a834a8a0147551fb6f44a0b615d26f79ddd3705ebf7bd67d41ec1` |
| Signer image | `2373bbcead6df61787a177e2e13e1a2a37449f0c93c32ae79c43d31cb1d2cf9a` |
| Deploy image, retained for matching tools | `7466f93ec56315428a2c597157c2d47853492b1108090e7b5aef6c1656d3a30c` |
| Running Composer binary | `a2c92bf6dc956a78156b8d7fb8c4a29d9491ae41429f51d4e8cacb3ac19fc6fc` |
| Running signer binary | `43680285f6ab00260e91201bf80d545a3b332a2094ab6a525135176b8a52c1cf` |
| Validation follower binary | `6b126905227068416657a23dccb7d0af258a9f70d4a378801dfc328abe5767ab` |

Image repositories are respectively `ghcr.io/eez-association/eez-rollupx-node`,
`eez-rollupx-proof-signer` and `eez-rollupx-deploy` under the same owner.
No image was rebuilt or promoted to `latest`, and neither PR was merged.

## Executed operation

An isolated deployment checkout at source `9e3599ab` received a private copy of
the existing configuration. Rendered Compose services were compared: only node
and signer image references differed. The explicit Compose project retained the
existing service ownership. All datadir mounts, logging, keys, fee settings and
genesis bindings were preserved. Contracts, genesis files and dependencies did
not change between the old and new runtime sources.

Before stopping, all four held-queue gauges were zero. The 40 blocks following
the accepted L1 commitment contained no non-history transactions. Public network
ingress was paused; both node and signer then exited cleanly with status 0.
Complete L1 and L2 directories were copied with stopped writers, alongside
configuration and genesis. Lighthouse and all auxiliary services remained running.
This update did not take a stopped-writer Beacon copy or rehearse backup restore.

`bash scripts/chiado-up.sh` ran from the new release checkout and passed. The
node stopped at 12:44:02 UTC and the replacement process started at 12:44:46 UTC;
these process times do not measure the exact RPC availability interval. The
signer started with the new artifact and fresh in-memory sessions, without L1
or L2 RPC configuration. A new canonical type-3 settlement and its selected
full blob were observed. Public ingress stayed closed for validation and was
restored at 12:53:35 UTC, after approximately 573 seconds. The gateway's complete
configuration was verified equal as parsed JSON to its
pre-maintenance configuration.

## Live validation

| Check | Result |
| --- | --- |
| Official startup checks | Passed; L2 advanced, L1 commitment advanced, new type-3 settlement and full blob retrieval succeeded |
| Fee-enabled cross-chain wave | `cross-chain-wave.sh`, mixed mode, one wave: eight successful source transactions, four posts; semantic assertions enabled |
| Application results | Direct and wrapper setters, no-return calls, L1-to-L2 deposit and L2-to-L1 withdrawal matched exact expected state and amounts |
| Same-block mixed inclusion | Four inbound sources and one outbound source settled in L1 block `23700344`; the other three outbound sources settled in the following three L1 blocks |
| Attestation | Runner observed successful signer output; signer logged no warnings or errors in the captured interval |
| Independent execution | Resumed the existing separate follower from its retained state, using only L1/Beacon, with no Composer RPC or P2P peers |
| Block comparison | All 491 blocks `49940` through `50430` matched, including hashes, state/transaction/receipt roots, PREVRANDAO, timestamps, gas and ordered transaction hashes |
| Receipt comparison | All 117 receipts in those blocks matched exactly; all four outbound source receipts also matched individually |
| Canonical settlement | All eight source receipts successful and canonical at read; outbound correlation records resolved to successful L1 posting receipts; final registry commitment matched the independent follower |
| Final queue | Zero queued, in-flight, blocked and unknown requests |
| Public routing | L1/L2 RPCs and both fronts returned the expected chain IDs; dashboard and both explorers returned HTTP 200 |
| Beacon | Synced, non-optimistic, execution layer online, 38 connected peers at the recorded check |

The final compared commitment was
`0xe840b1936701751b53eec34d1ff0c7d09efe5e6144fbc6d0ca0c413cbe940f97`.
Every adjacent compared L2 timestamp differed by one second, including the
restart catch-up. This is a canonical timestamp property, not uninterrupted
wall-clock production during maintenance.

The wave used a separately funded test account, never the posting account;
new actors received 0.006 test native units per side. Both known protocol
worktree wallet locks were held. The first preflight stopped before any broadcast
because an inventory parser assumed every Docker environment entry had an `=`.
Filtering unset entries corrected only that parser. The single broadcast wave
then passed without resend or assertion changes. The host's existing `/tmp`
quota required an owned `TMPDIR`; a shell wrapper redirected only the runner's
hardcoded deployment scratch directory, without changing its assertions.

Two bundle attempts were dropped (`excluded` and `slot_skipped`); normal
recomposition ultimately settled every source. Logs also contained temporary
missing-slot allowance deferrals and a rejection of reinserting a cross-chain
source into the ordinary pool. That source was verified canonically settled.
These diagnostics are retained; this is not a zero-warning or capacity result.

## Evidence and operational handoff

[evidence.json](evidence.json) contains compact results and hashes of retained
records. Raw logs, the CI manifest/archive, private gateway snapshots and
execution evidence are local-only under the implementation worktree's
`artifacts/rog-upgrade-9e3599ab`; they are not available in another checkout.
CI and immutable image/payload digests are remotely retrievable subject to
provider retention. This dated record was prepared locally after deployment;
it was not added to the already-qualified runtime commit.

The active core checkout is `artifacts/rog-release-9e3599ab`. Its private
`OPERATIONS-916969.md` records the restart commands and recovery paths. The
previous checkout still owns the live data and auxiliary service configuration;
it must be retained. Its operator note now points core operations to the new
checkout. The temporary follower was stopped cleanly after validation and its
separate data retained. The previously owned Kurtosis network was not altered.

This validates a compatible runtime replacement and a bounded semantic smoke
test. It does not rerun the earlier 770-job workload, establish sustained-load
capacity, demonstrate durable queue recovery, validate finality of every recent
receipt, or rehearse restoring backups after new L1 settlements.
