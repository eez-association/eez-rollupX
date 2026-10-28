// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Script, console} from "forge-std/Script.sol";

import {RollupX} from "../src/RollupX.sol";
import {TransparentUpgradeableProxy} from "@openzeppelin/contracts/proxy/transparent/TransparentUpgradeableProxy.sol";

/// @title DeployRollup
/// @notice Deploys the per-rollup `IRollupContract` manager (reference
///         `RollupX.sol`) configured for a single-PS, threshold-1 setup:
///         the workspace-local `ECDSAProofSystem` with vkey =
///         `bytes32(uint256(uint160(authorizedSigner)))`.
///
///         `Rollup` is upgradeable: its implementation locks itself, so
///         operational state lives in a transparent proxy initialized
///         atomically at construction, as in the pinned protocol's
///         `deployment/Deploy.s.sol`. `upgradeOwner` owns the proxy's
///         ProxyAdmin and may differ from the operational `owner`; register
///         and use the proxy address.
///
///         Outputs: ROLLUP_CONTRACT=<proxy address>,
///                  ROLLUP_IMPLEMENTATION=<address>, ROLLUP_PROXY_ADMIN=<address>
///
///         Call shape:
///           forge script ... DeployRollup
///             --sig "run(address,address,address,address,address,bytes)"
///                   $EEZ $ECDSA_PS $AUTHORIZED_SIGNER $OWNER $UPGRADE_OWNER $ENCODED_PROFILE
///             --rpc-url $L1_RPC --broadcast --private-key $PK
///
///         Args:
///           - eez                — central registry **proxy** address
///                                  (from DeployEEZ). The Rollup binds it
///                                  immutably, so never pass the
///                                  implementation.
///           - proofSystem        — workspace-local proof-system address
///                                  (from DeployECDSAProofSystem).
///           - authorizedSigner   — the same value passed to
///                                  DeployECDSAProofSystem; its address
///                                  becomes the vkey membership ticket
///                                  the registry stores.
///           - owner              — owner of the Rollup manager. The owner
///                                  can add / remove proof systems and
///                                  tune the threshold post-deploy.
///                                  Typically the same key driving every
///                                  other deploy (e.g.,
///                                  `cast wallet address --private-key $PK`).
///                                  Passed explicitly so the broadcaster
///                                  isn't conflated with the manager
///                                  owner — `msg.sender` inside a Foundry
///                                  `run()` is the script-runner default,
///                                  not the broadcaster.
///           - upgradeOwner       — owner of the Rollup proxy's ProxyAdmin.
contract DeployRollup is Script {
    error InvalidProofSystem();

    function run(
        address eez,
        address proofSystem,
        address authorizedSigner,
        address owner,
        address upgradeOwner,
        bytes calldata encodedProfile
    )
        external
    {
        if (proofSystem == address(0)) revert InvalidProofSystem();
        require(eez.code.length != 0, "EEZ has no code");
        require(owner != address(0) && upgradeOwner != address(0), "Zero address");
        RollupX.Profile memory profile = abi.decode(encodedProfile, (RollupX.Profile));
        require(keccak256(encodedProfile) == keccak256(abi.encode(profile)), "Noncanonical offline profile");

        address[] memory proofSystems = new address[](1);
        proofSystems[0] = proofSystem;

        bytes32[] memory vkeys = new bytes32[](1);
        // The registry treats vkey as opaque (only checks non-zero +
        // membership). Embedding the signer address as the vkey gives
        // off-chain tooling a way to read the expected signer without
        // a separate getter, and guarantees a non-zero membership
        // ticket. See `ECDSAProofSystem.sol`'s `signer()` for the
        // actual signing-address surface (the verifier reads it from
        // its own storage, not from the vkey).
        vkeys[0] = bytes32(uint256(uint160(authorizedSigner)));

        vm.startBroadcast();
        RollupX implementation = new RollupX(eez);
        bytes memory init = abi.encodeCall(RollupX.initialize, (owner, 1, proofSystems, vkeys, profile));
        RollupX rollup = RollupX(address(new TransparentUpgradeableProxy(address(implementation), upgradeOwner, init)));
        vm.stopBroadcast();
        // ERC-1967 admin slot: the ProxyAdmin contract the proxy created for `upgradeOwner`.
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
