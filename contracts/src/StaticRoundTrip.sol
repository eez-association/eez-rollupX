// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// Read-only view of `Counter.count`.
interface ICountView {
    function count() external view returns (uint256);
}

interface IStaticProxyManager {
    function computeCrossChainProxyAddress(address original, uint64 rollup) external view returns (address);
    function getOrCreateCrossChainProxy(address original, uint64 rollup) external returns (address);
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
    bytes32 public missingProxyErrorHash;

    constructor(address proxy) {
        forwarderProxy = ICountView(proxy);
    }

    function read() external returns (uint256) {
        lastRead = forwarderProxy.count();
        reads++;
        return lastRead;
    }

    /// Catch the resolver failure, deploy its missing callback proxy, and retry
    /// within the same transaction and source-table load.
    function readAfterDeployingCallback(
        address manager,
        address callback,
        uint64 remoteRollup
    )
        external
        returns (uint256)
    {
        address expectedProxy = IStaticProxyManager(manager).computeCrossChainProxyAddress(callback, remoteRollup);
        require(expectedProxy.code.length == 0, "callback proxy already exists");
        try forwarderProxy.count() returns (uint256) {
            revert("missing callback proxy unexpectedly resolved");
        } catch (bytes memory reason) {
            missingProxyErrorHash = keccak256(reason);
            require(
                missingProxyErrorHash
                    == keccak256(abi.encodeWithSignature("StaticCallProxyNotDeployed(address)", expectedProxy)),
                "wrong missing-proxy error"
            );
        }
        require(IStaticProxyManager(manager).getOrCreateCrossChainProxy(callback, remoteRollup) == expectedProxy);
        lastRead = forwarderProxy.count();
        reads++;
        return lastRead;
    }
}
