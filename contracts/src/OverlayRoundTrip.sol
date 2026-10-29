// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// Calls `target` and bubbles its revert data unchanged.
function callOrBubble(address target, bytes memory data) {
    callOrBubble(target, 0, data);
}

/// Calls `target` with `value` and bubbles its revert data unchanged.
function callOrBubble(address target, uint256 value, bytes memory data) {
    (bool ok, bytes memory ret) = target.call{value: value}(data);
    if (!ok) {
        assembly {
            revert(add(ret, 32), mload(ret))
        }
    }
}

/// L2 side of the cross-chain round-trip fixtures. `value` stands in for a
/// token balance: L1 callbacks deliver and burn it, and each entry point checks
/// that it is cleared after its cross-chain calls return.
///
/// Peers are call arguments: `driver` is this chain's proxy for
/// `OverlayDriverL1`, and `probe` is L1's proxy for this contract.
contract OverlayProbeL2 {
    uint256 public value;
    uint256 public completed;
    /// Native ETH received through `receiveRefund` callbacks.
    uint256 public refunded;

    error ValueNotCleared(uint256 value);
    error RefundNotReceived(uint256 expected, uint256 received);

    /// Callback: receive native ETH returned from L1.
    function receiveRefund() external payable {
        refunded += msg.value;
    }

    /// Send the attached ETH to L1, which returns it in a callback. The
    /// value-bearing callback must be funded by this call's L2 table load.
    function startRefundRoundTrip(address driver, address probe) external payable {
        uint256 before = refunded;
        callOrBubble(driver, msg.value, abi.encodeCall(OverlayDriverL1.refund, (probe)));
        if (refunded != before + msg.value) revert RefundNotReceived(msg.value, refunded - before);
        completed++;
    }

    /// Callback: credit `amount`.
    function deliver(uint256 amount) external {
        value += amount;
    }

    /// Callback: clear the value without leaving this chain.
    function burn() external {
        value = 0;
    }

    /// Callback: clear the value, then make an L1 call that does not re-enter
    /// this chain.
    function burnAndPing(address driver) external {
        value = 0;
        callOrBubble(driver, abi.encodeCall(OverlayDriverL1.ping, ()));
        _requireCleared();
    }

    /// One L1 call that makes a delivery callback, then a burnAndPing callback.
    function startDeliverThenBurnAndPing(address driver, address probe, uint256 amount) external {
        callOrBubble(driver, abi.encodeCall(OverlayDriverL1.deliverThenBurnAndPing, (probe, driver, amount)));
        _requireCleared();
        completed++;
    }

    /// One L1 call that makes delivery and burn callbacks, then a second L1 call
    /// that does not re-enter this chain.
    function startDeliverBurnThenPing(address driver, address probe, uint256 amount) external {
        callOrBubble(driver, abi.encodeCall(OverlayDriverL1.deliverThenBurn, (probe, amount)));
        _requireCleared();
        callOrBubble(driver, abi.encodeCall(OverlayDriverL1.ping, ()));
        _requireCleared();
        completed++;
    }

    /// An L1 call that makes a delivery callback and then reverts. The failed
    /// call must undo the delivery.
    function startDeliverThenRevert(address driver, address probe, uint256 amount) external {
        (bool ok,) = driver.call(abi.encodeCall(OverlayDriverL1.deliverThenRevert, (probe, amount)));
        require(!ok, "deliverThenRevert succeeded");
        _requireCleared();
        completed++;
    }

    function _requireCleared() private view {
        if (value != 0) revert ValueNotCleared(value);
    }
}

/// L1 side of the cross-chain round-trip fixtures. `probe` is L1's proxy for
/// `OverlayProbeL2`; `driver` is L2's proxy for this contract.
contract OverlayDriverL1 {
    uint256 public pings;

    error DeliveredThenReverted();

    function ping() external {
        pings++;
    }

    /// Return the attached ETH to L2 in a value-bearing callback.
    function refund(address probe) external payable {
        callOrBubble(probe, msg.value, abi.encodeCall(OverlayProbeL2.receiveRefund, ()));
    }

    /// Deliver on L2, then burn there and call back into L1 without re-entering L2.
    function deliverThenBurnAndPing(address probe, address driver, uint256 amount) external {
        callOrBubble(probe, abi.encodeCall(OverlayProbeL2.deliver, (amount)));
        callOrBubble(probe, abi.encodeCall(OverlayProbeL2.burnAndPing, (driver)));
    }

    /// Deliver and burn on L2 as two sibling callbacks.
    function deliverThenBurn(address probe, uint256 amount) external {
        callOrBubble(probe, abi.encodeCall(OverlayProbeL2.deliver, (amount)));
        callOrBubble(probe, abi.encodeCall(OverlayProbeL2.burn, ()));
    }

    /// Deliver on L2, then revert.
    function deliverThenRevert(address probe, uint256 amount) external {
        callOrBubble(probe, abi.encodeCall(OverlayProbeL2.deliver, (amount)));
        revert DeliveredThenReverted();
    }
}
