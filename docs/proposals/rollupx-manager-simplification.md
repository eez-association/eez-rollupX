# RollupX manager simplification

| Field | Value |
| --- | --- |
| Design status | Owner-requested scope accepted on 2026-10-28; implemented, validation recorded in the [audit](../audits/2026-10-28-rollupx-manager/README.md) |
| Source baseline | `6215df1a4395f58bb58bdb78bf4dc22f907c4169`, PR 112 after merging PR 115; identical tree to `f965725872be23dc514911abe73877053b4f7f65` |
| Protocol baseline | `84a5b5207880f192a6a5896cbfe0bac5249db47f`; no core edits or gitlink change |
| Scope | Rename the integration manager to `RollupX`, share anchor validation, retain the deployed proxy and existing network rules |

## Requirements and acceptance

| ID | Requirement | Acceptance |
| --- | --- | --- |
| R1 | No execution, historical replay, provider/RPC, native DA, public-input or profile change; validation contract remains 6 | Frozen protocol/prover vectors and source process tests; exact context comparison with baseline bytecode |
| R2 | Same storage, registry immutable, initialization lock, owner/admin authority, registration and proof policy | Full storage layout comparison; baseline proxy initialization, mutation, upgrade and rollback tests; pinned core policy differential test |
| R3 | Both anchor modes bind exactly the same bytes and reject the same invalid contexts | Differential fuzz and boundary tests, including rules transitions, accepted roots, coordinates and unavailable hashes; registry proof acceptance across upgrade |
| R4 | One production manager named `RollupX`; no old implementation or initializer wrapper | Source/artifact/deployment/test references updated; existing initializer state remains locked |
| R5 | Preserve fee quoting on existing and upgraded manager deployments | Exact old/new executable fingerprints, mutation rejection, estimation tests; actual deployed bytes still execute |
| R6 | Reduce duplication without weakening checks or adding gas overhead | Comparable compiler, runtime byte size and gas measurements for both anchor modes and initialization |

## Design boundary

The pinned reference `Rollup` has nonvirtual `initialize`, registration and
`getCustomData` functions. Direct inheritance would expose its context-free
initializer and replace this network's profile/root/anchor checks. Core contracts
must remain unchanged. Keep the reference proof policy, ownership and root escape
implementation together in this manager; adding a forwarding manager or a private
copy of core would add code and authority boundaries without removing that issue.

Consolidate the common anchor lookup and cache the profile/rollup ID across the
read-only registry call. Keep the two explicit flat ABI encodings: factoring
their common fields into a memory tuple adds gas, while a hand-trimmed encoding
adds assembly for only a small code-size reduction. Preserve all profile
validation, registry/genesis registration checks, active-rule search and accepted-root
checks. EVM `BLOCKHASH` already rejects current, future and older-than-256 anchors;
retaining the nonzero-hash rejection enforces the same availability condition.
The tests must establish the identical boundary and revert arguments.

Rename the deployment-only `initializeOffline` to `initialize` with the same profile
parameter. Existing proxies already consumed their initializer; upgrades use empty
calldata and preserve its namespaced lock. There is no old-selector alias. Preserve
existing profile event topics and custom error selectors because providers may
query them. The word "offline" in domain strings, wire profiles and those public
identities describes the unchanged protocol, not a second implementation.

## Compatibility and recovery

An existing validation-contract-6 network can retain its manager implementation
or upgrade the same proxy to this implementation bound to the same registry.
The change needs no hard fork, activation schedule, genesis edit, database migration,
profile/validation identity change, signer/follower update, or proof regeneration
solely because of the manager upgrade. Ordinary anchor expiry, rule changes and
accepted-root advancement still invalidate proofs normally.

The new creation/runtime bytes and metadata have different code hashes. Fresh
CREATE2 deployments using those bytes would have different addresses; no deployed
address changes on a proxy upgrade. EEZ, EEZL2, the history store and their genesis
fixtures remain unchanged. Fresh deployment tooling must use the new source artifact
and profile-bearing initializer selector.

Composer's conditional fee estimator recognizes exact executable fingerprints.
Update Composer before upgrading the manager; the new release accepts both equivalent
implementations and still executes the actual code. Old Composer estimation rejects
the new executable until updated. Existing signers and followers remain compatible.
Rollback of the manager uses the original implementation with empty calldata and
the same registry; rollback of Composer should follow that manager rollback.
Neither restart nor rollback recovers volatile queue requests.

The [runbook](../runbooks/chiado-deployment.md#rollupx-manager-implementation-upgrades)
owns the upgrade procedure; the [audit](../audits/2026-10-28-rollupx-manager/README.md)
records source comparisons, actual bytecode/gas results and remaining validation.
No deployment, wallet funding, core changes or live upgrade is authorized by this
record.
