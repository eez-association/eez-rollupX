// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;
import {Test} from "forge-std/Test.sol";
import {EEZ} from "eez-core-protocol/src/EEZ.sol";
import {Rollup} from "eez-core-protocol/src/rollupContract/Rollup.sol";
import {ProofSystemBatchPerVerificationEntries} from "eez-core-protocol/src/interfaces/IEEZ.sol";
import {ECDSAProofSystem} from "rollupx/ECDSAProofSystem.sol";

contract RegistryOracle is EEZ {
    constructor() EEZ(address(0x1234)) {}

    function setManager(uint64 id, address manager) external {
        rollups[id].rollupContract = manager;
    }

    function check(ProofSystemBatchPerVerificationEntries calldata batch, bytes32 key) external view {
        bytes32[][] memory keys = new bytes32[][](1);
        keys[0] = new bytes32[](1);
        keys[0][0] = key;
        _verifyProofSystemBatch(batch, keys);
    }
}

contract FixtureOracleTest is Test {
    function testBidirectional() public {
        checkFixture("native-offline-bidirectional");
    }

    function testLookup() public {
        checkFixture("native-osaka-lookup");
    }

    function testChangingStatic() public {
        checkFixture("native-osaka-changing-static");
    }

    function checkFixture(string memory name) internal {
        string memory input = vm.readFile(string.concat("../rollup-core-fixture-oracle/", name, ".json"));
        bytes memory calldataBytes = vm.parseJsonBytes(input, ".calldata");
        bytes memory args = new bytes(calldataBytes.length - 4);
        for (uint256 i; i < args.length; ++i) {
            args[i] = calldataBytes[i + 4];
        }
        ProofSystemBatchPerVerificationEntries memory batch = abi.decode(args, (ProofSystemBatchPerVerificationEntries));
        RegistryOracle registry = new RegistryOracle();
        Rollup manager = new Rollup(address(registry));
        assertEq(batch.rollupIdsWithProofSystems.length, 1);
        registry.setManager(batch.rollupIdsWithProofSystems[0].rollupId, address(manager));
        assertEq(batch.proofSystems.length, 1);
        bytes32 key = vm.parseJsonBytes32(input, ".vkey");
        assertEq(keccak256(vm.parseJsonBytes(input, ".profile_preimage")), vm.parseJsonBytes32(input, ".profile_hash"));
        assertEq(
            key,
            keccak256(
                abi.encode(
                    keccak256("EEZ_ROLLUPX_VERIFICATION_KEY_V1"),
                    vm.parseJsonBytes32(input, ".profile_hash"),
                    vm.parseJsonAddress(input, ".registry"),
                    vm.parseJsonAddress(input, ".proof_system"),
                    vm.parseJsonAddress(input, ".attester")
                )
            )
        );
        ECDSAProofSystem verifier = new ECDSAProofSystem(vm.parseJsonAddress(input, ".attester"));
        vm.etch(batch.proofSystems[0], address(verifier).code);
        bytes memory signature = vm.parseJsonBytes(input, ".signature");
        assertTrue(verifier.verify(signature, vm.parseJsonBytes32(input, ".digest")));
        batch.proofs = new bytes[](1);
        batch.proofs[0] = signature;
        vm.roll(vm.parseJsonUint(input, ".parent_number") + 1);
        vm.warp(vm.parseJsonUint(input, ".target_timestamp"));
        vm.setBlockhash(block.number - 1, vm.parseJsonBytes32(input, ".parent_hash"));
        vm.blobhashes(vm.parseJsonBytes32Array(input, ".blob_hashes"));
        registry.check(batch, key);
        vm.expectRevert(EEZ.InvalidProof.selector);
        registry.check(batch, bytes32(uint256(key) ^ 1));
        vm.warp(block.timestamp + 1);
        vm.expectRevert(EEZ.InvalidProof.selector);
        registry.check(batch, key);
    }
}
