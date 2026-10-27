// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Test} from "forge-std/Test.sol";
import {EEZ} from "eez-core-protocol/src/EEZ.sol";
import {Rollup} from "eez-core-protocol/src/rollupContract/Rollup.sol";
import {L1HistoryCode} from "../src/generated/L1HistoryCode.sol";
import {MockProofSystem} from "eez-core-protocol/test/mocks/MockProofSystem.sol";
import {
    ExpectedRootPerRollup,
    ProofSystemBatchPerVerificationEntries,
    RollupIdWithProofSystems
} from "eez-core-protocol/src/interfaces/IEEZ.sol";
import {TransparentUpgradeableProxy} from "@openzeppelin/contracts/proxy/transparent/TransparentUpgradeableProxy.sol";
import {OfflineRollup} from "../src/OfflineRollup.sol";
import {RollupDeployment} from "./utils/RollupDeployment.sol";

contract OfflineRollupTest is Test {
    EEZ internal registry;
    OfflineRollup internal manager;
    MockProofSystem internal verifier;
    uint64 internal rid;
    bytes32 internal constant ROOT = bytes32(uint256(0x123));
    bytes32 internal constant VKEY = bytes32(uint256(0x42));
    bytes32 internal constant HASH = bytes32(uint256(0xabc));

    function profile() internal view returns (OfflineRollup.Profile memory p) {
        uint64[] memory times = new uint64[](2);
        times[1] = 2000;
        p = OfflineRollup.Profile(
            block.chainid,
            900,
            ROOT,
            120,
            2,
            6,
            2,
            keccak256("fixed L1 rule document"),
            L1HistoryCode.HASH,
            address(0x1234),
            times
        );
    }

    function deploy(OfflineRollup.Profile memory p) internal returns (OfflineRollup) {
        OfflineRollup implementation = new OfflineRollup(address(registry));
        address[] memory systems = new address[](1);
        systems[0] = address(verifier);
        bytes32[] memory keys = new bytes32[](1);
        keys[0] = VKEY;
        bytes memory init = abi.encodeCall(OfflineRollup.initializeOffline, (address(this), 1, systems, keys, p));
        return OfflineRollup(address(new TransparentUpgradeableProxy(address(implementation), address(this), init)));
    }

    function setUp() public {
        vm.roll(101);
        vm.warp(1000);
        vm.setBlockhash(100, HASH);
        registry = new EEZ(address(0xDEAD));
        verifier = new MockProofSystem();
        manager = deploy(profile());
        rid = registry.registerRollup(address(manager), ROOT);
    }

    function testFeeEstimatorFixtureMatchesCompiledRuntime() public view {
        // Couple the Rust recognizer fixture to this build so a changed manager
        // cannot leave fee estimation recognizing an obsolete implementation.
        bytes memory fixture = vm.parseBytes(
            string.concat("0x", vm.readLine("../crates/eez-composer/src/composer/estimate/testdata/offline-rollup.hex"))
        );
        bytes memory compiled = vm.getDeployedCode("OfflineRollup.sol:OfflineRollup");
        assertEq(compiled.length, fixture.length);
        assertEq(keccak256(compiled), keccak256(fixture));
    }

    function testBothExactFlatContextEncodings() public view {
        bytes32 rules = keccak256(abi.encode(keccak256("EEZ_ROLLUPX_L1_RULES_V1"), profile().l1ConfigHash, uint64(0)));
        bytes memory actual = abi.encode(
            keccak256("EEZ_ROLLUPX_L1_HISTORY_V1"),
            block.chainid,
            address(registry),
            address(manager),
            rid,
            uint256(900),
            manager.protocolConfigHash(),
            rules,
            ROOT,
            uint8(1),
            uint64(100),
            HASH,
            uint64(1000)
        );
        assertEq(manager.getCustomData(type(uint64).max), actual);
        bytes memory historical = abi.encode(
            keccak256("EEZ_ROLLUPX_L1_HISTORY_V1"),
            block.chainid,
            address(registry),
            address(manager),
            rid,
            uint256(900),
            manager.protocolConfigHash(),
            rules,
            ROOT,
            uint8(2),
            uint64(100),
            HASH
        );
        assertEq(manager.getCustomData(100), historical);
        assertEq(actual.length, 13 * 32);
        assertEq(historical.length, 12 * 32);
    }

    function testHistoricalAgesOneAnd256Pass257Rejects() public {
        bytes memory expected = manager.getCustomData(100);
        vm.roll(356);
        vm.setBlockhash(100, HASH);
        assertEq(manager.getCustomData(100), expected);
        vm.roll(357);
        vm.expectRevert(abi.encodeWithSelector(OfflineRollup.BlockHashUnavailable.selector, uint64(100)));
        manager.getCustomData(100);
    }

    function testReservedCurrentFutureAndUnavailableAnchorsReject() public {
        vm.expectRevert(OfflineRollup.InvalidAnchorSelector.selector);
        manager.getCustomData(0);
        for (uint64 n = 101; n <= 102; ++n) {
            vm.expectRevert(abi.encodeWithSelector(OfflineRollup.BlockHashUnavailable.selector, n));
            manager.getCustomData(n);
        }
        vm.setBlockhash(99, bytes32(0));
        vm.expectRevert(abi.encodeWithSelector(OfflineRollup.BlockHashUnavailable.selector, uint64(99)));
        manager.getCustomData(99);
        vm.setBlockhash(100, bytes32(0));
        vm.expectRevert(abi.encodeWithSelector(OfflineRollup.BlockHashUnavailable.selector, uint64(100)));
        manager.getCustomData(type(uint64).max);
    }

    function testInclusionTimeChangesOnlyActualContextWithinRulePeriod() public {
        bytes memory historical = manager.getCustomData(100);
        bytes memory actual = manager.getCustomData(type(uint64).max);
        vm.warp(1999);
        assertEq(manager.getCustomData(100), historical);
        assertNotEq(manager.getCustomData(type(uint64).max), actual);
        vm.warp(2000);
        assertNotEq(manager.getCustomData(100), historical);
    }

    function testAcceptedRootAndAnchorBranchAreBound() public {
        bytes memory beforeRoot = manager.getCustomData(100);
        manager.setRoot(bytes32(uint256(0x456)));
        assertNotEq(manager.getCustomData(100), beforeRoot);
        bytes memory beforeReorg = manager.getCustomData(100);
        vm.setBlockhash(100, bytes32(uint256(0xabd)));
        assertNotEq(manager.getCustomData(100), beforeReorg);
    }

    function testCanonicalProfileCommitmentAndUnregisteredContextRejects() public {
        bytes memory expected = abi.encode(
            keccak256("EEZ_ROLLUPX_OFFLINE_PROFILE"),
            uint64(4),
            manager.HISTORY_ADDRESS(),
            uint64(64),
            uint64(2_000_000),
            profile()
        );
        assertEq(manager.protocolConfigHash(), keccak256(expected));
        OfflineRollup unregistered = deploy(profile());
        vm.expectRevert(OfflineRollup.UnregisteredOfflineRollup.selector);
        unregistered.getCustomData(100);
    }

    function testCollectorIsRequiredAndChangesTheAuthenticatedProfile() public {
        OfflineRollup.Profile memory p = profile();
        address[3] memory invalid = [
            address(0),
            address(0xffffFFFfFFffffffffffffffFfFFFfffFFFfFFfE),
            address(0x4B2760F84B83d6b3829E30A7573B0172B6d6402C)
        ];
        for (uint256 i; i < invalid.length; ++i) {
            p.feeCollector = invalid[i];
            vm.expectRevert(OfflineRollup.InvalidOfflineProfile.selector);
            this.deployExternal(p);
        }
        p.feeCollector = address(0x5678);
        OfflineRollup changed = deploy(p);
        assertNotEq(changed.protocolConfigHash(), manager.protocolConfigHash());
    }

    function testOnlyProfileInitializerExistsAndImplementationIsLocked() public {
        vm.expectRevert(OfflineRollup.InvalidEEZContract.selector);
        new OfflineRollup(address(0));
        OfflineRollup implementation = new OfflineRollup(address(registry));
        address[] memory systems = new address[](1);
        systems[0] = address(verifier);
        bytes32[] memory keys = new bytes32[](1);
        keys[0] = VKEY;
        vm.expectRevert(abi.encodeWithSignature("InvalidInitialization()"));
        implementation.initializeOffline(address(this), 1, systems, keys, profile());
        vm.expectRevert(abi.encodeWithSignature("InvalidInitialization()"));
        manager.initializeOffline(address(this), 1, systems, keys, profile());

        (bool accepted, bytes memory data) =
            address(implementation).call(abi.encodeCall(Rollup.initialize, (address(this), 0, systems, keys)));
        assertFalse(accepted, "context-free initialization must not be available");
        assertEq(data.length, 0, "unknown selector must revert without entering an initializer");
        assertEq(implementation.owner(), address(0));
        assertEq(manager.owner(), address(this));
        assertEq(manager.threshold(), 1);
        assertEq(manager.verificationKey(address(verifier)), VKEY);
    }

    function deployPolicy(address[] memory systems, bytes32[] memory keys) external returns (OfflineRollup) {
        OfflineRollup implementation = new OfflineRollup(address(registry));
        bytes memory init =
            abi.encodeCall(OfflineRollup.initializeOffline, (address(this), 1, systems, keys, profile()));
        return OfflineRollup(address(new TransparentUpgradeableProxy(address(implementation), address(this), init)));
    }

    function testInvalidInitialPolicyRejectsAtomically() public {
        address[] memory systems = new address[](2);
        systems[0] = address(verifier);
        systems[1] = address(verifier);
        bytes32[] memory keys = new bytes32[](1);
        keys[0] = VKEY;
        vm.expectRevert(OfflineRollup.InvalidConfig.selector);
        this.deployPolicy(systems, keys);
        keys = new bytes32[](2);
        keys[0] = VKEY;
        keys[1] = VKEY;
        vm.expectRevert(abi.encodeWithSelector(OfflineRollup.ProofSystemAlreadyAllowed.selector, address(verifier)));
        this.deployPolicy(systems, keys);
        systems[1] = address(0xBEEF);
        keys[1] = bytes32(0);
        vm.expectRevert(OfflineRollup.InvalidConfig.selector);
        this.deployPolicy(systems, keys);
    }

    function testRegistrationRequiresRegistryAndCurrentOwnerAndIsOneShot() public {
        OfflineRollup fresh = deploy(profile());
        vm.expectRevert(OfflineRollup.NotEEZRegistry.selector);
        fresh.rollupContractRegistered(rid + 1, address(this));
        address nextOwner = address(0xBEEF);
        fresh.transferOwnership(nextOwner);
        uint64 counter = registry.rollupCounter();
        vm.expectRevert(abi.encodeWithSelector(OfflineRollup.UnauthorizedRegistrantAccount.selector, address(this)));
        registry.registerRollup(address(fresh), ROOT);
        assertEq(fresh.rollupId(), 0);
        assertEq(registry.rollupCounter(), counter);
        vm.prank(nextOwner);
        assertEq(registry.registerRollup(address(fresh), ROOT), counter + 1);
        vm.prank(address(registry));
        vm.expectRevert(OfflineRollup.AlreadyRegistered.selector);
        fresh.rollupContractRegistered(counter + 2, nextOwner);
        assertEq(fresh.rollupId(), counter + 1);
    }

    function testOwnerPolicyChangesAndRootEscape() public {
        address extra = address(0xBEEF);
        manager.addProofSystem(extra, VKEY);
        bytes32 replacement = bytes32(uint256(0x43));
        manager.updateVerificationKey(extra, replacement);
        assertEq(manager.verificationKey(extra), replacement);
        manager.setThreshold(2);
        assertEq(manager.threshold(), 2);
        manager.removeProofSystem(extra);
        assertEq(manager.verificationKey(extra), bytes32(0));
        vm.expectRevert(abi.encodeWithSelector(OfflineRollup.ProofSystemNotAdded.selector, extra));
        manager.removeProofSystem(extra);
        vm.expectRevert(abi.encodeWithSelector(OfflineRollup.ProofSystemNotAdded.selector, extra));
        manager.updateVerificationKey(extra, VKEY);
        vm.expectRevert(OfflineRollup.InvalidConfig.selector);
        manager.updateVerificationKey(address(verifier), bytes32(0));
        vm.expectRevert(OfflineRollup.InvalidConfig.selector);
        manager.addProofSystem(extra, bytes32(0));
        vm.expectRevert(abi.encodeWithSelector(OfflineRollup.ProofSystemAlreadyAllowed.selector, address(verifier)));
        manager.addProofSystem(address(verifier), replacement);

        bytes[] memory calls = new bytes[](5);
        calls[0] = abi.encodeCall(OfflineRollup.addProofSystem, (extra, VKEY));
        calls[1] = abi.encodeCall(OfflineRollup.removeProofSystem, (address(verifier)));
        calls[2] = abi.encodeCall(OfflineRollup.updateVerificationKey, (address(verifier), replacement));
        calls[3] = abi.encodeCall(OfflineRollup.setThreshold, (0));
        calls[4] = abi.encodeCall(OfflineRollup.setRoot, (replacement));
        for (uint256 i; i < calls.length; ++i) {
            vm.prank(extra);
            (bool success, bytes memory errorData) = address(manager).call(calls[i]);
            assertFalse(success);
            assertEq(errorData, abi.encodeWithSignature("OwnableUnauthorizedAccount(address)", extra));
        }
        manager.setThreshold(1);
        verifier.setExpectedPublicInputsHash(hashForContext(manager.getCustomData(100)));
        registry.postAndVerifyBatch(batch(100));
        vm.expectRevert(abi.encodeWithSelector(EEZ.RollupBatchActiveThisBlock.selector, rid));
        manager.setRoot(replacement);
        (, bytes32 root,) = registry.rollups(rid);
        assertEq(root, ROOT);
        vm.roll(block.number + 1);
        manager.setRoot(replacement);
        (, root,) = registry.rollups(rid);
        assertEq(root, replacement);
    }

    function testFuzzProofPolicyMatchesPinnedReference(uint8 first, uint8 second, uint8 count, uint8 threshold) public {
        address[] memory systems = new address[](1);
        systems[0] = address(verifier);
        bytes32[] memory keys = new bytes32[](1);
        keys[0] = VKEY;
        uint256 required = bound(threshold, 0, 3);
        Rollup referenceManager = RollupDeployment.deploy(address(registry), address(this), required, systems, keys);
        manager.setThreshold(required);
        referenceManager.addProofSystem(address(0xBEEF), bytes32(uint256(0x43)));
        manager.addProofSystem(address(0xBEEF), bytes32(uint256(0x43)));
        address[4] memory choices = [address(0), address(0xBEEF), address(verifier), address(0xDEAD)];
        address[] memory subset = new address[](bound(count, 0, 2));
        if (subset.length > 0) subset[0] = choices[first % 4];
        if (subset.length > 1) subset[1] = choices[second % 4];
        bytes memory input = abi.encodeCall(OfflineRollup.checkProofSystemsAndGetVkeys, (subset));
        (bool expectedSuccess, bytes memory expected) = address(referenceManager).staticcall(input);
        (bool actualSuccess, bytes memory actual) = address(manager).staticcall(input);
        assertEq(actualSuccess, expectedSuccess);
        assertEq(actual, expected);
    }

    function testRegistrationRejectsDifferentGenesisAtomically() public {
        OfflineRollup fresh = deploy(profile());
        uint64 beforeCounter = registry.rollupCounter();
        vm.expectRevert(OfflineRollup.InvalidOfflineProfile.selector);
        registry.registerRollup(address(fresh), bytes32(uint256(0xdead)));
        assertEq(fresh.rollupId(), 0);
        assertEq(registry.rollupCounter(), beforeCounter);
        assertEq(registry.registerRollup(address(fresh), ROOT), beforeCounter + 1);
        assertEq(fresh.initialBlockHash(), ROOT);
    }

    function testInvalidProfileRejectsAtInitialization() public {
        OfflineRollup.Profile memory p = profile();
        p.updateOffset = p.syncInterval;
        vm.expectRevert(OfflineRollup.InvalidOfflineProfile.selector);
        this.deployExternal(p);
        p = profile();
        p.historyCodeHash = bytes32(uint256(1));
        vm.expectRevert(OfflineRollup.InvalidOfflineProfile.selector);
        this.deployExternal(p);
        p = profile();
        p.ruleTimes[1] = 0;
        vm.expectRevert(OfflineRollup.InvalidOfflineProfile.selector);
        this.deployExternal(p);
    }

    function deployExternal(OfflineRollup.Profile memory p) external returns (OfflineRollup) {
        return deploy(p);
    }

    function batch(uint64 selector) internal view returns (ProofSystemBatchPerVerificationEntries memory b) {
        b.blockNumber = selector;
        b.expectedRootPerRollup = new ExpectedRootPerRollup[](1);
        b.expectedRootPerRollup[0] = ExpectedRootPerRollup(rid, ROOT);
        b.proofSystems = new address[](1);
        b.proofSystems[0] = address(verifier);
        b.proofs = new bytes[](1);
        b.rollupIdsWithProofSystems = new RollupIdWithProofSystems[](1);
        uint64[] memory indexes = new uint64[](1);
        b.rollupIdsWithProofSystems[0] = RollupIdWithProofSystems(rid, indexes);
    }

    function hashForContext(bytes memory context) internal view returns (bytes32) {
        bytes32[] memory empty = new bytes32[](0);
        bytes32[] memory custom = new bytes32[](1);
        custom[0] = keccak256(abi.encode(rid, context));
        bytes32 shared = keccak256(
            abi.encodePacked(
                abi.encode(empty),
                abi.encode(empty),
                abi.encode(empty),
                keccak256(bytes("")),
                abi.encode(custom),
                address(0)
            )
        );
        bytes32 acc = keccak256(abi.encode(bytes32(0), rid, VKEY));
        return keccak256(abi.encodePacked(shared, acc));
    }

    // This tests real registry context verification, not complete signer/DA acceptance.
    function testRegistryRejectsContextSubstitutionBeforeAcceptingProof() public {
        ProofSystemBatchPerVerificationEntries memory b = batch(100);
        verifier.setExpectedPublicInputsHash(hashForContext(manager.getCustomData(100)));
        registry.postAndVerifyBatch(b);
        vm.warp(1500);
        registry.postAndVerifyBatch(b);
        b.blockNumber = type(uint64).max;
        vm.expectRevert(EEZ.InvalidProof.selector);
        registry.postAndVerifyBatch(b);
        b.blockNumber = 100;
        vm.warp(2000);
        vm.expectRevert(EEZ.InvalidProof.selector);
        registry.postAndVerifyBatch(b);
        (, bytes32 root,) = registry.rollups(rid);
        assertEq(root, ROOT);
    }
}
