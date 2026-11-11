// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {EEZ} from "eez-core-protocol/src/EEZ.sol";
import {EEZL2} from "eez-core-protocol/src/L2/EEZL2.sol";
import {EEZBase} from "eez-core-protocol/src/base/EEZBase.sol";
import {ICrossChainProxy} from "eez-core-protocol/src/interfaces/ICrossChainProxy.sol";

contract ProxyCodeCaller {
    function invoke(address proxy, bytes memory data, bool useCallcode)
        external
        returns (bool ok, bytes memory result)
    {
        if (!useCallcode) return proxy.delegatecall(data);
        // The copied returndata owns fresh, word-aligned memory; the CALLCODE
        // result must reach the assertion unchanged, including its revert selector.
        assembly ("memory-safe") {
            ok := callcode(gas(), proxy, 0, add(data, 32), mload(data), 0, 0)
            result := mload(0x40)
            mstore(result, returndatasize())
            returndatacopy(add(result, 32), 0, returndatasize())
            mstore(0x40, add(add(result, 32), and(add(returndatasize(), 31), not(31))))
        }
    }
}

contract ProxyForwardDestination {
    address public caller;

    function record() external returns (address) {
        caller = msg.sender;
        return caller;
    }
}

/// The pinned managers authorize the execution-context caller. Borrowing proxy
/// bytecode in an unregistered context cannot dispatch a cross-chain operation.
contract CrossChainProxyPolicyTest is Test {
    function testL2DelegatecallToProxyRejectsUnregisteredContext() external {
        _l2(false);
    }

    function testL2CallcodeToProxyRejectsUnregisteredContext() external {
        _l2(true);
    }

    function testL1DelegatecallToProxyRejectsUnregisteredContext() external {
        _l1(false);
    }

    function testL1CallcodeToProxyRejectsUnregisteredContext() external {
        _l1(true);
    }

    function testL1ManagerDelegatecallRetainsOrdinaryForwarding() external {
        EEZ manager = new EEZ(address(0xDEAD));
        _managerForwarding(address(manager), manager.createCrossChainProxy(address(0xCAFE), 1), false);
    }

    function testL2ManagerDelegatecallRetainsOrdinaryForwarding() external {
        EEZL2 manager = new EEZL2(1, address(0xBEEF), false, address(0xA11CE));
        _managerForwarding(address(manager), manager.createCrossChainProxy(address(0xCAFE), 0), false);
    }

    function testL1ManagerCallcodeDoesNotPreserveManagerCaller() external {
        EEZ manager = new EEZ(address(0xDEAD));
        _managerForwarding(address(manager), manager.createCrossChainProxy(address(0xCAFE), 1), true);
    }

    function testL2ManagerCallcodeDoesNotPreserveManagerCaller() external {
        EEZL2 manager = new EEZL2(1, address(0xBEEF), false, address(0xA11CE));
        _managerForwarding(address(manager), manager.createCrossChainProxy(address(0xCAFE), 0), true);
    }

    function testL1CallcodeCanRunSelfProbeWithoutDispatch() external {
        EEZ manager = new EEZ(address(0xDEAD));
        _selfProbe(manager.createCrossChainProxy(address(0xCAFE), 1));
    }

    function testL2CallcodeCanRunSelfProbeWithoutDispatch() external {
        EEZL2 manager = new EEZL2(1, address(0xBEEF), false, address(0xA11CE));
        _selfProbe(manager.createCrossChainProxy(address(0xCAFE), 0));
    }

    function _managerForwarding(address manager, address proxy, bool useCallcode) private {
        ProxyCodeCaller caller = new ProxyCodeCaller();
        ProxyForwardDestination destination = new ProxyForwardDestination();
        bytes memory data = abi.encodeCall(
            ICrossChainProxy.executeOnBehalf, (address(destination), 0, abi.encodeCall(destination.record, ()))
        );
        vm.recordLogs();
        vm.prank(manager);
        (bool ok, bytes memory result) = caller.invoke(proxy, data, useCallcode);
        if (useCallcode) {
            assertFalse(ok);
            assertEq(result, abi.encodeWithSelector(EEZBase.UnauthorizedProxy.selector));
            assertEq(destination.caller(), address(0));
        } else {
            assertTrue(ok);
            assertEq(abi.decode(result, (address)), address(caller));
            assertEq(destination.caller(), address(caller));
        }
        assertEq(vm.getRecordedLogs().length, 0, "ordinary forwarding emitted a cross-chain event");
    }

    function _selfProbe(address proxy) private {
        ProxyCodeCaller caller = new ProxyCodeCaller();
        vm.recordLogs();
        (bool ok, bytes memory result) = caller.invoke(proxy, abi.encodeWithSignature("staticCheck()"), true);
        assertTrue(ok);
        assertEq(result.length, 0);
        assertEq(vm.getRecordedLogs().length, 0);
    }

    function _l2(bool useCallcode) private {
        EEZL2 manager = new EEZL2(1, address(0xBEEF), false, address(0xA11CE));
        _reject(manager.createCrossChainProxy(address(0xCAFE), 0), useCallcode);
    }

    function _l1(bool useCallcode) private {
        EEZ manager = new EEZ(address(0xDEAD));
        _reject(manager.createCrossChainProxy(address(0xCAFE), 1), useCallcode);
    }

    function _reject(address proxy, bool useCallcode) private {
        ProxyCodeCaller caller = new ProxyCodeCaller();
        vm.recordLogs();
        vm.record();
        (bool ok, bytes memory result) = caller.invoke(proxy, hex"12345678", useCallcode);
        assertFalse(ok);
        assertEq(result, abi.encodeWithSelector(EEZBase.UnauthorizedProxy.selector));
        assertEq(vm.getRecordedLogs().length, 0, "rejected context emitted an execution event");
        (, bytes32[] memory writes) = vm.accesses(address(0xCAFE));
        assertEq(writes.length, 0, "rejected context wrote destination state");
    }
}
