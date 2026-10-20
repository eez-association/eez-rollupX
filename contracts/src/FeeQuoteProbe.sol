// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

/// Process-test fixture for gas discovery and final fee-dependent execution.
contract FeeQuoteProbe {
    function alwaysReverts() external pure {
        revert("quote probe rejected");
    }

    function gasSensitive() external view returns (uint256) {
        return gasleft() < 400_000 ? 1 : 2;
    }

    function rejectsSettlementTip(address proxy) external {
        require(tx.gasprice <= block.basefee, "positive tip rejected");
        (bool ok,) = proxy.call(abi.encodeWithSignature("setValue(uint256)", 123));
        require(ok, "cross-chain call failed");
    }
}
