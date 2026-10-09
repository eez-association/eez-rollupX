// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// Read-only view of `Counter.count`.
interface ICountView {
    function count() external view returns (uint256);
}

/// Static round-trip fixtures: a top-level static read whose remote target
/// makes one static callback to the reader's chain. They mirror the protocol's
/// `topLevelStaticReentrantCounter` scenarios and work in either direction.
///
/// On the remote chain, `StaticCountForwarder.count()` delegates to
/// `StaticCountCallback`, which reads `count()` through that chain's proxy for
/// a `Counter` on the reader's chain. The separate helper makes the callback
/// source differ from the read target.
contract StaticCountCallback is ICountView {
    ICountView public immutable target;

    constructor(address counterProxy) {
        target = ICountView(counterProxy);
    }

    function count() external view returns (uint256) {
        return target.count();
    }
}

contract StaticCountForwarder is ICountView {
    ICountView public immutable callback;

    constructor(address callbackHelper) {
        callback = ICountView(callbackHelper);
    }

    function count() external view returns (uint256) {
        return callback.count();
    }
}

/// Source transaction: one static read through `forwarderProxy`, then a local
/// write recording the result.
contract StaticRoundTripReader {
    ICountView public immutable forwarderProxy;
    uint256 public lastRead;
    uint256 public reads;

    constructor(address proxy) {
        forwarderProxy = ICountView(proxy);
    }

    function read() external returns (uint256) {
        lastRead = forwarderProxy.count();
        reads++;
        return lastRead;
    }
}
