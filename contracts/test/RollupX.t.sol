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
import {
    TransparentUpgradeableProxy,
    ITransparentUpgradeableProxy
} from "@openzeppelin/contracts/proxy/transparent/TransparentUpgradeableProxy.sol";
import {ProxyAdmin} from "@openzeppelin/contracts/proxy/transparent/ProxyAdmin.sol";
import {RollupX} from "../src/RollupX.sol";
import {RollupDeployment} from "./utils/RollupDeployment.sol";

contract RollupXTest is Test {
    EEZ internal registry;
    RollupX internal manager;
    MockProofSystem internal verifier;
    uint64 internal rid;
    bytes32 internal constant ROOT = bytes32(uint256(0x123));
    bytes32 internal constant VKEY = bytes32(uint256(0x42));
    bytes32 internal constant HASH = bytes32(uint256(0xabc));

    function profile() internal view returns (RollupX.Profile memory p) {
        uint64[] memory times = new uint64[](2);
        times[1] = 2000;
        p = RollupX.Profile(
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

    function deploy(RollupX.Profile memory p) internal returns (RollupX) {
        RollupX implementation = new RollupX(address(registry));
        address[] memory systems = new address[](1);
        systems[0] = address(verifier);
        bytes32[] memory keys = new bytes32[](1);
        keys[0] = VKEY;
        bytes memory init = abi.encodeCall(RollupX.initialize, (address(this), 1, systems, keys, p));
        return RollupX(address(new TransparentUpgradeableProxy(address(implementation), address(this), init)));
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
            string.concat("0x", vm.readLine("../crates/eez-composer/src/composer/estimate/testdata/rollupx.hex"))
        );
        bytes memory compiled = vm.getDeployedCode("RollupX.sol:RollupX");
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
        vm.expectRevert(abi.encodeWithSelector(RollupX.BlockHashUnavailable.selector, uint64(100)));
        manager.getCustomData(100);
    }

    function testReservedCurrentFutureAndUnavailableAnchorsReject() public {
        vm.expectRevert(RollupX.InvalidAnchorSelector.selector);
        manager.getCustomData(0);
        for (uint64 n = 101; n <= 102; ++n) {
            vm.expectRevert(abi.encodeWithSelector(RollupX.BlockHashUnavailable.selector, n));
            manager.getCustomData(n);
        }
        vm.setBlockhash(99, bytes32(0));
        vm.expectRevert(abi.encodeWithSelector(RollupX.BlockHashUnavailable.selector, uint64(99)));
        manager.getCustomData(99);
        vm.setBlockhash(100, bytes32(0));
        vm.expectRevert(abi.encodeWithSelector(RollupX.BlockHashUnavailable.selector, uint64(100)));
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
            uint64(6),
            manager.HISTORY_ADDRESS(),
            uint64(64),
            uint64(2_000_000),
            profile()
        );
        assertEq(manager.protocolConfigHash(), keccak256(expected));
        RollupX unregistered = deploy(profile());
        vm.expectRevert(RollupX.UnregisteredOfflineRollup.selector);
        unregistered.getCustomData(100);
    }

    function testCollectorIsRequiredAndChangesTheAuthenticatedProfile() public {
        RollupX.Profile memory p = profile();
        address[3] memory invalid = [
            address(0),
            address(0xffffFFFfFFffffffffffffffFfFFFfffFFFfFFfE),
            address(0x4B2760F84B83d6b3829E30A7573B0172B6d6402C)
        ];
        for (uint256 i; i < invalid.length; ++i) {
            p.feeCollector = invalid[i];
            vm.expectRevert(RollupX.InvalidOfflineProfile.selector);
            this.deployExternal(p);
        }
        p.feeCollector = address(0x5678);
        RollupX changed = deploy(p);
        assertNotEq(changed.protocolConfigHash(), manager.protocolConfigHash());
    }

    function testOnlyProfileInitializerExistsAndImplementationIsLocked() public {
        vm.expectRevert(RollupX.InvalidEEZContract.selector);
        new RollupX(address(0));
        RollupX implementation = new RollupX(address(registry));
        address[] memory systems = new address[](1);
        systems[0] = address(verifier);
        bytes32[] memory keys = new bytes32[](1);
        keys[0] = VKEY;
        vm.expectRevert(abi.encodeWithSignature("InvalidInitialization()"));
        implementation.initialize(address(this), 1, systems, keys, profile());
        vm.expectRevert(abi.encodeWithSignature("InvalidInitialization()"));
        manager.initialize(address(this), 1, systems, keys, profile());

        (bool accepted, bytes memory data) =
            address(implementation).call(abi.encodeCall(Rollup.initialize, (address(this), 0, systems, keys)));
        assertFalse(accepted, "context-free initialization must not be available");
        assertEq(data.length, 0, "unknown selector must revert without entering an initializer");
        assertEq(implementation.owner(), address(0));
        assertEq(manager.owner(), address(this));
        assertEq(manager.threshold(), 1);
        assertEq(manager.verificationKey(address(verifier)), VKEY);
    }

    function deployPolicy(address[] memory systems, bytes32[] memory keys) external returns (RollupX) {
        RollupX implementation = new RollupX(address(registry));
        bytes memory init = abi.encodeCall(RollupX.initialize, (address(this), 1, systems, keys, profile()));
        return RollupX(address(new TransparentUpgradeableProxy(address(implementation), address(this), init)));
    }

    function testInvalidInitialPolicyRejectsAtomically() public {
        address[] memory systems = new address[](2);
        systems[0] = address(verifier);
        systems[1] = address(verifier);
        bytes32[] memory keys = new bytes32[](1);
        keys[0] = VKEY;
        vm.expectRevert(RollupX.InvalidConfig.selector);
        this.deployPolicy(systems, keys);
        keys = new bytes32[](2);
        keys[0] = VKEY;
        keys[1] = VKEY;
        vm.expectRevert(abi.encodeWithSelector(RollupX.ProofSystemAlreadyAllowed.selector, address(verifier)));
        this.deployPolicy(systems, keys);
        systems[1] = address(0xBEEF);
        keys[1] = bytes32(0);
        vm.expectRevert(RollupX.InvalidConfig.selector);
        this.deployPolicy(systems, keys);
    }

    function testRegistrationRequiresRegistryAndCurrentOwnerAndIsOneShot() public {
        RollupX fresh = deploy(profile());
        vm.expectRevert(RollupX.NotEEZRegistry.selector);
        fresh.rollupContractRegistered(rid + 1, address(this));
        address nextOwner = address(0xBEEF);
        fresh.transferOwnership(nextOwner);
        uint64 counter = registry.rollupCounter();
        vm.expectRevert(abi.encodeWithSelector(RollupX.UnauthorizedRegistrantAccount.selector, address(this)));
        registry.registerRollup(address(fresh), ROOT);
        assertEq(fresh.rollupId(), 0);
        assertEq(registry.rollupCounter(), counter);
        vm.prank(nextOwner);
        assertEq(registry.registerRollup(address(fresh), ROOT), counter + 1);
        vm.prank(address(registry));
        vm.expectRevert(RollupX.AlreadyRegistered.selector);
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
        vm.expectRevert(abi.encodeWithSelector(RollupX.ProofSystemNotAdded.selector, extra));
        manager.removeProofSystem(extra);
        vm.expectRevert(abi.encodeWithSelector(RollupX.ProofSystemNotAdded.selector, extra));
        manager.updateVerificationKey(extra, VKEY);
        vm.expectRevert(RollupX.InvalidConfig.selector);
        manager.updateVerificationKey(address(verifier), bytes32(0));
        vm.expectRevert(RollupX.InvalidConfig.selector);
        manager.addProofSystem(extra, bytes32(0));
        vm.expectRevert(abi.encodeWithSelector(RollupX.ProofSystemAlreadyAllowed.selector, address(verifier)));
        manager.addProofSystem(address(verifier), replacement);

        bytes[] memory calls = new bytes[](5);
        calls[0] = abi.encodeCall(RollupX.addProofSystem, (extra, VKEY));
        calls[1] = abi.encodeCall(RollupX.removeProofSystem, (address(verifier)));
        calls[2] = abi.encodeCall(RollupX.updateVerificationKey, (address(verifier), replacement));
        calls[3] = abi.encodeCall(RollupX.setThreshold, (0));
        calls[4] = abi.encodeCall(RollupX.setRoot, (replacement));
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
        bytes memory input = abi.encodeCall(RollupX.checkProofSystemsAndGetVkeys, (subset));
        (bool expectedSuccess, bytes memory expected) = address(referenceManager).staticcall(input);
        (bool actualSuccess, bytes memory actual) = address(manager).staticcall(input);
        assertEq(actualSuccess, expectedSuccess);
        assertEq(actual, expected);
    }

    function testRegistrationRejectsDifferentGenesisAtomically() public {
        RollupX fresh = deploy(profile());
        uint64 beforeCounter = registry.rollupCounter();
        vm.expectRevert(RollupX.InvalidOfflineProfile.selector);
        registry.registerRollup(address(fresh), bytes32(uint256(0xdead)));
        assertEq(fresh.rollupId(), 0);
        assertEq(registry.rollupCounter(), beforeCounter);
        assertEq(registry.registerRollup(address(fresh), ROOT), beforeCounter + 1);
        assertEq(fresh.initialBlockHash(), ROOT);
    }

    function testInvalidProfileRejectsAtInitialization() public {
        RollupX.Profile memory p = profile();
        p.updateOffset = p.syncInterval;
        vm.expectRevert(RollupX.InvalidOfflineProfile.selector);
        this.deployExternal(p);
        p = profile();
        p.historyCodeHash = bytes32(uint256(1));
        vm.expectRevert(RollupX.InvalidOfflineProfile.selector);
        this.deployExternal(p);
        p = profile();
        p.ruleTimes[1] = 0;
        vm.expectRevert(RollupX.InvalidOfflineProfile.selector);
        this.deployExternal(p);
    }

    function deployExternal(RollupX.Profile memory p) external returns (RollupX) {
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

    function baseline(RollupX.Profile memory p) internal returns (RollupX previous, address implementation) {
        // Frozen creation bytes exercise the actual previously deployed implementation,
        // including its initializer and storage, without retaining another Solidity implementation.
        bytes memory creation = abi.encodePacked(
            vm.parseBytes(vm.readLine("test/fixtures/rollupx-baseline-init.hex")), abi.encode(address(registry))
        );
        vm.closeFile("test/fixtures/rollupx-baseline-init.hex");
        // SAFETY: creation owns the complete initcode and constructor arguments;
        // CREATE reads exactly its allocated payload, and a failed creation is checked below.
        assembly ("memory-safe") {
            implementation := create(0, add(creation, 32), mload(creation))
        }
        assertTrue(implementation != address(0));
        previous =
            RollupX(address(new TransparentUpgradeableProxy(implementation, address(this), initialization(p, true))));
        registry.registerRollup(address(previous), p.genesisHash);
    }

    function initialization(RollupX.Profile memory p, bool oldSelector) internal view returns (bytes memory) {
        address[] memory systems = new address[](1);
        systems[0] = address(verifier);
        bytes32[] memory keys = new bytes32[](1);
        keys[0] = VKEY;
        bytes4 selector = oldSelector
            ? bytes4(
                keccak256(
                    "initializeOffline(address,uint256,address[],bytes32[],(uint256,uint256,bytes32,uint64,uint64,uint32,uint32,bytes32,bytes32,address,uint64[]))"
                )
            )
            : RollupX.initialize.selector;
        return abi.encodePacked(selector, abi.encode(address(this), uint256(1), systems, keys, p));
    }

    function upgrade(RollupX proxy, address implementation) internal {
        address admin = address(
            uint160(
                uint256(vm.load(address(proxy), 0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103))
            )
        );
        ProxyAdmin(admin).upgradeAndCall(ITransparentUpgradeableProxy(address(proxy)), implementation, "");
    }

    function testFuzzUpgradePreservesContextAndRuleBytes(
        uint64 timestamp,
        uint64 number,
        uint16 age,
        bool actualParent,
        bytes32 root
    )
        public
    {
        RollupX.Profile memory p = profile();
        p.ruleTimes = new uint64[](64);
        for (uint64 i; i < 64; ++i) {
            p.ruleTimes[i] = i * 1000;
        }
        (RollupX previous,) = baseline(p);
        previous.setRoot(root);
        vm.warp(timestamp);
        vm.roll(number);
        uint64 selector = actualParent ? type(uint64).max : number - uint64(bound(age, 0, number));
        if (number > 0) vm.setBlockhash(number - 1, HASH);
        if (selector < number && number - selector <= 256) vm.setBlockhash(selector, HASH);
        bytes memory input = abi.encodeCall(RollupX.getCustomData, (selector));
        (bool expectedSuccess, bytes memory expected) = address(previous).staticcall(input);
        bytes32 expectedRules = previous.l1RulesId(timestamp);
        upgrade(previous, address(new RollupX(address(registry))));
        (bool actualSuccess, bytes memory actual) = address(previous).staticcall(input);
        assertEq(actualSuccess, expectedSuccess);
        assertEq(actual, expected);
        assertEq(previous.l1RulesId(timestamp), expectedRules);
    }

    function testUpgradeAndRollbackPreservePolicyStorageAndExistingProof() public {
        (RollupX previous, address oldImplementation) = baseline(profile());
        address extra = address(0xBEEF);
        previous.addProofSystem(extra, bytes32(uint256(7)));
        previous.updateVerificationKey(extra, bytes32(uint256(8)));
        previous.setThreshold(0);
        previous.transferOwnership(extra);
        bytes32[] memory state = new bytes32[](9);
        for (uint256 slot; slot < 8; ++slot) {
            state[slot] = vm.load(address(previous), bytes32(slot));
        }
        state[8] = vm.load(address(previous), keccak256(abi.encode(uint256(7))));
        bytes memory context = previous.getCustomData(100);
        manager = previous;
        rid = previous.rollupId();
        ProofSystemBatchPerVerificationEntries memory b = batch(100);
        verifier.setExpectedPublicInputsHash(hashForContext(context));
        address nextImplementation = address(new RollupX(address(registry)));
        upgrade(previous, nextImplementation);
        for (uint256 slot; slot < 8; ++slot) {
            assertEq(vm.load(address(previous), bytes32(slot)), state[slot]);
        }
        assertEq(vm.load(address(previous), keccak256(abi.encode(uint256(7)))), state[8]);
        assertEq(previous.EEZContract(), address(registry));
        assertEq(previous.owner(), extra);
        assertEq(previous.threshold(), 0);
        assertEq(previous.verificationKey(extra), bytes32(uint256(8)));
        assertEq(previous.verificationKey(address(verifier)), VKEY);
        assertEq(previous.getCustomData(100), context);
        registry.postAndVerifyBatch(b);
        vm.expectRevert(abi.encodeWithSignature("InvalidInitialization()"));
        previous.initialize(address(this), 1, new address[](0), new bytes32[](0), profile());
        vm.expectRevert(abi.encodeWithSignature("OwnableUnauthorizedAccount(address)", address(this)));
        previous.setThreshold(1);
        vm.prank(extra);
        previous.setThreshold(1);
        upgrade(previous, oldImplementation);
        assertEq(previous.owner(), extra);
        assertEq(previous.threshold(), 1);
        assertEq(previous.verificationKey(extra), bytes32(uint256(8)));
        assertEq(previous.getCustomData(100), context);
        registry.postAndVerifyBatch(b);
    }

    function testUpgradePreservesCoordinateAndRegistryRejections() public {
        (RollupX previous, address oldImplementation) = baseline(profile());
        address nextImplementation = address(new RollupX(address(registry)));
        uint256[4] memory numbers = [uint256(0), 101, uint256(type(uint64).max), uint256(type(uint64).max) + 1];
        uint256[3] memory times = [uint256(0), uint256(type(uint64).max), uint256(type(uint64).max) + 1];
        for (uint256 i; i < numbers.length; ++i) {
            for (uint256 j; j < times.length; ++j) {
                vm.roll(numbers[i]);
                vm.warp(times[j]);
                bytes memory input = abi.encodeCall(RollupX.getCustomData, (type(uint64).max));
                (bool ok, bytes memory data) = address(previous).staticcall(input);
                upgrade(previous, nextImplementation);
                (bool nextOk, bytes memory nextData) = address(previous).staticcall(input);
                assertEq(nextOk, ok);
                assertEq(nextData, data);
                upgrade(previous, oldImplementation);
            }
        }
        vm.roll(101);
        vm.warp(1000);
        vm.mockCall(
            address(registry),
            abi.encodeWithSelector(registry.rollups.selector, previous.rollupId()),
            abi.encode(address(0xBAD), ROOT, uint256(0))
        );
        vm.expectRevert(RollupX.UnregisteredOfflineRollup.selector);
        previous.getCustomData(100);
        upgrade(previous, nextImplementation);
        vm.expectRevert(RollupX.UnregisteredOfflineRollup.selector);
        previous.getCustomData(100);
    }

    function contextGas(RollupX proxy, address implementation, uint64 selector) internal returns (uint256 gas) {
        vm.cool(address(proxy));
        vm.cool(implementation);
        vm.cool(address(registry));
        proxy.getCustomData(selector);
        gas = vm.lastCallGas().gasTotalUsed;
    }

    function testContextGasDoesNotRegressAcrossUpgrade() public {
        for (uint256 epochs = 1; epochs <= 64; epochs *= 2) {
            RollupX.Profile memory p = profile();
            p.ruleTimes = new uint64[](epochs);
            for (uint64 i; i < epochs; ++i) {
                p.ruleTimes[i] = i * 100;
            }
            (RollupX previous, address oldImplementation) = baseline(p);
            address nextImplementation = address(new RollupX(address(registry)));
            uint256 oldActual = contextGas(previous, oldImplementation, type(uint64).max);
            uint256 oldHistorical = contextGas(previous, oldImplementation, 100);
            upgrade(previous, nextImplementation);
            uint256 actual = contextGas(previous, nextImplementation, type(uint64).max);
            uint256 historical = contextGas(previous, nextImplementation, 100);
            emit log_named_uint("rule epochs", epochs);
            emit log_named_uint("baseline actual-parent gas", oldActual);
            emit log_named_uint("RollupX actual-parent gas", actual);
            emit log_named_uint("baseline historical gas", oldHistorical);
            emit log_named_uint("RollupX historical gas", historical);
            assertLe(actual, oldActual);
            assertLe(historical, oldHistorical);
        }
    }

    function testInitializationGasAndProfileStayEquivalent() public {
        (, address oldImplementation) = baseline(profile());
        address nextImplementation = address(new RollupX(address(registry)));
        for (uint256 epochs = 1; epochs <= 64; epochs *= 2) {
            RollupX.Profile memory p = profile();
            p.ruleTimes = new uint64[](epochs);
            for (uint64 i; i < epochs; ++i) {
                p.ruleTimes[i] = i * 100;
            }
            bytes memory oldInit = initialization(p, true);
            bytes memory nextInit = initialization(p, false);
            vm.cool(oldImplementation);
            address previous = address(new TransparentUpgradeableProxy(oldImplementation, address(this), oldInit));
            uint256 oldGas = vm.lastCallGas().gasTotalUsed;
            vm.cool(nextImplementation);
            address next = address(new TransparentUpgradeableProxy(nextImplementation, address(this), nextInit));
            uint256 nextGas = vm.lastCallGas().gasTotalUsed;
            assertGt(oldGas, 0);
            assertEq(RollupX(previous).protocolConfigHash(), RollupX(next).protocolConfigHash());
            assertEq(RollupX(previous).l1RulesId(type(uint64).max), RollupX(next).l1RulesId(type(uint64).max));
            emit log_named_uint("rule epochs", epochs);
            emit log_named_uint("baseline initialization delegatecall gas", oldGas);
            emit log_named_uint("RollupX initialization delegatecall gas", nextGas);
            assertLe(nextGas, oldGas);
        }
    }
}
