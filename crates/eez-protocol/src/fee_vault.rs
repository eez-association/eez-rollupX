//! Ordinary L2 treasury predeploy. Execution credits its balance without calling it.

use alloy_primitives::{Address, address};

/// Fixed fee collector installed by the fresh-network genesis renderer.
pub const FEE_VAULT_ADDRESS: Address = address!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee0078");

alloy_sol_types::sol! {
    /// Owner-controlled recipient and permissionless L2 payment interface.
    interface IFeeVault {
        function owner() external view returns (address);
        function recipient() external view returns (address);
        function pendingRecipient() external view returns (address);
        function totalWithdrawn() external view returns (uint256);
        function minWithdrawalAmount() external view returns (uint256);
        function proposeRecipient(address newRecipient) external;
        function acceptRecipient() external;
        function cancelRecipientChange() external;
        function setMinWithdrawalAmount(uint256 newAmount) external;
        function withdraw() external returns (uint256 amount);
        event RecipientChangeProposed(address indexed recipient, address indexed proposedRecipient);
        event RecipientChangeCancelled(address indexed proposedRecipient);
        event RecipientUpdated(address indexed previousRecipient, address indexed recipient);
        event MinWithdrawalAmountUpdated(uint256 previousAmount, uint256 newAmount);
        event Withdrawal(address indexed recipient, address indexed caller, uint256 amount);
        error OwnableUnauthorizedAccount(address account);
    }
}
