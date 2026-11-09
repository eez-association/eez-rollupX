// SPDX-License-Identifier: MIT
pragma solidity ^0.8.34;

import {BaseL2} from "eez-core-protocol/test/BaseL2.t.sol";
import {EEZBase} from "eez-core-protocol/src/base/EEZBase.sol";
import {
    ExecutionEntry,
    CrossChainCall,
    ExpectedOutgoingCrossChainCall,
    StaticExecutionEntryL2
} from "eez-core-protocol/src/interfaces/IEEZL2.sol";

contract FundingNestedCaller {
    address private immutable remoteProxy;
    uint256 public calls;

    constructor(address proxy) {
        remoteProxy = proxy;
    }

    function invoke(bool expectedSuccess, uint8 attempts) external payable {
        calls++;
        for (uint256 i; i < attempts; ++i) {
            (bool success, bytes memory result) = remoteProxy.call("");
            require(success == expectedSuccess, "unexpected nested outcome");
            require(keccak256(result) == keccak256(expectedSuccess ? bytes("") : bytes(hex"abcd")), "nested result");
        }
    }
}

/// Funding feasibility for #105, using the pinned manager and proxy implementation.
/// vm.deal at group boundaries models the proposed cleanup; it does not implement
/// or validate the node's checkpoint, gas, native minting or settlement machinery.
contract ProtocolGroupFundingTest is BaseL2 {
    address private constant REMOTE = address(0x1010);
    address private constant NESTED_REMOTE = address(0x2020);
    address private constant RECIPIENT = address(0x3030);
    uint256 private constant OUTBOUND_VALUE = 1 ether;
    bytes private constant ORIGIN_DATA = hex"12345678";

    address private originProxy;
    FundingNestedCaller private nestedCaller;

    function setUp() public override {
        super.setUp();
        originProxy = manager.createCrossChainProxy(REMOTE, MAINNET);
        nestedCaller = new FundingNestedCaller(manager.createCrossChainProxy(NESTED_REMOTE, MAINNET));
        vm.deal(address(this), 100 ether);
        _clearProtocolBalances();
    }

    function _clearProtocolBalances() private {
        vm.deal(address(manager), 0);
        vm.deal(SYSTEM_ADDRESS, 0);
    }

    function _load(ExecutionEntry[] memory entries, uint256 value) private {
        assertEq(address(manager).balance, 0, "no inherited manager inventory");
        assertEq(SYSTEM_ADDRESS.balance, 0, "no inherited system inventory");
        // Represents exactly the new native transaction's credit, never prior inventory.
        vm.deal(SYSTEM_ADDRESS, value);
        vm.prank(SYSTEM_ADDRESS);
        manager.loadExecutionTable{value: value}(entries, new StaticExecutionEntryL2[](0));
        assertEq(SYSTEM_ADDRESS.balance, 0);
        assertEq(address(manager).balance, value);
    }

    function _consume() private {
        (bool success, bytes memory result) = originProxy.call{value: OUTBOUND_VALUE}(ORIGIN_DATA);
        assertTrue(success, "the funded outbound group completes");
        assertEq(result, "");
    }

    function _deliver(uint256 value) private {
        assertEq(address(manager).balance, 0, "delivery needs no previous table balance");
        assertEq(SYSTEM_ADDRESS.balance, 0, "delivery needs no previous system balance");
        uint256 before = RECIPIENT.balance;
        CrossChainCall memory call = _cc(RECIPIENT, value, "", REMOTE, MAINNET);
        bytes32 root = _incomingCallHash(call);
        ExecutionEntry[] memory entries = new ExecutionEntry[](1);
        entries[0] = _buildSimpleEntry(root, call, "", _rhSingle(root, call, true, ""));
        vm.deal(SYSTEM_ADDRESS, value);
        vm.prank(SYSTEM_ADDRESS);
        manager.executeIncomingCrossChainCall{value: value}(entries, new StaticExecutionEntryL2[](0));
        assertEq(RECIPIENT.balance, before + value);
        assertEq(address(manager).balance, 0);
        assertEq(SYSTEM_ADDRESS.balance, 0);
    }

    function _nestedTable(
        uint256 flatValue,
        uint256 nestedValue,
        bool nestedSuccess,
        uint8 attempts,
        bool inbound
    )
        private
        view
        returns (ExecutionEntry[] memory entries)
    {
        CrossChainCall memory flat = _cc(
            address(nestedCaller),
            flatValue,
            abi.encodeCall(FundingNestedCaller.invoke, (nestedSuccess, attempts)),
            REMOTE,
            MAINNET
        );
        bytes32 root = inbound
            ? _incomingCallHash(flat)
            : _outgoingCallHash(address(this), REMOTE, MAINNET, OUTBOUND_VALUE, 0, ORIGIN_DATA);
        CrossChainCall memory nested = _cc(RECIPIENT, nestedValue, "", NESTED_REMOTE, MAINNET);
        bytes32 outgoing = _outgoingCallHash(address(nestedCaller), NESTED_REMOTE, MAINNET, 0, 0, "");
        bytes32 atFire = _hCallBegin(_hEntryBeginL2(root), _incomingCallHash(flat));
        bytes32 nestedHash =
            _hCallEnd(_hCallBegin(_hNestedBegin(atFire, outgoing), _incomingCallHash(nested)), true, "");
        bytes32 finalHash = _hCallEnd(nestedSuccess ? _hNestedEnd(nestedHash) : atFire, true, "");
        entries = new ExecutionEntry[](1);
        entries[0] = _buildSimpleEntry(root, flat, "", finalHash);
        CrossChainCall[] memory callbacks = new CrossChainCall[](1);
        callbacks[0] = nested;
        entries[0].expectedOutgoingCalls = new ExpectedOutgoingCrossChainCall[](1);
        entries[0].expectedOutgoingCalls[0] = ExpectedOutgoingCrossChainCall({
            expectedOutgoingHash: _expectedOutgoingHash(outgoing, atFire),
            incomingCalls: callbacks,
            revertedOrStaticRollingHash: nestedSuccess ? bytes32(0) : nestedHash,
            success: nestedSuccess,
            returnData: nestedSuccess ? bytes("") : bytes(hex"abcd")
        });
    }

    function testFuzz_NestedOutboundAndFollowingInboundNeedNoPriorInventory(
        uint96 flatAmount,
        uint96 nestedAmount,
        uint96 nextAmount
    )
        public
    {
        uint256 flatValue = bound(flatAmount, 1, 10 ether);
        uint256 nestedValue = bound(nestedAmount, 1, 10 ether);
        uint256 nextValue = bound(nextAmount, 1, 10 ether);
        _load(_nestedTable(flatValue, nestedValue, true, 1, false), flatValue + nestedValue);
        _consume();
        assertEq(address(nestedCaller).balance, flatValue);
        assertEq(nestedCaller.calls(), 1);
        assertEq(RECIPIENT.balance, nestedValue);
        assertEq(address(manager).balance, 0);
        assertEq(SYSTEM_ADDRESS.balance, OUTBOUND_VALUE);
        _clearProtocolBalances();
        _deliver(nextValue);
    }

    // This is deliberately broader than RollupX's accepted profile, which rejects
    // failed nested rows with descendants. It checks core rollback only.
    function test_CoreRevertedNestedRetriesRestoreFundingBeforeReuse() public {
        _load(_nestedTable(3 ether, 5 ether, false, 2, false), 8 ether);
        _consume();
        assertEq(address(nestedCaller).balance, 3 ether);
        assertEq(nestedCaller.calls(), 1);
        assertEq(RECIPIENT.balance, 0, "both nested payments rolled back");
        assertEq(address(manager).balance, 5 ether, "same allocation survived both retries");
        assertEq(SYSTEM_ADDRESS.balance, OUTBOUND_VALUE);
        _clearProtocolBalances();
        _deliver(7 ether);
    }

    function test_RevertSpanResidualIsUnnecessaryForTheNextGroup() public {
        bytes32 root = _outgoingCallHash(address(this), REMOTE, MAINNET, OUTBOUND_VALUE, 0, ORIGIN_DATA);
        CrossChainCall memory rolledBack = _cc(RECIPIENT, 3 ether, "", REMOTE, MAINNET);
        CrossChainCall memory retained = _cc(RECIPIENT, 5 ether, "", REMOTE, MAINNET);
        bytes32 hash = _rhSingle(root, rolledBack, true, "");
        hash = _hCallEnd(_hCallBegin(hash, _incomingCallHash(retained)), true, "");
        ExecutionEntry[] memory entries = new ExecutionEntry[](1);
        entries[0] = _buildSimpleEntry(root, rolledBack, "", hash);
        entries[0].incomingCalls = new CrossChainCall[](2);
        rolledBack.revertNextNCalls = 1;
        entries[0].incomingCalls[0] = rolledBack;
        entries[0].incomingCalls[1] = retained;
        _load(entries, 8 ether);
        _consume();
        assertEq(RECIPIENT.balance, 5 ether);
        assertEq(address(manager).balance, 3 ether);
        _clearProtocolBalances();
        _deliver(7 ether);
    }

    function consumeThenRevert() external {
        require(msg.sender == address(this), "test self-call");
        (bool success,) = originProxy.call{value: OUTBOUND_VALUE}(ORIGIN_DATA);
        require(success, "outbound call must execute before the user revert");
        revert("user reverted");
    }

    function test_UserRevertRestoresAllLoadedFundingAndNextGroupIsIndependent() public {
        _load(_nestedTable(3 ether, 5 ether, true, 1, false), 8 ether);
        (bool success, bytes memory result) = address(this).call(abi.encodeCall(this.consumeThenRevert, ()));
        assertFalse(success);
        assertEq(result, abi.encodeWithSignature("Error(string)", "user reverted"));
        assertEq(address(manager).balance, 8 ether);
        assertEq(SYSTEM_ADDRESS.balance, 0);
        assertEq(manager.entryIndex(), 0);
        assertEq(address(nestedCaller).balance, 0);
        assertEq(nestedCaller.calls(), 0);
        assertEq(RECIPIENT.balance, 0);
        _clearProtocolBalances();
        _deliver(7 ether);
    }

    function test_RecoveredSourceFrameRetriesWithOnlyItsOriginalAllocation() public {
        _load(_nestedTable(3 ether, 5 ether, true, 1, false), 8 ether);
        (bool success, bytes memory result) = address(this).call(abi.encodeCall(this.consumeThenRevert, ()));
        assertFalse(success);
        assertEq(result, abi.encodeWithSignature("Error(string)", "user reverted"));
        assertEq(manager.entryIndex(), 0);
        assertEq(address(manager).balance, 8 ether);
        assertEq(RECIPIENT.balance, 0);
        assertEq(SYSTEM_ADDRESS.balance, 0);
        // The surrounding user execution recovers and retries the same row. No
        // second load, native credit or balance adjustment occurs between attempts.
        _consume();
        assertEq(manager.entryIndex(), 1);
        assertEq(address(nestedCaller).balance, 3 ether);
        assertEq(nestedCaller.calls(), 1);
        assertEq(RECIPIENT.balance, 5 ether);
        assertEq(address(manager).balance, 0);
        assertEq(SYSTEM_ADDRESS.balance, OUTBOUND_VALUE);
    }

    function test_OmittingNestedFundingFailsAtHashValidationAndExactFundingSucceeds() public {
        ExecutionEntry[] memory entries = _nestedTable(3 ether, 5 ether, true, 1, false);
        _load(entries, 3 ether);
        (bool success, bytes memory result) = originProxy.call{value: OUTBOUND_VALUE}(ORIGIN_DATA);
        assertFalse(success);
        assertEq(result, abi.encodeWithSelector(EEZBase.RollingHashMismatch.selector));
        assertEq(RECIPIENT.balance, 0);
        assertEq(manager.entryIndex(), 0);
        // Same table and call; restore precisely its missing callback allocation.
        vm.deal(address(manager), 8 ether);
        _consume();
        assertEq(address(nestedCaller).balance, 3 ether);
        assertEq(RECIPIENT.balance, 5 ether);
        assertEq(address(manager).balance, 0);
    }

    function test_NestedInboundAndFollowingOutboundNeedNoPriorInventory() public {
        ExecutionEntry[] memory entries = _nestedTable(3 ether, 5 ether, true, 1, true);
        vm.deal(SYSTEM_ADDRESS, 8 ether);
        vm.prank(SYSTEM_ADDRESS);
        manager.executeIncomingCrossChainCall{value: 8 ether}(entries, new StaticExecutionEntryL2[](0));
        assertEq(address(nestedCaller).balance, 3 ether);
        assertEq(nestedCaller.calls(), 1);
        assertEq(RECIPIENT.balance, 5 ether);
        assertEq(address(manager).balance, 0);
        assertEq(SYSTEM_ADDRESS.balance, 0);
        _clearProtocolBalances();
        _load(_nestedTable(2 ether, 4 ether, true, 1, false), 6 ether);
        _consume();
        assertEq(address(nestedCaller).balance, 5 ether);
        assertEq(nestedCaller.calls(), 2);
        assertEq(RECIPIENT.balance, 9 ether);
        assertEq(address(manager).balance, 0);
    }

    function test_MultipleRootsKeepTheirSharedFundingUntilTheWholeGroupEnds() public {
        ExecutionEntry[] memory entries = new ExecutionEntry[](2);
        bytes32 root = _outgoingCallHash(address(this), REMOTE, MAINNET, OUTBOUND_VALUE, 0, ORIGIN_DATA);
        CrossChainCall memory first = _cc(RECIPIENT, 3 ether, "", REMOTE, MAINNET);
        entries[0] = _buildSimpleEntry(root, first, "", _rhSingle(root, first, true, ""));
        bytes memory secondData = hex"87654321";
        bytes32 secondRoot = _outgoingCallHash(address(this), REMOTE, MAINNET, OUTBOUND_VALUE, 0, secondData);
        CrossChainCall memory second = _cc(RECIPIENT, 5 ether, "", REMOTE, MAINNET);
        entries[1] = _buildSimpleEntry(secondRoot, second, "", _rhSingle(secondRoot, second, true, ""));
        _load(entries, 8 ether);
        _consume();
        assertEq(address(manager).balance, 5 ether, "later roots still own their allocation");
        assertEq(manager.entryIndex(), 1);
        (bool success,) = originProxy.call{value: OUTBOUND_VALUE}(secondData);
        assertTrue(success);
        assertEq(manager.entryIndex(), 2);
        assertEq(RECIPIENT.balance, 8 ether);
        assertEq(address(manager).balance, 0);
        assertEq(SYSTEM_ADDRESS.balance, 2 * OUTBOUND_VALUE);
        _clearProtocolBalances();
        _deliver(7 ether);
    }
}
