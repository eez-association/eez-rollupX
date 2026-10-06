// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Test} from "forge-std/Test.sol";
import {EEZ} from "eez-core-protocol/src/EEZ.sol";
import {
    ExecutionEntry,
    ProofSystemBatchPerVerificationEntries,
    RollupUpdate
} from "eez-core-protocol/src/interfaces/IEEZ.sol";
import {EEZPostBatcher} from "../src/EEZPostBatcher.sol";

/// Records the forwarded batch and reports a configurable post-batch root.
contract MockEEZ {
    mapping(uint64 => bytes32) public root;
    uint256 public posts;
    address public lastSender;
    bytes32 public lastCallDataHash;

    function setRoot(uint64 rollupId, bytes32 value) external {
        root[rollupId] = value;
    }

    function postAndVerifyBatch(ProofSystemBatchPerVerificationEntries calldata) external {
        posts++;
        lastSender = msg.sender;
        lastCallDataHash = keccak256(msg.data);
    }

    function rollups(uint64 rollupId) external view returns (address, bytes32, uint256) {
        return (address(0), root[rollupId], 0);
    }
}

contract EEZPostBatcherTest is Test {
    MockEEZ internal registry;
    EEZPostBatcher internal batcher;

    function setUp() public {
        registry = new MockEEZ();
        batcher = new EEZPostBatcher(EEZ(address(registry)));
    }

    function _batch(
        uint256 immediate,
        bytes32 lastRoot
    )
        internal
        pure
        returns (ProofSystemBatchPerVerificationEntries memory batch)
    {
        batch.entries = new ExecutionEntry[](2);
        for (uint256 i = 0; i < 2; i++) {
            batch.entries[i].rollupUpdates = new RollupUpdate[](1);
            batch.entries[i].rollupUpdates[0].rollupId = 1;
            batch.entries[i].rollupUpdates[0].newRoot = bytes32(i + 1);
        }
        batch.entries[1].rollupUpdates[0].newRoot = lastRoot;
        batch.immediateEntryCount = immediate;
    }

    function test_ForwardsIdenticalCalldataAndAcceptsMatchingFinalRoot() public {
        registry.setRoot(1, bytes32(uint256(0xaa)));
        ProofSystemBatchPerVerificationEntries memory batch = _batch(2, bytes32(uint256(0xaa)));
        bytes memory data = abi.encodeCall(EEZPostBatcher.postAndVerifyBatch, (batch));
        // Same selector as EEZ, so derivation decodes the poster's input unchanged.
        assertEq(bytes4(data), EEZ.postAndVerifyBatch.selector);
        (bool ok,) = address(batcher).call(data);
        assertTrue(ok);
        assertEq(registry.posts(), 1);
        assertEq(registry.lastSender(), address(batcher));
        assertEq(registry.lastCallDataHash(), keccak256(data));
    }

    function test_RevertsWhenLastImmediateEntryDidNotReachItsRoot() public {
        // A skipped entry leaves the registry on an earlier root.
        registry.setRoot(1, bytes32(uint256(1)));
        vm.expectRevert(
            abi.encodeWithSelector(
                EEZPostBatcher.LastImmediateRootMismatch.selector,
                uint64(1),
                bytes32(uint256(0xaa)),
                bytes32(uint256(1))
            )
        );
        batcher.postAndVerifyBatch(_batch(2, bytes32(uint256(0xaa))));
    }

    function test_ChecksOnlyTheLastImmediateEntryAndSkipsAnEmptyPrefix() public {
        // The second entry is persistent, so the first one is the last immediate.
        registry.setRoot(1, bytes32(uint256(1)));
        batcher.postAndVerifyBatch(_batch(1, bytes32(uint256(0xaa))));
        registry.setRoot(1, bytes32(uint256(7)));
        batcher.postAndVerifyBatch(_batch(0, bytes32(uint256(0xaa))));
        batcher.postBatch(_batch(2, bytes32(uint256(0xaa))), false);
        assertEq(registry.posts(), 3);
    }

    function test_MetaHookOnlyAcceptsTheRegistry() public {
        vm.expectRevert(EEZPostBatcher.OnlyEEZ.selector);
        batcher.executeMetaCrossChainTransactions();
        vm.prank(address(registry));
        batcher.executeMetaCrossChainTransactions();
    }
}
