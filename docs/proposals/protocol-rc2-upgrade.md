# Protocol v0.1.0-rc.2: pin, contract upgrades and pending scenarios

| Field | Value |
| --- | --- |
| Design status | Draft. Not accepted; nothing implemented or deployed. |
| Date | 2026-10-08 |
| Source baseline | `eduadiez/rollupX` `main` at `17ff5fc` |
| Protocol baseline | Current pin `v0.1.0-rc.1` = `855fe0602484750861b4c31502a1a91355d33ed6`; target `v0.1.0-rc.2` = `9a1f815336c20d28e5ec5cecbf195c41fbd5ab0f` (tag object `2866c79`, a descendant of the pin) |
| Related work | Protocol `deployment/README.md`, `deployment/upgrade-mainnet.sh`, `deployment/MainnetUpgrade.s.sol` at rc.2; [STATIC_ENTRY.md](../../eez-core-protocol/docs/STATIC_ENTRY.md) §4.2–4.3; [L2 static execution position](l2-static-execution-position.md); [deployed genesis records](../../deployments/README.md) |

## Problem and scope

1. **Pin `v0.1.0-rc.2`** in this repository, for our code and tests.
2. **Upgrade the deployed contracts in place** to rc.2: Ethereum mainnet (L2
   696990), Chiado `eez.asuscomm.com` (906969) and Chiado `eez.dev` (699069).
   New deployments use regenerated genesis files.
3. **Fix the pending protocol scenarios** our composer does not pass.

### What rc.2 changes

- **Core contracts:** one change since rc.1. `_validateCallDestination` /
  `EEZDestinationForbidden` in `EEZ`, `EEZL2` and `EEZBase`: a forwarded
  cross-chain call may not target the manager, which closes re-entry under an
  authorized proxy's identity. Structs, hashes, events, storage layout,
  `CrossChainProxy`, `Rollup`, the proof system and compiler settings are
  unchanged. rc.2's contract sources equal reviewed commit `9744950`.
- **Mainnet upgrade tooling** (`deployment/upgrade-mainnet.sh` +
  `MainnetUpgrade.s.sol`), written for our mainnet (chains 1 and 696990). It
  pins the old and target implementation code hashes and checks
  `PROXY_INIT_CODE_HASH` and the `ProxyAdmin` owner. It simulates by default and
  broadcasts only with `--broadcast --traffic-paused`, keeping a resume journal
  and verifying afterwards.
- **E2E suite:** four new static round-trip scenarios, and the runner scripts
  moved (`run/local.sh` → `run/local/parallel.sh`, `run/network-*.sh` →
  `run/network/*.sh`, `run/prepare-network.sh` → `run/network/setup.sh`).

### Verified facts (2026-10-08)

- **Mainnet matches the upstream runner's pinned "old" state exactly** (recorded
  in [`deployments/ethereum-696990`](../../deployments/ethereum-696990/README.md)):
  - both managers at `0x4b27…402c`;
  - implementation code hashes `0x16c4c81a…` (L1) and `0x432e1c7a…` (L2);
  - `PROXY_INIT_CODE_HASH` `0x26f777…`;
  - the same `ProxyAdmin` and owner (`0xceaf…a141`) on both chains.

  The owner holds about 0.33 ETH on L1 and 0.01 ETH on L2.
- The contracts at rc.1 already support the static round-trip tables, so static
  callback support does not depend on the contract upgrade.
- No node, signer or deriver code pins the managers' code. Binaries built at rc.2
  work before and after each contract upgrade, so there is no activation height.
  The node and signer still upgrade together, as they do today.
- An in-place upgrade never changes a network's genesis. Mainnet's and
  `eez.asuscomm.com`'s genesis are recorded and verified under
  [`deployments/`](../../deployments/README.md). `eez.dev`'s must be added before
  its upgrade.

## Pending scenarios

| Scenario (rc.2) | In upstream `all` | Status on our composer | Plan |
| --- | --- | --- | --- |
| `topLevelStaticReentrantCounter` (L1→L2→L1 static) | yes | rejected: "static cross-chain calls with descendants are not supported" (seen on `eez.asuscomm.com`, tx `0xb772…3666`) | **Part A** |
| `topLevelStaticReentrantCounterL2` (L2→L1→L2 static) | yes | same rejection (tx `0x0214…8981`) | **Part A** |
| `topLevelStaticReentrantMissingProxy` / `…L2` | no (`E2E_EXCLUDE_FROM_ALL`, upstream "not ready") | not supported | Follow-up after upstream marks them ready |
| `staticLocalWrite`, `staticLocalWriteL2`, `nestedStaticLocalWriteL1/L2` | no (upstream "not live yet") | not run | Follow-up, same condition |
| `revertCounterL2`, `revertFromOtherChainL2` | yes | intermittent CI failure after a builder exclusion: the node replaces the optimistic Sync block (designed behaviour), but the harness verifies L2 tables in the replaced block using its first receipt | **Part D**: upstream harness fix |

Moving the pin makes our Kurtosis lane run the two `topLevelStaticReentrant…`
scenarios, which are in `all`. **Part A must merge before or with the pin bump.**

## Requirements and acceptance

| ID | Requirement | Acceptance check |
| --- | --- | --- |
| R1 | An L1 static read whose L2 target makes one static callback to L1 settles as one L1 static entry whose `l2ToL1Calls` carry the callback and whose `rollingHash` is the untagged static fold. Nothing is delivered or loaded on L2. | `topLevelStaticReentrantCounter` passes in Kurtosis; a process test with follower replay; the read key is absent from L2. |
| R2 | An L2 static read whose L1 target makes one static callback to L2 settles as one L2 static entry whose `incomingCalls` run the callback, plus one L1 zero-hash entry with a static row keyed right after `CALL_BEGIN`. | `topLevelStaticReentrantCounterL2` passes in Kurtosis; a process test with follower replay. |
| R3 | Only that shape is accepted: depth 1; the callback is static, successful, zero value and gas, no revert span, and targets the source chain; the parent is a top-level static call. | Unit tests for each rejected variant. |
| R4 | The L1 row result is bound to the result the L2 callback produces; the signer recomputes it independently. | Verifier tamper tests. |
| R5 | A callback whose source proxy is not deployed is rejected at admission with a clear error. | Unit or process test. |
| R6 | The repository pins rc.2: genesis templates, validator revision, CI runner paths, specs and fixture provenance are updated. | `update-eezl2-genesis.sh --check`; protocol core tests; Anvil runner; full CI including Kurtosis. |
| R7 | The composer rejects calls whose destination is the target chain's manager, before and after the upgrade. | Unit tests per direction and for static sub-calls. |
| R8 | Wallet gas estimation recognises both the rc.1 and the rc.2 registry implementation. | Estimator unit test. |
| R9 | Each network is upgraded in place, with preserved proxy addresses and recorded evidence. | The checklist in Part C; a record in `deployments/<network>/README.md`. |
| R10 | Recorded genesis files never change. | Review and recorded SHA-256. |

## Design

### Part A: top-level static callbacks (independent of the pin)

All logic is shared code in `eez-protocol`, so the composer and the signer stay
in agreement.

**Emission** (`entries/materialize.rs`):

- `push_static_entry` (L1-triggered): fill `l2ToL1Calls` from the static root's
  children and set `rollingHash` with the existing `StaticCallRollingHash`
  (seed 0, `keccak(prev ‖ success ‖ returnData)`).
- `push_l2_static_entry` (L2-triggered): fill `incomingCalls` the same way. The
  L1 host reuses the existing machinery for nested static rows (row keyed by
  `keccak(callHash ‖ hostHash)` at `CALL_BEGIN`). A static child of a static root
  must not open an L2 delivery entry.

**Validation:**

- `ensure_materializable_calls`: allow descendants only for a top-level static
  call whose children satisfy R3.
- `system_tx.rs` shape checks: allow static incoming calls on L2 static entries;
  require gas 0 on static sub-calls.
- `entries/verify.rs`: allow a non-flat host with static rows of static sources.
  Each L2 incoming call must match the next row at its position key, and the row
  results must fold to the L2 `rollingHash` (R4). Existing checks reject
  unconsumed or extra rows.
- Settlement observation (`settlement.rs`, `eez-driver/src/observation.rs`):
  answer the expected callbacks from the claim after checking frame, caller,
  target and calldata, and require every callback to be consumed.

**Admission (R5):** check that the callback's source proxy exists on the chain
that executes the static entry. Simulation cannot observe this: an undeployed
proxy is not intercepted and returns empty success, while the chain reverts with
`StaticCallProxyNotDeployed`.

**DA:** unchanged. Blobs carry static calls and their children as a semantic
forest, and followers re-materialize and byte-compare. Old signers and followers
fail closed on the new shapes.

### Part B: pin rc.2 in this repository

- Move the gitlink to `9a1f815`. Regenerate the genesis templates
  (`scripts/update-eezl2-genesis.sh`, then `--check`). Update the validator
  revision (`protocol=9a1f815…`), spec headers, `AGENTS.md` pin notes and
  fixture provenance (re-capture is optional).
- **CI runners:** update `scripts/ci/run-protocol-anvil.sh`,
  `scripts/ci/run-protocol-network.py`, `testing/kurtosis/scripts/run-protocol-e2e.sh`
  and the staged Chiado runner to the rc.2 runner paths and arguments. The
  unit tests in `scripts/ci/tests` cover their contracts.
- Composer guard for manager destinations (R7), decoding
  `EEZDestinationForbidden` for diagnostics.
- Estimator registry fingerprints as a list containing rc.1 and rc.2 (R8).

### Part C: upgrade the deployed contracts

Order: `eez.asuscomm.com`, then `eez.dev`, then mainnet. Ship the Part A + B
images first; they work with the old contracts.

**Mainnet: use upstream's reviewed runner unchanged.**

1. **Simulate now (read-only):**
   `bash deployment/upgrade-mainnet.sh --l1-rpc … --l2-rpc … --l1-manager 0x4b27…402c --l2-manager 0x4b27…402c --owner 0xceaf…a141`
   This checks both chains, the owner, the old hashes and `PROXY_INIT_CODE_HASH`,
   and simulates the deployment and upgrade with no key. Run it from an rc.2
   checkout.
2. **Pause traffic, as the runner requires:**
   - close the composer fronts at the gateway;
   - let pending cross-chain work settle or expire (`eez_getCrossChainTransaction`
     and the queue metrics show none owned);
   - keep the node running, because the L2 upgrade is an ordinary L2 transaction.
3. **Broadcast:** `--broadcast --traffic-paused`, entering the owner key privately.
   The runner deploys each implementation and calls
   `ProxyAdmin.upgradeAndCall(manager, impl, "")` on L1, then L2, with a resume
   journal, and verifies the target code hashes.
4. **Verify:**
   - the cross-chain smoke test and the two static scenarios;
   - a wallet estimate;
   - record the transaction hashes and new implementation addresses in
     `deployments/ethereum-696990/README.md`;
   - reopen the fronts.

**Chiado networks.** The mainnet runner refuses other chain IDs, and its pinned
hashes include mainnet's immutables. Use upstream's generic
`deployment/Upgrade.s.sol` (`UpgradeEEZ`, plus the documented L2 step) with the
same preflight values read from each network:
- the old implementation code hash recorded beforehand;
- an unchanged `PROXY_INIT_CODE_HASH`;
- the owner;
- the new implementation's immutables copied from the current one.

If upstream accepts it, propose parameterising `upgrade-mainnet.sh` by chain ID
and expected hashes, so all three networks use one reviewed path.

**Rollback:** the storage layout is unchanged, so each `ProxyAdmin` can point
back to the previous implementation. Binaries roll back to the previous image,
node and signer together.

### Part D: replaced-Sync-block harness flake

The rc.1/rc.2 harness (`script/e2e/shared/Verify.s.sol`) verifies L2 tables in
the block from the trigger's first receipt. After a builder exclusion the node
replaces that optimistic Sync block and re-includes the transaction later, so
the check fails although settlement succeeds. Propose upstream: re-read the
receipt (or resolve the transaction's canonical block) before L2 table
verification. Until it lands, treat this failure as a known flake and confirm
its signature (one excluded bundle, a replaced Sync block) before re-running.

## Alternatives considered

- **Redeploy every network at rc.2.** Rejected for the existing networks; still
  the path for new ones.
- **Our own mainnet upgrade script.** Rejected: upstream's runner is reviewed,
  pins exactly our mainnet's state, and enforces the traffic pause.
- **Static callbacks only after the contract upgrade.** Unnecessary: rc.1 already
  supports them.
- **Static trees of any depth.** Deferred; depth 1 covers the scenarios.

## Validation plan

- **Unit:**
  - materialization per direction with hand-computed hashes;
  - guard, shape and verifier tamper cases;
  - the observation inspector;
  - blob round trip and reconstruction;
  - signer semantics and outbound;
  - the estimator with both fingerprints;
  - the manager-destination guard;
  - the CI runner tests in `scripts/ci/tests`.
- **Process (`eez-node-e2e`, serial):**
  - one test per static direction, each with follower replay;
  - depth-2 and missing-proxy rejections;
  - the existing `cross_chain` and `chained_interstate` suites.
- **Protocol:** core tests, the Anvil runner and the Kurtosis suite at rc.2, with
  every eligible scenario, including both `topLevelStaticReentrant…` scenarios.
- **Operations:**
  - the mainnet runner in simulation mode before any broadcast;
  - the `eez.asuscomm.com` rehearsal;
  - evidence recorded per network.

## Implementation and follow-up

Status: draft, not implemented.

1. **PR 1, Part A**, at the current pin, with its regressions.
2. **PR 2, Part B** (pin rc.2), merged after PR 1. CI, including Kurtosis, must
   pass with the new scenarios.
3. **Upstream:** the Part D harness fix; optionally a chain-parameterised upgrade
   runner.
4. **Operations:**
   - the mainnet runner in simulation mode;
   - upgrade `eez.asuscomm.com`;
   - archive `eez.dev`'s genesis and upgrade it;
   - the mainnet maintenance window.

Open questions:

- How long a traffic pause on mainnet is acceptable, and whether the gateway is
  the right place to pause.
- Whether the L2 owner's 0.01 ETH covers the L2 implementation deployment; the
  simulation answers this.
- When upstream expects the `…MissingProxy` and `staticLocalWrite` scenarios to
  be ready.
