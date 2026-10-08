# Protocol b75dddf upgrade and top-level static callbacks

| Field | Value |
| --- | --- |
| Design status | Draft. Not accepted; nothing implemented or deployed. |
| Date | 2026-10-08 |
| Source baseline | `eduadiez/rollupX` `main` at `17ff5fc` |
| Protocol baseline | Current pin `855fe0602484750861b4c31502a1a91355d33ed6`; target `b75dddf19379bdb25d2eda021098db39c4437b20` (9 commits, the pin is an ancestor) |
| Related work | Protocol scenarios `topLevelStaticReentrantCounter` / `topLevelStaticReentrantCounterL2` ([STATIC_ENTRY.md](../../eez-core-protocol/docs/STATIC_ENTRY.md) §4.2–4.3, CORE §E.2, §F); [L2 static execution position](l2-static-execution-position.md); [deployed genesis records](../../deployments/README.md) |

## Problem and scope

Two pieces of work, planned together because they ship to the same networks:

1. **Top-level static callbacks.** The protocol scenarios above, a static read
   whose remote target makes one static callback to the source chain, are
   rejected by the composer's guard "static cross-chain calls with descendants
   are not supported" (`crates/eez-protocol/src/entries/mod.rs`). Their live
   triggers are never mined.
2. **Contracts at protocol `b75dddf`.** The only core change after our pin adds
   `_validateCallDestination` / `EEZDestinationForbidden` to `EEZ`, `EEZL2` and
   `EEZBase`: a forwarded cross-chain call may no longer target the manager,
   which closes re-entry under an authorized proxy's identity. Structs, hashes,
   events, storage layout, `CrossChainProxy`, `Rollup`, the proof system and
   compiler settings are unchanged. The rest of the diff is tests, scenarios,
   docs and periphery.

Networks: Ethereum mainnet (L2 696990), Chiado `eez.asuscomm.com` (906969) and
Chiado `eez.dev` (699069) are upgraded **in place** through their proxies. New
deployments use regenerated genesis files.

**Key findings that keep this simple:**

- The contracts at our current pin already support both static round trips. The
  same tables produce the same hashes and results at `855fe06` and `b75dddf`.
  Static-callback support therefore does not depend on the contract upgrade.
- No node, signer or deriver code pins the managers' code; they execute whatever
  is on chain. A binary built at the new pin works before and after the contract
  upgrade, so no activation height or lockstep with the contracts is needed.
- An in-place upgrade never changes a network's genesis. Deployed genesis files
  are recorded under [`deployments/`](../../deployments/README.md); mainnet's and
  `eez.asuscomm.com`'s are archived and checked against their block 0.
  `eez.dev`'s must be added before its upgrade.

**Non-goals:** static callbacks deeper than one level, mutable or failing
callbacks, the `…MissingProxy` scenario variants (the protocol marks them not
ready), and changing the DA or RPC wire formats.

## Requirements and acceptance

| ID | Requirement | Acceptance check |
| --- | --- | --- |
| R1 | An L1 static read whose L2 target makes one static callback to L1 settles as **one L1 static entry** whose `l2ToL1Calls` carry the callback and whose `rollingHash` is the untagged static fold of the callback results. Nothing is delivered or loaded on L2. | Process test modelled on `topLevelStaticReentrantCounter`; the read key is absent from L2 tables and events; follower replay. |
| R2 | An L2 static read whose L1 target makes one static callback to L2 settles as **one L2 static entry** whose `incomingCalls` run the callback, plus **one L1 zero-hash entry** that executes the static read with one static row keyed at the host hash right after `CALL_BEGIN`. | Process test modelled on `topLevelStaticReentrantCounterL2`; follower replay. |
| R3 | The composer, the shared verifier and the signer accept only that shape: depth 1, callback static, successful, zero value and gas, no revert span, targeting the source chain, under a top-level static call. Everything else stays rejected. | Unit tests for each rejected variant (depth 2, mutable or failing callback, nested static with a child, wrong target chain). |
| R4 | The L1 row's cached result is bound to the result the L2 callback actually produces, and the signer recomputes that binding independently. | Verifier tamper tests: row `returnData`, `rollingHash`, missing or extra row, non-static incoming call. |
| R5 | A callback whose source proxy is not deployed on the executing chain is rejected at admission with a clear error, never settled. | Unit or process test with the proxy absent. |
| R6 | The pin moves to `b75dddf`; genesis templates, validator revision, specs and fixture provenance are updated. | `scripts/update-eezl2-genesis.sh --check`; protocol core tests and Anvil runner; the full CI matrix. |
| R7 | The composer rejects a cross-chain call whose destination is the target chain's manager, before and after the contract upgrade. | Unit tests per direction and for static sub-calls. |
| R8 | Wallet gas estimation recognises both the current and the new L1 registry implementation. | Estimator unit test with both fingerprints. |
| R9 | Each network is upgraded in place with preserved proxy addresses and recorded evidence; `eez.asuscomm.com` first. | Upgrade checklist below; record in `deployments/<network>/README.md`. |
| R10 | Recorded genesis files never change. | Hash check in review; optional CI check of each recorded file against its README. |

## Design

### Part A: top-level static callbacks (no contract dependency)

All emission and validation logic is shared code in `eez-protocol`, so the
composer and the signer stay in agreement.

**Emission** (`entries/materialize.rs`):

- `push_static_entry` (L1-triggered): fill `l2ToL1Calls` from the static root's
  children and set `rollingHash` with the existing, currently unused
  `StaticCallRollingHash` (seed 0, `keccak(prev ‖ success ‖ returnData)`).
  Root pins are filled where they are today.
- `push_l2_static_entry` (L2-triggered): fill `incomingCalls` the same way. The
  host entry already executes the static root and reuses the machinery for
  nested static rows under mutable parents (row keyed by
  `keccak(callHash ‖ hostHash)` at `CALL_BEGIN`, no fold). One change: a static
  child of a static root must not open an L2 delivery entry, because the L2 side
  lives in `incomingCalls`.

**Validation:**

- `ensure_materializable_calls`: allow descendants only for a top-level static
  call whose children all satisfy R3.
- `system_tx.rs` shape checks: L2 static entries may carry static incoming calls
  (value 0, gas 0, no revert span, source rollup 0); with none, `rollingHash`
  must be zero. L1 static sub-calls additionally require gas 0.
- `entries/verify.rs`: allow a non-flat L1 host when its rows are static rows of
  static sources. Between the host's `CALL_BEGIN` and `CALL_END`, each L2
  incoming call must match the next row at its position key, with empty
  sub-calls and zero `revertedOrStaticRollingHash`. The fold of the row results
  must equal the L2 entry's `rollingHash`. That equality is the cross-chain
  binding (R4). Existing checks still reject unconsumed or extra rows.
- Settlement observation (`settlement.rs`, `eez-driver/src/observation.rs`,
  shared by the composer's witness generation and the signer): allow static
  roots with expected callbacks. The observation inspector answers each callback
  from the claim after checking frame, caller, target and calldata, and requires
  every callback to be consumed.
- Signer-only code needs no new rules: it re-materializes and byte-compares
  through the shared builder and verifier. Its spec documents the widened
  profile.

**Admission (R5):** before admitting, check that the callback's source proxy
exists on the chain that executes the static entry: the L1 proxy of the L2
target for L1-triggered reads, the L2 proxy of the L1 target for L2-triggered
reads. Simulation cannot observe this otherwise: a call to an undeployed proxy
is not intercepted and would return empty success, while the chain would revert
with `StaticCallProxyNotDeployed`.

**DA and derivation:** unchanged. Blobs already carry static calls and their
children as a semantic forest. Followers re-materialize through the same builder
and byte-compare, and EEZL2 enforces the L2 `rollingHash` when the callback
executes. Old signers and followers fail closed on the new shapes, so the node
and signer are upgraded together, as they already are.

### Part B: pin bump to `b75dddf`

- Bump the gitlink. Regenerate the genesis templates with
  `scripts/update-eezl2-genesis.sh` and check with `--check`.
- Update the validator revision string (`protocol=b75dddf…`). This changes the
  composer/signer identity, which they already upgrade together.
- Composer guard (R7): reject calls whose destination is the target chain's
  manager, and decode `EEZDestinationForbidden` for diagnostics. Rejecting them
  is correct against both old and new contracts.
- Estimator (R8): make the registry fingerprint a list containing the current and
  the new implementation, so estimation works on both sides of each upgrade.
- Update spec headers, `AGENTS.md` pin notes and the signer fixtures' provenance.
  The fixtures replay old-implementation history, which is what an upgraded
  network replays, so recapturing them is optional.

### Part C: in-place upgrade of each network

Order: `eez.asuscomm.com` (rehearsal), `eez.dev`, then mainnet. Ship the Part A
and Part B binaries first; they work with the old contracts.

**Preflight (all networks):**

1. The new `EEZ` implementation exposes the same `PROXY_INIT_CODE_HASH` as the
   deployed one. It embeds the `CrossChainProxy` initcode, whose metadata hash
   depends on the compile layout. If it changes, every computed proxy address
   changes. Compile exactly as the original deployment did and compare before
   broadcasting.
2. The same check for the new `EEZL2` implementation, which must also match the
   genesis implementation's immutables (rollup ID, system address,
   `USE_GAS_LEFT = false`, recovery address). Check its runtime size against
   EIP-170 and its initcode against the L2 DA limit.
3. Identify the owners: the `EEZ` registry `ProxyAdmin` owner on L1, and the
   EEZL2 `ProxyAdmin` owner on L2 (mainnet: `0xceaf908c…`). The L2 owner needs L2
   ETH, which only an L1 deposit can provide, since no account is funded at
   genesis.

**Upgrade:**

1. L1: deploy `new EEZ(sameRecoveryAddress)`, then
   `ProxyAdmin.upgradeAndCall(registryProxy, newImpl, "")`.
2. L2: deploy the new `EEZL2` implementation with an ordinary L2 transaction,
   then `ProxyAdmin.upgradeAndCall(0x4b27…, newImpl, "")` from the owner.
3. Verify:
   - both ERC-1967 implementation slots;
   - code hashes;
   - unchanged `PROXY_INIT_CODE_HASH()` and computed proxy addresses;
   - the cross-chain smoke test and the two static scenarios;
   - a wallet estimate.

   Then record transaction hashes and new implementation addresses in
   `deployments/<network>/README.md`.

**Rollback:** the storage layout is unchanged, so each `ProxyAdmin` can point
back to the previous implementation. Binaries roll back to the previous image,
node and signer together.

## Alternatives considered

- **Redeploy every network on the new protocol.** Rejected for the existing
  networks, mainnet especially; still the path for new networks.
- **Support static callbacks only after the contract upgrade.** Unnecessary: the
  current contracts already support them.
- **General static call trees of any depth.** Deferred. Depth 1 covers the
  protocol scenarios and keeps validation small.

## Validation plan

- **Unit:**
  - materialization per direction, with hand-computed hashes;
  - guard and shape rejections;
  - verifier tamper cases;
  - observation inspector (callback answered, mismatch, unconsumed);
  - blob round trip and reconstruction;
  - signer semantics and outbound tests;
  - the estimator with both fingerprints;
  - the manager-destination guard.
- **Process (`eez-node-e2e`, serial):**
  - one test per direction, each with follower replay;
  - a depth-2 rejection;
  - a missing-callback-proxy rejection;
  - the existing `cross_chain` and `chained_interstate` suites.
- **Protocol:** core tests, the Anvil runner, and the Kurtosis protocol suite at
  `b75dddf`, including the two new scenarios.
- **Operations:** the rog rehearsal before touching the other networks, with
  evidence recorded per network.

## Implementation and follow-up

Status: draft, not implemented.

1. **PR 1, Part A (static callbacks)** at the current pin, with its regressions.
2. **PR 2, Part B (pin bump)**, with the genesis regeneration, guard, estimator
   and docs.
3. **Operations:** rehearse on `eez.asuscomm.com`, then upgrade `eez.dev` (after
   archiving its genesis) and mainnet.

Open questions:

- Who holds the mainnet L1 registry `ProxyAdmin` and the L2 owner `0xceaf…` keys,
  and how will the L2 owner be funded?
- Is a callback to a contract that is itself a proxy target on another rollup in
  scope? Proposed: no, the callback must target the source chain.
- The protocol's `…MissingProxy` behaviour (fail, deploy the proxy, retry) as a
  follow-up.
