// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {Ownable} from "@openzeppelin/contracts/access/Ownable.sol";
import {FeeVault} from "../src/L2/FeeVault.sol";

contract RejectFeePayment {
    receive() external payable {
        revert("reject payment");
    }
}

contract ReenterFeePayment {
    FeeVault public vault;
    bool public reentrySucceeded;
    uint256 public receipts;

    function bind(FeeVault target) external {
        vault = target;
    }

    receive() external payable {
        ++receipts;
        if (receipts == 1) {
            // Replenish the vault first: an empty-balance rejection would not
            // prove that the reentrancy guard protects a second payment.
            (bool refunded,) = address(vault).call{value: 1 ether}("");
            require(refunded, "refund failed");
            (reentrySucceeded,) = address(vault).call(abi.encodeCall(FeeVault.withdraw, ()));
        }
    }
}

contract FeeVaultTest is Test {
    address private constant OWNER = address(0x100);
    address private constant RECIPIENT = address(0x200);
    address private constant NEXT_RECIPIENT = address(0x300);
    FeeVault private vault;

    function setUp() public {
        vault = new FeeVault(OWNER, RECIPIENT);
    }

    function testConstructorBindsOwnerAndRecipient() public view {
        assertEq(vault.owner(), OWNER);
        assertEq(vault.pendingOwner(), address(0));
        assertEq(vault.recipient(), RECIPIENT);
        assertEq(address(vault).balance, 0);
    }

    function testConstructorRejectsZeroOwnerAndInvalidRecipient() public {
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableInvalidOwner.selector, address(0)));
        new FeeVault(address(0), RECIPIENT);
        vm.expectRevert(abi.encodeWithSelector(FeeVault.InvalidRecipient.selector, address(0)));
        new FeeVault(OWNER, address(0));
    }

    function testOnlyOwnerCanChangeRecipient() public {
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(this)));
        vault.setRecipient(NEXT_RECIPIENT);
        assertEq(vault.recipient(), RECIPIENT);
    }

    function testRecipientRotationAppliesToAccumulatedAndFutureFunds() public {
        vm.deal(address(vault), 5 ether);
        vm.prank(OWNER);
        vault.setRecipient(NEXT_RECIPIENT);
        assertEq(address(vault).balance, 5 ether);
        assertEq(vault.withdraw(), 5 ether);
        assertEq(RECIPIENT.balance, 0);
        assertEq(NEXT_RECIPIENT.balance, 5 ether);
        vm.deal(address(vault), 2 ether);
        assertEq(vault.withdraw(), 2 ether);
        assertEq(NEXT_RECIPIENT.balance, 7 ether);
    }

    function testAnyoneCanWithdrawOnlyToConfiguredRecipient() public {
        vm.deal(address(vault), 1 ether);
        vm.expectEmit(true, true, false, true);
        emit FeeVault.Withdrawal(RECIPIENT, address(0x400), 1 ether);
        vm.prank(address(0x400));
        assertEq(vault.withdraw(), 1 ether);
        assertEq(RECIPIENT.balance, 1 ether);
        assertEq(address(vault).balance, 0);
    }

    function testInvalidRecipientUpdatesPreserveState() public {
        address[4] memory invalid = [
            address(0),
            address(vault),
            0xffffFFFfFFffffffffffffffFfFFFfffFFFfFFfE,
            0x4B2760F84B83d6b3829E30A7573B0172B6d6402C
        ];
        for (uint256 i; i < invalid.length; ++i) {
            vm.prank(OWNER);
            vm.expectRevert(abi.encodeWithSelector(FeeVault.InvalidRecipient.selector, invalid[i]));
            vault.setRecipient(invalid[i]);
            assertEq(vault.recipient(), RECIPIENT);
        }
    }

    function testEmptyWithdrawalRejects() public {
        vm.expectRevert(FeeVault.EmptyVault.selector);
        vault.withdraw();
    }

    function testRecipientFailureKeepsFundsAndCanRecoverAfterRecipientChange() public {
        RejectFeePayment rejecting = new RejectFeePayment();
        vm.prank(OWNER);
        vault.setRecipient(address(rejecting));
        vm.deal(address(vault), 3 ether);
        vm.expectRevert(abi.encodeWithSelector(FeeVault.WithdrawalFailed.selector, address(rejecting), 3 ether));
        vault.withdraw();
        assertEq(address(vault).balance, 3 ether);
        assertEq(address(rejecting).balance, 0);
        vm.prank(OWNER);
        vault.setRecipient(RECIPIENT);
        assertEq(vault.withdraw(), 3 ether);
        assertEq(RECIPIENT.balance, 3 ether);
    }

    function testReentryCannotWithdrawAgain() public {
        ReenterFeePayment receiver = new ReenterFeePayment();
        receiver.bind(vault);
        vm.prank(OWNER);
        vault.setRecipient(address(receiver));
        vm.deal(address(vault), 4 ether);
        vault.withdraw();
        assertFalse(receiver.reentrySucceeded());
        assertEq(receiver.receipts(), 1);
        assertEq(address(receiver).balance, 3 ether);
        assertEq(address(vault).balance, 1 ether);
        // The guard releases after the first payment; the returned funds are
        // withdrawable by a separate call, even in this same transaction.
        assertEq(vault.withdraw(), 1 ether);
        assertEq(receiver.receipts(), 2);
        assertEq(address(receiver).balance, 4 ether);
        assertEq(address(vault).balance, 0);
    }

    function testOwnershipTransferRequiresAcceptance() public {
        address nextOwner = address(0x500);
        vm.prank(OWNER);
        vault.transferOwnership(nextOwner);
        assertEq(vault.owner(), OWNER);
        vm.prank(nextOwner);
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, nextOwner));
        vault.setRecipient(NEXT_RECIPIENT);
        vm.prank(nextOwner);
        vault.acceptOwnership();
        assertEq(vault.owner(), nextOwner);
        assertEq(vault.pendingOwner(), address(0));
        vm.prank(nextOwner);
        vault.setRecipient(NEXT_RECIPIENT);
        assertEq(vault.recipient(), NEXT_RECIPIENT);
    }

    function testOrdinaryTransferIsAlsoWithdrawable() public {
        vm.deal(address(this), 1 ether);
        (bool success,) = address(vault).call{value: 1 ether}("");
        assertTrue(success);
        assertEq(vault.withdraw(), 1 ether);
        assertEq(RECIPIENT.balance, 1 ether);
    }
}
