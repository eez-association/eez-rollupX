// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Test, Vm} from "forge-std/Test.sol";
import {L1BlockHashCollector} from "../src/L1BlockHashCollector.sol";
import {L1BlockhashContract} from "../src/L2/L1BlockhashContract.sol";

/// @dev Models the unguarded lookup observed in issue #107's transaction trace.
///      The external application's full source is not part of this repository.
contract UnguardedHistoryCollector {
    L1BlockhashContract private immutable history;
    uint256 public nextL1BlockNumber;

    constructor(L1BlockhashContract source, uint64 first) {
        history = source;
        nextL1BlockNumber = first;
    }

    function poll() external returns (bytes32 hash) {
        hash = history.getL1BlockHash(uint64(nextL1BlockNumber));
        ++nextL1BlockNumber;
    }
}

contract L1BlockHashCollectorTest is Test {
    L1BlockhashContract private constant HISTORY = L1BlockhashContract(0xeeeeEeeeEeeeEeEeEEEeEeeeeEEEEeeEEEee0077);
    L1BlockHashCollector private collector;

    function setUp() public {
        vm.etch(address(HISTORY), type(L1BlockhashContract).runtimeCode);
        vm.store(address(HISTORY), bytes32(uint256(1)), bytes32(uint256(99)));
        vm.store(address(HISTORY), bytes32(uint256(2)), bytes32(uint256(99)));
        vm.store(address(HISTORY), bytes32(uint256(3)), bytes32(uint256(120)));
        vm.store(address(HISTORY), bytes32(uint256(4)), bytes32(uint256(12)));
        vm.store(address(HISTORY), bytes32(uint256(5)), bytes32(uint256(6)));
        vm.store(address(HISTORY), bytes32(uint256(6)), bytes32(uint256(64)));
        vm.store(address(HISTORY), hashSlot(99), expectedHash(99));
        vm.warp(126);
        collector = new L1BlockHashCollector(99);
    }

    function expectedHash(uint64 number) private pure returns (bytes32) {
        return keccak256(abi.encode(number));
    }

    function hashSlot(uint64 number) private pure returns (bytes32) {
        return keccak256(abi.encode(number, uint256(0)));
    }

    function importHashes(uint64 cutoff, uint64 first, uint64 count) private {
        bytes32[] memory hashes = new bytes32[](count);
        for (uint64 i; i < count; ++i) {
            hashes[i] = expectedHash(first + i);
        }
        vm.prank(HISTORY.SYSTEM_ADDRESS());
        HISTORY.importHistory(cutoff, hashes);
    }

    function assertCollected(uint64 number) private {
        vm.expectEmit(true, false, false, true, address(collector));
        emit L1BlockHashCollector.BlockHashCollected(number, expectedHash(number));
        (bool collected, bytes32 hash) = collector.poll();
        assertTrue(collected);
        assertEq(hash, expectedHash(number));
        assertEq(collector.blockHashes(number), hash);
        assertEq(collector.nextL1BlockNumber(), uint256(number) + 1);
    }

    function assertWaiting(uint256 next) private {
        vm.recordLogs();
        (bool collected, bytes32 hash) = collector.poll();
        Vm.Log[] memory logs = vm.getRecordedLogs();
        assertFalse(collected);
        assertEq(hash, bytes32(0));
        assertEq(collector.nextL1BlockNumber(), next);
        assertEq(logs.length, 0);
    }

    function testIssue107UnguardedSecondPollRevertsButGuardedPollWaits() public {
        UnguardedHistoryCollector old = new UnguardedHistoryCollector(HISTORY, 99);
        assertEq(old.poll(), expectedHash(99));
        vm.expectRevert(abi.encodeWithSelector(L1BlockhashContract.L1BlockNotImported.selector, uint64(100)));
        old.poll();
        assertEq(old.nextL1BlockNumber(), 100);

        // Both applications see the same history and the same L2 block.
        assertCollected(99);
        assertWaiting(100);
        assertWaiting(100);
        assertEq(collector.blockHashes(99), expectedHash(99));
        assertEq(collector.blockHashes(100), bytes32(0));
    }

    function testWaitsThroughOmittedUpdateAndResumesAfterScheduledImport() public {
        assertCollected(99);
        // No history transaction is needed while the origin is unchanged.
        assertWaiting(100);

        vm.warp(137);
        assertWaiting(100);
        vm.warp(138);
        importHashes(132, 100, 1);
        assertCollected(100);
        assertWaiting(101);
    }

    function testCatchUpCollectsEveryImportedBlockOnce() public {
        importHashes(120, 100, 3);
        for (uint64 number = 99; number <= 102; ++number) {
            assertCollected(number);
        }
        assertWaiting(103);
        for (uint64 number = 99; number <= 102; ++number) {
            assertEq(collector.blockHashes(number), expectedHash(number));
        }
    }

    function testStartMustBeInsideTheImportedRange() public {
        vm.expectRevert(abi.encodeWithSelector(L1BlockhashContract.L1BlockNotImported.selector, uint64(98)));
        new L1BlockHashCollector(98);
        vm.expectRevert(abi.encodeWithSelector(L1BlockhashContract.L1BlockNotImported.selector, uint64(100)));
        new L1BlockHashCollector(100);
    }

    function testInvalidCurrentStateIsNotTreatedAsWaiting() public {
        assertCollected(99);
        vm.store(address(HISTORY), hashSlot(99), bytes32(0));
        vm.expectRevert(L1BlockhashContract.InvalidHistoryState.selector);
        collector.poll();
        assertEq(collector.nextL1BlockNumber(), 100);
        assertEq(collector.blockHashes(99), expectedHash(99));
    }

    function testCoveredMissingHashRevertsWithoutAdvancing() public {
        assertCollected(99);
        importHashes(120, 100, 3);
        vm.store(address(HISTORY), hashSlot(100), bytes32(0));
        vm.expectRevert(abi.encodeWithSelector(L1BlockhashContract.L1BlockNotImported.selector, uint64(100)));
        collector.poll();
        assertEq(collector.nextL1BlockNumber(), 100);
        assertEq(collector.blockHashes(100), bytes32(0));
    }

    function testMalformedCurrentAbiRevertsWithoutAdvancing() public {
        assertCollected(99);
        vm.mockCall(address(HISTORY), abi.encodeCall(HISTORY.current, ()), hex"01");
        vm.expectRevert();
        collector.poll();
        assertEq(collector.nextL1BlockNumber(), 100);
    }

    function testUint64EndpointExhaustionDoesNotWrapCursor() public {
        uint64 last = type(uint64).max;
        vm.store(address(HISTORY), bytes32(uint256(1)), bytes32(uint256(last)));
        vm.store(address(HISTORY), bytes32(uint256(2)), bytes32(uint256(last)));
        vm.store(address(HISTORY), hashSlot(last), expectedHash(last));
        collector = new L1BlockHashCollector(last);
        assertCollected(last);
        assertWaiting(uint256(last) + 1);
    }
}
