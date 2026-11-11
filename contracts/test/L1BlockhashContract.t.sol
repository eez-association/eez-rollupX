// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Test} from "forge-std/Test.sol";
import {L1BlockhashContract} from "../src/L2/L1BlockhashContract.sol";
import {EEZProxy} from "eez-core-protocol/src/proxy/EEZProxy.sol";
import {ProxyAdmin} from "@openzeppelin/contracts/proxy/transparent/ProxyAdmin.sol";
import {ITransparentUpgradeableProxy} from "@openzeppelin/contracts/proxy/transparent/TransparentUpgradeableProxy.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";

contract L1HistoryV2 is L1BlockhashContract {
    function revision() external pure returns (uint256) {
        return 2;
    }
}

contract L1BlockhashContractTest is Test {
    bytes32 private constant ADMIN_SLOT = 0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103;
    bytes32 private constant IMPLEMENTATION_SLOT = 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;
    L1BlockhashContract internal history;
    L1BlockhashContract internal implementation;
    ProxyAdmin internal admin;
    address internal system;

    function setUp() public {
        implementation = new L1BlockhashContract();
        history = L1BlockhashContract(address(new EEZProxy(address(implementation), address(this))));
        admin = ProxyAdmin(address(uint160(uint256(vm.load(address(history), ADMIN_SLOT)))));
        system = history.SYSTEM_ADDRESS();
    }

    function pair(uint64 number) internal pure returns (L1BlockhashContract.L1Block memory) {
        return L1BlockhashContract.L1Block(number, keccak256(abi.encode(number)));
    }

    function importOne(uint64 number) internal {
        L1BlockhashContract.L1Block[] memory blocks = new L1BlockhashContract.L1Block[](1);
        blocks[0] = pair(number);
        vm.prank(system);
        history.importL1Blocks(blocks);
    }

    function upgrade(address next) internal {
        admin.upgradeAndCall(ITransparentUpgradeableProxy(address(history)), next, "");
    }

    function testInitiallyEmptyAndNoInitializer() public {
        (uint64 number, bytes32 blockHash) = history.getLatestImportedL1Block();
        assertEq(number, 0);
        assertEq(blockHash, bytes32(0));
        assertEq(history.getL1BlockHash(0), bytes32(0));
        assertEq(history.getL1BlockHash(type(uint64).max), bytes32(0));
        (bool ok,) = address(history).call(abi.encodeWithSignature("initialize()"));
        assertFalse(ok);
        (ok,) = address(implementation).call(abi.encodeWithSignature("initialize()"));
        assertFalse(ok);
        (number, blockHash) = implementation.getLatestImportedL1Block();
        assertEq(number, 0);
        assertEq(blockHash, bytes32(0));
    }

    function testZeroCannotBeFirstImport() public {
        vm.expectRevert(
            abi.encodeWithSelector(L1BlockhashContract.BlockNumberNotIncreasing.selector, uint64(0), uint64(0))
        );
        importOne(0);
    }

    function testIncreasingPairsGapsAndUnavailableHashes() public {
        L1BlockhashContract.L1Block[] memory blocks = new L1BlockhashContract.L1Block[](2);
        blocks[0] = pair(100);
        blocks[1] = pair(103);
        vm.prank(system);
        history.importL1Blocks(blocks);
        importOne(107);
        for (uint64 number = 99; number <= 108; ++number) {
            bytes32 expected = number == 100 || number == 103 || number == 107 ? pair(number).blockHash : bytes32(0);
            assertEq(history.getL1BlockHash(number), expected);
        }
        (uint64 latest, bytes32 blockHash) = history.getLatestImportedL1Block();
        assertEq(latest, 107);
        assertEq(blockHash, pair(107).blockHash);
    }

    function testGapsCannotBeBackfilledAndRecordsCannotBeOverwritten() public {
        importOne(103);
        for (uint64 number = 101; number <= 103; ++number) {
            vm.expectRevert(
                abi.encodeWithSelector(L1BlockhashContract.BlockNumberNotIncreasing.selector, uint64(103), number)
            );
            importOne(number);
        }
        assertEq(history.getL1BlockHash(102), bytes32(0));
        assertEq(history.getL1BlockHash(103), pair(103).blockHash);
    }

    function testUnauthorizedAndEmptyImports() public {
        L1BlockhashContract.L1Block[] memory blocks = new L1BlockhashContract.L1Block[](0);
        vm.expectRevert(abi.encodeWithSelector(L1BlockhashContract.UnauthorizedImport.selector, address(this)));
        history.importL1Blocks(blocks);
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.EmptyImport.selector);
        history.importL1Blocks(blocks);
    }

    function testInvalidLaterPairRollsBackTheWholeImport() public {
        importOne(99);
        for (uint256 mutation; mutation < 3; ++mutation) {
            L1BlockhashContract.L1Block[] memory blocks = new L1BlockhashContract.L1Block[](2);
            blocks[0] = pair(100);
            blocks[1] = pair(mutation == 0 ? 100 : mutation == 1 ? 99 : 103);
            if (mutation == 2) blocks[1].blockHash = bytes32(0);
            vm.prank(system);
            if (mutation == 2) {
                vm.expectRevert(abi.encodeWithSelector(L1BlockhashContract.InvalidBlockHash.selector, uint64(103)));
            } else {
                vm.expectRevert(
                    abi.encodeWithSelector(
                        L1BlockhashContract.BlockNumberNotIncreasing.selector, uint64(100), blocks[1].number
                    )
                );
            }
            history.importL1Blocks(blocks);
            assertEq(history.getL1BlockHash(100), bytes32(0));
            assertEq(history.getL1BlockHash(103), bytes32(0));
            (uint64 number, bytes32 blockHash) = history.getLatestImportedL1Block();
            assertEq(number, 99);
            assertEq(blockHash, pair(99).blockHash);
        }
    }

    function testMaximumNumberHasNoWraparound() public {
        importOne(type(uint64).max);
        assertEq(history.getL1BlockHash(type(uint64).max), pair(type(uint64).max).blockHash);
        vm.expectRevert(
            abi.encodeWithSelector(L1BlockhashContract.BlockNumberNotIncreasing.selector, type(uint64).max, uint64(1))
        );
        importOne(1);
    }

    function testTimingAndCountAreProtocolResponsibilities() public {
        // The contract itself accepts any timestamp and more than 64 pairs.
        // Native admission must reject these shapes independently of the EVM call.
        vm.warp(1);
        L1BlockhashContract.L1Block[] memory blocks = new L1BlockhashContract.L1Block[](65);
        for (uint64 i; i < 65; ++i) {
            blocks[i] = pair(i + 1);
        }
        vm.prank(system);
        history.importL1Blocks(blocks);
        assertEq(history.getL1BlockHash(65), pair(65).blockHash);
    }

    function testUpgradePreservesHistoryAndImports() public {
        importOne(100);
        importOne(103);
        L1HistoryV2 next = new L1HistoryV2();
        upgrade(address(next));
        assertEq(L1HistoryV2(address(history)).revision(), 2);
        assertEq(vm.load(address(history), IMPLEMENTATION_SLOT), bytes32(uint256(uint160(address(next)))));
        assertEq(history.getL1BlockHash(100), pair(100).blockHash);
        assertEq(history.getL1BlockHash(101), bytes32(0));
        assertEq(history.getL1BlockHash(103), pair(103).blockHash);
        importOne(107);
        upgrade(address(implementation));
        (uint64 number, bytes32 blockHash) = history.getLatestImportedL1Block();
        assertEq(number, 107);
        assertEq(blockHash, pair(107).blockHash);
    }

    function testOnlyProxyAdminOwnerCanUpgrade() public {
        L1HistoryV2 next = new L1HistoryV2();
        vm.prank(address(123));
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(123)));
        upgrade(address(next));
        vm.expectRevert();
        ITransparentUpgradeableProxy(address(history)).upgradeToAndCall(address(next), "");
        assertEq(vm.load(address(history), IMPLEMENTATION_SLOT), bytes32(uint256(uint160(address(implementation)))));
    }

    function testOwnershipTransferControlsFutureUpgrades() public {
        L1HistoryV2 next = new L1HistoryV2();
        admin.transferOwnership(address(123));
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(this)));
        upgrade(address(next));
        vm.prank(address(123));
        upgrade(address(next));
        assertEq(L1HistoryV2(address(history)).revision(), 2);
    }

    function testEmptyImplementationRejected() public {
        vm.expectRevert(abi.encodeWithSignature("ERC1967InvalidImplementation(address)", address(123)));
        upgrade(address(123));
    }

    function testFuzzPositiveFirstNumberAndUnavailableNeighbors(uint64 number) public {
        number = uint64(bound(number, 1, type(uint64).max));
        importOne(number);
        assertEq(history.getL1BlockHash(number), pair(number).blockHash);
        assertEq(history.getL1BlockHash(number - 1), bytes32(0));
        if (number < type(uint64).max) assertEq(history.getL1BlockHash(number + 1), bytes32(0));
    }
}
