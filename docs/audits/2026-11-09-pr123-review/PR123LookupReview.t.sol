// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;
import {Base} from "./Base.t.sol";
import {ExecutionEntry} from "../src/interfaces/IEEZ.sol";
contract PR123LookupReview is Base {
    uint256 public effects;
    address public callbackSender;
    function setUp() public { setUpBase(); }
    function recordEffect() external { effects++; callbackSender = msg.sender; }
    function conditionalLookup(address proxy, bytes calldata request) external {
        (bool ok,) = proxy.call(request);
        require(ok, "lookup failed");
        require(block.prevrandao != 0, "source wrapper reverts in predicted environment");
    }
    function runLookup(bool succeeds) internal {
        RollupHandle memory r = _makeRollup(bytes32(0));
        address remote = address(0x1234);
        address proxy = rollups.createCrossChainProxy(remote, uint64(r.id));
        bytes memory request = hex"12345678";
        bytes memory callback = abi.encodeCall(this.recordEffect, ());
        bytes32 key = _ccHash(false, address(this), 0, remote, uint64(r.id), 0, request);
        ExecutionEntry[] memory entries = new ExecutionEntry[](1);
        entries[0] = _shellEntry(r.id, _oneDelta(r.id, bytes32(0), bytes32(0), 0));
        entries[0].proxyEntryHash = key;
        entries[0].l2ToL1Calls = _oneCall(_call(remote, uint64(r.id), address(this), 0, callback));
        entries[0].rollingHash = _oneCallHash(entries[0].rollupUpdates, key,
            _ccHash(false, remote, uint64(r.id), address(this), 0, 0, callback), true, "");
        entries[0].success = succeeds;
        _postBatchOne(r, entries, _emptyStaticEntries(), 0, 0);
        if (succeeds) {
            vm.prevrandao(bytes32(0));
            (bool simulated,) = address(this).call(abi.encodeCall(this.conditionalLookup, (proxy, request)));
            assertFalse(simulated);
            assertEq(effects, 0, "predicted source rollback discards callback");
            vm.prevrandao(bytes32(uint256(1)));
            this.conditionalLookup(proxy, request);
        } else {
            (bool ok,) = proxy.call(request);
            assertFalse(ok);
        }
        assertEq(_getRollupState(r.id), bytes32(0));
        assertEq(effects, succeeds ? 1 : 0);
        if (succeeds) assertEq(callbackSender, proxy);
    }
    function test_SuccessfulNoTransitionLookupPersistsAuthenticatedCallback() public { runLookup(true); }
    function test_FailedLookupRollsBackCallbackControl() public { runLookup(false); }
}
