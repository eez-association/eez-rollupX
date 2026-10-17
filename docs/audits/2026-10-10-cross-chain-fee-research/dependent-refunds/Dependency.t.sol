// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Base} from "protocol/test/Base.t.sol";
import {EEZ} from "protocol/src/EEZ.sol";
import {EEZBase} from "protocol/src/base/EEZBase.sol";
import {ExecutionEntry} from "protocol/src/interfaces/IEEZ.sol";

/// @notice Research fixture for ordering two L1 users against one posted table.
/// @dev Uses the pinned core's mock-proof fixture and real nonempty execution
/// entries. Pranked calls are not signed transactions or builder bundles. This
/// checks queue/root semantics, not native blob transport, fees, or L2 replay.
contract DependentRefundProtocolTest is Base {
    RollupHandle internal rollup;
    address internal proxy;
    address internal userA;
    address internal userB;
    bytes32 internal constant ROOT_A = bytes32(uint256(1));
    bytes32 internal constant ROOT_B = bytes32(uint256(2));

    function setUp() public {
        setUpBase();
        vm.roll(100);
        rollup = _makeRollup(bytes32(0));
        proxy = rollups.createCrossChainProxy(L2_REMOTE, uint64(rollup.id));
        userA = makeAddr("refund user A");
        userB = makeAddr("refund user B");
    }

    function _postPair(bool bDependsOnA) internal {
        ExecutionEntry[] memory entries = new ExecutionEntry[](2);
        entries[0] = _immediateEntry(rollup.id, bytes32(0), ROOT_A);
        entries[1] = _immediateEntry(rollup.id, bDependsOnA ? ROOT_A : bytes32(0), ROOT_B);
        for (uint256 i; i < entries.length; i++) {
            address user = i == 0 ? userA : userB;
            entries[i].proxyEntryHash =
                rollups.computeCrossChainCallHash(false, user, 0, L2_REMOTE, uint64(rollup.id), 0, 0, "");
            entries[i].rollingHash = _hEntryBegin(entries[i].rollupUpdates, entries[i].proxyEntryHash);
        }
        _postBatchOne(rollup, entries, _emptyStaticEntries(), 0, 0);
        assertEq(_getRollupState(rollup.id), bytes32(0));
    }

    function _consume(address user, bytes32 expectedRoot) internal {
        vm.prank(user);
        (bool success, bytes memory result) = proxy.call("");
        assertTrue(success);
        assertEq(result, bytes(""));
        assertEq(_getRollupState(rollup.id), expectedRoot);
    }

    function _reject(address user, bytes memory expectedError) internal {
        vm.prank(user);
        (bool success, bytes memory result) = proxy.call("");
        assertFalse(success);
        assertEq(result, expectedError);
    }

    function test_ChildWithoutPostFailsSameBlockGate() public {
        _reject(userB, abi.encodeWithSelector(EEZ.ExecutionNotInCurrentBlock.selector, uint64(rollup.id)));
        assertEq(_getRollupState(rollup.id), bytes32(0));
    }

    function test_PostAloneDoesNotSatisfyChildRootDependency() public {
        _postPair(true);
        _reject(userB, abi.encodeWithSelector(EEZBase.ExecutionNotFound.selector));
        assertEq(rollups.entryQueueIndex(uint64(rollup.id)), 0);
        _consume(userA, ROOT_A);
        _consume(userB, ROOT_B);
        assertEq(rollups.entryQueueIndex(uint64(rollup.id)), 2);
    }

    function test_OnlyFirstUserCanConsumeWithoutCompletingBatch() public {
        _postPair(true);
        _consume(userA, ROOT_A);
        assertEq(rollups.lastVerifiedBlock(uint64(rollup.id)), block.number);
        assertEq(rollups.queueLength(uint64(rollup.id)), 2);
        assertEq(rollups.entryQueueIndex(uint64(rollup.id)), 1);
    }

    function test_UnconsumedChildExpiresNextBlock() public {
        _postPair(true);
        _consume(userA, ROOT_A);
        vm.roll(block.number + 1);
        _reject(userB, abi.encodeWithSelector(EEZ.ExecutionNotInCurrentBlock.selector, uint64(rollup.id)));
        assertEq(_getRollupState(rollup.id), ROOT_A);
    }

    function test_AnotherPostReplacesUnconsumedChildInSameBlock() public {
        _postPair(true);
        _consume(userA, ROOT_A);
        _postBatchOne(rollup, _emptyEntries(), _emptyStaticEntries(), 0, 0);
        _reject(userB, abi.encodeWithSelector(EEZBase.ExecutionNotFound.selector));
        assertEq(_getRollupState(rollup.id), ROOT_A);
        assertEq(rollups.queueLength(uint64(rollup.id)), 0);
    }

    function test_IndependentMatchingChildCanSkipFirstEntry() public {
        _postPair(false);
        _consume(userB, ROOT_B);
        assertEq(rollups.entryQueueIndex(uint64(rollup.id)), 2);
        _reject(userA, abi.encodeWithSelector(EEZBase.ExecutionNotFound.selector));
    }
}
