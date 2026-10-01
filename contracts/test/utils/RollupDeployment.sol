// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {EEZ} from "eez-core-protocol/src/EEZ.sol";
import {EEZProxy} from "eez-core-protocol/src/proxy/EEZProxy.sol";
import {Rollup} from "eez-core-protocol/src/rollupContract/Rollup.sol";
import {TransparentUpgradeableProxy} from "@openzeppelin/contracts/proxy/transparent/TransparentUpgradeableProxy.sol";

/// @notice Deploys the upgradeable L1 contracts the way `DeployEEZ` and
///         `DeployRollup` do. Tests must use the returned proxies.
library RollupDeployment {
    /// ERC-1967 implementation slot of both proxies.
    bytes32 internal constant IMPLEMENTATION_SLOT = bytes32(uint256(keccak256("eip1967.proxy.implementation")) - 1);

    /// @notice `EEZ` behind an uninitialized `EEZProxy`; `upgradeOwner` owns its ProxyAdmin.
    function deployEEZ(address recovery, address upgradeOwner) internal returns (EEZ) {
        return EEZ(address(new EEZProxy(address(new EEZ(recovery)), upgradeOwner)));
    }

    /// @notice A locked `Rollup` implementation behind a transparent proxy
    ///         initialized at construction. The owner also owns upgrades.
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
