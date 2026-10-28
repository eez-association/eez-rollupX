// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {Rollup} from "eez-core-protocol/src/rollupContract/Rollup.sol";
import {TransparentUpgradeableProxy} from "@openzeppelin/contracts/proxy/transparent/TransparentUpgradeableProxy.sol";

/// @notice Deploys the pinned Core Rollup with a single proof system and threshold one.
/// @dev The deployment tool validates the network profile and derives its verification
///      key before calling this script. Operational ownership and upgrade ownership
///      may differ. Register the proxy, never its locked implementation.
contract DeployRollup is Script {
    function run(
        address eez,
        address proofSystem,
        bytes32 verificationKey,
        address owner,
        address upgradeOwner
    )
        external
    {
        require(eez.code.length != 0 && proofSystem.code.length != 0, "Contract has no code");
        require(owner != address(0) && upgradeOwner != address(0), "Zero address");
        require(verificationKey != bytes32(0), "Zero verification key");
        address[] memory proofSystems = new address[](1);
        proofSystems[0] = proofSystem;
        bytes32[] memory vkeys = new bytes32[](1);
        vkeys[0] = verificationKey;

        vm.startBroadcast();
        Rollup implementation = new Rollup(eez);
        bytes memory init = abi.encodeCall(Rollup.initialize, (owner, 1, proofSystems, vkeys));
        Rollup rollup = Rollup(address(new TransparentUpgradeableProxy(address(implementation), upgradeOwner, init)));
        vm.stopBroadcast();
        address proxyAdmin = address(
            uint160(
                uint256(vm.load(address(rollup), 0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103))
            )
        );
        console.log("ROLLUP_CONTRACT=%s", address(rollup));
        console.log("ROLLUP_IMPLEMENTATION=%s", address(implementation));
        console.log("ROLLUP_PROXY_ADMIN=%s", proxyAdmin);
    }
}
