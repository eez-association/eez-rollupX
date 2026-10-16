// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

contract NestedSetterInner {
    address public immutable proxy;
    uint256 public completedProxyCalls;

    constructor(address _proxy) {
        proxy = _proxy;
    }

    function setViaProxy(uint256 v) public {
        (bool ok, bytes memory ret) = proxy.call(abi.encodeWithSignature("setValue(uint256)", v));
        require(ok, "cross-chain setValue reverted");
        abi.decode(ret, (bool, uint256));
        completedProxyCalls++;
    }

    function setViaProxyAndRevert(uint256 v) external {
        setViaProxy(v);
        revert("deliberate nested rollback");
    }

    /// A future-nonce quote must replay both chains, not just advance the EOA nonce.
    function setAfterProxy(uint256 expectedLocal, uint256 expectedRemote, uint256 v) external {
        require(completedProxyCalls == expectedLocal, "local predecessor missing");
        (bool ok, bytes memory ret) = proxy.staticcall(abi.encodeWithSignature("value()"));
        require(ok && abi.decode(ret, (uint256)) == expectedRemote, "remote predecessor missing");
        setViaProxy(v);
    }

    /// Conditional discovery sees the previous queue; native execution sees the
    /// newly posted shared queue. This forces a real, attributable batch failure.
    function setInSmallBatch(address registry, uint64 rollupId, uint256 maximum, uint256 v) external {
        (bool ok, bytes memory ret) = registry.staticcall(abi.encodeWithSignature("queueLength(uint64)", rollupId));
        require(ok && abi.decode(ret, (uint256)) <= maximum, "shared batch too large");
        setViaProxy(v);
    }
}

contract NestedSetterOuter {
    address public immutable inner;

    constructor(address _inner) {
        inner = _inner;
    }

    function setViaInner(uint256 v) external {
        NestedSetterInner(inner).setViaProxy(v);
    }

    function setViaInnerAndCatch(uint256 v) external {
        try NestedSetterInner(inner).setViaProxyAndRevert(v + 1) {} catch {}
        NestedSetterInner(inner).setViaProxy(v);
    }

    function setAfterInner(uint256 expectedLocal, uint256 expectedRemote, uint256 v) external {
        NestedSetterInner(inner).setAfterProxy(expectedLocal, expectedRemote, v);
    }
}
