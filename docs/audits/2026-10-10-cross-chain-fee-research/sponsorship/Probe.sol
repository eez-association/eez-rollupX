// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

// The runner copies this interface from the pinned protocol, without edits.
import {ProofSystemBatchPerVerificationEntries, RollupIdWithProofSystems} from "./IEEZ.sol";

interface Registry {
    function postAndVerifyBatch(ProofSystemBatchPerVerificationEntries calldata batch) external;
    function lastVerifiedBlock(uint64 rollupId) external view returns (uint256);
}

/// @notice Local fee-mechanics fixture, not a production rollup or proof verifier.
/// @dev Its proof is a public-input hash equality check, not an attestation of L2
/// execution. It binds the real blob hash and target context to catch accidental
/// changes while exercising the pinned registry's actual post transaction.
contract Probe {
    address public immutable registry;
    address public immutable owner;
    uint64 public rollupId;
    uint256 public calls;
    bytes32 internal constant VK = bytes32(uint256(0x100));

    constructor(address registry_) {
        registry = registry_;
        owner = msg.sender;
    }

    function rollupContractRegistered(uint64 id, address registrant) external {
        require(msg.sender == registry && registrant == owner && rollupId == 0);
        rollupId = id;
    }

    function checkProofSystemsAndGetVkeys(address[] calldata systems) external view returns (bytes32[] memory keys) {
        require(systems.length == 1 && systems[0] == address(this));
        keys = new bytes32[](1);
        keys[0] = VK;
    }

    function getCustomData(uint64 number) external view returns (bytes memory) {
        require(number == type(uint64).max);
        return abi.encode(block.timestamp, blockhash(block.number - 1));
    }

    function verify(bytes calldata proof, bytes32 actual) external pure returns (bool) {
        return proof.length == 32 && abi.decode(proof, (bytes32)) == actual;
    }

    function postData(bytes32 blobHash, uint64 timestamp, bytes32 parentHash) external view returns (bytes memory) {
        ProofSystemBatchPerVerificationEntries memory batch;
        batch.proofSystems = new address[](1);
        batch.proofSystems[0] = address(this);
        batch.rollupIdsWithProofSystems = new RollupIdWithProofSystems[](1);
        batch.rollupIdsWithProofSystems[0].rollupId = rollupId;
        batch.rollupIdsWithProofSystems[0].proofSystemIndexes = new uint64[](1);
        batch.blobIndices = new uint256[](1);
        batch.blockNumber = type(uint64).max;

        bytes32[] memory empty = new bytes32[](0);
        bytes32[] memory blobs = new bytes32[](1);
        blobs[0] = blobHash;
        bytes32[] memory custom = new bytes32[](1);
        custom[0] = keccak256(abi.encode(rollupId, abi.encode(uint256(timestamp), parentHash)));
        bytes32 shared = keccak256(
            abi.encodePacked(
                abi.encode(empty),
                abi.encode(empty),
                abi.encode(blobs),
                keccak256(bytes("")),
                abi.encode(custom),
                address(0)
            )
        );
        bytes32 accumulated = keccak256(abi.encode(bytes32(0), rollupId, VK));
        batch.proofs = new bytes[](1);
        batch.proofs[0] = abi.encode(keccak256(abi.encodePacked(shared, accumulated)));
        return abi.encodeCall(Registry.postAndVerifyBatch, (batch));
    }

    /// @dev Synthetic L1 companions depend on the post's same-block marker.
    /// They are not executions of actual cross-chain user requests.
    function consume() external {
        require(Registry(registry).lastVerifiedBlock(rollupId) == block.number);
        calls++;
    }
}
