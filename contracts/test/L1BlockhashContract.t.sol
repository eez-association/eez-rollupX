// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Test} from "forge-std/Test.sol";
import {L1BlockhashContract} from "../src/L2/L1BlockhashContract.sol";

contract L1HistoryFixture is L1BlockhashContract {
    constructor(uint64 first, uint64 cutoff, uint64 period, uint64 delay, uint64 bound) {
        firstNumber = first;
        endpoint = first;
        hashes[first] = keccak256(abi.encode(first));
        nextCutoff = cutoff;
        interval = period;
        updateDelay = delay;
        maxImportHeaders = bound;
    }
}

contract L1BlockhashContractTest is Test {
    L1HistoryFixture internal history;
    address internal system;

    function setUp() public {
        history = new L1HistoryFixture(99, 120, 12, 6, 64);
        system = history.SYSTEM_ADDRESS();
        vm.warp(126);
    }

    function list(uint256 count) internal pure returns (bytes32[] memory values) {
        values = new bytes32[](count);
        for (uint256 i; i < count; ++i) {
            values[i] = keccak256(abi.encode(100 + i));
        }
    }

    function testHistoryIncludesNeverSelectedBlocksAndPersists() public {
        vm.prank(system);
        history.importHistory(120, list(3));
        (uint64 number, bytes32 hash) = history.current();
        assertEq(number, 102);
        assertEq(hash, keccak256(abi.encode(uint256(102))));
        for (uint64 i = 99; i <= 102; ++i) {
            assertEq(history.getL1BlockHash(i), keccak256(abi.encode(i)));
        }
        vm.warp(138);
        vm.prank(system);
        history.importHistory(132, new bytes32[](0));
        (number, hash) = history.current();
        assertEq(number, 102);
        assertEq(hash, history.getL1BlockHash(102));
        assertEq(history.getL1BlockHash(99), keccak256(abi.encode(uint64(99))));
    }

    function testAbsentHistoryReverts() public {
        vm.expectRevert(abi.encodeWithSelector(L1BlockhashContract.L1BlockNotImported.selector, uint64(98)));
        history.getL1BlockHash(98);
        vm.expectRevert(abi.encodeWithSelector(L1BlockhashContract.L1BlockNotImported.selector, uint64(100)));
        history.getL1BlockHash(100);
    }

    function testUnauthorizedAndWrongPositionReject() public {
        vm.expectRevert(L1BlockhashContract.UnauthorizedHistoryImport.selector);
        history.importHistory(120, list(1));
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.InvalidHistoryPosition.selector);
        history.importHistory(132, list(1));
        vm.warp(125);
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.InvalidHistoryPosition.selector);
        history.importHistory(120, list(1));
    }

    function testEmptyImportAdvancesAndDuplicateRejects() public {
        vm.prank(system);
        history.importHistory(120, new bytes32[](0));
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.InvalidHistoryPosition.selector);
        history.importHistory(120, new bytes32[](0));
        vm.warp(138);
        vm.prank(system);
        history.importHistory(132, list(1));
        (uint64 number,) = history.current();
        assertEq(number, 100);
    }

    function testMalformedAndOversizedImportsRollback() public {
        bytes32[] memory invalid = list(2);
        invalid[1] = bytes32(0);
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.InvalidHistoryHash.selector);
        history.importHistory(120, invalid);
        (uint64 number,) = history.current();
        assertEq(number, 99);
        vm.expectRevert(abi.encodeWithSelector(L1BlockhashContract.L1BlockNotImported.selector, uint64(100)));
        history.getL1BlockHash(100);
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.HistoryImportTooLarge.selector);
        history.importHistory(120, list(65));
        vm.prank(system);
        history.importHistory(120, list(64));
        (number,) = history.current();
        assertEq(number, 163);
    }

    function testConflictingSlotRejectsAndRollsBack() public {
        bytes32 slot = keccak256(abi.encode(uint64(101), uint256(0)));
        vm.store(address(history), slot, keccak256(abi.encode(uint256(101))));
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.InvalidHistoryHash.selector);
        history.importHistory(120, list(2));
        vm.expectRevert(abi.encodeWithSelector(L1BlockhashContract.L1BlockNotImported.selector, uint64(100)));
        history.getL1BlockHash(100);
    }

    function testEndpointAndCutoffOverflowReject() public {
        L1HistoryFixture full = new L1HistoryFixture(type(uint64).max, 120, 12, 6, 64);
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.InvalidHistoryState.selector);
        full.importHistory(120, list(1));
        uint64 lastCutoff = type(uint64).max - 6;
        full = new L1HistoryFixture(99, lastCutoff, 12, 6, 64);
        vm.warp(type(uint64).max);
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.InvalidHistoryState.selector);
        full.importHistory(lastCutoff, new bytes32[](0));
    }
}
