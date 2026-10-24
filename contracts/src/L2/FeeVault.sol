// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {Ownable2Step} from "@openzeppelin/contracts/access/Ownable2Step.sol";
import {ReentrancyGuardTransient} from "@openzeppelin/contracts/utils/ReentrancyGuardTransient.sol";

/// @notice Accumulates L2 base fees at a stable address and pays an owner-selected L2 recipient.
/// @dev Fee credits are EVM balance updates: they invoke no recipient or vault code.
///      Changing the recipient is an ordinary state transition, not a network-profile change.
contract FeeVault is Ownable2Step, ReentrancyGuardTransient {
    address public recipient;

    error InvalidRecipient(address recipient);
    error EmptyVault();
    error WithdrawalFailed(address recipient, uint256 amount);

    event RecipientUpdated(address indexed previousRecipient, address indexed recipient);
    event Withdrawal(address indexed recipient, address indexed caller, uint256 amount);

    constructor(address initialOwner, address initialRecipient) Ownable(initialOwner) {
        _setRecipient(initialRecipient);
    }

    /// @notice Also accepts ordinary transfers; protocol fee credits do not execute this hook.
    receive() external payable {}

    /// @notice Choose the L2 recipient of subsequent withdrawals, including the accumulated balance.
    /// @dev Only the owner may update it. Rejects addresses that cannot retain protocol funds.
    function setRecipient(address newRecipient) external onlyOwner {
        _setRecipient(newRecipient);
    }

    /// @notice Send the entire current balance to the configured L2 recipient.
    /// @return amount The amount transferred. The caller cannot choose a destination.
    /// @dev Anyone may trigger payment. Failure reverts the transfer and event; reentry is forbidden.
    ///      Fees charged for this transaction are credited by the EVM after execution, so the
    ///      vault can have a nonzero balance again when the transaction finishes.
    function withdraw() external nonReentrant returns (uint256 amount) {
        amount = address(this).balance;
        if (amount == 0) revert EmptyVault();
        address target = recipient;
        (bool success,) = target.call{value: amount}("");
        if (!success) revert WithdrawalFailed(target, amount);
        emit Withdrawal(target, msg.sender, amount);
    }

    function _setRecipient(address newRecipient) private {
        if (
            newRecipient == address(0) || newRecipient == address(this)
                || newRecipient == 0xffffFFFfFFffffffffffffffFfFFFfffFFFfFFfE
                || newRecipient == 0x4B2760F84B83d6b3829E30A7573B0172B6d6402C
        ) revert InvalidRecipient(newRecipient);
        emit RecipientUpdated(recipient, newRecipient);
        recipient = newRecipient;
    }
}
