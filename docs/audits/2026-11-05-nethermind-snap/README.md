# Nethermind native SNAP validation, 5 November 2026

## Qualified Compose procedure at 542dc9365271

The exact base, P2P and SNAP overlays ran from an empty database on a second VM,
using only the published discv4 seed and no static/trusted peer or client filter.
Consumer image `542dc93652718b8a0ed0c102f81c4e3b35a5ef21` is pinned by digest in
the runbook; the serving follower remains at `a75cf55f016595162fa297a9e99d91ef477e01cf`.
No source database or node key was copied.

- Pivot 339690 downloaded with all headers, bodies and receipts; exit 0 and both
  product verification markers. Runtime was 391.997 seconds from process start,
  including a discovery/recent-IP admission wait for a replacement peer.
- The follower reached 341006; after a manual same-database restart it reached
  341090. Live-head lag was 1 at both checks. Canonical hashes and roots for
  latest/safe/finalized, three state reads and a post-pivot receipt matched the
  public source. Earlier checkpoint hashes remained canonical.
- The same image also recovered the exact database that failed at the earlier
  revision, reaching 340650 at lag 1 with the old checkpoints retained.
- The existing second node on the source VM was separately upgraded with a
  stopped backup. It reached 340887 at lag 1; twelve state reads and native
  `0x46` receipts matched. The public serving follower was not restarted.
- Temporary discovery and Compose rehearsal containers were removed. Operators
  retain their data and raw logs privately. Transfer from a different client
  remains untested.

Source-bound checks for this image:

- [Live integration: all 20 partitions](https://github.com/NethermindEth/nethermind/actions/runs/42052396645)
- [Streaming x64/ARM64: 2,965 EEZ and 48 execution/witness checks per architecture](https://github.com/NethermindEth/nethermind/actions/runs/42052395594)
- [Published image](https://github.com/NethermindEth/nethermind/actions/runs/42052492557)

## Earlier single-host check at 8a769ce0e80c

Nethermind consumer `8a769ce0e80c060abc1e34482da0050c931dcbca` downloaded and
verified a fresh database on Ethereum-backed RollupX chain 696991. The source
was the operated Nethermind follower at `a75cf55f016595162fa297a9e99d91ef477e01cf`.
No source database or node key was copied. This is a small-state real-network
result, not a sync performance guarantee or different-client qualification.

The [sanitized result](result.json) retains the pivot, process identities,
same-database restart, exact-height state/receipt checks and public seed policy.
Operators retain raw console and RPC records privately. The result is not a
publicly retrievable raw-log archive. No private endpoint or key is included.

The source had TCP/UDP 30313 enabled with only loopback RPC/Engine/metrics.
The consumer used normal discovery, 20 peers, empty static/trusted lists and no
client matcher. Its observed peer negotiated eth/72 and snap/1 and was neither
trusted nor static. Actual SNAP account/storage/code responses were observed.
External TCP reachability passed. A second VM then used only the published
discv4 seed, empty static/trusted lists and the packaged Compose overlays to
download real SNAP data over the public network. This also exercised external
UDP discovery and ETH/SNAP peer negotiation.

The process downloaded all headers, bodies and receipts, healed state and
exited 0 with the product's final verification marker. The follower then executed
native history transactions after the pivot. Canonical header hashes/roots,
12 balance/nonce/code/storage samples and receipts matched the public source at
fixed heights before and after a graceful process replacement on the same DB.
Safe and finalized heads advanced; old canonical hashes were retained.

The [SNAP runbook](../../runbooks/nethermind-snap.md) is the current procedure.
The current Compose qualification is recorded above. This earlier single-host
result alone did not qualify the packaged overlays or every restart boundary.

Earlier source-bound CI at `8a769ce0e80c`:

- [Native SNAP and live matrix](https://github.com/NethermindEth/nethermind/actions/runs/42022762109)
- [Streaming x64/ARM64](https://github.com/NethermindEth/nethermind/actions/runs/42022762083)
- [Published image](https://github.com/NethermindEth/nethermind/actions/runs/42027688013)

## Restart regression found by the Compose rehearsal

The first cross-VM rehearsal at `8a769ce0e80c` downloaded pivot 338346 in
120.413 seconds and exited 0. Its follower reached block 339342 at lag 1, with
matching canonical roots, state and a post-pivot receipt. A later graceful
restart failed: durable state loaded at 338850 while stored safe/finalized
pointers were 339318/338868. The bootstrap guard rejected these valid ahead
pointers before native recovery could replay stored blocks.

The EEZ-only fix is `542dc93652718b8a0ed0c102f81c4e3b35a5ef21`. It preserves
forkchoice, requires processed canonical ancestry back to the L1-authorized
pivot, and leaves execution recovery to the existing native recovery path.
Eight regression cases cover both durable-head positions and invalid metadata.
The edited image recovered that same preserved database without changing chain
data or the assertions. The independent fresh-image Compose download/follow/restart
also passed, as recorded above. The earlier successful restart alone did not
cover this durable-state boundary.
