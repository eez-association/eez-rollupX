// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

import {Test} from "forge-std/Test.sol";
import {RLP} from "@openzeppelin/contracts/utils/RLP.sol";
import {L1BlockhashContract} from "../src/L2/L1BlockhashContract.sol";
import {L1StorageProof} from "../src/L1StorageProof.sol";

contract L1StorageProofTest is Test {
    address constant ACCOUNT = address(0x1234);
    bytes32 constant SLOT = bytes32(uint256(7));
    uint64 constant NUMBER = 99;
    L1StorageProof verifier;

    function setUp() public {
        verifier = new L1StorageProof(
            type(uint64).max, type(uint64).max, type(uint64).max, type(uint64).max, type(uint64).max
        );
        vm.etch(verifier.HISTORY(), type(L1BlockhashContract).runtimeCode);
        vm.store(verifier.HISTORY(), bytes32(uint256(1)), bytes32(uint256(NUMBER)));
    }

    function record(bytes memory raw) internal {
        vm.store(verifier.HISTORY(), keccak256(abi.encode(NUMBER, uint256(0))), keccak256(raw));
    }

    function header(bytes32 root, uint64 number, uint64 timestamp, uint256 count) internal pure returns (bytes memory) {
        bytes[] memory fields = new bytes[](count);
        fields[0] = RLP.encode(bytes32(uint256(0x11)));
        fields[1] = RLP.encode(bytes32(uint256(0x22)));
        fields[2] = RLP.encode(address(0));
        fields[3] = RLP.encode(root);
        fields[4] = RLP.encode(bytes32(uint256(0x33)));
        fields[5] = RLP.encode(bytes32(uint256(0x44)));
        fields[6] = RLP.encode(new bytes(256));
        fields[7] = RLP.encode(uint256(0));
        fields[8] = RLP.encode(uint256(number));
        fields[9] = RLP.encode(uint256(30_000_000));
        fields[10] = RLP.encode(uint256(0));
        fields[11] = RLP.encode(uint256(timestamp));
        fields[12] = RLP.encode(bytes(""));
        fields[13] = RLP.encode(bytes32(uint256(0x55)));
        fields[14] = RLP.encode(new bytes(8));
        if (count > 15) fields[15] = RLP.encode(uint256(1));
        if (count > 16) fields[16] = RLP.encode(bytes32(uint256(0x66)));
        if (count > 17) {
            fields[17] = RLP.encode(uint256(0));
            fields[18] = RLP.encode(uint256(0));
            fields[19] = RLP.encode(bytes32(uint256(0x77)));
        }
        if (count > 20) fields[20] = RLP.encode(bytes32(uint256(0x88)));
        return RLP.encode(fields);
    }

    function leaf(bytes memory path, bytes memory value) internal pure returns (bytes memory) {
        bytes[] memory pair = new bytes[](2);
        pair[0] = RLP.encode(path);
        pair[1] = RLP.encode(value);
        return RLP.encode(pair);
    }

    function accountNode(bytes32 storageRoot) internal pure returns (bytes memory) {
        bytes[] memory fields = new bytes[](4);
        fields[0] = RLP.encode(uint256(1));
        fields[1] = RLP.encode(uint256(0));
        fields[2] = RLP.encode(storageRoot);
        fields[3] = RLP.encode(keccak256(bytes("")));
        return leaf(abi.encodePacked(bytes1(0x20), keccak256(abi.encodePacked(ACCOUNT))), RLP.encode(fields));
    }

    function one(bytes memory node) internal pure returns (bytes[] memory proof) {
        proof = new bytes[](1);
        proof[0] = node;
    }

    function fixture(uint256 value)
        internal
        returns (bytes memory h, bytes[] memory account, bytes[] memory storagePath)
    {
        bytes memory storageNode =
            leaf(abi.encodePacked(bytes1(0x20), keccak256(abi.encodePacked(SLOT))), RLP.encode(value));
        account = one(accountNode(keccak256(storageNode)));
        storagePath = one(storageNode);
        h = header(keccak256(account[0]), NUMBER, 1000, 15);
        record(h);
    }

    function testStorageValueAndCanonicalAbsenceBindAddressAndSlot() public {
        (bytes memory h, bytes[] memory account, bytes[] memory storagePath) = fixture(45);
        assertEq(verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, account, storagePath), 45);
        // A divergent authenticated leaf proves absence, not the original value.
        assertEq(verifier.readStorage(NUMBER, ACCOUNT, bytes32(uint256(8)), h, account, storagePath), 0);
        assertEq(verifier.readStorage(NUMBER, address(0x5678), SLOT, h, account, new bytes[](0)), 0);
        vm.expectRevert(L1StorageProof.InvalidProof.selector);
        verifier.readStorage(NUMBER, address(0x5678), SLOT, h, account, storagePath);
        vm.expectRevert(L1StorageProof.InvalidProof.selector);
        verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, new bytes[](0), storagePath);
        vm.expectRevert(L1StorageProof.InvalidProof.selector);
        verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, account, new bytes[](0));
    }

    function testEmptyStorageAndEmptyAccountTriesAreAuthenticatedAbsence() public {
        bytes[] memory account = one(accountNode(keccak256(hex"80")));
        bytes memory h = header(keccak256(account[0]), NUMBER, 1000, 15);
        record(h);
        assertEq(verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, account, new bytes[](0)), 0);
        h = header(keccak256(hex"80"), NUMBER, 1000, 15);
        record(h);
        assertEq(verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, new bytes[](0), new bytes[](0)), 0);
        vm.expectRevert(L1StorageProof.InvalidProof.selector);
        verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, account, new bytes[](0));
    }

    function testHeaderHashNumberAndMissingHistoryRejectSubstitution() public {
        (bytes memory h, bytes[] memory account, bytes[] memory storagePath) = fixture(45);
        bytes memory other = header(keccak256(account[0]), NUMBER + 1, 1000, 15);
        vm.expectRevert(L1StorageProof.InvalidHeader.selector);
        verifier.readStorage(NUMBER, ACCOUNT, SLOT, other, account, storagePath);
        record(other);
        vm.expectRevert(L1StorageProof.InvalidHeader.selector);
        verifier.readStorage(NUMBER, ACCOUNT, SLOT, other, account, storagePath);
        assertEq(L1BlockhashContract(verifier.HISTORY()).getL1BlockHash(NUMBER + 1), bytes32(0));
        vm.expectRevert(L1StorageProof.InvalidHeader.selector);
        verifier.readStorage(NUMBER + 1, ACCOUNT, SLOT, h, account, storagePath);
        record(bytes.concat(h, hex"00"));
        vm.expectRevert();
        verifier.readStorage(NUMBER, ACCOUNT, SLOT, bytes.concat(h, hex"00"), account, storagePath);
    }

    function testProofNodesCannotBeTruncatedReplacedOrAppended() public {
        (bytes memory h, bytes[] memory account, bytes[] memory storagePath) = fixture(45);
        bytes[] memory extra = new bytes[](2);
        extra[0] = storagePath[0];
        extra[1] = storagePath[0];
        vm.expectRevert(L1StorageProof.InvalidProof.selector);
        verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, account, extra);
        bytes memory bad = storagePath[0];
        bad[bad.length - 1] ^= 0x01;
        vm.expectRevert(L1StorageProof.InvalidProof.selector);
        verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, account, one(bad));
    }

    function testNonCanonicalScalarZeroLeafAndMalformedCompactPathReject() public {
        bytes32 key = keccak256(abi.encodePacked(SLOT));
        for (uint256 i; i < 4; ++i) {
            bytes memory path = abi.encodePacked(bytes1(0x20), key);
            bytes memory scalar = hex"82002d"; // leading zero, not the integer 45 encoding
            if (i == 1) scalar = hex"80"; // zero belongs in absence, never a storage leaf
            if (i == 2) path[0] = 0x21; // even compact path has nonzero padding
            if (i == 3) path[0] = 0x40; // invalid compact node type
            bytes memory node = leaf(path, scalar);
            bytes[] memory account = one(accountNode(keccak256(node)));
            bytes memory h = header(keccak256(account[0]), NUMBER, 1000, 15);
            record(h);
            vm.expectRevert();
            verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, account, one(node));
        }
    }

    function testForkFieldCountsAndUnsupportedLayoutsAreExact() public {
        verifier = new L1StorageProof(90, 100, 200, 300, 400);
        (, bytes[] memory account, bytes[] memory storagePath) = fixture(45);
        uint256[4] memory counts = [uint256(16), 17, 20, 21];
        for (uint256 i; i < counts.length; ++i) {
            uint64 timestamp = uint64(i * 100 + 50);
            bytes memory h = header(keccak256(account[0]), NUMBER, timestamp, counts[i]);
            record(h);
            assertEq(verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, account, storagePath), 45);
            bytes memory wrong = header(keccak256(account[0]), NUMBER, timestamp, i == 0 ? 17 : 16);
            record(wrong);
            vm.expectRevert(L1StorageProof.InvalidHeader.selector);
            verifier.readStorage(NUMBER, ACCOUNT, SLOT, wrong, account, storagePath);
        }
        bytes memory future = header(keccak256(account[0]), NUMBER, 400, 21);
        record(future);
        vm.expectRevert(L1StorageProof.InvalidHeader.selector);
        verifier.readStorage(NUMBER, ACCOUNT, SLOT, future, account, storagePath);
    }

    function testBranchAndExtensionPathsIncludingInlineChildrenAndProvenEmptyBranch() public {
        bytes32 key = keccak256(abi.encodePacked(SLOT));
        // Shared 62-nibble extension -> branch -> one-nibble inline leaves.
        bytes memory path = new bytes(32);
        for (uint256 i; i < 31; ++i) {
            path[i + 1] = key[i];
        }
        bytes[] memory branch = new bytes[](17);
        for (uint256 i; i < 17; ++i) {
            branch[i] = hex"80";
        }
        uint8 index = uint8(key[31]) >> 4;
        branch[index] = leaf(abi.encodePacked(bytes1(uint8(0x30) | uint8(key[31]) & 15)), RLP.encode(uint256(45)));
        branch[(index + 1) % 16] = leaf(hex"30", RLP.encode(uint256(46)));
        bytes memory encodedBranch = RLP.encode(branch);
        assertLt(encodedBranch.length, 32);
        bytes[] memory extension = new bytes[](2);
        extension[0] = RLP.encode(path);
        extension[1] = encodedBranch;
        bytes memory root = RLP.encode(extension);
        bytes[] memory account = one(accountNode(keccak256(root)));
        bytes memory h = header(keccak256(account[0]), NUMBER, 1000, 15);
        record(h);
        assertEq(verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, account, one(root)), 45);
        // A root branch whose requested child is empty is a complete absence proof.
        branch[index] = hex"80";
        root = RLP.encode(branch);
        account = one(accountNode(keccak256(root)));
        h = header(keccak256(account[0]), NUMBER, 1000, 15);
        record(h);
        assertEq(verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, account, one(root)), 0);
    }

    function testResourceBoundsRejectBeforeTrieTraversal() public {
        (bytes memory h, bytes[] memory account, bytes[] memory storagePath) = fixture(45);
        vm.expectRevert(L1StorageProof.ProofLimit.selector);
        verifier.readStorage(NUMBER, ACCOUNT, SLOT, new bytes(2049), account, storagePath);
        vm.expectRevert(L1StorageProof.ProofLimit.selector);
        verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, new bytes[](66), storagePath);
        account[0] = new bytes(1025);
        vm.expectRevert(L1StorageProof.ProofLimit.selector);
        verifier.readStorage(NUMBER, ACCOUNT, SLOT, h, account, storagePath);
    }
}
