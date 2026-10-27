// SPDX-License-Identifier: MIT
pragma solidity 0.8.34;

/// @notice Permanent L1 number-to-hash history installed by the L2 execution protocol.
/// @dev Genesis installs runtime and slots 0..6 directly; there is no public initializer.
///      The execution engine authenticates the recorded native transaction and its ancestry. This
///      contract enforces caller, schedule and append-only storage, not L1 consensus.
contract L1BlockhashContract {
    address public constant SYSTEM_ADDRESS = 0xffffFFFfFFffffffffffffffFfFFFfffFFFfFFfE;

    // Full-width scalar slots deliberately make the genesis/executor layout explicit.
    mapping(uint64 number => bytes32 hash) internal hashes; // slot 0
    uint256 internal firstNumber; // slot 1
    uint256 internal endpoint; // slot 2
    uint256 internal firstCutoff; // slot 3, immutable genesis phase
    uint256 internal interval; // slot 4
    uint256 internal updateDelay; // slot 5
    uint256 internal maxImportHeaders; // slot 6

    error UnauthorizedHistoryImport();
    error InvalidHistoryState();
    error InvalidHistoryPosition();
    error InvalidHistoryImportLength();
    error InvalidHistoryHash();
    error L1BlockNotImported(uint64 number);

    /// @notice Exact hash for an imported block. Absence is never represented by zero.
    function getL1BlockHash(uint64 number) external view returns (bytes32) {
        if (number < firstNumber || number > endpoint || hashes[number] == bytes32(0)) {
            revert L1BlockNotImported(number);
        }
        return hashes[number];
    }

    /// @notice The currently selected origin, also the last imported block.
    function current() external view returns (uint64 number, bytes32 hash) {
        if (endpoint > type(uint64).max || firstNumber > endpoint || hashes[uint64(endpoint)] == bytes32(0)) {
            revert InvalidHistoryState();
        }
        return (uint64(endpoint), hashes[uint64(endpoint)]);
    }

    /// @notice Append consecutive hashes at an eligible delayed cutoff.
    /// @dev The native execution engine admits at most one HistoryUpdate, at
    ///      transaction zero, and authenticates its hashes, chain ID and PREVRANDAO.
    ///      Omitted updates leave all storage unchanged. This call is fee-free
    ///      but metered, makes no external calls, returns no data and emits no logs.
    function importHistory(uint64 cutoff, bytes32[] calldata newHashes) external {
        if (msg.sender != SYSTEM_ADDRESS) revert UnauthorizedHistoryImport();
        if (
            interval == 0 || interval > type(uint64).max || updateDelay == 0 || updateDelay > interval
                || maxImportHeaders == 0 || firstNumber > endpoint || endpoint > type(uint64).max
                || hashes[uint64(endpoint)] == bytes32(0)
        ) revert InvalidHistoryState();
        if (
            cutoff < firstCutoff || (cutoff - firstCutoff) % interval != 0
                || uint256(cutoff) + updateDelay != block.timestamp
        ) {
            revert InvalidHistoryPosition();
        }
        if (newHashes.length == 0 || newHashes.length > maxImportHeaders) revert InvalidHistoryImportLength();
        uint256 newEndpoint = endpoint + newHashes.length;
        if (newEndpoint > type(uint64).max) revert InvalidHistoryState();
        for (uint256 i; i < newHashes.length; ++i) {
            uint64 number = uint64(endpoint + i + 1);
            // A conflicting existing record is invalid even if its hash agrees:
            // append-only numbering never revisits any installed record.
            if (newHashes[i] == bytes32(0) || hashes[number] != bytes32(0)) revert InvalidHistoryHash();
            hashes[number] = newHashes[i];
        }
        endpoint = newEndpoint;
    }
}
