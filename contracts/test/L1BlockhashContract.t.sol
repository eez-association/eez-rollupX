// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Test} from "forge-std/Test.sol";
import {L1BlockhashContract} from "../src/L2/L1BlockhashContract.sol";

contract L1HistoryFixture is L1BlockhashContract {
    constructor(uint64 first, uint64 cutoff, uint64 period, uint64 delay, uint64 bound) {
        firstNumber = first;
        endpoint = first;
        hashes[first] = keccak256(abi.encode(first));
        firstCutoff = cutoff;
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

    function testFutureHashReturnsZeroAndBecomesAvailableAfterImport() public {
        assertEq(history.getL1BlockHash(100), bytes32(0));
        assertEq(history.getL1BlockHash(type(uint64).max), bytes32(0));
        vm.prank(system);
        history.importHistory(120, list(1));
        assertEq(history.getL1BlockHash(100), keccak256(abi.encode(uint256(100))));
        assertEq(history.getL1BlockHash(101), bytes32(0));
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
        // Omitted updates make no call and preserve the imported endpoint.
        (number, hash) = history.current();
        assertEq(number, 102);
        assertEq(hash, history.getL1BlockHash(102));
        assertEq(history.getL1BlockHash(99), keccak256(abi.encode(uint64(99))));
    }

    function testBeforeBootstrapReturnsZero() public view {
        assertEq(history.getL1BlockHash(0), bytes32(0));
        assertEq(history.getL1BlockHash(98), bytes32(0));
        assertEq(history.getL1BlockHash(99), keccak256(abi.encode(uint64(99))));
    }

    function testRepeatedFutureLookupsWaitThroughOmittedUpdates() public {
        for (uint256 i; i < 3; ++i) {
            assertEq(history.getL1BlockHash(100), bytes32(0));
            assertEq(history.getL1BlockHash(100), bytes32(0));
            (uint64 number, bytes32 hash) = history.current();
            assertEq(number, 99);
            assertEq(hash, keccak256(abi.encode(uint64(99))));
            vm.warp(138 + i * 12);
        }
        vm.prank(system);
        history.importHistory(156, list(3));
        assertEq(history.getL1BlockHash(100), keccak256(abi.encode(uint256(100))));
        assertEq(history.getL1BlockHash(102), keccak256(abi.encode(uint256(102))));
        assertEq(history.getL1BlockHash(103), bytes32(0));
    }

    function testCoveredMissingHashStillReverts() public {
        vm.prank(system);
        history.importHistory(120, list(3));
        vm.store(address(history), keccak256(abi.encode(uint64(100), uint256(0))), bytes32(0));
        vm.expectRevert(abi.encodeWithSelector(L1BlockhashContract.L1BlockNotImported.selector, uint64(100)));
        history.getL1BlockHash(100);
    }

    function testInvalidEndpointCannotLookLikeTemporaryUnavailability() public {
        vm.store(address(history), keccak256(abi.encode(uint64(99), uint256(0))), bytes32(0));
        vm.expectRevert(L1BlockhashContract.InvalidHistoryState.selector);
        history.getL1BlockHash(100);
    }

    function testInvalidRangeCannotLookLikeTemporaryUnavailability() public {
        vm.store(address(history), bytes32(uint256(1)), bytes32(uint256(100)));
        vm.expectRevert(L1BlockhashContract.InvalidHistoryState.selector);
        history.getL1BlockHash(101);
        vm.store(address(history), bytes32(uint256(2)), bytes32(uint256(type(uint64).max) + 1));
        vm.expectRevert(L1BlockhashContract.InvalidHistoryState.selector);
        history.getL1BlockHash(type(uint64).max);
    }

    function testFuzzFutureHashIsZero(uint64 number) public view {
        number = uint64(bound(number, 100, type(uint64).max));
        assertEq(history.getL1BlockHash(number), bytes32(0));
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

    function testEmptyImportRejectsAndLaterEligibleImportNeedsNoIntermediateCalls() public {
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.InvalidHistoryImportLength.selector);
        history.importHistory(120, new bytes32[](0));
        vm.warp(162);
        vm.prank(system);
        history.importHistory(156, list(1));
        (uint64 number,) = history.current();
        assertEq(number, 100);
        assertEq(uint256(vm.load(address(history), bytes32(uint256(3)))), 120);
        vm.warp(163);
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.InvalidHistoryPosition.selector);
        history.importHistory(157, list(1));
    }

    function testMalformedAndOversizedImportsRollback() public {
        bytes32[] memory invalid = list(2);
        invalid[1] = bytes32(0);
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.InvalidHistoryHash.selector);
        history.importHistory(120, invalid);
        (uint64 number,) = history.current();
        assertEq(number, 99);
        assertEq(history.getL1BlockHash(100), bytes32(0));
        assertEq(vm.load(address(history), keccak256(abi.encode(uint64(100), uint256(0)))), bytes32(0));
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.InvalidHistoryImportLength.selector);
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
        assertEq(history.getL1BlockHash(100), bytes32(0));
        assertEq(vm.load(address(history), keccak256(abi.encode(uint64(100), uint256(0)))), bytes32(0));
    }

    function testEndpointOverflowRejectsAndFinalEligibleCutoffNeedsNoSuccessor() public {
        L1HistoryFixture full = new L1HistoryFixture(type(uint64).max, 120, 12, 6, 64);
        vm.prank(system);
        vm.expectRevert(L1BlockhashContract.InvalidHistoryState.selector);
        full.importHistory(120, list(1));
        assertEq(full.getL1BlockHash(type(uint64).max), keccak256(abi.encode(type(uint64).max)));
        uint64 lastCutoff = type(uint64).max - 6;
        full = new L1HistoryFixture(99, lastCutoff, 12, 6, 64);
        vm.warp(type(uint64).max);
        vm.prank(system);
        full.importHistory(lastCutoff, list(1));
        (uint64 number,) = full.current();
        assertEq(number, 100);
    }
}
