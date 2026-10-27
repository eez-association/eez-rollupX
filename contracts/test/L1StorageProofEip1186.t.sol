// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Test} from "forge-std/Test.sol";
import {L1BlockhashContract} from "../src/L2/L1BlockhashContract.sol";
import {L1StorageProof} from "../src/L1StorageProof.sol";

/// @dev Independent trie vectors from eth_getProof after an actual Anvil
///      transaction, rather than a trie assembled by the verifier's test helpers.
contract L1StorageProofEip1186Test is Test {
    L1StorageProof verifier;
    string fixture;
    uint64 number;
    bytes raw;
    address account;
    bytes[] accountProof;

    function setUp() public {
        fixture = vm.readFile(string.concat(vm.projectRoot(), "/test/fixtures/l1-storage-eip1186.json"));
        number = uint64(vm.parseJsonUint(fixture, ".number"));
        raw = vm.parseJsonBytes(fixture, ".rawHeader");
        account = vm.parseJsonAddress(fixture, ".present.address");
        accountProof = vm.parseJsonBytesArray(fixture, ".present.accountProof");
        verifier = new L1StorageProof(0, 0, 0, 0, type(uint64).max);
        vm.etch(verifier.HISTORY(), type(L1BlockhashContract).runtimeCode);
        vm.store(verifier.HISTORY(), bytes32(uint256(1)), bytes32(uint256(number)));
        bytes32 blockHash = vm.parseJsonBytes32(fixture, ".blockHash");
        assertEq(keccak256(raw), blockHash);
        vm.store(verifier.HISTORY(), keccak256(abi.encode(number, uint256(0))), blockHash);
    }

    function testRealTrieValueAndStorageAbsence() public view {
        bytes[] memory proof = vm.parseJsonBytesArray(fixture, ".present.storageProof[0].proof");
        assertGt(accountProof.length, 1);
        assertGt(proof.length, 1);
        assertEq(verifier.readStorage(number, account, bytes32(uint256(7)), raw, accountProof, proof), 45);
        proof = vm.parseJsonBytesArray(fixture, ".present.storageProof[1].proof");
        assertEq(verifier.readStorage(number, account, bytes32(uint256(999)), raw, accountProof, proof), 0);
    }

    function testRealTrieAccountAbsenceAndIncompleteProofRejection() public {
        address absent = vm.parseJsonAddress(fixture, ".absent.address");
        bytes[] memory proof = vm.parseJsonBytesArray(fixture, ".absent.accountProof");
        assertEq(verifier.readStorage(number, absent, bytes32(uint256(7)), raw, proof, new bytes[](0)), 0);
        bytes[] memory storagePath = vm.parseJsonBytesArray(fixture, ".present.storageProof[0].proof");
        bytes[] memory truncated = new bytes[](storagePath.length - 1);
        for (uint256 i; i < truncated.length; ++i) {
            truncated[i] = storagePath[i];
        }
        vm.expectRevert(L1StorageProof.InvalidProof.selector);
        verifier.readStorage(number, account, bytes32(uint256(7)), raw, accountProof, truncated);
        storagePath[storagePath.length - 1][0] ^= 0x01;
        vm.expectRevert(L1StorageProof.InvalidProof.selector);
        verifier.readStorage(number, account, bytes32(uint256(7)), raw, accountProof, storagePath);
    }
}
