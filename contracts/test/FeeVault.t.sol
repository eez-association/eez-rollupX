// SPDX-License-Identifier: MIT
pragma solidity ^0.8.28;

import {Test} from "forge-std/Test.sol";
import {OwnableUpgradeable as Ownable} from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";
import {Initializable} from "@openzeppelin/contracts/proxy/utils/Initializable.sol";
import {
    TransparentUpgradeableProxy,
    ITransparentUpgradeableProxy
} from "@openzeppelin/contracts/proxy/transparent/TransparentUpgradeableProxy.sol";
import {ProxyAdmin} from "@openzeppelin/contracts/proxy/transparent/ProxyAdmin.sol";
import {IERC1967} from "@openzeppelin/contracts/interfaces/IERC1967.sol";
import {FeeVault} from "../src/L2/FeeVault.sol";

contract RejectFeePayment {
    function acceptRecipient(FeeVault target) external {
        target.acceptRecipient();
    }

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

    function acceptRecipient() external {
        vault.acceptRecipient();
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

contract FeeVaultNext is FeeVault {
    uint256 public addedValue;

    function initializeNext(uint256 value) external reinitializer(2) {
        require(value != 0, "invalid migration");
        addedValue = value;
    }
}

contract FeeVaultTest is Test {
    address private constant OWNER = address(0x100);
    address private constant RECIPIENT = address(0x200);
    address private constant NEXT_RECIPIENT = address(0x300);
    FeeVault private vault;
    address private implementation;
    bytes32 private constant IMPLEMENTATION_SLOT = 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;
    bytes32 private constant ADMIN_SLOT = 0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103;

    function setUp() public {
        implementation = address(new FeeVault());
        vault = FeeVault(
            payable(address(
                    new TransparentUpgradeableProxy(
                        implementation, OWNER, abi.encodeCall(FeeVault.initialize, (OWNER, RECIPIENT))
                    )
                ))
        );
    }

    function testConstructorBindsOwnerAndRecipient() public view {
        assertEq(vault.owner(), OWNER);
        assertEq(vault.pendingOwner(), address(0));
        assertEq(vault.recipient(), RECIPIENT);
        assertEq(address(vault).balance, 0);
    }

    function testConstructorRejectsZeroOwnerAndInvalidRecipient() public {
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableInvalidOwner.selector, address(0)));
        new TransparentUpgradeableProxy(
            implementation, OWNER, abi.encodeCall(FeeVault.initialize, (address(0), RECIPIENT))
        );
        vm.expectRevert(abi.encodeWithSelector(FeeVault.InvalidRecipient.selector, address(0)));
        new TransparentUpgradeableProxy(implementation, OWNER, abi.encodeCall(FeeVault.initialize, (OWNER, address(0))));
    }

    function testOnlyOwnerCanChangeRecipient() public {
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(this)));
        vault.proposeRecipient(NEXT_RECIPIENT);
        assertEq(vault.recipient(), RECIPIENT);
    }

    function testRecipientRotationAppliesToAccumulatedAndFutureFunds() public {
        vm.deal(address(vault), 5 ether);
        vm.prank(OWNER);
        vault.proposeRecipient(NEXT_RECIPIENT);
        vm.prank(NEXT_RECIPIENT);
        vault.acceptRecipient();
        assertEq(address(vault).balance, 5 ether);
        assertEq(vault.withdraw(), 5 ether);
        assertEq(RECIPIENT.balance, 0);
        assertEq(NEXT_RECIPIENT.balance, 5 ether);
        vm.deal(address(vault), 2 ether);
        assertEq(vault.withdraw(), 2 ether);
        assertEq(NEXT_RECIPIENT.balance, 7 ether);
        assertEq(vault.totalWithdrawn(), 7 ether);
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
            vault.proposeRecipient(invalid[i]);
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
        vault.proposeRecipient(address(rejecting));
        rejecting.acceptRecipient(vault);
        vm.deal(address(vault), 3 ether);
        vm.expectRevert(abi.encodeWithSelector(FeeVault.WithdrawalFailed.selector, address(rejecting), 3 ether));
        vault.withdraw();
        assertEq(address(vault).balance, 3 ether);
        assertEq(vault.totalWithdrawn(), 0);
        assertEq(address(rejecting).balance, 0);
        vm.prank(OWNER);
        vault.proposeRecipient(RECIPIENT);
        vm.prank(RECIPIENT);
        vault.acceptRecipient();
        assertEq(vault.withdraw(), 3 ether);
        assertEq(RECIPIENT.balance, 3 ether);
    }

    function testReentryCannotWithdrawAgain() public {
        ReenterFeePayment receiver = new ReenterFeePayment();
        receiver.bind(vault);
        vm.prank(OWNER);
        vault.proposeRecipient(address(receiver));
        receiver.acceptRecipient();
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
        vault.proposeRecipient(NEXT_RECIPIENT);
        vm.prank(nextOwner);
        vault.acceptOwnership();
        assertEq(vault.owner(), nextOwner);
        assertEq(vault.pendingOwner(), address(0));
        ProxyAdmin admin = ProxyAdmin(address(uint160(uint256(vm.load(address(vault), ADMIN_SLOT)))));
        assertEq(admin.owner(), OWNER, "treasury ownership must not silently transfer upgrade authority");
        vm.prank(nextOwner);
        vault.proposeRecipient(NEXT_RECIPIENT);
        vm.prank(NEXT_RECIPIENT);
        vault.acceptRecipient();
        assertEq(vault.recipient(), NEXT_RECIPIENT);
    }

    function testOrdinaryTransferIsAlsoWithdrawable() public {
        vm.deal(address(this), 1 ether);
        (bool success,) = address(vault).call{value: 1 ether}("");
        assertTrue(success);
        assertEq(vault.withdraw(), 1 ether);
        assertEq(RECIPIENT.balance, 1 ether);
    }

    function testImplementationAndProxyCannotBeInitializedAgain() public {
        vm.expectRevert(Initializable.InvalidInitialization.selector);
        FeeVault(payable(implementation)).initialize(OWNER, RECIPIENT);
        vm.expectRevert(Initializable.InvalidInitialization.selector);
        vault.initialize(OWNER, RECIPIENT);
    }

    function testRecipientProposalRequiresAcceptanceAndCanBeReplacedOrCancelled() public {
        vm.prank(OWNER);
        vault.proposeRecipient(NEXT_RECIPIENT);
        assertEq(vault.recipient(), RECIPIENT);
        assertEq(vault.pendingRecipient(), NEXT_RECIPIENT);
        vm.expectRevert(abi.encodeWithSelector(FeeVault.NotPendingRecipient.selector, address(this)));
        vault.acceptRecipient();
        vm.prank(OWNER);
        vault.proposeRecipient(address(0x301));
        vm.prank(NEXT_RECIPIENT);
        vm.expectRevert(abi.encodeWithSelector(FeeVault.NotPendingRecipient.selector, NEXT_RECIPIENT));
        vault.acceptRecipient();
        vm.prank(OWNER);
        vault.cancelRecipientChange();
        vm.prank(address(0x301));
        vm.expectRevert(abi.encodeWithSelector(FeeVault.NotPendingRecipient.selector, address(0x301)));
        vault.acceptRecipient();
        assertEq(vault.pendingRecipient(), address(0));
        assertEq(vault.recipient(), RECIPIENT);
    }

    function testWithdrawalUsesActiveRecipientUntilAcceptance() public {
        vm.prank(OWNER);
        vault.proposeRecipient(NEXT_RECIPIENT);
        vm.deal(address(vault), 1 ether);
        vault.withdraw();
        assertEq(RECIPIENT.balance, 1 ether);
        assertEq(NEXT_RECIPIENT.balance, 0);
        vm.prank(NEXT_RECIPIENT);
        vault.acceptRecipient();
        assertEq(vault.pendingRecipient(), address(0));
    }

    function testMinimumWithdrawalBoundaryAndReset() public {
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(this)));
        vault.setMinWithdrawalAmount(10);
        vm.prank(OWNER);
        vault.setMinWithdrawalAmount(10);
        vm.deal(address(vault), 9);
        vm.expectRevert(abi.encodeWithSelector(FeeVault.BelowWithdrawalMinimum.selector, 9, 10));
        vault.withdraw();
        assertEq(vault.totalWithdrawn(), 0);
        vm.deal(address(vault), 10);
        vault.withdraw();
        assertEq(vault.totalWithdrawn(), 10);
        vm.prank(OWNER);
        vault.setMinWithdrawalAmount(0);
        vm.deal(address(vault), 1);
        vault.withdraw();
        assertEq(vault.totalWithdrawn(), 11);
    }

    function testUpgradeRetainsTreasuryStateAndRejectsUnauthorizedCaller() public {
        vm.deal(address(vault), 4 ether);
        vault.withdraw();
        vm.deal(address(vault), 2 ether);
        vm.startPrank(OWNER);
        vault.proposeRecipient(NEXT_RECIPIENT);
        vault.setMinWithdrawalAmount(1 ether);
        vault.transferOwnership(address(0x500));
        vm.stopPrank();
        ProxyAdmin admin = ProxyAdmin(address(uint160(uint256(vm.load(address(vault), ADMIN_SLOT)))));
        FeeVaultNext next = new FeeVaultNext();
        vm.expectRevert(abi.encodeWithSelector(Ownable.OwnableUnauthorizedAccount.selector, address(this)));
        admin.upgradeAndCall(ITransparentUpgradeableProxy(address(vault)), address(next), "");
        vm.expectEmit(true, false, false, true, address(vault));
        emit IERC1967.Upgraded(address(next));
        vm.prank(OWNER);
        admin.upgradeAndCall(
            ITransparentUpgradeableProxy(address(vault)),
            address(next),
            abi.encodeCall(FeeVaultNext.initializeNext, (7))
        );
        assertEq(address(uint160(uint256(vm.load(address(vault), IMPLEMENTATION_SLOT)))), address(next));
        assertEq(FeeVaultNext(payable(address(vault))).addedValue(), 7);
        assertEq(vault.owner(), OWNER);
        assertEq(vault.pendingOwner(), address(0x500));
        assertEq(vault.recipient(), RECIPIENT);
        assertEq(vault.pendingRecipient(), NEXT_RECIPIENT);
        assertEq(vault.minWithdrawalAmount(), 1 ether);
        assertEq(vault.totalWithdrawn(), 4 ether);
        assertEq(address(vault).balance, 2 ether);
        vm.prank(NEXT_RECIPIENT);
        vault.acceptRecipient();
        vault.withdraw();
        assertEq(vault.totalWithdrawn(), 6 ether);
        assertEq(NEXT_RECIPIENT.balance, 2 ether);
    }

    function testRevertingUpgradeIsAtomic() public {
        ProxyAdmin admin = ProxyAdmin(address(uint160(uint256(vm.load(address(vault), ADMIN_SLOT)))));
        FeeVaultNext next = new FeeVaultNext();
        vm.deal(address(vault), 2 ether);
        vm.prank(OWNER);
        vm.expectRevert("invalid migration");
        admin.upgradeAndCall(
            ITransparentUpgradeableProxy(address(vault)),
            address(next),
            abi.encodeCall(FeeVaultNext.initializeNext, (0))
        );
        assertEq(address(uint160(uint256(vm.load(address(vault), IMPLEMENTATION_SLOT)))), implementation);
        assertEq(vault.owner(), OWNER);
        assertEq(vault.recipient(), RECIPIENT);
        assertEq(address(vault).balance, 2 ether);
        assertEq(vault.withdraw(), 2 ether);
    }

    function testGovernanceEventsIdentifyProposalAcceptanceCancellationAndThreshold() public {
        vm.expectEmit(true, true, false, true, address(vault));
        emit FeeVault.RecipientChangeProposed(RECIPIENT, NEXT_RECIPIENT);
        vm.prank(OWNER);
        vault.proposeRecipient(NEXT_RECIPIENT);
        vm.expectEmit(true, true, false, true, address(vault));
        emit FeeVault.RecipientUpdated(RECIPIENT, NEXT_RECIPIENT);
        vm.prank(NEXT_RECIPIENT);
        vault.acceptRecipient();
        vm.prank(OWNER);
        vault.proposeRecipient(RECIPIENT);
        vm.expectEmit(true, false, false, true, address(vault));
        emit FeeVault.RecipientChangeCancelled(RECIPIENT);
        vm.prank(OWNER);
        vault.cancelRecipientChange();
        vm.expectEmit(false, false, false, true, address(vault));
        emit FeeVault.MinWithdrawalAmountUpdated(0, 1 ether);
        vm.prank(OWNER);
        vault.setMinWithdrawalAmount(1 ether);
    }
}
