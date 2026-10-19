// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Test} from "forge-std/Test.sol";
import {
    NestedCreationChild,
    NestedCreationFactory,
    NestedCreationRelay,
    NestedCreationCaller
} from "../src/NestedContractCreation.sol";

/// Same application calls on one EVM, without cross-chain proxies. This
/// control isolates mistakes in the fixture from Composer state propagation.
contract NestedContractCreationTest is Test {
    function test_createAndUseBothCreationOpcodes() public {
        NestedCreationFactory factory = new NestedCreationFactory();
        NestedCreationRelay relay = new NestedCreationRelay();
        NestedCreationCaller caller = new NestedCreationCaller();
        for (uint256 i; i < 2; ++i) {
            address child = caller.createAndUse(address(relay), address(factory), true, i == 1, bytes32(i));
            assertGt(child.code.length, 0);
            assertEq(NestedCreationChild(child).value(), 42);
            assertEq(caller.lastChild(), child);
            assertEq(caller.lastValue(), 42);
        }
    }

    function test_existingChildControl() public {
        NestedCreationFactory factory = new NestedCreationFactory();
        NestedCreationRelay relay = new NestedCreationRelay();
        NestedCreationCaller caller = new NestedCreationCaller();
        address existing = factory.obtain(true, false, bytes32(0), 41);
        assertEq(caller.createAndUse(address(relay), address(factory), false, false, bytes32(0)), existing);
        assertEq(NestedCreationChild(existing).value(), 42);
    }

    function test_createUseReenterAndCreateLocally() public {
        NestedCreationFactory factory = new NestedCreationFactory();
        NestedCreationRelay relay = new NestedCreationRelay();
        NestedCreationCaller caller = new NestedCreationCaller();
        for (uint256 i; i < 2; ++i) {
            uint64 nonce = vm.getNonce(address(factory));
            caller.createUseAndReenter(address(relay), address(factory), address(factory), i == 1, bytes32(i));
            assertEq(vm.getNonce(address(factory)), nonce + 2);
            assertEq(NestedCreationChild(caller.lastChild()).value(), 42);
            assertEq(NestedCreationChild(caller.secondChild()).value(), 77);
            assertTrue(caller.lastChild() != caller.secondChild());
        }
    }
}
