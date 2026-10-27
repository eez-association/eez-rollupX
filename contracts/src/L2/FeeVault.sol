// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Ownable2StepUpgradeable} from "@openzeppelin/contracts-upgradeable/access/Ownable2StepUpgradeable.sol";
import {ReentrancyGuardTransient} from "@openzeppelin/contracts/utils/ReentrancyGuardTransient.sol";

/// @notice Accumulates L2 base fees at a stable address and pays an owner-selected L2 recipient.
/// @dev Fee credits are EVM balance updates: they invoke no recipient or vault code.
///      Changing the recipient is an ordinary state transition, not a network-profile change.
contract FeeVault is Ownable2StepUpgradeable, ReentrancyGuardTransient {
    // Preserve these slots across upgrades; append new fields after them.
    address public recipient;
    address public pendingRecipient;
    uint256 public totalWithdrawn;
    uint256 public minWithdrawalAmount;

    error InvalidRecipient(address recipient);
    error EmptyVault();
    error WithdrawalFailed(address recipient, uint256 amount);
    error NotPendingRecipient(address caller);
    error BelowWithdrawalMinimum(uint256 balance, uint256 minimum);

    event RecipientUpdated(address indexed previousRecipient, address indexed recipient);
    event Withdrawal(address indexed recipient, address indexed caller, uint256 amount);
    event RecipientChangeProposed(address indexed recipient, address indexed proposedRecipient);
    event RecipientChangeCancelled(address indexed proposedRecipient);
    event MinWithdrawalAmountUpdated(uint256 previousAmount, uint256 newAmount);

    /// @dev The implementation is locked; genesis initializes the proxy atomically.
    constructor() {
        _disableInitializers();
    }

    function initialize(address initialOwner, address initialRecipient) external initializer {
        __Ownable_init(initialOwner);
        __Ownable2Step_init();
        _setRecipient(initialRecipient);
    }

    /// @notice Also accepts ordinary transfers; protocol fee credits do not execute this hook.
    receive() external payable {}

    /// @notice Propose a recipient; the current recipient remains active until acceptance.
    /// @dev A contract recipient must be able to call acceptRecipient itself.
    function proposeRecipient(address newRecipient) external onlyOwner {
        _validateRecipient(newRecipient);
        pendingRecipient = newRecipient;
        emit RecipientChangeProposed(recipient, newRecipient);
    }

    function cancelRecipientChange() external onlyOwner {
        address proposed = pendingRecipient;
        delete pendingRecipient;
        emit RecipientChangeCancelled(proposed);
    }

    /// @notice Accept all subsequent withdrawals, including funds already accumulated.
    function acceptRecipient() external {
        if (msg.sender != pendingRecipient) revert NotPendingRecipient(msg.sender);
        delete pendingRecipient;
        _setRecipient(msg.sender);
    }

    /// @notice Set the withdrawal threshold in wei; zero permits any positive balance.
    function setMinWithdrawalAmount(uint256 newAmount) external onlyOwner {
        emit MinWithdrawalAmountUpdated(minWithdrawalAmount, newAmount);
        minWithdrawalAmount = newAmount;
    }

    /// @notice Send the entire current balance to the configured L2 recipient.
    /// @return amount The amount transferred. The caller cannot choose a destination.
    /// @dev Anyone may trigger payment. Failure reverts the transfer and event; reentry is forbidden.
    ///      Fees charged for this transaction are credited by the EVM after execution, so the
    ///      vault can have a nonzero balance again when the transaction finishes.
    function withdraw() external nonReentrant returns (uint256 amount) {
        amount = address(this).balance;
        if (amount == 0) revert EmptyVault();
        if (amount < minWithdrawalAmount) revert BelowWithdrawalMinimum(amount, minWithdrawalAmount);
        address target = recipient;
        // Includes voluntary transfers as well as protocol fees. A failed payment
        // rolls back this counter along with the transfer and event.
        totalWithdrawn += amount;
        (bool success,) = target.call{value: amount}("");
        if (!success) revert WithdrawalFailed(target, amount);
        emit Withdrawal(target, msg.sender, amount);
    }

    function _setRecipient(address newRecipient) private {
        _validateRecipient(newRecipient);
        emit RecipientUpdated(recipient, newRecipient);
        recipient = newRecipient;
    }

    function _validateRecipient(address newRecipient) private view {
        if (
            newRecipient == address(0) || newRecipient == address(this)
                || newRecipient == 0xffffFFFfFFffffffffffffffFfFFFfffFFFfFFfE
                || newRecipient == 0x4B2760F84B83d6b3829E30A7573B0172B6d6402C
        ) revert InvalidRecipient(newRecipient);
    }
}
