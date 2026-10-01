// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Script, console} from "forge-std/Script.sol";

import {EEZ} from "eez-core-protocol/src/EEZ.sol";
import {EEZProxy} from "eez-core-protocol/src/proxy/EEZProxy.sol";

/// @title DeployEEZ
/// @notice Deploys the central `EEZ.sol` registry on L1 behind an `EEZProxy`,
///         following the pinned protocol's `deployment/Deploy.s.sol`.
///
///         EEZ has no initializer: the recovery address and the cross-chain
///         proxy init-code hash are implementation immutables, and the initial
///         persistent state is all zero, so the proxy starts uninitialized.
///         Every integration uses the **proxy** address: it holds the registry
///         storage, emits its events and is `address(this)` for cross-chain
///         proxy CREATE2 addresses. The proxy creates a ProxyAdmin owned by
///         `upgradeOwner`; upgrades must keep `RECOVERY_ADDRESS` and the proxy
///         init-code hash (the protocol's `deployment/Upgrade.s.sol` checks both).
///
///         Outputs: EEZ_IMPLEMENTATION=<address>, EEZ_PROXY_ADMIN=<address>,
///                  EEZ=<proxy address> (printed last)
///
///         Call shape:
///           forge script ... DeployEEZ
///             --sig "run(address,address)" $RECOVERY_ADDRESS $UPGRADE_OWNER
///             --private-key $PK
///
///         The private key must be funded on L1.
///         Default deployment via `forge script ... --broadcast`.
contract DeployEEZ is Script {
    /// ERC-1967 admin slot: the ProxyAdmin contract the proxy created.
    bytes32 internal constant ADMIN_SLOT = bytes32(uint256(keccak256("eip1967.proxy.admin")) - 1);

    function run(address recoveryAddress, address upgradeOwner) external {
        require(recoveryAddress != address(0) && upgradeOwner != address(0), "Zero address");
        vm.startBroadcast();
        EEZ implementation = new EEZ(recoveryAddress);
        EEZ eez = EEZ(address(new EEZProxy(address(implementation), upgradeOwner)));
        vm.stopBroadcast();
        console.log("EEZ_IMPLEMENTATION=%s", address(implementation));
        console.log("EEZ_PROXY_ADMIN=%s", address(uint160(uint256(vm.load(address(eez), ADMIN_SLOT)))));
        console.log("EEZ=%s", address(eez));
    }
}
