// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Rollup} from "eez-core-protocol/src/rollupContract/Rollup.sol";
import {TransparentUpgradeableProxy} from "@openzeppelin/contracts/proxy/transparent/TransparentUpgradeableProxy.sol";

/// @notice Deploys an upgradeable `Rollup` the way `DeployRollup` does: a locked
///         implementation behind a transparent proxy initialized at construction.
///         The owner also owns upgrades. Tests must use the returned proxy.
library RollupDeployment {
    function deploy(
        address eez,
        address owner,
        uint256 threshold,
        address[] memory proofSystems,
        bytes32[] memory vkeys
    )
        internal
        returns (Rollup)
    {
        Rollup implementation = new Rollup(eez);
        bytes memory init = abi.encodeCall(Rollup.initialize, (owner, threshold, proofSystems, vkeys));
        return Rollup(address(new TransparentUpgradeableProxy(address(implementation), owner, init)));
    }
}
