// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Test} from "forge-std/Test.sol";
import {EEZ} from "eez-core-protocol/src/EEZ.sol";
import {Rollup} from "eez-core-protocol/src/rollupContract/Rollup.sol";
import {
    ExpectedRootPerRollup,
    ProofSystemBatchPerVerificationEntries,
    RollupIdWithProofSystems
} from "eez-core-protocol/src/interfaces/IEEZ.sol";
import {ECDSAProofSystem} from "../src/ECDSAProofSystem.sol";
import {RollupDeployment} from "./utils/RollupDeployment.sol";

/// @dev Tests the real Core/ECDSA boundary; complete execution and DA acceptance
///      are covered by the signer and source-based process suites.
contract RollupContextTest is Test {
    uint256 private constant SIGNER_KEY = 0xA11CE;
    bytes32 private constant ROOT = bytes32(uint256(0x123));
    bytes32 private constant L1_HASH = bytes32(uint256(0xabc));
    EEZ private registry;
    Rollup private manager;
    ECDSAProofSystem private verifier;
    uint64 private rid;

    function setUp() public {
        vm.roll(101);
        vm.warp(1000);
        vm.setBlockhash(100, L1_HASH);
        registry = RollupDeployment.deployEEZ(address(0xDEAD), address(this));
        verifier = new ECDSAProofSystem(vm.addr(SIGNER_KEY));
        address[] memory systems = new address[](1);
        systems[0] = address(verifier);
        bytes32[] memory keys = new bytes32[](1);
        keys[0] = verificationKey(keccak256("network profile"));
        manager = RollupDeployment.deploy(address(registry), address(this), 1, systems, keys);
        rid = registry.registerRollup(address(manager), ROOT);
    }

    function verificationKey(bytes32 profile) private view returns (bytes32) {
        return keccak256(
            abi.encode(
                keccak256("EEZ_ROLLUPX_VERIFICATION_KEY_V1"),
                profile,
                address(registry),
                address(verifier),
                vm.addr(SIGNER_KEY)
            )
        );
    }

    function testCoreContextEncodingsAndHistoricalAvailability() public {
        assertEq(manager.getCustomData(type(uint64).max), abi.encode(uint256(1000), L1_HASH));
        assertEq(manager.getCustomData(100), abi.encode(uint256(0), L1_HASH));
        vm.roll(356);
        vm.setBlockhash(100, L1_HASH);
        assertEq(manager.getCustomData(100), abi.encode(uint256(0), L1_HASH));
        vm.roll(357);
        vm.expectRevert(abi.encodeWithSelector(Rollup.BlockHashUnavailable.selector, uint64(100)));
        manager.getCustomData(100);
    }

    function testFeeEstimatorFixtureMatchesCoreRuntime() public view {
        bytes memory fixture =
            vm.parseBytes(vm.readLine("../crates/eez-composer/src/composer/estimate/testdata/rollup.hex"));
        assertEq(vm.getDeployedCode("Rollup.sol:Rollup"), fixture);
    }

    function testActualParentProofBindsInclusionTimeAndHash() public {
        ProofSystemBatchPerVerificationEntries memory b = signedBatch(type(uint64).max);
        registry.postAndVerifyBatch(b);
        vm.warp(1001);
        vm.expectRevert(EEZ.InvalidProof.selector);
        registry.postAndVerifyBatch(b);
        vm.warp(1000);
        vm.setBlockhash(100, keccak256("different parent"));
        vm.expectRevert(EEZ.InvalidProof.selector);
        registry.postAndVerifyBatch(b);
    }

    function testHistoricalProofSurvivesTimeChangeButNotKeyRotation() public {
        ProofSystemBatchPerVerificationEntries memory b = signedBatch(100);
        registry.postAndVerifyBatch(b);
        vm.warp(2000);
        registry.postAndVerifyBatch(b);
        manager.updateVerificationKey(address(verifier), verificationKey(keccak256("upgraded network profile")));
        vm.expectRevert(EEZ.InvalidProof.selector);
        registry.postAndVerifyBatch(b);
        registry.postAndVerifyBatch(signedBatch(100));
    }

    function testHistoricalContextDoesNotExpireWhenAcceptedRootAdvances() public {
        bytes memory before = manager.getCustomData(100);
        ProofSystemBatchPerVerificationEntries memory b = signedBatch(100);
        bytes32 laterRoot = keccak256("later accepted root");
        manager.setRoot(laterRoot);
        assertEq(manager.getCustomData(100), before);
        // The optional live-root assertion changes, but the existing proof does not.
        b.expectedRootPerRollup[0].root = laterRoot;
        registry.postAndVerifyBatch(b);
    }

    function testCoreRejectsWrongUnsignedRootAssertion() public {
        ProofSystemBatchPerVerificationEntries memory b = signedBatch(100);
        b.expectedRootPerRollup[0].root = keccak256("wrong root");
        vm.expectRevert(abi.encodeWithSelector(EEZ.ExpectedRootMismatch.selector, rid));
        registry.postAndVerifyBatch(b);
        b.expectedRootPerRollup[0].root = ROOT;
        registry.postAndVerifyBatch(b);
    }

    function signedBatch(uint64 selector) private view returns (ProofSystemBatchPerVerificationEntries memory b) {
        b.blockNumber = selector;
        b.expectedRootPerRollup = new ExpectedRootPerRollup[](1);
        (, bytes32 root,) = registry.rollups(rid);
        b.expectedRootPerRollup[0] = ExpectedRootPerRollup(rid, root);
        b.proofSystems = new address[](1);
        b.proofSystems[0] = address(verifier);
        b.rollupIdsWithProofSystems = new RollupIdWithProofSystems[](1);
        b.rollupIdsWithProofSystems[0] = RollupIdWithProofSystems(rid, new uint64[](1));
        bytes32[] memory empty = new bytes32[](0);
        bytes32[] memory custom = new bytes32[](1);
        custom[0] = keccak256(abi.encode(rid, manager.getCustomData(selector)));
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
        bytes32 accumulator = keccak256(abi.encode(bytes32(0), rid, manager.verificationKey(address(verifier))));
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(SIGNER_KEY, keccak256(abi.encodePacked(shared, accumulator)));
        b.proofs = new bytes[](1);
        b.proofs[0] = abi.encodePacked(r, s, v);
    }
}
