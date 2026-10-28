// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

/// @notice Stores L1 block hashes supplied by the L2 execution protocol.
/// @dev The protocol authenticates L1 headers and controls import timing.
///      This contract restricts writes to the system sender and enforces
///      strictly increasing block numbers. History starts empty. Genesis installs
///      this implementation behind EEZProxy; upgrades must preserve the mapping
///      at slot zero, the uint64 endpoint at slot one and the protocol's rules.
contract L1BlockhashContract {
    address public constant SYSTEM_ADDRESS = 0xffffFFFfFFffffffffffffffFfFFFfffFFFfFFfE;

    struct L1Block {
        uint64 number;
        bytes32 blockHash;
    }

    mapping(uint64 number => bytes32 blockHash) private hashes;
    uint64 private latestImportedBlockNumber;

    error UnauthorizedImport(address caller);
    error EmptyImport();
    error InvalidBlockHash(uint64 number);
    error BlockNumberNotIncreasing(uint64 previousImportedBlockNumber, uint64 supplied);

    /// @notice Returns the stored L1 hash, or zero if unavailable.
    /// @dev Unavailable includes skipped numbers and numbers not yet imported.
    function getL1BlockHash(uint64 number) external view returns (bytes32) {
        return hashes[number];
    }

    /// @notice Returns the highest imported L1 block number and its hash.
    /// @dev Returns (0, 0) before the first import.
    ///      Earlier block numbers are not necessarily present.
    function getLatestImportedL1Block() external view returns (uint64 number, bytes32 blockHash) {
        number = latestImportedBlockNumber;
        blockHash = hashes[number];
    }

    /// @notice Appends authenticated L1 block-number/hash pairs.
    /// @param blocks Nonempty list in strictly increasing block-number order.
    /// @dev Each number must exceed the latest stored number, initially zero.
    ///      Gaps are allowed and cannot be backfilled.
    ///      Zero hashes are forbidden because zero denotes absence.
    ///
    ///      The protocol enforces authenticity, timing and execution limits.
    ///      No external calls or logs occur. Failure reverts the entire import.
    function importL1Blocks(L1Block[] calldata blocks) external {
        if (msg.sender != SYSTEM_ADDRESS) {
            revert UnauthorizedImport(msg.sender);
        }

        uint256 blocksLen = blocks.length;
        if (blocksLen == 0) revert EmptyImport();

        uint64 previousImportedBlockNumber = latestImportedBlockNumber;

        for (uint256 i; i < blocksLen; ++i) {
            L1Block calldata currentL1Block = blocks[i];

            if (currentL1Block.number <= previousImportedBlockNumber) {
                revert BlockNumberNotIncreasing(previousImportedBlockNumber, currentL1Block.number);
            }

            if (currentL1Block.blockHash == bytes32(0)) {
                revert InvalidBlockHash(currentL1Block.number);
            }

            hashes[currentL1Block.number] = currentL1Block.blockHash;
            previousImportedBlockNumber = currentL1Block.number;
        }

        latestImportedBlockNumber = previousImportedBlockNumber;
    }
}
