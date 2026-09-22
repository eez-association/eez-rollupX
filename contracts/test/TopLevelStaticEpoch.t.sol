// SPDX-License-Identifier: MIT
pragma solidity ^0.8.34;

import {BaseL2} from "eez-core-protocol/test/BaseL2.t.sol";
import {ExecutionEntry, StaticExecutionEntry} from "eez-core-protocol/src/interfaces/IEEZL2.sol";

/// @notice Reproduces the pinned manager's read/write/read ambiguity independently of the signer.
contract TopLevelStaticEpochTest is BaseL2 {
    function test_ReadWriteReadReturnsFirstCachedValue() public {
        address token = address(0x1234);
        address proxy = manager.createCrossChainProxy(token, MAINNET);
        bytes memory readData = abi.encodeWithSignature("balanceOf(address)", address(this));
        bytes memory writeData = abi.encodeWithSignature("transfer(address,uint256)", address(0x5678), 1);
        bytes32 readHash = _ccHash(IS_STATIC, address(this), TEST_ROLLUP_ID, token, MAINNET, 0, readData);
        bytes32 writeHash = _outgoingCallHash(address(this), token, MAINNET, 0, 0, writeData);
        ExecutionEntry[] memory writes = new ExecutionEntry[](1);
        writes[0] = _buildNoCalls(writeHash, abi.encode(true));
        StaticExecutionEntry[] memory reads = new StaticExecutionEntry[](2);
        reads[0].proxyEntryHash = readHash;
        reads[0].success = true;
        reads[0].returnData = abi.encode(uint256(10));
        reads[1].proxyEntryHash = readHash;
        reads[1].success = true;
        reads[1].returnData = abi.encode(uint256(9));
        _loadEntries(writes, reads);

        (bool beforeOk, bytes memory beforeData) = proxy.staticcall(readData);
        assertTrue(beforeOk);
        assertEq(abi.decode(beforeData, (uint256)), 10);
        (bool writeOk, bytes memory writeResult) = proxy.call(writeData);
        assertTrue(writeOk);
        assertTrue(abi.decode(writeResult, (bool)));
        (bool afterOk, bytes memory afterData) = proxy.staticcall(readData);
        assertTrue(afterOk);
        // Current contract behavior: the second prediction is unreachable even after the write.
        assertEq(abi.decode(afterData, (uint256)), 10);
        assertNotEq(abi.decode(afterData, (uint256)), 9);
    }
}
