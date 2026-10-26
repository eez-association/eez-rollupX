// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Script, console} from "forge-std/Script.sol";
import {Vm} from "forge-std/Vm.sol";

import {EEZL2} from "../src/L2/EEZL2.sol";
import {L1BlockhashContract} from "../src/L2/L1BlockhashContract.sol";
import {FeeVault} from "../src/L2/FeeVault.sol";
import {TransparentUpgradeableProxy} from "@openzeppelin/contracts/proxy/transparent/TransparentUpgradeableProxy.sol";

/// @notice Prints the genesis accounts of an EEZL2 behind an `EEZProxy`, as the
///         protocol's `deployment/Deploy.s.sol:_deployL2` would leave them, but
///         at fixed addresses.
/// @dev The implementation runtime is address-independent, so it is copied to
///      `implementation`. The proxy constructor runs at `proxy` with nonce 1, as
///      for a contract created by a transaction, so its ProxyAdmin is
///      `CREATE(proxy, 1)` and the proxy's admin immutable and ERC-1967 slots
///      are those of a real deployment. Every storage write is recorded and
///      printed; the caller validates the complete account set.
///      Output lines: `GENESIS_ACCOUNT <address> <nonce> <code>` and
///      `GENESIS_STORAGE <address> <slot> <value>`.
contract GenerateEEZL2Genesis is Script {
    function run(
        uint64 rollupId,
        address systemAddress,
        bool useGasLeft,
        address recoveryAddress,
        address upgradeOwner,
        address proxy,
        address implementation,
        address feeVault,
        address feeVaultImplementation,
        address treasuryOwner,
        address treasuryRecipient
    ) external {
        require(proxy.code.length == 0 && implementation.code.length == 0, "Target already has code");
        require(upgradeOwner != address(0), "Zero upgrade owner");

        console.log("L1_HISTORY_RUNTIME", vm.toString(type(L1BlockhashContract).runtimeCode));

        EEZL2 template = new EEZL2(rollupId, systemAddress, useGasLeft, recoveryAddress);
        vm.etch(implementation, address(template).code);
        vm.setNonce(implementation, 1);

        vm.etch(proxy, abi.encodePacked(vm.getCode("EEZProxy.sol:EEZProxy"), abi.encode(implementation, upgradeOwner)));
        vm.setNonce(proxy, 1);
        vm.startStateDiffRecording();
        (bool ok, bytes memory proxyRuntime) = proxy.call("");
        Vm.AccountAccess[] memory accesses = vm.stopAndReturnStateDiff();
        require(ok, "EEZProxy constructor failed");
        vm.etch(proxy, proxyRuntime);

        address proxyAdmin = vm.computeCreateAddress(proxy, 1);
        require(proxyAdmin.code.length != 0, "ProxyAdmin was not created at CREATE(proxy, 1)");

        EEZL2 manager = EEZL2(proxy);
        require(manager.ROLLUP_ID() == rollupId, "ROLLUP_ID immutable mismatch");
        require(manager.SYSTEM_ADDRESS() == systemAddress, "SYSTEM_ADDRESS immutable mismatch");
        require(manager.USE_GAS_LEFT() == useGasLeft, "USE_GAS_LEFT immutable mismatch");
        require(manager.RECOVERY_ADDRESS() == recoveryAddress, "RECOVERY_ADDRESS immutable mismatch");

        _account(implementation);
        _account(proxy);
        _account(proxyAdmin);
        _storage(accesses);

        require(
            feeVault.code.length == 0 && feeVaultImplementation.code.length == 0, "Fee vault target already has code"
        );
        vm.etch(feeVaultImplementation, type(FeeVault).creationCode);
        vm.setNonce(feeVaultImplementation, 1);
        vm.startStateDiffRecording();
        (bool implementationOk, bytes memory implementationRuntime) = feeVaultImplementation.call("");
        Vm.AccountAccess[] memory implementationAccesses = vm.stopAndReturnStateDiff();
        require(implementationOk, "FeeVault implementation constructor failed");
        vm.etch(feeVaultImplementation, implementationRuntime);
        _account(feeVaultImplementation);
        _storage(implementationAccesses);

        vm.etch(
            feeVault,
            abi.encodePacked(
                type(TransparentUpgradeableProxy).creationCode,
                abi.encode(
                    feeVaultImplementation,
                    treasuryOwner,
                    abi.encodeCall(FeeVault.initialize, (treasuryOwner, treasuryRecipient))
                )
            )
        );
        vm.setNonce(feeVault, 1);
        vm.startStateDiffRecording();
        (bool vaultOk, bytes memory vaultRuntime) = feeVault.call("");
        Vm.AccountAccess[] memory vaultAccesses = vm.stopAndReturnStateDiff();
        require(vaultOk, "FeeVault constructor failed");
        vm.etch(feeVault, vaultRuntime);
        require(FeeVault(payable(feeVault)).owner() == treasuryOwner, "FeeVault owner mismatch");
        require(FeeVault(payable(feeVault)).recipient() == treasuryRecipient, "FeeVault recipient mismatch");
        require(FeeVault(payable(feeVault)).pendingOwner() == address(0), "FeeVault pending owner mismatch");
        require(FeeVault(payable(feeVault)).pendingRecipient() == address(0), "FeeVault pending recipient mismatch");
        require(FeeVault(payable(feeVault)).totalWithdrawn() == 0, "FeeVault withdrawal counter mismatch");
        require(FeeVault(payable(feeVault)).minWithdrawalAmount() == 0, "FeeVault threshold mismatch");
        address vaultAdmin = vm.computeCreateAddress(feeVault, 1);
        require(vaultAdmin.code.length != 0, "FeeVault ProxyAdmin missing");
        _account(feeVault);
        _account(vaultAdmin);
        _storage(vaultAccesses);
    }

    function _storage(Vm.AccountAccess[] memory accesses) private view {
        for (uint256 i; i < accesses.length; ++i) {
            Vm.StorageAccess[] memory slots = accesses[i].storageAccesses;
            for (uint256 j; j < slots.length; ++j) {
                if (slots[j].isWrite && !slots[j].reverted) {
                    // Initializers write their guard slot repeatedly. Emit each
                    // unique slot once, with its final value, for exact validation.
                    bool seen;
                    for (uint256 a; a <= i; ++a) {
                        uint256 end = a == i ? j : accesses[a].storageAccesses.length;
                        for (uint256 b; b < end; ++b) {
                            Vm.StorageAccess memory prior = accesses[a].storageAccesses[b];
                            if (
                                prior.isWrite && !prior.reverted && prior.account == slots[j].account
                                    && prior.slot == slots[j].slot
                            ) {
                                seen = true;
                            }
                        }
                    }
                    if (seen) continue;
                    console.log(
                        string.concat(
                            "GENESIS_STORAGE ",
                            vm.toString(slots[j].account),
                            " ",
                            vm.toString(slots[j].slot),
                            " ",
                            vm.toString(vm.load(slots[j].account, slots[j].slot))
                        )
                    );
                }
            }
        }
    }

    function _account(address account) private view {
        console.log(
            string.concat(
                "GENESIS_ACCOUNT ",
                vm.toString(account),
                " ",
                vm.toString(uint256(vm.getNonce(account))),
                " ",
                vm.toString(account.code)
            )
        );
    }
}
