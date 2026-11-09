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

    function feeDependentCalls(address proxy) external {
        (bool ok,) = proxy.call(abi.encodeWithSignature("setValue(uint256)", 123));
        require(ok, "first cross-chain call failed");
        if (tx.gasprice > block.basefee) {
            (ok,) = proxy.call(abi.encodeWithSignature("setValue(uint256)", 456));
            require(ok, "priced cross-chain call failed");
        }
    }

    function gasDependentCalls(address proxy) external returns (bool ok) {
        (ok,) = proxy.call(abi.encodeWithSignature("setValue(uint256)", 123));
        require(ok, "first cross-chain call failed");
        // Composition and table replay spend different gas on the first call.
        // Catch a missing-entry failure so source success alone cannot validate
        // this transaction; the quote must compare the observed call effects.
        (ok,) = proxy.call(abi.encodeWithSignature("setValue(uint256)", gasleft()));
    }

    function gasDependentOutput(address proxy) external returns (uint256) {
        (bool ok,) = proxy.call(abi.encodeWithSignature("setValue(uint256)", 123));
        require(ok, "cross-chain call failed");
        // Effects match, but the final source output must also be validated.
        return gasleft();
    }
}
