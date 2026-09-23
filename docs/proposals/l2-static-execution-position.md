# L2 static reads at an execution position

| Field | Value |
| --- | --- |
| Design status | Accepted scope: operator requested the feature/events port and sound Composer/Signer support on 2026-09-23; implemented and deployed to the replacement network, with local and live semantic validation |
| Source baseline | Fork 1832f970884ca9460646a4cde87cca70eaffb3e1, with existing local settlement/timing fixes preserved |
| Protocol baseline | 3c0cdf4955941a5c23639346932b55b33babebbd → 820e50f749f63165be6a5797d4c5fe52cbbbbe34 |

The [replacement-network record](../audits/2026-09-23-events-network/README.md)
and [complete staged suite](../audits/2026-09-23-events-staged-all/README.md)
record deployment and all four live read/write scenarios. This is a fresh-network
cutover, not an in-place upgrade or old-history replay claim.

## Problem and requirements

Repeated identical L1 reads around a successful mutable cross-chain call previously selected the first cached answer. The new L2 static entry carries `expectedEntryIndex`, and lookup compares both this cursor and the proxy hash. Two reads at the same position must still agree. Local writes without a successful mutable cross-chain consumption do not advance this position.

- R1: Materialize the cursor from the ordered execution forest; preserve it through the system load, proof authorization, DA evidence and follower replay.
- R2: Independently verify every static row's position against the L1 host's ordered calls. Reject missing, extra, conflicting or misplaced rows; retain exact ABI decoding and public-input recomputation.
- R3: Preserve existing mixed-profile limits (terminal reads, successful flat mutable sources, no root revert spans or explicit call gas). Broader upstream support does not extend these limits.
- R4: Accept and independently replay two identical reads, a write, and two identical reads with a changed result. Reject different results at the same hash and cursor.

## Design

Use the native L2 static ABI type at all L2 source boundaries, separate from the unchanged L1 static entry. The materializer records the mutable cursor at each read; the signer reconstructs this fact rather than trusting Composer's index. Update changed event selectors, proxy construction and genesis fixtures at the same pin.

Upstream specification discrepancy: parts of `STATIC_ENTRY.md` and the core prose still describe hash-only static selection. The pinned `IEEZL2.sol`, `EEZL2.sol` and `EEZL2StaticCursor.t.sol` explicitly require the execution-position key. This fork adopts that implemented and tested contract; stale prose is not evidence of hash-only compatibility. Upstream also permits deduplicated/reordered static pools. This fork retains one canonical row per observed static root in call order, so complete-load matching remains exact; it does not accept arbitrary upstream table layouts merely because the contract can execute them.

## Compatibility and recovery

This is a coordinated protocol revision, not a binary-only update to the current Chiado deployment. L2 function selectors, static sidecar bytes, proxy initcode and manager runtime change. Old static sidecars/system transactions are not interchangeable with the new format. Existing genesis/contracts require a separately specified migration or a new network; replaying old history with the new ABI is unsupported. Validation identity schema 2 prevents mixed-version peers from sharing cached validation facts. Keep current deployments on their matching binary/configuration/genesis. Do not deploy this port automatically. New deployments must render genesis and deploy matching contracts before starting matching Composer, Signer and followers. Downgrading binaries after new-format blocks is unsupported. Pending queues remain in memory; restart is not durable recovery.

## Validation

Baseline Solidity reproduction demonstrates the old cached answer after a write. Repeat with cursor-tagged entries at the new pin, test signer cursor tampering and same-position conflicts, and run a source-built cross-chain process test with independent follower replay. Run affected codec/signer/prover tests, protocol/integration Foundry tests, Anvil suites, genesis regeneration/check, formatting and shipping-feature builds. Record actual results and omissions in the [audit](../audits/2026-09-23-events-protocol/README.md).

## Progress

The new position is retained through Composer, signer authorization, native DA
materialization and follower reconstruction. All 32 pinned Anvil scenarios and
463 selected core tests pass, including the four read/write/read scenarios added
in the final branch update. All 34 selected native process tests and 1,021 affected
Rust tests pass. The changing-read test settles and independently replays; a
portable cold signer capture validates execution and rejects tampered positions
and group bytes. Final shipping-feature build status and validation exclusions
are recorded in the audit. This status does not authorize deployment to an old
network.
