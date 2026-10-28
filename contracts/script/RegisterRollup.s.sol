// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Script, console} from "forge-std/Script.sol";

import {EEZ} from "eez-core-protocol/src/EEZ.sol";
import {Rollup} from "eez-core-protocol/src/rollupContract/Rollup.sol";

/// @title RegisterRollup
/// @notice Calls `EEZ.registerRollup(rollupContract, initialRoot)`. The
///         registry assigns the next sequential `rollupId` (starting at
///         1 — `MAINNET_ROLLUP_ID = 0` is reserved for L1 self), emits
///         `RollupCreated(rollupId, rollupContract, initialRoot)`, and
///         calls back `Rollup.rollupContractRegistered(rollupId, registrant)` to
///         wire the new id into the manager. The broadcaster must be the
///         owner configured when the Rollup was deployed.
///
///         Outputs: L2_ROLLUP_ID=<id>
///
///         Call shape:
///           forge script ... RegisterRollup
///             --sig "run(address,address,bytes32)" $EEZ $ROLLUP_CONTRACT $INITIAL_STATE
///             --rpc-url $L1_RPC --broadcast --private-key $PK
///
///         Args:
///           - eez            — central registry proxy address.
///           - rollupContract — the per-rollup manager deployed by
///                              DeployRollup.
///           - initialRoot    — genesis block hash commitment for the new
///                              rollup. The protocol ABI calls this a root,
///                              but this deployment profile assigns block-hash
///                              semantics to the opaque bytes32 value.
contract RegisterRollup is Script {
    function run(address eez, address rollupContract, bytes32 initialRoot) external {
        require(initialRoot != bytes32(0), "Zero genesis commitment");
        // The pinned protocol's `deployment/Deploy.s.sol:RegisterRollup` checks.
        require(eez.code.length != 0 && rollupContract.code.length != 0, "Contract has no code");
        require(Rollup(rollupContract).EEZContract() == eez, "Wrong registry");
        require(Rollup(rollupContract).rollupId() == 0, "Already registered");
        vm.startBroadcast();
        (, address broadcaster,) = vm.readCallers();
        require(broadcaster == Rollup(rollupContract).owner(), "Broadcast as rollup owner");
        uint64 rollupId = EEZ(eez).registerRollup(rollupContract, initialRoot);
        vm.stopBroadcast();
        console.log("L2_ROLLUP_ID=%s", rollupId);
    }
}
